#![allow(
    dead_code,
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

//! P7 stdio gate shells over the real `wxml-lsp --stdio --backend native` binary.
//!
//! These nine tests keep the frozen `G-P5-E2E-06..14` gate coordinates, but the
//! old Node process boundary is replaced by the disposable native package from
//! `native_parity/native_package.rs`. Each shell starts one fresh server over
//! pipes, asserts the gate's signature answer, and requires a clean
//! `shutdown`/`exit`. The deep oracle for every gate stays in
//! `native_parity/native_engine_real.rs`; this file deliberately does not
//! duplicate it.
//!
//! Every test is ignored by default and is run by `scripts/native-gates.sh`:
//!
//! ```bash
//! WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
//! TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
//! TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
//! TSGO_TYPINGS_ROOT=backends/typescript/node_modules/miniprogram-api-typings \
//! cargo test -p wxml-lsp --test stdio_type_loop -- --ignored --test-threads=1
//! ```
//!
//! Running an ignored shell without `WXML_TEST_NATIVE=1` is a hard failure, not
//! a silent skip. Required mode additionally makes a missing package fatal.

use serde_json::{Value, json};

#[path = "native_parity/native_gate_env.rs"]
mod native_gate_env;
#[path = "native_parity/native_lsp.rs"]
mod native_lsp;
#[path = "native_parity/native_package.rs"]
mod native_package;

/// The harness modules only need the workspace root from this crate's ledger.
mod ledger {
    use std::path::PathBuf;

    /// Returns the nearest ancestor containing the workspace `Cargo.toml`.
    pub(crate) fn workspace_root() -> PathBuf {
        let start = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for ancestor in start.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            if text.lines().any(|line| line.trim() == "[workspace]") {
                return ancestor.to_path_buf();
            }
        }
        panic!("workspace root not found above {}", start.display());
    }
}

use native_lsp::{Session, definition_locations, hover_is_empty, hover_markdown, lsp_position};
use native_package::NativeGate;

/// Writes the AC-001 page fixture used by gates 06, 07, 08 and 09.
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

fn error_diagnostics(diagnostics: &Value) -> Vec<&Value> {
    diagnostics
        .as_array()
        .unwrap_or_else(|| panic!("diagnostics are not an array: {diagnostics}"))
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect()
}

/// G-P5-E2E-06: page data hover keeps its `JSDoc` signature.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn ac001_page_data_and_jsdoc_hover() {
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

    let name = session.hover(&uri, lsp_position(wxml, "name"));
    let name_text = hover_markdown("G-P5-E2E-06", "user.name hover", &name);
    assert!(
        name_text.contains("string"),
        "user.name must be string: {}",
        session.describe(&name)
    );
    assert!(
        name_text.contains("用户显示名"),
        "user.name hover must include the JSDoc text: {}",
        session.describe(&name)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-07: one exact typo diagnostic, cleared by `didChange`.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn ac002_field_typo_diagnostic_and_fix_clear() {
    let Some(gate) = NativeGate::require("G-P5-E2E-07") else {
        return;
    };
    let broken = "{{ user.nmae }}";
    write_page_fixture(&gate, broken);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, broken, 1);
    let errors = error_diagnostics(&diagnostics);
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
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn ac003_unicode_and_crlf_positions() {
    let Some(gate) = NativeGate::require("G-P5-E2E-08") else {
        return;
    };
    let wxml = "标题😀 {{ user.nmae }}\r\n第二行 {{ user.nmae }}\r\n";
    write_page_fixture(&gate, wxml);

    let mut session = Session::start(&gate);
    let uri = gate.uri("pages/index/index.wxml");
    let diagnostics = session.open_wxml(&uri, wxml, 1);
    let errors = error_diagnostics(&diagnostics);
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

/// G-P5-E2E-09: an ownerless template degrades honestly.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn ac011_unknown_owner_degrades_honestly() {
    let Some(gate) = NativeGate::require("G-P5-E2E-09") else {
        return;
    };
    write_page_fixture(&gate, "{{ user.name }}");
    let orphan = "{{ unknown.value }}";
    gate.write("orphan.wxml", orphan);

    let mut session = Session::start(&gate);
    let uri = gate.uri("orphan.wxml");
    let diagnostics = session.open_wxml(&uri, orphan, 1);
    let warning = diagnostics
        .as_array()
        .expect("diagnostics array")
        .iter()
        .find(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is"))
        })
        .unwrap_or_else(|| {
            panic!("an unknown owner must publish a degradation warning: {diagnostics}")
        });
    assert!(
        warning["message"].as_str().is_some_and(
            |message| message.contains("unsupported") || message.contains("unavailable")
        ),
        "{warning}"
    );

    let hover = session.hover(&uri, lsp_position(orphan, "value"));
    assert!(
        hover_is_empty(&hover),
        "an unknown owner must not fabricate a hover type: {}",
        session.describe(&hover)
    );
    assert!(
        error_diagnostics(&diagnostics).is_empty(),
        "an unknown owner must not fabricate a type diagnostic: {diagnostics}"
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-10: a component property hover keeps the honest Partial reason.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn ac013_component_properties_hover() {
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
    assert!(
        error_diagnostics(&diagnostics).is_empty(),
        "a declared component property must not produce a type error: {diagnostics}"
    );
    let status = diagnostics
        .as_array()
        .expect("diagnostics array")
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

    let hover = session.hover(&uri, lsp_position(text, "title"));
    let hover_text = hover_markdown("G-P5-E2E-10", "component property hover", &hover);
    assert!(
        hover_text.contains("string"),
        "component property title must be string: {}",
        session.describe(&hover)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-11: Behavior data and properties join the component instance type.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn component_behavior_data_and_properties_work_over_stdio() {
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

    let shared = lsp_position(wxml_text, "sharedCount");
    let hover = session.hover(&uri, shared);
    let hover_text = hover_markdown("G-P5-E2E-11", "behavior data hover", &hover);
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
    let locations = definition_locations("G-P5-E2E-11", "behavior data definition", &definition);
    assert_eq!(
        locations[0]["uri"],
        json!(gate.uri("components/card/card.ts")),
        "{definition}"
    );

    let title = lsp_position(wxml_text, "cardTitle");
    let title_hover = session.hover(&uri, title);
    let title_text = hover_markdown("G-P5-E2E-11", "optionalTypes property hover", &title_hover);
    assert!(
        title_text.contains("string | number"),
        "optionalTypes must compose string | number: {}",
        session.describe(&title_hover)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-12: static WXS/template hover, definition and completion.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn static_wxs_and_template_semantics_work_over_stdio() {
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

    let format = lsp_position(wxml_text, "format(pageTitle)");
    let hover = session.hover(&uri, (format.0, format.1 + 1));
    let hover_text = hover_markdown("G-P5-E2E-12", "WXS function hover", &hover);
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
    let locations = definition_locations("G-P5-E2E-12", "WXS function definition", &definition);
    assert_eq!(locations[0]["uri"], json!(uri), "{definition}");
    assert_eq!(
        locations[0]["range"]["start"]["line"],
        json!(1),
        "{definition}"
    );

    let card = lsp_position(wxml_text, "is=\"card\"");
    let template_hover = session.hover(&uri, (card.0, card.1 + 4));
    let template_text = hover_markdown("G-P5-E2E-12", "template hover", &template_hover);
    assert!(
        template_text.contains("template `card`"),
        "{}",
        session.describe(&template_hover)
    );

    let incomplete_is = wxml_text.replace("is=\"shared\"", "is=\"sh\"");
    session.change_wxml(&uri, &incomplete_is, 2);
    let is_position = lsp_position(&incomplete_is, "\"sh\"");
    let completion = session.completion(&uri, (is_position.0, is_position.1 + 3));
    assert!(
        completion_labels(&completion).contains(&"shared"),
        "{}",
        session.describe(&completion)
    );

    let incomplete_member = wxml_text.replace("utils.format(pageTitle)", "utils.");
    session.change_wxml(&uri, &incomplete_member, 3);
    let member_position = lsp_position(&incomplete_member, "utils.");
    let member_completion = session.completion(&uri, (member_position.0, member_position.1 + 6));
    assert!(
        completion_labels(&member_completion).contains(&"format"),
        "{}",
        session.describe(&member_completion)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-13: static WXS answers survive an unavailable type backend.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn static_wxs_answers_survive_an_unavailable_type_backend() {
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

    // The package engine is verified before launch; delete it after startup so
    // the static layer survives while the native type session cannot start.
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
        .expect("diagnostics array")
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

    let format = lsp_position(wxml_text, "format(title)");
    let hover = session.hover(&uri, (format.0, format.1 + 1));
    let hover_text = hover_markdown("G-P5-E2E-13", "static WXS hover", &hover);
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
    let locations = definition_locations("G-P5-E2E-13", "static WXS definition", &definition);
    assert!(
        !locations.is_empty(),
        "static WXS definition must survive an unavailable type backend: {definition}"
    );

    let typed = lsp_position(wxml_text, "{{ title }}");
    let typed_hover = session.hover(&uri, (typed.0, typed.1 + 3));
    assert!(
        hover_is_empty(&typed_hover),
        "an unavailable type backend must not fabricate a typed hover: {}",
        session.describe(&typed_hover)
    );

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}

/// G-P5-E2E-14: an unsaved WXS overlay replaces disk facts and restores on close.
#[test]
#[ignore = "native stdio gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn unsaved_wxs_overlay_updates_static_hover_and_completion() {
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

    let mut session = Session::start(&gate);
    let wxml = gate.uri("pages/index/index.wxml");
    let wxs = gate.uri("pages/index/u.wxs");
    session.open_wxml(&wxml, wxml_text, 1);

    let label = lsp_position(wxml_text, "label");
    let disk_hover = session.hover(&wxml, (label.0, label.1 + 1));
    let disk_text = hover_markdown("G-P5-E2E-14", "disk WXS hover", &disk_hover);
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
    let renamed_text = hover_markdown("G-P5-E2E-14", "overlay WXS hover", &renamed_hover);
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
    let restored_text = hover_markdown("G-P5-E2E-14", "restored WXS hover", &restored);
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

    session.assert_no_node_exec();
    session.shutdown();
    gate.tripwire().assert_absent();
}
