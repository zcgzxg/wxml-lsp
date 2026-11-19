//! Build-time provisioning of locked npm assets (F3-03 L2).
//!
//! The provisioner turns the submitted `assets.lock.json` plus local npm
//! tarballs into an immutable, content-addressed cache. It never downloads:
//! the only accepted source is `--from-dir`. `--offline` is therefore the
//! effective mode of this build; `--allow-download` is rejected explicitly
//! until the HTTPS path is implemented.
//!
//! # Pipeline
//!
//! 1. P0: read, parse and validate the repository lock before any cache I/O.
//! 2. P1: require a linux-x64 host (the lock is currently frozen to it).
//! 3. P2: look up the cache key `<payload.treeDigest>.<lockSha256[..16]>`.
//! 4. P3-P5: stage the exact `--from-dir` tarball and verify size/SRI/SHA-256
//!    before extraction.
//! 5. P6-P7: safely unpack a npm tarball (`package/` prefix, normalized modes)
//!    and verify every upstream byte before assembling the payload.
//! 6. P8: write provenance, the submitted lock and payload checksums, fsync and
//!    atomically rename into `assets/<name>/<key>/`.
//! 7. P9: every cache hit is re-hashed; a corrupt or foreign entry is never
//!    repaired in place.
//!
//! No function in this module starts a process or executes Node/npm.

mod assemble;
mod entry;
mod extract;
mod prescan;
mod source;

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::assets_lock::{ASSETS_LOCK_FILE_NAME, Asset, AssetsLock, MAX_ASSETS_LOCK_BYTES};
use crate::error::{DeliveryError, Result};
use crate::manifest::set_mode;
use crate::paths::{validate_relative_path, validate_sha256};

pub use extract::extract_npm_tar_gz;

/// Cache directory holding immutable provisioned entries.
pub const CACHE_ASSETS_DIR: &str = "assets";
/// Cache scratch directory; always mode `0700`.
pub const CACHE_TMP_DIR: &str = "tmp";
/// Per-entry provenance file name.
pub const PROVENANCE_FILE: &str = "provenance.json";
/// Exact submitted lock copy stored with every entry.
pub const LOCK_COPY_FILE: &str = "lock.json";
/// Payload-only checksum file.
pub const SHA256SUMS_FILE: &str = "SHA256SUMS";
/// Maximum accepted provenance file size.
pub const MAX_PROVENANCE_BYTES: u64 = 64 * 1024;
/// Maximum accepted cached `SHA256SUMS` size.
pub const MAX_CHECKSUMS_BYTES: u64 = 16 * 1024 * 1024;

/// Runtime options for `wxml-lsp-package provision`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionOptions {
    /// Repository `assets.lock.json`.
    pub lock: PathBuf,
    /// Absolute cache directory.
    pub cache: PathBuf,
    /// Absolute directory containing the locked tarball file names.
    pub from_dir: Option<PathBuf>,
    /// Reserved for scripts; this build always reads only the cache and
    /// `--from-dir` and never contacts the network.
    pub offline: bool,
    /// Describe the plan and exit without writing anything.
    pub print_plan: bool,
    /// Unimplemented network opt-in; always rejected in this build.
    pub allow_download: bool,
}

/// How an asset entered the cache during this run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// Staged from `--from-dir`.
    FromDir,
    /// Read from an existing, fully re-verified cache entry.
    Cache,
}

impl SourceKind {
    /// Stable lowercase label used by the CLI report.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FromDir => "from-dir",
            Self::Cache => "cache",
        }
    }
}

/// One asset in a read-only provisioning plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetPlan {
    /// Frozen asset name.
    pub name: String,
    /// `<payload.treeDigest>.<lockSha256[..16]>`.
    pub cache_key: String,
    /// Whether an entry directory with provenance is present.
    pub cache_hit: bool,
    /// Source that `provision` would use, if any.
    pub source: Option<SourceKind>,
    /// Exact tarball file name expected in `--from-dir`.
    pub tarball_file_name: String,
}

/// Read-only provisioning plan (`--print-plan`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionPlan {
    /// Canonical lock identity.
    pub lock_sha256: String,
    /// One plan row per locked asset, in lock order.
    pub assets: Vec<AssetPlan>,
}

/// A fully verified cache entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheEntry {
    /// Frozen asset name.
    pub name: String,
    /// Content-addressed entry key.
    pub cache_key: String,
    /// Absolute entry directory.
    pub directory: PathBuf,
    /// Payload tree digest from the verified files.
    pub payload_tree_digest: String,
    /// Number of payload files.
    pub file_count: u64,
    /// Total payload bytes.
    pub total_bytes: u64,
    /// How this run obtained the entry.
    pub source: SourceKind,
}

/// Result of a successful provisioning run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionReport {
    /// Canonical lock identity.
    pub lock_sha256: String,
    /// Verified entries in lock order.
    pub entries: Vec<CacheEntry>,
}

/// Returns whether this build can provision on the given host.
///
/// # Errors
///
/// Returns [`DeliveryError::UnsupportedPlatform`] unless the host is
/// `linux`/`x86_64`; the frozen lock currently has no other platform entry.
pub fn require_supported_host(os: &str, arch: &str) -> Result<()> {
    if os == "linux" && arch == "x86_64" {
        return Ok(());
    }
    Err(DeliveryError::UnsupportedPlatform(format!(
        "asset provisioning supports only linux-x64; this host is {os}-{arch}"
    )))
}

/// Computes a read-only plan without creating or modifying any file.
///
/// # Errors
///
/// Rejects `--allow-download`, an invalid lock/cache/source argument, an
/// unsupported host, an unsafe existing cache entry, or a malformed lock.
pub fn plan(options: &ProvisionOptions) -> Result<ProvisionPlan> {
    reject_download(options)?;
    let context = LockContext::load(options, false)?;
    check_existing_private_subdir(&context.cache, CACHE_ASSETS_DIR)?;
    check_existing_private_subdir(&context.cache, CACHE_TMP_DIR)?;
    let mut states = Vec::with_capacity(context.lock.assets.len());
    for asset in &context.lock.assets {
        let entry_dir = cache_entry_dir(&context.cache, asset, &context.lock_sha256)?;
        let state = cache_entry_state(&entry_dir)?;
        if state == CacheState::Invalid {
            return Err(invalid_cache_entry());
        }
        states.push((asset, state));
    }
    let has_miss = states
        .iter()
        .any(|(_, state)| *state == CacheState::Missing);
    let from_dir = if has_miss {
        source::prepare_from_dir(options.from_dir.as_deref())?
    } else {
        None
    };
    let mut assets = Vec::with_capacity(states.len());
    for (asset, state) in states {
        let source = if state == CacheState::Present {
            Some(SourceKind::Cache)
        } else if let Some(directory) = from_dir.as_deref() {
            source::find_tarball(directory, asset)?.map(|_| SourceKind::FromDir)
        } else {
            None
        };
        assets.push(AssetPlan {
            name: asset.name.clone(),
            cache_key: cache_key(asset, &context.lock_sha256)?,
            cache_hit: state == CacheState::Present,
            source,
            tarball_file_name: source::tarball_file_name(asset)?.to_owned(),
        });
    }
    Ok(ProvisionPlan {
        lock_sha256: context.lock_sha256,
        assets,
    })
}

/// Provisions every locked asset into the cache.
///
/// Cache hits are re-verified with P9; misses require `--from-dir`. Failures
/// remove the temporary workspace and never leave a partial cache entry.
///
/// # Errors
///
/// Returns a typed error for argument, platform, source, archive, integrity or
/// cache-state failures. It never falls back to a download or Node process.
pub fn provision(options: &ProvisionOptions) -> Result<ProvisionReport> {
    reject_download(options)?;
    if options.print_plan {
        return Err(DeliveryError::InvalidArgument(
            "--print-plan is read-only; use plan() instead of provision()".to_owned(),
        ));
    }
    let context = LockContext::load(options, true)?;
    let mut entries = Vec::with_capacity(context.lock.assets.len());
    for asset in &context.lock.assets {
        entries.push(provision_asset(
            asset,
            &context.cache,
            options.from_dir.as_deref(),
            &context.lock_sha256,
            &context.lock_bytes,
        )?);
    }
    Ok(ProvisionReport {
        lock_sha256: context.lock_sha256,
        entries,
    })
}

/// Provisions one asset from an already validated lock.
///
/// This is the L2 seam used by tests and by [`provision`]. The caller must pass
/// an [`Asset`] taken from a lock that passed [`AssetsLock::validate`]; the
/// per-asset pipeline re-checks the upstream/payload tables but does not repeat
/// the frozen-constant reconciliation.
///
/// # Errors
///
/// See [`provision`].
pub fn provision_asset(
    asset: &Asset,
    cache_root: &Path,
    from_dir: Option<&Path>,
    lock_sha256: &str,
    lock_bytes: &[u8],
) -> Result<CacheEntry> {
    validate_sha256(lock_sha256)?;
    if lock_bytes.is_empty()
        || u64::try_from(lock_bytes.len()).unwrap_or(u64::MAX) > MAX_ASSETS_LOCK_BYTES
    {
        return Err(DeliveryError::SizeLimit(
            "submitted assets lock bytes are empty or exceed the accepted size".to_owned(),
        ));
    }
    let cache = resolve_cache(cache_root, true)?;
    ensure_private_dir(&cache.join(CACHE_ASSETS_DIR))?;
    ensure_private_dir(&cache.join(CACHE_TMP_DIR))?;
    let entry_dir = cache_entry_dir(&cache, asset, lock_sha256)?;
    match cache_entry_state(&entry_dir)? {
        CacheState::Present => {
            return assemble::verify_cache_entry(asset, &entry_dir, lock_sha256, lock_bytes);
        }
        CacheState::Invalid => return Err(invalid_cache_entry()),
        CacheState::Missing => {}
    }
    let from_dir = source::prepare_from_dir(from_dir)?;
    let Some(from_dir) = from_dir else {
        return Err(no_source_error(asset, "no --from-dir was supplied"));
    };
    let file_name = source::tarball_file_name(asset)?;
    let Some(tarball) = source::find_tarball(&from_dir, asset)? else {
        return Err(no_source_error(
            asset,
            &format!("`{file_name}` is missing from --from-dir"),
        ));
    };
    source::provision_new_entry(
        asset,
        &cache,
        &entry_dir,
        &tarball,
        file_name,
        lock_sha256,
        lock_bytes,
    )
}

fn reject_download(options: &ProvisionOptions) -> Result<()> {
    if options.allow_download {
        return Err(DeliveryError::InvalidArgument(
            "--allow-download is not implemented in this build; use --from-dir (or --offline with a provisioned cache)"
                .to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn invalid_cache_entry() -> DeliveryError {
    DeliveryError::Integrity(
        "assets cache entry exists but is incomplete or unsafe; delete it and re-provision"
            .to_owned(),
    )
}

struct LockContext {
    lock: AssetsLock,
    lock_bytes: Vec<u8>,
    lock_sha256: String,
    cache: PathBuf,
}

impl LockContext {
    fn load(options: &ProvisionOptions, create_cache: bool) -> Result<Self> {
        let lock_path = canonical_lock_path(&options.lock)?;
        let lock_bytes = read_lock_bytes(&lock_path)?;
        let lock = AssetsLock::parse(&lock_bytes)?;
        lock.validate()?;
        require_supported_host(std::env::consts::OS, std::env::consts::ARCH)?;
        let lock_sha256 = lock.lock_sha256()?;
        let cache = resolve_cache(&options.cache, create_cache)?;
        Ok(Self {
            lock,
            lock_bytes,
            lock_sha256,
            cache,
        })
    }
}

fn canonical_lock_path(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(|error| DeliveryError::io(path, error))?;
    let metadata = std::fs::symlink_metadata(&canonical)
        .map_err(|error| DeliveryError::io(&canonical, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(
            "--lock must resolve to a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_ASSETS_LOCK_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock exceeds {MAX_ASSETS_LOCK_BYTES} bytes"
        )));
    }
    if canonical.file_name() != Some(std::ffi::OsStr::new(ASSETS_LOCK_FILE_NAME)) {
        return Err(DeliveryError::InvalidArgument(format!(
            "--lock file name must be `{ASSETS_LOCK_FILE_NAME}`"
        )));
    }
    let root = canonical.parent().ok_or_else(|| {
        DeliveryError::InvalidArgument("--lock has no repository root parent".to_owned())
    })?;
    if !root.join(".git").exists() && !root.join("Cargo.toml").is_file() {
        return Err(DeliveryError::InvalidArgument(
            "--lock parent directory must be a repository root containing `.git` or `Cargo.toml`"
                .to_owned(),
        ));
    }
    Ok(canonical)
}

fn read_lock_bytes(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).map_err(|error| DeliveryError::io(path, error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ASSETS_LOCK_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock exceeds {MAX_ASSETS_LOCK_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn resolve_cache(path: &Path, create: bool) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(DeliveryError::InvalidArgument(
            "--cache must be an absolute path".to_owned(),
        ));
    }
    reject_cache_components(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(_) => resolve_existing_cache(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(path.to_path_buf());
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| DeliveryError::io(parent, error))?;
            }
            match create_directory(path, 0o700) {
                Ok(()) => {}
                Err(error) if is_already_exists(&error) => {}
                Err(error) => return Err(error),
            }
            resolve_existing_cache(path)
        }
        Err(error) => Err(DeliveryError::io(path, error)),
    }
}

fn resolve_existing_cache(path: &Path) -> Result<PathBuf> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(DeliveryError::InvalidArgument(
            "--cache must be a directory (not a symlink or file)".to_owned(),
        ));
    }
    require_private_mode(&metadata, "--cache")?;
    let canonical = std::fs::canonicalize(path).map_err(|error| DeliveryError::io(path, error))?;
    reject_cache_components(&canonical)?;
    Ok(canonical)
}

fn reject_cache_components(path: &Path) -> Result<()> {
    let mut has_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                has_normal = true;
                if name == "versions" {
                    return Err(DeliveryError::InvalidArgument(
                        "--cache must not be inside a `versions/` directory".to_owned(),
                    ));
                }
            }
            Component::ParentDir => {
                return Err(DeliveryError::InvalidArgument(
                    "--cache must not contain `..` components".to_owned(),
                ));
            }
            _ => {}
        }
    }
    if !has_normal {
        return Err(DeliveryError::InvalidArgument(
            "--cache must be a directory below the filesystem root".to_owned(),
        ));
    }
    Ok(())
}

fn check_existing_private_subdir(cache: &Path, name: &str) -> Result<()> {
    let path = cache.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(DeliveryError::InvalidArgument(format!(
                    "cache `{name}` must be a directory (not a symlink or file)"
                )));
            }
            require_private_mode(&metadata, name)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeliveryError::io(&path, error)),
    }
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{}` must be a directory (not a symlink or file)",
                    crate::paths::redact(path)
                )));
            }
            require_private_mode(&metadata, &crate::paths::redact(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match create_directory(path, 0o700) {
                Ok(()) => Ok(()),
                Err(error) if is_already_exists(&error) => {
                    let metadata = std::fs::symlink_metadata(path)
                        .map_err(|error| DeliveryError::io(path, error))?;
                    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                        require_private_mode(&metadata, &crate::paths::redact(path))
                    } else {
                        Err(DeliveryError::InvalidArgument(format!(
                            "`{}` must be a directory (not a symlink or file)",
                            crate::paths::redact(path)
                        )))
                    }
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(DeliveryError::io(path, error)),
    }
}

pub(super) fn is_already_exists(error: &DeliveryError) -> bool {
    matches!(
        error,
        DeliveryError::Io { source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists
    )
}

fn require_private_mode(metadata: &std::fs::Metadata, label: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(DeliveryError::InvalidArgument(format!(
                "`{label}` must be private (mode 0700, no group/world access)"
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, label);
    }
    Ok(())
}

/// Creates a directory with an atomic mode and then normalizes it exactly.
pub(super) fn create_directory(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(mode);
        builder
            .create(path)
            .map_err(|error| DeliveryError::io(path, error))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path).map_err(|error| DeliveryError::io(path, error))?;
    }
    set_mode(path, mode)
}

fn no_source_error(asset: &Asset, detail: &str) -> DeliveryError {
    let name = source::tarball_file_name(asset).unwrap_or("the locked tarball");
    DeliveryError::InvalidArgument(format!(
        "asset `{}` has no verified cache entry and no usable source: {detail}; pass --from-dir <dir> containing `{name}` (network downloads are not implemented)",
        asset.name
    ))
}

fn cache_key(asset: &Asset, lock_sha256: &str) -> Result<String> {
    validate_sha256(&asset.payload.tree_digest)?;
    let prefix = lock_sha256.get(..16).ok_or_else(|| {
        DeliveryError::Integrity("assets lock SHA-256 is shorter than 16 characters".to_owned())
    })?;
    Ok(format!("{}.{prefix}", asset.payload.tree_digest))
}

fn cache_entry_dir(cache: &Path, asset: &Asset, lock_sha256: &str) -> Result<PathBuf> {
    validate_asset_name(asset)?;
    Ok(cache
        .join(CACHE_ASSETS_DIR)
        .join(&asset.name)
        .join(cache_key(asset, lock_sha256)?))
}

fn validate_asset_name(asset: &Asset) -> Result<()> {
    if asset.name.is_empty()
        || asset.name.len() > 64
        || asset
            .name
            .bytes()
            .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    {
        return Err(DeliveryError::InvalidPackage(
            "asset name is not a safe single cache directory component".to_owned(),
        ));
    }
    if asset.name.starts_with('-') || asset.name.ends_with('-') {
        return Err(DeliveryError::InvalidPackage(
            "asset name must not start or end with `-`".to_owned(),
        ));
    }
    validate_relative_path(&asset.name, false)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheState {
    Missing,
    Present,
    Invalid,
}

fn cache_entry_state(entry: &Path) -> Result<CacheState> {
    match std::fs::symlink_metadata(entry) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Ok(CacheState::Invalid);
            }
            match std::fs::symlink_metadata(entry.join(PROVENANCE_FILE)) {
                Ok(provenance) if provenance.file_type().is_file() => Ok(CacheState::Present),
                Ok(_) => Ok(CacheState::Invalid),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(CacheState::Invalid)
                }
                Err(error) => Err(DeliveryError::io(entry, error)),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CacheState::Missing),
        Err(error) => Err(DeliveryError::io(entry, error)),
    }
}
