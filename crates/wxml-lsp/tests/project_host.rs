#![allow(clippy::expect_used, clippy::unwrap_used, clippy::assert_is_empty)]

//! Integration tests for the real project host (P7-1).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use wxml_lsp::project_host::{
    ChangeDocument, DocumentRoute, HostError, OpenDocument, ProjectHost, ProjectState,
};
use wxml_types::{CapabilityStatus, FileId, ProjectId, SnapshotId, SourceRevision, SourceSnapshot};

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
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let root = base.join(format!("wxml-p7-host-{stamp}-{id}"));
        std::fs::create_dir_all(&root).expect("create temp project");
        Self { root }
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(target, text).expect("write fixture");
    }

    fn uri(&self, path: &str) -> String {
        format!("file://{}", self.root.join(path).display())
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
        r#"{"pages":["pages/index/index"],"usingComponents":{"card":"./components/card/card"}}"#,
    );
    project.write(
        "pages/index/index.wxml",
        r#"<view class="page">{{ user.name }}</view>"#,
    );
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write("components/card/card.wxml", "<view>{{ title }}</view>");
    project.write(
        "components/card/card.ts",
        "Component({ properties: { title: String } });",
    );
    project.write("models/user.ts", "export interface User { name: string }");
    project.write(
        "tsconfig.json",
        r#"{"compilerOptions":{"strict":true,"unknownOption":1,"baseUrl":"."},"include":["**/*.ts"]}"#,
    );
    project.write("orphan.wxml", "<view>{{ unknown.value }}</view>");
    project
}

fn host_with(project: &TempProject) -> ProjectState {
    let host = ProjectState::new();
    host.open_root(Some(&project.root_uri()))
        .expect("open root");
    host
}

fn snapshot(host: &ProjectState, path: &str, text: &str) -> Arc<SourceSnapshot> {
    let path = wxml_project::ProjectPath::new(path).expect("path");
    let file = host.file_id(&path).expect("file id");
    Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        text,
    ))
}

#[test]
fn routes_and_file_ids_are_real_and_stable() {
    let project = fixture();
    let host = host_with(&project);
    let wxml_uri = project.uri("pages/index/index.wxml");
    let script_uri = project.uri("pages/index/index.ts");
    let json_uri = project.uri("app.json");
    let outside_uri = "file:///etc/passwd";

    let wxml_path = host.path_for_uri(&wxml_uri).expect("wxml path");
    assert_eq!(wxml_path.as_str(), "pages/index/index.wxml");
    assert!(matches!(
        host.route(&wxml_uri),
        DocumentRoute::Wxml(ref path) if path == &wxml_path
    ));
    assert!(matches!(host.route(&script_uri), DocumentRoute::Script(_)));
    assert!(matches!(host.route(&json_uri), DocumentRoute::Json(_)));
    assert_eq!(host.route(outside_uri), DocumentRoute::Unknown);

    let file = host.file_id(&wxml_path).expect("file id");
    assert_eq!(file.project().as_str(), host.project_id().as_str());
    assert_eq!(file.path(), "pages/index/index.wxml");
    assert!(
        !file
            .as_str()
            .contains(project.root.to_string_lossy().as_ref())
    );
    assert_eq!(host.uri_for(&file).as_deref(), Some(wxml_uri.as_str()));
    assert_eq!(host.path_for_uri(outside_uri), Err(HostError::InvalidUri));
    assert_eq!(
        host.path_for_uri("https://example.com/a.wxml"),
        Err(HostError::InvalidUri)
    );
}

#[test]
fn discovers_page_and_component_views() {
    let project = fixture();
    let host = host_with(&project);
    let page = wxml_project::ProjectPath::new("pages/index/index.wxml").expect("page path");
    let target = host.view_for(&page).expect("page view");
    assert_eq!(target.owner, wxml_types::RootKind::Page);
    assert_eq!(target.script.as_str(), "pages/index/index.ts");
    assert_eq!(target.script_file.path(), "pages/index/index.ts");
    assert!(target.view_revision.get() >= 1);

    let component =
        wxml_project::ProjectPath::new("components/card/card.wxml").expect("component path");
    let target = host.view_for(&component).expect("component view");
    assert_eq!(target.owner, wxml_types::RootKind::Component);

    let orphan = wxml_project::ProjectPath::new("orphan.wxml").expect("orphan path");
    assert!(matches!(
        host.view_for(&orphan),
        Err(HostError::Unsupported { .. })
    ));
}

#[test]
fn prepares_files_fragments_and_filtered_compiler_options() {
    let project = fixture();
    let host = host_with(&project);
    let snapshot = snapshot(
        &host,
        "pages/index/index.wxml",
        r#"<view class="page">{{ user.name }}</view>"#,
    );
    let view = host.analysis(&snapshot).expect("analysis");
    assert!(view.target.is_some());
    assert!(!view.fragments.is_empty());
    assert!(
        view.files
            .iter()
            .any(|file| file.path.as_str() == "pages/index/index.ts")
    );
    assert!(
        view.files
            .iter()
            .any(|file| file.path.as_str() == "models/user.ts")
    );
    assert_eq!(
        view.compiler_options.get("strict"),
        Some(&serde_json::Value::Bool(true))
    );
    assert!(!view.compiler_options.contains_key("unknownOption"));
    let types = view.compiler_options["types"].as_array().expect("types");
    assert!(
        types
            .iter()
            .any(|entry| entry.as_str() == Some("miniprogram-api-typings"))
    );
    assert!(matches!(
        view.status_hint,
        Some(CapabilityStatus::Partial { .. })
    ));
    assert!(view.syntax_diagnostics.is_empty());
    assert!(view.view_id.starts_with("pages/index/index.wxml:"));
}

#[test]
fn malformed_and_unknown_templates_still_produce_syntax_diagnostics() {
    let project = fixture();
    let host = host_with(&project);
    let malformed = snapshot(&host, "pages/index/index.wxml", "<view></span>");
    let view = host.analysis(&malformed).expect("analysis");
    assert!(!view.syntax_diagnostics.is_empty());

    let orphan = snapshot(&host, "orphan.wxml", "<view></span>");
    let view = host.analysis(&orphan).expect("analysis");
    assert!(view.target.is_none());
    assert!(view.fragments.is_empty());
    assert!(!view.syntax_diagnostics.is_empty());
    assert!(matches!(
        view.status_hint,
        Some(CapabilityStatus::Unsupported { .. })
    ));
}

#[test]
fn empty_template_has_zero_fragments_and_a_supported_hint() {
    let project = fixture();
    let host = host_with(&project);
    let empty = snapshot(&host, "pages/index/index.wxml", "<view>plain</view>");
    let view = host.analysis(&empty).expect("analysis");
    assert!(view.fragments.is_empty());
    assert!(view.target.is_some());
}

#[test]
fn script_overlays_are_visible_and_mark_views_dirty() {
    let project = fixture();
    let host = host_with(&project);
    let script_uri = project.uri("pages/index/index.ts");
    let update = host
        .open_document(OpenDocument {
            uri: script_uri.clone(),
            text: "Page({ data: { user: { name: 'Overlay' } } });".to_owned(),
            version: Some(1),
        })
        .expect("open overlay");
    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/index/index.wxml")
    );
    assert!(update.project_revision.is_some());
    let snapshot = snapshot(
        &host,
        "pages/index/index.wxml",
        r#"<view class="page">{{ user.name }}</view>"#,
    );
    let view = host.analysis(&snapshot).expect("analysis");
    let script = view
        .files
        .iter()
        .find(|file| file.path.as_str() == "pages/index/index.ts")
        .expect("script file");
    assert!(script.text.contains("Overlay"));

    host.change_document(ChangeDocument {
        uri: script_uri.clone(),
        text: "Page({ data: { user: { name: 'Changed' } } });".to_owned(),
        version: 2,
    })
    .expect("change overlay");
    let view = host.analysis(&snapshot).expect("analysis");
    let script = view
        .files
        .iter()
        .find(|file| file.path.as_str() == "pages/index/index.ts")
        .expect("script file");
    assert!(script.text.contains("Changed"));

    host.close_document(&script_uri).expect("close overlay");
    let view = host.analysis(&snapshot).expect("analysis");
    let script = view
        .files
        .iter()
        .find(|file| file.path.as_str() == "pages/index/index.ts")
        .expect("script file");
    assert!(script.text.contains("Ada"));
}

#[test]
fn wxml_snapshots_round_trip_and_advance_view_revision() {
    let project = fixture();
    let host = host_with(&project);
    let path = wxml_project::ProjectPath::new("pages/index/index.wxml").expect("path");
    let file = host.file_id(&path).expect("file");
    let before = host.view_for(&path).expect("view").view_revision;
    let snapshot = Arc::new(SourceSnapshot::new(
        file.clone(),
        SourceRevision::new(1),
        SnapshotId::new(6),
        "<view>{{ user.name }}</view>",
    ));
    host.set_wxml_snapshot(file.clone(), Arc::clone(&snapshot));
    assert_eq!(
        host.source(&file).as_deref().map(SourceSnapshot::text),
        Some("<view>{{ user.name }}</view>")
    );
    assert!(host.view_for(&path).expect("view").view_revision > before);
    host.remove_wxml_snapshot(&file);
    assert!(host.source(&file).is_none());
}

#[test]
fn no_project_degrades_explicitly() {
    let host = ProjectState::new();
    assert_eq!(host.project_id().as_str(), "lsp");
    assert!(host.open_root(None).is_ok());
    let file = FileId::new(ProjectId::new("lsp"), "documents/1.wxml").expect("file");
    let snapshot = SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        "<view>{{ user.name }}",
    );
    let view = host.analysis(&snapshot).expect("syntax-only analysis");
    assert!(view.target.is_none());
    assert!(view.fragments.is_empty());
    assert!(!view.syntax_diagnostics.is_empty());
    assert!(matches!(
        view.status_hint,
        Some(CapabilityStatus::Unsupported { .. })
    ));
    assert_eq!(host.route("file:///a.wxml"), DocumentRoute::Unknown);
}

#[test]
fn invalid_root_fails_without_leaking_paths() {
    let host = ProjectState::new();
    let error = host
        .open_root(Some("file:///definitely/not/a/project"))
        .expect_err("invalid root");
    assert_eq!(error, HostError::ProjectOpen);
    assert!(!error.to_string().contains("/definitely"));
    let file = FileId::new(ProjectId::new("lsp"), "documents/1.wxml").expect("file");
    let snapshot = SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        "<view>{{ user.name }}</view>",
    );
    let view = host.analysis(&snapshot).expect("analysis");
    assert!(matches!(
        view.status_hint,
        Some(CapabilityStatus::Unsupported { .. })
    ));
}

#[test]
fn root_and_uri_helpers_do_not_expose_host_paths_in_ids() {
    let project = fixture();
    let host = host_with(&project);
    let id = host.project_id();
    assert!(id.as_str().starts_with("proj-"));
    assert!(
        !id.as_str()
            .contains(project.root.to_string_lossy().as_ref())
    );
    assert!(Path::new(project.root.to_string_lossy().as_ref()).exists());
    assert!(
        host.uri_for(&FileId::new(id, "pages/index/index.wxml").expect("file"))
            .is_some()
    );
}
