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

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::ledger;
use crate::oracle_manifest::{self, OracleManifest};

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

/// Frozen expectation for one fixture tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrozenTree {
    /// Repository-relative tree path.
    pub(crate) path: &'static str,
    /// Expected regular-file count.
    pub(crate) files: usize,
    /// Expected total raw bytes.
    pub(crate) bytes: u64,
    /// Expected v1 SHA-256.
    pub(crate) sha256: &'static str,
    /// True when the tree may be absent after P7 deletion.
    pub(crate) legacy_archived: bool,
}

/// Outcome of checking one frozen tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeStatus {
    /// The tree exists and its bytes were re-hashed.
    Live(TreeSummary),
    /// The legacy tree is gone; the frozen records were cross-checked.
    Archived,
}

const FROZEN_TREES: &[FrozenTree] = &[
    FrozenTree {
        path: "backends/typescript/test",
        files: 22,
        bytes: 253_965,
        sha256: "630a16af41623c78f19f47bcaba1a7f1282201ac27b4414115ac0b3764afc32e",
        legacy_archived: true,
    },
    FrozenTree {
        path: "backends/typescript/samples",
        files: 35,
        bytes: 6_069,
        sha256: "00a7fa3d6962da50581c396f73c13f7ffb043efae060a0801b373928726e5c1f",
        legacy_archived: true,
    },
    FrozenTree {
        path: "tests/fixtures",
        files: 136,
        bytes: 61_928,
        sha256: "7c9ddedda18c70bc217af6b3bbd23dd22a33be9c885276cbd587adc0596f04a9",
        legacy_archived: false,
    },
    FrozenTree {
        path: "crates/wxml-lsp/tests/fixtures",
        files: 43,
        bytes: 6_456,
        sha256: "834eea4defcb7565c4b84e91a173079cf3be85201d44fb4a7a6d0a32af3da073",
        legacy_archived: false,
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

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a Map<String, Value>, String> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{context}.{key} must be a JSON object"))
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a [Value], String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context}.{key} must be an array"))
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context}.{key} must be a string"))
}

fn required_u64(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64, String> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{context}.{key} must be an unsigned integer"))
}

/// Verifies the four frozen fixture trees against the live bytes (when
/// present), the ledger, and the legacy oracle manifest.
///
/// A legacy tree may be absent after P7 deletion only when the manifest and the
/// ledger agree on its frozen record. A retained tree (`tests/fixtures`,
/// `crates/wxml-lsp/tests/fixtures`) is always required.
pub(crate) fn verify_fixture_trees(
    workspace: &Path,
    ledger_value: &Value,
    manifest: &OracleManifest,
    expected: &[FrozenTree],
) -> Result<BTreeMap<String, TreeStatus>, String> {
    let root = ledger_value
        .as_object()
        .ok_or_else(|| "ledger root must be a JSON object".to_owned())?;
    let fixture_trees = required_object(root, "fixture_trees", "ledger")?;
    let algorithm = required_object(fixture_trees, "algorithm", "ledger.fixture_trees")?;
    if required_str(algorithm, "name", "ledger.fixture_trees.algorithm")? != "v1" {
        return Err("ledger.fixture_trees.algorithm.name must be v1".to_owned());
    }
    if required_u64(
        algorithm,
        "deterministic_runs",
        "ledger.fixture_trees.algorithm",
    )? != 2
    {
        return Err("ledger.fixture_trees.algorithm.deterministic_runs must be 2".to_owned());
    }
    if required_u64(algorithm, "symlinks", "ledger.fixture_trees.algorithm")? != 0 {
        return Err("ledger.fixture_trees.algorithm.symlinks must be 0".to_owned());
    }

    let declared = required_array(fixture_trees, "trees", "ledger.fixture_trees")?;
    if declared.len() != expected.len() {
        return Err(format!(
            "ledger.fixture_trees.trees has {} entries, expected {}",
            declared.len(),
            expected.len()
        ));
    }
    let mut declared_by_path = BTreeMap::new();
    for entry in declared {
        let entry = entry
            .as_object()
            .ok_or_else(|| "ledger.fixture_trees.trees[] must be an object".to_owned())?;
        let path = required_str(entry, "tree", "ledger.fixture_trees.trees[]")?;
        let record = (
            required_u64(entry, "files", path)?,
            required_u64(entry, "bytes", path)?,
            required_str(entry, "sha256", path)?,
        );
        if declared_by_path.insert(path.to_owned(), record).is_some() {
            return Err(format!(
                "ledger.fixture_trees contains duplicate tree {path}"
            ));
        }
    }

    for tree in expected {
        let Some((files, bytes, sha256)) = declared_by_path.get(tree.path) else {
            return Err(format!("ledger.fixture_trees is missing {}", tree.path));
        };
        if *files
            != u64::try_from(tree.files).map_err(|_| "file count overflowed u64".to_owned())?
            || *bytes != tree.bytes
            || *sha256 != tree.sha256
        {
            return Err(format!(
                "ledger tree record drift for {}: ledger ({files}, {bytes}, {sha256}) \
                 != expected ({}, {}, {})",
                tree.path, tree.files, tree.bytes, tree.sha256
            ));
        }

        let manifest_tree = manifest
            .trees
            .iter()
            .find(|entry| entry.tree == tree.path)
            .ok_or_else(|| format!("legacy oracle manifest is missing tree {}", tree.path))?;
        if manifest_tree.files
            != u64::try_from(tree.files).map_err(|_| "file count overflowed u64".to_owned())?
            || manifest_tree.bytes != tree.bytes
            || manifest_tree.sha256 != tree.sha256
        {
            return Err(format!(
                "manifest tree hash drift for {}: manifest ({}, {}, {}) \
                 != expected ({}, {}, {})",
                tree.path,
                manifest_tree.files,
                manifest_tree.bytes,
                manifest_tree.sha256,
                tree.files,
                tree.bytes,
                tree.sha256
            ));
        }
        if manifest_tree.archived_ok != tree.legacy_archived {
            return Err(format!(
                "manifest archive policy drift for {}: archived_ok is {}, expected {}",
                tree.path, manifest_tree.archived_ok, tree.legacy_archived
            ));
        }
    }

    let reconciliation = required_object(root, "plan_reconciliation", "ledger")?;
    let fixture_counts = required_object(
        reconciliation,
        "fixture_counts",
        "ledger.plan_reconciliation",
    )?;
    if fixture_counts.len() != expected.len() {
        return Err(format!(
            "ledger.plan_reconciliation.fixture_counts has {} entries, expected {}",
            fixture_counts.len(),
            expected.len()
        ));
    }
    for tree in expected {
        let declared = required_u64(
            fixture_counts,
            tree.path,
            "ledger.plan_reconciliation.fixture_counts",
        )?;
        if declared
            != u64::try_from(tree.files).map_err(|_| "file count overflowed u64".to_owned())?
        {
            return Err(format!(
                "plan_reconciliation.fixture_counts drifted for {}: {declared} != {}",
                tree.path, tree.files
            ));
        }
    }

    let mut statuses = BTreeMap::new();
    for tree in expected {
        let path = workspace.join(tree.path);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let summary =
                    hash_tree(&path).map_err(|error| format!("hash {}: {error}", tree.path))?;
                if summary.files != tree.files {
                    return Err(format!(
                        "file count drift in {}: live {} != frozen {}",
                        tree.path, summary.files, tree.files
                    ));
                }
                if summary.bytes != tree.bytes {
                    return Err(format!(
                        "byte count drift in {}: live {} != frozen {}",
                        tree.path, summary.bytes, tree.bytes
                    ));
                }
                if summary.sha256 != tree.sha256 {
                    return Err(format!(
                        "v1 SHA-256 drift in {}: live {} != frozen {}",
                        tree.path, summary.sha256, tree.sha256
                    ));
                }
                statuses.insert(tree.path.to_owned(), TreeStatus::Live(summary));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !tree.legacy_archived {
                    return Err(format!(
                        "required fixture tree is missing: {} (only archived legacy trees may be absent)",
                        tree.path
                    ));
                }
                statuses.insert(tree.path.to_owned(), TreeStatus::Archived);
            }
            Err(error) => {
                return Err(format!("stat fixture tree {}: {error}", path.display()));
            }
        }
    }
    Ok(statuses)
}

#[test]
fn fixture_tree_hashes_match_frozen_contract() {
    let workspace = ledger::workspace_root();
    let ledger_value = ledger::load_ledger();
    let manifest = oracle_manifest::load_manifest(&workspace);
    let statuses = verify_fixture_trees(&workspace, &ledger_value, &manifest, FROZEN_TREES)
        .unwrap_or_else(|error| panic!("fixture tree gate: {error}"));
    for tree in FROZEN_TREES {
        let live = workspace.join(tree.path).is_dir();
        match statuses.get(tree.path).expect("tree status") {
            TreeStatus::Live(summary) => {
                assert!(
                    live,
                    "{} reported live but the directory is absent",
                    tree.path
                );
                assert_eq!(summary.files, tree.files);
                assert_eq!(summary.bytes, tree.bytes);
                assert_eq!(summary.sha256, tree.sha256);
            }
            TreeStatus::Archived => {
                assert!(
                    !live,
                    "{} is live but the gate reported it archived",
                    tree.path
                );
                assert!(
                    tree.legacy_archived,
                    "{} is not allowed to be archived",
                    tree.path
                );
            }
        }
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

mod synthetic_tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;
    use crate::oracle_manifest::{ManifestSupport, ManifestTree, OracleManifest};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "wxml-fixture-gate-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("temporary tree");
            Self { root }
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("temporary parent");
            }
            fs::write(path, text).expect("temporary file");
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn static_sha(value: &str) -> &'static str {
        Box::leak(value.to_owned().into_boxed_str())
    }

    fn expected_tree(
        path: &'static str,
        summary: &TreeSummary,
        legacy_archived: bool,
    ) -> FrozenTree {
        FrozenTree {
            path,
            files: summary.files,
            bytes: summary.bytes,
            sha256: static_sha(&summary.sha256),
            legacy_archived,
        }
    }

    fn manifest_with(tree: &FrozenTree) -> OracleManifest {
        OracleManifest {
            schema: oracle_manifest::MANIFEST_SCHEMA.to_owned(),
            generated_at: "2026-11-20T00:00:00+08:00".to_owned(),
            head: oracle_manifest::ORACLE_BASELINE_HEAD.to_owned(),
            branch: "tsgo/migration".to_owned(),
            source_root: "backends/typescript/test".to_owned(),
            ledger_schema: "wxml-tsgo-test-ledger/1".to_owned(),
            support: ManifestSupport {
                path: "backends/typescript/test/support.ts".to_owned(),
                sha256: "0".repeat(64),
                lines: 1,
            },
            files: Vec::new(),
            trees: vec![ManifestTree {
                tree: tree.path.to_owned(),
                files: u64::try_from(tree.files).expect("file count"),
                bytes: tree.bytes,
                sha256: tree.sha256.to_owned(),
                archived_ok: tree.legacy_archived,
            }],
            cases: Vec::new(),
            projection_sha256: String::new(),
        }
    }

    fn ledger_with(tree: &FrozenTree) -> Value {
        let mut counts = Map::new();
        counts.insert(
            tree.path.to_owned(),
            Value::from(u64::try_from(tree.files).expect("file count")),
        );
        json!({
            "fixture_trees": {
                "algorithm": {
                    "name": "v1",
                    "deterministic_runs": 2,
                    "symlinks": 0,
                },
                "trees": [{
                    "tree": tree.path,
                    "files": u64::try_from(tree.files).expect("file count"),
                    "bytes": tree.bytes,
                    "sha256": tree.sha256,
                }],
            },
            "plan_reconciliation": {
                "fixture_counts": Value::Object(counts),
            },
        })
    }

    #[test]
    fn live_tree_is_rehashed_and_matches() {
        let temp = TempTree::new();
        temp.write("legacy/tree/a.txt", "x");
        let summary = hash_tree(&temp.root.join("legacy/tree")).expect("tree hash");
        let expected = [expected_tree("legacy/tree", &summary, true)];
        let statuses = verify_fixture_trees(
            &temp.root,
            &ledger_with(&expected[0]),
            &manifest_with(&expected[0]),
            &expected,
        )
        .expect("live tree gate");
        assert!(matches!(
            statuses.get("legacy/tree"),
            Some(TreeStatus::Live(_))
        ));
    }

    #[test]
    fn archived_legacy_tree_passes_without_live_bytes() {
        let summary = TreeSummary {
            files: 1,
            bytes: 1,
            sha256: "0".repeat(64),
        };
        let expected = [expected_tree("backends/typescript/test", &summary, true)];
        let temp = TempTree::new();
        let statuses = verify_fixture_trees(
            &temp.root,
            &ledger_with(&expected[0]),
            &manifest_with(&expected[0]),
            &expected,
        )
        .expect("archived legacy tree");
        assert_eq!(
            statuses.get("backends/typescript/test"),
            Some(&TreeStatus::Archived)
        );
    }

    #[test]
    fn missing_retained_tree_fails() {
        let summary = TreeSummary {
            files: 1,
            bytes: 1,
            sha256: "0".repeat(64),
        };
        let expected = [expected_tree("tests/fixtures", &summary, false)];
        let temp = TempTree::new();
        let error = verify_fixture_trees(
            &temp.root,
            &ledger_with(&expected[0]),
            &manifest_with(&expected[0]),
            &expected,
        )
        .expect_err("a retained tree is required");
        assert!(
            error.contains("required fixture tree is missing"),
            "{error}"
        );
    }

    #[test]
    fn tampered_manifest_tree_hash_fails() {
        let temp = TempTree::new();
        temp.write("legacy/tree/a.txt", "x");
        let summary = hash_tree(&temp.root.join("legacy/tree")).expect("tree hash");
        let expected = [expected_tree("legacy/tree", &summary, true)];
        let mut manifest = manifest_with(&expected[0]);
        manifest.trees[0].sha256 = "f".repeat(64);
        let error =
            verify_fixture_trees(&temp.root, &ledger_with(&expected[0]), &manifest, &expected)
                .expect_err("manifest tamper must fail");
        assert!(error.contains("manifest tree hash drift"), "{error}");
    }

    #[test]
    fn tampered_ledger_tree_count_fails() {
        let temp = TempTree::new();
        temp.write("legacy/tree/a.txt", "x");
        let summary = hash_tree(&temp.root.join("legacy/tree")).expect("tree hash");
        let expected = [expected_tree("legacy/tree", &summary, true)];
        let mut ledger = ledger_with(&expected[0]);
        ledger["fixture_trees"]["trees"][0]["files"] = Value::from(999_u64);
        let error =
            verify_fixture_trees(&temp.root, &ledger, &manifest_with(&expected[0]), &expected)
                .expect_err("ledger tamper must fail");
        assert!(error.contains("ledger tree record drift"), "{error}");
    }

    #[test]
    fn live_tree_drift_fails() {
        let temp = TempTree::new();
        temp.write("legacy/tree/a.txt", "x");
        let summary = hash_tree(&temp.root.join("legacy/tree")).expect("tree hash");
        let expected = [expected_tree("legacy/tree", &summary, true)];
        temp.write("legacy/tree/a.txt", "y");
        let error = verify_fixture_trees(
            &temp.root,
            &ledger_with(&expected[0]),
            &manifest_with(&expected[0]),
            &expected,
        )
        .expect_err("live drift must fail");
        assert!(error.contains("v1 SHA-256 drift"), "{error}");
    }

    fn copy_tree(source: &Path, target: &Path) {
        fs::create_dir_all(target).expect("copied fixture directory");
        let entries = fs::read_dir(source).expect("source fixture directory");
        for entry in entries {
            let entry = entry.expect("source fixture entry");
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("source fixture metadata");
            let destination = target.join(entry.file_name());
            if metadata.is_dir() {
                copy_tree(&path, &destination);
            } else if metadata.is_file() {
                fs::copy(&path, &destination).expect("copied fixture file");
            } else {
                panic!("unexpected non-regular fixture: {}", path.display());
            }
        }
    }

    #[test]
    fn fixture_gate_passes_after_legacy_trees_are_deleted() {
        let workspace = ledger::workspace_root();
        let ledger_value = ledger::load_ledger();
        let manifest = oracle_manifest::load_manifest(&workspace);
        let temp = TempTree::new();
        for tree in FROZEN_TREES.iter().filter(|tree| !tree.legacy_archived) {
            copy_tree(&workspace.join(tree.path), &temp.root.join(tree.path));
        }
        let statuses = verify_fixture_trees(&temp.root, &ledger_value, &manifest, FROZEN_TREES)
            .expect("frozen fixture gate must pass after the legacy trees are deleted");
        for tree in FROZEN_TREES {
            match statuses.get(tree.path).expect("tree status") {
                TreeStatus::Archived => {
                    assert!(tree.legacy_archived, "{} may not be archived", tree.path);
                }
                TreeStatus::Live(_) => {
                    assert!(!tree.legacy_archived, "{} must be archived", tree.path);
                }
            }
        }
    }
}
