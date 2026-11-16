//! Verified download cache.
//!
//! Cache entries are only created after the archive and its manifest have
//! passed every integrity check. Offline installs re-check the archive hash and
//! then the full extracted tree, so a tampered cache never becomes a runtime.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DeliveryError, Result};
use crate::manifest::{
    PACKAGE_NAME, Release, SCHEMA_VERSION, SUPPORTED_PROTOCOL_VERSIONS, read_limited, sha256_file,
};
use crate::paths::{validate_platform, validate_sha256, validate_version};
use crate::semver::{Semver, compare_versions, is_prerelease};

/// Maximum accepted cache metadata file.
const MAX_ENTRY_BYTES: u64 = 64 * 1024;

/// How the cached archive was originally obtained.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// HTTPS source.
    Https,
    /// Loopback HTTP test source.
    LoopbackHttp,
    /// Explicit local release directory.
    LocalDirectory,
}

impl SourceKind {
    /// Derives the kind from a validated source.
    #[must_use]
    pub fn from_source(source: &crate::source::Source) -> Self {
        match source {
            crate::source::Source::Remote(url) if url.scheme() == "https" => Self::Https,
            crate::source::Source::Remote(_) => Self::LoopbackHttp,
            crate::source::Source::Local(_) => Self::LocalDirectory,
        }
    }
}

/// Metadata stored next to one cached archive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEntry {
    /// Cache schema.
    pub schema_version: u32,
    /// Package identity.
    pub package: String,
    /// Release version.
    pub version: String,
    /// Target platform.
    pub platform: String,
    /// Type-bridge protocol version.
    pub protocol_version: u64,
    /// Minimum Node.js version.
    pub node_minimum_version: String,
    /// Archive SHA-256.
    pub sha256: String,
    /// Archive size.
    pub size: u64,
    /// Archive file name inside the entry directory.
    pub archive: String,
    /// Original source class, used to enforce the trusted-source policy offline.
    pub source_kind: SourceKind,
    /// Set only after full package verification.
    pub verified: bool,
    /// Verification timestamp (UTC, RFC 3339).
    pub verified_at: String,
}

impl CacheEntry {
    /// Builds an entry from a validated release.
    #[must_use]
    pub fn from_release(release: &Release, source_kind: SourceKind, verified_at: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            package: PACKAGE_NAME.to_owned(),
            version: release.version.clone(),
            platform: release.platform.clone(),
            protocol_version: release.protocol_version,
            node_minimum_version: release.node_minimum_version.clone(),
            sha256: release.sha256.clone(),
            size: release.size,
            archive: "archive.tar.gz".to_owned(),
            source_kind,
            verified: true,
            verified_at,
        }
    }

    /// Validates cache metadata without touching the archive.
    ///
    /// # Errors
    ///
    /// Returns an error for schema, package, version, hash, size or source
    /// mismatches.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(DeliveryError::InstallState(
                "unsupported cache entry schema".to_owned(),
            ));
        }
        if self.package != PACKAGE_NAME {
            return Err(DeliveryError::InstallState(
                "cache entry belongs to another package".to_owned(),
            ));
        }
        validate_version(&self.version)?;
        validate_platform(&self.platform)?;
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&self.protocol_version) {
            return Err(DeliveryError::InstallState(
                "cache entry has an unsupported protocol".to_owned(),
            ));
        }
        Semver::parse(&self.node_minimum_version)?;
        validate_sha256(&self.sha256)?;
        if self.size == 0 || self.size > crate::manifest::MAX_ARCHIVE_BYTES {
            return Err(DeliveryError::InstallState(
                "cache entry has an invalid archive size".to_owned(),
            ));
        }
        if self.archive != "archive.tar.gz" {
            return Err(DeliveryError::InstallState(
                "cache entry has an unexpected archive name".to_owned(),
            ));
        }
        if !self.verified {
            return Err(DeliveryError::InstallState(
                "cache entry was never verified".to_owned(),
            ));
        }
        if self.verified_at.is_empty() || self.verified_at.len() > 64 {
            return Err(DeliveryError::InstallState(
                "cache entry has an invalid verification time".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns the directory name for this entry.
    #[must_use]
    pub fn directory_name(&self) -> String {
        format!(
            "{}-{}-{}",
            self.version,
            self.platform,
            &self.sha256[..16.min(self.sha256.len())]
        )
    }
}

/// Ensures the cache root exists.
///
/// # Errors
///
/// Returns an error when the directory cannot be created.
pub fn ensure_cache_root(root: &Path) -> Result<()> {
    let existed = root.exists();
    std::fs::create_dir_all(root).map_err(|error| DeliveryError::io(root, error))?;
    if !existed {
        crate::manifest::set_mode(root, 0o755)?;
    }
    Ok(())
}

/// Stores a fully verified archive in the cache atomically.
///
/// # Errors
///
/// Returns an error for invalid metadata, an archive hash mismatch or I/O
/// failures.
pub fn store_verified(
    cache_root: &Path,
    archive_source: &Path,
    entry: &CacheEntry,
) -> Result<PathBuf> {
    entry.validate()?;
    ensure_cache_root(cache_root)?;
    let expected_size = std::fs::metadata(archive_source)
        .map_err(|error| DeliveryError::io(archive_source, error))?
        .len();
    if expected_size != entry.size {
        return Err(DeliveryError::Integrity(
            "archive size changed before caching".to_owned(),
        ));
    }
    if sha256_file(archive_source)? != entry.sha256 {
        return Err(DeliveryError::Integrity(
            "archive hash changed before caching".to_owned(),
        ));
    }

    let final_dir = cache_root.join(entry.directory_name());
    if final_dir.exists() {
        if load_verified(cache_root, entry).is_ok() {
            return Ok(final_dir.join(&entry.archive));
        }
        std::fs::remove_dir_all(&final_dir)
            .map_err(|error| DeliveryError::io(&final_dir, error))?;
    }

    let temporary = cache_root.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    std::fs::create_dir(&temporary).map_err(|error| DeliveryError::io(&temporary, error))?;
    let result = (|| {
        let archive_path = temporary.join(&entry.archive);
        std::fs::copy(archive_source, &archive_path)
            .map_err(|error| DeliveryError::io(&archive_path, error))?;
        crate::manifest::set_mode(&archive_path, 0o644)?;
        let metadata = serde_json::to_vec_pretty(entry)
            .map_err(|error| DeliveryError::InvalidPackage(error.to_string()))?;
        crate::manifest::atomic_write(&temporary.join("entry.json"), &metadata, 0o644)?;
        std::fs::rename(&temporary, &final_dir)
            .map_err(|error| DeliveryError::io(&final_dir, error))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(error);
    }
    Ok(final_dir.join(&entry.archive))
}

/// Loads and hash-checks one cache entry.
///
/// # Errors
///
/// Returns an error when metadata is malformed, the archive is missing or the
/// stored bytes no longer match the recorded hash.
pub fn load_verified(cache_root: &Path, entry: &CacheEntry) -> Result<PathBuf> {
    entry.validate()?;
    let directory = cache_root.join(entry.directory_name());
    let archive = directory.join(&entry.archive);
    let metadata =
        std::fs::symlink_metadata(&archive).map_err(|error| DeliveryError::io(&archive, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InstallState(
            "cached archive is not a regular file".to_owned(),
        ));
    }
    if metadata.len() != entry.size {
        return Err(DeliveryError::InstallState(
            "cached archive size does not match its metadata".to_owned(),
        ));
    }
    if sha256_file(&archive)? != entry.sha256 {
        return Err(DeliveryError::InstallState(
            "cached archive hash does not match its metadata".to_owned(),
        ));
    }
    Ok(archive)
}

/// Reads all structurally valid cache entries for one platform.
///
/// Corrupt or unrelated entries are skipped; a caller that asked for a specific
/// version reports the difference separately.
///
/// # Errors
///
/// Returns an error when the cache root cannot be read.
pub fn list_entries(cache_root: &Path, platform: &str) -> Result<Vec<CacheEntry>> {
    let entries = match std::fs::read_dir(cache_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DeliveryError::io(cache_root, error)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(cache_root, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if !metadata.file_type().is_dir() {
            continue;
        }
        let entry_path = path.join("entry.json");
        let Ok(metadata) = std::fs::symlink_metadata(&entry_path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let Ok(bytes) = read_limited(&entry_path, MAX_ENTRY_BYTES) else {
            continue;
        };
        let Ok(cached) = serde_json::from_slice::<CacheEntry>(&bytes) else {
            continue;
        };
        if cached.validate().is_err() || cached.platform != platform {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) != Some(cached.directory_name().as_str())
        {
            continue;
        }
        found.push(cached);
    }
    found.sort_by(|left, right| compare_versions(&left.version, &right.version));
    Ok(found)
}

/// Finds a cached release version.
///
/// # Errors
///
/// Returns an error when the cache cannot be read. A missing entry is `None`.
pub fn find_version(
    cache_root: &Path,
    platform: &str,
    version: &str,
    expected_sha256: Option<&str>,
) -> Result<Option<CacheEntry>> {
    let entries = list_entries(cache_root, platform)?;
    Ok(entries
        .into_iter()
        .filter(|entry| entry.version == version)
        .filter(|entry| expected_sha256.is_none_or(|sha| entry.sha256 == sha))
        .max_by(|left, right| left.verified_at.cmp(&right.verified_at)))
}

/// Returns the newest verified cached version for a platform.
///
/// # Errors
///
/// Returns an error when the cache cannot be read.
pub fn latest(cache_root: &Path, platform: &str) -> Result<Option<CacheEntry>> {
    let entries = list_entries(cache_root, platform)?;
    Ok(entries
        .into_iter()
        .filter(|entry| !is_prerelease(&entry.version))
        .max_by(|left, right| {
            compare_versions(&left.version, &right.version)
                .then_with(|| left.verified_at.cmp(&right.verified_at))
        }))
}

/// Removes one cache entry under the caller's install lock.
///
/// # Errors
///
/// Returns an error when the cache directory cannot be removed.
pub fn remove(cache_root: &Path, entry: &CacheEntry) -> Result<()> {
    entry.validate()?;
    let directory = cache_root.join(entry.directory_name());
    match std::fs::remove_dir_all(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeliveryError::io(&directory, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::sha256_bytes;

    fn entries() -> Vec<CacheEntry> {
        ["0.1.0", "0.2.0"]
            .into_iter()
            .map(|version| CacheEntry {
                schema_version: SCHEMA_VERSION,
                package: PACKAGE_NAME.to_owned(),
                version: version.to_owned(),
                platform: "linux-x86_64".to_owned(),
                protocol_version: 2,
                node_minimum_version: "24.0.0".to_owned(),
                sha256: sha256_bytes(version.as_bytes()),
                size: version.len() as u64,
                archive: "archive.tar.gz".to_owned(),
                source_kind: SourceKind::Https,
                verified: true,
                verified_at: format!("2026-01-0{}T00:00:00Z", version.len()),
            })
            .collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-cache-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn stores_loads_and_orders_verified_entries() {
        let base = scratch("store");
        let cache = base.join("cache");
        for entry in entries() {
            let source = base.join(format!("{}.tar.gz", entry.version));
            std::fs::write(&source, entry.version.as_bytes()).unwrap();
            store_verified(&cache, &source, &entry).unwrap();
            let loaded = load_verified(&cache, &entry).unwrap();
            assert_eq!(std::fs::read(loaded).unwrap(), entry.version.as_bytes());
        }
        let found = find_version(&cache, "linux-x86_64", "0.1.0", None)
            .unwrap()
            .unwrap();
        assert_eq!(found.version, "0.1.0");
        assert_eq!(
            latest(&cache, "linux-x86_64").unwrap().unwrap().version,
            "0.2.0"
        );
        assert_eq!(list_entries(&cache, "linux-x86_64").unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn detects_corruption_and_removes_entries() {
        let base = scratch("corrupt");
        let cache = base.join("cache");
        let entry = entries().remove(0);
        let source = base.join("archive");
        std::fs::write(&source, b"0.1.0").unwrap();
        let stored = store_verified(&cache, &source, &entry).unwrap();
        std::fs::write(&stored, b"tampered").unwrap();
        assert!(load_verified(&cache, &entry).is_err());
        remove(&cache, &entry).unwrap();
        assert!(
            find_version(&cache, "linux-x86_64", "0.1.0", None)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn list_entries_skips_malformed_and_linked_metadata() {
        let base = scratch("malformed");
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("not-a-directory"), b"x").unwrap();
        let malformed = cache.join("malformed");
        std::fs::create_dir_all(&malformed).unwrap();
        std::fs::write(malformed.join("entry.json"), b"{").unwrap();
        let linked = cache.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", linked.join("entry.json")).unwrap();
        assert_eq!(list_entries(&cache, "linux-x86_64").unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_unverified_entries() {
        let mut entry = entries().remove(0);
        entry.verified = false;
        assert!(entry.validate().is_err());
    }
}
