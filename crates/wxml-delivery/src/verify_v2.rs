//! On-disk verification for native schema v2 package trees.
//!
//! Verification is an allowlist check: the exact file set must equal
//! `files[] ∪ {manifest.json, SHA256SUMS}` with no extra directories, links or
//! modified bytes. Type-asset bundle digests and Node-runtime negative rules
//! are re-checked from the manifest, and all hashing is local.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{DeliveryError, Result};
use crate::manifest::{
    FileEntry, MAX_MANIFEST_BYTES, format_sha256sums, parse_sha256sums, read_limited, sha256_bytes,
    sha256_file,
};
use crate::manifest_v2::{
    ExpectedManifestV2, MINIPROGRAM_TYPINGS_ASSET, MINIPROGRAM_TYPINGS_BUNDLE_DIGEST,
    MINIPROGRAM_TYPINGS_TOTAL_BYTES, ManifestV2, compute_type_asset_bundle, forbidden_runtime_path,
    is_allowed_miniprogram_typings_path, read_manifest_v2,
};
use crate::paths::validate_relative_path;
use crate::state::expected_directories;

/// Summary of a verified native package tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTreeReport {
    /// Validated manifest.
    pub manifest: ManifestV2,
    /// Number of payload files (excluding `manifest.json` and `SHA256SUMS`).
    pub file_count: usize,
    /// Total payload bytes.
    pub total_bytes: u64,
}

/// Verifies a native package tree against its own manifest and checksums.
///
/// # Errors
///
/// Returns an error for a legacy schema, missing/extra files or directories,
/// links, wrong modes/sizes/digests, a non-canonical `SHA256SUMS`, or a
/// Node-runtime payload.
pub fn verify_tree_v2(root: &Path, expected: ExpectedManifestV2<'_>) -> Result<NativeTreeReport> {
    let metadata =
        std::fs::symlink_metadata(root).map_err(|error| DeliveryError::io(root, error))?;
    if !metadata.file_type().is_dir() {
        return Err(DeliveryError::InstallState(
            "package root must be a real directory, not a link".to_owned(),
        ));
    }
    let manifest = read_manifest_v2(root, expected)?;
    let checksums_path = root.join("SHA256SUMS");
    let checksum_bytes = read_limited(&checksums_path, MAX_MANIFEST_BYTES)?;
    let checksums = parse_sha256sums(&checksum_bytes)?;
    if checksum_bytes != format_sha256sums(&checksums) {
        return Err(DeliveryError::Integrity(
            "SHA256SUMS is not in canonical sorted form".to_owned(),
        ));
    }

    let mut required: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    required.insert("manifest.json".to_owned());
    let listed: BTreeSet<String> = checksums.keys().cloned().collect();
    if listed != required {
        return Err(DeliveryError::Integrity(
            "SHA256SUMS does not list exactly the manifest payload".to_owned(),
        ));
    }

    let mut total_bytes = 0_u64;
    for file in &manifest.files {
        total_bytes = total_bytes
            .checked_add(file.size)
            .ok_or_else(|| DeliveryError::SizeLimit("payload sizes overflow".to_owned()))?;
        verify_payload_file(root, file, checksums.get(&file.path))?;
    }

    let manifest_path = root.join("manifest.json");
    check_regular_file(&manifest_path)?;
    check_mode(&manifest_path, 0o644)?;
    let manifest_digest = sha256_file(&manifest_path)?;
    if checksums.get("manifest.json") != Some(&manifest_digest) {
        return Err(DeliveryError::Integrity(
            "manifest.json does not match SHA256SUMS".to_owned(),
        ));
    }
    check_regular_file(&checksums_path)?;
    check_mode(&checksums_path, 0o644)?;

    let tree = collect_tree(root)?;
    let mut expected_files = required;
    expected_files.insert("SHA256SUMS".to_owned());
    if tree.files != expected_files {
        return Err(DeliveryError::Integrity(
            "package tree contains missing or unexpected files".to_owned(),
        ));
    }
    let expected_dirs = expected_directories(expected_files.iter().cloned());
    if tree.directories != expected_dirs {
        return Err(DeliveryError::Integrity(
            "package tree contains missing or unexpected directories".to_owned(),
        ));
    }
    for directory in &expected_dirs {
        check_mode(&root.join(directory), 0o755)?;
    }
    for path in &tree.files {
        if let Some(reason) = forbidden_runtime_path(path) {
            return Err(DeliveryError::InvalidPackage(format!(
                "package tree contains `{path}`: {reason}"
            )));
        }
        if !is_allowed_miniprogram_typings_path(path) {
            return Err(DeliveryError::InvalidPackage(format!(
                "package tree contains `{path}` outside the frozen miniprogram typings payload"
            )));
        }
    }

    // Recompute bundle digests from the verified payload entries so the
    // manifest cannot declare a digest over a different file set.
    for asset in &manifest.type_assets {
        let bundle = compute_type_asset_bundle(&manifest.files, &asset.root, &asset.include)?;
        if bundle.file_count != asset.file_count || bundle.sha256 != asset.sha256 {
            return Err(DeliveryError::Integrity(format!(
                "type asset `{}` does not match its bundle digest",
                asset.name
            )));
        }
        if asset.name == MINIPROGRAM_TYPINGS_ASSET
            && (bundle.total_bytes != MINIPROGRAM_TYPINGS_TOTAL_BYTES
                || bundle.sha256 != MINIPROGRAM_TYPINGS_BUNDLE_DIGEST)
        {
            return Err(DeliveryError::Integrity(
                "miniprogram typings payload does not match the frozen identity".to_owned(),
            ));
        }
    }

    Ok(NativeTreeReport {
        file_count: manifest.files.len(),
        total_bytes,
        manifest,
    })
}

fn verify_payload_file(root: &Path, file: &FileEntry, checksum: Option<&String>) -> Result<()> {
    if let Some(reason) = forbidden_runtime_path(&file.path) {
        return Err(DeliveryError::InvalidPackage(format!(
            "payload `{}` is not allowed: {reason}",
            file.path
        )));
    }
    let path = root.join(&file.path);
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DeliveryError::Integrity(format!("payload `{}` is missing", file.path))
        } else {
            DeliveryError::io(&path, error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::Integrity(format!(
            "payload `{}` is not a regular file",
            file.path
        )));
    }
    if metadata.len() != file.size {
        return Err(DeliveryError::Integrity(format!(
            "payload `{}` has the wrong size",
            file.path
        )));
    }
    check_mode(&path, file.mode_bits()?)?;
    let digest = sha256_file(&path)?;
    if digest != file.sha256 || checksum != Some(&digest) {
        return Err(DeliveryError::Integrity(format!(
            "payload `{}` does not match its SHA-256",
            file.path
        )));
    }
    Ok(())
}

fn check_regular_file(path: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::Integrity(format!(
            "`{}` is not a regular file",
            crate::paths::redact(path)
        )));
    }
    Ok(())
}

fn check_mode(path: &Path, expected: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
        let actual = metadata.permissions().mode() & 0o777;
        if actual != expected {
            return Err(DeliveryError::Integrity(format!(
                "`{}` has mode {actual:o}, expected {expected:o}",
                crate::paths::redact(path)
            )));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, expected);
        Ok(())
    }
}

/// Files and directories observed below a package root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TreeContents {
    /// Package-relative regular files.
    pub files: BTreeSet<String>,
    /// Package-relative directories, excluding the root itself.
    pub directories: BTreeSet<String>,
}

/// Collects the exact regular-file and directory sets below `root`.
///
/// # Errors
///
/// Returns an error for symbolic links, sockets, devices or unreadable paths.
pub fn collect_tree(root: &Path) -> Result<TreeContents> {
    let mut contents = TreeContents::default();
    collect_inner(root, root, &mut contents)?;
    Ok(contents)
}

fn collect_inner(root: &Path, directory: &Path, contents: &mut TreeContents) -> Result<()> {
    let entries =
        std::fs::read_dir(directory).map_err(|error| DeliveryError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(directory, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        let relative_os = path
            .strip_prefix(root)
            .map_err(|_| DeliveryError::Integrity("package path escaped its root".to_owned()))?;
        let relative = relative_os.to_str().ok_or_else(|| {
            DeliveryError::Integrity("package tree contains a non-UTF-8 path".to_owned())
        })?;
        if relative.contains('\\') {
            return Err(DeliveryError::Integrity(
                "package tree contains a backslash in a path".to_owned(),
            ));
        }
        validate_relative_path(relative, true)?;
        let relative = relative.to_owned();
        if metadata.file_type().is_symlink() {
            return Err(DeliveryError::Integrity(format!(
                "package tree contains a symbolic link at `{relative}`"
            )));
        }
        if metadata.file_type().is_dir() {
            contents.directories.insert(relative);
            collect_inner(root, &path, contents)?;
        } else if metadata.file_type().is_file() {
            contents.files.insert(relative);
        } else {
            return Err(DeliveryError::Integrity(format!(
                "package tree contains a non-regular entry at `{relative}`"
            )));
        }
    }
    Ok(())
}

/// Writes the checksum file for a completed native package tree.
///
/// # Errors
///
/// Returns an error when a payload file cannot be hashed or changed after the
/// manifest was written.
pub fn checksums_for_tree_v2(root: &Path, manifest: &ManifestV2) -> Result<Vec<u8>> {
    let mut entries = BTreeMap::new();
    for file in &manifest.files {
        let path = root.join(&file.path);
        let digest = sha256_file(&path)?;
        if digest != file.sha256 {
            return Err(DeliveryError::Integrity(format!(
                "payload `{}` changed before checksum generation",
                file.path
            )));
        }
        entries.insert(file.path.clone(), digest);
    }
    entries.insert(
        "manifest.json".to_owned(),
        sha256_file(&root.join("manifest.json"))?,
    );
    Ok(format_sha256sums(&entries))
}

/// One directory row for the canonical tree fingerprint.
pub(crate) struct TreeFingerprintDirectory<'a> {
    /// Root-relative directory path using `/` separators.
    pub path: &'a str,
    /// POSIX mode without the file-type bits.
    pub mode: u32,
}

/// One file row for the canonical tree fingerprint.
pub(crate) struct TreeFingerprintFile<'a> {
    /// Root-relative file path using `/` separators.
    pub path: &'a str,
    /// POSIX mode without the file-type bits.
    pub mode: u32,
    /// Lowercase SHA-256 of the file bytes.
    pub sha256: &'a str,
}

/// Builds the canonical tree-fingerprint preimage shared by the installer and
/// the asset lock.
///
/// Directories are sorted by the raw UTF-8 bytes of their path and serialized
/// as `D <octal mode> <path>\n`; files follow, sorted the same way and
/// serialized as `F <octal mode> <sha256> <path>\n`. Octal modes have no
/// leading zero. The caller hashes the returned bytes with SHA-256.
#[must_use]
pub(crate) fn tree_fingerprint_preimage(
    directories: &[TreeFingerprintDirectory<'_>],
    files: &[TreeFingerprintFile<'_>],
) -> Vec<u8> {
    let mut directories: Vec<&TreeFingerprintDirectory<'_>> = directories.iter().collect();
    directories.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    let mut lines = String::new();
    for directory in directories {
        lines.push_str("D ");
        let _ = write!(lines, "{:o} ", directory.mode);
        lines.push_str(directory.path);
        lines.push('\n');
    }
    let mut files: Vec<&TreeFingerprintFile<'_>> = files.iter().collect();
    files.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    for file in files {
        lines.push_str("F ");
        let _ = write!(lines, "{:o} ", file.mode);
        lines.push_str(file.sha256);
        lines.push(' ');
        lines.push_str(file.path);
        lines.push('\n');
    }
    lines.into_bytes()
}

/// Returns a stable fingerprint of a package tree's paths, modes and bytes.
///
/// Used by tests to prove that a failed transaction left the previous tree
/// byte-identical.
///
/// # Errors
///
/// Returns an error when the tree contains unsafe entries or cannot be read.
pub fn tree_fingerprint(root: &Path) -> Result<String> {
    let contents = collect_tree(root)?;
    let mut directories = Vec::with_capacity(contents.directories.len());
    for directory in &contents.directories {
        let path = root.join(directory);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        directories.push(TreeFingerprintDirectory {
            path: directory,
            mode: mode_of(&metadata),
        });
    }
    let mut hashes = Vec::with_capacity(contents.files.len());
    for file in &contents.files {
        hashes.push(sha256_file(&root.join(file))?);
    }
    let files: Vec<TreeFingerprintFile<'_>> = contents
        .files
        .iter()
        .zip(&hashes)
        .map(|(file, sha256)| {
            let metadata = std::fs::symlink_metadata(root.join(file))
                .map_err(|error| DeliveryError::io(&root.join(file), error))?;
            Ok(TreeFingerprintFile {
                path: file,
                mode: mode_of(&metadata),
                sha256,
            })
        })
        .collect::<Result<_>>()?;
    Ok(sha256_bytes(&tree_fingerprint_preimage(
        &directories,
        &files,
    )))
}

fn mode_of(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

/// Checks the static requirements for the native engine binary.
///
/// # Errors
///
/// Returns an error when the path is missing, linked, non-executable, or does
/// not match the manifest size and SHA-256.
pub fn check_native_engine(root: &Path, manifest: &ManifestV2) -> Result<PathBuf> {
    let path = root.join(&manifest.engine.binary);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::Runtime(
            "native engine is not a regular file".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(DeliveryError::Runtime(
                "native engine is not executable".to_owned(),
            ));
        }
    }
    let file = manifest.file(&manifest.engine.binary).ok_or_else(|| {
        DeliveryError::Runtime("manifest does not list the native engine".to_owned())
    })?;
    if metadata.len() != file.size {
        return Err(DeliveryError::Runtime(
            "native engine size does not match the manifest".to_owned(),
        ));
    }
    if sha256_file(&path)? != file.sha256 {
        return Err(DeliveryError::Runtime(
            "native engine SHA-256 does not match the manifest".to_owned(),
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_checksum_format_is_exact() {
        let mut entries = BTreeMap::new();
        entries.insert("a".to_owned(), "0".repeat(64));
        entries.insert("b".to_owned(), "1".repeat(64));
        let bytes = format_sha256sums(&entries);
        let parsed = parse_sha256sums(&bytes).unwrap();
        assert_eq!(bytes, format_sha256sums(&parsed));
    }

    #[test]
    fn tree_fingerprint_preimage_sorts_directories_then_files() {
        let directories = [
            TreeFingerprintDirectory {
                path: "z",
                mode: 0o755,
            },
            TreeFingerprintDirectory {
                path: "a",
                mode: 0o755,
            },
        ];
        let files = [
            TreeFingerprintFile {
                path: "b",
                mode: 0o644,
                sha256: "1",
            },
            TreeFingerprintFile {
                path: "a",
                mode: 0o755,
                sha256: "0",
            },
        ];
        let preimage = tree_fingerprint_preimage(&directories, &files);
        assert_eq!(
            String::from_utf8(preimage).unwrap(),
            "D 755 a\nD 755 z\nF 755 0 a\nF 644 1 b\n"
        );
    }
}
