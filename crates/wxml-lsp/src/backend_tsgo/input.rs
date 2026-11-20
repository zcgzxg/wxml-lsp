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
use wxml_backend_tsgo::native::strings::{
    Utf16Offset, Utf16Position, offset_to_position, utf16_len,
};
use wxml_backend_tsgo::native::vfs::environment_degradations_for;
use wxml_backend_tsgo::script::behavior::BehaviorProperty;
use wxml_backend_tsgo::script::registration::RegistrationKind as ScriptRegistrationKind;
use wxml_backend_tsgo::script::{ScriptRequest, analyze_script};
use wxml_backend_tsgo::shadow::guards::{
    GuardChain as ShadowGuardChain, GuardCondition as ShadowGuardCondition,
    GuardKind as ShadowGuardKind,
};
use wxml_backend_tsgo::shadow::layout::{
    BehaviorPropertyOverlay, FragmentSpec, JsCheckPolicy, RootKind as ShadowRootKind,
    ShadowRequest, build_in_place_shadow_with_behavior,
};
use wxml_backend_tsgo::shadow::locals::{
    LocalBinding as ShadowLocalBinding, LocalRole as ShadowLocalRole,
};
use wxml_backend_tsgo::shadow::mapping::SourceRange as ShadowSourceRange;
use wxml_backend_tsgo::shadow::mapping::utf16_to_byte;
use wxml_syntax::{Expr, ExprKind};
use wxml_types::{
    BackendError, CapabilityStatus, CheckFragment, Diagnostic, EventHandler, FileId, GuardBranch,
    GuardChain, LocalRole, ProjectRevision, RequestContext, RootKind, SnapshotId, SourceRevision,
    SourceSnapshot, ViewRevision,
};

use crate::backend_tsgo::environment::{Environment, degradation_reason};
use crate::backend_tsgo::hover::DeclaredPropertyType;
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

/// Why one template request plan could not be built.
///
/// The split is user-visible and must not be collapsed: an owner shape the
/// shadow deliberately cannot analyze (for example a non-literal `Page(...)`
/// registration) is a static language limit and keeps `Unsupported`
/// semantics, while a host or native-input failure stays `Unavailable` so a
/// real backend failure is never reported as an unsupported language form.
/// Reasons are path-free: they describe the owner shape, never project paths.
#[derive(Debug)]
pub(crate) enum PlanError {
    /// The template owner has no statically analyzable shape.
    Unsupported(String),
    /// The host or the native input layer could not provide a plan.
    Backend(BackendError),
}

impl From<BackendError> for PlanError {
    fn from(error: BackendError) -> Self {
        Self::Backend(error)
    }
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
/// Shared by the shadow plan and every derived request plan so the
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
    /// Content-aware fingerprint of every file sent to the native engine.
    ///
    /// The fingerprint covers path, document revision and text, including
    /// non-owner overlay files and the derived owner shadow. It makes a
    /// non-owner overlay change visible even when the request context's
    /// project/view revisions and the prepared `view_id` are unchanged.
    pub(crate) files_hash: u64,
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

/// Synthetic fragment id used by an incomplete-member completion plan.
pub(crate) const COMPLETION_FRAGMENT_ID: &str = "completion#0";
/// Synthetic fragment id used by an event-handler point plan.
///
/// A point plan mounts at most this one member fragment, and only for the
/// handler token under the cursor. It never appears in diagnostics or
/// completion plans.
pub(crate) const EVENT_POINT_FRAGMENT_ID: &str = "event-point#0";
/// Maximum handler-name byte length mounted as a point member fragment.
///
/// The template analyzer only emits plain method names, so this is a defensive
/// bound against a stale or hand-built view rather than a language rule.
const MAX_EVENT_POINT_HANDLER_BYTES: usize = 256;
/// Synthetic suffix inserted after the mounted completion prefix.
///
/// Only the inserted `.` is queried: the engine sees a member-access position
/// while every published range still comes from the original template bytes.
const COMPLETION_QUERY_SUFFIX: &str = ".__wxml_completion";

/// One valid expression prefix plus its exact original template bytes.
///
/// The template analyzer drops `{{ user. }}` because the expression is not
/// valid. Completion mounts the valid prefix (`user`) and appends a synthetic
/// member query, so the published replacement range never changes.
#[derive(Debug, Clone)]
pub(crate) struct CompletionFragment {
    /// Valid expression before the trailing dot.
    pub(crate) expr: String,
    /// Exact original byte range of `expr`.
    pub(crate) wxml_range: wxml_types::SourceRange,
    /// Effective `wx:for` bindings at the completion cursor, outermost first.
    ///
    /// The shadow renderer turns these into nested declarations, so a template
    /// local such as `item` resolves its real element type instead of falling
    /// back to a `data` root. Completion carries no guards, so the declarations
    /// only affect types and never narrow the expression.
    pub(crate) bindings: Vec<wxml_types::LocalBinding>,
}

/// One static event handler under a point-query cursor.
///
/// The member fragment mounts `this.<handler>`; only the handler name maps
/// back to the original WXML token, and the synthetic `this.` prefix has no
/// source range. The fragment is added only when the cursor is inside the
/// handler token, so attribute names, whitespace, mustaches and dynamic
/// handlers never grow the shadow.
#[derive(Debug, Clone)]
struct EventPointFragment {
    /// Static handler name exactly as written in the template.
    handler: String,
    /// Byte range of the handler token in the original WXML.
    wxml_range: wxml_types::SourceRange,
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
    /// Explicit derived UTF-16 query position for a synthetic completion plan.
    ///
    /// Normal plans leave this `None`; completion then maps the template
    /// offset through the shadow itself.
    pub(crate) completion_query: Option<Utf16Position>,
    /// Declaration-derived property types used to keep hover fidelity for
    /// `type` + `optionalTypes` unions the native checker under-reports.
    pub(crate) declared_properties: BTreeMap<String, DeclaredPropertyType>,
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
    /// Host-prepared view used by the component-property binding pass.
    ///
    /// Hover/definition/completion never read it; only template diagnostics
    /// resolve component declarations and attribute mounts.
    pub(crate) view: PreparedView,
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
    /// Content-aware hash of the project file paths, revisions and text sent
    /// to the engine.
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

/// Behavior facts needed by the shadow and the hover overlay.
#[derive(Debug, Default)]
struct ScriptFacts {
    /// Static behavior contribution; `None` when the script has no
    /// `behaviors` member.
    behavior: Option<BehaviorFacts>,
    /// Declaration-derived property types for hover fidelity.
    declared_properties: BTreeMap<String, DeclaredPropertyType>,
}

/// Owned behavior properties plus their resolution completeness.
#[derive(Debug)]
struct BehaviorFacts {
    properties: Vec<BehaviorProperty>,
    complete: bool,
    reason: Option<String>,
}

impl BehaviorFacts {
    fn overlay(&self) -> BehaviorPropertyOverlay<'_> {
        BehaviorPropertyOverlay {
            properties: &self.properties,
            complete: self.complete,
            reason: self.reason.as_deref(),
        }
    }
}

/// Resolves the script overlay once per request plan.
///
/// The dedicated `script` module already owns registration selection, the
/// behavior graph and the `type` + `optionalTypes` union. Running it here keeps
/// the in-place shadow and the hover overlay on that single implementation
/// instead of re-guessing behavior properties inside the shadow builder.
///
/// The analysis runs over the effective project text (owning script plus every
/// prepared project file) and is bounded by the script module's member,
/// behavior, depth and file budgets. When the script has neither `properties`
/// nor `behaviors`, the scan is skipped.
fn collect_script_facts(
    view: &PreparedView,
    project: &ProjectKey,
    project_root: &ProjectRoot,
    script_path: &str,
    shadow_root: ShadowRootKind,
) -> ScriptFacts {
    let Some(script) = view
        .files
        .iter()
        .find(|file| file.path.as_str() == script_path)
    else {
        return ScriptFacts::default();
    };
    let has_properties = script.text.contains("properties");
    let has_behaviors = script.text.contains("behaviors");
    if !has_properties && !has_behaviors {
        return ScriptFacts::default();
    }

    let files = view
        .files
        .iter()
        .filter(|file| Language::from_path(file.path.as_str()).is_some())
        .filter_map(|file| {
            SourceFile::new(
                file.path.as_str(),
                file.text.to_string(),
                DocumentRevision::new(file.source_revision),
            )
            .ok()
        })
        .collect::<Vec<_>>();
    let Ok(input) = AnalysisInput::new(
        project.clone(),
        project_root.clone(),
        SnapshotRevision::new(0),
        files,
    ) else {
        return ScriptFacts {
            behavior: has_behaviors.then(|| BehaviorFacts {
                properties: Vec::new(),
                complete: false,
                reason: Some("the script overlay could not be built from this project".to_owned()),
            }),
            declared_properties: BTreeMap::new(),
        };
    };
    let kind = match shadow_root {
        ShadowRootKind::Page => ScriptRegistrationKind::Page,
        ShadowRootKind::Component | ShadowRootKind::Behavior => ScriptRegistrationKind::Component,
    };
    let Ok(analysis) = analyze_script(&ScriptRequest::new(&input, script_path, kind)) else {
        return ScriptFacts {
            behavior: has_behaviors.then(|| BehaviorFacts {
                properties: Vec::new(),
                complete: false,
                reason: Some("the script overlay could not be analyzed".to_owned()),
            }),
            declared_properties: BTreeMap::new(),
        };
    };

    let overlay_complete = analysis.properties.complete;
    let declared_properties = analysis
        .properties
        .properties
        .iter()
        .map(|property| {
            (
                property.name.clone(),
                DeclaredPropertyType {
                    type_text: property.type_text.clone(),
                    complete: property.type_complete && overlay_complete,
                },
            )
        })
        .collect();
    let behavior = has_behaviors.then(|| BehaviorFacts {
        properties: analysis.behavior.properties.clone(),
        complete: !analysis.behavior.properties_incomplete
            && analysis.behavior.status.is_supported(),
        reason: analysis.behavior.status.reason().map(str::to_owned),
    });
    ScriptFacts {
        behavior,
        declared_properties,
    }
}

/// Builds one request plan from the host view and the in-place shadow.
pub(crate) fn build_request_plan(
    provider: &dyn ViewProvider,
    context: &RequestContext,
    environment: &Environment,
) -> Result<RequestPlan, PlanError> {
    build_request_plan_with_completion(provider, context, environment, None, None)
}

/// Builds one point-query plan for the event handler under `offset`.
///
/// When the offset is inside a static handler token of the prepared view, the
/// shadow mounts exactly one member fragment (`this.<handler>`) and maps the
/// name back to the original WXML token. Every other offset plan is identical
/// to [`build_request_plan`], so hover/definition outside an event value keeps
/// the existing expression behavior.
pub(crate) fn build_point_request_plan(
    provider: &dyn ViewProvider,
    context: &RequestContext,
    environment: &Environment,
    offset: u32,
) -> Result<RequestPlan, PlanError> {
    build_request_plan_with_completion(provider, context, environment, None, Some(offset))
}

/// Builds one completion-only plan for an incomplete trailing-dot expression.
///
/// The plan mounts the valid expression prefix and appends a synthetic member
/// query. It deliberately differs in identity from the normal plan (extra
/// fragment and script bytes), so the session is updated for the query and
/// updated back by the next non-completion request.
pub(crate) fn build_completion_request_plan(
    provider: &dyn ViewProvider,
    context: &RequestContext,
    environment: &Environment,
    fragment: &CompletionFragment,
) -> Result<RequestPlan, PlanError> {
    build_request_plan_with_completion(provider, context, environment, Some(fragment), None)
}

/// Builds one request plan with an optional completion fragment or point offset.
///
/// `completion` and `point` are mutually exclusive: completion is an incomplete
/// expression plan, while `point` may append one event-handler member fragment
/// when the cursor is inside a static handler token.
#[allow(clippy::too_many_lines, reason = "one linear input/shadow pipeline")]
fn build_request_plan_with_completion(
    provider: &dyn ViewProvider,
    context: &RequestContext,
    environment: &Environment,
    completion: Option<&CompletionFragment>,
    point: Option<u32>,
) -> Result<RequestPlan, PlanError> {
    let snapshot = context.source();
    let view = prepare_view(provider, snapshot)?;
    let event_point = point.and_then(|offset| event_point_fragment(&view, snapshot.text(), offset));
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
        return Err(PlanError::Backend(BackendError::Unavailable(
            "the owning script uses a language the native input cannot represent".to_owned(),
        )));
    }
    let original = SourceFile::new(
        script.path.as_str(),
        script.text.to_string(),
        DocumentRevision::new(script.source_revision),
    )
    .map_err(|error| input_error(&error))?;

    let (included, omitted_literal, omitted_malformed) = eligible_fragments(&view.fragments);
    let mut fragments = included
        .iter()
        .map(|fragment| fragment_spec(fragment))
        .collect::<Vec<_>>();
    if let Some(fragment) = completion {
        let spec = completion_fragment_spec(fragment);
        fragments.push(spec);
    }
    if let Some(fragment) = &event_point {
        fragments.push(event_point_fragment_spec(fragment));
    }
    let shadow_root = match target.owner {
        RootKind::Page => ShadowRootKind::Page,
        RootKind::Component => ShadowRootKind::Component,
    };
    let check_js = view
        .compiler_options
        .get("checkJs")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let project_root =
        ProjectRoot::new(view.project_root.as_os_str().to_str().ok_or_else(|| {
            BackendError::Unavailable("the project root is not valid UTF-8".to_owned())
        })?)
        .map_err(|error| input_error(&error))?;
    let project = ProjectKey::new(context.file().project().as_str());
    let facts = collect_script_facts(&view, &project, &project_root, &script_path, shadow_root);
    let behavior_overlay = facts.behavior.as_ref().map(BehaviorFacts::overlay);
    let built = build_in_place_shadow_with_behavior(
        &ShadowRequest {
            script: &original,
            root: shadow_root,
            fragments: &fragments,
            js_check: JsCheckPolicy::Inherit,
            check_js,
        },
        behavior_overlay.as_ref(),
    )
    .map_err(|error| shadow_error(&error))?;

    // A completion plan mounts the valid expression prefix and appends a
    // synthetic `.` plus placeholder. The query sits after the inserted dot, so
    // the engine sees `__wxml_root.user.|` without any original byte changing.
    let (derived, completion_query) = if completion.is_some() {
        let mut derived = built.view.text.clone();
        let mount = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == COMPLETION_FRAGMENT_ID)
            .ok_or_else(|| {
                BackendError::Unavailable(
                    "the completion expression was not mounted in the shadow".to_owned(),
                )
            })?;
        let end = mount
            .start_utf16
            .checked_add(utf16_len(&mount.text))
            .ok_or_else(|| BackendError::Internal("completion query offset overflow".to_owned()))?;
        let byte = utf16_to_byte(&derived, end).map_err(|_| {
            BackendError::Internal("completion query offset is invalid in the shadow".to_owned())
        })?;
        derived.insert_str(byte, COMPLETION_QUERY_SUFFIX);
        let query = end.checked_add(1).ok_or_else(|| {
            BackendError::Internal("completion query position overflow".to_owned())
        })?;
        let position = offset_to_position(&derived, Utf16Offset::new(query)).map_err(|_| {
            BackendError::Internal("completion query position is invalid in the shadow".to_owned())
        })?;
        (derived, Some(position))
    } else {
        (built.view.text.clone(), None)
    };

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
    if let Some(fragment) = completion {
        if let Some(mount) = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == COMPLETION_FRAGMENT_ID)
        {
            fragment_maps.push(FragmentMap {
                wxml_range: fragment.wxml_range,
                mount: mount.clone(),
            });
        }
    }
    if let Some(fragment) = &event_point {
        if let Some(mount) = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == EVENT_POINT_FRAGMENT_ID)
        {
            fragment_maps.push(FragmentMap {
                wxml_range: fragment.wxml_range,
                mount: mount.clone(),
            });
        } else {
            merge_reason(
                &mut degradation,
                Some("the event-handler point fragment was not mounted in the shadow".to_owned()),
            );
        }
    }

    let mut files = Vec::with_capacity(view.files.len());
    let mut file_ids = BTreeMap::new();
    for prepared in view.files.iter() {
        let path = prepared.path.as_str();
        let target_file = path == script_path;
        let text = if target_file {
            derived.clone()
        } else {
            prepared.text.to_string()
        };
        if Language::from_path(path).is_none() {
            if target_file {
                return Err(PlanError::Backend(BackendError::Unavailable(
                    "the owning script language is not supported by the native input".to_owned(),
                )));
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
                    return Err(PlanError::Backend(input_error(&error)));
                }
                merge_reason(
                    &mut degradation,
                    Some(format!("script `{path}` was omitted: {error}")),
                );
            }
        }
    }

    let (compiler_options, environment_identity) =
        environment_options(environment, &view.compiler_options)?;
    let environment_degradation =
        environment_degradation(environment, &project_root, &files, &compiler_options);
    let identity = RequestIdentity {
        project_revision: context.project_revision(),
        view_revision: context.view_revision(),
        source: context.file().clone(),
        source_revision: snapshot.revision(),
        source_snapshot: snapshot.snapshot(),
        files_hash: input_files_hash(&files),
        script_path: script_path.clone(),
        script_revision: script.source_revision,
        script_bytes: derived.len(),
        view_id: view.view_id.clone(),
        project_root: project_root.as_str().to_owned(),
        fragment_count: fragment_maps.len(),
        environment: environment_identity,
    };

    Ok(RequestPlan {
        identity,
        project,
        root: project_root,
        files,
        environment: environment.clone(),
        compiler_options,
        file_ids,
        layout: ShadowLayout::new(
            snapshot.text().to_owned(),
            derived,
            built.view.map,
            fragment_maps,
        ),
        completion_query,
        declared_properties: facts.declared_properties,
        syntax_diagnostics: view.syntax_diagnostics.clone(),
        degradation,
        environment_degradation,
        view,
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

/// Hashes the sorted path, revision and text of one input file set.
///
/// The hash is deliberately content-aware: a host that reuses a revision or a
/// stale `view_id` must still force an in-place snapshot update when a
/// non-owner overlay text changes. Length prefixes keep adjacent fields
/// unambiguous.
fn input_files_hash(files: &[SourceFile]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut entries = files
        .iter()
        .map(|file| (file.path(), file.revision().get(), file.text()))
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut hash = FNV_OFFSET_BASIS;
    for (path, revision, text) in entries {
        hash_bytes(&mut hash, path.as_bytes(), FNV_PRIME);
        hash_bytes(&mut hash, &revision.to_le_bytes(), FNV_PRIME);
        hash_bytes(&mut hash, text.as_bytes(), FNV_PRIME);
    }
    hash
}

/// Mixes one length-prefixed byte slice into an FNV-1a hash.
fn hash_bytes(hash: &mut u64, bytes: &[u8], prime: u64) {
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    for byte in length
        .to_le_bytes()
        .into_iter()
        .chain(bytes.iter().copied())
    {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(prime);
    }
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

/// Returns whether one parsed expression has a free data root.
///
/// The completion site uses this before mounting a trailing-dot prefix; a
/// literal-only expression cannot be answered by the shadow.
pub(crate) fn expression_has_free_root(expr: &Expr, source: &str) -> bool {
    has_free_root(expr, source, &HashSet::new())
}

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

/// Builds the synthetic fragment spec for a trailing-dot completion request.
///
/// The completion fragment reuses the ordinary shadow local rendering: its
/// `locals` and `bindings` mirror the effective `wx:for` scope at the cursor,
/// so the mounted prefix can reference a template local. There is no target
/// (the rightmost value identifier is queried) and no guard (completion never
/// narrows), which keeps the published replacement range original.
fn completion_fragment_spec(fragment: &CompletionFragment) -> FragmentSpec {
    FragmentSpec {
        id: COMPLETION_FRAGMENT_ID.to_owned(),
        expr: fragment.expr.clone(),
        wxml_range: ShadowSourceRange::new(fragment.wxml_range.start(), fragment.wxml_range.end()),
        target: None,
        locals: fragment
            .bindings
            .iter()
            .map(|binding| binding.name.clone())
            .collect(),
        bindings: fragment.bindings.iter().map(local_binding).collect(),
        guards: Vec::new(),
        member: false,
    }
}

/// Finds the static event handler whose token contains `offset`.
///
/// Only a static handler already present in the prepared view can match, so a
/// mustache value, an empty value and a member expression never append a
/// fragment. The text slice and the length check re-validate the host range
/// against the request snapshot before it becomes a shadow mount; a stale or
/// inconsistent range is skipped instead of failing the whole request.
fn event_point_fragment(
    view: &PreparedView,
    text: &str,
    offset: u32,
) -> Option<EventPointFragment> {
    let event = matching_event(&view.events, offset)?;
    let range = event.handler_range;
    let handler = &event.handler;
    if handler.is_empty()
        || handler.len() > MAX_EVENT_POINT_HANDLER_BYTES
        || !is_member_name(handler)
    {
        return None;
    }
    let length = range.end().checked_sub(range.start())?;
    if usize::try_from(length).ok()? != handler.len() {
        return None;
    }
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    if text.get(start..end) != Some(handler) {
        return None;
    }
    Some(EventPointFragment {
        handler: handler.clone(),
        wxml_range: range,
    })
}

/// Returns the event whose handler token contains `offset`, if any.
fn matching_event(events: &[EventHandler], offset: u32) -> Option<&EventHandler> {
    events
        .iter()
        .find(|event| event.handler_range.start() <= offset && offset < event.handler_range.end())
}

/// Returns whether `name` can be mounted after a `this.` member prefix.
fn is_member_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first.is_alphabetic() || matches!(first, '_' | '$')) {
        return false;
    }
    characters.all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
}

/// Builds the member fragment spec for one event-handler point query.
fn event_point_fragment_spec(fragment: &EventPointFragment) -> FragmentSpec {
    FragmentSpec {
        id: EVENT_POINT_FRAGMENT_ID.to_owned(),
        expr: fragment.handler.clone(),
        wxml_range: ShadowSourceRange::new(fragment.wxml_range.start(), fragment.wxml_range.end()),
        target: None,
        locals: Vec::new(),
        bindings: Vec::new(),
        guards: Vec::new(),
        member: true,
    }
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
        member: false,
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

fn shadow_error(error: &wxml_backend_tsgo::shadow::ShadowError) -> PlanError {
    use wxml_backend_tsgo::shadow::ShadowError;

    match error {
        // A registration/root shape the shadow deliberately cannot analyze is
        // a static language limit, not a dead backend. `feature` is a stable
        // marker and `reason` is a bounded owner-shape description, so the
        // published reason is path-free.
        ShadowError::Unsupported { feature, reason } => PlanError::Unsupported(format!(
            "the template owner is not statically analyzable ({feature}): {reason}"
        )),
        // Budgets, malformed input and mapping failures stay fail-closed.
        _ => PlanError::Backend(BackendError::Unavailable(format!(
            "shadow layout is unavailable: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> wxml_types::SourceRange {
        wxml_types::SourceRange::from_bounds(start, end).expect("valid range")
    }

    fn binding(name: &str, role: LocalRole, source_expr: &str) -> wxml_types::LocalBinding {
        wxml_types::LocalBinding {
            name: name.to_owned(),
            role,
            source_expr: source_expr.to_owned(),
            source_range: range(0, u32::try_from(source_expr.len()).expect("short expr")),
        }
    }

    fn completion_fragment(
        expr: &str,
        bindings: Vec<wxml_types::LocalBinding>,
    ) -> CompletionFragment {
        CompletionFragment {
            expr: expr.to_owned(),
            wxml_range: range(0, u32::try_from(expr.len()).expect("short expr")),
            bindings,
        }
    }

    #[test]
    fn completion_fragment_without_bindings_has_no_locals() {
        let spec = completion_fragment_spec(&completion_fragment("item", Vec::new()));
        assert_eq!(spec.id, COMPLETION_FRAGMENT_ID);
        assert_eq!(spec.expr, "item");
        assert_eq!(spec.wxml_range, ShadowSourceRange::new(0, 4));
        assert!(spec.target.is_none());
        assert_eq!(spec.bindings, Vec::<ShadowLocalBinding>::new());
        assert_eq!(spec.locals, Vec::<String>::new());
        assert_eq!(spec.guards, Vec::<ShadowGuardChain>::new());
    }

    #[test]
    fn nested_bindings_keep_outermost_first_order() {
        let spec = completion_fragment_spec(&completion_fragment(
            "entry",
            vec![
                binding("row", LocalRole::Item, "groups"),
                binding("rowKey", LocalRole::Index, "groups"),
                binding("entry", LocalRole::Item, "row.members"),
                binding("entryKey", LocalRole::Index, "row.members"),
            ],
        ));

        let names = spec
            .bindings
            .iter()
            .map(|binding| binding.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["row", "rowKey", "entry", "entryKey"]);

        let roles = spec
            .bindings
            .iter()
            .map(|binding| binding.role)
            .collect::<Vec<_>>();
        assert_eq!(
            roles,
            [
                ShadowLocalRole::Item,
                ShadowLocalRole::Index,
                ShadowLocalRole::Item,
                ShadowLocalRole::Index,
            ]
        );

        let sources = spec
            .bindings
            .iter()
            .map(|binding| binding.source_expr.as_str())
            .collect::<Vec<_>>();
        assert_eq!(sources, ["groups", "groups", "row.members", "row.members"]);
    }

    #[test]
    fn locals_mirror_binding_names_in_order() {
        let spec = completion_fragment_spec(&completion_fragment(
            "cell",
            vec![
                binding("row", LocalRole::Item, "groups"),
                binding("rowIndex", LocalRole::Index, "groups"),
                binding("cell", LocalRole::Item, "row.cells"),
            ],
        ));

        assert_eq!(spec.locals, ["row", "rowIndex", "cell"]);
        assert_eq!(
            spec.locals,
            spec.bindings
                .iter()
                .map(|binding| binding.name.clone())
                .collect::<Vec<_>>()
        );
    }

    use wxml_types::{BackendEpoch, BackendId, EventBindingKind, ProjectId, ProjectRevision};

    use crate::backend_tsgo::tests::{SCRIPT_PATH, empty_view, prepared_file};
    use crate::project_host::PreparedView;

    const POINT_WXML: &str = "<view bindtap=\"onBackTap\" />";
    const POINT_SCRIPT: &str = "Page({ data: {}, onBackTap() {} });";

    struct PointViews(PreparedView);

    impl ViewProvider for PointViews {
        fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
            Ok(self.0.clone())
        }
    }

    fn point_project() -> ProjectId {
        ProjectId::new("demo")
    }

    fn event_view(wxml: &str, handler: Option<&str>) -> PreparedView {
        let mut view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, POINT_SCRIPT)]);
        if let Some(handler) = handler {
            let start = u32::try_from(wxml.find(handler).expect("handler in template"))
                .expect("handler start");
            let end = start + u32::try_from(handler.len()).expect("handler length");
            let range = range(start, end);
            view.events = Arc::from(vec![EventHandler {
                attribute: "bindtap".to_owned(),
                binding: EventBindingKind::Bind,
                event: "tap".to_owned(),
                handler: handler.to_owned(),
                range,
                attribute_range: range,
                handler_range: range,
            }]);
        }
        view
    }

    fn point_context(wxml: &str) -> RequestContext {
        RequestContext::new(
            ProjectRevision::new(point_project(), 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("test"), 1),
            Arc::new(SourceSnapshot::new(
                FileId::new(point_project(), "pages/index/index.wxml").expect("wxml file"),
                SourceRevision::new(1),
                SnapshotId::new(1),
                wxml,
            )),
        )
        .expect("context")
    }

    fn point_plan(wxml: &str, view: PreparedView, offset: u32) -> RequestPlan {
        build_point_request_plan(
            &PointViews(view),
            &point_context(wxml),
            &Environment::Legacy,
            offset,
        )
        .expect("point plan")
    }

    #[test]
    fn a_cursor_inside_an_event_token_mounts_one_member_fragment() {
        let view = event_view(POINT_WXML, Some("onBackTap"));
        let start = u32::try_from(POINT_WXML.find("onBackTap").expect("handler")).expect("start");
        let plan = point_plan(POINT_WXML, view, start + 2);

        assert!(
            plan.layout.derived().contains("this.onBackTap"),
            "{}",
            plan.layout.derived()
        );
        assert_eq!(plan.layout.fragment_count(), 1);
        let mount = plan
            .layout
            .mount_for_range(range(
                start,
                start + u32::try_from("onBackTap".len()).expect("handler length"),
            ))
            .expect("member mount");
        assert_eq!(mount.fragment_id, EVENT_POINT_FRAGMENT_ID);
        assert!(mount.text.starts_with("this.onBackTap"), "{:?}", mount.text);
        assert_eq!(mount.target_start_utf16 - mount.start_utf16, 5);
        let derived = plan
            .layout
            .derived_offset(start + 2, true)
            .expect("derived");
        assert_eq!(
            derived - mount.start_utf16,
            7,
            "the handler name maps behind the synthetic `this.` prefix"
        );
    }

    #[test]
    fn a_cursor_outside_the_event_token_never_mounts_a_member_fragment() {
        let view = event_view(POINT_WXML, Some("onBackTap"));
        let handler_start =
            u32::try_from(POINT_WXML.find("onBackTap").expect("handler")).expect("start");
        let attribute_start =
            u32::try_from(POINT_WXML.find("bindtap").expect("attribute")).expect("attribute");
        for offset in [
            attribute_start,
            attribute_start + 1,
            handler_start - 1,
            handler_start - 2,
        ] {
            let plan = point_plan(POINT_WXML, view.clone(), offset);
            assert_eq!(
                plan.layout.fragment_count(),
                0,
                "offset {offset} must not mount the handler"
            );
            assert!(
                !plan.layout.derived().contains("this.onBackTap"),
                "{offset}"
            );
            assert!(
                plan.layout
                    .mount_for_range(range(handler_start, handler_start + 9))
                    .is_none(),
                "{offset}"
            );
        }
    }

    #[test]
    fn a_dynamic_handler_never_mounts_a_member_fragment() {
        let wxml = "<view bindtap=\"{{ handler }}\" />";
        let view = event_view(wxml, None);
        let offset = u32::try_from(wxml.find("handler").expect("handler token")).expect("offset");
        let plan = point_plan(wxml, view, offset);

        assert_eq!(plan.layout.fragment_count(), 0);
        assert!(!plan.layout.derived().contains("this.handler"));
        assert!(
            plan.layout
                .mount_for_range(range(
                    offset,
                    offset + u32::try_from("handler".len()).expect("handler length")
                ))
                .is_none()
        );
    }
}
