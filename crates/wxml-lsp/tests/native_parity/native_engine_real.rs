//! Opt-in native replacements for the mandatory P5 gates.
//!
//! The 2026-11-19 typed-asset slice made real hover and diagnostics available
//! through `--backend native`. These tests are the native replacements for:
//!
//! - `G-P5-E2E-01` / `non_owner_script_overlay_propagates_and_survives_a_node_restart`
//! - `G-P5-E2E-02` / `real_node_event_handler_diagnostics_definition_and_completion`
//! - `G-P5-E2E-03` / `real_node_expression_and_event_completion_smoke`
//! - `G-P5-E2E-04` / `real_node_hover_and_diagnostics_smoke`
//! - `G-P5-E2E-05` / `real_node_symbols_smoke`
//! - `G-P5-E2E-06` / `ac001_page_data_and_jsdoc_hover`
//! - `G-P5-E2E-07` / `ac002_field_typo_diagnostic_and_fix_clear`
//! - `G-P5-E2E-08` / `ac003_unicode_and_crlf_positions`
//! - `G-P5-E2E-09` / `ac011_unknown_owner_degrades_honestly`
//! - `G-P5-E2E-10` / `ac013_component_properties_hover`
//! - `G-P5-E2E-11` / `component_behavior_data_and_properties_work_over_stdio`
//! - `G-P5-E2E-12` / `static_wxs_and_template_semantics_work_over_stdio`
//! - `G-P5-E2E-13` / `static_wxs_answers_survive_an_unavailable_type_backend`
//! - `G-P5-E2E-14` / `unsaved_wxs_overlay_updates_static_hover_and_completion`
//! - `G-P5-E2E-15` / `ac005_loop_scope_typed_locals`
//! - `G-P5-E2E-16` / `ac006_conditional_union_narrowing`
//! - `G-P5-E2E-17` / `ac008_alias_project_hover_and_definition`
//! - `G-P5-E2E-18` / `ac008_multi_view_fixture_isolation`
//! - `G-P5-E2E-19` / `ac008_simultaneous_views_do_not_pollute_each_other`
//! - `G-P5-E2E-20` / `ac012_component_property_bindings`
//! - `G-P5-E2E-21` / `ac012_event_handlers_basic`
//! - `G-P5-PERF-01` / `q2_performance_benchmark`
//! - `G-P5-STABILITY-01` / `ten_round_lifecycle_stability`
//!
//! Every test is `#[ignore]`: the default workspace job reports it ignored,
//! never passed. The required native job runs them with `--ignored` and
//! `WXML_REQUIRE_REAL_ENGINE=1`; a missing package/engine is then fatal.
//! Without `WXML_REQUIRE_REAL_ENGINE` a missing package is an explicit,
//! printed optional-mode skip. Type-behavior gates additionally require
//! `TSGO_TYPINGS_ROOT` so the disposable package carries the pinned
//! miniprogram typings; a green run without them is not claimed.

use serde_json::{Value, json};

#[cfg(target_os = "linux")]
use std::time::Instant;
#[cfg(target_os = "linux")]
use wxml_backend_tsgo::config::{AST_PROTOCOL_VERSION, ENGINE_VERSION, PinnedEngine};

#[cfg(target_os = "linux")]
use crate::ledger;
use crate::native_lsp::{self, Session};
#[cfg(target_os = "linux")]
use crate::native_measure::{
    MAX_POINTS, Stats, check_p95, elapsed_ms, engine_children, git_head, growth_percent,
    kill_process, median, rss_kib, sample_tree, u64_to_f64, wait_gone, write_report,
};
use crate::native_package::NativeGate;

/// Workspace-relative file that declares the ignored native gates.
pub(crate) const NATIVE_GATE_FILE: &str =
    "crates/wxml-lsp/tests/native_parity/native_engine_real.rs";

/// Ledger gate id -> native replacement test.
///
/// All twenty-three are `gate_green` in the ledger with recorded opt-in evidence;
/// this map keeps the exact gate/replacement link checked by `native_gates.rs`.
pub(crate) const GATE_REPLACEMENTS: &[(&str, &str)] = &[
    (
        "G-P5-E2E-01",
        "native_non_owner_overlay_replay_survives_restart",
    ),
    (
        "G-P5-E2E-02",
        "native_event_handler_hover_diagnostics_definition_completion",
    ),
    (
        "G-P5-E2E-03",
        "native_expression_and_event_completion_smoke",
    ),
    ("G-P5-E2E-04", "native_hover_and_diagnostics_smoke"),
    ("G-P5-E2E-05", "native_symbols_smoke"),
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
        "G-P5-E2E-11",
        "native_component_behavior_data_and_properties",
    ),
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
    ("G-P5-E2E-16", "native_ac006_conditional_union_narrowing"),
    (
        "G-P5-E2E-17",
        "native_ac008_alias_project_hover_and_definition",
    ),
    ("G-P5-E2E-18", "native_ac008_multi_view_fixture_isolation"),
    (
        "G-P5-E2E-19",
        "native_ac008_simultaneous_views_do_not_pollute_each_other",
    ),
    ("G-P5-E2E-20", "native_ac012_component_property_bindings"),
    ("G-P5-E2E-21", "native_ac012_event_handlers_basic"),
    ("G-P5-PERF-01", "native_q2_performance_benchmark"),
    ("G-P5-STABILITY-01", "native_ten_round_lifecycle_stability"),
];

/// The exact ignored-native-gate set.
///
/// `ignored_functions()` in `native_gates.rs` subtracts this list from the
/// scanned `#[ignore]` set and still requires the remainder to equal the
/// ledger's 23 old gates exactly.
pub(crate) const IGNORED_GATE_TESTS: &[(&str, &str)] = &[
    (
        NATIVE_GATE_FILE,
        "native_expression_and_event_completion_smoke",
    ),
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
        "native_component_behavior_data_and_properties",
    ),
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
    (NATIVE_GATE_FILE, "native_ac006_conditional_union_narrowing"),
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
    (NATIVE_GATE_FILE, "native_ac012_component_property_bindings"),
    (NATIVE_GATE_FILE, "native_ac012_event_handlers_basic"),
    (
        NATIVE_GATE_FILE,
        "native_event_handler_hover_diagnostics_definition_completion",
    ),
    (
        NATIVE_GATE_FILE,
        "native_non_owner_overlay_replay_survives_restart",
    ),
    (NATIVE_GATE_FILE, "native_q2_performance_benchmark"),
    (NATIVE_GATE_FILE, "native_symbols_smoke"),
    (NATIVE_GATE_FILE, "native_ten_round_lifecycle_stability"),
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

/// G-P5-E2E-01: non-owner overlays propagate, update and replay after restart.
///
/// Four frozen behaviours from the retired Node gate:
///
/// 1. an unsaved non-owner model overlay is visible on the owning template;
/// 2. closing the overlay restores the disk declaration;
/// 3. re-opening it is visible again;
/// 4. a restarted engine replays every still-open overlay, and the **first**
///    request after the restart already succeeds.
///
/// The same test also covers the multi-overlay case and the content-aware
/// `files_hash`: two overlays stay independent, and a second `didChange` is
/// visible immediately without a restart.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
#[allow(clippy::too_many_lines, reason = "one frozen overlay/replay lifecycle")]
fn native_non_owner_overlay_replay_survives_restart() {
    let Some(gate) = NativeGate::require("G-P5-E2E-01") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/second/second"],"usingComponents":{}}"#,
    );
    let index_text = "{{ user.name }} {{ extra.code }}";
    gate.write("pages/index/index.wxml", index_text);
    gate.write(
        "pages/index/index.ts",
        "import type { User } from \"../../models/user\";\nimport type { Extra } from \"../../models/extra\";\nPage({ data: { user: null as unknown as User, extra: null as unknown as Extra } });\n",
    );
    let second_text = "{{ user.name }}";
    gate.write("pages/second/second.wxml", second_text);
    gate.write(
        "pages/second/second.ts",
        "import type { User } from \"../../models/user\";\nPage({ data: { user: null as unknown as User } });\n",
    );
    gate.write("models/user.ts", "export interface User { name: string }\n");
    gate.write(
        "models/extra.ts",
        "export interface Extra { code: number }\n",
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let index_uri = gate.uri("pages/index/index.wxml");
    let second_uri = gate.uri("pages/second/second.wxml");
    let user_uri = gate.uri("models/user.ts");
    let extra_uri = gate.uri("models/extra.ts");
    session.open_wxml(&index_uri, index_text, 1);
    session.open_wxml(&second_uri, second_text, 1);

    let name_before = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ user.name }}",
        "name",
    );
    let name_before_text =
        native_lsp::hover_markdown("G-P5-E2E-01", "disk model hover", &name_before);
    assert!(
        name_before_text.contains("string"),
        "the disk model must type `name` as string: {}",
        session.describe(&name_before)
    );
    let code_before = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ extra.code }}",
        "code",
    );
    let code_before_text =
        native_lsp::hover_markdown("G-P5-E2E-01", "disk extra hover", &code_before);
    assert!(
        code_before_text.contains("number"),
        "the disk model must type `code` as number: {}",
        session.describe(&code_before)
    );

    // 1. One non-owner overlay becomes visible on both templates.
    open_overlay(
        &mut session,
        &user_uri,
        "typescript",
        "export interface User { name: \"OVERLAY\" }\n",
    );
    let overlay_name = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ user.name }}",
        "name",
    );
    let overlay_name_text =
        native_lsp::hover_markdown("G-P5-E2E-01", "overlay model hover", &overlay_name);
    assert!(
        overlay_name_text.contains("OVERLAY"),
        "the overlay must replace the disk declaration: {}",
        session.describe(&overlay_name)
    );
    let second_overlay = hover_target_end(
        &mut session,
        &second_uri,
        second_text,
        "{{ user.name }}",
        "name",
    );
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-01", "cross-view overlay hover", &second_overlay)
            .contains("OVERLAY"),
        "a non-owner overlay is project-wide: {}",
        session.describe(&second_overlay)
    );
    let untouched = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ extra.code }}",
        "code",
    );
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-01", "untouched extra hover", &untouched)
            .contains("number"),
        "opening one overlay must not hide another file: {}",
        session.describe(&untouched)
    );

    // The content-aware files hash: a second same-session overlay text is
    // visible immediately, without a restart or an explicit close.
    session.change_document(
        &user_uri,
        "export interface User { name: \"OVERLAY_TWO\" }\n",
        2,
    );
    let updated = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ user.name }}",
        "name",
    );
    let updated_text = native_lsp::hover_markdown("G-P5-E2E-01", "updated overlay hover", &updated);
    assert!(
        updated_text.contains("OVERLAY_TWO"),
        "an overlay text change must update the effective files hash: {}",
        session.describe(&updated)
    );
    assert!(
        !updated_text.contains("\"OVERLAY\""),
        "the stale overlay text must not win: {}",
        session.describe(&updated)
    );

    // 2. Multiple overlays stay independent.
    open_overlay(
        &mut session,
        &extra_uri,
        "typescript",
        "export interface Extra { code: \"EXTRA\" }\n",
    );
    let extra_overlay = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ extra.code }}",
        "code",
    );
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-01", "second overlay hover", &extra_overlay)
            .contains("EXTRA"),
        "a second overlay must be visible: {}",
        session.describe(&extra_overlay)
    );
    let both = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ user.name }}",
        "name",
    );
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-01", "both overlays visible", &both)
            .contains("OVERLAY_TWO"),
        "the second overlay must not replace the first: {}",
        session.describe(&both)
    );

    // 3. Kill the engine: the first request after the restart must replay the
    //    current overlays and succeed on its own (one-shot dead-session retry).
    let engines = engine_children(session.pid(), gate.package_root());
    assert_eq!(
        engines.len(),
        1,
        "the session must hold exactly one engine before the restart: {engines:?}"
    );
    let old_engine = engines[0];
    kill_process(old_engine);
    wait_gone(&[old_engine], "killed engine");
    let first = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ user.name }}",
        "name",
    );
    let first_text =
        native_lsp::hover_markdown("G-P5-E2E-01", "first request after restart", &first);
    assert!(
        first_text.contains("OVERLAY_TWO"),
        "the first request after a restart must replay the open overlay: {}",
        session.describe(&first)
    );
    let extra_after = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ extra.code }}",
        "code",
    );
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-01", "second overlay after restart", &extra_after)
            .contains("EXTRA"),
        "every open overlay must be replayed: {}",
        session.describe(&extra_after)
    );
    let new_engines = engine_children(session.pid(), gate.package_root());
    assert_eq!(
        new_engines.len(),
        1,
        "the restart must leave exactly one live engine: {new_engines:?}"
    );
    assert_ne!(
        new_engines[0], old_engine,
        "the restart reused the killed engine process"
    );

    // 4. Closing the overlay restores the disk declaration, without losing the
    //    other overlay.
    session.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": user_uri}}),
    );
    let restored = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ user.name }}",
        "name",
    );
    let restored_text = native_lsp::hover_markdown("G-P5-E2E-01", "restored disk hover", &restored);
    assert!(
        restored_text.contains("string"),
        "closing the overlay must restore the disk declaration: {}",
        session.describe(&restored)
    );
    assert!(
        !restored_text.contains("OVERLAY"),
        "the closed overlay must not stay visible: {}",
        session.describe(&restored)
    );
    let extra_kept = hover_target_end(
        &mut session,
        &index_uri,
        index_text,
        "{{ extra.code }}",
        "code",
    );
    assert!(
        native_lsp::hover_markdown("G-P5-E2E-01", "remaining overlay", &extra_kept)
            .contains("EXTRA"),
        "closing one overlay must not drop another: {}",
        session.describe(&extra_kept)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.assert_no_native_sandbox();
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

/// Hovers the first byte of `target` inside `marker`.
fn hover_target_start(
    session: &mut Session,
    uri: &str,
    text: &str,
    marker: &str,
    target: &str,
) -> Value {
    let marker_at = native_lsp::lsp_position(text, marker);
    let target_at = native_lsp::lsp_position(marker, target);
    session.hover(uri, (marker_at.0 + target_at.0, marker_at.1 + target_at.1))
}

/// Asserts one hover range covers exactly `target` inside `marker`.
fn assert_hover_token(hover: &Value, text: &str, marker: &str, target: &str, context: &str) {
    let marker_byte = text
        .find(marker)
        .unwrap_or_else(|| panic!("{context}: marker {marker:?} is not in the document"));
    let target_byte = marker
        .find(target)
        .unwrap_or_else(|| panic!("{context}: target {target:?} is not in {marker:?}"));
    let start = native_lsp::position_at(text, marker_byte + target_byte);
    let end = native_lsp::position_at(text, marker_byte + target_byte + target.len());
    assert_eq!(
        hover["result"]["range"],
        json!({
            "start": {"line": start.0, "character": start.1},
            "end": {"line": end.0, "character": end.1},
        }),
        "{context}: hover must cover exactly {target:?}: {hover}"
    );
}

/// Returns an LSP position tuple from one JSON position.
fn json_position(value: &Value) -> (u32, u32) {
    (
        u32::try_from(value["line"].as_u64().expect("start line")).expect("line fits"),
        u32::try_from(value["character"].as_u64().expect("start character"))
            .expect("character fits"),
    )
}

/// Returns both LSP positions of one JSON range.
fn json_range(value: &Value) -> ((u32, u32), (u32, u32)) {
    (json_position(&value["start"]), json_position(&value["end"]))
}

/// Fails when any diagnostic overlaps the exact bytes of `marker`.
fn assert_no_diagnostic_overlapping(diagnostics: &Value, text: &str, marker: &str, context: &str) {
    let marker_byte = text
        .find(marker)
        .unwrap_or_else(|| panic!("{context}: marker {marker:?} is not in the document"));
    let start = native_lsp::position_at(text, marker_byte);
    let end = native_lsp::position_at(text, marker_byte + marker.len());
    for diagnostic in diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("{context}: diagnostics are not an array: {diagnostics}"))
    {
        let (diagnostic_start, diagnostic_end) = json_range(&diagnostic["range"]);
        assert!(
            !(diagnostic_start < end && start < diagnostic_end),
            "{context}: {marker:?} produced a diagnostic: {diagnostic}"
        );
    }
}

/// Asserts the document publishes exactly one zero-width degradation warning.
fn assert_status_warning(diagnostics: &Value, label: &str, context: &str) {
    let warnings = diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("{context}: diagnostics are not an array: {diagnostics}"))
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(2))
        .collect::<Vec<_>>();
    assert_eq!(
        warnings.len(),
        1,
        "{context}: expected exactly one status warning, got {diagnostics}"
    );
    let warning = warnings[0];
    assert!(
        warning["message"]
            .as_str()
            .is_some_and(|message| message.contains(label)),
        "{context}: the status warning must say {label:?}: {warning}"
    );
    assert_eq!(
        json_range(&warning["range"]),
        ((0, 0), (0, 0)),
        "{context}: the status warning must be zero-width at 0:0: {warning}"
    );
}

/// Asserts one location list contains exactly the `token` at UTF-8 `start`.
fn assert_location_at(
    locations: &[Value],
    uri: &str,
    text: &str,
    start: usize,
    token: &str,
    context: &str,
) {
    let expected_start = native_lsp::position_at(text, start);
    let expected_end = native_lsp::position_at(text, start + token.len());
    let expected = json!({
        "start": {"line": expected_start.0, "character": expected_start.1},
        "end": {"line": expected_end.0, "character": expected_end.1},
    });
    assert!(
        locations
            .iter()
            .any(|location| { location["uri"] == json!(uri) && location["range"] == expected }),
        "{context}: no location covers exactly {token:?} at {start}: {locations:?}"
    );
}

/// Returns the edits for one `WorkspaceEdit.documentChanges` URI.
fn document_change_edits<'a>(edit: &'a Value, uri: &str, context: &str) -> &'a [Value] {
    let changes = edit["documentChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("{context}: no documentChanges: {edit}"));
    let entry = changes
        .iter()
        .find(|entry| entry["textDocument"]["uri"] == json!(uri))
        .unwrap_or_else(|| panic!("{context}: no documentChanges entry for {uri}: {edit}"));
    entry["edits"]
        .as_array()
        .unwrap_or_else(|| panic!("{context}: entry has no edits: {entry}"))
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

/// Returns the error diagnostics in source order.
fn error_diagnostics(diagnostics: &Value) -> Vec<&Value> {
    diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("diagnostics are not an array: {diagnostics}"))
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect()
}

/// Asserts one definition location covers exactly `token` in `script`.
fn assert_definition_covers(
    gate_id: &str,
    gate: &NativeGate,
    response: &Value,
    script: &str,
    token: &str,
) {
    let locations = native_lsp::definition_locations(gate_id, token, response);
    let program = native_lsp::read_project_file(gate, script);
    let expected_uri = gate.uri(script);
    let covered = locations
        .iter()
        .find_map(|location| {
            if location["uri"] != json!(expected_uri) {
                return None;
            }
            let line = usize::try_from(location["range"]["start"]["line"].as_u64()?).ok()?;
            let start = usize::try_from(location["range"]["start"]["character"].as_u64()?).ok()?;
            let end = usize::try_from(location["range"]["end"]["character"].as_u64()?).ok()?;
            program.lines().nth(line)?.get(start..end)
        })
        .unwrap_or_else(|| {
            panic!("{gate_id}: no definition in {script} covering {token:?}: {response}")
        });
    assert_eq!(
        covered, token,
        "{gate_id}: definition range must cover exactly {token:?} in {script}: {response}"
    );
}

/// Returns one completion item by exact label.
fn completion_item<'a>(response: &'a Value, label: &str) -> &'a Value {
    response["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("completion without items: {response}"))
        .iter()
        .find(|item| item["label"] == json!(label))
        .unwrap_or_else(|| panic!("no {label:?} completion item: {response}"))
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

/// G-P5-E2E-03: expression, root, trailing-dot and event completion over stdio.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_expression_and_event_completion_smoke() {
    let Some(gate) = NativeGate::require("G-P5-E2E-03") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    let text = r#"<view>{{ user.na }}</view><view bindtap="onT" /><view>{{  }}</view><view>{{ user. }}</view>"#;
    gate.write("pages/index/index.wxml", text);
    gate.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada', age: 36 } }, onTap() {}, onLoad() {} });",
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    session.open_wxml(&uri, text, 1);

    let member_position = native_lsp::lsp_position(text, "user.na");
    let members = session.completion(&uri, (member_position.0, member_position.1 + 7));
    assert!(
        completion_labels(&members).contains(&"name"),
        "{}",
        session.describe(&members)
    );

    let handler_position = native_lsp::lsp_position(text, "onT");
    let handlers = session.completion(&uri, (handler_position.0, handler_position.1 + 3));
    let handler_labels = completion_labels(&handlers);
    assert!(
        handler_labels.contains(&"onTap"),
        "{}",
        session.describe(&handlers)
    );
    let item = completion_item(&handlers, "onTap");
    assert_eq!(
        item["textEdit"]["range"]["start"],
        json!({"line": handler_position.0, "character": handler_position.1}),
        "event completion must replace the whole handler token: {}",
        session.describe(&handlers)
    );

    let root_position = native_lsp::lsp_position(text, "{{  }}");
    let roots = session.completion(&uri, (root_position.0, root_position.1 + 3));
    assert!(
        completion_labels(&roots).contains(&"user"),
        "{}",
        session.describe(&roots)
    );

    let dot_position = native_lsp::lsp_position(text, "{{ user. }}");
    let trailing = session.completion(&uri, (dot_position.0, dot_position.1 + 8));
    assert!(
        completion_labels(&trailing).contains(&"name"),
        "{}",
        session.describe(&trailing)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// Returns the LSP position of `target` inside `marker`.
fn token_position(text: &str, marker: &str, target: &str) -> (u32, u32) {
    let marker_at = native_lsp::lsp_position(text, marker);
    let target_at = native_lsp::lsp_position(marker, target);
    (marker_at.0 + target_at.0, marker_at.1 + target_at.1)
}

/// G-P5-E2E-02: event-handler hover, diagnostics, definition and completion.
///
/// This is the native replacement for the retired Node gate. It keeps the old
/// fixture shape: a page with `onTap`, an arrow handler, a shorthand handler,
/// a component `methods` owner, and the three degraded binding forms
/// (`{{ dynamic }}`, empty and non-identifier) that must stay silent.
///
/// The hover assertion is the new native capability: `onTap` returns its exact
/// one-line signature plus `JSDoc`, and `onShorthandTap` returns its shorthand
/// signature; both ranges cover exactly the handler token in the WXML source.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
#[allow(clippy::too_many_lines, reason = "one frozen event gate fixture")]
fn native_event_handler_hover_diagnostics_definition_completion() {
    let Some(gate) = NativeGate::require("G-P5-E2E-02") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    gate.write(
        "pages/index/index.json",
        r#"{"usingComponents":{"widget":"../../components/widget/widget"}}"#,
    );
    let page_text = concat!(
        "<view bindtap=\"onTap\" catchtap=\"missingHandler\" />\n",
        "<view onTapPlaceholder=\"\" capture-bind:tap=\"onTap\" mut-bind:tap=\"onTap\" />\n",
        "<view bindtap=\"{{ dynamicHandler }}\" />\n",
        "<view catchtap=\"\" />\n",
        "<view bindtap=\"this.invalid\" />\n",
        "<view bindtap=\"onArrow\" />\n",
        "<view bindtap=\"onShorthandTap\" />\n",
    );
    gate.write("pages/index/index.wxml", page_text);
    gate.write(
        "pages/index/index.ts",
        concat!(
            "Page({\n",
            "  data: { label: 'x' },\n",
            "  /** 点击处理 😀 */\n",
            "  onTap(event: { detail: { x: number } }) { void event; },\n",
            "  onArrow: (event: { detail: { y: number } }) => { void event; },\n",
            "  onShorthandTap() { const label = 'x'; void label; },\n",
            "});\n",
        ),
    );
    gate.write("components/widget/widget.json", r#"{"component":true}"#);
    gate.write(
        "components/widget/widget.wxml",
        "<view bindtap=\"onWidgetTap\" /><view bindtap=\"missingWidget\" />",
    );
    gate.write(
        "components/widget/widget.ts",
        concat!(
            "Component({\n",
            "  properties: {},\n",
            "  methods: {\n",
            "    onWidgetTap(event: { detail: { z: number } }) { void event; },\n",
            "    onWidgetArrow: () => {},\n",
            "  },\n",
            "});\n",
        ),
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, page_text, 1);
    assert_no_status_diagnostic(&diagnostics, "E2E-02 page");
    let errors = error_diagnostics(&diagnostics);
    assert_eq!(
        errors.len(),
        1,
        "the page must report only the missing handler: {diagnostics}"
    );
    assert_error_at(
        &diagnostics,
        page_text,
        "catchtap=\"missingHandler\"",
        "missingHandler",
        "E2E-02 missing handler",
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("missingHandler") && message.contains("tap")),
        "the missing-handler message must name the handler and the event: {}",
        errors[0]
    );
    for marker in [
        "bindtap=\"{{ dynamicHandler }}\"",
        "catchtap=\"\"",
        "bindtap=\"this.invalid\"",
    ] {
        assert_no_diagnostic_overlapping(&diagnostics, page_text, marker, "E2E-02 degraded");
    }

    let on_tap = hover_target_start(&mut session, &uri, page_text, "bindtap=\"onTap\"", "onTap");
    let on_tap_text = native_lsp::hover_markdown("G-P5-E2E-02", "onTap hover", &on_tap);
    assert!(
        on_tap_text.contains("onTap(event: { detail: { x: number } })"),
        "onTap hover must carry its exact signature: {}",
        session.describe(&on_tap)
    );
    assert!(
        on_tap_text.contains("点击处理 😀"),
        "onTap hover must carry its JSDoc: {}",
        session.describe(&on_tap)
    );
    assert_hover_token(
        &on_tap,
        page_text,
        "bindtap=\"onTap\"",
        "onTap",
        "E2E-02 onTap",
    );

    let shorthand = hover_target_start(
        &mut session,
        &uri,
        page_text,
        "bindtap=\"onShorthandTap\"",
        "onShorthandTap",
    );
    let shorthand_text =
        native_lsp::hover_markdown("G-P5-E2E-02", "onShorthandTap hover", &shorthand);
    assert!(
        shorthand_text.contains("onShorthandTap()"),
        "the shorthand handler hover must carry its signature: {}",
        session.describe(&shorthand)
    );
    assert_hover_token(
        &shorthand,
        page_text,
        "bindtap=\"onShorthandTap\"",
        "onShorthandTap",
        "E2E-02 onShorthandTap",
    );

    let on_tap_definition = session.definition(
        &uri,
        token_position(page_text, "bindtap=\"onTap\"", "onTap"),
    );
    assert_definition_covers(
        "G-P5-E2E-02",
        &gate,
        &on_tap_definition,
        "pages/index/index.ts",
        "onTap",
    );
    let on_arrow_definition = session.definition(
        &uri,
        token_position(page_text, "bindtap=\"onArrow\"", "onArrow"),
    );
    assert_definition_covers(
        "G-P5-E2E-02",
        &gate,
        &on_arrow_definition,
        "pages/index/index.ts",
        "onArrow",
    );
    let missing_definition = session.definition(
        &uri,
        token_position(page_text, "catchtap=\"missingHandler\"", "missingHandler"),
    );
    assert!(
        missing_definition["result"].is_null(),
        "a missing handler is a supported check with no definition: {missing_definition}"
    );

    let completion_prefix = token_position(page_text, "bindtap=\"onArrow\"", "onArrow");
    let handlers = session.completion(&uri, (completion_prefix.0, completion_prefix.1 + 3));
    let labels = completion_labels(&handlers);
    for label in ["onTap", "onArrow"] {
        assert!(
            labels.contains(&label),
            "missing {label} in {}",
            session.describe(&handlers)
        );
    }
    assert!(
        !labels.contains(&"label") && labels.iter().all(|label| !label.starts_with("__wxml_")),
        "data or synthetic members leaked into event completion: {}",
        session.describe(&handlers)
    );
    let on_tap_item = completion_item(&handlers, "onTap");
    assert_eq!(
        on_tap_item["textEdit"]["range"]["start"],
        json!({
            "line": completion_prefix.0,
            "character": completion_prefix.1,
        }),
        "event completion must replace the whole handler token: {}",
        session.describe(&handlers)
    );

    let widget_text = native_lsp::read_project_file(&gate, "components/widget/widget.wxml");
    let widget_uri = gate.uri("components/widget/widget.wxml");
    let widget_diagnostics = session.open_wxml(&widget_uri, &widget_text, 1);
    let widget_errors = error_diagnostics(&widget_diagnostics);
    assert_eq!(
        widget_errors.len(),
        1,
        "the component must report only the missing method: {widget_diagnostics}"
    );
    assert!(
        widget_errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("missingWidget")),
        "component methods lookup did not flag missingWidget: {}",
        widget_errors[0]
    );
    let widget_definition = session.definition(
        &widget_uri,
        token_position(&widget_text, "bindtap=\"onWidgetTap\"", "onWidgetTap"),
    );
    assert_definition_covers(
        "G-P5-E2E-02",
        &gate,
        &widget_definition,
        "components/widget/widget.ts",
        "onWidgetTap",
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-20: the frozen `component-bindings` acceptance fixture.
///
/// Covers all four surfaces of the old gate: the two typed hovers with
/// `JSDoc`, the single property mismatch (`title="{{ total }}"`), the single
/// missing-handler error, and the two event definitions (an exact declaration
/// token and a checked `null`). Property bindings add no definition
/// expectation, and event attributes are excluded from the property pass, so
/// the published counts are exact.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac012_component_property_bindings() {
    let Some(gate) = NativeGate::require("G-P5-E2E-20") else {
        return;
    };
    copy_fixture(&gate, "component-bindings");
    let text = native_lsp::read_project_file(&gate, "pages/index/index.wxml");

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, &text, 1);

    // The two typed hovers: page data with its `JSDoc`, exact token ranges.
    for (marker, target, type_text, documentation) in [
        (
            "title=\"{{ pageTitle }}\"",
            "pageTitle",
            "string",
            "页面标题",
        ),
        ("count=\"{{ total }}\"", "total", "number", "卡片总数"),
    ] {
        let hover = hover_target_end(&mut session, &uri, &text, marker, target);
        let contents = native_lsp::hover_markdown("G-P5-E2E-20", target, &hover);
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
        assert_hover_token(&hover, &text, marker, target, "E2E-20 hover");
    }

    // Exactly one property error at the bound expression, one missing-handler
    // error, and nothing else. The fixture resolves every binding, so the
    // status stays Supported and no degradation warning is published.
    assert_no_status_diagnostic(&diagnostics, "AC-012 component bindings");
    let errors = error_diagnostics(&diagnostics);
    assert_eq!(
        errors.len(),
        2,
        "the page must report exactly the property mismatch and the missing handler: {diagnostics}"
    );
    assert_error_at(
        &diagnostics,
        &text,
        "title=\"{{ total }}\"",
        "total",
        "AC-012 property mismatch",
    );
    let property_error = errors
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("component property \"title\""))
        })
        .unwrap_or_else(|| panic!("no component property diagnostic for `title`: {diagnostics}"));
    assert!(
        property_error["message"]
            .as_str()
            .is_some_and(|message| message.contains("number")),
        "the property mismatch must name the `number` source: {property_error}"
    );
    assert_error_at(
        &diagnostics,
        &text,
        "bindtap=\"missingHandler\"",
        "missingHandler",
        "AC-012 missing handler",
    );

    // The two event definitions: `onCardTap` covers the exact declaration
    // token; `missingHandler` is a checked `null`, never a fabricated target.
    let definition = session.definition(
        &uri,
        token_position(&text, "bind:tap=\"onCardTap\"", "onCardTap"),
    );
    assert_definition_covers(
        "G-P5-E2E-20",
        &gate,
        &definition,
        "pages/index/index.ts",
        "onCardTap",
    );
    let missing_definition = session.definition(
        &uri,
        token_position(&text, "bindtap=\"missingHandler\"", "missingHandler"),
    );
    assert!(
        missing_definition["result"].is_null(),
        "a missing handler is a supported check with no definition: {missing_definition}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-21: the frozen `events-basic` acceptance fixture.
///
/// Covers all four surfaces of the old gate: the three typed hovers with
/// `JSDoc`, the two missing-handler errors, the 13 definitions (non-null ones
/// covering the exact declaration token, missing ones a checked `null`), and
/// the event completion replacement range. The two degradation classes are
/// asserted through their LSP form: `Partial` and `Unsupported` publish exactly
/// one zero-width status warning and never an error.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
#[allow(clippy::too_many_lines, reason = "one frozen acceptance fixture")]
fn native_ac012_event_handlers_basic() {
    let Some(gate) = NativeGate::require("G-P5-E2E-21") else {
        return;
    };
    copy_fixture(&gate, "events-basic");
    let index_text = native_lsp::read_project_file(&gate, "pages/index/index.wxml");
    let widget_text = native_lsp::read_project_file(&gate, "components/widget/widget.wxml");
    let multi_text = native_lsp::read_project_file(&gate, "pages/multi/multi.wxml");
    let nonliteral_text = native_lsp::read_project_file(&gate, "pages/nonliteral/nonliteral.wxml");

    let mut session = Session::start(&gate);
    let index_uri = gate.uri("pages/index/index.wxml");
    let widget_uri = gate.uri("components/widget/widget.wxml");
    let multi_uri = gate.uri("pages/multi/multi.wxml");
    let nonliteral_uri = gate.uri("pages/nonliteral/nonliteral.wxml");
    let index_diagnostics = session.open_wxml(&index_uri, &index_text, 1);
    let widget_diagnostics = session.open_wxml(&widget_uri, &widget_text, 1);
    let multi_diagnostics = session.open_wxml(&multi_uri, &multi_text, 1);
    let nonliteral_diagnostics = session.open_wxml(&nonliteral_uri, &nonliteral_text, 1);

    // The three typed hovers with their exact range and documentation.
    for (uri, text, marker, target, documentation) in [
        (&index_uri, &index_text, "{{ label }}", "label", "页面标签"),
        (
            &widget_uri,
            &widget_text,
            "{{ title }}",
            "title",
            "组件标题",
        ),
        (
            &multi_uri,
            &multi_text,
            "{{ firstLabel }}",
            "firstLabel",
            "首个注册的标签",
        ),
    ] {
        let hover = hover_target_end(&mut session, uri, text, marker, target);
        let contents = native_lsp::hover_markdown("G-P5-E2E-21", target, &hover);
        assert!(
            contents.contains("string"),
            "{target} must be string: {}",
            session.describe(&hover)
        );
        assert!(
            contents.contains(documentation),
            "{target} must include {documentation:?}: {}",
            session.describe(&hover)
        );
        assert_hover_token(&hover, text, marker, target, "E2E-21 hover");
    }

    // Page: exactly one error, and silence over every degraded form.
    assert_no_status_diagnostic(&index_diagnostics, "AC-012 page");
    let index_errors = error_diagnostics(&index_diagnostics);
    assert_eq!(
        index_errors.len(),
        1,
        "the page must report only the missing handler: {index_diagnostics}"
    );
    assert_error_at(
        &index_diagnostics,
        &index_text,
        "bindtap=\"missingHandler\"",
        "missingHandler",
        "AC-012 page missing handler",
    );
    for marker in [
        "bindtap=\"{{dynamicHandler}}\"",
        "bindtap=\"\"",
        "bindtap=\"this.onTap\"",
    ] {
        assert_no_diagnostic_overlapping(&index_diagnostics, &index_text, marker, "AC-012 page");
    }

    // Component: one missing-method error; `methods` is the owner table.
    let widget_errors = error_diagnostics(&widget_diagnostics);
    assert_eq!(
        widget_errors.len(),
        1,
        "the component must report only the missing method: {widget_diagnostics}"
    );
    assert!(
        widget_errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("missingWidgetHandler")),
        "{widget_diagnostics}"
    );
    assert_status_warning(
        &widget_diagnostics,
        "type checking is partial",
        "AC-012 widget",
    );

    // Multiple registrations keep the first owner table and degrade to
    // Partial: a method that may live in the unread registration is not an
    // error.
    let multi_errors = error_diagnostics(&multi_diagnostics);
    assert!(
        multi_errors.is_empty(),
        "pages/multi/multi.wxml fabricated an error: {multi_diagnostics}"
    );
    assert_status_warning(
        &multi_diagnostics,
        "type checking is partial",
        "AC-012 multi",
    );

    // A non-literal root has no analyzable owner object: `Unsupported`, not
    // `Unavailable`, and no fabricated methods.
    let nonliteral_errors = error_diagnostics(&nonliteral_diagnostics);
    assert!(
        nonliteral_errors.is_empty(),
        "pages/nonliteral/nonliteral.wxml fabricated an error: {nonliteral_diagnostics}"
    );
    assert_status_warning(
        &nonliteral_diagnostics,
        "type checking is unsupported",
        "AC-012 nonliteral",
    );

    // The 13 frozen definitions: exact declaration tokens, or a checked null.
    for (file, marker, target, script) in [
        (
            "pages/index/index.wxml",
            "bindtap=\"onShorthandTap\"",
            "onShorthandTap",
            "pages/index/index.ts",
        ),
        (
            "pages/index/index.wxml",
            "catchtap=\"onFunctionTap\"",
            "onFunctionTap",
            "pages/index/index.ts",
        ),
        (
            "pages/index/index.wxml",
            "bind:tap=\"onArrowTap\"",
            "onArrowTap",
            "pages/index/index.ts",
        ),
        (
            "pages/index/index.wxml",
            "capture-bind:tap=\"onCaptureTap\"",
            "onCaptureTap",
            "pages/index/index.ts",
        ),
        (
            "pages/index/index.wxml",
            "capture-catch:tap=\"onCaptureCatchTap\"",
            "onCaptureCatchTap",
            "pages/index/index.ts",
        ),
        (
            "pages/index/index.wxml",
            "mut-bind:tap=\"onMutTap\"",
            "onMutTap",
            "pages/index/index.ts",
        ),
        (
            "pages/index/index.wxml",
            "catch:tap=\"onCatchTap\"",
            "onCatchTap",
            "pages/index/index.ts",
        ),
        (
            "components/widget/widget.wxml",
            "bindtap=\"onWidgetTap\"",
            "onWidgetTap",
            "components/widget/widget.ts",
        ),
        (
            "components/widget/widget.wxml",
            "bindtap=\"onWidgetArrow\"",
            "onWidgetArrow",
            "components/widget/widget.ts",
        ),
    ] {
        let (uri, text) = match file {
            "pages/index/index.wxml" => (&index_uri, &index_text),
            "components/widget/widget.wxml" => (&widget_uri, &widget_text),
            other => panic!("unexpected definition file {other}"),
        };
        let definition = session.definition(uri, token_position(text, marker, target));
        assert_definition_covers("G-P5-E2E-21", &gate, &definition, script, target);
    }
    for (uri, text, marker, target) in [
        (
            &index_uri,
            &index_text,
            "bindtap=\"missingHandler\"",
            "missingHandler",
        ),
        (
            &widget_uri,
            &widget_text,
            "bindtap=\"missingWidgetHandler\"",
            "missingWidgetHandler",
        ),
        (
            &multi_uri,
            &multi_text,
            "bindtap=\"onSecondTap\"",
            "onSecondTap",
        ),
        (
            &nonliteral_uri,
            &nonliteral_text,
            "bindtap=\"onNonLiteralTap\"",
            "onNonLiteralTap",
        ),
    ] {
        let definition = session.definition(uri, token_position(text, marker, target));
        assert!(
            definition["result"].is_null(),
            "{target} must not resolve to a fabricated definition: {definition}"
        );
    }

    // Event completion enumerates the owner's methods only.
    let value_start = token_position(&index_text, "bindtap=\"onShorthandTap\"", "onShorthandTap");
    let handlers = session.completion(&index_uri, (value_start.0, value_start.1 + 3));
    let labels = completion_labels(&handlers);
    for label in ["onShorthandTap", "onFunctionTap", "onArrowTap"] {
        assert!(
            labels.contains(&label),
            "missing {label} in {}",
            session.describe(&handlers)
        );
    }
    for reserved in ["label", "data", "__wxml_page"] {
        assert!(
            !labels.contains(&reserved),
            "{reserved} leaked into event completion: {}",
            session.describe(&handlers)
        );
    }
    let shorthand_item = completion_item(&handlers, "onShorthandTap");
    assert_eq!(
        shorthand_item["textEdit"]["range"]["start"],
        json!({"line": value_start.0, "character": value_start.1}),
        "event completion must replace the whole handler token: {}",
        session.describe(&handlers)
    );

    let widget_start = token_position(&widget_text, "bindtap=\"onWidgetTap\"", "onWidgetTap");
    let widget_handlers = session.completion(&widget_uri, (widget_start.0, widget_start.1 + 6));
    let widget_labels = completion_labels(&widget_handlers);
    for label in ["onWidgetTap", "onWidgetArrow"] {
        assert!(
            widget_labels.contains(&label),
            "missing {label} in {}",
            session.describe(&widget_handlers)
        );
    }

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-05: native script symbols through their LSP consumers.
///
/// `Backend::symbols` is not itself an LSP method; its users are
/// `textDocument/references`, `prepareRename`, `rename` and the missing-handler
/// quick fix. All four answer only when the `SymbolResult` is `complete`, so a
/// green run proves the `DataPath` declaration, the `this.data` read, the
/// `setData` path segment and the missing `onSave` insertion are exact.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one frozen symbol/reference/rename fixture"
)]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_symbols_smoke() {
    let Some(gate) = NativeGate::require("G-P5-E2E-05") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    let wxml = "<view bindtap=\"onTap\">{{ user.name }}</view>\n<view bindtap=\"onSave\"></view>";
    gate.write("pages/index/index.wxml", wxml);
    let script = concat!(
        "Page({\n",
        "  data: { user: { name: 'Ada' } },\n",
        "  onLoad() { const label = this.data.user.name; this.setData({ \"user.name\": label }); },\n",
        "  onTap() {},\n",
        "});\n",
    );
    gate.write("pages/index/index.ts", script);
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let script_uri = gate.uri("pages/index/index.ts");
    session.open_wxml(&uri, wxml, 1);
    let name_position = token_position(wxml, "{{ user.name }}", "name");

    // DataPath references: declaration, `this.data` read and the `setData`
    // path segment, plus the WXML token, all exact.
    let references = session.references(&uri, name_position, true);
    let locations = references["result"]
        .as_array()
        .unwrap_or_else(|| panic!("references returned no locations: {references}"));
    assert_eq!(
        locations.len(),
        4,
        "the script has exactly four proven `name` tokens: {references}"
    );
    let declaration = script.find("name: 'Ada'").expect("declaration token");
    assert_location_at(
        locations,
        &script_uri,
        script,
        declaration,
        "name",
        "E2E-05 declaration",
    );
    let read =
        script.find("this.data.user.name").expect("this.data read") + "this.data.user.".len();
    assert_location_at(locations, &script_uri, script, read, "name", "E2E-05 read");
    let set_data = script.find("\"user.name\"").expect("setData key") + 1 + "user.".len();
    assert_location_at(
        locations,
        &script_uri,
        script,
        set_data,
        "name",
        "E2E-05 setData segment",
    );
    let wxml_name = wxml.find("{{ user.name }}").expect("WXML name") + "{{ user.".len();
    assert_location_at(
        locations,
        &uri,
        wxml,
        wxml_name,
        "name",
        "E2E-05 WXML token",
    );

    // prepareRename is a checked answer, not a guess.
    let prepare = session.prepare_rename(&uri, name_position);
    let expected_start = native_lsp::position_at(wxml, wxml_name);
    let expected_end = native_lsp::position_at(wxml, wxml_name + "name".len());
    assert_eq!(
        prepare["result"]["placeholder"],
        json!("name"),
        "prepareRename must name the exact token: {prepare}"
    );
    assert_eq!(
        prepare["result"]["range"],
        json!({
            "start": {"line": expected_start.0, "character": expected_start.1},
            "end": {"line": expected_end.0, "character": expected_end.1},
        }),
        "prepareRename must cover exactly `name`: {prepare}"
    );

    // Rename edits exactly the declaration, the read, the setData segment and
    // the WXML token; nothing is fabricated and the quotes are kept.
    let rename = session.rename(&uri, name_position, "displayName");
    let script_edits = document_change_edits(&rename["result"], &script_uri, "E2E-05 rename");
    assert_eq!(
        script_edits.len(),
        3,
        "rename must edit the declaration, read and setData segment: {rename}"
    );
    for (start, context) in [
        (declaration, "declaration"),
        (read, "read"),
        (set_data, "setData segment"),
    ] {
        let position = native_lsp::position_at(script, start);
        let found = script_edits.iter().find(|edit| {
            edit["range"]["start"] == json!({"line": position.0, "character": position.1})
        });
        let edit = found.unwrap_or_else(|| panic!("{context} edit missing: {rename}"));
        assert_eq!(edit["newText"], json!("displayName"), "{context}: {rename}");
    }
    let wxml_edits = document_change_edits(&rename["result"], &uri, "E2E-05 rename");
    assert_eq!(
        wxml_edits.len(),
        1,
        "rename must edit exactly one WXML token: {rename}"
    );
    assert_eq!(
        wxml_edits[0]["range"]["start"],
        json!({"line": expected_start.0, "character": expected_start.1}),
        "{rename}"
    );
    assert_eq!(wxml_edits[0]["newText"], json!("displayName"), "{rename}");

    // Missing `onSave`: the quick fix inserts a handler into the Page options
    // object at the exact closing-brace offset.
    let on_save = token_position(wxml, "bindtap=\"onSave\"", "onSave");
    let on_save_end = native_lsp::position_at(wxml, wxml.find("onSave").expect("onSave") + 6);
    let actions = session.code_action(&uri, (on_save, on_save_end));
    let actions = actions["result"]
        .as_array()
        .unwrap_or_else(|| panic!("codeAction returned no array: {actions}"));
    assert_eq!(
        actions.len(),
        1,
        "expected one missing-handler fix: {actions:?}"
    );
    assert_eq!(
        actions[0]["title"],
        json!("Create event handler 'onSave'"),
        "{actions:?}"
    );
    assert_eq!(actions[0]["kind"], json!("quickfix"), "{actions:?}");
    let fix_edits = document_change_edits(&actions[0]["edit"], &script_uri, "E2E-05 quick fix");
    assert_eq!(fix_edits.len(), 1, "one insertion edit: {actions:?}");
    let close = script.rfind("});").expect("options close");
    let close_position = native_lsp::position_at(script, close);
    assert_eq!(
        fix_edits[0]["range"]["start"],
        json!({"line": close_position.0, "character": close_position.1}),
        "the insertion must sit on the options closing brace: {actions:?}"
    );
    assert_eq!(
        fix_edits[0]["range"]["end"], fix_edits[0]["range"]["start"],
        "the quick fix must be a zero-width insertion: {actions:?}"
    );
    let insertion = fix_edits[0]["newText"].as_str().expect("insertion text");
    assert!(insertion.contains("onSave()"), "{insertion}");
    assert!(
        !insertion.contains("methods: {"),
        "a Page handler must not add a Component methods object: {insertion}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-11: Behavior data and properties join the component instance type.
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_component_behavior_data_and_properties() {
    let Some(gate) = NativeGate::require("G-P5-E2E-11") else {
        return;
    };
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    gate.write("pages/index/index.wxml", "<card />");
    gate.write(
        "pages/index/index.json",
        r#"{"usingComponents":{"card":"/components/card/card"}}"#,
    );
    gate.write("pages/index/index.ts", "Page({ data: {} });");
    gate.write("components/card/card.json", r#"{"component":true}"#);
    gate.write(
        "components/card/card.ts",
        "Component({
  behaviors: [Behavior({
    data: {
      /** 共享计数 */
      sharedCount: 3,
    },
    properties: {
      cardTitle: { type: String, optionalTypes: [Number] },
    },
  })],
  data: {},
});",
    );
    let wxml_text = "{{ sharedCount }}{{ cardTitle }}";
    gate.write("components/card/card.wxml", wxml_text);
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&gate);
    let uri = gate.uri("components/card/card.wxml");
    let diagnostics = session.open_wxml(&uri, wxml_text, 1);
    assert_eq!(
        diagnostics,
        json!([]),
        "a valid behavior-backed component must publish no diagnostics: {diagnostics}"
    );

    let shared = native_lsp::lsp_position(wxml_text, "sharedCount");
    let hover = session.hover(&uri, shared);
    let hover_text = native_lsp::hover_markdown("G-P5-E2E-11", "behavior data hover", &hover);
    assert!(
        hover_text.contains("number"),
        "sharedCount must be number: {}",
        session.describe(&hover)
    );
    assert!(
        hover_text.contains("共享计数"),
        "sharedCount hover must include its JSDoc: {}",
        session.describe(&hover)
    );

    let definition = session.definition(&uri, shared);
    assert_definition_covers(
        "G-P5-E2E-11",
        &gate,
        &definition,
        "components/card/card.ts",
        "sharedCount",
    );

    let title = native_lsp::lsp_position(wxml_text, "cardTitle");
    let title_hover = session.hover(&uri, title);
    let title_text =
        native_lsp::hover_markdown("G-P5-E2E-11", "optionalTypes property hover", &title_hover);
    assert!(
        title_text.contains("string | number"),
        "optionalTypes must compose string | number: {}",
        session.describe(&title_hover)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-16: discriminator unions, guards and exact null ranges.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "keeps the frozen AC-006 hover and diagnostic assertions together"
)]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ac006_conditional_union_narrowing() {
    let Some(gate) = NativeGate::require("G-P5-E2E-16") else {
        return;
    };
    copy_fixture(&gate, "conditional-union");
    let text = native_lsp::read_project_file(&gate, "pages/index/index.wxml");

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_no_status_diagnostic(&diagnostics, "AC-006 conditional union");

    for (marker, target, type_text, documentation) in [
        (
            "就绪用户：{{ state.user.name }}",
            "state.user.name",
            "string",
            "用户显示名称",
        ),
        ("错误码：{{ state.code }}", "state.code", "number", "错误码"),
        (
            "守卫内：{{ maybeUser.name }}",
            "maybeUser.name",
            "string",
            "用户显示名称",
        ),
    ] {
        let hover = hover_target_end(&mut session, &uri, &text, marker, target);
        let contents = native_lsp::hover_markdown("G-P5-E2E-16", target, &hover);
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

    let errors = error_diagnostics(&diagnostics);
    assert_eq!(
        errors.len(),
        2,
        "the fixture has exactly two negatives: {diagnostics}"
    );
    assert_error_at(
        &diagnostics,
        &text,
        "非法年龄：{{ state.user.age }}",
        "age",
        "AC-006 discriminator miss",
    );
    assert_error_at(
        &diagnostics,
        &text,
        "守卫外：{{ maybeUser.name }}",
        "maybeUser",
        "AC-006 outside-guard null",
    );
    assert!(
        errors.iter().any(|error| error["message"]
            .as_str()
            .is_some_and(|message| message.contains("null"))),
        "the outside-guard error must name null: {diagnostics}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

// ---------------------------------------------------------------------------
// G-P5-PERF-01 / G-P5-STABILITY-01 native baselines.
//
// Both gates drive the real `--stdio --backend native` package binary over
// JSON-RPC with the pinned 7.1.0-beta engine. Latency is measured around the
// client call, so diagnostics include the server's 120 ms debounce and the real
// engine update. `G-P5-PERF-01` is a release-only baseline; a debug build is
// rejected instead of being compared with plan section 9.5 budgets. Resource
// sampling reads only `/proc` and never starts `ps`, `top` or Node.
// ---------------------------------------------------------------------------

/// Measured samples per performance metric (plan section 9.5 minimum).
#[cfg(target_os = "linux")]
const PERF_SAMPLES: usize = 200;
/// Untimed warm-up calls per performance metric.
#[cfg(target_os = "linux")]
const PERF_WARMUP: usize = 5;
/// Hot type-hover budget from plan section 9.5.
#[cfg(target_os = "linux")]
const HOVER_P95_MS: f64 = 200.0;
/// Hot expression-member completion budget from plan section 9.5.
#[cfg(target_os = "linux")]
const COMPLETION_P95_MS: f64 = 200.0;
/// Stop-typing diagnostics budget from plan section 9.5.
#[cfg(target_os = "linux")]
const DIAGNOSTICS_P95_MS: f64 = 500.0;
/// Steady RSS growth cap from plan section 9.5.
#[cfg(target_os = "linux")]
const RSS_GROWTH_PERCENT_MAX: f64 = 15.0;

/// Open/update/release/restart cycles for the stability gate (10 minimum).
#[cfg(target_os = "linux")]
const STABILITY_CYCLES: usize = 12;
/// Bounded hover burst after the cycles.
#[cfg(target_os = "linux")]
const STABILITY_BURST: usize = 16;

#[cfg(target_os = "linux")]
const PERF_WXML: &str = "{{ user.name }} {{ title }}";
#[cfg(target_os = "linux")]
const PERF_COMPLETE_WXML: &str = "<view>{{ user. }}</view>";
#[cfg(target_os = "linux")]
const PERF_SCRIPT: &str = "Page({ data: { user: { name: 'Ada', age: 36 }, title: 'Hello' } });";

#[cfg(target_os = "linux")]
fn perf_page_text(iteration: usize) -> String {
    format!("{PERF_WXML}\n<!-- perf sample {iteration} -->\n")
}

#[cfg(target_os = "linux")]
fn write_perf_fixture(gate: &NativeGate) {
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/complete/complete"],"usingComponents":{}}"#,
    );
    gate.write("pages/index/index.wxml", PERF_WXML);
    gate.write("pages/index/index.ts", PERF_SCRIPT);
    gate.write("pages/complete/complete.wxml", PERF_COMPLETE_WXML);
    gate.write("pages/complete/complete.ts", PERF_SCRIPT);
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
}

#[cfg(target_os = "linux")]
fn require_hover_type(
    session: &mut Session,
    uri: &str,
    position: (u32, u32),
    expected: &str,
    context: &str,
) {
    let response = session.hover(uri, position);
    let text = native_lsp::hover_markdown("native hover", context, &response);
    assert!(
        text.contains(expected),
        "{context}: hover must contain {expected:?}: {}",
        session.describe(&response)
    );
}

#[cfg(target_os = "linux")]
fn require_completion(
    session: &mut Session,
    uri: &str,
    position: (u32, u32),
    expected: &str,
    context: &str,
) {
    let response = session.completion(uri, position);
    assert!(
        completion_labels(&response).contains(&expected),
        "{context}: completion must contain {expected:?}: {}",
        session.describe(&response)
    );
}

/// G-P5-PERF-01: native cold start, hover, diagnostics and completion budgets.
#[cfg(target_os = "linux")]
#[test]
#[allow(
    clippy::assertions_on_constants,
    reason = "the release profile is a constant prerequisite of the baseline"
)]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_q2_performance_benchmark() {
    let Some(gate) = NativeGate::require("G-P5-PERF-01") else {
        return;
    };
    assert!(
        !cfg!(debug_assertions),
        "G-P5-PERF-01 is a release baseline: run `cargo test -p wxml-lsp --test native_parity --release ... --ignored`; \
         a debug build cannot be compared with the plan section 9.5 budgets"
    );
    write_perf_fixture(&gate);

    let index_uri = gate.uri("pages/index/index.wxml");
    let complete_uri = gate.uri("pages/complete/complete.wxml");
    let name_position = native_lsp::lsp_position(PERF_WXML, "name");
    let member = native_lsp::lsp_position(PERF_COMPLETE_WXML, "user.");
    let member_position = (member.0, member.1 + 5);

    let started = Instant::now();
    let mut session = Session::start(&gate);
    let initialize_ms = elapsed_ms(started);

    let open_started = Instant::now();
    let diagnostics = session.open_wxml(&index_uri, PERF_WXML, 1);
    let first_page_ms = elapsed_ms(open_started);
    assert_eq!(
        diagnostics,
        json!([]),
        "the performance page must start clean: {diagnostics}"
    );
    let cold_open_total_ms = elapsed_ms(started);

    let first_hover_started = Instant::now();
    require_hover_type(
        &mut session,
        &index_uri,
        name_position,
        "string",
        "cold first hover",
    );
    let first_engine_hover_ms = elapsed_ms(first_hover_started);

    session.open_wxml(&complete_uri, PERF_COMPLETE_WXML, 1);

    let mut version = 1_u64;
    for iteration in 0..PERF_WARMUP {
        require_hover_type(
            &mut session,
            &index_uri,
            name_position,
            "string",
            "hover warm-up",
        );
        require_completion(
            &mut session,
            &complete_uri,
            member_position,
            "name",
            "completion warm-up",
        );
        version += 1;
        let text = perf_page_text(iteration);
        let diagnostics = session.change_wxml(&index_uri, &text, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "diagnostics warm-up must stay clean: {diagnostics}"
        );
    }

    let baseline = sample_tree(session.pid());
    assert!(
        baseline.rss_kib > 0,
        "the native server tree must expose a non-zero RSS baseline"
    );
    let mut points = vec![baseline.to_json("baseline")];
    let parent_rss_start = rss_kib(std::process::id()).unwrap_or(0);

    let mut hover_samples = Vec::with_capacity(PERF_SAMPLES);
    for sample in 0..PERF_SAMPLES {
        let started = Instant::now();
        require_hover_type(
            &mut session,
            &index_uri,
            name_position,
            "string",
            "hot hover",
        );
        hover_samples.push(elapsed_ms(started));
        if sample % 50 == 0 {
            push_point(&mut points, &session, "hover");
        }
    }

    let mut completion_samples = Vec::with_capacity(PERF_SAMPLES);
    for sample in 0..PERF_SAMPLES {
        let started = Instant::now();
        require_completion(
            &mut session,
            &complete_uri,
            member_position,
            "name",
            "hot completion",
        );
        completion_samples.push(elapsed_ms(started));
        if sample % 50 == 0 {
            push_point(&mut points, &session, "completion");
        }
    }

    let mut diagnostics_samples = Vec::with_capacity(PERF_SAMPLES);
    for sample in 0..PERF_SAMPLES {
        version += 1;
        let text = perf_page_text(PERF_WARMUP + sample);
        let started = Instant::now();
        let diagnostics = session.change_wxml(&index_uri, &text, version);
        diagnostics_samples.push(elapsed_ms(started));
        assert_eq!(
            diagnostics,
            json!([]),
            "a clean edit must publish no diagnostics: {diagnostics}"
        );
        if sample % 25 == 0 {
            push_point(&mut points, &session, "diagnostics");
        }
    }

    let final_sample = sample_tree(session.pid());
    points.push(final_sample.to_json("final"));
    let parent_rss_end = rss_kib(std::process::id()).unwrap_or(0);

    let engines = engine_children(session.pid(), gate.package_root());
    assert_eq!(
        engines.len(),
        1,
        "the performance run must keep exactly one engine child: {engines:?}"
    );
    assert!(
        session.descendant_pids().len() <= 1,
        "the performance run leaked child processes: {:?}",
        session.descendant_pids()
    );
    session.assert_no_node_exec();

    let hover = Stats::from_samples(&hover_samples);
    let completion = Stats::from_samples(&completion_samples);
    let diagnostics = Stats::from_samples(&diagnostics_samples);
    let mut failures = Vec::new();
    check_p95(&mut failures, "hover", &hover, HOVER_P95_MS);
    check_p95(&mut failures, "completion", &completion, COMPLETION_P95_MS);
    check_p95(
        &mut failures,
        "diagnostics",
        &diagnostics,
        DIAGNOSTICS_P95_MS,
    );
    let rss_growth = growth_percent(baseline.rss_kib, final_sample.rss_kib);
    println!(
        "server tree RSS: baseline {} KiB, final {} KiB, growth {rss_growth:.2}%",
        baseline.rss_kib, final_sample.rss_kib
    );
    if !rss_growth.is_finite() {
        failures.push("server tree RSS baseline is zero".to_owned());
    } else if rss_growth > RSS_GROWTH_PERCENT_MAX {
        failures.push(format!(
            "server tree RSS grew {rss_growth:.2}% (cap {RSS_GROWTH_PERCENT_MAX:.0}%)"
        ));
    }

    let cpu_seconds = u64_to_f64(final_sample.cpu_ticks.saturating_sub(baseline.cpu_ticks)) / 100.0;
    let report = json!({
        "gate": "G-P5-PERF-01",
        "harness": "native-stdio-perf",
        "git_head": git_head(&ledger::workspace_root()),
        "engine": {
            "version": ENGINE_VERSION,
            "git_head": PinnedEngine::pinned().git_head(),
            "ast_protocol": AST_PROTOCOL_VERSION,
            "executable_sha256": PinnedEngine::pinned().executable_sha256(),
        },
        "environment": {
            "transport": "--stdio --backend native",
            "profile": "release (debug is rejected by the gate)",
            "samples_per_metric": PERF_SAMPLES,
            "warmup_per_metric": PERF_WARMUP,
            "diagnostics_debounce_included_ms": 120,
            "cpus": std::thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get),
        },
        "cold_start": {
            "spawn_initialize_ms": round_ms(initialize_ms),
            "open_and_first_diagnostics_ms": round_ms(first_page_ms),
            "first_page_total_ms": round_ms(cold_open_total_ms),
            "first_engine_hover_ms": round_ms(first_engine_hover_ms),
            "budget": "recorded only; plan section 9.5 defines no cold-start p95",
        },
        "metrics": {
            "hover": hover.to_json(),
            "completion": completion.to_json(),
            "diagnostics": diagnostics.to_json(),
        },
        "budgets": {
            "hover_p95_ms": HOVER_P95_MS,
            "completion_p95_ms": COMPLETION_P95_MS,
            "diagnostics_p95_ms": DIAGNOSTICS_P95_MS,
            "rss_growth_percent_max": RSS_GROWTH_PERCENT_MAX,
        },
        "resources": {
            "parent_rss_kib_start": parent_rss_start,
            "parent_rss_kib_end": parent_rss_end,
            "server_tree_rss_kib_baseline": baseline.rss_kib,
            "server_tree_rss_kib_final": final_sample.rss_kib,
            "server_tree_rss_growth_percent": round_ms(rss_growth),
            "server_tree_cpu_ticks_delta": final_sample.cpu_ticks.saturating_sub(baseline.cpu_ticks),
            "server_tree_cpu_seconds_at_100hz": round_ms(cpu_seconds),
            "engine_processes": engines.len(),
            "timeline": points,
        },
        "thresholds": {
            "pass": failures.is_empty(),
            "failures": failures,
        },
        "notes": [
            "Latency is measured around the JSON-RPC client call and includes serialization and server queueing.",
            "Diagnostics samples include the default 120 ms stop-typing debounce and one real engine update.",
            "Cold start is recorded, not asserted; only plan section 9.5 p95 and growth caps are enforced.",
            "RSS is the summed VmRSS of the server tree sampled from /proc with a hard process cap.",
            "No Node process, Node shim or external sampler is used.",
        ],
    });
    let path = write_report("WXML_NATIVE_PERF_OUT", "perf", &report);
    println!("native performance report: {}", path.display());
    println!(
        "hover {} | completion {} | diagnostics {}",
        hover.to_json(),
        completion.to_json(),
        diagnostics.to_json()
    );
    assert!(
        report["thresholds"]["pass"].as_bool() == Some(true),
        "native performance budgets failed: {}\nreport: {}",
        report["thresholds"]["failures"],
        path.display()
    );

    let pid = session.pid();
    let descendants_before = session.descendant_pids();
    let evidence = session.shutdown_with_evidence();
    assert!(
        !evidence.stderr.contains("detaching it"),
        "server detached a worker on shutdown; stderr:\n{}",
        evidence.stderr
    );
    wait_gone(&[pid], "server after perf shutdown");
    wait_gone(&descendants_before, "engine after perf shutdown");
    gate.assert_no_native_sandbox();
    gate.tripwire().assert_absent();
}

#[cfg(target_os = "linux")]
fn push_point(points: &mut Vec<Value>, session: &Session, label: &str) {
    let sample = sample_tree(session.pid());
    if points.len() < MAX_POINTS {
        points.push(sample.to_json(label));
    } else {
        *points.last_mut().expect("timeline") = sample.to_json(label);
    }
}

#[cfg(target_os = "linux")]
fn round_ms(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(target_os = "linux")]
fn stability_page_text(round: usize, stage: &str) -> String {
    format!("{{{{ user.name }}}} <!-- cycle {round} {stage} -->\n")
}

#[cfg(target_os = "linux")]
fn write_stability_fixture(gate: &NativeGate) {
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    gate.write("pages/index/index.wxml", &stability_page_text(0, "initial"));
    gate.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
}

#[cfg(target_os = "linux")]
fn hover_class(response: &Value) -> String {
    response.get("error").map_or_else(
        || {
            if native_lsp::hover_is_empty(response) {
                "empty".to_owned()
            } else {
                "contents".to_owned()
            }
        },
        |error| {
            format!(
                "error: {}",
                error["message"].as_str().unwrap_or("unknown error")
            )
        },
    )
}

/// Requests hover until it contains `expected` or the attempts are exhausted.
#[cfg(target_os = "linux")]
fn hover_until_type(
    session: &mut Session,
    uri: &str,
    position: (u32, u32),
    expected: &str,
    attempts: usize,
) -> Value {
    let mut last = Value::Null;
    for _ in 0..attempts {
        last = session.hover(uri, position);
        if last["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains(expected))
        {
            return last;
        }
    }
    last
}

/// G-P5-STABILITY-01: 12 update/release/restart cycles over native stdio.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "native gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn native_ten_round_lifecycle_stability() {
    let Some(gate) = NativeGate::require("G-P5-STABILITY-01") else {
        return;
    };
    write_stability_fixture(&gate);

    let uri = gate.uri("pages/index/index.wxml");
    let script_uri = gate.uri("pages/index/index.ts");
    let mut initial_text = stability_page_text(0, "initial");
    let name_position = native_lsp::lsp_position(&initial_text, "name");

    let mut session = Session::start(&gate);
    let mut version = 1_u64;
    let diagnostics = session.open_wxml(&uri, &initial_text, version);
    assert_eq!(
        diagnostics,
        json!([]),
        "the stability page must start clean: {diagnostics}"
    );
    require_hover_type(
        &mut session,
        &uri,
        name_position,
        "string",
        "initial disk hover",
    );

    let cpu_start = sample_tree(session.pid()).cpu_ticks;
    let mut rounds = Vec::new();
    let mut points = Vec::new();
    let mut baseline_rss = 0_u64;
    let mut cycle_final_rss = 0_u64;
    let mut first_request_classes = Vec::new();

    for round in 0..STABILITY_CYCLES {
        let round_started = Instant::now();

        // Three real template updates; every diagnostics answer must be for the
        // requested version and must stay clean.
        for step in 0..3 {
            version += 1;
            initial_text = stability_page_text(round, &format!("update {step}"));
            let diagnostics = session.change_wxml(&uri, &initial_text, version);
            assert_eq!(
                diagnostics,
                json!([]),
                "round {round}: update {step} must stay clean: {diagnostics}"
            );
        }

        // Unsaved script overlay: `name` becomes a number. The overlay must be
        // visible immediately and must survive the forced engine restart.
        let overlay = format!("Page({{ data: {{ user: {{ name: {round} }} }} }});");
        session.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": script_uri,
                    "languageId": "typescript",
                    "version": u64::try_from(round).expect("round fits u64") + 1,
                    "text": overlay,
                }
            }),
        );
        version += 1;
        initial_text = stability_page_text(round, "overlay");
        let diagnostics = session.change_wxml(&uri, &initial_text, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "round {round}: overlay diagnostics must stay clean: {diagnostics}"
        );
        require_hover_type(
            &mut session,
            &uri,
            name_position,
            "number",
            "overlay hover before restart",
        );

        // Force a native restart by killing the engine child. This gate
        // records the first request class; the dedicated E2E-01 gate asserts
        // transparent first-request replay. The second request must start a
        // new session and replay the open overlay.
        let engines = engine_children(session.pid(), gate.package_root());
        assert_eq!(
            engines.len(),
            1,
            "round {round}: expected one engine before restart: {engines:?}"
        );
        let old_engine = engines[0];
        kill_process(old_engine);
        wait_gone(&[old_engine], "killed engine");
        let first = session.hover(&uri, name_position);
        let first_class = hover_class(&first);
        first_request_classes.push(first_class.clone());
        let replay = hover_until_type(&mut session, &uri, name_position, "number", 3);
        let replay_text =
            native_lsp::hover_markdown("G-P5-STABILITY-01", "restart replay", &replay);
        assert!(
            replay_text.contains("number"),
            "round {round}: overlay was not replayed after restart (first request: {first_class}): {}",
            session.describe(&replay)
        );
        let new_engines = engine_children(session.pid(), gate.package_root());
        assert_eq!(
            new_engines.len(),
            1,
            "round {round}: restart did not leave one live engine: {new_engines:?}"
        );
        assert_ne!(
            new_engines[0], old_engine,
            "round {round}: restart reused the killed engine process"
        );

        // Release the overlay: disk `name` is a string again.
        session.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": script_uri}}),
        );
        version += 1;
        initial_text = stability_page_text(round, "released");
        let diagnostics = session.change_wxml(&uri, &initial_text, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "round {round}: release diagnostics must stay clean: {diagnostics}"
        );
        require_hover_type(
            &mut session,
            &uri,
            name_position,
            "string",
            "disk hover after release",
        );

        // Live snapshot / process accounting: one engine child, no growth in
        // process count, and a bounded timeline sample.
        let children = session.descendant_pids();
        assert!(
            children.len() <= 1,
            "round {round}: leaked child processes: {children:?}"
        );
        let sample = sample_tree(session.pid());
        assert!(
            sample.rss_kib > 0,
            "round {round}: server tree RSS must be non-zero"
        );
        if round == 0 {
            baseline_rss = sample.rss_kib;
        }
        cycle_final_rss = sample.rss_kib;
        let growth = growth_percent(baseline_rss, sample.rss_kib);
        rounds.push(json!({
            "round": round,
            "version": version,
            "wall_ms": round_ms(elapsed_ms(round_started)),
            "restart_first_request": first_class,
            "rss_kib": sample.rss_kib,
            "rss_growth_from_round0_percent": round_ms(growth),
            "threads": sample.threads,
            "processes": sample.pids.len(),
            "engine_pids": new_engines,
        }));
        if points.len() < MAX_POINTS {
            points.push(sample.to_json("cycle"));
        }
        println!(
            "stability round {round}: v{version} rss={} KiB growth={growth:.2}% first={first_class} wall={:.1} ms",
            sample.rss_kib,
            elapsed_ms(round_started)
        );
    }

    // Bounded burst: the queue must not reject a normal hover burst after all
    // restarts, and every answer must still see the disk string.
    for _ in 0..STABILITY_BURST {
        require_hover_type(
            &mut session,
            &uri,
            name_position,
            "string",
            "post-cycle hover burst",
        );
    }

    version += 1;
    let final_text = stability_page_text(STABILITY_CYCLES, "final");
    let diagnostics = session.change_wxml(&uri, &final_text, version);
    assert_eq!(
        diagnostics,
        json!([]),
        "the final diagnostics answer must be clean: {diagnostics}"
    );

    // Settle the allocator with more real updates before the steady-state
    // sample. The burst above is a queue check; sampling every update shows
    // whether memory plateaus (allocator retention) or grows per edit (leak).
    let mut settle_samples = Vec::new();
    let mut settle_rss = Vec::new();
    for settle in 0..8 {
        version += 1;
        let text = stability_page_text(STABILITY_CYCLES, &format!("settle {settle}"));
        let diagnostics = session.change_wxml(&uri, &text, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "settle {settle} diagnostics must stay clean: {diagnostics}"
        );
        let sample = sample_tree(session.pid());
        println!(
            "settle {settle}: rss={} KiB threads={}",
            sample.rss_kib, sample.threads
        );
        settle_rss.push(sample.rss_kib);
        settle_samples.push(json!({
            "settle": settle,
            "rss_kib": sample.rss_kib,
            "threads": sample.threads,
        }));
    }

    let final_sample = sample_tree(session.pid());
    let final_engines = engine_children(session.pid(), gate.package_root());
    assert_eq!(
        final_engines.len(),
        1,
        "the final cycle must keep exactly one engine: {final_engines:?}"
    );
    points.push(final_sample.to_json("final"));

    let cycle_growth = growth_percent(baseline_rss, cycle_final_rss);
    let steady_baseline = settle_rss.get(1).copied().unwrap_or(0);
    let steady_tail = median(&settle_rss.iter().rev().take(3).copied().collect::<Vec<_>>());
    let steady_growth = growth_percent(steady_baseline, steady_tail);
    let cold_to_steady_growth = growth_percent(baseline_rss, steady_tail);
    let cpu_ticks = final_sample.cpu_ticks.saturating_sub(cpu_start);
    let mut failures = Vec::new();
    if baseline_rss == 0 {
        failures.push("round-0 RSS baseline is zero".to_owned());
    } else if cycle_growth > RSS_GROWTH_PERCENT_MAX {
        failures.push(format!(
            "cycle RSS grew {cycle_growth:.2}% across {STABILITY_CYCLES} cycles (cap {RSS_GROWTH_PERCENT_MAX:.0}%)"
        ));
    }
    if steady_baseline == 0 {
        failures.push("steady RSS baseline is zero".to_owned());
    } else if steady_growth > RSS_GROWTH_PERCENT_MAX {
        failures.push(format!(
            "steady RSS grew {steady_growth:.2}% across 8 edits after warm-up (cap {RSS_GROWTH_PERCENT_MAX:.0}%)"
        ));
    }

    let pid = session.pid();
    let descendants_before = session.descendant_pids();
    let evidence = session.shutdown_with_evidence();
    let detached = evidence.stderr.contains("detaching it");
    if detached {
        failures.push("server detached a scheduler worker on shutdown".to_owned());
    }
    wait_gone(&[pid], "server after stability shutdown");
    wait_gone(&descendants_before, "engine after stability shutdown");
    gate.assert_no_native_sandbox();
    gate.tripwire().assert_absent();

    let report = json!({
        "gate": "G-P5-STABILITY-01",
        "harness": "native-stdio-stability",
        "git_head": git_head(&ledger::workspace_root()),
        "engine": {
            "version": ENGINE_VERSION,
            "git_head": PinnedEngine::pinned().git_head(),
            "ast_protocol": AST_PROTOCOL_VERSION,
            "executable_sha256": PinnedEngine::pinned().executable_sha256(),
        },
        "environment": {
            "transport": "--stdio --backend native",
            "cycles": STABILITY_CYCLES,
            "updates_per_cycle": 3,
            "burst_hovers": STABILITY_BURST,
            "restart": "SIGKILL the engine child; the first request class is recorded and the second request must replay the open overlay",
        },
        "restart": {
            "first_request_classes": first_request_classes,
            "e2e_01_status": "gate_green in G-P5-E2E-01: native_non_owner_overlay_replay_survives_restart asserts transparent first-request replay; this stability gate records the observed class and requires the second request to replay the open overlay",
        },
        "rounds": rounds,
        "resources": {
            "rss_kib_round0": baseline_rss,
            "rss_kib_cycle11": cycle_final_rss,
            "cycle_rss_growth_percent": round_ms(cycle_growth),
            "rss_kib_steady_baseline": steady_baseline,
            "rss_kib_steady_tail_median": steady_tail,
            "steady_rss_growth_percent": round_ms(steady_growth),
            "cold_to_steady_rss_growth_percent": round_ms(cold_to_steady_growth),
            "rss_kib_final": final_sample.rss_kib,
            "cpu_ticks": cpu_ticks,
            "cpu_seconds_at_100hz": round_ms(u64_to_f64(cpu_ticks) / 100.0),
            "engine_processes": final_engines.len(),
            "settle_samples": settle_samples,
            "timeline": points,
        },
        "shutdown": {
            "server_pid": evidence.pid,
            "elapsed_ms": round_ms(evidence.elapsed.as_secs_f64() * 1000.0),
            "detached_worker": detached,
            "stderr_bytes": evidence.stderr.len(),
            "sandbox_left": false,
            "processes_left": 0,
        },
        "thresholds": {
            "cycle_rss_growth_percent_max": RSS_GROWTH_PERCENT_MAX,
            "steady_rss_growth_percent_max": RSS_GROWTH_PERCENT_MAX,
            "pass": failures.is_empty(),
            "failures": failures,
        },
        "notes": [
            "Every cycle performs three template updates, an unsaved script overlay, a forced engine restart, an overlay release and a diagnostics round-trip.",
            "The first hover after SIGKILL is observed here; transparent first-request replay is asserted by the dedicated E2E-01 gate, not by this stability budget.",
            "The second request must open a fresh engine and replay the unsaved overlay; a reused PID fails the gate.",
            "The steady RSS sample follows the hover burst and eight settle edits; the cap is applied to the post-warm-up steady baseline, not to the one-time cold working-set step.",
            "The cold-to-steady step is recorded as evidence so it is visible and cannot hide a later sustained increase.",
            "Shutdown must reap the server and engine, leave no sandbox and never log a detached scheduler worker.",
            "RSS is the summed VmRSS of the server tree sampled from /proc with a hard process cap.",
        ],
    });
    let path = write_report("WXML_NATIVE_STABILITY_OUT", "stability", &report);
    println!("native stability report: {}", path.display());
    assert!(
        report["thresholds"]["pass"].as_bool() == Some(true),
        "native stability budgets failed: {}\nreport: {}",
        report["thresholds"]["failures"],
        path.display()
    );
}
