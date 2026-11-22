//! Opt-in native-engine LSP matrix for engine-checked component properties.
//!
//! The matrix reuses the `documentSymbol` fixture (`support::provision`): it
//! builds the disposable typings-only package from `TSRS_TYPINGS_ROOT`,
//! replaces the sample project with a page matrix plus one declared custom
//! component, starts `<package>/bin/wxml-lsp --stdio --backend native` and
//! drives real `textDocument/publishDiagnostics` traffic.
//!
//! It proves the Wave 4 property contract surface of the
//! `engine-checked-component-bindings` change:
//!
//! - a global `declare global { interface WxmlComponentMap { ... } }` map and
//!   a module-level `export interface WxmlComponentMap` map both drive the
//!   engine assignment check (the module map through a type-level
//!   `import('...').WxmlComponentMap` reference);
//! - `string`, `string[]`, object fields, unions, conditionals, indexing,
//!   arithmetic, generics, mapped types and literal unions report exactly one
//!   `2322`/`2345` inside the bound attribute expression, never on the
//!   generated `__wxml_bind_` name;
//! - a structurally compatible object literal with extra fields stays clean
//!   (TS `2353` is never published for a WXML binding);
//! - `any` stays supported and clean, `unknown` degrades to `Partial` without a
//!   type error;
//! - CJK text, CRLF line endings and a multi-line mustache keep the anchor on
//!   the trimmed expression, matching the existing UTF-16 mapping.
//!
//! Without `TSRS_TYPINGS_ROOT` the fixture prints an explicit `SKIP` reason and
//! returns `None`; `WXML_REQUIRE_REAL_ENGINE=1|true` turns a missing input into
//! a panic. Only a *missing* input is skippable.

#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines
)]

#[path = "native_lsp_document_symbols/support.rs"]
#[allow(dead_code, unused_imports)]
mod support;

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use support::{Session, provision};

const TSCONFIG: &str = "{}\n";
const CARD_SCRIPT: &str = "Component({ properties: {} });\n";
const CARD_WXML: &str = "<view />\n";
const EMPTY_JSON: &str = "{}\n";

/// The component declaration used by the global-map test.
///
/// `card` carries the types outside the `wxml-project` `TypeText` subset
/// (`Box<T>`, `Model` references, `Pick<Model, 'id'>`); the contract probe
/// references the declared member itself, so only the real engine decides
/// assignability. `plain-card` keeps every member inside the subset so a fully
/// checked page can stay `Supported`.
const GLOBAL_DECLARATION: &str = concat!(
    "declare global {\n",
    "  interface WxmlComponentMap {\n",
    "    card: CardDeclaration;\n",
    "    'plain-card': PlainCardDeclaration;\n",
    "  }\n",
    "\n",
    "  interface Box<T> {\n",
    "    value: T;\n",
    "  }\n",
    "\n",
    "  interface Model {\n",
    "    id: number;\n",
    "    name: string;\n",
    "  }\n",
    "\n",
    "  interface CardDeclaration {\n",
    "    props: {\n",
    "      text: string;\n",
    "      labels: string[];\n",
    "      model: Model;\n",
    "      kind: 'a' | 'b';\n",
    "      box: Box<number>;\n",
    "      picked: Pick<Model, 'id'>;\n",
    "      size: 1 | 2;\n",
    "      conditional: string;\n",
    "      indexed: string;\n",
    "      sum: string;\n",
    "    };\n",
    "  }\n",
    "\n",
    "  interface PlainCardDeclaration {\n",
    "    props: {\n",
    "      label: string;\n",
    "      loose: any;\n",
    "      model: { id: number; name: string };\n",
    "      missing: { id: number; name: string };\n",
    "      mystery: unknown;\n",
    "    };\n",
    "  }\n",
    "}\n",
    "\n",
    "export {};\n",
);

const MATRIX_APP_JSON: &str = concat!(
    "{\"pages\":[",
    "\"pages/matrix/index\",",
    "\"pages/clean/index\",",
    "\"pages/unknown/index\"",
    "],\"usingComponents\":{",
    "\"card\":\"/components/card/card\",",
    "\"plain-card\":\"/components/card/card\"",
    "}}\n",
);

/// Every page script stays a module (`export {}`), so identical data names in
/// different pages do not become duplicate global declarations.
const MATRIX_SCRIPT: &str = concat!(
    "export {};\n",
    "\n",
    "const userCount: number = 7;\n",
    "const mixed: (string | number)[] = ['ok', 1];\n",
    "const badModel: { id: string; name: string } = { id: 'wrong', name: 'Card' };\n",
    "const badKind = 'c' as const;\n",
    "const boxValue: Box<string> = { value: 'text' };\n",
    "const pickedValue = { id: 'bad' };\n",
    "const pageSize: number = 3;\n",
    "const flag: boolean = true;\n",
    "const nums: number[] = [1, 2];\n",
    "const count: number = 2;\n",
    "Page({\n",
    "  data: {\n",
    "    userCount: userCount,\n",
    "    mixed: mixed,\n",
    "    badModel: badModel,\n",
    "    badKind: badKind,\n",
    "    boxValue: boxValue,\n",
    "    pickedValue: pickedValue,\n",
    "    pageSize: pageSize,\n",
    "    flag: flag,\n",
    "    nums: nums,\n",
    "    count: count,\n",
    "  },\n",
    "});\n",
);

const MATRIX_WXML: &str = concat!(
    "<card\n",
    "  text=\"{{ userCount }}\"\n",
    "  labels=\"{{ mixed }}\"\n",
    "  model=\"{{ badModel }}\"\n",
    "  kind=\"{{ badKind }}\"\n",
    "  box=\"{{ boxValue }}\"\n",
    "  picked=\"{{ pickedValue }}\"\n",
    "  size=\"{{ pageSize }}\"\n",
    "  conditional=\"{{ flag ? 'ok' : 2 }}\"\n",
    "  indexed=\"{{ nums[0] }}\"\n",
    "  sum=\"{{ count + 1 }}\"\n",
    "/>\n",
);

/// Every binding whose engine verdict must be one `2322`/`2345`.
const MATRIX_EXPRESSIONS: [&str; 10] = [
    "userCount",
    "mixed",
    "badModel",
    "badKind",
    "boxValue",
    "pickedValue",
    "pageSize",
    "flag ? 'ok' : 2",
    "nums[0]",
    "count + 1",
];

const CLEAN_SCRIPT: &str = concat!(
    "export {};\n",
    "\n",
    "const label: string = 'ok';\n",
    "const userCount: number = 7;\n",
    "Page({\n",
    "  data: { label: label, userCount: userCount },\n",
    "});\n",
);

/// The first `model` literal is structurally compatible and only carries an
/// extra field (TS `2353`, which WXML drops); the second one binds the same
/// declared member with a wrong field type (TS `2322`), proving the literal
/// form is actually checked rather than silently skipped.
const CLEAN_WXML: &str = concat!(
    "<plain-card label=\"{{ label }}\" ",
    "model=\"{{ { id: 1, name: 'Card', extra: true } }}\" />\n",
    "<plain-card model=\"{{ { id: 'bad', name: 'Card' } }}\" ",
    "loose=\"{{ userCount }}\" />\n",
);

const CLEAN_EXPRESSIONS: [&str; 1] = ["{ id: 'bad', name: 'Card' }"];

const UNKNOWN_SCRIPT: &str = concat!(
    "export {};\n",
    "\n",
    "const userCount: number = 7;\n",
    "Page({\n",
    "  data: { userCount: userCount },\n",
    "});\n",
);

const UNKNOWN_WXML: &str = "<plain-card mystery=\"{{ userCount }}\" />\n";

const MODULE_APP_JSON: &str = "{\"pages\":[\"pages/module/index\"],\"usingComponents\":{\"card\":\"/components/card/card\"}}\n";

/// A module-level map: no `declare global`, so the probe must reference the
/// declaration file through `import('../../components').WxmlComponentMap`.
const MODULE_DECLARATION: &str = concat!(
    "export interface WxmlComponentMap {\n",
    "  card: CardDeclaration;\n",
    "}\n",
    "\n",
    "export interface Box<T> {\n",
    "  value: T;\n",
    "}\n",
    "\n",
    "export interface CardDeclaration {\n",
    "  props: {\n",
    "    text: string;\n",
    "    box: Box<number>;\n",
    "  };\n",
    "}\n",
);

const MODULE_SCRIPT: &str = concat!(
    "import type { Box } from '../../components';\n",
    "\n",
    "const userCount: number = 7;\n",
    "const boxValue: Box<string> = { value: 'text' };\n",
    "Page({\n",
    "  data: { userCount: userCount, boxValue: boxValue },\n",
    "});\n",
);

const MODULE_WXML: &str = "<card text=\"{{ userCount }}\" box=\"{{ boxValue }}\" />\n";

const MODULE_EXPRESSIONS: [&str; 2] = ["userCount", "boxValue"];

const ANCHOR_APP_JSON: &str = "{\"pages\":[\"pages/anchors/index\"],\"usingComponents\":{\"plain-card\":\"/components/card/card\"}}\n";

const ANCHOR_SCRIPT: &str = concat!(
    "export {};\n",
    "\n",
    "const 数量: number = 7;\n",
    "const 总计: number = 1;\n",
    "Page({\n",
    "  data: { 数量: 数量, 总计: 总计 },\n",
    "});\n",
);

/// CRLF line endings, CJK text before the first binding and one expression that
/// spans two lines. The anchors are the trimmed expression bytes, so the first
/// range starts after the CJK text and the second one covers the newline.
const ANCHOR_WXML: &str = concat!(
    "<view>中文标题</view><plain-card label=\"{{ 数量 }}\" />\r\n",
    "<plain-card label=\"{{ 总计 +\r\n",
    "  1 }}\" />\r\n",
);

const MISSING_APP_JSON: &str = "{\"pages\":[\"pages/missing/index\"],\"usingComponents\":{\"plain-card\":\"/components/card/card\"}}\n";

const MISSING_SCRIPT: &str = concat!(
    "export {};\n",
    "\n",
    "const missingModel = { name: 'Card' };\n",
    "Page({\n",
    "  data: { missingModel: missingModel },\n",
    "});\n",
);

const MISSING_WXML: &str = "<plain-card missing=\"{{ missingModel }}\" />\n";

#[test]
fn global_component_map_checks_property_contracts() {
    let Some(fixture) = provision("engine-property-global") else {
        return;
    };
    prepare_project(
        fixture.project(),
        &[
            ("app.json", MATRIX_APP_JSON),
            ("tsconfig.json", TSCONFIG),
            ("components.d.ts", GLOBAL_DECLARATION),
            ("components/card/card.wxml", CARD_WXML),
            ("components/card/card.json", EMPTY_JSON),
            ("components/card/card.ts", CARD_SCRIPT),
            ("pages/matrix/index.wxml", MATRIX_WXML),
            ("pages/matrix/index.json", EMPTY_JSON),
            ("pages/matrix/index.ts", MATRIX_SCRIPT),
            ("pages/clean/index.wxml", CLEAN_WXML),
            ("pages/clean/index.json", EMPTY_JSON),
            ("pages/clean/index.ts", CLEAN_SCRIPT),
            ("pages/unknown/index.wxml", UNKNOWN_WXML),
            ("pages/unknown/index.json", EMPTY_JSON),
            ("pages/unknown/index.ts", UNKNOWN_SCRIPT),
        ],
    );

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    // Each page is opened and awaited on its own: a concurrent open advances
    // the project revision and drops the pending diagnostics of the other
    // pages.
    let matrix = fixture.uri("pages/matrix/index.wxml");
    session.did_open(&matrix, "wxml", MATRIX_WXML);
    let matrix_publication = wait_for_page(&session, &matrix);
    println!("matrix publication: {matrix_publication:#?}");
    assert_contract_errors(&matrix_publication, MATRIX_WXML, &MATRIX_EXPRESSIONS);
    let mapped = diagnostic_for(&matrix_publication, MATRIX_WXML, "pickedValue");
    let message = mapped["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Pick<Model"),
        "the declared mapped type must keep its identity: {mapped}"
    );

    let clean = fixture.uri("pages/clean/index.wxml");
    session.did_open(&clean, "wxml", CLEAN_WXML);
    let clean_publication = wait_for_page(&session, &clean);
    println!("clean publication: {clean_publication:#?}");
    assert_contract_errors(&clean_publication, CLEAN_WXML, &CLEAN_EXPRESSIONS);
    assert!(
        !diagnostics(&clean_publication)
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("2353")),
        "a structurally compatible literal must not publish TS2353: {clean_publication}"
    );
    assert_supported(&clean_publication);

    let unknown = fixture.uri("pages/unknown/index.wxml");
    session.did_open(&unknown, "wxml", UNKNOWN_WXML);
    let unknown_publication = wait_for_page(&session, &unknown);
    println!("unknown declaration publication: {unknown_publication:#?}");
    assert_no_contract_error(&unknown_publication);
    let statuses = statuses(&unknown_publication);
    assert_eq!(
        statuses.len(),
        1,
        "an `unknown` expected type must degrade to Partial: {unknown_publication}"
    );
    let message = statuses[0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("unknown"),
        "the Partial reason must name the `unknown` expectation: {unknown_publication}"
    );

    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn module_component_map_is_referenced_through_a_type_import() {
    let Some(fixture) = provision("engine-property-module") else {
        return;
    };
    prepare_project(
        fixture.project(),
        &[
            ("app.json", MODULE_APP_JSON),
            ("tsconfig.json", TSCONFIG),
            ("components.d.ts", MODULE_DECLARATION),
            ("components/card/card.wxml", CARD_WXML),
            ("components/card/card.json", EMPTY_JSON),
            ("components/card/card.ts", CARD_SCRIPT),
            ("pages/module/index.wxml", MODULE_WXML),
            ("pages/module/index.json", EMPTY_JSON),
            ("pages/module/index.ts", MODULE_SCRIPT),
        ],
    );

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    let page = fixture.uri("pages/module/index.wxml");
    session.did_open(&page, "wxml", MODULE_WXML);
    let page_publication = wait_for_page(&session, &page);
    println!("module declaration publication: {page_publication:#?}");
    assert_contract_errors(&page_publication, MODULE_WXML, &MODULE_EXPRESSIONS);

    let generic = diagnostic_for(&page_publication, MODULE_WXML, "boxValue");
    let message = generic["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Box<number>"),
        "the module map must keep the declared generic identity: {generic}"
    );

    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn diagnostic_anchors_follow_utf16_mapping_for_cjk_crlf_and_multiline() {
    let Some(fixture) = provision("engine-property-anchors") else {
        return;
    };
    prepare_project(
        fixture.project(),
        &[
            ("app.json", ANCHOR_APP_JSON),
            ("tsconfig.json", TSCONFIG),
            ("components.d.ts", GLOBAL_DECLARATION),
            ("components/card/card.wxml", CARD_WXML),
            ("components/card/card.json", EMPTY_JSON),
            ("components/card/card.ts", CARD_SCRIPT),
            ("pages/anchors/index.wxml", ANCHOR_WXML),
            ("pages/anchors/index.json", EMPTY_JSON),
            ("pages/anchors/index.ts", ANCHOR_SCRIPT),
        ],
    );

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    let page = fixture.uri("pages/anchors/index.wxml");
    session.did_open(&page, "wxml", ANCHOR_WXML);
    let page_publication = wait_for_page(&session, &page);
    println!("anchor publication: {page_publication:#?}");
    assert_contract_errors(&page_publication, ANCHOR_WXML, &["数量", "总计 +\r\n  1"]);
    assert_supported(&page_publication);

    // The first anchor starts after the CJK prefix on the same line; the second
    // one spans the CRLF and stops at the trimmed `1`.
    let text = ANCHOR_WXML;
    let first = diagnostic_for(&page_publication, text, "数量");
    let start = text.find("数量").expect("CJK binding");
    assert_eq!(
        first["range"],
        range_json(text, start, start + "数量".len()),
        "the CJK anchor must use UTF-16 offsets, not UTF-8 bytes: {first}"
    );
    let second = diagnostic_for(&page_publication, text, "总计 +\r\n  1");
    let start = text.find("总计").expect("multi-line binding");
    assert_eq!(
        second["range"],
        range_json(text, start, start + "总计 +\r\n  1".len()),
        "the multi-line anchor must cover the trimmed expression only: {second}"
    );
    assert_eq!(
        second["range"]["start"]["line"],
        json!(1),
        "the multi-line anchor starts on the second CRLF line: {second}"
    );
    assert_eq!(
        second["range"]["end"]["line"],
        json!(2),
        "the multi-line anchor ends on the third CRLF line: {second}"
    );

    session.shutdown();
    fixture.tripwire().assert_absent();
}

/// A missing required object property is an assignment error too.
///
/// The engine reports this case with `2741` ("Property 'id' is missing ... but
/// required ..."), or `2739` when several properties are missing. Both are
/// assignment failures on the generated probe name, so the contract anchor must
/// publish them on the binding like `2322`/`2345`.
#[test]
fn missing_object_property_is_reported_on_the_binding() {
    let Some(fixture) = provision("engine-property-missing") else {
        return;
    };
    prepare_project(
        fixture.project(),
        &[
            ("app.json", MISSING_APP_JSON),
            ("tsconfig.json", TSCONFIG),
            ("components.d.ts", GLOBAL_DECLARATION),
            ("components/card/card.wxml", CARD_WXML),
            ("components/card/card.json", EMPTY_JSON),
            ("components/card/card.ts", CARD_SCRIPT),
            ("pages/missing/index.wxml", MISSING_WXML),
            ("pages/missing/index.json", EMPTY_JSON),
            ("pages/missing/index.ts", MISSING_SCRIPT),
        ],
    );

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    let page = fixture.uri("pages/missing/index.wxml");
    session.did_open(&page, "wxml", MISSING_WXML);
    let page_publication = wait_for_page(&session, &page);
    println!("missing property publication: {page_publication:#?}");
    assert_contract_errors(&page_publication, MISSING_WXML, &["missingModel"]);

    session.shutdown();
    fixture.tripwire().assert_absent();
}

/// Replaces the shared fixture project with one self-contained project.
fn prepare_project(project: &Path, files: &[(&str, &str)]) {
    let sample = project.join("pages/index");
    if sample.exists() {
        fs::remove_dir_all(&sample)
            .unwrap_or_else(|error| panic!("remove {}: {error}", sample.display()));
    }
    for (relative, text) in files {
        let path = project.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
        }
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
}

/// Opens one page and returns its first diagnostics publication.
fn wait_for_page(session: &Session, uri: &str) -> Value {
    session
        .wait_for_diagnostics(&[uri], support::REQUEST_TIMEOUT)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no diagnostics publication for {uri}"))
}

fn diagnostics(publication: &Value) -> &[Value] {
    publication["params"]["diagnostics"].as_array().map_or_else(
        || panic!("diagnostics must be an array: {publication}"),
        Vec::as_slice,
    )
}

fn is_contract_error(diagnostic: &Value) -> bool {
    matches!(
        diagnostic["code"].as_str(),
        Some("2322" | "2345" | "2739" | "2741")
    )
}

/// The `Partial` marker published by `wxml-lsp` itself starts with
/// `type checking is ...` and carries no diagnostic code.
fn is_status(diagnostic: &Value) -> bool {
    diagnostic["source"] == json!("wxml-lsp")
        && diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("type checking is "))
}

fn statuses(publication: &Value) -> Vec<&Value> {
    diagnostics(publication)
        .iter()
        .filter(|diagnostic| is_status(diagnostic))
        .collect()
}

/// Asserts the page is a complete, clean check (no `Partial` warning).
fn assert_supported(publication: &Value) {
    let statuses = statuses(publication);
    assert!(
        statuses.is_empty(),
        "a fully checked page must stay Supported: {publication}"
    );
}

/// Asserts no binding published an assignment diagnostic.
fn assert_no_contract_error(publication: &Value) {
    let errors: Vec<&Value> = diagnostics(publication)
        .iter()
        .filter(|diagnostic| is_contract_error(diagnostic))
        .collect();
    assert!(
        errors.is_empty(),
        "no binding may report an assignment error here: {errors:?}"
    );
}

/// Asserts one assignment diagnostic inside each expected binding expression.
///
/// The published range may be the whole trimmed expression (the engine reported
/// the generated probe name and the contract anchor re-owned it) or a precise
/// sub-range inside it (an error reported on the bound expression itself). Both
/// must lie inside the attribute expression, never on generated text.
fn assert_contract_errors(publication: &Value, text: &str, expected: &[&str]) {
    let all = diagnostics(publication);
    let errors: Vec<&Value> = all
        .iter()
        .filter(|diagnostic| is_contract_error(diagnostic))
        .collect();
    let unexpected: Vec<&Value> = all
        .iter()
        .filter(|diagnostic| !is_contract_error(diagnostic) && !is_status(diagnostic))
        .collect();
    println!(
        "contract diagnostics: {:?}",
        errors
            .iter()
            .map(|diagnostic| (
                diagnostic["code"].clone(),
                slice_range(text, &diagnostic["range"]).to_owned(),
                diagnostic["message"].clone()
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        unexpected.is_empty(),
        "only contract assignment errors are expected here: {unexpected:?}"
    );
    assert_eq!(
        errors.len(),
        expected.len(),
        "one diagnostic per binding, no duplicates: {publication}"
    );
    for expression in expected {
        let start = text
            .find(expression)
            .unwrap_or_else(|| panic!("{expression:?} is not in the template"));
        let end = start + expression.len();
        let hits: Vec<&&Value> = errors
            .iter()
            .filter(|diagnostic| range_within(text, &diagnostic["range"], start, end))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "the binding {expression:?} must report exactly one diagnostic inside the \
             attribute expression: {publication}"
        );
        let hit = hits[0];
        assert!(
            !slice_range(text, &hit["range"]).is_empty(),
            "the diagnostic range must not be empty: {hit}"
        );
    }
}

/// Returns the single contract diagnostic inside one binding expression.
fn diagnostic_for<'a>(publication: &'a Value, text: &str, expression: &str) -> &'a Value {
    let start = text
        .find(expression)
        .unwrap_or_else(|| panic!("{expression:?} is not in the template"));
    let end = start + expression.len();
    diagnostics(publication)
        .iter()
        .filter(|diagnostic| is_contract_error(diagnostic))
        .find(|diagnostic| range_within(text, &diagnostic["range"], start, end))
        .unwrap_or_else(|| panic!("no contract diagnostic inside {expression:?}: {publication}"))
}

/// Returns whether one UTF-16 LSP range lies inside `[start, end)` bytes.
fn range_within(text: &str, range: &Value, start: usize, end: usize) -> bool {
    let first = position_to_byte(text, &range["start"]);
    let last = position_to_byte(text, &range["end"]);
    start <= first && first <= last && last <= end
}

/// Builds the JSON range covering `[start, end)` bytes of `text`.
fn range_json(text: &str, start: usize, end: usize) -> Value {
    let (start_line, start_character) = byte_position(text, start);
    let (end_line, end_character) = byte_position(text, end);
    json!({
        "start": {"line": start_line, "character": start_character},
        "end": {"line": end_line, "character": end_character},
    })
}

/// Returns the source text covered by one UTF-16 JSON `Range`.
fn slice_range<'a>(text: &'a str, range: &Value) -> &'a str {
    let start = position_to_byte(text, &range["start"]);
    let end = position_to_byte(text, &range["end"]);
    &text[start..end]
}

/// Converts one UTF-16 line/character position to a byte offset.
///
/// `\r\n` is one line break and the `\r` has no LSP width, matching the
/// production conversion and the CRLF fixture.
fn position_to_byte(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().expect("line");
    let character = position["character"].as_u64().expect("character");
    let mut line_start = 0_usize;
    let mut current = 0_u64;
    while current < line {
        let offset = text[line_start..]
            .find('\n')
            .unwrap_or_else(|| panic!("line {line} is outside the text: {text:?}"));
        line_start += offset + 1;
        current += 1;
    }
    if character == 0 {
        return line_start;
    }
    let mut units = 0_u64;
    let mut byte = line_start;
    for value in text[line_start..].chars() {
        if value == '\n' {
            break;
        }
        byte += value.len_utf8();
        if value != '\r' {
            units += u64::try_from(value.len_utf16()).expect("UTF-16 width fits");
        }
        if units == character {
            return byte;
        }
    }
    assert_eq!(
        units, character,
        "character {character} is outside line {line}: {text:?}"
    );
    byte
}

/// Converts one byte offset to a UTF-16 line/character position.
fn byte_position(text: &str, target: usize) -> (u64, u64) {
    let mut line = 0_u64;
    let mut character = 0_u64;
    let mut byte = 0_usize;
    for value in text.chars() {
        if byte == target {
            return (line, character);
        }
        byte += value.len_utf8();
        if value == '\n' {
            line += 1;
            character = 0;
        } else if value != '\r' {
            character += u64::try_from(value.len_utf16()).expect("UTF-16 width fits");
        }
    }
    assert_eq!(
        byte, target,
        "byte offset {target} is outside the text: {text:?}"
    );
    (line, character)
}
