//! Payload assembly and P9 cache re-verification.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::assets_lock::{Asset, canonical_tree_digest};
use crate::error::{DeliveryError, Result};
use crate::manifest::{
    FileEntry, MAX_FILES, format_sha256sums, parse_sha256sums, set_mode, sha256_file,
};
use crate::paths::validate_relative_path;
use crate::type_assets::compute_type_asset_bundle;

use super::entry;
use super::extract;
use super::{
    CacheEntry, LOCK_COPY_FILE, MAX_ASSETS_LOCK_BYTES, MAX_CHECKSUMS_BYTES, SHA256SUMS_FILE,
    SourceKind, cache_key,
};

/// Verified payload identity used for provenance and the returned entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PayloadSummary {
    pub(super) tree_digest: String,
    pub(super) file_count: u64,
    pub(super) total_bytes: u64,
}

/// Assembles the exact lock payload below `<entry>/payload/`.
pub(super) fn assemble_payload(
    asset: &Asset,
    extracted_root: &Path,
    entry_dir: &Path,
) -> Result<PayloadSummary> {
    let (included, _excluded) = extract::classify(asset)?;
    let selected = payload_relative_set(asset)?;
    if selected != included {
        return Err(DeliveryError::Integrity(
            "payload file set does not match `extract.include` minus `extract.exclude`".to_owned(),
        ));
    }
    ensure_directory(entry_dir, 0o700)?;
    let payload_root = entry_dir.join("payload");
    ensure_directory(&payload_root, 0o755)?;
    for file in &asset.payload.files {
        copy_payload_file(asset, extracted_root, &payload_root, file)?;
    }
    let summary = verify_payload_layout(asset, &payload_root)?;
    sync_directories(&payload_root)?;
    Ok(summary)
}

fn sync_directories(root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let handle = std::fs::File::open(&directory)
            .map_err(|error| DeliveryError::io(&directory, error))?;
        handle
            .sync_all()
            .map_err(|error| DeliveryError::io(&directory, error))?;
        for entry in
            std::fs::read_dir(&directory).map_err(|error| DeliveryError::io(&directory, error))?
        {
            let entry = entry.map_err(|error| DeliveryError::io(&directory, error))?;
            if entry
                .file_type()
                .map_err(|error| DeliveryError::io(&directory, error))?
                .is_dir()
            {
                stack.push(entry.path());
            }
        }
    }
    Ok(())
}

fn payload_relative_set(asset: &Asset) -> Result<BTreeSet<String>> {
    let prefix = format!("{}/", asset.package_root);
    let mut selected = BTreeSet::new();
    for file in &asset.payload.files {
        let Some(relative) = file.path.strip_prefix(&prefix) else {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock payload `{}` is outside `{}`",
                file.path, asset.package_root
            )));
        };
        if relative.is_empty() {
            return Err(DeliveryError::InvalidPackage(
                "assets lock payload has an empty package-relative path".to_owned(),
            ));
        }
        validate_relative_path(relative, false)?;
        if !selected.insert(relative.to_owned()) {
            return Err(DeliveryError::InvalidPackage(
                "assets lock payload repeats a package-relative path".to_owned(),
            ));
        }
    }
    Ok(selected)
}

fn copy_payload_file(
    asset: &Asset,
    extracted_root: &Path,
    payload_root: &Path,
    file: &FileEntry,
) -> Result<()> {
    let prefix = format!("{}/", asset.package_root);
    let relative = file.path.strip_prefix(&prefix).ok_or_else(|| {
        DeliveryError::InvalidPackage(format!(
            "assets lock payload `{}` is outside `{}`",
            file.path, asset.package_root
        ))
    })?;
    let source = extracted_root.join(relative);
    let metadata =
        std::fs::symlink_metadata(&source).map_err(|error| DeliveryError::io(&source, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::Integrity(format!(
            "extracted upstream `{relative}` is not a regular file"
        )));
    }
    if metadata.len() != file.size || sha256_file(&source)? != file.sha256 {
        return Err(DeliveryError::Integrity(format!(
            "extracted upstream `{relative}` does not match the payload table"
        )));
    }
    let destination = payload_root.join(&file.path);
    ensure_parents(payload_root, &file.path)?;
    std::fs::copy(&source, &destination).map_err(|error| DeliveryError::io(&destination, error))?;
    set_mode(&destination, file.mode_bits()?)?;
    std::fs::File::open(&destination)
        .and_then(|file| file.sync_all())
        .map_err(|error| DeliveryError::io(&destination, error))?;
    let copied = std::fs::symlink_metadata(&destination)
        .map_err(|error| DeliveryError::io(&destination, error))?;
    if !copied.file_type().is_file()
        || copied.len() != file.size
        || sha256_file(&destination)? != file.sha256
    {
        return Err(DeliveryError::Integrity(format!(
            "payload `{}` changed while copying",
            file.path
        )));
    }
    Ok(())
}

fn ensure_parents(root: &Path, path: &str) -> Result<()> {
    let mut accumulated = PathBuf::new();
    let components: Vec<&str> = path.split('/').collect();
    let Some((_, parents)) = components.split_last() else {
        return Ok(());
    };
    for component in parents {
        accumulated.push(component);
        ensure_directory(&root.join(&accumulated), 0o755)?;
    }
    Ok(())
}

fn ensure_directory(path: &Path, mode: u32) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(DeliveryError::UnsafeArchive(format!(
            "`{}` is not a directory",
            crate::paths::redact(path)
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            super::create_directory(path, mode)
        }
        Err(error) => Err(DeliveryError::io(path, error)),
    }
}

/// Recomputes and checks the complete payload tree against the lock table.
fn verify_payload_layout(asset: &Asset, payload_root: &Path) -> Result<PayloadSummary> {
    let walk = walk_tree(payload_root)?;
    if walk.directories != expected_directories(asset) {
        return Err(DeliveryError::Integrity(
            "payload directory set does not match the lock".to_owned(),
        ));
    }
    verify_payload_files(asset, &walk.files)?;
    let tree_digest = canonical_tree_digest(&walk.files, &asset.package_root)?;
    if tree_digest != asset.payload.tree_digest {
        return Err(DeliveryError::Integrity(
            "payload tree digest does not match the lock".to_owned(),
        ));
    }
    let (file_count, total_bytes) = payload_totals(&walk.files)?;
    if file_count != asset.payload.file_count || total_bytes != asset.payload.total_bytes {
        return Err(DeliveryError::Integrity(
            "payload counts do not match the lock".to_owned(),
        ));
    }
    verify_bundles(asset, &walk.files)?;
    Ok(PayloadSummary {
        tree_digest,
        file_count,
        total_bytes,
    })
}

fn verify_payload_files(asset: &Asset, files: &[FileEntry]) -> Result<()> {
    let expected: BTreeMap<&str, &FileEntry> = asset
        .payload
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    if files.len() != expected.len() {
        return Err(DeliveryError::Integrity(
            "payload file set does not match the lock".to_owned(),
        ));
    }
    for file in files {
        let Some(locked) = expected.get(file.path.as_str()) else {
            return Err(DeliveryError::Integrity(format!(
                "payload `{}` is not in the lock",
                file.path
            )));
        };
        if locked.sha256 != file.sha256 || locked.size != file.size || locked.mode != file.mode {
            return Err(DeliveryError::Integrity(format!(
                "payload `{}` does not match the lock",
                file.path
            )));
        }
    }
    Ok(())
}

fn payload_totals(files: &[FileEntry]) -> Result<(u64, u64)> {
    let total_bytes = files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.size)
            .ok_or_else(|| DeliveryError::SizeLimit("payload bytes overflow".to_owned()))
    })?;
    let file_count = u64::try_from(files.len())
        .map_err(|_| DeliveryError::SizeLimit("payload file count overflows".to_owned()))?;
    Ok((file_count, total_bytes))
}

fn expected_directories(asset: &Asset) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for file in &asset.payload.files {
        for (index, _) in file.path.match_indices('/') {
            let Some(directory) = file.path.get(..index) else {
                continue;
            };
            directories.insert(directory.to_owned());
        }
    }
    directories
}

fn verify_bundles(asset: &Asset, files: &[FileEntry]) -> Result<()> {
    for bundle in &asset.payload.bundles {
        let computed = compute_type_asset_bundle(files, &bundle.root, &bundle.include)?;
        if computed.file_count != bundle.file_count
            || computed.total_bytes != bundle.total_bytes
            || computed.sha256 != bundle.sha256
        {
            return Err(DeliveryError::Integrity(format!(
                "type asset bundle `{}` does not match the payload",
                bundle.name
            )));
        }
    }
    Ok(())
}

#[derive(Default)]
struct TreeWalk {
    directories: BTreeSet<String>,
    files: Vec<FileEntry>,
}

fn walk_tree(root: &Path) -> Result<TreeWalk> {
    let mut walk = TreeWalk::default();
    let mut seen = 0_usize;
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let directory = root.join(&relative);
        let entries =
            std::fs::read_dir(&directory).map_err(|error| DeliveryError::io(&directory, error))?;
        for entry in entries {
            seen += 1;
            if seen > MAX_FILES.saturating_mul(2) {
                return Err(DeliveryError::SizeLimit(
                    "payload contains too many entries".to_owned(),
                ));
            }
            let entry = entry.map_err(|error| DeliveryError::io(&directory, error))?;
            visit_entry(&entry, &relative, &mut walk, &mut stack)?;
        }
    }
    walk.files
        .sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    Ok(walk)
}

fn visit_entry(
    entry: &std::fs::DirEntry,
    relative: &Path,
    walk: &mut TreeWalk,
    stack: &mut Vec<PathBuf>,
) -> Result<()> {
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return Err(DeliveryError::UnsafeArchive(
            "payload path is not valid UTF-8".to_owned(),
        ));
    };
    let child = if relative.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        relative.join(name)
    };
    let text = child.to_string_lossy().into_owned();
    validate_relative_path(&text, false)?;
    let metadata = std::fs::symlink_metadata(entry.path())
        .map_err(|error| DeliveryError::io(&entry.path(), error))?;
    if metadata.file_type().is_symlink() {
        return Err(DeliveryError::UnsafeArchive(format!(
            "payload path `{text}` is a symlink"
        )));
    }
    if metadata.file_type().is_dir() {
        checked_mode(&metadata, true, &text)?;
        walk.directories.insert(text);
        stack.push(child);
    } else if metadata.file_type().is_file() {
        let mode = checked_mode(&metadata, false, &text)?;
        walk.files.push(FileEntry {
            path: text,
            sha256: sha256_file(&entry.path())?,
            size: metadata.len(),
            mode: format!("{mode:04o}"),
        });
    } else {
        return Err(DeliveryError::UnsafeArchive(format!(
            "payload path `{text}` is not a regular file or directory"
        )));
    }
    Ok(())
}

fn checked_mode(metadata: &std::fs::Metadata, directory: bool, path: &str) -> Result<u32> {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let mode = {
        let _ = metadata;
        if directory { 0o755 } else { 0o644 }
    };
    if mode & 0o7000 != 0 {
        return Err(DeliveryError::Integrity(format!(
            "payload `{path}` has setuid, setgid or sticky bits"
        )));
    }
    if mode & 0o022 != 0 {
        return Err(DeliveryError::Integrity(format!(
            "payload `{path}` is group- or world-writable"
        )));
    }
    if directory {
        if mode != 0o755 {
            return Err(DeliveryError::Integrity(format!(
                "payload directory `{path}` has an unexpected mode"
            )));
        }
    } else if !matches!(mode, 0o644 | 0o755) {
        return Err(DeliveryError::Integrity(format!(
            "payload file `{path}` has an unexpected mode"
        )));
    }
    Ok(mode)
}

/// Re-verifies a cache entry (P9) without modifying it.
pub(super) fn verify_cache_entry(
    asset: &Asset,
    entry_dir: &Path,
    lock_sha256: &str,
    lock_bytes: &[u8],
) -> Result<CacheEntry> {
    entry::read_provenance(entry_dir, asset, lock_sha256)?;
    verify_lock_copy(entry_dir, lock_bytes)?;
    verify_checksums(asset, entry_dir)?;
    let summary = verify_payload_layout(asset, &entry_dir.join("payload"))?;
    Ok(CacheEntry {
        name: asset.name.clone(),
        cache_key: cache_key(asset, lock_sha256)?,
        directory: entry_dir.to_path_buf(),
        payload_tree_digest: summary.tree_digest,
        file_count: summary.file_count,
        total_bytes: summary.total_bytes,
        source: SourceKind::Cache,
    })
}

fn verify_lock_copy(entry_dir: &Path, lock_bytes: &[u8]) -> Result<()> {
    let path = entry_dir.join(LOCK_COPY_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeliveryError::Integrity(
                "assets cache entry is missing its lock copy".to_owned(),
            ));
        }
        Err(error) => return Err(DeliveryError::io(&path, error)),
    };
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(DeliveryError::Integrity(
            "assets cache entry is missing its lock copy".to_owned(),
        ));
    }
    if metadata.len() > MAX_ASSETS_LOCK_BYTES {
        return Err(DeliveryError::Integrity(
            "assets cache lock copy exceeds the accepted size".to_owned(),
        ));
    }
    if metadata.len() != u64::try_from(lock_bytes.len()).unwrap_or(u64::MAX) {
        return Err(DeliveryError::Integrity(
            "assets cache lock copy does not match the submitted lock".to_owned(),
        ));
    }
    let stored = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
    if stored != lock_bytes {
        return Err(DeliveryError::Integrity(
            "assets cache lock copy does not match the submitted lock".to_owned(),
        ));
    }
    Ok(())
}

fn verify_checksums(asset: &Asset, entry_dir: &Path) -> Result<()> {
    let path = entry_dir.join(SHA256SUMS_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeliveryError::Integrity(
                "assets cache entry is missing SHA256SUMS".to_owned(),
            ));
        }
        Err(error) => return Err(DeliveryError::io(&path, error)),
    };
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::Integrity(
            "assets cache entry is missing SHA256SUMS".to_owned(),
        ));
    }
    if metadata.len() > MAX_CHECKSUMS_BYTES {
        return Err(DeliveryError::Integrity(
            "assets cache SHA256SUMS exceeds the accepted size".to_owned(),
        ));
    }
    let bytes = std::fs::read(&path).map_err(|error| DeliveryError::io(&path, error))?;
    let actual = parse_sha256sums(&bytes)?;
    let expected: BTreeMap<String, String> = asset
        .payload
        .files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect();
    if actual != expected {
        return Err(DeliveryError::Integrity(
            "assets cache SHA256SUMS does not match the lock".to_owned(),
        ));
    }
    Ok(())
}

/// Formats the payload-only checksum file written by P8.
pub(super) fn payload_checksums(asset: &Asset) -> Vec<u8> {
    let entries: BTreeMap<String, String> = asset
        .payload
        .files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect();
    format_sha256sums(&entries)
}
