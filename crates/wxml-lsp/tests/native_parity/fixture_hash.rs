//! Fixture tree hash gate (v1).
//!
//! The algorithm is frozen by `docs/reference/tsgo-test-contract.md` section 2:
//! regular files only (symlinks rejected), root-relative UTF-8 paths with `/`
//! separators, entries sorted by the UTF-8 bytes of the relative path, and
//! SHA-256 over `relative_path + NUL + raw_bytes + NUL` for every file.
//!
//! This gate only reads. It never rewrites the ledger or the expected hashes;
//! a mismatch is a failure, not an invitation to auto-update.
//!
//! Line endings: v1 hashes are raw bytes from an LF checkout. A CRLF checkout
//! changes every hash; see the LF note in `docs/reference/tsgo-test-contract.md`
//! rather than relying silently on Linux CI.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::ledger;

/// Summary of one hashed tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeSummary {
    /// Number of regular files.
    pub(crate) files: usize,
    /// Total raw bytes.
    pub(crate) bytes: u64,
    /// Lowercase hex SHA-256.
    pub(crate) sha256: String,
}

/// One regular file discovered under a tree root.
#[derive(Debug, Clone)]
pub(crate) struct TreeFile {
    /// Root-relative path with `/` separators.
    pub(crate) relative_path: String,
    /// Absolute path used for reading.
    pub(crate) absolute_path: PathBuf,
}

struct FrozenTree {
    path: &'static str,
    files: usize,
    bytes: u64,
    sha256: &'static str,
}

const FROZEN_TREES: &[FrozenTree] = &[
    FrozenTree {
        path: "backends/typescript/test",
        files: 22,
        bytes: 253_965,
        sha256: "630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e",
    },
    FrozenTree {
        path: "backends/typescript/samples",
        files: 35,
        bytes: 6_069,
        sha256: "00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f",
    },
    FrozenTree {
        path: "tests/fixtures",
        files: 136,
        bytes: 61_928,
        sha256: "7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9",
    },
    FrozenTree {
        path: "crates/wxml-lsp/tests/fixtures",
        files: 43,
        bytes: 6_456,
        sha256: "834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073",
    },
];

/// Collects regular files under `root`, rejecting symlinks and special files.
pub(crate) fn collect_tree_files(root: &Path) -> Result<Vec<TreeFile>, String> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("symlink_metadata {}: {error}", root.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing symlinked fixture root: {}",
            root.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "fixture root must be a directory: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    Ok(files)
}

/// Computes the frozen v1 tree hash.
pub(crate) fn hash_tree(root: &Path) -> Result<TreeSummary, String> {
    let mut files = collect_tree_files(root)?;
    files.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
    });

    let mut hasher = Sha256::new();
    let mut total_bytes = 0_u64;
    for file in &files {
        let bytes = fs::read(&file.absolute_path).map_err(|error| {
            format!(
                "read fixture file {}: {error}",
                file.absolute_path.display()
            )
        })?;
        hasher.update(file.relative_path.as_bytes());
        hasher.update([0_u8]);
        hasher.update(&bytes);
        hasher.update([0_u8]);
        total_bytes = total_bytes
            .checked_add(
                u64::try_from(bytes.len())
                    .map_err(|_| format!("file is too large: {}", file.absolute_path.display()))?,
            )
            .ok_or_else(|| "fixture tree byte count overflowed u64".to_owned())?;
    }

    Ok(TreeSummary {
        files: files.len(),
        bytes: total_bytes,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn walk(root: &Path, directory: &Path, files: &mut Vec<TreeFile>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read_dir {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("read_dir entry under {}: {error}", directory.display()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("symlink_metadata {}: {error}", path.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(format!(
                "refusing symlink in fixture tree: {}",
                path.display()
            ));
        }
        if file_type.is_dir() {
            walk(root, &path, files)?;
        } else if file_type.is_file() {
            let relative_path = ledger::relative_slash(root, &path)?;
            files.push(TreeFile {
                relative_path,
                absolute_path: path,
            });
        } else {
            return Err(format!(
                "refusing non-regular file in fixture tree: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[test]
fn fixture_tree_hashes_match_frozen_contract() {
    let workspace = ledger::workspace_root();
    for tree in FROZEN_TREES {
        let summary = hash_tree(&workspace.join(tree.path))
            .unwrap_or_else(|error| panic!("hash {}: {error}", tree.path));
        assert_eq!(
            summary.files, tree.files,
            "file count drift in {}; a fixture change requires a deliberate ledger update",
            tree.path
        );
        assert_eq!(
            summary.bytes, tree.bytes,
            "byte count drift in {}; a fixture change requires a deliberate ledger update",
            tree.path
        );
        assert_eq!(
            summary.sha256, tree.sha256,
            "v1 SHA-256 drift in {}; never update the hash just to mask content changes",
            tree.path
        );
    }

    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let fixture_trees = ledger::required_object(root, "fixture_trees", "ledger root");
    let algorithm = ledger::required_object(fixture_trees, "algorithm", "ledger.fixture_trees");
    assert_eq!(
        ledger::required_str(algorithm, "name", "ledger.fixture_trees.algorithm"),
        "v1"
    );
    assert_eq!(
        ledger::required_u64(
            algorithm,
            "deterministic_runs",
            "ledger.fixture_trees.algorithm"
        ),
        2
    );
    assert_eq!(
        ledger::required_u64(algorithm, "symlinks", "ledger.fixture_trees.algorithm"),
        0
    );

    let declared_trees = ledger::required_array(fixture_trees, "trees", "ledger.fixture_trees");
    assert_eq!(declared_trees.len(), FROZEN_TREES.len());
    let mut declared: BTreeMap<&str, (u64, u64, &str)> = BTreeMap::new();
    for entry in declared_trees {
        let entry = ledger::object(entry, "ledger.fixture_trees.trees[]");
        let path = ledger::required_str(entry, "tree", "fixture tree");
        let files = ledger::required_u64(entry, "files", path);
        let bytes = ledger::required_u64(entry, "bytes", path);
        let sha256 = ledger::required_str(entry, "sha256", path);
        assert!(
            declared.insert(path, (files, bytes, sha256)).is_none(),
            "duplicate fixture tree {path}"
        );
    }

    for tree in FROZEN_TREES {
        let (files, bytes, sha256) = declared
            .get(tree.path)
            .unwrap_or_else(|| panic!("ledger is missing fixture tree {}", tree.path));
        assert_eq!(
            *files,
            u64::try_from(tree.files).expect("file count fits u64")
        );
        assert_eq!(*bytes, tree.bytes);
        assert_eq!(*sha256, tree.sha256);
    }

    let reconciliation = ledger::required_object(root, "plan_reconciliation", "ledger root");
    let fixture_counts = ledger::required_object(
        reconciliation,
        "fixture_counts",
        "ledger.plan_reconciliation",
    );
    assert_eq!(fixture_counts.len(), FROZEN_TREES.len());
    for tree in FROZEN_TREES {
        assert_eq!(
            ledger::required_u64(
                fixture_counts,
                tree.path,
                "ledger.plan_reconciliation.fixture_counts"
            ),
            u64::try_from(tree.files).expect("file count fits u64"),
            "plan_reconciliation.fixture_counts drifted for {}",
            tree.path
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_fixture_root_is_rejected() {
    use std::os::unix::fs::symlink;

    let base = std::env::temp_dir().join(format!("wxml-parity-symlink-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(base.join("real")).expect("real fixture directory");
    fs::write(base.join("real/file.txt"), b"x").expect("fixture file");
    symlink(base.join("real"), base.join("link")).expect("fixture symlink");

    let error = hash_tree(&base.join("link")).expect_err("a symlinked root must fail");
    assert!(error.contains("symlink"), "{error}");

    fs::remove_dir_all(&base).expect("cleanup");
}
