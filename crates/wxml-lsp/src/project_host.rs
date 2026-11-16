//! Project-aware document routing and analysis inputs for the LSP server.
//!
//! The protocol layer owns UTF-16 positions and LSP messages; this module owns
//! the bridge to the project layer (P3) and the template analyzer (P6). It
//! resolves a client URI to a real [`ProjectPath`]/[`FileId`], tracks unsaved
//! script and JSON overlays, stores WXML snapshots, and produces the
//! [`PreparedView`] that a type backend needs.
//!
//! A server may run without a project (no `rootUri`, an invalid root, or a
//! document outside the root). In that case [`NullHost`] or a
//! [`ProjectState`] without a project keeps the server alive and returns
//! explicit [`HostError::Unsupported`] reasons instead of guessing a project
//! or a view.

// Read/write guards intentionally cover the whole operation so the state and
// the project cannot be swapped between a check and its use.
#![allow(clippy::significant_drop_tightening)]

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use thiserror::Error;
use wxml_project::{
    ComponentPropertyFacts, ComponentTargetKind, Page, Project, ProjectPath, SourceFile,
    SourceKind, SourceLimits, UriError,
};
use wxml_syntax::{ParseErrorKind, ParseResult};
use wxml_types::{
    CapabilityStatus, CheckFragment, ComponentUsage, Diagnostic, DiagnosticSeverity, EventHandler,
    FileId, ProjectId, ProjectRevision, RootKind, SourceSnapshot, TemplateAnalyzer,
    UnsupportedKind, ViewRevision,
};

/// Marker used by the synthetic host and the synthetic workspace project.
pub const SYNTHETIC_PROJECT: &str = "lsp";

/// Compiler options that the C1 Node backend accepts.
///
/// This mirrors `REQUEST_COMPILER_OPTION_KEYS` in
/// `backends/typescript/src/protocol.ts`. Unknown keys are filtered here and
/// reported as a `Partial` hint instead of being sent to Node, which rejects
/// unknown options.
const SUPPORTED_COMPILER_OPTIONS: [&str; 33] = [
    "strict",
    "strictNullChecks",
    "strictFunctionTypes",
    "strictBindCallApply",
    "strictPropertyInitialization",
    "noImplicitAny",
    "noImplicitThis",
    "noUncheckedIndexedAccess",
    "exactOptionalPropertyTypes",
    "strictBuiltinIteratorReturn",
    "useUnknownInCatchVariables",
    "allowJs",
    "checkJs",
    "esModuleInterop",
    "allowSyntheticDefaultImports",
    "resolveJsonModule",
    "isolatedModules",
    "verbatimModuleSyntax",
    "forceConsistentCasingInFileNames",
    "skipLibCheck",
    "target",
    "module",
    "moduleResolution",
    "jsx",
    "moduleDetection",
    "lib",
    "baseUrl",
    "paths",
    "types",
    "typeRoots",
    "rootDir",
    "experimentalDecorators",
    "outDir",
];

/// Where a custom component tag is registered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentOrigin {
    /// Global `app.json` `usingComponents`.
    App {
        /// Project-relative `app.json` path.
        file: ProjectPath,
    },
    /// Page- or component-local `usingComponents`.
    Local {
        /// Project-relative page/component JSON path, when it exists.
        file: Option<ProjectPath>,
    },
}

impl ComponentOrigin {
    /// Returns the project-relative declaration file, if any.
    #[must_use]
    pub const fn file(&self) -> Option<&ProjectPath> {
        match self {
            Self::App { file } => Some(file),
            Self::Local { file } => file.as_ref(),
        }
    }
}

/// One custom component registered by the current view's `usingComponents`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewComponent {
    /// Tag name used in the template (for example `my-card`).
    pub name: String,
    /// Raw `usingComponents` specifier.
    pub specifier: String,
    /// Form of the specifier (local, plugin or package).
    pub kind: ComponentTargetKind,
    /// Resolved component base path, when the specifier is local.
    pub component: Option<ProjectPath>,
    /// Existing component WXML, when discovery found the local target.
    pub wxml: Option<ProjectPath>,
    /// Existing component JSON, when discovery found the local target.
    pub json: Option<ProjectPath>,
    /// Statically declared component properties and their coverage.
    pub property_facts: ComponentPropertyFacts,
    /// Page/component-local or global registration source.
    pub origin: ComponentOrigin,
}

impl ViewComponent {
    /// Applies live property facts without erasing a known discovery list.
    ///
    /// An incomplete, empty extraction keeps the previously known declarations
    /// and records the reason; a complete result, or any result that found at
    /// least one property, replaces the snapshot.
    pub(crate) fn apply_live_facts(&mut self, facts: ComponentPropertyFacts) {
        if !facts.complete
            && facts.properties.is_empty()
            && !self.property_facts.properties.is_empty()
        {
            self.property_facts.complete = false;
            self.property_facts.reason = facts.reason;
            return;
        }
        self.property_facts = facts;
    }
}

/// Why a host operation could not be completed.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum HostError {
    /// No project root is open.
    #[error("no project root is open")]
    NoProject,
    /// The URI is not a valid local file URI inside the project root.
    #[error("document URI is not inside the project")]
    InvalidUri,
    /// The project root could not be opened.
    #[error("project root could not be opened")]
    ProjectOpen,
    /// The document is not a discovered page or component, or has no script.
    #[error("{reason}")]
    Unsupported {
        /// Short, path-free explanation.
        reason: &'static str,
    },
    /// A project file could not be read.
    #[error("project file could not be read")]
    Read,
}

/// How a URI is routed by the LSP server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentRoute {
    /// A WXML template in the project.
    Wxml(ProjectPath),
    /// A script in the project.
    Script(ProjectPath),
    /// A project JSON file (`app.json`, page/component JSON).
    Json(ProjectPath),
    /// Not a project document (no project, outside the root, unknown extension).
    Unknown,
}

/// The template and script behind one discovered WXML file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewTarget {
    /// WXML template path.
    pub wxml: ProjectPath,
    /// Page or component registration root.
    pub owner: RootKind,
    /// Script path.
    pub script: ProjectPath,
    /// Script file identity.
    pub script_file: FileId,
    /// Revision of the view (template + script + project context).
    pub view_revision: ViewRevision,
}

/// One document opened by the client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenDocument {
    /// Client URI.
    pub uri: String,
    /// Effective full text.
    pub text: String,
    /// Client document version, when supplied.
    pub version: Option<u64>,
}

/// One open document change. The LSP layer has already applied the edits and
/// passes the resulting full text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeDocument {
    /// Client URI.
    pub uri: String,
    /// Effective full text after the change.
    pub text: String,
    /// Client document version.
    pub version: u64,
}

/// What changed after a document/overlay update.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostUpdate {
    /// Views that must be re-checked.
    pub dirty_views: Vec<ProjectPath>,
    /// New project revision, when the update changed project state.
    pub project_revision: Option<ProjectRevision>,
}

/// One `workspace/didChangeWatchedFiles` entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchedFileChange {
    /// Client URI of the changed file.
    pub uri: String,
    /// What happened to the file.
    pub kind: WatchedChangeKind,
}

/// LSP `FileChangeType` values understood by this server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchedChangeKind {
    /// The file was created (`1`).
    Created,
    /// The file changed (`2`).
    Changed,
    /// The file was deleted (`3`).
    Deleted,
}

impl WatchedChangeKind {
    /// Maps the numeric LSP `FileChangeType` to a kind.
    #[must_use]
    pub const fn from_lsp(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Created),
            2 => Some(Self::Changed),
            3 => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// One source file sent to the type backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedFile {
    /// File identity bound to the project.
    pub file: FileId,
    /// Project-relative path.
    pub path: ProjectPath,
    /// C1 script kind (`ts`, `tsx`, `js`, `jsx`, `d.ts`).
    pub kind: &'static str,
    /// Effective text (overlay first).
    pub text: Arc<str>,
    /// Revision of this file's text.
    pub source_revision: u64,
}

/// Maximum static event handlers forwarded to a type backend for one view.
///
/// Mirrors the C1 event budget: a large template must not turn into an
/// unbounded request or diagnostics list. Truncation is reported as a
/// [`CapabilityStatus::Partial`] hint.
pub const MAX_PREPARED_EVENTS: usize = 1024;

/// Everything a type backend needs for one WXML snapshot.
#[derive(Clone, Debug)]
pub struct PreparedView {
    /// Stable view identity (`<wxml>:<viewRevision>`).
    pub view_id: String,
    /// Discovered view, absent when the template has no known owner/script.
    pub target: Option<ViewTarget>,
    /// Script files in the project, including the owning script.
    pub files: Arc<[PreparedFile]>,
    /// Filtered effective compiler options.
    pub compiler_options: serde_json::Map<String, serde_json::Value>,
    /// Template fragments in source order.
    pub fragments: Arc<[CheckFragment]>,
    /// Custom-component usages with resolved local component scripts.
    pub components: Arc<[ComponentUsage]>,
    /// Static event handlers in source order, capped at [`MAX_PREPARED_EVENTS`].
    pub events: Arc<[EventHandler]>,
    /// Absolute project root. Never sent to an LSP client.
    pub project_root: PathBuf,
    /// Parser and template-analyzer diagnostics that must be published even
    /// when the type backend is unavailable.
    pub syntax_diagnostics: Vec<Diagnostic>,
    /// Degradation hint produced before asking the type backend.
    pub status_hint: Option<CapabilityStatus>,
}

/// Project state and analysis inputs for one workspace root.
pub trait ProjectHost: Send + Sync {
    /// Opens `root_uri` (or clears the project when it is `None`).
    fn open_root(&self, root_uri: Option<&str>) -> Result<(), HostError>;
    /// Returns the stable project identity.
    fn project_id(&self) -> ProjectId;
    /// Returns the current project revision.
    fn project_revision(&self) -> ProjectRevision;
    /// Returns whether a project root is currently loaded.
    fn is_project_open(&self) -> bool {
        false
    }
    /// Maps a client URI to a project-relative path.
    fn path_for_uri(&self, uri: &str) -> Result<ProjectPath, HostError>;
    /// Maps a file identity back to a client URI.
    fn uri_for(&self, file: &FileId) -> Option<String>;
    /// Creates the file identity for a project path.
    fn file_id(&self, path: &ProjectPath) -> Result<FileId, HostError>;
    /// Routes a client URI.
    fn route(&self, uri: &str) -> DocumentRoute;
    /// Returns the discovered view for a WXML path.
    fn view_for(&self, wxml: &ProjectPath) -> Result<ViewTarget, HostError>;
    /// Records an opened script/JSON document overlay.
    fn open_document(&self, open: OpenDocument) -> Result<HostUpdate, HostError>;
    /// Records a changed script/JSON document overlay.
    fn change_document(&self, change: ChangeDocument) -> Result<HostUpdate, HostError>;
    /// Removes an overlay when the client closes a document.
    fn close_document(&self, uri: &str) -> Result<HostUpdate, HostError>;
    /// Handles `textDocument/didSave` for a script or JSON document.
    fn save_document(&self, _uri: &str) -> Result<HostUpdate, HostError> {
        Ok(HostUpdate::default())
    }
    /// Handles `workspace/didChangeWatchedFiles`.
    fn watched_files_changed(
        &self,
        _changes: &[WatchedFileChange],
    ) -> Result<HostUpdate, HostError> {
        Ok(HostUpdate::default())
    }
    /// Stores the current WXML snapshot and advances its view revision.
    fn set_wxml_snapshot(&self, file: FileId, snapshot: Arc<SourceSnapshot>);
    /// Removes a WXML snapshot and advances its view revision.
    fn remove_wxml_snapshot(&self, file: &FileId);
    /// Returns a stored WXML snapshot.
    fn source(&self, file: &FileId) -> Option<Arc<SourceSnapshot>>;
    /// Returns effective text for any project file (overlay first, then disk).
    ///
    /// This is used to convert byte ranges in definition targets to LSP
    /// positions without opening the target document.
    fn file_text(&self, file: &FileId) -> Option<Arc<str>>;
    /// Returns the custom components registered for one WXML template.
    ///
    /// Local page/component `usingComponents` shadow global ones. A host with
    /// no project (or an unknown template) returns an empty list; static
    /// completion then simply omits custom component tags.
    fn view_components(&self, _wxml: &ProjectPath) -> Vec<ViewComponent> {
        Vec::new()
    }

    /// Returns current component properties for one registered tag.
    ///
    /// Unlike [`Self::view_components`] this reads the effective script text
    /// (overlay first), so an unsaved `properties` edit is reflected in static
    /// completion, hover and definition. The default returns nothing.
    fn component_properties(
        &self,
        _wxml: &ProjectPath,
        _tag: &str,
    ) -> Option<ComponentPropertyFacts> {
        None
    }

    /// Prepares the analysis inputs for a WXML snapshot.
    fn analysis(&self, snapshot: &SourceSnapshot) -> Result<PreparedView, HostError>;
}

/// A host with no project. Used by `run_stdio` and the skeleton tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullHost;

impl ProjectHost for NullHost {
    fn open_root(&self, _root_uri: Option<&str>) -> Result<(), HostError> {
        Ok(())
    }

    fn project_id(&self) -> ProjectId {
        ProjectId::new(SYNTHETIC_PROJECT)
    }

    fn project_revision(&self) -> ProjectRevision {
        ProjectRevision::new(ProjectId::new(SYNTHETIC_PROJECT), 1)
    }

    fn path_for_uri(&self, _uri: &str) -> Result<ProjectPath, HostError> {
        Err(HostError::NoProject)
    }

    fn uri_for(&self, _file: &FileId) -> Option<String> {
        None
    }

    fn file_id(&self, _path: &ProjectPath) -> Result<FileId, HostError> {
        Err(HostError::NoProject)
    }

    fn route(&self, _uri: &str) -> DocumentRoute {
        DocumentRoute::Unknown
    }

    fn view_for(&self, _wxml: &ProjectPath) -> Result<ViewTarget, HostError> {
        Err(HostError::NoProject)
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
        None
    }

    fn analysis(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Err(HostError::NoProject)
    }
}

#[derive(Debug)]
struct ViewState {
    owner: RootKind,
    script: Option<ProjectPath>,
    script_file: Option<FileId>,
    view_revision: ViewRevision,
    /// Local `usingComponents` entries: display tag to component base path.
    components: BTreeMap<String, ProjectPath>,
}

/// One Mini Program candidate and the TypeScript project that owns it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AppRoot {
    /// Directory containing `app.json`; used to pick a project from a URI.
    app_dir: PathBuf,
    /// Directory containing the nearest `tsconfig.json` (or `app_dir` when
    /// there is none). This is the backend `projectRoot`.
    project_root: PathBuf,
    /// `app.json` relative to `project_root`.
    app_json: ProjectPath,
}

#[derive(Debug)]
struct State {
    project: Option<Project>,
    /// Candidate Mini Program roots found below the workspace root.
    app_roots: Vec<AppRoot>,
    /// The candidate currently loaded, if any.
    active_app: Option<AppRoot>,
    project_id: ProjectId,
    project_revision: ProjectRevision,
    /// Monotonic revision sequence shared by every loaded app root.
    project_revision_seq: u64,
    views: BTreeMap<ProjectPath, ViewState>,
    snapshots: HashMap<FileId, Arc<SourceSnapshot>>,
    /// Unsaved script/JSON text keyed by client URI, replayed when an app root
    /// is loaded again after another root became active.
    overlays: BTreeMap<String, String>,
    /// Parsed component properties for a script overlay revision.
    property_cache: BTreeMap<ProjectPath, (u64, ComponentPropertyFacts)>,
    next_view_revision: u64,
    next_overlay_revision: u64,
}

/// A project root plus overlays, WXML snapshots and discovered views.
#[derive(Debug)]
pub struct ProjectState {
    state: RwLock<State>,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectState {
    /// Creates a host with no project open.
    #[must_use]
    pub fn new() -> Self {
        let project_id = ProjectId::new(SYNTHETIC_PROJECT);
        Self {
            state: RwLock::new(State {
                project: None,
                app_roots: Vec::new(),
                active_app: None,
                project_revision: ProjectRevision::new(project_id.clone(), 1),
                project_id,
                project_revision_seq: 1,
                views: BTreeMap::new(),
                snapshots: HashMap::new(),
                overlays: BTreeMap::new(),
                property_cache: BTreeMap::new(),
                next_view_revision: 1,
                next_overlay_revision: 1,
            }),
        }
    }

    /// Locks the state for reading, recovering poisoned data.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the state for writing, recovering poisoned data.
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a project root and rebuilds the view table.
    ///
    /// A failed open leaves the host without a project; the caller can keep
    /// serving synthetic documents.
    pub fn open_root(&self, root_uri: Option<&str>) -> Result<(), HostError> {
        let mut state = self.write();
        state.project = None;
        state.app_roots.clear();
        state.active_app = None;
        state.views.clear();
        state.snapshots.clear();
        state.overlays.clear();
        state.property_cache.clear();
        state.project_id = ProjectId::new(SYNTHETIC_PROJECT);
        state.project_revision = ProjectRevision::new(ProjectId::new(SYNTHETIC_PROJECT), 1);
        state.project_revision_seq = 1;
        let Some(uri) = root_uri else {
            return Ok(());
        };
        let root = wxml_project::root_path_from_uri(uri).map_err(|_| HostError::ProjectOpen)?;
        let roots = resolve_app_roots(root);
        if roots.is_empty() {
            return Err(HostError::ProjectOpen);
        }
        state.app_roots.clone_from(&roots);
        state.next_view_revision = 1;
        state.next_overlay_revision = 1;
        if roots.len() == 1 {
            let root = roots.into_iter().next().ok_or(HostError::ProjectOpen)?;
            load_project(&mut state, &root)?;
        }
        Ok(())
    }

    /// Returns the project root, if one is open.
    #[must_use]
    pub fn root(&self) -> Option<PathBuf> {
        self.read()
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
    }
}

impl ProjectHost for ProjectState {
    fn open_root(&self, root_uri: Option<&str>) -> Result<(), HostError> {
        Self::open_root(self, root_uri)
    }

    fn project_id(&self) -> ProjectId {
        self.read().project_id.clone()
    }

    fn project_revision(&self) -> ProjectRevision {
        self.read().project_revision.clone()
    }

    fn is_project_open(&self) -> bool {
        let state = self.read();
        state.project.is_some() || !state.app_roots.is_empty()
    }

    fn path_for_uri(&self, uri: &str) -> Result<ProjectPath, HostError> {
        let mut state = self.write();
        ensure_project_for_uri(&mut state, uri)?;
        let project = state.project.as_ref().ok_or(HostError::NoProject)?;
        project.path_from_uri(uri).map_err(map_uri_error)
    }

    fn uri_for(&self, file: &FileId) -> Option<String> {
        let state = self.read();
        let project = state.project.as_ref()?;
        let path = ProjectPath::new(file.path()).ok()?;
        project.uri_for(&path)
    }

    fn file_id(&self, path: &ProjectPath) -> Result<FileId, HostError> {
        let state = self.read();
        FileId::new(state.project_id.clone(), path.as_str()).map_err(|_| HostError::InvalidUri)
    }

    fn route(&self, uri: &str) -> DocumentRoute {
        let Ok(path) = self.path_for_uri(uri) else {
            return DocumentRoute::Unknown;
        };
        match path.extension().map(str::to_ascii_lowercase).as_deref() {
            Some("wxml") => DocumentRoute::Wxml(path),
            Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs" | "d.ts") => {
                DocumentRoute::Script(path)
            }
            Some("json") => DocumentRoute::Json(path),
            _ => DocumentRoute::Unknown,
        }
    }

    fn view_for(&self, wxml: &ProjectPath) -> Result<ViewTarget, HostError> {
        let state = self.read();
        let view = state.views.get(wxml).ok_or(HostError::Unsupported {
            reason: "template is not a discovered page or component",
        })?;
        let script = view.script.clone().ok_or(HostError::Unsupported {
            reason: "page or component has no script",
        })?;
        let script_file = view.script_file.clone().ok_or(HostError::Unsupported {
            reason: "page or component has no script file identity",
        })?;
        Ok(ViewTarget {
            wxml: wxml.clone(),
            owner: view.owner,
            script,
            script_file,
            view_revision: view.view_revision,
        })
    }

    fn open_document(&self, open: OpenDocument) -> Result<HostUpdate, HostError> {
        self.overlay_update(&open.uri, &open.text)
    }

    fn change_document(&self, change: ChangeDocument) -> Result<HostUpdate, HostError> {
        self.overlay_update(&change.uri, &change.text)
    }

    fn close_document(&self, uri: &str) -> Result<HostUpdate, HostError> {
        self.close_overlay(uri)
    }

    fn save_document(&self, uri: &str) -> Result<HostUpdate, HostError> {
        self.save_overlay(uri)
    }

    fn watched_files_changed(
        &self,
        changes: &[WatchedFileChange],
    ) -> Result<HostUpdate, HostError> {
        self.watched_changes(changes)
    }

    fn set_wxml_snapshot(&self, file: FileId, snapshot: Arc<SourceSnapshot>) {
        let mut state = self.write();
        state.snapshots.insert(file.clone(), snapshot);
        if let Ok(path) = ProjectPath::new(file.path()) {
            if let Some(view) = state.views.get_mut(&path) {
                view.view_revision = view.view_revision.next();
            }
        }
    }

    fn remove_wxml_snapshot(&self, file: &FileId) {
        let mut state = self.write();
        state.snapshots.remove(file);
        if let Ok(path) = ProjectPath::new(file.path()) {
            if let Some(view) = state.views.get_mut(&path) {
                view.view_revision = view.view_revision.next();
            }
        }
    }

    fn source(&self, file: &FileId) -> Option<Arc<SourceSnapshot>> {
        self.read().snapshots.get(file).cloned()
    }

    fn file_text(&self, file: &FileId) -> Option<Arc<str>> {
        if let Some(snapshot) = self.source(file) {
            return Some(Arc::from(snapshot.text()));
        }
        let mut state = self.write();
        ensure_project_for_id(&mut state, file.project()).ok()?;
        let project = state.project.as_ref()?;
        let path = ProjectPath::new(file.path()).ok()?;
        project.read(&path).ok().map(Arc::from)
    }

    fn view_components(&self, wxml: &ProjectPath) -> Vec<ViewComponent> {
        let state = self.read();
        let Some(project) = state.project.as_ref() else {
            return Vec::new();
        };
        // Pages keep their own `PageConfig`; components now keep theirs too, so
        // a component's own `usingComponents` drives its WXML like a page's.
        let (config, local_file) =
            if let Some(page) = project.pages().iter().find(|page| page.wxml == *wxml) {
                (&page.config, page.json.clone())
            } else if let Some(component) = project
                .components()
                .iter()
                .find(|component| component.wxml == *wxml)
            {
                (&component.config, component.json.clone())
            } else {
                return Vec::new();
            };
        let app_json = project.app_json();
        let mut components: Vec<ViewComponent> = config
            .iter()
            .map(|(name, usage)| {
                let discovered = usage
                    .component
                    .as_ref()
                    .and_then(|base| project.component(base));
                let origin = if config.local.contains_key(name) {
                    ComponentOrigin::Local {
                        file: local_file.clone(),
                    }
                } else {
                    ComponentOrigin::App {
                        file: app_json.clone(),
                    }
                };
                ViewComponent {
                    name: name.clone(),
                    specifier: usage.specifier.clone(),
                    kind: usage.kind,
                    component: usage.component.clone(),
                    wxml: discovered.map(|component| component.wxml.clone()),
                    json: discovered.and_then(|component| component.json.clone()),
                    property_facts: discovered.map_or_else(
                        || ComponentPropertyFacts {
                            properties: Vec::new(),
                            complete: false,
                            reason: Some("component metadata is unavailable"),
                        },
                        |component| component.property_facts.clone(),
                    ),
                    origin,
                }
            })
            .collect();
        components.sort_by(|left, right| left.name.cmp(&right.name));
        components
    }

    fn component_properties(
        &self,
        wxml: &ProjectPath,
        tag: &str,
    ) -> Option<ComponentPropertyFacts> {
        let mut state = self.write();
        let (script, revision, facts) = {
            let project = state.project.as_ref()?;
            let (config, _) = view_config(project, wxml)?;
            let usage = config.get(tag)?;
            if usage.kind == ComponentTargetKind::Plugin {
                return None;
            }
            let base = usage.component.as_ref()?;
            let component = project.component(base)?;
            let script = component.script.as_ref()?;
            // Without an overlay the discovery snapshot is already the
            // effective disk state; avoid re-reading the script.
            let Some(revision) = project.overlay().revision(script) else {
                return Some(component.property_facts.clone());
            };
            if let Some((cached_revision, facts)) = state.property_cache.get(script) {
                if *cached_revision == revision {
                    return Some(facts.clone());
                }
            }
            let Ok(text) = project.read(script) else {
                crate::server::log(format_args!(
                    "component property overlay could not be read; keeping discovery facts"
                ));
                return None;
            };
            let roots = wxml_project::extract_data_roots(script, &text);
            let facts = wxml_project::extract_component_property_facts(&roots);
            (script.clone(), revision, facts)
        };
        state
            .property_cache
            .insert(script, (revision, facts.clone()));
        Some(facts)
    }

    #[allow(clippy::too_many_lines)] // one linear P7-1 pipeline
    fn analysis(&self, snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        let parsed = wxml_syntax::parse(snapshot.text());
        let mut syntax = syntax_diagnostics(&parsed, snapshot);

        let mut state = self.write();
        if ensure_project_for_id(&mut state, snapshot.file().project()).is_err() {
            return Ok(syntax_only_view(
                snapshot,
                syntax,
                "no project root is open",
            ));
        }
        let Some(project) = state.project.as_ref() else {
            return Ok(syntax_only_view(
                snapshot,
                syntax,
                "no project root is open",
            ));
        };
        let Ok(path) = ProjectPath::new(snapshot.file().path()) else {
            return Ok(syntax_only_view(
                snapshot,
                syntax,
                "document path is not a project path",
            ));
        };

        let Some(view) = state.views.get(&path) else {
            return Ok(syntax_only_view(
                snapshot,
                syntax,
                "template is not a discovered page or component",
            ));
        };

        let owner = view.owner;
        let view_components = view.components.clone();
        let analysis = TemplateAnalyzer::new(owner, snapshot.revision())
            .with_component_tags(view_components.keys().cloned())
            .analyze(&parsed.tree);
        let mut hint = None;
        if analysis.truncated {
            merge_hint(
                &mut hint,
                CapabilityStatus::Partial {
                    reason: "template fragment limit reached".to_owned(),
                },
            );
            if let Some(range) = analysis
                .fragments
                .first()
                .map(|fragment| fragment.wxml_range)
                .or_else(|| {
                    analysis
                        .unsupported
                        .first()
                        .map(wxml_types::UnsupportedFeature::range)
                })
            {
                push_diagnostic(
                    &mut syntax,
                    Diagnostic {
                        range,
                        severity: DiagnosticSeverity::Warning,
                        message:
                            "template fragment limit reached; not all expressions were checked"
                                .to_owned(),
                    },
                );
            }
        }
        for feature in &analysis.unsupported {
            if is_silent_unsupported(feature.kind()) {
                continue;
            }
            let severity = match feature.kind() {
                UnsupportedKind::FragmentLimit
                | UnsupportedKind::MalformedExpression
                | UnsupportedKind::InvalidRange => DiagnosticSeverity::Warning,
                _ => DiagnosticSeverity::Information,
            };
            push_diagnostic(
                &mut syntax,
                Diagnostic {
                    range: feature.range(),
                    severity,
                    message: feature.reason().to_owned(),
                },
            );
        }
        if analysis
            .unsupported
            .iter()
            .any(|feature| !is_silent_unsupported(feature.kind()))
        {
            merge_hint(
                &mut hint,
                CapabilityStatus::Partial {
                    reason: "some template features are not analyzed".to_owned(),
                },
            );
        }

        let events = prepared_events(analysis.events, &mut hint);

        let Some(script) = view.script.as_ref() else {
            return Ok(PreparedView {
                view_id: format!("{}:0", path.as_str()),
                target: None,
                files: Arc::from(Vec::new()),
                compiler_options: serde_json::Map::new(),
                fragments: Arc::from(analysis.fragments),
                components: Arc::from(Vec::new()),
                events,
                project_root: project.root().to_path_buf(),
                syntax_diagnostics: syntax,
                status_hint: Some(CapabilityStatus::Unsupported {
                    reason: "page or component has no script".to_owned(),
                }),
            });
        };
        let script_file = view.script_file.clone();
        let view_revision = view.view_revision;
        let project_id = state.project_id.clone();
        let components =
            resolve_components(project, &project_id, &view_components, analysis.components);

        // The read guard intentionally spans source collection. Caching the
        // SourceSet and shortening this lock is deferred (P7 security M6).
        let sources = project.collect_sources(SourceLimits::default());
        let mut files = Vec::new();
        for source in &sources.files {
            if let Some(file) = prepared_file(&project_id, source) {
                files.push(file);
            }
        }
        let mut script_present = files.iter().any(|file| file.path == *script);
        if !script_present {
            match project.read(script) {
                Ok(text) => {
                    if let Some(file) = manual_file(&project_id, script, &text) {
                        files.push(file);
                    }
                }
                Err(_) => {
                    merge_hint(
                        &mut hint,
                        CapabilityStatus::Unavailable {
                            reason: "owning script could not be read".to_owned(),
                        },
                    );
                }
            }
        }
        if sources.truncated {
            merge_hint(
                &mut hint,
                CapabilityStatus::Partial {
                    reason: "project source collection was truncated".to_owned(),
                },
            );
        }
        limit_request_files(&mut files, script, &mut hint);
        script_present = files.iter().any(|file| file.path == *script);
        if !script_present {
            merge_hint(
                &mut hint,
                CapabilityStatus::Unavailable {
                    reason: "owning script exceeds the request budget".to_owned(),
                },
            );
        }

        let mut filtered_partial = false;
        let compiler_options = filter_compiler_options(
            project.compiler_options(),
            project.root(),
            &mut filtered_partial,
        );
        if filtered_partial {
            merge_hint(
                &mut hint,
                CapabilityStatus::Partial {
                    reason: "unsupported compiler options filtered".to_owned(),
                },
            );
        }

        let target = if script_present {
            script_file.map(|script_file| ViewTarget {
                wxml: path.clone(),
                owner,
                script: script.clone(),
                script_file,
                view_revision,
            })
        } else {
            None
        };
        if target.is_none() && hint.is_none() {
            merge_hint(
                &mut hint,
                CapabilityStatus::Unavailable {
                    reason: "owning script is not available".to_owned(),
                },
            );
        }

        Ok(PreparedView {
            view_id: format!("{}:{}", path.as_str(), view_revision.get()),
            target,
            files: Arc::from(files),
            compiler_options,
            fragments: Arc::from(analysis.fragments),
            components: Arc::from(components),
            events,
            project_root: project.root().to_path_buf(),
            syntax_diagnostics: syntax,
            status_hint: hint,
        })
    }
}

impl ProjectState {
    /// Applies a script or JSON overlay and reports affected views.
    ///
    /// Non-config JSON (for example `package.json`) and scripts in excluded
    /// directories are tracked by the workspace only, so they never change
    /// project state or the revision.
    fn overlay_update(&self, uri: &str, text: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        ensure_project_for_uri(&mut state, uri)?;
        state.overlays.insert(uri.to_owned(), text.to_owned());
        let path = {
            let project = state.project.as_ref().ok_or(HostError::NoProject)?;
            project.path_from_uri(uri).map_err(map_uri_error)?
        };
        let is_json = {
            let project = state.project.as_ref().ok_or(HostError::NoProject)?;
            if !project.accepts_overlay(&path) {
                return Ok(HostUpdate::default());
            }
            project.accepts_json_overlay(&path)
        };
        let revision = state.next_overlay_revision.saturating_add(1);
        state.next_overlay_revision = revision;
        {
            let project = state.project.as_mut().ok_or(HostError::NoProject)?;
            project
                .set_overlay(&path, text.to_owned(), revision)
                .map_err(|_| HostError::Read)?;
            if is_json {
                // Full rediscovery per JSON overlay is intentional for
                // correctness; incremental discovery is deferred (M6).
                project.rediscover();
            }
        }
        state.property_cache.clear();
        if is_json {
            rebuild_views(&mut state)?;
        }
        let mut dirty = dirty_after(&state, &path, is_json);
        if !is_json && dirty.is_empty() {
            // The script owns no view, but every project source feeds the type
            // backend, so an imported module can change any view's results.
            // Re-analyze all views instead of leaving diagnostics stale.
            dirty = state.views.keys().cloned().collect();
        }
        let project_revision = bump_project_revision(&mut state);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(project_revision),
        })
    }

    /// Removes an overlay when the client closes a tracked document.
    fn close_overlay(&self, uri: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        ensure_project_for_uri(&mut state, uri)?;
        state.overlays.remove(uri);
        let path = {
            let project = state.project.as_ref().ok_or(HostError::NoProject)?;
            project.path_from_uri(uri).map_err(map_uri_error)?
        };
        let is_json = {
            let project = state.project.as_ref().ok_or(HostError::NoProject)?;
            if !project.accepts_overlay(&path) {
                return Ok(HostUpdate::default());
            }
            project.accepts_json_overlay(&path)
        };
        {
            let project = state.project.as_mut().ok_or(HostError::NoProject)?;
            project.remove_overlay(&path);
            if is_json {
                project.rediscover();
            }
        }
        state.property_cache.clear();
        if is_json {
            rebuild_views(&mut state)?;
        } else {
            let _ = refresh_component_script_facts(&mut state, &path);
        }
        let mut dirty = dirty_after(&state, &path, is_json);
        if !is_json && dirty.is_empty() {
            dirty = state.views.keys().cloned().collect();
        }
        let revision = bump_project_revision(&mut state);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(revision),
        })
    }

    /// Handles a saved script/JSON document.
    ///
    /// Overlays are kept because the client still holds the buffer; only the
    /// project state that depends on the saved file is refreshed.
    fn save_overlay(&self, uri: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        ensure_project_for_uri(&mut state, uri)?;
        let path = {
            let project = state.project.as_ref().ok_or(HostError::NoProject)?;
            project.path_from_uri(uri).map_err(map_uri_error)?
        };
        let is_json = {
            let project = state.project.as_ref().ok_or(HostError::NoProject)?;
            if project.accepts_overlay(&path) {
                project.accepts_json_overlay(&path)
            } else if project.is_project_json(&path) {
                // A JSON file that discovery has not seen yet (for example a
                // page config created after startup) still affects
                // discovery when saved.
                true
            } else {
                return Ok(HostUpdate::default());
            }
        };
        state.property_cache.clear();
        if is_json {
            let project = state.project.as_mut().ok_or(HostError::NoProject)?;
            project.rediscover();
            rebuild_views(&mut state)?;
            let dirty = state.views.keys().cloned().collect();
            let revision = bump_project_revision(&mut state);
            return Ok(HostUpdate {
                dirty_views: dirty,
                project_revision: Some(revision),
            });
        }
        let refreshed = refresh_component_script_facts(&mut state, &path);
        let dirty = dirty_script_views(&state, &path);
        if refreshed {
            crate::server::log(format_args!(
                "refreshed component property facts for a saved script"
            ));
        }
        if dirty.is_empty() {
            // The saved script does not own a discovered view, so nothing
            // observable changed.
            return Ok(HostUpdate::default());
        }
        let revision = bump_project_revision(&mut state);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(revision),
        })
    }

    /// Applies `workspace/didChangeWatchedFiles`.
    ///
    /// Discovery runs at most once per notification and the project revision
    /// advances at most once. Paths outside the root, excluded scripts and
    /// irrelevant extensions are ignored.
    fn watched_changes(&self, changes: &[WatchedFileChange]) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        if state.project.is_none() {
            return Ok(HostUpdate::default());
        }
        let mut json_changes: Vec<ProjectPath> = Vec::new();
        let mut script_changes: Vec<(ProjectPath, WatchedChangeKind)> = Vec::new();
        let mut wxml_need_rediscovery = false;
        let mut ignored = 0_usize;
        for change in changes {
            let Ok(path) = state
                .project
                .as_ref()
                .ok_or(HostError::NoProject)?
                .path_from_uri(&change.uri)
            else {
                ignored = ignored.saturating_add(1);
                continue;
            };
            let (is_json, is_script, is_discovery_json) = {
                let project = state.project.as_ref().ok_or(HostError::NoProject)?;
                let is_json = project.accepts_json_overlay(&path);
                let is_script = project.accepts_script_overlay(&path);
                (
                    is_json,
                    is_script,
                    !is_json && project.is_project_json(&path),
                )
            };
            if is_json || is_discovery_json {
                json_changes.push(path);
            } else if is_script {
                script_changes.push((path, change.kind));
            } else if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wxml"))
            {
                // Created/deleted templates change discovery inputs; changed
                // templates are re-read when their view is next analyzed.
                if matches!(
                    change.kind,
                    WatchedChangeKind::Created | WatchedChangeKind::Deleted
                ) {
                    wxml_need_rediscovery = true;
                }
            } else {
                ignored = ignored.saturating_add(1);
            }
        }
        if json_changes.is_empty() && script_changes.is_empty() && !wxml_need_rediscovery {
            if ignored > 0 {
                crate::server::log(format_args!(
                    "ignoring {ignored} watched file change(s) that do not affect analysis"
                ));
            }
            return Ok(HostUpdate::default());
        }

        let scripts_need_rediscovery = script_changes.iter().any(|(_, kind)| {
            matches!(
                kind,
                WatchedChangeKind::Created | WatchedChangeKind::Deleted
            )
        });
        let rediscover =
            !json_changes.is_empty() || scripts_need_rediscovery || wxml_need_rediscovery;
        {
            let project = state.project.as_mut().ok_or(HostError::NoProject)?;
            for path in &json_changes {
                project.remove_overlay(path);
            }
            for (path, _) in &script_changes {
                project.remove_overlay(path);
            }
            if rediscover {
                project.rediscover();
            }
        }
        state.property_cache.clear();
        for (path, _) in &script_changes {
            let _ = refresh_component_script_facts(&mut state, path);
        }
        if rediscover {
            rebuild_views(&mut state)?;
        }

        let mut dirty = Vec::new();
        if rediscover {
            dirty.extend(state.views.keys().cloned());
        } else {
            for (path, _) in &script_changes {
                merge_paths(&mut dirty, dirty_script_views(&state, path));
            }
            if !script_changes.is_empty() && dirty.is_empty() {
                // Imported scripts can affect every view's type results.
                dirty.extend(state.views.keys().cloned());
            }
        }
        dirty.sort();
        dirty.dedup();
        let revision = bump_project_revision(&mut state);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(revision),
        })
    }
}

/// Rebuilds the view table after rediscovery.
fn rebuild_views(state: &mut State) -> Result<(), HostError> {
    let project_id = state.project_id.clone();
    let mut next_view_revision = state.next_view_revision;
    let views = {
        let project = state.project.as_ref().ok_or(HostError::NoProject)?;
        build_views(project, &project_id, &mut next_view_revision)
    };
    state.views = views;
    state.next_view_revision = next_view_revision;
    Ok(())
}

/// Views affected by one overlay path.
fn dirty_after(state: &State, path: &ProjectPath, is_json: bool) -> Vec<ProjectPath> {
    if is_json {
        state.views.keys().cloned().collect()
    } else {
        dirty_script_views(state, path)
    }
}

/// Appends paths that are not already present.
fn merge_paths(target: &mut Vec<ProjectPath>, paths: Vec<ProjectPath>) {
    for path in paths {
        if !target.contains(&path) {
            target.push(path);
        }
    }
}

const fn map_uri_error(_error: UriError) -> HostError {
    HostError::InvalidUri
}

/// Directories that cannot contain the Mini Program root of this workspace.
const APP_ROOT_SKIP: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    "dist",
    "build",
    "target",
    "coverage",
    ".cache",
    ".next",
    ".turbo",
    "out",
    "vendor",
    ".venv",
    "venv",
];

/// Bounds the nested-root search so a monorepo root stays cheap to open.
const MAX_APP_ROOT_DEPTH: usize = 6;
const MAX_APP_ROOT_DIRS: usize = 4096;

/// Resolves the directories that contain an `app.json`.
///
/// Editors commonly open a monorepo root while the Mini Program lives in a
/// nested package (`apps/miniprogram/src`). If the root has no `app.json`,
/// search its descendants once and return every candidate. The active root is
/// selected later from the document URI, so a monorepo with several Mini
/// Programs does not force the user to open each app separately.
fn resolve_app_roots(root: PathBuf) -> Vec<AppRoot> {
    let workspace_root = std::fs::canonicalize(&root).unwrap_or(root);
    let app_dirs = if workspace_root.join("app.json").is_file() {
        vec![workspace_root.clone()]
    } else {
        find_app_dirs(&workspace_root)
    };
    let mut found: Vec<AppRoot> = app_dirs
        .into_iter()
        .filter_map(|app_dir| app_root_for(&workspace_root, app_dir))
        .collect();
    found.sort_by(|left, right| left.app_dir.cmp(&right.app_dir));
    found.dedup_by(|left, right| left.app_dir == right.app_dir);
    found
}

/// Finds every descendant directory that contains `app.json`.
fn find_app_dirs(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut found = Vec::new();
    let mut visited = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if visited >= MAX_APP_ROOT_DIRS {
            break;
        }
        visited += 1;
        if directory.join("app.json").is_file() {
            found.push(directory);
            continue;
        }
        if depth >= MAX_APP_ROOT_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if APP_ROOT_SKIP.contains(&name.as_ref()) {
                continue;
            }
            pending.push((entry.path(), depth + 1));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Pairs an `app.json` directory with the nearest enclosing `tsconfig.json`.
fn app_root_for(workspace_root: &Path, app_dir: PathBuf) -> Option<AppRoot> {
    let mut cursor = Some(app_dir.as_path());
    let mut project_root = app_dir.clone();
    while let Some(directory) = cursor {
        if !directory.starts_with(workspace_root) {
            break;
        }
        if directory.join("tsconfig.json").is_file() {
            project_root = directory.to_path_buf();
            break;
        }
        if directory == workspace_root {
            break;
        }
        cursor = directory.parent();
    }
    let relative = app_dir.strip_prefix(&project_root).ok()?;
    let prefix = relative.to_str()?.replace('\\', "/");
    let app_json = if prefix.is_empty() {
        "app.json".to_owned()
    } else {
        format!("{prefix}/app.json")
    };
    Some(AppRoot {
        app_dir,
        project_root,
        app_json: ProjectPath::new(app_json).ok()?,
    })
}

/// Loads one candidate app root, replaying unsaved overlays that belong to it.
fn load_project(state: &mut State, candidate: &AppRoot) -> Result<(), HostError> {
    if state.project.as_ref().is_some_and(|project| {
        project.root() == candidate.project_root && project.app_json() == &candidate.app_json
    }) {
        state.active_app = Some(candidate.clone());
        return Ok(());
    }
    let mut project =
        Project::open_with_app_json(&candidate.project_root, candidate.app_json.clone())
            .map_err(|_| HostError::ProjectOpen)?;
    let project_id = project_id_for(&candidate.app_dir);
    let mut rediscover = false;
    for (uri, text) in &state.overlays {
        let Ok(path) = project.path_from_uri(uri) else {
            continue;
        };
        if !project.accepts_overlay(&path) {
            continue;
        }
        let revision = state.next_overlay_revision.saturating_add(1);
        state.next_overlay_revision = revision;
        project
            .set_overlay(&path, text.clone(), revision)
            .map_err(|_| HostError::Read)?;
        rediscover |= project.accepts_json_overlay(&path);
    }
    if rediscover {
        project.rediscover();
    }
    state.project_revision_seq = state.project_revision_seq.saturating_add(1);
    state.project_revision = ProjectRevision::new(project_id.clone(), state.project_revision_seq);
    state.project_id = project_id.clone();
    let mut next_view_revision = state.next_view_revision;
    state.views = build_views(&project, &project_id, &mut next_view_revision);
    state.next_view_revision = next_view_revision;
    state.project = Some(project);
    state.active_app = Some(candidate.clone());
    state.property_cache.clear();
    Ok(())
}

/// Selects the candidate app root that contains `uri`.
fn ensure_project_for_uri(state: &mut State, uri: &str) -> Result<(), HostError> {
    let path = wxml_project::root_path_from_uri(uri).map_err(|_| HostError::InvalidUri)?;
    if state.app_roots.is_empty() {
        return Err(HostError::NoProject);
    }
    if let Some(candidate) = state
        .app_roots
        .iter()
        .filter(|candidate| path.starts_with(&candidate.app_dir))
        .max_by_key(|candidate| candidate.app_dir.as_os_str().len())
        .cloned()
    {
        if state.active_app.as_ref() == Some(&candidate) {
            return Ok(());
        }
        return load_project(state, &candidate);
    }
    // A tsconfig or other file may live above `app.json` but inside the loaded
    // TypeScript project. Keep the active project for it.
    if state
        .active_app
        .as_ref()
        .is_some_and(|active| path.starts_with(&active.project_root))
    {
        return Ok(());
    }
    let mut roots = state
        .app_roots
        .iter()
        .filter(|candidate| path.starts_with(&candidate.project_root));
    let candidate = roots.next().cloned().ok_or(HostError::InvalidUri)?;
    if roots.next().is_some() {
        return Err(HostError::InvalidUri);
    }
    load_project(state, &candidate)
}

/// Reloads the app root that owns `project_id`.
fn ensure_project_for_id(state: &mut State, project_id: &ProjectId) -> Result<(), HostError> {
    if state.project.is_some() && &state.project_id == project_id {
        return Ok(());
    }
    let candidate = state
        .app_roots
        .iter()
        .find(|candidate| &project_id_for(&candidate.app_dir) == project_id)
        .cloned()
        .ok_or(HostError::NoProject)?;
    load_project(state, &candidate)
}

fn project_id_for(root: &Path) -> ProjectId {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in root.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    ProjectId::new(format!("proj-{hash:016x}"))
}

/// Returns the effective page/component config for one template.
fn view_config<'a>(
    project: &'a Project,
    wxml: &ProjectPath,
) -> Option<(&'a wxml_project::PageConfig, Option<ProjectPath>)> {
    if let Some(page) = project.pages().iter().find(|page| page.wxml == *wxml) {
        return Some((&page.config, page.json.clone()));
    }
    project
        .components()
        .iter()
        .find(|component| component.wxml == *wxml)
        .map(|component| (&component.config, component.json.clone()))
}

fn build_views(
    project: &Project,
    project_id: &ProjectId,
    next_view_revision: &mut u64,
) -> BTreeMap<ProjectPath, ViewState> {
    let mut views = BTreeMap::new();
    for page in project.pages() {
        insert_view(
            next_view_revision,
            &mut views,
            page,
            RootKind::Page,
            project_id,
        );
    }
    for component in project.components() {
        insert_view(
            next_view_revision,
            &mut views,
            component,
            RootKind::Component,
            project_id,
        );
    }
    views
}

fn insert_view<T>(
    next_view_revision: &mut u64,
    views: &mut BTreeMap<ProjectPath, ViewState>,
    entry: &T,
    owner: RootKind,
    project_id: &ProjectId,
) where
    T: ViewEntry,
{
    let wxml = entry.wxml();
    if views.contains_key(wxml) {
        return;
    }
    let revision = ViewRevision::new(*next_view_revision);
    *next_view_revision = next_view_revision.saturating_add(1);
    let script = entry.script().cloned();
    let script_file = script
        .as_ref()
        .and_then(|path| FileId::new(project_id.clone(), path.as_str()).ok());
    views.insert(
        wxml.clone(),
        ViewState {
            owner,
            script,
            script_file,
            view_revision: revision,
            components: entry.components(),
        },
    );
}

trait ViewEntry {
    fn wxml(&self) -> &ProjectPath;
    fn script(&self) -> Option<&ProjectPath>;
    /// Local `usingComponents` entries: display tag to component base path.
    fn components(&self) -> BTreeMap<String, ProjectPath>;
}

impl ViewEntry for Page {
    fn wxml(&self) -> &ProjectPath {
        &self.wxml
    }

    fn script(&self) -> Option<&ProjectPath> {
        self.script.as_ref()
    }

    fn components(&self) -> BTreeMap<String, ProjectPath> {
        // `PageConfig::iter` already lets page-local entries shadow globals;
        // only local components have a project path to resolve.
        local_component_paths(&self.config)
    }
}

/// Returns effective local component targets for one page/component config.
fn local_component_paths(config: &wxml_project::PageConfig) -> BTreeMap<String, ProjectPath> {
    config
        .iter()
        .filter(|(_, component)| {
            component.kind == ComponentTargetKind::Local
                || component.kind == ComponentTargetKind::Package
        })
        .filter_map(|(name, component)| {
            component
                .component
                .as_ref()
                .map(|path| (name.clone(), path.clone()))
        })
        .collect()
}

impl ViewEntry for wxml_project::Component {
    fn wxml(&self) -> &ProjectPath {
        &self.wxml
    }

    fn script(&self) -> Option<&ProjectPath> {
        self.script.as_ref()
    }

    fn components(&self) -> BTreeMap<String, ProjectPath> {
        local_component_paths(&self.config)
    }
}

/// Resolves template component usages against the project's component table.
///
/// Usages whose tag is not a local component, whose component was not
/// discovered or whose component has no script keep `component_file = None`.
fn resolve_components(
    project: &Project,
    project_id: &ProjectId,
    view_components: &BTreeMap<String, ProjectPath>,
    usages: Vec<ComponentUsage>,
) -> Vec<ComponentUsage> {
    usages
        .into_iter()
        .map(|mut usage| {
            if let Some(base) = view_components.get(&usage.tag) {
                if let Some(script) = project
                    .component(base)
                    .and_then(|component| component.script.as_ref())
                {
                    usage.component_file = FileId::new(project_id.clone(), script.as_str()).ok();
                }
            }
            usage
        })
        .collect()
}

/// Refreshes discovered property facts for one component script.
fn refresh_component_script_facts(state: &mut State, script: &ProjectPath) -> bool {
    let refreshed = state
        .project
        .as_mut()
        .is_some_and(|project| project.refresh_component_properties(script));
    if refreshed {
        state.property_cache.remove(script);
    }
    refreshed
}

fn dirty_scripts(state: &State, script: &ProjectPath) -> Vec<ProjectPath> {
    state
        .views
        .iter()
        .filter(|(_, view)| view.script.as_ref() == Some(script))
        .map(|(path, _)| path.clone())
        .collect()
}

/// Views whose component binding depends on `script`.
///
/// A component script change invalidates the component's own view through
/// [`dirty_scripts`] and every page/component view that registers that
/// component. The reverse edges are rebuilt from the existing `ViewState`
/// data; this is a bounded stand-in until P provides `affected_views`.
fn dirty_script_views(state: &State, script: &ProjectPath) -> Vec<ProjectPath> {
    let mut dirty = dirty_scripts(state, script);
    for path in affected_component_views(state, script) {
        if !dirty.contains(&path) {
            dirty.push(path);
        }
    }
    dirty
}

/// Propagates one changed component script through component users.
fn affected_component_views(state: &State, script: &ProjectPath) -> Vec<ProjectPath> {
    let mut base_by_wxml: BTreeMap<ProjectPath, ProjectPath> = BTreeMap::new();
    let mut users_by_base: BTreeMap<ProjectPath, BTreeSet<ProjectPath>> = BTreeMap::new();
    for (wxml, view) in &state.views {
        for base in view.components.values() {
            if let Ok(component_wxml) = base.with_appended_extension("wxml") {
                base_by_wxml.insert(component_wxml, base.clone());
            }
            users_by_base
                .entry(base.clone())
                .or_default()
                .insert(wxml.clone());
        }
    }

    let mut pending: VecDeque<ProjectPath> = state
        .views
        .iter()
        .filter(|(_, view)| view.script.as_ref() == Some(script))
        .filter_map(|(wxml, _)| base_by_wxml.get(wxml).cloned())
        .collect();
    let mut seen = BTreeSet::new();
    let mut affected = BTreeSet::new();
    while let Some(base) = pending.pop_front() {
        if !seen.insert(base.clone()) {
            continue;
        }
        let Some(users) = users_by_base.get(&base) else {
            continue;
        };
        for wxml in users {
            if affected.insert(wxml.clone()) {
                if let Some(parent) = base_by_wxml.get(wxml) {
                    pending.push_back(parent.clone());
                }
            }
        }
    }
    affected.into_iter().collect()
}

fn bump_project_revision(state: &mut State) -> ProjectRevision {
    state.project_revision_seq = state.project_revision_seq.saturating_add(1);
    state.project_revision =
        ProjectRevision::new(state.project_id.clone(), state.project_revision_seq);
    state.project_revision.clone()
}

/// Keeps the owning script and applies the C1 `files[]` count/byte bounds.
fn limit_request_files(
    files: &mut Vec<PreparedFile>,
    script: &ProjectPath,
    hint: &mut Option<CapabilityStatus>,
) {
    const MAX_FILES: usize = 2048;
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    if let Some(position) = files.iter().position(|file| file.path == *script) {
        if position > 0 {
            let file = files.remove(position);
            files.insert(0, file);
        }
    }
    if files.len() > MAX_FILES {
        files.truncate(MAX_FILES);
        merge_hint(
            hint,
            CapabilityStatus::Partial {
                reason: "source file count budget was truncated".to_owned(),
            },
        );
    }
    let mut total = 0_usize;
    let mut kept = Vec::with_capacity(files.len());
    for file in files.drain(..) {
        if total.saturating_add(file.text.len()) > MAX_BYTES {
            merge_hint(
                hint,
                CapabilityStatus::Partial {
                    reason: "source file byte budget was truncated".to_owned(),
                },
            );
            continue;
        }
        total = total.saturating_add(file.text.len());
        kept.push(file);
    }
    *files = kept;
}

fn prepared_file(project_id: &ProjectId, source: &SourceFile) -> Option<PreparedFile> {
    let file = FileId::new(project_id.clone(), source.path.as_str()).ok()?;
    Some(PreparedFile {
        file,
        path: source.path.clone(),
        kind: source_kind_str(source.kind),
        text: Arc::from(source.text.as_str()),
        source_revision: source.source_revision,
    })
}

fn manual_file(project_id: &ProjectId, path: &ProjectPath, text: &str) -> Option<PreparedFile> {
    let file = FileId::new(project_id.clone(), path.as_str()).ok()?;
    Some(PreparedFile {
        file,
        path: path.clone(),
        kind: script_kind(path),
        text: Arc::from(text),
        source_revision: 1,
    })
}

#[allow(clippy::match_same_arms)] // `_` covers future non_exhaustive kinds
const fn source_kind_str(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Ts => "ts",
        SourceKind::Tsx => "tsx",
        SourceKind::Js => "js",
        SourceKind::Jsx => "jsx",
        SourceKind::Dts => "d.ts",
        _ => "ts",
    }
}

#[allow(clippy::match_same_arms)] // unknown extensions fall back to `ts`
fn script_kind(path: &ProjectPath) -> &'static str {
    let name = path.file_name().to_ascii_lowercase();
    if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
        return "d.ts";
    }
    match path.extension().map(str::to_ascii_lowercase).as_deref() {
        Some("ts" | "mts" | "cts") => "ts",
        Some("tsx") => "tsx",
        Some("js" | "mjs" | "cjs") => "js",
        Some("jsx") => "jsx",
        _ => "ts",
    }
}

/// Converts parser errors and truncation into publishable diagnostics.
pub(crate) fn syntax_diagnostics(
    parsed: &ParseResult,
    snapshot: &SourceSnapshot,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for error in &parsed.errors {
        let range = error.range();
        if let Some(range) = snapshot.source_range(range) {
            diagnostics.push(Diagnostic {
                range,
                severity: if matches!(
                    error.kind(),
                    ParseErrorKind::TooManyErrors
                        | ParseErrorKind::TooManyNodes
                        | ParseErrorKind::TooManyTokens
                ) {
                    DiagnosticSeverity::Warning
                } else {
                    DiagnosticSeverity::Error
                },
                message: error.message.to_owned(),
            });
        }
    }
    if parsed.truncated {
        if let Some(range) = parsed
            .tree
            .text_range(parsed.tree.root())
            .and_then(|range| snapshot.source_range(range))
        {
            diagnostics.push(Diagnostic {
                range,
                severity: DiagnosticSeverity::Warning,
                message: "parsing stopped at a resource limit; diagnostics are incomplete"
                    .to_owned(),
            });
        }
    }
    diagnostics
}

/// Parses a snapshot and returns only the P2 syntax diagnostics.
pub(crate) fn parse_syntax_diagnostics(snapshot: &SourceSnapshot) -> Vec<Diagnostic> {
    let parsed = wxml_syntax::parse(snapshot.text());
    syntax_diagnostics(&parsed, snapshot)
}

fn syntax_only_view(
    snapshot: &SourceSnapshot,
    syntax: Vec<Diagnostic>,
    reason: &'static str,
) -> PreparedView {
    PreparedView {
        view_id: format!("{}:0", snapshot.file().path()),
        target: None,
        files: Arc::from(Vec::new()),
        compiler_options: serde_json::Map::new(),
        fragments: Arc::from(Vec::new()),
        components: Arc::from(Vec::new()),
        events: Arc::from(Vec::new()),
        project_root: PathBuf::new(),
        syntax_diagnostics: syntax,
        status_hint: Some(CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        }),
    }
}

fn push_diagnostic(diagnostics: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    if diagnostics.len() < 512 {
        diagnostics.push(diagnostic);
    }
}

/// Returns `true` for unsupported records that must stay silent.
///
/// Dynamic, empty and non-identifier event handlers are valid WXML that simply
/// cannot be resolved statically. They are recorded for callers and for the
/// status of the event chain, but publishing a diagnostic for each one would
/// add noise to every template that uses them, and they must not degrade a
/// check that otherwise ran completely.
const fn is_silent_unsupported(kind: UnsupportedKind) -> bool {
    matches!(
        kind,
        UnsupportedKind::DynamicEvent | UnsupportedKind::EmptyEvent | UnsupportedKind::InvalidEvent
    )
}

/// Caps the event list and records why later bindings were dropped.
///
/// The prepared view is what the request and the response mapping agree on, so
/// the cap lives here: ids are assigned from this list and anything beyond it
/// is never requested.
fn prepared_events(
    mut events: Vec<EventHandler>,
    hint: &mut Option<CapabilityStatus>,
) -> Arc<[EventHandler]> {
    if events.len() > MAX_PREPARED_EVENTS {
        events.truncate(MAX_PREPARED_EVENTS);
        merge_hint(
            hint,
            CapabilityStatus::Partial {
                reason: "event binding limit reached; later handlers were skipped".to_owned(),
            },
        );
    }
    Arc::from(events)
}

fn merge_hint(hint: &mut Option<CapabilityStatus>, candidate: CapabilityStatus) {
    if hint
        .as_ref()
        .is_none_or(|current| rank(&candidate) > rank(current))
    {
        *hint = Some(candidate);
    }
}

#[allow(clippy::match_same_arms)] // `_` covers future non_exhaustive variants
const fn rank(status: &CapabilityStatus) -> u8 {
    match status {
        CapabilityStatus::Unavailable { .. } => 5,
        CapabilityStatus::Disabled { .. } => 4,
        CapabilityStatus::Unsupported { .. } => 3,
        CapabilityStatus::Unknown { .. } => 2,
        CapabilityStatus::Partial { .. } => 1,
        CapabilityStatus::Supported => 0,
        _ => 2,
    }
}

/// Normalizes TypeScript enum spellings to the protocol's canonical names.
///
/// `tsconfig.json` values are case-insensitive (`Bundler`, `es2017`), while
/// the Node bridge compares exact strings. Normalizing here keeps a valid
/// project config from rejecting the whole analysis request.
fn normalize_compiler_option(key: &str, value: &serde_json::Value) -> serde_json::Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };
    let normalized = match key {
        "target" => {
            let upper = text.to_ascii_uppercase();
            if upper == "ESNEXT" {
                "ESNext".to_owned()
            } else {
                upper
            }
        }
        "module" => match text.to_ascii_uppercase().as_str() {
            "ESNEXT" => "ESNext".to_owned(),
            "COMMONJS" => "CommonJS".to_owned(),
            "PRESERVE" => "Preserve".to_owned(),
            _ => text.to_owned(),
        },
        "moduleResolution" | "jsx" | "moduleDetection" => text.to_ascii_lowercase(),
        _ => text.to_owned(),
    };
    serde_json::Value::String(normalized)
}

/// Resolves existing `typeRoots` entries against the project root.
///
/// The backend reads declarations from these directories, so missing entries
/// are reported as a partial result instead of silently dropped.
fn resolve_type_roots(
    value: &serde_json::Value,
    root: &Path,
    partial: &mut bool,
) -> Option<serde_json::Value> {
    let Some(entries) = value.as_array() else {
        *partial = true;
        return None;
    };
    let mut roots = Vec::new();
    for entry in entries {
        let Some(text) = entry.as_str() else {
            *partial = true;
            continue;
        };
        let candidate = Path::new(text);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let candidate: PathBuf = candidate
            .components()
            .filter(|component| !matches!(component, std::path::Component::CurDir))
            .collect();
        if candidate.is_dir() {
            roots.push(serde_json::Value::String(
                candidate.to_string_lossy().into_owned(),
            ));
        } else {
            *partial = true;
        }
    }
    if roots.is_empty() {
        *partial = true;
        None
    } else {
        Some(serde_json::Value::Array(roots))
    }
}

fn filter_compiler_options(
    input: Option<&BTreeMap<String, serde_json::Value>>,
    root: &Path,
    partial: &mut bool,
) -> serde_json::Map<String, serde_json::Value> {
    let mut output = serde_json::Map::new();
    let Some(input) = input else {
        output.insert(
            "types".to_owned(),
            serde_json::Value::Array(vec![serde_json::Value::String(
                "miniprogram-api-typings".to_owned(),
            )]),
        );
        return output;
    };
    for (key, value) in input {
        if !SUPPORTED_COMPILER_OPTIONS
            .iter()
            .any(|supported| *supported == key)
        {
            *partial = true;
            continue;
        }
        let value = &normalize_compiler_option(key, value);
        if key == "baseUrl" {
            if let Some(base) = value.as_str() {
                let base = Path::new(base);
                // An absolute or parent-traversing baseUrl would point the
                // backend outside the project, so drop it instead of resolving.
                if base.is_absolute()
                    || base
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    *partial = true;
                } else {
                    let absolute: PathBuf = root
                        .join(base)
                        .components()
                        .filter(|component| !matches!(component, std::path::Component::CurDir))
                        .collect();
                    output.insert(
                        key.clone(),
                        serde_json::Value::String(absolute.to_string_lossy().into_owned()),
                    );
                }
            } else {
                *partial = true;
            }
            continue;
        }
        if key == "typeRoots" {
            if let Some(roots) = resolve_type_roots(value, root, partial) {
                output.insert(key.clone(), roots);
            }
            continue;
        }
        if key == "types" {
            if let Some(array) = value.as_array() {
                let mut types: Vec<serde_json::Value> = array
                    .iter()
                    .filter(|entry| entry.is_string())
                    .cloned()
                    .collect();
                if !types
                    .iter()
                    .any(|entry| entry.as_str() == Some("miniprogram-api-typings"))
                {
                    types.push(serde_json::Value::String(
                        "miniprogram-api-typings".to_owned(),
                    ));
                }
                output.insert(key.clone(), serde_json::Value::Array(types));
            } else {
                *partial = true;
            }
            continue;
        }
        output.insert(key.clone(), value.clone());
    }
    if !output.contains_key("types") {
        output.insert(
            "types".to_owned(),
            serde_json::Value::Array(vec![serde_json::Value::String(
                "miniprogram-api-typings".to_owned(),
            )]),
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use wxml_types::{SnapshotId, SourceRevision};

    #[test]
    fn using_components_are_exposed_for_a_page() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/completion-components");
        let state = ProjectState::new();
        state
            .open_root(Some(&format!("file://{}", root.display())))
            .expect("open fixture");
        let path = ProjectPath::new("index.wxml").expect("path");
        let components = state.view_components(&path);
        let names: Vec<_> = components.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["local-badge", "my-card"]);
        assert!(components.iter().any(|entry| entry.component.is_some()));
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one end-to-end host scenario
    fn components_expose_their_own_using_components_and_local_overrides() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/component-intelligence");
        let state = ProjectState::new();
        state
            .open_root(Some(&format!("file://{}", root.display())))
            .expect("open fixture");

        let page = ProjectPath::new("pages/index/index.wxml").expect("page");
        let components = state.view_components(&page);
        let shared = components
            .iter()
            .find(|component| component.name == "shared-card")
            .expect("shared card");
        assert_eq!(
            shared.wxml.as_ref().map(ProjectPath::as_str),
            Some("components/page-card/page-card.wxml"),
            "the page-local entry must shadow the app entry"
        );
        assert_eq!(
            shared.origin.file().map(ProjectPath::as_str),
            Some("pages/index/index.json")
        );
        assert!(
            shared
                .property_facts
                .properties
                .iter()
                .any(|property| property.name == "pageOnly")
        );
        let remote = components
            .iter()
            .find(|component| component.name == "remote-card")
            .expect("plugin component");
        assert_eq!(remote.kind, ComponentTargetKind::Plugin);
        assert!(remote.wxml.is_none());
        assert_eq!(remote.property_facts.properties.len(), 0);

        let own = ProjectPath::new("components/outer-card/outer-card.wxml").expect("component");
        let inner = state
            .view_components(&own)
            .into_iter()
            .find(|component| component.name == "inner-badge")
            .expect("component-own usingComponents");
        assert_eq!(
            inner.wxml.as_ref().map(ProjectPath::as_str),
            Some("components/inner-badge/inner-badge.wxml")
        );
        assert_eq!(
            inner.origin.file().map(ProjectPath::as_str),
            Some("components/outer-card/outer-card.json")
        );
        assert!(
            inner
                .property_facts
                .properties
                .iter()
                .any(|property| property.name == "label")
        );
        let own_shared = state
            .view_components(&own)
            .into_iter()
            .find(|component| component.name == "shared-card")
            .expect("component-local override");
        assert_eq!(
            own_shared.wxml.as_ref().map(ProjectPath::as_str),
            Some("components/page-card/page-card.wxml")
        );
        assert_eq!(
            own_shared.origin.file().map(ProjectPath::as_str),
            Some("components/outer-card/outer-card.json")
        );

        let script = ProjectPath::new("components/outer-card/outer-card.ts").expect("script");
        let uri = format!("file://{}/{}", root.display(), script.as_str());
        let update = state
            .open_document(OpenDocument {
                uri,
                text: "Component({ properties: { freshCount: { type: Number } } });".to_owned(),
                version: Some(2),
            })
            .expect("script overlay");
        assert!(
            update
                .dirty_views
                .contains(&ProjectPath::new("pages/index/index.wxml").expect("page")),
            "a component script change must dirty its callers: {:?}",
            update.dirty_views
        );
        assert!(
            update.dirty_views.contains(
                &ProjectPath::new("components/outer-card/outer-card.wxml").expect("view")
            ),
            "the owning component view must stay dirty: {:?}",
            update.dirty_views
        );
        let properties = state
            .component_properties(&page, "outer-card")
            .expect("live properties");
        assert_eq!(properties.properties.len(), 1);
        assert_eq!(properties.properties[0].name, "freshCount");
        assert_eq!(properties.properties[0].type_text, "number");
        assert!(properties.complete);

        let inner_script =
            ProjectPath::new("components/inner-badge/inner-badge.ts").expect("inner script");
        let inner_uri = format!("file://{}/{}", root.display(), inner_script.as_str());
        let update = state
            .open_document(OpenDocument {
                uri: inner_uri,
                text: "Component({ properties: { freshLabel: { type: String } } });".to_owned(),
                version: Some(3),
            })
            .expect("nested script overlay");
        assert!(
            update
                .dirty_views
                .contains(&ProjectPath::new("pages/index/index.wxml").expect("page")),
            "nested component changes must propagate recursively: {:?}",
            update.dirty_views
        );
        assert!(
            update.dirty_views.contains(
                &ProjectPath::new("components/outer-card/outer-card.wxml").expect("outer")
            ),
            "the direct component user must be dirty: {:?}",
            update.dirty_views
        );
    }

    #[test]
    fn nested_app_root_is_discovered_from_a_monorepo_root() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-nested-root-{stamp}"));
        let app = root.join("apps/miniprogram/src");
        std::fs::create_dir_all(app.join("pages/index")).expect("create app root");
        std::fs::write(app.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
            .expect("write app.json");
        std::fs::write(
            app.join("pages/index/index.wxml"),
            "<view>{{ title }}</view>",
        )
        .expect("write wxml");
        std::fs::write(
            app.join("pages/index/index.ts"),
            "Page({ data: { title: 'nested' } });",
        )
        .expect("write script");
        std::fs::create_dir_all(root.join("apps/miniprogram/dist")).expect("create dist");
        std::fs::write(
            root.join("apps/miniprogram/dist/app.json"),
            r#"{"pages":["pages/ignored/ignored"]}"#,
        )
        .expect("write ignored app.json");

        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open monorepo root");
        let wxml = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        let view = host.view_for(&wxml).expect("nested view");
        assert_eq!(view.script.as_str(), "pages/index/index.ts");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn documents_select_their_app_root_in_a_multi_app_workspace() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-multi-root-{stamp}"));
        for app in ["apps/a/src", "apps/b/src"] {
            let app = root.join(app);
            std::fs::create_dir_all(app.join("pages/index")).expect("create app root");
            std::fs::write(app.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
                .expect("write app.json");
            std::fs::write(
                app.join("pages/index/index.wxml"),
                "<view>{{ title }}</view>",
            )
            .expect("write wxml");
            std::fs::write(
                app.join("pages/index/index.ts"),
                "Page({ data: { title: 'page' } });",
            )
            .expect("write script");
        }

        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open monorepo root");
        assert_eq!(host.project_id().as_str(), SYNTHETIC_PROJECT);

        let b_uri = format!(
            "file://{}/apps/b/src/pages/index/index.wxml",
            root.display()
        );
        let b_path = host.path_for_uri(&b_uri).expect("b path");
        let b_view = host.view_for(&b_path).expect("b view");
        assert_eq!(b_view.script.as_str(), "pages/index/index.ts");
        assert_ne!(host.project_id().as_str(), SYNTHETIC_PROJECT);

        let a_uri = format!(
            "file://{}/apps/a/src/pages/index/index.wxml",
            root.display()
        );
        let a_path = host.path_for_uri(&a_uri).expect("a path");
        let a_view = host.view_for(&a_path).expect("a view");
        assert_eq!(a_view.script.as_str(), "pages/index/index.ts");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn project_id_does_not_contain_the_root_path() {
        let id = project_id_for(Path::new("/tmp/very/secret/project"));
        assert!(id.as_str().starts_with("proj-"));
        assert!(!id.as_str().contains("secret"));
        assert!(!id.as_str().contains('/'));
    }

    #[test]
    fn normalizes_case_insensitive_tsconfig_enums() {
        for (key, input, expected) in [
            ("target", "es2017", "ES2017"),
            ("target", "ESNext", "ESNext"),
            ("module", "esnext", "ESNext"),
            ("module", "commonjs", "CommonJS"),
            ("moduleResolution", "Bundler", "bundler"),
            ("jsx", "React-JSX", "react-jsx"),
        ] {
            let value = serde_json::Value::String(input.to_owned());
            assert_eq!(
                normalize_compiler_option(key, &value),
                serde_json::Value::String(expected.to_owned()),
                "{key}={input}"
            );
        }
    }

    #[test]
    fn resolves_existing_type_roots_and_reports_missing_ones() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let existing = root.join("tests/fixtures");
        let mut input = BTreeMap::new();
        input.insert(
            "typeRoots".to_owned(),
            serde_json::json!(["tests/fixtures", existing.to_string_lossy()]),
        );
        let mut partial = false;
        let output = filter_compiler_options(Some(&input), &root, &mut partial);
        assert!(!partial);
        assert_eq!(
            output["typeRoots"],
            serde_json::json!([existing.to_string_lossy(), existing.to_string_lossy()])
        );

        input.insert(
            "typeRoots".to_owned(),
            serde_json::json!(["does-not-exist"]),
        );
        let mut partial = false;
        let output = filter_compiler_options(Some(&input), &root, &mut partial);
        assert!(partial);
        assert!(!output.contains_key("typeRoots"));
    }

    #[test]
    fn passes_decorator_and_output_options_through() {
        let mut input = BTreeMap::new();
        input.insert(
            "experimentalDecorators".to_owned(),
            serde_json::Value::Bool(true),
        );
        input.insert(
            "outDir".to_owned(),
            serde_json::Value::String("./dist/".to_owned()),
        );
        let mut partial = false;
        let output = filter_compiler_options(Some(&input), Path::new("/tmp/root"), &mut partial);
        assert!(!partial);
        assert_eq!(
            output["experimentalDecorators"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            output["outDir"],
            serde_json::Value::String("./dist/".to_owned())
        );
    }

    #[test]
    fn filters_unknown_compiler_options_and_merges_typings() {
        let mut input = BTreeMap::new();
        input.insert("strict".to_owned(), serde_json::Value::Bool(true));
        input.insert("unknownOption".to_owned(), serde_json::Value::Bool(true));
        input.insert(
            "types".to_owned(),
            serde_json::Value::Array(vec![serde_json::Value::String("node".to_owned())]),
        );
        input.insert(
            "baseUrl".to_owned(),
            serde_json::Value::String(".".to_owned()),
        );
        let mut partial = false;
        let output = filter_compiler_options(Some(&input), Path::new("/tmp/root"), &mut partial);
        assert!(partial);
        assert_eq!(output["strict"], serde_json::Value::Bool(true));
        assert!(!output.contains_key("unknownOption"));
        let types = output["types"].as_array().expect("types");
        assert!(
            types
                .iter()
                .any(|entry| entry.as_str() == Some("miniprogram-api-typings"))
        );
        assert_eq!(
            output["baseUrl"],
            serde_json::Value::String("/tmp/root".to_owned())
        );
    }

    #[test]
    fn rejects_absolute_or_parent_base_url() {
        for base in ["/etc", "../outside", "sub/../../outside"] {
            let mut input = BTreeMap::new();
            input.insert(
                "baseUrl".to_owned(),
                serde_json::Value::String(base.to_owned()),
            );
            let mut partial = false;
            let output =
                filter_compiler_options(Some(&input), Path::new("/tmp/root"), &mut partial);
            assert!(partial, "{base}");
            assert!(!output.contains_key("baseUrl"), "{base}");
        }
        let mut input = BTreeMap::new();
        input.insert(
            "baseUrl".to_owned(),
            serde_json::Value::String("src".to_owned()),
        );
        let mut partial = false;
        let output = filter_compiler_options(Some(&input), Path::new("/tmp/root"), &mut partial);
        assert!(!partial);
        assert_eq!(
            output["baseUrl"],
            serde_json::Value::String("/tmp/root/src".to_owned())
        );
    }

    #[test]
    fn file_budget_keeps_the_owning_script_first() {
        let project = ProjectId::new("p");
        let script = ProjectPath::new("late.ts").expect("script");
        let mut files = Vec::new();
        for index in 0..2100 {
            let path = ProjectPath::new(format!("f{index}.ts")).expect("path");
            files.push(PreparedFile {
                file: FileId::new(project.clone(), path.as_str()).expect("file"),
                path,
                kind: "ts",
                text: Arc::from(""),
                source_revision: 1,
            });
        }
        files.push(PreparedFile {
            file: FileId::new(project, script.as_str()).expect("file"),
            path: script.clone(),
            kind: "ts",
            text: Arc::from("script"),
            source_revision: 1,
        });
        let mut hint = None;
        limit_request_files(&mut files, &script, &mut hint);
        assert_eq!(files.len(), 2048);
        assert_eq!(files[0].path, script);
        assert!(matches!(hint, Some(CapabilityStatus::Partial { .. })));
    }

    #[test]
    fn status_rank_prefers_unavailable() {
        let mut hint = Some(CapabilityStatus::Partial {
            reason: "partial".to_owned(),
        });
        merge_hint(
            &mut hint,
            CapabilityStatus::Unavailable {
                reason: "down".to_owned(),
            },
        );
        assert!(matches!(hint, Some(CapabilityStatus::Unavailable { .. })));
    }

    #[test]
    fn script_kind_matches_c1_names() {
        let path = ProjectPath::new("types.d.ts").expect("path");
        assert_eq!(script_kind(&path), "d.ts");
        let path = ProjectPath::new("a.mts").expect("path");
        assert_eq!(script_kind(&path), "ts");
    }

    #[test]
    fn pages_resolve_local_using_components_to_component_scripts() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        let root = std::fs::canonicalize(root).expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open project");
        let path = ProjectPath::new("pages/index/index.wxml").expect("path");
        let file = host.file_id(&path).expect("file id");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            r#"<card title="{{ pageTitle }}" />"#,
        ));
        let view = host.analysis(&snapshot).expect("analysis");
        assert_eq!(view.components.len(), 1);
        let usage = &view.components[0];
        assert_eq!(usage.tag, "card");
        assert_eq!(usage.attributes.len(), 1);
        assert_eq!(usage.attributes[0].name, "title");
        let component_file = usage.component_file.as_ref().expect("component file");
        assert_eq!(component_file.path(), "components/card/card.ts");
    }

    #[test]
    fn analysis_exposes_static_events_and_filters_dynamic_handlers() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        let root = std::fs::canonicalize(root).expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open project");
        let path = ProjectPath::new("pages/index/index.wxml").expect("path");
        let file = host.file_id(&path).expect("file id");
        let text =
            r#"<view bindtap="onTap" catch:tap="{{ dynamic }}" bind:touchstart="item.onTap" />"#;
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(2),
            text,
        ));
        let view = host.analysis(&snapshot).expect("analysis");
        assert_eq!(view.events.len(), 1);
        let event = &view.events[0];
        assert_eq!(event.attribute, "bindtap");
        assert_eq!(event.event, "tap");
        assert_eq!(event.handler, "onTap");
        assert_eq!(
            text.get(
                usize::try_from(event.handler_range.start()).expect("start")
                    ..usize::try_from(event.handler_range.end()).expect("end")
            ),
            Some("onTap")
        );
        assert_eq!(
            text.get(
                usize::try_from(event.attribute_range.start()).expect("start")
                    ..usize::try_from(event.attribute_range.end()).expect("end")
            ),
            Some(r#"bindtap="onTap""#)
        );
        assert!(
            view.syntax_diagnostics.is_empty(),
            "event handlers must not publish noise: {:?}",
            view.syntax_diagnostics
        );
        assert!(
            view.status_hint.is_none(),
            "silent event forms must not degrade the check: {:?}",
            view.status_hint
        );
    }

    #[test]
    fn analysis_caps_prepared_events_and_merges_a_partial_hint() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        let root = std::fs::canonicalize(root).expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open project");
        let path = ProjectPath::new("pages/index/index.wxml").expect("path");
        let file = host.file_id(&path).expect("file id");
        let mut text = String::from("<view>");
        for index in 0..=MAX_PREPARED_EVENTS {
            write!(text, "<view bindtap=\"onTap{index}\" />").expect("write");
        }
        text.push_str("</view>");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(3),
            text,
        ));
        let view = host.analysis(&snapshot).expect("analysis");
        assert_eq!(view.events.len(), MAX_PREPARED_EVENTS);
        assert_eq!(view.events[0].handler, "onTap0");
        let Some(CapabilityStatus::Partial { reason }) = view.status_hint else {
            panic!("expected a partial hint, got {:?}", view.status_hint);
        };
        assert!(reason.contains("event binding limit"), "{reason}");
    }

    #[test]
    fn component_facts_refresh_after_save_close_and_watched_changes() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-component-refresh-{stamp}"));
        std::fs::create_dir_all(root.join("components/card")).expect("component dir");
        std::fs::create_dir_all(root.join("pages/index")).expect("page dir");
        std::fs::write(
            root.join("app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{"card":"/components/card/card"}}"#,
        )
        .expect("app");
        std::fs::write(root.join("pages/index/index.wxml"), "<card />").expect("page");
        std::fs::write(root.join("pages/index/index.ts"), "Page({});").expect("page script");
        std::fs::write(root.join("components/card/card.wxml"), "<view/>").expect("card wxml");
        std::fs::write(
            root.join("components/card/card.ts"),
            "Component({ properties: { oldProp: { type: String } } });",
        )
        .expect("card script");
        let state = ProjectState::new();
        state
            .open_root(Some(&format!("file://{}", root.display())))
            .expect("open");
        let page = ProjectPath::new("pages/index/index.wxml").expect("page path");
        let script = ProjectPath::new("components/card/card.ts").expect("script path");
        let script_uri = format!("file://{}/{}", root.display(), script.as_str());

        state
            .open_document(OpenDocument {
                uri: script_uri.clone(),
                text: "Component({ properties: { newProp: { type: Number } } });".to_owned(),
                version: Some(2),
            })
            .expect("overlay");
        let facts = state
            .component_properties(&page, "card")
            .expect("overlay facts");
        assert_eq!(facts.properties[0].name, "newProp");

        state.save_document(&script_uri).expect("save");
        std::fs::write(
            root.join("components/card/card.ts"),
            "Component({ properties: { newProp: { type: Number } } });",
        )
        .expect("save disk");
        state.close_document(&script_uri).expect("close");
        let facts = state
            .component_properties(&page, "card")
            .expect("saved facts");
        assert_eq!(facts.properties[0].name, "newProp");
        assert_eq!(facts.properties[0].script, script);

        std::fs::write(
            root.join("components/card/card.ts"),
            "Component({ properties: { watchedProp: { type: Boolean } } });",
        )
        .expect("watched disk");
        let update = state
            .watched_files_changed(&[WatchedFileChange {
                uri: script_uri.clone(),
                kind: WatchedChangeKind::Changed,
            }])
            .expect("watched");
        assert!(
            update
                .dirty_views
                .contains(&ProjectPath::new("pages/index/index.wxml").expect("page")),
            "watched component script must dirty callers: {:?}",
            update.dirty_views
        );
        let facts = state
            .component_properties(&page, "card")
            .expect("watched facts");
        assert_eq!(facts.properties[0].name, "watchedProp");

        std::fs::remove_file(root.join("components/card/card.ts")).expect("delete script");
        state
            .watched_files_changed(&[WatchedFileChange {
                uri: script_uri,
                kind: WatchedChangeKind::Deleted,
            }])
            .expect("deleted");
        let facts = state.component_properties(&page, "card");
        assert!(facts.is_none_or(|facts| facts.properties.is_empty()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn live_component_facts_do_not_erase_known_properties_when_incomplete() {
        let mut component = ViewComponent {
            name: "card".to_owned(),
            specifier: "./card".to_owned(),
            kind: ComponentTargetKind::Local,
            component: Some(ProjectPath::new("card").expect("path")),
            wxml: Some(ProjectPath::new("card.wxml").expect("path")),
            json: None,
            property_facts: ComponentPropertyFacts {
                properties: vec![wxml_project::ComponentProperty {
                    name: "title".to_owned(),
                    type_text: "string".to_owned(),
                    default_text: None,
                    documentation: None,
                    script: ProjectPath::new("card.ts").expect("path"),
                    range: wxml_text::TextRange::new(0, 5).expect("range"),
                }],
                complete: true,
                reason: None,
            },
            origin: ComponentOrigin::Local { file: None },
        };
        component.apply_live_facts(ComponentPropertyFacts {
            properties: Vec::new(),
            complete: false,
            reason: Some("properties object is malformed"),
        });
        assert_eq!(component.property_facts.properties.len(), 1);
        assert!(!component.property_facts.complete);
        assert_eq!(
            component.property_facts.reason,
            Some("properties object is malformed")
        );

        component.apply_live_facts(ComponentPropertyFacts {
            properties: Vec::new(),
            complete: true,
            reason: None,
        });
        assert_eq!(component.property_facts.properties.len(), 0);
        assert!(component.property_facts.complete);
    }

    #[test]
    fn null_host_is_explicitly_unsupported() {
        let host = NullHost;
        assert_eq!(host.route("file:///a.wxml"), DocumentRoute::Unknown);
        assert_eq!(
            host.path_for_uri("file:///a.wxml"),
            Err(HostError::NoProject)
        );
        assert!(
            host.uri_for(&FileId::new(ProjectId::new("x"), "a.wxml").expect("file"))
                .is_none()
        );
        assert_eq!(
            host.view_for(&ProjectPath::new("a.wxml").expect("path")),
            Err(HostError::NoProject)
        );
    }
}
