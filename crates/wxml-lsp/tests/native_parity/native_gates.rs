//! Mandatory future gates for ignored tests and silent returns.
//!
//! P3-T does not migrate these gates. It freezes their inventory and proves
//! that the old Rust surface still contains exactly the 23 `#[ignore]` tests
//! and the two `semantic_tools` early returns recorded by the ledger. The
//! 23 gates and the two silent returns must become real native commands before
//! the old Node-backed tests may be deleted or treated as green.

use std::collections::BTreeSet;
use std::path::Path;

use crate::fixture_hash::collect_tree_files;
use crate::ledger;
use crate::source_scan;

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
fn all_ignored_rust_tests_are_mandatory_unmigrated_gates() {
    let ledger_value = ledger::load_ledger();
    let root = ledger::object(&ledger_value, "ledger root");
    let gates = ledger::required_array(root, "ignored_rust_tests", "ledger root");
    assert_eq!(gates.len(), 23, "the frozen ignored-test inventory drifted");

    let cases = ledger::required_array(root, "cases", "ledger root");
    let case_ids = ledger::case_ids(cases);
    let mut gate_ids = BTreeSet::new();
    let mut expected = BTreeSet::new();

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

    let (count, actual) = ignored_functions();
    assert_eq!(
        count,
        gates.len(),
        "found {count} #[ignore] lines but the ledger records {} gates",
        gates.len()
    );
    assert_eq!(
        actual, expected,
        "the #[ignore] test set drifted from the ledger; update the ledger \
         deliberately and never drop a gate silently"
    );
    assert_eq!(gate_ids.len(), 23);
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
