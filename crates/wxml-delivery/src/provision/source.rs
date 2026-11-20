//! Tarball staging, source resolution and the private temporary workspace.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::assets_lock::Asset;
use crate::error::{DeliveryError, Result};
use crate::manifest::{MAX_ARCHIVE_BYTES, set_mode};
use crate::paths::validate_relative_path;
use crate::sri::verify_asset_file;

use super::{CACHE_TMP_DIR, CacheEntry, SourceKind, assemble, entry, extract};

const COPY_BUFFER_BYTES: usize = 64 * 1024;
const TMP_ATTEMPTS: u64 = 32;
static TMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Returns the exact tarball file name derived from `tarball.url`.
pub(super) fn tarball_file_name(asset: &Asset) -> Result<&str> {
    let name = asset
        .tarball
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            DeliveryError::InvalidPackage("assets lock tarball URL has no file name".to_owned())
        })?;
    validate_relative_path(name, false)?;
    if name.contains('/')
        || !Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tgz"))
    {
        return Err(DeliveryError::InvalidPackage(
            "assets lock tarball file name must be a single `.tgz` component".to_owned(),
        ));
    }
    Ok(name)
}

/// Validates and canonicalizes `--from-dir`.
pub(super) fn prepare_from_dir(path: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if !path.is_absolute() {
        return Err(DeliveryError::InvalidArgument(
            "--from-dir must be an absolute path".to_owned(),
        ));
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(DeliveryError::InvalidArgument(
            "--from-dir must be a directory (not a symlink or file)".to_owned(),
        ));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| DeliveryError::io(path, error))?;
    Ok(Some(canonical))
}

/// Finds the exact locked tarball below a canonical `--from-dir`.
pub(super) fn find_tarball(directory: &Path, asset: &Asset) -> Result<Option<PathBuf>> {
    let name = tarball_file_name(asset)?;
    let candidate = directory.join(name);
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{name}` in --from-dir must not be a symlink"
                )));
            }
            if !metadata.file_type().is_file() {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{name}` in --from-dir must be a regular file"
                )));
            }
            reject_hard_link(&metadata, name)?;
            if metadata.len() > MAX_ARCHIVE_BYTES {
                return Err(DeliveryError::SizeLimit(format!(
                    "asset archive exceeds {MAX_ARCHIVE_BYTES} bytes"
                )));
            }
            let canonical = std::fs::canonicalize(&candidate)
                .map_err(|error| DeliveryError::io(&candidate, error))?;
            if !canonical.starts_with(directory) {
                return Err(DeliveryError::InvalidArgument(format!(
                    "`{name}` in --from-dir escapes the directory"
                )));
            }
            Ok(Some(canonical))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DeliveryError::io(&candidate, error)),
    }
}

/// Runs P4-P8 for one asset and returns the resulting cache entry.
///
/// `origin` records how `tarball` was obtained. Whichever the origin, the
/// staged copy is verified against the lock and published to the archive cache
/// before extraction, so a later offline run does not need the source tarball.
#[allow(clippy::too_many_arguments)]
pub(super) fn provision_new_entry(
    asset: &Asset,
    cache: &Path,
    final_dir: &Path,
    tarball: &Path,
    file_name: &str,
    lock_sha256: &str,
    lock_bytes: &[u8],
    origin: SourceKind,
) -> Result<CacheEntry> {
    let workspace = TmpWorkspace::new(&cache.join(CACHE_TMP_DIR))?;
    let staged = workspace.path().join(file_name);
    stage_and_verify(tarball, &staged, asset)?;
    super::archive_cache::store(cache, asset, &staged)?;
    let extracted = workspace.path().join("extracted");
    let files = extract::extract_npm_tar_gz(&staged, &extracted, &asset.extract)?;
    extract::verify_upstream(asset, &files)?;
    let entry_dir = workspace.path().join("entry");
    let summary = assemble::assemble_payload(asset, &extracted, &entry_dir)?;
    entry::write_metadata(&entry_dir, asset, lock_sha256, lock_bytes, origin, &summary)?;
    match entry::publish(&entry_dir, final_dir)? {
        entry::Publish::Created => Ok(CacheEntry {
            name: asset.name.clone(),
            cache_key: super::cache_key(asset, lock_sha256)?,
            directory: final_dir.to_path_buf(),
            payload_tree_digest: summary.tree_digest,
            file_count: summary.file_count,
            total_bytes: summary.total_bytes,
            source: origin,
        }),
        entry::Publish::Existing => {
            assemble::verify_cache_entry(asset, final_dir, lock_sha256, lock_bytes)
        }
    }
}

/// Rejects a source archive that has another filesystem name.
fn reject_hard_link(metadata: &std::fs::Metadata, name: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(DeliveryError::InvalidArgument(format!(
                "`{name}` in --from-dir must not be a hard link"
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, name);
    }
    Ok(())
}

fn stage_and_verify(source: &Path, destination: &Path, asset: &Asset) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(source).map_err(|error| DeliveryError::io(source, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(
            "asset tarball must be a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "asset archive exceeds {MAX_ARCHIVE_BYTES} bytes"
        )));
    }
    let mut input = File::open(source).map_err(|error| DeliveryError::io(source, error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| DeliveryError::io(destination, error))?;
    let copied = copy_bounded(&mut input, &mut output, destination)?;
    output
        .sync_all()
        .map_err(|error| DeliveryError::io(destination, error))?;
    drop(output);
    set_mode(destination, 0o600)?;
    if copied > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "asset archive exceeds {MAX_ARCHIVE_BYTES} bytes"
        )));
    }
    verify_asset_file(
        destination,
        &asset.tarball.integrity,
        &asset.tarball.sha256,
        asset.tarball.size,
    )?;
    Ok(())
}

pub(super) fn copy_bounded(input: &mut File, output: &mut File, label: &Path) -> Result<u64> {
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut copied = 0_u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| DeliveryError::io(label, error))?;
        if read == 0 {
            return Ok(copied);
        }
        let read_u64 = u64::try_from(read)
            .map_err(|_| DeliveryError::SizeLimit("asset size does not fit u64".to_owned()))?;
        copied = copied
            .checked_add(read_u64)
            .ok_or_else(|| DeliveryError::SizeLimit("asset size overflows".to_owned()))?;
        if copied > MAX_ARCHIVE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "asset archive exceeds {MAX_ARCHIVE_BYTES} bytes"
            )));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| DeliveryError::io(label, error))?;
    }
}

/// A unique `<cache>/tmp/<nonce>` directory removed on drop.
pub(super) struct TmpWorkspace {
    path: PathBuf,
}

impl TmpWorkspace {
    pub(super) fn new(tmp_root: &Path) -> Result<Self> {
        for _ in 0..TMP_ATTEMPTS {
            let nonce = format!(
                "{}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_nanos()),
                TMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            let path = tmp_root.join(nonce);
            match super::create_directory(&path, 0o700) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if super::is_already_exists(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Err(DeliveryError::InstallState(
            "could not allocate a unique provisioning temporary directory".to_owned(),
        ))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
