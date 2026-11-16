#![allow(clippy::assert_is_empty)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use wxml_text::SourceRange;
use wxml_types::{
    Backend, BackendEpoch, BackendId, CheckFragment, DefinitionRequest, Diagnostic,
    DiagnosticSeverity, DiagnosticsRequest, FileId, HoverRequest, LocalBinding, LocalRole,
    MarkupKind, ProjectId, ProjectRevision, RequestContext, RootKind, SnapshotId, SourceRevision,
    SourceSnapshot, TypeQuality, ViewRevision,
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
        project_root: std::path::PathBuf::from("/tmp/project"),
        syntax_diagnostics: Vec::new(),
        status_hint: None,
    }
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
            let value = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3) + 5;
            json!({
                "fragmentId": fragment["fragmentId"],
                "hover": {
                    "typeText": "string",
                    "typeOnly": "string",
                    "quality": "known",
                    "documentation": "the name",
                    "documentationSource": "quickinfo"
                },
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
fn hover_answers_anywhere_inside_the_expression() {
    let shared = Shared::default();
    let backend = backend(&shared, StubHost::new(view(Vec::new())));
    let context = context();
    let hover = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 10,
        })
        .expect("hover");
    assert_eq!(hover.quality, TypeQuality::Known);
    assert_eq!(hover.markup, MarkupKind::Markdown);
    assert_eq!(
        hover.range.map(|range| (range.start(), range.end())),
        Some((8, 12))
    );

    // `user` is inside the expression but outside the target token `name`.
    let expression = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 5,
        })
        .expect("hover expression");
    assert_eq!(expression.quality, TypeQuality::Known);
    assert_eq!(
        expression.range.map(|range| (range.start(), range.end())),
        Some((8, 12))
    );

    // The end of the expression is a valid point (right edge of the text).
    let end = backend
        .hover(HoverRequest {
            context: context.clone(),
            offset: 12,
        })
        .expect("hover end");
    assert_eq!(end.quality, TypeQuality::Known);

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
        value["result"]["fragments"][0]["hover"]["typeText"] = json!("a```b");
        value["result"]["fragments"][0]["hover"]["documentation"] =
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
