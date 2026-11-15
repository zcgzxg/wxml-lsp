//! P3b integration tests: compiler options, project-root URIs and source
//! collection.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wxml_project::discovery::MAX_COMPONENT_SCRIPT_BYTES;
use wxml_project::files::root_path_from_uri;
use wxml_project::sources::MAX_SOURCE_ISSUES;
use wxml_project::{
    DiscoveryIssueKind, Project, ProjectError, ProjectPath, SourceIssueKind, SourceKind,
    SourceLimits, UriError,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "wxml-project-p3b-{}-{id}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
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

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn path(raw: &str) -> ProjectPath {
    ProjectPath::new(raw).expect("valid path")
}

#[test]
fn compiler_options_merge_extends_and_paths() {
    let temp = TempDir::new("compiler-options");
    temp.write("app.json", r#"{"pages": []}"#);
    temp.write(
        "tsconfig.base.json",
        r#"{
            "compilerOptions": {
                "strict": true,
                "target": "ES2020",
                "moduleResolution": "node",
                "noUncheckedSideEffectImports": true,
                "paths": {
                    "@/*": ["src/*"],
                    "shared": ["shared/index"]
                }
            }
        }"#,
    );
    temp.write(
        "tsconfig.json",
        r#"{
            "extends": "./tsconfig.base",
            "compilerOptions": {
                "target": "ES2022",
                "experimentalDecorators": true,
                "paths": {
                    "@/*": ["app/*"],
                    "extra": ["extra/*"]
                }
            }
        }"#,
    );

    let project = Project::open(temp.path.clone()).expect("open");
    let options = project.compiler_options().expect("merged options");

    assert_eq!(options.get("strict").and_then(Value::as_bool), Some(true));
    assert_eq!(
        options.get("target").and_then(Value::as_str),
        Some("ES2022")
    );
    assert_eq!(
        options.get("moduleResolution").and_then(Value::as_str),
        Some("node")
    );
    assert_eq!(
        options
            .get("experimentalDecorators")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        options
            .get("noUncheckedSideEffectImports")
            .and_then(Value::as_bool),
        Some(true)
    );

    let paths = options
        .get("paths")
        .and_then(Value::as_object)
        .expect("raw paths object");
    assert_eq!(
        paths
            .get("@/*")
            .and_then(Value::as_array)
            .and_then(|targets| targets.first())
            .and_then(Value::as_str),
        Some("app/*")
    );
    assert_eq!(
        paths
            .get("shared")
            .and_then(Value::as_array)
            .and_then(|targets| targets.first())
            .and_then(Value::as_str),
        Some("shared/index")
    );
    assert_eq!(
        paths
            .get("extra")
            .and_then(Value::as_array)
            .and_then(|targets| targets.first())
            .and_then(Value::as_str),
        Some("extra/*")
    );

    let config = project.tsconfig().expect("tsconfig");
    assert_eq!(config.paths["@/*"], vec![path("app/*")]);
    assert_eq!(config.paths["shared"], vec![path("shared/index")]);
}

#[test]
fn compiler_options_none_without_tsconfig() {
    let temp = TempDir::new("no-tsconfig");
    temp.write("app.json", r#"{"pages": []}"#);
    let project = Project::open(temp.path.clone()).expect("open");
    assert!(project.tsconfig().is_none());
    assert!(project.compiler_options().is_none());
}

#[test]
fn open_uri_accepts_file_uri_and_rejects_remote_parent_nul() {
    let temp = TempDir::new("open-uri");
    temp.write("app.json", r#"{"pages": []}"#);
    let root = temp.root();
    let root_text = root.to_str().expect("utf8 root");
    let uri = format!("file://{root_text}");

    assert_eq!(root_path_from_uri(&uri).expect("decoded root"), root);
    let project = Project::open_uri(&uri).expect("open uri");
    assert_eq!(project.root(), root);

    let localhost = format!("file://localhost{root_text}");
    assert_eq!(
        root_path_from_uri(&localhost).expect("localhost root"),
        root
    );

    assert_eq!(
        root_path_from_uri("file://example.com/tmp/x"),
        Err(UriError::RemoteHost)
    );
    assert_eq!(
        root_path_from_uri("file:///tmp/../etc"),
        Err(UriError::ParentComponent)
    );
    assert_eq!(root_path_from_uri("file:///tmp/a%00b"), Err(UriError::Nul));
    assert_eq!(
        root_path_from_uri("file:relative"),
        Err(UriError::RelativePath)
    );
    assert!(matches!(
        Project::open_uri("https://example.com/project"),
        Err(ProjectError::Uri(UriError::NotFileScheme))
    ));
}

#[test]
fn collect_sources_includes_overlay_and_excludes_node_modules() {
    let temp = TempDir::new("sources");
    temp.write("app.json", r#"{"pages": []}"#);
    temp.write("src/a.ts", "export const a = 1;\n");
    temp.write("node_modules/x.ts", "export const x = 1;\n");
    temp.write("NODE_MODULES/y.ts", "export const y = 1;\n");
    temp.write("dist/z.ts", "export const z = 1;\n");

    let mut project = Project::open(temp.path.clone()).expect("open");
    project
        .set_overlay(&path("src/unsaved.ts"), "export const u = 2;\n", 7)
        .expect("overlay");

    let set = project.collect_sources(SourceLimits::default());
    let paths: Vec<&str> = set.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["src/a.ts", "src/unsaved.ts"]);
    assert_eq!(set.files[0].kind, SourceKind::Ts);
    assert_eq!(set.files[0].source_revision, 1);
    assert_eq!(set.files[1].kind, SourceKind::Ts);
    assert_eq!(set.files[1].source_revision, 7);
    assert!(!set.truncated);
    assert_eq!(set.issues, Vec::new());
}

#[test]
fn collect_sources_respects_file_and_total_limits() {
    let temp = TempDir::new("source-limits");
    temp.write("app.json", r#"{"pages": []}"#);
    // Create in reverse order to prove the result is path-sorted.
    temp.write("c.ts", "0123456789");
    temp.write("b.ts", "0123456789");
    temp.write("a.ts", "0123456789");
    temp.write("d.d.ts", "x");

    let project = Project::open(temp.path.clone()).expect("open");

    let by_files = project.collect_sources(SourceLimits {
        max_files: 2,
        max_total_bytes: 100,
        max_file_bytes: 16 * 1024 * 1024,
    });
    let paths: Vec<&str> = by_files
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["a.ts", "b.ts"]);
    assert!(by_files.truncated);
    assert_eq!(
        by_files
            .issues
            .iter()
            .filter(|issue| issue.kind == SourceIssueKind::Truncated)
            .count(),
        1
    );
    assert_eq!(
        by_files.issues.last().map(|issue| issue.path.as_str()),
        Some("c.ts")
    );

    let by_total = project.collect_sources(SourceLimits {
        max_files: 10,
        max_total_bytes: 15,
        max_file_bytes: 16 * 1024 * 1024,
    });
    let paths: Vec<&str> = by_total
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["a.ts"]);
    assert!(by_total.truncated);
    assert_eq!(
        by_total
            .issues
            .iter()
            .filter(|issue| issue.kind == SourceIssueKind::Truncated)
            .count(),
        1
    );

    let by_file_size = project.collect_sources(SourceLimits {
        max_files: 10,
        max_total_bytes: 100,
        max_file_bytes: 4,
    });
    let paths: Vec<&str> = by_file_size
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["d.d.ts"]);
    assert!(!by_file_size.truncated);
    assert_eq!(
        by_file_size
            .issues
            .iter()
            .filter(|issue| issue.kind == SourceIssueKind::TooLarge)
            .count(),
        3
    );
}

#[cfg(unix)]
#[test]
fn collect_sources_skips_symlinks() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new("source-symlink");
    temp.write("app.json", r#"{"pages": []}"#);
    temp.write("src/a.ts", "export const a = 1;\n");
    symlink(temp.path.join("src/a.ts"), temp.path.join("src/link.ts")).expect("symlink script");

    let project = Project::open(temp.path.clone()).expect("open");
    let set = project.collect_sources(SourceLimits::default());
    let paths: Vec<&str> = set.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["src/a.ts"]);
    assert_eq!(
        set.issues
            .iter()
            .map(|issue| (issue.path.as_str(), issue.kind))
            .collect::<Vec<_>>(),
        [("src/link.ts", SourceIssueKind::Unreadable)]
    );
}

#[cfg(unix)]
#[test]
fn source_issues_are_bounded() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const LINKS: usize = 1_100;

    let temp = TempDir::new("source-issue-budget");
    temp.write("app.json", r#"{"pages": []}"#);
    temp.write("src/a.ts", "export const a = 1;\n");
    let target = temp.path.join("src/a.ts");
    for index in 0..LINKS {
        symlink(&target, temp.path.join(format!("link{index:04}.ts"))).expect("symlink script");
    }

    let project = Project::open(temp.path.clone()).expect("open");
    let started = Instant::now();
    let set = project.collect_sources(SourceLimits::default());
    let elapsed = started.elapsed();

    assert_eq!(set.issues.len(), MAX_SOURCE_ISSUES);
    assert_eq!(
        set.issues
            .iter()
            .filter(|issue| issue.kind == SourceIssueKind::Truncated)
            .count(),
        1
    );
    assert!(set.truncated);
    assert!(elapsed < Duration::from_secs(5), "elapsed={elapsed:?}");
}

#[test]
fn overlay_revision_zero_is_preserved() {
    let temp = TempDir::new("zero-revision");
    temp.write("app.json", r#"{"pages": []}"#);
    let mut project = Project::open(temp.path.clone()).expect("open");
    project
        .set_overlay(&path("src/zero.ts"), "export const zero = 0;\n", 0)
        .expect("overlay");

    let set = project.collect_sources(SourceLimits::default());
    assert_eq!(set.files.len(), 1);
    assert_eq!(set.files[0].path, path("src/zero.ts"));
    assert_eq!(set.files[0].source_revision, 0);
}

#[cfg(unix)]
#[test]
fn discovery_json_budget_limits_symlinked_pages() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const PAGES: usize = 300;

    let temp = TempDir::new("json-budget-pages");
    let routes: Vec<String> = (0..PAGES)
        .map(|index| format!("\"pages/p{index}/p{index}\""))
        .collect();
    temp.write(
        "app.json",
        &format!("{{\"pages\": [{}]}}", routes.join(",")),
    );

    let padding = "x".repeat(8 * 1024 * 1024);
    temp.write(
        "shared-page.json",
        &format!("{{\"padding\": \"{padding}\", \"usingComponents\": {{}}}}"),
    );
    let shared = temp.path.join("shared-page.json");
    for index in 0..PAGES {
        temp.write(&format!("pages/p{index}/p{index}.wxml"), "<view/>");
        symlink(
            &shared,
            temp.path.join(format!("pages/p{index}/p{index}.json")),
        )
        .expect("symlink page json");
    }

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();
    assert_eq!(project.pages().len(), PAGES);
    assert!(
        project
            .issues()
            .iter()
            .any(|issue| issue.kind == DiscoveryIssueKind::Truncated)
    );
    assert!(elapsed < Duration::from_secs(5), "elapsed={elapsed:?}");
}

#[cfg(unix)]
#[test]
fn discovery_json_budget_limits_symlinked_components() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const COMPONENTS: usize = 80;

    let temp = TempDir::new("json-budget-components");
    let entries: Vec<String> = (0..COMPONENTS)
        .map(|index| format!("\"c{index}\": \"/components/c{index}/c{index}\""))
        .collect();
    temp.write(
        "app.json",
        &format!(
            "{{\"pages\": [], \"usingComponents\": {{{}}}}}",
            entries.join(",")
        ),
    );

    let padding = "x".repeat(4 * 1024 * 1024);
    temp.write(
        "shared-component.json",
        &format!("{{\"padding\": \"{padding}\", \"usingComponents\": {{}}}}"),
    );
    let shared = temp.path.join("shared-component.json");
    for index in 0..COMPONENTS {
        temp.write(&format!("components/c{index}/c{index}.wxml"), "<view/>");
        symlink(
            &shared,
            temp.path.join(format!("components/c{index}/c{index}.json")),
        )
        .expect("symlink component json");
    }

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();
    assert_eq!(project.components().len(), COMPONENTS);
    assert!(
        project
            .issues()
            .iter()
            .any(|issue| issue.kind == DiscoveryIssueKind::Truncated)
    );
    assert!(elapsed < Duration::from_secs(5), "elapsed={elapsed:?}");
}

#[cfg(unix)]
#[test]
fn component_script_budget_is_bounded() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const COMPONENTS: usize = 100;

    let temp = TempDir::new("component-script-budget");
    let entries: Vec<String> = (0..COMPONENTS)
        .map(|index| format!("\"c{index}\": \"/components/c{index}/c{index}\""))
        .collect();
    temp.write(
        "app.json",
        &format!(
            "{{\"pages\": [], \"usingComponents\": {{{}}}}}",
            entries.join(",")
        ),
    );

    let padding = "x".repeat(4 * 1024 * 1024);
    temp.write(
        "shared.ts",
        &format!("Component({{ properties: {{ big: {{ value: \"{padding}\" }} }} }});"),
    );
    temp.write("shared-component.json", r#"{"usingComponents": {}}"#);
    let shared_script = temp.path.join("shared.ts");
    let shared_json = temp.path.join("shared-component.json");
    for index in 0..COMPONENTS {
        temp.write(&format!("components/c{index}/c{index}.wxml"), "<view/>");
        symlink(
            &shared_json,
            temp.path.join(format!("components/c{index}/c{index}.json")),
        )
        .expect("symlink component json");
        symlink(
            &shared_script,
            temp.path.join(format!("components/c{index}/c{index}.ts")),
        )
        .expect("symlink component script");
    }

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();

    assert_eq!(project.components().len(), COMPONENTS);
    let retained: usize = project
        .components()
        .iter()
        .flat_map(|component| component.properties.iter())
        .map(|root| root.text.len())
        .sum();
    assert!(
        retained <= MAX_COMPONENT_SCRIPT_BYTES,
        "retained={retained}"
    );
    assert!(project.truncated());
    assert_eq!(
        project
            .issues()
            .iter()
            .filter(|issue| issue.kind == DiscoveryIssueKind::Truncated)
            .count(),
        1
    );
    assert!(
        !project
            .issues()
            .iter()
            .any(|issue| issue.kind == DiscoveryIssueKind::UnreadableFile)
    );
    assert!(elapsed < Duration::from_secs(5), "elapsed={elapsed:?}");
}

#[cfg(unix)]
#[test]
fn invalid_utf8_component_scripts_are_budgeted() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const COMPONENTS: usize = 100;

    let temp = TempDir::new("invalid-script-budget");
    let entries: Vec<String> = (0..COMPONENTS)
        .map(|index| format!("\"c{index}\": \"/components/c{index}/c{index}\""))
        .collect();
    temp.write(
        "app.json",
        &format!(
            "{{\"pages\": [], \"usingComponents\": {{{}}}}}",
            entries.join(",")
        ),
    );

    let mut invalid = vec![b'a'; 16 * 1024 * 1024];
    invalid[0] = 0xFF;
    fs::write(temp.path.join("shared.ts"), &invalid).expect("write invalid script");
    let shared_script = temp.path.join("shared.ts");
    for index in 0..COMPONENTS {
        temp.write(&format!("components/c{index}/c{index}.wxml"), "<view/>");
        symlink(
            &shared_script,
            temp.path.join(format!("components/c{index}/c{index}.ts")),
        )
        .expect("symlink component script");
    }

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();

    assert_eq!(project.components().len(), COMPONENTS);
    assert!(
        project
            .components()
            .iter()
            .all(|component| component.properties.is_empty())
    );
    assert!(project.truncated());
    assert_eq!(
        project
            .issues()
            .iter()
            .filter(|issue| issue.kind == DiscoveryIssueKind::Truncated)
            .count(),
        1
    );
    let unreadable = project
        .issues()
        .iter()
        .filter(|issue| issue.kind == DiscoveryIssueKind::UnreadableFile)
        .count();
    assert!(unreadable <= 2, "unreadable={unreadable}");
    assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
}

#[cfg(unix)]
#[test]
fn invalid_utf8_page_json_is_budgeted() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const PAGES: usize = 1_000;

    let temp = TempDir::new("invalid-page-json");
    let routes: Vec<String> = (0..PAGES)
        .map(|index| format!("\"pages/p{index}/p{index}\""))
        .collect();
    temp.write(
        "app.json",
        &format!("{{\"pages\": [{}]}}", routes.join(",")),
    );

    let mut invalid = vec![b'a'; 16 * 1024 * 1024];
    invalid[0] = 0xFF;
    fs::write(temp.path.join("shared.json"), &invalid).expect("write invalid json");
    let shared = temp.path.join("shared.json");
    for index in 0..PAGES {
        temp.write(&format!("pages/p{index}/p{index}.wxml"), "<view/>");
        symlink(
            &shared,
            temp.path.join(format!("pages/p{index}/p{index}.json")),
        )
        .expect("symlink page json");
    }

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();

    assert_eq!(project.pages().len(), PAGES);
    assert!(project.truncated());
    assert_eq!(
        project
            .issues()
            .iter()
            .filter(|issue| issue.kind == DiscoveryIssueKind::Truncated)
            .count(),
        1
    );
    let unreadable = project
        .issues()
        .iter()
        .filter(|issue| issue.kind == DiscoveryIssueKind::UnreadableFile)
        .count();
    assert!(unreadable <= 2, "unreadable={unreadable}");
    assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
}

#[cfg(unix)]
#[test]
fn invalid_utf8_component_json_is_budgeted() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    const COMPONENTS: usize = 1_000;

    let temp = TempDir::new("invalid-component-json");
    let entries: Vec<String> = (0..COMPONENTS)
        .map(|index| format!("\"c{index}\": \"/components/c{index}/c{index}\""))
        .collect();
    temp.write(
        "app.json",
        &format!(
            "{{\"pages\": [], \"usingComponents\": {{{}}}}}",
            entries.join(",")
        ),
    );

    let mut invalid = vec![b'a'; 16 * 1024 * 1024];
    invalid[0] = 0xFF;
    fs::write(temp.path.join("shared.json"), &invalid).expect("write invalid json");
    let shared = temp.path.join("shared.json");
    for index in 0..COMPONENTS {
        temp.write(&format!("components/c{index}/c{index}.wxml"), "<view/>");
        symlink(
            &shared,
            temp.path.join(format!("components/c{index}/c{index}.json")),
        )
        .expect("symlink component json");
    }

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();

    assert_eq!(project.components().len(), COMPONENTS);
    assert!(project.truncated());
    assert_eq!(
        project
            .issues()
            .iter()
            .filter(|issue| issue.kind == DiscoveryIssueKind::Truncated)
            .count(),
        1
    );
    let unreadable = project
        .issues()
        .iter()
        .filter(|issue| issue.kind == DiscoveryIssueKind::UnreadableFile)
        .count();
    assert!(unreadable <= 2, "unreadable={unreadable}");
    assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
}

#[test]
fn invalid_utf8_app_json_is_budgeted() {
    use std::time::{Duration, Instant};

    let temp = TempDir::new("invalid-app-json");
    let mut invalid = vec![b'a'; 1024 * 1024];
    invalid[0] = 0xFF;
    fs::write(temp.path.join("app.json"), &invalid).expect("write invalid app json");

    let started = Instant::now();
    let project = Project::open(temp.path.clone()).expect("open");
    let elapsed = started.elapsed();

    assert_eq!(project.pages(), &[] as &[wxml_project::Page]);
    assert!(project.truncated());
    assert_eq!(
        project
            .issues()
            .iter()
            .filter(|issue| issue.kind == DiscoveryIssueKind::Truncated)
            .count(),
        1
    );
    assert_eq!(
        project
            .issues()
            .iter()
            .filter(|issue| issue.kind == DiscoveryIssueKind::UnreadableFile)
            .count(),
        1
    );
    assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
}
