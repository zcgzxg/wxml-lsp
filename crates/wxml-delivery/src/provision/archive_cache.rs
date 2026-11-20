//! Content-addressed cache of verified npm tarballs (P8a).
//!
//! An entry lives at `archives/<name>/<sha256>-<size>/` and contains the exact
//! tarball bytes plus `archive.json`. The directory name is only a lookup key:
//! every read copies the tarball into the provisioning workspace and re-verifies
//! size, SHA-256 and SHA-512 SRI against the submitted `assets.lock.json`.
//!
//! Writes happen in `<cache>/tmp` and are published with one atomic rename.
//! Concurrent provisioners either observe no entry or a complete entry; an
//! existing entry is re-verified and never patched in place.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::assets_lock::Asset;
use crate::error::{DeliveryError, Result};
use crate::manifest::set_mode;
use crate::paths::validate_sha256;

use super::{
    CACHE_TMP_DIR, SourceKind, ensure_private_dir, invalid_cache_entry, require_private_mode,
    validate_asset_name,
};

/// Directory below the cache root holding verified tarballs.
pub const CACHE_ARCHIVES_DIR: &str = "archives";
/// Tarball file name inside one archive cache entry.
pub(super) const ARCHIVE_FILE: &str = "archive.tgz";
/// Metadata file name inside one archive cache entry.
pub(super) const ARCHIVE_METADATA_FILE: &str = "archive.json";
/// Maximum accepted archive metadata size.
const MAX_METADATA_BYTES: u64 = 16 * 1024;
const METADATA_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveMetadata {
    schema_version: u32,
    asset_name: String,
    asset_version: String,
    url: String,
    integrity: String,
    sha256: String,
    size: u64,
    archive: String,
}

/// Returns `Some(ArchiveCache)` when a structurally valid entry is present.
///
/// The tarball bytes are not hashed here; the provisioning pipeline stages and
/// verifies them against the lock before any extraction.
pub(super) fn available(cache: &Path, asset: &Asset) -> Result<Option<SourceKind>> {
    Ok(present(cache, asset)?.map(|_| SourceKind::ArchiveCache))
}

/// Structurally validates one archive cache entry and returns its tarball.
///
/// A missing cache or entry is `None`. A symlink, foreign metadata, unsafe mode
/// or wrong size is a fail-closed error: a damaged cache is never treated as a
/// miss and never repaired in place.
pub(super) fn present(cache: &Path, asset: &Asset) -> Result<Option<PathBuf>> {
    let root = cache.join(CACHE_ARCHIVES_DIR);
    match std::fs::symlink_metadata(&root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(DeliveryError::InvalidArgument(
                    "cache `archives` must be a directory (not a symlink or file)".to_owned(),
                ));
            }
            require_private_mode(&metadata, CACHE_ARCHIVES_DIR)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DeliveryError::io(&root, error)),
    }
    let entry = entry_dir(cache, asset)?;
    match std::fs::symlink_metadata(&entry) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(invalid_cache_entry());
            }
            require_private_mode(&metadata, "archive cache entry")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DeliveryError::io(&entry, error)),
    }
    verify_metadata(&entry, asset)?;
    let archive = entry.join(ARCHIVE_FILE);
    let metadata =
        std::fs::symlink_metadata(&archive).map_err(|error| DeliveryError::io(&archive, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(DeliveryError::Integrity(
            "asset archive cache tarball is not a regular file".to_owned(),
        ));
    }
    if metadata.len() != asset.tarball.size {
        return Err(DeliveryError::Integrity(format!(
            "asset archive cache size {} does not match the lock's {} bytes",
            metadata.len(),
            asset.tarball.size
        )));
    }
    Ok(Some(archive))
}

/// Copies one already verified tarball into the archive cache.
///
/// The copy is re-verified against the lock before publication. If a concurrent
/// provisioner published the same content-addressed entry first, the existing
/// entry is re-checked and reused. The call never rewrites an existing entry.
pub(super) fn store(cache: &Path, asset: &Asset, verified: &Path) -> Result<()> {
    let entry = entry_dir(cache, asset)?;
    match std::fs::symlink_metadata(&entry) {
        Ok(_) => {
            return match present(cache, asset)? {
                Some(_) => Ok(()),
                None => Err(invalid_cache_entry()),
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DeliveryError::io(&entry, error)),
    }
    let parent = entry.parent().ok_or_else(|| {
        DeliveryError::InvalidArgument("archive cache entry path has no parent".to_owned())
    })?;
    ensure_private_dir(&cache.join(CACHE_ARCHIVES_DIR))?;
    ensure_private_dir(parent)?;

    let workspace = super::source::TmpWorkspace::new(&cache.join(CACHE_TMP_DIR))?;
    let staged = workspace.path().join(ARCHIVE_FILE);
    let result = (|| {
        copy_verified(verified, &staged, asset)?;
        write_metadata(workspace.path(), asset)?;
        sync_directory(workspace.path())?;
        match std::fs::rename(workspace.path(), &entry) {
            Ok(()) => sync_directory(parent),
            Err(error) => {
                // A concurrent provisioner may have published the same entry.
                match present(cache, asset) {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => Err(DeliveryError::io(&entry, error)),
                    Err(inner) => Err(inner),
                }
            }
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(workspace.path());
    }
    result
}

fn entry_dir(cache: &Path, asset: &Asset) -> Result<PathBuf> {
    validate_asset_name(asset)?;
    validate_sha256(&asset.tarball.sha256)?;
    if asset.tarball.size == 0 || asset.tarball.size > crate::manifest::MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(
            "asset archive cache size is outside the accepted range".to_owned(),
        ));
    }
    Ok(cache
        .join(CACHE_ARCHIVES_DIR)
        .join(&asset.name)
        .join(format!("{}-{}", asset.tarball.sha256, asset.tarball.size)))
}

fn verify_metadata(entry: &Path, asset: &Asset) -> Result<()> {
    let path = entry.join(ARCHIVE_METADATA_FILE);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(DeliveryError::Integrity(
            "asset archive cache metadata is not a regular file".to_owned(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_METADATA_BYTES {
        return Err(DeliveryError::Integrity(
            "asset archive cache metadata is outside the accepted size".to_owned(),
        ));
    }
    let bytes = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
    let value: ArchiveMetadata = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::Integrity(format!(
            "asset archive cache metadata is malformed: {error}"
        ))
    })?;
    if value.schema_version != METADATA_SCHEMA_VERSION {
        return Err(DeliveryError::Integrity(
            "asset archive cache metadata schema is unsupported".to_owned(),
        ));
    }
    if value.asset_name != asset.name
        || value.asset_version != asset.version
        || value.url != asset.tarball.url
        || value.integrity != asset.tarball.integrity
        || value.sha256 != asset.tarball.sha256
        || value.size != asset.tarball.size
        || value.archive != ARCHIVE_FILE
    {
        return Err(DeliveryError::Integrity(
            "asset archive cache metadata does not match the assets lock".to_owned(),
        ));
    }
    Ok(())
}

fn copy_verified(source: &Path, destination: &Path, asset: &Asset) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(source).map_err(|error| DeliveryError::io(source, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(
            "staged asset tarball is not a regular file".to_owned(),
        ));
    }
    if metadata.len() != asset.tarball.size {
        return Err(DeliveryError::Integrity(
            "staged asset tarball size changed before caching".to_owned(),
        ));
    }
    let mut input = File::open(source).map_err(|error| DeliveryError::io(source, error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| DeliveryError::io(destination, error))?;
    super::source::copy_bounded(&mut input, &mut output, destination)?;
    output
        .sync_all()
        .map_err(|error| DeliveryError::io(destination, error))?;
    drop(output);
    set_mode(destination, 0o600)?;
    crate::sri::verify_asset_file(
        destination,
        &asset.tarball.integrity,
        &asset.tarball.sha256,
        asset.tarball.size,
    )?;
    Ok(())
}

fn write_metadata(entry: &Path, asset: &Asset) -> Result<()> {
    let value = ArchiveMetadata {
        schema_version: METADATA_SCHEMA_VERSION,
        asset_name: asset.name.clone(),
        asset_version: asset.version.clone(),
        url: asset.tarball.url.clone(),
        integrity: asset.tarball.integrity.clone(),
        sha256: asset.tarball.sha256.clone(),
        size: asset.tarball.size,
        archive: ARCHIVE_FILE.to_owned(),
    };
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|error| {
        DeliveryError::Integrity(format!(
            "archive cache metadata cannot be serialized: {error}"
        ))
    })?;
    bytes.push(b'\n');
    let path = entry.join(ARCHIVE_METADATA_FILE);
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

fn sync_directory(path: &Path) -> Result<()> {
    let directory = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    directory
        .sync_all()
        .map_err(|error| DeliveryError::io(path, error))
}
