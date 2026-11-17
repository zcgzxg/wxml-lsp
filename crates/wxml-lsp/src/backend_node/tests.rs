#![allow(clippy::assert_is_empty)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use wxml_text::SourceRange;
use wxml_types::{
    Backend, BackendEpoch, BackendId, CheckFragment, DefinitionRequest, Diagnostic,
    DiagnosticSeverity, DiagnosticsRequest, EventBindingKind, EventHandler, FileId, HoverRequest,
    LocalBinding, LocalRole, MarkupKind, ProjectId, ProjectRevision, RequestContext, RootKind,
    SnapshotId, SourceRevision, SourceSnapshot, TypeQuality, ViewRevision,
};

use super::config::NodeBackendConfig;
use super::process::{NodeTransport, NodeTransportError};
use super::response::MAX_DEFINITION_LOCATIONS;
use super::*;
use crate::project_host::{
    ChangeDocument, DocumentRoute, HostError, HostUpdate, OpenDocument, PreparedFile, ProjectHost,
    ViewTarget,
};

const WXML: &str = "pages/index/index.wxml";
const SCRIPT: &str = "pages/index/index.ts";

fn project() -> ProjectId {
    ProjectId::new("proj-test")
}

fn file(path: &str) -> FileId {
    FileId::new(project(), path).expect("file")
}

fn fragment(bindings: Vec<LocalBinding>) -> CheckFragment {
    CheckFragment {
        fragment_id: "f0".to_owned(),
        expr_text: "user.name".to_owned(),
        target: Some("name".to_owned()),
        bindings,
        guards: Vec::new(),
        wxml_range: SourceRange::from_bounds(3, 12).expect("range"),
        fragment_revision: 1,
        root: RootKind::Page,
    }
}

fn view_without_target(bindings: Vec<LocalBinding>) -> PreparedView {
    let mut prepared = view(bindings);
    prepared.target = None;
    prepared.status_hint = Some(CapabilityStatus::Unsupported {
        reason: "no project view".to_owned(),
    });
    prepared
}

fn view(bindings: Vec<LocalBinding>) -> PreparedView {
    let script = file(SCRIPT);
    PreparedView {
        view_id: "pages/index/index.wxml:1".to_owned(),
        target: Some(ViewTarget {
            wxml: wxml_project::ProjectPath::new(WXML).expect("wxml"),
            owner: RootKind::Page,
            script: wxml_project::ProjectPath::new(SCRIPT).expect("script"),
            script_file: script.clone(),
            view_revision: ViewRevision::new(1),
        }),
        files: Arc::from(vec![PreparedFile {
            file: script,
            path: wxml_project::ProjectPath::new(SCRIPT).expect("script"),
            kind: "ts",
            text: Arc::from("Page({ data: { user: { name: 'Ada' } } });"),
            source_revision: 1,
        }]),
        compiler_options: serde_json::Map::new(),
        fragments: Arc::from(vec![fragment(bindings)]),
        components: Arc::from(Vec::new()),
        events: Arc::from(Vec::new()),
        project_root: std::path::PathBuf::from("/tmp/project"),
        syntax_diagnostics: Vec::new(),
        status_hint: None,
        source_complete: true,
    }
}

/// A view with one static `bindtap="onTap"` handler at bytes 10..15.
///
/// The context template is `{{ user.name }}` (13 bytes), so offset 12 falls
/// inside the handler token without touching the fragment target.
fn view_with_events() -> PreparedView {
    let mut prepared = view(Vec::new());
    prepared.events = Arc::from(vec![EventHandler {
        attribute: "bindtap".to_owned(),
        binding: EventBindingKind::Bind,
        event: "tap".to_owned(),
        handler: "onTap".to_owned(),
        range: SourceRange::from_bounds(2, 16).expect("range"),
        attribute_range: SourceRange::from_bounds(2, 16).expect("range"),
        handler_range: SourceRange::from_bounds(10, 15).expect("range"),
    }]);
    prepared
}

/// A backend whose fake Node answers `analyze` with `event_results`.
fn backend_with_events(shared: &Shared, host: StubHost, event_results: Value) -> NodeBackend {
    backend_with(shared, host, move |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        value["result"]["eventResults"] = event_results.clone();
        Ok(serde_json::to_string(&value).expect("json"))
    })
}

fn context() -> RequestContext {
    RequestContext::new(
        ProjectRevision::new(project(), 1),
        ViewRevision::new(1),
        BackendEpoch::new(BackendId::new("node"), 1),
        Arc::new(SourceSnapshot::new(
            file(WXML),
            SourceRevision::new(1),
            SnapshotId::new(1),
            "{{ user.name }}",
        )),
    )
    .expect("context")
}

struct StubHost {
    view: PreparedView,
    fail: bool,
}

impl StubHost {
    fn new(view: PreparedView) -> Self {
        Self { view, fail: false }
    }

    fn failing(view: PreparedView) -> Self {
        Self { view, fail: true }
    }
}

impl ProjectHost for StubHost {
    fn open_root(&self, _root_uri: Option<&str>) -> Result<(), HostError> {
        Ok(())
    }

    fn project_id(&self) -> ProjectId {
        project()
    }

    fn project_revision(&self) -> ProjectRevision {
        ProjectRevision::new(project(), 1)
    }

    fn path_for_uri(&self, _uri: &str) -> Result<wxml_project::ProjectPath, HostError> {
        Err(HostError::NoProject)
    }

    fn uri_for(&self, file: &FileId) -> Option<String> {
        Some(format!("file:///project/{}", file.path()))
    }

    fn file_id(&self, path: &wxml_project::ProjectPath) -> Result<FileId, HostError> {
        FileId::new(project(), path.as_str()).map_err(|_| HostError::InvalidUri)
    }

    fn route(&self, _uri: &str) -> DocumentRoute {
        DocumentRoute::Unknown
    }

    fn view_for(&self, _wxml: &wxml_project::ProjectPath) -> Result<ViewTarget, HostError> {
        Err(HostError::Unsupported { reason: "stub" })
    }

    fn open_document(&self, _open: OpenDocument) -> Result<HostUpdate, HostError> {
        Ok(HostUpdate::default())
    }

    fn change_document(&self, _change: ChangeDocument) -> Result<HostUpdate, HostError> {
        Ok(HostUpdate::default())
    }

    fn close_document(&self, _uri: &str) -> Result<HostUpdate, HostError> {
        Ok(HostUpdate::default())
    }

    fn set_wxml_snapshot(&self, _file: FileId, _snapshot: Arc<SourceSnapshot>) {}

    fn remove_wxml_snapshot(&self, _file: &FileId) {}

    fn source(&self, _file: &FileId) -> Option<Arc<SourceSnapshot>> {
        None
    }

    fn file_text(&self, _file: &FileId) -> Option<Arc<str>> {
        Some(Arc::from("Page({ data: { user: { name: 'Ada' } } });"))
    }

    fn analysis(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        if self.fail {
            return Err(HostError::NoProject);
        }
        Ok(self.view.clone())
    }
}

type Responder = Arc<dyn Fn(&str) -> Result<String, NodeTransportError> + Send + Sync + 'static>;

#[derive(Clone, Default)]
struct Shared {
    requests: Arc<Mutex<Vec<String>>>,
    connects: Arc<AtomicUsize>,
}

struct FakeTransport {
    shared: Shared,
    responder: Responder,
}

impl NodeTransport for FakeTransport {
    fn request(&self, line: &str, _timeout: Duration) -> Result<String, NodeTransportError> {
        self.shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_owned());
        (self.responder)(line)
    }

    fn shutdown(&self) {}
}

fn backend(shared: &Shared, host: StubHost) -> NodeBackend {
    backend_with(shared, host, |line| Ok(respond(line)))
}

fn backend_with(
    shared: &Shared,
    host: StubHost,
    responder: impl Fn(&str) -> Result<String, NodeTransportError> + Send + Sync + 'static,
) -> NodeBackend {
    let shared = shared.clone();
    let responder: Responder = Arc::new(responder);
    NodeBackend::with_transport_factory(NodeBackendConfig::default(), Arc::new(host), move || {
        shared.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeTransport {
            shared: shared.clone(),
            responder: Arc::clone(&responder),
        }))
    })
}

fn respond(line: &str) -> String {
    let request: Value = serde_json::from_str(line).expect("request json");
    let id = &request["id"];
    let params = &request["params"];
    let request_id = params["requestId"].as_str().expect("requestId");
    let revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .expect("viewRevision");
    let fragments = params["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|fragment| {
            let start = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3);
            let value = start + 5;
            json!({
                "fragmentId": fragment["fragmentId"],
                "hover": {
                    "typeText": "string",
                    "typeOnly": "string",
                    "quality": "known",
                    "documentation": "the name",
                    "documentationSource": "quickinfo"
                },
                "hovers": [{
                    "range": {"start": start, "end": start + 4},
                    "typeText": "(property) user: { name: string; }",
                    "typeOnly": "{ name: string; }",
                    "quality": "known",
                    "documentation": "the user",
                    "documentationSource": "quickinfo"
                }, {
                    "range": {"start": value, "end": value + 4},
                    "typeText": "string",
                    "typeOnly": "string",
                    "quality": "known",
                    "documentation": "the name",
                    "documentationSource": "quickinfo"
                }],
                "diagnostics": [{
                    "code": 2339,
                    "range": {"start": value, "end": value + 4},
                    "message": "Property 'nmae' does not exist",
                    "attribution": "source"
                }],
                "definition": {
                    "kind": "script",
                    "fileId": SCRIPT,
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
            "insertions": [{
                "viewRevision": revision,
                "backendFileVersion": 1
            }],
            "mounts": [],
            "fragments": fragments,
            "moduleKind": {"before": "script", "after": "script"},
            "suppressed": {"count": 0, "details": []}
        }
    }))
    .expect("response json")
}

fn requests(shared: &Shared) -> usize {
    shared
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len()
}

#[test]
fn one_analysis_serves_all_three_capabilities() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let context = context();
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: context.clone(),
        })
        .expect("diagnostics");
    assert!(diagnostics.status.is_supported());
    assert_eq!(diagnostics.diagnostics.len(), 1);
    let hover = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 10,
        })
        .expect("hover");
    assert!(hover.contents.contains("string"));
    let definition = backend
        .definition(DefinitionRequest {
            context,
            offset: 10,
        })
        .expect("definition");
    assert_eq!(definition.locations.len(), 1);
    assert_eq!(requests(&shared), 1);
    assert_eq!(shared.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn a_second_revision_requests_a_new_analysis() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let context = context();
    let _ = backend.diagnostics(DiagnosticsRequest {
        context: context.clone(),
    });
    let second = RequestContext::new(
        context.project_revision(),
        ViewRevision::new(2),
        context.backend_epoch(),
        Arc::new(context.source().clone()),
    )
    .expect("context");
    let _ = backend.diagnostics(DiagnosticsRequest { context: second });
    assert_eq!(requests(&shared), 2);
}

#[test]
fn hover_answers_the_identifier_under_the_pointer() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let context = context();

    let user = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 5,
        })
        .expect("hover user");
    assert_eq!(user.quality, TypeQuality::Known);
    assert_eq!(user.markup, MarkupKind::Markdown);
    assert!(user.contents.contains("name: string"), "{}", user.contents);
    assert!(user.contents.contains("the user"), "{}", user.contents);
    assert_eq!(
        user.range.map(|range| (range.start(), range.end())),
        Some((3, 7))
    );

    let name = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 10,
        })
        .expect("hover name");
    assert_eq!(name.quality, TypeQuality::Known);
    assert_eq!(
        name.range.map(|range| (range.start(), range.end())),
        Some((8, 12))
    );
    assert!(name.contents.contains("the name"), "{}", name.contents);

    // The end of the expression is a valid point (right edge of the text).
    let end = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 12,
        })
        .expect("hover end");
    assert_eq!(
        end.range.map(|range| (range.start(), range.end())),
        Some((8, 12))
    );

    let outside = backend
        .hover(HoverRequest { context, offset: 0 })
        .expect("hover outside");
    assert!(outside.contents.is_empty());
    assert_eq!(outside.range, None);
    assert_eq!(requests(&shared), 1);
}

#[test]
fn diagnostics_keep_syntax_and_drop_unattributable_or_invalid_ranges() {
    let shared = Shared::default();
    let mut prepared = view(Vec::new());
    prepared.syntax_diagnostics.push(Diagnostic {
        range: SourceRange::from_bounds(0, 1).expect("range"),
        severity: DiagnosticSeverity::Error,
        message: "syntax".to_owned(),
        code: None,
    });
    let backend = backend_with(&shared, StubHost::new(prepared), |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        value["result"]["fragments"][0]["diagnostics"] = json!([
            {"code": 1, "range": {"start": 8, "end": 12}, "message": "valid", "attribution": "source"},
            {"code": 1, "range": {"start": 8, "end": 12}, "message": "duplicate", "attribution": "source"},
            {"code": 2, "range": {"start": 0, "end": 1}, "message": "synthetic", "attribution": "unattributable"},
            {"code": 3, "range": {"start": 900, "end": 901}, "message": "outside", "attribution": "source"}
        ]);
        Ok(serde_json::to_string(&value).expect("json"))
    });
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
    assert_eq!(result.diagnostics[0].message, "syntax");
    assert_eq!(result.diagnostics[1].message, "valid");
}

#[test]
fn truncated_response_is_partial_and_does_not_restart() {
    let shared = Shared::default();
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        let template = value["result"]["fragments"][0].clone();
        let diagnostics = serde_json::json!([
            {"code": 1, "range": {"start": 8, "end": 12}, "message": "a", "attribution": "source"},
            {"code": 2, "range": {"start": 8, "end": 12}, "message": "b", "attribution": "source"},
            {"code": 3, "range": {"start": 8, "end": 12}, "message": "c", "attribution": "source"},
            {"code": 4, "range": {"start": 8, "end": 12}, "message": "d", "attribution": "source"},
            {"code": 5, "range": {"start": 8, "end": 12}, "message": "e", "attribution": "source"}
        ]);
        let mut fragments = Vec::new();
        for _ in 0..2_048 {
            let mut fragment = template.clone();
            fragment["diagnostics"] = diagnostics.clone();
            fragments.push(fragment);
        }
        value["result"]["fragments"] = Value::Array(fragments);
        Ok(serde_json::to_string(&value).expect("json"))
    });
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("truncated diagnostics");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(requests(&shared), 1);
}

#[test]
fn node_failure_keeps_syntax_and_advances_the_epoch() {
    let shared = Shared::default();
    let mut prepared = view(Vec::new());
    prepared.syntax_diagnostics.push(Diagnostic {
        range: SourceRange::from_bounds(0, 1).expect("range"),
        severity: DiagnosticSeverity::Error,
        message: "syntax".to_owned(),
        code: None,
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let responder_attempts = Arc::clone(&attempts);
    let backend = backend_with(&shared, StubHost::new(prepared), move |line| {
        if responder_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(NodeTransportError::Timeout)
        } else {
            Ok(respond(line))
        }
    });
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert_eq!(result.diagnostics.len(), 1);
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(backend.epoch().get(), 2);

    let hover = backend
        .hover(HoverRequest {
            context: context(),
            offset: 10,
        })
        .expect("hover after restart");
    assert!(hover.contents.contains("string"));
    assert_eq!(shared.connects.load(Ordering::SeqCst), 2);
    assert_eq!(backend.epoch().get(), 2);
}

#[test]
fn hover_escapes_markdown_and_uses_a_safe_fence() {
    let shared = Shared::default();
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        value["result"]["fragments"][0]["hovers"][1]["typeText"] = json!("a```b");
        value["result"]["fragments"][0]["hovers"][1]["documentation"] =
            json!("](command:evil) ![x](https://x) <b>");
        Ok(serde_json::to_string(&value).expect("json"))
    });
    let hover = backend
        .hover(HoverRequest {
            context: context(),
            offset: 10,
        })
        .expect("hover");
    assert!(hover.contents.contains("````ts"), "{}", hover.contents);
    assert!(!hover.contents.contains("](command:"), "{}", hover.contents);
    assert!(!hover.contents.contains("![x]("), "{}", hover.contents);
}

#[test]
fn no_target_returns_null_hover_and_definition_but_keeps_diagnostics() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view_without_target(Vec::new())));
    assert!(
        backend
            .hover(HoverRequest {
                context: context(),
                offset: 10
            })
            .is_err()
    );
    assert!(
        backend
            .definition(DefinitionRequest {
                context: context(),
                offset: 10
            })
            .is_err()
    );
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(requests(&shared), 0);
}

#[test]
fn zero_fragments_do_not_start_node() {
    let shared = Shared::default();
    let mut prepared = view(Vec::new());
    prepared.fragments = Arc::from(Vec::new());
    let backend = backend(&shared, StubHost::new(prepared));
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert!(result.status.is_supported());
    assert!(result.diagnostics.is_empty());
    assert_eq!(requests(&shared), 0);
    assert_eq!(shared.connects.load(Ordering::SeqCst), 0);
}

#[test]
fn bindings_do_not_force_partial_status() {
    let shared = Shared::default();
    let bindings = vec![
        LocalBinding {
            name: "item".to_owned(),
            role: LocalRole::Item,
            source_expr: "groups".to_owned(),
            source_range: SourceRange::from_bounds(100, 106).expect("range"),
        },
        LocalBinding {
            name: "index".to_owned(),
            role: LocalRole::Index,
            source_expr: "groups".to_owned(),
            source_range: SourceRange::from_bounds(100, 106).expect("range"),
        },
    ];
    let backend = backend(&shared, StubHost::new(view(bindings)));
    let context = context();
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: context.clone(),
        })
        .expect("diagnostics");
    assert!(diagnostics.status.is_supported());
    let hover = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 10,
        })
        .expect("hover");
    assert!(hover.status.is_supported());
    let definition = backend
        .definition(DefinitionRequest {
            context,
            offset: 10,
        })
        .expect("definition");
    assert!(definition.status.is_supported());
}

fn many_definitions(shared: &Shared, host: StubHost, distinct: bool) -> NodeBackend {
    backend_with(shared, host, move |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        let template = value["result"]["fragments"][0].clone();
        let mut fragments = Vec::new();
        for index in 0..300_u32 {
            let mut fragment = template.clone();
            let start = if distinct { index } else { 0 };
            fragment["definition"]["range"] = json!({"start": start, "end": start + 1});
            fragments.push(fragment);
        }
        value["result"]["fragments"] = Value::Array(fragments);
        Ok(serde_json::to_string(&value).expect("json"))
    })
}

#[test]
fn definition_locations_are_deduplicated_and_capped() {
    let shared = Shared::default();
    let backend = many_definitions(&shared, StubHost::new(view(Vec::new())), true);
    let result = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 10,
        })
        .expect("definition");
    assert_eq!(result.locations.len(), MAX_DEFINITION_LOCATIONS);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(requests(&shared), 1);
}

#[test]
fn duplicate_definition_locations_are_dropped_and_marked_partial() {
    let shared = Shared::default();
    let backend = many_definitions(&shared, StubHost::new(view(Vec::new())), false);
    let result = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 10,
        })
        .expect("definition");
    assert_eq!(result.locations.len(), 1);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn disabled_diagnostics_drop_type_diagnostics_but_keep_syntax() {
    let shared = Shared::default();
    let mut prepared = view(Vec::new());
    prepared.syntax_diagnostics.push(Diagnostic {
        range: SourceRange::from_bounds(0, 1).expect("range"),
        severity: DiagnosticSeverity::Error,
        message: "syntax".to_owned(),
        code: None,
    });
    let backend = backend_with(&shared, StubHost::new(prepared), |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        value["result"]["states"]["diagnostics"] =
            json!({"status": "disabled", "reason": "@ts-nocheck"});
        Ok(serde_json::to_string(&value).expect("json"))
    });
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "syntax");
    assert!(matches!(result.status, CapabilityStatus::Disabled { .. }));
}

#[test]
fn repeated_failures_enter_a_cooldown() {
    let shared = Shared::default();
    let config = NodeBackendConfig {
        restart_cooldown: Duration::from_secs(60),
        ..NodeBackendConfig::default()
    };
    let shared_for_factory = shared.clone();
    let backend = NodeBackend::with_transport_factory(
        config,
        Arc::new(StubHost::new(view(Vec::new()))),
        move || {
            shared_for_factory.connects.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeTransport {
                shared: shared_for_factory.clone(),
                responder: Arc::new(|_line| Err(NodeTransportError::Timeout)),
            }))
        },
    );
    for _ in 0..3 {
        let _ = backend.diagnostics(DiagnosticsRequest { context: context() });
    }
    assert_eq!(shared.connects.load(Ordering::SeqCst), 3);
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("cooldown diagnostics");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(shared.connects.load(Ordering::SeqCst), 3);
}

#[test]
fn dropped_definition_is_partial_and_has_no_location() {
    let shared = Shared::default();
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        value["result"]["fragments"][0]["definition"] = json!({
            "kind": "dropped",
            "reason": "generated declaration"
        });
        Ok(serde_json::to_string(&value).expect("json"))
    });
    let result = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 10,
        })
        .expect("definition");
    assert!(result.locations.is_empty());
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn definition_maps_file_ids_through_the_host() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let result = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 10,
        })
        .expect("definition");
    assert_eq!(result.locations.len(), 1);
    assert_eq!(result.locations[0].file.path(), SCRIPT);
}

#[test]
fn revision_mismatch_invalidates_the_cache_and_reports_protocol() {
    let shared = Shared::default();
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), |line| {
        let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
        value["result"]["insertions"][0]["viewRevision"] = json!(99);
        Ok(serde_json::to_string(&value).expect("json"))
    });
    let result = backend.hover(HoverRequest {
        context: context(),
        offset: 10,
    });
    assert!(matches!(result, Err(BackendError::Protocol(_))));
    assert_eq!(backend.epoch().get(), 2);
}

#[test]
fn missing_project_view_keeps_syntax_diagnostics() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::failing(view(Vec::new())));
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert!(result.diagnostics.is_empty());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert!(
        backend
            .hover(HoverRequest {
                context: context(),
                offset: 10
            })
            .is_err()
    );
    assert_eq!(requests(&shared), 0);
}

#[test]
fn unconfigured_backend_returns_unavailable_without_spawning() {
    let config = NodeBackendConfig {
        server: None,
        backend_dir: None,
        ..NodeBackendConfig::default()
    };
    let backend = NodeBackend::new(config, Arc::new(StubHost::new(view(Vec::new()))));
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert!(
        backend
            .hover(HoverRequest {
                context: context(),
                offset: 10
            })
            .is_err()
    );
}

#[test]
fn capabilities_are_four_supported_features() {
    let backend = backend(&Shared::default(), StubHost::new(view(Vec::new())));
    let capabilities = backend.capabilities();
    assert!(capabilities.hover.is_supported());
    assert!(capabilities.diagnostics.is_supported());
    assert!(capabilities.definition.is_supported());
    assert!(capabilities.completion.is_supported());
}

#[test]
fn completion_prefix_keeps_the_tail_of_a_long_token() {
    let text = format!("{}onTap", "a".repeat(300));
    let prefix = completion_prefix(&text, u32::try_from(text.len()).expect("offset"));
    assert_eq!(prefix.len(), 256);
    assert!(prefix.ends_with("onTap"));
    assert!(text.ends_with(&prefix));
}

fn last_request(shared: &Shared) -> Value {
    let line = shared
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .last()
        .cloned()
        .expect("a request");
    serde_json::from_str(&line).expect("request json")
}

#[test]
fn event_results_flow_into_diagnostics_and_definition() {
    let shared = Shared::default();
    let backend = backend_with_events(
        &shared,
        StubHost::new(view_with_events()),
        json!([{
            "eventId": "e0",
            "state": {"status": "supported"},
            "definition": {"kind": "script", "fileId": SCRIPT, "range": {"start": 5, "end": 10}},
            "signature": "onTap(event: { detail: { x: number } })",
            "diagnostic": {
                "range": {"start": 10, "end": 15},
                "severity": "error",
                "message": "Cannot find name 'onTap' for event tap"
            }
        }]),
    );
    let context = context();
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: context.clone(),
        })
        .expect("diagnostics");
    let event = diagnostics
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("onTap"))
        .expect("event diagnostic");
    assert_eq!(event.severity, DiagnosticSeverity::Error);
    assert_eq!((event.range.start(), event.range.end()), (10, 15));
    assert!(diagnostics.status.is_supported());

    // Offset 12 is inside both the event handler and the fragment target; the
    // event result must win and must not fall through to the fragment.
    let definition = backend
        .definition(DefinitionRequest {
            context: context.clone(),
            offset: 12,
        })
        .expect("definition");
    assert_eq!(definition.locations.len(), 1);
    assert_eq!(definition.locations[0].file.path(), SCRIPT);
    assert_eq!(
        (
            definition.locations[0].range.start(),
            definition.locations[0].range.end()
        ),
        (5, 10)
    );
    assert!(definition.status.is_supported());

    // The same event must also answer hover with the backend signature, proving
    // the cached event list is passed into the response mapper.
    let hover = backend
        .hover(HoverRequest {
            context,
            offset: 12,
        })
        .expect("hover");
    assert!(
        hover
            .contents
            .contains("onTap(event: { detail: { x: number } })")
    );
    assert_eq!(
        hover.range.map(|range| (range.start(), range.end())),
        Some((10, 15))
    );
    assert_eq!(hover.quality, TypeQuality::Known);
    assert!(hover.status.is_supported());

    assert_eq!(requests(&shared), 1);
    let request = last_request(&shared);
    assert_eq!(request["params"]["events"][0]["eventId"], "e0");
    assert_eq!(request["params"]["events"][0]["handler"], "onTap");
    assert_eq!(request["params"]["events"][0]["event"], "tap");
}

#[test]
fn event_definition_in_an_imported_behavior_file_is_mapped() {
    let shared = Shared::default();
    let backend = backend_with_events(
        &shared,
        StubHost::new(view_with_events()),
        json!([{
            "eventId": "e0",
            "state": {"status": "supported"},
            "definition": {"fileId": "pages/other.ts", "range": {"start": 5, "end": 10}}
        }]),
    );
    let definition = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 12,
        })
        .expect("definition");
    assert!(definition.status.is_supported(), "{definition:?}");
    assert_eq!(definition.locations.len(), 1, "{definition:?}");
    let location = &definition.locations[0];
    assert_eq!(location.file.path(), "pages/other.ts");
    assert_eq!((location.range.start(), location.range.end()), (5, 10));
}

#[test]
fn event_definition_without_a_kind_is_mapped() {
    let shared = Shared::default();
    let backend = backend_with_events(
        &shared,
        StubHost::new(view_with_events()),
        json!([{
            "eventId": "e0",
            "state": {"status": "supported"},
            "definition": {"fileId": SCRIPT, "range": {"start": 7, "end": 12}}
        }]),
    );
    let definition = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 12,
        })
        .expect("definition");
    assert_eq!(definition.locations.len(), 1);
    assert_eq!(definition.locations[0].file.path(), SCRIPT);
    assert_eq!(
        (
            definition.locations[0].range.start(),
            definition.locations[0].range.end()
        ),
        (7, 12)
    );
    assert!(definition.status.is_supported());
}

#[test]
fn missing_event_definition_is_supported_without_locations() {
    let shared = Shared::default();
    let backend = backend_with_events(
        &shared,
        StubHost::new(view_with_events()),
        json!([{"eventId": "e0", "state": {"status": "supported"}}]),
    );
    let definition = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 12,
        })
        .expect("definition");
    assert!(definition.locations.is_empty());
    assert!(definition.status.is_supported());
}

#[test]
fn partial_event_result_is_partial_and_has_no_definition() {
    let shared = Shared::default();
    let backend = backend_with_events(
        &shared,
        StubHost::new(view_with_events()),
        json!([{
            "eventId": "e0",
            "state": {"status": "partial", "reason": "multiple registrations"}
        }]),
    );
    let definition = backend
        .definition(DefinitionRequest {
            context: context(),
            offset: 12,
        })
        .expect("definition");
    assert!(definition.locations.is_empty());
    let CapabilityStatus::Partial { reason } = definition.status else {
        panic!("expected partial");
    };
    assert_eq!(reason, "multiple registrations");
}

#[test]
fn events_alone_start_the_backend() {
    let shared = Shared::default();
    let mut prepared = view_with_events();
    prepared.fragments = Arc::from(Vec::new());
    let backend = backend_with_events(
        &shared,
        StubHost::new(prepared),
        json!([{
            "eventId": "e0",
            "state": {"status": "supported"},
            "diagnostic": {
                "range": {"start": 10, "end": 15},
                "severity": "error",
                "message": "Cannot find name 'onTap'"
            }
        }]),
    );
    let result = backend
        .diagnostics(DiagnosticsRequest { context: context() })
        .expect("diagnostics");
    assert_eq!(requests(&shared), 1);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "Cannot find name 'onTap'");
}

#[test]
fn alternating_views_keep_a_bounded_cache() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let first = context();
    let second = RequestContext::new(
        ProjectRevision::new(ProjectId::new("proj-other"), 1),
        first.view_revision(),
        first.backend_epoch(),
        Arc::new(SourceSnapshot::new(
            FileId::new(ProjectId::new("proj-other"), WXML).expect("file"),
            SourceRevision::new(1),
            SnapshotId::new(2),
            "{{ user.name }}",
        )),
    )
    .expect("second context");
    for _ in 0..3 {
        let _ = backend.diagnostics(DiagnosticsRequest {
            context: first.clone(),
        });
        let _ = backend.diagnostics(DiagnosticsRequest {
            context: second.clone(),
        });
    }
    assert_eq!(
        requests(&shared),
        2,
        "both alternating views must stay cached"
    );
}

/// A context for one distinct analysis snapshot of the standard test view.
fn revision_context(revision: u64) -> RequestContext {
    RequestContext::new(
        ProjectRevision::new(project(), revision),
        ViewRevision::new(revision),
        BackendEpoch::new(BackendId::new("node"), 1),
        Arc::new(SourceSnapshot::new(
            file(WXML),
            SourceRevision::new(revision),
            SnapshotId::new(revision),
            "{{ user.name }}",
        )),
    )
    .expect("context")
}

#[test]
fn analysis_cache_is_bounded_and_reuses_unchanged_inputs() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let revisions = u64::try_from(MAX_CACHED_ANALYSES).expect("revisions") + 1;
    for revision in 1..=revisions {
        let context = revision_context(revision);
        let _ = backend.diagnostics(DiagnosticsRequest { context });
    }
    assert_eq!(requests(&shared), MAX_CACHED_ANALYSES + 1);

    let newest = revision_context(revisions);
    let _ = backend.diagnostics(DiagnosticsRequest { context: newest });
    assert_eq!(
        requests(&shared),
        MAX_CACHED_ANALYSES + 1,
        "the newest analysis stays cached"
    );

    let oldest = revision_context(1);
    let _ = backend.diagnostics(DiagnosticsRequest { context: oldest });
    assert_eq!(
        requests(&shared),
        MAX_CACHED_ANALYSES + 2,
        "the cache must evict the least recently used analysis"
    );
}

#[test]
fn project_revision_is_part_of_the_cache_key() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let first = context();
    let mut second = context();
    second = RequestContext::new(
        ProjectRevision::new(project(), 2),
        second.view_revision(),
        second.backend_epoch(),
        Arc::new(second.source().clone()),
    )
    .expect("second context");
    let _ = backend.diagnostics(DiagnosticsRequest { context: first });
    let _ = backend.diagnostics(DiagnosticsRequest { context: second });
    assert_eq!(
        requests(&shared),
        2,
        "a project revision change must bypass the cached analysis"
    );
}

#[test]
fn a_failed_analysis_clears_the_cache() {
    let shared = Shared::default();
    let attempts = Arc::new(AtomicUsize::new(0));
    let responder_attempts = Arc::clone(&attempts);
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), move |line| {
        if responder_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(NodeTransportError::Timeout)
        } else {
            Ok(respond(line))
        }
    });
    let context = context();
    let _ = backend.diagnostics(DiagnosticsRequest {
        context: context.clone(),
    });
    assert_eq!(requests(&shared), 1);
    assert_eq!(backend.epoch().get(), 2);
    let _ = backend.diagnostics(DiagnosticsRequest { context });
    assert_eq!(
        requests(&shared),
        2,
        "a failed generation must not serve its cached analysis"
    );
    assert_eq!(backend.epoch().get(), 2);
}

#[test]
fn invalidate_keeps_the_restart_backoff() {
    let shared = Shared::default();
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), |_line| {
        Err(NodeTransportError::Timeout)
    });
    for _ in 0..3 {
        let _ = backend.diagnostics(DiagnosticsRequest { context: context() });
    }
    assert_eq!(backend.failures.load(Ordering::SeqCst), 3);
    assert!(
        backend
            .cooldown_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some(),
        "three failures must start the restart cooldown"
    );
    backend.invalidate();
    assert_eq!(
        backend.failures.load(Ordering::SeqCst),
        3,
        "project invalidation is not a backend success"
    );
    assert!(
        backend
            .cooldown_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some(),
        "project invalidation must not clear the restart backoff"
    );
}

#[test]
fn cache_keys_use_the_request_epoch_and_live_generation() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let request = context();
    let before = backend.key(&request);
    backend.invalidate();
    let after = backend.key(&request);
    assert_eq!(
        after.epoch, before.epoch,
        "the key epoch must come from the request context, not a live read"
    );
    assert_ne!(after.generation, before.generation);
}

#[test]
fn invalidate_clears_the_generation_and_cache() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let context = context();
    let _ = backend.diagnostics(DiagnosticsRequest {
        context: context.clone(),
    });
    assert_eq!(requests(&shared), 1);
    let epoch = backend.epoch();
    backend.invalidate();
    assert!(backend.epoch() > epoch);
    let _ = backend.diagnostics(DiagnosticsRequest { context });
    assert_eq!(
        requests(&shared),
        2,
        "a removed project must not be served from the old cache"
    );
}

#[test]
fn symbol_parameter_errors_do_not_restart_the_backend() {
    use wxml_types::{SymbolQuery, SymbolRequest};

    let shared = Shared::default();
    let backend = backend_with(&shared, StubHost::new(view(Vec::new())), |line: &str| {
        let request: Value = serde_json::from_str(line).expect("request");
        Ok(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {"code": -32602, "message": "Invalid params", "data": {"code": "bad-params"}},
        })
        .to_string())
    });
    let epoch = backend.epoch();
    let result = backend
        .symbols(SymbolRequest {
            context: context(),
            subject: file(SCRIPT),
            query: SymbolQuery::DataRoots,
        })
        .expect("symbols");
    assert!(!result.status.is_supported());
    assert!(!result.complete);
    assert_eq!(backend.epoch(), epoch);
    assert_eq!(shared.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn truncated_source_snapshot_disables_script_symbols_without_starting_node() {
    use wxml_types::{SymbolQuery, SymbolRequest};

    let shared = Shared::default();
    let mut prepared = view(Vec::new());
    prepared.source_complete = false;
    let backend = backend(&shared, StubHost::new(prepared));
    let result = backend
        .symbols(SymbolRequest {
            context: context(),
            subject: file(SCRIPT),
            query: SymbolQuery::DataRoots,
        })
        .expect("symbols");
    assert!(!result.complete);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(shared.connects.load(Ordering::SeqCst), 0);
}
