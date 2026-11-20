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

use crate::oracle_manifest;

/// Repository-relative ledger path.
pub(crate) const LEDGER_RELATIVE_PATH: &str = "docs/plans/tsgo-test-ledger.json";

/// Frozen expanded registration count.
pub(crate) const TOTAL_CASES: usize = 339;
/// Exact-parity cases promoted to `green`; the wave-9 assertion backfill
/// promoted three more, the wave-10 follow-up promoted `TS-REWRITE-0207` plus
/// nine exact-ready cases, the wave-10 conditional follow-up promoted
/// `TS-ANALYSIS-0184` plus `TS-REWRITE-0197`, the wave-11 exact backfill
/// promoted `TS-ANALYSIS-0315/0332` plus ten messages/probes cases, the
/// wave-11 blocked followup promoted `TS-COMPONENT-0292`, and the wave-11
/// matrix backfill promoted 22 in-place plus 4 top-level matrix cases after the
/// `methods-nonliteral` semantic fix.
pub(crate) const GREEN_CASES: usize = 109;
/// P7-1 closeout requires zero `planned` cases.
pub(crate) const PLANNED_CASES: usize = 0;
/// Evidence-bearing cases whose residual risk the owner accepted at P7.
/// They keep their `near_exact`/`weaker`/`structural`/`blocked` parity and
/// never count as exact or green.
pub(crate) const REVIEWED_CASES: usize = 130;
/// Owner-signed gaps: no Rust evidence, or a named known defect. They are
/// not green and not reviewed.
pub(crate) const WAIVED_CASES: usize = 29;
/// Frozen retired-with-decision count: the 2026-11-17 Candidate-B decision
/// (36 cases), the wave-11 `TS-COMPONENT-0774`/`TS-META-0027` cases, and the
/// 2026-11-20 C-lite closeout (30 mechanism/contract + 3 internal evidence).
pub(crate) const RETIRED_CASES: usize = 71;
/// Retired cases that keep a non-obsolete disposition and therefore stay in
/// the routed target set: `TS-META-0027` plus the 33 C-lite retirements.
const NON_OBSOLETE_RETIRED_CASES: usize = 34;
/// Retired cases with `disposition = obsolete`: the 36 historical Candidate-B
/// cases plus the wave-11 `TS-COMPONENT-0774` Candidate-B-only case.
const OBSOLETE_CASES: usize = RETIRED_CASES - NON_OBSOLETE_RETIRED_CASES;
/// Near-exact cases intentionally left non-green with a green recommendation
/// after `TS-REWRITE-0207` was promoted to exact; the wave-11 blocked followup
/// added `TS-EVENTS-0188`.
pub(crate) const NEAR_EXACT_CASES: usize = 3;
/// Weaker evidence cases left `planned` after the 2026-11-20 wave-10 ledger
/// backfill, which registered twelve existing-test weaker candidates, plus the
/// wave-10b backfill of the thirteen remaining B-class candidates, the
/// wave-10 conditional `TS-ANALYSIS-0161` registration, the wave-11 blocked
/// followup (0193/0201/0343/0498), and the wave-11 matrix backfill, which
/// reclassified the four matrix weaker cases.
pub(crate) const WEAKER_CASES: usize = 96;
/// Structural evidence cases left `planned` after the wave-6 component-binding
/// follow-up.
pub(crate) const STRUCTURAL_CASES: usize = 7;
/// Blocked evidence cases left after the wave-6 component-binding follow-up;
/// the wave-11 blocked followup reclassified six of them, the wave-11 matrix
/// backfill registered 11 in-place plus 6 top-level matrix gaps, and the
/// C-lite closeout moved two dual-layout blocked cases to retired. The
/// remaining 31 are 24 reviewed coverage gaps plus 7 known-defect waivers.
pub(crate) const BLOCKED_CASES: usize = 31;
/// Waived cases with no Rust evidence (the other 7 waivers are known defects
/// that keep their blocked evidence).
pub(crate) const WAIVED_WITHOUT_EVIDENCE: usize = 22;

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
    "closeout",
];

const ALLOWED_DISPOSITIONS: &[&str] = &["direct", "rewrite", "split", "obsolete"];
const ALLOWED_STATUSES: &[&str] = &[
    "planned",
    "green",
    "reviewed",
    "waived",
    "retired-with-decision",
];
const ALLOWED_ORACLE_TYPES: &[&str] = &["assertion", "unit", "parity"];
const ALLOWED_PARITIES: &[&str] = &["exact", "near_exact", "weaker", "structural", "blocked"];
const ALLOWED_RECOMMENDATIONS: &[&str] = &["green", "planned"];
/// Frozen SHA-256 of the matrix projection consumed by `real_matrix.rs`.
pub(crate) const MATRIX_PROJECTION_SHA256: &str =
    "9d47f7d0e9510c4c735e3290d0b2cb0d66fb713042e2fa0c59f6e7d5b752d911";
/// Matrix evidence layouts: frozen in-place shadow, or the top-level in-place half.
const ALLOWED_MATRIX_LAYOUTS: &[&str] = &["in-place-shadow", "in-place-half"];
/// Product/checker groups behind a `blocked_by=checker` matrix case.
const ALLOWED_MATRIX_BLOCKED_GROUPS: &[&str] = &["docs", "state", "probe", "root", "checker"];
/// Retirement scopes accepted by the wave-11 and C-lite closeout extensions.
/// Adding a scope requires extending this list, the test contract and the
/// affected case together; an unknown scope must never slip through.
const ALLOWED_RETIREMENT_SCOPES: &[&str] = &[
    "candidate_b_layout",
    "superseded_contract",
    "mechanism_replaced",
    "internal_evidence",
    "owner_scope_waiver",
];
/// Every wave-11 retirement is decided on this date.
const RETIREMENT_DECISION_DATE: &str = "2026-11-20";
/// Legacy Candidate-B decision date. Only the frozen 2026-11-17 cases may
/// retire through the reason string instead of a `retirement_decision`.
const LEGACY_CANDIDATE_B_DECISION_DATE: &str = "2026-11-17";
/// Replacement kinds and strengths accepted inside a retirement decision.
const ALLOWED_REPLACEMENT_KINDS: &[&str] = &["rust_test", "none"];
const ALLOWED_REPLACEMENT_STRENGTHS: &[&str] = &[
    "exact",
    "near_exact",
    "weaker",
    "structural",
    "blocked",
    "opt_in",
];
/// Exact wave-11 retirement decisions as `(case_id, decision_id, scope)`.
/// This is the retired-decision sentinel: a new retirement must add its stable
/// id here and in the test contract instead of only editing a count.
const RETIREMENT_DECISIONS: &[(&str, &str, &str)] = &[
    (
        "TS-ANALYSIS-0226",
        "wave12-e-ts-analysis-0226",
        "superseded_contract",
    ),
    (
        "TS-ANALYSIS-0272",
        "wave12-e-ts-analysis-0272",
        "superseded_contract",
    ),
    (
        "TS-ANALYSIS-0371",
        "wave12-e-ts-analysis-0371",
        "superseded_contract",
    ),
    (
        "TS-COMPILER-0050",
        "wave12-e-ts-compiler-0050",
        "mechanism_replaced",
    ),
    (
        "TS-COMPILER-0086",
        "wave12-e-ts-compiler-0086",
        "mechanism_replaced",
    ),
    (
        "TS-COMPONENT-0279",
        "wave12-e-ts-component-0279",
        "mechanism_replaced",
    ),
    ("TS-COMPONENT-0774", "wave11-e-0774", "candidate_b_layout"),
    (
        "TS-EVENTS-0375",
        "wave12-e-ts-events-0375",
        "superseded_contract",
    ),
    (
        "TS-EVENTS-0387",
        "wave12-e-ts-events-0387",
        "superseded_contract",
    ),
    (
        "TS-EVENTS-0399",
        "wave12-e-ts-events-0399",
        "superseded_contract",
    ),
    (
        "TS-EVIDENCE-0011",
        "wave12-e-ts-evidence-0011",
        "internal_evidence",
    ),
    (
        "TS-EVIDENCE-0025",
        "wave12-e-ts-evidence-0025",
        "internal_evidence",
    ),
    (
        "TS-EVIDENCE-0050",
        "wave12-e-ts-evidence-0050",
        "internal_evidence",
    ),
    (
        "TS-LAYOUTS-0062",
        "wave12-e-ts-layouts-0062",
        "mechanism_replaced",
    ),
    (
        "TS-LAYOUTS-0084",
        "wave12-e-ts-layouts-0084",
        "superseded_contract",
    ),
    (
        "TS-LAYOUTS-0101",
        "wave12-e-ts-layouts-0101",
        "superseded_contract",
    ),
    ("TS-META-0027", "wave11-e-meta-0027", "superseded_contract"),
    (
        "TS-PROTOCOL-0021",
        "wave12-e-ts-protocol-0021",
        "mechanism_replaced",
    ),
    (
        "TS-PROTOCOL-0141",
        "wave12-e-ts-protocol-0141",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0154",
        "wave12-e-ts-protocol-0154",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0178",
        "wave12-e-ts-protocol-0178",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0250",
        "wave12-e-ts-protocol-0250",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0329",
        "wave12-e-ts-protocol-0329",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0348",
        "wave12-e-ts-protocol-0348",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0368",
        "wave12-e-ts-protocol-0368",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0395",
        "wave12-e-ts-protocol-0395",
        "mechanism_replaced",
    ),
    (
        "TS-PROTOCOL-0421",
        "wave12-e-ts-protocol-0421",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0450",
        "wave12-e-ts-protocol-0450",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0561",
        "wave12-e-ts-protocol-0561",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0606",
        "wave12-e-ts-protocol-0606",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0644",
        "wave12-e-ts-protocol-0644",
        "superseded_contract",
    ),
    (
        "TS-PROTOCOL-0653",
        "wave12-e-ts-protocol-0653",
        "superseded_contract",
    ),
    (
        "TS-SERVER-0094",
        "wave12-e-ts-server-0094",
        "mechanism_replaced",
    ),
    (
        "TS-SERVER-0109",
        "wave12-e-ts-server-0109",
        "mechanism_replaced",
    ),
    (
        "TS-SERVER-0203",
        "wave12-e-ts-server-0203",
        "superseded_contract",
    ),
];
const GREEN_TARGET_STATUS: &str = "migrated_green";
const PLANNED_TARGET_STATUS: &str = "proposed";
const REVIEWED_TARGET_STATUS: &str = "reviewed_residual_risk";
const WAIVED_TARGET_STATUS: &str = "waived_owner_accepted";
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
const DIRTY_INCREMENT_STATUSES: &[&str] = &["reviewed", "waived", "no-ledger-cases"];
const CLOSEOUT_POLICY: &str = "c-lite-owner-scope-waiver";
const CLOSEOUT_DATE: &str = "2026-11-20";
const CLOSEOUT_SCOPE: &str = "owner-scope-waiver";
const ALLOWED_WAIVER_GAP_KINDS: &[&str] = &["no-rust-evidence", "known-defect"];

/// Corrected contract vocabulary from `docs/reference/tsgo-test-contract.md`
/// and the integrator decision. Asserting the exact arrays prevents a stale
/// ledger block from being reintroduced.
const CONTRACT_STATUS_VALUES: &[&str] = &[
    "planned",
    "red",
    "green",
    "reviewed",
    "waived",
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

/// Validates one wave-11 retirement decision and returns its stable id/scope.
///
/// The 36 legacy Candidate-B cases keep only the 2026-11-17
/// `accepted-obsolete` decision, so they are not routed through this helper.
/// Every new retired case must carry the object: the scope decides why the
/// oracle stopped being a product gate, and the replacement list must name
/// real tests with an honest strength. `migration_evidence` is intentionally
/// not accepted on a retired case.
fn validate_retirement_decision<'a>(
    case: &'a Map<String, Value>,
    case_id: &str,
    workspace: &Path,
) -> (&'a str, &'a str) {
    let decision = required_object(case, "retirement_decision", case_id);
    let id = required_str(decision, "id", case_id);
    assert!(
        !id.is_empty()
            && id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
            }),
        "{case_id} retirement decision id is not a stable slug: {id}"
    );
    assert_eq!(
        required_str(decision, "date", case_id),
        RETIREMENT_DECISION_DATE,
        "{case_id} retirement decision date drifted"
    );
    assert_eq!(
        required_str(decision, "owner", case_id),
        "integrator",
        "{case_id} retirement decision owner drifted"
    );
    assert!(
        !required_str(decision, "required_by", case_id).is_empty(),
        "{case_id} retirement decision needs a required_by"
    );
    let scope = required_str(decision, "scope", case_id);
    assert!(
        ALLOWED_RETIREMENT_SCOPES.contains(&scope),
        "{case_id} has unexpected retirement scope {scope:?}"
    );
    let reason = required_str(decision, "reason", case_id);
    assert!(
        reason.len() > 40,
        "{case_id} retirement decision needs a substantive reason"
    );
    assert!(
        !required_str(decision, "product_impact", case_id).is_empty(),
        "{case_id} retirement decision needs a product_impact"
    );

    let evidence = required_string_array(decision, "evidence", case_id);
    assert!(
        !evidence.is_empty(),
        "{case_id} retirement decision needs non-empty code evidence"
    );
    assert!(
        evidence.iter().all(|entry| !entry.is_empty()),
        "{case_id} retirement decision has empty code evidence"
    );

    let replacement = required_array(decision, "replacement", case_id);
    assert!(
        !replacement.is_empty(),
        "{case_id} retirement decision needs replacement evidence"
    );
    for item in replacement {
        let item = object(item, &format!("{case_id}.retirement_decision.replacement"));
        let kind = required_str(item, "kind", case_id);
        assert!(
            ALLOWED_REPLACEMENT_KINDS.contains(&kind),
            "{case_id} has unexpected replacement kind {kind:?}"
        );
        let strength = required_str(item, "strength", case_id);
        assert!(
            ALLOWED_REPLACEMENT_STRENGTHS.contains(&strength),
            "{case_id} has unexpected replacement strength {strength:?}"
        );
        let note = required_str(item, "note", case_id);
        assert!(
            note.len() > 20,
            "{case_id} replacement {kind} needs a substantive note"
        );
        if kind == "rust_test" {
            let path = required_str(item, "path", case_id);
            assert!(
                !path.is_empty() && !Path::new(path).is_absolute(),
                "{case_id} replacement path must stay repository-relative: {path}"
            );
            assert!(
                Path::new(path)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rs")),
                "{case_id} replacement path is not a .rs file: {path}"
            );
            let line = usize::try_from(required_u64(item, "line", case_id))
                .expect("replacement line fits usize");
            let test = required_str(item, "test", case_id);
            assert!(
                !test.is_empty()
                    && test.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    }),
                "{case_id} replacement test name is not a Rust function: {test}"
            );
            let source = std::fs::read_to_string(workspace.join(path)).unwrap_or_else(|error| {
                panic!(
                    "{case_id} replacement source is missing at {}: {error}",
                    workspace.join(path).display()
                )
            });
            let declaration = source
                .lines()
                .nth(line.saturating_sub(1))
                .unwrap_or_default();
            assert!(
                declaration.trim_start().starts_with(&format!("fn {test}(")),
                "{case_id} replacement {test} is not declared at {path}:{line}"
            );
        }
    }

    (id, scope)
}

fn is_stable_slug(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

/// Validates a `reviewed` case and returns its review-group id.
fn validate_review_decision<'a>(case: &'a Map<String, Value>, case_id: &str) -> &'a str {
    let review = required_object(case, "review", case_id);
    let group = required_str(review, "group_id", case_id);
    assert!(
        is_stable_slug(group),
        "{case_id} review group id is not a stable slug: {group}"
    );
    let note = required_str(review, "note", case_id);
    assert!(
        note.len() > 20,
        "{case_id} reviewed case needs a substantive review note"
    );
    let evidence = required_object(case, "migration_evidence", case_id);
    let parity = required_str(evidence, "parity", case_id);
    assert!(
        parity != "exact",
        "{case_id} reviewed case must not carry exact parity"
    );
    assert!(
        !required_bool(evidence, "green_eligible", case_id),
        "{case_id} reviewed case must not be green eligible"
    );
    group
}

/// Validates a `waived` case and returns `(group_id, gap_kind)`.
fn validate_waiver<'a>(case: &'a Map<String, Value>, case_id: &str) -> (&'a str, &'a str) {
    let waiver = required_object(case, "waiver", case_id);
    let group = required_str(waiver, "group_id", case_id);
    assert!(
        is_stable_slug(group),
        "{case_id} waiver group id is not a stable slug: {group}"
    );
    let gap_kind = required_str(waiver, "gap_kind", case_id);
    assert!(
        ALLOWED_WAIVER_GAP_KINDS.contains(&gap_kind),
        "{case_id} has unexpected waiver gap_kind {gap_kind:?}"
    );
    let note = required_str(waiver, "note", case_id);
    assert!(
        note.len() > 20,
        "{case_id} waived case needs a substantive note"
    );
    assert!(
        !required_str(waiver, "follow_up", case_id).is_empty(),
        "{case_id} waived case needs a follow_up"
    );
    match gap_kind {
        "no-rust-evidence" => {
            assert!(
                case.get("migration_evidence").is_none(),
                "{case_id} no-rust-evidence waiver must not carry migration_evidence"
            );
        }
        "known-defect" => {
            assert!(
                !required_str(waiver, "product_impact", case_id).is_empty(),
                "{case_id} known-defect waiver needs a product_impact"
            );
            let evidence = required_object(case, "migration_evidence", case_id);
            assert_eq!(
                required_str(evidence, "parity", case_id),
                "blocked",
                "{case_id} known-defect waiver must keep blocked parity"
            );
            assert!(
                !required_bool(evidence, "green_eligible", case_id),
                "{case_id} known-defect waiver must not be green eligible"
            );
        }
        other => panic!("{case_id} has unexpected waiver gap_kind {other:?}"),
    }
    (group, gap_kind)
}

/// Validates the C-lite closeout policy and its group counts against the cases.
fn validate_closeout(root: &Map<String, Value>, cases: &[Value]) {
    let closeout = required_object(root, "closeout", "ledger root");
    assert_eq!(
        required_str(closeout, "policy", "ledger.closeout"),
        CLOSEOUT_POLICY
    );
    assert_eq!(
        required_str(closeout, "date", "ledger.closeout"),
        CLOSEOUT_DATE
    );
    assert_eq!(
        required_str(closeout, "scope", "ledger.closeout"),
        CLOSEOUT_SCOPE
    );
    assert!(
        !required_str(closeout, "owner", "ledger.closeout").is_empty(),
        "ledger.closeout needs an owner"
    );
    assert!(
        !required_str(closeout, "decision", "ledger.closeout").is_empty(),
        "ledger.closeout needs a decision"
    );
    assert_eq!(
        required_u64(closeout, "planned_target", "ledger.closeout"),
        0,
        "P7-1 closeout target must be zero planned"
    );

    let mut review_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut waiver_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut known_defects = 0_usize;
    let mut no_evidence = 0_usize;
    let mut never_green = 0_usize;

    for case in cases {
        let case = object(case, "case");
        let case_id = required_str(case, "case_id", "case");
        *status_counts
            .entry(required_str(case, "status", case_id).to_owned())
            .or_default() += 1;
        if let Some(review) = case.get("review") {
            let review = object(review, &format!("{case_id}.review"));
            *review_counts
                .entry(required_str(review, "group_id", case_id).to_owned())
                .or_default() += 1;
        }
        if let Some(waiver) = case.get("waiver") {
            let waiver = object(waiver, &format!("{case_id}.waiver"));
            *waiver_counts
                .entry(required_str(waiver, "group_id", case_id).to_owned())
                .or_default() += 1;
            match required_str(waiver, "gap_kind", case_id) {
                "known-defect" => {
                    known_defects += 1;
                    never_green += 1;
                }
                "no-rust-evidence" => no_evidence += 1,
                other => panic!("{case_id} has unexpected waiver gap_kind {other:?}"),
            }
        }
        if let Some(decision) = case.get("retirement_decision") {
            let decision = object(decision, &format!("{case_id}.retirement_decision"));
            let id = required_str(decision, "id", case_id);
            let scope = required_str(decision, "scope", case_id);
            if id.starts_with("wave12-e-") && scope != "internal_evidence" {
                never_green += 1;
            }
        }
        if case_id == "TS-COMPILER-0095" {
            never_green += 1;
        }
    }

    assert_eq!(review_counts.values().sum::<usize>(), REVIEWED_CASES);
    assert_eq!(waiver_counts.values().sum::<usize>(), WAIVED_CASES);
    assert_eq!(
        status_counts.get("planned").copied().unwrap_or_default(),
        PLANNED_CASES
    );
    assert_eq!(
        status_counts.get("green").copied().unwrap_or_default(),
        GREEN_CASES
    );
    assert_eq!(
        status_counts.get("reviewed").copied().unwrap_or_default(),
        REVIEWED_CASES
    );
    assert_eq!(
        status_counts.get("waived").copied().unwrap_or_default(),
        WAIVED_CASES
    );
    assert_eq!(
        status_counts
            .get("retired-with-decision")
            .copied()
            .unwrap_or_default(),
        RETIRED_CASES
    );

    let counts = required_object(closeout, "counts", "ledger.closeout");
    for (key, expected) in [
        ("green", GREEN_CASES),
        ("reviewed", REVIEWED_CASES),
        ("waived", WAIVED_CASES),
        ("retired", RETIRED_CASES),
        ("planned", PLANNED_CASES),
        ("never_green", 38),
        ("known_defects", 7),
        ("no_evidence", 22),
    ] {
        assert_eq!(
            required_u64(counts, key, "ledger.closeout.counts"),
            u64::try_from(expected).expect("closeout count fits u64"),
            "ledger.closeout.counts.{key} drifted"
        );
    }
    assert_eq!(known_defects, 7, "known-defect count drifted");
    assert_eq!(
        no_evidence, WAIVED_WITHOUT_EVIDENCE,
        "no-evidence count drifted"
    );
    assert_eq!(never_green, 38, "never-green count drifted");

    let review_groups = required_array(closeout, "review_groups", "ledger.closeout");
    assert_eq!(review_groups.len(), review_counts.len());
    let mut review_ids = BTreeSet::new();
    for group in review_groups {
        let group = object(group, "ledger.closeout.review_groups[]");
        let id = required_str(group, "id", "ledger.closeout.review_groups[]");
        assert!(
            review_ids.insert(id.to_owned()),
            "duplicate review group {id}"
        );
        assert_eq!(
            required_u64(group, "count", id),
            u64::try_from(review_counts.get(id).copied().unwrap_or_default())
                .expect("review count fits u64"),
            "review group {id} count drifted"
        );
        assert!(
            required_str(group, "accepted_risk", id).len() > 40,
            "review group {id} needs a substantive accepted_risk"
        );
        assert!(
            !required_str(group, "product_impact", id).is_empty(),
            "review group {id} needs a product_impact"
        );
        assert!(
            !required_str(group, "follow_up", id).is_empty(),
            "review group {id} needs a follow_up"
        );
    }
    assert_eq!(
        review_ids,
        review_counts.keys().cloned().collect::<BTreeSet<_>>()
    );

    let waiver_groups = required_array(closeout, "waiver_groups", "ledger.closeout");
    assert_eq!(waiver_groups.len(), waiver_counts.len());
    let mut waiver_ids = BTreeSet::new();
    for group in waiver_groups {
        let group = object(group, "ledger.closeout.waiver_groups[]");
        let id = required_str(group, "id", "ledger.closeout.waiver_groups[]");
        assert!(
            waiver_ids.insert(id.to_owned()),
            "duplicate waiver group {id}"
        );
        assert_eq!(
            required_u64(group, "count", id),
            u64::try_from(waiver_counts.get(id).copied().unwrap_or_default())
                .expect("waiver count fits u64"),
            "waiver group {id} count drifted"
        );
        let gap_kind = required_str(group, "gap_kind", id);
        assert!(
            ALLOWED_WAIVER_GAP_KINDS.contains(&gap_kind),
            "waiver group {id} has unexpected gap_kind {gap_kind:?}"
        );
        assert!(
            required_str(group, "reason", id).len() > 40,
            "waiver group {id} needs a substantive reason"
        );
        assert!(
            !required_str(group, "product_impact", id).is_empty(),
            "waiver group {id} needs a product_impact"
        );
        assert!(
            !required_str(group, "follow_up", id).is_empty(),
            "waiver group {id} needs a follow_up"
        );
    }
    assert_eq!(
        waiver_ids,
        waiver_counts.keys().cloned().collect::<BTreeSet<_>>()
    );

    let non_claims = required_string_array(closeout, "non_claims", "ledger.closeout");
    assert!(!non_claims.is_empty());
    assert!(non_claims.iter().all(|claim| !claim.is_empty()));
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
        ("rewrite", 51),
        ("split", 11),
        ("obsolete", 37),
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
    let reviewed = statuses.get("reviewed").copied().unwrap_or_default();
    let waived = statuses.get("waived").copied().unwrap_or_default();
    let retired = statuses
        .get("retired-with-decision")
        .copied()
        .unwrap_or_default();
    assert_eq!(green, GREEN_CASES, "green case count drifted");
    assert_eq!(planned, PLANNED_CASES, "planned case count drifted");
    assert_eq!(reviewed, REVIEWED_CASES, "reviewed case count drifted");
    assert_eq!(waived, WAIVED_CASES, "waived case count drifted");
    assert_eq!(retired, RETIRED_CASES, "retired case count drifted");
    assert_eq!(statuses.len(), 4, "unexpected non-zero status bucket");
    assert_eq!(
        green + planned + reviewed + waived + retired,
        TOTAL_CASES,
        "status counts must sum to the total case count"
    );

    let declared_statuses = required_object(counts, "statuses", "ledger.counts");
    assert_eq!(
        declared_statuses.len(),
        5,
        "counts.statuses must contain each status bucket"
    );
    for (status, expected) in [
        ("planned", planned),
        ("green", green),
        ("reviewed", reviewed),
        ("waived", waived),
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
        integrator.len(),
        8,
        "the integrator decision buckets drifted"
    );
    for (key, expected) in [
        ("accepted_obsolete", OBSOLETE_CASES),
        ("accepted_rewrite", 0),
        ("accepted_superseded_contract", 23),
        ("accepted_mechanism_replaced", 8),
        ("accepted_internal_evidence", 3),
        ("accepted_reviewed", REVIEWED_CASES),
        ("accepted_waived", WAIVED_CASES),
        ("remaining_blocked", 0),
    ] {
        assert_eq!(
            required_u64(integrator, key, "ledger.counts.integrator_decisions"),
            u64::try_from(expected).expect("integrator count fits u64"),
            "integrator decision {key} drifted"
        );
    }
}

#[test]
fn ledger_cases_are_unique_and_routed_without_claims() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");
    let cases = required_array(root, "cases", "ledger root");
    let all_case_ids = case_ids(cases);
    assert_eq!(all_case_ids.len(), TOTAL_CASES, "duplicate case IDs");

    let workspace = workspace_root();
    let mut targets = BTreeSet::new();
    let mut non_obsolete = 0_usize;
    let mut obsolete = 0_usize;
    let mut decision_ids = Vec::new();
    let mut non_obsolete_decision_ids = Vec::new();
    let mut retirement_decisions = Vec::new();

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
        let retirement_scope = if case.get("retirement_decision").is_some() {
            assert_eq!(
                status, "retired-with-decision",
                "{case_id} carries a retirement_decision without retired status"
            );
            let (id, scope) = validate_retirement_decision(case, case_id, &workspace);
            retirement_decisions.push((case_id, id, scope));
            Some(scope)
        } else {
            None
        };
        let review_group = if case.get("review").is_some() {
            assert_eq!(
                status, "reviewed",
                "{case_id} carries a review without reviewed status"
            );
            assert!(
                case.get("retirement_decision").is_none(),
                "{case_id} reviewed case must not carry a retirement_decision"
            );
            Some(validate_review_decision(case, case_id))
        } else {
            None
        };
        let waiver = if case.get("waiver").is_some() {
            assert_eq!(
                status, "waived",
                "{case_id} carries a waiver without waived status"
            );
            assert!(
                case.get("retirement_decision").is_none(),
                "{case_id} waived case must not carry a retirement_decision"
            );
            Some(validate_waiver(case, case_id))
        } else {
            None
        };

        let target = required_str(case, "target_rust_test", case_id);
        match disposition {
            "obsolete" => {
                obsolete += 1;
                assert_eq!(status, "retired-with-decision", "{case_id}");
                assert_eq!(target_status, RETIRED_TARGET_STATUS, "{case_id}");
                assert!(requires_decision, "{case_id} needs an integrator decision");
                assert!(review_group.is_none(), "{case_id}");
                assert!(waiver.is_none(), "{case_id}");
                match retirement_scope {
                    Some(scope) => assert_eq!(
                        scope, "candidate_b_layout",
                        "{case_id} obsolete retirement must use candidate_b_layout"
                    ),
                    None => {
                        assert_eq!(
                            required_str(case, "decision_status", case_id),
                            "accepted-obsolete",
                            "{case_id} needs a retirement_decision or the 2026-11-17 accepted-obsolete decision"
                        );
                        assert_eq!(
                            required_str(case, "decision_at", case_id),
                            LEGACY_CANDIDATE_B_DECISION_DATE,
                            "{case_id} legacy Candidate-B decision date drifted"
                        );
                        assert!(
                            reason.contains("Candidate B"),
                            "{case_id} must cite the retired Candidate B layout"
                        );
                    }
                }
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
                        assert!(review_group.is_none(), "{case_id}");
                        assert!(waiver.is_none(), "{case_id}");
                        assert!(retirement_scope.is_none(), "{case_id}");
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
                    "planned" => {
                        assert_eq!(target_status, PLANNED_TARGET_STATUS, "{case_id}");
                        assert!(review_group.is_none(), "{case_id}");
                        assert!(waiver.is_none(), "{case_id}");
                        assert!(retirement_scope.is_none(), "{case_id}");
                    }
                    "reviewed" => {
                        assert_eq!(target_status, REVIEWED_TARGET_STATUS, "{case_id}");
                        assert!(review_group.is_some(), "{case_id}");
                        assert!(waiver.is_none(), "{case_id}");
                        assert!(retirement_scope.is_none(), "{case_id}");
                        let evidence = required_object(case, "migration_evidence", case_id);
                        assert_ne!(
                            required_str(evidence, "parity", case_id),
                            "exact",
                            "{case_id} reviewed case must not carry exact parity"
                        );
                        assert!(
                            !required_bool(evidence, "green_eligible", case_id),
                            "{case_id} reviewed case must not be green eligible"
                        );
                        assert!(
                            !required_array(evidence, "rust_tests", case_id).is_empty(),
                            "{case_id} reviewed case needs real Rust evidence"
                        );
                    }
                    "waived" => {
                        assert_eq!(target_status, WAIVED_TARGET_STATUS, "{case_id}");
                        let (_, gap_kind) = waiver.expect("waived case needs a waiver");
                        assert!(review_group.is_none(), "{case_id}");
                        assert!(retirement_scope.is_none(), "{case_id}");
                        match gap_kind {
                            "no-rust-evidence" => assert!(
                                case.get("migration_evidence").is_none(),
                                "{case_id} no-evidence waiver must not carry migration_evidence"
                            ),
                            "known-defect" => assert!(
                                case.get("migration_evidence").is_some(),
                                "{case_id} known-defect waiver must keep blocked evidence"
                            ),
                            other => panic!("{case_id} has unexpected waiver gap_kind {other:?}"),
                        }
                    }
                    "retired-with-decision" => {
                        assert_eq!(target_status, RETIRED_TARGET_STATUS, "{case_id}");
                        assert!(
                            case.get("migration_evidence").is_none(),
                            "{case_id} retired evidence must stay outside the migration ledger"
                        );
                        assert!(review_group.is_none(), "{case_id}");
                        assert!(waiver.is_none(), "{case_id}");
                        assert!(
                            retirement_scope.is_some(),
                            "{case_id} non-obsolete retirement needs a retirement_decision"
                        );
                    }
                    other => panic!("{case_id} has unexpected status {other:?}"),
                }
            }
            other => panic!("{case_id} has unexpected disposition {other:?}"),
        }
    }

    assert_eq!(
        non_obsolete,
        PLANNED_CASES + GREEN_CASES + REVIEWED_CASES + WAIVED_CASES + NON_OBSOLETE_RETIRED_CASES
    );
    assert_eq!(obsolete, OBSOLETE_CASES);
    assert_eq!(
        targets.len(),
        PLANNED_CASES + GREEN_CASES + REVIEWED_CASES + WAIVED_CASES + NON_OBSOLETE_RETIRED_CASES
    );
    assert_eq!(decision_ids.len(), OBSOLETE_CASES);
    assert!(
        non_obsolete_decision_ids.is_empty(),
        "wave-11 retirements must use retirement_decision, not requires_decision: {non_obsolete_decision_ids:?}"
    );
    retirement_decisions.sort_unstable();
    assert_eq!(
        retirement_decisions, RETIREMENT_DECISIONS,
        "the wave-11/C-lite retirement decision ids/scopes drifted"
    );
    validate_closeout(root, cases);

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
        let increment_status = required_str(increment, "status", id);
        assert!(
            DIRTY_INCREMENT_STATUSES.contains(&increment_status),
            "dirty increment {id} has unexpected terminal status {increment_status:?}"
        );
        assert_ne!(
            increment_status, "green",
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
    let mut reviewed = 0_usize;
    let mut waived_cases = 0_usize;
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

        if status == "retired-with-decision" {
            assert_eq!(target_status, RETIRED_TARGET_STATUS, "{case_id}");
            assert!(
                case.get("migration_evidence").is_none(),
                "{case_id} retired evidence must stay outside the migration ledger"
            );
            if disposition != "obsolete" {
                assert!(
                    case.get("retirement_decision").is_some(),
                    "{case_id} non-obsolete retirement needs a retirement_decision"
                );
            }
            continue;
        }

        let module = target_partition(required_str(case, "target_rust_test", case_id));
        let partition = partition_counts.entry(module.to_owned()).or_default();
        *partition.entry("routed_cases".to_owned()).or_default() += 1;
        if status == "planned" {
            planned += 1;
        }
        if status == "reviewed" {
            reviewed += 1;
            *partition.entry("reviewed_cases".to_owned()).or_default() += 1;
        }
        if status == "waived" {
            waived_cases += 1;
            *partition.entry("waived_cases".to_owned()).or_default() += 1;
        }

        let Some(evidence) = case.get("migration_evidence") else {
            assert_eq!(
                status, "waived",
                "{case_id} must be a no-evidence waiver without migration_evidence"
            );
            assert_eq!(target_status, WAIVED_TARGET_STATUS, "{case_id}");
            let waiver = required_object(case, "waiver", case_id);
            assert_eq!(
                required_str(waiver, "gap_kind", case_id),
                "no-rust-evidence",
                "{case_id} has no evidence but is not a no-rust-evidence waiver"
            );
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

        if case_id.starts_with("TS-MATRIX") {
            validate_matrix_evidence(case_id, evidence, parity);
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
                assert_eq!(status, "reviewed", "{case_id} near-exact must be reviewed");
                assert_eq!(target_status, REVIEWED_TARGET_STATUS, "{case_id}");
                assert_eq!(recommendation, "green", "{case_id}");
            }
            "weaker" | "structural" => {
                assert!(
                    !eligible,
                    "{case_id} non-exact evidence must not be eligible"
                );
                assert_eq!(status, "reviewed", "{case_id}");
                assert_eq!(target_status, REVIEWED_TARGET_STATUS, "{case_id}");
                assert_eq!(recommendation, "planned", "{case_id}");
            }
            "blocked" => {
                assert!(!eligible, "{case_id} blocked evidence must not be eligible");
                assert_eq!(recommendation, "planned", "{case_id}");
                if status == "waived" {
                    assert_eq!(target_status, WAIVED_TARGET_STATUS, "{case_id}");
                    let waiver = required_object(case, "waiver", case_id);
                    assert_eq!(
                        required_str(waiver, "gap_kind", case_id),
                        "known-defect",
                        "{case_id} blocked waiver must be a known-defect waiver"
                    );
                } else {
                    assert_eq!(status, "reviewed", "{case_id}");
                    assert_eq!(target_status, REVIEWED_TARGET_STATUS, "{case_id}");
                }
            }
            other => panic!("{case_id} has unexpected parity {other:?}"),
        }
    }

    assert_eq!(green, GREEN_CASES, "green count drifted");
    assert_eq!(planned, PLANNED_CASES, "planned count drifted");
    assert_eq!(reviewed, REVIEWED_CASES, "reviewed count drifted");
    assert_eq!(waived_cases, WAIVED_CASES, "waived count drifted");
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
        without_evidence, WAIVED_WITHOUT_EVIDENCE,
        "waived cases without evidence drifted"
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
        u64::try_from(PLANNED_CASES).expect("planned count fits u64")
    );
    assert_eq!(
        required_u64(
            declared,
            "waived_without_evidence",
            "ledger.counts.migration_evidence"
        ),
        u64::try_from(without_evidence).expect("waived count fits u64")
    );

    let declared_statuses =
        required_object(declared, "statuses", "ledger.counts.migration_evidence");
    for (status, expected) in [
        ("planned", planned),
        ("green", green),
        ("reviewed", reviewed),
        ("waived", waived_cases),
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
            "reviewed_cases",
            "waived_cases",
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
fn ledger_closeout_policy_is_frozen() {
    let ledger = load_ledger();
    let root = object(&ledger, "ledger root");
    let cases = required_array(root, "cases", "ledger root");
    validate_closeout(root, cases);

    let mut exact_non_green = 0_usize;
    let mut green_non_exact = 0_usize;
    let mut reviewed_or_waived_eligible = 0_usize;
    let mut retired_with_evidence = 0_usize;
    let mut no_evidence_waiver_with_evidence = 0_usize;

    for case in cases {
        let case = object(case, "case");
        let case_id = required_str(case, "case_id", "case");
        let status = required_str(case, "status", case_id);
        let evidence = case.get("migration_evidence");
        let parity = evidence.and_then(|value| {
            value
                .as_object()
                .and_then(|value| value.get("parity"))
                .and_then(Value::as_str)
        });
        if parity == Some("exact") && status != "green" {
            exact_non_green += 1;
        }
        if status == "green" && parity != Some("exact") {
            green_non_exact += 1;
        }
        if matches!(status, "reviewed" | "waived") {
            if let Some(evidence) = evidence {
                let evidence = object(evidence, case_id);
                if required_bool(evidence, "green_eligible", case_id) {
                    reviewed_or_waived_eligible += 1;
                }
            }
        }
        if status == "retired-with-decision" && evidence.is_some() {
            retired_with_evidence += 1;
        }
        if status == "waived" {
            let waiver = required_object(case, "waiver", case_id);
            if required_str(waiver, "gap_kind", case_id) == "no-rust-evidence" && evidence.is_some()
            {
                no_evidence_waiver_with_evidence += 1;
            }
        }
    }

    assert_eq!(exact_non_green, 0, "exact parity must always be green");
    assert_eq!(green_non_exact, 0, "green must always be exact");
    assert_eq!(
        reviewed_or_waived_eligible, 0,
        "reviewed/waived cases must never be green eligible"
    );
    assert_eq!(
        retired_with_evidence, 0,
        "retired cases must not carry migration evidence"
    );
    assert_eq!(
        no_evidence_waiver_with_evidence, 0,
        "no-evidence waivers must not carry migration evidence"
    );
}

/// Validates the frozen matrix projection and the shared required-native
/// evidence for every `TS-MATRIX-*` case.
///
/// The per-case `target_rust_test` stays the ledger routing placeholder used
/// by all 43 matrix cases; the actual engine assertions live in
/// `native_parity/real_matrix.rs` and are named explicitly in `rust_tests`.
fn validate_matrix_evidence(case_id: &str, evidence: &Map<String, Value>, parity: &str) {
    let matrix = required_object(evidence, "scenario_matrix", case_id);
    assert_eq!(
        required_str(matrix, "case", case_id),
        case_id,
        "{case_id} matrix evidence names another case"
    );
    assert_eq!(
        required_str(matrix, "projection_sha256", case_id),
        MATRIX_PROJECTION_SHA256,
        "{case_id} matrix projection SHA drifted"
    );
    let layout = required_str(matrix, "layout", case_id);
    assert!(
        ALLOWED_MATRIX_LAYOUTS.contains(&layout),
        "{case_id} has unexpected matrix layout {layout:?}"
    );
    let scenarios = required_array(matrix, "scenarios", case_id);
    assert!(!scenarios.is_empty(), "{case_id} has no matrix scenarios");
    assert!(
        scenarios
            .iter()
            .all(|scenario| scenario.as_str().is_some_and(|value| !value.is_empty())),
        "{case_id} matrix scenarios must be non-empty strings"
    );
    let assertions = required_array(matrix, "oracle_assertions", case_id);
    assert!(
        !assertions.is_empty(),
        "{case_id} has no matrix oracle assertions"
    );
    assert!(
        assertions
            .iter()
            .all(|assertion| assertion.as_str().is_some_and(|value| !value.is_empty())),
        "{case_id} matrix oracle assertions must be non-empty strings"
    );

    if let Some(stem) = case_id
        .strip_prefix("TS-MATRIX-0102-")
        .and_then(|stem| stem.strip_suffix("-IN_PLACE_SHADOW"))
    {
        let expected = stem.to_ascii_lowercase().replace('_', "-");
        assert_eq!(
            layout, "in-place-shadow",
            "{case_id} in-place case must use the frozen layout"
        );
        assert_eq!(
            scenarios,
            &[Value::String(expected)],
            "{case_id} matrix scenario does not match its case id"
        );
    } else {
        assert_eq!(
            layout, "in-place-half",
            "{case_id} top-level case must use the in-place half"
        );
    }

    if parity == "blocked" {
        assert_eq!(
            required_str(evidence, "blocked_by", case_id),
            "checker",
            "{case_id} matrix blocked evidence must stay checker-owned"
        );
        let group = required_str(evidence, "blocked_group", case_id);
        assert!(
            ALLOWED_MATRIX_BLOCKED_GROUPS.contains(&group),
            "{case_id} has unexpected matrix blocked group {group:?}"
        );
        assert!(
            !required_str(evidence, "observed", case_id).is_empty(),
            "{case_id} matrix blocked evidence needs an observed fact"
        );
        assert!(
            !required_str(evidence, "expected", case_id).is_empty(),
            "{case_id} matrix blocked evidence needs an expected oracle fact"
        );
    } else {
        assert!(
            evidence.get("blocked_by").is_none(),
            "{case_id} exact matrix evidence must not carry blocked_by"
        );
        assert!(
            evidence.get("blocked_group").is_none(),
            "{case_id} exact matrix evidence must not carry blocked_group"
        );
    }

    let required_tests: &[&str] = match (parity, case_id.contains("-IN_PLACE_SHADOW")) {
        ("exact", true) => &[
            "real_matrix_manifest_is_frozen_and_oracle_derived",
            "real_matrix_in_place_matches_frozen_oracle",
        ],
        ("exact", false) => &[
            "real_matrix_manifest_is_frozen_and_oracle_derived",
            "real_matrix_top_level_cases",
        ],
        ("blocked", _) => &[
            "real_matrix_manifest_is_frozen_and_oracle_derived",
            "real_matrix_known_gaps_are_characterized",
        ],
        _ => &[],
    };
    let tests = required_array(evidence, "rust_tests", case_id);
    for required in required_tests {
        assert!(
            tests.iter().any(|test| {
                test.get("test").and_then(Value::as_str) == Some(*required)
                    && test
                        .get("path")
                        .and_then(Value::as_str)
                        .is_some_and(|path| path.ends_with("native_parity/real_matrix.rs"))
            }),
            "{case_id} matrix evidence is missing required test {required}"
        );
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

    // P7-2: the archive manifest is required whether or not the live legacy
    // tree still exists. When live bytes are present they are re-hashed; when
    // the tree is deleted, the frozen manifest is verified against this ledger.
    let manifest = oracle_manifest::load_manifest(&workspace);
    let report = oracle_manifest::verify_legacy_oracle(
        &ledger,
        &manifest,
        &workspace.join(oracle_manifest::LEGACY_TEST_SOURCE_ROOT),
    )
    .unwrap_or_else(|error| panic!("legacy oracle gate: {error}"));
    assert_eq!(
        report.files,
        files.len(),
        "legacy oracle file count drifted"
    );
    assert_eq!(
        report.cases, TOTAL_CASES,
        "legacy oracle case count drifted"
    );
    let live_root = workspace.join(oracle_manifest::LEGACY_TEST_SOURCE_ROOT);
    let expected_mode = if live_root.is_dir() {
        oracle_manifest::LegacySourceMode::Live
    } else {
        oracle_manifest::LegacySourceMode::Frozen
    };
    assert_eq!(
        report.mode, expected_mode,
        "legacy oracle gate used the wrong source authority"
    );

    let mut file_names = BTreeSet::new();
    let mut file_line_limits: BTreeMap<String, u64> = BTreeMap::new();
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

        let declared_sha = required_str(file, "sha256", name);
        assert!(
            is_lowercase_sha256(declared_sha),
            "{name} has an invalid source hash"
        );
        let manifest_file = manifest
            .files
            .iter()
            .find(|entry| entry.file == name)
            .unwrap_or_else(|| panic!("legacy oracle manifest is missing {name}"));
        assert_eq!(
            manifest_file.sha256, declared_sha,
            "manifest/ledger source hash drift for {name}"
        );
        file_line_limits.insert(name.to_owned(), manifest_file.lines);

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
    assert_eq!(
        required_str(support, "sha256", "ledger.support_file"),
        manifest.support.sha256,
        "support source hash drifted from the legacy oracle manifest"
    );
    assert_eq!(
        support_path, manifest.support.path,
        "support path drifted from the legacy oracle manifest"
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

        let line = required_u64(source, "line", case_id);
        assert!(line > 0, "{case_id} has line 0");
        let line_limit = file_line_limits
            .get(file)
            .unwrap_or_else(|| panic!("{case_id} references unknown file {file}"));
        assert!(
            line <= *line_limit,
            "{case_id} points past the frozen end of {file}:{line} > {line_limit}"
        );
        let manifest_case = manifest
            .cases
            .iter()
            .find(|entry| entry.case_id == case_id)
            .unwrap_or_else(|| panic!("{case_id} is missing from the legacy oracle manifest"));
        assert_eq!(manifest_case.file, file, "{case_id} manifest file drifted");
        assert_eq!(manifest_case.line, line, "{case_id} manifest line drifted");
        assert_eq!(
            manifest_case.body_sha256,
            required_str(source, "body_sha256", case_id),
            "{case_id} body_sha256 drifted from the legacy oracle manifest"
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

#[test]
fn frozen_oracle_manifest_matches_the_ledger() {
    let workspace = workspace_root();
    let ledger = load_ledger();
    let manifest = oracle_manifest::load_manifest(&workspace);
    oracle_manifest::verify_frozen_projection(&ledger, &manifest)
        .unwrap_or_else(|error| panic!("legacy oracle manifest cross-check: {error}"));

    assert_eq!(
        manifest.head,
        oracle_manifest::ORACLE_BASELINE_HEAD,
        "the frozen oracle manifest was regenerated at a different HEAD"
    );
    assert_eq!(
        manifest.cases.len(),
        TOTAL_CASES,
        "the manifest must archive all 339 cases"
    );
    assert_eq!(
        manifest.files.len(),
        21,
        "the manifest must archive 21 files"
    );
    assert_eq!(
        manifest.trees.len(),
        4,
        "the manifest must archive four fixture trees"
    );
    assert_eq!(
        manifest.projection_sha256,
        oracle_manifest::FROZEN_MANIFEST_SHA256,
        "the manifest projection digest drifted"
    );
}

#[test]
fn frozen_oracle_gate_passes_when_legacy_tree_is_absent() {
    let workspace = workspace_root();
    let ledger = load_ledger();
    let manifest = oracle_manifest::load_manifest(&workspace);
    // Simulate the P7-2 deletion without touching the live checkout: the gate
    // receives a root that does not exist and must use the frozen manifest.
    let absent = workspace.join("backends/typescript/test.__simulated_deleted__");
    assert!(
        !absent.exists(),
        "the simulated deletion path must not exist: {}",
        absent.display()
    );
    let report = oracle_manifest::verify_legacy_oracle(&ledger, &manifest, &absent)
        .unwrap_or_else(|error| panic!("frozen oracle gate must pass without live bytes: {error}"));
    assert_eq!(report.mode, oracle_manifest::LegacySourceMode::Frozen);
    assert_eq!(report.files, 21);
    assert_eq!(report.cases, TOTAL_CASES);
}
