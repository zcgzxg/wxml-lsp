//! Cache entry metadata, provenance and atomic publication (P8).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::assets_lock::Asset;
use crate::error::{DeliveryError, Result};
use crate::manifest::set_mode as set_file_mode;

use super::assemble::{self, PayloadSummary};
use super::{
    CacheState, LOCK_COPY_FILE, MAX_PROVENANCE_BYTES, PROVENANCE_FILE, SHA256SUMS_FILE, SourceKind,
    cache_entry_state,
};

const PROVENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheProvenance {
    schema_version: u32,
    lock_sha256: String,
    asset_name: String,
    tarball: ProvenanceTarball,
    payload_tree_digest: String,
    file_count: u64,
    total_bytes: u64,
    source_kind: SourceKind,
    tool_version: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProvenanceTarball {
    url: String,
    size: u64,
    integrity: String,
    sha256: String,
}

/// Writes `provenance.json`, the submitted lock and the payload checksums.
pub(super) fn write_metadata(
    entry_dir: &Path,
    asset: &Asset,
    lock_sha256: &str,
    lock_bytes: &[u8],
    source: SourceKind,
    summary: &PayloadSummary,
) -> Result<()> {
    let provenance = CacheProvenance {
        schema_version: PROVENANCE_SCHEMA_VERSION,
        lock_sha256: lock_sha256.to_owned(),
        asset_name: asset.name.clone(),
        tarball: ProvenanceTarball {
            url: asset.tarball.url.clone(),
            size: asset.tarball.size,
            integrity: asset.tarball.integrity.clone(),
            sha256: asset.tarball.sha256.clone(),
        },
        payload_tree_digest: summary.tree_digest.clone(),
        file_count: summary.file_count,
        total_bytes: summary.total_bytes,
        source_kind: source,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let mut bytes = serde_json::to_vec_pretty(&provenance).map_err(|error| {
        DeliveryError::Integrity(format!("cache provenance cannot be serialized: {error}"))
    })?;
    bytes.push(b'\n');
    write_new_file(&entry_dir.join(PROVENANCE_FILE), &bytes, 0o644)?;
    write_new_file(&entry_dir.join(LOCK_COPY_FILE), lock_bytes, 0o644)?;
    write_new_file(
        &entry_dir.join(SHA256SUMS_FILE),
        &assemble::payload_checksums(asset),
        0o644,
    )?;
    sync_directory(entry_dir)
}

/// Reads and validates the P9 provenance fields without modifying the entry.
pub(super) fn read_provenance(entry_dir: &Path, asset: &Asset, lock_sha256: &str) -> Result<()> {
    let path = entry_dir.join(PROVENANCE_FILE);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_PROVENANCE_BYTES {
        return Err(DeliveryError::Integrity(
            "assets cache provenance is not a bounded regular file".to_owned(),
        ));
    }
    let bytes = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
    let provenance: CacheProvenance = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::Integrity(format!("assets cache provenance is malformed: {error}"))
    })?;
    if provenance.schema_version != PROVENANCE_SCHEMA_VERSION {
        return Err(DeliveryError::Integrity(
            "assets cache provenance schema is unsupported".to_owned(),
        ));
    }
    if provenance.lock_sha256 != lock_sha256 {
        return Err(DeliveryError::InstallState(
            "assets cache entry belongs to a different lock".to_owned(),
        ));
    }
    if provenance.asset_name != asset.name
        || provenance.payload_tree_digest != asset.payload.tree_digest
        || provenance.file_count != asset.payload.file_count
        || provenance.total_bytes != asset.payload.total_bytes
        || provenance.tarball.url != asset.tarball.url
        || provenance.tarball.size != asset.tarball.size
        || provenance.tarball.integrity != asset.tarball.integrity
        || provenance.tarball.sha256 != asset.tarball.sha256
        || provenance.tool_version.is_empty()
    {
        return Err(DeliveryError::Integrity(
            "assets cache provenance does not match the lock".to_owned(),
        ));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| DeliveryError::io(path, error))?;
    file.write_all(bytes)
        .map_err(|error| DeliveryError::io(path, error))?;
    file.sync_all()
        .map_err(|error| DeliveryError::io(path, error))?;
    drop(file);
    set_file_mode(path, mode)
}

fn sync_directory(path: &Path) -> Result<()> {
    let directory = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    directory
        .sync_all()
        .map_err(|error| DeliveryError::io(path, error))
}

/// Outcome of publishing a completed entry.
pub(super) enum Publish {
    Created,
    Existing,
}

/// Atomically renames a completed entry into the cache.
///
/// A concurrent provisioner may have produced the same content-addressed
/// directory first. That is not an error: the existing entry is re-verified by
/// the caller (P9) and never patched in place.
pub(super) fn publish(entry_dir: &Path, final_dir: &Path) -> Result<Publish> {
    let parent = final_dir.parent().ok_or_else(|| {
        DeliveryError::InvalidArgument("cache entry path has no parent".to_owned())
    })?;
    super::ensure_private_dir(parent)?;
    match cache_entry_state(final_dir)? {
        CacheState::Present => return Ok(Publish::Existing),
        CacheState::Invalid => return Err(super::invalid_cache_entry()),
        CacheState::Missing => {}
    }
    match std::fs::rename(entry_dir, final_dir) {
        Ok(()) => {
            sync_directory(parent)?;
            Ok(Publish::Created)
        }
        Err(error) => match cache_entry_state(final_dir) {
            Ok(CacheState::Present) => Ok(Publish::Existing),
            Ok(_) => Err(DeliveryError::io(final_dir, error)),
            Err(inner) => Err(inner),
        },
    }
}
