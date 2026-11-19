//! Opt-in native replacements for the mandatory P5 gates.
//!
//! The 2026-11-19 typed-asset slice made real hover and diagnostics available
//! through `--backend native`. These tests are the native replacements for:
//!
//! - `G-P5-E2E-04` / `real_node_hover_and_diagnostics_smoke`
//! - `G-P5-E2E-06` / `ac001_page_data_and_jsdoc_hover`
//! - `G-P5-E2E-07` / `ac002_field_typo_diagnostic_and_fix_clear`
//! - `G-P5-E2E-08` / `ac003_unicode_and_crlf_positions`
//! - `G-P5-E2E-09` / `ac011_unknown_owner_degrades_honestly`
//! - `G-P5-E2E-10` / `ac013_component_properties_hover`
//! - `G-P5-E2E-12` / `static_wxs_and_template_semantics_work_over_stdio`
//! - `G-P5-E2E-13` / `static_wxs_answers_survive_an_unavailable_type_backend`
//! - `G-P5-E2E-14` / `unsaved_wxs_overlay_updates_static_hover_and_completion`
//! - `G-P5-E2E-15` / `ac005_loop_scope_typed_locals`
//! - `G-P5-E2E-17` / `ac008_alias_project_hover_and_definition`
//! - `G-P5-E2E-18` / `ac008_multi_view_fixture_isolation`
//! - `G-P5-E2E-19` / `ac008_simultaneous_views_do_not_pollute_each_other`
//!
//! Every test is `#[ignore]`: the default workspace job reports it ignored,
//! never passed. The required native job runs them with `--ignored` and
//! `WXML_REQUIRE_REAL_ENGINE=1`; a missing package/engine is then fatal.
//! Without `WXML_REQUIRE_REAL_ENGINE` a missing package is an explicit,
//! printed optional-mode skip. Type-behavior gates additionally require
//! `TSGO_TYPINGS_ROOT` so the disposable package carries the pinned
//! miniprogram typings; a green run without them is not claimed.

use serde_json::{Value, json};

use crate::native_lsp::{self, Session};
use crate::native_package::NativeGate;

/// Workspace-relative file that declares the ignored native gates.
pub(crate) const NATIVE_GATE_FILE: &str =
    "crates/wxml-lsp/tests/native_parity/native_engine_real.rs";

/// Ledger gate id -> native replacement test.
///
/// All thirteen are `gate_green` in the ledger with recorded opt-in evidence;
/// this map keeps the exact gate/replacement link checked by `native_gates.rs`.
pub(crate) const GATE_REPLACEMENTS: &[(&str, &str)] = &[
    ("G-P5-E2E-04", "native_hover_and_diagnostics_smoke"),
    ("G-P5-E2E-06", "native_ac001_page_data_and_jsdoc_hover"),
    (
        "G-P5-E2E-07",
        "native_ac002_field_typo_diagnostic_and_fix_clear",
    ),
    ("G-P5-E2E-08", "native_ac003_unicode_and_crlf_positions"),
    (
        "G-P5-E2E-09",
        "native_ac011_unknown_owner_degrades_honestly",
    ),
    ("G-P5-E2E-10", "native_ac013_component_properties_hover"),
    (
        "G-P5-E2E-12",
        "native_static_wxs_and_template_semantics_work_over_stdio",
    ),
    (
        "G-P5-E2E-13",
        "native_static_wxs_answers_survive_an_unavailable_type_backend",
    ),
    (
        "G-P5-E2E-14",
        "native_unsaved_wxs_overlay_updates_static_hover_and_completion",
    ),
    ("G-P5-E2E-15", "native_ac005_loop_scope_typed_locals"),
    (
        "G-P5-E2E-17",
        "native_ac008_alias_project_hover_and_definition",
    ),
    ("G-P5-E2E-18", "native_ac008_multi_view_fixture_isolation"),
    (
        "G-P5-E2E-19",
        "native_ac008_simultaneous_views_do_not_pollute_each_other",
    ),
];

/// The exact ignored-native-gate set.
///
/// `ignored_functions()` in `native_gates.rs` subtracts this list from the
/// scanned `#[ignore]` set and still requires the remainder to equal the
/// ledger's 23 old gates exactly.
pub(crate) const IGNORED_GATE_TESTS: &[(&str, &str)] = &[
    (NATIVE_GATE_FILE, "native_hover_and_diagnostics_smoke"),
    (NATIVE_GATE_FILE, "native_ac001_page_data_and_jsdoc_hover"),
    (
        NATIVE_GATE_FILE,
        "native_ac002_field_typo_diagnostic_and_fix_clear",
    ),
    (NATIVE_GATE_FILE, "native_ac003_unicode_and_crlf_positions"),
    (
        NATIVE_GATE_FILE,
        "native_ac011_unknown_owner_degrades_honestly",
    ),
    (NATIVE_GATE_FILE, "native_ac013_component_properties_hover"),
    (
        NATIVE_GATE_FILE,
        "native_static_wxs_and_template_semantics_work_over_stdio",
    ),
    (
        NATIVE_GATE_FILE,
        "native_static_wxs_answers_survive_an_unavailable_type_backend",
    ),
    (
        NATIVE_GATE_FILE,
        "native_unsaved_wxs_overlay_updates_static_hover_and_completion",
    ),
    (NATIVE_GATE_FILE, "native_ac005_loop_scope_typed_locals"),
    (
        NATIVE_GATE_FILE,
        "native_ac008_alias_project_hover_and_definition",
    ),
    (
        NATIVE_GATE_FILE,
        "native_ac008_multi_view_fixture_isolation",
    ),
    (
        NATIVE_GATE_FILE,
        "native_ac008_simultaneous_views_do_not_pollute_each_other",
    ),
];

fn write_page_fixture(gate: &NativeGate, wxml: &str) {
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    gate.write("pages/index/index.wxml", wxml);
    gate.write(
        "pages/index/index.ts",
        r"
Page({
  data: {
    user: {
      /** 用户显示名 */
      name: 'Ada'
    },
    /** 页面标题 */
    title: 'Hello'
  }
});
",
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
}

fn completion_labels(response: &Value) -> Vec<&str> {
    response["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("completion without items: {response}"))
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect()
}

/// G-P5-E2E-09: an ownerless template degrades honestly on the native backend.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac011_unknown_owner_degrades_honestly() {
    let Some(gate) = NativeGate::require("G-P5-E2E-09") else {
        return;
    };
    write_page_fixture(&gate, "{{ user.name }}");
    let orphan = "{{ unknown.value }}";
    gate.write("orphan.wxml", orphan);

    let mut session = Session::start(&gate);
    let uri = gate.uri("orphan.wxml");
    let diagnostics = session.open_wxml(&uri, orphan, 1);
    eprintln!("AC-011 orphan diagnostics: {diagnostics}");

    let warnings: Vec<&Value> = diagnostics
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is"))
        })
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "an unknown owner must publish exactly one degradation warning: {diagnostics}"
    );
    let warning = warnings[0]["message"].as_str().expect("warning message");
    assert!(
        warning.contains("unsupported") || warning.contains("unavailable"),
        "the unknown-owner warning must name the degradation: {warning}"
    );

    let hover = session.hover(&uri, native_lsp::lsp_position(orphan, "value"));
    eprintln!("AC-011 hover: {hover}");
    assert!(
        native_lsp::hover_is_empty(&hover),
        "an unknown owner must not fabricate a hover type: {}",
        session.describe(&hover)
    );
    assert!(
        diagnostics
            .as_array()
            .expect("diagnostics array")
            .iter()
            .all(|diagnostic| diagnostic["severity"] != json!(1)),
        "an unknown owner must not fabricate a type diagnostic: {diagnostics}"
    );

    session.assert_no_node_exec();
    session.assert_no_engine_exec(&gate);
    // Check while the session is alive: an open native session holds its
    // sandbox, while shutdown would remove every trace of it.
    gate.assert_no_native_sandbox();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-12: static WXS/template hover, definition and completion.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_static_wxs_and_template_semantics_work_over_stdio() {
    let Some(gate) = NativeGate::require("G-P5-E2E-12") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    let wxml_text = r#"<wxs module="utils">/** Formats a value. */
function format(value) { return "x"; }
module.exports = { format };</wxs>
<template name="card"><view>{{ title }} {{ count }}</view></template>
<import src="./shared.wxml" />
<view>{{ utils.format(pageTitle) }}</view>
<template is="card" data="{{ { title: pageTitle } }}" />
<template is="shared" />"#;
    gate.write("pages/index/index.wxml", wxml_text);
    gate.write(
        "pages/index/index.ts",
        "Page({ data: { pageTitle: 'x' } });",
    );
    gate.write(
        "pages/index/shared.wxml",
        r#"<template name="shared"><view>{{ sharedTitle }}</view></template>"#,
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    session.open_wxml(&uri, wxml_text, 1);

    let format = native_lsp::lsp_position(wxml_text, "format(pageTitle)");
    let hover = session.hover(&uri, (format.0, format.1 + 1));
    let hover_text = native_lsp::hover_markdown("G-P5-E2E-12", "WXS function hover", &hover);
    assert!(
        hover_text.contains("utils.format"),
        "{}",
        session.describe(&hover)
    );
    assert!(
        hover_text.contains("=> string"),
        "{}",
        session.describe(&hover)
    );
    assert!(
        hover_text.contains("Formats a value."),
        "{}",
        session.describe(&hover)
    );

    let definition = session.definition(&uri, (format.0, format.1 + 1));
    let locations =
        native_lsp::definition_locations("G-P5-E2E-12", "WXS function definition", &definition);
    assert_eq!(locations[0]["uri"], json!(uri), "{definition}");
    assert_eq!(
        locations[0]["range"]["start"]["line"],
        json!(1),
        "{definition}"
    );

    let card = native_lsp::lsp_position(wxml_text, "is=\"card\"");
    let template_hover = session.hover(&uri, (card.0, card.1 + 4));
    let template_text =
        native_lsp::hover_markdown("G-P5-E2E-12", "template hover", &template_hover);
    assert!(
        template_text.contains("template `card`"),
        "{}",
        session.describe(&template_hover)
    );

    let shared = native_lsp::lsp_position(wxml_text, "is=\"shared\"");
    let shared_definition = session.definition(&uri, (shared.0, shared.1 + 4));
    let shared_locations = native_lsp::definition_locations(
        "G-P5-E2E-12",
        "imported template definition",
        &shared_definition,
    );
    assert_eq!(
        shared_locations[0]["uri"],
        json!(gate.uri("pages/index/shared.wxml")),
        "{shared_definition}"
    );

    let incomplete_is = wxml_text.replace("is=\"shared\"", "is=\"sh\"");
    session.change_wxml(&uri, &incomplete_is, 2);
    let is_position = native_lsp::lsp_position(&incomplete_is, "\"sh\"");
    let completion = session.completion(&uri, (is_position.0, is_position.1 + 3));
    assert!(
        completion_labels(&completion).contains(&"shared"),
        "{}",
        session.describe(&completion)
    );

    let incomplete_member = wxml_text.replace("utils.format(pageTitle)", "utils.");
    session.change_wxml(&uri, &incomplete_member, 3);
    let member_position = native_lsp::lsp_position(&incomplete_member, "utils.");
    let member_completion = session.completion(&uri, (member_position.0, member_position.1 + 6));
    assert!(
        completion_labels(&member_completion).contains(&"format"),
        "{}",
        session.describe(&member_completion)
    );

    let incomplete_data = wxml_text.replace("{ title: pageTitle }", "{ ti");
    session.change_wxml(&uri, &incomplete_data, 4);
    let data_position = native_lsp::lsp_position(&incomplete_data, "data=\"{{ { ti");
    let data_completion = session.completion(&uri, (data_position.0, data_position.1 + 13));
    assert!(
        completion_labels(&data_completion).contains(&"title"),
        "{}",
        session.describe(&data_completion)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-14: an unsaved WXS overlay replaces disk facts and restores on close.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_unsaved_wxs_overlay_updates_static_hover_and_completion() {
    let Some(gate) = NativeGate::require("G-P5-E2E-14") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    let wxml_text = r#"<wxs module="u" src="./u.wxs" />
<view>{{ u.label }}</view>"#;
    gate.write("pages/index/index.wxml", wxml_text);
    gate.write("pages/index/index.ts", "Page({ data: {} });");
    gate.write("pages/index/u.wxs", "module.exports = { label: 'disk' };");
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    let disk_wxs = native_lsp::read_project_file(&gate, "pages/index/u.wxs");

    let mut session = Session::start(&gate);
    let wxml = gate.uri("pages/index/index.wxml");
    let wxs = gate.uri("pages/index/u.wxs");
    session.open_wxml(&wxml, wxml_text, 1);

    let label = native_lsp::lsp_position(wxml_text, "label");
    let disk_hover = session.hover(&wxml, (label.0, label.1 + 1));
    let disk_text = native_lsp::hover_markdown("G-P5-E2E-14", "disk WXS hover", &disk_hover);
    assert!(
        disk_text.contains("string"),
        "{}",
        session.describe(&disk_hover)
    );

    session.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": wxs,
                "languageId": "javascript",
                "version": 1,
                "text": "module.exports = { label: 456, renamed: 1 };",
            }
        }),
    );
    let renamed_hover = session.hover(&wxml, (label.0, label.1 + 1));
    let renamed_text =
        native_lsp::hover_markdown("G-P5-E2E-14", "overlay WXS hover", &renamed_hover);
    assert!(
        renamed_text.contains("number"),
        "{}",
        session.describe(&renamed_hover)
    );

    let completion = session.completion(&wxml, (label.0, label.1));
    assert!(
        completion_labels(&completion).contains(&"renamed"),
        "{}",
        session.describe(&completion)
    );

    session.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": wxs}}),
    );
    let restored = session.hover(&wxml, (label.0, label.1 + 1));
    let restored_text = native_lsp::hover_markdown("G-P5-E2E-14", "restored WXS hover", &restored);
    assert!(
        restored_text.contains("string"),
        "{}",
        session.describe(&restored)
    );
    assert!(
        restored_text.contains("label"),
        "{}",
        session.describe(&restored)
    );

    assert_eq!(
        disk_wxs,
        native_lsp::read_project_file(&gate, "pages/index/u.wxs"),
        "an unsaved WXS overlay must never write through to disk"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// Fails when a diagnostics payload carries a "type checking is ..." status.
fn assert_no_status_diagnostic(diagnostics: &Value, context: &str) {
    let values = diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("{context}: diagnostics are not an array: {diagnostics}"));
    assert!(
        values.iter().all(|diagnostic| {
            diagnostic["severity"] != json!(2)
                || !diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is"))
        }),
        "{context}: an unexpected status diagnostic was published: {diagnostics}"
    );
}

/// Asserts one error diagnostic spans exactly `target` after `marker`.
fn assert_error_at(diagnostics: &Value, text: &str, marker: &str, target: &str, context: &str) {
    let marker_at = native_lsp::lsp_position(text, marker);
    let target_at = native_lsp::lsp_position(marker, target);
    let start = (marker_at.0 + target_at.0, marker_at.1 + target_at.1);
    let end = (
        start.0,
        start.1 + u32::try_from(target.len()).expect("target length fits"),
    );
    let values = diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("{context}: diagnostics are not an array: {diagnostics}"));
    assert!(
        values.iter().any(|diagnostic| {
            diagnostic["severity"] == json!(1)
                && diagnostic["range"]["start"]["line"] == json!(start.0)
                && diagnostic["range"]["start"]["character"] == json!(start.1)
                && diagnostic["range"]["end"]["line"] == json!(end.0)
                && diagnostic["range"]["end"]["character"] == json!(end.1)
        }),
        "{context}: no error at {}..{} for {target:?}: {diagnostics}",
        start.1,
        end.1
    );
}

/// Hovers the last character of `target` inside `marker`.
fn hover_target_end(
    session: &mut Session,
    uri: &str,
    text: &str,
    marker: &str,
    target: &str,
) -> Value {
    let marker_at = native_lsp::lsp_position(text, marker);
    let target_at = native_lsp::lsp_position(marker, target);
    let last = u32::try_from(target.len().saturating_sub(1)).expect("target fits");
    session.hover(
        uri,
        (marker_at.0 + target_at.0, marker_at.1 + target_at.1 + last),
    )
}

/// Copies one repository fixture into the disposable gate project.
fn copy_fixture(gate: &NativeGate, fixture: &str) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures")
        .join(fixture);
    copy_fixture_tree(&root, &root, gate);
}

fn copy_fixture_tree(root: &std::path::Path, path: &std::path::Path, gate: &NativeGate) {
    for entry in std::fs::read_dir(path).expect("fixture directory") {
        let entry = entry.expect("fixture entry");
        let path = entry.path();
        if path.is_dir() {
            copy_fixture_tree(root, &path, gate);
        } else {
            let relative = path.strip_prefix(root).expect("fixture relative");
            let text = std::fs::read_to_string(&path).expect("fixture text");
            gate.write(&relative.to_string_lossy(), &text);
        }
    }
}

/// Opens one non-template overlay without waiting for diagnostics.
fn open_overlay(session: &mut Session, uri: &str, language_id: &str, text: &str) {
    session.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        }),
    );
}

/// G-P5-E2E-04: typed hover and diagnostics over the native backend.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_hover_and_diagnostics_smoke() {
    let Some(gate) = NativeGate::require("G-P5-E2E-04") else {
        return;
    };
    let wxml = "{{ user.name }}";
    write_page_fixture(&gate, wxml);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, wxml, 1);
    assert!(
        diagnostics.as_array().is_some_and(Vec::is_empty),
        "a clean page must publish no diagnostics: {diagnostics}"
    );

    let name = native_lsp::lsp_position(wxml, "name");
    let hover = session.hover(&uri, (name.0, name.1 + 2));
    let text = native_lsp::hover_markdown("G-P5-E2E-04", "typed hover", &hover);
    assert!(
        text.contains("string"),
        "user.name must be string: {}",
        session.describe(&hover)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-06: page data hover with `JSDoc` and exact ranges.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "keeps the frozen AC-001 assertions together"
)]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac001_page_data_and_jsdoc_hover() {
    let Some(gate) = NativeGate::require("G-P5-E2E-06") else {
        return;
    };
    let wxml = "{{ user.name }} {{ title }}";
    write_page_fixture(&gate, wxml);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, wxml, 1);
    assert_eq!(
        diagnostics,
        json!([]),
        "a valid page data binding must publish no diagnostics: {diagnostics}"
    );

    let name_position = native_lsp::lsp_position(wxml, "name");
    let name = session.hover(&uri, name_position);
    let name_value = native_lsp::hover_markdown("G-P5-E2E-06", "user.name hover", &name);
    assert!(
        name_value.contains("string"),
        "user.name must be string: {}",
        session.describe(&name)
    );
    assert!(
        name_value.contains("用户显示名"),
        "user.name hover must include the JSDoc text: {}",
        session.describe(&name)
    );
    assert_eq!(
        name["result"]["range"]["start"],
        json!({"line": name_position.0, "character": name_position.1}),
        "hover range must point at `name`: {name}"
    );
    assert_eq!(
        name["result"]["range"]["end"],
        json!({"line": name_position.0, "character": name_position.1 + 4}),
        "hover range must point at `name`: {name}"
    );

    let user_position = native_lsp::lsp_position(wxml, "user");
    let user = session.hover(&uri, user_position);
    let user_value = native_lsp::hover_markdown("G-P5-E2E-06", "user hover", &user);
    assert!(
        user_value.contains("name"),
        "the user hover should expose the object shape: {}",
        session.describe(&user)
    );
    assert!(
        !user_value.contains("用户显示名"),
        "hovering user must not show the name field's JSDoc: {}",
        session.describe(&user)
    );
    assert_eq!(
        user["result"]["range"]["start"],
        json!({"line": user_position.0, "character": user_position.1}),
        "hover range must point at `user`: {user}"
    );
    assert_eq!(
        user["result"]["range"]["end"],
        json!({"line": user_position.0, "character": user_position.1 + 4}),
        "hover range must point at `user`: {user}"
    );

    let expression_end = (user_position.0, user_position.1 + 9);
    let end = session.hover(&uri, expression_end);
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-06", "expression end", &end).contains("string"),
        "the expression end is a valid hover point: {}",
        session.describe(&end)
    );

    let outside = session.hover(&uri, (0, 0));
    assert!(
        native_lsp::hover_is_empty(&outside),
        "plain markup outside a mustache must not produce a tooltip: {}",
        session.describe(&outside)
    );

    let title_position = native_lsp::lsp_position(wxml, "title");
    let title = session.hover(&uri, title_position);
    let title_value = native_lsp::hover_markdown("G-P5-E2E-06", "title hover", &title);
    assert!(
        title_value.contains("string"),
        "title must be string: {}",
        session.describe(&title)
    );
    assert_eq!(
        title["result"]["range"]["start"],
        json!({"line": title_position.0, "character": title_position.1}),
        "hover range must point at `title`: {title}"
    );
    assert_eq!(
        title["result"]["range"]["end"],
        json!({"line": title_position.0, "character": title_position.1 + 5}),
        "hover range must point at `title`: {title}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-07: one exact typo diagnostic, cleared by `didChange`.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac002_field_typo_diagnostic_and_fix_clear() {
    let Some(gate) = NativeGate::require("G-P5-E2E-07") else {
        return;
    };
    let broken = "{{ user.nmae }}";
    write_page_fixture(&gate, broken);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, broken, 1);
    assert_no_status_diagnostic(&diagnostics, "AC-002 typo");
    assert_error_at(&diagnostics, broken, broken, "nmae", "AC-002 typo");
    let values = diagnostics.as_array().expect("diagnostics");
    let errors = values
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "the typo must produce exactly one error diagnostic: {diagnostics}"
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("does not exist")),
        "the typo message must name the missing property: {}",
        errors[0]
    );
    let (line, character) = native_lsp::lsp_position(broken, "nmae");
    assert_eq!(
        errors[0]["range"],
        json!({
            "start": {"line": line, "character": character},
            "end": {"line": line, "character": character + 4},
        }),
        "the only error must sit exactly on `nmae`: {diagnostics}"
    );

    let fixed = "{{ user.name }}";
    let cleared = session.change_wxml(&uri, fixed, 2);
    assert_eq!(
        cleared,
        json!([]),
        "AC-002 diagnostics must clear after didChange: {cleared}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-08: UTF-16 and CRLF diagnostics positions.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac003_unicode_and_crlf_positions() {
    let Some(gate) = NativeGate::require("G-P5-E2E-08") else {
        return;
    };
    let wxml = "标题😀 {{ user.nmae }}\r\n第二行 {{ user.nmae }}\r\n";
    write_page_fixture(&gate, wxml);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, wxml, 1);
    assert_no_status_diagnostic(&diagnostics, "AC-003 unicode");
    let values = diagnostics.as_array().expect("diagnostics");
    let errors = values
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        2,
        "both `nmae` occurrences must be diagnosed: {diagnostics}"
    );

    let first = errors
        .iter()
        .find(|diagnostic| diagnostic["range"]["start"]["line"] == json!(0))
        .unwrap_or_else(|| panic!("no line-0 diagnostic: {diagnostics}"));
    assert_eq!(first["range"]["start"]["character"], json!(13), "{first}");
    assert_eq!(first["range"]["end"]["line"], json!(0), "{first}");
    assert_eq!(first["range"]["end"]["character"], json!(17), "{first}");
    assert!(
        first["message"]
            .as_str()
            .is_some_and(|message| message.contains("nmae")),
        "{first}"
    );

    let second = errors
        .iter()
        .find(|diagnostic| diagnostic["range"]["start"]["line"] == json!(1))
        .unwrap_or_else(|| panic!("no line-1 diagnostic: {diagnostics}"));
    assert_eq!(second["range"]["start"]["character"], json!(12), "{second}");
    assert_eq!(second["range"]["end"]["line"], json!(1), "{second}");
    assert_eq!(second["range"]["end"]["character"], json!(16), "{second}");

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-10: component property hover keeps the honest Partial reason.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac013_component_properties_hover() {
    let Some(gate) = NativeGate::require("G-P5-E2E-10") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{"card":"./components/card/card"}}"#,
    );
    gate.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    gate.write(
        "pages/index/index.wxml",
        r#"<card title="{{ user.name }}" />"#,
    );
    gate.write("components/card/card.wxml", "<view>{{ title }}</view>");
    gate.write(
        "components/card/card.ts",
        r"
Component({
  properties: {
    title: { type: String, value: '' }
  }
});
",
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let uri = gate.uri("components/card/card.wxml");
    let text = "<view>{{ title }}</view>";
    let diagnostics = session.open_wxml(&uri, text, 1);
    let values = diagnostics.as_array().expect("diagnostics");
    assert!(
        values
            .iter()
            .all(|diagnostic| diagnostic["severity"] != json!(1)),
        "a declared component property must not produce a type error: {diagnostics}"
    );
    let status = values
        .iter()
        .find(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is partial"))
        })
        .unwrap_or_else(|| {
            panic!("properties-only component must report its Partial status: {diagnostics}")
        });
    assert!(
        status["message"]
            .as_str()
            .is_some_and(|message| message.contains("registration has no `data` member")),
        "{status}"
    );

    let hover = session.hover(&uri, native_lsp::lsp_position(text, "title"));
    let hover_text = native_lsp::hover_markdown("G-P5-E2E-10", "component property hover", &hover);
    assert!(
        hover_text.contains("string"),
        "component property title must be string: {}",
        session.describe(&hover)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-13: static WXS answers survive an unavailable type backend.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_static_wxs_answers_survive_an_unavailable_type_backend() {
    let Some(gate) = NativeGate::require("G-P5-E2E-13") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    let wxml_text = r#"<wxs module="u">function format(v) { return "x"; }
module.exports = { format };</wxs>
<view>{{ u.format(title) }}</view>
<view>{{ title }}</view>"#;
    gate.write("pages/index/index.wxml", wxml_text);
    gate.write("pages/index/index.ts", "Page({ data: { title: 'x' } });");
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    // The engine is verified during discovery; remove it before the first
    // type query so the started server keeps its static layer but the native
    // type session can no longer be established.
    let engine = gate.package_root().join("engine").join("lib").join("tsc");
    assert!(
        engine.is_file(),
        "the disposable package must contain the pinned engine before launch"
    );
    let mut session = Session::start(&gate);
    std::fs::remove_file(&engine).expect("remove the engine after startup");

    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, wxml_text, 1);
    let status = diagnostics
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is unavailable"))
        })
        .unwrap_or_else(|| panic!("an unavailable type backend must be reported: {diagnostics}"));
    assert!(
        status["message"]
            .as_str()
            .is_some_and(|message| message.contains("unavailable")),
        "{status}"
    );
    assert!(
        diagnostics
            .as_array()
            .expect("diagnostics")
            .iter()
            .all(|diagnostic| diagnostic["severity"] != json!(1)),
        "an unavailable type backend must not fabricate an error: {diagnostics}"
    );

    let format = native_lsp::lsp_position(wxml_text, "format(title)");
    let hover = session.hover(&uri, (format.0, format.1 + 1));
    let hover_text = native_lsp::hover_markdown("G-P5-E2E-13", "static WXS hover", &hover);
    assert!(
        hover_text.contains("utils.format") || hover_text.contains("u.format"),
        "{}",
        session.describe(&hover)
    );
    assert!(
        hover_text.contains("=> string"),
        "{}",
        session.describe(&hover)
    );

    let definition = session.definition(&uri, (format.0, format.1 + 1));
    let locations =
        native_lsp::definition_locations("G-P5-E2E-13", "static WXS definition", &definition);
    assert!(
        !locations.is_empty(),
        "static WXS definition must survive an unavailable type backend: {definition}"
    );

    let typed = native_lsp::lsp_position(wxml_text, "{{ title }}");
    let typed_hover = session.hover(&uri, (typed.0, typed.1 + 3));
    assert!(
        native_lsp::hover_is_empty(&typed_hover),
        "an unavailable type backend must not fabricate a typed hover: {}",
        session.describe(&typed_hover)
    );

    // The failed session start must leave no sandbox behind.
    gate.assert_no_native_sandbox();

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-15: `wx:for` local typing and the one outside-loop error.
#[test]
#[allow(clippy::too_many_lines, reason = "one frozen eight-hover fixture")]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac005_loop_scope_typed_locals() {
    let Some(gate) = NativeGate::require("G-P5-E2E-15") else {
        return;
    };
    copy_fixture(&gate, "loop-scope");
    let text = native_lsp::read_project_file(&gate, "pages/index/index.wxml");

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_no_status_diagnostic(&diagnostics, "AC-005 loop scope");

    for (marker, target, type_text, documentation) in [
        ("外层索引：{{ itemIndex }}", "itemIndex", "number", None),
        (
            "外层组名：{{ item.name }}",
            "item.name",
            "string",
            Some("组名"),
        ),
        (
            "外层标题：{{ item.title }}",
            "item.title",
            "string",
            Some("组标题"),
        ),
        ("内层索引：{{ index }}", "index", "number", None),
        (
            "内层成员：{{ item.name }}",
            "item.name",
            "string",
            Some("成员名称"),
        ),
        (
            "退出内层后标题：{{ item.title }}",
            "item.title",
            "string",
            Some("组标题"),
        ),
        ("对象键：{{ index }}", "index", "string", None),
        (
            "对象值标题：{{ item.title }}",
            "item.title",
            "string",
            Some("组标题"),
        ),
    ] {
        let hover = hover_target_end(&mut session, &uri, &text, marker, target);
        let contents = native_lsp::hover_markdown("G-P5-E2E-15", target, &hover);
        assert!(
            contents.contains(type_text),
            "{target} must be {type_text}: {}",
            session.describe(&hover)
        );
        match documentation {
            Some(documentation) => assert!(
                contents.contains(documentation),
                "{target} must include {documentation:?}: {}",
                session.describe(&hover)
            ),
            None => assert!(
                !contents.contains("\n\n"),
                "{target} must not carry documentation: {}",
                session.describe(&hover)
            ),
        }
    }

    let values = diagnostics.as_array().expect("diagnostics");
    let errors = values
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "the only loop-scope negative is `{{ item }}` outside the loop: {diagnostics}"
    );
    assert_error_at(
        &diagnostics,
        &text,
        "循环外访问：{{ item }}",
        "item",
        "AC-005 outside-loop item",
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("item")),
        "{}",
        errors[0]
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-17: `extends` + `paths` alias hover and definition.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac008_alias_project_hover_and_definition() {
    let Some(gate) = NativeGate::require("G-P5-E2E-17") else {
        return;
    };
    copy_fixture(&gate, "alias-project");
    let text = native_lsp::read_project_file(&gate, "pages/index/index.wxml");

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_no_status_diagnostic(&diagnostics, "AC-008 alias");
    let error_count = diagnostics
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .count();
    assert_eq!(
        error_count, 1,
        "the alias fixture has exactly one type error: {diagnostics}"
    );
    assert_error_at(
        &diagnostics,
        &text,
        "{{ user.nmae }}",
        "nmae",
        "AC-008 alias typo",
    );

    let name = hover_target_end(&mut session, &uri, &text, "{{ user.name }}", "user.name");
    let contents = native_lsp::hover_markdown("G-P5-E2E-17", "alias hover", &name);
    assert!(contents.contains("string"), "{}", session.describe(&name));
    assert!(
        contents.contains("用户显示名称"),
        "{}",
        session.describe(&name)
    );

    let definition = session.definition(&uri, {
        let marker = native_lsp::lsp_position(&text, "{{ user.name }}");
        let target = native_lsp::lsp_position("{{ user.name }}", "user.name");
        (marker.0 + target.0, marker.1 + target.1 + 8)
    });
    let locations =
        native_lsp::definition_locations("G-P5-E2E-17", "alias definition", &definition);
    let location = locations
        .iter()
        .find(|location| location["uri"] == json!(gate.uri("models/user.ts")))
        .unwrap_or_else(|| {
            panic!("`@models/user` definition did not resolve to models/user.ts: {definition}")
        });
    let model_text = native_lsp::read_project_file(&gate, "models/user.ts");
    let line = usize::try_from(
        location["range"]["start"]["line"]
            .as_u64()
            .expect("definition line"),
    )
    .expect("line fits");
    let start = usize::try_from(
        location["range"]["start"]["character"]
            .as_u64()
            .expect("definition start"),
    )
    .expect("start fits");
    let end = usize::try_from(
        location["range"]["end"]["character"]
            .as_u64()
            .expect("definition end"),
    )
    .expect("end fits");
    let covered = model_text
        .lines()
        .nth(line)
        .and_then(|line| line.get(start..end))
        .unwrap_or_else(|| panic!("definition range {line}:{start}..{end} is out of range"));
    assert!(
        covered.contains("name"),
        "definition range must cover the `name` property, got {covered:?}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-18: seven cross-view hovers and one card-local diagnostic.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one frozen seven-hover multi-view fixture"
)]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac008_multi_view_fixture_isolation() {
    let Some(gate) = NativeGate::require("G-P5-E2E-18") else {
        return;
    };
    copy_fixture(&gate, "multi-view");

    let card_text = native_lsp::read_project_file(&gate, "components/card/card.wxml");
    let badge_text = native_lsp::read_project_file(&gate, "components/badge/badge.wxml");
    let a_text = native_lsp::read_project_file(&gate, "pages/a/a.wxml");
    let b_text = native_lsp::read_project_file(&gate, "pages/b/b.wxml");

    let mut session = Session::start(&gate);
    let card_uri = gate.uri("components/card/card.wxml");
    let badge_uri = gate.uri("components/badge/badge.wxml");
    let a_uri = gate.uri("pages/a/a.wxml");
    let b_uri = gate.uri("pages/b/b.wxml");

    let card_diagnostics = session.open_wxml(&card_uri, &card_text, 1);
    session.open_wxml(&badge_uri, &badge_text, 1);
    let a_diagnostics = session.open_wxml(&a_uri, &a_text, 1);
    let b_diagnostics = session.open_wxml(&b_uri, &b_text, 1);

    for (uri, text, marker, target, type_text, documentation) in [
        (
            &card_uri,
            &card_text,
            "卡片标题：{{ title }}",
            "title",
            "string",
            "卡片标题",
        ),
        (
            &card_uri,
            &card_text,
            "卡片用户：{{ user.name }}",
            "user.name",
            "string",
            "用户显示名称",
        ),
        (
            &badge_uri,
            &badge_text,
            "徽标数字：{{ title }}",
            "title",
            "number",
            "徽标数字",
        ),
        (
            &a_uri,
            &a_text,
            "title=\"{{ aTitle }}\"",
            "aTitle",
            "string",
            "A 页面标题",
        ),
        (
            &a_uri,
            &a_text,
            "title=\"{{ aBadge }}\"",
            "aBadge",
            "number",
            "A 页面徽标数",
        ),
        (
            &b_uri,
            &b_text,
            "title=\"{{ bTitle }}\"",
            "bTitle",
            "string",
            "B 页面标题",
        ),
        (
            &b_uri,
            &b_text,
            "title=\"{{ bBadge }}\"",
            "bBadge",
            "number",
            "B 页面徽标数",
        ),
    ] {
        let hover = hover_target_end(&mut session, uri, text, marker, target);
        let contents = native_lsp::hover_markdown("G-P5-E2E-18", target, &hover);
        assert!(
            contents.contains(type_text),
            "{target} must be {type_text}: {}",
            session.describe(&hover)
        );
        assert!(
            contents.contains(documentation),
            "{target} must include {documentation:?}: {}",
            session.describe(&hover)
        );
    }

    let card_errors = card_diagnostics
        .as_array()
        .expect("card diagnostics")
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        card_errors.len(),
        1,
        "the card typo must stay the only card error: {card_diagnostics}"
    );
    assert!(
        card_errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("nmae")),
        "{}",
        card_errors[0]
    );
    for (file, diagnostics) in [
        ("pages/a/a.wxml", &a_diagnostics),
        ("pages/b/b.wxml", &b_diagnostics),
    ] {
        assert!(
            diagnostics.as_array().is_some_and(Vec::is_empty),
            "{file}: a clean page must publish no diagnostics: {diagnostics}"
        );
        assert!(
            diagnostics
                .as_array()
                .expect("diagnostics")
                .iter()
                .all(|diagnostic| !diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("nmae"))),
            "{file}: the card typo leaked into another view: {diagnostics}"
        );
    }

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-19: interleaved views keep their own type and documentation.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one frozen interleaved multi-view fixture"
)]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac008_simultaneous_views_do_not_pollute_each_other() {
    let Some(gate) = NativeGate::require("G-P5-E2E-19") else {
        return;
    };
    copy_fixture(&gate, "multi-view");

    let card_text = native_lsp::read_project_file(&gate, "components/card/card.wxml");
    let a_text = native_lsp::read_project_file(&gate, "pages/a/a.wxml");
    let b_text = native_lsp::read_project_file(&gate, "pages/b/b.wxml");
    let a_script = native_lsp::read_project_file(&gate, "pages/a/a.ts");
    let b_script = native_lsp::read_project_file(&gate, "pages/b/b.ts");

    let mut session = Session::start(&gate);
    let a_uri = gate.uri("pages/a/a.wxml");
    let b_uri = gate.uri("pages/b/b.wxml");
    let card_uri = gate.uri("components/card/card.wxml");
    open_overlay(
        &mut session,
        &gate.uri("pages/a/a.ts"),
        "typescript",
        &a_script,
    );
    open_overlay(
        &mut session,
        &gate.uri("pages/b/b.ts"),
        "typescript",
        &b_script,
    );
    let a_diagnostics = session.open_wxml(&a_uri, &a_text, 1);
    let b_diagnostics = session.open_wxml(&b_uri, &b_text, 1);
    let card_diagnostics = session.open_wxml(&card_uri, &card_text, 1);
    for (file, diagnostics) in [
        ("pages/a/a.wxml", &a_diagnostics),
        ("pages/b/b.wxml", &b_diagnostics),
    ] {
        assert!(
            diagnostics.as_array().is_some_and(Vec::is_empty),
            "{file}: a clean page must publish no diagnostics: {diagnostics}"
        );
    }

    let a_badge = hover_target_end(
        &mut session,
        &a_uri,
        &a_text,
        "title=\"{{ aBadge }}\"",
        "aBadge",
    );
    let card_title = hover_target_end(
        &mut session,
        &card_uri,
        &card_text,
        "卡片标题：{{ title }}",
        "title",
    );
    let b_badge = hover_target_end(
        &mut session,
        &b_uri,
        &b_text,
        "title=\"{{ bBadge }}\"",
        "bBadge",
    );
    let card_user = hover_target_end(
        &mut session,
        &card_uri,
        &card_text,
        "卡片用户：{{ user.name }}",
        "user.name",
    );
    let a_title = hover_target_end(
        &mut session,
        &a_uri,
        &a_text,
        "title=\"{{ aTitle }}\"",
        "aTitle",
    );
    let b_title = hover_target_end(
        &mut session,
        &b_uri,
        &b_text,
        "title=\"{{ bTitle }}\"",
        "bTitle",
    );

    let a_badge_text = native_lsp::hover_markdown("G-P5-E2E-19", "A badge", &a_badge);
    assert!(
        a_badge_text.contains("number"),
        "{}",
        session.describe(&a_badge)
    );
    let b_badge_text = native_lsp::hover_markdown("G-P5-E2E-19", "B badge", &b_badge);
    assert!(
        b_badge_text.contains("number"),
        "{}",
        session.describe(&b_badge)
    );
    let card_title_text = native_lsp::hover_markdown("G-P5-E2E-19", "card title", &card_title);
    assert!(
        card_title_text.contains("string"),
        "{}",
        session.describe(&card_title)
    );
    let card_user_text = native_lsp::hover_markdown("G-P5-E2E-19", "card user", &card_user);
    assert!(
        card_user_text.contains("string"),
        "{}",
        session.describe(&card_user)
    );
    let a_title_text = native_lsp::hover_markdown("G-P5-E2E-19", "A title", &a_title);
    assert!(
        a_title_text.contains("A 页面标题"),
        "{}",
        session.describe(&a_title)
    );
    let b_title_text = native_lsp::hover_markdown("G-P5-E2E-19", "B title", &b_title);
    assert!(
        b_title_text.contains("B 页面标题"),
        "{}",
        session.describe(&b_title)
    );
    assert!(
        !a_title_text.contains("B 页面标题"),
        "page A hover picked up page B documentation: {}",
        session.describe(&a_title)
    );
    assert!(
        !b_title_text.contains("A 页面标题"),
        "page B hover picked up page A documentation: {}",
        session.describe(&b_title)
    );
    assert!(
        !card_title_text.contains("页面标题"),
        "card hover picked up a page's documentation: {}",
        session.describe(&card_title)
    );

    let card_errors = card_diagnostics
        .as_array()
        .expect("card diagnostics")
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        card_errors.len(),
        1,
        "the card typo must stay in card.wxml: {card_diagnostics}"
    );
    assert!(
        card_errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("nmae")),
        "{}",
        card_errors[0]
    );
    for (file, diagnostics) in [
        ("pages/a/a.wxml", &a_diagnostics),
        ("pages/b/b.wxml", &b_diagnostics),
    ] {
        assert!(
            diagnostics
                .as_array()
                .expect("diagnostics")
                .iter()
                .all(|diagnostic| !diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("nmae"))),
            "{file}: the card typo leaked across views: {diagnostics}"
        );
        assert!(
            diagnostics
                .as_array()
                .expect("diagnostics")
                .iter()
                .all(|diagnostic| diagnostic["severity"] != json!(1)),
            "{file}: a clean page produced an error: {diagnostics}"
        );
    }

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}
