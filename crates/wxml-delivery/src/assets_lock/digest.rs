//! Canonical tree fingerprints and type-asset runtime closure.

use std::collections::BTreeSet;

use crate::error::{DeliveryError, Result};
use crate::manifest::{FileEntry, sha256_bytes};
use crate::paths::{validate_relative_path, validate_sha256};
use crate::type_assets::TypeAssetBundle;
use crate::verify_v2::{TreeFingerprintDirectory, TreeFingerprintFile, tree_fingerprint_preimage};

/// Computes the canonical tree fingerprint of a lock file table.
///
/// The preimage is exactly the one used by
/// [`crate::verify_v2::tree_fingerprint`]:
///
/// 1. every directory from the ancestor closure of `files[]`, sorted by UTF-8
///    path bytes, as `D 755 <directory>\n`,
/// 2. every file, sorted by UTF-8 path bytes, as
///    `F <octal mode> <sha256> <path>\n`,
/// 3. SHA-256 over those bytes.
///
/// `root` is `""` for the upstream tarball tree and the asset `packageRoot`
/// (for example `engine`) for the payload tree. The root itself is stripped
/// and never appears in a `D` row. The lock has no `directories[]` field, so
/// empty directories cannot be represented and must be rejected or ignored by
/// the L2/L3 extractor.
///
/// This L1 helper is not a frozen public API.
///
/// # Errors
///
/// Returns an error for a malformed root, a file outside `root`, an unsafe
/// relative path, an invalid mode or a non-canonical SHA-256.
pub fn canonical_tree_digest(files: &[FileEntry], root: &str) -> Result<String> {
    let prefix = tree_root_prefix(root)?;
    let mut directories = BTreeSet::new();
    let mut rows = Vec::with_capacity(files.len());
    for file in files {
        let relative = tree_relative_path(file, root, &prefix)?;
        collect_ancestor_directories(relative, &mut directories);
        rows.push(TreeFingerprintFile {
            path: relative,
            mode: file.mode_bits()?,
            sha256: &file.sha256,
        });
    }
    let directory_rows: Vec<TreeFingerprintDirectory<'_>> = directories
        .iter()
        .map(|path| TreeFingerprintDirectory {
            path: path.as_str(),
            mode: 0o755,
        })
        .collect();
    Ok(sha256_bytes(&tree_fingerprint_preimage(
        &directory_rows,
        &rows,
    )))
}

fn tree_root_prefix(root: &str) -> Result<String> {
    if root.is_empty() {
        return Ok(String::new());
    }
    validate_relative_path(root, false)?;
    if root.ends_with('/') {
        return Err(DeliveryError::InvalidPackage(
            "assets lock tree root must not end with `/`".to_owned(),
        ));
    }
    Ok(format!("{root}/"))
}

fn tree_relative_path<'a>(file: &'a FileEntry, root: &str, prefix: &str) -> Result<&'a str> {
    let relative = if prefix.is_empty() {
        file.path.as_str()
    } else {
        file.path.strip_prefix(prefix).ok_or_else(|| {
            DeliveryError::InvalidPackage(format!(
                "assets lock tree file `{}` is outside `{root}`",
                file.path
            ))
        })?
    };
    if relative.is_empty() {
        return Err(DeliveryError::InvalidPackage(
            "assets lock tree file has an empty root-relative path".to_owned(),
        ));
    }
    validate_relative_path(relative, true)?;
    validate_sha256(&file.sha256)?;
    Ok(relative)
}

fn collect_ancestor_directories(path: &str, directories: &mut BTreeSet<String>) {
    for (index, _) in path.match_indices('/') {
        if let Some(directory) = path.get(..index) {
            directories.insert(directory.to_owned());
        }
    }
}

pub(super) fn compute_runtime_closure(
    files: &[FileEntry],
    package_root: &str,
) -> Result<TypeAssetBundle> {
    let prefix = format!("{package_root}/");
    let mut matched = Vec::new();
    let mut total_bytes = 0_u64;
    for file in files {
        let Some(relative) = file.path.strip_prefix(&prefix) else {
            continue;
        };
        if relative == "index.d.ts"
            || (relative.starts_with("types/") && relative.ends_with(".d.ts"))
        {
            total_bytes = total_bytes.checked_add(file.size).ok_or_else(|| {
                DeliveryError::SizeLimit("runtime typings bytes overflow".to_owned())
            })?;
            matched.push((file.path.as_str(), relative, file.sha256.as_str()));
        }
    }
    matched.sort_by(|left, right| left.0.cmp(right.0));
    let mut lines = String::new();
    for (_, relative, sha256) in &matched {
        lines.push_str(sha256);
        lines.push_str("  ");
        lines.push_str(relative);
        lines.push('\n');
    }
    let file_count = u64::try_from(matched.len())
        .map_err(|_| DeliveryError::SizeLimit("runtime typings count overflows".to_owned()))?;
    Ok(TypeAssetBundle {
        file_count,
        total_bytes,
        sha256: sha256_bytes(lines.as_bytes()),
    })
}
