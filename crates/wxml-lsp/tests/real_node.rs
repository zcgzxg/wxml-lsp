#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Optional smoke test against the real P5 Node backend.
//!
//! Build the backend first:
//!
//! ```bash
//! (cd backends/typescript && npm ci && npm run build)
//! WXML_TEST_NODE=1 cargo test -p wxml-lsp --test real_node -- --ignored
//! ```
//!
//! The test is `#[ignore]`, so `--ignored` is an explicit gate request:
//! missing `WXML_TEST_NODE=1` or a missing backend is a hard failure.

use std::path::PathBuf;
use std::sync::Arc;

use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig};
use wxml_lsp::project_host::{ProjectHost, ProjectState};
use wxml_types::{
    Backend, BackendEpoch, BackendId, DiagnosticsRequest, HoverRequest, ProjectRevision,
    RequestContext, SnapshotId, SourceRevision, SourceSnapshot,
};

mod common;

use common::TempProject;

#[test]
#[ignore = "requires Node 24 and a built P5 dist/server.js; set WXML_TEST_NODE=1"]
fn real_node_hover_and_diagnostics_smoke() {
    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the real Node smoke test"
    );
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root")
        .to_path_buf();
    let server = std::env::var_os("WXML_TYPE_BACKEND").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    let backend_dir = std::env::var_os("WXML_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);
    assert!(
        server.is_file(),
        "WXML_TEST_NODE=1 but {} is missing; run `npm ci && npm run build` in backends/typescript",
        server.display()
    );
    assert!(
        backend_dir.is_dir(),
        "WXML_TEST_NODE=1 but backend dir {} is missing",
        backend_dir.display()
    );

    let project = TempProject::new("wxml-p7-real-node");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write("pages/index/index.wxml", "{{ user.name }}");
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let host = Arc::new(ProjectState::new());
    host.open_root(Some(&project.root_uri()))
        .expect("open project");
    let path = wxml_project::ProjectPath::new("pages/index/index.wxml").expect("path");
    let file = host.file_id(&path).expect("file id");
    let snapshot = Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        "{{ user.name }}",
    ));
    let target = host.view_for(&path).expect("view");
    let context = RequestContext::new(
        ProjectRevision::new(host.project_id(), 1),
        target.view_revision,
        BackendEpoch::new(BackendId::new("node"), 1),
        Arc::clone(&snapshot),
    )
    .expect("context");

    let config = NodeBackendConfig {
        server: Some(server),
        backend_dir: Some(backend_dir),
        timeout: std::time::Duration::from_secs(20),
        ..NodeBackendConfig::default()
    };
    let backend = NodeBackend::new(config, host);
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: context.clone(),
        })
        .expect("real node diagnostics");
    assert!(
        diagnostics.status.is_supported()
            || matches!(
                diagnostics.status,
                wxml_types::CapabilityStatus::Partial { .. }
            ),
        "{diagnostics:?}"
    );
    let hover = backend
        .hover(HoverRequest {
            context,
            offset: 10,
        })
        .expect("real node hover");
    assert!(
        hover.contents.contains("string"),
        "unexpected hover: {hover:?}"
    );
}

#[test]
#[ignore = "requires Node 24 and a built P5 dist/server.js; set WXML_TEST_NODE=1"]
#[allow(clippy::too_many_lines)]
fn real_node_expression_and_event_completion_smoke() {
    use wxml_types::{CompletionKind, CompletionRequest};

    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the real Node smoke test"
    );
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root")
        .to_path_buf();
    let server = std::env::var_os("WXML_TYPE_BACKEND").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    let backend_dir = std::env::var_os("WXML_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);

    let project = TempProject::new("wxml-wave5-completion");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    let text = r#"<view>{{ user.na }}</view><view bindtap="onT" /><view>{{  }}</view><view>{{ user. }}</view>"#;
    project.write("pages/index/index.wxml", text);
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada', age: 36 } }, onTap() {}, onLoad() {} });",
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let host = Arc::new(ProjectState::new());
    host.open_root(Some(&project.root_uri()))
        .expect("open project");
    let path = wxml_project::ProjectPath::new("pages/index/index.wxml").expect("path");
    let file = host.file_id(&path).expect("file id");
    let snapshot = Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        text,
    ));
    let target = host.view_for(&path).expect("view");
    let context = RequestContext::new(
        ProjectRevision::new(host.project_id(), 1),
        target.view_revision,
        BackendEpoch::new(BackendId::new("node"), 1),
        Arc::clone(&snapshot),
    )
    .expect("context");
    let config = NodeBackendConfig {
        server: Some(server),
        backend_dir: Some(backend_dir),
        timeout: std::time::Duration::from_secs(20),
        ..NodeBackendConfig::default()
    };
    let backend = NodeBackend::new(config, host);

    let member_offset = u32::try_from(text.find("na").expect("member") + 2).expect("offset");
    let members = backend
        .completion(CompletionRequest {
            context: context.clone(),
            offset: member_offset,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("expression completion");
    assert!(
        members.items.iter().any(|item| item.label == "name"),
        "{members:?}"
    );

    let handler_offset = u32::try_from(text.find("onT").expect("handler") + 3).expect("offset");
    let handlers = backend
        .completion(CompletionRequest {
            context: context.clone(),
            offset: handler_offset,
            kind: CompletionKind::EventHandler,
        })
        .expect("event completion");
    assert!(
        handlers.items.iter().any(|item| item.label == "onTap"),
        "{handlers:?}"
    );
    assert!(
        handlers
            .items
            .iter()
            .filter(|item| item.label == "onTap")
            .all(|item| item.replace.start() == handler_offset - 3)
    );

    let root_offset =
        u32::try_from(text.find("{{  }}").expect("empty mustache") + 3).expect("offset");
    let roots = backend
        .completion(CompletionRequest {
            context: context.clone(),
            offset: root_offset,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("root completion");
    assert!(
        roots.items.iter().any(|item| item.label == "user"),
        "{roots:?}"
    );

    let dot_offset = u32::try_from(text.rfind("user.").expect("member dot") + 5).expect("offset");
    let members = backend
        .completion(CompletionRequest {
            context,
            offset: dot_offset,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("trailing-dot completion");
    assert!(
        members.items.iter().any(|item| item.label == "name"),
        "{members:?}"
    );
}

/// WT-C end-to-end: event-handler diagnostics, definition and completion over
/// the real Node backend, including component `methods`, arrow handlers and
/// degraded (dynamic / non-identifier) forms.
#[test]
#[ignore = "requires Node 24 and a built P5 dist/server.js; set WXML_TEST_NODE=1"]
#[allow(clippy::too_many_lines)]
fn real_node_event_handler_diagnostics_definition_and_completion() {
    use wxml_types::{
        CapabilityStatus, CompletionKind, CompletionRequest, DefinitionRequest, DiagnosticSeverity,
    };

    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the real Node smoke test"
    );
    let (server, backend_dir) = real_node_backend_paths();

    let project = TempProject::new("wxml-wave5-events");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write(
        "pages/index/index.json",
        r#"{"usingComponents":{"widget":"../../components/widget/widget"}}"#,
    );
    let page_wxml = concat!(
        "<view bindtap=\"onTap\" catchtap=\"missingHandler\" />",
        "<view onTapPlaceholder=\"\" capture-bind:tap=\"onTap\" mut-bind:tap=\"onTap\" />",
        "<view bindtap=\"{{ dynamicHandler }}\" catchtap=\"\" bindtap=\"this.invalid\" />",
        "<view bindtap=\"onArrow\" />",
        "<widget bindtap=\"onWidgetTap\" />",
    );
    project.write("pages/index/index.wxml", page_wxml);
    project.write(
        "pages/index/index.ts",
        "Page({\n  data: { label: 'x' },\n  onTap(event: { detail: { x: number } }) { void event; },\n  onArrow: (event: { detail: { y: number } }) => { void event; },\n});\n",
    );
    project.write("components/widget/widget.json", r#"{"component":true}"#);
    project.write(
        "components/widget/widget.wxml",
        "<view bindtap=\"onWidgetTap\" /><view bindtap=\"missingWidget\" />",
    );
    project.write(
        "components/widget/widget.ts",
        "Component({\n  properties: {},\n  methods: {\n    onWidgetTap(event: { detail: { z: number } }) { void event; },\n    onWidgetArrow: () => {},\n  },\n});\n",
    );
    project.write(
        "tsconfig.json",
        r#"{"compilerOptions":{"strict":true,"target":"ES2020","module":"CommonJS"},"include":["**/*.ts","**/*.js"]}"#,
    );

    let host = Arc::new(ProjectState::new());
    host.open_root(Some(&project.root_uri()))
        .expect("open project");
    let backend = NodeBackend::new(
        NodeBackendConfig {
            server: Some(server),
            backend_dir: Some(backend_dir),
            timeout: std::time::Duration::from_secs(30),
            ..NodeBackendConfig::default()
        },
        host.clone(),
    );

    let context_for = |relative: &str, text: &str| {
        let path = wxml_project::ProjectPath::new(relative).expect("path");
        let file = host.file_id(&path).expect("file id");
        let target = host.view_for(&path).expect("view");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            text,
        ));
        RequestContext::new(
            ProjectRevision::new(host.project_id(), 1),
            target.view_revision,
            BackendEpoch::new(BackendId::new("node"), 1),
            Arc::clone(&snapshot),
        )
        .expect("context")
    };
    let page_context = context_for("pages/index/index.wxml", page_wxml);
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: page_context.clone(),
        })
        .expect("page diagnostics");
    let missing_start = page_wxml.find("missingHandler").expect("missing handler");
    let missing_end = missing_start + "missingHandler".len();
    let missing = diagnostics
        .diagnostics
        .iter()
        .find(|diagnostic| {
            usize::try_from(diagnostic.range.start()).ok() == Some(missing_start)
                && usize::try_from(diagnostic.range.end()).ok() == Some(missing_end)
        })
        .unwrap_or_else(|| {
            panic!(
                "no missing-handler diagnostic (status {:?}): {:?}",
                diagnostics.status, diagnostics.diagnostics
            )
        });
    assert_eq!(missing.severity, DiagnosticSeverity::Error);
    assert!(missing.message.contains("missingHandler"), "{missing:?}");
    assert!(missing.message.contains("tap"), "{missing:?}");
    for marker in ["{{ dynamicHandler }}", "catchtap=\"\"", "this.invalid"] {
        let start = page_wxml.find(marker).expect("degraded marker");
        let end = start + marker.len();
        assert!(
            diagnostics.diagnostics.iter().all(|diagnostic| {
                let diagnostic_start = usize::try_from(diagnostic.range.start()).unwrap_or(0);
                let diagnostic_end = usize::try_from(diagnostic.range.end()).unwrap_or(0);
                !(diagnostic_start < end && start < diagnostic_end)
            }),
            "{marker:?} produced a diagnostic: {:?}",
            diagnostics.diagnostics
        );
    }

    let page_script = std::fs::read_to_string(project.root().join("pages/index/index.ts"))
        .expect("read page script");
    let definition_at = |offset: usize| {
        backend
            .definition(DefinitionRequest {
                context: page_context.clone(),
                offset: u32::try_from(offset).expect("offset"),
            })
            .expect("definition")
    };
    let on_tap_start = page_wxml.find("onTap").expect("onTap");
    let on_tap = definition_at(on_tap_start);
    let on_tap_location = on_tap
        .locations
        .iter()
        .find(|location| location.file.path() == "pages/index/index.ts")
        .unwrap_or_else(|| panic!("onTap definition missing: {on_tap:?}"));
    let range_start = usize::try_from(on_tap_location.range.start()).expect("start");
    let range_end = usize::try_from(on_tap_location.range.end()).expect("end");
    assert_eq!(&page_script[range_start..range_end], "onTap");

    let hover = backend
        .hover(HoverRequest {
            context: page_context.clone(),
            offset: u32::try_from(on_tap_start).expect("offset"),
        })
        .expect("onTap hover");
    assert!(
        hover.contents.contains("onTap"),
        "handler hover: {:?}",
        hover.contents
    );
    assert_eq!(
        hover.range.map(|range| (range.start(), range.end())),
        Some((
            u32::try_from(on_tap_start).expect("start"),
            u32::try_from(on_tap_start + "onTap".len()).expect("end")
        ))
    );

    let on_arrow_start = page_wxml.find("onArrow").expect("onArrow");
    let on_arrow = definition_at(on_arrow_start);
    let on_arrow_location = on_arrow
        .locations
        .iter()
        .find(|location| location.file.path() == "pages/index/index.ts")
        .unwrap_or_else(|| panic!("onArrow definition missing: {on_arrow:?}"));
    let range_start = usize::try_from(on_arrow_location.range.start()).expect("start");
    let range_end = usize::try_from(on_arrow_location.range.end()).expect("end");
    assert_eq!(&page_script[range_start..range_end], "onArrow");

    let missing_definition = definition_at(missing_start);
    assert!(
        missing_definition.locations.is_empty(),
        "{missing_definition:?}"
    );
    assert!(
        missing_definition.status.is_supported(),
        "a missing handler is a supported check with no definition, got {:?}",
        missing_definition.status
    );

    let completion_offset = page_wxml.find("onA").expect("completion prefix") + "onA".len();
    let handlers = backend
        .completion(CompletionRequest {
            context: page_context.clone(),
            offset: u32::try_from(completion_offset).expect("offset"),
            kind: CompletionKind::EventHandler,
        })
        .expect("event completion");
    for label in ["onTap", "onArrow"] {
        assert!(
            handlers.items.iter().any(|item| item.label == label),
            "missing {label} in {handlers:?}"
        );
    }
    assert!(
        handlers
            .items
            .iter()
            .all(|item| item.label != "label" && !item.label.starts_with("__wxml_")),
        "reserved/data members leaked into event completion: {handlers:?}"
    );

    let widget_wxml = std::fs::read_to_string(project.root().join("components/widget/widget.wxml"))
        .expect("read widget wxml");
    let widget_context = context_for("components/widget/widget.wxml", &widget_wxml);
    let widget_diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: widget_context.clone(),
        })
        .expect("widget diagnostics");
    let widget_missing_start = widget_wxml.find("missingWidget").expect("widget missing");
    assert!(
        widget_diagnostics.diagnostics.iter().any(|diagnostic| {
            usize::try_from(diagnostic.range.start()).ok() == Some(widget_missing_start)
                && diagnostic.severity == DiagnosticSeverity::Error
                && diagnostic.message.contains("missingWidget")
        }),
        "component methods lookup did not flag missingWidget: {:?}",
        widget_diagnostics.diagnostics
    );
    let widget_tap_start = widget_wxml.find("onWidgetTap").expect("onWidgetTap");
    let widget_definition = backend
        .definition(DefinitionRequest {
            context: widget_context,
            offset: u32::try_from(widget_tap_start).expect("offset"),
        })
        .expect("widget definition");
    assert!(
        widget_definition
            .locations
            .iter()
            .any(|location| location.file.path() == "components/widget/widget.ts"),
        "{widget_definition:?}"
    );
    assert!(matches!(
        widget_definition.status,
        CapabilityStatus::Supported | CapabilityStatus::Partial { .. }
    ));
}

fn real_node_backend_paths() -> (PathBuf, PathBuf) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root")
        .to_path_buf();
    let server = std::env::var_os("WXML_TYPE_BACKEND").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    let backend_dir = std::env::var_os("WXML_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);
    assert!(
        server.is_file(),
        "WXML_TEST_NODE=1 but {} is missing; run `npm ci && npm run build` in backends/typescript",
        server.display()
    );
    assert!(
        backend_dir.is_dir(),
        "WXML_TEST_NODE=1 but backend dir {} is missing",
        backend_dir.display()
    );
    (server, backend_dir)
}
