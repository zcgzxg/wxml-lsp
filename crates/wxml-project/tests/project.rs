//! End-to-end project model tests over public APIs and temporary directories.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

use wxml_project::ProjectPath;
use wxml_project::{
    DataRootKind, DiscoveryIssueKind, FileError, OverlayError, Project, ProjectError,
};

/// Base for temporary directories: `TMPDIR` when set, otherwise `/tmp/opencode`.
fn temp_base() -> PathBuf {
    std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from)
}

struct TempDir {
    /// Owns creation and recursive cleanup.
    _dir: tempfile::TempDir,
    /// Directory handle used by the existing helpers.
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(name)
            .tempdir_in(temp_base())
            .expect("create temp dir");
        let path = dir.path().to_path_buf();
        Self { _dir: dir, path }
    }

    fn root(&self) -> PathBuf {
        self.path.canonicalize().expect("canonical root")
    }

    fn write(&self, relative: &str, text: &str) {
        let target = self.path.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(target, text).expect("write file");
    }
}

fn path(raw: &str) -> ProjectPath {
    ProjectPath::new(raw).expect("valid path")
}

fn issue_kinds(project: &Project) -> Vec<DiscoveryIssueKind> {
    project.issues().iter().map(|issue| issue.kind).collect()
}

#[test]
fn opens_and_discovers_pages_and_components() {
    let temp = TempDir::new("open");
    temp.write(
        "app.json",
        r#"{
            "pages": ["pages/index/index"],
            "subPackages": [{"root": "pkg", "pages": ["a/a"]}]
        }"#,
    );
    temp.write("pages/index/index.wxml", "<view/>");
    temp.write("pages/index/index.ts", "Page({ data: { user: 'Ada' } });");
    temp.write(
        "pages/index/index.json",
        r#"{"usingComponents": {"card": "../../components/card/card"}}"#,
    );
    temp.write("pkg/a/a.wxml", "<view/>");
    temp.write("components/card/card.wxml", "<view/>");
    temp.write(
        "components/card/card.ts",
        "Component({ properties: { title: { type: String } } });",
    );

    let project = Project::open(temp.path.clone()).expect("open");
    assert_eq!(project.root(), temp.root());

    let routes: Vec<&str> = project
        .pages()
        .iter()
        .map(|page| page.route.as_str())
        .collect();
    assert_eq!(routes, ["pages/index/index", "pkg/a/a"]);

    let index = project.page("pages/index/index").expect("page");
    assert_eq!(index.wxml, path("pages/index/index.wxml"));
    assert_eq!(index.script, Some(path("pages/index/index.ts")));
    assert_eq!(
        index
            .config
            .get("card")
            .and_then(|entry| entry.component.as_ref()),
        Some(&path("components/card/card"))
    );
    assert!(project.page("./pages/index/index").is_some());
    assert!(project.page("pages\\index\\index").is_some());
    assert!(project.page("pages/missing/missing").is_none());

    let card = project
        .component(&path("components/card/card"))
        .expect("component");
    assert_eq!(card.properties.len(), 1);
    assert_eq!(card.properties[0].kind, DataRootKind::ComponentProperties);
    assert!(project.component(&path("components/other/other")).is_none());

    assert_eq!(issue_kinds(&project), Vec::new());
    assert!(project.tsconfig().is_none());
}

#[test]
fn overlay_wins_then_falls_back_to_disk() {
    let temp = TempDir::new("overlay");
    temp.write("app.json", r#"{"pages": []}"#);
    temp.write("pages/a/a.wxml", "<view>disk</view>");

    let mut project = Project::open(temp.path).expect("open");
    let page = path("pages/a/a.wxml");
    assert_eq!(project.read(&page).expect("disk"), "<view>disk</view>");

    project
        .set_overlay(&page, "<view>unsaved</view>", 2)
        .expect("set");
    assert_eq!(
        project.read(&page).expect("overlay"),
        "<view>unsaved</view>"
    );
    assert_eq!(project.overlay().revision(&page), Some(2));

    assert_eq!(
        project.set_overlay(&page, "<view>stale</view>", 2),
        Err(OverlayError::StaleRevision {
            stored: 2,
            attempted: 2
        })
    );
    assert_eq!(project.read(&page).expect("kept"), "<view>unsaved</view>");

    assert!(project.remove_overlay(&page));
    assert_eq!(
        project.read(&page).expect("disk again"),
        "<view>disk</view>"
    );

    let missing = path("pages/missing/missing.wxml");
    assert_eq!(
        project.read(&missing),
        Err(FileError::NotFound {
            path: missing.clone()
        })
    );
    assert_eq!(project.read_optional(&missing).expect("optional"), None);
}

#[test]
fn overlay_configuration_requires_rediscovery() {
    let temp = TempDir::new("rediscover");
    temp.write("app.json", r#"{"pages": ["pages/a/a"]}"#);
    temp.write("pages/a/a.wxml", "<view/>");
    temp.write("pages/b/b.wxml", "<view/>");

    let mut project = Project::open(temp.path).expect("open");
    assert_eq!(project.pages()[0].route, "pages/a/a");

    project
        .set_overlay(&path("app.json"), r#"{"pages": ["pages/b/b"]}"#, 1)
        .expect("overlay");
    assert_eq!(project.pages()[0].route, "pages/a/a");

    project.rediscover();
    assert_eq!(project.pages().len(), 1);
    assert_eq!(project.pages()[0].route, "pages/b/b");
}

#[test]
fn loads_tsconfig_and_reports_alias_errors_as_issues() {
    let temp = TempDir::new("tsconfig");
    temp.write("app.json", r#"{"pages": []}"#);
    temp.write(
        "tsconfig.json",
        r#"{
            "compilerOptions": {
                "baseUrl": ".",
                "paths": {"@/*": ["src/*"]}
            }
        }"#,
    );

    let project = Project::open(temp.path.clone()).expect("open");
    let config = project.tsconfig().expect("tsconfig");
    assert_eq!(config.resolve_alias("@/app"), Some(path("src/app")));
    assert_eq!(issue_kinds(&project), Vec::new());

    temp.write("tsconfig.json", "{");
    let project = Project::open(temp.path.clone()).expect("open");
    assert!(project.tsconfig().is_none());
    assert_eq!(issue_kinds(&project), [DiscoveryIssueKind::InvalidConfig]);
    assert_eq!(project.issues()[0].file, path("tsconfig.json"));
    assert!(
        !project.issues()[0]
            .message
            .contains(temp.root().to_str().expect("utf8 root"))
    );
}

#[test]
fn missing_app_json_is_an_issue_not_a_failure() {
    let temp = TempDir::new("missing-app");
    let project = Project::open(temp.path).expect("open");
    assert_eq!(project.pages(), &[] as &[wxml_project::Page]);
    assert_eq!(issue_kinds(&project), [DiscoveryIssueKind::MissingFile]);
}

#[test]
fn debug_output_hides_the_absolute_root() {
    let temp = TempDir::new("debug");
    temp.write("app.json", r#"{"pages": []}"#);
    let project = Project::open(temp.path.clone()).expect("open");
    let debug = format!("{project:?}");
    assert!(!debug.contains(temp.root().to_str().expect("utf8 root")));
    assert!(debug.contains("<project root>"));
    assert!(debug.contains("tsconfig"));
}

#[test]
fn maps_file_uris_inside_the_project_only() {
    let temp = TempDir::new("uri");
    temp.write("app.json", r#"{"pages": []}"#);
    let project = Project::open(temp.path).expect("open");

    let page = path("pages/a/a.wxml");
    let uri = project.uri_for(&page).expect("uri");
    assert_eq!(project.path_from_uri(&uri).expect("path"), page);
    assert_eq!(
        project.path_from_uri("file:///etc/passwd"),
        Err(wxml_project::UriError::OutsideRoot)
    );
}

#[test]
fn rejects_inaccessible_and_non_directory_roots() {
    let temp = TempDir::new("roots");
    let missing = temp.path.join("missing");
    assert!(matches!(
        Project::open(missing),
        Err(ProjectError::RootUnavailable { .. })
    ));

    let file = temp.path.join("file.txt");
    fs::write(&file, "x").expect("write");
    assert!(matches!(
        Project::open(file),
        Err(ProjectError::RootNotDirectory)
    ));
}

#[test]
fn data_roots_prefer_overlay_and_ignore_missing_scripts() {
    let temp = TempDir::new("data-roots");
    temp.write("app.json", r#"{"pages": ["pages/a/a"]}"#);
    temp.write("pages/a/a.wxml", "<view/>");
    temp.write("pages/a/a.ts", "Page({ data: { from: 'disk' } });");

    let mut project = Project::open(temp.path).expect("open");
    let script = path("pages/a/a.ts");
    let roots = project.data_roots(&script).expect("roots");
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].kind, DataRootKind::PageData);
    assert_eq!(roots[0].text, "{ from: 'disk' }");

    project
        .set_overlay(&script, "Page({ data: { from: 'overlay' } });", 1)
        .expect("set");
    let roots = project.data_roots(&script).expect("roots");
    assert_eq!(roots[0].text, "{ from: 'overlay' }");

    assert!(matches!(
        project.data_roots(&path("pages/a/a.js")),
        Err(FileError::NotFound { .. })
    ));
}

#[cfg(unix)]
#[test]
fn project_read_rejects_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new("symlink");
    let base = temp.root();
    let root = base.join("project");
    let outside = base.join("outside");
    fs::create_dir_all(&root).expect("create root");
    fs::create_dir_all(&outside).expect("create outside");
    fs::write(outside.join("secret.wxml"), "secret").expect("write");
    symlink(outside.join("secret.wxml"), root.join("secret-link.wxml")).expect("symlink");

    let project = Project::open(&root).expect("open");
    let escaped = path("secret-link.wxml");
    assert_eq!(
        project.read(&escaped),
        Err(FileError::EscapesRoot {
            path: escaped.clone()
        })
    );
    let uri = project.uri_for(&escaped).expect("uri");
    assert_eq!(
        project.path_from_uri(&uri),
        Err(wxml_project::UriError::OutsideRoot)
    );
}
