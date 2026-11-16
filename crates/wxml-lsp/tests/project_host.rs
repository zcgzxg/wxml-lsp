#![allow(clippy::expect_used, clippy::unwrap_used, clippy::assert_is_empty)]

//! Integration tests for the real project host (P7-1).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use wxml_lsp::configuration::InitializationOptions;
use wxml_lsp::project_host::{
    ChangeDocument, DocumentRoute, HostError, HostUpdate, OpenDocument, ProjectHost, ProjectState,
    WatchedChangeKind, WatchedFileChange,
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

fn project_path(raw: &str) -> wxml_project::ProjectPath {
    wxml_project::ProjectPath::new(raw).expect("project path")
}

fn opened(uri: &str, text: &str) -> OpenDocument {
    OpenDocument {
        uri: uri.to_owned(),
        text: text.to_owned(),
        version: Some(1),
    }
}

fn watched(uri: &str, kind: WatchedChangeKind) -> WatchedFileChange {
    WatchedFileChange {
        uri: uri.to_owned(),
        kind,
    }
}

#[test]
fn unreferenced_project_json_overlay_is_ignored() {
    let project = fixture();
    project.write("package.json", r#"{"name":"fixture"}"#);
    let host = host_with(&project);
    let path = project_path("package.json");
    let file = host.file_id(&path).expect("file");
    let before_revision = host.project_revision();
    let before_text = host.file_text(&file).expect("disk text");

    let update = host
        .open_document(opened(
            &project.uri("package.json"),
            r#"{"name":"overlay"}"#,
        ))
        .expect("open overlay");

    assert_eq!(update, HostUpdate::default());
    assert_eq!(host.project_revision(), before_revision);
    assert_eq!(host.file_text(&file), Some(before_text));
    assert!(
        host.file_text(&file)
            .is_some_and(|text| text.contains("fixture"))
    );
}

#[test]
fn excluded_directory_script_overlay_is_ignored() {
    let project = fixture();
    project.write("node_modules/pkg/index.js", "module.exports = 1;\n");
    let host = host_with(&project);
    let path = project_path("node_modules/pkg/index.js");
    let file = host.file_id(&path).expect("file");
    let before_revision = host.project_revision();
    let before_text = host.file_text(&file).expect("disk text");

    let update = host
        .open_document(opened(
            &project.uri("node_modules/pkg/index.js"),
            "module.exports = 2;\n",
        ))
        .expect("open overlay");

    assert_eq!(update, HostUpdate::default());
    assert_eq!(host.project_revision(), before_revision);
    assert_eq!(host.file_text(&file), Some(before_text));
}

#[test]
fn documents_outside_the_project_root_are_rejected() {
    let project = fixture();
    let host = host_with(&project);
    assert_eq!(
        host.open_document(opened("file:///etc/evil.ts", "export const x = 1;")),
        Err(HostError::InvalidUri)
    );
    assert_eq!(
        host.save_document("file:///etc/evil.ts"),
        Err(HostError::InvalidUri)
    );
    assert_eq!(host.route("file:///etc/evil.ts"), DocumentRoute::Unknown);
}

#[test]
fn script_overlay_reaches_the_owning_view_analysis() {
    let project = fixture();
    let host = host_with(&project);
    let wxml = project_path("pages/index/index.wxml");
    let file = host.file_id(&wxml).expect("file");
    let before_view = host.view_for(&wxml).expect("view").view_revision;

    let update = host
        .open_document(opened(
            &project.uri("pages/index/index.ts"),
            "Page({ data: { user: { name: 'AC004_OVERLAY' as const } } });",
        ))
        .expect("open overlay");

    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/index/index.wxml")
    );
    assert!(update.project_revision.is_some());

    let snapshot = Arc::new(SourceSnapshot::new(
        file.clone(),
        SourceRevision::new(1),
        SnapshotId::new(11),
        r#"<view class="page">{{ user.name }}</view>"#,
    ));
    host.set_wxml_snapshot(file, Arc::clone(&snapshot));
    assert!(
        host.view_for(&wxml).expect("view").view_revision > before_view,
        "the WXML view revision must advance after a save/didChange snapshot"
    );

    let view = host.analysis(&snapshot).expect("analysis");
    let script = view
        .files
        .iter()
        .find(|file| file.path.as_str() == "pages/index/index.ts")
        .expect("owning script");
    assert!(script.text.contains("AC004_OVERLAY"));
}

#[test]
fn save_document_tracks_owning_views_only() {
    let project = fixture();
    let host = host_with(&project);
    let before_revision = host.project_revision();

    let update = host
        .save_document(&project.uri("pages/index/index.ts"))
        .expect("save owning script");
    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/index/index.wxml")
    );
    assert!(update.project_revision.is_some());
    assert_ne!(host.project_revision(), before_revision);

    let before_revision = host.project_revision();
    let update = host
        .save_document(&project.uri("models/user.ts"))
        .expect("save unreferenced script");
    assert!(update.dirty_views.is_empty());
    assert!(update.project_revision.is_none());
    assert_eq!(host.project_revision(), before_revision);
}

#[test]
fn non_owning_script_overlays_reanalyze_every_view() {
    let project = fixture();
    let host = host_with(&project);
    let before = host.project_revision();

    let update = host
        .open_document(opened(
            &project.uri("models/user.ts"),
            "export interface User { name: 'ZED_OVERLAY' }",
        ))
        .expect("open non-owning overlay");
    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/index/index.wxml"),
        "an imported module can change any view's type results"
    );
    assert!(update.project_revision.is_some());
    assert_ne!(host.project_revision(), before);

    let before = host.project_revision();
    let update = host
        .close_document(&project.uri("models/user.ts"))
        .expect("close non-owning overlay");
    assert!(!update.dirty_views.is_empty());
    assert!(update.project_revision.is_some());
    assert_ne!(host.project_revision(), before);
}

#[test]
fn a_page_json_created_after_startup_is_discovered_by_a_watched_event() {
    let project = TempProject::new();
    project.write(
        "app.json",
        r#"{"pages":["pages/other/other"],"usingComponents":{}}"#,
    );
    project.write("pages/other/other.wxml", "<view>{{ other }}</view>");
    project.write("pages/other/other.ts", "Page({ data: { other: 1 } });");
    let host = host_with(&project);
    project.write("pages/other/other.json", r#"{"usingComponents":{}}"#);

    let update = host
        .watched_files_changed(&[watched(
            &project.uri("pages/other/other.json"),
            WatchedChangeKind::Created,
        )])
        .expect("watched created page config");
    assert!(update.project_revision.is_some());
    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/other/other.wxml")
    );

    // The config is now part of discovery and accepts unsaved overlays.
    let update = host
        .open_document(opened(
            &project.uri("pages/other/other.json"),
            r#"{"usingComponents":{"x":"/x"}}"#,
        ))
        .expect("open discovered page config");
    assert!(update.project_revision.is_some());
}

#[test]
fn tsconfig_extends_files_accept_overlays() {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":[],"usingComponents":{}}"#);
    project.write(
        "tsconfig.base.json",
        r#"{"compilerOptions":{"strict":true}}"#,
    );
    project.write("tsconfig.json", r#"{"extends":"./tsconfig.base.json"}"#);
    let host = host_with(&project);

    let update = host
        .open_document(opened(
            &project.uri("tsconfig.base.json"),
            r#"{"compilerOptions":{"strict":false}}"#,
        ))
        .expect("open parent config overlay");
    assert!(update.project_revision.is_some());
}

#[test]
fn saving_app_json_rediscovers_and_dirties_every_view() {
    let project = fixture();
    project.write("pages/other/other.wxml", "<view>{{ other }}</view>");
    project.write("pages/other/other.ts", "Page({ data: { other: 1 } });");
    let host = host_with(&project);
    let other = project_path("pages/other/other.wxml");
    assert!(host.view_for(&other).is_err(), "not discovered before save");

    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/other/other"]}"#,
    );
    let update = host
        .save_document(&project.uri("app.json"))
        .expect("save app.json");

    assert!(host.view_for(&other).is_ok(), "save must rediscover pages");
    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/index/index.wxml")
    );
    assert!(update.dirty_views.iter().any(|path| path == &other));
    assert!(update.project_revision.is_some());
}

#[test]
fn watched_app_json_change_rediscovers_and_drops_the_overlay() {
    let project = fixture();
    project.write("pages/third/third.wxml", "<view>{{ third }}</view>");
    project.write("pages/third/third.ts", "Page({ data: { third: 1 } });");
    project.write("pages/overlay/overlay.wxml", "<view>{{ overlay }}</view>");
    project.write(
        "pages/overlay/overlay.ts",
        "Page({ data: { overlay: 1 } });",
    );
    let host = host_with(&project);
    let stale = project_path("pages/overlay/overlay.wxml");
    let fresh = project_path("pages/third/third.wxml");

    let update = host
        .open_document(opened(
            &project.uri("app.json"),
            r#"{"pages":["pages/index/index","pages/overlay/overlay"]}"#,
        ))
        .expect("open overlay");
    assert!(update.dirty_views.iter().any(|path| path == &stale));
    assert!(host.view_for(&stale).is_ok());

    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/third/third"]}"#,
    );
    let update = host
        .watched_files_changed(&[watched(
            &project.uri("app.json"),
            WatchedChangeKind::Changed,
        )])
        .expect("watched app.json");

    assert!(
        host.view_for(&fresh).is_ok(),
        "disk change must be picked up"
    );
    assert!(
        host.view_for(&stale).is_err(),
        "stale overlay must be dropped"
    );
    assert!(
        update
            .dirty_views
            .iter()
            .any(|path| path.as_str() == "pages/index/index.wxml")
    );
    assert!(update.dirty_views.iter().any(|path| path == &fresh));
    assert!(update.project_revision.is_some());
}

#[test]
fn watched_script_deletion_clears_page_script_and_view_target() {
    let project = fixture();
    let host = host_with(&project);
    let wxml = project_path("pages/index/index.wxml");
    assert!(host.view_for(&wxml).is_ok());

    std::fs::remove_file(project.root.join("pages/index/index.ts")).expect("remove script");
    let update = host
        .watched_files_changed(&[watched(
            &project.uri("pages/index/index.ts"),
            WatchedChangeKind::Deleted,
        )])
        .expect("watched deletion");

    assert!(matches!(
        host.view_for(&wxml),
        Err(HostError::Unsupported { .. })
    ));
    assert!(update.project_revision.is_some());
    assert!(update.dirty_views.iter().any(|path| path == &wxml));
}

#[test]
fn watched_script_change_dirties_exactly_its_views() {
    let project = fixture();
    let host = host_with(&project);
    let update = host
        .watched_files_changed(&[watched(
            &project.uri("pages/index/index.ts"),
            WatchedChangeKind::Changed,
        )])
        .expect("watched change");
    assert_eq!(
        update.dirty_views,
        vec![project_path("pages/index/index.wxml")]
    );
    assert!(update.project_revision.is_some());
}

#[test]
fn watched_other_files_and_outside_roots_are_ignored() {
    let project = fixture();
    project.write("pages/index/index.wxss", "view { color: red; }");
    project.write("package.json", r#"{"name":"fixture"}"#);
    let host = host_with(&project);
    let before_revision = host.project_revision();

    let update = host
        .watched_files_changed(&[
            watched(
                &project.uri("pages/index/index.wxss"),
                WatchedChangeKind::Changed,
            ),
            watched(&project.uri("package.json"), WatchedChangeKind::Changed),
            watched("file:///etc/passwd", WatchedChangeKind::Changed),
        ])
        .expect("watched ignored files");

    assert_eq!(update, HostUpdate::default());
    assert_eq!(host.project_revision(), before_revision);
}

#[test]
fn configured_project_root_supplies_tsconfig_for_a_nested_app() {
    let project = TempProject::new();
    project.write(
        "apps/miniprogram/src/app.json",
        r#"{"pages":["pages/index/index"]}"#,
    );
    project.write(
        "apps/miniprogram/src/pages/index/index.wxml",
        "<view>{{ user.name }}</view>",
    );
    project.write(
        "apps/miniprogram/src/pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write(
        "apps/miniprogram/src/models/user.ts",
        "export interface User { name: string }",
    );
    project.write(
        "apps/miniprogram/tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]},"target":"es2017","lib":["ES2022","DOM"]}}"#,
    );

    project.write("tsconfig.json", r#"{"compilerOptions":{"target":"es5"}}"#);
    let host = ProjectState::new();
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({"apps": [{
            "root": "apps/miniprogram/src", "projectRoot": "apps/miniprogram"
        }]}))
        .expect("options"),
    )
    .expect("configured root");
    let path = host
        .path_for_uri(&project.uri("apps/miniprogram/src/pages/index/index.wxml"))
        .expect("source path");
    assert_eq!(path.as_str(), "src/pages/index/index.wxml");
    let file = host.file_id(&path).expect("file id");
    let snapshot = Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        "<view>{{ user.name }}</view>",
    ));
    let view = host.analysis(&snapshot).expect("analysis");
    assert_eq!(
        view.target.as_ref().expect("target").script.as_str(),
        "src/pages/index/index.ts"
    );
    assert_eq!(
        view.compiler_options["baseUrl"],
        project
            .root
            .join("apps/miniprogram")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        view.compiler_options["target"],
        serde_json::Value::String("ES2017".to_owned())
    );
    assert_eq!(view.compiler_options["paths"]["@/*"][0], "src/*");
    assert!(
        view.files
            .iter()
            .any(|file| file.path.as_str() == "src/models/user.ts")
    );
}

#[test]
fn each_app_uses_its_configured_tsconfig_and_preserves_identity() {
    let project = TempProject::new();
    for app in ["a", "b"] {
        project.write(
            &format!("apps/{app}/src/app.json"),
            r#"{"pages":["pages/index/index"]}"#,
        );
        project.write(
            &format!("apps/{app}/src/pages/index/index.wxml"),
            "<view>{{ title }}</view>",
        );
        project.write(
            &format!("apps/{app}/src/pages/index/index.ts"),
            "Page({ data: { title: 'x' } });",
        );
        project.write(
            &format!("apps/{app}/tsconfig.json"),
            &format!(
                r#"{{"compilerOptions":{{"baseUrl":".","paths":{{"@/*":["src/*"]}},"strict":{}}}}}"#,
                app == "a"
            ),
        );
    }
    let host = ProjectState::new();
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({"apps": [
            {"root": "apps/a/src", "projectRoot": "apps/a"},
            {"root": "apps/b/src", "projectRoot": "apps/b"}
        ]}))
        .expect("options"),
    )
    .expect("configured roots");
    let mut files = Vec::new();
    let mut revisions = Vec::new();
    for app in ["a", "b", "a"] {
        let path = host
            .path_for_uri(&project.uri(&format!("apps/{app}/src/pages/index/index.wxml")))
            .expect("source path");
        let file = host.file_id(&path).expect("file id");
        let snapshot = Arc::new(SourceSnapshot::new(
            file.clone(),
            SourceRevision::new(1),
            SnapshotId::new(1),
            "<view>{{ title }}</view>",
        ));
        let view = host.analysis(&snapshot).expect("analysis");
        let expected_root = project.root.join(format!("apps/{app}"));
        assert_eq!(
            view.compiler_options["baseUrl"],
            expected_root.to_string_lossy().as_ref()
        );
        assert_eq!(
            view.compiler_options["strict"],
            serde_json::Value::Bool(app == "a")
        );
        files.push(file);
        revisions.push(host.project_revision());
    }
    assert_ne!(files[0].project(), files[1].project());
    assert_eq!(files[0], files[2]);
    assert_eq!(
        revisions[0], revisions[2],
        "routing alone preserves revisions"
    );
    // Resolving a definition from B must not use whichever app was last active.
    assert_eq!(
        host.uri_for(&files[1]),
        Some(project.uri("apps/b/src/pages/index/index.wxml"))
    );
    assert_eq!(
        host.uri_for(&files[0]),
        Some(project.uri("apps/a/src/pages/index/index.wxml"))
    );
    // Background jobs must not change the foreground app's routing identity.
    let active = host.project_id();
    let active_revision = host.project_revision();
    let background = Arc::new(SourceSnapshot::new(
        files[1].clone(),
        SourceRevision::new(2),
        SnapshotId::new(2),
        "{{ title }}",
    ));
    host.analysis(&background).expect("background analysis");
    assert_eq!(host.project_id(), active);
    assert_eq!(host.project_revision(), active_revision);
    let script = FileId::new(files[1].project().clone(), "src/pages/index/index.ts")
        .expect("script identity");
    assert!(host.file_text(&script).is_some());
    assert_eq!(host.project_id(), active);
}

#[test]
fn switching_apps_replays_newly_discovered_configuration_overlays() {
    let project = TempProject::new();
    for app in ["a", "b"] {
        project.write(&format!("apps/{app}/app.json"), r#"{"pages":[]}"#);
    }
    for view in [
        "pages/new/index",
        "components/card/index",
        "components/leaf/index",
    ] {
        project.write(&format!("apps/a/{view}.wxml"), "<leaf />");
        project.write(&format!("apps/a/{view}.ts"), "Component({});");
        project.write(&format!("apps/a/{view}.json"), "{}");
    }
    let host = ProjectState::new();
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({"apps":[{"root":"apps/a"},{"root":"apps/b"}]}))
            .expect("options"),
    )
    .expect("configured roots");
    for (path, text) in [
        ("app.json", r#"{"pages":["pages/new/index"]}"#),
        (
            "pages/new/index.json",
            r#"{"usingComponents":{"review-card":"../../components/card/index"}}"#,
        ),
        (
            "components/card/index.json",
            r#"{"component":true,"usingComponents":{"leaf":"../leaf/index"}}"#,
        ),
        (
            "components/leaf/index.ts",
            "Component({data:{unsaved:42}});",
        ),
    ] {
        host.open_document(opened(&project.uri(&format!("apps/a/{path}")), text))
            .expect("open overlay");
    }
    let page = project_path("pages/new/index.wxml");
    assert_eq!(host.view_components(&page)[0].name, "review-card");
    let background = snapshot(&host, page.as_str(), "<review-card />");
    let leaf = snapshot(&host, "components/leaf/index.wxml", "{{ unsaved }}");
    assert!(host.analysis(&leaf).expect("initial leaf").target.is_some());
    host.path_for_uri(&project.uri("apps/b/app.json"))
        .expect("switch to B");
    let view = host.analysis(&background).expect("background A analysis");
    assert_eq!(view.components.len(), 1, "replay the whole config chain");
    assert!(view.components[0].component_file.is_some());
    assert!(
        host.analysis(&leaf)
            .expect("background leaf")
            .target
            .is_some()
    );
    assert!(
        view.files
            .iter()
            .any(|file| file.text.contains("unsaved:42"))
    );
    host.path_for_uri(&project.uri("apps/a/pages/new/index.wxml"))
        .expect("switch back to A");
    assert_eq!(host.view_components(&page)[0].name, "review-card");
    assert_eq!(
        host.analysis(&background)
            .expect("foreground analysis")
            .components
            .len(),
        1
    );
}

#[test]
fn nested_app_roots_use_the_longest_match_in_either_configuration_order() {
    let project = TempProject::new();
    project.write("apps/outer/app.json", "{}");
    project.write("apps/outer/nested/app.json", "{}");
    for roots in [
        json!([{"root":"apps/outer"}, {"root":"apps/outer/nested"}]),
        json!([{"root":"apps/outer/nested"}, {"root":"apps/outer"}]),
    ] {
        let host = ProjectState::new();
        host.open_root_with_options(
            Some(&project.root_uri()),
            &InitializationOptions::parse(&json!({"apps": roots})).expect("options"),
        )
        .expect("configured roots");
        let path = host
            .path_for_uri(&project.uri("apps/outer/nested/pages/new.wxml"))
            .expect("nested file");
        assert_eq!(path.as_str(), "pages/new.wxml");
        assert_eq!(host.root(), Some(project.root.join("apps/outer/nested")));
        let nested = host.project_id();
        host.path_for_uri(&project.uri("apps/outer/pages/new.wxml"))
            .expect("outer file");
        assert_eq!(host.root(), Some(project.root.join("apps/outer")));
        assert_ne!(host.project_id(), nested);
    }
}

#[test]
fn shared_project_files_use_an_active_or_unique_owner_and_reject_ambiguity() {
    let project = TempProject::new();
    project.write("apps/a/app.json", "{}");
    project.write("apps/b/app.json", "{}");
    project.write("shared/model.ts", "export const value = 1;");
    let host = ProjectState::new();
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({"apps": [
            {"root":"apps/a", "projectRoot":"."},
            {"root":"apps/b", "projectRoot":"."}
        ]}))
        .expect("options"),
    )
    .expect("configured roots");
    let shared = project.uri("shared/model.ts");
    assert!(matches!(
        host.path_for_uri(&shared),
        Err(HostError::InvalidUri)
    ));
    for app in ["a", "b"] {
        host.path_for_uri(&project.uri(&format!("apps/{app}/page.wxml")))
            .expect("select app");
        let owner = host.project_id();
        assert_eq!(
            host.path_for_uri(&shared)
                .expect("active shared owner")
                .as_str(),
            "shared/model.ts"
        );
        assert_eq!(host.project_id(), owner);
    }
    let active = host.project_id();
    let other = host
        .resolve_file(&project.uri("apps/a/page.wxml"))
        .expect("resolve A");
    assert_ne!(other.project(), &active);
    assert_eq!(host.project_id(), active, "resolving an owner is read-only");
    assert_eq!(
        host.resolve_file(&shared).expect("shared owner").project(),
        &active
    );
    host.activate_project(other.project())
        .expect("activate saved owner");
    assert_eq!(&host.project_id(), other.project());
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({"apps": [
            {"root":"apps/a", "projectRoot":"apps"},
            {"root":"apps/b"}
        ]}))
        .expect("options"),
    )
    .expect("configured roots");
    assert_eq!(
        host.path_for_uri(&project.uri("apps/shared.ts"))
            .expect("unique shared owner")
            .as_str(),
        "shared.ts"
    );
    assert_eq!(host.root(), Some(project.root.join("apps")));
}

#[test]
fn explicit_app_does_not_infer_a_parent_tsconfig() {
    let project = TempProject::new();
    project.write("apps/a/src/app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("apps/a/src/pages/index/index.wxml", "{{ title }}");
    project.write(
        "apps/a/src/pages/index/index.ts",
        "Page({data:{title:'a'}});",
    );
    project.write(
        "apps/a/tsconfig.json",
        r#"{"compilerOptions":{"strict":false}}"#,
    );
    let host = ProjectState::new();
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({"apps":[{"root":"apps/a/src"}]})).expect("options"),
    )
    .expect("configured root");
    assert_eq!(host.root(), Some(project.root.join("apps/a/src")));
    let view = host
        .analysis(&snapshot(&host, "pages/index/index.wxml", "{{ title }}"))
        .expect("analysis");
    assert_ne!(view.compiler_options.get("strict"), Some(&json!(false)));
}

#[test]
fn watched_kind_maps_lsp_type_numbers() {
    assert_eq!(
        WatchedChangeKind::from_lsp(1),
        Some(WatchedChangeKind::Created)
    );
    assert_eq!(
        WatchedChangeKind::from_lsp(2),
        Some(WatchedChangeKind::Changed)
    );
    assert_eq!(
        WatchedChangeKind::from_lsp(3),
        Some(WatchedChangeKind::Deleted)
    );
    assert_eq!(WatchedChangeKind::from_lsp(0), None);
    assert_eq!(WatchedChangeKind::from_lsp(4), None);
}

// ---------------------------------------------------------------------------
// Explicit multi-app isolation and dependency invalidation (package P)
// ---------------------------------------------------------------------------

fn explicit_two_app_host(project: &TempProject) -> ProjectState {
    for app in ["a", "b"] {
        project.write(
            &format!("{app}/app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
        );
        project.write(
            &format!("{app}/pages/index/index.wxml"),
            "<view>{{ user.name }}</view>",
        );
        project.write(
            &format!("{app}/pages/index/index.ts"),
            &format!(
                "Page({{ data: {{ user: {{ name: 'DISK_{}' }} }} }});",
                app.to_uppercase()
            ),
        );
        project.write(
            &format!("{app}/tsconfig.json"),
            &format!(r#"{{"compilerOptions":{{"strict":{}}}}}"#, app == "a"),
        );
    }
    let host = ProjectState::new();
    host.open_root_with_options(
        Some(&project.root_uri()),
        &InitializationOptions::parse(&json!({
            "apps": [{"root": "a"}, {"root": "b"}]
        }))
        .expect("options"),
    )
    .expect("configured apps");
    host
}

#[test]
fn explicit_apps_isolate_same_relative_paths_and_overlays() {
    let project = TempProject::new();
    let host = explicit_two_app_host(&project);

    let path_a = host
        .path_for_uri(&project.uri("a/pages/index/index.wxml"))
        .expect("a path");
    let file_a = host.file_id(&path_a).expect("a file");
    let view_a = host.view_for_file(&file_a).expect("a view");
    let path_b = host
        .path_for_uri(&project.uri("b/pages/index/index.wxml"))
        .expect("b path");
    let file_b = host.file_id(&path_b).expect("b file");
    let view_b = host.view_for_file(&file_b).expect("b view");

    assert_eq!(path_a, path_b);
    assert_ne!(file_a, file_b);
    assert_ne!(file_a.project(), file_b.project());
    assert_ne!(view_a.view_revision, view_b.view_revision);
    assert_eq!(
        host.uri_for(&file_a).as_deref(),
        Some(project.uri("a/pages/index/index.wxml").as_str())
    );
    assert_eq!(
        host.uri_for(&file_b).as_deref(),
        Some(project.uri("b/pages/index/index.wxml").as_str())
    );

    let snapshot_a = snapshot(&host, path_a.as_str(), "<view>{{ user.name }}</view>");
    let snapshot_a = Arc::new(SourceSnapshot::new(
        file_a.clone(),
        snapshot_a.revision(),
        snapshot_a.snapshot(),
        snapshot_a.text(),
    ));
    let snapshot_b = Arc::new(SourceSnapshot::new(
        file_b,
        SourceRevision::new(1),
        SnapshotId::new(2),
        "<view>{{ user.name }}</view>",
    ));
    let analysis_a = host.analysis(&snapshot_a).expect("a analysis");
    let analysis_b = host.analysis(&snapshot_b).expect("b analysis");
    assert_eq!(analysis_a.project_root, project.root.join("a"));
    assert_eq!(analysis_b.project_root, project.root.join("b"));
    assert_eq!(analysis_a.compiler_options["strict"], json!(true));
    assert_eq!(analysis_b.compiler_options["strict"], json!(false));

    let update = host
        .open_document(opened(
            &project.uri("a/pages/index/index.ts"),
            "Page({ data: { user: { name: 'ZED_A_OVERLAY' } } });",
        ))
        .expect("a overlay");
    assert_eq!(
        update.dirty_views,
        vec![project_path("pages/index/index.wxml")]
    );
    assert_eq!(
        update
            .project_revision
            .as_ref()
            .map(wxml_types::ProjectRevision::project),
        Some(file_a.project())
    );
    let analysis_a = host.analysis(&snapshot_a).expect("a analysis with overlay");
    assert!(
        analysis_a
            .files
            .iter()
            .any(|file| file.text.contains("ZED_A_OVERLAY"))
    );
    let analysis_b = host
        .analysis(&snapshot_b)
        .expect("b analysis after a overlay");
    assert!(
        analysis_b
            .files
            .iter()
            .filter(|file| file.path.as_str() == "pages/index/index.ts")
            .all(|file| !file.text.contains("ZED_A_OVERLAY"))
    );

    // Background analysis of A must not steal foreground routing from B.
    host.path_for_uri(&project.uri("b/pages/index/index.wxml"))
        .expect("activate b");
    let active = host.project_id();
    let analysis_a = host.analysis(&snapshot_a).expect("background a");
    assert_eq!(host.project_id(), active);
    assert_eq!(analysis_a.project_root, project.root.join("a"));
}

#[test]
fn watched_changes_only_refresh_dependent_views() {
    let project = TempProject::new();
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/other/other"],"usingComponents":{}}"#,
    );
    for page in ["index", "other"] {
        project.write(
            &format!("pages/{page}/{page}.wxml"),
            "<view>{{ title }}</view>",
        );
        project.write(
            &format!("pages/{page}/{page}.ts"),
            "Page({ data: { title: 'x' } });",
        );
    }
    project.write(
        "pages/index/index.json",
        r#"{"usingComponents":{"card":"../../components/card/card"}}"#,
    );
    project.write("components/card/card.wxml", "<view>{{ label }}</view>");
    project.write(
        "components/card/card.ts",
        "Component({ properties: { label: String } });",
    );
    project.write("utils/helper.ts", "export const helper = 1;\n");
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    let host = host_with(&project);

    let update = host
        .watched_files_changed(&[watched(
            &project.uri("components/card/card.ts"),
            WatchedChangeKind::Changed,
        )])
        .expect("component script change");
    assert_eq!(
        update.dirty_views,
        vec![
            project_path("components/card/card.wxml"),
            project_path("pages/index/index.wxml")
        ]
    );

    let update = host
        .watched_files_changed(&[watched(
            &project.uri("pages/index/index.json"),
            WatchedChangeKind::Changed,
        )])
        .expect("page json change");
    assert_eq!(
        update.dirty_views,
        vec![project_path("pages/index/index.wxml")]
    );

    let update = host
        .watched_files_changed(&[watched(
            &project.uri("pages/other/other.wxml"),
            WatchedChangeKind::Changed,
        )])
        .expect("template change");
    assert_eq!(
        update.dirty_views,
        vec![project_path("pages/other/other.wxml")]
    );

    let update = host
        .watched_files_changed(&[watched(
            &project.uri("utils/helper.ts"),
            WatchedChangeKind::Changed,
        )])
        .expect("unknown script change");
    assert_eq!(update.dirty_views.len(), 3);

    let update = host
        .watched_files_changed(&[watched(
            &project.uri("tsconfig.json"),
            WatchedChangeKind::Changed,
        )])
        .expect("tsconfig change");
    assert_eq!(update.dirty_views.len(), 3);
}

#[test]
fn json_overlay_removing_a_page_clears_the_old_view() {
    let project = TempProject::new();
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/other/other"],"usingComponents":{}}"#,
    );
    for page in ["index", "other"] {
        project.write(
            &format!("pages/{page}/{page}.wxml"),
            "<view>{{ title }}</view>",
        );
        project.write(
            &format!("pages/{page}/{page}.ts"),
            "Page({ data: { title: 'x' } });",
        );
    }
    let host = host_with(&project);
    let index = project_path("pages/index/index.wxml");
    let other = project_path("pages/other/other.wxml");

    let update = host
        .open_document(opened(
            &project.uri("app.json"),
            r#"{"pages":["pages/other/other"]}"#,
        ))
        .expect("app.json overlay removing index");
    assert!(
        update.dirty_views.contains(&index),
        "removed view must be refreshed to clear diagnostics: {:?}",
        update.dirty_views
    );
    assert!(host.view_for(&index).is_err());
    assert!(host.view_for(&other).is_ok());

    let update = host
        .close_document(&project.uri("app.json"))
        .expect("close app.json overlay");
    assert!(
        update.dirty_views.contains(&index),
        "recreated view must be refreshed: {:?}",
        update.dirty_views
    );
    assert!(host.view_for(&index).is_ok());
}

#[test]
fn default_options_load_only_the_workspace_root() {
    let project = TempProject::new();
    project.write("apps/nested/app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("apps/nested/pages/index/index.wxml", "{{ title }}");
    project.write(
        "apps/nested/pages/index/index.ts",
        "Page({ data: { title: 1 } });",
    );
    let host = ProjectState::new();
    host.open_root_with_options(Some(&project.root_uri()), &InitializationOptions::default())
        .expect("default workspace root");
    assert_eq!(host.root(), Some(project.root.clone()));
    let path = host
        .path_for_uri(&project.uri("apps/nested/pages/index/index.wxml"))
        .expect("workspace-relative path");
    assert_eq!(path.as_str(), "apps/nested/pages/index/index.wxml");
    assert!(
        host.view_for(&path).is_err(),
        "nested app.json must not be auto-discovered"
    );
}

#[test]
fn reopening_the_workspace_produces_stable_project_ids() {
    let project = TempProject::new();
    for app in ["a", "b"] {
        project.write(&format!("{app}/app.json"), r#"{"pages":[]}"#);
    }
    let options = InitializationOptions::parse(&json!({
        "apps": [{"root": "a"}, {"root": "b"}]
    }))
    .expect("options");
    let collect = || {
        let host = ProjectState::new();
        host.open_root_with_options(Some(&project.root_uri()), &options)
            .expect("open");
        ["a", "b"]
            .into_iter()
            .map(|app| {
                host.path_for_uri(&project.uri(&format!("{app}/app.json")))
                    .expect("route app");
                host.project_id()
            })
            .collect::<Vec<_>>()
    };
    let first = collect();
    let second = collect();
    assert_eq!(
        first, second,
        "project identity must be stable across LSP restarts"
    );
    assert_ne!(first[0], first[1]);
}

#[test]
#[allow(clippy::too_many_lines)]
fn new_shared_tsconfig_reloads_the_non_owner_app() {
    let project = TempProject::new();
    for app in ["a", "b"] {
        project.write(
            &format!("{app}/app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
        );
        project.write(
            &format!("{app}/pages/index/index.wxml"),
            "<view>{{ user.name }}</view>",
        );
        project.write(
            &format!("{app}/pages/index/index.ts"),
            "Page({ data: { user: { name: 'x' } } });",
        );
    }
    let options = InitializationOptions::parse(&json!({
        "apps": [
            {"root": "a", "projectRoot": "."},
            {"root": "b", "projectRoot": "."}
        ]
    }))
    .expect("options");
    let host = ProjectState::new();
    host.open_root_with_options(Some(&project.root_uri()), &options)
        .expect("open shared root");

    let a_path = host
        .path_for_uri(&project.uri("a/pages/index/index.wxml"))
        .expect("a path");
    let a_file = host.file_id(&a_path).expect("a file");
    let a_snapshot = Arc::new(SourceSnapshot::new(
        a_file.clone(),
        SourceRevision::new(1),
        SnapshotId::new(1),
        "<view>{{ user.name }}</view>",
    ));
    let a_before = host.analysis(&a_snapshot).expect("a before");
    assert_eq!(a_before.compiler_options.get("strict"), None);

    let b_path = host
        .path_for_uri(&project.uri("b/pages/index/index.wxml"))
        .expect("b path");
    let b_file = host.file_id(&b_path).expect("b file");
    let b_snapshot = Arc::new(SourceSnapshot::new(
        b_file.clone(),
        SourceRevision::new(1),
        SnapshotId::new(2),
        "<view>{{ user.name }}</view>",
    ));
    assert_eq!(
        host.analysis(&b_snapshot)
            .expect("b before")
            .compiler_options
            .get("strict"),
        None
    );
    assert_ne!(a_file.project(), b_file.project());

    // A differently-cased file is not the project tsconfig and must not load.
    project.write("TSConfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    let update = host
        .watched_files_changed(&[watched(
            &project.uri("TSConfig.json"),
            WatchedChangeKind::Created,
        )])
        .expect("uppercase ignored");
    assert_eq!(update, HostUpdate::default());
    assert_eq!(
        host.analysis(&a_snapshot)
            .expect("a after uppercase")
            .compiler_options
            .get("strict"),
        None
    );

    // Creating a real tsconfig at runtime refreshes the active owner and marks
    // the other app sharing its projectRoot for a fresh disk load.
    project.write(
        "tsconfig.base.json",
        r#"{"compilerOptions":{"strict":true}}"#,
    );
    project.write("tsconfig.json", r#"{"extends":"./tsconfig.base.json"}"#);
    host.watched_files_changed(&[watched(
        &project.uri("tsconfig.json"),
        WatchedChangeKind::Created,
    )])
    .expect("new shared tsconfig");

    let a_after = host.analysis(&a_snapshot).expect("a after new tsconfig");
    assert_eq!(a_after.compiler_options.get("strict"), Some(&json!(true)));
    let b_after = host.analysis(&b_snapshot).expect("b after new tsconfig");
    assert_eq!(b_after.compiler_options.get("strict"), Some(&json!(true)));
    assert_ne!(
        a_after
            .target
            .as_ref()
            .expect("a target")
            .script_file
            .project(),
        b_after
            .target
            .as_ref()
            .expect("b target")
            .script_file
            .project()
    );

    // Renaming the tsconfig away refreshes both apps again; the uppercase file
    // is still not treated as configuration.
    std::fs::remove_file(project.root.join("tsconfig.json")).expect("remove tsconfig");
    host.watched_files_changed(&[watched(
        &project.uri("tsconfig.json"),
        WatchedChangeKind::Deleted,
    )])
    .expect("removed shared tsconfig");
    assert_eq!(
        host.analysis(&a_snapshot)
            .expect("a after removal")
            .compiler_options
            .get("strict"),
        None
    );
    assert_eq!(
        host.analysis(&b_snapshot)
            .expect("b after removal")
            .compiler_options
            .get("strict"),
        None
    );
}
