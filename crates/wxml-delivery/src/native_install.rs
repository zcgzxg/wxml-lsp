//! Native install, upgrade, rollback and recovery state machine.
//!
//! The core invariant is that `current` changes only after a staged tree has
//! passed manifest validation, exact allowlist hashing and the engine version
//! probe. Before the switch, every failure leaves `current` and both trees
//! byte-identical. After the switch, a native `N1 -> N2` failure restores `N1`
//! only while `state.json` still names it; once the new state is persisted the
//! verified target is rolled forward. A legacy schema v1 package is never
//! executed, restored or recorded as a native rollback target.
//!
//! The API/schema handshake (`engine/lib/tsc --api --async`) is deliberately
//! not implemented here. [`crate::native_install::NativeEngineProbe`] is the
//! extension point a later wave uses to add the F0 handshake without changing
//! the transaction code.
//!
//! `WXML_LSP_BIN` is checked by the CLI before it creates a root. Native
//! mutating entry points are exposed as `*_locked` variants so a caller can
//! hold one transaction lock across state inspection, acquisition, staging and
//! activation without taking a second lock (which would self-deadlock).
//!
//! BLOCKED in this wave: F3-01/F3-02 (no clean-PATH/strace exec audit),
//! F3-03 (no v2 packager), F3-12 (no network-namespace audit), F3-13 (no real
//! asset/ABI data), F3-14 (no Zed smoke) and F3-15 (Zed forwarding not
//! exercised). F3-04/05/10/11/16 have library-level invariants covered here
//! but still require real assets and the CLI for end-to-end acceptance.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{DeliveryError, Result};
use crate::install::{InstallLock, InstallRoot, SwitchOutcome};
use crate::manifest::{now_rfc3339, sha256_file};
use crate::manifest_v2::{
    ExpectedManifestV2, ManifestSchema, ManifestV2, NATIVE_PACKAGE_PLATFORM, read_manifest_schema,
    read_manifest_v2,
};
use crate::paths::host_platform;
use crate::semver::compare_versions;
use crate::state::{
    InstallState, Journal, TransactionOp, TransactionState, cleanup_tmp, ensure_safe_layout,
    fsync_dir, fsync_tree,
};
use crate::verify::output_with_timeout;
use crate::verify_v2::{check_native_engine, tree_fingerprint, verify_tree_v2};

/// Default timeout for activation probes.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Rust LSP entry point.
pub const LSP_BINARY_PATH: &str = "bin/wxml-lsp";
/// Rust installer entry point.
pub const INSTALLER_BINARY_PATH: &str = "bin/wxml-lsp-install";

/// A transaction checkpoint that tests can turn into an injected failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailPoint {
    /// The `staged` journal was written.
    AfterStagedJournal,
    /// The target tree exists under `versions/`.
    AfterRename,
    /// The journal was marked `committed`.
    AfterCommittedJournal,
    /// Immediately before the `current` symlink switch.
    BeforeSwitch,
    /// Immediately after the `current` symlink switch.
    AfterSwitch,
    /// Immediately before a post-switch failure switches `current` back to the
    /// previous verified native version.
    BeforeRestoreSwitch,
    /// Immediately before `state.json` is replaced.
    BeforeStateWrite,
    /// Immediately after `state.json` is replaced.
    AfterStateWrite,
    /// Immediately after the `activated` journal is written.
    AfterActivatedJournal,
}

/// Transaction checkpoints. Production uses [`NoHooks`].
pub trait InstallHooks {
    /// Called at each checkpoint. Returning an error aborts the transaction as
    /// if the process had stopped at that point.
    ///
    /// # Errors
    ///
    /// Implementations return an error to inject a failure.
    fn checkpoint(&self, _point: FailPoint) -> Result<()> {
        Ok(())
    }
}

/// No-op production hooks.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoHooks;

impl InstallHooks for NoHooks {}

/// Engine probe contract.
///
/// Production uses [`CommandEngineProbe`]. Tests inject a fake probe so no
/// package-provided binary is executed.
pub trait NativeEngineProbe {
    /// Probes a staged or installed package tree.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine is missing, unsafe, cannot run or
    /// reports an identity other than the manifest's.
    fn probe(&self, tree: &Path, manifest: &ManifestV2) -> Result<()>;
}

/// Runs `engine/lib/tsc --version` with a cleared environment and timeout.
#[derive(Clone, Copy, Debug)]
pub struct CommandEngineProbe {
    timeout: Duration,
}

impl Default for CommandEngineProbe {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }
}

impl CommandEngineProbe {
    /// Creates a probe with an explicit timeout.
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl NativeEngineProbe for CommandEngineProbe {
    fn probe(&self, tree: &Path, manifest: &ManifestV2) -> Result<()> {
        check_engine_version(tree, manifest, self.timeout)
    }
}

/// Runs the manifest-declared version probe on a staged or installed tree.
///
/// The `--api --async` / AST-header handshake is intentionally deferred to the
/// F0/native-client wave; no activation here may claim that handshake passed.
///
/// # Errors
///
/// Returns an error when the binary is not a matching regular executable, the
/// command cannot start, times out, exits non-zero, prints non-UTF-8 output or
/// does not print exactly `<expectPrefix><version>` after trailing whitespace
/// is removed.
pub fn check_engine_version(tree: &Path, manifest: &ManifestV2, timeout: Duration) -> Result<()> {
    let binary = check_native_engine(tree, manifest)?;
    let engine_dir = tree.join("engine");
    if !engine_dir.is_dir() {
        return Err(DeliveryError::Runtime(
            "native engine directory is missing".to_owned(),
        ));
    }
    let mut command = Command::new(&binary);
    command
        .args(&manifest.engine.probe.args)
        .current_dir(engine_dir)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null());
    let output = output_with_timeout(&mut command, timeout)?;
    if !output.status.success() {
        return Err(DeliveryError::Runtime(
            "native engine version probe exited unsuccessfully".to_owned(),
        ));
    }
    match_engine_version_output(
        &output.stdout,
        &manifest.engine.probe.expect_prefix,
        &manifest.engine.version,
    )
}

/// Validates the exact version line printed by the native engine.
///
/// # Errors
///
/// Returns an error for non-UTF-8 output or a line that does not equal
/// `<prefix><version>` after trailing whitespace is removed.
pub fn match_engine_version_output(stdout: &[u8], prefix: &str, version: &str) -> Result<()> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| DeliveryError::Runtime("engine version output was not UTF-8".to_owned()))?;
    let expected = format!("{prefix}{version}");
    if text.trim_end() != expected {
        return Err(DeliveryError::Runtime(format!(
            "engine version output does not equal `{expected}`"
        )));
    }
    Ok(())
}

/// Runs the Rust LSP and installer version probes before activation.
///
/// # Errors
///
/// Returns an error when either binary is missing, linked, non-executable,
/// does not match the manifest, exits unsuccessfully, times out or prints
/// anything other than the exact version line.
pub fn check_activation_binaries(
    tree: &Path,
    manifest: &ManifestV2,
    timeout: Duration,
) -> Result<()> {
    check_rust_binary_version(tree, manifest, LSP_BINARY_PATH, "wxml-lsp", timeout)?;
    check_rust_binary_version(
        tree,
        manifest,
        INSTALLER_BINARY_PATH,
        "wxml-lsp-install",
        timeout,
    )
}

fn check_rust_binary_version(
    tree: &Path,
    manifest: &ManifestV2,
    path: &str,
    expected_name: &str,
    timeout: Duration,
) -> Result<()> {
    let binary = tree.join(path);
    let metadata =
        std::fs::symlink_metadata(&binary).map_err(|error| DeliveryError::io(&binary, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::Runtime(format!(
            "`{path}` is not a regular file"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(DeliveryError::Runtime(format!(
                "`{path}` is not executable"
            )));
        }
    }
    let file = manifest
        .file(path)
        .ok_or_else(|| DeliveryError::Runtime(format!("manifest does not list `{path}`")))?;
    if metadata.len() != file.size || sha256_file(&binary)? != file.sha256 {
        return Err(DeliveryError::Runtime(format!(
            "`{path}` does not match the manifest"
        )));
    }

    let mut command = Command::new(&binary);
    command
        .arg("--version")
        .current_dir(tree)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null());
    let output = output_with_timeout(&mut command, timeout)?;
    if !output.status.success() {
        return Err(DeliveryError::Runtime(format!(
            "`{path} --version` exited unsuccessfully"
        )));
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|_| DeliveryError::Runtime(format!("`{path} --version` output was not UTF-8")))?;
    let expected = format!("{expected_name} {}", manifest.package_version);
    if stdout.trim_end() != expected {
        return Err(DeliveryError::Runtime(format!(
            "`{path} --version` output does not equal `{expected}`"
        )));
    }
    Ok(())
}

/// Current installation kind, detected without executing anything.
#[derive(Clone, Debug)]
pub enum CurrentInstall {
    /// No `current` link.
    None,
    /// Legacy schema v1 package. Read-only; never executed or reactivated.
    Legacy {
        /// Legacy version from the symlink target.
        version: String,
    },
    /// Verified schema v2 native package.
    Native {
        /// Active version.
        version: String,
        /// Parsed native manifest.
        manifest: Box<ManifestV2>,
    },
}

impl CurrentInstall {
    /// Returns the active version, if any.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Legacy { version } | Self::Native { version, .. } => Some(version),
        }
    }
}

/// Result of a successful native mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeInstallOutcome {
    /// First native install into an empty root.
    Installed,
    /// Legacy Node package migrated to a native package.
    Migrated,
    /// Native N1 upgraded to N2.
    Upgraded,
    /// The requested version was already active and verified.
    AlreadyInstalled,
    /// A verified native version was reactivated.
    RolledBack,
}

/// Summary returned by [`verify_native`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeVerifyReport {
    /// Active version.
    pub version: String,
    /// Active engine version.
    pub engine_version: String,
    /// Verified payload file count.
    pub file_count: usize,
    /// Verified payload bytes.
    pub total_bytes: u64,
}

/// Summary returned by [`recover_native`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Human-readable recovery actions.
    pub actions: Vec<String>,
    /// Active version after recovery, if any.
    pub current: Option<String>,
}

/// Inspects `current` without executing the package.
///
/// # Errors
///
/// Returns an error for a dangling/unsafe link, unknown schema, malformed
/// manifest or a legacy manifest that fails the minimal identity checks.
pub fn inspect_current(root: &InstallRoot) -> Result<CurrentInstall> {
    let Some(version) = root.current_version()? else {
        return Ok(CurrentInstall::None);
    };
    let directory = root.versions_dir().join(&version);
    let metadata = match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeliveryError::InstallState(format!(
                "current points at missing version `{version}`"
            )));
        }
        Err(error) => return Err(DeliveryError::io(&directory, error)),
    };
    if !metadata.file_type().is_dir() {
        return Err(DeliveryError::InstallState(format!(
            "version `{version}` is not a real directory"
        )));
    }
    match read_manifest_schema(&directory)? {
        ManifestSchema::V1 => {
            // Read only the identity projection. The old binary and Node
            // runtime are never started.
            crate::manifest_v2::read_legacy_manifest(&directory)?;
            Ok(CurrentInstall::Legacy { version })
        }
        ManifestSchema::V2 => {
            let manifest = read_manifest_v2(
                &directory,
                ExpectedManifestV2 {
                    version: Some(&version),
                    platform: Some(NATIVE_PACKAGE_PLATFORM),
                    ..ExpectedManifestV2::default()
                },
            )?;
            Ok(CurrentInstall::Native {
                version,
                manifest: Box::new(manifest),
            })
        }
    }
}

/// Installs, upgrades or migrates a staged native tree.
///
/// The tree must be below `<root>/tmp/`. Verification and the probe run before
/// any rename or symlink switch. This wrapper takes the transaction lock and
/// then calls [`install_native_locked`].
///
/// # Errors
///
/// Returns an error for a non-native host, unsafe staging path, failed
/// verification/probe, downgrade request, same-version content mismatch,
/// invalid existing state or an I/O failure. Before the switch, the previous
/// `current` and tree remain byte-identical. After the switch, the function
/// either restores the previous verified native version (only while state
/// still names it) or rolls the verified target forward; a legacy Node current
/// is never restored.
pub fn install_native(
    root: &InstallRoot,
    tree: &Path,
    expected: ExpectedManifestV2<'_>,
    probe: &dyn NativeEngineProbe,
    hooks: &dyn InstallHooks,
) -> Result<NativeInstallOutcome> {
    let lock = root.lock()?;
    install_native_locked(root, tree, expected, probe, hooks, &lock)
}

/// Installs, upgrades or migrates a staged native tree while the caller holds
/// the exclusive install lock.
///
/// `lock` must come from [`InstallRoot::lock`] on the same root. Holding it
/// across state inspection, acquisition and this call serializes concurrent
/// mutating commands and prevents [`crate::state::cleanup_tmp`] from removing
/// another process's in-flight staging tree.
///
/// # Errors
///
/// See [`install_native`].
#[allow(clippy::too_many_lines)]
pub fn install_native_locked(
    root: &InstallRoot,
    tree: &Path,
    expected: ExpectedManifestV2<'_>,
    probe: &dyn NativeEngineProbe,
    hooks: &dyn InstallHooks,
    _lock: &InstallLock,
) -> Result<NativeInstallOutcome> {
    ensure_native_host()?;
    ensure_safe_layout(root)?;
    let tree = canonical_staging_tree(root, tree)?;
    cleanup_tmp(root, Some(&tree))?;
    clean_staged_journals(root)?;

    let prepared = match prepare_install(root, &tree, expected, probe) {
        Ok(value) => value,
        Err(error) => {
            let _ = remove_staged(root, &tree);
            return Err(error);
        }
    };
    let Some(PreparedInstall {
        manifest,
        target,
        op,
        previous,
        outcome,
        fingerprint,
    }) = prepared
    else {
        remove_staged(root, &tree)?;
        if let Some(version) = root.current_version()? {
            remove_journals_for_target(root, &version, None)?;
        }
        return Ok(NativeInstallOutcome::AlreadyInstalled);
    };

    let destination = root.versions_dir().join(&target);
    let destination_exists = destination.exists();

    let activation_path = if destination_exists {
        destination.clone()
    } else {
        tree.clone()
    };
    let recheck = verify_tree_v2(
        &activation_path,
        ExpectedManifestV2 {
            version: Some(&target),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    );
    if let Err(error) = recheck {
        let _ = remove_staged(root, &tree);
        return Err(error);
    }
    let actual_fingerprint = match tree_fingerprint(&activation_path) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let _ = remove_staged(root, &tree);
            return Err(error);
        }
    };
    if actual_fingerprint != fingerprint {
        let _ = remove_staged(root, &tree);
        return Err(DeliveryError::InstallState(
            "activation tree changed after probes and before commit".to_owned(),
        ));
    }

    let versions = root.versions_dir();
    let versions_existed = versions.exists();
    std::fs::create_dir_all(&versions).map_err(|error| DeliveryError::io(&versions, error))?;
    if !versions_existed {
        crate::manifest::set_mode(&versions, 0o755)?;
    }

    let started_at = now_rfc3339();
    let mut journal = Journal::new(op, &target, previous.as_deref(), &started_at)?;
    if let Err(error) = journal.write(root) {
        let _ = cleanup_tmp(root, None);
        remove_empty_versions(root);
        return Err(error);
    }
    fsync_dir(&root.tmp_dir())?;
    fsync_dir(root.path())?;
    if let Err(error) = hooks.checkpoint(FailPoint::AfterStagedJournal) {
        let _ = Journal::remove(root, &journal.id);
        if !destination_exists {
            let _ = cleanup_tmp(root, None);
            remove_empty_versions(root);
        }
        return Err(error);
    }

    if !destination_exists {
        if let Err(error) = fsync_tree(&tree) {
            let _ = Journal::remove(root, &journal.id);
            let _ = cleanup_tmp(root, None);
            remove_empty_versions(root);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&tree, &destination) {
            let _ = Journal::remove(root, &journal.id);
            let _ = cleanup_tmp(root, None);
            remove_empty_versions(root);
            return Err(DeliveryError::io(&destination, error));
        }
        fsync_dir(&root.versions_dir())?;
    }
    if let Err(error) = hooks.checkpoint(FailPoint::AfterRename) {
        mark_committed(root, &mut journal)?;
        return Err(error);
    }

    mark_committed(root, &mut journal)?;
    hooks.checkpoint(FailPoint::AfterCommittedJournal)?;
    hooks.checkpoint(FailPoint::BeforeSwitch)?;

    // The target tree is complete after this switch; a pre-switch failure
    // leaves `current` unchanged. Once the link moved, a failure must either
    // restore the previous *verified native* version or reconcile the
    // already-verified target. A legacy Node `current` is never restored.
    match root.switch(&target) {
        Ok(SwitchOutcome::Switched) => {}
        Ok(SwitchOutcome::SwitchedButNotDurable) => {
            let error = DeliveryError::InstallState(
                "current was switched but the install root fsync failed".to_owned(),
            );
            return handle_post_switch_failure(
                root,
                &target,
                previous.as_deref(),
                probe,
                hooks,
                &journal,
                outcome,
                &error,
            );
        }
        Err(error) => return Err(error),
    }
    if let Err(error) = hooks.checkpoint(FailPoint::AfterSwitch) {
        return handle_post_switch_failure(
            root,
            &target,
            previous.as_deref(),
            probe,
            hooks,
            &journal,
            outcome,
            &error,
        );
    }
    if let Err(error) = hooks.checkpoint(FailPoint::BeforeStateWrite) {
        return handle_post_switch_failure(
            root,
            &target,
            previous.as_deref(),
            probe,
            hooks,
            &journal,
            outcome,
            &error,
        );
    }

    let finished_at = now_rfc3339();
    journal.transition(TransactionState::Activated, &finished_at)?;
    let new_state = match InstallState::new(
        &target,
        previous.as_deref(),
        &manifest,
        &journal,
        &finished_at,
    ) {
        Ok(state) => state,
        Err(error) => {
            return handle_post_switch_failure(
                root,
                &target,
                previous.as_deref(),
                probe,
                hooks,
                &journal,
                outcome,
                &error,
            );
        }
    };
    if let Err(error) = new_state.write(root) {
        return handle_post_switch_failure(
            root,
            &target,
            previous.as_deref(),
            probe,
            hooks,
            &journal,
            outcome,
            &error,
        );
    }
    if let Err(error) = hooks.checkpoint(FailPoint::AfterStateWrite) {
        return handle_post_switch_failure(
            root,
            &target,
            previous.as_deref(),
            probe,
            hooks,
            &journal,
            outcome,
            &error,
        );
    }
    if let Err(error) = journal.write(root) {
        return handle_post_switch_failure(
            root,
            &target,
            previous.as_deref(),
            probe,
            hooks,
            &journal,
            outcome,
            &error,
        );
    }
    hooks.checkpoint(FailPoint::AfterActivatedJournal)?;
    Journal::remove(root, &journal.id)?;
    remove_journals_for_target(root, &target, None)?;
    cleanup_tmp(root, None)?;
    Ok(outcome)
}

struct PreparedInstall {
    manifest: ManifestV2,
    target: String,
    op: TransactionOp,
    previous: Option<String>,
    outcome: NativeInstallOutcome,
    fingerprint: String,
}

fn prepare_install(
    root: &InstallRoot,
    tree: &Path,
    expected: ExpectedManifestV2<'_>,
    probe: &dyn NativeEngineProbe,
) -> Result<Option<PreparedInstall>> {
    let fingerprint_before = tree_fingerprint(tree)?;
    let report = verify_tree_v2(tree, expected)?;
    probe.probe(tree, &report.manifest)?;
    check_activation_binaries(tree, &report.manifest, DEFAULT_PROBE_TIMEOUT)?;
    let after = verify_tree_v2(tree, expected)?;
    if tree_fingerprint(tree)? != fingerprint_before {
        return Err(DeliveryError::InstallState(
            "activation probe modified the staged tree".to_owned(),
        ));
    }
    let manifest = after.manifest;
    let target = manifest.package_version.clone();
    let state = InstallState::read(root)?;
    let current = inspect_current(root)?;
    validate_current_state(root, &current, state.as_ref())?;

    let (op, previous, outcome) = plan_operation(&current, &manifest, probe, &root.versions_dir())?;
    if outcome == NativeInstallOutcome::AlreadyInstalled {
        return Ok(None);
    }
    let destination = root.versions_dir().join(&target);
    let fingerprint = if destination.exists() {
        let fingerprint_before = tree_fingerprint(&destination)?;
        let existing = verify_tree_v2(
            &destination,
            ExpectedManifestV2 {
                version: Some(&target),
                platform: Some(NATIVE_PACKAGE_PLATFORM),
                ..ExpectedManifestV2::default()
            },
        )?;
        if !same_native_payload(&existing.manifest, &manifest) {
            return Err(DeliveryError::InstallState(format!(
                "version {target} is already installed with different contents; \
                 use a new version or remove it explicitly"
            )));
        }
        probe.probe(&destination, &existing.manifest)?;
        check_activation_binaries(&destination, &existing.manifest, DEFAULT_PROBE_TIMEOUT)?;
        if tree_fingerprint(&destination)? != fingerprint_before {
            return Err(DeliveryError::InstallState(
                "activation probe modified the existing target tree".to_owned(),
            ));
        }
        remove_staged(root, tree)?;
        fingerprint_before
    } else {
        fingerprint_before
    };
    Ok(Some(PreparedInstall {
        manifest,
        target,
        op,
        previous,
        outcome,
        fingerprint,
    }))
}

fn plan_operation(
    current: &CurrentInstall,
    manifest: &ManifestV2,
    probe: &dyn NativeEngineProbe,
    versions: &Path,
) -> Result<(TransactionOp, Option<String>, NativeInstallOutcome)> {
    let target = &manifest.package_version;
    match current {
        CurrentInstall::None => Ok((
            TransactionOp::Install,
            None,
            NativeInstallOutcome::Installed,
        )),
        CurrentInstall::Legacy { .. } => {
            Ok((TransactionOp::Install, None, NativeInstallOutcome::Migrated))
        }
        CurrentInstall::Native {
            version: active,
            manifest: active_manifest,
        } => match compare_versions(target, active) {
            Ordering::Greater => {
                // `previous` becomes the native rollback target, so it must
                // still match its own manifest byte for byte. Legacy/Node
                // packages never reach this branch.
                verify_tree_v2(
                    &versions.join(active),
                    ExpectedManifestV2 {
                        version: Some(active),
                        platform: Some(NATIVE_PACKAGE_PLATFORM),
                        ..ExpectedManifestV2::default()
                    },
                )?;
                Ok((
                    TransactionOp::Upgrade,
                    Some(active.clone()),
                    NativeInstallOutcome::Upgraded,
                ))
            }
            Ordering::Equal => {
                if !same_native_payload(active_manifest, manifest) {
                    return Err(DeliveryError::InstallState(format!(
                        "version {target} is already active with different contents; \
                         rebuild with a new version number"
                    )));
                }
                let active_dir = versions.join(active);
                let fingerprint = tree_fingerprint(&active_dir)?;
                let installed = verify_tree_v2(
                    &active_dir,
                    ExpectedManifestV2 {
                        version: Some(active),
                        platform: Some(NATIVE_PACKAGE_PLATFORM),
                        ..ExpectedManifestV2::default()
                    },
                )?;
                probe.probe(&active_dir, &installed.manifest)?;
                check_activation_binaries(&active_dir, &installed.manifest, DEFAULT_PROBE_TIMEOUT)?;
                if tree_fingerprint(&active_dir)? != fingerprint {
                    return Err(DeliveryError::InstallState(
                        "activation probe modified the active tree".to_owned(),
                    ));
                }
                Ok((
                    TransactionOp::Install,
                    None,
                    NativeInstallOutcome::AlreadyInstalled,
                ))
            }
            Ordering::Less => Err(DeliveryError::InstallState(format!(
                "refusing to downgrade from {active} to {target}; use `rollback` explicitly"
            ))),
        },
    }
}

fn validate_current_state(
    root: &InstallRoot,
    current: &CurrentInstall,
    state: Option<&InstallState>,
) -> Result<()> {
    match current {
        CurrentInstall::Native { .. } => {
            let state = state.ok_or_else(|| {
                DeliveryError::InstallState(
                    "native current exists without state.json; run recovery".to_owned(),
                )
            })?;
            state.validate_against(root)?;
        }
        CurrentInstall::Legacy { .. } => {
            if state.is_some() {
                return Err(DeliveryError::InstallState(
                    "state.json must not exist for a legacy current".to_owned(),
                ));
            }
        }
        CurrentInstall::None => {
            if state.is_some() {
                return Err(DeliveryError::InstallState(
                    "state.json exists but no version is active".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

/// Rolls back to a previously installed, fully verified native version.
///
/// This wrapper takes the transaction lock and then calls
/// [`rollback_native_locked`].
///
/// # Errors
///
/// Returns an error when there is no native current, the target is missing,
/// the target is legacy schema 1, verification/probe fails or an I/O operation
/// fails. A legacy target is never executed or activated.
pub fn rollback_native(
    root: &InstallRoot,
    to: &str,
    probe: &dyn NativeEngineProbe,
    hooks: &dyn InstallHooks,
) -> Result<NativeInstallOutcome> {
    let lock = root.lock()?;
    rollback_native_locked(root, to, probe, hooks, &lock)
}

/// Rolls back while the caller holds the exclusive install lock.
///
/// `lock` must come from [`InstallRoot::lock`] on the same root.
///
/// # Errors
///
/// See [`rollback_native`].
#[allow(clippy::too_many_lines)]
pub fn rollback_native_locked(
    root: &InstallRoot,
    to: &str,
    probe: &dyn NativeEngineProbe,
    hooks: &dyn InstallHooks,
    _lock: &InstallLock,
) -> Result<NativeInstallOutcome> {
    ensure_native_host()?;
    ensure_safe_layout(root)?;
    cleanup_tmp(root, None)?;
    let current = inspect_current(root)?;
    let active = match &current {
        CurrentInstall::Native { version, .. } => version.clone(),
        CurrentInstall::Legacy { .. } => {
            return Err(DeliveryError::InstallState(
                "legacy Node package is not a Node-free rollback target".to_owned(),
            ));
        }
        CurrentInstall::None => {
            return Err(DeliveryError::InstallState(
                "rollback requires a native current".to_owned(),
            ));
        }
    };
    let state = InstallState::read(root)?.ok_or_else(|| {
        DeliveryError::InstallState("rollback requires a native state.json".to_owned())
    })?;
    state.validate_against(root)?;
    crate::paths::validate_version(to)?;
    let target_dir = root.versions_dir().join(to);
    if !target_dir.is_dir() {
        return Err(DeliveryError::InstallState(format!(
            "rollback target {to} is not installed"
        )));
    }
    match read_manifest_schema(&target_dir)? {
        ManifestSchema::V1 => {
            return Err(DeliveryError::InstallState(
                "legacy Node package is not a Node-free rollback target".to_owned(),
            ));
        }
        ManifestSchema::V2 => {}
    }
    let expected = ExpectedManifestV2 {
        version: Some(to),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        ..ExpectedManifestV2::default()
    };
    let fingerprint = tree_fingerprint(&target_dir)?;
    let report = verify_tree_v2(&target_dir, expected)?;
    probe.probe(&target_dir, &report.manifest)?;
    check_activation_binaries(&target_dir, &report.manifest, DEFAULT_PROBE_TIMEOUT)?;
    if tree_fingerprint(&target_dir)? != fingerprint {
        return Err(DeliveryError::InstallState(
            "activation probe modified the rollback target".to_owned(),
        ));
    }
    if to == active {
        return Ok(NativeInstallOutcome::AlreadyInstalled);
    }

    let started_at = now_rfc3339();
    let mut journal = Journal::new(TransactionOp::Rollback, to, Some(&active), &started_at)?;
    journal.write(root)?;
    hooks.checkpoint(FailPoint::AfterStagedJournal)?;
    // Persist `committed` before the switch so a crash after the symlink
    // rename is classified as a committed rollback, not a disposable stage.
    mark_committed(root, &mut journal)?;
    hooks.checkpoint(FailPoint::BeforeSwitch)?;
    match root.switch(to) {
        Ok(SwitchOutcome::Switched) => {}
        Ok(SwitchOutcome::SwitchedButNotDurable) => {
            let error = DeliveryError::InstallState(
                "current was switched but the install root fsync failed".to_owned(),
            );
            return handle_post_switch_failure(
                root,
                to,
                Some(&active),
                probe,
                hooks,
                &journal,
                NativeInstallOutcome::RolledBack,
                &error,
            );
        }
        Err(error) => return Err(error),
    }
    if let Err(error) = hooks.checkpoint(FailPoint::AfterSwitch) {
        return handle_post_switch_failure(
            root,
            to,
            Some(&active),
            probe,
            hooks,
            &journal,
            NativeInstallOutcome::RolledBack,
            &error,
        );
    }
    if let Err(error) = hooks.checkpoint(FailPoint::BeforeStateWrite) {
        return handle_post_switch_failure(
            root,
            to,
            Some(&active),
            probe,
            hooks,
            &journal,
            NativeInstallOutcome::RolledBack,
            &error,
        );
    }

    let finished_at = now_rfc3339();
    journal.transition(TransactionState::Activated, &finished_at)?;
    let new_state =
        match InstallState::new(to, Some(&active), &report.manifest, &journal, &finished_at) {
            Ok(state) => state,
            Err(error) => {
                return handle_post_switch_failure(
                    root,
                    to,
                    Some(&active),
                    probe,
                    hooks,
                    &journal,
                    NativeInstallOutcome::RolledBack,
                    &error,
                );
            }
        };
    if let Err(error) = new_state.write(root) {
        return handle_post_switch_failure(
            root,
            to,
            Some(&active),
            probe,
            hooks,
            &journal,
            NativeInstallOutcome::RolledBack,
            &error,
        );
    }
    if let Err(error) = hooks.checkpoint(FailPoint::AfterStateWrite) {
        return handle_post_switch_failure(
            root,
            to,
            Some(&active),
            probe,
            hooks,
            &journal,
            NativeInstallOutcome::RolledBack,
            &error,
        );
    }
    if let Err(error) = journal.write(root) {
        return handle_post_switch_failure(
            root,
            to,
            Some(&active),
            probe,
            hooks,
            &journal,
            NativeInstallOutcome::RolledBack,
            &error,
        );
    }
    hooks.checkpoint(FailPoint::AfterActivatedJournal)?;
    Journal::remove(root, &journal.id)?;
    remove_journals_for_target(root, to, None)?;
    Ok(NativeInstallOutcome::RolledBack)
}

/// Verifies the active native installation without changing it.
///
/// Read-only commands do not create the root or a lock file.
///
/// # Errors
///
/// Returns an error when nothing is active, the active package is legacy,
/// `state.json` is inconsistent, the tree fails verification or the probe
/// fails.
pub fn verify_native(
    root: &InstallRoot,
    probe: &dyn NativeEngineProbe,
) -> Result<NativeVerifyReport> {
    ensure_native_host()?;
    ensure_safe_layout(root)?;
    let current = inspect_current(root)?;
    let version = match current {
        CurrentInstall::Native { version, .. } => version,
        CurrentInstall::Legacy { .. } => {
            return Err(DeliveryError::InstallState(
                "legacy Node package is not a Node-free target; it was not executed".to_owned(),
            ));
        }
        CurrentInstall::None => {
            return Err(DeliveryError::InstallState(
                "no version is active".to_owned(),
            ));
        }
    };
    let state = InstallState::read(root)?.ok_or_else(|| {
        DeliveryError::InstallState("native current exists without state.json".to_owned())
    })?;
    state.validate_against(root)?;
    let directory = root.versions_dir().join(&version);
    let fingerprint = tree_fingerprint(&directory)?;
    let report = verify_tree_v2(
        &directory,
        ExpectedManifestV2 {
            version: Some(&version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    probe.probe(&directory, &report.manifest)?;
    check_activation_binaries(&directory, &report.manifest, DEFAULT_PROBE_TIMEOUT)?;
    let recheck = verify_tree_v2(
        &directory,
        ExpectedManifestV2 {
            version: Some(&version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    if recheck.file_count != report.file_count || tree_fingerprint(&directory)? != fingerprint {
        return Err(DeliveryError::InstallState(
            "activation probe modified the active tree".to_owned(),
        ));
    }
    Ok(NativeVerifyReport {
        version,
        engine_version: report.manifest.engine.version.clone(),
        file_count: report.file_count,
        total_bytes: report.total_bytes,
    })
}

/// Recovers after a process stop by applying the journal rules.
///
/// `staged` removes temporary state and never activates; `committed` keeps the
/// target and never auto-activates unless `current` already points at it (then
/// it re-verifies and completes the state write); `activated` is an idempotent
/// confirmation.
///
/// # Errors
///
/// Returns an error for inconsistent journals, a dangling `current`, failed
/// re-verification or an I/O failure.
pub fn recover_native(root: &InstallRoot, probe: &dyn NativeEngineProbe) -> Result<RecoveryReport> {
    ensure_native_host()?;
    ensure_safe_layout(root)?;
    let _lock = root.lock()?;
    let mut actions = Vec::new();
    for journal in Journal::list(root)? {
        match journal.state {
            TransactionState::Staged => recover_staged(root, journal, probe, &mut actions)?,
            TransactionState::Committed => {
                recover_committed(root, journal, probe, &mut actions)?;
            }
            TransactionState::Activated => {
                recover_activated(root, journal, probe, &mut actions)?;
            }
        }
    }
    cleanup_tmp(root, None)?;
    reconcile_current_state(root)?;
    let current = inspect_current(root)?.version().map(str::to_owned);
    Ok(RecoveryReport { actions, current })
}

fn recover_staged(
    root: &InstallRoot,
    mut journal: Journal,
    probe: &dyn NativeEngineProbe,
    actions: &mut Vec<String>,
) -> Result<()> {
    let target_dir = root.versions_dir().join(&journal.target);
    let target_exists = real_directory_exists(&target_dir)?;
    let current = inspect_current(root)?;
    if target_exists {
        if current.version() == Some(journal.target.as_str()) {
            complete_current_target(root, journal, probe, actions)?;
        } else {
            // A crash after rename but before the journal transition leaves a
            // valid target tree on disk. Keep it and record the observed disk
            // state instead of deleting evidence.
            journal.transition(TransactionState::Committed, &now_rfc3339())?;
            journal.write(root)?;
            cleanup_tmp(root, None)?;
            actions.push(format!(
                "staged {} -> target exists; marked committed; not auto-activated",
                journal.target
            ));
        }
    } else {
        cleanup_tmp(root, None)?;
        Journal::remove(root, &journal.id)?;
        actions.push(format!(
            "staged {} -> removed temporary state; current unchanged",
            journal.target
        ));
    }
    Ok(())
}

fn recover_committed(
    root: &InstallRoot,
    journal: Journal,
    probe: &dyn NativeEngineProbe,
    actions: &mut Vec<String>,
) -> Result<()> {
    let target_dir = root.versions_dir().join(&journal.target);
    if !real_directory_exists(&target_dir)? {
        return Err(DeliveryError::InstallState(format!(
            "committed journal {} has no target directory {}",
            journal.id, journal.target
        )));
    }
    let current = inspect_current(root)?;
    if current.version() == Some(journal.target.as_str()) {
        complete_current_target(root, journal, probe, actions)?;
        return Ok(());
    }
    let state_targets_journal = InstallState::read(root)?
        .as_ref()
        .is_some_and(|state| state.active == journal.target);
    if state_targets_journal && matches!(current, CurrentInstall::Native { .. }) {
        // Repair the "current is a verified native version, state.json names
        // the committed journal target" window. Re-verify both sides before
        // moving `current`, then complete the journal idempotently.
        verify_current_native(root, &current)?;
        verify_journal_target(root, &journal)?;
        let current_version = current.version().unwrap_or("<none>").to_owned();
        match root.switch(&journal.target)? {
            SwitchOutcome::Switched => {}
            SwitchOutcome::SwitchedButNotDurable => {
                return Err(DeliveryError::InstallState(format!(
                    "recovery rolled current forward to {} but the install root fsync failed; \
                     retry `wxml-lsp-install recover`",
                    journal.target
                )));
            }
        }
        actions.push(format!(
            "committed {} -> current {current_version} did not match state; \
             re-verified and rolled forward",
            journal.target
        ));
        write_state_for_current_target(root, journal, probe, actions)?;
        return Ok(());
    }
    cleanup_tmp(root, None)?;
    actions.push(format!(
        "committed {} -> kept target; not auto-activated",
        journal.target
    ));
    Ok(())
}

fn verify_current_native(root: &InstallRoot, current: &CurrentInstall) -> Result<()> {
    let CurrentInstall::Native { version, .. } = current else {
        return Err(DeliveryError::InstallState(
            "repair requires a native current".to_owned(),
        ));
    };
    verify_tree_v2(
        &root.versions_dir().join(version),
        ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    Ok(())
}

fn verify_journal_target(root: &InstallRoot, journal: &Journal) -> Result<()> {
    verify_tree_v2(
        &root.versions_dir().join(&journal.target),
        ExpectedManifestV2 {
            version: Some(&journal.target),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    verify_journal_previous(root, journal)
}

/// Rejects a journal that names a legacy Node package as `previous` and
/// re-verifies any native previous tree before it can enter `state.json`.
fn verify_journal_previous(root: &InstallRoot, journal: &Journal) -> Result<()> {
    let Some(previous) = journal.previous.as_deref() else {
        return Ok(());
    };
    let previous_dir = root.versions_dir().join(previous);
    if read_manifest_schema(&previous_dir)? == ManifestSchema::V1 {
        return Err(DeliveryError::InstallState(
            "recovery journal records a legacy Node package as previous".to_owned(),
        ));
    }
    verify_tree_v2(
        &previous_dir,
        ExpectedManifestV2 {
            version: Some(previous),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    Ok(())
}

fn recover_activated(
    root: &InstallRoot,
    journal: Journal,
    probe: &dyn NativeEngineProbe,
    actions: &mut Vec<String>,
) -> Result<()> {
    let current = inspect_current(root)?;
    if current.version() != Some(journal.target.as_str()) {
        return Err(DeliveryError::InstallState(format!(
            "activated journal {} does not match current",
            journal.target
        )));
    }
    if valid_state_for_target(root, &journal.target)? {
        // Idempotent confirmation: never rewrite previous/lastTransaction when
        // the on-disk state already records this active target.
        Journal::remove(root, &journal.id)?;
        actions.push(format!(
            "activated {} -> valid state already active; journal removed",
            journal.target
        ));
        return Ok(());
    }
    write_state_for_current_target(root, journal, probe, actions)
}

fn complete_current_target(
    root: &InstallRoot,
    journal: Journal,
    probe: &dyn NativeEngineProbe,
    actions: &mut Vec<String>,
) -> Result<()> {
    if valid_state_for_target(root, &journal.target)? {
        Journal::remove(root, &journal.id)?;
        actions.push(format!(
            "{} {} -> valid state already active; journal removed",
            journal.state.as_str(),
            journal.target
        ));
        return Ok(());
    }
    write_state_for_current_target(root, journal, probe, actions)
}

fn write_state_for_current_target(
    root: &InstallRoot,
    mut journal: Journal,
    probe: &dyn NativeEngineProbe,
    actions: &mut Vec<String>,
) -> Result<()> {
    let target_dir = root.versions_dir().join(&journal.target);
    let fingerprint = tree_fingerprint(&target_dir)?;
    let report = verify_tree_v2(
        &target_dir,
        ExpectedManifestV2 {
            version: Some(&journal.target),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    probe.probe(&target_dir, &report.manifest)?;
    check_activation_binaries(&target_dir, &report.manifest, DEFAULT_PROBE_TIMEOUT)?;
    verify_tree_v2(
        &target_dir,
        ExpectedManifestV2 {
            version: Some(&journal.target),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            ..ExpectedManifestV2::default()
        },
    )?;
    if tree_fingerprint(&target_dir)? != fingerprint {
        return Err(DeliveryError::InstallState(
            "recovery probe modified the target tree".to_owned(),
        ));
    }
    verify_journal_previous(root, &journal)?;
    let finished_at = now_rfc3339();
    if journal.state != TransactionState::Activated {
        journal.transition(TransactionState::Activated, &finished_at)?;
    }
    let state = InstallState::new(
        &journal.target,
        journal.previous.as_deref(),
        &report.manifest,
        &journal,
        &finished_at,
    )?;
    state.write(root)?;
    journal.write(root)?;
    Journal::remove(root, &journal.id)?;
    remove_journals_for_target(root, &journal.target, None)?;
    actions.push(format!("{} -> verified and state written", journal.target));
    Ok(())
}

fn valid_state_for_target(root: &InstallRoot, target: &str) -> Result<bool> {
    let Some(state) = InstallState::read(root)? else {
        return Ok(false);
    };
    if state.active != target {
        return Ok(false);
    }
    state.validate_against(root)?;
    Ok(true)
}

fn real_directory_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(DeliveryError::InstallState(
            "target version directory must not be a symbolic link".to_owned(),
        )),
        Ok(_) => Err(DeliveryError::InstallState(
            "target version path is not a directory".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(DeliveryError::io(path, error)),
    }
}

/// Reconciles `current` and `state.json` when no journal can explain a
/// mismatch. Never reports success for an inconsistent install.
///
/// # Errors
///
/// Returns [`DeliveryError::InstallState`] for state without current, native
/// current without state, legacy current with state, or active/current
/// disagreement.
pub fn reconcile_current_state(root: &InstallRoot) -> Result<()> {
    let current = inspect_current(root)?;
    let state = InstallState::read(root)?;
    match (&current, &state) {
        (CurrentInstall::None | CurrentInstall::Legacy { .. }, None) => Ok(()),
        (CurrentInstall::Native { .. }, Some(state)) => state.validate_against(root),
        (CurrentInstall::None, Some(_)) => Err(DeliveryError::InstallState(
            "state.json exists but no version is active".to_owned(),
        )),
        (CurrentInstall::Legacy { .. }, Some(_)) => Err(DeliveryError::InstallState(
            "state.json exists for a legacy Node current".to_owned(),
        )),
        (CurrentInstall::Native { version, .. }, None) => Err(DeliveryError::InstallState(
            format!("native current {version} has no state.json and no journal repair"),
        )),
    }
}

fn remove_journals_for_target(root: &InstallRoot, target: &str, keep: Option<&str>) -> Result<()> {
    for journal in Journal::list(root)? {
        if journal.target == target && keep != Some(journal.id.as_str()) {
            Journal::remove(root, &journal.id)?;
        }
    }
    Ok(())
}

/// Returns a stable fingerprint of the active tree without executing it.
///
/// # Errors
///
/// Returns an error when no native version is active or the tree is unsafe.
pub fn active_tree_fingerprint(root: &InstallRoot) -> Result<String> {
    let directory = root.current_dir()?;
    tree_fingerprint(&directory)
}

fn ensure_native_host() -> Result<()> {
    let platform = host_platform()?;
    if platform != NATIVE_PACKAGE_PLATFORM {
        return Err(DeliveryError::UnsupportedPlatform(format!(
            "native packages require {NATIVE_PACKAGE_PLATFORM}, host is {platform}"
        )));
    }
    Ok(())
}

fn canonical_staging_tree(root: &InstallRoot, tree: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(tree).map_err(|error| DeliveryError::io(tree, error))?;
    if !canonical.is_dir() {
        return Err(DeliveryError::InvalidArgument(
            "staging tree is not a directory".to_owned(),
        ));
    }
    let tmp = std::fs::canonicalize(root.tmp_dir())
        .map_err(|error| DeliveryError::io(&root.tmp_dir(), error))?;
    if !canonical.starts_with(&tmp) {
        return Err(DeliveryError::InstallState(
            "staging tree is outside the install root's tmp directory".to_owned(),
        ));
    }
    Ok(canonical)
}

fn mark_committed(root: &InstallRoot, journal: &mut Journal) -> Result<()> {
    journal.transition(TransactionState::Committed, &now_rfc3339())?;
    journal.write(root)
}

fn remove_staged(root: &InstallRoot, tree: &Path) -> Result<()> {
    if tree.exists() {
        std::fs::remove_dir_all(tree).map_err(|error| DeliveryError::io(tree, error))?;
    }
    fsync_dir(&root.tmp_dir())
}

fn remove_empty_versions(root: &InstallRoot) {
    let versions = root.versions_dir();
    if let Ok(mut entries) = std::fs::read_dir(&versions) {
        if entries.next().is_none() {
            let _ = std::fs::remove_dir(&versions);
        }
    }
}

fn clean_staged_journals(root: &InstallRoot) -> Result<()> {
    let current = inspect_current(root)?;
    for mut journal in Journal::list(root)? {
        if journal.state != TransactionState::Staged {
            continue;
        }
        let target_dir = root.versions_dir().join(&journal.target);
        if real_directory_exists(&target_dir)? {
            // Rename happened before the committed write; retain evidence.
            journal.transition(TransactionState::Committed, &now_rfc3339())?;
            journal.write(root)?;
        } else if current.version() == Some(journal.target.as_str()) {
            return Err(DeliveryError::InstallState(format!(
                "staged journal {} points at current but the target is missing",
                journal.target
            )));
        } else {
            Journal::remove(root, &journal.id)?;
        }
    }
    Ok(())
}

fn same_native_payload(left: &ManifestV2, right: &ManifestV2) -> bool {
    left.package_version == right.package_version
        && left.platform == right.platform
        && left.protocol_version == right.protocol_version
        && left.engine == right.engine
        && left.type_assets == right.type_assets
        && left.binaries == right.binaries
        && left.files == right.files
}

/// Returns whether `state.json` names the version `current` points at.
fn state_names_current(root: &InstallRoot) -> Result<bool> {
    let current = root.current_version()?;
    Ok(match (InstallState::read(root)?, current) {
        (None, None) => true,
        (Some(state), Some(current)) => state.active == current,
        _ => false,
    })
}

fn switched_but_not_durable(
    version: &str,
    previous: Option<&str>,
    state_reconciled: bool,
    error: &DeliveryError,
    recovery: &str,
) -> DeliveryError {
    DeliveryError::SwitchedButNotDurable {
        version: version.to_owned(),
        previous: previous.map(str::to_owned),
        state_reconciled,
        reason: error.to_string(),
        recovery: recovery.to_owned(),
    }
}

/// Handles any failure that happens after `current` was atomically switched.
///
/// A previous native version is restored only while `state.json` still names
/// it, so the restore does not need a state rewrite. If the new state was
/// already persisted, the verified target is rolled forward instead. A legacy
/// migration never restores the Node current; it rolls the verified target
/// forward and reports the typed recovery state.
#[allow(clippy::too_many_arguments)]
fn handle_post_switch_failure(
    root: &InstallRoot,
    target: &str,
    previous: Option<&str>,
    probe: &dyn NativeEngineProbe,
    hooks: &dyn InstallHooks,
    journal: &Journal,
    outcome: NativeInstallOutcome,
    error: &DeliveryError,
) -> Result<NativeInstallOutcome> {
    let state = InstallState::read(root)?;
    let current_is_target = root.current_version()?.as_deref() == Some(target);

    if previous.is_none() && outcome != NativeInstallOutcome::Migrated {
        if matches!(error, DeliveryError::Io { .. }) {
            // A state/journal I/O failure is always typed, including a first
            // install with no previous version.
            return Err(switched_but_not_durable(
                target,
                None,
                state_names_current(root).unwrap_or(false),
                error,
                "run `wxml-lsp-install recover` to complete the first install;                  never activate a legacy Node package",
            ));
        }
        // Other first-install interruptions keep the committed journal for
        // explicit recovery, matching the pre-existing first-install contract.
        return Err(DeliveryError::InstallState(format!(
            "activation failed after switch and no native previous version exists: {error}"
        )));
    }

    if outcome == NativeInstallOutcome::Migrated {
        if !current_is_target {
            return Err(switched_but_not_durable(
                target,
                None,
                state_names_current(root).unwrap_or(false),
                error,
                "run `wxml-lsp-install recover`; `current` must be reconciled to a verified \
                 native target, never a legacy Node package",
            ));
        }
        return Err(roll_forward_after_switch(
            root, target, probe, journal, error,
        ));
    }

    let Some(previous) = previous else {
        return Err(DeliveryError::InstallState(
            "post-switch failure has no previous native version and is not a legacy migration"
                .to_owned(),
        ));
    };
    if !current_is_target {
        return Err(switched_but_not_durable(
            target,
            Some(previous),
            state_names_current(root).unwrap_or(false),
            error,
            "run `wxml-lsp-install recover`; `current` must be reconciled to a verified \
             native version, never a legacy Node package",
        ));
    }
    if state.as_ref().is_some_and(|state| state.active == target) {
        // The new state was already persisted. Switching back would create a
        // current/state split if the old-state rewrite failed, so roll the
        // verified target forward instead.
        return Err(roll_forward_after_switch(
            root, target, probe, journal, error,
        ));
    }
    if state.as_ref().is_some_and(|state| state.active == previous) {
        // `state.json` still names the previous native version. Switch back
        // without rewriting state: there is no state-write crash window.
        return restore_previous(root, target, previous, hooks, error);
    }

    Err(switched_but_not_durable(
        target,
        Some(previous),
        state_names_current(root).unwrap_or(false),
        error,
        "state.json names neither the target nor the previous native version; \
         run `wxml-lsp-install recover`; never restore a legacy Node package",
    ))
}

/// Rolls the already-switched verified target forward and reconciles
/// `state.json`/the journal. Legacy Node packages are never recorded as
/// `previous`.
fn roll_forward_after_switch(
    root: &InstallRoot,
    target: &str,
    probe: &dyn NativeEngineProbe,
    journal: &Journal,
    error: &DeliveryError,
) -> DeliveryError {
    let mut actions = Vec::new();
    let (state_reconciled, recovery) =
        match write_state_for_current_target(root, journal.clone(), probe, &mut actions) {
            Ok(()) => (
                true,
                format!(
                    "state.json records `{target}`; run `wxml-lsp-install verify`; \
                 the legacy Node current is never restored"
                ),
            ),
            Err(reconcile_error) => (
                state_names_current(root).unwrap_or(false),
                format!(
                    "run `wxml-lsp-install recover` to complete `{target}`; committed journal \
                 retained; never restore the legacy Node package: {reconcile_error}"
                ),
            ),
        };
    DeliveryError::SwitchedButNotDurable {
        version: target.to_owned(),
        previous: journal.previous.clone(),
        state_reconciled,
        reason: error.to_string(),
        recovery,
    }
}

/// Restores the previous verified native version after a post-switch failure.
///
/// The caller only reaches this path while `state.json` still names
/// `previous`; the state file is deliberately not rewritten. If the previous
/// tree cannot be verified or the switch back fails, the error is
/// [`DeliveryError::SwitchedButNotDurable`] with an explicit recovery action.
/// A legacy Node package is never a candidate.
fn restore_previous(
    root: &InstallRoot,
    target: &str,
    previous: &str,
    hooks: &dyn InstallHooks,
    error: &DeliveryError,
) -> Result<NativeInstallOutcome> {
    let directory = root.versions_dir().join(previous);
    let expected = ExpectedManifestV2 {
        version: Some(previous),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        ..ExpectedManifestV2::default()
    };
    if let Err(verify_error) = verify_tree_v2(&directory, expected) {
        return Err(switched_but_not_durable(
            target,
            Some(previous),
            state_names_current(root).unwrap_or(false),
            error,
            &format!(
                "repair or reinstall native `{previous}`, then run `wxml-lsp-install recover`; \
                 never activate a legacy Node package: {verify_error}"
            ),
        ));
    }
    hooks.checkpoint(FailPoint::BeforeRestoreSwitch)?;
    match root.switch(previous) {
        Ok(SwitchOutcome::Switched) => {
            if matches!(error, DeliveryError::Io { .. }) {
                // A state-write failure must stay typed even though the
                // previous native version was restored without a rewrite.
                return Err(switched_but_not_durable(
                    previous,
                    Some(target),
                    state_names_current(root).unwrap_or(false),
                    error,
                    "the previous native version was restored but the state write failed; \
                     run `wxml-lsp-install verify`; never activate a legacy Node package",
                ));
            }
            Err(DeliveryError::InstallState(format!(
                "activation failed after switch; restored previous native {previous}: {error}"
            )))
        }
        Ok(SwitchOutcome::SwitchedButNotDurable) => Err(switched_but_not_durable(
            previous,
            Some(target),
            state_names_current(root).unwrap_or(false),
            error,
            "the switch back completed but the install root fsync failed; \
             run `wxml-lsp-install verify`",
        )),
        Err(restore) => Err(switched_but_not_durable(
            target,
            Some(previous),
            state_names_current(root).unwrap_or(false),
            error,
            &format!(
                "run `wxml-lsp-install recover`; if `current` is not a verified native tree, \
                 reinstall a native package; never activate a legacy Node package: {restore}"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_version_output_must_match_exactly() {
        match_engine_version_output(b"Version 7.1.0-beta\n", "Version ", "7.1.0-beta").unwrap();
        match_engine_version_output(b"Version 7.1.0-beta   \n", "Version ", "7.1.0-beta").unwrap();
        for output in [
            &b"Version 7.1.0-beta-extra\n"[..],
            b"version 7.1.0-beta\n",
            b" Version 7.1.0-beta\n",
            b"\xff\n",
            b"",
        ] {
            assert!(
                match_engine_version_output(output, "Version ", "7.1.0-beta").is_err(),
                "{output:?}"
            );
        }
    }

    #[test]
    fn fail_points_have_stable_debug_names() {
        assert_eq!(
            format!("{:?}", FailPoint::AfterCommittedJournal),
            "AfterCommittedJournal"
        );
        assert_eq!(crate::state::JOURNAL_DIR, "txn");
    }
}
