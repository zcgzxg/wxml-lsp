//! Deterministic archive creation and hardened extraction.
//!
//! The installer treats archives as hostile input. Only regular files and
//! directories are accepted; links and device nodes are always rejected, and
//! every path is validated before anything is created below the extraction
//! root.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tar::{Archive, Builder, EntryType, Header};

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
    let metadata = std::fs::metadata(archive).map_err(|error| DeliveryError::io(archive, error))?;
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

    let result = extract_inner(archive, destination);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    result
}

fn extract_inner(archive: &Path, destination: &Path) -> Result<ExtractReport> {
    let file = File::open(archive).map_err(|error| DeliveryError::io(archive, error))?;
    let decoder = GzDecoder::new(file);
    let mut tar = Archive::new(decoder);
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
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
        let header = entry.header();
        let kind = header.entry_type();
        let raw_path = entry.path_bytes().into_owned();
        let path_text = std::str::from_utf8(&raw_path).map_err(|_| {
            DeliveryError::UnsafeArchive("archive path is not valid UTF-8".to_owned())
        })?;
        if path_text.len() > MAX_PATH_BYTES {
            return Err(DeliveryError::SizeLimit(
                "archive path exceeds the path-length limit".to_owned(),
            ));
        }
        validate_relative_path(path_text, true)?;
        let relative = PathBuf::from(path_text);

        if kind.is_dir() {
            check_entry_mode(header.mode().unwrap_or(0o755), true, path_text)?;
            ensure_directory(destination, &relative, &mut directories, &files)?;
            continue;
        }
        if !kind.is_file() {
            return Err(DeliveryError::UnsafeArchive(format!(
                "entry `{path_text}` is not a regular file or directory"
            )));
        }
        let size = header
            .size()
            .map_err(|error| DeliveryError::io(archive, error))?;
        if size > MAX_FILE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "entry `{path_text}` exceeds the per-file limit"
            )));
        }
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or_else(|| DeliveryError::SizeLimit("archive sizes overflow".to_owned()))?;
        if total_bytes > MAX_EXTRACTED_BYTES {
            return Err(DeliveryError::SizeLimit(
                "archive exceeds the total extracted-size limit".to_owned(),
            ));
        }
        if !files.insert(path_text.to_owned()) {
            return Err(DeliveryError::UnsafeArchive(format!(
                "duplicate archive path `{path_text}`"
            )));
        }
        if directories.contains(path_text) {
            return Err(DeliveryError::UnsafeArchive(format!(
                "archive path `{path_text}` is both a file and a directory"
            )));
        }
        check_entry_mode(header.mode().unwrap_or(0o644), false, path_text)?;
        let executable = header.mode().is_ok_and(|mode| mode & 0o111 != 0);
        let destination_path = destination.join(&relative);
        ensure_parents(destination, &relative, &files)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination_path)
            .map_err(|error| DeliveryError::io(&destination_path, error))?;
        let written = std::io::copy(&mut entry, &mut output)
            .map_err(|error| DeliveryError::io(&destination_path, error))?;
        if written != size {
            return Err(DeliveryError::Integrity(format!(
                "entry `{path_text}` was truncated"
            )));
        }
        set_mode(&destination_path, if executable { 0o755 } else { 0o644 })?;
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

    fn build_raw_archive_with_size(path: &Path, name: &str, size: u64) {
        let mut block = [0_u8; 512];
        block[..name.len()].copy_from_slice(name.as_bytes());
        block[100..108].copy_from_slice(b"0000644\0");
        block[108..116].copy_from_slice(b"0000000\0");
        block[116..124].copy_from_slice(b"0000000\0");
        let size_field = format!("{size:011o}\0");
        block[124..136].copy_from_slice(size_field.as_bytes());
        block[136..148].copy_from_slice(b"00000000000\0");
        block[148..156].fill(b' ');
        block[156] = b'0';
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        block[148..156].copy_from_slice(checksum.as_bytes());
        let file = File::create(path).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder.write_all(&block).unwrap();
        encoder.write_all(&[0_u8; 1024]).unwrap();
        encoder.finish().unwrap();
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
            &[Spec::Dir("nested"), Spec::File("nested/a", 0o644, b"a")],
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
