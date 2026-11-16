#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

//! Wave 5 WT-B acceptance: AC-005/006/008/012 fixture runner.
//!
//! The fixtures are opened in place (no copying), one `ProjectState` per
//! fixture root, and queried through the public [`NodeBackend`] boundary with
//! the same `RequestContext` construction as `real_node.rs`.
//!
//! ```bash
//! (cd backends/typescript && npm ci && npm run build)
//! WXML_TEST_NODE=1 cargo test -p wxml-lsp --test wave5_acceptance -- --ignored
//! ```
//!
//! Every test is `#[ignore]`: `--ignored` is the explicit real-Node gate, so a
//! missing `WXML_TEST_NODE=1` or a missing backend is a hard failure, not a
//! green skip. The orchestrate agent runs these after the WT-B implementation
//! lands.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig};
use wxml_lsp::project_host::{OpenDocument, ProjectHost, ProjectState};
use wxml_project::ProjectPath;
use wxml_types::{
    Backend, BackendEpoch, BackendId, DefinitionResult, DiagnosticSeverity, DiagnosticsRequest,
    DiagnosticsResult, HoverRequest, HoverResult, RequestContext, SnapshotId, SourceRevision,
    SourceSnapshot, TypeQuality,
};

mod common;

/// Text of the removed C1.1 locals degradation; it must never appear again.
const LOCALS_DEGRADATION: [&str; 2] = [
    "template locals are bound as any",
    "loop item/index member checks deferred",
];

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Expected fixture JSON (subset of the fields the runner asserts).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct HoverFile {
    queries: Vec<HoverQuery>,
}

#[derive(Debug, Deserialize)]
struct HoverQuery {
    file: String,
    marker: String,
    target: String,
    expect: HoverExpect,
}

#[derive(Debug, Deserialize)]
struct HoverExpect {
    #[serde(rename = "type")]
    type_text: String,
    #[serde(default)]
    documentation: String,
}

#[derive(Debug, Deserialize)]
struct DiagnosticsFile {
    diagnostics: Vec<DiagnosticExpect>,
}

#[derive(Debug, Deserialize)]
struct DiagnosticExpect {
    file: String,
    marker: String,
    target: String,
    severity: String,
    #[serde(rename = "messageContains")]
    message_contains: String,
}

#[derive(Debug, Deserialize)]
struct DefinitionFile {
    queries: Vec<DefinitionQuery>,
}

#[derive(Debug, Deserialize)]
struct DefinitionQuery {
    marker: String,
}

// ---------------------------------------------------------------------------
// Backend path resolution (same contract as `real_node.rs`).
// ---------------------------------------------------------------------------

struct BackendPaths {
    server: PathBuf,
    backend_dir: PathBuf,
}

fn backend_paths() -> BackendPaths {
    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the Wave 5 acceptance gate"
    );
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf();
    // The gate must exercise *this* worktree's backend. A globally exported
    // `WXML_TYPE_BACKEND` often points at another checkout (for example the
    // main workspace), which would silently validate stale code, so refuse it.
    let inside_repo = |path: &Path| -> bool {
        let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        resolved.starts_with(&repo)
    };
    let server = std::env::var_os("WXML_TYPE_BACKEND").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    assert!(
        inside_repo(&server),
        "WXML_TYPE_BACKEND={} is outside this worktree ({}); unset it so the gate tests this checkout",
        server.display(),
        repo.display()
    );
    let backend_dir = std::env::var_os("WXML_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);
    assert!(
        inside_repo(&backend_dir),
        "WXML_BACKEND_DIR={} is outside this worktree ({}); unset it so the gate tests this checkout",
        backend_dir.display(),
        repo.display()
    );
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
    BackendPaths {
        server,
        backend_dir,
    }
}

fn backend_config(paths: &BackendPaths) -> NodeBackendConfig {
    NodeBackendConfig {
        server: Some(paths.server.clone()),
        backend_dir: Some(paths.backend_dir.clone()),
        timeout: REQUEST_TIMEOUT,
        ..NodeBackendConfig::default()
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures")
        .canonicalize()
        .expect("tests/fixtures root")
}

fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

// ---------------------------------------------------------------------------
// One fixture project opened in place.
// ---------------------------------------------------------------------------

struct FixtureProject {
    root: PathBuf,
    host: Arc<ProjectState>,
    backend: NodeBackend,
}

impl FixtureProject {
    fn open(name: &str, paths: &BackendPaths) -> Self {
        let root = fixtures_root().join(name);
        assert!(
            root.join("app.json").is_file(),
            "fixture {name} is missing app.json at {}",
            root.display()
        );
        let host = Arc::new(ProjectState::new());
        host.open_root(Some(&path_to_uri(&root)))
            .unwrap_or_else(|error| panic!("open fixture {name}: {error}"));
        assert!(
            host.root().is_some(),
            "fixture {name} did not open as a project root"
        );
        let backend_host: Arc<dyn ProjectHost> = host.clone();
        let backend = NodeBackend::new(backend_config(paths), backend_host);
        Self {
            root,
            host,
            backend,
        }
    }

    fn text(&self, relative: &str) -> String {
        let path = self.root.join(relative);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }

    /// Builds the same request context `real_node.rs` uses: a snapshot that
    /// owns the file's real text with revision 1 and the view's revision.
    fn context_for(&self, relative: &str, text: &str) -> RequestContext {
        let path = ProjectPath::new(relative).expect("project-relative path");
        let file = self.host.file_id(&path).expect("file id");
        let target = self.host.view_for(&path).expect("discovered view");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            text,
        ));
        RequestContext::new(
            self.host.project_revision(),
            target.view_revision,
            BackendEpoch::new(BackendId::new("node"), 1),
            snapshot,
        )
        .expect("matching project identities")
    }

    fn hover(&self, context: &RequestContext, offset: u32) -> HoverResult {
        let hover = self
            .backend
            .hover(HoverRequest {
                context: context.clone(),
                offset,
            })
            .expect("real Node hover");
        assert_no_locals_reason(&hover.contents, "hover contents");
        if let Some(reason) = hover.status.reason() {
            assert_no_locals_reason(reason, "hover status reason");
        }
        hover
    }

    fn diagnostics(&self, context: &RequestContext) -> DiagnosticsResult {
        let diagnostics = self
            .backend
            .diagnostics(DiagnosticsRequest {
                context: context.clone(),
            })
            .expect("real Node diagnostics");
        if let Some(reason) = diagnostics.status.reason() {
            assert_no_locals_reason(reason, "diagnostics status reason");
        }
        for diagnostic in &diagnostics.diagnostics {
            assert_no_locals_reason(&diagnostic.message, "diagnostic message");
        }
        diagnostics
    }

    fn definition(&self, context: &RequestContext, offset: u32) -> DefinitionResult {
        let definition = self
            .backend
            .definition(wxml_types::DefinitionRequest {
                context: context.clone(),
                offset,
            })
            .expect("real Node definition");
        if let Some(reason) = definition.status.reason() {
            assert_no_locals_reason(reason, "definition status reason");
        }
        definition
    }

    /// Opens a script/JSON overlay without touching the working tree, so the
    /// multi-view isolation test models several simultaneously open documents.
    fn open_script(&self, relative: &str) {
        let text = self.text(relative);
        let path = ProjectPath::new(relative).expect("project-relative path");
        let file = self.host.file_id(&path).expect("file id");
        let uri = self.host.uri_for(&file).expect("project uri");
        self.host
            .open_document(OpenDocument {
                uri,
                text,
                version: Some(1),
            })
            .unwrap_or_else(|error| panic!("open overlay {relative}: {error}"));
    }
}

// ---------------------------------------------------------------------------
// Runner with a per-file diagnostics cache.
// ---------------------------------------------------------------------------

struct Runner {
    project: FixtureProject,
    diagnostics: RefCell<BTreeMap<String, DiagnosticsResult>>,
}

impl Runner {
    fn open(name: &str, paths: &BackendPaths) -> Self {
        Self {
            project: FixtureProject::open(name, paths),
            diagnostics: RefCell::new(BTreeMap::new()),
        }
    }

    fn root(&self) -> &Path {
        &self.project.root
    }

    fn diagnostics_for(&self, file: &str) -> DiagnosticsResult {
        if let Some(cached) = self.diagnostics.borrow().get(file) {
            return cached.clone();
        }
        let text = self.project.text(file);
        let context = self.project.context_for(file, &text);
        let result = self.project.diagnostics(&context);
        self.diagnostics
            .borrow_mut()
            .insert(file.to_owned(), result.clone());
        result
    }

    /// Asserts one hover query and that the same target is not an error.
    fn assert_hover(&self, query: &HoverQuery) -> HoverResult {
        let text = self.project.text(&query.file);
        let (start, end) = locate(&text, &query.marker, &query.target);
        let context = self.project.context_for(&query.file, &text);
        let hover = self.project.hover(&context, start);

        assert!(
            !hover.contents.is_empty(),
            "{}: empty hover for marker {:?}",
            query.file,
            query.marker
        );
        assert!(
            hover.contents.contains(&query.expect.type_text),
            "{}: hover for {:?} must contain type {:?}, got {:?}",
            query.file,
            query.target,
            query.expect.type_text,
            hover.contents
        );
        if query.expect.documentation.is_empty() {
            assert!(
                !hover.contents.contains("\n\n"),
                "{}: hover for {:?} must not carry documentation, got {:?}",
                query.file,
                query.target,
                hover.contents
            );
        } else {
            assert!(
                hover.contents.contains(&query.expect.documentation),
                "{}: hover for {:?} must contain documentation {:?}, got {:?}",
                query.file,
                query.target,
                query.expect.documentation,
                hover.contents
            );
        }
        assert_ne!(
            hover.quality,
            TypeQuality::Missing,
            "{}: hover for {:?} must not be Missing: {hover:?}",
            query.file,
            query.target
        );
        assert!(
            matches!(
                &hover.status,
                wxml_types::CapabilityStatus::Supported
                    | wxml_types::CapabilityStatus::Partial { .. }
            ),
            "{}: hover for {:?} must be Supported or Partial, got {:?}",
            query.file,
            query.target,
            hover.status
        );

        self.assert_no_error_at(&query.file, start, end);
        hover
    }

    /// Asserts the exact target range of one expected diagnostic.
    fn assert_diagnostic(&self, expected: &DiagnosticExpect) {
        let text = self.project.text(&expected.file);
        let (start, end) = locate(&text, &expected.marker, &expected.target);
        let severity = severity(&expected.severity);
        let result = self.diagnostics_for(&expected.file);
        let found = result.diagnostics.iter().any(|diagnostic| {
            diagnostic.range.start() == start
                && diagnostic.range.end() == end
                && diagnostic.severity == severity
                && diagnostic.message.contains(&expected.message_contains)
        });
        assert!(
            found,
            "{}: expected a {} diagnostic at {start}..{end} (marker {:?}) containing {:?}; got {:?}",
            expected.file,
            expected.severity,
            expected.marker,
            expected.message_contains,
            result.diagnostics
        );
    }

    /// Positives must not be reported as an error at their own target range.
    fn assert_no_error_at(&self, file: &str, start: u32, end: u32) {
        let result = self.diagnostics_for(file);
        for diagnostic in &result.diagnostics {
            assert!(
                !(diagnostic.severity == DiagnosticSeverity::Error
                    && diagnostic.range.start() == start
                    && diagnostic.range.end() == end),
                "{file}: positive target {start}..{end} produced an error: {diagnostic:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Expected JSON loading.
// ---------------------------------------------------------------------------

fn load_hover(root: &Path) -> Vec<HoverQuery> {
    let path = root.join("expected/hover.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str::<HoverFile>(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
        .queries
}

fn load_diagnostics(root: &Path) -> Vec<DiagnosticExpect> {
    let path = root.join("expected/diagnostics.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str::<DiagnosticsFile>(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
        .diagnostics
}

fn load_definitions(root: &Path) -> Vec<DefinitionQuery> {
    let path = root.join("expected/definition.json");
    if !path.is_file() {
        return Vec::new();
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str::<DefinitionFile>(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
        .queries
}

// ---------------------------------------------------------------------------
// Shared assertions and byte-offset math.
// ---------------------------------------------------------------------------

fn assert_no_locals_reason(text: &str, context: &str) {
    for needle in LOCALS_DEGRADATION {
        assert!(
            !text.contains(needle),
            "{context}: removed locals degradation leaked ({needle:?}) in {text:?}"
        );
    }
}

fn severity(name: &str) -> DiagnosticSeverity {
    match name {
        "error" => DiagnosticSeverity::Error,
        "warning" => DiagnosticSeverity::Warning,
        "information" => DiagnosticSeverity::Information,
        "hint" => DiagnosticSeverity::Hint,
        other => panic!("unknown fixture severity {other:?}"),
    }
}

/// Returns the absolute UTF-8 byte range of `target` inside `marker`.
fn locate(text: &str, marker: &str, target: &str) -> (u32, u32) {
    let marker_start = unique_offset(text, marker);
    let target_start = marker_start + unique_offset(marker, target);
    let end = target_start + target.len();
    (
        u32::try_from(target_start).expect("target start fits in u32"),
        u32::try_from(end).expect("target end fits in u32"),
    )
}

fn unique_offset(haystack: &str, needle: &str) -> usize {
    let mut matches = haystack.match_indices(needle);
    let first = matches
        .next()
        .unwrap_or_else(|| panic!("{needle:?} not found in fixture text"))
        .0;
    assert!(
        matches.next().is_none(),
        "{needle:?} is not unique in fixture text"
    );
    first
}

/// Event-handler markers belong to WT-C; WT-B must not assert them.
fn is_event_marker(marker: &str) -> bool {
    ["bind", "catch", "capture", "mut-"]
        .iter()
        .any(|prefix| marker.starts_with(prefix))
}

fn query_for<'a>(queries: &'a [HoverQuery], file: &str, target: &str) -> &'a HoverQuery {
    queries
        .iter()
        .find(|query| query.file == file && query.target == target)
        .unwrap_or_else(|| panic!("no hover query for {file} target {target:?}"))
}

// ---------------------------------------------------------------------------
// AC-005: wx:for locals.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1"]
fn ac005_loop_scope_typed_locals() {
    let paths = backend_paths();
    let runner = Runner::open("loop-scope", &paths);

    let hovers = load_hover(runner.root());
    assert_eq!(hovers.len(), 8, "loop-scope hover query count changed");
    for query in &hovers {
        runner.assert_hover(query);
    }

    // `itemIndex` carries no documentation, so the hover must not grow a
    // second Markdown paragraph.
    let item_index = query_for(&hovers, "pages/index/index.wxml", "itemIndex");
    assert!(
        item_index.expect.documentation.is_empty(),
        "fixture itemIndex documentation changed"
    );
    let text = runner.project.text(&item_index.file);
    let (start, _) = locate(&text, &item_index.marker, &item_index.target);
    let context = runner.project.context_for(&item_index.file, &text);
    let hover = runner.project.hover(&context, start);
    assert!(
        !hover.contents.contains("\n\n"),
        "itemIndex hover must not include documentation: {:?}",
        hover.contents
    );

    let diagnostics = load_diagnostics(runner.root());
    assert_eq!(diagnostics.len(), 1, "loop-scope negative count changed");
    for expected in &diagnostics {
        runner.assert_diagnostic(expected);
    }

    // The only loop-scope negative is `{{ item }}` outside the loop.
    let outside = &diagnostics[0];
    assert_eq!(outside.target, "item");
    assert_eq!(outside.severity, "error");

    // Removing the C1.1 locals degradation must not replace it with a blanket
    // Partial: typed locals are a supported check now.
    let status = runner.diagnostics_for("pages/index/index.wxml").status;
    assert!(
        status.is_supported(),
        "loop-scope diagnostics must stay Supported, got {status:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-006: wx:if/elif/else narrowing.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1"]
fn ac006_conditional_union_narrowing() {
    let paths = backend_paths();
    let runner = Runner::open("conditional-union", &paths);

    let hovers = load_hover(runner.root());
    assert_eq!(
        hovers.len(),
        3,
        "conditional-union hover query count changed"
    );
    for query in &hovers {
        runner.assert_hover(query);
    }

    let diagnostics = load_diagnostics(runner.root());
    assert_eq!(
        diagnostics.len(),
        2,
        "conditional-union negative count changed"
    );
    for expected in &diagnostics {
        runner.assert_diagnostic(expected);
    }

    // `state.user.age` is a discriminator-union miss; the outside-guard
    // `maybeUser` access is a possibly-null error.
    assert!(diagnostics.iter().any(|d| d.target == "age"));
    let maybe_user = diagnostics
        .iter()
        .find(|d| d.target == "maybeUser")
        .expect("outside-guard null diagnostic");
    assert_eq!(maybe_user.message_contains, "null");

    // Guard narrowing is a supported check; the fixture must not fall back to
    // a Partial blanket reason.
    let status = runner.diagnostics_for("pages/index/index.wxml").status;
    assert!(
        status.is_supported(),
        "conditional-union diagnostics must stay Supported, got {status:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-012: custom component property bindings.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1"]
fn ac012_component_property_bindings() {
    let paths = backend_paths();
    let runner = Runner::open("component-bindings", &paths);

    let hovers = load_hover(runner.root());
    assert_eq!(hovers.len(), 2, "component-bindings hover count changed");
    for query in &hovers {
        runner.assert_hover(query);
    }

    // WT-C owns the event-handler markers in this shared fixture; only the
    // property-binding mismatch is WT-B's assertion.
    let property_diagnostics = load_diagnostics(runner.root())
        .into_iter()
        .filter(|expected| !is_event_marker(&expected.marker))
        .collect::<Vec<_>>();
    assert_eq!(
        property_diagnostics.len(),
        1,
        "component-bindings property diagnostic count changed"
    );
    for expected in &property_diagnostics {
        runner.assert_diagnostic(expected);
        assert_eq!(expected.target, "total");
        assert!(expected.message_contains.contains("number"));
    }

    let property_definitions = load_definitions(runner.root())
        .into_iter()
        .filter(|query| !is_event_marker(&query.marker))
        .collect::<Vec<_>>();
    assert!(
        property_definitions.is_empty(),
        "component-bindings gained a property definition expectation; extend this runner: \
         {property_definitions:?}"
    );
}

// ---------------------------------------------------------------------------
// AC-008: same-script multi-view isolation.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1"]
fn ac008_multi_view_fixture_isolation() {
    let paths = backend_paths();
    let runner = Runner::open("multi-view", &paths);

    let hovers = load_hover(runner.root());
    assert_eq!(hovers.len(), 7, "multi-view hover query count changed");
    for query in &hovers {
        runner.assert_hover(query);
    }

    let diagnostics = load_diagnostics(runner.root());
    assert_eq!(diagnostics.len(), 1, "multi-view negative count changed");
    for expected in &diagnostics {
        runner.assert_diagnostic(expected);
    }

    // The `user.nmae` typo lives only in card.wxml; pages/a and pages/b must
    // stay clean at their own binding targets (already checked by
    // `assert_hover`) and must not contain the typo message at all.
    for file in ["pages/a/a.wxml", "pages/b/b.wxml"] {
        let result = runner.diagnostics_for(file);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.message.contains("nmae")),
            "{file}: card typo leaked into another view: {:?}",
            result.diagnostics
        );
    }
}

#[test]
#[ignore = "requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1"]
fn ac008_simultaneous_views_do_not_pollute_each_other() {
    let paths = backend_paths();
    let runner = Runner::open("multi-view", &paths);

    // Model the editor state: two scripts open next to three templates.
    runner.project.open_script("pages/a/a.ts");
    runner.project.open_script("pages/b/b.ts");

    let hovers = load_hover(runner.root());
    let a_badge = query_for(&hovers, "pages/a/a.wxml", "aBadge");
    let a_title = query_for(&hovers, "pages/a/a.wxml", "aTitle");
    let b_badge = query_for(&hovers, "pages/b/b.wxml", "bBadge");
    let b_title = query_for(&hovers, "pages/b/b.wxml", "bTitle");
    let card_title = query_for(&hovers, "components/card/card.wxml", "title");
    let card_user = query_for(&hovers, "components/card/card.wxml", "user.name");

    // Interleave the views; a stale view cache would swap the types or the
    // JSDoc here.
    let a_badge_hover = runner.assert_hover(a_badge);
    let card_title_hover = runner.assert_hover(card_title);
    let b_badge_hover = runner.assert_hover(b_badge);
    let card_user_hover = runner.assert_hover(card_user);
    let a_title_hover = runner.assert_hover(a_title);
    let b_title_hover = runner.assert_hover(b_title);

    assert!(a_badge_hover.contents.contains("number"));
    assert!(b_badge_hover.contents.contains("number"));
    assert!(card_title_hover.contents.contains("string"));
    assert!(card_user_hover.contents.contains("string"));
    assert!(a_title_hover.contents.contains("A 页面标题"));
    assert!(b_title_hover.contents.contains("B 页面标题"));
    assert!(
        !a_title_hover.contents.contains("B 页面标题"),
        "page A hover picked up page B documentation: {:?}",
        a_title_hover.contents
    );
    assert!(
        !b_title_hover.contents.contains("A 页面标题"),
        "page B hover picked up page A documentation: {:?}",
        b_title_hover.contents
    );
    assert!(
        !card_title_hover.contents.contains("页面标题"),
        "card hover picked up a page's documentation: {:?}",
        card_title_hover.contents
    );

    let card_diagnostics = runner.diagnostics_for("components/card/card.wxml");
    assert!(
        card_diagnostics
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("nmae")),
        "card typo must stay in card.wxml: {:?}",
        card_diagnostics.diagnostics
    );
    for file in ["pages/a/a.wxml", "pages/b/b.wxml"] {
        let result = runner.diagnostics_for(file);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.message.contains("nmae")),
            "{file}: card typo leaked across views: {:?}",
            result.diagnostics
        );
        assert!(
            result
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != DiagnosticSeverity::Error),
            "{file}: a clean page produced an error: {:?}",
            result.diagnostics
        );
    }
}

// ---------------------------------------------------------------------------
// AC-008: tsconfig `extends` + path alias.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Node 24 and a built dist/server.js; set WXML_TEST_NODE=1"]
fn ac008_alias_project_hover_and_definition() {
    let paths = backend_paths();
    let runner = Runner::open("alias-project", &paths);

    let hovers = load_hover(runner.root());
    assert_eq!(hovers.len(), 1, "alias-project hover count changed");
    for query in &hovers {
        runner.assert_hover(query);
    }

    let diagnostics = load_diagnostics(runner.root());
    assert_eq!(diagnostics.len(), 1, "alias-project negative count changed");
    for expected in &diagnostics {
        runner.assert_diagnostic(expected);
    }

    // Definition of `name` inside `{{ user.name }}` must cross the
    // `@models/*` alias back to the real model file.
    let file = "pages/index/index.wxml";
    let text = runner.project.text(file);
    let (start, _) = locate(&text, "{{ user.name }}", "user.name");
    let name_offset = start + u32::try_from("user.".len()).expect("name offset fits");
    let context = runner.project.context_for(file, &text);
    let definition = runner.project.definition(&context, name_offset);
    let model_text = runner.project.text("models/user.ts");
    let location = definition
        .locations
        .iter()
        .find(|location| location.file.path() == "models/user.ts")
        .unwrap_or_else(|| {
            panic!(
                "`@models/user` definition did not resolve to models/user.ts: {:?}",
                definition.locations
            )
        });
    let range_start = usize::try_from(location.range.start()).expect("range start fits");
    let range_end = usize::try_from(location.range.end()).expect("range end fits");
    assert!(
        range_start < range_end && range_end <= model_text.len(),
        "definition range {range_start}..{range_end} is not inside models/user.ts ({} bytes)",
        model_text.len()
    );
    let covered = &model_text[range_start..range_end];
    assert!(
        covered.contains("name"),
        "definition range must cover the `name` property, got {covered:?}"
    );
}
