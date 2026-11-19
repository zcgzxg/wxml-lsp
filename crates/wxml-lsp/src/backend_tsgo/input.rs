//! Host snapshot to native [`AnalysisInput`] conversion and shadow layout.
//!
//! The LSP owns the effective project text. This module asks the
//! [`ProjectHost`] for one prepared view, rebuilds the in-place shadow for the
//! owning script and validates that the target script is present before any
//! native process is started. A missing target file is an `Unavailable`
//! backend error; it never becomes an empty supported result.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use wxml_backend_tsgo::input::{
    AnalysisInput, CompilerOptions, DocumentRevision, Language, ProjectKey, ProjectRoot,
    SnapshotRevision, SourceFile, TypeEnvironmentIdentity,
};
use wxml_backend_tsgo::native::vfs::environment_degradations_for;
use wxml_backend_tsgo::shadow::guards::{
    GuardChain as ShadowGuardChain, GuardCondition as ShadowGuardCondition,
    GuardKind as ShadowGuardKind,
};
use wxml_backend_tsgo::shadow::layout::{
    FragmentSpec, JsCheckPolicy, RootKind as ShadowRootKind, ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::locals::{
    LocalBinding as ShadowLocalBinding, LocalRole as ShadowLocalRole,
};
use wxml_backend_tsgo::shadow::mapping::SourceRange as ShadowSourceRange;
use wxml_syntax::{Expr, ExprKind};
use wxml_types::{
    BackendError, CapabilityStatus, CheckFragment, Diagnostic, FileId, GuardBranch, GuardChain,
    LocalRole, ProjectRevision, RequestContext, RootKind, SnapshotId, SourceRevision,
    SourceSnapshot, ViewRevision,
};

use crate::backend_tsgo::environment::{Environment, degradation_reason};
use crate::backend_tsgo::mapping::{FragmentMap, ShadowLayout};
use crate::project_host::{HostError, PreparedView, ProjectHost};

/// Why a standalone script view could not be prepared.
///
/// The distinction is user-visible: an unsupported document keeps the
/// `Unsupported` capability semantics (it is not a project script), while a
/// host that cannot produce the project facts at all is `Unavailable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScriptViewError {
    /// The document is not an analyzable project script.
    Unsupported(String),
    /// The host could not provide the project script facts.
    Unavailable(String),
}

/// Supplies the effective project view for one template snapshot.
pub(crate) trait ViewProvider: Send + Sync {
    /// Prepares the view or returns a path-free host error.
    fn prepare(&self, snapshot: &SourceSnapshot) -> Result<PreparedView, HostError>;

    /// Prepares the project fact set for one standalone `.ts`/`.js` document.
    ///
    /// The returned view carries the project's effective script files and
    /// compiler options. Template fragments, template syntax diagnostics and
    /// `status_hint` are template-specific and must not be merged into a
    /// script result. The default rejects hosts that cannot answer for a
    /// standalone script.
    fn prepare_script(&self, _file: &FileId) -> Result<PreparedView, ScriptViewError> {
        Err(ScriptViewError::Unsupported(
            "script views are not provided by this host".to_owned(),
        ))
    }
}

/// Prepares one host view, mapping host failures to a typed backend error.
///
/// Shared by the shadow plan and the event-handler fast path so the
/// `HostError -> BackendError` mapping has one owner.
pub(crate) fn prepare_view(
    provider: &dyn ViewProvider,
    snapshot: &SourceSnapshot,
) -> Result<PreparedView, BackendError> {
    provider
        .prepare(snapshot)
        .map_err(|error| host_error(&error))
}

/// [`ViewProvider`] backed by the LSP [`ProjectHost`].
pub(crate) struct HostViewProvider {
    host: Arc<dyn ProjectHost>,
}

impl HostViewProvider {
    /// Creates a provider over the server's project host.
    #[must_use]
    pub(crate) fn new(host: Arc<dyn ProjectHost>) -> Self {
        Self { host }
    }
}

impl std::fmt::Debug for HostViewProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HostViewProvider").finish()
    }
}

impl ViewProvider for HostViewProvider {
    fn prepare(&self, snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        self.host.analysis(snapshot)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear host template selection and snapshot pipeline"
    )]
    fn prepare_script(&self, file: &FileId) -> Result<PreparedView, ScriptViewError> {
        if self.host.file_text(file).is_none() {
            return Err(ScriptViewError::Unsupported(
                "the script is not part of the open project".to_owned(),
            ));
        }
        let template = match self.host.view_for_script(file) {
            Ok(target) => target.wxml,
            Err(_) => self
                .host
                .view_templates(file)
                .and_then(|templates| {
                    templates
                        .templates
                        .iter()
                        .find_map(|template| self.host.view_for_file(&template.file).ok())
                        .map(|target| target.wxml)
                })
                .ok_or_else(|| {
                    ScriptViewError::Unsupported(
                        "the project has no page or component template".to_owned(),
                    )
                })?,
        };
        let template_file = self.host.file_id(&template).map_err(|_| {
            ScriptViewError::Unsupported(
                "the project template has no stable file identity".to_owned(),
            )
        })?;
        let snapshot = if let Some(snapshot) = self.host.source(&template_file) {
            snapshot
        } else {
            // The template is only used to obtain the project-wide script file
            // set and compiler options; a closed template contributes no
            // fragment data to a script result.
            let text = self.host.file_text(&template_file).ok_or_else(|| {
                ScriptViewError::Unsupported("the project template could not be read".to_owned())
            })?;
            let revision = self.host.file_revision(&template_file).unwrap_or(0);
            Arc::new(SourceSnapshot::new(
                template_file,
                SourceRevision::new(revision),
                SnapshotId::new(revision),
                text,
            ))
        };
        self.host.analysis(&snapshot).map_err(|error| match error {
            HostError::Unsupported { reason } => ScriptViewError::Unsupported(reason.to_owned()),
            HostError::NoProject | HostError::InvalidUri => ScriptViewError::Unsupported(
                "the script is not part of a discovered project view".to_owned(),
            ),
            HostError::ProjectOpen | HostError::Configuration(_) | HostError::Read => {
                ScriptViewError::Unavailable("the project view is unavailable".to_owned())
            }
        })
    }
}

/// Identity of one derived input snapshot.
///
/// The fields are the host revisions that can change the rendered shadow or
/// the project file set. Reusing a native snapshot is allowed only when all of
/// them are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestIdentity {
    /// Project revision supplied with the request.
    pub(crate) project_revision: ProjectRevision,
    /// WXML view revision supplied with the request.
    pub(crate) view_revision: ViewRevision,
    /// Template file identity.
    pub(crate) source: FileId,
    /// Template content revision.
    pub(crate) source_revision: SourceRevision,
    /// Template snapshot id.
    pub(crate) source_snapshot: SnapshotId,
    /// Owning script path.
    pub(crate) script_path: String,
    /// Host revision of the owning script.
    pub(crate) script_revision: u64,
    /// Effective script byte length.
    pub(crate) script_bytes: usize,
    /// Prepared-view identity, including its internal revision.
    pub(crate) view_id: String,
    /// Normalized project root.
    pub(crate) project_root: String,
    /// Number of fragments mounted in the shadow.
    pub(crate) fragment_count: usize,
    /// Configured type environment, if the adapter is trusted.
    ///
    /// `None` is the legacy no-asset mode; `Some` must stay byte-identical for
    /// an in-place `update_project` and forces a session restart when any
    /// asset root, pin field or compiler-option digest changes.
    pub(crate) environment: Option<TypeEnvironmentIdentity>,
}

/// One request plan before a native snapshot revision is assigned.
#[derive(Debug)]
pub(crate) struct RequestPlan {
    /// Input identity used for cache comparison.
    pub(crate) identity: RequestIdentity,
    project: ProjectKey,
    root: ProjectRoot,
    files: Vec<SourceFile>,
    /// Configured type environment for this plan.
    ///
    /// This is an enum, not an `Option` fallback: a trusted plan always builds
    /// an [`AnalysisInput::with_environment`] input.
    environment: Environment,
    /// Validated compiler options used by a trusted environment.
    compiler_options: CompilerOptions,
    /// Exact host file identity for every path sent to the native session.
    file_ids: BTreeMap<String, FileId>,
    /// Template-to-shadow mapping for this request.
    pub(crate) layout: ShadowLayout,
    /// Host parser/template diagnostics that must be merged into every
    /// diagnostics result, including unavailable ones.
    pub(crate) syntax_diagnostics: Vec<Diagnostic>,
    /// Stable degradation reason from host/shadow preparation, if any.
    pub(crate) degradation: Option<String>,
    /// Path-free type-environment degradation computed before any session
    /// exists.
    ///
    /// Only typed queries merge this reason; pure-syntax document symbols use
    /// [`Self::degradation`] alone so a project `types`/`typeRoots` setting
    /// cannot make them falsely `Partial`.
    pub(crate) environment_degradation: Option<String>,
}

impl RequestPlan {
    /// Builds the bounded native input at `revision`.
    pub(crate) fn input(&self, revision: SnapshotRevision) -> Result<AnalysisInput, BackendError> {
        let built = match &self.environment {
            Environment::Trusted(assets) => AnalysisInput::with_environment(
                self.project.clone(),
                self.root.clone(),
                revision,
                self.files.clone(),
                self.compiler_options.clone(),
                Arc::clone(assets),
            ),
            Environment::Legacy => AnalysisInput::new(
                self.project.clone(),
                self.root.clone(),
                revision,
                self.files.clone(),
            ),
        };
        built.map_err(|error| input_error(&error))
    }

    /// Returns the host file identity for a path sent to the native session.
    pub(crate) fn file_id(&self, path: &str) -> Option<FileId> {
        self.file_ids.get(path).cloned()
    }

    /// Returns the original text of a path sent to the native session.
    ///
    /// For the owning script this is the derived shadow text, which is exactly
    /// what the native engine saw; use [`ShadowLayout`] to map its ranges back.
    pub(crate) fn file_text<'a>(&'a self, path: &str) -> Option<&'a str> {
        self.files
            .iter()
            .find(|file| file.path() == path)
            .map(SourceFile::text)
    }
}

impl OpenPlan for RequestPlan {
    fn plan_identity(&self) -> PlanIdentity {
        PlanIdentity::Template(self.identity.clone())
    }

    fn input(&self, revision: SnapshotRevision) -> Result<AnalysisInput, BackendError> {
        Self::input(self, revision)
    }

    fn degradation(&self) -> Option<String> {
        self.degradation.clone()
    }

    fn environment_degradation(&self) -> Option<String> {
        self.environment_degradation.clone()
    }

    fn environment_identity(&self) -> Option<&TypeEnvironmentIdentity> {
        self.identity.environment.as_ref()
    }
}

/// Identity of one open native input, regardless of the request subject.
///
/// Template and script requests share the adapter's single session but must
/// never be treated as the same snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanIdentity {
    /// A WXML template plus its in-place shadow.
    Template(RequestIdentity),
    /// A standalone project script without a shadow.
    Script(ScriptIdentity),
}

impl PlanIdentity {
    /// Returns the configured type environment of this input.
    pub(crate) const fn environment(&self) -> Option<&TypeEnvironmentIdentity> {
        match self {
            Self::Template(identity) => identity.environment.as_ref(),
            Self::Script(identity) => identity.environment.as_ref(),
        }
    }
}

/// Identity of one standalone script input snapshot.
///
/// This deliberately has no fragment or template field: a script result must
/// not depend on the template that happened to provide the project file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScriptIdentity {
    /// Project revision supplied with the request.
    pub(crate) project_revision: ProjectRevision,
    /// View revision supplied with the request.
    pub(crate) view_revision: ViewRevision,
    /// Script file identity.
    pub(crate) source: FileId,
    /// Script content revision.
    pub(crate) source_revision: SourceRevision,
    /// Script snapshot id.
    pub(crate) source_snapshot: SnapshotId,
    /// Project-relative script path.
    pub(crate) script_path: String,
    /// Effective script byte length.
    pub(crate) script_bytes: usize,
    /// Stable hash of the project file paths and revisions sent to the engine.
    pub(crate) files_hash: u64,
    /// Normalized project root.
    pub(crate) project_root: String,
    /// Configured type environment, if the adapter is trusted.
    pub(crate) environment: Option<TypeEnvironmentIdentity>,
}

/// Operations the adapter's session owner needs from one input plan.
pub(crate) trait OpenPlan {
    /// Returns the identity used to decide whether an update is needed.
    fn plan_identity(&self) -> PlanIdentity;
    /// Builds the bounded native input at `revision`.
    fn input(&self, revision: SnapshotRevision) -> Result<AnalysisInput, BackendError>;
    /// Host degradation that applies to this request.
    fn degradation(&self) -> Option<String>;
    /// Path-free type-environment degradation for typed requests.
    fn environment_degradation(&self) -> Option<String>;
    /// Configured type environment of this input.
    fn environment_identity(&self) -> Option<&TypeEnvironmentIdentity>;
}

/// One standalone script request plan before a native revision is assigned.
#[derive(Debug)]
pub(crate) struct ScriptRequestPlan {
    /// Input identity used for cache comparison.
    pub(crate) identity: ScriptIdentity,
    project: ProjectKey,
    root: ProjectRoot,
    files: Vec<SourceFile>,
    environment: Environment,
    compiler_options: CompilerOptions,
    /// Project-relative path of the requested script.
    pub(crate) path: String,
    /// Stable degradation reason from host preparation, if any.
    pub(crate) degradation: Option<String>,
    /// Path-free type-environment degradation computed before any session.
    pub(crate) environment_degradation: Option<String>,
}

impl ScriptRequestPlan {
    /// Builds the bounded native input at `revision`.
    pub(crate) fn input(&self, revision: SnapshotRevision) -> Result<AnalysisInput, BackendError> {
        let built = match &self.environment {
            Environment::Trusted(assets) => AnalysisInput::with_environment(
                self.project.clone(),
                self.root.clone(),
                revision,
                self.files.clone(),
                self.compiler_options.clone(),
                Arc::clone(assets),
            ),
            Environment::Legacy => AnalysisInput::new(
                self.project.clone(),
                self.root.clone(),
                revision,
                self.files.clone(),
            ),
        };
        built.map_err(|error| input_error(&error))
    }
}

impl OpenPlan for ScriptRequestPlan {
    fn plan_identity(&self) -> PlanIdentity {
        PlanIdentity::Script(self.identity.clone())
    }

    fn input(&self, revision: SnapshotRevision) -> Result<AnalysisInput, BackendError> {
        Self::input(self, revision)
    }

    fn degradation(&self) -> Option<String> {
        self.degradation.clone()
    }

    fn environment_degradation(&self) -> Option<String> {
        self.environment_degradation.clone()
    }

    fn environment_identity(&self) -> Option<&TypeEnvironmentIdentity> {
        self.identity.environment.as_ref()
    }
}

/// Builds one request plan from the host view and the in-place shadow.
#[allow(clippy::too_many_lines, reason = "one linear input/shadow pipeline")]
pub(crate) fn build_request_plan(
    provider: &dyn ViewProvider,
    context: &RequestContext,
    environment: &Environment,
) -> Result<RequestPlan, BackendError> {
    let snapshot = context.source();
    let view = prepare_view(provider, snapshot)?;
    let target = view.target.as_ref().ok_or_else(|| {
        BackendError::Unavailable(
            "project view has no page or component script for this template".to_owned(),
        )
    })?;
    let script_path = target.script.as_str().to_owned();
    let script = view
        .files
        .iter()
        .find(|file| file.path.as_str() == script_path)
        .ok_or_else(|| {
            BackendError::Unavailable(
                "the owning script is missing from the prepared view".to_owned(),
            )
        })?;
    if Language::from_path(&script_path).is_none() {
        return Err(BackendError::Unavailable(
            "the owning script uses a language the native input cannot represent".to_owned(),
        ));
    }
    let original = SourceFile::new(
        script.path.as_str(),
        script.text.to_string(),
        DocumentRevision::new(script.source_revision),
    )
    .map_err(|error| input_error(&error))?;

    let (included, omitted_literal, omitted_malformed) = eligible_fragments(&view.fragments);
    let fragments = included
        .iter()
        .map(|fragment| fragment_spec(fragment))
        .collect::<Vec<_>>();
    let root = match target.owner {
        RootKind::Page => ShadowRootKind::Page,
        RootKind::Component => ShadowRootKind::Component,
    };
    let check_js = view
        .compiler_options
        .get("checkJs")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let built = build_in_place_shadow(&ShadowRequest {
        script: &original,
        root,
        fragments: &fragments,
        js_check: JsCheckPolicy::Inherit,
        check_js,
    })
    .map_err(|error| shadow_error(&error))?;

    let mut degradation = None;
    merge_reason(
        &mut degradation,
        view.status_hint
            .as_ref()
            .and_then(CapabilityStatus::reason)
            .map(str::to_owned),
    );
    if !view.source_complete {
        merge_reason(
            &mut degradation,
            Some("the prepared source file list is incomplete".to_owned()),
        );
    }
    if let Some(reason) = built.reason.as_deref() {
        merge_reason(
            &mut degradation,
            Some(format!("shadow coverage is partial: {reason}")),
        );
    }
    if omitted_literal > 0 {
        merge_reason(
            &mut degradation,
            Some(format!(
                "{omitted_literal} template fragment(s) without a free-root identifier were omitted"
            )),
        );
    }
    if omitted_malformed > 0 {
        merge_reason(
            &mut degradation,
            Some(format!(
                "{omitted_malformed} malformed template fragment(s) were omitted"
            )),
        );
    }

    let mut fragment_maps = Vec::with_capacity(included.len());
    for fragment in included {
        let Some(mount) = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == fragment.fragment_id)
        else {
            merge_reason(
                &mut degradation,
                Some(format!(
                    "template fragment `{}` was not mounted in the shadow",
                    fragment.fragment_id
                )),
            );
            continue;
        };
        fragment_maps.push(FragmentMap {
            wxml_range: fragment.wxml_range,
            mount: mount.clone(),
        });
    }

    let mut files = Vec::with_capacity(view.files.len());
    let mut file_ids = BTreeMap::new();
    for prepared in view.files.iter() {
        let path = prepared.path.as_str();
        let target_file = path == script_path;
        let text = if target_file {
            built.view.text.clone()
        } else {
            prepared.text.to_string()
        };
        if Language::from_path(path).is_none() {
            if target_file {
                return Err(BackendError::Unavailable(
                    "the owning script language is not supported by the native input".to_owned(),
                ));
            }
            merge_reason(
                &mut degradation,
                Some(format!(
                    "script `{path}` was omitted because its language is not supported"
                )),
            );
            continue;
        }
        match SourceFile::new(path, text, DocumentRevision::new(prepared.source_revision)) {
            Ok(file) => {
                file_ids.insert(path.to_owned(), prepared.file.clone());
                files.push(file);
            }
            Err(error) => {
                if target_file {
                    return Err(input_error(&error));
                }
                merge_reason(
                    &mut degradation,
                    Some(format!("script `{path}` was omitted: {error}")),
                );
            }
        }
    }

    let root = ProjectRoot::new(view.project_root.as_os_str().to_str().ok_or_else(|| {
        BackendError::Unavailable("the project root is not valid UTF-8".to_owned())
    })?)
    .map_err(|error| input_error(&error))?;
    let project = ProjectKey::new(context.file().project().as_str());
    let (compiler_options, environment_identity) =
        environment_options(environment, &view.compiler_options)?;
    let environment_degradation =
        environment_degradation(environment, &root, &files, &compiler_options);
    let identity = RequestIdentity {
        project_revision: context.project_revision(),
        view_revision: context.view_revision(),
        source: context.file().clone(),
        source_revision: snapshot.revision(),
        source_snapshot: snapshot.snapshot(),
        script_path: script_path.clone(),
        script_revision: script.source_revision,
        script_bytes: built.view.text.len(),
        view_id: view.view_id.clone(),
        project_root: root.as_str().to_owned(),
        fragment_count: fragment_maps.len(),
        environment: environment_identity,
    };

    Ok(RequestPlan {
        identity,
        project,
        root,
        files,
        environment: environment.clone(),
        compiler_options,
        file_ids,
        layout: ShadowLayout::new(
            snapshot.text().to_owned(),
            built.view.text,
            built.view.map,
            fragment_maps,
        ),
        syntax_diagnostics: view.syntax_diagnostics.clone(),
        degradation,
        environment_degradation,
    })
}

/// Builds one standalone script plan without a template shadow.
///
/// The target text is the request snapshot's text, not the host's prepared
/// copy, so the engine checks exactly the document the client sees. Other
/// project files keep their effective prepared text. Template fragments and
/// template syntax diagnostics are intentionally absent, and the template's
/// degradation hint is not merged.
#[allow(clippy::too_many_lines, reason = "one linear script input pipeline")]
pub(crate) fn build_script_request_plan(
    provider: &dyn ViewProvider,
    context: &RequestContext,
    environment: &Environment,
) -> Result<ScriptRequestPlan, ScriptViewError> {
    let snapshot = context.source();
    let path = context.file().path().to_owned();
    if Language::from_path(&path).is_none() {
        return Err(ScriptViewError::Unsupported(
            "only TypeScript and JavaScript project scripts are analyzed".to_owned(),
        ));
    }
    let view = provider.prepare_script(context.file())?;
    let mut degradation = None;
    if !view.source_complete {
        merge_reason(
            &mut degradation,
            Some("the prepared project source list is incomplete".to_owned()),
        );
    }
    if view.files.is_empty() {
        return Err(ScriptViewError::Unavailable(
            "the project has no analyzable script sources".to_owned(),
        ));
    }

    let mut files = Vec::with_capacity(view.files.len() + 1);
    let mut target_found = false;
    for prepared in view.files.iter() {
        let file_path = prepared.path.as_str();
        if Language::from_path(file_path).is_none() {
            merge_reason(
                &mut degradation,
                Some(format!(
                    "script `{file_path}` was omitted because its language is not supported"
                )),
            );
            continue;
        }
        let target_file = file_path == path;
        let text = if target_file {
            target_found = true;
            snapshot.text().to_owned()
        } else {
            prepared.text.to_string()
        };
        let revision = if target_file {
            snapshot.revision().get()
        } else {
            prepared.source_revision
        };
        match SourceFile::new(file_path, text, DocumentRevision::new(revision)) {
            Ok(file) => files.push(file),
            Err(error) => {
                if target_file {
                    return Err(ScriptViewError::Unavailable(
                        "the requested script is not representable by the native input".to_owned(),
                    ));
                }
                merge_reason(
                    &mut degradation,
                    Some(format!("script `{file_path}` was omitted: {error}")),
                );
            }
        }
    }

    if !target_found {
        if view.source_complete {
            return Err(ScriptViewError::Unavailable(
                "the requested script is missing from the prepared project sources".to_owned(),
            ));
        }
        // The host reported an incomplete list, so the target may simply have
        // been dropped by a budget. The request snapshot is authoritative and
        // is added explicitly; the incomplete list stays a Partial reason.
        let target = SourceFile::new(
            path.clone(),
            snapshot.text().to_owned(),
            DocumentRevision::new(snapshot.revision().get()),
        )
        .map_err(|_| {
            ScriptViewError::Unavailable(
                "the requested script is not representable by the native input".to_owned(),
            )
        })?;
        files.push(target);
        merge_reason(
            &mut degradation,
            Some(
                "the requested script was missing from an incomplete project source list"
                    .to_owned(),
            ),
        );
    }

    let root = ProjectRoot::new(view.project_root.as_os_str().to_str().ok_or_else(|| {
        ScriptViewError::Unavailable("the project root is not valid UTF-8".to_owned())
    })?)
    .map_err(|_| {
        ScriptViewError::Unavailable(
            "the project root is not supported by the native input".to_owned(),
        )
    })?;
    let project = ProjectKey::new(context.file().project().as_str());
    let (compiler_options, environment_identity) =
        environment_options(environment, &view.compiler_options).map_err(|_| {
            ScriptViewError::Unavailable(
                "project compiler options are not supported by the native input".to_owned(),
            )
        })?;
    let environment_degradation =
        environment_degradation(environment, &root, &files, &compiler_options);
    let identity = ScriptIdentity {
        project_revision: context.project_revision(),
        view_revision: context.view_revision(),
        source: context.file().clone(),
        source_revision: snapshot.revision(),
        source_snapshot: snapshot.snapshot(),
        script_path: path.clone(),
        script_bytes: snapshot.text().len(),
        files_hash: input_files_hash(&files),
        project_root: root.as_str().to_owned(),
        environment: environment_identity,
    };
    Ok(ScriptRequestPlan {
        identity,
        project,
        root,
        files,
        environment: environment.clone(),
        compiler_options,
        path,
        degradation,
        environment_degradation,
    })
}

/// Computes the path-free type-environment degradation for one input file set.
fn environment_degradation(
    environment: &Environment,
    root: &ProjectRoot,
    files: &[SourceFile],
    compiler_options: &CompilerOptions,
) -> Option<String> {
    environment.assets().and_then(|assets| {
        let absolute = files
            .iter()
            .map(|file| format!("{}/{}", root.as_str(), file.path()))
            .collect::<Vec<_>>();
        degradation_reason(&environment_degradations_for(
            root.as_str(),
            &absolute,
            compiler_options,
            assets,
        ))
    })
}

/// Hashes the sorted `(path, revision)` pairs of one input file set.
fn input_files_hash(files: &[SourceFile]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut entries = files
        .iter()
        .map(|file| (file.path(), file.revision().get()))
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut hash = FNV_OFFSET_BASIS;
    for (path, revision) in entries {
        for byte in path.as_bytes().iter().copied().chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        for byte in revision.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

/// Fragments that can be mounted, plus counts of the two rejection reasons.
fn eligible_fragments(fragments: &[CheckFragment]) -> (Vec<&CheckFragment>, usize, usize) {
    let mut included = Vec::new();
    let mut omitted_literal = 0;
    let mut omitted_malformed = 0;
    for fragment in fragments {
        match fragment_eligibility(fragment) {
            FragmentEligibility::Eligible => included.push(fragment),
            FragmentEligibility::NoFreeRoot => omitted_literal += 1,
            FragmentEligibility::Malformed => omitted_malformed += 1,
        }
    }
    (included, omitted_literal, omitted_malformed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragmentEligibility {
    Eligible,
    NoFreeRoot,
    Malformed,
}

fn fragment_eligibility(fragment: &CheckFragment) -> FragmentEligibility {
    let parsed = wxml_syntax::parse_expression(&fragment.expr_text, 0);
    if !parsed.errors.is_empty() || parsed.expr.walk().any(Expr::is_missing) {
        return FragmentEligibility::Malformed;
    }
    let locals: HashSet<&str> = fragment
        .bindings
        .iter()
        .map(|binding| binding.name.as_str())
        .collect();
    if has_free_root(&parsed.expr, &fragment.expr_text, &locals) {
        FragmentEligibility::Eligible
    } else {
        FragmentEligibility::NoFreeRoot
    }
}

/// Nodes that are values in JavaScript and cannot be template data roots.
const NODE_GLOBALS: &[&str] = &[
    "Math",
    "JSON",
    "Date",
    "Number",
    "String",
    "Boolean",
    "Array",
    "Object",
    "RegExp",
    "Symbol",
    "BigInt",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Promise",
    "console",
    "Intl",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "encodeURI",
    "decodeURI",
    "encodeURIComponent",
    "decodeURIComponent",
    "undefined",
    "NaN",
    "Infinity",
];

/// ECMAScript reserved words that cannot be a free expression root.
const RESERVED_ROOTS: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "await",
];

fn has_free_root(expr: &Expr, source: &str, locals: &HashSet<&str>) -> bool {
    match expr.kind() {
        ExprKind::Identifier { name } => slice_relative(source, *name).is_some_and(|text| {
            locals.contains(text)
                || (!NODE_GLOBALS.contains(&text) && !RESERVED_ROOTS.contains(&text))
        }),
        ExprKind::String { .. }
        | ExprKind::Number
        | ExprKind::Boolean(_)
        | ExprKind::Null
        | ExprKind::Missing => false,
        ExprKind::Member { object, .. } => has_free_root(object, source, locals),
        _ => expr
            .children()
            .any(|child| has_free_root(child, source, locals)),
    }
}

fn slice_relative(source: &str, range: wxml_text::TextRange) -> Option<&str> {
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    source.get(start..end)
}

fn fragment_spec(fragment: &CheckFragment) -> FragmentSpec {
    FragmentSpec {
        id: fragment.fragment_id.clone(),
        expr: fragment.expr_text.clone(),
        wxml_range: ShadowSourceRange::new(fragment.wxml_range.start(), fragment.wxml_range.end()),
        target: fragment.target.clone(),
        locals: fragment
            .bindings
            .iter()
            .map(|binding| binding.name.clone())
            .collect(),
        bindings: fragment.bindings.iter().map(local_binding).collect(),
        guards: fragment.guards.iter().map(guard_chain).collect(),
    }
}

fn local_binding(binding: &wxml_types::LocalBinding) -> ShadowLocalBinding {
    ShadowLocalBinding {
        name: binding.name.clone(),
        role: match binding.role {
            LocalRole::Index => ShadowLocalRole::Index,
            // `LocalRole` is non-exhaustive. Item is the common binding and
            // the conservative fallback for a future role.
            _ => ShadowLocalRole::Item,
        },
        source_expr: binding.source_expr.clone(),
    }
}

fn guard_chain(chain: &GuardChain) -> ShadowGuardChain {
    ShadowGuardChain {
        conditions: chain
            .conditions
            .iter()
            .map(|condition| ShadowGuardCondition {
                branch: match condition.branch {
                    GuardBranch::If => ShadowGuardKind::If,
                    GuardBranch::Elif => ShadowGuardKind::Elif,
                    // `GuardBranch` is non-exhaustive. A future branch cannot
                    // be narrowed safely; `Else` avoids inventing a true
                    // condition for it.
                    _ => ShadowGuardKind::Else,
                },
                expr: condition.expr_text.clone(),
            })
            .collect(),
        selected: chain.selected,
    }
}

/// Validates the view's compiler options for a trusted environment.
///
/// Legacy plans keep the historical empty option set so their behavior is
/// unchanged. A trusted plan converts every option before any session exists;
/// an unsupported key or value fails the request instead of silently dropping
/// configuration.
fn environment_options(
    environment: &Environment,
    view_options: &serde_json::Map<String, serde_json::Value>,
) -> Result<(CompilerOptions, Option<TypeEnvironmentIdentity>), BackendError> {
    if !environment.is_trusted() {
        return Ok((CompilerOptions::default(), None));
    }
    let values = view_options
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let options = CompilerOptions::new(values).map_err(|error| {
        BackendError::Unavailable(format!(
            "project compiler options are not supported by the native input: {error}"
        ))
    })?;
    let identity = environment.identity(&options);
    Ok((options, identity))
}

pub(crate) fn merge_reason(target: &mut Option<String>, reason: Option<String>) {
    let Some(reason) = reason else {
        return;
    };
    match target {
        Some(existing) => {
            if !existing.contains(&reason) {
                existing.push_str("; ");
                existing.push_str(&reason);
            }
        }
        None => *target = Some(reason),
    }
}

fn host_error(error: &HostError) -> BackendError {
    BackendError::Unavailable(format!("project view is unavailable: {error}"))
}

pub(crate) fn input_error(error: &wxml_backend_tsgo::input::InputError) -> BackendError {
    BackendError::Unavailable(format!("native input is unavailable: {error}"))
}

fn shadow_error(error: &wxml_backend_tsgo::shadow::ShadowError) -> BackendError {
    BackendError::Unavailable(format!("shadow layout is unavailable: {error}"))
}
