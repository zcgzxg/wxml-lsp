#![allow(clippy::expect_used, clippy::unwrap_used, clippy::assert_is_empty)]

//! Integration tests for the C1 Node backend with a fake transport.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig, NodeTransport, NodeTransportError};
use wxml_lsp::project_host::{ProjectHost, ProjectState};
use wxml_types::{
    Backend, BackendEpoch, BackendId, DefinitionRequest, DiagnosticsRequest, HoverRequest,
    ProjectRevision, RequestContext, SnapshotId, SourceRevision, SourceSnapshot, ViewRevision,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new() -> Self {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!(
            "wxml-p7-node-{stamp}-{}",
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("temp project");
        Self { root }
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(target, text).expect("fixture");
    }

    fn root_uri(&self) -> String {
        format!("file://{}", self.root.display())
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fixture() -> TempProject {
    let project = TempProject::new();
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write("pages/index/index.wxml", "{{ user.name }}");
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write("models/user.ts", "export interface User { name: string }");
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    project
}

fn host(project: &TempProject) -> Arc<ProjectState> {
    let host = Arc::new(ProjectState::new());
    host.open_root(Some(&project.root_uri())).expect("open");
    host
}

fn snapshot(host: &ProjectState, path: &str, text: &str) -> Arc<SourceSnapshot> {
    let path = wxml_project::ProjectPath::new(path).expect("path");
    let file = host.file_id(&path).expect("file");
    Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        text,
    ))
}

fn context(host: &ProjectState, snapshot: &Arc<SourceSnapshot>) -> RequestContext {
    let path = wxml_project::ProjectPath::new(snapshot.file().path()).expect("path");
    let target = host.view_for(&path).expect("view");
    RequestContext::new(
        ProjectRevision::new(host.project_id(), 1),
        target.view_revision,
        BackendEpoch::new(BackendId::new("node"), 1),
        Arc::clone(snapshot),
    )
    .expect("context")
}

type Responder = Arc<dyn Fn(&str) -> Result<String, NodeTransportError> + Send + Sync>;

#[derive(Clone, Default)]
struct Shared {
    requests: Arc<Mutex<Vec<String>>>,
    connects: Arc<AtomicU64>,
}

struct Fake {
    shared: Shared,
    responder: Responder,
}

impl NodeTransport for Fake {
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

fn backend(shared: &Shared, host: Arc<ProjectState>) -> NodeBackend {
    backend_with(shared, host, |line| Ok(respond(line)))
}

fn backend_with(
    shared: &Shared,
    host: Arc<ProjectState>,
    responder: impl Fn(&str) -> Result<String, NodeTransportError> + Send + Sync + 'static,
) -> NodeBackend {
    let shared = shared.clone();
    let responder: Responder = Arc::new(responder);
    NodeBackend::with_transport_factory(NodeBackendConfig::default(), host, move || {
        shared.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Fake {
            shared: shared.clone(),
            responder: Arc::clone(&responder),
        }))
    })
}

fn respond(line: &str) -> String {
    let request: serde_json::Value = serde_json::from_str(line).expect("request json");
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
            let start = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3) + 5;
            serde_json::json!({
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
                    "range": {"start": start, "end": start + 4},
                    "message": "Property 'nmae' does not exist",
                    "attribution": "source"
                }],
                "definition": {
                    "kind": "script",
                    "fileId": "pages/index/index.ts",
                    "range": {"start": 0, "end": 4}
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&serde_json::json!({
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

#[test]
fn one_analysis_serves_hover_diagnostics_and_cross_file_definition() {
    let project = fixture();
    let host = host(&project);
    let shared = Shared::default();
    let backend = backend(&shared, Arc::clone(&host));
    let snapshot = snapshot(&host, "pages/index/index.wxml", "{{ user.name }}");
    let context = context(&host, &snapshot);
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: context.clone(),
        })
        .expect("diagnostics");
    assert!(diagnostics.status.is_supported());
    assert_eq!(diagnostics.diagnostics.len(), 1, "{diagnostics:?}");
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
    assert_eq!(definition.locations[0].file.path(), "pages/index/index.ts");
    assert_eq!(
        shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );
}

#[test]
fn timeout_keeps_syntax_and_reconnects_with_a_new_epoch() {
    let project = fixture();
    let host = host(&project);
    let shared = Shared::default();
    let attempts = Arc::new(AtomicU64::new(0));
    let responder_attempts = Arc::clone(&attempts);
    let backend = backend_with(&shared, Arc::clone(&host), move |line| {
        if responder_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(NodeTransportError::Timeout)
        } else {
            Ok(respond(line))
        }
    });
    let snapshot = snapshot(&host, "pages/index/index.wxml", "{{ user.name }}");
    let context = context(&host, &snapshot);
    let diagnostics = backend
        .diagnostics(DiagnosticsRequest {
            context: context.clone(),
        })
        .expect("diagnostics");
    assert!(matches!(
        diagnostics.status,
        wxml_types::CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(backend.epoch().get(), 2);

    let hover = backend
        .hover(HoverRequest {
            context,
            offset: 10,
        })
        .expect("hover after reconnect");
    assert!(hover.contents.contains("string"));
    assert_eq!(shared.connects.load(Ordering::SeqCst), 2);
}

#[test]
fn syntax_is_preserved_when_node_fails() {
    let project = fixture();
    let host = host(&project);
    let shared = Shared::default();
    let backend = backend_with(&shared, Arc::clone(&host), |_line| {
        Err(NodeTransportError::Timeout)
    });
    let snapshot = snapshot(
        &host,
        "pages/index/index.wxml",
        "<view></span>{{ user.name }}",
    );
    let context = context(&host, &snapshot);
    let result = backend
        .diagnostics(DiagnosticsRequest { context })
        .expect("diagnostics");
    assert!(
        !result.diagnostics.is_empty(),
        "P2 syntax diagnostics must survive a type backend failure"
    );
}

#[test]
fn a_new_project_revision_triggers_a_new_analysis() {
    let project = fixture();
    let host = host(&project);
    let shared = Shared::default();
    let backend = backend(&shared, Arc::clone(&host));
    let snapshot = snapshot(&host, "pages/index/index.wxml", "{{ user.name }}");
    let context = context(&host, &snapshot);
    let _ = backend.diagnostics(DiagnosticsRequest {
        context: context.clone(),
    });
    let second = RequestContext::new(
        ProjectRevision::new(host.project_id(), 2),
        context.view_revision(),
        context.backend_epoch(),
        Arc::new(context.source().clone()),
    )
    .expect("context");
    let _ = backend.diagnostics(DiagnosticsRequest { context: second });
    assert_eq!(
        shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        2
    );
}

#[test]
fn no_project_view_degrades_without_starting_node() {
    let shared = Shared::default();
    let host = Arc::new(ProjectState::new());
    let backend = backend(&shared, Arc::clone(&host));
    let file = wxml_types::FileId::new(host.project_id(), "documents/1.wxml").expect("file");
    let snapshot = Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        "<view></span>",
    ));
    let context = RequestContext::new(
        host.project_revision(),
        ViewRevision::new(1),
        BackendEpoch::new(BackendId::new("node"), 1),
        Arc::clone(&snapshot),
    )
    .expect("context");
    let result = backend
        .diagnostics(DiagnosticsRequest { context })
        .expect("diagnostics");
    assert!(!result.diagnostics.is_empty());
    assert!(matches!(
        result.status,
        wxml_types::CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(shared.connects.load(Ordering::SeqCst), 0);
}
