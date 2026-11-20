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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use thiserror::Error;
use wxml_project::script::RegistrationKind;
use wxml_project::{
    ComponentPropertyFacts, ComponentTargetKind, Page, Project, ProjectPath, ProjectSnapshot,
    ResourceCandidates, ResourceResolution, SourceFile, SourceKind, SourceLimits, StyleConfig,
    StyleDiagnosticsPolicy, StyleIndex, StyleIsolation, StyleScope, StyleViewFacts, UriError,
};

use crate::workspace_packages::WorkspacePackages;
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

/// Compiler options accepted by the native backend.
///
/// This mirrors `REQUEST_COMPILER_OPTION_KEYS` in the retired
/// `backends/typescript/src/protocol.ts`. Unknown keys are filtered here and
/// reported as a `Partial` hint instead of being forwarded, which would reject
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
#[non_exhaustive]
pub enum DocumentRoute {
    /// A WXML template in the project.
    Wxml(ProjectPath),
    /// A script in the project.
    Script(ProjectPath),
    /// A project JSON file (`app.json`, page/component JSON).
    Json(ProjectPath),
    /// A WXSS/SCSS/CSS style file.
    Style(ProjectPath),
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
    /// Views whose type analysis must be re-checked.
    pub dirty_views: Vec<ProjectPath>,
    /// Views whose local style/resource diagnostics must be refreshed without
    /// starting a type-backend request.
    pub style_views: Vec<ProjectPath>,
    /// New project revision, when the update changed project state.
    pub project_revision: Option<ProjectRevision>,
    /// App that owns the affected views, including style-only updates.
    ///
    /// Relative paths can repeat across explicitly configured apps, so the
    /// project identity is required to route local diagnostics precisely.
    pub project: Option<ProjectId>,
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

/// Generated directories that S must never propose edits for.
const EDIT_EXCLUDED_DIRECTORIES: [&str; 3] = ["generated", "__generated__", "gen"];

/// Maximum project WXML files returned for one semantic query.
const MAX_VIEW_TEMPLATES: usize = 512;

/// Maximum estimated bytes of project WXML text returned for one query.
const MAX_VIEW_TEMPLATE_BYTES: usize = 8 * 1024 * 1024;

/// One WXML file of the project, with its effective text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectViewTemplate {
    /// Project-bound file identity.
    pub file: FileId,
    /// Overlay-first text.
    pub text: Arc<str>,
}

/// Bounded project-wide WXML list used by references and workspace symbols.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectViewTemplates {
    /// Discovered page/component templates, sorted by path.
    pub templates: Vec<ProjectViewTemplate>,
    /// `false` when a count/byte budget or unreadable file hid templates.
    pub complete: bool,
    /// Short path-free reason for an incomplete list.
    pub reason: Option<&'static str>,
}

/// Maximum static event handlers forwarded to a type backend for one view.
///
/// Mirrors the C1 event budget: a large template must not turn into an
/// unbounded request or diagnostics list. Truncation is reported as a
/// [`CapabilityStatus::Partial`] hint.
pub const MAX_PREPARED_EVENTS: usize = 1024;

/// Maximum style fact entries kept per project.
const MAX_STYLE_VIEW_CACHE: usize = 256;

/// Maximum estimated class-reference bytes kept per project.
const MAX_STYLE_FACT_BYTES: usize = 8 * 1024 * 1024;

/// Maximum class references kept per project.
const MAX_STYLE_FACT_CLASSES: usize = 200_000;

/// Maximum project style caches retained in one host.
const MAX_STYLE_PROJECTS: usize = 4;

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
    /// `false` when a source file/count/byte budget removed a script from
    /// `files`; script symbol coverage can then never be complete.
    pub source_complete: bool,
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
    /// Returns every loaded project identity (not only the active one).
    fn project_ids(&self) -> Vec<ProjectId> {
        vec![self.project_id()]
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
    /// Returns the discovered view that owns `file` as its script.
    ///
    /// A script may belong to at most one page/component view; a script shared
    /// by several views has no unambiguous outline context and is rejected.
    /// Hosts without a project return [`HostError::Unsupported`].
    fn view_for_script(&self, _file: &FileId) -> Result<ViewTarget, HostError> {
        Err(HostError::Unsupported {
            reason: "script is not an owning page or component script",
        })
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
    /// Returns effective file size for pre-read limits, when the host can
    /// determine it cheaply. A simple host may return `None`.
    fn file_size(&self, _file: &FileId) -> Option<u64> {
        None
    }
    /// Returns effective text for any project file (overlay first, then disk).
    ///
    /// This is used to convert byte ranges in definition targets to LSP
    /// positions without opening the target document.
    fn file_text(&self, file: &FileId) -> Option<Arc<str>>;
    /// Returns every discovered WXML template of the project that owns
    /// `file`, in path order.
    ///
    /// Hosts without a project return `None`. The result is bounded and says
    /// whether it is complete; callers must not present an incomplete list as
    /// a complete reference set.
    fn view_templates(&self, _file: &FileId) -> Option<ProjectViewTemplates> {
        None
    }

    /// Returns the host's content revision for a file, when it tracks one.
    fn file_revision(&self, _file: &FileId) -> Option<u64> {
        None
    }

    /// Returns whether `file` is an existing, writable project file that is
    /// safe to edit.
    ///
    /// A host rejects package/build directories, declaration files, paths that
    /// escape the project through a symlink and files that are not inside the
    /// project root. Open buffers are validated separately by the workspace.
    fn writable_target(&self, _file: &FileId) -> bool {
        false
    }

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

    /// Project-bound variant of [`Self::component_properties`].
    fn component_properties_for(&self, file: &FileId, tag: &str) -> Option<ComponentPropertyFacts> {
        let path = ProjectPath::new(file.path()).ok()?;
        self.component_properties(&path, tag)
    }

    /// Returns the style class facts that apply to one WXML view.
    ///
    /// Hosts without a project return `None`; callers then keep their previous
    /// static behavior instead of inventing an empty style index.
    fn style_facts(&self, _wxml: &ProjectPath) -> Option<Arc<StyleViewFacts>> {
        None
    }

    /// Project-bound variant of [`Self::style_facts`].
    ///
    /// Paths are relative to their app root, so two apps can contain the same
    /// relative view path. Callers holding a URI must use this method to keep
    /// style lookup in the file's own project.
    fn style_facts_for(&self, file: &FileId) -> Option<Arc<StyleViewFacts>> {
        ProjectPath::new(file.path())
            .ok()
            .and_then(|path| self.style_facts(&path))
    }

    /// Resolves one local resource value for a WXML view.
    fn resource_resolution(
        &self,
        _wxml: &ProjectPath,
        _specifier: &str,
    ) -> Option<ResourceResolution> {
        None
    }

    /// Project-bound variant of [`Self::resource_resolution`].
    fn resource_resolution_for(
        &self,
        file: &FileId,
        specifier: &str,
    ) -> Option<ResourceResolution> {
        ProjectPath::new(file.path())
            .ok()
            .and_then(|path| self.resource_resolution(&path, specifier))
    }

    /// Lists one directory for resource completion.
    fn resource_candidates(
        &self,
        _wxml: &ProjectPath,
        _prefix: &str,
    ) -> Option<ResourceCandidates> {
        None
    }

    /// Project-bound variant of [`Self::resource_candidates`].
    fn resource_candidates_for(&self, file: &FileId, prefix: &str) -> Option<ResourceCandidates> {
        ProjectPath::new(file.path())
            .ok()
            .and_then(|path| self.resource_candidates(&path, prefix))
    }

    /// Applies style/resource settings from initialization options.
    fn configure_styles(&self, _config: StyleConfig, _diagnostics: StyleDiagnosticsPolicy) {}

    /// Returns local class/resource diagnostics without a type backend.
    fn style_diagnostics(&self, _snapshot: &SourceSnapshot) -> Vec<Diagnostic> {
        Vec::new()
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

#[derive(Clone, Debug)]
struct ViewState {
    owner: RootKind,
    script: Option<ProjectPath>,
    script_file: Option<FileId>,
    view_revision: ViewRevision,
    /// Local `usingComponents` entries: display tag to component base path.
    components: BTreeMap<String, ProjectPath>,
    /// The view was derived from an adjacent script instead of `app.json`.
    lazy: bool,
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
    /// The project boundary is the whole workspace, not one app package.
    workspace_scoped: bool,
}

#[derive(Debug)]
struct State {
    /// Explicit app roots from `InitializationOptions::resolve`.
    app_roots: Vec<AppRoot>,
    /// Canonical workspace root used by zero-config lazy app discovery.
    workspace_root: Option<PathBuf>,
    /// Workspace package manifest cache for lazy app discovery.
    workspace_packages: Option<Arc<WorkspacePackages>>,
    /// Loaded apps keyed by stable, path-free project identity.
    projects: BTreeMap<ProjectId, ProjectSlot>,
    /// App that owns foreground/path-only host queries.
    active_project: Option<ProjectId>,
    snapshots: HashMap<FileId, Arc<SourceSnapshot>>,
    /// Authoritative unsaved text keyed by client URI.
    overlays: BTreeMap<String, (String, u64)>,
    /// Parsed component properties keyed by project and script revision.
    property_cache: BTreeMap<(ProjectId, ProjectPath), (u64, ComponentPropertyFacts)>,
    next_view_revision: u64,
    next_overlay_revision: u64,
    next_file_version: u64,
    /// Increments whenever discovery replaces the view table; style facts
    /// from the previous discovery are invalid.
    style_epoch: u64,
}

/// Settings shared by every project style cache.
#[derive(Clone, Debug, Default)]
struct StyleSettings {
    config: StyleConfig,
    diagnostics: StyleDiagnosticsPolicy,
}

/// Parsed style sheets and class facts for one project identity.
#[derive(Debug, Default)]
struct ProjectStyleCache {
    epoch: u64,
    index: StyleIndex,
    facts: BTreeMap<ProjectPath, Arc<StyleViewFacts>>,
    order: VecDeque<ProjectPath>,
    fact_classes: usize,
    fact_bytes: usize,
}

impl ProjectStyleCache {
    /// Drops every class-fact entry but keeps parsed sheets.
    fn clear_facts(&mut self) {
        self.facts.clear();
        self.order.clear();
        self.fact_classes = 0;
        self.fact_bytes = 0;
    }

    /// Removes one cached view and updates the budget counters.
    fn remove_fact(&mut self, view: &ProjectPath) {
        if let Some(previous) = self.facts.remove(view) {
            self.fact_classes = self.fact_classes.saturating_sub(previous.classes.len());
            self.fact_bytes = self.fact_bytes.saturating_sub(previous.estimated_bytes());
        }
        self.order.retain(|entry| entry != view);
    }

    /// Inserts one view's facts within the cache budgets.
    fn insert(&mut self, view: ProjectPath, facts: Arc<StyleViewFacts>) {
        if let Some(previous) = self.facts.remove(&view) {
            self.fact_classes = self.fact_classes.saturating_sub(previous.classes.len());
            self.fact_bytes = self.fact_bytes.saturating_sub(previous.estimated_bytes());
            self.order.retain(|entry| entry != &view);
        }
        let classes = facts.classes.len();
        let bytes = facts.estimated_bytes();
        if classes > MAX_STYLE_FACT_CLASSES || bytes > MAX_STYLE_FACT_BYTES {
            return;
        }
        while self.order.len() >= MAX_STYLE_VIEW_CACHE
            || self.fact_classes.saturating_add(classes) > MAX_STYLE_FACT_CLASSES
            || self.fact_bytes.saturating_add(bytes) > MAX_STYLE_FACT_BYTES
        {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.facts.remove(&evicted) {
                self.fact_classes = self.fact_classes.saturating_sub(old.classes.len());
                self.fact_bytes = self.fact_bytes.saturating_sub(old.estimated_bytes());
            }
        }
        self.fact_classes = self.fact_classes.saturating_add(classes);
        self.fact_bytes = self.fact_bytes.saturating_add(bytes);
        self.order.push_back(view.clone());
        self.facts.insert(view, facts);
    }
}

/// Per-project style caches plus the language-server settings.
#[derive(Debug, Default)]
struct StyleStore {
    settings: StyleSettings,
    projects: BTreeMap<ProjectId, ProjectStyleCache>,
    order: VecDeque<ProjectId>,
}

impl StyleStore {
    /// Replaces settings and drops every project cache.
    fn configure(&mut self, config: StyleConfig, diagnostics: StyleDiagnosticsPolicy) {
        self.settings = StyleSettings {
            config,
            diagnostics,
        };
        self.projects.clear();
        self.order.clear();
    }

    /// Returns the current diagnostic settings.
    const fn diagnostics(&self) -> StyleDiagnosticsPolicy {
        self.settings.diagnostics
    }

    /// Drops every project cache.
    fn clear_all(&mut self) {
        self.projects.clear();
        self.order.clear();
    }

    /// Invalidates one changed style file and the cached facts that used it.
    ///
    /// Returns the views whose facts were dropped, so callers can republish
    /// only affected templates instead of broadcasting every open WXML view.
    fn invalidate_path(&mut self, project_id: &ProjectId, path: &ProjectPath) -> Vec<ProjectPath> {
        let Some(cache) = self.projects.get_mut(project_id) else {
            return Vec::new();
        };
        cache.index.invalidate(path);
        let affected: Vec<ProjectPath> = cache
            .facts
            .iter()
            .filter(|(_, facts)| facts.files.iter().any(|file| &file.path == path))
            .map(|(view, _)| view.clone())
            .collect();
        for view in &affected {
            cache.remove_fact(view);
        }
        affected
    }

    /// Drops class facts after discovery/configuration changed.
    fn clear_facts(&mut self, project_id: &ProjectId) {
        if let Some(cache) = self.projects.get_mut(project_id) {
            cache.clear_facts();
        }
    }

    /// Returns one project's cache, creating it within the project budget.
    fn project_mut(&mut self, project_id: &ProjectId) -> &mut ProjectStyleCache {
        if !self.projects.contains_key(project_id) {
            while self.projects.len() >= MAX_STYLE_PROJECTS {
                let Some(evicted) = self.order.pop_front() else {
                    break;
                };
                self.projects.remove(&evicted);
            }
            self.order.push_back(project_id.clone());
            self.projects
                .insert(project_id.clone(), ProjectStyleCache::default());
        } else if self.order.back() != Some(project_id) {
            self.order.retain(|entry| entry != project_id);
            self.order.push_back(project_id.clone());
        }
        self.projects.entry(project_id.clone()).or_default()
    }

    /// Returns facts for one view, building them outside the project lock.
    fn facts(
        &mut self,
        project_id: &ProjectId,
        snapshot: &ProjectSnapshot,
        view: &ProjectPath,
        scope: StyleScope,
        isolation: StyleIsolation,
        epoch: u64,
    ) -> Arc<StyleViewFacts> {
        let config = self.settings.config.clone();
        let cache = self.project_mut(project_id);
        if cache.epoch != epoch {
            // Discovery changed the view table; parsed style sheets remain
            // valid, only view-specific facts need rebuilding.
            cache.epoch = epoch;
            cache.clear_facts();
        }
        if let Some(facts) = cache.facts.get(view) {
            let facts = Arc::clone(facts);
            cache.order.retain(|entry| entry != view);
            cache.order.push_back(view.clone());
            return facts;
        }
        let facts = Arc::new(cache.index.view_facts_for(
            snapshot,
            view,
            scope,
            isolation,
            &config,
            Some(project_id.as_str()),
        ));
        cache.insert(view.clone(), Arc::clone(&facts));
        facts
    }
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
    styles: Mutex<StyleStore>,
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
            styles: Mutex::new(StyleStore::default()),
            state: RwLock::new(State {
                app_roots: Vec::new(),
                workspace_root: None,
                workspace_packages: None,
                projects: BTreeMap::new(),
                active_project: None,
                snapshots: HashMap::new(),
                overlays: BTreeMap::new(),
                property_cache: BTreeMap::new(),
                next_view_revision: 1,
                next_overlay_revision: 1,
                next_file_version: 1,
                style_epoch: 1,
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

    /// Locks the independent style cache.
    fn styles(&self) -> MutexGuard<'_, StyleStore> {
        self.styles.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Captures project identity, scope and an overlay snapshot under a short
    /// read lock. Style parsing happens after this returns.
    fn style_context(
        &self,
        wxml: &ProjectPath,
    ) -> Option<(ProjectSnapshot, ProjectId, StyleScope, StyleIsolation, u64)> {
        let project = self.ensure_view_project(wxml)?;
        self.style_context_in(&project, wxml)
    }

    /// Captures one explicitly identified project's style context.
    fn style_context_in(
        &self,
        project: &ProjectId,
        wxml: &ProjectPath,
    ) -> Option<(ProjectSnapshot, ProjectId, StyleScope, StyleIsolation, u64)> {
        let state = self.read();
        let slot = state.projects.get(project)?;
        if !slot.project.owns_wxml(wxml) {
            return None;
        }
        Some((
            slot.project.snapshot(),
            project.clone(),
            slot.project.style_scope(wxml),
            slot.project.style_isolation(wxml),
            state.style_epoch,
        ))
    }

    /// Builds style facts through one identified project.
    fn style_facts_in(
        &self,
        project: &ProjectId,
        wxml: &ProjectPath,
    ) -> Option<Arc<StyleViewFacts>> {
        let (snapshot, project_id, scope, isolation, epoch) =
            self.style_context_in(project, wxml)?;
        let facts = self
            .styles()
            .facts(&project_id, &snapshot, wxml, scope, isolation, epoch);
        // External `~class` / `^class` references live in the open template,
        // not in the style chain. Keep the cached index complete but return a
        // partial view to consumers that must not claim rename/reference
        // completeness.
        let file = FileId::new(project_id, wxml.as_str()).ok();
        let external = file.and_then(|file| {
            self.read()
                .snapshots
                .get(&file)
                .map(|snapshot| crate::styles::has_external_class_reference(snapshot.text()))
        });
        if external == Some(true) && facts.complete {
            let mut partial = (*facts).clone();
            partial.complete = false;
            partial.reason = Some(
                "template uses external ~class/^class references; caller styles are unknown"
                    .to_owned(),
            );
            return Some(Arc::new(partial));
        }
        Some(facts)
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
        state.workspace_root = None;
        state.workspace_packages = None;
        state.projects.clear();
        state.active_project = None;
        state.snapshots.clear();
        state.overlays.clear();
        state.next_view_revision = 1;
        state.next_overlay_revision = 1;
        state.next_file_version = 1;
        state.property_cache.clear();
        let Some(uri) = root_uri else {
            if !options.apps.is_empty() {
                return Err(ConfigurationError::MissingWorkspace.into());
            }
            return Ok(());
        };
        let root = wxml_project::root_path_from_uri(uri).map_err(|_| HostError::ProjectOpen)?;
        let root = std::fs::canonicalize(root).map_err(|_| HostError::ProjectOpen)?;
        if !root.is_dir() {
            return Err(HostError::ProjectOpen);
        }
        if options.apps.is_empty() {
            state.workspace_root = Some(root);
            state.style_epoch = state.style_epoch.saturating_add(1);
            self.styles().clear_all();
            return Ok(());
        }
        let roots = options.resolve(&root)?;
        state.app_roots.clone_from(&roots);
        state.style_epoch = state.style_epoch.saturating_add(1);
        self.styles().clear_all();
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
        if let Some(root) = &state.workspace_root {
            return Some(root.clone());
        }
        let project = active_project_id(&state)?;
        state
            .projects
            .get(project)
            .map(|slot| slot.app.project_root.clone())
    }
}

impl ProjectState {
    /// Resolves the project that owns one template, loading it lazily when the
    /// workspace was opened without explicit app roots.
    fn ensure_view_project(&self, wxml: &ProjectPath) -> Option<ProjectId> {
        let existing = {
            let state = self.read();
            let active = state
                .active_project
                .as_ref()
                .filter(|id| {
                    state.projects.get(*id).is_some_and(|slot| {
                        slot.views.contains_key(wxml) || slot.project.owns_wxml(wxml)
                    })
                })
                .cloned();
            active
                .or_else(|| {
                    state
                        .projects
                        .iter()
                        .find(|(_, slot)| slot.views.contains_key(wxml))
                        .map(|(id, _)| id.clone())
                })
                .or_else(|| {
                    state
                        .projects
                        .iter()
                        .find(|(_, slot)| slot.project.owns_wxml(wxml))
                        .map(|(id, _)| id.clone())
                })
        };
        if existing.is_some() {
            return existing;
        }
        let mut state = self.write();
        state.workspace_root.as_ref()?;
        let project = load_project_for_path(&mut state, wxml, true).ok()?;
        ensure_lazy_view(&mut state, &project, wxml);
        Some(project)
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
        let state = self.read();
        !state.app_roots.is_empty() || state.workspace_root.is_some()
    }

    fn project_ids(&self) -> Vec<ProjectId> {
        let state = self.read();
        let mut ids: Vec<ProjectId> = state.app_roots.iter().map(project_id_for).collect();
        ids.extend(state.projects.keys().cloned());
        ids.sort();
        ids.dedup();
        ids
    }

    fn path_for_uri(&self, uri: &str) -> Result<ProjectPath, HostError> {
        let mut state = self.write();
        let (_, path) = route_project(&mut state, uri)?;
        Ok(path)
    }

    fn resolve_file(&self, uri: &str) -> Result<FileId, HostError> {
        let mut state = self.write();
        let app = app_for_uri(&mut state, uri)?;
        let path = wxml_project::files::project_path_from_uri(&app.project_root, uri)
            .map_err(map_uri_error)?;
        FileId::new(project_id_for(&app), path.as_str()).map_err(|_| HostError::InvalidUri)
    }

    fn activate_project(&self, project: &ProjectId) -> Result<(), HostError> {
        let mut state = self.write();
        let candidate = state
            .app_roots
            .iter()
            .find(|app| &project_id_for(app) == project)
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
            .find(|app| project_id_for(app) == *file.project())?;
        wxml_project::files::uri_for_project_path(&app.project_root, &path)
    }

    fn file_id(&self, path: &ProjectPath) -> Result<FileId, HostError> {
        let mut state = self.write();
        if let Some(workspace) = state.workspace_root.clone() {
            let absolute = workspace.join(path.as_str());
            let packages = workspace_packages_for(&mut state, &workspace);
            let app = discover_app_for_path(&workspace, &absolute, &packages)?;
            cache_app(&mut state, app.clone());
            return FileId::new(project_id_for(&app), path.as_str())
                .map_err(|_| HostError::InvalidUri);
        }
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
            // WXS buffers are tracked as side documents so their unsaved text
            // reaches the static WXS analyzer; they are not part of the Node
            // script snapshot.
            Some("wxs") => DocumentRoute::Script(path),
            Some("json") => DocumentRoute::Json(path),
            Some("wxss" | "scss" | "css") => DocumentRoute::Style(path),
            _ => DocumentRoute::Unknown,
        }
    }

    fn view_for(&self, wxml: &ProjectPath) -> Result<ViewTarget, HostError> {
        let mut state = self.write();
        let project = load_project_for_path(&mut state, wxml, true)?;
        if !ensure_lazy_view(&mut state, &project, wxml) {
            return Err(HostError::Unsupported {
                reason: "template has no adjacent page or component script",
            });
        }
        view_in_slot(&state, &project, wxml)
    }

    fn view_for_file(&self, file: &FileId) -> Result<ViewTarget, HostError> {
        let path = ProjectPath::new(file.path()).map_err(|_| HostError::InvalidUri)?;
        let mut state = self.write();
        let project = if state.projects.contains_key(file.project()) {
            file.project().clone()
        } else {
            load_project_for_path(&mut state, &path, true)?
        };
        if !ensure_lazy_view(&mut state, &project, &path) {
            return Err(HostError::Unsupported {
                reason: "template has no adjacent page or component script",
            });
        }
        view_in_slot(&state, &project, &path)
    }

    fn view_for_script(&self, file: &FileId) -> Result<ViewTarget, HostError> {
        let script_path = ProjectPath::new(file.path()).map_err(|_| HostError::InvalidUri)?;
        let mut state = self.write();
        let project = if state.projects.contains_key(file.project()) {
            file.project().clone()
        } else {
            load_project_for_path(&mut state, &script_path, true)?
        };
        let find_owner = |state: &State| {
            let slot = state.projects.get(&project)?;
            let mut owners = slot
                .views
                .iter()
                .filter(|(_, view)| view.script_file.as_ref() == Some(file));
            let first = owners
                .next()
                .map(|(wxml, view)| (wxml.clone(), view.clone()));
            if owners.next().is_some() {
                return None;
            }
            first
        };
        let mut owner = find_owner(&state);
        if owner.is_none() {
            if state.projects.get(&project).is_some_and(|slot| {
                slot.views
                    .values()
                    .filter(|view| view.script_file.as_ref() == Some(file))
                    .count()
                    > 1
            }) {
                return Err(HostError::Unsupported {
                    reason: "script is shared by more than one page or component",
                });
            }
            if let Ok(wxml) = script_path.with_extension("wxml") {
                if state
                    .projects
                    .get(&project)
                    .is_some_and(|slot| slot.project.file_size(&wxml).is_some())
                    && ensure_lazy_view(&mut state, &project, &wxml)
                {
                    owner = find_owner(&state);
                }
            }
        }
        let (wxml, view) = owner.ok_or(HostError::Unsupported {
            reason: "script is not an owning page or component script",
        })?;
        let script = view.script.clone().ok_or(HostError::Unsupported {
            reason: "page or component has no script",
        })?;
        Ok(ViewTarget {
            wxml,
            owner: view.owner,
            script,
            script_file: file.clone(),
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

    fn file_size(&self, file: &FileId) -> Option<u64> {
        let path = ProjectPath::new(file.path()).ok()?;
        let mut state = self.write();
        if !state.projects.contains_key(file.project()) {
            load_project_for_id(&mut state, file.project(), &path, false).ok()?;
        }
        let slot = state.projects.get(file.project())?;
        slot.project.file_size(&path)
    }

    fn file_text(&self, file: &FileId) -> Option<Arc<str>> {
        if let Some(snapshot) = self.source(file) {
            return Some(Arc::from(snapshot.text()));
        }
        let path = ProjectPath::new(file.path()).ok()?;
        let mut state = self.write();
        if !state.projects.contains_key(file.project()) {
            load_project_for_id(&mut state, file.project(), &path, false).ok()?;
        }
        let slot = state.projects.get(file.project())?;
        slot.project.read(&path).ok().map(Arc::from)
    }

    fn view_templates(&self, file: &FileId) -> Option<ProjectViewTemplates> {
        let paths: Vec<ProjectPath> = {
            let state = self.read();
            let slot = state.projects.get(file.project())?;
            slot.views
                .keys()
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("wxml"))
                })
                .cloned()
                .collect()
        };
        let mut templates = Vec::new();
        let mut bytes = 0usize;
        let mut complete = true;
        for path in paths {
            if templates.len() >= MAX_VIEW_TEMPLATES {
                complete = false;
                break;
            }
            let Ok(target) = FileId::new(file.project().clone(), path.as_str()) else {
                complete = false;
                continue;
            };
            let Some(text) = self.file_text(&target) else {
                complete = false;
                continue;
            };
            bytes = bytes.saturating_add(text.len());
            if bytes > MAX_VIEW_TEMPLATE_BYTES {
                complete = false;
                break;
            }
            templates.push(ProjectViewTemplate { file: target, text });
        }
        templates.sort_by(|left, right| left.file.cmp(&right.file));
        Some(ProjectViewTemplates {
            templates,
            complete,
            reason: if complete {
                None
            } else {
                Some("the project WXML list is incomplete")
            },
        })
    }

    fn file_revision(&self, file: &FileId) -> Option<u64> {
        let state = self.read();
        let slot = state.projects.get(file.project())?;
        let path = ProjectPath::new(file.path()).ok()?;
        slot.file_versions
            .get(&path)
            .copied()
            .or_else(|| slot.project.overlay().revision(&path))
            .or(Some(1))
    }

    fn writable_target(&self, file: &FileId) -> bool {
        let root = {
            let state = self.read();
            let Some(slot) = state.projects.get(file.project()) else {
                return false;
            };
            slot.app.project_root.clone()
        };
        let Ok(path) = ProjectPath::new(file.path()) else {
            return false;
        };
        let lower = path.as_str().to_ascii_lowercase();
        if lower.ends_with(".d.ts") || lower.ends_with(".d.mts") || lower.ends_with(".d.cts") {
            return false;
        }
        if path.as_str().split('/').any(|component| {
            wxml_project::sources::EXCLUDED_DIRECTORIES
                .iter()
                .chain(EDIT_EXCLUDED_DIRECTORIES.iter())
                .any(|excluded| component.eq_ignore_ascii_case(excluded))
        }) {
            return false;
        }
        let absolute = root.join(path.as_str());
        if std::fs::symlink_metadata(&absolute)
            .map_or(true, |metadata| metadata.file_type().is_symlink())
        {
            return false;
        }
        let Ok(canonical_root) = std::fs::canonicalize(&root) else {
            return false;
        };
        let Ok(canonical) = std::fs::canonicalize(&absolute) else {
            return false;
        };
        canonical.starts_with(canonical_root)
    }

    fn view_components(&self, wxml: &ProjectPath) -> Vec<ViewComponent> {
        let Some(project) = self.ensure_view_project(wxml) else {
            return Vec::new();
        };
        let state = self.read();
        view_components_in_slot(&state, &project, wxml)
    }

    fn view_components_for(&self, file: &FileId) -> Vec<ViewComponent> {
        let Ok(path) = ProjectPath::new(file.path()) else {
            return Vec::new();
        };
        let project = {
            let state = self.read();
            state
                .projects
                .contains_key(file.project())
                .then(|| file.project().clone())
        };
        let Some(project) = project.or_else(|| self.ensure_view_project(&path)) else {
            return Vec::new();
        };
        let state = self.read();
        view_components_in_slot(&state, &project, &path)
    }

    fn style_facts(&self, wxml: &ProjectPath) -> Option<Arc<StyleViewFacts>> {
        let (_, project_id, _, _, _) = self.style_context(wxml)?;
        self.style_facts_in(&project_id, wxml)
    }

    fn style_facts_for(&self, file: &FileId) -> Option<Arc<StyleViewFacts>> {
        let path = ProjectPath::new(file.path()).ok()?;
        if !self.read().projects.contains_key(file.project()) {
            self.ensure_view_project(&path)?;
        }
        self.style_facts_in(file.project(), &path)
    }

    fn resource_resolution(
        &self,
        wxml: &ProjectPath,
        specifier: &str,
    ) -> Option<ResourceResolution> {
        let (snapshot, _, _, _, _) = self.style_context(wxml)?;
        Some(snapshot.resolve_resource(wxml, specifier))
    }

    fn resource_resolution_for(
        &self,
        file: &FileId,
        specifier: &str,
    ) -> Option<ResourceResolution> {
        let path = ProjectPath::new(file.path()).ok()?;
        let (snapshot, _, _, _, _) = self.style_context_in(file.project(), &path)?;
        Some(snapshot.resolve_resource(&path, specifier))
    }

    fn resource_candidates(&self, wxml: &ProjectPath, prefix: &str) -> Option<ResourceCandidates> {
        let (snapshot, _, _, _, _) = self.style_context(wxml)?;
        Some(snapshot.resource_candidates(wxml, prefix))
    }

    fn resource_candidates_for(&self, file: &FileId, prefix: &str) -> Option<ResourceCandidates> {
        let path = ProjectPath::new(file.path()).ok()?;
        let (snapshot, _, _, _, _) = self.style_context_in(file.project(), &path)?;
        Some(snapshot.resource_candidates(&path, prefix))
    }

    fn configure_styles(&self, config: StyleConfig, diagnostics: StyleDiagnosticsPolicy) {
        self.styles().configure(config, diagnostics);
    }

    fn style_diagnostics(&self, snapshot: &SourceSnapshot) -> Vec<Diagnostic> {
        let policy = self.styles().diagnostics();
        if !policy.resources && !policy.classes {
            return Vec::new();
        }
        let Ok(path) = ProjectPath::new(snapshot.file().path()) else {
            return Vec::new();
        };
        if !self.read().projects.contains_key(snapshot.file().project()) {
            self.ensure_view_project(&path);
        }
        let Some((project, _, _, _, _)) = self.style_context_in(snapshot.file().project(), &path)
        else {
            return Vec::new();
        };
        let facts = if policy.classes {
            self.style_facts_for(snapshot.file())
        } else {
            None
        };
        let mut diagnostics = Vec::new();
        if let (true, Some(facts)) = (policy.classes, facts.as_ref()) {
            if facts.complete && !crate::styles::has_external_class_reference(snapshot.text()) {
                let defined: std::collections::BTreeSet<&str> = facts
                    .classes
                    .iter()
                    .map(wxml_project::ClassDefinition::name)
                    .collect();
                for class in crate::styles::class_tokens(snapshot.text()) {
                    if defined.contains(class.name.as_str()) {
                        continue;
                    }
                    let Ok(text_range) =
                        wxml_text::TextRange::new(class.token.start(), class.token.end())
                    else {
                        continue;
                    };
                    let Some(range) = snapshot.source_range(text_range) else {
                        continue;
                    };
                    diagnostics.push(Diagnostic {
                        range,
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "class `{}` is not defined in the indexed styles",
                            class.name
                        ),
                        code: Some("style.unknown-class".to_owned()),
                    });
                }
            }
        }
        if policy.resources {
            let mut resolved = 0_usize;
            for site in crate::styles::resource_sites(
                snapshot.text(),
                crate::styles::MAX_WXML_RESOURCE_SITES,
            ) {
                if resolved >= crate::styles::MAX_RESOURCE_RESOLUTIONS {
                    break;
                }
                resolved = resolved.saturating_add(1);
                if !crate::styles::looks_like_path(&site.text) {
                    continue;
                }
                if matches!(
                    project.resolve_resource(&path, &site.text),
                    ResourceResolution::Missing
                ) {
                    let Ok(text_range) =
                        wxml_text::TextRange::new(site.value.start(), site.value.end())
                    else {
                        continue;
                    };
                    let Some(range) = snapshot.source_range(text_range) else {
                        continue;
                    };
                    diagnostics.push(Diagnostic {
                        range,
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "local {} target `{}` does not exist",
                            site.kind.label(),
                            crate::handlers::code_text(&site.text, 120)
                        ),
                        code: Some("style.missing-resource".to_owned()),
                    });
                }
            }
        }
        diagnostics
    }

    fn component_properties(
        &self,
        wxml: &ProjectPath,
        tag: &str,
    ) -> Option<ComponentPropertyFacts> {
        let state = self.read();
        let project = active_project_id(&state)?.clone();
        drop(state);
        self.component_properties_in(&project, wxml, tag)
    }

    fn component_properties_for(&self, file: &FileId, tag: &str) -> Option<ComponentPropertyFacts> {
        let path = ProjectPath::new(file.path()).ok()?;
        self.component_properties_in(file.project(), &path, tag)
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

        // Load the owning project when needed, then derive a view from the
        // adjacent script if discovery did not find one.
        {
            let mut state = self.write();
            let needs_load = state
                .projects
                .get(&project_id)
                .is_none_or(|slot| slot.config_stale);
            if needs_load && load_project_for_id(&mut state, &project_id, &path, false).is_err() {
                return Ok(syntax_only_view(
                    snapshot,
                    syntax,
                    "no project root is open",
                ));
            }
            if !ensure_lazy_view(&mut state, &project_id, &path) {
                return Ok(syntax_only_view(
                    snapshot,
                    syntax,
                    "template has no adjacent page or component script",
                ));
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
                    "template has no adjacent page or component script",
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
    /// Resolves component properties inside one app, honoring its overlay.
    fn component_properties_in(
        &self,
        project_id: &ProjectId,
        wxml: &ProjectPath,
        tag: &str,
    ) -> Option<ComponentPropertyFacts> {
        let mut state = self.write();
        let (script, revision, discovery_facts) = {
            let slot = state.projects.get(project_id)?;
            let project = &slot.project;
            let config = if let Some(page) = project.pages().iter().find(|page| page.wxml == *wxml)
            {
                &page.config
            } else {
                let component = project
                    .components()
                    .iter()
                    .find(|component| component.wxml == *wxml)?;
                &component.config
            };
            let usage = config.get(tag)?;
            if usage.kind == ComponentTargetKind::Plugin {
                return None;
            }
            let base = usage.component.as_ref()?;
            let component = project.component(base)?;
            let script = component.script.as_ref()?.clone();
            let Some(revision) = project.overlay().revision(&script) else {
                return Some(component.property_facts.clone());
            };
            (script, revision, component.property_facts.clone())
        };
        let key = (project_id.clone(), script.clone());
        if let Some((cached_revision, facts)) = state.property_cache.get(&key) {
            if *cached_revision == revision {
                return Some(facts.clone());
            }
        }
        let text = {
            let slot = state.projects.get(project_id)?;
            slot.project.read(&script).ok()?
        };
        let roots = wxml_project::extract_data_roots(&script, &text);
        let facts = wxml_project::extract_component_property_facts(&roots);
        if facts.complete || !facts.properties.is_empty() || discovery_facts.properties.is_empty() {
            state.property_cache.insert(key, (revision, facts.clone()));
        }
        Some(facts)
    }

    /// Applies a script or JSON overlay and reports affected views.
    ///
    /// Non-config JSON (for example `package.json`) and scripts in excluded
    /// directories are tracked by the workspace only, so they never change
    /// project state or the revision.
    fn overlay_update(&self, uri: &str, text: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let previous_active = state.active_project.clone();
        let (project, path) = route_project(&mut state, uri)?;
        mark_shared_config_stale(&mut state, &project, &path);
        let revision = state.next_overlay_revision.saturating_add(1);
        state.next_overlay_revision = revision;
        state
            .overlays
            .insert(uri.to_owned(), (text.to_owned(), revision));
        let (accepted, is_json, is_style) = {
            let slot = state.projects.get(&project).ok_or(HostError::NoProject)?;
            (
                slot.project.accepts_overlay(&path),
                slot.project.accepts_json_overlay(&path),
                slot.project.accepts_style_overlay(&path),
            )
        };
        if !accepted && !is_style {
            state.active_project = previous_active;
            return Ok(HostUpdate::default());
        }
        let before = if is_style {
            Vec::new()
        } else {
            dirty_for_source(&state, &project, &path)
        };
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
        state
            .property_cache
            .remove(&(project.clone(), path.clone()));
        let style_views = if is_json {
            rebuild_slot_views(&mut state, &project);
            dirty.extend(dirty_for_source(&state, &project, &path));
            self.styles().clear_facts(&project);
            all_slot_views(&state, &project)
        } else if is_style {
            self.styles().invalidate_path(&project, &path)
        } else {
            Vec::new()
        };
        if is_style {
            return Ok(HostUpdate {
                dirty_views: Vec::new(),
                style_views,
                project_revision: None,
                project: Some(project),
            });
        }
        // Unknown sources (including .wxs until the graph records WXS edges)
        // invalidate every view through `dirty_for_source`; known page,
        // component and config sources stay precise.
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            style_views,
            project_revision: Some(project_revision),
            project: Some(project),
        })
    }

    /// Removes an overlay when the client closes a tracked document.
    fn close_overlay(&self, uri: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let previous_active = state.active_project.clone();
        let (project, path) = route_project(&mut state, uri)?;
        mark_shared_config_stale(&mut state, &project, &path);
        state.overlays.remove(uri);
        let (accepted, is_json, is_style) = {
            let slot = state.projects.get(&project).ok_or(HostError::NoProject)?;
            (
                slot.project.accepts_overlay(&path),
                slot.project.accepts_json_overlay(&path),
                slot.project.accepts_style_overlay(&path),
            )
        };
        if !accepted && !is_style {
            state.active_project = previous_active;
            return Ok(HostUpdate::default());
        }
        let before = if is_style {
            Vec::new()
        } else {
            dirty_for_source(&state, &project, &path)
        };
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
        state
            .property_cache
            .remove(&(project.clone(), path.clone()));
        let style_views = if is_json {
            rebuild_slot_views(&mut state, &project);
            dirty.extend(dirty_for_source(&state, &project, &path));
            self.styles().clear_facts(&project);
            all_slot_views(&state, &project)
        } else if is_style {
            self.styles().invalidate_path(&project, &path)
        } else {
            let _ = refresh_component_script_facts(&mut state, &project, &path);
            Vec::new()
        };
        if is_style {
            return Ok(HostUpdate {
                dirty_views: Vec::new(),
                style_views,
                project_revision: None,
                project: Some(project),
            });
        }
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            style_views,
            project_revision: Some(project_revision),
            project: Some(project),
        })
    }

    /// Handles a saved script/JSON document.
    ///
    /// Overlays are kept because the client still holds the buffer; only the
    /// project state that depends on the saved file is refreshed.
    fn save_overlay(&self, uri: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let previous_active = state.active_project.clone();
        let (project, path) = route_project(&mut state, uri)?;
        mark_shared_config_stale(&mut state, &project, &path);
        let (accepted, is_json, is_project_json, is_style) = {
            let slot = state.projects.get(&project).ok_or(HostError::NoProject)?;
            (
                slot.project.accepts_overlay(&path),
                slot.project.accepts_json_overlay(&path),
                slot.project.is_project_json(&path),
                slot.project.accepts_style_overlay(&path),
            )
        };
        if !accepted && !is_project_json && !is_style {
            state.active_project = previous_active;
            return Ok(HostUpdate::default());
        }
        bump_file_version(&mut state, &project, &path);
        if is_style {
            let style_views = self.styles().invalidate_path(&project, &path);
            return Ok(HostUpdate {
                dirty_views: Vec::new(),
                style_views,
                project_revision: None,
                project: Some(project),
            });
        }
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
            self.styles().clear_facts(&project);
            let style_views = all_slot_views(&state, &project);
            let mut dirty = before;
            dirty.extend(dirty_for_source(&state, &project, &path));
            let dirty = mark_views_dirty(&mut state, &project, dirty);
            let project_revision = bump_slot_revision(&mut state, &project);
            return Ok(HostUpdate {
                dirty_views: dirty,
                style_views,
                project_revision: Some(project_revision),
                project: Some(project),
            });
        }
        let refreshed = refresh_component_script_facts(&mut state, &project, &path);
        if refreshed {
            crate::server::log(format_args!(
                "refreshed component property facts for a saved script"
            ));
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
            style_views: Vec::new(),
            project_revision: Some(project_revision),
            project: Some(project),
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
        let mut style_paths = Vec::new();
        let mut resource_paths = Vec::new();
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
            let is_style = slot.project.accepts_style_overlay(&path);
            let is_wxml = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wxml"));
            if is_json || is_project_json {
                json_paths.push((path, change.kind, change.uri.clone()));
            } else if is_script {
                script_paths.push((path, change.kind, change.uri.clone()));
            } else if is_wxml {
                wxml_paths.push((path, change.kind, change.uri.clone()));
            } else if is_style {
                style_paths.push((path, change.kind, change.uri.clone()));
            } else if is_watch_resource(&path) {
                resource_paths.push((path, change.kind, change.uri.clone()));
            } else {
                ignored = ignored.saturating_add(1);
            }
        }
        if json_paths.is_empty()
            && script_paths.is_empty()
            && wxml_paths.is_empty()
            && style_paths.is_empty()
            && resource_paths.is_empty()
        {
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
        for (path, _, _) in &script_paths {
            let _ = refresh_component_script_facts(&mut state, &project, path);
        }
        let mut affected_styles = std::collections::BTreeSet::new();
        for (path, _, _) in &style_paths {
            affected_styles.extend(self.styles().invalidate_path(&project, path));
        }
        if !json_paths.is_empty() || full_refresh {
            if let Some(slot) = state.projects.get_mut(&project) {
                slot.project.rediscover();
            }
            rebuild_slot_views(&mut state, &project);
            self.styles().clear_facts(&project);
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
        let style_views = if resource_paths.is_empty() {
            affected_styles.into_iter().collect()
        } else {
            // Any template can reference a changed local asset; only open
            // templates have published diagnostics, so this stays bounded.
            all_slot_views(&state, &project)
        };
        let dirty = mark_views_dirty(&mut state, &project, dirty);
        if ignored > 0 {
            crate::server::log(format_args!(
                "ignoring {ignored} watched file change(s) that do not affect analysis"
            ));
        }
        if dirty.is_empty() {
            if style_views.is_empty() {
                return Ok(HostUpdate::default());
            }
            return Ok(HostUpdate {
                dirty_views: Vec::new(),
                style_views,
                project_revision: None,
                project: Some(project),
            });
        }
        let project_revision = bump_slot_revision(&mut state, &project);
        Ok(HostUpdate {
            dirty_views: dirty,
            style_views,
            project_revision: Some(project_revision),
            project: Some(project),
        })
    }
}

const fn map_uri_error(_error: UriError) -> HostError {
    HostError::InvalidUri
}

/// Returns whether a watched path can be a local template resource.
fn is_watch_resource(path: &ProjectPath) -> bool {
    path.extension().is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "svg"
                | "webp"
                | "bmp"
                | "ico"
                | "avif"
                | "heic"
                | "heif"
                | "mp4"
                | "mov"
                | "m4v"
                | "webm"
                | "avi"
                | "mkv"
                | "mp3"
                | "wav"
                | "aac"
                | "m4a"
                | "ogg"
                | "flac"
                | "wma"
                | "wxs"
                | "ttf"
                | "otf"
                | "woff"
                | "woff2"
        )
    })
}

/// Marks other apps sharing a changed tsconfig project root for reload.
fn mark_shared_config_stale(state: &mut State, project: &ProjectId, path: &ProjectPath) {
    let tsconfig = {
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
        slot.app.tsconfig.clone()
    };
    for (id, slot) in &mut state.projects {
        if id != project && slot.app.tsconfig == tsconfig {
            slot.config_stale = true;
            slot.prepared.clear();
        }
    }
}

/// Loads the project for a known file identity, preferring explicit app roots.
fn load_project_for_id(
    state: &mut State,
    project_id: &ProjectId,
    path: &ProjectPath,
    activate: bool,
) -> Result<ProjectId, HostError> {
    if state.workspace_root.is_some() {
        return load_project_for_path(state, path, activate);
    }
    if let Some(app) = state
        .app_roots
        .iter()
        .find(|app| project_id_for(app) == *project_id)
        .cloned()
    {
        return ensure_loaded(state, &app, activate);
    }
    load_project_for_path(state, path, activate)
}

/// Loads the project for a project path. In workspace mode the owning app is
/// discovered from the path itself, so queries work before any explicit app is
/// configured or loaded.
fn load_project_for_path(
    state: &mut State,
    path: &ProjectPath,
    activate: bool,
) -> Result<ProjectId, HostError> {
    if let Some(workspace) = state.workspace_root.clone() {
        let absolute = workspace.join(path.as_str());
        let packages = workspace_packages_for(state, &workspace);
        let app = discover_app_for_path(&workspace, &absolute, &packages)?;
        cache_app(state, app.clone());
        return ensure_loaded(state, &app, activate);
    }
    if let Some(project) = active_project_id(state) {
        return Ok(project.clone());
    }
    if state.projects.len() == 1 {
        if let Some(project) = state.projects.keys().next() {
            return Ok(project.clone());
        }
    }
    Err(HostError::Unsupported {
        reason: "no project root is open",
    })
}

/// Resolves and loads the project that owns `uri`.
fn route_project(state: &mut State, uri: &str) -> Result<(ProjectId, ProjectPath), HostError> {
    let app = app_for_uri(state, uri)?;
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
    let project_id = project_id_for(app);
    if let Some(slot) = state.projects.get(&project_id) {
        if slot.app == *app && !slot.config_stale {
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
    let workspace_scoped = state.workspace_root.as_ref() == Some(&app.project_root);
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
            workspace_scoped,
        },
    );
    if activate {
        state.active_project = Some(project_id.clone());
    }
    Ok(project_id)
}

/// Opens an app with its effective buffers without changing foreground routing.
fn open_app(state: &State, candidate: &AppRoot) -> Result<Project, HostError> {
    let mut project = Project::open_with_config(
        &candidate.project_root,
        candidate.app_json.clone(),
        candidate.source_roots.clone(),
        candidate.tsconfig.clone(),
        candidate.dependency_paths.clone(),
    )
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

/// Selects the configured app root that contains `uri`, discovering one on
/// demand in zero-config mode.
fn app_for_uri(state: &mut State, uri: &str) -> Result<AppRoot, HostError> {
    let path = wxml_project::root_path_from_uri(uri).map_err(|_| HostError::InvalidUri)?;
    if let Some(workspace) = state.workspace_root.clone() {
        let packages = workspace_packages_for(state, &workspace);
        let app = discover_app_for_path(&workspace, &path, &packages)?;
        cache_app(state, app.clone());
        return Ok(app);
    }
    if let Some(candidate) = state
        .app_roots
        .iter()
        .filter(|candidate| path.starts_with(&candidate.app_dir))
        .max_by_key(|candidate| candidate.app_dir.as_os_str().len())
        .cloned()
    {
        return Ok(candidate);
    }
    if state.app_roots.is_empty() {
        return Err(HostError::NoProject);
    }
    // A tsconfig or other file may live above `app.json` but inside the loaded
    // TypeScript project. Keep the active project for it.
    if let Some(active) = state.active_project.as_ref() {
        if let Some(app) = state
            .app_roots
            .iter()
            .find(|app| &project_id_for(app) == active)
        {
            if path.starts_with(&app.project_root) {
                return Ok(app.clone());
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
    Ok(candidate.clone())
}

/// Caches a lazily discovered app for later path-only queries.
fn cache_app(state: &mut State, app: AppRoot) {
    if let Some(existing) = state
        .app_roots
        .iter_mut()
        .find(|existing| project_id_for(existing) == project_id_for(&app))
    {
        *existing = app;
    } else {
        state.app_roots.push(app);
        state
            .app_roots
            .sort_by(|left, right| left.app_dir.cmp(&right.app_dir));
    }
}

/// Discovers the nearest Mini Program manifest and TypeScript project above
/// one workspace file.
/// Returns the cached workspace package index, discovering it once per open root.
fn workspace_packages_for(state: &mut State, workspace: &Path) -> Arc<WorkspacePackages> {
    if let Some(packages) = &state.workspace_packages {
        return Arc::clone(packages);
    }
    let packages = Arc::new(WorkspacePackages::discover(workspace));
    state.workspace_packages = Some(Arc::clone(&packages));
    packages
}

fn discover_app_for_path(
    workspace: &Path,
    path: &Path,
    packages: &WorkspacePackages,
) -> Result<AppRoot, HostError> {
    let path = canonicalize_existing_or_parent(workspace, path)?;
    if !path.starts_with(workspace) {
        return Err(HostError::InvalidUri);
    }
    let mut current = if path.is_dir() {
        path.as_path()
    } else {
        path.parent().unwrap_or(path.as_path())
    };
    let mut app_json = None;
    let mut tsconfig = None;
    loop {
        if app_json.is_none() && current.join("app.json").is_file() {
            app_json = Some(current.join("app.json"));
        }
        if tsconfig.is_none() && current.join("tsconfig.json").is_file() {
            tsconfig = Some(current.join("tsconfig.json"));
        }
        if app_json.is_some() && tsconfig.is_some() {
            break;
        }
        if current == workspace {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if !parent.starts_with(workspace) {
            break;
        }
        current = parent;
    }

    let identity = tsconfig
        .as_ref()
        .and_then(|path| path.parent())
        .or_else(|| app_json.as_ref().and_then(|path| path.parent()))
        .unwrap_or_else(|| path.parent().unwrap_or(workspace))
        .to_path_buf();
    let identity = if tsconfig.is_none() && app_json.is_none() {
        nearest_package_json(workspace, &identity)
            .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
            .unwrap_or(identity)
    } else {
        identity
    };
    if app_json.is_none() {
        for candidate in [
            identity.join("app.json"),
            identity.join("src").join("app.json"),
        ] {
            if candidate.is_file() {
                app_json = Some(candidate);
                break;
            }
        }
    }
    let app_json = app_json.unwrap_or_else(|| identity.join("app.json"));
    let tsconfig = tsconfig.unwrap_or_else(|| identity.join("tsconfig.json"));
    let app_json = project_path_under(workspace, &app_json)?;
    let tsconfig = project_path_under(workspace, &tsconfig)?;
    let include_project_root = identity.as_path() == workspace;
    let mut source_roots = if include_project_root {
        Vec::new()
    } else {
        vec![project_path_under(workspace, &identity)?]
    };
    let manifest = nearest_package_json(workspace, &identity);
    let mut dependency_paths = BTreeMap::new();
    if let Some(manifest) = manifest {
        for package in packages.dependencies_of(&manifest) {
            source_roots.push(package.relative.clone());
            for (alias, targets) in &package.aliases {
                dependency_paths
                    .entry(alias.clone())
                    .or_insert_with(Vec::new)
                    .extend(targets.iter().cloned());
            }
        }
    }
    source_roots.sort();
    source_roots.dedup();
    Ok(AppRoot {
        app_dir: identity,
        project_root: workspace.to_path_buf(),
        app_json,
        tsconfig,
        source_roots,
        include_project_root,
        dependency_paths,
    })
}

/// Finds the nearest package manifest at or above one app directory.
fn nearest_package_json(workspace: &Path, start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(directory) = current {
        let candidate = directory.join("package.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if directory == workspace {
            break;
        }
        current = directory
            .parent()
            .filter(|parent| parent.starts_with(workspace));
    }
    None
}

/// Resolves symlinks when possible, otherwise canonicalizes the nearest
/// existing parent and keeps the missing file name.
fn canonicalize_existing_or_parent(workspace: &Path, path: &Path) -> Result<PathBuf, HostError> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Ok(canonical);
    }
    let parent = path.parent().ok_or(HostError::InvalidUri)?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|_| HostError::InvalidUri)?;
    let name = path.file_name().ok_or(HostError::InvalidUri)?;
    let joined = canonical_parent.join(name);
    if joined.starts_with(workspace) {
        Ok(joined)
    } else {
        Err(HostError::InvalidUri)
    }
}

/// Converts one host path below the workspace into a project path.
fn project_path_under(workspace: &Path, path: &Path) -> Result<ProjectPath, HostError> {
    let relative = path
        .strip_prefix(workspace)
        .map_err(|_| HostError::InvalidUri)?;
    let text = relative.to_string_lossy();
    let text = if text.is_empty() { "." } else { text.as_ref() };
    ProjectPath::new(text).map_err(|_| HostError::InvalidUri)
}

fn project_id_for(app: &AppRoot) -> ProjectId {
    project_id_for_parts(&app.app_dir, &app.app_json)
}

/// Hashes a project's boundary and route table without exposing host paths.
fn project_id_for_parts(root: &Path, app_json: &ProjectPath) -> ProjectId {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in root
        .to_string_lossy()
        .bytes()
        .chain([0])
        .chain(app_json.as_str().bytes())
    {
        hash ^= u64::from(byte);
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
    if let Some(previous) = previous {
        for (path, view) in previous {
            if view.lazy && !views.contains_key(path) && project.file_size(path).is_some() {
                let mut refreshed = view.clone();
                if let Some(base_text) = path.as_str().strip_suffix(".wxml") {
                    if let Ok(base) = ProjectPath::new(base_text) {
                        refreshed.components = lazy_view_components(project, &base);
                    }
                }
                views.insert(path.clone(), refreshed);
            }
        }
    }
    views
}

/// Reads the effective component config for a template that has no entry in
/// the discovered page/component graph.
fn lazy_view_config(
    project: &Project,
    base: &ProjectPath,
) -> (wxml_project::PageConfig, Option<ProjectPath>) {
    let local_file = base.with_appended_extension("json").ok();
    let local = match local_file.as_ref().and_then(|path| {
        project
            .read_optional(path)
            .ok()
            .flatten()
            .map(|text| (path, text))
    }) {
        Some((path, text)) => wxml_project::discovery::parse_page_config(path, &text)
            .map(|parsed| parsed.value.local)
            .unwrap_or_default(),
        None => BTreeMap::new(),
    };
    let global = project
        .read_optional(project.app_json())
        .ok()
        .flatten()
        .and_then(|text| {
            wxml_project::discovery::parse_app_config(project.app_json(), &text)
                .ok()
                .map(|parsed| parsed.value.using_components)
        })
        .unwrap_or_default();
    (
        wxml_project::PageConfig::from_components(Arc::new(global), local),
        local_file,
    )
}

fn lazy_view_components(project: &Project, base: &ProjectPath) -> BTreeMap<String, ProjectPath> {
    let (config, _) = lazy_view_config(project, base);
    local_component_paths(&config)
}

/// Derives a page or component view from the adjacent script when discovery
/// did not find one. Returns whether the template now has a usable target.
fn ensure_lazy_view(state: &mut State, project_id: &ProjectId, wxml: &ProjectPath) -> bool {
    let Some(slot) = state.projects.get(project_id) else {
        return false;
    };
    if let Some(existing) = slot.views.get(wxml) {
        if !existing.lazy {
            return true;
        }
        let Some(base_text) = wxml.as_str().strip_suffix(".wxml") else {
            return false;
        };
        let Ok(base) = ProjectPath::new(base_text) else {
            return false;
        };
        let components = lazy_view_components(&slot.project, &base);
        if existing.components == components {
            return true;
        }
        let revision = ViewRevision::new(state.next_view_revision);
        state.next_view_revision = state.next_view_revision.saturating_add(1);
        if let Some(slot) = state.projects.get_mut(project_id) {
            if let Some(view) = slot.views.get_mut(wxml) {
                view.components = components;
                view.view_revision = revision;
            }
            slot.prepared.clear();
        }
        return true;
    }
    let raw = wxml.as_str();
    let Some(base_text) = raw.strip_suffix(".wxml") else {
        return false;
    };
    let Ok(base) = ProjectPath::new(base_text) else {
        return false;
    };
    let resolution = wxml_project::script::resolve_script(&base, |candidate| {
        slot.project.file_size(candidate).is_some()
    });
    let Some(script) = resolution.script else {
        return false;
    };
    let Ok(text) = slot.project.read(&script) else {
        return false;
    };
    let owner = match wxml_project::script::registration_style_hints(&text).registration {
        Some(RegistrationKind::Page) => RootKind::Page,
        _ => RootKind::Component,
    };
    let components = lazy_view_components(&slot.project, &base);
    let Ok(script_file) = FileId::new(project_id.clone(), script.as_str()) else {
        return false;
    };
    let revision = ViewRevision::new(state.next_view_revision);
    state.next_view_revision = state.next_view_revision.saturating_add(1);
    let Some(slot) = state.projects.get_mut(project_id) else {
        return false;
    };
    slot.views.insert(
        wxml.clone(),
        ViewState {
            owner,
            script: Some(script),
            script_file: Some(script_file),
            view_revision: revision,
            components,
            lazy: true,
        },
    );
    slot.prepared.clear();
    true
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
            lazy: false,
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

/// Refreshes discovered property facts for one component script.
fn refresh_component_script_facts(
    state: &mut State,
    project: &ProjectId,
    script: &ProjectPath,
) -> bool {
    let refreshed = state
        .projects
        .get_mut(project)
        .is_some_and(|slot| slot.project.refresh_component_properties(script));
    if refreshed {
        state
            .property_cache
            .remove(&(project.clone(), script.clone()));
    }
    refreshed
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
        reason: "template has no adjacent page or component script",
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
    let (config, local_file) =
        if let Some(page) = slot.project.pages().iter().find(|page| page.wxml == *wxml) {
            (&page.config, page.json.clone())
        } else if let Some(component) = slot
            .project
            .components()
            .iter()
            .find(|component| component.wxml == *wxml)
        {
            (&component.config, component.json.clone())
        } else {
            let Some(view) = slot.views.get(wxml) else {
                return Vec::new();
            };
            if !view.lazy {
                return Vec::new();
            }
            let Some(base) = wxml.as_str().strip_suffix(".wxml") else {
                return Vec::new();
            };
            let Ok(base) = ProjectPath::new(base) else {
                return Vec::new();
            };
            let (config, local_file) = lazy_view_config(&slot.project, &base);
            return components_from_config(slot, &config, local_file.as_ref());
        };
    components_from_config(slot, config, local_file.as_ref())
}

/// Builds the LSP component list from one effective page/component config.
fn components_from_config(
    slot: &ProjectSlot,
    config: &wxml_project::PageConfig,
    local_file: Option<&ProjectPath>,
) -> Vec<ViewComponent> {
    let app_json = slot.project.app_json().clone();
    let mut components: Vec<ViewComponent> = config
        .iter()
        .map(|(name, usage)| {
            let discovered = usage
                .component
                .as_ref()
                .and_then(|base| slot.project.component(base));
            let origin = if config.local.contains_key(name) {
                ComponentOrigin::Local {
                    file: local_file.cloned(),
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
                    code: None,
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
                code: None,
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
            source_complete: false,
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

    let source_limits = if slot.workspace_scoped {
        SourceLimits::workspace()
    } else {
        SourceLimits::default()
    };
    let sources = slot.project.collect_sources(source_limits);
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
    let skipped_sources = sources.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            wxml_project::sources::SourceIssueKind::TooLarge
                | wxml_project::sources::SourceIssueKind::Unreadable
        )
    });
    let mut source_complete = !sources.truncated && !skipped_sources;
    if sources.truncated {
        merge_hint(
            &mut hint,
            CapabilityStatus::Partial {
                reason: "project source collection was truncated".to_owned(),
            },
        );
    } else if skipped_sources {
        merge_hint(
            &mut hint,
            CapabilityStatus::Partial {
                reason: "project source collection skipped an oversized or unreadable script"
                    .to_owned(),
            },
        );
    }
    source_complete &= limit_request_files(&mut files, script, source_limits, &mut hint);
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
        source_complete,
    }
}
/// Keeps the owning script and applies the C1 `files[]` count/byte bounds.
/// Returns `false` when a budget removed any file.
fn limit_request_files(
    files: &mut Vec<PreparedFile>,
    script: &ProjectPath,
    limits: SourceLimits,
    hint: &mut Option<CapabilityStatus>,
) -> bool {
    let mut complete = true;
    if let Some(position) = files.iter().position(|file| file.path == *script) {
        if position > 0 {
            let file = files.remove(position);
            files.insert(0, file);
        }
    }
    if files.len() > limits.max_files {
        complete = false;
        files.truncate(limits.max_files);
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
        if total.saturating_add(file.text.len()) > limits.max_total_bytes {
            complete = false;
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
    complete
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
                code: None,
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
                code: None,
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
        source_complete: false,
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
    fn nested_app_root_is_discovered_lazily_and_explicit_configuration_still_works() {
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
        let lazy_wxml = ProjectPath::new("apps/miniprogram/src/pages/index/index.wxml")
            .expect("lazy wxml path");
        let view = host.view_for(&lazy_wxml).expect("nested view");
        assert_eq!(
            view.script.as_str(),
            "apps/miniprogram/src/pages/index/index.ts"
        );

        host.open_root_with_options(
            Some(&format!("file://{}", root.display())),
            &InitializationOptions::parse(&serde_json::json!({
                "apps": [{"root": "apps/miniprogram/src"}]
            }))
            .expect("options"),
        )
        .expect("open configured root");
        let wxml = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        let view = host.view_for(&wxml).expect("configured nested view");
        assert_eq!(view.script.as_str(), "pages/index/index.ts");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn analysis_without_adjacent_script_is_syntax_only() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-lazy-no-script-{stamp}"));
        let app = root.join("apps/miniprogram/src");
        std::fs::create_dir_all(app.join("pages/index")).expect("create app root");
        std::fs::write(app.join("app.json"), r#"{"pages":[]}"#).expect("write app.json");
        std::fs::write(
            app.join("pages/index/index.wxml"),
            "<view>{{ title }}</view>",
        )
        .expect("write wxml");

        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open monorepo root");
        let path =
            ProjectPath::new("apps/miniprogram/src/pages/index/index.wxml").expect("wxml path");
        let file = host.file_id(&path).expect("file id");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(2),
            "<view>{{ title }}</view>",
        ));
        let view = host.analysis(&snapshot).expect("analysis");
        assert!(view.target.is_none());
        assert!(matches!(
            view.status_hint,
            Some(CapabilityStatus::Unsupported { reason })
                if reason == "template has no adjacent page or component script"
        ));

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
        let id = project_id_for_parts(
            Path::new("/tmp/very/secret/project"),
            &ProjectPath::new("app.json").expect("app json"),
        );
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
        let _ = limit_request_files(&mut files, &script, SourceLimits::default(), &mut hint);
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
    fn view_for_script_resolves_the_unique_owning_view() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        let root = std::fs::canonicalize(root).expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open project");
        let script = ProjectPath::new("pages/index/index.ts").expect("script");
        let file = host.file_id(&script).expect("script file");
        let view = host.view_for_script(&file).expect("owning view");
        assert_eq!(view.wxml.as_str(), "pages/index/index.wxml");
        assert_eq!(view.script, script);
        assert_eq!(view.script_file, file);

        let wxml = ProjectPath::new("pages/index/index.wxml").expect("wxml");
        let wxml_file = host.file_id(&wxml).expect("wxml file");
        assert!(matches!(
            host.view_for_script(&wxml_file),
            Err(HostError::Unsupported { .. })
        ));
    }

    #[test]
    fn view_for_script_rejects_a_script_shared_by_two_views() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        let root = std::fs::canonicalize(root).expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open project");
        let script = ProjectPath::new("pages/index/index.ts").expect("script");
        let file = host.file_id(&script).expect("script file");
        host.view_for_script(&file).expect("owning view");
        // Discovery never maps two views to one script today, so construct the
        // ambiguous view table directly. The host must reject it instead of
        // picking one outline context arbitrarily.
        {
            let mut state = host.write();
            let slot = state
                .projects
                .get_mut(file.project())
                .expect("project slot");
            let extra = ProjectPath::new("pages/index/index-copy.wxml").expect("extra wxml");
            slot.views.insert(
                extra,
                ViewState {
                    owner: RootKind::Page,
                    script: Some(script),
                    script_file: Some(file.clone()),
                    view_revision: ViewRevision::new(99),
                    components: BTreeMap::new(),
                    lazy: false,
                },
            );
        }

        let error = host.view_for_script(&file).expect_err("shared owner");
        assert!(
            matches!(error, HostError::Unsupported { reason } if reason.contains("shared")),
            "{error:?}"
        );
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

    fn fake_facts(view: &str, classes: usize) -> Arc<StyleViewFacts> {
        use wxml_project::{ClassDefinition, ClassToken, StyleIsolation, StyleScope};
        let token = Arc::new(ClassToken {
            name: "c".to_owned(),
            range: wxml_text::SourceRange::from_bounds(0, 1).expect("range"),
            selector: ".c".to_owned(),
            comment: None,
            rename_safe: true,
        });
        let file = Arc::new(ProjectPath::new("a.wxss").expect("path"));
        Arc::new(StyleViewFacts {
            view: ProjectPath::new(view).expect("view"),
            project_id: Some("test".to_owned()),
            scope: StyleScope::Page,
            isolation: StyleIsolation::PageShared,
            files: Vec::new(),
            classes: (0..classes)
                .map(|order| ClassDefinition {
                    token: Arc::clone(&token),
                    file: Arc::clone(&file),
                    scope: StyleScope::Page,
                    imported: false,
                    order: u32::try_from(order).unwrap_or(u32::MAX),
                })
                .collect(),
            issues: Vec::new(),
            complete: true,
            reason: None,
        })
    }

    #[test]
    fn style_fact_cache_is_bounded_and_rejects_oversized_entries() {
        let mut cache = ProjectStyleCache::default();
        for index in 0..(MAX_STYLE_VIEW_CACHE + 10) {
            let view = format!("pages/p{index}/index.wxml");
            cache.insert(
                ProjectPath::new(view).expect("view"),
                fake_facts(&format!("pages/p{index}/index.wxml"), 1),
            );
        }
        assert_eq!(cache.order.len(), MAX_STYLE_VIEW_CACHE);
        assert_eq!(cache.facts.len(), MAX_STYLE_VIEW_CACHE);
        assert!(cache.fact_classes <= MAX_STYLE_FACT_CLASSES);

        let oversized = ProjectPath::new("pages/big/index.wxml").expect("view");
        cache.insert(
            oversized.clone(),
            fake_facts("pages/big/index.wxml", 200_001),
        );
        assert!(!cache.facts.contains_key(&oversized));
        assert!(cache.fact_classes <= MAX_STYLE_FACT_CLASSES);
    }

    #[test]
    fn local_resource_diagnostics_are_bounded() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-style-diag-budget-{stamp}"));
        std::fs::create_dir_all(root.join("pages/index")).expect("dirs");
        std::fs::write(root.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
            .expect("app json");
        std::fs::write(root.join("pages/index/index.wxml"), "<view/>").expect("wxml");
        std::fs::write(root.join("pages/index/index.ts"), "Page({})").expect("script");
        std::fs::write(root.join("pages/index/index.wxss"), ".a {}").expect("style");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open");
        let path = ProjectPath::new("pages/index/index.wxml").expect("path");
        let file = host.file_id(&path).expect("file");
        let text = "<image src=\"missing.png\"/>".repeat(2_200);
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(99),
            text,
        ));
        let diagnostics = host.style_diagnostics(&snapshot);
        assert_eq!(diagnostics.len(), 2048);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn script_changes_keep_style_facts_but_style_changes_invalidate_them() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-style-cache-{stamp}"));
        std::fs::create_dir_all(root.join("pages/index")).expect("dirs");
        std::fs::write(root.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
            .expect("app json");
        std::fs::write(root.join("pages/index/index.wxml"), "<view class=\"a\"/>").expect("wxml");
        std::fs::write(root.join("pages/index/index.ts"), "Page({})").expect("script");
        std::fs::write(root.join("pages/index/index.wxss"), ".a { color: red; }\n").expect("style");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open");
        let path = ProjectPath::new("pages/index/index.wxml").expect("path");
        let first = host.style_facts(&path).expect("facts");
        let script_uri = format!("file://{}/pages/index/index.ts", root.display());
        host.open_document(OpenDocument {
            uri: script_uri,
            text: "Page({ data: { x: 1 } })".to_owned(),
            version: Some(1),
        })
        .expect("script overlay");
        let second = host.style_facts(&path).expect("facts");
        assert!(
            Arc::ptr_eq(&first, &second),
            "script edits must not rebuild styles"
        );

        let style_uri = format!("file://{}/pages/index/index.wxss", root.display());
        host.open_document(OpenDocument {
            uri: style_uri,
            text: ".b { color: blue; }".to_owned(),
            version: Some(1),
        })
        .expect("style overlay");
        let third = host.style_facts(&path).expect("facts");
        assert!(
            !Arc::ptr_eq(&second, &third),
            "style edits must rebuild facts"
        );
        assert!(third.primary("b").is_some());
        assert!(third.primary("a").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn style_edits_republish_only_views_whose_chain_changed() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-style-affected-{stamp}"));
        std::fs::create_dir_all(root.join("pages/a")).expect("a dir");
        std::fs::create_dir_all(root.join("pages/b")).expect("b dir");
        std::fs::write(
            root.join("app.json"),
            r#"{"pages":["pages/a/index","pages/b/index"]}"#,
        )
        .expect("app json");
        std::fs::write(root.join("app.wxss"), ".global { color: black; }\n").expect("app style");
        for page in ["a", "b"] {
            std::fs::write(
                root.join(format!("pages/{page}/index.wxml")),
                format!(r#"<view class="{page}"/>"#),
            )
            .expect("wxml");
            std::fs::write(root.join(format!("pages/{page}/index.ts")), "Page({})")
                .expect("script");
            std::fs::write(
                root.join(format!("pages/{page}/index.wxss")),
                format!(".{page} {{ color: red; }}\n"),
            )
            .expect("page style");
        }
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open");
        let a_path = ProjectPath::new("pages/a/index.wxml").expect("a path");
        let b_path = ProjectPath::new("pages/b/index.wxml").expect("b path");
        host.style_facts(&a_path).expect("a facts");
        host.style_facts(&b_path).expect("b facts");

        let a_style = format!("file://{}/pages/a/index.wxss", root.display());
        let update = host
            .open_document(OpenDocument {
                uri: a_style,
                text: ".a { color: blue; }\n".to_owned(),
                version: Some(1),
            })
            .expect("a style overlay");
        assert_eq!(update.style_views, vec![a_path.clone()]);
        assert!(
            update.project.is_some(),
            "style updates carry owner identity"
        );
        // The diagnostics handler rebuilds facts for the republished view.
        host.style_facts(&a_path).expect("a facts after republish");

        let app_style = format!("file://{}/app.wxss", root.display());
        let update = host
            .open_document(OpenDocument {
                uri: app_style,
                text: ".global { color: green; }\n".to_owned(),
                version: Some(1),
            })
            .expect("app style overlay");
        assert!(update.style_views.contains(&a_path));
        assert!(update.style_views.contains(&b_path));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn external_class_references_make_open_template_facts_partial() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-style-external-{stamp}"));
        std::fs::create_dir_all(root.join("pages/index")).expect("dirs");
        std::fs::write(root.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
            .expect("app json");
        std::fs::write(root.join("pages/index/index.wxml"), "<view class=\"a\"/>").expect("wxml");
        std::fs::write(root.join("pages/index/index.ts"), "Page({})").expect("script");
        std::fs::write(root.join("pages/index/index.wxss"), ".a { color: red; }\n").expect("style");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open");
        let path = ProjectPath::new("pages/index/index.wxml").expect("path");
        assert!(host.style_facts(&path).expect("facts").complete);

        let file = host.file_id(&path).expect("view file");
        let snapshot = Arc::new(SourceSnapshot::new(
            file.clone(),
            SourceRevision::new(1),
            SnapshotId::new(7),
            "<view class=\"~external a\"/>".to_owned(),
        ));
        host.set_wxml_snapshot(file, snapshot);
        let facts = host.style_facts(&path).expect("external facts");
        assert!(!facts.complete);
        assert!(facts.reason.is_some());
        assert!(facts.primary("a").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn style_facts_are_isolated_by_project_identity() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-style-project-isolation-{stamp}"));
        std::fs::create_dir_all(&root).expect("workspace");
        std::fs::write(root.join("tsconfig.json"), r#"{"compilerOptions":{}}"#).expect("tsconfig");
        for app in ["a", "b"] {
            let dir = root.join(app);
            std::fs::create_dir_all(dir.join("pages/index")).expect("app dir");
            std::fs::write(dir.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
                .expect("app json");
            std::fs::write(dir.join("pages/index/index.ts"), "Page({})").expect("script");
            std::fs::write(
                dir.join("pages/index/index.wxml"),
                r#"<view class="page"/>"#,
            )
            .expect("wxml");
            std::fs::write(
                dir.join("pages/index/index.wxss"),
                format!(".{app}-page {{ color: red; }}\n"),
            )
            .expect("page style");
            std::fs::write(
                dir.join("app.wxss"),
                format!(".{app}-global {{ color: blue; }}\n"),
            )
            .expect("app style");
        }
        let host = ProjectState::new();
        host.open_root_with_options(
            Some(&format!("file://{}", root.display())),
            &InitializationOptions::parse(&serde_json::json!({
                "apps": [{"root": "a"}, {"root": "b"}]
            }))
            .expect("options"),
        )
        .expect("open workspace");
        let a_uri = format!("file://{}/a/pages/index/index.wxml", root.display());
        let b_uri = format!("file://{}/b/pages/index/index.wxml", root.display());
        host.path_for_uri(&a_uri).expect("load a");
        let a_file = host.resolve_file(&a_uri).expect("a file");
        let facts_a = host.style_facts_for(&a_file).expect("a facts");
        assert!(facts_a.primary("a-page").is_some());
        assert!(facts_a.primary("b-page").is_none());
        assert_eq!(
            facts_a.project_id.as_deref(),
            Some(host.project_id().as_str())
        );

        host.path_for_uri(&b_uri).expect("load b");
        let b_file = host.resolve_file(&b_uri).expect("b file");
        assert_ne!(a_file.project(), b_file.project());
        assert_eq!(a_file.path(), b_file.path());
        let facts_a = host
            .style_facts_for(&a_file)
            .expect("a file keeps its owning project styles");
        assert!(facts_a.primary("a-page").is_some());
        assert!(facts_a.primary("b-page").is_none());
        assert_ne!(
            facts_a.project_id.as_deref(),
            Some(host.project_id().as_str()),
            "a path must not use b's project identity"
        );
        let facts_b = host.style_facts_for(&b_file).expect("b facts");
        assert!(facts_b.primary("b-page").is_some());
        assert!(facts_b.primary("a-page").is_none());

        host.path_for_uri(&a_uri).expect("reload a");
        let facts_a = host.style_facts_for(&a_file).expect("a facts again");
        assert!(facts_a.primary("a-page").is_some());
        assert!(facts_a.primary("b-page").is_none());
        let _ = std::fs::remove_dir_all(root);
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
    fn lazy_app_discovery_mounts_workspace_package_sources_and_aliases() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-lazy-packages-{stamp}"));
        let app = root.join("apps/miniprogram");
        let src = app.join("src");
        let page = src.join("pages/index");
        let api = root.join("packages/api");
        std::fs::create_dir_all(&page).expect("page dir");
        std::fs::create_dir_all(api.join("src/enum")).expect("api dir");
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*","packages/*"]}"#,
        )
        .expect("root manifest");
        std::fs::write(
            app.join("package.json"),
            r#"{"name":"jl-miniprogram","dependencies":{"jl-api":"workspace:*"}}"#,
        )
        .expect("app manifest");
        std::fs::write(
            api.join("package.json"),
            r#"{"name":"jl-api","exports":{"./*":"./src/*/index.ts"}}"#,
        )
        .expect("api manifest");
        std::fs::write(api.join("src/enum/index.ts"), "export const value = 1;")
            .expect("api source");
        std::fs::write(src.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
            .expect("app json");
        std::fs::write(app.join("tsconfig.json"), "{}").expect("tsconfig");
        let wxml = page.join("index.wxml");
        std::fs::write(&wxml, "<view/>").expect("wxml");
        std::fs::write(page.join("index.ts"), "Page({ data: { value: 1 } });").expect("script");

        let packages = WorkspacePackages::discover(&root);
        let discovered = discover_app_for_path(&root, &wxml, &packages).expect("lazy app");
        assert_eq!(discovered.app_dir, app);
        assert_eq!(discovered.project_root, root);
        assert!(
            discovered
                .source_roots
                .iter()
                .any(|path| path.as_str() == "apps/miniprogram")
        );
        assert!(
            discovered
                .source_roots
                .iter()
                .any(|path| path.as_str() == "packages/api")
        );
        assert_eq!(
            discovered
                .dependency_paths
                .get("jl-api/*")
                .and_then(|targets| targets.first())
                .map(ProjectPath::as_str),
            Some("packages/api/src/*/index.ts")
        );

        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open lazy workspace");
        let path = ProjectPath::new("apps/miniprogram/src/pages/index/index.wxml").expect("path");
        let view = host.view_for(&path).expect("lazy view");
        assert_eq!(
            view.script.as_str(),
            "apps/miniprogram/src/pages/index/index.ts"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lazy_root_app_collects_its_own_sources_with_dependencies() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-lazy-root-app-{stamp}"));
        let page = root.join("pages/index");
        let api = root.join("packages/api");
        std::fs::create_dir_all(&page).expect("page dir");
        std::fs::create_dir_all(api.join("src")).expect("api dir");
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root-app","workspaces":["packages/*"],"dependencies":{"jl-api":"workspace:*"}}"#,
        )
        .expect("root manifest");
        std::fs::write(
            api.join("package.json"),
            r#"{"name":"jl-api","exports":{"./*":"./src/*.ts"}}"#,
        )
        .expect("api manifest");
        std::fs::write(api.join("src/index.ts"), "export const value = 1;").expect("api source");
        std::fs::write(root.join("app.json"), r#"{"pages":["pages/index/index"]}"#)
            .expect("app json");
        std::fs::write(root.join("tsconfig.json"), "{}").expect("tsconfig");
        let wxml = page.join("index.wxml");
        std::fs::write(&wxml, "<view/>").expect("wxml");
        std::fs::write(page.join("index.ts"), "Page({ data: { value: 1 } });").expect("script");

        let packages = WorkspacePackages::discover(&root);
        let discovered = discover_app_for_path(&root, &wxml, &packages).expect("lazy app");
        assert!(discovered.include_project_root);
        assert_eq!(
            discovered
                .source_roots
                .iter()
                .map(ProjectPath::as_str)
                .collect::<Vec<_>>(),
            ["packages/api"]
        );
        let project = Project::open_with_config_scoped(
            &root,
            discovered.app_json.clone(),
            discovered.source_roots.clone(),
            discovered.tsconfig.clone(),
            discovered.dependency_paths.clone(),
            discovered.include_project_root,
        )
        .expect("open scoped project");
        let sources = project.collect_sources(SourceLimits::workspace());
        let paths: Vec<&str> = sources
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert!(paths.contains(&"pages/index/index.ts"), "{paths:?}");
        assert!(paths.contains(&"packages/api/src/index.ts"), "{paths:?}");
        assert!(
            project.accepts_script_overlay(
                &ProjectPath::new("pages/index/index.ts").expect("page script")
            ),
            "root-app scripts must remain overlayable"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lazy_project_ids_include_the_route_table() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-lazy-identities-{stamp}"));
        std::fs::create_dir_all(&root).expect("root dir");
        std::fs::write(root.join("tsconfig.json"), "{}").expect("root tsconfig");
        std::fs::create_dir_all(root.join("apps/a/src/pages/index")).expect("a dir");
        std::fs::create_dir_all(root.join("apps/b/src/pages/index")).expect("b dir");
        std::fs::write(
            root.join("apps/a/src/app.json"),
            r#"{"pages":["pages/index/index"]}"#,
        )
        .expect("a app json");
        std::fs::write(
            root.join("apps/b/src/app.json"),
            r#"{"pages":["pages/other/index"]}"#,
        )
        .expect("b app json");
        let a = root.join("apps/a/src/pages/index/index.wxml");
        let b = root.join("apps/b/src/pages/index/index.wxml");
        std::fs::write(&a, "<view/>").expect("a wxml");
        std::fs::write(&b, "<view/>").expect("b wxml");
        std::fs::write(root.join("apps/a/src/pages/index/index.ts"), "Page({});")
            .expect("a script");
        std::fs::write(root.join("apps/b/src/pages/index/index.ts"), "Page({});")
            .expect("b script");

        let packages = WorkspacePackages::discover(&root);
        let a_app = discover_app_for_path(&root, &a, &packages).expect("a app");
        let b_app = discover_app_for_path(&root, &b, &packages).expect("b app");
        assert_eq!(a_app.app_dir, root);
        assert_eq!(b_app.app_dir, root);
        assert_ne!(a_app.app_json, b_app.app_json);
        assert_ne!(project_id_for(&a_app), project_id_for(&b_app));

        let _ = std::fs::remove_dir_all(root);
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
