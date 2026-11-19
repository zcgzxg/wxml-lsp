//! Schema v2 native package acquisition from an explicit local or remote source
//! plus a private, content-addressed native archive cache.
//!
//! Acquisition is deliberately separate from the transaction in
//! [`crate::native_install`]: it obtains an archive, verifies its SRI, SHA-256
//! and size against the selected release record, extracts it with the
//! formal-release rules, and hashes the whole tree before the caller can
//! activate it.
//!
//! Every verified archive is stored in `<install root>/cache/v2` so a later
//! `--offline` install, upgrade or rollback can run without the source tarball.
//! A cache hit is never trusted by directory name or metadata: the archive is
//! re-verified against the release record read from `index.json`. If the index
//! itself was stored by a previous explicit install, offline commands can use it
//! without a source. A missing or damaged entry fails closed and never falls
//! back to the network.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::archive::extract_tar_gz;
use crate::error::{DeliveryError, Result};
use crate::index_v2::{IndexV2, ReleaseV2};
use crate::install::InstallRoot;
use crate::manifest::{MAX_ARCHIVE_BYTES, MAX_INDEX_BYTES, PACKAGE_NAME, read_limited, set_mode};
use crate::manifest_v2::{
    ExpectedManifestV2, ManifestV2, NATIVE_PACKAGE_PLATFORM, read_manifest_v2,
};
use crate::paths::host_platform;
use crate::source::Source;
use crate::state::fsync_dir;
use crate::verify_v2::verify_tree_v2;

/// Cache subdirectory below the install root's `cache` directory.
pub const NATIVE_CACHE_SUBDIR: &str = "v2";
/// File name of the cached release index.
pub const NATIVE_CACHE_INDEX: &str = "index.json";
/// File name of one cached release archive.
pub const NATIVE_CACHE_ARCHIVE: &str = "archive.tar.gz";
/// Metadata file name inside one native archive cache entry.
pub const NATIVE_CACHE_ENTRY: &str = "entry.json";
/// Native archive cache metadata schema.
const NATIVE_CACHE_SCHEMA_VERSION: u32 = 1;
const MAX_CACHE_METADATA_BYTES: u64 = 64 * 1024;

/// What to select from the schema v2 index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcquireModeV2 {
    /// An exact release version.
    Exact(String),
    /// The newest stable release for this platform.
    Latest,
}

/// Inputs for one schema v2 acquisition.
#[derive(Clone, Debug)]
pub struct AcquireOptionsV2<'a> {
    /// Install root used for temporary staging and the archive cache.
    pub root: &'a InstallRoot,
    /// Explicit source. It may be absent for a cache hit; a cache miss without
    /// a source fails closed instead of touching the network.
    pub source: Option<&'a Source>,
    /// Already validated index read from the same source or the cached index.
    pub index: &'a IndexV2,
    /// Version selection.
    pub mode: AcquireModeV2,
    /// Whether a remote source may be contacted. A local source never needs it.
    pub allow_download: bool,
    /// Reject network access even if a remote source was supplied.
    pub offline: bool,
}

/// A verified schema v2 extraction tree plus its manifest and release record.
#[derive(Clone, Debug)]
pub struct AcquiredPackageV2 {
    /// Tree below the install root's `tmp/` directory.
    pub tree: PathBuf,
    /// Validated native manifest.
    pub manifest: ManifestV2,
    /// Selected release metadata.
    pub release: ReleaseV2,
    /// True when the verified archive came from the cache.
    pub cache_hit: bool,
}

/// Returns the private native archive cache directory for one install root.
#[must_use]
pub fn native_cache_dir(root: &InstallRoot) -> PathBuf {
    root.cache_dir().join(NATIVE_CACHE_SUBDIR)
}

/// Reads and validates the cached native `index.json`, if one exists.
///
/// The index is only a descriptor: archives are re-verified against it on every
/// hit. A malformed or tampered cached index fails closed so an offline install
/// never activates unverified bytes.
///
/// # Errors
///
/// Returns an error for an unsafe cache directory, a non-regular index file, an
/// oversized file or an index that fails schema/frozen-constant validation.
pub fn read_cached_native_index(root: &InstallRoot) -> Result<Option<Vec<u8>>> {
    let cache = root.cache_dir();
    match std::fs::symlink_metadata(&cache) {
        Ok(metadata) => require_real_directory(&metadata, "cache directory")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DeliveryError::io(&cache, error)),
    }
    let directory = native_cache_dir(root);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => {
            require_real_directory(&metadata, "native cache directory")?;
            require_private_mode(&metadata, "native cache directory")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DeliveryError::io(&directory, error)),
    }
    let path = directory.join(NATIVE_CACHE_INDEX);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            let bytes = read_limited(&path, MAX_INDEX_BYTES)?;
            crate::index_v2::parse_index_v2(&bytes)?;
            Ok(Some(bytes))
        }
        Ok(_) => Err(DeliveryError::InstallState(
            "cached native index is not a regular file".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DeliveryError::io(&path, error)),
    }
}

/// Atomically stores one validated native `index.json` for offline reuse.
///
/// The bytes are validated before anything is written. The cache directory is
/// private (`0700`) and the file is replaced atomically.
///
/// # Errors
///
/// Returns an error for an oversized/invalid index, an unsafe cache layout or an
/// I/O failure.
pub fn store_native_index(root: &InstallRoot, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_INDEX_BYTES {
        return Err(DeliveryError::SizeLimit(
            "native index is empty or exceeds the accepted size".to_owned(),
        ));
    }
    crate::index_v2::parse_index_v2(bytes)?;
    let directory = ensure_native_cache_dir(root)?;
    crate::manifest::atomic_write(&directory.join(NATIVE_CACHE_INDEX), bytes, 0o600)?;
    fsync_dir(&directory)
}

/// Selects and verifies one native release.
///
/// # Errors
///
/// Returns an error for an offline remote source, a disabled download, an
/// unsupported host, a missing/unstable release, a damaged cache entry, a cache
/// miss without a source, an unsafe archive, a digest/size mismatch, a failed
/// tree verification or a manifest that disagrees with the index.
pub fn acquire_v2(options: &AcquireOptionsV2<'_>) -> Result<AcquiredPackageV2> {
    let platform = host_platform()?;
    if platform != NATIVE_PACKAGE_PLATFORM {
        return Err(DeliveryError::UnsupportedPlatform(format!(
            "native packages require {NATIVE_PACKAGE_PLATFORM}, host is {platform}"
        )));
    }
    if options.offline && matches!(options.source, Some(Source::Remote(_))) {
        return Err(DeliveryError::InvalidArgument(
            "offline version selection cannot query a remote source; pass --from-dir".to_owned(),
        ));
    }
    if matches!(options.source, Some(Source::Remote(_))) && !options.allow_download {
        return Err(DeliveryError::InvalidArgument(
            "download is disabled; pass --enable-download to contact the source".to_owned(),
        ));
    }
    options.index.validate()?;
    let release = match &options.mode {
        AcquireModeV2::Exact(version) => options.index.find(version, &platform)?.clone(),
        AcquireModeV2::Latest => options.index.latest(&platform)?.clone(),
    };
    release.validate()?;

    let cache = NativeArchiveCache { root: options.root };
    let cached = cache.lookup(&release)?;
    let temporary = options.root.new_temp_dir("native")?;
    let result = (|| {
        let archive = temporary.join("archive.tar.gz");
        let cache_hit = if let Some(cached) = cached {
            copy_cached_archive(&cached, &archive)?;
            release.verify_archive(&archive)?;
            true
        } else {
            let source = options.source.ok_or_else(|| cache_miss(&release))?;
            if let Source::Local(directory) = source {
                if !directory.join(&release.archive).is_file() {
                    return Err(source_archive_missing(&release));
                }
            }
            source.fetch_archive_v2(&release, &archive)?;
            false
        };
        let tree = temporary.join("tree");
        extract_tar_gz(&archive, &tree)?;
        let report = verify_tree_v2(
            &tree,
            ExpectedManifestV2 {
                version: Some(&release.version),
                platform: Some(NATIVE_PACKAGE_PLATFORM),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        if !release.matches_manifest(&report.manifest) {
            return Err(DeliveryError::Integrity(
                "native manifest metadata does not match the release index".to_owned(),
            ));
        }
        // Re-read from disk so the caller gets exactly the verified bytes, not
        // a manifest value that could have been reconstructed in memory.
        let manifest = read_manifest_v2(
            &tree,
            ExpectedManifestV2 {
                version: Some(&release.version),
                platform: Some(NATIVE_PACKAGE_PLATFORM),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        if !cache_hit {
            cache.store(&release, &archive)?;
        }
        Ok(AcquiredPackageV2 {
            tree,
            manifest,
            release,
            cache_hit,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

fn cache_miss(release: &ReleaseV2) -> DeliveryError {
    DeliveryError::InstallState(format!(
        "native archive `{}` is not in the verified cache and no source tarball is available; pass --from-dir (offline installs never download)",
        release.archive
    ))
}

fn source_archive_missing(release: &ReleaseV2) -> DeliveryError {
    DeliveryError::InstallState(format!(
        "native archive `{}` is not in the verified cache and is missing from the source (I/O: no such file); pass --from-dir containing the release archive",
        release.archive
    ))
}

struct NativeArchiveCache<'a> {
    root: &'a InstallRoot,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeCacheEntry {
    schema_version: u32,
    package: String,
    version: String,
    platform: String,
    protocol_version: u64,
    engine_version: String,
    archive: String,
    archive_integrity: String,
    sha256: String,
    size: u64,
}

impl NativeArchiveCache<'_> {
    fn entry_dir(&self, release: &ReleaseV2) -> Result<PathBuf> {
        let prefix = release.sha256.get(..16).ok_or_else(|| {
            DeliveryError::Integrity("release SHA-256 is shorter than 16 characters".to_owned())
        })?;
        Ok(native_cache_dir(self.root)
            .join(format!("{}-{}-{prefix}", release.version, release.platform)))
    }

    /// Finds a cached archive and validates its metadata against the index.
    ///
    /// The returned path is not hashed here; the caller copies and re-verifies
    /// the bytes against the release record before extraction.
    fn lookup(&self, release: &ReleaseV2) -> Result<Option<PathBuf>> {
        let cache = self.root.cache_dir();
        match std::fs::symlink_metadata(&cache) {
            Ok(metadata) => require_real_directory(&metadata, "cache directory")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(DeliveryError::io(&cache, error)),
        }
        let directory = native_cache_dir(self.root);
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                require_real_directory(&metadata, "native cache directory")?;
                require_private_mode(&metadata, "native cache directory")?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(DeliveryError::io(&directory, error)),
        }
        let entry = self.entry_dir(release)?;
        match std::fs::symlink_metadata(&entry) {
            Ok(metadata) => {
                require_real_directory(&metadata, "native cache entry")?;
                require_private_mode(&metadata, "native cache entry")?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(DeliveryError::io(&entry, error)),
        }
        verify_entry_metadata(&entry, release)?;
        let archive = entry.join(NATIVE_CACHE_ARCHIVE);
        let metadata = match std::fs::symlink_metadata(&archive) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(DeliveryError::InstallState(
                    "native archive cache entry has no regular archive file".to_owned(),
                ));
            }
            Err(error) => return Err(DeliveryError::io(&archive, error)),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(DeliveryError::InstallState(
                "native archive cache entry has no regular archive file".to_owned(),
            ));
        }
        if metadata.len() != release.size {
            return Err(DeliveryError::Integrity(
                "native archive cache size does not match the release index".to_owned(),
            ));
        }
        Ok(Some(archive))
    }

    /// Publishes one already verified archive, or reuses a concurrent winner.
    fn store(&self, release: &ReleaseV2, archive: &Path) -> Result<()> {
        let entry = self.entry_dir(release)?;
        match std::fs::symlink_metadata(&entry) {
            Ok(_) => {
                return match self.lookup(release) {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => Err(DeliveryError::InstallState(
                        "native archive cache entry is incomplete".to_owned(),
                    )),
                    Err(error) => Err(error),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(DeliveryError::io(&entry, error)),
        }
        ensure_native_cache_dir(self.root)?;
        let workspace = self.root.new_temp_dir("native-cache")?;
        set_mode(&workspace, 0o700)?;
        let result = (|| {
            let staged = workspace.join(NATIVE_CACHE_ARCHIVE);
            copy_archive(archive, &staged)?;
            set_mode(&staged, 0o600)?;
            release.verify_archive(&staged)?;
            write_entry_metadata(&workspace, release)?;
            fsync_dir(&workspace)?;
            match std::fs::rename(&workspace, &entry) {
                Ok(()) => {
                    if let Some(parent) = entry.parent() {
                        fsync_dir(parent)?;
                    }
                    Ok(())
                }
                Err(error) => match self.lookup(release) {
                    Ok(Some(_)) => {
                        // A concurrent installer won the rename; its entry is
                        // complete and verified. Drop our duplicate staging
                        // directory instead of leaving it in tmp/.
                        let _ = std::fs::remove_dir_all(&workspace);
                        Ok(())
                    }
                    Ok(None) => Err(DeliveryError::io(&entry, error)),
                    Err(inner) => Err(inner),
                },
            }
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&workspace);
        }
        result
    }
}

fn verify_entry_metadata(entry: &Path, release: &ReleaseV2) -> Result<()> {
    let path = entry.join(NATIVE_CACHE_ENTRY);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(DeliveryError::InstallState(
            "native archive cache metadata is not a regular file".to_owned(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_CACHE_METADATA_BYTES {
        return Err(DeliveryError::InstallState(
            "native archive cache metadata is outside the accepted size".to_owned(),
        ));
    }
    let bytes = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
    let value: NativeCacheEntry = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::Integrity(format!(
            "native archive cache metadata is malformed: {error}"
        ))
    })?;
    if value.schema_version != NATIVE_CACHE_SCHEMA_VERSION {
        return Err(DeliveryError::InstallState(
            "native archive cache metadata schema is unsupported".to_owned(),
        ));
    }
    if value.package != PACKAGE_NAME
        || value.version != release.version
        || value.platform != release.platform
        || value.protocol_version != release.protocol_version
        || value.engine_version != release.engine_version
        || value.archive != release.archive
        || value.archive_integrity != release.archive_integrity
        || value.sha256 != release.sha256
        || value.size != release.size
    {
        return Err(DeliveryError::Integrity(
            "native archive cache metadata does not match the release index".to_owned(),
        ));
    }
    Ok(())
}

fn write_entry_metadata(entry: &Path, release: &ReleaseV2) -> Result<()> {
    let value = NativeCacheEntry {
        schema_version: NATIVE_CACHE_SCHEMA_VERSION,
        package: PACKAGE_NAME.to_owned(),
        version: release.version.clone(),
        platform: release.platform.clone(),
        protocol_version: release.protocol_version,
        engine_version: release.engine_version.clone(),
        archive: release.archive.clone(),
        archive_integrity: release.archive_integrity.clone(),
        sha256: release.sha256.clone(),
        size: release.size,
    };
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|error| {
        DeliveryError::Integrity(format!(
            "native archive cache metadata cannot be serialized: {error}"
        ))
    })?;
    bytes.push(b'\n');
    let path = entry.join(NATIVE_CACHE_ENTRY);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| DeliveryError::io(&path, error))?;
    file.write_all(&bytes)
        .map_err(|error| DeliveryError::io(&path, error))?;
    file.sync_all()
        .map_err(|error| DeliveryError::io(&path, error))?;
    drop(file);
    set_mode(&path, 0o644)
}

fn copy_archive(source: &Path, destination: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(source).map_err(|error| DeliveryError::io(source, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(
            "release archive is not a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(
            "release archive exceeds the size limit".to_owned(),
        ));
    }
    std::fs::copy(source, destination).map_err(|error| DeliveryError::io(destination, error))?;
    Ok(())
}

fn copy_cached_archive(source: &Path, destination: &Path) -> Result<()> {
    copy_archive(source, destination)?;
    set_mode(destination, 0o600)
}

fn ensure_native_cache_dir(root: &InstallRoot) -> Result<PathBuf> {
    let cache = root.cache_dir();
    match std::fs::symlink_metadata(&cache) {
        Ok(metadata) => require_real_directory(&metadata, "cache directory")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir(&cache)?;
        }
        Err(error) => return Err(DeliveryError::io(&cache, error)),
    }
    let directory = native_cache_dir(root);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => {
            require_real_directory(&metadata, "native cache directory")?;
            require_private_mode(&metadata, "native cache directory")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_dir(&directory)?;
        }
        Err(error) => return Err(DeliveryError::io(&directory, error)),
    }
    Ok(directory)
}

fn create_private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(DeliveryError::io(path, error)),
        }
    }
    #[cfg(not(unix))]
    {
        match std::fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(DeliveryError::io(path, error)),
        }
    }
    set_mode(path, 0o700)
}

fn require_real_directory(metadata: &std::fs::Metadata, label: &str) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(DeliveryError::InstallState(format!(
            "{label} must be a directory (not a symlink or file)"
        )));
    }
    Ok(())
}

fn require_private_mode(metadata: &std::fs::Metadata, label: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(DeliveryError::InstallState(format!(
                "{label} must be private (mode 0700, no group/world access)"
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, label);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-acquire-v2-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn fresh_root_has_no_cached_index() {
        let base = scratch("offline");
        let root = InstallRoot::create(&base.join("root")).unwrap();
        assert!(read_cached_native_index(&root).unwrap().is_none());
        assert!(native_cache_dir(&root).ends_with("cache/v2"));
        assert!(!root.cache_dir().exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
