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
use wxml_project::{Page, Project, ProjectPath, SourceFile, SourceKind, SourceLimits, UriError};
use wxml_syntax::{ParseErrorKind, ParseResult};
use wxml_types::{
    CapabilityStatus, CheckFragment, Diagnostic, DiagnosticSeverity, FileId, ProjectId,
    ProjectRevision, RootKind, SourceSnapshot, TemplateAnalyzer, UnsupportedKind, ViewRevision,
};

/// Marker used by the synthetic host and the synthetic workspace project.
pub const SYNTHETIC_PROJECT: &str = "lsp";

/// Compiler options that the C1 Node backend accepts.
///
/// This mirrors `REQUEST_COMPILER_OPTION_KEYS` in
/// `backends/typescript/src/protocol.ts`. Unknown keys are filtered here and
/// reported as a `Partial` hint instead of being sent to Node, which rejects
/// unknown options.
const SUPPORTED_COMPILER_OPTIONS: [&str; 31] = [
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
}

#[derive(Debug)]
struct State {
    project: Option<Project>,
    project_id: ProjectId,
    project_revision: ProjectRevision,
    views: BTreeMap<ProjectPath, ViewState>,
    snapshots: HashMap<FileId, Arc<SourceSnapshot>>,
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
                project_revision: ProjectRevision::new(project_id.clone(), 1),
                project_id,
                views: BTreeMap::new(),
                snapshots: HashMap::new(),
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
        state.views.clear();
        state.snapshots.clear();
        state.project_id = ProjectId::new(SYNTHETIC_PROJECT);
        state.project_revision = ProjectRevision::new(ProjectId::new(SYNTHETIC_PROJECT), 1);
        let Some(uri) = root_uri else {
            return Ok(());
        };
        let project = Project::open_uri(uri).map_err(|_| HostError::ProjectOpen)?;
        let project_id = project_id_for(project.root());
        state.project_id = project_id.clone();
        state.project_revision = ProjectRevision::new(project_id.clone(), 1);
        state.next_view_revision = 1;
        state.next_overlay_revision = 1;
        let mut next_view_revision = state.next_view_revision;
        let views = build_views(&project, &project_id, &mut next_view_revision);
        state.views = views;
        state.next_view_revision = next_view_revision;
        state.project = Some(project);
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

    fn path_for_uri(&self, uri: &str) -> Result<ProjectPath, HostError> {
        let state = self.read();
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
        let mut state = self.write();
        let path = {
            let project = state.project.as_mut().ok_or(HostError::NoProject)?;
            let path = project.path_from_uri(uri).map_err(map_uri_error)?;
            project.remove_overlay(&path);
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                project.rediscover();
            }
            path
        };
        let is_json = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if is_json {
            let project_id = state.project_id.clone();
            let mut next_view_revision = state.next_view_revision;
            let views = {
                let project = state.project.as_ref().ok_or(HostError::NoProject)?;
                build_views(project, &project_id, &mut next_view_revision)
            };
            state.views = views;
            state.next_view_revision = next_view_revision;
        }
        let dirty = if is_json {
            state.views.keys().cloned().collect()
        } else {
            dirty_scripts(&state, &path)
        };
        let revision = bump_project_revision(&mut state);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(revision),
        })
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
        let state = self.read();
        let project = state.project.as_ref()?;
        let path = ProjectPath::new(file.path()).ok()?;
        project.read(&path).ok().map(Arc::from)
    }

    fn view_components(&self, wxml: &ProjectPath) -> Vec<ViewComponent> {
        let state = self.read();
        let Some(project) = state.project.as_ref() else {
            return Vec::new();
        };
        // Discovery records `usingComponents` for pages; component JSON is
        // parsed for properties only, so a component's own WXML has no static
        // component list yet.
        let Some(config) = project
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

    #[allow(clippy::too_many_lines)] // one linear P7-1 pipeline
    fn analysis(&self, snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        let parsed = wxml_syntax::parse(snapshot.text());
        let mut syntax = syntax_diagnostics(&parsed, snapshot);

        let state = self.read();
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
        let analysis = TemplateAnalyzer::new(owner, snapshot.revision()).analyze(&parsed.tree);
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
        if !analysis.unsupported.is_empty() {
            merge_hint(
                &mut hint,
                CapabilityStatus::Partial {
                    reason: "some template features are not analyzed".to_owned(),
                },
            );
        }

        let Some(script) = view.script.as_ref() else {
            return Ok(PreparedView {
                view_id: format!("{}:0", path.as_str()),
                target: None,
                files: Arc::from(Vec::new()),
                compiler_options: serde_json::Map::new(),
                fragments: Arc::from(analysis.fragments),
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
        if analysis
            .fragments
            .iter()
            .any(|fragment| !fragment.locals.is_empty())
        {
            merge_hint(
                &mut hint,
                CapabilityStatus::Partial {
                    reason:
                        "template locals are bound as any; loop item/index member checks deferred"
                            .to_owned(),
                },
            );
        }

        Ok(PreparedView {
            view_id: format!("{}:{}", path.as_str(), view_revision.get()),
            target,
            files: Arc::from(files),
            compiler_options,
            fragments: Arc::from(analysis.fragments),
            project_root: project.root().to_path_buf(),
            syntax_diagnostics: syntax,
            status_hint: hint,
        })
    }
}

impl ProjectState {
    /// Applies a script or JSON overlay and reports affected views.
    fn overlay_update(&self, uri: &str, text: &str) -> Result<HostUpdate, HostError> {
        let mut state = self.write();
        let revision = state.next_overlay_revision.saturating_add(1);
        state.next_overlay_revision = revision;
        let path = {
            let project = state.project.as_mut().ok_or(HostError::NoProject)?;
            let path = project.path_from_uri(uri).map_err(map_uri_error)?;
            let extension = path
                .extension()
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if extension == "wxml" {
                return Ok(HostUpdate::default());
            }
            project
                .set_overlay(&path, text.to_owned(), revision)
                .map_err(|_| HostError::Read)?;
            if extension == "json" {
                // Full rediscovery per JSON overlay is intentional for
                // correctness; incremental discovery is deferred (M6).
                project.rediscover();
            }
            path
        };
        let is_json = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if is_json {
            let project_id = state.project_id.clone();
            let mut next_view_revision = state.next_view_revision;
            let views = {
                let project = state.project.as_ref().ok_or(HostError::NoProject)?;
                build_views(project, &project_id, &mut next_view_revision)
            };
            state.views = views;
            state.next_view_revision = next_view_revision;
        }
        let dirty = if is_json {
            state.views.keys().cloned().collect()
        } else {
            dirty_scripts(&state, &path)
        };
        let project_revision = bump_project_revision(&mut state);
        Ok(HostUpdate {
            dirty_views: dirty,
            project_revision: Some(project_revision),
        })
    }
}

const fn map_uri_error(_error: UriError) -> HostError {
    HostError::InvalidUri
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
        },
    );
}

trait ViewEntry {
    fn wxml(&self) -> &ProjectPath;
    fn script(&self) -> Option<&ProjectPath>;
}

impl ViewEntry for Page {
    fn wxml(&self) -> &ProjectPath {
        &self.wxml
    }

    fn script(&self) -> Option<&ProjectPath> {
        self.script.as_ref()
    }
}

impl ViewEntry for wxml_project::Component {
    fn wxml(&self) -> &ProjectPath {
        &self.wxml
    }

    fn script(&self) -> Option<&ProjectPath> {
        self.script.as_ref()
    }
}

fn dirty_scripts(state: &State, script: &ProjectPath) -> Vec<ProjectPath> {
    state
        .views
        .iter()
        .filter(|(_, view)| view.script.as_ref() == Some(script))
        .map(|(path, _)| path.clone())
        .collect()
}

fn bump_project_revision(state: &mut State) -> ProjectRevision {
    state.project_revision = state.project_revision.clone().next();
    state.project_revision.clone()
}

/// Keeps the owning script and applies the C1 `files[]` count/byte bounds.
fn limit_request_files(
    files: &mut Vec<PreparedFile>,
    script: &ProjectPath,
    hint: &mut Option<CapabilityStatus>,
) {
    const MAX_FILES: usize = 512;
    const MAX_BYTES: usize = 8 * 1024 * 1024;
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
    use super::*;

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
    fn project_id_does_not_contain_the_root_path() {
        let id = project_id_for(Path::new("/tmp/very/secret/project"));
        assert!(id.as_str().starts_with("proj-"));
        assert!(!id.as_str().contains("secret"));
        assert!(!id.as_str().contains('/'));
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
        for index in 0..600 {
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
        assert_eq!(files.len(), 512);
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
