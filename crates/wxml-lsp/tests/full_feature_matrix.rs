//! Cross-feature JSON-RPC integration matrix (IT-01, IT-02, IT-03, IT-05).
//!
//! These tests exercise the real `wxml-lsp` event loop over a socket pair. The
//! type backend is in-process: either the stock `MockBackend`, a small backend
//! that derives its results from the actual WXML text, or the real
//! `NodeBackend` pointed at a fake NDJSON transport. No Node process is
//! started.

#![cfg(unix)]
#![allow(
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

#[path = "full_feature_matrix/support.rs"]
mod support;

use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use support::{
    Session, TempProject, apply_completion_item, definition_uris, diagnostic_codes, item, labels,
    position_at,
};
use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig, NodeTransport, NodeTransportError};
use wxml_lsp::project_host::ProjectHost;
use wxml_text::SourceRange;
use wxml_types::{
    Backend, BackendCapabilities, BackendEpoch, BackendError, BackendId, CapabilityStatus,
    CompletionItem, CompletionItemKind, CompletionKind, CompletionRequest, CompletionResult,
    DefinitionRequest, DefinitionResult, Diagnostic, DiagnosticSeverity, DiagnosticsRequest,
    DiagnosticsResult, FileId, HoverRequest, HoverResult, InsertTextFormat, MarkupKind,
    SourceLocation, TypeQuality,
};

fn diagnostics_version(notification: &Value) -> u64 {
    notification["params"]["version"].as_u64().expect("version")
}

fn hover_text(hover: &Value) -> String {
    hover["contents"]["value"]
        .as_str()
        .expect("hover markdown")
        .to_owned()
}

fn definition_range(location: &Value) -> Value {
    location["range"].clone()
}

/// A backend that answers from the actual WXML snapshot text.
///
/// It is deliberately small: hover, definition, diagnostics and expression
/// completion only fire for the legacy `user.name` / `user.na` member and its
/// ranges are derived from the request's source text. The server still owns
/// UTF-16 conversion, request routing, diagnostics merging and result
/// attribution, so a wrong dispatch or range mapping fails the test.
struct TextBackend {
    calls: Mutex<Vec<&'static str>>,
    script_text: String,
}

impl TextBackend {
    const fn new(script_text: String) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            script_text,
        }
    }

    fn record(&self, call: &'static str) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn range_of(text: &str, needle: &str) -> Option<(u32, u32)> {
    let start = text.find(needle)?;
    let end = start + needle.len();
    Some((u32::try_from(start).ok()?, u32::try_from(end).ok()?))
}

fn source_range(start: u32, end: u32) -> SourceRange {
    SourceRange::from_bounds(start, end).expect("valid source range")
}

impl Backend for TextBackend {
    fn epoch(&self) -> BackendEpoch {
        BackendEpoch::new(BackendId::new("matrix-text"), 1)
    }

    fn hover(&self, request: HoverRequest) -> Result<HoverResult, BackendError> {
        self.record("hover");
        let text = request.context.source().text();
        let Some((start, end)) = range_of(text, "user.name") else {
            return Err(BackendError::Unavailable("no member".to_owned()));
        };
        if !(start..end).contains(&request.offset) {
            return Err(BackendError::Unavailable("not on member".to_owned()));
        }
        Ok(HoverResult {
            contents: "string".to_owned(),
            markup: MarkupKind::Markdown,
            range: Some(source_range(start, end)),
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Known,
        })
    }

    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError> {
        self.record("diagnostics");
        let text = request.context.source().text();
        let diagnostics = range_of(text, "user.name")
            .map(|(start, end)| Diagnostic {
                // The inner `name` member, not the whole expression.
                range: source_range(start + 5, end),
                severity: DiagnosticSeverity::Warning,
                message: "legacy Page member is checked".to_owned(),
                code: Some("test.legacy-member".to_owned()),
            })
            .into_iter()
            .collect();
        Ok(DiagnosticsResult {
            diagnostics,
            status: CapabilityStatus::Supported,
        })
    }

    fn definition(&self, request: DefinitionRequest) -> Result<DefinitionResult, BackendError> {
        self.record("definition");
        let text = request.context.source().text();
        let Some((start, end)) = range_of(text, "user.name") else {
            return Ok(DefinitionResult {
                locations: Vec::new(),
                status: CapabilityStatus::Supported,
            });
        };
        if !(start..end).contains(&request.offset) {
            return Ok(DefinitionResult {
                locations: Vec::new(),
                status: CapabilityStatus::Supported,
            });
        }
        let script = FileId::new(
            request.context.file().project().clone(),
            "pages/index/index.ts",
        )
        .expect("script file id");
        let (name_start, _name_end) =
            range_of(&self.script_text, "name: 'Ada'").expect("script name");
        Ok(DefinitionResult {
            locations: vec![SourceLocation {
                file: script,
                range: source_range(name_start, name_start + 4),
            }],
            status: CapabilityStatus::Supported,
        })
    }

    fn completion(&self, request: CompletionRequest) -> Result<CompletionResult, BackendError> {
        self.record("completion");
        if request.kind != CompletionKind::ExpressionMember {
            return Ok(CompletionResult {
                items: Vec::new(),
                status: CapabilityStatus::Supported,
                truncated: false,
            });
        }
        let text = request.context.source().text();
        let Some((start, _end)) = range_of(text, "user.na") else {
            return Ok(CompletionResult {
                items: Vec::new(),
                status: CapabilityStatus::Supported,
                truncated: false,
            });
        };
        Ok(CompletionResult {
            items: vec![CompletionItem {
                label: "name".to_owned(),
                kind: CompletionItemKind::Property,
                detail: Some("string".to_owned()),
                documentation: Some("legacy Page data member".to_owned()),
                insert_text: "name".to_owned(),
                insert_text_format: InsertTextFormat::PlainText,
                replace: source_range(start + 5, start + 7),
                sort_text: None,
                filter_text: None,
            }],
            status: CapabilityStatus::Supported,
            truncated: false,
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            hover: CapabilityStatus::Supported,
            diagnostics: CapabilityStatus::Supported,
            definition: CapabilityStatus::Supported,
            completion: CapabilityStatus::Supported,
        }
    }
}

fn legacy_fixture() -> TempProject {
    let project = TempProject::new("wxml-it01");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{"card":"/components/card/card"}}"#,
    );
    project.write(
        "pages/index/index.json",
        r#"{"usingComponents":{"card":"../../components/card/card"}}"#,
    );
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write("pages/index/index.wxml", "<view />");
    project.write("components/card/card.json", r#"{"component":true}"#);
    project.write(
        "components/card/card.ts",
        "Component({\n  properties: {\n    /** 计数 */\n    count: { type: Number, value: 0 },\n  },\n});",
    );
    project.write("components/card/card.wxml", "<view>{{ count }}</view>");
    project
}

/// IT-01: legacy Page/Component hover, definition, completion and diagnostics
/// keep their dispatch while every new local capability is enabled.
#[test]
fn it_01_legacy_page_and_component_queries_survive_new_capabilities() {
    let project = legacy_fixture();
    let script_text =
        std::fs::read_to_string(project.root().join("pages/index/index.ts")).expect("script text");
    let backend = Arc::new(TextBackend::new(script_text.clone()));
    let backend_for_session = Arc::clone(&backend);
    let mut session = Session::start_with(
        &project,
        json!({}),
        json!({
            "textDocument": {
                "completion": {"completionItem": {"snippetSupport": true}},
            },
        }),
        move |_| backend_for_session as Arc<dyn Backend>,
    );

    let capabilities = &session.initialize["capabilities"];
    for provider in [
        "hoverProvider",
        "definitionProvider",
        "documentFormattingProvider",
        "documentRangeFormattingProvider",
        "linkedEditingRangeProvider",
    ] {
        assert_eq!(capabilities[provider], json!(true), "{provider}");
    }
    assert_eq!(
        capabilities["documentLinkProvider"]["resolveProvider"],
        json!(false)
    );
    assert_eq!(
        capabilities["completionProvider"]["resolveProvider"],
        json!(false)
    );

    let uri = project.uri("pages/index/index.wxml");
    let text = "<view>😀 {{ user.name }}</view>\n<card count=\"1\" />";
    session.open_wxml(&uri, text, 1);
    let published = session.diagnostics_version(&uri, 1);
    assert!(
        diagnostic_codes(&published).contains(&"test.legacy-member".to_owned()),
        "backend diagnostics must still publish: {published}"
    );
    let user_start = text.find("user.name").expect("user member");
    let name_position = position_at(text, user_start + 5);
    let expected_hover_range = json!({
        "start": position_at(text, user_start),
        "end": position_at(text, user_start + "user.name".len()),
    });
    let expected_diagnostic_range = json!({
        "start": name_position,
        "end": position_at(text, user_start + "user.name".len()),
    });
    let member_diagnostic = published["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == json!("test.legacy-member"))
        .expect("legacy diagnostic");
    assert_eq!(member_diagnostic["range"], expected_diagnostic_range);

    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": name_position,
        }),
    );
    let hover = session.result(id);
    assert_eq!(hover_text(&hover), "string", "{hover}");
    assert_eq!(hover["range"], expected_hover_range, "{hover}");

    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": name_position,
        }),
    );
    let definition = session.result(id);
    assert_eq!(
        definition_uris(&definition),
        vec![project.uri("pages/index/index.ts")],
        "{definition}"
    );
    let script_name = script_text.find("name: 'Ada'").expect("script name");
    assert_eq!(
        definition_range(&definition[0]),
        json!({
            "start": position_at(&script_text, script_name),
            "end": position_at(&script_text, script_name + 4),
        }),
        "{definition}"
    );

    // Expression completion still goes through the backend.
    let expr_text = "<view>😀 {{ user.na }}</view>";
    session.change(&uri, expr_text, 2);
    let na_end = expr_text.find("na }}").expect("typed member") + 2;
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(expr_text, na_end),
        }),
    );
    let completion = session.result(id);
    let name = item(&completion, "name");
    assert_eq!(name["detail"], json!("string"), "{name}");
    assert_eq!(
        apply_completion_item(expr_text, name),
        "<view>😀 {{ user.name }}</view>"
    );

    // Static tag and component completion/definition keep their old dispatch.
    let tag_text = "<vi";
    session.change(&uri, tag_text, 3);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(tag_text, tag_text.len()),
        }),
    );
    let tags = session.result(id);
    assert!(labels(&tags).contains(&"view".to_owned()), "{tags}");

    let component_text = "<card ";
    session.change(&uri, component_text, 4);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(component_text, component_text.len()),
        }),
    );
    let attributes = session.result(id);
    assert!(
        labels(&attributes).contains(&"count".to_owned()),
        "{attributes}"
    );
    let count = item(&attributes, "count");
    assert_eq!(count["detail"], json!("number"), "{count}");

    let card_text = "<card count=\"1\" />";
    session.change(&uri, card_text, 5);
    let card_name = card_text.find("card").expect("card tag") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(card_text, card_name),
        }),
    );
    let card_hover = session.result(id);
    let card_hover_text = hover_text(&card_hover);
    assert!(
        card_hover_text.contains("components/card/card.wxml"),
        "{card_hover_text}"
    );
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(card_text, card_name),
        }),
    );
    let card_definition = session.result(id);
    assert!(
        definition_uris(&card_definition)
            .iter()
            .any(|uri| uri.ends_with("components/card/card.wxml")),
        "{card_definition}"
    );

    let count_start = card_text.find("count").expect("count attribute");
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(card_text, count_start + 2),
        }),
    );
    let property_definition = session.result(id);
    assert!(
        definition_uris(&property_definition)
            .iter()
            .any(|uri| uri.ends_with("components/card/card.ts")),
        "{property_definition}"
    );
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(card_text, count_start + 2),
        }),
    );
    let property_hover = session.result(id);
    assert!(
        hover_text(&property_hover).contains("number"),
        "{property_hover}"
    );

    // The new local handlers answer without overriding the legacy ones.
    let formatted_source = "<view   class=\"a\"   >\n  <text>x</text>\n</view>";
    session.change(&uri, formatted_source, 6);
    let id = session.request(
        "textDocument/formatting",
        json!({
            "textDocument": {"uri": uri},
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let edits = session.result(id);
    let formatted = support::apply_edits(formatted_source, &edits);
    assert_eq!(formatted, "<view class=\"a\">\n  <text>x</text>\n</view>");
    support::assert_parses_cleanly(&formatted);

    let id = session.request(
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(formatted_source, 2),
        }),
    );
    let linked = session.result(id);
    assert_eq!(
        linked["ranges"].as_array().expect("linked ranges").len(),
        2,
        "{linked}"
    );

    let id = session.request(
        "textDocument/documentLink",
        json!({"textDocument": {"uri": uri}}),
    );
    let links = session.result(id);
    assert!(links.is_array(), "{links}");

    // A hover after the new handlers still reaches the old backend path.
    let final_text = "<view>😀 {{ user.name }}</view>";
    session.change(&uri, final_text, 7);
    let final_start = final_text.find("user.name").expect("user member");
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(final_text, final_start + 5),
        }),
    );
    let final_hover = session.result(id);
    assert_eq!(hover_text(&final_hover), "string", "{final_hover}");

    let calls = backend.calls();
    for expected in ["hover", "definition", "completion", "diagnostics"] {
        assert!(
            calls.contains(&expected),
            "{expected} not called: {calls:?}"
        );
    }
    session.finish();
}

fn component_fixture() -> TempProject {
    let project = TempProject::new("wxml-it02");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{
            "shared-card":"/components/app-shared/app-shared",
            "app-only":"/components/app-only/app-only"
        }}"#,
    );
    project.write(
        "pages/index/index.json",
        r#"{"usingComponents":{
            "shared-card":"../../components/page-shared/page-shared",
            "page-only":"../../components/page-only/page-only",
            "host-card":"../../components/host-card/host-card"
        }}"#,
    );
    project.write("pages/index/index.ts", "Page({ data: {} });");
    project.write("pages/index/index.wxml", "<view />");
    for (path, json_body, script, wxml) in [
        (
            "components/app-shared/app-shared",
            r#"{"component":true}"#,
            "Component({ properties: { appOnly: { type: String } } });",
            "<view>{{ appOnly }}</view>",
        ),
        (
            "components/page-shared/page-shared",
            r#"{"component":true}"#,
            "Component({ properties: {\n  /** 页面共享属性 */\n  pageOnly: { type: Number, value: 1 },\n} });",
            "<view>{{ pageOnly }}</view>",
        ),
        (
            "components/page-only/page-only",
            r#"{"component":true}"#,
            "Component({ properties: { pageProp: { type: Boolean } } });",
            "<view>{{ pageProp }}</view>",
        ),
        (
            "components/app-only/app-only",
            r#"{"component":true}"#,
            "Component({ properties: { appProp: { type: String } } });",
            "<view>{{ appProp }}</view>",
        ),
        (
            "components/inner/inner",
            r#"{"component":true}"#,
            "Component({ properties: { label: { type: String } } });",
            "<view>{{ label }}</view>",
        ),
    ] {
        project.write(&format!("{path}.json"), json_body);
        project.write(&format!("{path}.ts"), script);
        project.write(&format!("{path}.wxml"), wxml);
    }
    project.write(
        "components/host-card/host-card.json",
        r#"{"component":true,"usingComponents":{"inner":"../inner/inner"}}"#,
    );
    project.write(
        "components/host-card/host-card.ts",
        "Component({ properties: { count: { type: Number } } });",
    );
    project.write("components/host-card/host-card.wxml", "<view />");
    project
}

/// IT-02: page, app and component-own `usingComponents` drive one resolution
/// result for completion, attribute hover and component definition.
#[test]
fn it_02_component_resolution_is_shared_by_completion_hover_and_definition() {
    let project = component_fixture();
    let mut session = Session::start(&project, json!({}));
    let uri = project.uri("pages/index/index.wxml");
    session.open_wxml(&uri, "<view />", 1);
    let _ = session.diagnostics_version(&uri, 1);

    // Page-local `shared-card` wins over the app entry for every feature.
    let text = "<shared-card ";
    session.change(&uri, text, 2);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, text.len()),
        }),
    );
    let completion = session.result(id);
    let visible = labels(&completion);
    assert!(visible.contains(&"pageOnly".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"appOnly".to_owned()), "{visible:?}");
    let property = item(&completion, "pageOnly");
    assert_eq!(property["detail"], json!("number"), "{property}");
    assert!(
        property["documentation"]
            .as_str()
            .is_some_and(|documentation| documentation.contains("页面共享属性")),
        "{property}"
    );

    let tag_text = "<shared-card />";
    session.change(&uri, tag_text, 3);
    let tag_offset = tag_text.find("shared-card").expect("tag") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(tag_text, tag_offset),
        }),
    );
    let tag_hover = session.result(id);
    let tag_hover_text = hover_text(&tag_hover);
    assert!(
        tag_hover_text.contains("components/page-shared/page-shared.wxml"),
        "{tag_hover_text}"
    );
    assert!(
        tag_hover_text.contains("pages/index/index.json"),
        "{tag_hover_text}"
    );
    assert!(
        !tag_hover_text.contains("components/app-shared"),
        "{tag_hover_text}"
    );
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(tag_text, tag_offset),
        }),
    );
    let tag_definition = session.result(id);
    assert_eq!(
        definition_uris(&tag_definition),
        vec![project.uri("components/page-shared/page-shared.wxml")],
        "hover and definition must resolve the same page-local target: {tag_definition}"
    );

    let attr_text = "<shared-card pageOnly=\"1\" />";
    session.change(&uri, attr_text, 4);
    let attr_offset = attr_text.find("pageOnly").expect("property") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(attr_text, attr_offset),
        }),
    );
    let property_hover = session.result(id);
    let property_hover_text = hover_text(&property_hover);
    assert!(
        property_hover_text.contains("number"),
        "{property_hover_text}"
    );
    assert!(
        property_hover_text.contains("components/page-shared/page-shared.ts"),
        "{property_hover_text}"
    );
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(attr_text, attr_offset),
        }),
    );
    let property_definition = session.result(id);
    assert_eq!(
        definition_uris(&property_definition),
        vec![project.uri("components/page-shared/page-shared.ts")],
        "{property_definition}"
    );
    let script =
        std::fs::read_to_string(project.root().join("components/page-shared/page-shared.ts"))
            .expect("page-shared script");
    let script_property = script.find("pageOnly").expect("script property");
    assert_eq!(
        property_definition[0]["range"]["start"],
        position_at(&script, script_property),
        "{property_definition}"
    );

    // App-level components still resolve for the page and from a component.
    let app_text = "<app-only ";
    session.change(&uri, app_text, 5);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(app_text, app_text.len()),
        }),
    );
    let app_completion = session.result(id);
    assert!(
        labels(&app_completion).contains(&"appProp".to_owned()),
        "{app_completion}"
    );

    let app_tag = "<app-only />";
    session.change(&uri, app_tag, 6);
    let app_offset = app_tag.find("app-only").expect("app tag") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(app_tag, app_offset),
        }),
    );
    let app_hover = session.result(id);
    assert!(hover_text(&app_hover).contains("app.json"), "{app_hover}");
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(app_tag, app_offset),
        }),
    );
    let app_definition = session.result(id);
    assert_eq!(
        definition_uris(&app_definition),
        vec![project.uri("components/app-only/app-only.wxml")],
        "{app_definition}"
    );

    // A component's own `usingComponents` resolves independently of the page.
    let component_uri = project.uri("components/host-card/host-card.wxml");
    session.open_wxml(&component_uri, "<view />", 1);
    let inner_text = "<inner ";
    session.change(&component_uri, inner_text, 2);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": component_uri},
            "position": position_at(inner_text, inner_text.len()),
        }),
    );
    let inner_completion = session.result(id);
    assert!(
        labels(&inner_completion).contains(&"label".to_owned()),
        "{inner_completion}"
    );
    let inner_tag = "<inner />";
    session.change(&component_uri, inner_tag, 3);
    let inner_offset = inner_tag.find("inner").expect("inner tag") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": component_uri},
            "position": position_at(inner_tag, inner_offset),
        }),
    );
    let inner_hover = session.result(id);
    assert!(
        hover_text(&inner_hover).contains("components/inner/inner.wxml"),
        "{inner_hover}"
    );
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": component_uri},
            "position": position_at(inner_tag, inner_offset),
        }),
    );
    let inner_definition = session.result(id);
    assert_eq!(
        definition_uris(&inner_definition),
        vec![project.uri("components/inner/inner.wxml")],
        "{inner_definition}"
    );

    // The page-only override must not leak into the component scope.
    let shared_tag = "<shared-card />";
    session.change(&component_uri, shared_tag, 4);
    let shared_offset = shared_tag.find("shared-card").expect("shared tag") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": component_uri},
            "position": position_at(shared_tag, shared_offset),
        }),
    );
    let shared_hover = session.result(id);
    let shared_hover_text = hover_text(&shared_hover);
    assert!(
        shared_hover_text.contains("components/app-shared/app-shared.wxml"),
        "{shared_hover_text}"
    );
    assert!(
        shared_hover_text.contains("app.json"),
        "{shared_hover_text}"
    );

    session.finish();
}

fn picker_fixture() -> TempProject {
    let project = TempProject::new("wxml-it03");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({ data: {} });");
    project.write("pages/index/index.wxml", "<view />");
    project
}

/// IT-03: picker mode changes update completion and diagnostics together
/// without duplicating an attribute or using the previous mode.
#[test]
fn it_03_picker_mode_completion_and_diagnostics_stay_in_sync() {
    let project = picker_fixture();
    let mut session = Session::start(&project, json!({}));
    let uri = project.uri("pages/index/index.wxml");

    let selector = "<picker mode=\"selector\" start=\"1\" />";
    session.open_wxml(&uri, selector, 1);
    let published = session.diagnostics_version(&uri, 1);
    assert!(
        diagnostic_codes(&published).contains(&"wxml-picker-mode-attribute".to_owned()),
        "{published}"
    );

    let typed = "<picker mode=\"selector\" range=\"a\" ra";
    session.change(&uri, typed, 2);
    let _ = session.diagnostics_version(&uri, 2);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(typed, typed.len()),
        }),
    );
    let completion = session.result(id);
    let visible = labels(&completion);
    assert!(visible.contains(&"range-key".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"range".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"start".to_owned()), "{visible:?}");
    let unique: std::collections::BTreeSet<_> = visible.iter().collect();
    assert_eq!(
        unique.len(),
        visible.len(),
        "duplicate candidates: {visible:?}"
    );
    let range_key = item(&completion, "range-key");
    assert_eq!(
        range_key["textEdit"]["range"]["start"],
        position_at(typed, typed.len() - 2),
        "{range_key}"
    );
    let applied = apply_completion_item(typed, range_key);
    assert_eq!(applied.matches("range-key").count(), 1, "{applied}");
    assert_eq!(applied.matches("range=").count(), 1, "{applied}");

    let date = "<picker mode=\"date\" range=\"a\" start=\"1\" />";
    session.change(&uri, date, 3);
    let published = session.diagnostics_version(&uri, 3);
    assert!(
        diagnostic_codes(&published).contains(&"wxml-picker-mode-attribute".to_owned()),
        "{published}"
    );

    let date_attrs = "<picker mode=\"date\" ";
    session.change(&uri, date_attrs, 4);
    let _ = session.diagnostics_version(&uri, 4);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(date_attrs, date_attrs.len()),
        }),
    );
    let date_completion = session.result(id);
    let date_visible = labels(&date_completion);
    for expected in ["start", "end", "fields"] {
        assert!(
            date_visible.contains(&expected.to_owned()),
            "{expected} missing: {date_visible:?}"
        );
    }
    assert!(
        !date_visible.contains(&"range".to_owned()),
        "{date_visible:?}"
    );
    assert!(
        !date_visible.contains(&"bindcolumnchange".to_owned()),
        "{date_visible:?}"
    );

    let editing_mode = "<picker mode=\"date\" mode";
    session.change(&uri, editing_mode, 5);
    let _ = session.diagnostics_version(&uri, 5);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(editing_mode, editing_mode.len()),
        }),
    );
    let mode_completion = session.result(id);
    let mode_item = item(&mode_completion, "mode");
    let mode_applied = apply_completion_item(editing_mode, mode_item);
    assert_eq!(mode_applied.matches("mode=").count(), 1, "{mode_applied}");
    assert_eq!(mode_applied, "<picker mode=\"date\" mode", "{mode_applied}");

    let closed = "<picker mode=\"date\" />";
    session.change(&uri, closed, 6);
    let published = session.diagnostics_version(&uri, 6);
    assert!(
        diagnostic_codes(&published).is_empty(),
        "mode diagnostics must clear: {published}"
    );
    assert_eq!(diagnostics_version(&published), 6);

    session.finish();
}

fn multi_app_fixture() -> TempProject {
    let project = TempProject::new("wxml-it05");
    for (app, alias) in [("a", "@a/*"), ("b", "@b/*")] {
        project.write(
            &format!("apps/{app}/miniprogram/app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
        );
        project.write(
            &format!("apps/{app}/tsconfig.json"),
            &format!(
                r#"{{"compilerOptions":{{"baseUrl":".","paths":{{"{alias}":["miniprogram/models/*"]}}}},"include":["miniprogram/**/*.ts"]}}"#
            ),
        );
        project.write(
            &format!("apps/{app}/miniprogram/pages/index/index.json"),
            r#"{"usingComponents":{"card":"../../components/card/card"}}"#,
        );
        project.write(
            &format!("apps/{app}/miniprogram/pages/index/index.ts"),
            "Page({ data: { user: { name: 'Ada' } } });",
        );
        project.write(
            &format!("apps/{app}/miniprogram/pages/index/index.wxml"),
            "<view />",
        );
        project.write(
            &format!("apps/{app}/miniprogram/pages/index/index.wxss"),
            &format!(
                "/* {app} page dup */\n.dup {{ color: red; }}\n.{app}-only {{ color: blue; }}\n"
            ),
        );
        project.write(
            &format!("apps/{app}/miniprogram/app.wxss"),
            &format!("/* {app} global */\n.app-{app} {{ color: black; }}\n"),
        );
        project.write(
            &format!("apps/{app}/miniprogram/components/card/card.json"),
            r#"{"component":true}"#,
        );
        project.write(
            &format!("apps/{app}/miniprogram/components/card/card.ts"),
            &format!("Component({{ properties: {{ {app}Card: {{ type: String }} }} }});"),
        );
        project.write(
            &format!("apps/{app}/miniprogram/components/card/card.wxml"),
            "<view />",
        );
    }
    project
}

type Responder = Arc<dyn Fn(&Value) -> String + Send + Sync>;

struct FakeNode {
    requests: Arc<Mutex<Vec<Value>>>,
    responder: Responder,
}

impl NodeTransport for FakeNode {
    fn request(
        &self,
        line: &str,
        _timeout: std::time::Duration,
    ) -> Result<String, NodeTransportError> {
        let request: Value = serde_json::from_str(line).expect("request json");
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok((self.responder)(&request))
    }

    fn shutdown(&self) {}
}

/// Returns a C1 `analyze` response whose hover text names the effective
/// tsconfig alias and whose definition points at the request's own script.
fn respond_with_tsconfig(request: &Value) -> String {
    let id = &request["id"];
    let params = &request["params"];
    let request_id = params["requestId"].as_str().expect("requestId");
    let revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .expect("viewRevision");
    let script = params["scriptFileId"].as_str().expect("scriptFileId");
    let paths = &params["project"]["compilerOptions"]["paths"];
    let alias = paths
        .as_object()
        .and_then(|paths| paths.keys().next())
        .expect("one tsconfig alias");
    let root = params["project"]["projectRoot"]
        .as_str()
        .expect("projectRoot");
    let fragments = params["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|fragment| {
            json!({
                "fragmentId": fragment["fragmentId"],
                "hover": {
                    "typeText": alias,
                    "typeOnly": alias,
                    "quality": "known",
                    "documentation": format!("{alias} from {root}"),
                    "documentationSource": "quickinfo"
                },
                "diagnostics": [],
                "definition": {
                    "kind": "script",
                    "fileId": script,
                    "range": {"start": 0, "end": 4}
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": 2,
            "requestId": request_id,
            "jsCheckApplied": "pragma",
            "registration": {"kind": "page", "count": 1, "topLevel": true},
            "states": {
                "hover": {"status": "supported"},
                "diagnostics": {"status": "supported"},
                "definition": {"status": "supported"}
            },
            "insertions": [{"viewRevision": revision, "backendFileVersion": 1}],
            "mounts": [],
            "fragments": fragments,
            "moduleKind": {"before": "script", "after": "script"},
            "suppressed": {"count": 0, "details": []}
        }
    }))
    .expect("response json")
}

fn start_multi_app_session(project: &TempProject, requests: Arc<Mutex<Vec<Value>>>) -> Session {
    let requests_for_factory = Arc::clone(&requests);
    Session::start_with(
        project,
        json!({
            "apps": [
                {"root": "apps/a/miniprogram", "projectRoot": "apps/a"},
                {"root": "apps/b/miniprogram", "projectRoot": "apps/b"},
            ],
            "wxml": {"styleDiagnostics": {"resources": false, "classes": true}},
        }),
        json!({}),
        move |host| {
            let host: Arc<dyn ProjectHost> = Arc::clone(&host) as _;
            let requests = Arc::clone(&requests_for_factory);
            Arc::new(NodeBackend::with_transport_factory(
                NodeBackendConfig::default(),
                host,
                move || {
                    Ok(Box::new(FakeNode {
                        requests: Arc::clone(&requests),
                        responder: Arc::new(respond_with_tsconfig),
                    }))
                },
            )) as Arc<dyn Backend>
        },
    )
}

fn app_page_uri(project: &TempProject, app: &str) -> String {
    project.uri(&format!("apps/{app}/miniprogram/pages/index/index.wxml"))
}

fn app_page_text(app: &str) -> String {
    format!("{{{{ user.name }}}}\n<view class=\"{app}-only dup\" />\n<card />")
}

/// Requests hover and definition for the `user.name` fragment.
fn backend_hover_and_definition(session: &mut Session, uri: &str, text: &str) -> (Value, Value) {
    let offset = text.find("name").expect("name") + 2;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, offset),
        }),
    );
    let hover = session.result(id);
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, offset),
        }),
    );
    let definition = session.result(id);
    (hover, definition)
}

/// IT-05: two explicit nested app roots keep their own tsconfig paths, styles
/// and same-relative-path components without crossing roots.
#[test]
fn it_05_two_explicit_nested_apps_do_not_cross_roots() {
    let project = multi_app_fixture();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut session = start_multi_app_session(&project, Arc::clone(&requests));
    assert_eq!(
        session.initialize["capabilities"]["definitionProvider"],
        json!(true)
    );

    let a_uri = app_page_uri(&project, "a");
    let a_text = app_page_text("a");
    session.open_wxml(&a_uri, &a_text, 1);
    let a_diagnostics = session.diagnostics_version(&a_uri, 1);
    assert!(
        diagnostic_codes(&a_diagnostics).is_empty(),
        "{a_diagnostics}"
    );

    let (a_hover, a_definition) = backend_hover_and_definition(&mut session, &a_uri, &a_text);
    let a_hover_text = hover_text(&a_hover);
    assert!(a_hover_text.contains("@a/*"), "{a_hover_text}");
    assert!(!a_hover_text.contains("@b/*"), "{a_hover_text}");
    assert_eq!(
        definition_uris(&a_definition),
        vec![project.uri("apps/a/miniprogram/pages/index/index.ts")],
        "{a_definition}"
    );

    let class_typed = "<view class=\"a-only \" />".to_owned();
    session.change(&a_uri, &class_typed, 2);
    let _ = session.diagnostics_version(&a_uri, 2);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": a_uri},
            "position": position_at(&class_typed, class_typed.find("\" />").expect("cursor")),
        }),
    );
    let class_completion = session.result(id);
    let a_classes = labels(&class_completion);
    assert!(a_classes.contains(&"dup".to_owned()), "{a_classes:?}");
    assert!(a_classes.contains(&"app-a".to_owned()), "{a_classes:?}");
    assert!(!a_classes.contains(&"b-only".to_owned()), "{a_classes:?}");
    assert!(!a_classes.contains(&"app-b".to_owned()), "{a_classes:?}");

    let a_tags = "<view class=\"a-only dup\" />\n<card />";
    session.change(&a_uri, a_tags, 3);
    let _ = session.diagnostics_version(&a_uri, 3);
    let dup_offset = a_tags.find("dup").expect("dup") + 1;
    let id = session.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": a_uri},
            "position": position_at(a_tags, dup_offset),
        }),
    );
    let dup_hover = session.result(id);
    let dup_hover_text = hover_text(&dup_hover);
    assert!(dup_hover_text.contains("a page dup"), "{dup_hover_text}");
    assert!(!dup_hover_text.contains("b page dup"), "{dup_hover_text}");
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": a_uri},
            "position": position_at(a_tags, dup_offset),
        }),
    );
    let dup_definition = session.result(id);
    let dup_uris = definition_uris(&dup_definition);
    assert!(!dup_uris.is_empty(), "{dup_definition}");
    assert!(
        dup_uris.iter().all(|uri| uri.contains("/apps/a/")),
        "{dup_uris:?}"
    );

    let card_offset = a_tags.find("card").expect("card") + 1;
    let id = session.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": a_uri},
            "position": position_at(a_tags, card_offset),
        }),
    );
    let card_definition = session.result(id);
    assert_eq!(
        definition_uris(&card_definition),
        vec![project.uri("apps/a/miniprogram/components/card/card.wxml")],
        "{card_definition}"
    );

    // App B resolves its own same-name, same-relative-path resources.
    let b_uri = app_page_uri(&project, "b");
    let b_text = app_page_text("b");
    session.open_wxml(&b_uri, &b_text, 1);
    let b_diagnostics = session.diagnostics_version(&b_uri, 1);
    assert!(
        diagnostic_codes(&b_diagnostics).is_empty(),
        "{b_diagnostics}"
    );

    // Both apps are loaded: host routing keeps the same relative paths apart.
    let a_file = session.host.resolve_file(&a_uri).expect("a file");
    let b_file = session.host.resolve_file(&b_uri).expect("b file");
    let a_view = session.host.view_for_file(&a_file).expect("a view");
    let b_view = session.host.view_for_file(&b_file).expect("b view");
    assert_ne!(a_view.script_file.project(), b_view.script_file.project());
    assert_eq!(a_view.script.as_str(), b_view.script.as_str());
    assert_eq!(
        session.host.uri_for(&a_view.script_file).as_deref(),
        Some(
            project
                .uri("apps/a/miniprogram/pages/index/index.ts")
                .as_str()
        )
    );
    assert_eq!(
        session.host.uri_for(&b_view.script_file).as_deref(),
        Some(
            project
                .uri("apps/b/miniprogram/pages/index/index.ts")
                .as_str()
        )
    );
    let (b_hover, b_definition) = backend_hover_and_definition(&mut session, &b_uri, &b_text);
    let b_hover_text = hover_text(&b_hover);
    assert!(b_hover_text.contains("@b/*"), "{b_hover_text}");
    assert!(!b_hover_text.contains("@a/*"), "{b_hover_text}");
    assert_eq!(
        definition_uris(&b_definition),
        vec![project.uri("apps/b/miniprogram/pages/index/index.ts")],
        "{b_definition}"
    );

    let class_typed_b = "<view class=\"b-only \" />".to_owned();
    session.change(&b_uri, &class_typed_b, 2);
    let _ = session.diagnostics_version(&b_uri, 2);
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": b_uri},
            "position": position_at(&class_typed_b, class_typed_b.find("\" />").expect("cursor")),
        }),
    );
    let b_completion = session.result(id);
    let b_classes = labels(&b_completion);
    assert!(b_classes.contains(&"app-b".to_owned()), "{b_classes:?}");
    assert!(!b_classes.contains(&"a-only".to_owned()), "{b_classes:?}");
    assert!(!b_classes.contains(&"app-a".to_owned()), "{b_classes:?}");

    // A class defined only in B is still unknown in A.
    let crossed = "<view class=\"a-only b-only\" />";
    session.change(&a_uri, crossed, 4);
    let a_diagnostics = session.diagnostics_version(&a_uri, 4);
    let unknown = a_diagnostics["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == json!("style.unknown-class"))
        .unwrap_or_else(|| panic!("cross-root class must be unknown: {a_diagnostics}"));
    assert!(
        unknown["message"]
            .as_str()
            .is_some_and(|message| message.contains("b-only")),
        "{unknown}"
    );

    // The fake Node transport saw one tsconfig path set per app and never a
    // merged set.
    let requests = requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(requests.len() >= 2, "{requests:#?}");
    for request in &requests {
        let params = &request["params"];
        let paths = params["project"]["compilerOptions"]["paths"]
            .as_object()
            .expect("paths");
        let root = params["project"]["projectRoot"]
            .as_str()
            .expect("projectRoot");
        if root.ends_with("/apps/a") {
            assert_eq!(paths.keys().cloned().collect::<Vec<_>>(), vec!["@a/*"]);
        } else if root.ends_with("/apps/b") {
            assert_eq!(paths.keys().cloned().collect::<Vec<_>>(), vec!["@b/*"]);
        } else {
            panic!("unexpected project root {root}");
        }
    }

    session.finish();
}
