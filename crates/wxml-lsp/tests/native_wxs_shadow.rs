//! Opt-in native-engine LSP gate for a static WXS module used from a template.
//!
//! The gate reuses the shared `documentSymbol` fixture (`support::provision`):
//! it builds the disposable typings-only package from `TSGO_TYPINGS_ROOT`, replaces the
//! sample project with one page that imports a `src`-backed WXS module, starts
//! `<package>/bin/wxml-lsp --stdio --backend native` and drives real
//! `textDocument/publishDiagnostics` and `textDocument/hover` traffic.
//!
//! It proves both halves of the `wxs-alias-and-shadow-roots` change on a live
//! session:
//!
//! - a static `<wxs src=... module="m1">` root used inside a mustache is a
//!   shadow local value, not a page `data` field, so the native type check no
//!   longer publishes `Property 'm1' does not exist` for the template;
//! - `module.exports = { getLimitSettingText: getLimitSettingText }` keeps the
//!   local function's kind and signature, so hovering `m1` reports the exported
//!   parameter names instead of a bare value type.
//!
//! Other template diagnostics are tolerated: the gate asserts the absence of
//! the specific WXS-root false positive, not an empty list.
//!
//! Without `TSGO_TYPINGS_ROOT` the fixture prints an explicit `SKIP` reason through
//! [`provision`] and returns `None`; `WXML_REQUIRE_REAL_ENGINE=1|true` turns a
//! missing input into a panic. Only a *missing* input is skippable.

#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines
)]

#[path = "native_lsp_document_symbols/support.rs"]
// The shared fixture/client module is compiled into several test binaries and
// each uses a different subset of it.
#[allow(dead_code, unused_imports)]
mod support;

use std::fs;
use std::path::Path;

use support::{Session, provision};

const APP_JSON: &str = "{\"pages\":[\"pages/wxs/index\"]}\n";
const TSCONFIG: &str = "{\"compilerOptions\":{}}\n";

const PAGE_SCRIPT: &str = concat!(
    "Component({\n",
    "  data: { formData: { attendLimit: 1, userLimit: 2 } },\n",
    "  methods: {}\n",
    "});\n",
);

const WXS_SOURCE: &str = concat!(
    "function getLimitSettingText(attendLimitSetting, userAppointLimit) {\n",
    "  if (userAppointLimit === 0) return '不限制';\n",
    "  return attendLimitSetting > 0 ? '限制' : '不限制';\n",
    "}\n",
    "module.exports = { getLimitSettingText: getLimitSettingText };\n",
);

const WXML: &str = concat!(
    "<wxs src=\"./index.wxs\" module=\"m1\"></wxs>\n",
    "<view>{{ m1.getLimitSettingText(formData.attendLimit, formData.userLimit) }}</view>\n",
);

/// The mustache prefix whose end starts the module token. The hover point is
/// the byte after this marker, i.e. the `m` in `m1`.
const MUSTACHE_MARKER: &str = "{{ m1";
/// The two parameter names the aliased export must keep.
const SIGNATURE_PARAMETERS: [&str; 2] = ["attendLimitSetting", "userAppointLimit"];

#[test]
fn real_engine_wxs_module_root_has_no_missing_property_and_alias_hover_keeps_parameters() {
    let Some(fixture) = provision("wxs-shadow") else {
        return;
    };
    prepare_project(fixture.project());

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        serde_json::json!("utf-16"),
        "{initialized}"
    );

    let wxml_uri = fixture.uri("pages/wxs/index.wxml");
    session.did_open(&wxml_uri, "wxml", WXML);

    // (1) The static WXS module root is a shadow local, not a data field: the
    // generated probe must not ask the engine for `__wxml_root.m1`.
    let published = session.wait_for_diagnostics(&[&wxml_uri], support::REQUEST_TIMEOUT);
    let publication = published
        .iter()
        .find(|message| message["params"]["uri"] == serde_json::json!(wxml_uri))
        .unwrap_or_else(|| panic!("no template diagnostics publication: {published:?}"));
    let diagnostics = publication["params"]["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("template diagnostics must be an array: {publication}"));
    println!("wxml diagnostics: {diagnostics:?}");
    for diagnostic in diagnostics {
        let message_text = diagnostic["message"].as_str().unwrap_or_default();
        assert!(
            !message_text.contains("Property 'm1' does not exist"),
            "the static WXS module root must not be reported as a missing property: {diagnostic}"
        );
        assert!(
            !(message_text.contains("'m1'") && message_text.contains("does not exist")),
            "a diagnostic must not quote 'm1' as a missing property: {diagnostic}"
        );
    }

    // (2) The aliased export keeps the local function signature; the hover
    // point is the `m1` token inside the mustache, not the declaration.
    let byte = WXML
        .find(MUSTACHE_MARKER)
        .unwrap_or_else(|| panic!("{MUSTACHE_MARKER:?} is not in the template"))
        + MUSTACHE_MARKER.len();
    let (line, character) = line_character(WXML, byte);
    let hover = session.hover(&wxml_uri, line, character);
    println!("wxs module hover: {hover}");
    assert!(hover.get("error").is_none(), "hover failed: {hover}");
    let result = &hover["result"];
    assert!(!result.is_null(), "hover must not be empty: {hover}");
    let markdown = result["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("hover has no Markdown contents: {hover}"));
    for parameter in SIGNATURE_PARAMETERS {
        assert!(
            markdown.contains(parameter),
            "hover must keep the aliased function parameter {parameter:?}: {hover}"
        );
    }
    assert!(
        markdown.contains("=> string"),
        "hover must keep the aliased function return type: {hover}"
    );
    assert!(
        markdown.contains("(attendLimitSetting: any, userAppointLimit: any) => string"),
        "hover must keep the aliased function signature: {hover}"
    );

    session.shutdown();
    fixture.tripwire().assert_absent();
}

/// Replaces the shared fixture project with this gate's single WXS page.
///
/// The package fixture is reused unchanged; only its disposable project is
/// rewritten. The sample page is removed so the native program contains only
/// this gate's scripts.
fn prepare_project(project: &Path) {
    let sample = project.join("pages/index");
    if sample.exists() {
        fs::remove_dir_all(&sample)
            .unwrap_or_else(|error| panic!("remove {}: {error}", sample.display()));
    }
    for (relative, text) in [
        ("app.json", APP_JSON),
        ("tsconfig.json", TSCONFIG),
        ("pages/wxs/index.ts", PAGE_SCRIPT),
        ("pages/wxs/index.wxs", WXS_SOURCE),
        ("pages/wxs/index.wxml", WXML),
    ] {
        let path = project.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
        }
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
}

/// Converts a byte offset in `text` into an LSP UTF-16 `(line, character)`.
///
/// The template is ASCII, so byte counting would already be correct; this
/// helper keeps the conversion explicit and UTF-16 safe for any later edit.
fn line_character(text: &str, byte: usize) -> (u64, u64) {
    let mut line = 0_u64;
    let mut character = 0_u64;
    for (index, value) in text.char_indices() {
        if index == byte {
            break;
        }
        assert!(index < byte, "byte offset is not a char boundary: {byte}");
        if value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += u64::try_from(value.len_utf16()).expect("UTF-16 width fits");
        }
    }
    (line, character)
}
