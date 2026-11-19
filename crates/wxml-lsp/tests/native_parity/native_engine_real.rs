//! First-wave `enable-now-native` gates from
//! `docs/evidence/2026-11-18-tsgo-p6-gate-conversion-plan.md`.
//!
//! These are the exact native replacements for:
//!
//! - `G-P5-E2E-09` / `ac011_unknown_owner_degrades_honestly`
//! - `G-P5-E2E-12` / `static_wxs_and_template_semantics_work_over_stdio`
//! - `G-P5-E2E-14` / `unsaved_wxs_overlay_updates_static_hover_and_completion`
//!
//! Every test is `#[ignore]`: the default workspace job reports it ignored,
//! never passed. The required native job runs them with `--ignored` and
//! `WXML_REQUIRE_REAL_ENGINE=1`; a missing package/engine is then fatal.
//! Without `WXML_REQUIRE_REAL_ENGINE` a missing package is an explicit,
//! printed optional-mode skip.

use serde_json::{Value, json};

use crate::native_lsp::{self, Session};
use crate::native_package::NativeGate;

/// Workspace-relative file that declares the ignored native gates.
pub(crate) const NATIVE_GATE_FILE: &str =
    "crates/wxml-lsp/tests/native_parity/native_engine_real.rs";

/// Ledger gate id -> first-wave native replacement test.
///
/// All three are `gate_green` in the ledger with the frozen opt-in evidence;
/// this map keeps the exact gate/replacement link checked by `native_gates.rs`.
pub(crate) const GATE_REPLACEMENTS: &[(&str, &str)] = &[
    (
        "G-P5-E2E-09",
        "native_ac011_unknown_owner_degrades_honestly",
    ),
    (
        "G-P5-E2E-12",
        "native_static_wxs_and_template_semantics_work_over_stdio",
    ),
    (
        "G-P5-E2E-14",
        "native_unsaved_wxs_overlay_updates_static_hover_and_completion",
    ),
];

/// The exact ignored-native-gate set for the first wave.
///
/// `ignored_functions()` in `native_gates.rs` subtracts this list from the
/// scanned `#[ignore]` set and still requires the remainder to equal the
/// ledger's 23 old gates exactly.
pub(crate) const IGNORED_GATE_TESTS: &[(&str, &str)] = &[
    (
        NATIVE_GATE_FILE,
        "native_ac011_unknown_owner_degrades_honestly",
    ),
    (
        NATIVE_GATE_FILE,
        "native_static_wxs_and_template_semantics_work_over_stdio",
    ),
    (
        NATIVE_GATE_FILE,
        "native_unsaved_wxs_overlay_updates_static_hover_and_completion",
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
