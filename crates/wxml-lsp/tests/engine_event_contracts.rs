//! Opt-in native-engine gate for declared component event contracts.
//!
//! The gate reuses the shared `documentSymbol` fixture (`support::provision`):
//! it builds the disposable typings-only package from `TSRS_TYPINGS_ROOT`,
//! replaces the sample project with this gate's pages, starts
//! `<package>/bin/wxml-lsp --stdio --backend native` and drives real
//! `textDocument/publishDiagnostics` traffic.
//!
//! It proves the engine-owned half of `engine-checked-component-bindings` on a
//! live session:
//!
//! - a declared `events.change.detail` plus an explicitly annotated `Page`
//!   handler is checked by the generated contract probe, so parameter widening,
//!   extra optional fields, required-to-optional and literal-to-base detail
//!   fields pass, while an `unknown` field, an extra required handler parameter
//!   and strict variance report `2322`/`2345` exactly on the handler token;
//! - an unannotated parameter skips the probe and only degrades to `Partial`;
//! - a missing handler keeps its own missing-handler diagnostic and produces no
//!   contract type error;
//! - `strictFunctionTypes` decides the `unknown` case;
//! - JavaScript only gets a probe with `checkJs` or a file-level `@ts-check`;
//! - a project without `components.d.ts` keeps its pre-declaration diagnostics
//!   (the `HostOnly` unit test covers the unavailable-engine case).
//!
//! Without `TSRS_TYPINGS_ROOT` the fixture prints an explicit `SKIP` reason
//! through [`provision`] and returns `None`; `WXML_REQUIRE_REAL_ENGINE=1|true`
//! turns a missing input into a panic. Only a *missing* input is skippable.

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

/// The declared component tag used by every page.
const TAG: &str = "van-picker";
/// The contract type errors the engine may report on a generated probe.
const CONTRACT_CODES: [&str; 2] = ["2322", "2345"];
/// Prefix of the zero-width capability status warning.
const STATUS_PREFIX: &str = "type checking is";

const COMPONENT_SCRIPT: &str = "Component({ methods: {} });\n";
const COMPONENT_JSON: &str = "{\"component\":true}\n";
const COMPONENT_WXML: &str = "<view />\n";
const COMPONENT_PATH: &str = "components/picker/index";

/// One plain object detail, one literal detail and one `unknown` detail field.
const DECLARATIONS: &str = concat!(
    "declare global {\n",
    "  interface WxmlComponentMap {\n",
    "    'van-picker': PickerProps;\n",
    "  }\n",
    "\n",
    "  interface PickerProps {\n",
    "    props: {\n",
    "      disabled?: boolean;\n",
    "    };\n",
    "    events: {\n",
    "      change: { detail: { value: number } };\n",
    "      pick: { detail: { kind: 'a' | 'b' } };\n",
    "      raw: { detail: { value: unknown } };\n",
    "    };\n",
    "  }\n",
    "}\n",
    "\n",
    "export {};\n",
);

const STRICT_TSCONFIG: &str = "{\"compilerOptions\":{\"strict\":true,\"target\":\"ES2020\"}}\n";
const LOOSE_TSCONFIG: &str = concat!(
    "{\"compilerOptions\":{\"strict\":true,\"strictFunctionTypes\":false,",
    "\"target\":\"ES2020\"}}\n",
);

const CHANGE_WXML: &str = "<van-picker bind:change=\"onChange\" />\n";
const PICK_WXML: &str = "<van-picker bind:pick=\"onPick\" />\n";
const RAW_WXML: &str = "<van-picker bind:raw=\"onRaw\" />\n";
const MISSING_WXML: &str = "<van-picker bind:change=\"onMissing\" />\n";

/// One page: template, owning script and script language.
struct PageCase {
    /// Page directory below `pages/`.
    dir: &'static str,
    /// Template text.
    wxml: &'static str,
    /// Owning `Page` script.
    script: &'static str,
    /// Script extension (`ts` or `js`).
    language: &'static str,
}

/// The strict matrix: every case is one page in one project.
const STRICT_PAGES: &[PageCase] = &[
    // Detail `{ value: number }` against a handler that accepts a wider value.
    PageCase {
        dir: "widen",
        wxml: CHANGE_WXML,
        script: "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value: number | string }>) { void event; } });\n",
        language: "ts",
    },
    // Structural compatibility allows the handler object to carry an extra
    // optional field.
    PageCase {
        dir: "extra",
        wxml: CHANGE_WXML,
        script: "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value: number; extra?: string }>) { void event; } });\n",
        language: "ts",
    },
    // A required detail field satisfies an optional handler field.
    PageCase {
        dir: "optional",
        wxml: CHANGE_WXML,
        script: "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value?: number }>) { void event; } });\n",
        language: "ts",
    },
    // A literal union detail field widens to its base type.
    PageCase {
        dir: "literal",
        wxml: PICK_WXML,
        script: "Page({ data: {}, onPick(event: WechatMiniprogram.CustomEvent<{ kind: string }>) { void event; } });\n",
        language: "ts",
    },
    // `unknown` in the declaration against a narrow handler field. A function
    // expression keeps the parameter comparison strict under
    // `strictFunctionTypes`.
    PageCase {
        dir: "unknown",
        wxml: RAW_WXML,
        script: "Page({ data: {}, onRaw: function (event: WechatMiniprogram.CustomEvent<{ value: string }>) { void event; } });\n",
        language: "ts",
    },
    // The handler requires one parameter the declaration does not provide.
    PageCase {
        dir: "arity",
        wxml: CHANGE_WXML,
        script: "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value: number }>, extra: string) { void event; void extra; } });\n",
        language: "ts",
    },
    // A zero-argument handler accepts any event.
    PageCase {
        dir: "noparam",
        wxml: CHANGE_WXML,
        script: "Page({ data: {}, onChange() {} });\n",
        language: "ts",
    },
    // No explicit annotation: the probe is skipped and the binding is only
    // `Partial`.
    PageCase {
        dir: "unannotated",
        wxml: CHANGE_WXML,
        script: "Page({ data: {}, onChange(event) { void event; } });\n",
        language: "ts",
    },
    // A missing handler keeps the existing missing-handler error.
    PageCase {
        dir: "missing",
        wxml: MISSING_WXML,
        script: "Page({ data: {}, other() {} });\n",
        language: "ts",
    },
];

/// An incompatible JavaScript handler: the declared detail is
/// `{ value: number }`, the annotation says `{ value: string }`.
const JS_INCOMPATIBLE: &str = "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value: string }>) { void event; } });\n";
/// The same handler with a file-level opt-in pragma.
const JS_TS_CHECK: &str = concat!(
    "// @ts-check\n",
    "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value: string }>) { void event; } });\n",
);

const JS_PAGES: &[PageCase] = &[
    PageCase {
        dir: "plain",
        wxml: CHANGE_WXML,
        script: JS_INCOMPATIBLE,
        language: "js",
    },
    PageCase {
        dir: "tscheck",
        wxml: CHANGE_WXML,
        script: JS_TS_CHECK,
        language: "js",
    },
];

#[test]
fn real_engine_event_contracts_follow_strict_function_types() {
    let Some(fixture) = provision("event-contracts-strict") else {
        return;
    };
    prepare_project(fixture.project(), STRICT_TSCONFIG, STRICT_PAGES, true);

    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    for case in STRICT_PAGES {
        let diagnostics = open_page(&mut session, fixture.project(), case);
        println!("{}: {diagnostics:?}", case.dir);
        match case.dir {
            "widen" | "extra" | "optional" | "literal" | "noparam" => {
                assert_no_contract_error(&diagnostics, case.dir);
                assert_supported(&diagnostics, case.dir);
            }
            "unknown" | "arity" => {
                let errors = contract_errors(&diagnostics);
                assert_eq!(
                    errors.len(),
                    1,
                    "{} must report one type error: {diagnostics:?}",
                    case.dir
                );
                assert_eq!(
                    range_text(case.wxml, errors[0]),
                    handler_of(case.wxml),
                    "{} must report on the handler token: {diagnostics:?}",
                    case.dir
                );
            }
            "unannotated" => {
                assert_no_contract_error(&diagnostics, case.dir);
                let reason = status_reason(&diagnostics)
                    .unwrap_or_else(|| panic!("{} must be Partial: {diagnostics:?}", case.dir));
                assert!(
                    reason.contains("explicit type annotation"),
                    "{} must name the unannotated parameter: {reason}",
                    case.dir
                );
            }
            "missing" => {
                assert_no_contract_error(&diagnostics, case.dir);
                assert!(
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("onMissing"))
                    }),
                    "the missing handler diagnostic must stay: {diagnostics:?}"
                );
            }
            other => panic!("unexpected case {other}"),
        }
    }

    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn real_engine_event_contracts_relax_when_strict_function_types_is_off() {
    let Some(fixture) = provision("event-contracts-loose") else {
        return;
    };
    prepare_project(fixture.project(), LOOSE_TSCONFIG, STRICT_PAGES, true);

    let mut session = Session::start(&fixture);
    session.initialize(&fixture.project_uri());

    // `unknown` versus a narrow handler field is bivariant without
    // `strictFunctionTypes`, so the probe stays clean.
    let unknown = open_page(&mut session, fixture.project(), &STRICT_PAGES[4]);
    println!("loose unknown: {unknown:?}");
    assert_no_contract_error(&unknown, "unknown");
    assert_supported(&unknown, "unknown");

    // Arity is not a variance question: the required extra parameter still
    // cannot be satisfied.
    let arity = open_page(&mut session, fixture.project(), &STRICT_PAGES[5]);
    println!("loose arity: {arity:?}");
    let errors = contract_errors(&arity);
    assert_eq!(errors.len(), 1, "arity must still fail: {arity:?}");
    assert_eq!(range_text(CHANGE_WXML, errors[0]), "onChange");

    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn real_engine_javascript_event_contracts_require_check_js() {
    let Some(fixture) = provision("event-contracts-js-off") else {
        return;
    };
    prepare_project(
        fixture.project(),
        "{\"compilerOptions\":{\"strict\":true,\"allowJs\":true,\"checkJs\":false}}\n",
        JS_PAGES,
        true,
    );

    let mut session = Session::start(&fixture);
    session.initialize(&fixture.project_uri());

    // `checkJs` is off and no pragma is present: no probe is generated, so the
    // incompatible annotation is never compared and the page is only Partial.
    let plain = open_page(&mut session, fixture.project(), &JS_PAGES[0]);
    println!("js checkJs=false: {plain:?}");
    assert_no_contract_error(&plain, "plain");
    let reason = status_reason(&plain)
        .unwrap_or_else(|| panic!("the disabled check must be Partial: {plain:?}"));
    assert!(
        reason.contains("checkJs") || reason.contains("@ts-check"),
        "the Partial reason must name the disabled JavaScript check: {reason}"
    );

    // The file-level pragma opts in even with `checkJs: false`.
    let ts_check = open_page(&mut session, fixture.project(), &JS_PAGES[1]);
    println!("js @ts-check: {ts_check:?}");
    let errors = contract_errors(&ts_check);
    assert_eq!(
        errors.len(),
        1,
        "@ts-check must enable the probe: {ts_check:?}"
    );
    assert_eq!(range_text(CHANGE_WXML, errors[0]), "onChange");

    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn real_engine_javascript_event_contracts_follow_check_js() {
    let Some(fixture) = provision("event-contracts-js-on") else {
        return;
    };
    prepare_project(
        fixture.project(),
        "{\"compilerOptions\":{\"strict\":true,\"allowJs\":true,\"checkJs\":true}}\n",
        JS_PAGES,
        true,
    );

    let mut session = Session::start(&fixture);
    session.initialize(&fixture.project_uri());

    let plain = open_page(&mut session, fixture.project(), &JS_PAGES[0]);
    println!("js checkJs=true: {plain:?}");
    let errors = contract_errors(&plain);
    assert_eq!(errors.len(), 1, "checkJs must enable the probe: {plain:?}");
    assert_eq!(range_text(CHANGE_WXML, errors[0]), "onChange");

    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn real_engine_project_without_declarations_keeps_pre_declaration_diagnostics() {
    let Some(fixture) = provision("event-contracts-undeclared") else {
        return;
    };
    let pages = [
        PageCase {
            dir: "annotated",
            wxml: CHANGE_WXML,
            script: "Page({ data: {}, onChange(event: WechatMiniprogram.CustomEvent<{ value: string }>) { void event; } });\n",
            language: "ts",
        },
        PageCase {
            dir: "missing",
            wxml: MISSING_WXML,
            script: "Page({ data: {}, other() {} });\n",
            language: "ts",
        },
    ];
    prepare_project(fixture.project(), STRICT_TSCONFIG, &pages, false);

    let mut session = Session::start(&fixture);
    session.initialize(&fixture.project_uri());

    // Without `components.d.ts` the tag has no declared events: the
    // incompatible annotation is not compared and nothing is published.
    let annotated = open_page(&mut session, fixture.project(), &pages[0]);
    println!("undeclared annotated: {annotated:?}");
    assert!(
        annotated.is_empty(),
        "a project without declarations must stay clean: {annotated:?}"
    );

    // The missing-handler error is the only pre-declaration diagnostic.
    let missing = open_page(&mut session, fixture.project(), &pages[1]);
    println!("undeclared missing: {missing:?}");
    assert_no_contract_error(&missing, "missing");
    assert_eq!(
        missing.len(),
        1,
        "only the missing handler may be reported: {missing:?}"
    );
    assert!(
        missing[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("onMissing")),
        "the diagnostic must name the missing handler: {missing:?}"
    );

    session.shutdown();
    fixture.tripwire().assert_absent();
}

/// Opens one page and returns its first published diagnostics.
fn open_page(session: &mut Session, project: &Path, case: &PageCase) -> Vec<Value> {
    let uri = format!(
        "file://{}",
        project
            .join(format!("pages/{}/index.wxml", case.dir))
            .display()
    );
    session.did_open(&uri, "wxml", case.wxml);
    wait_for(session, &uri)
}

/// Rewrites the fixture project with this gate's pages.
///
/// `declared` writes the fixed `components.d.ts` next to `tsconfig.json`; the
/// no-declaration project omits it so the tag keeps its pre-declaration shape.
fn prepare_project(project: &Path, tsconfig: &str, pages: &[PageCase], declared: bool) {
    let sample = project.join("pages/index");
    if sample.exists() {
        fs::remove_dir_all(&sample)
            .unwrap_or_else(|error| panic!("remove {}: {error}", sample.display()));
    }

    let pages_json = pages
        .iter()
        .map(|case| format!("\"pages/{}/index\"", case.dir))
        .collect::<Vec<_>>()
        .join(",");
    let app = format!(
        "{{\"pages\":[{pages_json}],\"usingComponents\":{{\"{TAG}\":\"/{COMPONENT_PATH}\"}}}}\n"
    );
    let mut files = vec![
        ("app.json".to_owned(), app),
        ("tsconfig.json".to_owned(), tsconfig.to_owned()),
        (format!("{COMPONENT_PATH}.ts"), COMPONENT_SCRIPT.to_owned()),
        (format!("{COMPONENT_PATH}.json"), COMPONENT_JSON.to_owned()),
        (format!("{COMPONENT_PATH}.wxml"), COMPONENT_WXML.to_owned()),
    ];
    if declared {
        files.push(("components.d.ts".to_owned(), DECLARATIONS.to_owned()));
    }
    for case in pages {
        files.push((
            format!("pages/{}/index.wxml", case.dir),
            case.wxml.to_owned(),
        ));
        files.push((
            format!("pages/{}/index.{}", case.dir, case.language),
            case.script.to_owned(),
        ));
    }

    for (relative, text) in files {
        let path = project.join(&relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
        }
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
}

/// Waits for and returns one document's published diagnostics.
fn wait_for(session: &Session, uri: &str) -> Vec<Value> {
    let published = session.wait_for_diagnostics(&[uri], support::REQUEST_TIMEOUT);
    let message = published
        .iter()
        .find(|message| message["params"]["uri"] == json!(uri))
        .unwrap_or_else(|| panic!("no diagnostics published for {uri}: {published:?}"));
    message["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("diagnostics must be an array: {message}"))
}

/// Returns the published contract type errors.
fn contract_errors(diagnostics: &[Value]) -> Vec<&Value> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .is_some_and(|code| CONTRACT_CODES.contains(&code))
        })
        .collect()
}

/// Asserts the binding produced no contract type error.
fn assert_no_contract_error(diagnostics: &[Value], label: &str) {
    let errors = contract_errors(diagnostics);
    assert!(
        errors.is_empty(),
        "{label} must stay clean, got {errors:?}: {diagnostics:?}"
    );
}

/// Asserts the capability status is not degraded.
fn assert_supported(diagnostics: &[Value], label: &str) {
    assert!(
        status_reason(diagnostics).is_none(),
        "{label} must be a complete check: {diagnostics:?}"
    );
}

/// Returns the capability status reason when a status warning was published.
fn status_reason(diagnostics: &[Value]) -> Option<String> {
    diagnostics.iter().find_map(|diagnostic| {
        diagnostic["message"]
            .as_str()
            .filter(|message| message.starts_with(STATUS_PREFIX))
            .map(str::to_owned)
    })
}

/// Returns the template text covered by one diagnostic range.
fn range_text(text: &str, diagnostic: &Value) -> String {
    let start = byte_at(
        text,
        &diagnostic["range"]["start"]["line"],
        &diagnostic["range"]["start"]["character"],
    );
    let end = byte_at(
        text,
        &diagnostic["range"]["end"]["line"],
        &diagnostic["range"]["end"]["character"],
    );
    text[start..end].to_owned()
}

/// Returns the static handler token of a template.
fn handler_of(wxml: &str) -> &str {
    let start = wxml.find('"').expect("handler value") + 1;
    let end = wxml[start..].find('"').expect("handler value end") + start;
    &wxml[start..end]
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
