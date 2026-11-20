//! Unit tests for the frozen legacy oracle manifest.
//!
//! These tests use a synthetic one-file/three-case oracle so they keep running
//! after `backends/typescript/test` is deleted. The production gate uses the
//! same parser and verifier with the real 21/339/4 inventory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use super::*;

const SAMPLE_FILE: &str = "backends/typescript/test/alpha.test.ts";
const SAMPLE_SUPPORT: &str = "backends/typescript/test/support.ts";
const SAMPLE_TREE: &str = "tests/fixtures";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempWorkspace {
    root: PathBuf,
}

impl TempWorkspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "wxml-oracle-manifest-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("temporary workspace");
        Self { root }
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("temporary parent directory");
        }
        fs::write(path, text).expect("temporary file");
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn sample_source() -> String {
    concat!(
        "test(\"alpha\", () => {\n",
        "  assert.ok(true);\n",
        "});\n",
        "test(\"beta\", () => {\n",
        "  assert.equal(1, 1);\n",
        "});\n",
        "test(\"gamma\", () => {});\n",
    )
    .to_owned()
}

fn sample_support() -> String {
    "export const marker = 1;\n".to_owned()
}

fn body_hash(source: &str, line: usize) -> String {
    let range = source_scan::test_call_range(source, line).expect("sample test declaration");
    hash_bytes(source[range].as_bytes())
}

fn sample_manifest() -> Value {
    let source = sample_source();
    let support = sample_support();
    let lines = u64::try_from(source.lines().count()).expect("line count");
    json!({
        "schema": MANIFEST_SCHEMA,
        "generated_at": "2026-11-20T00:00:00+08:00",
        "baseline": {
            "head": ORACLE_BASELINE_HEAD,
            "branch": "tsgo/migration",
            "source_root": LEGACY_TEST_SOURCE_ROOT,
            "ledger_schema": LEDGER_SCHEMA,
        },
        "algorithm": {
            "name": "v1",
            "path_order": "LC_ALL=C byte order of the UTF-8 relative path",
            "case_body": "sha256 of the exact CallExpression text",
            "file_hash": "sha256 of the raw file bytes",
            "line_count": "str::lines() count",
            "tree": "regular files only; sorted path + NUL + bytes + NUL",
            "symlinks": 0,
        },
        "counts": {
            "test_files": 1,
            "support_files": 1,
            "cases": 3,
            "fixture_trees": 1,
        },
        "support_file": {
            "path": SAMPLE_SUPPORT,
            "sha256": hash_bytes(support.as_bytes()),
            "lines": 1,
        },
        "files": [{
            "file": SAMPLE_FILE,
            "sha256": hash_bytes(source.as_bytes()),
            "lines": lines,
            "test_declarations": 3,
            "literal_declarations": 3,
            "dynamic_declarations": 0,
            "registrations": 3,
        }],
        "fixture_trees": [{
            "tree": SAMPLE_TREE,
            "files": 1,
            "bytes": 2,
            "sha256": "0".repeat(64),
            "archived_ok": false,
        }],
        "cases": [
            {
                "case_id": "TS-ALPHA-0001",
                "file": SAMPLE_FILE,
                "line": 1,
                "body_sha256": body_hash(&source, 1),
            },
            {
                "case_id": "TS-ALPHA-0002",
                "file": SAMPLE_FILE,
                "line": 4,
                "body_sha256": body_hash(&source, 4),
            },
            {
                "case_id": "TS-ALPHA-0003",
                "file": SAMPLE_FILE,
                "line": 7,
                "body_sha256": body_hash(&source, 7),
            },
        ],
        "projection_sha256": "",
    })
}

fn sample_ledger() -> Value {
    let source = sample_source();
    let support = sample_support();
    json!({
        "counts": { "test_files": 1, "cases_in_ledger": 3 },
        "files": [{
            "file": SAMPLE_FILE,
            "sha256": hash_bytes(source.as_bytes()),
            "test_declarations": 3,
            "literal_declarations": 3,
            "dynamic_declarations": 0,
            "registrations": 3,
        }],
        "support_file": {
            "path": SAMPLE_SUPPORT,
            "sha256": hash_bytes(support.as_bytes()),
        },
        "fixture_trees": {
            "trees": [{
                "tree": SAMPLE_TREE,
                "files": 1,
                "bytes": 2,
                "sha256": "0".repeat(64),
            }],
        },
        "cases": [
            {
                "case_id": "TS-ALPHA-0001",
                "source": {
                    "file": SAMPLE_FILE,
                    "line": 1,
                    "body_sha256": body_hash(&source, 1),
                },
            },
            {
                "case_id": "TS-ALPHA-0002",
                "source": {
                    "file": SAMPLE_FILE,
                    "line": 4,
                    "body_sha256": body_hash(&source, 4),
                },
            },
            {
                "case_id": "TS-ALPHA-0003",
                "source": {
                    "file": SAMPLE_FILE,
                    "line": 7,
                    "body_sha256": body_hash(&source, 7),
                },
            },
        ],
    })
}

fn refresh_projection(value: &mut Value) {
    let root = value.as_object().expect("manifest object");
    let manifest = parse_parts(root, "test").expect("valid manifest parts");
    let digest = canonical_projection_sha256(&manifest);
    value
        .as_object_mut()
        .expect("manifest object")
        .insert("projection_sha256".to_owned(), Value::String(digest));
}

fn manifest_text(value: &Value) -> String {
    serde_json::to_string(value).expect("serialize manifest")
}

fn parse_sample(value: &mut Value) -> OracleManifest {
    refresh_projection(value);
    parse_manifest(&manifest_text(value), "test", None, None).expect("valid sample manifest")
}

fn parse_error(value: &Value) -> String {
    parse_manifest(&manifest_text(value), "test", None, None).expect_err("manifest must fail")
}

fn case_object(value: &mut Value, index: usize) -> &mut Map<String, Value> {
    value["cases"][index]
        .as_object_mut()
        .expect("sample case object")
}

fn write_sample_source(workspace: &TempWorkspace) {
    workspace.write("alpha.test.ts", &sample_source());
    workspace.write("support.ts", &sample_support());
}

#[test]
fn manifest_parses_and_cross_checks_the_ledger() {
    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.cases.len(), 3);
    assert_eq!(manifest.support.path, SAMPLE_SUPPORT);
    verify_frozen_projection(&sample_ledger(), &manifest).expect("ledger cross-check");
}

#[test]
#[should_panic(expected = "legacy oracle manifest is required")]
fn missing_manifest_is_a_hard_failure() {
    let workspace = TempWorkspace::new();
    let _manifest = load_manifest(workspace.path());
}

#[test]
fn frozen_mode_needs_no_live_source() {
    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    let workspace = TempWorkspace::new();
    let report = verify_legacy_oracle(
        &sample_ledger(),
        &manifest,
        &workspace.path().join("deleted"),
    )
    .expect("frozen verification");
    assert_eq!(report.mode, LegacySourceMode::Frozen);
    assert_eq!(report.files, 1);
    assert_eq!(report.cases, 3);
}

#[test]
fn live_mode_rehashes_the_source() {
    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    let workspace = TempWorkspace::new();
    write_sample_source(&workspace);
    let report = verify_legacy_oracle(&sample_ledger(), &manifest, workspace.path())
        .expect("live verification");
    assert_eq!(report.mode, LegacySourceMode::Live);
    assert_eq!(report.files, 1);
    assert_eq!(report.cases, 3);
}

#[test]
fn live_file_drift_fails_instead_of_masking_the_manifest() {
    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    let workspace = TempWorkspace::new();
    workspace.write(
        "alpha.test.ts",
        &sample_source().replace("assert.ok(true);", "assert.ok(false);"),
    );
    workspace.write("support.ts", &sample_support());
    let error = verify_legacy_oracle(&sample_ledger(), &manifest, workspace.path())
        .expect_err("live drift must fail");
    assert!(error.contains("file hash drift"), "{error}");
}

#[test]
fn live_extra_file_is_rejected() {
    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    let workspace = TempWorkspace::new();
    write_sample_source(&workspace);
    workspace.write("extra.test.ts", "test(\"extra\", () => {});\n");
    let error = verify_legacy_oracle(&sample_ledger(), &manifest, workspace.path())
        .expect_err("extra source must fail");
    assert!(
        error.contains("does not match the frozen inventory"),
        "{error}"
    );
    assert!(error.contains("extra.test.ts"), "{error}");
}

#[cfg(unix)]
#[test]
fn live_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    let workspace = TempWorkspace::new();
    write_sample_source(&workspace);
    symlink(
        workspace.path().join("alpha.test.ts"),
        workspace.path().join("link.test.ts"),
    )
    .expect("synthetic symlink");
    let error = verify_legacy_oracle(&sample_ledger(), &manifest, workspace.path())
        .expect_err("symlink must fail");
    assert!(error.contains("symlink"), "{error}");
}

#[test]
fn live_root_that_is_a_file_is_rejected() {
    let mut value = sample_manifest();
    let manifest = parse_sample(&mut value);
    let workspace = TempWorkspace::new();
    workspace.write("not-a-directory", "x");
    let error = verify_legacy_oracle(
        &sample_ledger(),
        &manifest,
        &workspace.path().join("not-a-directory"),
    )
    .expect_err("file root must fail");
    assert!(error.contains("must be a directory"), "{error}");
}

#[test]
fn tampered_body_hash_is_rejected_by_the_projection() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    case_object(&mut value, 0).insert("body_sha256".to_owned(), Value::String("a".repeat(64)));
    let error = parse_error(&value);
    assert!(error.contains("projection_sha256"), "{error}");
}

#[test]
fn tampered_file_hash_is_rejected_by_the_projection() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    value["files"][0]["sha256"] = Value::String("b".repeat(64));
    let error = parse_error(&value);
    assert!(error.contains("projection_sha256"), "{error}");
}

#[test]
fn tampered_support_hash_is_rejected_by_the_projection() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    value["support_file"]["sha256"] = Value::String("c".repeat(64));
    let error = parse_error(&value);
    assert!(error.contains("projection_sha256"), "{error}");
}

#[test]
fn tampered_tree_hash_is_rejected_by_the_projection() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    value["fixture_trees"][0]["sha256"] = Value::String("d".repeat(64));
    let error = parse_error(&value);
    assert!(error.contains("projection_sha256"), "{error}");
}

#[test]
fn case_count_tamper_is_rejected() {
    let mut value = sample_manifest();
    value["cases"].as_array_mut().expect("cases").pop();
    let error = parse_error(&value);
    assert!(error.contains("counts.cases"), "{error}");
}

#[test]
fn missing_case_field_is_rejected() {
    let mut value = sample_manifest();
    case_object(&mut value, 0).remove("line");
    let error = parse_error(&value);
    assert!(error.contains("line"), "{error}");
}

#[test]
fn extra_top_level_key_is_rejected() {
    let mut value = sample_manifest();
    value
        .as_object_mut()
        .expect("manifest object")
        .insert("unexpected".to_owned(), Value::Bool(true));
    let error = parse_error(&value);
    assert!(error.contains("unexpected key set"), "{error}");
}

#[test]
fn unsorted_cases_are_rejected() {
    let mut value = sample_manifest();
    value["cases"].as_array_mut().expect("cases").swap(0, 1);
    let root = value.as_object().expect("manifest object");
    let error = parse_parts(root, "test").expect_err("unsorted cases must fail");
    assert!(error.contains("sorted"), "{error}");
}

#[test]
fn duplicate_case_id_is_rejected() {
    let mut value = sample_manifest();
    case_object(&mut value, 1).insert(
        "case_id".to_owned(),
        Value::String("TS-ALPHA-0001".to_owned()),
    );
    let root = value.as_object().expect("manifest object");
    let error = parse_parts(root, "test").expect_err("duplicate case must fail");
    assert!(error.contains("duplicate case_id"), "{error}");
}

#[test]
fn consistently_tampered_body_hash_is_caught_by_the_ledger() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    case_object(&mut value, 1).insert("body_sha256".to_owned(), Value::String("e".repeat(64)));
    refresh_projection(&mut value);
    let manifest = parse_manifest(&manifest_text(&value), "test", None, None)
        .expect("self-consistent manifest");
    let error = verify_frozen_projection(&sample_ledger(), &manifest)
        .expect_err("ledger must catch the tamper");
    assert!(error.contains("case anchor drift"), "{error}");
}

#[test]
fn consistently_tampered_file_hash_is_caught_by_the_ledger() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    value["files"][0]["sha256"] = Value::String("f".repeat(64));
    refresh_projection(&mut value);
    let manifest = parse_manifest(&manifest_text(&value), "test", None, None)
        .expect("self-consistent manifest");
    let error = verify_frozen_projection(&sample_ledger(), &manifest)
        .expect_err("ledger must catch the tamper");
    assert!(error.contains("file hash drift"), "{error}");
}

#[test]
fn case_count_disagreement_is_caught_by_the_ledger() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    value["cases"].as_array_mut().expect("cases").pop();
    value["counts"]["cases"] = Value::from(2_u64);
    refresh_projection(&mut value);
    let manifest = parse_manifest(&manifest_text(&value), "test", None, None)
        .expect("self-consistent manifest");
    let error = verify_frozen_projection(&sample_ledger(), &manifest)
        .expect_err("ledger must catch the count");
    assert!(
        error.contains("manifest has 2 cases") || error.contains("ledger.cases has 3 entries"),
        "{error}"
    );
}

#[test]
fn support_hash_disagreement_is_caught_by_the_ledger() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    value["support_file"]["sha256"] = Value::String("9".repeat(64));
    refresh_projection(&mut value);
    let manifest = parse_manifest(&manifest_text(&value), "test", None, None)
        .expect("self-consistent manifest");
    let error = verify_frozen_projection(&sample_ledger(), &manifest)
        .expect_err("ledger must catch the support drift");
    assert!(error.contains("support.ts drift"), "{error}");
}

#[test]
fn pinned_projection_and_head_are_enforced() {
    let mut value = sample_manifest();
    refresh_projection(&mut value);
    let text = manifest_text(&value);
    let digest = value["projection_sha256"]
        .as_str()
        .expect("projection digest")
        .to_owned();
    let wrong_digest = "0".repeat(64);
    let error = parse_manifest(&text, "test", Some(&wrong_digest), None)
        .expect_err("wrong digest must fail");
    assert!(error.contains("frozen pin"), "{error}");

    let wrong_head = "0".repeat(40);
    let error = parse_manifest(&text, "test", Some(&digest), Some(&wrong_head))
        .expect_err("wrong head must fail");
    assert!(error.contains("baseline.head"), "{error}");
}
