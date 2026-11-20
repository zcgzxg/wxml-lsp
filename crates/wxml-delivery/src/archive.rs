//! Deterministic archive creation and hardened extraction.
//!
//! The installer treats archives as hostile input. Only regular files and
//! directories are accepted; links and device nodes are always rejected, and
//! every path is validated before anything is created below the extraction
//! root.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tar::{Archive, Builder, Entry, EntryType, Header};

use crate::error::{DeliveryError, Result};
use crate::manifest::{
    FileEntry, MAX_ARCHIVE_BYTES, MAX_EXTRACTED_BYTES, MAX_FILE_BYTES, MAX_FILES, set_mode,
};
use crate::paths::{MAX_PATH_BYTES, validate_relative_path};

/// One regular file written during extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedFile {
    /// Validated package-relative path.
    pub path: String,
    /// Declared and observed size.
    pub size: u64,
}

/// Result of a successful extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractReport {
    /// Regular files in stable path order.
    pub files: Vec<ExtractedFile>,
    /// Total extracted file bytes.
    pub total_bytes: u64,
}

/// Creates a deterministic gzip-compressed tar archive from `entries`.
///
/// Header ownership, timestamps and modes are fixed from the manifest, so the
/// same payload and `mtime` produce byte-identical archives.
///
/// # Errors
///
/// Returns an error when a payload file is missing, changed size, has an
/// unsupported mode or the archive cannot be written.
pub fn create_tar_gz(root: &Path, archive: &Path, entries: &[FileEntry], mtime: u64) -> Result<()> {
    let parent = archive
        .parent()
        .ok_or_else(|| DeliveryError::InvalidArgument("archive path has no parent".to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
    let temporary = archive.with_extension(format!("tmp-{}", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| DeliveryError::io(&temporary, error))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);

    let result = (|| {
        for entry in entries {
            validate_relative_path(&entry.path, true)?;
            let path = root.join(&entry.path);
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| DeliveryError::io(&path, error))?;
            if !metadata.file_type().is_file() {
                return Err(DeliveryError::InvalidPackage(format!(
                    "payload `{}` is not a regular file",
                    entry.path
                )));
            }
            if metadata.len() != entry.size {
                return Err(DeliveryError::Integrity(format!(
                    "payload `{}` changed size after hashing",
                    entry.path
                )));
            }
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(entry.size);
            header.set_mode(entry.mode_bits()?);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(mtime);
            let mut input = File::open(&path).map_err(|error| DeliveryError::io(&path, error))?;
            builder
                .append_data(&mut header, &entry.path, &mut input)
                .map_err(|error| DeliveryError::io(&path, error))?;
        }
        builder
            .finish()
            .map_err(|error| DeliveryError::io(&temporary, error))?;
        let encoder = builder
            .into_inner()
            .map_err(|error| DeliveryError::io(&temporary, error))?;
        encoder
            .finish()
            .map_err(|error| DeliveryError::io(&temporary, error))?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    set_mode(&temporary, 0o644)?;
    std::fs::rename(&temporary, archive).map_err(|error| DeliveryError::io(archive, error))
}

/// Upper bound for one GNU/PAX extension header (long name, link or PAX
/// record). Real paths are tiny; a large value here is an archive attack.
const MAX_EXTENSION_BYTES: u64 = 64 * 1024;

/// Hard budget for decompressed tar bytes, including headers and padding.
const MAX_DECOMPRESSED_BYTES: u64 = MAX_EXTRACTED_BYTES + 64 * 1024 * 1024;

const TAR_BLOCK: usize = 512;

/// A `Read` adapter that fails once the decompressed budget is exhausted.
struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> BoundedReader<R> {
    const fn new(inner: R, remaining: u64) -> Self {
        Self { inner, remaining }
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("decompressed size limit exceeded"));
        }
        let maximum = buffer
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buffer[..maximum])?;
        self.remaining -= u64::try_from(read).unwrap_or(0);
        Ok(read)
    }
}

/// Scans tar headers without materialising extension payloads.
///
/// `tar-rs` reads GNU long-name and PAX extension headers into a `Vec` before
/// returning an entry. A crafted archive can therefore claim a multi-gigabyte
/// extension header and exhaust memory before our entry loop runs. This
/// streaming pre-scan rejects oversized extension headers and conflicting PAX
/// sizes before the real extractor is allowed to see them.
fn prescan_tar(mut reader: impl Read) -> Result<()> {
    let mut total_files = 0_usize;
    let mut total_bytes = 0_u64;
    let mut pending_pax_size = None;

    loop {
        let Some(header) = read_header(&mut reader)? else {
            return Ok(());
        };
        if header.iter().all(|byte| *byte == 0) {
            return Ok(());
        }
        let kind = header[156];
        let raw_size = parse_tar_size(&header[124..136])?;
        if kind == b'S' {
            return Err(DeliveryError::UnsafeArchive(
                "GNU sparse tar entries are not accepted".to_owned(),
            ));
        }
        let is_extension = matches!(kind, b'L' | b'K' | b'x' | b'g');
        if is_extension && raw_size > MAX_EXTENSION_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "tar extension header declares {raw_size} bytes"
            )));
        }
        if kind == b'x' {
            let content = read_exact_vec(&mut reader, raw_size)?;
            pending_pax_size = parse_pax_size(&content)?;
            skip_padding(&mut reader, raw_size)?;
            continue;
        }
        if is_extension {
            skip_padded(&mut reader, raw_size)?;
            continue;
        }

        total_files += 1;
        if total_files > MAX_FILES.saturating_mul(2) {
            return Err(DeliveryError::SizeLimit(
                "archive contains too many entries".to_owned(),
            ));
        }
        let effective_size = match pending_pax_size.take() {
            Some(pax_size) if pax_size != raw_size => {
                return Err(DeliveryError::UnsafeArchive(
                    "PAX size overrides the tar header size".to_owned(),
                ));
            }
            Some(pax_size) => pax_size,
            None => raw_size,
        };
        if effective_size > MAX_FILE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "entry declares {effective_size} bytes, above the per-file limit"
            )));
        }
        total_bytes = total_bytes
            .checked_add(effective_size)
            .ok_or_else(|| DeliveryError::SizeLimit("archive sizes overflow".to_owned()))?;
        if total_bytes > MAX_EXTRACTED_BYTES {
            return Err(DeliveryError::SizeLimit(
                "archive exceeds the total extracted-size limit".to_owned(),
            ));
        }
        skip_padded(&mut reader, effective_size)?;
    }
}

fn read_header(reader: &mut impl Read) -> Result<Option<[u8; TAR_BLOCK]>> {
    let mut header = [0_u8; TAR_BLOCK];
    let mut read = 0;
    while read < TAR_BLOCK {
        match reader.read(&mut header[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => {
                return Err(DeliveryError::Integrity(
                    "tar header was truncated".to_owned(),
                ));
            }
            Ok(count) => read += count,
            Err(error) => return Err(DeliveryError::io_label("archive", error)),
        }
    }
    Ok(Some(header))
}

fn read_exact_vec(reader: &mut impl Read, size: u64) -> Result<Vec<u8>> {
    let size = usize::try_from(size)
        .map_err(|_| DeliveryError::SizeLimit("extension header is too large".to_owned()))?;
    let mut bytes = vec![0_u8; size];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| DeliveryError::io_label("archive", error))?;
    Ok(bytes)
}

fn skip_padding(reader: &mut impl Read, size: u64) -> Result<()> {
    let padding = (u64::try_from(TAR_BLOCK).unwrap_or(512)
        - size % u64::try_from(TAR_BLOCK).unwrap_or(512))
        % u64::try_from(TAR_BLOCK).unwrap_or(512);
    let mut buffer = [0_u8; TAR_BLOCK];
    let padding = usize::try_from(padding).unwrap_or(0);
    reader
        .read_exact(&mut buffer[..padding])
        .map_err(|error| DeliveryError::io_label("archive", error))
}

fn skip_padded(reader: &mut impl Read, size: u64) -> Result<()> {
    let padded = size
        .checked_add(u64::try_from(TAR_BLOCK - 1).unwrap_or(511))
        .ok_or_else(|| DeliveryError::SizeLimit("tar size overflow".to_owned()))?
        / u64::try_from(TAR_BLOCK).unwrap_or(512)
        * u64::try_from(TAR_BLOCK).unwrap_or(512);
    let mut remaining = padded;
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        reader
            .read_exact(&mut buffer[..count])
            .map_err(|error| DeliveryError::io_label("archive", error))?;
        remaining -= u64::try_from(count).unwrap_or(0);
    }
    Ok(())
}

fn parse_tar_size(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        return Err(DeliveryError::UnsafeArchive(
            "base-256 tar size fields are not accepted".to_owned(),
        ));
    }
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    let text = std::str::from_utf8(&field[..end])
        .map_err(|_| DeliveryError::UnsafeArchive("tar size field is not ASCII".to_owned()))?;
    // tar-rs truncates at NUL and trims whitespace before parsing octal.
    // Mirror that behavior so the pre-scan and tar-rs agree on the next
    // header offset. An empty field is invalid rather than silently zero.
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
        return Err(DeliveryError::UnsafeArchive(
            "tar size field is not canonical octal".to_owned(),
        ));
    }
    u64::from_str_radix(text, 8)
        .map_err(|_| DeliveryError::UnsafeArchive("tar size field is malformed".to_owned()))
}

fn parse_pax_size(content: &[u8]) -> Result<Option<u64>> {
    let mut rest = content;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| DeliveryError::UnsafeArchive("PAX record has no length".to_owned()))?;
        let length_text = std::str::from_utf8(&rest[..space])
            .map_err(|_| DeliveryError::UnsafeArchive("PAX length is not ASCII".to_owned()))?;
        let length = length_text
            .parse::<usize>()
            .map_err(|_| DeliveryError::UnsafeArchive("PAX length is malformed".to_owned()))?;
        if length == 0 || length > rest.len() || length <= space + 1 {
            return Err(DeliveryError::UnsafeArchive(
                "PAX record length is inconsistent".to_owned(),
            ));
        }
        let record = &rest[..length];
        if record.last() != Some(&b'\n') {
            return Err(DeliveryError::UnsafeArchive(
                "PAX record has no newline".to_owned(),
            ));
        }
        let payload = &record[space + 1..record.len() - 1];
        if let Some(value) = payload.strip_prefix(b"size=") {
            let value = std::str::from_utf8(value)
                .map_err(|_| DeliveryError::UnsafeArchive("PAX size is not ASCII".to_owned()))?;
            let size = value.parse::<u64>().map_err(|_| {
                DeliveryError::UnsafeArchive("PAX size value is malformed".to_owned())
            })?;
            return Ok(Some(size));
        }
        rest = &rest[length..];
    }
    Ok(None)
}

/// Extracts a gzip-compressed tar archive into a new directory.
///
/// The destination must not exist. On failure the partial directory is
/// removed, so a caller never observes a half-extracted tree.
///
/// # Errors
///
/// Returns an error for oversized archives/files, unsafe paths, links, device
/// entries, duplicate paths, unsupported modes or truncated data.
pub fn extract_tar_gz(archive: &Path, destination: &Path) -> Result<ExtractReport> {
    extract_tar_gz_inner(archive, destination, None)
}

/// Extracts a formal release archive and enforces an exact file allowlist.
///
/// Every regular file must be listed, every listed file must be present, and
/// directories must only exist as parents of allowed files. Links, devices,
/// traversal, duplicates, case-folding collisions and unsafe modes are rejected
/// by the same rules as [`extract_tar_gz`].
///
/// # Errors
///
/// See [`extract_tar_gz`], plus missing, extra or unsafe allowlist entries.
pub fn extract_tar_gz_allowlisted(
    archive: &Path,
    destination: &Path,
    allowed: &BTreeSet<String>,
) -> Result<ExtractReport> {
    for path in allowed {
        validate_relative_path(path, true)?;
    }
    extract_tar_gz_inner(archive, destination, Some(allowed))
}

fn extract_tar_gz_inner(
    archive: &Path,
    destination: &Path,
    allowed: Option<&BTreeSet<String>>,
) -> Result<ExtractReport> {
    let metadata =
        std::fs::symlink_metadata(archive).map_err(|error| DeliveryError::io(archive, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::UnsafeArchive(
            "archive path is not a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "archive exceeds the {MAX_ARCHIVE_BYTES} byte limit"
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

    let result = extract_inner(archive, destination, allowed);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn extract_inner(
    archive: &Path,
    destination: &Path,
    allowed: Option<&BTreeSet<String>>,
) -> Result<ExtractReport> {
    let mut file = File::open(archive).map_err(|error| DeliveryError::io(archive, error))?;
    prescan_tar(BoundedReader::new(
        GzDecoder::new(&mut file),
        MAX_DECOMPRESSED_BYTES,
    ))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| DeliveryError::io(archive, error))?;
    let decoder = GzDecoder::new(file);
    let mut tar = Archive::new(BoundedReader::new(decoder, MAX_DECOMPRESSED_BYTES));
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut total_bytes = 0_u64;
    let mut entry_count = 0_usize;

    for entry in tar
        .entries()
        .map_err(|error| DeliveryError::io(archive, error))?
    {
        entry_count += 1;
        if entry_count > MAX_FILES.saturating_mul(2) {
            return Err(DeliveryError::SizeLimit(
                "archive contains too many entries".to_owned(),
            ));
        }
        let mut entry = entry.map_err(|error| DeliveryError::io(archive, error))?;
        process_entry(
            &mut entry,
            archive,
            destination,
            &mut files,
            &mut directories,
            &mut folded,
            &mut total_bytes,
            allowed,
        )?;
    }

    if let Some(allowed) = allowed {
        if &files != allowed {
            return Err(DeliveryError::Integrity(
                "archive file set does not match the package allowlist".to_owned(),
            ));
        }
    }

    let mut files: Vec<ExtractedFile> = files
        .into_iter()
        .map(|path| ExtractedFile { path, size: 0 })
        .collect();
    for file in &mut files {
        let metadata = std::fs::symlink_metadata(destination.join(&file.path))
            .map_err(|error| DeliveryError::io(&destination.join(&file.path), error))?;
        file.size = metadata.len();
    }
    Ok(ExtractReport { files, total_bytes })
}

#[allow(clippy::too_many_arguments)]
fn process_entry<R: Read>(
    entry: &mut Entry<'_, R>,
    archive: &Path,
    destination: &Path,
    files: &mut BTreeSet<String>,
    directories: &mut BTreeSet<String>,
    folded: &mut BTreeSet<String>,
    total_bytes: &mut u64,
    allowed: Option<&BTreeSet<String>>,
) -> Result<()> {
    let header = entry.header();
    let kind = header.entry_type();
    let raw_path = entry.path_bytes().into_owned();
    let mut path_text = std::str::from_utf8(&raw_path)
        .map_err(|_| DeliveryError::UnsafeArchive("archive path is not valid UTF-8".to_owned()))?
        .to_owned();
    if kind.is_dir() {
        if let Some(stripped) = path_text.strip_suffix('/') {
            path_text = stripped.to_owned();
        }
    }
    if path_text.len() > MAX_PATH_BYTES {
        return Err(DeliveryError::SizeLimit(
            "archive path exceeds the path-length limit".to_owned(),
        ));
    }
    validate_relative_path(&path_text, true)?;
    if !folded.insert(path_text.to_ascii_lowercase()) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{path_text}` collides with another path by case folding"
        )));
    }
    let relative = PathBuf::from(&path_text);
    let mode = header
        .mode()
        .map_err(|error| DeliveryError::io(archive, error))?;
    let header_size = header
        .size()
        .map_err(|error| DeliveryError::io(archive, error))?;
    let size = entry.size();
    if size != header_size {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{path_text}` has conflicting sizes"
        )));
    }

    if kind.is_dir() {
        check_entry_mode(mode, true, &path_text)?;
        if let Some(allowed) = allowed {
            let prefix = format!("{path_text}/");
            if !allowed.iter().any(|path| path.starts_with(&prefix)) {
                return Err(DeliveryError::UnsafeArchive(format!(
                    "archive directory `{path_text}` is not a parent of an allowed file"
                )));
            }
        }
        return ensure_directory(destination, &relative, directories, files);
    }
    if !kind.is_file() {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{path_text}` is not a regular file or directory"
        )));
    }
    if let Some(allowed) = allowed {
        if !allowed.contains(&path_text) {
            return Err(DeliveryError::UnsafeArchive(format!(
                "archive entry `{path_text}` is not in the package allowlist"
            )));
        }
    }
    if size > MAX_FILE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "entry `{path_text}` exceeds the per-file limit"
        )));
    }
    *total_bytes = total_bytes
        .checked_add(size)
        .ok_or_else(|| DeliveryError::SizeLimit("archive sizes overflow".to_owned()))?;
    if *total_bytes > MAX_EXTRACTED_BYTES {
        return Err(DeliveryError::SizeLimit(
            "archive exceeds the total extracted-size limit".to_owned(),
        ));
    }
    if !files.insert(path_text.clone()) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "duplicate archive path `{path_text}`"
        )));
    }
    if directories.contains(&path_text) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{path_text}` is both a file and a directory"
        )));
    }
    check_entry_mode(mode, false, &path_text)?;
    let executable = mode & 0o111 != 0;
    let destination_path = destination.join(&relative);
    ensure_parents(destination, &relative, files)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination_path)
        .map_err(|error| DeliveryError::io(&destination_path, error))?;
    let written = std::io::copy(&mut entry.by_ref().take(MAX_FILE_BYTES + 1), &mut output)
        .map_err(|error| DeliveryError::io(&destination_path, error))?;
    if written > MAX_FILE_BYTES || written != size {
        return Err(DeliveryError::Integrity(format!(
            "entry `{path_text}` was truncated or oversized"
        )));
    }
    set_mode(&destination_path, if executable { 0o755 } else { 0o644 })
}

fn check_entry_mode(mode: u32, directory: bool, path: &str) -> Result<()> {
    if mode & 0o7000 != 0 {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{path}` has setuid, setgid or sticky bits"
        )));
    }
    if mode & 0o022 != 0 {
        return Err(DeliveryError::UnsafeArchive(format!(
            "entry `{path}` is group- or world-writable"
        )));
    }
    let masked = mode & 0o777;
    if directory {
        if masked != 0o755 {
            return Err(DeliveryError::UnsafeArchive(format!(
                "directory `{path}` has an unexpected mode"
            )));
        }
    } else if !matches!(masked, 0o644 | 0o755) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "file `{path}` has an unexpected mode"
        )));
    }
    Ok(())
}

fn ensure_directory(
    destination: &Path,
    relative: &Path,
    directories: &mut BTreeSet<String>,
    files: &BTreeSet<String>,
) -> Result<()> {
    let text = relative.to_string_lossy().into_owned();
    if files.contains(&text) {
        return Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{text}` is both a file and a directory"
        )));
    }
    if !directories.insert(text) {
        return Ok(());
    }
    ensure_parents(destination, relative, files)?;
    let path = destination.join(relative);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(DeliveryError::UnsafeArchive(format!(
            "archive path `{}` is not a directory",
            relative.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&path).map_err(|error| DeliveryError::io(&path, error))?;
            set_mode(&path, 0o755)
        }
        Err(error) => Err(DeliveryError::io(&path, error)),
    }
}

fn ensure_parents(destination: &Path, relative: &Path, files: &BTreeSet<String>) -> Result<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut accumulated = PathBuf::new();
    for component in parent.components() {
        accumulated.push(component.as_os_str());
        let text = accumulated.to_string_lossy().into_owned();
        if files.contains(&text) {
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
                set_mode(&path, 0o755)?;
            }
            Err(error) => return Err(DeliveryError::io(&path, error)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;
    use crate::manifest::MAX_ARCHIVE_BYTES;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let _ = std::fs::create_dir_all(&base);
        base.join(format!(
            "wxml-archive-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ))
    }

    enum Spec {
        Dir(&'static str),
        File(&'static str, u32, &'static [u8]),
        Symlink(&'static str, &'static str),
        Fifo(&'static str),
        Hardlink(&'static str, &'static str),
        Char(&'static str),
    }

    fn build_archive(path: &Path, specs: &[Spec]) {
        let file = File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(encoder);
        for spec in specs {
            let mut header = Header::new_gnu();
            match spec {
                Spec::Dir(name) => {
                    header.set_entry_type(EntryType::Directory);
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_mtime(0);
                    header.set_path(name).unwrap();
                    header.set_cksum();
                    builder.append(&header, Cursor::new(Vec::new())).unwrap();
                }
                Spec::File(name, mode, body) => {
                    header.set_entry_type(EntryType::Regular);
                    header.set_size(body.len() as u64);
                    header.set_mode(*mode);
                    header.set_mtime(0);
                    header.set_path(name).unwrap();
                    header.set_cksum();
                    builder.append(&header, Cursor::new(body)).unwrap();
                }
                Spec::Fifo(name) => {
                    header.set_entry_type(EntryType::Fifo);
                    header.set_size(0);
                    header.set_mode(0o644);
                    header.set_mtime(0);
                    header.set_path(name).unwrap();
                    header.set_cksum();
                    builder.append(&header, Cursor::new(Vec::new())).unwrap();
                }
                Spec::Symlink(name, target) => {
                    header.set_entry_type(EntryType::Symlink);
                    header.set_size(0);
                    header.set_mode(0o777);
                    header.set_mtime(0);
                    header.set_path(name).unwrap();
                    header.set_link_name(target).unwrap();
                    header.set_cksum();
                    builder.append(&header, Cursor::new(Vec::new())).unwrap();
                }
                Spec::Hardlink(name, target) => {
                    header.set_entry_type(EntryType::Link);
                    header.set_size(0);
                    header.set_mode(0o644);
                    header.set_mtime(0);
                    header.set_path(name).unwrap();
                    header.set_link_name(target).unwrap();
                    header.set_cksum();
                    builder.append(&header, Cursor::new(Vec::new())).unwrap();
                }
                Spec::Char(name) => {
                    header.set_entry_type(EntryType::Char);
                    header.set_size(0);
                    header.set_mode(0o644);
                    header.set_mtime(0);
                    header.set_path(name).unwrap();
                    header.set_cksum();
                    builder.append(&header, Cursor::new(Vec::new())).unwrap();
                }
            }
        }
        builder.finish().unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
    }

    fn build_raw_archive(path: &Path, name: &str) {
        build_raw_archive_with_size(path, name, 0);
    }

    fn raw_header_with_size_field(name: &str, size_field: &[u8; 12], typeflag: u8) -> [u8; 512] {
        let mut block = [0_u8; 512];
        block[..name.len()].copy_from_slice(name.as_bytes());
        block[100..108].copy_from_slice(b"0000644\0");
        block[108..116].copy_from_slice(b"0000000\0");
        block[116..124].copy_from_slice(b"0000000\0");
        block[124..136].copy_from_slice(size_field);
        block[136..148].copy_from_slice(b"00000000000\0");
        block[148..156].fill(b' ');
        block[156] = typeflag;
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        block[148..156].copy_from_slice(checksum.as_bytes());
        block
    }

    fn raw_header(name: &str, size: u64, typeflag: u8) -> [u8; 512] {
        let size_field = format!("{size:011o}\0");
        let mut field = [0_u8; 12];
        field.copy_from_slice(size_field.as_bytes());
        raw_header_with_size_field(name, &field, typeflag)
    }

    fn build_raw_archive_with_size(path: &Path, name: &str, size: u64) {
        let file = File::create(path).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(&raw_header(name, size, b'0')).unwrap();
        encoder.write_all(&[0_u8; 1024]).unwrap();
        encoder.finish().unwrap();
    }

    fn pax_record(key: &str, value: &str) -> String {
        let payload = format!(" {key}={value}\n");
        let mut length = payload.len() + 1;
        loop {
            let record = format!("{length}{payload}");
            if record.len() == length {
                return record;
            }
            length = record.len();
        }
    }

    #[test]
    fn parses_leading_space_tar_size_like_tar_rs() {
        assert_eq!(parse_tar_size(b" 00000000012\0").unwrap(), 10);
        assert!(parse_tar_size(b"           \0").is_err());
    }

    #[test]
    fn rejects_pax_record_that_splits_a_utf8_character() {
        let content = b"3 \xc3\xa9";
        assert!(parse_pax_size(content).is_err());
    }

    #[test]
    fn leading_space_size_cannot_hide_a_huge_extension_header() {
        let base = scratch("leading-space-size");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        let file = File::create(&archive).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        let leading = b" 00000000012";
        encoder
            .write_all(&raw_header_with_size_field("a", leading, b'0'))
            .unwrap();
        encoder.write_all(&[0_u8; 512]).unwrap();
        encoder
            .write_all(&raw_header("longname", 1024 * 1024 * 1024, b'L'))
            .unwrap();
        encoder.write_all(&[0_u8; 1024]).unwrap();
        encoder.finish().unwrap();
        let started = std::time::Instant::now();
        let error = extract_tar_gz(&archive, &base.join("out")).unwrap_err();
        assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_pax_size_overrides_before_writing_data() {
        let base = scratch("pax-size");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        let content = pax_record("size", &(MAX_FILE_BYTES + 1).to_string());
        let file = File::create(&archive).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder
            .write_all(&raw_header("pax", content.len() as u64, b'x'))
            .unwrap();
        encoder.write_all(content.as_bytes()).unwrap();
        let padding = (TAR_BLOCK - content.len() % TAR_BLOCK) % TAR_BLOCK;
        encoder.write_all(&vec![0_u8; padding]).unwrap();
        encoder.write_all(&raw_header("payload", 1, b'0')).unwrap();
        encoder.write_all(&[0_u8; 1024]).unwrap();
        encoder.finish().unwrap();
        let dest = base.join("out");
        let error = extract_tar_gz(&archive, &dest).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{error:?}"
        );
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_huge_extension_headers_without_reading_them() {
        let base = scratch("huge-extension");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        let file = File::create(&archive).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder
            .write_all(&raw_header("longname", 1024 * 1024 * 1024, b'L'))
            .unwrap();
        encoder.write_all(&[0_u8; 1024]).unwrap();
        encoder.finish().unwrap();
        let started = std::time::Instant::now();
        let error = extract_tar_gz(&archive, &base.join("out")).unwrap_err();
        assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn extracts_regular_files_with_explicit_modes() {
        let base = scratch("normal");
        let archive = base.join("a.tar.gz");
        let dest = base.join("out");
        std::fs::create_dir_all(&base).unwrap();
        build_archive(
            &archive,
            &[
                Spec::File("bin/tool", 0o755, b"tool"),
                Spec::File("data.txt", 0o644, b"data"),
            ],
        );

        let report = extract_tar_gz(&archive, &dest).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.total_bytes, 8);
        assert_eq!(std::fs::read(dest.join("data.txt")).unwrap(), b"data");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn extracts_directories_and_nested_files() {
        let base = scratch("dirs");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        build_archive(
            &archive,
            &[Spec::Dir("nested/"), Spec::File("nested/a", 0o644, b"a")],
        );
        let dest = base.join("out");
        let report = extract_tar_gz(&archive, &dest).unwrap();
        assert_eq!(report.files.len(), 1);
        assert!(dest.join("nested/a").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        for (name, path) in [("absolute", "/etc/passwd"), ("parent", "../escape")] {
            let base = scratch(name);
            let archive = base.join("a.tar.gz");
            let dest = base.join("out");
            std::fs::create_dir_all(&base).unwrap();
            build_raw_archive(&archive, path);
            assert!(
                extract_tar_gz(&archive, &dest).is_err(),
                "spec should fail: {name}"
            );
            assert!(!dest.exists(), "failed extraction must be removed");
            assert!(!base.join("escape").exists());
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn rejects_links_and_device_entries() {
        for (name, specs) in [
            ("symlink", vec![Spec::Symlink("link", "/etc/passwd")]),
            ("hardlink", vec![Spec::Hardlink("link", "target")]),
            ("char", vec![Spec::Char("device")]),
            ("fifo", vec![Spec::Fifo("pipe")]),
        ] {
            let base = scratch(name);
            let archive = base.join("a.tar.gz");
            let dest = base.join("out");
            std::fs::create_dir_all(&base).unwrap();
            build_archive(&archive, &specs);
            assert!(
                extract_tar_gz(&archive, &dest).is_err(),
                "spec should fail: {name}"
            );
            assert!(!dest.exists(), "failed extraction must be removed");
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn rejects_unsafe_modes_and_duplicates() {
        for (name, specs) in [
            ("setuid", vec![Spec::File("a", 0o4755, b"x")]),
            ("writable", vec![Spec::File("a", 0o666, b"x")]),
            (
                "duplicate",
                vec![Spec::File("a", 0o644, b"x"), Spec::File("a", 0o644, b"y")],
            ),
        ] {
            let base = scratch(name);
            let archive = base.join("a.tar.gz");
            let dest = base.join("out");
            std::fs::create_dir_all(&base).unwrap();
            build_archive(&archive, &specs);
            assert!(
                extract_tar_gz(&archive, &dest).is_err(),
                "spec should fail: {name}"
            );
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn allowlist_rejects_extra_and_missing_files() {
        let base = scratch("allowlist");
        std::fs::create_dir_all(&base).unwrap();

        let with_extra = base.join("extra.tar.gz");
        build_archive(
            &with_extra,
            &[
                Spec::File("a", 0o644, b"a"),
                Spec::File("extra", 0o644, b"x"),
            ],
        );
        let only_a = BTreeSet::from(["a".to_owned()]);
        let dest = base.join("out-extra");
        let error = extract_tar_gz_allowlisted(&with_extra, &dest, &only_a).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{error:?}"
        );
        assert!(!dest.exists());

        let only_a_archive = base.join("only-a.tar.gz");
        build_archive(&only_a_archive, &[Spec::File("a", 0o644, b"a")]);
        let mut missing = only_a.clone();
        missing.insert("missing".to_owned());
        let dest = base.join("out-missing");
        let error = extract_tar_gz_allowlisted(&only_a_archive, &dest, &missing).unwrap_err();
        assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
        assert!(!dest.exists());

        let dest = base.join("out-ok");
        let report = extract_tar_gz_allowlisted(&only_a_archive, &dest, &only_a).unwrap();
        assert_eq!(report.files.len(), 1);
        assert_eq!(std::fs::read(dest.join("a")).unwrap(), b"a");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_duplicate_directory_entries() {
        let base = scratch("duplicate-dir");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        build_archive(&archive, &[Spec::Dir("d/"), Spec::Dir("d/")]);
        let dest = base.join("out");
        let error = extract_tar_gz(&archive, &dest).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{error:?}"
        );
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_case_folding_collisions() {
        let base = scratch("case-fold");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        build_archive(
            &archive,
            &[
                Spec::File("Lib.d.ts", 0o644, b"x"),
                Spec::File("lib.d.ts", 0o644, b"y"),
            ],
        );
        let dest = base.join("out");
        let error = extract_tar_gz(&archive, &dest).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{error:?}"
        );
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_an_oversized_file_header_before_reading_it() {
        let base = scratch("oversize-file");
        let archive = base.join("a.tar.gz");
        let dest = base.join("out");
        std::fs::create_dir_all(&base).unwrap();
        build_raw_archive_with_size(&archive, "huge.bin", MAX_FILE_BYTES + 1);
        let error = extract_tar_gz(&archive, &dest).unwrap_err();
        assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_oversized_archive_before_decode() {
        let base = scratch("oversize");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        let file = File::create(&archive).unwrap();
        file.set_len(MAX_ARCHIVE_BYTES + 1).unwrap();
        let dest = base.join("out");
        let error = extract_tar_gz(&archive, &dest).unwrap_err();
        assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn refuses_to_extract_over_an_existing_directory() {
        let base = scratch("existing");
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        build_archive(&archive, &[Spec::File("a", 0o644, b"x")]);
        std::fs::create_dir(base.join("out")).unwrap();
        assert!(extract_tar_gz(&archive, &base.join("out")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn created_archives_are_deterministic_and_extractable() {
        let base = scratch("create");
        let root = base.join("root");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/tool"), b"tool").unwrap();
        std::fs::write(root.join("data"), b"data").unwrap();
        let entries = vec![
            FileEntry {
                path: "bin/tool".to_owned(),
                sha256: "0".repeat(64),
                size: 4,
                mode: "0755".to_owned(),
            },
            FileEntry {
                path: "data".to_owned(),
                sha256: "0".repeat(64),
                size: 4,
                mode: "0644".to_owned(),
            },
        ];
        let first = base.join("first.tar.gz");
        let second = base.join("second.tar.gz");
        create_tar_gz(&root, &first, &entries, 0).unwrap();
        create_tar_gz(&root, &second, &entries, 0).unwrap();
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap()
        );
        let dest = base.join("out");
        let report = extract_tar_gz(&first, &dest).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!(std::fs::read(dest.join("bin/tool")).unwrap(), b"tool");
        let _ = std::fs::remove_dir_all(&base);
    }
}
