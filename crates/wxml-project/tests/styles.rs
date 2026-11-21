#![allow(clippy::assert_is_empty, clippy::expect_used, clippy::unwrap_used)]

//! Project-level style index and local resource resolution tests.

use std::path::PathBuf;

use wxml_project::{
    Project, ProjectPath, ResourceKind, ResourceResolution, StyleConfig, StyleDiagnosticsPolicy,
    StyleIndex, StyleIsolation, StyleIssueKind, StyleLimits, StyleScope,
};

/// Base for temporary directories: `TMPDIR` when set, otherwise `/tmp/opencode`.
fn temp_base() -> PathBuf {
    std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from)
}

struct TempProject {
    /// Owns creation and recursive cleanup.
    _dir: tempfile::TempDir,
    /// Directory handle used by the existing helpers.
    root: PathBuf,
}

impl TempProject {
    fn new() -> Self {
        let dir = tempfile::tempdir_in(temp_base()).expect("create temp project");
        let root = dir.path().to_path_buf();
        Self { _dir: dir, root }
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(target, text).expect("write fixture");
    }
}

fn fixture() -> TempProject {
    let project = TempProject::new();
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{"card":"/components/card/card"}}"#,
    );
    project.write(
        "app.wxss",
        "/* app global */\n@import \"styles/theme.scss\";\n.app-global { color: black; }\n",
    );
    project.write(
        "styles/theme.scss",
        "/* theme */\n.theme { color: blue; }\n.card { color: green; }\n",
    );
    project.write(
        "styles/extra.wxss",
        "/* configured */\n.configured { color: gray; }\n",
    );
    project.write("pages/index/index.wxml", "<view class=\"page\">x</view>");
    project.write("pages/index/index.ts", "Page({ data: { title: 'ok' } });");
    project.write(
        "pages/index/index.wxss",
        r#"
@import "../../shared/card";
.page { color: red; }
.card {
  &-header { color: blue; }
  &.active { color: green; }
  .title { color: black; }
}
"#,
    );
    project.write(
        "shared/card.scss",
        ".shared { color: red; }\n/* shared card */\n.card { color: orange; }\n",
    );
    project.write("components/card/card.wxml", "<view class=\"comp\">x</view>");
    project.write("components/card/card.ts", "Component({})");
    project.write("components/card/card.json", "{}");
    project.write("components/card/card.wxss", ".comp { color: red; }\n");
    project
}

fn open(project: &TempProject) -> Project {
    Project::open(&project.root).expect("open project")
}

fn path(raw: &str) -> ProjectPath {
    ProjectPath::new(raw).expect("valid project path")
}

fn facts(
    project: &Project,
    index: &mut StyleIndex,
    view: &str,
    config: &StyleConfig,
) -> wxml_project::StyleViewFacts {
    index.view_facts(project, &path(view), config)
}

#[test]
fn page_component_app_and_configured_classes_are_scoped() {
    let project = fixture();
    let project = open(&project);
    let config = StyleConfig::with_global_styles([path("styles/extra.wxss")]);
    let mut index = StyleIndex::new();
    let page = facts(&project, &mut index, "pages/index/index.wxml", &config);
    assert!(page.complete, "{:?}", page.issues);
    let names = page.class_names();
    for expected in [
        "page",
        "shared",
        "card",
        "card-header",
        "active",
        "title",
        "app-global",
        "theme",
        "configured",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    assert_eq!(page.primary("page").expect("page").scope, StyleScope::Page);
    assert_eq!(
        page.primary("app-global").expect("global").scope,
        StyleScope::AppGlobal
    );
    assert_eq!(
        page.primary("configured").expect("configured").scope,
        StyleScope::ConfiguredGlobal
    );
    let card_definitions = page.definitions("card");
    assert_eq!(card_definitions.len(), 3, "{card_definitions:?}");
    assert_eq!(
        card_definitions[0].file().as_str(),
        "pages/index/index.wxss",
        "the page's own rule follows the import in the cascade"
    );
    assert!(card_definitions.iter().any(|class| {
        class.file().as_str() == "shared/card.scss" && class.comment() == Some("shared card")
    }));
    assert!(
        card_definitions
            .iter()
            .any(|class| class.file().as_str() == "styles/theme.scss")
    );

    let component = facts(&project, &mut index, "components/card/card.wxml", &config);
    let component_names = component.class_names();
    assert!(component_names.contains(&"comp"));
    assert!(
        !component_names.contains(&"app-global"),
        "default component isolation excludes app styles: {component_names:?}"
    );
    assert!(!component_names.contains(&"page"), "{component_names:?}");
    assert!(component.complete);
    assert_eq!(component.isolation, StyleIsolation::Isolated);
    assert_eq!(
        component.primary("comp").expect("comp").scope,
        StyleScope::Component
    );

    let page_file = path("pages/index/index.wxss");
    let page_classes: Vec<_> = page
        .classes_in_file(&page_file)
        .map(wxml_project::ClassDefinition::name)
        .collect();
    assert_eq!(
        page_classes,
        vec!["page", "card", "card-header", "active", "title"]
    );
}

#[test]
fn missing_or_invalid_configured_global_styles_make_the_index_partial() {
    let project = fixture();
    let project = open(&project);
    let config = StyleConfig {
        global_styles: vec![path("styles/missing.wxss"), path("styles/not-a-style.txt")],
        invalid_global_styles: vec!["/absolute/theme.wxss".to_owned()],
    };
    let mut index = StyleIndex::new();
    let facts = facts(&project, &mut index, "pages/index/index.wxml", &config);
    assert!(!facts.complete);
    assert!(
        facts
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::MissingImport)
    );
    assert!(facts.reason.is_some());
    assert!(
        facts.primary("app-global").is_some(),
        "valid app style remains"
    );
}

#[test]
fn nested_and_ampersand_selectors_are_expanded() {
    let project = fixture();
    let project = open(&project);
    let mut index = StyleIndex::new();
    let page = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(
        page.primary("card-header").expect("header").selector(),
        ".card-header"
    );
    assert_eq!(
        page.primary("active").expect("active").selector(),
        ".card.active"
    );
    assert_eq!(
        page.primary("title").expect("title").selector(),
        ".card .title"
    );
}

#[test]
fn cycles_remote_and_escaping_imports_are_bounded() {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.wxml", "<view/>");
    project.write("pages/index/index.ts", "Page({})");
    project.write(
        "pages/index/index.wxss",
        "@import \"../../shared/a.scss\";\n.page { }\n",
    );
    project.write("shared/a.scss", "@import \"b.scss\";\n.a { }\n");
    project.write("shared/b.scss", "@import \"a.scss\";\n.b { }\n");
    project.write("pages/remote/index.wxml", "<view/>");
    project.write("pages/remote/index.ts", "Page({})");
    project.write(
        "pages/remote/index.wxss",
        "@import \"https://example.com/x.css\";\n.remote { }\n",
    );
    project.write("outside.scss", ".outside { }");
    project.write("pages/escape/index.wxml", "<view/>");
    project.write("pages/escape/index.ts", "Page({})");
    project.write(
        "pages/escape/index.wxss",
        "@import \"../../../outside.scss\";\n.escape { }\n",
    );
    // Discovery lists the extra pages only when app.json mentions them.
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/remote/index","pages/escape/index"]}"#,
    );
    let project = open(&project);
    let mut index = StyleIndex::new();

    let cycle = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!cycle.complete);
    assert!(
        cycle
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::CyclicImport),
        "{:?}",
        cycle.issues
    );
    assert!(cycle.primary("a").is_some());
    assert!(cycle.primary("b").is_some());

    let remote = facts(
        &project,
        &mut index,
        "pages/remote/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!remote.complete);
    assert!(
        remote
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::RemoteImport),
        "{:?}",
        remote.issues
    );
    assert!(remote.primary("remote").is_some());

    let escape = facts(
        &project,
        &mut index,
        "pages/escape/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!escape.complete);
    assert!(escape.primary("outside").is_none());
    assert!(escape.primary("escape").is_some());
    assert!(
        escape
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::EscapesRoot),
        "{:?}",
        escape.issues
    );
}

#[cfg(unix)]
#[test]
fn symlinked_style_files_do_not_escape_the_root() {
    use std::os::unix::fs::symlink;

    let project = TempProject::new();
    let outside = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.wxml", "<view/>");
    project.write("pages/index/index.ts", "Page({})");
    project.write(
        "pages/index/index.wxss",
        "@import \"../../shared/link.scss\";\n.page { }\n",
    );
    outside.write("secret.scss", ".secret { color: red; }\n");
    std::fs::create_dir_all(project.root.join("shared")).expect("shared");
    symlink(
        outside.root.join("secret.scss"),
        project.root.join("shared/link.scss"),
    )
    .expect("symlink");
    let project = open(&project);
    let mut index = StyleIndex::new();
    let facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!facts.complete);
    assert!(facts.primary("secret").is_none());
    assert!(facts.primary("page").is_some());
}

#[cfg(unix)]
#[test]
fn unquoted_remote_url_import_is_not_complete() {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.wxml", "<view/>");
    project.write("pages/index/index.ts", "Page({})");
    project.write(
        "pages/index/index.scss",
        "@import url(https://cdn.example/x.css);\n.local { }\n",
    );
    let project = open(&project);
    let mut index = StyleIndex::new();
    let facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!facts.complete);
    assert!(
        facts
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::RemoteImport)
    );
    assert!(facts.primary("local").is_some());
}

#[test]
fn scss_builtins_and_use_with_do_not_make_the_index_partial() {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.wxml", "<view/>");
    project.write("pages/index/index.ts", "Page({})");
    project.write(
        "pages/index/index.scss",
        "@use \"sass:math\";\n@use \"theme\" with ($primary: \"body\");\n.local { }\n",
    );
    project.write("pages/index/theme.scss", ".theme-class { }\n");
    let project = open(&project);
    let mut index = StyleIndex::new();
    let facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(facts.primary("local").is_some());
    assert!(facts.primary("theme-class").is_some());
    assert!(facts.complete, "{:?}", facts.issues);
}

#[cfg(unix)]
#[test]
fn symlinked_view_style_root_reports_escape() {
    use std::os::unix::fs::symlink;
    let project = fixture();
    let outside = TempProject::new();
    outside.write(
        "secret.scss",
        ".secret { }
",
    );
    std::fs::remove_file(project.root.join("pages/index/index.wxss")).expect("remove style");
    symlink(
        outside.root.join("secret.scss"),
        project.root.join("pages/index/index.wxss"),
    )
    .expect("symlink");
    let project = open(&project);
    let mut index = StyleIndex::new();
    let facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!facts.complete);
    assert!(facts.primary("secret").is_none());
    assert!(
        facts
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::EscapesRoot)
    );
}

#[test]
fn budgets_limit_file_count() {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.wxml", "<view/>");
    project.write("pages/index/index.ts", "Page({})");
    project.write(
        "pages/index/index.wxss",
        "@import \"../../shared/big.scss\";\n.page { }\n",
    );
    project.write("shared/big.scss", ".big { }\n");
    let project = open(&project);

    let mut index = StyleIndex::with_limits(StyleLimits {
        max_files: 1,
        ..StyleLimits::default()
    });
    let limited = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!limited.complete);
    assert_eq!(limited.files.len(), 1);
    assert!(
        limited
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::Truncated),
        "{:?}",
        limited.issues
    );
}

#[test]
fn total_byte_and_import_depth_budgets_are_really_triggered() {
    let project = fixture();
    let project = open(&project);
    let mut index = StyleIndex::with_limits(StyleLimits {
        max_total_bytes: 1,
        ..StyleLimits::default()
    });
    let limited = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!limited.complete);
    assert!(
        limited
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::TooLarge)
    );

    let project = fixture();
    let project = open(&project);
    let mut index = StyleIndex::with_limits(StyleLimits {
        max_import_depth: 0,
        ..StyleLimits::default()
    });
    let shallow = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!shallow.complete);
    assert!(shallow.primary("page").is_some());
    assert!(
        shallow.primary("shared").is_none(),
        "imported classes are beyond the depth budget"
    );

    let project = fixture();
    let project = open(&project);
    let mut index = StyleIndex::with_limits(StyleLimits {
        max_imports_per_view: 0,
        ..StyleLimits::default()
    });
    let no_imports = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!no_imports.complete);
    assert!(no_imports.primary("page").is_some());
    assert!(no_imports.primary("shared").is_none());
    assert!(
        no_imports
            .issues
            .iter()
            .any(|issue| issue.kind == StyleIssueKind::Truncated)
    );
}

#[test]
fn overlays_and_invalidation_refresh_class_facts() {
    let project = fixture();
    let mut project = open(&project);
    let path = path("pages/index/index.wxss");
    let mut index = StyleIndex::new();
    let before = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(before.primary("page").is_some());

    project
        .set_overlay(&path, ".overlay { color: red; }\n", 1)
        .expect("overlay");
    index.invalidate(&path);
    let after = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(after.primary("overlay").is_some());
    assert!(after.primary("page").is_none());

    assert!(project.remove_overlay(&path));
    index.invalidate_all();
    let restored = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(restored.primary("page").is_some());
    assert!(restored.primary("overlay").is_none());
}

#[test]
fn style_diagnostics_default_off_and_require_a_complete_index() {
    let project = fixture();
    let project = open(&project);
    let mut index = StyleIndex::new();
    let facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    let used = vec![
        "missing".to_owned(),
        "page".to_owned(),
        "{{dynamic}}".to_owned(),
    ];
    assert!(
        StyleDiagnosticsPolicy::default()
            .undefined_classes(&facts, &used)
            .is_empty()
    );
    assert_eq!(
        StyleDiagnosticsPolicy::enabled().undefined_classes(&facts, &used),
        vec!["missing".to_owned()]
    );

    let mut partial = facts;
    partial.complete = false;
    assert!(
        StyleDiagnosticsPolicy::enabled()
            .undefined_classes(&partial, &used)
            .is_empty()
    );
}

struct ResourceFixture {
    project_root: TempProject,
    _outside: TempProject,
    project: Project,
}

#[test]
fn component_isolation_controls_app_and_page_styles() {
    let project = fixture();
    project.write(
        "components/card/card.json",
        r#"{"styleIsolation":"apply-shared"}"#,
    );
    let project = open(&project);
    let mut index = StyleIndex::new();
    let component = facts(
        &project,
        &mut index,
        "components/card/card.wxml",
        &StyleConfig::default(),
    );
    let names = component.class_names();
    assert!(names.contains(&"app-global"), "{names:?}");
    assert!(
        !component.complete,
        "unknown enclosing page styles must keep the component index partial"
    );
    assert_eq!(component.isolation, StyleIsolation::ApplyShared);
    let shared_token = component
        .primary("app-global")
        .expect("component app class");
    let page_facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    let page_token = page_facts.primary("app-global").expect("page app class");
    assert!(
        std::sync::Arc::ptr_eq(&shared_token.token, &page_token.token),
        "parsed class tokens are shared across view facts"
    );
}

#[test]
fn shared_component_and_add_global_class_include_app_but_stay_partial() {
    for config in [
        r#"{"styleIsolation":"shared"}"#,
        r#"{"addGlobalClass":true}"#,
    ] {
        let project = fixture();
        project.write("components/card/card.json", config);
        let project = open(&project);
        let mut index = StyleIndex::new();
        let component = facts(
            &project,
            &mut index,
            "components/card/card.wxml",
            &StyleConfig::default(),
        );
        assert!(component.class_names().contains(&"app-global"), "{config}");
        assert!(!component.complete, "{config}");
    }
}

#[test]
fn component_script_options_are_combined_with_json() {
    let project = fixture();
    project.write(
        "components/card/card.ts",
        "Component({ options: { addGlobalClass: true } })",
    );
    let opened = open(&project);
    let mut index = StyleIndex::new();
    let component = facts(
        &opened,
        &mut index,
        "components/card/card.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(component.isolation, StyleIsolation::ApplyShared);
    assert!(component.class_names().contains(&"app-global"));
    assert!(!component.complete);

    project.write(
        "components/card/card.json",
        r#"{"styleIsolation":"isolated"}"#,
    );
    let reopened = open(&project);
    let mut index = StyleIndex::new();
    let conflict = facts(
        &reopened,
        &mut index,
        "components/card/card.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(conflict.isolation, StyleIsolation::Unknown);
    assert!(!conflict.complete);
}

#[test]
fn page_modes_and_component_constructed_pages_follow_official_isolation() {
    let project = fixture();
    project.write("pages/index/index.ts", "Component({ options: {} })");
    let project = open(&project);
    let mut index = StyleIndex::new();
    let component_page = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(component_page.isolation, StyleIsolation::Shared);
    assert!(component_page.class_names().contains(&"app-global"));
    assert!(!component_page.complete);

    let project = fixture();
    project.write(
        "pages/index/index.ts",
        "Component({ options: { addGlobalClass: true } })",
    );
    let project = open(&project);
    let mut index = StyleIndex::new();
    let global_component_page = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(global_component_page.isolation, StyleIsolation::Shared);
    assert!(global_component_page.class_names().contains(&"app-global"));
    assert!(!global_component_page.complete);

    let project = fixture();
    project.write(
        "pages/index/index.ts",
        "Component({ options: { styleIsolation: 'isolated' } })",
    );
    let project = open(&project);
    let mut index = StyleIndex::new();
    let isolated_component_page = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(
        isolated_component_page.isolation,
        StyleIsolation::PageShared
    );
    assert!(
        !isolated_component_page
            .class_names()
            .contains(&"app-global")
    );
    assert!(!isolated_component_page.complete);

    for (mode, expected, app_global, complete) in [
        ("page-isolated", StyleIsolation::PageIsolated, false, true),
        (
            "page-apply-shared",
            StyleIsolation::PageApplyShared,
            false,
            false,
        ),
        ("page-shared", StyleIsolation::PageShared, false, false),
    ] {
        let project = fixture();
        project.write(
            "pages/index/index.json",
            &format!(r#"{{"styleIsolation":"{mode}"}}"#),
        );
        let project = open(&project);
        let mut index = StyleIndex::new();
        let page = facts(
            &project,
            &mut index,
            "pages/index/index.wxml",
            &StyleConfig::default(),
        );
        assert_eq!(page.isolation, expected, "{mode}");
        assert_eq!(
            page.class_names().contains(&"app-global"),
            app_global,
            "{mode}"
        );
        assert_eq!(page.complete, complete, "{mode}");
    }

    // Ordinary Page with no JSON remains the app-sharing default.
    let project = fixture();
    let project = open(&project);
    let mut index = StyleIndex::new();
    let page = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(page.isolation, StyleIsolation::Default);
    assert!(page.class_names().contains(&"app-global"));
    assert!(page.complete);
}

#[test]
fn unknown_isolation_values_are_partial() {
    let project = fixture();
    project.write(
        "components/card/card.json",
        r#"{"styleIsolation":"future-mode"}"#,
    );
    let project = open(&project);
    let mut index = StyleIndex::new();
    let component = facts(
        &project,
        &mut index,
        "components/card/card.wxml",
        &StyleConfig::default(),
    );
    assert_eq!(component.isolation, StyleIsolation::Unknown);
    assert!(!component.complete);
    assert!(component.class_names().contains(&"app-global"));
}

#[test]
fn page_isolation_and_unowned_templates_are_not_reported_complete() {
    let project = fixture();
    project.write(
        "pages/index/index.json",
        r#"{"styleIsolation":"page-isolated"}"#,
    );
    project.write("orphan.wxml", "<view/>");
    let project = open(&project);
    let mut index = StyleIndex::new();
    let page = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(!page.class_names().contains(&"app-global"));
    assert!(page.complete);
    assert_eq!(page.isolation, StyleIsolation::PageIsolated);

    let orphan = facts(&project, &mut index, "orphan.wxml", &StyleConfig::default());
    assert_eq!(orphan.scope, StyleScope::Unknown);
    assert!(!orphan.complete);
    assert!(orphan.reason.is_some());
}

fn resource_fixture() -> ResourceFixture {
    let project = TempProject::new();
    let outside = TempProject::new();
    project.write("src/app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write(
        "src/pages/index/index.wxml",
        "<image src=\"../../images/logo.png\" />",
    );
    project.write("src/pages/index/index.ts", "Page({})");
    project.write("src/images/logo.png", "png");
    project.write("src/images/my icon.png", "png");
    project.write("src/images/icons/a.png", "png");
    project.write("src/data/a.json", "{}");
    project.write("outside/secret.png", "secret");
    std::fs::create_dir_all(project.root.join("outside")).expect("outside");
    outside.write("secret.png", "secret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(
            outside.root.join("secret.png"),
            project.root.join("src/images/link.png"),
        )
        .expect("symlink file");
        symlink(&outside.root, project.root.join("src/images/outside-dir")).expect("symlink dir");
        project.write("shared/logo.png", "png");
        symlink(
            project.root.join("shared/logo.png"),
            project.root.join("src/images/project-link.png"),
        )
        .expect("symlink outside app root");
    }
    let project_path = ProjectPath::new("src/app.json").expect("app path");
    let open = Project::open_with_app_json(&project.root, project_path).expect("open nested app");
    ResourceFixture {
        project_root: project,
        _outside: outside,
        project: open,
    }
}

#[test]
fn resources_resolve_relative_root_relative_and_file_uris() {
    let fixture = resource_fixture();
    let project = &fixture.project;
    let wxml = path("src/pages/index/index.wxml");
    let relative = project.resolve_resource(&wxml, "../../images/logo.png");
    assert_eq!(
        relative,
        ResourceResolution::File(path("src/images/logo.png"))
    );
    assert_eq!(
        project.resolve_resource(&wxml, "/images/logo.png"),
        ResourceResolution::File(path("src/images/logo.png"))
    );
    assert_eq!(
        project.resolve_resource(&wxml, "../../images"),
        ResourceResolution::Directory(path("src/images"))
    );
    assert_eq!(
        project.resolve_resource(&wxml, "../../images/missing.png"),
        ResourceResolution::Missing
    );
    assert_eq!(
        project.resolve_resource(&wxml, "../../../outside/secret.png"),
        ResourceResolution::OutsideRoot
    );
    for remote in [
        "https://example.com/a.png",
        "http://example.com/a.png",
        "//cdn.example.com/a.png",
        "data:image/png;base64,AAAA",
        "wxfile://tmp/a.png",
        "cloud://env/a.png",
    ] {
        assert_eq!(
            project.resolve_resource(&wxml, remote),
            ResourceResolution::Remote,
            "{remote}"
        );
    }
    assert_eq!(
        project.resolve_resource(&wxml, "https://example.com/a.png?x=1"),
        ResourceResolution::Remote
    );
    assert_eq!(
        project.resolve_resource(&wxml, "{{image}}"),
        ResourceResolution::Dynamic
    );
    assert_eq!(
        project.resolve_resource(&wxml, ""),
        ResourceResolution::Invalid
    );

    let spaced = path("src/images/my icon.png");
    let uri = project.uri_for(&spaced).expect("uri");
    assert!(uri.contains("%20"), "{uri}");
    assert_eq!(
        project.resolve_resource(&wxml, &uri),
        ResourceResolution::File(spaced)
    );
}

#[cfg(unix)]
#[test]
fn resource_symlinks_that_leave_the_root_are_not_targets() {
    let fixture = resource_fixture();
    let project = &fixture.project;
    let wxml = path("src/pages/index/index.wxml");
    assert_eq!(
        project.resolve_resource(&wxml, "../../images/link.png"),
        ResourceResolution::OutsideRoot
    );
    assert_eq!(
        project.resolve_resource(&wxml, "../../images/project-link.png"),
        ResourceResolution::OutsideRoot,
        "a symlink may not leave the Mini Program root even if it stays in the workspace"
    );
}

#[test]
fn resource_completion_lists_one_directory_and_preserves_prefixes() {
    let fixture = resource_fixture();
    let project = &fixture.project;
    let wxml = path("src/pages/index/index.wxml");

    let images = project.resource_candidates(&wxml, "../../images/");
    assert!(!images.truncated);
    let labels: Vec<_> = images
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert!(labels.contains(&"logo.png"), "{labels:?}");
    assert!(labels.contains(&"icons/"), "{labels:?}");
    assert!(labels.contains(&"my icon.png"), "{labels:?}");
    assert!(
        !labels.contains(&"link.png"),
        "symlink escape must be omitted"
    );
    assert_eq!(images.items[0].kind, ResourceKind::Directory);
    assert_eq!(images.items[0].insert, "../../images/icons/");

    let filtered = project.resource_candidates(&wxml, "../../images/lo");
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].insert, "../../images/logo.png");

    let root_relative = project.resource_candidates(&wxml, "/images/lo");
    assert_eq!(root_relative.items.len(), 1);
    assert_eq!(root_relative.items[0].insert, "/images/logo.png");

    assert!(
        project
            .resource_candidates(&wxml, "../../../outside/")
            .items
            .is_empty()
    );
    assert!(
        project
            .resource_candidates(&wxml, "https://")
            .items
            .is_empty()
    );
    assert!(
        project
            .resource_candidates(&wxml, "{{dir}}/")
            .items
            .is_empty()
    );
}

#[test]
fn percent_encoded_relative_resources_and_root_escape_are_consistent() {
    let fixture = resource_fixture();
    let project = &fixture.project;
    let wxml = path("src/pages/index/index.wxml");
    assert_eq!(
        project.resolve_resource(&wxml, "../../images/my%20icon.png"),
        ResourceResolution::File(path("src/images/my icon.png"))
    );
    assert_eq!(
        project.resolve_resource(&wxml, "../../images/%2e%2e/%2e%2e/../outside/secret.png"),
        ResourceResolution::OutsideRoot
    );
    assert_eq!(
        project.resolve_resource(&wxml, "../../images/%2.png"),
        ResourceResolution::Invalid
    );
    assert!(
        project
            .resource_candidates(&wxml, "/../images/")
            .items
            .is_empty(),
        "/.. must not list outside the app root"
    );
}

#[test]
fn resource_completion_skips_excluded_directories_and_bounds_results() {
    let fixture = resource_fixture();
    let project = &fixture.project;
    let wxml = path("src/pages/index/index.wxml");
    fixture.project_root.write("src/node_modules/x.png", "png");
    fixture.project_root.write("src/.git/y.png", "png");
    assert_eq!(
        project.resolve_resource(&wxml, "/node_modules/x.png"),
        ResourceResolution::OutsideRoot
    );
    for index in 0..300 {
        fixture
            .project_root
            .write(&format!("src/many/f{index}.png"), "png");
    }
    assert!(
        project
            .resource_candidates(&wxml, "/node_modules/")
            .items
            .is_empty()
    );
    assert!(
        project
            .resource_candidates(&wxml, "/.git/")
            .items
            .is_empty()
    );
    let many = project.resource_candidates(&wxml, "/many/");
    assert!(many.truncated);
    assert!(many.items.len() <= wxml_project::resources::MAX_RESOURCE_CANDIDATES);
    let encoded = project.resource_candidates(&wxml, "../../images/my%20");
    assert_eq!(encoded.items.len(), 1);
    assert_eq!(encoded.items[0].insert, "../../images/my icon.png");
    assert_eq!(
        project.resolve_resource(&wxml, &encoded.items[0].insert),
        ResourceResolution::File(path("src/images/my icon.png"))
    );
}

#[test]
fn resource_diagnostics_skip_remote_dynamic_and_directories() {
    let fixture = resource_fixture();
    let project = &fixture.project;
    let wxml = path("src/pages/index/index.wxml");
    let used = vec![
        "../../images/logo.png".to_owned(),
        "../../images/missing.png".to_owned(),
        "https://example.com/a.png".to_owned(),
        "{{dynamic}}".to_owned(),
        "../../images".to_owned(),
    ];
    assert_eq!(
        StyleDiagnosticsPolicy::default().missing_resources(project, &wxml, &used),
        vec!["../../images/missing.png".to_owned()],
        "resource diagnostics are on by default"
    );
    assert!(
        StyleDiagnosticsPolicy {
            resources: false,
            classes: false,
        }
        .missing_resources(project, &wxml, &used)
        .is_empty()
    );
    assert_eq!(
        StyleDiagnosticsPolicy::enabled().missing_resources(project, &wxml, &used),
        vec!["../../images/missing.png".to_owned()]
    );
}

#[test]
fn style_sheet_cache_counts_structure_and_invalidates() {
    let project = fixture();
    let project = open(&project);
    let path = path("shared/card.scss");
    let mut index = StyleIndex::new();
    let sheet = index.sheet(&project.snapshot(), &path);
    assert!(sheet.estimated_bytes > sheet.bytes);
    assert!(index.cached_estimated_bytes() >= sheet.estimated_bytes);
    assert_eq!(index.cached_sheet_count(), 1);
    assert!(index.invalidate(&path));
    assert_eq!(index.cached_sheet_count(), 0);
    assert_eq!(index.cached_estimated_bytes(), 0);
}

#[test]
fn scss_imports_prefer_scss_partials_over_wxss_siblings() {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.wxml", "<view/>");
    project.write("pages/index/index.ts", "Page({})");
    project.write(
        "pages/index/index.scss",
        "@import \"../../shared/theme\";\n.local { }\n",
    );
    project.write("shared/theme.wxss", ".from-wxss { }\n");
    project.write("shared/_theme.scss", ".from-scss { }\n");
    let project = open(&project);
    let mut index = StyleIndex::new();
    let facts = facts(
        &project,
        &mut index,
        "pages/index/index.wxml",
        &StyleConfig::default(),
    );
    assert!(facts.primary("from-scss").is_some(), "{:?}", facts.classes);
    assert!(facts.primary("from-wxss").is_none(), "{:?}", facts.classes);
    assert_eq!(
        facts
            .primary("from-scss")
            .expect("scss class")
            .file
            .as_str(),
        "shared/_theme.scss"
    );
}
