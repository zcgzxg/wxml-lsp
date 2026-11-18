//! `state.json`, transaction journals and durability helpers.
//!
//! The state file records the active native version and the last successful
//! transaction. Journals are limited to `staged`, `committed` and `activated`
//! so a crash can be classified without guessing. All file replacements use a
//! temporary sibling plus `rename`; directories are fsynced where practical.
//!
//! Power-loss durability is not proven by these tests. `fsync` ordering covers
//! process crashes on a normal filesystem, but a sudden power cut may still
//! lose a directory entry depending on the filesystem and storage device. See
//! `docs/reference/tsgo-delivery-contract.md` §6.3 and §10.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{DeliveryError, Result};
use crate::install::InstallRoot;
use crate::manifest::{atomic_write, now_rfc3339, read_limited, set_mode};
use crate::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_NAME, ENGINE_SOURCE_COMMIT,
    ENGINE_VERSION, ExpectedManifestV2, ManifestSchema, ManifestV2,
    SUPPORTED_NATIVE_PROTOCOL_VERSIONS, read_manifest_schema, read_manifest_v2,
};
use crate::paths::{validate_relative_path, validate_version};

/// State file and journal schema version.
pub const STATE_SCHEMA_VERSION: u32 = 2;
/// Maximum accepted `state.json` or journal size.
pub const MAX_STATE_BYTES: u64 = 64 * 1024;
/// Directory that holds transaction journals.
pub const JOURNAL_DIR: &str = "txn";
/// State file name.
pub const STATE_FILE: &str = "state.json";

static TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Engine identity copied into `state.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateEngine {
    /// Engine name.
    pub name: String,
    /// Exact engine version.
    pub version: String,
    /// Engine source commit.
    pub source_commit: String,
    /// Rust-to-engine protocol version.
    pub protocol_version: u64,
    /// AST/API schema version.
    pub api_schema_version: u64,
    /// AST header size.
    pub ast_header_bytes: u64,
}

impl StateEngine {
    /// Copies the identity fields from a native manifest.
    #[must_use]
    pub fn from_manifest(manifest: &ManifestV2) -> Self {
        Self {
            name: manifest.engine.name.clone(),
            version: manifest.engine.version.clone(),
            source_commit: manifest.engine.source_commit.clone(),
            protocol_version: manifest.protocol_version,
            api_schema_version: manifest.engine.api_schema_version,
            ast_header_bytes: manifest.engine.ast_header_bytes,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.name != ENGINE_NAME
            || self.version != ENGINE_VERSION
            || self.source_commit != ENGINE_SOURCE_COMMIT
            || self.api_schema_version != ENGINE_API_SCHEMA_VERSION
            || self.ast_header_bytes != ENGINE_AST_HEADER_BYTES
            || !SUPPORTED_NATIVE_PROTOCOL_VERSIONS.contains(&self.protocol_version)
        {
            return Err(DeliveryError::InstallState(
                "state.json engine identity does not match the frozen engine".to_owned(),
            ));
        }
        Ok(())
    }

    fn matches_manifest(&self, manifest: &ManifestV2) -> bool {
        *self == Self::from_manifest(manifest)
    }
}

/// Transaction operation recorded in journals and state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionOp {
    /// Fresh install or legacy migration.
    Install,
    /// Native upgrade.
    Upgrade,
    /// Native rollback.
    Rollback,
}

impl TransactionOp {
    /// Stable lowercase label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::Rollback => "rollback",
        }
    }
}

/// Journal lifecycle. No other states are accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionState {
    /// The staged tree has been verified and the journal is durable.
    Staged,
    /// The target tree has been renamed into `versions/`.
    Committed,
    /// `current` points at the target and the state file is written.
    Activated,
}

impl TransactionState {
    /// Stable lowercase label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Committed => "committed",
            Self::Activated => "activated",
        }
    }
}

/// Last successful transaction embedded in `state.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LastTransaction {
    /// Transaction identifier.
    pub id: String,
    /// Operation.
    pub op: TransactionOp,
    /// Final state; only `activated` is valid here.
    pub state: TransactionState,
    /// Activated target version.
    pub target: String,
    /// Previous active version, or null for a first native install/migration.
    pub previous: Option<String>,
    /// UTC start time.
    pub started_at: String,
    /// UTC finish time.
    pub finished_at: String,
}

/// Contents of `state.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallState {
    /// State schema; must be 2.
    pub schema_version: u32,
    /// Active native version.
    pub active: String,
    /// Previous native version, or null. Legacy packages are never recorded.
    pub previous: Option<String>,
    /// Active engine identity.
    pub engine: StateEngine,
    /// Last successful transaction.
    pub last_transaction: Option<LastTransaction>,
}

impl InstallState {
    /// Builds the state written after a successful activation.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid versions or transaction identifiers.
    pub fn new(
        active: &str,
        previous: Option<&str>,
        manifest: &ManifestV2,
        transaction: &Journal,
        finished_at: &str,
    ) -> Result<Self> {
        let last = transaction.last_transaction(finished_at)?;
        let state = Self {
            schema_version: STATE_SCHEMA_VERSION,
            active: active.to_owned(),
            previous: previous.map(str::to_owned),
            engine: StateEngine::from_manifest(manifest),
            last_transaction: Some(last),
        };
        state.validate()?;
        Ok(state)
    }

    /// Validates structural invariants without reading the installation root.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown schema, invalid versions, a legacy
    /// `previous`, mismatched engine identity or an inconsistent last
    /// transaction.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != STATE_SCHEMA_VERSION {
            return Err(DeliveryError::InstallState(format!(
                "unsupported state schema {}",
                self.schema_version
            )));
        }
        validate_version(&self.active)?;
        if let Some(previous) = &self.previous {
            validate_version(previous)?;
            if previous == &self.active {
                return Err(DeliveryError::InstallState(
                    "state.previous must differ from state.active".to_owned(),
                ));
            }
        }
        self.engine.validate()?;
        if let Some(last) = &self.last_transaction {
            validate_transaction_id(&last.id)?;
            if last.state != TransactionState::Activated {
                return Err(DeliveryError::InstallState(
                    "state.lastTransaction.state must be `activated`".to_owned(),
                ));
            }
            if last.target != self.active {
                return Err(DeliveryError::InstallState(
                    "state.lastTransaction.target does not match state.active".to_owned(),
                ));
            }
            if last.previous != self.previous {
                return Err(DeliveryError::InstallState(
                    "state.lastTransaction.previous does not match state.previous".to_owned(),
                ));
            }
            if last.op != TransactionOp::Install && last.previous.is_none() {
                return Err(DeliveryError::InstallState(
                    "upgrade/rollback lastTransaction requires a previous version".to_owned(),
                ));
            }
            require_timestamp(&last.started_at, "lastTransaction.startedAt")?;
            require_timestamp(&last.finished_at, "lastTransaction.finishedAt")?;
        }
        Ok(())
    }

    /// Validates the state against the on-disk `current`, `versions/` and
    /// manifest files without executing anything.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryError::InstallState`] when `current` disagrees with
    /// `active`, a version directory is missing or legacy, or the engine
    /// identity differs from the active manifest.
    pub fn validate_against(&self, root: &InstallRoot) -> Result<()> {
        self.validate()?;
        let current = root.current_version()?.ok_or_else(|| {
            DeliveryError::InstallState("state.json exists but no version is active".to_owned())
        })?;
        if current != self.active {
            return Err(DeliveryError::InstallState(format!(
                "state.json active `{}` does not match current `{current}`",
                self.active
            )));
        }
        let active_dir = root.versions_dir().join(&self.active);
        match read_manifest_schema(&active_dir)? {
            ManifestSchema::V2 => {}
            ManifestSchema::V1 => {
                return Err(DeliveryError::InstallState(
                    "state.json active version is a legacy Node package".to_owned(),
                ));
            }
        }
        let manifest = read_manifest_v2(
            &active_dir,
            ExpectedManifestV2 {
                version: Some(&self.active),
                ..ExpectedManifestV2::default()
            },
        )?;
        if !self.engine.matches_manifest(&manifest) {
            return Err(DeliveryError::InstallState(
                "state.json engine identity does not match the active manifest".to_owned(),
            ));
        }
        if let Some(previous) = &self.previous {
            let previous_dir = root.versions_dir().join(previous);
            if !previous_dir.is_dir() {
                return Err(DeliveryError::InstallState(format!(
                    "state.previous `{previous}` is not installed"
                )));
            }
            match read_manifest_schema(&previous_dir)? {
                ManifestSchema::V2 => {}
                ManifestSchema::V1 => {
                    return Err(DeliveryError::InstallState(
                        "state.previous must be a native schema v2 package".to_owned(),
                    ));
                }
            }
            read_manifest_v2(
                &previous_dir,
                ExpectedManifestV2 {
                    version: Some(previous),
                    ..ExpectedManifestV2::default()
                },
            )?;
        }
        Ok(())
    }

    /// Reads `state.json`, returning `None` when it does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is oversized, malformed or invalid.
    pub fn read(root: &InstallRoot) -> Result<Option<Self>> {
        let path = state_path(root);
        let bytes = match read_limited(&path, MAX_STATE_BYTES) {
            Ok(bytes) => bytes,
            Err(DeliveryError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let state: Self = serde_json::from_slice(&bytes).map_err(|error| {
            DeliveryError::InstallState(format!("state.json is malformed: {error}"))
        })?;
        state.validate()?;
        Ok(Some(state))
    }

    /// Atomically writes `state.json` and fsyncs the install root.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid state or an I/O failure.
    pub fn write(&self, root: &InstallRoot) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| DeliveryError::InstallState(error.to_string()))?;
        atomic_write(&state_path(root), &bytes, 0o644)?;
        fsync_dir(root.path())
    }
}

/// One transaction journal file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Journal {
    /// Journal schema; must be 2.
    pub schema_version: u32,
    /// Transaction identifier; also the file stem.
    pub id: String,
    /// Operation.
    pub op: TransactionOp,
    /// Target version.
    pub target: String,
    /// Active version before the transaction; null for legacy/fresh installs.
    pub previous: Option<String>,
    /// Current lifecycle state.
    pub state: TransactionState,
    /// UTC start time.
    pub started_at: String,
    /// UTC time of the last state write.
    pub updated_at: String,
}

impl Journal {
    /// Creates a `staged` journal with a unique identifier.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid versions or transaction constraints.
    pub fn new(
        op: TransactionOp,
        target: &str,
        previous: Option<&str>,
        started_at: &str,
    ) -> Result<Self> {
        let journal = Self {
            schema_version: STATE_SCHEMA_VERSION,
            id: next_transaction_id(),
            op,
            target: target.to_owned(),
            previous: previous.map(str::to_owned),
            state: TransactionState::Staged,
            started_at: started_at.to_owned(),
            updated_at: started_at.to_owned(),
        };
        journal.validate()?;
        Ok(journal)
    }

    /// Validates journal fields and operation/previous constraints.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid schema, identifier, version, timestamp
    /// or missing previous version for upgrade/rollback.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != STATE_SCHEMA_VERSION {
            return Err(DeliveryError::InstallState(format!(
                "unsupported journal schema {}",
                self.schema_version
            )));
        }
        validate_transaction_id(&self.id)?;
        validate_version(&self.target)?;
        if let Some(previous) = &self.previous {
            validate_version(previous)?;
            if previous == &self.target {
                return Err(DeliveryError::InstallState(
                    "journal.previous must differ from journal.target".to_owned(),
                ));
            }
        }
        if self.op != TransactionOp::Install && self.previous.is_none() {
            return Err(DeliveryError::InstallState(
                "upgrade/rollback journals require a previous version".to_owned(),
            ));
        }
        require_timestamp(&self.started_at, "journal.startedAt")?;
        require_timestamp(&self.updated_at, "journal.updatedAt")?;
        Ok(())
    }

    /// Moves the journal to the next lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid transition or timestamp.
    pub fn transition(&mut self, next: TransactionState, now: &str) -> Result<()> {
        let valid = matches!(
            (self.state, next),
            (
                TransactionState::Staged,
                TransactionState::Committed | TransactionState::Activated
            ) | (TransactionState::Committed, TransactionState::Activated)
        );
        if !valid {
            return Err(DeliveryError::InstallState(format!(
                "invalid journal transition {} -> {}",
                self.state.as_str(),
                next.as_str()
            )));
        }
        require_timestamp(now, "journal.updatedAt")?;
        self.state = next;
        now.clone_into(&mut self.updated_at);
        Ok(())
    }

    /// Builds the `state.json` last-transaction record.
    ///
    /// # Errors
    ///
    /// Returns an error unless the journal is activated and its identifier is
    /// valid.
    pub fn last_transaction(&self, finished_at: &str) -> Result<LastTransaction> {
        self.validate()?;
        if self.state != TransactionState::Activated {
            return Err(DeliveryError::InstallState(
                "only an activated journal can become state.lastTransaction".to_owned(),
            ));
        }
        require_timestamp(finished_at, "lastTransaction.finishedAt")?;
        Ok(LastTransaction {
            id: self.id.clone(),
            op: self.op,
            state: self.state,
            target: self.target.clone(),
            previous: self.previous.clone(),
            started_at: self.started_at.clone(),
            finished_at: finished_at.to_owned(),
        })
    }

    /// Atomically writes the journal below `<root>/txn/`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid journal or an I/O failure.
    pub fn write(&self, root: &InstallRoot) -> Result<()> {
        self.validate()?;
        let directory = journal_dir(root);
        ensure_journal_dir(&directory)?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| DeliveryError::InstallState(error.to_string()))?;
        atomic_write(&directory.join(format!("{}.json", self.id)), &bytes, 0o644)?;
        fsync_dir(&directory)
    }

    /// Reads one journal by identifier.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid identifier, a missing or oversized file,
    /// malformed JSON or a filename/content mismatch.
    pub fn read(root: &InstallRoot, id: &str) -> Result<Self> {
        validate_transaction_id(id)?;
        let path = journal_path(root, id);
        let bytes = read_limited(&path, MAX_STATE_BYTES)?;
        let journal: Self = serde_json::from_slice(&bytes).map_err(|error| {
            DeliveryError::InstallState(format!("transaction journal is malformed: {error}"))
        })?;
        journal.validate()?;
        if journal.id != id {
            return Err(DeliveryError::InstallState(
                "transaction journal id does not match its filename".to_owned(),
            ));
        }
        Ok(journal)
    }

    /// Lists all valid journals in stable identifier order.
    ///
    /// # Errors
    ///
    /// Returns an error for an unreadable journal directory or a malformed
    /// journal file.
    pub fn list(root: &InstallRoot) -> Result<Vec<Self>> {
        let directory = journal_dir(root);
        ensure_real_directory_or_missing(&directory, "transaction directory")?;
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(DeliveryError::io(&directory, error)),
        };
        let mut journals = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| DeliveryError::io(&directory, error))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            journals.push(Self::read(root, id)?);
        }
        journals.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(journals)
    }

    /// Removes one journal, ignoring an already missing file.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid identifier or an I/O failure other than
    /// `NotFound`.
    pub fn remove(root: &InstallRoot, id: &str) -> Result<()> {
        validate_transaction_id(id)?;
        let path = journal_path(root, id);
        match std::fs::remove_file(&path) {
            Ok(()) => fsync_dir(&journal_dir(root)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DeliveryError::io(&path, error)),
        }
    }
}

/// Returns `<root>/state.json`.
#[must_use]
pub fn state_path(root: &InstallRoot) -> PathBuf {
    root.path().join(STATE_FILE)
}

/// Returns `<root>/txn`.
#[must_use]
pub fn journal_dir(root: &InstallRoot) -> PathBuf {
    root.path().join(JOURNAL_DIR)
}

/// Returns `<root>/txn/<id>.json`.
#[must_use]
pub fn journal_path(root: &InstallRoot, id: &str) -> PathBuf {
    journal_dir(root).join(format!("{id}.json"))
}

fn ensure_journal_dir(directory: &Path) -> Result<()> {
    ensure_real_directory_or_missing(directory, "transaction directory")?;
    let existed = directory.exists();
    std::fs::create_dir_all(directory).map_err(|error| DeliveryError::io(directory, error))?;
    if !existed {
        set_mode(directory, 0o755)?;
    }
    Ok(())
}

fn ensure_real_directory_or_missing(path: &Path, label: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(DeliveryError::InstallState(
            format!("{label} must not be a symbolic link"),
        )),
        Ok(metadata) if !metadata.file_type().is_dir() => Err(DeliveryError::InstallState(
            format!("{label} is not a directory"),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeliveryError::io(path, error)),
    }
}

fn next_transaction_id() -> String {
    let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = now_rfc3339();
    let compact: String = now
        .chars()
        .filter(|ch| !matches!(ch, '-' | ':' | 'Z'))
        .collect();
    format!("txn-{compact}Z-{}-{counter}", std::process::id())
}

fn validate_transaction_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 {
        return Err(DeliveryError::InstallState(
            "transaction id must be 1-128 bytes".to_owned(),
        ));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(DeliveryError::InstallState(format!(
            "transaction id `{id}` contains unsupported characters"
        )));
    }
    Ok(())
}

fn require_timestamp(value: &str, label: &str) -> Result<()> {
    if is_rfc3339_utc(value) {
        Ok(())
    } else {
        Err(DeliveryError::InstallState(format!(
            "{label} must be a UTC RFC 3339 timestamp"
        )))
    }
}

/// Returns true for a `YYYY-MM-DDTHH:MM:SSZ` UTC timestamp.
#[must_use]
pub fn is_rfc3339_utc(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 {
        return false;
    }
    let positions = [4, 7, 10, 13, 16];
    let separators = *b"--T::";
    for (position, separator) in positions.iter().zip(separators) {
        if bytes.get(*position) != Some(&separator) {
            return false;
        }
    }
    if bytes.get(19) != Some(&b'Z') {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if positions.contains(&index) || index == 19 {
            continue;
        }
        if !byte.is_ascii_digit() {
            return false;
        }
    }
    let year = parse_two(bytes, 0) * 100 + parse_two(bytes, 2);
    let month = parse_two(bytes, 5);
    let day = parse_two(bytes, 8);
    let hour = parse_two(bytes, 11);
    let minute = parse_two(bytes, 14);
    let second = parse_two(bytes, 17);
    (1970..=9999).contains(&year)
        && (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second <= 60
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn parse_two(bytes: &[u8], offset: usize) -> u32 {
    let tens = bytes.get(offset).copied().unwrap_or(b'0') - b'0';
    let ones = bytes.get(offset + 1).copied().unwrap_or(b'0') - b'0';
    u32::from(tens) * 10 + u32::from(ones)
}

/// Validates a UTC RFC 3339 timestamp for manifest fields.
///
/// # Errors
///
/// Returns [`DeliveryError::InvalidPackage`] for a malformed timestamp.
pub fn validate_rfc3339_utc(value: &str) -> Result<()> {
    if is_rfc3339_utc(value) {
        Ok(())
    } else {
        Err(DeliveryError::InvalidPackage(
            "timestamp must be UTC RFC 3339 (`YYYY-MM-DDTHH:MM:SSZ`)".to_owned(),
        ))
    }
}

/// Validates that an optional path is a real directory inside the install root.
///
/// # Errors
///
/// Returns an error for a symbolic link, a non-directory, an escaped canonical
/// path or an I/O failure. Missing paths are accepted for mutating commands.
pub fn ensure_safe_directory(root: &InstallRoot, path: &Path, label: &str) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(DeliveryError::io(path, error)),
    };
    if metadata.file_type().is_symlink() {
        return Err(DeliveryError::InstallState(format!(
            "{label} must be a real directory, not a symbolic link"
        )));
    }
    if !metadata.file_type().is_dir() {
        return Err(DeliveryError::InstallState(format!(
            "{label} is not a directory"
        )));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| DeliveryError::io(path, error))?;
    if !canonical.starts_with(root.path()) {
        return Err(DeliveryError::InstallState(format!(
            "{label} resolves outside the install root"
        )));
    }
    Ok(())
}

/// Rejects symlinked or non-directory mutable root paths.
///
/// This covers `versions/`, `tmp/`, `txn/`, `cache/` and `state.json`.
///
/// # Errors
///
/// Returns [`DeliveryError::InstallState`] for a link, non-directory or escaped
/// path, and an I/O error when metadata cannot be read.
pub fn ensure_safe_layout(root: &InstallRoot) -> Result<()> {
    ensure_safe_directory(root, &root.versions_dir(), "versions directory")?;
    ensure_safe_directory(root, &root.tmp_dir(), "temporary directory")?;
    ensure_safe_directory(root, &journal_dir(root), "transaction directory")?;
    ensure_safe_directory(root, &root.cache_dir(), "cache directory")?;
    let state = state_path(root);
    match std::fs::symlink_metadata(&state) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(DeliveryError::InstallState(
            "state.json must not be a symbolic link".to_owned(),
        )),
        Ok(metadata) if !metadata.file_type().is_file() => Err(DeliveryError::InstallState(
            "state.json is not a regular file".to_owned(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeliveryError::io(&state, error)),
    }
}

/// Fsyncs a directory entry on Unix.
///
/// # Errors
///
/// Returns an error when the directory cannot be opened or synced.
pub fn fsync_dir(path: &Path) -> Result<()> {
    let directory = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    directory
        .sync_all()
        .map_err(|error| DeliveryError::io(path, error))
}

/// Fsyncs every regular file and directory below `root`.
///
/// Links and other non-regular entries are rejected rather than followed.
///
/// # Errors
///
/// Returns an error for an unsafe entry or an I/O failure.
pub fn fsync_tree(root: &Path) -> Result<()> {
    fsync_tree_inner(root)?;
    fsync_dir(root)
}

fn fsync_tree_inner(directory: &Path) -> Result<()> {
    let entries =
        std::fs::read_dir(directory).map_err(|error| DeliveryError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(directory, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(DeliveryError::UnsafeArchive(
                "staging tree contains a symbolic link".to_owned(),
            ));
        }
        if metadata.file_type().is_dir() {
            fsync_tree_inner(&path)?;
            fsync_dir(&path)?;
        } else if metadata.file_type().is_file() {
            let file = File::open(&path).map_err(|error| DeliveryError::io(&path, error))?;
            file.sync_all()
                .map_err(|error| DeliveryError::io(&path, error))?;
        } else {
            return Err(DeliveryError::UnsafeArchive(
                "staging tree contains a non-regular entry".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Removes leftover staging entries, preserving `keep` when it is inside
/// `<root>/tmp`.
///
/// Callers must hold the root's exclusive install lock. Without it, this may
/// delete another process's in-flight staging tree.
///
/// # Errors
///
/// Returns an error when an entry cannot be removed.
pub fn cleanup_tmp(root: &InstallRoot, keep: Option<&Path>) -> Result<()> {
    let tmp = root.tmp_dir();
    ensure_real_directory_or_missing(&tmp, "temporary directory")?;
    let entries = match std::fs::read_dir(&tmp) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(DeliveryError::io(&tmp, error)),
    };
    let keep = keep.and_then(|path| std::fs::canonicalize(path).ok());
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(&tmp, error))?;
        let path = entry.path();
        let canonical = std::fs::canonicalize(&path).ok();
        if let (Some(keep), Some(canonical)) = (&keep, &canonical) {
            if keep.starts_with(canonical) {
                continue;
            }
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if metadata.file_type().is_dir() {
            std::fs::remove_dir_all(&path).map_err(|error| DeliveryError::io(&path, error))?;
        } else {
            std::fs::remove_file(&path).map_err(|error| DeliveryError::io(&path, error))?;
        }
    }
    Ok(())
}

/// Returns all directory paths that must exist for a set of files.
#[must_use]
pub fn expected_directories(files: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for file in files {
        let mut current = PathBuf::from(&file);
        while let Some(parent) = current.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            let text = parent.to_string_lossy().replace('\\', "/");
            if validate_relative_path(&text, false).is_ok() {
                directories.insert(text);
            }
            current = parent.to_path_buf();
        }
    }
    directories
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_utc_timestamps_strictly() {
        assert!(is_rfc3339_utc("2026-11-17T00:00:00Z"));
        assert!(is_rfc3339_utc("2024-02-29T23:59:60Z"));
        for value in [
            "",
            "2026-11-17T00:00:00",
            "2026-11-17 00:00:00Z",
            "2026-13-17T00:00:00Z",
            "2026-02-31T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2026-11-17T24:00:00Z",
            "2026-11-17T00:60:00Z",
            "2026-11-17T00:00:00+00:00",
            "2026-11-17T00:00:00z",
        ] {
            assert!(!is_rfc3339_utc(value), "should reject {value:?}");
        }
    }

    #[test]
    fn journals_only_allow_frozen_transitions() {
        let mut journal = Journal::new(
            TransactionOp::Install,
            "0.2.0",
            None,
            "2026-11-17T00:00:00Z",
        )
        .unwrap();
        journal
            .transition(TransactionState::Committed, "2026-11-17T00:00:02Z")
            .unwrap();
        assert!(
            journal
                .transition(TransactionState::Staged, "2026-11-17T00:00:03Z")
                .is_err()
        );
        journal
            .transition(TransactionState::Activated, "2026-11-17T00:00:04Z")
            .unwrap();
        assert!(
            journal
                .transition(TransactionState::Activated, "2026-11-17T00:00:05Z")
                .is_err()
        );
        let last = journal.last_transaction("2026-11-17T00:00:06Z").unwrap();
        assert_eq!(last.state, TransactionState::Activated);
        assert_eq!(last.target, "0.2.0");

        // Rollback has no rename step, so `staged -> activated` is allowed.
        let mut rollback = Journal::new(
            TransactionOp::Rollback,
            "0.1.0",
            Some("0.2.0"),
            "2026-11-17T00:00:00Z",
        )
        .unwrap();
        rollback
            .transition(TransactionState::Activated, "2026-11-17T00:00:01Z")
            .unwrap();
    }

    #[test]
    fn upgrade_and_rollback_require_a_previous_version() {
        for op in [TransactionOp::Upgrade, TransactionOp::Rollback] {
            let error = Journal::new(op, "0.2.0", None, "2026-11-17T00:00:00Z").unwrap_err();
            assert!(matches!(error, DeliveryError::InstallState(_)), "{error:?}");
        }
        Journal::new(
            TransactionOp::Upgrade,
            "0.2.0",
            Some("0.1.0"),
            "2026-11-17T00:00:00Z",
        )
        .unwrap();
    }

    #[test]
    fn transaction_ids_are_unique_and_safe() {
        let first = Journal::new(
            TransactionOp::Install,
            "0.1.0",
            None,
            "2026-11-17T00:00:00Z",
        )
        .unwrap()
        .id;
        let second = Journal::new(
            TransactionOp::Install,
            "0.1.0",
            None,
            "2026-11-17T00:00:00Z",
        )
        .unwrap()
        .id;
        assert_ne!(first, second);
        for id in [&first, &second] {
            assert!(id.starts_with("txn-"));
            assert!(
                id.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            );
        }
        assert!(validate_transaction_id("../escape").is_err());
        assert!(validate_transaction_id("").is_err());
    }

    #[test]
    fn expected_directories_are_deterministic() {
        let directories = expected_directories(["a/b/c".to_owned(), "a/d".to_owned()]);
        assert_eq!(
            directories,
            BTreeSet::from(["a".to_owned(), "a/b".to_owned()])
        );
    }
}
