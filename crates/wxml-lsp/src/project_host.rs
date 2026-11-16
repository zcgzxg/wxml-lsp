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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use thiserror::Error;
use wxml_project::{
    ComponentTargetKind, Page, Project, ProjectPath, SourceFile, SourceKind, SourceLimits, UriError,
};
use wxml_syntax::{ParseErrorKind, ParseResult};
use wxml_types::{
    CapabilityStatus, CheckFragment, ComponentUsage, Diagnostic, DiagnosticSeverity, EventHandler,
    FileId, ProjectId, ProjectRevision, RootKind, SnapshotId, SourceSnapshot, TemplateAnalyzer,
    UnsupportedKind, ViewRevision,
};

use crate::configuration::{AppRoot, ConfigurationError, InitializationOptions};
use crate::dependencies::DependencyIndex;

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

/// One custom component registered by the current view's `usingComponents`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewComponent {
    /// Tag name used in the template (for example `my-card`).
    pub name: String,
    /// Raw `usingComponents` specifier.
    pub specifier: String,
    /// Resolved component WXML path, when discovery could resolve it.
    pub component: Option<ProjectPath>,
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
    /// Explicit app configuration is invalid; never fall back to discovery.
    #[error(transparent)]
    Configuration(#[from] ConfigurationError),
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
    /// Opens explicitly configured apps; simple hosts retain their root behavior.
    fn open_root_with_options(
        &self,
        root_uri: Option<&str>,
        _options: &InitializationOptions,
    ) -> Result<(), HostError> {
        self.open_root(root_uri)
    }
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
    /// Resolves a URI's file identity without changing foreground routing.
    fn resolve_file(&self, uri: &str) -> Result<FileId, HostError> {
        let path = self.path_for_uri(uri)?;
        self.file_id(&path)
    }
    /// Activates an already resolved project identity for a foreground update.
    fn activate_project(&self, project: &ProjectId) -> Result<(), HostError> {
        if &self.project_id() == project {
            Ok(())
        } else {
            Err(HostError::NoProject)
        }
    }
    /// Maps a file identity back to a client URI.
    fn uri_for(&self, file: &FileId) -> Option<String>;
    /// Creates the file identity for a project path.
    fn file_id(&self, path: &ProjectPath) -> Result<FileId, HostError>;
    /// Creates the file identity for a project path in an explicit project.
    fn file_id_in(&self, _project: &ProjectId, path: &ProjectPath) -> Result<FileId, HostError> {
        self.file_id(path)
    }
    /// Routes a client URI.
    fn route(&self, uri: &str) -> DocumentRoute;
    /// Returns the discovered view for a WXML path.
    fn view_for(&self, wxml: &ProjectPath) -> Result<ViewTarget, HostError>;
    /// Returns the discovered view for a project file identity.
    fn view_for_file(&self, file: &FileId) -> Result<ViewTarget, HostError> {
        let path = ProjectPath::new(file.path()).map_err(|_| HostError::InvalidUri)?;
        self.view_for(&path)
    }
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

    /// Returns the custom components registered for one view identity.
    fn view_components_for(&self, file: &FileId) -> Vec<ViewComponent> {
        ProjectPath::new(file.path())
            .ok()
            .map_or_else(Vec::new, |path| self.view_components(&path))
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

/// Cache key for one prepared analysis input set.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedCacheKey {
    path: ProjectPath,
    snapshot: SnapshotId,
    view_revision: ViewRevision,
    project_revision: ProjectRevision,
}

#[derive(Clone, Debug)]
struct PreparedCacheEntry {
    key: PreparedCacheKey,
    view: Arc<PreparedView>,
}

/// Number of recently prepared views kept per app.
const MAX_PREPARED_CACHE: usize = 4;

/// One loaded Mini Program app and everything derived from it.
#[derive(Debug)]
struct ProjectSlot {
    app: AppRoot,
    project: Project,
    project_revision: ProjectRevision,
    /// Per-project revision sequence; never shared with another app.
    revision_seq: u64,
    views: BTreeMap<ProjectPath, ViewState>,
    dependencies: DependencyIndex,
    /// Monotonic content versions for disk files observed to change.
    file_versions: BTreeMap<ProjectPath, u64>,
    /// Small per-project prepared-view cache, newest first.
    prepared: Vec<PreparedCacheEntry>,
    /// A shared `tsconfig` chain changed; reload from disk on next use.
    config_stale: bool,
}

#[derive(Debug)]
struct State {
    /// Explicit app roots from `InitializationOptions::resolve`.
    app_roots: Vec<AppRoot>,
    /// Loaded apps keyed by stable, path-free project identity.
    projects: BTreeMap<ProjectId, ProjectSlot>,
    /// App that owns foreground/path-only host queries.
    active_project: Option<ProjectId>,
    snapshots: HashMap<FileId, Arc<SourceSnapshot>>,
    /// Authoritative unsaved text keyed by client URI.
    overlays: BTreeMap<String, (String, u64)>,
    next_view_revision: u64,
    next_overlay_revision: u64,
    next_file_version: u64,
}

/// Project state and analysis inputs for one workspace root.
///
/// A workspace root may contain several explicitly configured Mini Programs.
/// Each loaded app keeps its own project revision, views, dependency index,
/// file versions and prepared-view cache, so two apps with the same relative
/// paths never share analysis state.
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
        Self {
            state: RwLock::new(State {
                app_roots: Vec::new(),
                projects: BTreeMap::new(),
                active_project: None,
                snapshots: HashMap::new(),
                overlays: BTreeMap::new(),
                next_view_revision: 1,
                next_overlay_revision: 1,
                next_file_version: 1,
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
        self.open_root_with_options(root_uri, &InitializationOptions::default())
    }

    /// Opens configured apps without scanning directories or inferring roots.
    pub fn open_root_with_options(
        &self,
        root_uri: Option<&str>,
        options: &InitializationOptions,
    ) -> Result<(), HostError> {
        let mut state = self.write();
        state.app_roots.clear();
        state.projects.clear();
        state.active_project = None;
        state.snapshots.clear();
        state.overlays.clear();
        state.next_view_revision = 1;
        state.next_overlay_revision = 1;
        state.next_file_version = 1;
        let Some(uri) = root_uri else {
            if !options.apps.is_empty() {
                return Err(ConfigurationError::MissingWorkspace.into());
            }
            return Ok(());
        };
        let root = wxml_project::root_path_from_uri(uri).map_err(|_| HostError::ProjectOpen)?;
        let root = std::fs::canonicalize(root).map_err(|_| HostError::ProjectOpen)?;
        let roots = options.resolve(&root)?;
        state.app_roots.clone_from(&roots);
        if roots.len() == 1 {
            let root = roots.into_iter().next().ok_or(HostError::ProjectOpen)?;
            ensure_loaded(&mut state, &root, true)?;
        }
        Ok(())
    }

    /// Returns the project root of the active (or only) loaded project.
    #[must_use]
    pub fn root(&self) -> Option<PathBuf> {
        let state = self.read();
        let project = active_project_id(&state)?;
        state
            .projects
            .get(project)
            .map(|slot| slot.app.project_root.clone())
    }
}

impl ProjectHost for ProjectState {
    fn open_root(&self, root_uri: Option<&str>) -> Result<(), HostError> {
        Self::open_root(self, root_uri)
    }

    fn open_root_with_options(
        &self,
        root_uri: Option<&str>,
        options: &InitializationOptions,
    ) -> Result<(), HostError> {
        Self::open_root_with_options(self, root_uri, options)
    }

    fn project_id(&self) -> ProjectId {
        let state = self.read();
        active_project_id(&state)
            .cloned()
            .unwrap_or_else(|| ProjectId::new(SYNTHETIC_PROJECT))
    }

    fn project_revision(&self) -> ProjectRevision {
        let state = self.read();
        active_project_id(&state)
            .and_then(|id| state.projects.get(id))
            .map_or_else(
                || ProjectRevision::new(ProjectId::new(SYNTHETIC_PROJECT), 1),
                |slot| slot.project_revision.clone(),
            )
    }

    fn is_project_open(&self) -> bool {
        !self.read().app_roots.is_empty()
    }

    fn path_for_uri(&self, uri: &str) -> Result<ProjectPath, HostError> {
        let mut state = self.write();
        let (_, path) = route_project(&mut state, uri)?;
        Ok(path)
    }

    fn resolve_file(&self, uri: &str) -> Result<FileId, HostError> {
        let state = self.read();
        let app = app_for_uri(&state, uri)?;
        let path = wxml_project::files::project_path_from_uri(&app.project_root, uri)
            .map_err(map_uri_error)?;
        FileId::new(project_id_for(&app.app_dir), path.as_str()).map_err(|_| HostError::InvalidUri)
    }

    fn activate_project(&self, project: &ProjectId) -> Result<(), HostError> {
        let mut state = self.write();
        let candidate = state
            .app_roots
            .iter()
            .find(|app| &project_id_for(&app.app_dir) == project)
            .cloned()
            .ok_or(HostError::NoProject)?;
        ensure_loaded(&mut state, &candidate, true).map(|_| ())
    }

    fn uri_for(&self, file: &FileId) -> Option<String> {
        let state = self.read();
        let path = ProjectPath::new(file.path()).ok()?;
        if let Some(slot) = state.projects.get(file.project()) {
            return wxml_project::files::uri_for_project_path(&slot.app.project_root, &path);
        }
        let app = state
            .app_roots
            .iter()
            .find(|app| project_id_for(&app.app_dir) == *file.project())?;
        wxml_project::files::uri_for_project_path(&app.project_root, &path)
    }

    fn file_id(&self, path: &ProjectPath) -> Result<FileId, HostError> {
        let state = self.read();
        let project = active_project_id(&state).cloned().unwrap_or_else(|| {
            if state.projects.len() == 1 {
                state
                    .projects
                    .keys()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| ProjectId::new(SYNTHETIC_PROJECT))
            } else {
                ProjectId::new(SYNTHETIC_PROJECT)
            }
        });
        FileId::new(project, path.as_str()).map_err(|_| HostError::InvalidUri)
    }

    fn file_id_in(&self, project: &ProjectId, path: &ProjectPath) -> Result<FileId, HostError> {
        FileId::new(project.clone(), path.as_str()).map_err(|_| HostError::InvalidUri)
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
        let project = active_project_id(&state).ok_or(HostError::Unsupported {
            reason: "template is not a discovered page or component",
        })?;
        view_in_slot(&state, project, wxml)
    }

    fn view_for_file(&self, file: &FileId) -> Result<ViewTarget, HostError> {
        let state = self.read();
        let path = ProjectPath::new(file.path()).map_err(|_| HostError::InvalidUri)?;
        view_in_slot(&state, file.project(), &path)
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
            bump_view_revision(&mut state, file.project(), &path);
        }
    }

    fn remove_wxml_snapshot(&self, file: &FileId) {
        let mut state = self.write();
        state.snapshots.remove(file);
        if let Ok(path) = ProjectPath::new(file.path()) {
            bump_view_revision(&mut state, file.project(), &path);
        }
    }

    fn source(&self, file: &FileId) -> Option<Arc<SourceSnapshot>> {
        self.read().snapshots.get(file).cloned()
    }

    fn file_text(&self, file: &FileId) -> Option<Arc<str>> {
        if let Some(snapshot) = self.source(file) {
            return Some(Arc::from(snapshot.text()));
        }
        let state = self.read();
        let slot = state.projects.get(file.project())?;
        let path = ProjectPath::new(file.path()).ok()?;
        slot.project.read(&path).ok().map(Arc::from)
    }

    fn view_components(&self, wxml: &ProjectPath) -> Vec<ViewComponent> {
        let state = self.read();
        let Some(project) = active_project_id(&state) else {
            return Vec::new();
        };
        view_components_in_slot(&state, project, wxml)
    }

    fn view_components_for(&self, file: &FileId) -> Vec<ViewComponent> {
        let Ok(path) = ProjectPath::new(file.path()) else {
            return Vec::new();
        };
        let state = self.read();
        view_components_in_slot(&state, file.project(), &path)
    }

    #[allow(clippy::too_many_lines)] // one linear analysis pipeline
    fn analysis(&self, snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        let Ok(path) = ProjectPath::new(snapshot.file().path()) else {
            let parsed = wxml_syntax::parse(snapshot.text());
            return Ok(syntax_only_view(
                snapshot,
                syntax_diagnostics(&parsed, snapshot),
                "document path is not a project path",
            ));
        };
        let project_id = snapshot.file().project().clone();

        // Fast path: the per-project cache already contains this snapshot.
        {
            let state = self.read();
            if let Some(slot) = state.projects.get(&project_id) {
                if !slot.config_stale {
                    if let Some(view) = slot.views.get(&path) {
                        if let Some(cache) = slot.prepared.iter().find(|cache| {
                            cache.key.path == path
                                && cache.key.snapshot == snapshot.snapshot()
                                && cache.key.view_revision == view.view_revision
                                && cache.key.project_revision == slot.project_revision
                        }) {
                            return Ok((*cache.view).clone());
                        }
                    }
                }
            }
        }

        let parsed = wxml_syntax::parse(snapshot.text());
        let syntax = syntax_diagnostics(&parsed, snapshot);

        // Load an inactive app once, including a shared tsconfig that was
        // created after startup; never change foreground identity here.
        let needs_load = {
            let state = self.read();
            state
                .projects
                .get(&project_id)
                .is_none_or(|slot| slot.config_stale)
        };
        if needs_load {
            let mut state = self.write();
            let needs_load = state
                .projects
                .get(&project_id)
                .is_none_or(|slot| slot.config_stale);
            if needs_load {
                let app = state
                    .app_roots
                    .iter()
                    .find(|app| project_id_for(&app.app_dir) == project_id)
                    .cloned();
                let Some(app) = app else {
                    return Ok(syntax_only_view(
                        snapshot,
                        syntax,
                        "no project root is open",
                    ));
                };
                if ensure_loaded(&mut state, &app, false).is_err() {
                    return Ok(syntax_only_view(
                        snapshot,
                        syntax,
                        "no project root is open",
                    ));
                }
            }
        }

        let (view, key) = {
            let state = self.read();
            let Some(slot) = state.projects.get(&project_id) else {
                return Ok(syntax_only_view(
                    snapshot,
                    syntax,
                    "no project root is open",
                ));
            };
            let Some(view_state) = slot.views.get(&path) else {
                return Ok(syntax_only_view(
                    snapshot,
                    syntax,
                    "template is not a discovered page or component",
                ));
            };
            let key = PreparedCacheKey {
                path: path.clone(),
                snapshot: snapshot.snapshot(),
                view_revision: view_state.view_revision,
                project_revision: slot.project_revision.clone(),
            };
            if let Some(cache) = slot.prepared.iter().find(|cache| cache.key == key) {
                return Ok((*cache.view).clone());
            }
            let prepared = build_prepared(
                slot,
                &project_id,
                &path,
                view_state,
                snapshot,
                &parsed,
                syntax,
            );
            (Arc::new(prepared), key)
        };

        {
            let mut state = self.write();
            if let Some(slot) = state.projects.get_mut(&project_id) {
                let current = slot.views.get(&path).map(|view| view.view_revision);
                if current == Some(key.view_revision)
                    && slot.project_revision == key.project_revision
                {
                    slot.prepared.retain(|cache| cache.key != key);
                    slot.prepared.insert(
                        0,
                        PreparedCacheEntry {
                            key,
                            view: Arc::clone(&view),
                        },
                    );
                    slot.prepared.truncate(MAX_PREPARED_CACHE);
                }
            }
        }
        Ok((*view).clone())
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
        let (project, path) = route_project(&mut state, uri)?;
        mark_shared_config_stale(&mut state, &project, &path);
        let revision = state.next_overlay_revision.saturating_add(1);
        state.next_overlay_revision = revision;
        state
            .overlays
            .insert(uri.to_owned(), (text.to_owned(), revision));
        let (accepted, is_json) = {
            let slot = state.projects.get(&project).ok_or(HostError::NoProject)?;
            (
                slot.project.accepts_overlay(&path),
                slot.project.accepts_json_overlay(&path),
            )
        };
        if !accepted {
            return Ok(HostUpdate::default());
        }
        let before = dirty_for_source(&state, &project, &path);
        {
            let slot = state
                .projects
                .get_mut(&project)
                .ok_or(HostError::NoProject)?;
            slot.project
                .set_overlay(&path, text.to_owned(), revision)
                .map_err(|_| HostError::Read)?;
            if is_json {
                // Full rediscovery per JSON overlay is intentional for
                // correctness; incremental discovery is deferred (M6).
                slot.project.rediscover();
            }
        }
        let mut dirty = before;
        if is_json {
            rebuild_slot_views(&mut state, &project);
            dirty.extend(dirty_for_source(&state, &project, &path));
        }
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(project_revision),
        })
    }

    /// Removes an overlay when the client closes a tracked document.
    fn close_overlay(&self, uri: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let (project, path) = route_project(&mut state, uri)?;
        mark_shared_config_stale(&mut state, &project, &path);
        state.overlays.remove(uri);
        let (accepted, is_json) = {
            let slot = state.projects.get(&project).ok_or(HostError::NoProject)?;
            (
                slot.project.accepts_overlay(&path),
                slot.project.accepts_json_overlay(&path),
            )
        };
        if !accepted {
            return Ok(HostUpdate::default());
        }
        let before = dirty_for_source(&state, &project, &path);
        {
            let slot = state
                .projects
                .get_mut(&project)
                .ok_or(HostError::NoProject)?;
            slot.project.remove_overlay(&path);
            if is_json {
                slot.project.rediscover();
            }
        }
        let mut dirty = before;
        if is_json {
            rebuild_slot_views(&mut state, &project);
            dirty.extend(dirty_for_source(&state, &project, &path));
        }
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(project_revision),
        })
    }

    /// Handles a saved script/JSON document.
    ///
    /// Overlays are kept because the client still holds the buffer; only the
    /// project state that depends on the saved file is refreshed.
    fn save_overlay(&self, uri: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let (project, path) = route_project(&mut state, uri)?;
        mark_shared_config_stale(&mut state, &project, &path);
        let (accepted, is_json, is_project_json) = {
            let slot = state.projects.get(&project).ok_or(HostError::NoProject)?;
            (
                slot.project.accepts_overlay(&path),
                slot.project.accepts_json_overlay(&path),
                slot.project.is_project_json(&path),
            )
        };
        if !accepted && !is_project_json {
            return Ok(HostUpdate::default());
        }
        bump_file_version(&mut state, &project, &path);
        if is_json || is_project_json {
            let before = dirty_for_source(&state, &project, &path);
            {
                let slot = state
                    .projects
                    .get_mut(&project)
                    .ok_or(HostError::NoProject)?;
                slot.project.rediscover();
            }
            rebuild_slot_views(&mut state, &project);
            let mut dirty = before;
            dirty.extend(dirty_for_source(&state, &project, &path));
            let dirty = mark_views_dirty(&mut state, &project, dirty);
            let project_revision = bump_slot_revision(&mut state, &project);
            return Ok(HostUpdate {
                dirty_views: dirty,
                project_revision: Some(project_revision),
            });
        }
        let dirty = state
            .projects
            .get(&project)
            .and_then(|slot| slot.dependencies.affected(&path))
            .unwrap_or_default();
        if dirty.is_empty() {
            // A saved script that no view depends on has no observable effect.
            return Ok(HostUpdate::default());
        }
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(project_revision),
        })
    }

    /// Applies `workspace/didChangeWatchedFiles`.
    ///
    /// The protocol groups changes by owning app and activates that app before
    /// calling here. Discovery runs at most once per notification, and known
    /// dependency edges select views precisely; unknown files refresh only the
    /// active project, never another app.
    #[allow(clippy::too_many_lines, clippy::unnecessary_wraps)] // trait contract
    fn watched_changes(&self, changes: &[WatchedFileChange]) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let Some(project) = active_project_id(&state).cloned() else {
            return Ok(HostUpdate::default());
        };
        let Some(slot) = state.projects.get(&project) else {
            return Ok(HostUpdate::default());
        };
        let mut json_paths = Vec::new();
        let mut script_paths = Vec::new();
        let mut wxml_paths = Vec::new();
        let mut ignored = 0_usize;
        for change in changes {
            let Ok(path) =
                wxml_project::files::project_path_from_uri(&slot.app.project_root, &change.uri)
            else {
                ignored = ignored.saturating_add(1);
                continue;
            };
            let is_json = slot.project.accepts_json_overlay(&path);
            let is_project_json = !is_json && slot.project.is_project_json(&path);
            let is_script = slot.project.accepts_script_overlay(&path);
            let is_wxml = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wxml"));
            if is_json || is_project_json {
                json_paths.push((path, change.kind, change.uri.clone()));
            } else if is_script {
                script_paths.push((path, change.kind, change.uri.clone()));
            } else if is_wxml {
                wxml_paths.push((path, change.kind, change.uri.clone()));
            } else {
                ignored = ignored.saturating_add(1);
            }
        }
        if json_paths.is_empty() && script_paths.is_empty() && wxml_paths.is_empty() {
            if ignored > 0 {
                crate::server::log(format_args!(
                    "ignoring {ignored} watched file change(s) that do not affect analysis"
                ));
            }
            return Ok(HostUpdate::default());
        }

        let app_json = slot.project.app_json().clone();
        let tsconfig_paths: Vec<ProjectPath> =
            slot.project.tsconfig().map_or_else(Vec::new, |config| {
                std::iter::once(config.path.clone())
                    .chain(config.extends.iter().cloned())
                    .collect()
            });
        let full_refresh = json_paths.iter().any(|(path, _, _)| {
            *path == app_json
                || tsconfig_paths.contains(path)
                || (!slot.project.accepts_json_overlay(path) && slot.project.is_project_json(path))
        }) || script_paths.iter().any(|(_, kind, _)| {
            matches!(
                kind,
                WatchedChangeKind::Created | WatchedChangeKind::Deleted
            )
        }) || wxml_paths.iter().any(|(_, kind, _)| {
            matches!(
                kind,
                WatchedChangeKind::Created | WatchedChangeKind::Deleted
            )
        });
        for (path, _, _) in &json_paths {
            mark_shared_config_stale(&mut state, &project, path);
        }

        let mut dirty = Vec::new();
        if full_refresh {
            dirty.extend(all_slot_views(&state, &project));
        } else {
            for (path, _, _) in json_paths
                .iter()
                .chain(script_paths.iter())
                .chain(wxml_paths.iter())
            {
                dirty.extend(dirty_for_source(&state, &project, path));
            }
        }

        // Drop authoritative overlays for closed documents before refreshing
        // the project from disk.
        for (path, _, uri) in json_paths
            .iter()
            .chain(script_paths.iter())
            .chain(wxml_paths.iter())
        {
            state.overlays.remove(uri);
            if let Some(slot) = state.projects.get_mut(&project) {
                slot.project.remove_overlay(path);
            }
        }
        if !json_paths.is_empty() || full_refresh {
            if let Some(slot) = state.projects.get_mut(&project) {
                slot.project.rediscover();
            }
            rebuild_slot_views(&mut state, &project);
            if full_refresh {
                dirty.extend(all_slot_views(&state, &project));
            } else {
                for (path, _, _) in json_paths
                    .iter()
                    .chain(script_paths.iter())
                    .chain(wxml_paths.iter())
                {
                    dirty.extend(dirty_for_source(&state, &project, path));
                }
            }
        }
        for (path, _, _) in json_paths
            .iter()
            .chain(script_paths.iter())
            .chain(wxml_paths.iter())
        {
            bump_file_version(&mut state, &project, path);
        }
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        if ignored > 0 {
            crate::server::log(format_args!(
                "ignoring {ignored} watched file change(s) that do not affect analysis"
            ));
        }
        if dirty.is_empty() {
            return Ok(HostUpdate::default());
        }
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(project_revision),
        })
    }
}

const fn map_uri_error(_error: UriError) -> HostError {
    HostError::InvalidUri
}

/// Marks other apps sharing a changed tsconfig project root for reload.
fn mark_shared_config_stale(state: &mut State, project: &ProjectId, path: &ProjectPath) {
    let root = {
        let Some(slot) = state.projects.get(project) else {
            return;
        };
        let is_tsconfig = path.as_str() == "tsconfig.json"
            || slot
                .project
                .tsconfig()
                .is_some_and(|config| config.path == *path || config.extends.contains(path));
        if !is_tsconfig {
            return;
        }
        slot.app.project_root.clone()
    };
    for (id, slot) in &mut state.projects {
        if id != project && slot.app.project_root == root {
            slot.config_stale = true;
            slot.prepared.clear();
        }
    }
}

/// Resolves and loads the project that owns `uri`.
fn route_project(state: &mut State, uri: &str) -> Result<(ProjectId, ProjectPath), HostError> {
    let app = app_for_uri(state, uri)?.clone();
    let project = ensure_loaded(state, &app, true)?;
    let path = {
        let slot = state.projects.get(&project).ok_or(HostError::ProjectOpen)?;
        slot.project.path_from_uri(uri).map_err(map_uri_error)?
    };
    state.active_project = Some(project.clone());
    Ok((project, path))
}

/// Loads or activates one app project without scanning directories.
fn ensure_loaded(state: &mut State, app: &AppRoot, activate: bool) -> Result<ProjectId, HostError> {
    let project_id = project_id_for(&app.app_dir);
    if let Some(slot) = state.projects.get(&project_id) {
        if slot.app.project_root == app.project_root
            && slot.app.app_json == app.app_json
            && !slot.config_stale
        {
            if activate {
                state.active_project = Some(project_id.clone());
            }
            return Ok(project_id);
        }
    }
    let project = open_app(state, app)?;
    let mut next_view_revision = state.next_view_revision;
    let views = {
        let previous = state.projects.get(&project_id).map(|slot| &slot.views);
        build_views(&project, &project_id, &mut next_view_revision, previous)
    };
    let dependencies = DependencyIndex::build(&project);
    state.next_view_revision = next_view_revision;
    let revision_seq = state
        .projects
        .get(&project_id)
        .map_or(1, |slot| slot.revision_seq.saturating_add(1));
    let project_revision = ProjectRevision::new(project_id.clone(), revision_seq);
    let file_versions = state
        .projects
        .get(&project_id)
        .map(|slot| slot.file_versions.clone())
        .unwrap_or_default();
    state.projects.insert(
        project_id.clone(),
        ProjectSlot {
            app: app.clone(),
            project,
            project_revision,
            revision_seq,
            views,
            dependencies,
            file_versions,
            prepared: Vec::new(),
            config_stale: false,
        },
    );
    if activate {
        state.active_project = Some(project_id.clone());
    }
    Ok(project_id)
}

/// Opens an app with its effective buffers without changing foreground routing.
fn open_app(state: &State, candidate: &AppRoot) -> Result<Project, HostError> {
    let mut project =
        Project::open_with_app_json(&candidate.project_root, candidate.app_json.clone())
            .map_err(|_| HostError::ProjectOpen)?;
    let mut pending: Vec<_> = state
        .overlays
        .iter()
        .filter_map(|(uri, overlay)| project.path_from_uri(uri).ok().map(|path| (path, overlay)))
        .collect();
    loop {
        let mut rediscover = false;
        for (path, overlay) in std::mem::take(&mut pending) {
            if !project.accepts_overlay(&path) {
                pending.push((path, overlay));
                continue;
            }
            let (text, revision) = overlay;
            project
                .set_overlay(&path, text.clone(), *revision)
                .map_err(|_| HostError::Read)?;
            rediscover |= project.accepts_json_overlay(&path);
        }
        if !rediscover {
            break;
        }
        // Each pass consumes at least one JSON overlay. Rediscovery can expose
        // further page/component configs, so replay those before finishing.
        project.rediscover();
    }
    Ok(project)
}

/// Selects the configured app root that contains `uri`.
fn app_for_uri<'a>(state: &'a State, uri: &str) -> Result<&'a AppRoot, HostError> {
    let path = wxml_project::root_path_from_uri(uri).map_err(|_| HostError::InvalidUri)?;
    if state.app_roots.is_empty() {
        return Err(HostError::NoProject);
    }
    if let Some(candidate) = state
        .app_roots
        .iter()
        .filter(|candidate| path.starts_with(&candidate.app_dir))
        .max_by_key(|candidate| candidate.app_dir.as_os_str().len())
    {
        return Ok(candidate);
    }
    // A tsconfig or other file may live above `app.json` but inside the loaded
    // TypeScript project. Keep the active project for it.
    if let Some(active) = state.active_project.as_ref() {
        if let Some(app) = state
            .app_roots
            .iter()
            .find(|app| &project_id_for(&app.app_dir) == active)
        {
            if path.starts_with(&app.project_root) {
                return Ok(app);
            }
        }
    }
    let mut roots = state
        .app_roots
        .iter()
        .filter(|candidate| path.starts_with(&candidate.project_root));
    let candidate = roots.next().ok_or(HostError::InvalidUri)?;
    if roots.next().is_some() {
        return Err(HostError::InvalidUri);
    }
    Ok(candidate)
}

fn project_id_for(root: &Path) -> ProjectId {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in root.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    ProjectId::new(format!("proj-{hash:016x}"))
}

fn build_views(
    project: &Project,
    project_id: &ProjectId,
    next_view_revision: &mut u64,
    previous: Option<&BTreeMap<ProjectPath, ViewState>>,
) -> BTreeMap<ProjectPath, ViewState> {
    let mut views = BTreeMap::new();
    for page in project.pages() {
        insert_view(
            next_view_revision,
            previous,
            &mut views,
            page,
            RootKind::Page,
            project_id,
        );
    }
    for component in project.components() {
        insert_view(
            next_view_revision,
            previous,
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
    previous: Option<&BTreeMap<ProjectPath, ViewState>>,
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
    // Unaffected views keep their revision across rediscovery so a page JSON
    // change does not invalidate every cached analysis in the project.
    let revision = previous
        .and_then(|previous| previous.get(wxml))
        .map_or_else(
            || {
                let revision = ViewRevision::new(*next_view_revision);
                *next_view_revision = next_view_revision.saturating_add(1);
                revision
            },
            |view| view.view_revision,
        );
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
        self.config
            .iter()
            .filter(|(_, component)| component.kind == ComponentTargetKind::Local)
            .filter_map(|(name, component)| {
                component
                    .component
                    .as_ref()
                    .map(|path| (name.clone(), path.clone()))
            })
            .collect()
    }
}

impl ViewEntry for wxml_project::Component {
    fn wxml(&self) -> &ProjectPath {
        &self.wxml
    }

    fn script(&self) -> Option<&ProjectPath> {
        self.script.as_ref()
    }

    fn components(&self) -> BTreeMap<String, ProjectPath> {
        // `wxml-project` does not expose a `PageConfig` for components, so
        // component-internal `usingComponents` cannot be resolved here yet.
        BTreeMap::new()
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

/// Rebuilds discovery-derived views, preserving unaffected revisions.
fn rebuild_slot_views(state: &mut State, project: &ProjectId) {
    let Some(slot) = state.projects.get(project) else {
        return;
    };
    let mut next_view_revision = state.next_view_revision;
    let views = build_views(
        &slot.project,
        project,
        &mut next_view_revision,
        Some(&slot.views),
    );
    let dependencies = DependencyIndex::build(&slot.project);
    if let Some(slot) = state.projects.get_mut(project) {
        slot.views = views;
        slot.dependencies = dependencies;
        slot.prepared.clear();
    }
    state.next_view_revision = next_view_revision;
}

/// Returns views affected by one known source, or every view when unknown.
fn dirty_for_source(state: &State, project: &ProjectId, source: &ProjectPath) -> Vec<ProjectPath> {
    let Some(slot) = state.projects.get(project) else {
        return Vec::new();
    };
    slot.dependencies
        .affected(source)
        .unwrap_or_else(|| slot.views.keys().cloned().collect())
}

/// Returns all views of one loaded project.
fn all_slot_views(state: &State, project: &ProjectId) -> Vec<ProjectPath> {
    state
        .projects
        .get(project)
        .map(|slot| slot.views.keys().cloned().collect())
        .unwrap_or_default()
}

/// Advances the view revision of affected paths and returns the deduplicated
/// list, including paths whose view disappeared so open documents can clear
/// stale diagnostics.
fn mark_views_dirty(
    state: &mut State,
    project: &ProjectId,
    paths: Vec<ProjectPath>,
) -> Vec<ProjectPath> {
    let mut next_view_revision = state.next_view_revision;
    let mut dirty = Vec::new();
    if let Some(slot) = state.projects.get_mut(project) {
        slot.prepared.clear();
        for path in paths {
            if let Some(view) = slot.views.get_mut(&path) {
                view.view_revision = ViewRevision::new(next_view_revision);
                next_view_revision = next_view_revision.saturating_add(1);
            }
            dirty.push(path);
        }
    }
    state.next_view_revision = next_view_revision;
    dirty.sort();
    dirty.dedup();
    dirty
}

/// Advances one view revision without changing which files are dirty.
fn bump_view_revision(state: &mut State, project: &ProjectId, path: &ProjectPath) {
    let mut next_view_revision = state.next_view_revision;
    if let Some(slot) = state.projects.get_mut(project) {
        slot.prepared.clear();
        if let Some(view) = slot.views.get_mut(path) {
            view.view_revision = ViewRevision::new(next_view_revision);
            next_view_revision = next_view_revision.saturating_add(1);
        }
    }
    state.next_view_revision = next_view_revision;
}

/// Advances one project revision and returns it.
fn bump_slot_revision(state: &mut State, project: &ProjectId) -> ProjectRevision {
    if let Some(slot) = state.projects.get_mut(project) {
        slot.revision_seq = slot.revision_seq.saturating_add(1);
        slot.project_revision = ProjectRevision::new(project.clone(), slot.revision_seq);
        slot.prepared.clear();
        return slot.project_revision.clone();
    }
    ProjectRevision::new(project.clone(), 1)
}

/// Advances one disk file version observed through save/watch.
fn bump_file_version(state: &mut State, project: &ProjectId, path: &ProjectPath) {
    let version = state.next_file_version.saturating_add(1);
    state.next_file_version = version;
    if let Some(slot) = state.projects.get_mut(project) {
        slot.file_versions.insert(path.clone(), version);
        slot.prepared.clear();
    }
}

/// Returns the active project, or the only loaded project.
fn active_project_id(state: &State) -> Option<&ProjectId> {
    state
        .active_project
        .as_ref()
        .filter(|id| state.projects.contains_key(*id))
        .or_else(|| {
            (state.projects.len() == 1)
                .then(|| state.projects.keys().next())
                .flatten()
        })
}

/// Looks up a view in one explicit project.
fn view_in_slot(
    state: &State,
    project: &ProjectId,
    wxml: &ProjectPath,
) -> Result<ViewTarget, HostError> {
    let slot = state.projects.get(project).ok_or(HostError::Unsupported {
        reason: "project is not loaded",
    })?;
    let view = slot.views.get(wxml).ok_or(HostError::Unsupported {
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

/// Returns effective custom components for one view in one project.
fn view_components_in_slot(
    state: &State,
    project: &ProjectId,
    wxml: &ProjectPath,
) -> Vec<ViewComponent> {
    let Some(slot) = state.projects.get(project) else {
        return Vec::new();
    };
    let Some(config) = slot
        .project
        .pages()
        .iter()
        .find(|page| page.wxml == *wxml)
        .map(|page| &page.config)
    else {
        return Vec::new();
    };
    let mut components: Vec<ViewComponent> = config
        .iter()
        .map(|(name, usage)| ViewComponent {
            name: name.clone(),
            specifier: usage.specifier.clone(),
            component: usage.component.clone(),
        })
        .collect();
    components.sort_by(|left, right| left.name.cmp(&right.name));
    components
}

/// Builds one prepared view from an immutable slot snapshot.
#[allow(clippy::too_many_lines)] // one linear analysis pipeline
fn build_prepared(
    slot: &ProjectSlot,
    project_id: &ProjectId,
    path: &ProjectPath,
    view: &ViewState,
    snapshot: &SourceSnapshot,
    parsed: &ParseResult,
    mut syntax: Vec<Diagnostic>,
) -> PreparedView {
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
                    message: "template fragment limit reached; not all expressions were checked"
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
        return PreparedView {
            view_id: format!("{}:0", path.as_str()),
            target: None,
            files: Arc::from(Vec::new()),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(analysis.fragments),
            components: Arc::from(Vec::new()),
            events,
            project_root: slot.project.root().to_path_buf(),
            syntax_diagnostics: syntax,
            status_hint: Some(CapabilityStatus::Unsupported {
                reason: "page or component has no script".to_owned(),
            }),
        };
    };
    let script_file = view.script_file.clone();
    let view_revision = view.view_revision;
    let components = resolve_components(
        &slot.project,
        project_id,
        &view_components,
        analysis.components,
    );

    let sources = slot.project.collect_sources(SourceLimits::default());
    let mut files = Vec::new();
    for source in &sources.files {
        let version = slot
            .file_versions
            .get(&source.path)
            .copied()
            .map_or(source.source_revision, |version| {
                version.max(source.source_revision)
            });
        if let Some(file) = prepared_file(project_id, source, version) {
            files.push(file);
        }
    }
    let mut script_present = files.iter().any(|file| file.path == *script);
    if !script_present {
        match slot.project.read(script) {
            Ok(text) => {
                if let Some(file) = manual_file(project_id, script, &text) {
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
        slot.project.compiler_options(),
        slot.project.root(),
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

    PreparedView {
        view_id: format!("{}:{}", path.as_str(), view_revision.get()),
        target,
        files: Arc::from(files),
        compiler_options,
        fragments: Arc::from(analysis.fragments),
        components: Arc::from(components),
        events,
        project_root: slot.project.root().to_path_buf(),
        syntax_diagnostics: syntax,
        status_hint: hint,
    }
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

fn prepared_file(
    project_id: &ProjectId,
    source: &SourceFile,
    version: u64,
) -> Option<PreparedFile> {
    let file = FileId::new(project_id.clone(), source.path.as_str()).ok()?;
    Some(PreparedFile {
        file,
        path: source.path.clone(),
        kind: source_kind_str(source.kind),
        text: Arc::from(source.text.as_str()),
        source_revision: version,
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
    fn nested_app_root_requires_explicit_configuration() {
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
        assert_eq!(host.root().as_deref(), Some(root.as_path()));
        let wxml = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        assert!(
            host.view_for(&wxml).is_err(),
            "must not scan nested app.json"
        );
        host.open_root_with_options(
            Some(&format!("file://{}", root.display())),
            &InitializationOptions::parse(&serde_json::json!({
                "apps": [{"root": "apps/miniprogram/src"}]
            }))
            .expect("options"),
        )
        .expect("open configured root");
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
        host.open_root_with_options(
            Some(&format!("file://{}", root.display())),
            &InitializationOptions::parse(&serde_json::json!({
                "apps": [{"root": "apps/a/src"}, {"root": "apps/b/src"}]
            }))
            .expect("options"),
        )
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
