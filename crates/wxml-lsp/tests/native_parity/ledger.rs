//! Frozen migration ledger gate.
//!
//! The JSON ledger is the single source of truth for the case inventory and
//! routing. These tests fail loudly when the ledger is missing, has an
//! unexpected schema, drifts from the frozen counts, loses an old source
//! location, or claims a case migrated without complete Rust evidence. They
//! never skip.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::source_scan;

/// Repository-relative ledger path.
pub(crate) const LEDGER_RELATIVE_PATH: &str = "docs/plans/tsgo-test-ledger.json";

/// Frozen expanded registration count.
pub(crate) const TOTAL_CASES: usize = 339;
/// Exact-parity cases promoted to `green`; wave-6 added no new exact case.
pub(crate) const GREEN_CASES: usize = 44;
/// Cases still `planned` after the 2026-11-20 wave-6 backfill.
pub(crate) const PLANNED_CASES: usize = 259;
/// Frozen retired-with-decision count.
pub(crate) const RETIRED_CASES: usize = 36;
/// Near-exact cases intentionally left `planned` with a green recommendation.
pub(crate) const NEAR_EXACT_CASES: usize = 3;
/// Weaker evidence cases left `planned` after the 2026-11-20 wave-6 backfill.
pub(crate) const WEAKER_CASES: usize = 80;
/// Structural evidence cases left `planned` after the 2026-11-20 wave-6 backfill.
pub(crate) const STRUCTURAL_CASES: usize = 7;
/// Blocked evidence cases left `planned` after the 2026-11-20 wave-6 backfill.
pub(crate) const BLOCKED_CASES: usize = 20;

/// Partitions named by the test contract. P3-T creates all 16 skeletons.
pub(crate) const PARTITION_NAMES: &[&str] = &[
    "hover_docs",
    "definitions",
    "completion",
    "loops",
    "guards_js",
    "components",
    "behaviors",
    "events",
    "project_overlay",
    "symbols_edits",
    "scenario_matrix",
    "protocol_limits",
    "messages_evidence",
    "shadow_rewrite",
    "mapping_utf16",
    "perf_budget",
];

const REQUIRED_TOP_LEVEL_KEYS: &[&str] = &[
    "schema",
    "generated_at",
    "baseline",
    "contract",
    "counts",
    "plan_reconciliation",
    "fixture_trees",
    "dirty_increments",
    "dirty_increment_summary",
    "support_file",
    "files",
    "blocked_decisions",
    "ignored_rust_tests",
    "silent_skip_gates",
    "silent_skips",
    "discrepancies",
    "cases",
    "integrator_decision",
];

const ALLOWED_DISPOSITIONS: &[&str] = &["direct", "rewrite", "split", "obsolete"];
const ALLOWED_STATUSES: &[&str] = &["planned", "green", "retired-with-decision"];
const ALLOWED_ORACLE_TYPES: &[&str] = &["assertion", "unit", "parity"];
const ALLOWED_PARITIES: &[&str] = &["exact", "near_exact", "weaker", "structural", "blocked"];
const ALLOWED_RECOMMENDATIONS: &[&str] = &["green", "planned"];
const GREEN_TARGET_STATUS: &str = "migrated_green";
const PLANNED_TARGET_STATUS: &str = "proposed";
const RETIRED_TARGET_STATUS: &str = "not_migrated_internal_evidence";
const ALLOWED_COMPARISON_TYPES: &[&str] = &[
    "semantic_exact",
    "safety_exact",
    "contract_translated",
    "mechanism_replaced",
    "blocked_decision",
];
const COVER_KEYS: &[&str] = &[
    "loops", "guards", "behavior", "props", "events", "symbols", "rename", "unsaved", "utf16",
    "protocol", "recovery", "security",
];
const DIRTY_INCREMENT_IDS: &[&str] = &["D1", "D2", "D3", "D4", "D5", "D6", "D7"];

/// Corrected contract vocabulary from `docs/reference/tsgo-test-contract.md`
/// and the integrator decision. Asserting the exact arrays prevents a stale
/// ledger block from being reintroduced.
const CONTRACT_STATUS_VALUES: &[&str] = &[
    "planned",
    "red",
    "green",
    "reviewed",
    "closed",
    "retired-with-decision",
];
const CONTRACT_DISPOSITION_VALUES: &[&str] =
    &["direct", "rewrite", "split", "obsolete", "blocked-decision"];
const FROZEN_ORACLE_REVISION: &str = "F2 frozen 2026-11-17 (dcc1f15)";
const FROZEN_NOTE_2_SHA256: &str =
    "8bffb12cbd5cc3d91edab637aea67a4d1dbfe0ef926721a09fcc9e636bef8913";
const FROZEN_NOTE_3_SHA256: &str =
    "512436c5617d109411c1a35a04e9c046cd43e4de202092073de2fd9ee08d1d05";
const FROZEN_X8_SHA256: &str = "ae211e05f50a827a9ff4831ebfc731cceddaf84a926c46aaa55e29a503c125cf";

/// Returns the workspace root by walking up to the manifest with `[workspace]`.
pub(crate) fn workspace_root() -> PathBuf {
    let start = Path::new(env!("CARGO_MANIFEST_DIR"));
    for ancestor in start.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        if text.lines().any(|line| line.trim() == "[workspace]") {
            return ancestor.to_path_buf();
        }
    }
    panic!(
        "no Cargo.toml containing [workspace] was found above {}",
        start.display()
    );
}

/// Loads the ledger, panicking with the path when it is missing or invalid.
///
/// A missing ledger is always a failure; there is no skip path.
pub(crate) fn load_ledger() -> Value {
    let path = workspace_root().join(LEDGER_RELATIVE_PATH);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "migration ledger is required but missing/unreadable at {}: {error}",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "migration ledger {} is not valid JSON: {error}",
            path.display()
        )
    })
}

/// Requires a JSON object.
pub(crate) fn object<'a>(value: &'a Value, context: &str) -> &'a Map<String, Value> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{context} must be a JSON object, got {value}"))
}

/// Requires a string field.
pub(crate) fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> &'a str {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{context}.{key} must be a string"))
}

/// Requires an unsigned integer field.
pub(crate) fn required_u64(object: &Map<String, Value>, key: &str, context: &str) -> u64 {
    object
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{context}.{key} must be an unsigned integer"))
}

/// Requires a boolean field.
pub(crate) fn required_bool(object: &Map<String, Value>, key: &str, context: &str) -> bool {
    object
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("{context}.{key} must be a boolean"))
}

/// Requires an array field.
pub(crate) fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> &'a [Value] {
    object
        .get(key)
        .and_then(Value::as_array)
        .map_or_else(|| panic!("{context}.{key} must be an array"), Vec::as_slice)
}

/// Requires an array of strings.
pub(crate) fn required_string_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Vec<&'a str> {
    required_array(object, key, context)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("{context}.{key} must contain only strings"))
        })
        .collect()
}

/// Parses the integer immediately before `label` in a recorded status line.
fn labeled_count(text: &str, label: &str) -> Option<u64> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for window in tokens.windows(2) {
        let word = window[1].trim_matches(|character: char| !character.is_ascii_alphanumeric());
        if word == label {
            let number = window[0].trim_matches(|character: char| !character.is_ascii_digit());
            return number.parse().ok();
        }
    }
    None
}

/// Requires an object field.
pub(crate) fn required_object<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> &'a Map<String, Value> {
    map.get(key).map_or_else(
        || panic!("{context}.{key} must be an object"),
        |value| object(value, &format!("{context}.{key}")),
    )
}

/// Hashes bytes as lowercase SHA-256 hex.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

/// Returns true for a lowercase 64-character SHA-256 hex string.
pub(crate) fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Joins path components with `/` and rejects non-relative or non-UTF-8 paths.
pub(crate) fn slash_path(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| format!("non-UTF-8 path component in {}", path.display()))?,
            ),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("path must be relative: {}", path.display()));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Returns a root-relative path with `/` separators.
pub(crate) fn relative_slash(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path.strip_prefix(root).map_err(|error| {
        format!(
            "{} is not under {}: {error}",
            path.display(),
            root.display()
        )
    })?;
    slash_path(relative)
}

fn tally<'a>(cases: &'a [Value], field: &str, allowed: &[&str]) -> BTreeMap<&'a str, usize> {
    let mut counts = BTreeMap::new();
    for case in cases {
        let case = object(case, "case");
        let value = required_str(case, field, "case");
        assert!(
            allowed.contains(&value),
            "unexpected case {field} {value:?}; allowed: {allowed:?}"
        );
        *counts.entry(value).or_default() += 1;
    }
    counts
}

fn cover_counts(cases: &[Value]) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> =
        COVER_KEYS.iter().map(|key| (*key, 0)).collect();
    for case in cases {
        let case = object(case, "case");
        let covers = required_object(case, "covers", "case");
        assert_eq!(
            covers.len(),
            COVER_KEYS.len(),
            "case {} has an unexpected covers key set",
            required_str(case, "case_id", "case")
        );
        for key in COVER_KEYS.iter().copied() {
            if required_bool(covers, key, "case.covers") {
                *counts.get_mut(key).expect("cover key initialized") += 1;
            }
        }
    }
    counts
}

fn validate_target_module<'a>(target: &'a str, case_id: &str) -> &'a str {
    let (file, test_id) = target
        .split_once("::")
        .unwrap_or_else(|| panic!("{case_id} target lacks a test separator: {target}"));
    let module = file
        .strip_prefix("crates/wxml-lsp/tests/native_parity/")
        .and_then(|file| file.strip_suffix(".rs"))
        .unwrap_or_else(|| {
            panic!("{case_id} target must live under tests/native_parity: {target}")
        });
    assert!(
        PARTITION_NAMES.contains(&module),
        "{case_id} targets unknown partition module {module}"
    );
    assert!(
        test_id.starts_with("case_") && test_id.len() > "case_".len(),
        "{case_id} target test id must start with case_ and be non-empty: {test_id}"
    );
    assert!(
        test_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
        "{case_id} target test id has unexpected characters: {test_id}"
    );
    module
}

fn is_case_id(value: &str) -> bool {
    value.starts_with("TS-")
        && value.len() > 4
        && value.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

/// Returns the native-parity module named by a ledger target path.
pub(crate) fn target_partition(target: &str) -> &str {
    let (file, _) = target
        .split_once("::")
        .unwrap_or_else(|| panic!("target lacks a test separator: {target}"));
    file.rsplit('/')
        .next()
        .and_then(|file| file.strip_suffix(".rs"))
        .unwrap_or_else(|| panic!("target is not a Rust module path: {target}"))
}

/// Counts non-obsolete cases from the actual ledger inventory.
pub(crate) fn non_obsolete_case_count(cases: &[Value]) -> usize {
    cases
        .iter()
        .filter(|case| required_str(object(case, "case"), "disposition", "case") != "obsolete")
        .count()
}

pub(crate) fn case_ids(cases: &[Value]) -> BTreeSet<String> {
    cases
        .iter()
        .map(|case| required_str(object(case, "case"), "case_id", "case").to_owned())
        .collect()
}

#[test]
fn ledger_schema_is_frozen() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");

    for key in REQUIRED_TOP_LEVEL_KEYS {
        assert!(
            root.contains_key(*key),
            "ledger is missing top-level key {key}"
        );
    }
    assert_eq!(
        root.len(),
        REQUIRED_TOP_LEVEL_KEYS.len(),
        "ledger has unexpected top-level keys"
    );
    assert_eq!(
        required_str(root, "schema", "ledger root"),
        "wxml-tsgo-test-ledger/1"
    );
    assert!(!required_str(root, "generated_at", "ledger root").is_empty());

    let contract = required_object(root, "contract", "ledger root");
    assert_eq!(
        required_string_array(contract, "status_values", "ledger.contract").as_slice(),
        CONTRACT_STATUS_VALUES,
        "ledger.contract.status_values drifted from the corrected vocabulary"
    );
    assert_eq!(
        required_string_array(contract, "disposition_values", "ledger.contract").as_slice(),
        CONTRACT_DISPOSITION_VALUES,
        "ledger.contract.disposition_values drifted from the corrected vocabulary"
    );
    assert_eq!(
        required_str(contract, "this_round_status", "ledger.contract"),
        "green"
    );
    assert!(required_bool(
        contract,
        "no_old_test_deleted",
        "ledger.contract"
    ));
    assert!(required_bool(
        contract,
        "old_test_source_hashes_recorded",
        "ledger.contract"
    ));
    assert_eq!(
        required_str(contract, "oracle_revision", "ledger.contract"),
        FROZEN_ORACLE_REVISION,
        "the frozen oracle revision changed"
    );
    let notes = required_string_array(contract, "notes", "ledger.contract");
    assert_eq!(notes.len(), 6, "ledger.contract.notes drifted");
    assert_eq!(
        sha256_hex(notes[2].as_bytes()),
        FROZEN_NOTE_2_SHA256,
        "the stale 'every status is planned' note was reintroduced"
    );
    assert_eq!(
        sha256_hex(notes[3].as_bytes()),
        FROZEN_NOTE_3_SHA256,
        "the stale 'no case is obsolete' note was reintroduced"
    );

    let counts = required_object(root, "counts", "ledger root");
    let support = required_object(root, "support_file", "ledger root");
    let support_files = required_string_array(counts, "support_files", "ledger.counts");
    assert_eq!(
        support_files,
        vec![required_str(support, "path", "ledger.support_file")],
        "counts.support_files must name the single support.ts authority"
    );

    let baseline = required_object(root, "baseline", "ledger root");
    assert_eq!(
        required_str(baseline, "branch", "ledger.baseline"),
        "tsgo/migration"
    );
    assert!(!required_str(baseline, "worktree", "ledger.baseline").is_empty());
    let head = required_str(baseline, "head", "ledger.baseline");
    assert_eq!(head.len(), 40, "baseline.head must be a git object id");
    assert!(
        head.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "baseline.head must be hex: {head}"
    );
    let rust_status = required_str(baseline, "rust_test_status", "ledger.baseline");
    assert_eq!(labeled_count(rust_status, "passed"), Some(1_297));
    assert_eq!(labeled_count(rust_status, "failed"), Some(0));
    assert_eq!(labeled_count(rust_status, "ignored"), Some(23));
    let ts_status = required_str(baseline, "ts_test_status", "ledger.baseline");
    let executed = required_object(counts, "executed", "ledger.counts");
    assert_eq!(
        labeled_count(ts_status, "tests"),
        Some(required_u64(executed, "tests", "ledger.counts.executed"))
    );
    assert_eq!(
        labeled_count(ts_status, "passed"),
        Some(required_u64(executed, "passed", "ledger.counts.executed"))
    );
    assert_eq!(
        labeled_count(ts_status, "failed"),
        Some(required_u64(executed, "failed", "ledger.counts.executed"))
    );
    assert_eq!(
        labeled_count(ts_status, "skipped"),
        Some(required_u64(executed, "skipped", "ledger.counts.executed"))
    );
    let perf = required_object(baseline, "perf_baseline", "ledger.baseline");
    assert_eq!(
        required_u64(perf, "threshold_ms", "ledger.baseline.perf_baseline"),
        8_000
    );
    assert!(!required_bool(
        perf,
        "test_changed",
        "ledger.baseline.perf_baseline"
    ));
    assert!(!required_bool(
        perf,
        "threshold_changed",
        "ledger.baseline.perf_baseline"
    ));

    let reconciliation = required_object(root, "plan_reconciliation", "ledger root");
    assert_eq!(
        required_u64(
            reconciliation,
            "actual_static_declarations",
            "ledger.plan_reconciliation"
        ),
        274
    );
    assert_eq!(
        required_u64(
            reconciliation,
            "actual_expanded_registrations",
            "ledger.plan_reconciliation"
        ),
        339
    );
    assert!(!required_str(reconciliation, "verdict", "ledger.plan_reconciliation").is_empty());
    let rust_counts = required_object(
        reconciliation,
        "rust_attribute_counts",
        "plan_reconciliation",
    );
    assert_eq!(
        required_u64(
            rust_counts,
            "compiled_test_functions",
            "plan_reconciliation.rust_attribute_counts"
        ),
        1_319
    );
    assert_eq!(
        required_u64(
            rust_counts,
            "ignored",
            "plan_reconciliation.rust_attribute_counts"
        ),
        23
    );

    assert!(
        required_array(root, "blocked_decisions", "ledger root").is_empty(),
        "P3-T requires zero blocked decisions"
    );

    let discrepancies = required_array(root, "discrepancies", "ledger root");
    assert_eq!(discrepancies.len(), 12, "the discrepancy register drifted");
    let mut discrepancy_ids = BTreeSet::new();
    for discrepancy in discrepancies {
        let discrepancy = object(discrepancy, "ledger.discrepancies[]");
        let id = required_str(discrepancy, "id", "ledger.discrepancies[]");
        assert!(
            discrepancy_ids.insert(id.to_owned()),
            "duplicate discrepancy {id}"
        );
        for key in [
            "severity",
            "area",
            "claim",
            "evidence",
            "root_cause",
            "action",
        ] {
            assert!(
                !required_str(discrepancy, key, &format!("discrepancy {id}")).is_empty(),
                "discrepancy {id} has an empty {key}"
            );
        }
    }
    let expected_discrepancy_ids: BTreeSet<String> =
        (1..=12).map(|index| format!("X{index}")).collect();
    assert_eq!(discrepancy_ids, expected_discrepancy_ids);

    let x8 = discrepancies
        .iter()
        .map(|discrepancy| object(discrepancy, "ledger.discrepancies[]"))
        .find(|discrepancy| required_str(discrepancy, "id", "X8") == "X8")
        .expect("X8 must remain in the discrepancy register");
    let x8_text = format!(
        "{}\0{}\0{}",
        required_str(x8, "claim", "X8"),
        required_str(x8, "evidence", "X8"),
        required_str(x8, "action", "X8")
    );
    assert_eq!(
        sha256_hex(x8_text.as_bytes()),
        FROZEN_X8_SHA256,
        "the corrected X8 claim/evidence/action were reverted"
    );

    let integrator = required_object(root, "integrator_decision", "ledger root");
    assert!(!required_str(integrator, "date", "ledger.integrator_decision").is_empty());
    assert!(!required_str(integrator, "decision", "ledger.integrator_decision").is_empty());
    assert!(!required_str(integrator, "invariant", "ledger.integrator_decision").is_empty());
    assert!(!required_array(integrator, "evidence", "ledger.integrator_decision").is_empty());
    assert_eq!(
        required_u64(
            integrator,
            "remaining_blocked",
            "ledger.integrator_decision"
        ),
        0
    );
}

#[test]
fn ledger_counts_and_dispositions_are_frozen() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");
    let counts = required_object(root, "counts", "ledger root");
    let cases = required_array(root, "cases", "ledger root");

    assert_eq!(required_u64(counts, "test_files", "ledger.counts"), 21);
    assert_eq!(
        required_u64(counts, "static_test_declarations", "ledger.counts"),
        274
    );
    assert_eq!(
        required_u64(counts, "literal_declarations", "ledger.counts"),
        273
    );
    assert_eq!(
        required_u64(counts, "dynamic_declarations", "ledger.counts"),
        1
    );
    assert_eq!(
        required_u64(counts, "expanded_registrations", "ledger.counts"),
        339
    );
    assert_eq!(
        required_u64(counts, "cases_in_ledger", "ledger.counts"),
        339
    );
    assert_eq!(required_u64(counts, "matrix_cases", "ledger.counts"), 78);
    assert_eq!(
        required_u64(counts, "matrix_registrations", "ledger.counts"),
        78
    );
    assert_eq!(
        required_u64(counts, "matrix_dynamic_registrations", "ledger.counts"),
        66
    );
    assert_eq!(required_u64(counts, "scenarios", "ledger.counts"), 33);
    assert_eq!(
        required_u64(counts, "assertion_calls_static", "ledger.counts"),
        1_145
    );
    assert_eq!(
        required_u64(counts, "semantic_assertion_calls_static", "ledger.counts"),
        930
    );
    assert_eq!(
        required_u64(counts, "blocked_decision_count", "ledger.counts"),
        0
    );

    assert_eq!(
        cases.len(),
        TOTAL_CASES,
        "the ledger must contain 339 cases"
    );
    assert_eq!(
        usize::try_from(required_u64(counts, "cases_in_ledger", "ledger.counts"))
            .expect("case count fits usize"),
        TOTAL_CASES
    );

    let dispositions = tally(cases, "disposition", ALLOWED_DISPOSITIONS);
    let expected_dispositions = [
        ("direct", 240_usize),
        ("rewrite", 52),
        ("split", 11),
        ("obsolete", 36),
    ];
    for (disposition, expected) in expected_dispositions {
        assert_eq!(
            dispositions.get(disposition).copied().unwrap_or_default(),
            expected,
            "disposition {disposition} drifted"
        );
    }
    assert_eq!(dispositions.len(), expected_dispositions.len());
    assert_eq!(
        dispositions.values().sum::<usize>(),
        TOTAL_CASES,
        "disposition counts must sum to 339"
    );

    let declared_dispositions = required_object(counts, "dispositions", "ledger.counts");
    assert_eq!(
        declared_dispositions.len(),
        5,
        "counts.dispositions must include the zero blocked-decision bucket"
    );
    for (disposition, expected) in expected_dispositions {
        assert_eq!(
            required_u64(
                declared_dispositions,
                disposition,
                "ledger.counts.dispositions"
            ),
            u64::try_from(expected).expect("disposition count fits u64"),
            "declared disposition {disposition} drifted"
        );
    }
    assert_eq!(
        required_u64(
            declared_dispositions,
            "blocked-decision",
            "ledger.counts.dispositions"
        ),
        0
    );

    let statuses = tally(cases, "status", ALLOWED_STATUSES);
    let green = statuses.get("green").copied().unwrap_or_default();
    let planned = statuses.get("planned").copied().unwrap_or_default();
    let retired = statuses
        .get("retired-with-decision")
        .copied()
        .unwrap_or_default();
    assert_eq!(green, GREEN_CASES, "green case count drifted");
    assert_eq!(planned, PLANNED_CASES, "planned case count drifted");
    assert_eq!(retired, RETIRED_CASES, "retired case count drifted");
    assert_eq!(statuses.len(), 3, "unexpected status bucket");
    assert_eq!(
        green + planned + retired,
        TOTAL_CASES,
        "status counts must sum to the total case count"
    );

    let declared_statuses = required_object(counts, "statuses", "ledger.counts");
    assert_eq!(
        declared_statuses.len(),
        3,
        "counts.statuses must contain each status bucket"
    );
    for (status, expected) in [
        ("planned", planned),
        ("green", green),
        ("retired-with-decision", retired),
    ] {
        assert_eq!(
            required_u64(declared_statuses, status, "ledger.counts.statuses"),
            u64::try_from(expected).expect("status count fits u64"),
            "declared status {status} drifted"
        );
    }

    let oracle_types = tally(cases, "oracle_type", ALLOWED_ORACLE_TYPES);
    let expected_oracle = [("assertion", 125_usize), ("unit", 114), ("parity", 100)];
    let declared_oracle = required_object(counts, "oracle_types", "ledger.counts");
    for (oracle_type, expected) in expected_oracle {
        assert_eq!(
            oracle_types.get(oracle_type).copied().unwrap_or_default(),
            expected
        );
        assert_eq!(
            required_u64(declared_oracle, oracle_type, "ledger.counts.oracle_types"),
            u64::try_from(expected).expect("oracle count fits u64")
        );
    }

    let computed_covers = cover_counts(cases);
    let declared_covers = required_object(counts, "coverage_tag_counts", "ledger.counts");
    assert_eq!(declared_covers.len(), COVER_KEYS.len());
    for key in COVER_KEYS.iter().copied() {
        assert_eq!(
            u64::try_from(computed_covers[key]).expect("cover count fits u64"),
            required_u64(declared_covers, key, "ledger.counts.coverage_tag_counts"),
            "coverage tag {key} drifted"
        );
    }

    let executed = required_object(counts, "executed", "ledger.counts");
    assert_eq!(
        required_u64(executed, "tests", "ledger.counts.executed"),
        339
    );
    assert_eq!(
        required_u64(executed, "passed", "ledger.counts.executed"),
        338
    );
    assert_eq!(
        required_u64(executed, "failed", "ledger.counts.executed"),
        1
    );
    assert_eq!(
        required_u64(executed, "skipped", "ledger.counts.executed"),
        0
    );
    assert_eq!(required_u64(executed, "todo", "ledger.counts.executed"), 0);
    assert_eq!(
        required_u64(executed, "passed", "ledger.counts.executed")
            + required_u64(executed, "failed", "ledger.counts.executed"),
        required_u64(executed, "tests", "ledger.counts.executed")
    );

    let perf = required_object(counts, "perf_isolated", "ledger.counts");
    let perf_threshold = required_u64(perf, "threshold_ms", "ledger.counts.perf_isolated");
    assert_eq!(perf_threshold, 8_000);
    let perf_runs = required_u64(perf, "runs", "ledger.counts.perf_isolated");
    assert_eq!(perf_runs, 3);
    assert_eq!(
        required_u64(perf, "passed", "ledger.counts.perf_isolated"),
        perf_runs
    );
    assert_eq!(
        required_u64(perf, "failed", "ledger.counts.perf_isolated"),
        0
    );
    let perf_samples = required_array(perf, "cold_start_page_basic_in_place_ms", "perf_isolated");
    assert_eq!(
        u64::try_from(perf_samples.len()).expect("sample count fits u64"),
        perf_runs
    );
    let threshold = f64::from(u32::try_from(perf_threshold).expect("threshold fits u32"));
    for sample in perf_samples {
        let value = sample.as_f64().expect("perf sample must be numeric");
        assert!(
            value <= threshold,
            "perf sample {value} exceeds the frozen {perf_threshold} ms threshold"
        );
    }

    let integrator = required_object(counts, "integrator_decisions", "ledger.counts");
    assert_eq!(
        required_u64(
            integrator,
            "accepted_obsolete",
            "ledger.counts.integrator_decisions"
        ),
        36
    );
    assert_eq!(
        required_u64(
            integrator,
            "accepted_rewrite",
            "ledger.counts.integrator_decisions"
        ),
        1
    );
    assert_eq!(
        required_u64(
            integrator,
            "remaining_blocked",
            "ledger.counts.integrator_decisions"
        ),
        0
    );
}

#[test]
fn ledger_cases_are_unique_and_routed_without_claims() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");
    let cases = required_array(root, "cases", "ledger root");
    let all_case_ids = case_ids(cases);
    assert_eq!(all_case_ids.len(), TOTAL_CASES, "duplicate case IDs");

    let mut targets = BTreeSet::new();
    let mut non_obsolete = 0_usize;
    let mut obsolete = 0_usize;
    let mut decision_ids = Vec::new();
    let mut non_obsolete_decision_ids = Vec::new();

    for case in cases {
        let context = "ledger case";
        let case = object(case, context);
        let case_id = required_str(case, "case_id", context);
        assert!(
            is_case_id(case_id),
            "case_id {case_id:?} is not a stable TS-... identifier"
        );

        let source = required_object(case, "source", case_id);
        for key in ["file", "title", "static_declaration_id", "body_sha256"] {
            assert!(
                !required_str(source, key, &format!("{case_id}.source")).is_empty(),
                "{case_id} has an empty source.{key}"
            );
        }
        assert!(
            is_lowercase_sha256(required_str(source, "body_sha256", case_id)),
            "{case_id} source.body_sha256 must be lowercase SHA-256 hex"
        );
        assert!(!required_str(case, "expanded_title", case_id).is_empty());
        assert!(!required_str(case, "expansion_id", case_id).is_empty());
        for key in [
            "inner_expansions",
            "fixtures",
            "dirty_increments",
            "capabilities",
        ] {
            assert!(
                required_array(case, key, case_id)
                    .iter()
                    .all(Value::is_string),
                "{case_id}.{key} must contain only strings"
            );
        }
        for key in ["scenario", "layout"] {
            let value = case
                .get(key)
                .unwrap_or_else(|| panic!("{case_id}.{key} is missing"));
            assert!(
                value.is_null() || value.is_string(),
                "{case_id}.{key} must be null or a string"
            );
        }

        let assertions = required_object(case, "assertions", case_id);
        let assertion_count = required_u64(assertions, "count", case_id);
        let semantic_count = required_u64(assertions, "semantic_count", case_id);
        assert!(
            semantic_count <= assertion_count,
            "{case_id} has more semantic assertions than assertions"
        );
        let summary = required_array(assertions, "summary", case_id);
        assert!(
            summary.len() <= usize::try_from(assertion_count).expect("assertion count fits usize"),
            "{case_id} has more assertion summaries than assertions"
        );
        assert!(
            assertion_count == 0 || !required_object(assertions, "kinds", case_id).is_empty(),
            "{case_id} has assertions but no assertion kinds"
        );
        assert!(!required_str(assertions, "summary_text", case_id).is_empty());

        let disposition = required_str(case, "disposition", case_id);
        assert!(
            ALLOWED_DISPOSITIONS.contains(&disposition),
            "{case_id} has unexpected disposition {disposition:?}"
        );
        let reason = required_str(case, "disposition_reason", case_id);
        assert!(
            reason.len() > 20,
            "{case_id} needs a substantive disposition reason"
        );
        let status = required_str(case, "status", case_id);
        assert!(
            ALLOWED_STATUSES.contains(&status),
            "{case_id} has unexpected status: {status:?}"
        );
        let target_status = required_str(case, "target_rust_test_status", case_id);
        let oracle_type = required_str(case, "oracle_type", case_id);
        assert!(ALLOWED_ORACLE_TYPES.contains(&oracle_type));
        let comparison_type = required_str(case, "comparison_type", case_id);
        assert!(
            ALLOWED_COMPARISON_TYPES.contains(&comparison_type),
            "{case_id} has unexpected comparison type {comparison_type:?}"
        );
        let requires_decision = required_bool(case, "requires_decision", case_id);
        if requires_decision {
            decision_ids.push(case_id);
            if disposition != "obsolete" {
                non_obsolete_decision_ids.push(case_id);
            }
        }

        let target = required_str(case, "target_rust_test", case_id);
        match disposition {
            "obsolete" => {
                obsolete += 1;
                assert_eq!(status, "retired-with-decision", "{case_id}");
                assert_eq!(target_status, RETIRED_TARGET_STATUS, "{case_id}");
                assert!(
                    reason.contains("Candidate B"),
                    "{case_id} must cite the retired Candidate B layout"
                );
                assert!(requires_decision, "{case_id} needs an integrator decision");
            }
            "direct" | "rewrite" | "split" => {
                non_obsolete += 1;
                validate_target_module(target, case_id);
                assert!(
                    targets.insert(target.to_owned()),
                    "duplicate target Rust test {target} for {case_id}"
                );
                match status {
                    "green" => {
                        assert_eq!(target_status, GREEN_TARGET_STATUS, "{case_id}");
                        let evidence = required_object(case, "migration_evidence", case_id);
                        assert_eq!(
                            required_str(evidence, "parity", case_id),
                            "exact",
                            "{case_id} may only be green with exact parity"
                        );
                        assert!(
                            required_bool(evidence, "green_eligible", case_id),
                            "{case_id} green evidence must be eligible"
                        );
                        assert!(
                            !required_array(evidence, "rust_tests", case_id).is_empty(),
                            "{case_id} green requires at least one Rust test"
                        );
                    }
                    "planned" => assert_eq!(target_status, PLANNED_TARGET_STATUS, "{case_id}"),
                    other => panic!("{case_id} has unexpected status {other:?}"),
                }
            }
            other => panic!("{case_id} has unexpected disposition {other:?}"),
        }
    }

    assert_eq!(non_obsolete, PLANNED_CASES + GREEN_CASES);
    assert_eq!(obsolete, RETIRED_CASES);
    assert_eq!(targets.len(), PLANNED_CASES + GREEN_CASES);
    assert_eq!(decision_ids.len(), 37);
    non_obsolete_decision_ids.sort_unstable();
    assert_eq!(non_obsolete_decision_ids, ["TS-META-0027"]);

    let dirty_increments = required_array(root, "dirty_increments", "ledger root");
    assert_eq!(dirty_increments.len(), DIRTY_INCREMENT_IDS.len());
    let mut declared_dirty = BTreeSet::new();
    let mut dirty_case_ids = BTreeSet::new();
    for increment in dirty_increments {
        let increment = object(increment, "ledger.dirty_increments[]");
        let id = required_str(increment, "id", "ledger.dirty_increments[]");
        assert!(
            DIRTY_INCREMENT_IDS.contains(&id),
            "unexpected dirty increment {id}"
        );
        assert!(
            declared_dirty.insert(id.to_owned()),
            "duplicate dirty increment {id}"
        );
        assert_eq!(
            required_str(increment, "status", id),
            "planned",
            "dirty increment {id} must not claim completion"
        );
        for ledger_case in required_array(increment, "ledger_cases", id) {
            let ledger_case = ledger_case
                .as_str()
                .unwrap_or_else(|| panic!("dirty increment {id} has a non-string case"));
            assert!(
                all_case_ids.contains(ledger_case),
                "dirty increment {id} references unknown case {ledger_case}"
            );
            dirty_case_ids.insert(ledger_case.to_owned());
        }
    }
    for case in cases {
        let case = object(case, "case");
        let case_id = required_str(case, "case_id", "case");
        for increment in required_array(case, "dirty_increments", case_id) {
            let increment = increment
                .as_str()
                .unwrap_or_else(|| panic!("{case_id} has a non-string dirty increment"));
            assert!(
                declared_dirty.contains(increment),
                "{case_id} references unknown dirty increment {increment}"
            );
        }
        if !required_array(case, "dirty_increments", case_id).is_empty() {
            assert!(
                dirty_case_ids.contains(case_id),
                "{case_id} is missing from dirty_increment.ledger_cases"
            );
        }
    }
    assert_eq!(dirty_case_ids.len(), 8);
}

#[test]
fn ledger_migration_evidence_is_complete_and_consistent() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");
    let cases = required_array(root, "cases", "ledger root");
    let counts = required_object(root, "counts", "ledger root");
    let workspace = workspace_root();

    let mut green = 0_usize;
    let mut planned = 0_usize;
    let mut evidence_cases = 0_usize;
    let mut without_evidence = 0_usize;
    let mut parity_counts: BTreeMap<&str, usize> =
        ALLOWED_PARITIES.iter().map(|parity| (*parity, 0)).collect();
    let mut partition_counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

    for case in cases {
        let case = object(case, "case");
        let case_id = required_str(case, "case_id", "case");
        let disposition = required_str(case, "disposition", case_id);
        let status = required_str(case, "status", case_id);
        let target_status = required_str(case, "target_rust_test_status", case_id);

        if disposition == "obsolete" {
            assert_eq!(status, "retired-with-decision", "{case_id}");
            assert_eq!(target_status, RETIRED_TARGET_STATUS, "{case_id}");
            assert!(
                case.get("migration_evidence").is_none(),
                "{case_id} retired evidence must stay outside the migration ledger"
            );
            continue;
        }

        let module = target_partition(required_str(case, "target_rust_test", case_id));
        let partition = partition_counts.entry(module.to_owned()).or_default();
        *partition.entry("routed_cases".to_owned()).or_default() += 1;
        if status == "planned" {
            planned += 1;
        }

        let Some(evidence) = case.get("migration_evidence") else {
            assert_eq!(
                status, "planned",
                "{case_id} must stay planned without evidence"
            );
            assert_eq!(target_status, PLANNED_TARGET_STATUS, "{case_id}");
            without_evidence += 1;
            continue;
        };

        evidence_cases += 1;
        *partition.entry("evidence_cases".to_owned()).or_default() += 1;
        let evidence = object(evidence, case_id);
        assert_eq!(
            required_str(evidence, "case_id", case_id),
            case_id,
            "{case_id} evidence names another case"
        );
        assert!(
            !required_str(evidence, "assessed_at", case_id).is_empty(),
            "{case_id} evidence has no assessment date"
        );
        let head = required_str(evidence, "implementation_head", case_id);
        assert_eq!(
            head.len(),
            40,
            "{case_id} implementation head is not a git id"
        );
        assert!(
            head.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{case_id} implementation head is not hex"
        );

        let parity = required_str(evidence, "parity", case_id);
        assert!(
            ALLOWED_PARITIES.contains(&parity),
            "{case_id} has unexpected parity {parity:?}"
        );
        *parity_counts.get_mut(parity).expect("known parity") += 1;
        *partition.entry(parity.to_owned()).or_default() += 1;

        let eligible = required_bool(evidence, "green_eligible", case_id);
        let recommendation = required_str(evidence, "status_recommendation", case_id);
        assert!(
            ALLOWED_RECOMMENDATIONS.contains(&recommendation),
            "{case_id} has unexpected recommendation {recommendation:?}"
        );
        assert_eq!(
            required_str(evidence, "status_applied", case_id),
            status,
            "{case_id} status_applied must match status"
        );
        let note = required_str(evidence, "note", case_id);
        assert!(
            note.len() > 20,
            "{case_id} needs a substantive evidence note"
        );

        let tests = required_array(evidence, "rust_tests", case_id);
        assert!(
            !tests.is_empty(),
            "{case_id} must never accept empty Rust evidence"
        );
        for test in tests {
            let test = object(test, case_id);
            let path = required_str(test, "path", case_id);
            assert!(!path.is_empty(), "{case_id} has an empty Rust test path");
            assert!(
                Path::new(path)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rs")),
                "{case_id} Rust test path is not a .rs file: {path}"
            );
            assert!(
                !Path::new(path).is_absolute(),
                "{case_id} Rust test path must stay repository-relative: {path}"
            );
            let absolute = workspace.join(path);
            let source = std::fs::read_to_string(&absolute).unwrap_or_else(|error| {
                panic!(
                    "{case_id} Rust test source is missing at {}: {error}",
                    absolute.display()
                )
            });
            let line = usize::try_from(required_u64(test, "line", case_id))
                .expect("Rust test line fits usize");
            assert!(
                line > 0 && line <= source.lines().count(),
                "{case_id} Rust test line {line} is outside {path}"
            );
            let test_name = required_str(test, "test", case_id);
            assert!(!test_name.is_empty(), "{case_id} has an empty test name");
            assert!(
                test_name.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                }),
                "{case_id} test name has unexpected characters: {test_name}"
            );
            assert!(
                source
                    .lines()
                    .any(|line| line.trim_start().starts_with(&format!("fn {test_name}("))),
                "{case_id} test {test_name} is not declared in {path}"
            );
            let fully_qualified = required_str(test, "fully_qualified", case_id);
            assert!(
                !fully_qualified.is_empty(),
                "{case_id} has an empty fully-qualified test name"
            );
            assert_eq!(
                fully_qualified.rsplit("::").next(),
                Some(test_name),
                "{case_id} fully-qualified name does not end with {test_name}"
            );
            assert_eq!(
                fully_qualified.split_whitespace().count(),
                1,
                "{case_id} fully-qualified name contains whitespace"
            );
            let test_note = required_str(test, "note", case_id);
            assert!(
                test_note.len() > 20,
                "{case_id} needs a substantive Rust test note"
            );
        }

        match parity {
            "exact" => {
                assert!(eligible, "{case_id} exact parity must be green eligible");
                assert_eq!(status, "green", "{case_id} exact parity must be green");
                assert_eq!(target_status, GREEN_TARGET_STATUS, "{case_id}");
                assert_eq!(recommendation, "green", "{case_id}");
                green += 1;
                *partition.entry("green_cases".to_owned()).or_default() += 1;
            }
            "near_exact" => {
                assert!(!eligible, "{case_id} near-exact must not be green eligible");
                assert_eq!(status, "planned", "{case_id} near-exact must stay planned");
                assert_eq!(target_status, PLANNED_TARGET_STATUS, "{case_id}");
                assert_eq!(recommendation, "green", "{case_id}");
            }
            "weaker" | "structural" | "blocked" => {
                assert!(
                    !eligible,
                    "{case_id} non-exact evidence must not be eligible"
                );
                assert_eq!(
                    status, "planned",
                    "{case_id} non-exact evidence must stay planned"
                );
                assert_eq!(target_status, PLANNED_TARGET_STATUS, "{case_id}");
                assert_eq!(recommendation, "planned", "{case_id}");
            }
            other => panic!("{case_id} has unexpected parity {other:?}"),
        }
    }

    assert_eq!(green, GREEN_CASES, "green count drifted");
    assert_eq!(planned, PLANNED_CASES, "planned count drifted");
    assert_eq!(parity_counts["exact"], GREEN_CASES);
    assert_eq!(parity_counts["near_exact"], NEAR_EXACT_CASES);
    assert_eq!(parity_counts["weaker"], WEAKER_CASES);
    assert_eq!(parity_counts["structural"], STRUCTURAL_CASES);
    assert_eq!(parity_counts["blocked"], BLOCKED_CASES);
    assert_eq!(
        parity_counts.values().sum::<usize>(),
        evidence_cases,
        "parity buckets must cover every evidence case"
    );
    assert_eq!(
        without_evidence,
        PLANNED_CASES
            - parity_counts["near_exact"]
            - parity_counts["weaker"]
            - parity_counts["structural"]
            - parity_counts["blocked"],
        "planned cases without evidence drifted"
    );

    for partition in partition_counts.values_mut() {
        let evidence = partition.get("evidence_cases").copied().unwrap_or_default();
        let green_cases = partition.get("green_cases").copied().unwrap_or_default();
        partition.insert("planned_evidence_cases".to_owned(), evidence - green_cases);
        let routed = partition.get("routed_cases").copied().unwrap_or_default();
        partition.insert("unassessed_cases".to_owned(), routed - evidence);
    }

    let declared = required_object(counts, "migration_evidence", "ledger.counts");
    assert_eq!(
        required_u64(
            declared,
            "assessed_cases",
            "ledger.counts.migration_evidence"
        ),
        u64::try_from(evidence_cases).expect("evidence count fits u64")
    );
    for (parity, expected) in parity_counts {
        assert_eq!(
            required_u64(declared, parity, "ledger.counts.migration_evidence"),
            u64::try_from(expected).expect("parity count fits u64"),
            "declared parity {parity} drifted"
        );
    }
    assert_eq!(
        required_u64(
            declared,
            "green_eligible",
            "ledger.counts.migration_evidence"
        ),
        u64::try_from(GREEN_CASES).expect("green count fits u64")
    );
    assert_eq!(
        required_u64(
            declared,
            "promoted_to_green",
            "ledger.counts.migration_evidence"
        ),
        u64::try_from(GREEN_CASES).expect("green count fits u64")
    );
    assert_eq!(
        required_u64(
            declared,
            "near_exact_recommendations",
            "ledger.counts.migration_evidence"
        ),
        u64::try_from(NEAR_EXACT_CASES).expect("near-exact count fits u64")
    );
    assert_eq!(
        required_u64(
            declared,
            "remaining_planned_without_evidence",
            "ledger.counts.migration_evidence"
        ),
        u64::try_from(without_evidence).expect("planned count fits u64")
    );

    let declared_statuses =
        required_object(declared, "statuses", "ledger.counts.migration_evidence");
    for (status, expected) in [
        ("planned", planned),
        ("green", green),
        ("retired-with-decision", RETIRED_CASES),
    ] {
        assert_eq!(
            required_u64(
                declared_statuses,
                status,
                "ledger.counts.migration_evidence.statuses"
            ),
            u64::try_from(expected).expect("status count fits u64"),
            "declared migration status {status} drifted"
        );
    }

    let declared_partitions =
        required_object(declared, "by_partition", "ledger.counts.migration_evidence");
    assert_eq!(
        declared_partitions.len(),
        PARTITION_NAMES.len(),
        "migration evidence must cover every partition"
    );
    for (module, partition) in &partition_counts {
        let declared_partition = required_object(
            declared_partitions,
            module,
            "ledger.counts.migration_evidence",
        );
        for key in [
            "routed_cases",
            "evidence_cases",
            "green_cases",
            "planned_evidence_cases",
            "unassessed_cases",
            "exact",
            "near_exact",
            "weaker",
            "structural",
            "blocked",
        ] {
            assert_eq!(
                required_u64(
                    declared_partition,
                    key,
                    &format!("ledger.counts.migration_evidence.by_partition.{module}")
                ),
                u64::try_from(partition.get(key).copied().unwrap_or_default())
                    .expect("partition count fits u64"),
                "{module}.{key} drifted from the actual cases"
            );
        }
    }
}

#[test]
fn ledger_old_test_sources_and_lines_exist() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");
    let workspace = workspace_root();
    let cases = required_array(root, "cases", "ledger root");
    let files = required_array(root, "files", "ledger root");
    assert_eq!(files.len(), 21, "the old test-file inventory drifted");

    let mut file_names = BTreeSet::new();
    let mut file_lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut file_sources: BTreeMap<String, String> = BTreeMap::new();
    let mut file_partitions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut routing_union = BTreeSet::new();
    let mut declaration_sum = 0_u64;
    let mut literal_sum = 0_u64;
    let mut dynamic_sum = 0_u64;
    let mut registration_sum = 0_u64;
    let mut assertion_sum = 0_u64;
    let mut semantic_assertion_sum = 0_u64;
    let mut executed_total_sum = 0_u64;
    let mut executed_passed_sum = 0_u64;
    let mut executed_failed_sum = 0_u64;

    for file in files {
        let context = "ledger.files[]";
        let file = object(file, context);
        let name = required_str(file, "file", context);
        assert!(
            name.starts_with("backends/typescript/test/") && name.ends_with(".test.ts"),
            "unexpected old test path {name}"
        );
        assert!(
            file_names.insert(name.to_owned()),
            "duplicate old test {name}"
        );

        let path = workspace.join(name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("old test file missing at {}: {error}", path.display()));
        let text = String::from_utf8(bytes.clone()).unwrap_or_else(|error| {
            panic!("old test file is not UTF-8 {}: {error}", path.display())
        });
        let declared_sha = required_str(file, "sha256", name);
        assert!(
            is_lowercase_sha256(declared_sha),
            "{name} has an invalid source hash"
        );
        assert_eq!(
            sha256_hex(&bytes),
            declared_sha,
            "old test source hash drifted for {name}; update the ledger deliberately"
        );
        file_lines.insert(name.to_owned(), text.lines().map(str::to_owned).collect());
        file_sources.insert(name.to_owned(), text);

        let declarations = required_u64(file, "test_declarations", name);
        let literals = required_u64(file, "literal_declarations", name);
        let dynamics = required_u64(file, "dynamic_declarations", name);
        let registrations = required_u64(file, "registrations", name);
        assert!(declarations > 0, "{name} has no test declarations");
        assert_eq!(
            literals + dynamics,
            declarations,
            "{name} literal/dynamic declarations do not sum"
        );
        assert!(
            registrations >= declarations,
            "{name} registers fewer cases than declarations"
        );
        declaration_sum += declarations;
        literal_sum += literals;
        dynamic_sum += dynamics;
        registration_sum += registrations;
        assertion_sum += required_u64(file, "assertion_calls", name);
        semantic_assertion_sum += required_u64(file, "semantic_assertion_calls", name);

        let executed = required_object(file, "executed", name);
        let executed_total = required_u64(executed, "total", name);
        let executed_passed = required_u64(executed, "passed", name);
        let executed_failed = required_u64(executed, "failed", name);
        assert_eq!(
            executed_total, registrations,
            "{name} executed.total must equal registrations"
        );
        assert_eq!(
            executed_passed + executed_failed,
            executed_total,
            "{name} executed counts do not sum"
        );
        executed_total_sum += executed_total;
        executed_passed_sum += executed_passed;
        executed_failed_sum += executed_failed;

        let partitions = required_array(file, "partitions", name);
        assert!(!partitions.is_empty(), "{name} has no target partitions");
        let mut partitions_for_file = BTreeSet::new();
        for partition in partitions {
            let partition = partition
                .as_str()
                .unwrap_or_else(|| panic!("{name} has a non-string partition"));
            assert!(
                PARTITION_NAMES.contains(&partition),
                "{name} targets unknown partition {partition}"
            );
            partitions_for_file.insert(partition.to_owned());
            routing_union.insert(partition.to_owned());
        }
        file_partitions.insert(name.to_owned(), partitions_for_file);
        assert!(
            required_array(file, "fixtures", name)
                .iter()
                .all(Value::is_string),
            "{name}.fixtures must contain only strings"
        );
    }

    let counts = required_object(root, "counts", "ledger root");
    assert_eq!(
        declaration_sum,
        required_u64(counts, "static_test_declarations", "ledger.counts")
    );
    assert_eq!(
        literal_sum,
        required_u64(counts, "literal_declarations", "ledger.counts")
    );
    assert_eq!(
        dynamic_sum,
        required_u64(counts, "dynamic_declarations", "ledger.counts")
    );
    assert_eq!(
        registration_sum,
        required_u64(counts, "expanded_registrations", "ledger.counts")
    );
    assert_eq!(
        assertion_sum,
        required_u64(counts, "assertion_calls_static", "ledger.counts")
    );
    assert_eq!(
        semantic_assertion_sum,
        required_u64(counts, "semantic_assertion_calls_static", "ledger.counts")
    );
    assert_eq!(registration_sum, 339);
    assert_eq!(declaration_sum, 274);
    let executed_counts = required_object(counts, "executed", "ledger.counts");
    assert_eq!(
        executed_total_sum,
        required_u64(executed_counts, "tests", "ledger.counts.executed")
    );
    assert_eq!(executed_total_sum, 339);
    assert_eq!(executed_passed_sum, 338);
    assert_eq!(
        executed_passed_sum,
        required_u64(executed_counts, "passed", "ledger.counts.executed")
    );
    assert_eq!(executed_failed_sum, 1);
    assert_eq!(
        executed_failed_sum,
        required_u64(executed_counts, "failed", "ledger.counts.executed")
    );
    assert_eq!(
        routing_union,
        PARTITION_NAMES
            .iter()
            .map(|partition| (*partition).to_owned())
            .collect::<BTreeSet<_>>(),
        "files[].partitions must cover every contract partition exactly"
    );

    let support = required_object(root, "support_file", "ledger root");
    let support_path = required_str(support, "path", "ledger.support_file");
    assert_eq!(support_path, "backends/typescript/test/support.ts");
    let support_absolute = workspace.join(support_path);
    let support_bytes = std::fs::read(&support_absolute).unwrap_or_else(|error| {
        panic!(
            "support file missing at {}: {error}",
            support_absolute.display()
        )
    });
    assert_eq!(
        sha256_hex(&support_bytes),
        required_str(support, "sha256", "ledger.support_file"),
        "support source hash drifted; update the ledger deliberately"
    );
    assert!(!required_array(support, "exports", "ledger.support_file").is_empty());
    let used_by = required_string_array(support, "used_by", "ledger.support_file");
    assert!(!used_by.is_empty());
    let file_basenames: BTreeSet<&str> = file_names
        .iter()
        .filter_map(|name| name.rsplit('/').next())
        .collect();
    for used in used_by {
        assert!(
            file_basenames.contains(used),
            "support_file.used_by references unregistered test {used}"
        );
    }

    let mut source_files = BTreeSet::new();
    for case in cases {
        let case = object(case, "case");
        let case_id = required_str(case, "case_id", "case");
        let source = required_object(case, "source", case_id);
        let file = required_str(source, "file", case_id);
        assert!(
            file_names.contains(file),
            "{case_id} references an unregistered old test {file}"
        );
        source_files.insert(file.to_owned());

        let line =
            usize::try_from(required_u64(source, "line", case_id)).expect("case line fits usize");
        assert!(line > 0, "{case_id} has line 0");
        let lines = file_lines
            .get(file)
            .unwrap_or_else(|| panic!("{case_id} references unread file {file}"));
        let source_line = lines
            .get(line - 1)
            .unwrap_or_else(|| panic!("{case_id} points past the end of {file}:{line}"));
        assert!(
            source_scan::is_test_declaration(source_line),
            "{case_id} points at {file}:{line}, which is not a test declaration: {source_line:?}"
        );
        let source_text = file_sources
            .get(file)
            .unwrap_or_else(|| panic!("{case_id} references unread source {file}"));
        let call_range = source_scan::test_call_range(source_text, line)
            .unwrap_or_else(|| panic!("{case_id} test call range was not found at {file}:{line}"));
        // The ledger hashes the exact TypeScript CallExpression text:
        // `test("...", () => { ... })`, excluding the trailing semicolon.
        assert_eq!(
            sha256_hex(source_text[call_range].as_bytes()),
            required_str(source, "body_sha256", case_id),
            "{case_id} body_sha256 drifted at {file}:{line}"
        );
        let target = required_str(case, "target_rust_test", case_id);
        let target_module = validate_target_module(target, case_id);
        assert!(
            file_partitions
                .get(file)
                .is_some_and(|partitions| partitions.contains(target_module)),
            "{case_id} targets {target_module}, which is not routed by {file}.partitions"
        );
        let declaration_id = required_str(source, "static_declaration_id", case_id);
        let file_name = file.rsplit('/').next().expect("file name");
        assert_eq!(
            declaration_id,
            format!("{file_name}:{line}"),
            "{case_id} static_declaration_id drifted"
        );
        assert!(
            !required_str(source, "title", case_id).is_empty(),
            "{case_id} has an empty old title"
        );
    }
    assert_eq!(
        source_files.len(),
        file_names.len(),
        "some old test files have no ledger case"
    );
}
