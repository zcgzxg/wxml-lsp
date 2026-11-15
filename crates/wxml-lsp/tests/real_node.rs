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
