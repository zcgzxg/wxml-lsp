//! Mandatory future gates for ignored tests and silent returns.
//!
//! P3-T does not migrate these gates. It freezes their inventory and proves
//! that the old Rust surface still contains exactly the 23 `#[ignore]` tests
//! and the two `semantic_tools` early returns recorded by the ledger. The
//! 23 gates and the two silent returns must become real native commands before
//! the old Node-backed tests may be deleted or treated as green.
//!
//! P6-2 now records twenty-three opt-in native gates as `gate_green`. The first
//! wave (`G-P5-E2E-09/12/14`) needed only the selector and static semantics.
//! The 2026-11-19 trusted type-assets slice added real typed hover and
//! diagnostics through `--backend native`, so ten more gates have exact native
//! replacements: `G-P5-E2E-04/06/07/08/10/13/15/17/18/19`. The 2026-11-20
//! routing/guard slice added root and trailing-dot completion, Behavior
//! properties and exact conditional-guard ranges, so
//! `G-P5-E2E-03/11/16` are also green. The 2026-11-20 performance/stability
//! slice added a real `--stdio --backend native` p50/p95/p99 and RSS baseline
//! plus twelve update/release/restart cycles, so `G-P5-PERF-01` and
//! `G-P5-STABILITY-01` are green. The 2026-11-20 fourth wave added non-owner
//! overlay replay with the one-shot dead-session retry, event-handler hover,
//! script-symbol references/rename/quick fix and the non-literal `Unsupported`
//! class, so `G-P5-E2E-01/02/05/21` are green. The 2026-11-20 fifth wave added
//! component property binding diagnostics, so `G-P5-E2E-20` is green and no
//! gate stays `required_not_migrated`.
//!
//! `gate_green` is an opt-in required-native-job result (`--ignored`), not a
//! default-CI pass and not `mandatory_green`.

use std::collections::BTreeSet;
use std::path::Path;

use wxml_backend_tsgo::config::{AST_PROTOCOL_VERSION, ENGINE_VERSION, PinnedEngine};

use crate::fixture_hash::collect_tree_files;
use crate::ledger;
use crate::native_engine_real::{GATE_REPLACEMENTS, IGNORED_GATE_TESTS, NATIVE_GATE_FILE};
use crate::source_scan;

/// The only gates allowed to carry `gate_green` in P6-2.
const GATE_GREEN_IDS: &[&str] = &[
    "G-P5-E2E-01",
    "G-P5-E2E-02",
    "G-P5-E2E-03",
    "G-P5-E2E-04",
    "G-P5-E2E-05",
    "G-P5-E2E-06",
    "G-P5-E2E-07",
    "G-P5-E2E-08",
    "G-P5-E2E-09",
    "G-P5-E2E-10",
    "G-P5-E2E-11",
    "G-P5-E2E-12",
    "G-P5-E2E-13",
    "G-P5-E2E-14",
    "G-P5-E2E-15",
    "G-P5-E2E-16",
    "G-P5-E2E-17",
    "G-P5-E2E-18",
    "G-P5-E2E-19",
    "G-P5-E2E-20",
    "G-P5-E2E-21",
    "G-P5-PERF-01",
    "G-P5-STABILITY-01",
];
/// No gate waits for a native replacement after the fifth-wave promotion.
///
/// `G-P5-E2E-20` gained component property binding diagnostics; the last
/// blocker is closed in
/// `docs/evidence/2026-11-20-tsgo-p6-native-gates-wave5.md`.
const REQUIRED_NOT_MIGRATED_IDS: &[&str] = &[];
/// Frozen `gate_green` count after the fifth-wave promotion.
const GATE_GREEN_COUNT: usize = 23;
/// Frozen `required_not_migrated` count after the fifth-wave promotion.
const REQUIRED_NOT_MIGRATED_COUNT: usize = 0;
/// Frozen ignored-gate inventory size; P6-2 never deletes a gate.
const IGNORED_GATE_COUNT: usize = 23;
/// Pinned size of the `7.1.0-beta` `lib/tsc` artifact.
const PINNED_ENGINE_SIZE: u64 = 24_522_912;

/// The exact ignored-native-gate set declared by `native_engine_real`.
fn declared_native_gate_tests() -> BTreeSet<(String, String)> {
    IGNORED_GATE_TESTS
        .iter()
        .map(|(file, function)| ((*file).to_owned(), (*function).to_owned()))
        .collect()
}

fn read_source(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn ignored_functions() -> (usize, BTreeSet<(String, String)>) {
    let workspace = ledger::workspace_root();
    let tests_root = workspace.join("crates/wxml-lsp/tests");
    let mut files = collect_tree_files(&tests_root)
        .unwrap_or_else(|error| panic!("walk {}: {error}", tests_root.display()));
    files.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
    });

    let mut count = 0_usize;
    let mut functions = BTreeSet::new();
    for file in files.iter().filter(|file| {
        Path::new(&file.relative_path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
    }) {
        let source = read_source(&file.absolute_path);
        let lines: Vec<&str> = source.lines().collect();
        let workspace_relative = ledger::relative_slash(&workspace, &file.absolute_path)
            .unwrap_or_else(|error| panic!("relative path: {error}"));
        for (index, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with("#[ignore") {
                continue;
            }
            count += 1;
            let (function_index, name) = lines
                .iter()
                .enumerate()
                .skip(index + 1)
                .take(32)
                .find_map(|(function_index, candidate)| {
                    source_scan::parse_fn_name(candidate).map(|name| (function_index, name))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no fn after #[ignore] in {workspace_relative}:{}",
                        index + 1
                    )
                });
            assert!(
                source_scan::attribute_block_contains(&lines, function_index, "#[ignore"),
                "{workspace_relative}::{name} must keep #[ignore] in its attribute block"
            );
            functions.insert((workspace_relative.clone(), name));
        }
    }
    (count, functions)
}

fn assert_related_cases_exist(
    gate: &serde_json::Map<String, serde_json::Value>,
    gate_id: &str,
    case_ids: &BTreeSet<String>,
) {
    for case in ledger::required_array(gate, "related_ledger_cases", gate_id) {
        let case = case
            .as_str()
            .unwrap_or_else(|| panic!("{gate_id} has a non-string related case"));
        assert!(
            case_ids.contains(case),
            "{gate_id} references unknown ledger case {case}"
        );
    }
}

fn assert_required_text(
    gate: &serde_json::Map<String, serde_json::Value>,
    gate_id: &str,
    keys: &[&str],
) {
    for key in keys {
        assert!(
            !ledger::required_str(gate, key, gate_id).is_empty(),
            "{gate_id}.{key} must not be empty"
        );
    }
}

fn assert_git_object_id(value: &str, context: &str) {
    assert_eq!(value.len(), 40, "{context} must be a git object id");
    assert!(
        value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{context} must be hex: {value}"
    );
}

/// Requires complete, pinned evidence for a `gate_green` gate.
///
/// A green gate must name its replacement, its native prerequisite, its
/// removal condition, the exact opt-in command and the pinned engine/asset
/// identity. Empty evidence or a different engine must fail here rather than
/// silently counting as a pass.
fn assert_gate_green_evidence(gate: &serde_json::Map<String, serde_json::Value>, gate_id: &str) {
    for key in [
        "replacement_test",
        "replacement_kind",
        "native_prerequisite",
        "removal_condition",
    ] {
        assert!(
            !ledger::required_str(gate, key, gate_id).is_empty(),
            "{gate_id}.{key} must not be empty for a gate_green gate"
        );
    }

    let evidence = ledger::required_object(gate, "gate_evidence", gate_id);
    assert_required_text(
        evidence,
        &format!("{gate_id}.gate_evidence"),
        &["command", "run_date", "result", "scope"],
    );

    let command = ledger::required_str(evidence, "command", gate_id);
    for expected in [
        "WXML_TEST_NATIVE=1",
        "WXML_REQUIRE_REAL_ENGINE=1",
        "native_engine_real",
        "--ignored",
    ] {
        assert!(
            command.contains(expected),
            "{gate_id} evidence command must contain {expected:?}: {command}"
        );
    }

    let result = ledger::required_str(evidence, "result", gate_id);
    let passed = ledger::required_u64(evidence, "passed", gate_id);
    let filtered = ledger::required_u64(evidence, "filtered", gate_id);
    assert!(
        result.contains(&format!("{passed} passed"))
            && result.contains("0 failed")
            && result.contains(&format!("{filtered} filtered")),
        "{gate_id} evidence must record {passed} passed / 0 failed / {filtered} filtered: {result}"
    );
    assert_eq!(
        ledger::required_u64(evidence, "exit", gate_id),
        0,
        "{gate_id} evidence must record a zero exit"
    );

    for key in ["run_head", "recorded_at_head"] {
        assert_git_object_id(
            ledger::required_str(evidence, key, gate_id),
            &format!("{gate_id}.gate_evidence.{key}"),
        );
    }

    let engine = ledger::required_object(evidence, "engine_identity", gate_id);
    let pinned = PinnedEngine::pinned();
    assert_eq!(
        ledger::required_str(engine, "version", gate_id),
        ENGINE_VERSION,
        "{gate_id} engine identity drifted from the pin"
    );
    assert_eq!(
        ledger::required_str(engine, "git_head", gate_id),
        pinned.git_head(),
        "{gate_id} engine git head drifted from the pin"
    );
    assert_eq!(
        ledger::required_str(engine, "executable_sha256", gate_id),
        pinned.executable_sha256(),
        "{gate_id} engine SHA-256 drifted from the pin"
    );
    assert_eq!(
        ledger::required_u64(engine, "ast_protocol", gate_id),
        u64::from(AST_PROTOCOL_VERSION),
        "{gate_id} AST protocol drifted from the pin"
    );
    assert_eq!(
        ledger::required_u64(engine, "executable_size_bytes", gate_id),
        PINNED_ENGINE_SIZE,
        "{gate_id} engine size drifted from the pin"
    );

    let asset = ledger::required_object(evidence, "asset_identity", gate_id);
    assert_required_text(
        asset,
        &format!("{gate_id}.gate_evidence.asset_identity"),
        &["tsgo_asset_root", "standard_library", "typings_payload"],
    );

    let red = ledger::required_object(evidence, "fail_closed_evidence", gate_id);
    assert_required_text(
        red,
        &format!("{gate_id}.gate_evidence.fail_closed_evidence"),
        &["command", "result", "reason"],
    );
    let red_command = ledger::required_str(red, "command", gate_id);
    assert!(
        red_command.contains("--ignored") && !red_command.contains("WXML_TEST_NATIVE=1"),
        "{gate_id} fail-closed command must run the ignored gate without the opt-in \
         flag: {red_command}"
    );
    let red_result = ledger::required_str(red, "result", gate_id);
    let red_failed = ledger::required_u64(red, "failed", gate_id);
    assert!(
        red_result.contains("0 passed") && red_result.contains(&format!("{red_failed} failed")),
        "{gate_id} fail-closed evidence must record the 0 passed / {red_failed} failed run: \
         {red_result}"
    );
}

fn native_parity_function(target: &str, gate_id: &str) -> String {
    let (file, rest) = target
        .split_once("::")
        .unwrap_or_else(|| panic!("{gate_id} replacement lacks a Rust test path: {target}"));
    assert_eq!(
        file, "crates/wxml-lsp/tests/native_parity/symbols_edits.rs",
        "{gate_id} replacement must land in the symbols_edits partition"
    );
    let function = rest.split_whitespace().next().unwrap_or(rest);
    assert!(
        !function.is_empty(),
        "{gate_id} replacement has an empty function name"
    );
    function.to_owned()
}

fn assert_blocked_native_gate(function: &str, gate_id: &str) {
    assert!(
        crate::symbols_edits::BLOCKED_NATIVE_GATES
            .iter()
            .any(|gate| gate.test_id == function),
        "{gate_id} replacement {function} is not registered in \
         symbols_edits::BLOCKED_NATIVE_GATES"
    );
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn all_ignored_rust_tests_are_mandatory_gates_with_frozen_status() {
    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let gates = ledger::required_array(root, "ignored_rust_tests", "ledger root");
    assert_eq!(
        gates.len(),
        IGNORED_GATE_COUNT,
        "the frozen ignored-test inventory drifted"
    );

    let cases = ledger::required_array(root, "cases", "ledger root");
    let case_ids = ledger::case_ids(cases);
    let mut gate_ids = BTreeSet::new();
    let mut expected = BTreeSet::new();
    let mut green = BTreeSet::new();
    let mut required = BTreeSet::new();

    for gate in gates {
        let gate = ledger::object(gate, "ledger.ignored_rust_tests[]");
        let gate_id = ledger::required_str(gate, "gate_id", "ignored gate");
        assert!(
            gate_ids.insert(gate_id.to_owned()),
            "duplicate ignored gate id {gate_id}"
        );
        assert!(
            ledger::required_bool(gate, "must_migrate", gate_id),
            "{gate_id} must remain a required migration gate"
        );
        match ledger::required_str(gate, "status", gate_id) {
            "required_not_migrated" => {
                for key in ["replacement_test", "replacement_kind", "gate_evidence"] {
                    assert!(
                        gate.get(key).is_none(),
                        "{gate_id} is required_not_migrated and must not carry {key}"
                    );
                }
                assert!(
                    required.insert(gate_id.to_owned()),
                    "duplicate required gate {gate_id}"
                );
            }
            "gate_green" => {
                assert_gate_green_evidence(gate, gate_id);
                assert!(
                    green.insert(gate_id.to_owned()),
                    "duplicate gate_green gate {gate_id}"
                );
            }
            other => panic!(
                "{gate_id} has unexpected status {other:?}; P6-2 only promotes the \
                 twenty-three evidence-backed native gates"
            ),
        }
        assert_required_text(
            gate,
            gate_id,
            &[
                "old_reason",
                "target_gate",
                "proposed_command",
                "fail_closed",
            ],
        );
        assert_related_cases_exist(gate, gate_id, &case_ids);

        let file = ledger::required_str(gate, "file", gate_id);
        let test = ledger::required_str(gate, "test", gate_id);
        assert!(
            expected.insert((file.to_owned(), test.to_owned())),
            "duplicate ignored test {file}::{test}"
        );
    }

    let frozen_green: BTreeSet<String> = GATE_GREEN_IDS
        .iter()
        .map(|gate_id| (*gate_id).to_owned())
        .collect();
    assert_eq!(
        green, frozen_green,
        "only the evidence-backed native gates may be gate_green"
    );
    assert_eq!(
        green.len(),
        GATE_GREEN_COUNT,
        "the gate_green count drifted from the routing/guard promotion"
    );
    assert_eq!(
        required.len(),
        REQUIRED_NOT_MIGRATED_COUNT,
        "the remaining gates must stay required_not_migrated"
    );
    let frozen_required: BTreeSet<String> = REQUIRED_NOT_MIGRATED_IDS
        .iter()
        .map(|gate_id| (*gate_id).to_owned())
        .collect();
    assert_eq!(
        frozen_required.len(),
        REQUIRED_NOT_MIGRATED_COUNT,
        "the frozen required gate list has duplicates"
    );
    assert_eq!(
        required, frozen_required,
        "only the frozen required gate may stay required_not_migrated"
    );
    assert_eq!(green.len() + required.len(), IGNORED_GATE_COUNT);

    let (count, mut actual) = ignored_functions();
    let native = declared_native_gate_tests();
    assert_eq!(
        native.len(),
        IGNORED_GATE_TESTS.len(),
        "the declared native gate list has duplicates"
    );
    assert_eq!(
        count,
        gates.len() + native.len(),
        "found {count} #[ignore] lines but the ledger records {} old gates plus {} native gate tests",
        gates.len(),
        native.len()
    );
    for (file, function) in &native {
        assert!(
            actual.remove(&(file.clone(), function.clone())),
            "declared native gate {file}::{function} must be an #[ignore] test"
        );
    }
    assert_eq!(
        actual, expected,
        "the #[ignore] test set drifted from the ledger; update the ledger \
         deliberately and never drop a gate silently"
    );
    assert_eq!(gate_ids.len(), IGNORED_GATE_COUNT);
}

#[test]
fn native_replacements_are_linked_with_gate_green_evidence() {
    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let gates = ledger::required_array(root, "ignored_rust_tests", "ledger root");
    let mut by_id = std::collections::BTreeMap::new();
    for gate in gates {
        let gate = ledger::object(gate, "ledger.ignored_rust_tests[]");
        let gate_id = ledger::required_str(gate, "gate_id", "ignored gate");
        by_id.insert(gate_id.to_owned(), gate);
    }

    let replacements: BTreeSet<&str> = IGNORED_GATE_TESTS
        .iter()
        .map(|(_, function)| *function)
        .collect();
    let mut linked: BTreeSet<&str> = BTreeSet::new();
    for (gate_id, replacement) in GATE_REPLACEMENTS {
        let gate = by_id
            .get(*gate_id)
            .unwrap_or_else(|| panic!("ledger is missing the promoted gate {gate_id}"));
        assert_eq!(
            ledger::required_str(gate, "status", gate_id),
            "gate_green",
            "{gate_id} must be gate_green only with the recorded opt-in native evidence"
        );
        assert_eq!(
            ledger::required_str(gate, "replacement_test", gate_id),
            format!("{NATIVE_GATE_FILE}::{replacement}"),
            "{gate_id} replacement_test must name the linked native gate"
        );
        assert!(
            replacements.contains(*replacement),
            "{gate_id} replacement {replacement} is not an ignored native gate"
        );
        assert!(
            linked.insert(*replacement),
            "{replacement} is linked to more than one ledger gate"
        );
    }
    assert_eq!(GATE_REPLACEMENTS.len(), GATE_GREEN_COUNT);
    assert_eq!(
        linked, replacements,
        "every promoted native replacement must link to exactly one ledger gate"
    );
}

#[test]
fn native_gate_tests_are_declared_ignored_and_fail_closed() {
    let workspace = ledger::workspace_root();
    let source = read_source(&workspace.join(NATIVE_GATE_FILE));
    let lines: Vec<&str> = source.lines().collect();
    let mut declared = BTreeSet::new();
    let mut ignored = BTreeSet::new();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test]") {
            let name = lines
                .iter()
                .skip(index + 1)
                .take(16)
                .find_map(|line| source_scan::parse_fn_name(line))
                .unwrap_or_else(|| panic!("{NATIVE_GATE_FILE} has a #[test] without a fn"));
            assert!(
                declared.insert(name),
                "{NATIVE_GATE_FILE} declares a duplicate #[test]"
            );
        }
        if trimmed.starts_with("#[ignore") {
            let (function_index, name) = lines
                .iter()
                .enumerate()
                .skip(index + 1)
                .take(16)
                .find_map(|(function_index, line)| {
                    source_scan::parse_fn_name(line).map(|name| (function_index, name))
                })
                .unwrap_or_else(|| panic!("{NATIVE_GATE_FILE} has an #[ignore] without a fn"));
            assert!(
                source_scan::attribute_block_contains(&lines, function_index, "#[ignore"),
                "{NATIVE_GATE_FILE}::{name} must keep #[ignore] in its attribute block"
            );
            ignored.insert(name);
        }
    }

    let expected: BTreeSet<String> = IGNORED_GATE_TESTS
        .iter()
        .map(|(_, function)| (*function).to_owned())
        .collect();
    assert_eq!(declared, expected, "the native gate test set is frozen");
    assert_eq!(
        ignored, expected,
        "every native gate must stay #[ignore] until the native job is required"
    );

    for file in [
        NATIVE_GATE_FILE,
        "crates/wxml-lsp/tests/native_parity/native_gate_env.rs",
        "crates/wxml-lsp/tests/native_parity/native_lsp.rs",
        "crates/wxml-lsp/tests/native_parity/native_package.rs",
    ] {
        let source = read_source(&workspace.join(file));
        for forbidden in [
            "WXML_TEST_NODE",
            "dist/server.js",
            "Command::new(\"node\")",
            "tsc.exists()",
            "skipping real Node LSP test",
        ] {
            assert!(
                !source.contains(forbidden),
                "{file} must not contain the superseded Node path {forbidden:?}"
            );
        }
    }

    for name in &expected {
        let line = lines
            .iter()
            .position(|line| line.contains(&format!("fn {name}(")))
            .unwrap_or_else(|| panic!("{NATIVE_GATE_FILE} does not declare {name}"))
            + 1;
        let body = source_scan::function_body(&source, line)
            .unwrap_or_else(|| panic!("{NATIVE_GATE_FILE}::{name} body is missing"));
        let request = body.find("NativeGate::require(").unwrap_or_else(|| {
            panic!("{NATIVE_GATE_FILE}::{name} must call NativeGate::require()")
        });
        for forbidden in ["write_page_fixture(", "gate.write(", "Session::start("] {
            if let Some(index) = body.find(forbidden) {
                assert!(
                    request < index,
                    "{NATIVE_GATE_FILE}::{name} must provision the native package before `{forbidden}`"
                );
            }
        }
    }
}

#[test]
fn semantic_tools_silent_returns_are_mandatory_unmigrated_gates() {
    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let gates = ledger::required_array(root, "silent_skip_gates", "ledger root");
    assert_eq!(
        gates.len(),
        2,
        "the ledger must keep both semantic_tools early returns"
    );

    let workspace = ledger::workspace_root();
    let cases = ledger::required_array(root, "cases", "ledger root");
    let case_ids = ledger::case_ids(cases);
    let mut replacements = BTreeSet::new();

    for gate in gates {
        let gate = ledger::object(gate, "ledger.silent_skip_gates[]");
        let gate_id = ledger::required_str(gate, "gate_id", "silent gate");
        assert!(
            ledger::required_bool(gate, "must_migrate", gate_id),
            "{gate_id} must remain a required migration gate"
        );
        assert!(
            ledger::required_bool(gate, "old_test_must_fail_when_engine_missing", gate_id),
            "{gate_id} must fail when the native engine is missing"
        );
        assert_eq!(
            ledger::required_str(gate, "status", gate_id),
            "required_not_migrated",
            "{gate_id} must not be marked migrated"
        );
        assert_required_text(
            gate,
            gate_id,
            &[
                "old_reason",
                "target_gate",
                "proposed_command",
                "fail_closed",
                "required_old_test_change",
            ],
        );
        assert_related_cases_exist(gate, gate_id, &case_ids);

        let file = ledger::required_str(gate, "file", gate_id);
        let test = ledger::required_str(gate, "test", gate_id);
        let line = usize::try_from(ledger::required_u64(gate, "line", gate_id))
            .expect("silent gate line fits usize");
        assert!(line > 0, "{gate_id} has line 0");
        let source_path = workspace.join(file);
        let source = read_source(&source_path);
        let lines: Vec<&str> = source.lines().collect();
        let declaration = lines
            .get(line - 1)
            .unwrap_or_else(|| panic!("{gate_id} points past the end of {file}:{line}"));
        assert!(
            declaration.contains(&format!("fn {test}(")),
            "{file}:{line} must be the {test} declaration: {declaration:?}"
        );
        assert!(
            !source_scan::attribute_block_contains(&lines, line - 1, "#[ignore"),
            "{file}::{test} must not be #[ignore] while the ledger records a silent return"
        );

        let body = source_scan::function_body(&source, line)
            .unwrap_or_else(|| panic!("{gate_id} function body was not found at {file}:{line}"));
        let else_block = source_scan::else_block_after(body, "real_node_paths()")
            .unwrap_or_else(|| panic!("{gate_id} has no real_node_paths() else block"));
        let normalized = normalize_whitespace(else_block);
        assert!(
            normalized.contains("return;"),
            "{file}::{test} must return early when the old assets are absent"
        );
        assert!(
            normalized.contains("eprintln!"),
            "{file}::{test} must report the silent skip"
        );
        assert!(
            normalized.contains("skipping real Node LSP test"),
            "{file}::{test} no longer records the silent-return message"
        );

        let replacement = ledger::required_str(gate, "replacement_test", gate_id);
        let replacement = native_parity_function(replacement, gate_id);
        assert_blocked_native_gate(&replacement, gate_id);
        assert!(
            replacements.insert(replacement),
            "{gate_id} reuses a replacement test"
        );

        if let Some(tsc) = gate.get("tsc_replacement").and_then(|value| value.as_str()) {
            let tsc = native_parity_function(tsc, gate_id);
            assert_blocked_native_gate(&tsc, gate_id);
            assert!(
                replacements.insert(tsc),
                "{gate_id} reuses the tsc replacement test"
            );
        }
    }
    assert_eq!(replacements.len(), 3);
    for gate in crate::symbols_edits::BLOCKED_NATIVE_GATES {
        assert!(
            !gate.reason.is_empty(),
            "{} must document why the native gate is blocked",
            gate.test_id
        );
    }
    assert!(
        crate::symbols_edits::MIGRATED_CASES.is_empty(),
        "symbols_edits must not claim a migrated case in P3-T"
    );

    let silent = ledger::required_object(root, "silent_skips", "ledger root");
    let rust_entries = ledger::required_array(silent, "rust_silent_skips", "ledger.silent_skips");
    assert_eq!(
        rust_entries.len(),
        3,
        "the ledger must keep two early returns plus the optional tsc branch"
    );
    for entry in rust_entries {
        let entry = ledger::object(entry, "ledger.silent_skips.rust_silent_skips[]");
        let line = ledger::required_u64(entry, "line", "rust silent skip");
        assert!(line > 0);
        assert!(
            ledger::required_bool(
                entry,
                "old_test_must_fail_when_engine_missing",
                "rust silent skip"
            ),
            "every recorded silent skip must fail when the engine is missing"
        );
        for key in [
            "file",
            "test",
            "behavior",
            "baseline_result",
            "replacement_test",
            "replacement_kind",
            "required_old_test_change",
            "required_gate",
            "target_rust_landing",
        ] {
            assert!(!ledger::required_str(entry, key, "rust silent skip").is_empty());
        }
    }

    let tsc_entry = rust_entries
        .iter()
        .map(|entry| ledger::object(entry, "rust silent skip"))
        .find(|entry| ledger::required_str(entry, "behavior", "rust silent skip").contains("tsc"))
        .expect("the ledger must record the optional tsc branch");
    let tsc_file = ledger::required_str(tsc_entry, "file", "tsc silent skip");
    let tsc_line = usize::try_from(ledger::required_u64(tsc_entry, "line", "tsc silent skip"))
        .expect("tsc line fits usize");
    assert!(tsc_line > 0);
    let tsc_source = read_source(&workspace.join(tsc_file));
    let tsc_lines: Vec<&str> = tsc_source.lines().collect();
    let tsc_declaration = tsc_lines.get(tsc_line - 1).expect("tsc line must exist");
    assert!(
        tsc_declaration.trim_start().starts_with("if tsc.exists()"),
        "the tsc ledger line moved: {tsc_declaration:?}"
    );
    let (_, tsc_else) = source_scan::if_else_blocks(&tsc_source, tsc_line)
        .expect("the tsc if/else structure must parse");
    let normalized_tsc = normalize_whitespace(tsc_else);
    assert!(
        normalized_tsc.contains("eprintln!"),
        "the tsc branch must still report the optional skip"
    );
    assert!(
        normalized_tsc.contains("tsc not installed; skipped post-edit type check"),
        "the optional tsc skip branch must stay visible until the native helper replaces it"
    );
    assert!(
        !normalized_tsc.contains("panic!"),
        "the optional tsc branch must not become a hidden pass"
    );

    let tsc_replacement = ledger::required_str(tsc_entry, "replacement_test", "tsc silent skip");
    let tsc_replacement = native_parity_function(tsc_replacement, "tsc silent skip");
    assert_blocked_native_gate(&tsc_replacement, "tsc silent skip");
}
