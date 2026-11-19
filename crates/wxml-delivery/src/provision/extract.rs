//! Safe npm-tarball extraction and upstream-table verification (P6-P7).
//!
//! This extractor is deliberately separate from [`crate::archive`]: npm
//! packages use a `package/` prefix and may carry `0664` modes, while the
//! formal release extractor requires an exact allowlist and `0644/0755` only.
//! The safety rules are otherwise the same: no links, devices, traversal,
//! duplicate paths, case-folding collisions or oversized extension headers.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::{Archive, Entry};

use crate::assets_lock::{Asset, AssetExtract};
use crate::error::{DeliveryError, Result};
use crate::manifest::{
    FileEntry, MAX_ARCHIVE_BYTES, MAX_EXTRACTED_BYTES, MAX_FILE_BYTES, MAX_FILES,
};
use crate::paths::{MAX_PATH_BYTES, validate_relative_path};
use crate::type_assets::glob_matches;

use super::prescan::{self, BoundedReader, MAX_DECOMPRESSED_BYTES};

const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Extracts a npm tarball into a new directory and returns the normalized
/// upstream file table.
///
/// Every entry must live below a single top-level `package/` directory. Files
/// listed in `extract.executables` are written `0755`; all other files are
/// `0644` and directories are `0755`. The destination must not exist and is
/// removed on failure.
///
/// # Errors
///
/// Returns [`DeliveryError::UnsafeArchive`] for unsafe paths/types, and
/// [`DeliveryError::SizeLimit`] when a fixed archive budget is exceeded.
pub fn extract_npm_tar_gz(
    archive: &Path,
    destination: &Path,
    extract: &AssetExtract,
) -> Result<Vec<FileEntry>> {
    let executables = validated_executables(extract)?;
    let metadata =
        std::fs::symlink_metadata(archive).map_err(|error| DeliveryError::io(archive, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::UnsafeArchive(
            "archive path is not a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "asset archive exceeds {MAX_ARCHIVE_BYTES} bytes"
        )));
    }
    if destination.exists() {
        return Err(DeliveryError::InstallState(
            "extraction destination already exists".to_owned(),
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
    }
    std::fs::create_dir(destination).map_err(|error| DeliveryError::io(destination, error))?;
    let result = extract_inner(archive, destination, &executables);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn validated_executables(extract: &AssetExtract) -> Result<BTreeSet<&str>> {
    let mut executables = BTreeSet::new();
    for path in &extract.executables {
        validate_relative_path(path, false)?;
        if !executables.insert(path.as_str()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock extract.executables repeats `{path}`"
            )));
        }
    }
    Ok(executables)
}

fn extract_inner(
    archive: &Path,
    destination: &Path,
    executables: &BTreeSet<&str>,
) -> Result<Vec<FileEntry>> {
    let mut file = File::open(archive).map_err(|error| DeliveryError::io(archive, error))?;
    prescan::prescan_tar(BoundedReader::new(
        GzDecoder::new(&mut file),
        MAX_DECOMPRESSED_BYTES,
    ))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| DeliveryError::io(archive, error))?;
    let decoder = GzDecoder::new(file);
    let mut tar = Archive::new(BoundedReader::new(decoder, MAX_DECOMPRESSED_BYTES));
    let mut state = ExtractState::default();
    for entry in tar
        .entries()
        .map_err(|error| DeliveryError::io(archive, error))?
    {
        state.entry_count += 1;
        if state.entry_count > MAX_FILES.saturating_mul(2) {
            return Err(DeliveryError::SizeLimit(
                "archive contains too many entries".to_owned(),
            ));
        }
        let mut entry = entry.map_err(|error| DeliveryError::io(archive, error))?;
        process_entry(&mut entry, archive, destination, executables, &mut state)?;
    }
    if state.files.is_empty() {
        return Err(DeliveryError::UnsafeArchive(
            "archive contains no files below `package/`".to_owned(),
        ));
    }
    Ok(state.files.into_values().collect())
}

#[derive(Default)]
struct ExtractState {
    entry_count: usize,
    files: BTreeMap<String, FileEntry>,
    directories: BTreeSet<String>,
    folded: BTreeMap<String, FoldedKind>,
    total_bytes: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FoldedKind {
    File,
    Directory,
}

fn process_entry<R: Read>(
    entry: &mut Entry<'_, R>,
    archive: &Path,
    destination: &Path,
    executables: &BTreeSet<&str>,
    state: &mut ExtractState,
) -> Result<()> {
    let kind = entry.header().entry_type();
    let raw_path = entry.path_bytes().into_owned();
    let text = std::str::from_utf8(&raw_path)
        .map_err(|_| DeliveryError::UnsafeArchive("archive path is not valid UTF-8".to_owned()))?;
    let Some(relative) = strip_npm_prefix(text, kind.is_dir())? else {
        return Ok(());
    };
    if relative.len() > MAX_PATH_BYTES {
        return Err(DeliveryError::SizeLimit(
            "archive path exceeds the path-length limit".to_owned(),
        ));
    }
    validate_relative_path(relative, false)?;
    classify_folded(state, relative, kind.is_dir())?;
    if kind.is_dir() {
        let mode = entry
            .header()
            .mode()
            .map_err(|error| DeliveryError::io(archive, error))?;
        if mode & 0o7000 != 0 {
            return Err(DeliveryError::UnsafeArchive(format!(
                "directory `{relative}` has setuid, setgid or sticky bits"
            )));
        }
        if mode & 0o022 != 0 {
            return Err(DeliveryError::UnsafeArchive(format!(
                "directory `{relative}` is group- or world-writable"
            )));
        }
        return extract_directory(destination, relative, state);
    }
    if !kind.is_file() {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{relative}` is not a regular file or directory"
        )));
    }
    extract_file(entry, archive, destination, relative, executables, state)
}

fn strip_npm_prefix(text: &str, is_dir: bool) -> Result<Option<&str>> {
    let text = if is_dir {
        text.strip_suffix('/').unwrap_or(text)
    } else {
        text
    };
    if text == "package" {
        return if is_dir {
            Ok(None)
        } else {
            Err(DeliveryError::UnsafeArchive(
                "archive top-level `package` must be a directory".to_owned(),
            ))
        };
    }
    let Some(relative) = text.strip_prefix("package/") else {
        return Err(DeliveryError::UnsafeArchive(format!(
            "archive entry `{text}` is outside the `package/` prefix"
        )));
    };
    if relative.is_empty() {
        return Err(DeliveryError::UnsafeArchive(
            "archive entry has an empty path below `package/`".to_owned(),
        ));
    }
    Ok(Some(relative))
}

fn classify_folded(state: &mut ExtractState, relative: &str, is_dir: bool) -> Result<()> {
    for (index, _) in relative.match_indices('/') {
        let Some(prefix) = relative.get(..index) else {
            continue;
        };
        let folded = prefix.to_ascii_lowercase();
        if state.folded.get(&folded) == Some(&FoldedKind::File) {
            return Err(DeliveryError::UnsafeArchive(format!(
                "archive path `{relative}` is nested below a file at `{prefix}`"
            )));
        }
        state.folded.insert(folded, FoldedKind::Directory);
    }
    let folded = relative.to_ascii_lowercase();
    match (state.folded.get(&folded).copied(), is_dir) {
        (None, true) => {
            state.folded.insert(folded, FoldedKind::Directory);
            Ok(())
        }
        (None, false) => {
            state.folded.insert(folded, FoldedKind::File);
            Ok(())
        }
        (Some(FoldedKind::Directory), true) => Ok(()),
        (Some(FoldedKind::Directory), false) => Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{relative}` is both a file and a directory"
        ))),
        (Some(FoldedKind::File), _) => Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{relative}` collides with another path by case folding"
        ))),
    }
}

fn extract_directory(destination: &Path, relative: &str, state: &mut ExtractState) -> Result<()> {
    if state.files.contains_key(relative) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{relative}` is both a file and a directory"
        )));
    }
    if !state.directories.insert(relative.to_owned()) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "duplicate archive directory `{relative}`"
        )));
    }
    ensure_parents(destination, Path::new(relative), &state.files)?;
    let path = destination.join(relative);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{relative}` is not a directory"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&path).map_err(|error| DeliveryError::io(&path, error))?;
            crate::manifest::set_mode(&path, 0o755)
        }
        Err(error) => Err(DeliveryError::io(&path, error)),
    }
}

fn extract_file<R: Read>(
    entry: &mut Entry<'_, R>,
    archive: &Path,
    destination: &Path,
    relative: &str,
    executables: &BTreeSet<&str>,
    state: &mut ExtractState,
) -> Result<()> {
    let size = checked_file_size(entry, archive, relative, state)?;
    write_file_entry(entry, destination, relative, executables, size, state)
}

fn checked_file_size<R: Read>(
    entry: &Entry<'_, R>,
    archive: &Path,
    relative: &str,
    state: &mut ExtractState,
) -> Result<u64> {
    let header = entry.header();
    let mode = header
        .mode()
        .map_err(|error| DeliveryError::io(archive, error))?;
    if mode & 0o7000 != 0 {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{relative}` has setuid, setgid or sticky bits"
        )));
    }
    let size = entry.size();
    if size
        != header
            .size()
            .map_err(|error| DeliveryError::io(archive, error))?
    {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{relative}` has conflicting sizes"
        )));
    }
    if size > MAX_FILE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "entry `{relative}` exceeds the per-file limit"
        )));
    }
    if state.directories.contains(relative) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{relative}` is both a file and a directory"
        )));
    }
    if state.files.contains_key(relative) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "duplicate archive path `{relative}`"
        )));
    }
    charge_total_bytes(state, size)?;
    Ok(size)
}

fn charge_total_bytes(state: &mut ExtractState, size: u64) -> Result<()> {
    state.total_bytes = state
        .total_bytes
        .checked_add(size)
        .ok_or_else(|| DeliveryError::SizeLimit("archive sizes overflow".to_owned()))?;
    if state.total_bytes > MAX_EXTRACTED_BYTES {
        return Err(DeliveryError::SizeLimit(
            "archive exceeds the total extracted-size limit".to_owned(),
        ));
    }
    Ok(())
}

fn write_file_entry<R: Read>(
    entry: &mut Entry<'_, R>,
    destination: &Path,
    relative: &str,
    executables: &BTreeSet<&str>,
    size: u64,
    state: &mut ExtractState,
) -> Result<()> {
    let relative_path = PathBuf::from(relative);
    ensure_parents(destination, &relative_path, &state.files)?;
    let destination_path = destination.join(&relative_path);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination_path)
        .map_err(|error| DeliveryError::io(&destination_path, error))?;
    let (sha256, written) = write_hashed(entry, &mut output, &destination_path, size)?;
    output
        .sync_all()
        .map_err(|error| DeliveryError::io(&destination_path, error))?;
    drop(output);
    if written != size {
        return Err(DeliveryError::Integrity(format!(
            "entry `{relative}` was truncated or oversized"
        )));
    }
    let normalized = if executables.contains(relative) {
        0o755
    } else {
        0o644
    };
    crate::manifest::set_mode(&destination_path, normalized)?;
    state.files.insert(
        relative.to_owned(),
        FileEntry {
            path: relative.to_owned(),
            sha256,
            size,
            mode: format!("{normalized:04o}"),
        },
    );
    Ok(())
}

fn write_hashed<R: Read>(
    entry: &mut Entry<'_, R>,
    output: &mut File,
    label: &Path,
    expected: u64,
) -> Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut written = 0_u64;
    loop {
        let read = entry
            .read(&mut buffer)
            .map_err(|error| DeliveryError::io(label, error))?;
        if read == 0 {
            break;
        }
        written = written
            .checked_add(u64::try_from(read).unwrap_or(0))
            .ok_or_else(|| DeliveryError::SizeLimit("entry size overflows".to_owned()))?;
        if written > expected {
            return Err(DeliveryError::Integrity(
                "archive entry is larger than its header".to_owned(),
            ));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| DeliveryError::io(label, error))?;
        hasher.update(&buffer[..read]);
    }
    Ok((hex_digest(&hasher.finalize()), written))
}

fn ensure_parents(
    destination: &Path,
    relative: &Path,
    files: &BTreeMap<String, FileEntry>,
) -> Result<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut accumulated = PathBuf::new();
    for component in parent.components() {
        accumulated.push(component.as_os_str());
        let text = accumulated.to_string_lossy().into_owned();
        if files.contains_key(&text) {
            return Err(DeliveryError::UnsafeArchive(format!(
                "parent `{text}` is a regular file"
            )));
        }
        let path = destination.join(&accumulated);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(DeliveryError::UnsafeArchive(format!(
                    "parent `{text}` is not a directory"
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&path).map_err(|error| DeliveryError::io(&path, error))?;
                crate::manifest::set_mode(&path, 0o755)?;
            }
            Err(error) => return Err(DeliveryError::io(&path, error)),
        }
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Verifies that the extracted tree equals the lock's complete upstream table
/// and that `include`/`exclude` classify every upstream file exactly once.
pub(super) fn verify_upstream(asset: &Asset, extracted: &[FileEntry]) -> Result<()> {
    let (included, excluded) = classify(asset)?;
    verify_upstream_table(asset, extracted)?;
    verify_upstream_classification(extracted, &included, &excluded)?;
    verify_upstream_bytes(asset, extracted)
}

fn verify_upstream_table(asset: &Asset, extracted: &[FileEntry]) -> Result<()> {
    if extracted.len() != asset.upstream.files.len() {
        return Err(DeliveryError::Integrity(format!(
            "extracted upstream file count {} does not match the lock count {}",
            extracted.len(),
            asset.upstream.files.len()
        )));
    }
    let actual: BTreeMap<&str, &FileEntry> = extracted
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    for expected in &asset.upstream.files {
        let Some(file) = actual.get(expected.path.as_str()) else {
            return Err(DeliveryError::Integrity(format!(
                "extracted upstream tree is missing `{}`",
                expected.path
            )));
        };
        if file.sha256 != expected.sha256
            || file.size != expected.size
            || file.mode != expected.mode
        {
            return Err(DeliveryError::Integrity(format!(
                "extracted upstream `{}` does not match the lock",
                expected.path
            )));
        }
    }
    if actual.len() != asset.upstream.files.len() {
        return Err(DeliveryError::Integrity(
            "extracted upstream tree contains files that are not in the lock".to_owned(),
        ));
    }
    Ok(())
}

fn verify_upstream_classification(
    extracted: &[FileEntry],
    included: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> Result<()> {
    for file in extracted {
        let is_included = included.contains(&file.path);
        let is_excluded = excluded.contains(&file.path);
        if is_included == is_excluded {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock does not classify upstream file `{}`",
                file.path
            )));
        }
    }
    Ok(())
}

fn verify_upstream_bytes(asset: &Asset, extracted: &[FileEntry]) -> Result<()> {
    let total = extracted.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(file.size)
            .ok_or_else(|| DeliveryError::SizeLimit("upstream bytes overflow".to_owned()))
    })?;
    if total != asset.upstream.unpacked_size {
        return Err(DeliveryError::Integrity(
            "extracted upstream bytes do not match the lock".to_owned(),
        ));
    }
    Ok(())
}

/// Returns the include-selected and exclude-selected upstream paths.
pub(super) fn classify(asset: &Asset) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let excluded = validate_exclusions(asset)?;
    let mut included = BTreeSet::new();
    for file in &asset.upstream.files {
        let matched = asset
            .extract
            .include
            .iter()
            .any(|pattern| glob_matches(pattern, &file.path));
        let is_excluded = excluded.contains(&file.path);
        match (matched, is_excluded) {
            (true, false) => {
                included.insert(file.path.clone());
            }
            (false, true) => {}
            (true, true) => {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock classifies `{}` as both included and excluded",
                    file.path
                )));
            }
            (false, false) => {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock does not classify upstream file `{}`",
                    file.path
                )));
            }
        }
    }
    Ok((included, excluded))
}

fn validate_exclusions(asset: &Asset) -> Result<BTreeSet<String>> {
    let mut excluded = BTreeSet::new();
    for exclusion in &asset.extract.exclude {
        validate_relative_path(&exclusion.path, false)?;
        if !excluded.insert(exclusion.path.clone()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock extract.exclude repeats `{}`",
                exclusion.path
            )));
        }
        if !asset
            .upstream
            .files
            .iter()
            .any(|file| file.path == exclusion.path)
        {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock extract.exclude `{}` is not an upstream file",
                exclusion.path
            )));
        }
    }
    Ok(excluded)
}
