//! Opt-in native-engine LSP gate for event-handler hover and definition.
//!
//! The gate reuses the `documentSymbol` fixture (`support::provision`): it
//! builds the disposable package from the pinned `TSGO_BIN`, replaces the
//! sample project with four pages, starts `<package>/bin/wxml-lsp --stdio
//! --backend native` and drives real `textDocument/hover` and
//! `textDocument/definition` requests.
//!
//! It proves the two native paths the change requires:
//!
//! - a `Component` behavior imported through the tsconfig `paths` alias and
//!   through a relative path resolves `this.<handler>` to the behavior
//!   declaration and its `JSDoc`;
//! - a `Page` own method resolves the same way, while a handler provided only
//!   by a `Page` behavior degrades to an empty hover and definition instead of
//!   fabricating a type, signature or location.
//!
//! Without `TSGO_BIN` the fixture prints an explicit `SKIP` reason through
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

use serde_json::{Value, json};
use support::{Session, provision};

const APP_JSON: &str = concat!(
    "{\"pages\":[",
    "\"pages/alias/index\",",
    "\"pages/relative/index\",",
    "\"pages/page/index\",",
    "\"pages/pagebehavior/index\"",
    "]}\n",
);
/// The pinned engine is TypeScript 7.1.0-beta, where `paths` targets are
/// tsconfig-relative and `baseUrl` no longer exists.
const TSCONFIG: &str = "{\"compilerOptions\":{\"paths\":{\"@/*\":[\"./src/*\"]}}}\n";
/// Behavior with a documented handler.
///
/// The `JSDoc` must be its own leading line: with the pinned engine the
/// single-line `methods: { /** ... */ onBackTap() {} }` form loses the
/// documentation on the `this.onBackTap` hover, while the same comment on its
/// own line is reported verbatim. The fixture therefore documents the handler
/// without weakening the hover assertion.
const BASE_BEHAVIOR: &str = concat!(
    "export default Behavior({\n",
    "  methods: {\n",
    "    /** 导航栏上的后退或Home按钮点击时触发 */\n",
    "    onBackTap(e: WechatMiniprogram.CustomEvent<never, never, { groupId: string }>) ",
    "{ void e; }\n",
    "  }\n",
    "});\n",
);
const ALIAS_SCRIPT: &str = concat!(
    "import BasePage from '@/behaviors/base';\n",
    "Component({ behaviors: [BasePage], methods: { onLoad() {} } });\n",
);
const RELATIVE_SCRIPT: &str = concat!(
    "import BasePage from '../../src/behaviors/base';\n",
    "Component({ behaviors: [BasePage], methods: { onLoad() {} } });\n",
);
const PAGE_SCRIPT: &str = concat!(
    "Page({ ",
    "/** 页面点击时触发 */ ",
    "onTap(e: WechatMiniprogram.TouchEvent) { void e; } });\n",
);
const PAGE_BEHAVIOR_SCRIPT: &str = concat!(
    "import BasePage from '@/behaviors/base';\n",
    "Page({ behaviors: [BasePage] });\n",
);

const ALIAS_WXML: &str = "<view bind:click-left=\"onBackTap\" />\n";
const PAGE_WXML: &str = "<view bindtap=\"onTap\" />\n";

const BEHAVIOR_JSDOC: &str = "导航栏上的后退或Home按钮点击时触发";
const PAGE_JSDOC: &str = "页面点击时触发";
const BEHAVIOR_FILE: &str = "src/behaviors/base.ts";

/// UTF-16 position inside the first `onBackTap` token in the WXML above.
const POINT: (u64, u64) = (0, 24);
/// UTF-16 position inside the first `onTap` token in the WXML above.
const PAGE_POINT: (u64, u64) = (0, 16);

#[test]
fn real_engine_event_handler_hover_and_definition_use_native_types() {
    let Some(fixture) = provision("event-hover") else {
        return;
    };
    prepare_project(fixture.project());

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    let alias = fixture.uri("pages/alias/index.wxml");
    let relative = fixture.uri("pages/relative/index.wxml");
    let page = fixture.uri("pages/page/index.wxml");
    let page_behavior = fixture.uri("pages/pagebehavior/index.wxml");
    session.did_open(&alias, "wxml", ALIAS_WXML);
    session.did_open(&relative, "wxml", ALIAS_WXML);
    session.did_open(&page, "wxml", PAGE_WXML);
    session.did_open(&page_behavior, "wxml", ALIAS_WXML);

    let (line, character) = POINT;
    // (a) Component + tsconfig `paths` alias behavior.
    let hover = session.hover(&alias, line, character);
    println!("alias behavior hover: {hover}");
    assert_hover(&hover, ALIAS_WXML, "onBackTap", BEHAVIOR_JSDOC);
    let definition = session.definition(&alias, line, character);
    println!("alias behavior definition: {definition}");
    assert_definition(
        &definition,
        BEHAVIOR_FILE,
        BASE_BEHAVIOR,
        "onBackTap",
        "alias behavior",
    );

    // (b) Component + relative behavior path.
    let hover = session.hover(&relative, line, character);
    println!("relative behavior hover: {hover}");
    assert_hover(&hover, ALIAS_WXML, "onBackTap", BEHAVIOR_JSDOC);
    let definition = session.definition(&relative, line, character);
    println!("relative behavior definition: {definition}");
    assert_definition(
        &definition,
        BEHAVIOR_FILE,
        BASE_BEHAVIOR,
        "onBackTap",
        "relative behavior",
    );

    // (c) Page own method.
    let (page_line, page_character) = PAGE_POINT;
    let hover = session.hover(&page, page_line, page_character);
    println!("page method hover: {hover}");
    assert_hover(&hover, PAGE_WXML, "onTap", PAGE_JSDOC);
    let definition = session.definition(&page, page_line, page_character);
    println!("page method definition: {definition}");
    assert_definition(
        &definition,
        "pages/page/index.ts",
        PAGE_SCRIPT,
        "onTap",
        "page method",
    );

    // (d) A Page handler supplied only by a behavior is not an own instance
    // member. `Page.Instance` does not merge behavior methods, so the native
    // query has no symbol, type or declaration to publish and the adapter
    // degrades both hover and definition to an empty result.
    let hover = session.hover(&page_behavior, line, character);
    println!("page behavior hover: {hover}");
    assert_eq!(
        hover["result"],
        Value::Null,
        "a Page behavior handler must not fabricate hover evidence: {hover}"
    );
    let definition = session.definition(&page_behavior, line, character);
    println!("page behavior definition: {definition}");
    assert_eq!(
        definition["result"],
        Value::Null,
        "a Page behavior handler must not fabricate a definition: {definition}"
    );

    session.shutdown();
    fixture.tripwire().assert_absent();
}

/// Replaces the shared fixture project with this gate's four pages.
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
        (BEHAVIOR_FILE, BASE_BEHAVIOR),
        ("pages/alias/index.ts", ALIAS_SCRIPT),
        ("pages/alias/index.wxml", ALIAS_WXML),
        ("pages/relative/index.ts", RELATIVE_SCRIPT),
        ("pages/relative/index.wxml", ALIAS_WXML),
        ("pages/page/index.ts", PAGE_SCRIPT),
        ("pages/page/index.wxml", PAGE_WXML),
        ("pages/pagebehavior/index.ts", PAGE_BEHAVIOR_SCRIPT),
        ("pages/pagebehavior/index.wxml", ALIAS_WXML),
    ] {
        let path = project.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
        }
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
}

/// Asserts one non-empty native hover whose range is the handler token.
///
/// The composed Markdown is the frozen `ts` type fence plus the `JSDoc`
/// comment; the backend deliberately never repeats the symbol name, so the
/// handler token itself is proven by the exact range and by the definition
/// target below.
fn assert_hover(response: &Value, document: &str, token: &str, jsdoc: &str) {
    assert!(response.get("error").is_none(), "hover failed: {response}");
    let result = &response["result"];
    assert!(!result.is_null(), "hover must not be empty: {response}");
    let markdown = result["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("hover has no Markdown contents: {response}"));
    assert!(
        markdown.contains(jsdoc),
        "hover must carry the handler JSDoc {jsdoc:?}: {response}"
    );
    assert!(
        markdown.contains("```ts"),
        "hover must carry the native handler type: {response}"
    );
    assert_eq!(
        result["range"],
        token_range(document, token),
        "the hover range must cover exactly the {token} token: {response}"
    );
}

/// Asserts one definition target file plus its exact handler-name token.
fn assert_definition(response: &Value, path: &str, script: &str, token: &str, label: &str) {
    assert!(
        response.get("error").is_none(),
        "{label} definition failed: {response}"
    );
    let locations = response["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{label} definition must be a location array: {response}"));
    assert_eq!(locations.len(), 1, "{label} definition: {response}");
    let uri = locations[0]["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("{label} location has no URI: {response}"));
    assert!(
        uri.ends_with(path),
        "{label} definition must target {path}: {response}"
    );
    assert_eq!(
        slice_range(script, &locations[0]["range"]),
        token,
        "{label} definition must cover exactly the {token} declaration token: {response}"
    );
}

/// Returns the JSON `Range` of the first `token` occurrence in `text`.
fn token_range(text: &str, token: &str) -> Value {
    let (line, character) = token_position(text, token);
    json!({
        "start": {"line": line, "character": character},
        "end": {
            "line": line,
            "character": character + utf16_len(token),
        },
    })
}

/// Returns the UTF-16 `(line, character)` of the first `token` occurrence.
fn token_position(text: &str, token: &str) -> (u64, u64) {
    let byte = text
        .find(token)
        .unwrap_or_else(|| panic!("{token} is not in the test text"));
    let mut line = 0_u64;
    let mut character = 0_u64;
    for value in text[..byte].chars() {
        if value == '\n' {
            line += 1;
            character = 0;
        } else {
            character += u64::try_from(value.len_utf16()).expect("UTF-16 width fits");
        }
    }
    (line, character)
}

/// Returns the source text covered by one UTF-16 JSON `Range`.
fn slice_range<'a>(text: &'a str, range: &Value) -> &'a str {
    let start = byte_at(text, &range["start"]["line"], &range["start"]["character"]);
    let end = byte_at(text, &range["end"]["line"], &range["end"]["character"]);
    &text[start..end]
}

/// Converts one UTF-16 line/character position to a byte offset.
fn byte_at(text: &str, line: &Value, character: &Value) -> usize {
    let line = usize::try_from(line.as_u64().expect("line")).expect("line fits");
    let character =
        usize::try_from(character.as_u64().expect("character")).expect("character fits");
    let mut bytes = 0_usize;
    for (index, current) in text.split('\n').enumerate() {
        if index == line {
            let mut units = 0_usize;
            for value in current.chars() {
                if units == character {
                    return bytes;
                }
                units += value.len_utf16();
                bytes += value.len_utf8();
            }
            assert_eq!(units, character, "character is outside the line: {text:?}");
            return bytes;
        }
        bytes += current.len() + 1;
    }
    panic!("line is outside the text: {text:?}");
}

/// Returns the UTF-16 length of `text`.
fn utf16_len(text: &str) -> u64 {
    u64::try_from(text.encode_utf16().count()).expect("UTF-16 length fits")
}
