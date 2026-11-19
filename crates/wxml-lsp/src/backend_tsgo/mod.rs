//! Opt-in `wxml_types::Backend` adapter for the native `tsgo` backend.
//!
//! # Scope and safety
//!
//! This module is not constructed by `server.rs`, `project_host.rs` or the
//! configuration defaults. The only native entry point is the explicit
//! [`run_stdio_native`] selector used by `wxml-lsp --stdio --backend native`;
//! `--stdio` without that flag still constructs the Node backend. The existing
//! Node backend stays untouched and remains the default.
//!
//! The adapter owns a [`NativeSession`](wxml_backend_tsgo::native::session::NativeSession)
//! lazily. Every request converts the host's effective template snapshot into
//! an [`AnalysisInput`](wxml_backend_tsgo::input::AnalysisInput) whose owning
//! script is the in-place shadow, maps the query position through
//! [`ShadowMap`](wxml_backend_tsgo::shadow::mapping::ShadowMap), and maps
//! native ranges back to original UTF-8 template bytes. Missing input, a
//! missing owning script, mapping gaps and native failures are typed errors or
//! explicit degraded statuses; they never become a supported empty result.
//!
//! # Type environment
//!
//! [`run_stdio_native`] loads the pinned type assets once through
//! `TsgoBackendAdapter::with_install`, before any sandbox or session exists.
//! A missing, unsafe or pin-mismatched tree aborts startup; there is no Node
//! or legacy fallback. `new` and `with_host` remain legacy test/probe
//! constructors and are not reachable from the production selector.
//!
//! Only Linux x86-64 with POSIX absolute roots is supported: Windows
//! drive-letter/UNC roots, non-UTF-8 roots, symlinks, hard links and
//! non-regular files are rejected (fail closed).
//! The exposed [`BackendEpoch`] advances on [`Backend::invalidate`] and fatal
//! native failures, not on a lazy start or snapshot update: the in-flight
//! request is already stamped and [`RequestIdentity`] guards input changes.
//!
//! # Blocked capabilities
//!
//! Rename edits and script symbol facts remain explicitly `Unsupported`. The
//! native API has no `getRenameInfo` or `getEditsForRename`, and script symbol
//! extraction is not wired into this adapter. Definition is limited to
//! expression-member targets that the native referenced-symbol path can decode
//! and map back to original text; the cursor must be inside a mounted template
//! expression. The adapter does not fake locations or edits and does not call a
//! Node fallback.
//!
//! Native document symbols are wired for the prepared view's **owning script**
//! only. `NativeSession::document_symbols` returns hierarchical symbols with
//! verified name tokens; [`Backend::document_symbols`] maps their shadow ranges
//! back to original script bytes and drops anything that only exists in a
//! mounted template fragment or generated probe text. The WXML outline stays
//! static and is never merged with script symbols. Truncation, skipped
//! candidates and mapping drops are reported as `Partial`; native `Unsupported`
//! stays `Unsupported` rather than an empty success.
//!
//! # Test seams
//!
//! [`ViewProvider`] and [`SessionFactory`] are internal seams: unit tests use
//! a fake session, the real install gate needs `TSGO_BIN`,
//! `TSGO_TYPINGS_ROOT` and `WXML_REQUIRE_REAL_ENGINE=1`.

#![allow(
    dead_code,
    reason = "opt-in native selector: legacy constructors/query seams and manifest \
              projections are test- or forward-compatibility-only"
)]

pub(crate) mod completion;
pub(crate) mod definition;
pub(crate) mod diagnostics;
pub(crate) mod document_symbol;
pub(crate) mod engine;
pub(crate) mod environment;
pub(crate) mod event_completion;
pub(crate) mod hover;
pub(crate) mod input;
pub(crate) mod mapping;
pub(crate) mod sandbox;
pub(crate) mod session;

pub use self::engine::EngineDiscoveryError;
pub use self::environment::EnvironmentLoadError;
pub use self::sandbox::SandboxError;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use thiserror::Error;
use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::input::{AnalysisInput, ProjectKey, SnapshotRevision};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::session::SessionOptions;
use wxml_types::{
    Backend, BackendCapabilities, BackendEpoch, BackendError, BackendId, CapabilityStatus,
    CompletionKind, CompletionRequest, CompletionResult, DefinitionRequest, DefinitionResult,
    DiagnosticsRequest, DiagnosticsResult, DocumentSymbolsRequest, DocumentSymbolsResult,
    HoverRequest, HoverResult, SymbolRequest, SymbolResult,
};

use self::engine::DiscoveredInstall;
use self::environment::Environment;
use self::event_completion::EventCandidateCache;
use self::input::{RequestIdentity, RequestPlan, ViewProvider, build_request_plan, merge_reason};
use self::sandbox::SandboxSessionFactory;
use self::session::{NativeSessionFactory, QuerySession, SessionFactory};

/// Stable backend identity used in [`BackendEpoch`].
pub(crate) const BACKEND_ID: &str = "tsgo-native";

/// Why the opt-in native startup path could not begin serving.
///
/// Messages are path-free and stable enough for stderr. A failure never
/// constructs the Node backend or probes a JavaScript backend; the selector
/// fails closed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NativeStartupError {
    /// The running executable is not installed with a verified pinned engine.
    #[error("native engine discovery failed: {0}")]
    Discovery(#[from] EngineDiscoveryError),
    /// The verified pinned type assets could not be loaded.
    #[error("native type assets are unavailable: {0}")]
    Environment(#[from] EnvironmentLoadError),
    /// A private per-session sandbox could not be validated or created.
    #[error("native sandbox is unavailable: {0}")]
    Sandbox(#[from] SandboxError),
    /// The LSP transport itself failed.
    #[error("native server failed: {0}")]
    Server(#[from] crate::ServerError),
}

/// Serves the LSP over stdio with the opt-in native backend.
///
/// Discovery starts at the running executable and requires
/// `<package>/bin/wxml-lsp` with a verified manifest and `engine/lib/tsc`.
/// This function also loads and verifies the pinned standard-library and
/// mini-program typings once, before any sandbox or session exists. Missing,
/// corrupt or unsafe installations or assets return [`NativeStartupError`]
/// before the LSP `initialize` exchange; there is no Node fallback and no
/// legacy no-asset session.
pub fn run_stdio_native() -> Result<crate::ServerOutcome, NativeStartupError> {
    let install = engine::discover_from_current_exe()?;
    let host = Arc::new(crate::project_host::ProjectState::new());
    let adapter = TsgoBackendAdapter::with_install(host.clone(), &install)?;
    crate::server::run_stdio_with_host(Arc::new(adapter), host).map_err(NativeStartupError::Server)
}

/// Reason returned for script symbol facts until they are wired.
///
/// Document symbols have their own [`Backend::document_symbols`] method and
/// are not expressed by the C0 [`SymbolRequest`]/[`SymbolResult`] DTOs; this
/// reason only covers the data-root/handler/reference facts those requests
/// need.
pub(crate) const SYMBOLS_UNSUPPORTED: &str = "native data-root, handler and reference symbol facts are not wired to the C0 SymbolRequest/SymbolResult DTOs";

/// Opt-in adapter between the LSP project host and the native session.
///
/// Construction is explicit. No default server path constructs it, so adding
/// this type does not select the native backend.
pub(crate) struct TsgoBackendAdapter {
    engine: PinnedEngine,
    views: Arc<dyn ViewProvider>,
    sessions: Arc<dyn SessionFactory>,
    environment: Environment,
    state: Mutex<AdapterState>,
    /// Last script-method candidate list used by event-handler completion.
    ///
    /// This cache is independent of the native session and its epoch: event
    /// candidates are syntactic script facts. The entry is replaced whenever
    /// the project, owning script/owner, prepared-source completeness or the
    /// sorted script revision set changes; template revisions and cursor
    /// offsets are not part of the key.
    event_cache: Mutex<Option<EventCandidateCache>>,
    epoch: AtomicU64,
}

impl std::fmt::Debug for TsgoBackendAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TsgoBackendAdapter")
            .field("engine", &self.engine)
            .field("environment", &self.environment)
            .field("epoch", &self.epoch.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl TsgoBackendAdapter {
    /// Creates an adapter around explicit session options.
    ///
    /// The caller owns executable, asset and sandbox verification; this
    /// constructor performs none and always uses [`Environment::Legacy`]. The
    /// opt-in production startup path uses [`Self::with_install`].
    #[must_use]
    pub(crate) fn with_host(
        host: Arc<dyn crate::project_host::ProjectHost>,
        options: SessionOptions,
    ) -> Self {
        Self::new(
            PinnedEngine::pinned(),
            Arc::new(input::HostViewProvider::new(host)),
            Arc::new(NativeSessionFactory::new(options)),
        )
    }

    /// Creates the opt-in production adapter from a discovered install.
    ///
    /// The pinned standard-library and typings assets are loaded and verified
    /// once, before any sandbox or session exists, so a missing or
    /// pin-mismatched asset tree fails startup instead of producing a legacy
    /// session. Every native session then gets a fresh private sandbox below
    /// the OS temporary directory; the package root is rejected as a sandbox
    /// base so runtime files never land in the immutable version tree.
    ///
    /// Engine manifest projection and asset sets are verified independently;
    /// cross-checking the manifest `typeAssets` fields remains P6-3-4/F3-03.
    pub(crate) fn with_install(
        host: Arc<dyn crate::project_host::ProjectHost>,
        install: &DiscoveredInstall,
    ) -> Result<Self, NativeStartupError> {
        let environment = environment::load_environment(install)?;
        let sessions = SandboxSessionFactory::new(install.engine.clone(), &install.package_root)?;
        Ok(Self::new_with_environment(
            PinnedEngine::pinned(),
            Arc::new(input::HostViewProvider::new(host)),
            Arc::new(sessions),
            environment,
        ))
    }

    /// Starts and immediately stops one native session without entering the
    /// LSP transport.
    ///
    /// Test-only success-path seam for P6-4a. It exercises discovery output,
    /// per-session sandbox creation, engine verification, handshake, schema
    /// probe and sandbox cleanup. No production caller uses it.
    #[cfg(test)]
    pub(crate) fn start_and_stop_session_for_test(&self) -> Result<(), BackendError> {
        let session = self.sessions.start().map_err(map_native_error)?;
        session.shutdown();
        Ok(())
    }

    /// Creates a legacy adapter with explicit view/session seams.
    ///
    /// `new` and [`Self::with_host`] never mount type assets; only
    /// [`Self::new_with_environment`] can construct a trusted adapter.
    #[must_use]
    pub(crate) fn new(
        engine: PinnedEngine,
        views: Arc<dyn ViewProvider>,
        sessions: Arc<dyn SessionFactory>,
    ) -> Self {
        Self::new_with_environment(engine, views, sessions, Environment::legacy())
    }

    /// Creates an adapter with an explicit type environment.
    #[must_use]
    pub(crate) fn new_with_environment(
        engine: PinnedEngine,
        views: Arc<dyn ViewProvider>,
        sessions: Arc<dyn SessionFactory>,
        environment: Environment,
    ) -> Self {
        Self {
            engine,
            views,
            sessions,
            environment,
            state: Mutex::new(AdapterState::new(1)),
            event_cache: Mutex::new(None),
            epoch: AtomicU64::new(1),
        }
    }

    /// Returns the pinned engine identity.
    #[must_use]
    pub(crate) const fn engine(&self) -> &PinnedEngine {
        &self.engine
    }

    fn epoch_matches(&self, context: &wxml_types::RequestContext) -> bool {
        context.backend_epoch() == self.epoch()
    }

    fn advance_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::SeqCst).saturating_add(1)
    }

    fn prepare(&self, context: &wxml_types::RequestContext) -> Result<RequestPlan, BackendError> {
        build_request_plan(self.views.as_ref(), context, &self.environment)
    }

    /// Runs one purely syntactic query without the session environment reason.
    ///
    /// `document_symbols` cannot be affected by a degraded type environment,
    /// so this path neither clones nor merges `environment_reason`.
    fn with_session<R>(
        &self,
        plan: &RequestPlan,
        operation: impl FnOnce(&mut dyn QuerySession) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        self.with_session_mode(plan, false, |session, _degradation| operation(session))
    }

    /// Runs one typed query and merges the open-session environment reason.
    ///
    /// The reason is read after [`Self::ensure_open`] so the first request that
    /// opens a degraded VFS already reports `Partial`.
    fn with_session_environment<R>(
        &self,
        plan: &RequestPlan,
        operation: impl FnOnce(&mut dyn QuerySession, Option<String>) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        self.with_session_mode(plan, true, operation)
    }

    fn with_session_mode<R>(
        &self,
        plan: &RequestPlan,
        include_environment: bool,
        operation: impl FnOnce(&mut dyn QuerySession, Option<String>) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.ensure_open(&mut state, plan)?;
        let mut degradation = plan.degradation.clone();
        if include_environment {
            merge_reason(&mut degradation, state.environment_reason.clone());
        }
        let session = state.session.as_mut().ok_or_else(|| {
            BackendError::Unavailable("native session was not established".to_owned())
        })?;
        let outcome = operation(session.as_mut(), degradation);
        let current = self.epoch.load(Ordering::SeqCst);
        let invalidated = state.generation != current;
        let fatal = matches!(
            &outcome,
            Err(BackendError::Unavailable(_) | BackendError::Protocol(_) | BackendError::Timeout)
        );
        let dead = if invalidated {
            let session = state.session.take();
            state.open = None;
            state.environment_reason = None;
            state.generation = current;
            session
        } else if fatal {
            let next = self.advance_epoch();
            state.generation = next;
            let session = state.session.take();
            state.open = None;
            state.environment_reason = None;
            session
        } else {
            None
        };
        drop(state);
        if let Some(session) = dead {
            session.shutdown();
        }
        if invalidated {
            return Err(BackendError::Cancelled);
        }
        outcome
    }

    fn ensure_open(
        &self,
        state: &mut AdapterState,
        plan: &RequestPlan,
    ) -> Result<(), BackendError> {
        self.drop_invalidated(state);
        if let Some(open) = &state.open {
            if open.identity == plan.identity && state.session.is_some() {
                return Ok(());
            }
        }
        let revision = next_revision(state)?;
        let input = plan.input(revision)?;
        // A trusted adapter must never start or update a session from a legacy
        // no-asset input (and a legacy adapter must never mount assets). The
        // check runs before any session/sandbox exists, so a mismatch cannot
        // silently degrade the type environment.
        if input.has_type_environment() != self.environment.is_trusted() {
            return Err(BackendError::Unavailable(
                "type environment does not match request input".to_owned(),
            ));
        }

        if state.session.is_none() {
            return self.start_and_open(state, plan, &input);
        }
        if same_open_project(state, plan, &input) && self.update_existing(state, plan, &input)? {
            return Ok(());
        }
        // A failed update, a project/root change or any environment/option
        // change cannot be repaired in place: stop the old session and open the
        // new input from scratch.
        if let Some(session) = state.session.take() {
            session.shutdown();
        }
        state.open = None;
        state.environment_reason = None;
        self.start_and_open(state, plan, &input)
    }

    /// Updates the open snapshot when only source/view revisions changed.
    ///
    /// Returns `false` when the backend rejected the update, which makes the
    /// caller restart; environment and project changes never reach this path.
    fn update_existing(
        &self,
        state: &mut AdapterState,
        plan: &RequestPlan,
        input: &AnalysisInput,
    ) -> Result<bool, BackendError> {
        let Some(session) = state.session.as_mut() else {
            return Ok(false);
        };
        if session.update_project(input).is_err() {
            return Ok(false);
        }
        if self.epoch.load(Ordering::SeqCst) != state.generation {
            self.drop_invalidated(state);
            return Err(BackendError::Cancelled);
        }
        state.environment_reason = session.environment_degradation();
        state.open = Some(OpenProject::new(plan, input));
        Ok(true)
    }

    /// Drops a session whose generation was invalidated while it was busy.
    fn drop_invalidated(&self, state: &mut AdapterState) {
        let current = self.epoch.load(Ordering::SeqCst);
        if state.generation == current {
            return;
        }
        state.generation = current;
        state.open = None;
        state.environment_reason = None;
        if let Some(session) = state.session.take() {
            session.shutdown();
        }
    }

    fn start_and_open(
        &self,
        state: &mut AdapterState,
        plan: &RequestPlan,
        input: &AnalysisInput,
    ) -> Result<(), BackendError> {
        // No session exists yet, so no environment degradation can survive.
        state.environment_reason = None;
        let mut session = self.sessions.start().map_err(map_native_error)?;
        if let Err(error) = session.open_project(input) {
            session.shutdown();
            return Err(map_native_error(error));
        }
        let current = self.epoch.load(Ordering::SeqCst);
        if state.generation != current {
            session.shutdown();
            state.generation = current;
            state.open = None;
            state.environment_reason = None;
            return Err(BackendError::Cancelled);
        }
        state.environment_reason = session.environment_degradation();
        state.session = Some(session);
        state.open = Some(OpenProject::new(plan, input));
        Ok(())
    }
}

impl Backend for TsgoBackendAdapter {
    fn epoch(&self) -> BackendEpoch {
        BackendEpoch::new(
            BackendId::new(BACKEND_ID),
            self.epoch.load(Ordering::SeqCst),
        )
    }

    fn hover(&self, request: HoverRequest) -> Result<HoverResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = self.prepare(&request.context)?;
        let script_path = plan.identity.script_path.clone();
        self.with_session_environment(&plan, |session, degradation| {
            hover::hover(
                session,
                &plan.layout,
                &script_path,
                degradation.as_deref(),
                &request,
            )
        })
    }

    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let Ok(plan) = self.prepare(&request.context) else {
            return Ok(unavailable_diagnostics(
                crate::project_host::parse_syntax_diagnostics(request.context.source()),
                "type backend unavailable",
            ));
        };
        let script_path = plan.identity.script_path.clone();
        let syntax = merged_syntax(&plan.syntax_diagnostics, request.context.source());
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if self.ensure_open(&mut state, &plan).is_err() {
            return Ok(unavailable_diagnostics(syntax, "type backend unavailable"));
        }
        let mut degradation = plan.degradation.clone();
        merge_reason(&mut degradation, state.environment_reason.clone());
        let Some(session) = state.session.as_mut() else {
            return Ok(unavailable_diagnostics(syntax, "type backend unavailable"));
        };
        let run = diagnostics::diagnostics(
            session.as_mut(),
            &plan.layout,
            &script_path,
            &syntax,
            degradation.as_deref(),
        );
        let current = self.epoch.load(Ordering::SeqCst);
        let invalidated = state.generation != current;
        let dead = if invalidated || run.fatal {
            let session = state.session.take();
            state.open = None;
            state.environment_reason = None;
            if invalidated {
                state.generation = current;
            }
            session
        } else {
            None
        };
        drop(state);
        if let Some(session) = dead {
            session.shutdown();
        }
        if invalidated {
            return Ok(unavailable_diagnostics(syntax, "request cancelled"));
        }
        Ok(run.result)
    }

    fn definition(&self, request: DefinitionRequest) -> Result<DefinitionResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = self.prepare(&request.context)?;
        let Some(position) = definition::query_position(&plan.layout, request.offset)? else {
            return Ok(definition::outside_expression());
        };
        self.with_session_environment(&plan, |session, degradation| {
            definition::definition(session, &plan, position, degradation.as_deref())
        })
    }

    fn completion(&self, request: CompletionRequest) -> Result<CompletionResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        if request.kind == CompletionKind::EventHandler {
            // Pure script facts: no shadow, no native session, no environment.
            return event_completion::completion(self.views.as_ref(), &request, &self.event_cache);
        }
        if request.kind != CompletionKind::ExpressionMember {
            return Ok(CompletionResult {
                items: Vec::new(),
                status: CapabilityStatus::Unsupported {
                    reason:
                        "only expression-member and event-handler completion are implemented for the native backend"
                            .to_owned(),
                },
                truncated: false,
            });
        }
        let plan = self.prepare(&request.context)?;
        let script_path = plan.identity.script_path.clone();
        self.with_session_environment(&plan, |session, degradation| {
            completion::completion(
                session,
                &plan.layout,
                &script_path,
                degradation.as_deref(),
                &request,
            )
        })
    }

    fn symbols(&self, request: SymbolRequest) -> Result<SymbolResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        Ok(SymbolResult::unsupported(SYMBOLS_UNSUPPORTED))
    }

    fn document_symbols(
        &self,
        request: DocumentSymbolsRequest,
    ) -> Result<DocumentSymbolsResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = self.prepare(&request.context)?;
        if plan.file_id(&plan.identity.script_path).as_ref() != Some(&request.subject) {
            return Ok(DocumentSymbolsResult::unsupported(
                document_symbol::OWNING_SCRIPT_ONLY,
            ));
        }
        let degradation = plan.degradation.clone();
        self.with_session(&plan, |session| {
            document_symbol::document_symbols(
                session,
                &plan,
                &request.subject,
                degradation.as_deref(),
            )
        })
    }

    fn invalidate(&self) {
        let next = self.advance_epoch();
        // Never block the protocol loop on an in-flight native query. If the
        // lock is free the session is dropped now; otherwise the query thread
        // observes the generation change and drops it on return.
        let session = self.state.try_lock().map_or_else(
            |_| None,
            |mut state| {
                state.generation = next;
                state.open = None;
                state.environment_reason = None;
                state.session.take()
            },
        );
        if let Some(session) = session {
            // Forced invalidation must not run the graceful release path on
            // the protocol thread; dropping the session closes/kills it.
            drop(session);
        }
    }

    /// Reports the wired template-type features.
    ///
    /// Document symbols are intentionally absent from
    /// [`BackendCapabilities`]: the LSP `documentSymbolProvider` already covers
    /// the static WXML outline, and a script outline reports its own
    /// `Supported`/`Partial`/`Unsupported` status per request. No capability
    /// entry here claims a complete script outline.
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            hover: CapabilityStatus::Partial {
                reason:
                    "native expression-member hover is wired; event-handler and component semantics are served by the static layer"
                        .to_owned(),
            },
            diagnostics: CapabilityStatus::Partial {
                reason:
                    "host syntax plus native fragment diagnostics are wired; event-handler and component diagnostics are not covered"
                        .to_owned(),
            },
            definition: CapabilityStatus::Partial {
                reason:
                    "native expression-member definition is wired; event-handler, component and non-expression targets are not covered"
                        .to_owned(),
            },
            completion: CapabilityStatus::Partial {
                reason:
                    "expression-member completion is native; the pinned API has no per-entry documentation and auto-imports need prepareAutoImports at snapshot creation, so non-empty native results stay Partial; event-handler completion is served from the Rust script method table without a native session and can be Supported; `this.`/method-body completion, custom component event signatures and handler signature help are not implemented"
                        .to_owned(),
            },
        }
    }
}

struct AdapterState {
    session: Option<Box<dyn QuerySession>>,
    open: Option<OpenProject>,
    /// VFS environment degradation of the currently open session.
    ///
    /// Refreshed after a successful open/update and cleared whenever the
    /// session is dropped, invalidated or restarted.
    environment_reason: Option<String>,
    next_revision: u64,
    generation: u64,
}

impl AdapterState {
    const fn new(generation: u64) -> Self {
        Self {
            session: None,
            open: None,
            environment_reason: None,
            next_revision: 0,
            generation,
        }
    }
}

struct OpenProject {
    identity: RequestIdentity,
    project: ProjectKey,
    root: String,
}

impl OpenProject {
    fn new(plan: &RequestPlan, input: &AnalysisInput) -> Self {
        Self {
            identity: plan.identity.clone(),
            project: input.project().clone(),
            root: input.root().as_str().to_owned(),
        }
    }
}

/// Returns whether the open snapshot can be advanced in place.
///
/// The environment identity and project/root must be byte-identical; only
/// source/view revisions may differ.
fn same_open_project(state: &AdapterState, plan: &RequestPlan, input: &AnalysisInput) -> bool {
    state.open.as_ref().is_some_and(|open| {
        open.project == *input.project()
            && open.root == input.root().as_str()
            && open.identity.environment == plan.identity.environment
    })
}

fn next_revision(state: &mut AdapterState) -> Result<SnapshotRevision, BackendError> {
    let next = state
        .next_revision
        .checked_add(1)
        .ok_or_else(|| BackendError::Internal("native input revision overflow".to_owned()))?;
    state.next_revision = next;
    Ok(SnapshotRevision::new(next))
}

fn merged_syntax(
    view_syntax: &[wxml_types::Diagnostic],
    snapshot: &wxml_types::SourceSnapshot,
) -> Vec<wxml_types::Diagnostic> {
    let mut merged = view_syntax.to_vec();
    let mut seen: BTreeSet<(Option<String>, u32, u32, String)> = merged
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.clone(),
                diagnostic.range.start(),
                diagnostic.range.end(),
                diagnostic.message.clone(),
            )
        })
        .collect();
    for diagnostic in crate::project_host::parse_syntax_diagnostics(snapshot) {
        let key = (
            diagnostic.code.clone(),
            diagnostic.range.start(),
            diagnostic.range.end(),
            diagnostic.message.clone(),
        );
        if seen.insert(key) {
            merged.push(diagnostic);
        }
    }
    merged
}

fn unavailable_diagnostics(
    diagnostics: Vec<wxml_types::Diagnostic>,
    reason: &str,
) -> DiagnosticsResult {
    DiagnosticsResult {
        diagnostics,
        status: CapabilityStatus::Unavailable {
            reason: reason.to_owned(),
        },
    }
}

pub(crate) fn map_native_error(error: NativeError) -> BackendError {
    match error {
        NativeError::Unavailable { reason } => {
            BackendError::Unavailable(reason.as_str().to_owned())
        }
        NativeError::Protocol(message) => BackendError::Protocol(message),
        NativeError::Timeout(_) => BackendError::Timeout,
        NativeError::StaleHandle | NativeError::StaleSnapshot => BackendError::Cancelled,
        NativeError::NoSnapshot => {
            BackendError::Unavailable("no native snapshot is open".to_owned())
        }
        NativeError::InvalidHandle { reason } | NativeError::InvalidParams { reason } => {
            BackendError::Protocol(reason.to_owned())
        }
        NativeError::InputTooLarge { bytes, limit } => BackendError::Unavailable(format!(
            "native input is too large: {bytes} bytes exceeds {limit}"
        )),
        NativeError::Unsupported { method } => {
            BackendError::Unavailable(format!("native method is unavailable: {method}"))
        }
        NativeError::MethodNotAllowed(method) => {
            BackendError::Protocol(format!("native method is not allowlisted: {method}"))
        }
        NativeError::EngineError { code } => {
            BackendError::Internal(format!("engine internal error (code {code})"))
        }
        NativeError::Unimplemented { feature } => {
            BackendError::Internal(format!("native capability is not implemented: {feature}"))
        }
    }
}
