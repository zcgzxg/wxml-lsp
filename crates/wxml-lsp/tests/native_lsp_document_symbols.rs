//! Opt-in native-engine LSP `textDocument/documentSymbol` gate.
//!
//! The gate builds a disposable package from a pinned `TSGO_BIN`, starts
//! `<package>/bin/wxml-lsp --stdio --backend native`, and drives real
//! Content-Length JSON-RPC frames. It proves the script outline is produced by
//! the native engine (not by the static WXML path) and that the static WXML
//! outline still answers while the native backend is active.
//!
//! Without `TSGO_BIN` the end-to-end test prints an explicit `SKIP` reason and
//! passes; `WXML_REQUIRE_REAL_ENGINE=1|true` turns that into a panic. Only a
//! *missing* input is skippable; a present but invalid engine/asset/typings
//! input is always fatal. The fail-closed package tests do not need the engine.
//!
//! This gate depends on the trusted type-environment handoff
//! (`run_stdio_native` -> `with_install` -> `with_environment`), because the
//! project `allowJs` option is what puts the `.js` file in the native program.
//! Before that handoff the `.js` query fails rather than skips, so this test
//! must be committed after the handoff.

#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn
)]

#[path = "native_lsp_document_symbols/support.rs"]
mod support;

use std::ffi::OsString;
use std::fs;

use serde_json::{Value, json};
use support::{
    DEFAULT_TMP_BASE, PackageDefect, ProvisionEnv, ProvisionError, Session, TestRoot,
    broken_package, provision, provision_with_env, repo_root, resolve, resolve_tmp_base,
    run_startup_failure,
};

#[test]
fn real_engine_document_symbols_for_script_and_wxml_outline() {
    let Some(fixture) = provision("document-symbols") else {
        return;
    };
    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    // `documentSymbolProvider` is advertised by the static WXML layer. This
    // assertion is a protocol precondition, not evidence for the native
    // script outline; the result below is the native evidence.
    assert_eq!(
        initialized["result"]["capabilities"]["documentSymbolProvider"],
        json!(true),
        "{initialized}"
    );

    let wxml_uri = fixture.uri("pages/index/index.wxml");
    let js_uri = fixture.uri("pages/index/index.js");
    session.did_open(&wxml_uri, "wxml", support::PROJECT_WXML);
    session.did_open(&js_uri, "javascript", support::PROJECT_JS);

    let script = session.document_symbol(&js_uri);
    assert!(
        script.get("error").is_none(),
        "script documentSymbol failed: {script}"
    );
    let symbols = script["result"].as_array().expect("script result array");
    println!("native script document symbols: {:?}", pairs(symbols));
    // The `Page(...)` registration is an expression statement, so the native
    // walker records it as a gap and does not descend into its object literal.
    // The outline therefore proves the engine-derived top-level declarations
    // and the helper-class method; the literal `data` property stays a known
    // capability gap rather than being asserted as supported.
    assert_eq!(
        pairs(symbols),
        vec![
            ("user".to_owned(), 13),
            ("onTap".to_owned(), 12),
            ("PageHelpers".to_owned(), 5),
        ],
        "script outline must be engine-derived and in source order: {script}"
    );
    assert_selection(&symbols[0], 0, 6, 0, 10);
    assert_selection(&symbols[1], 1, 9, 1, 14);
    assert_selection(&symbols[2], 2, 6, 2, 17);
    let helper = &symbols[2]["children"];
    let helper = helper.as_array().expect("class children");
    assert_eq!(helper.len(), 1, "{script}");
    assert_eq!(helper[0]["name"], json!("formatName"));
    assert_eq!(helper[0]["kind"], json!(6));
    assert_selection(&helper[0], 3, 2, 3, 12);

    let template = session.document_symbol(&wxml_uri);
    assert!(
        template.get("error").is_none(),
        "WXML documentSymbol failed: {template}"
    );
    let outline = template["result"].as_array().expect("WXML result array");
    println!("static WXML document symbols: {:?}", pairs(outline));
    assert_eq!(
        pairs(outline),
        vec![("view".to_owned(), 5), ("button".to_owned(), 5)],
        "the static WXML outline must still answer: {template}"
    );
    assert_eq!(outline[0]["detail"], json!(".root"));

    let stderr = session.stderr_text();
    assert!(
        !stderr.contains("server.js"),
        "the native path must not start the Node backend:\n{stderr}"
    );
    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn real_engine_script_diagnostics_publish_2322_without_template_facts() {
    let Some(fixture) = provision("script-diagnostics") else {
        return;
    };
    let mut session = Session::start(&fixture);
    let initialized = session.initialize(&fixture.project_uri());
    assert_eq!(
        initialized["result"]["capabilities"]["positionEncoding"],
        json!("utf-16"),
        "{initialized}"
    );

    let wxml_uri = fixture.uri("pages/index/index.wxml");
    let ts_uri = fixture.uri("pages/index/typed.ts");
    session.did_open(&wxml_uri, "wxml", support::PROJECT_WXML);
    session.did_open(&ts_uri, "typescript", support::PROJECT_TS);

    let published = session.wait_for_diagnostics(&[&wxml_uri, &ts_uri], support::REQUEST_TIMEOUT);
    let script = published
        .iter()
        .find(|message| message["params"]["uri"] == json!(ts_uri))
        .unwrap_or_else(|| panic!("no script diagnostics publication: {published:?}"));
    assert_eq!(script["params"]["version"], json!(1), "{script}");
    let diagnostics = script["params"]["diagnostics"]
        .as_array()
        .expect("script diagnostics array");
    let error = diagnostics
        .iter()
        .find(|diagnostic| diagnostic["code"] == json!("2322"))
        .unwrap_or_else(|| panic!("no TS2322 in the script publication: {script}"));
    assert_eq!(error["severity"], json!(1), "{script}");
    assert_eq!(
        error["range"]["start"]["line"],
        json!(0),
        "the error is on the first line: {script}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["code"] == json!("2322")),
        "template/style diagnostics must not be mixed into a script payload: {script}"
    );

    let template = published
        .iter()
        .find(|message| message["params"]["uri"] == json!(wxml_uri))
        .unwrap_or_else(|| panic!("no template diagnostics publication: {published:?}"));
    assert!(
        !template["params"]["diagnostics"]
            .as_array()
            .expect("template diagnostics array")
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!("2322")),
        "a script diagnostic must not leak into the template payload: {template}"
    );
    assert!(
        !session.stderr_text().contains("server.js"),
        "the native path must not start Node:\n{}",
        session.stderr_text()
    );
    session.shutdown();
    fixture.tripwire().assert_absent();
}

#[test]
fn a_missing_engine_fails_before_initialize_without_node_fallback() {
    let broken = broken_package("missing-engine", PackageDefect::MissingEngine);
    let failure = run_startup_failure(&broken);
    assert!(!failure.status.success(), "{failure:?}");
    assert!(
        !failure.stdout.contains("\"result\""),
        "the broken package answered initialize: {}",
        failure.stdout
    );
    assert!(
        failure.stderr.contains("native engine discovery failed"),
        "{}",
        failure.stderr
    );
    assert!(failure.stderr.contains("engine"), "{}", failure.stderr);
    assert!(!failure.stderr.contains("server.js"), "{}", failure.stderr);
    assert!(!failure.stdout.contains("server.js"), "{}", failure.stdout);
    broken.tripwire().assert_absent();
}

#[test]
fn a_tampered_engine_fails_before_initialize() {
    let broken = broken_package("tampered-engine", PackageDefect::TamperedEngine);
    let failure = run_startup_failure(&broken);
    assert!(!failure.status.success(), "{failure:?}");
    assert!(
        !failure.stdout.contains("\"result\""),
        "the tampered package answered initialize: {}",
        failure.stdout
    );
    assert!(
        failure.stderr.contains("native engine discovery failed"),
        "{}",
        failure.stderr
    );
    assert!(
        failure
            .stderr
            .contains("does not match the pinned artifact"),
        "{}",
        failure.stderr
    );
    assert!(!failure.stderr.contains("server.js"), "{}", failure.stderr);
    assert!(!failure.stdout.contains("server.js"), "{}", failure.stdout);
    broken.tripwire().assert_absent();
}

#[test]
#[should_panic(expected = "WXML_REQUIRE_REAL_ENGINE")]
fn required_mode_without_engine_fails_closed() {
    let true_spelling = ProvisionEnv {
        require_real_engine: Some(OsString::from("true")),
        ..ProvisionEnv::default()
    };
    assert!(true_spelling.required());
    let env = ProvisionEnv {
        require_real_engine: Some(OsString::from("1")),
        tsgo_bin: None,
        tmp_base: Some(OsString::from(DEFAULT_TMP_BASE)),
        ..ProvisionEnv::default()
    };
    let _ = provision_with_env(&env, "required-missing-engine");
}

#[test]
fn optional_mode_without_engine_is_an_explicit_skip() {
    let env = ProvisionEnv {
        tmp_base: Some(OsString::from(DEFAULT_TMP_BASE)),
        ..ProvisionEnv::default()
    };
    assert!(provision_with_env(&env, "optional-missing-engine").is_none());
}

#[test]
fn a_repository_tmp_base_is_rejected_without_creating_it() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let base = repo_root().join(format!(
        "wxml-native-ds-repo-leak-{}-{stamp}",
        std::process::id()
    ));
    assert!(
        !base.exists(),
        "test path must start absent: {}",
        base.display()
    );
    let env = ProvisionEnv {
        tmp_base: Some(base.clone().into_os_string()),
        ..ProvisionEnv::default()
    };
    let result = resolve_tmp_base(&env);
    let leaked = base.exists();
    if leaked {
        let _ = fs::remove_dir_all(&base);
    }
    let error = result.expect_err("a repository path must be rejected");
    assert!(matches!(error, ProvisionError::Invalid { .. }), "{error:?}");
    assert!(
        !leaked,
        "the repository path must not be created before the error: {}",
        base.display()
    );
}

#[test]
fn a_present_non_pinned_engine_is_always_fatal() {
    let base = resolve_tmp_base(&ProvisionEnv::default()).expect("test base");
    let root = TestRoot::new(&base, "wrong-engine");
    let fake = root.path().join("tsc");
    fs::write(&fake, b"#!/bin/sh\nexit 0\n").expect("fake engine");
    let env = ProvisionEnv {
        tsgo_bin: Some(fake.into_os_string()),
        ..ProvisionEnv::default()
    };
    let error = resolve(&env).expect_err("a non-pinned engine must be rejected");
    assert!(matches!(error, ProvisionError::Invalid { .. }), "{error:?}");
    assert!(error.message().contains("pinned"), "{error:?}");
}

/// Returns `(name, kind)` pairs in result order.
fn pairs(symbols: &[Value]) -> Vec<(String, u64)> {
    symbols
        .iter()
        .map(|symbol| {
            (
                symbol["name"].as_str().expect("symbol name").to_owned(),
                symbol["kind"].as_u64().expect("symbol kind"),
            )
        })
        .collect()
}

/// Asserts one exact `selectionRange`.
fn assert_selection(
    symbol: &Value,
    start_line: u64,
    start_char: u64,
    end_line: u64,
    end_char: u64,
) {
    assert_eq!(
        symbol["selectionRange"],
        json!({
            "start": {"line": start_line, "character": start_char},
            "end": {"line": end_line, "character": end_char}
        }),
        "{symbol}"
    );
}
