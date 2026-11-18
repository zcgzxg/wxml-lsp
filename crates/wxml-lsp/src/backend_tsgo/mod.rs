//! Opt-in `wxml_types::Backend` adapter for the native `tsgo` backend.
//!
//! # Scope and safety
//!
//! This module is deliberately not constructed by `server.rs`,
//! `project_host.rs` or the configuration defaults. It exists so a later P6
//! switch can select a real backend without changing the C0 boundary. The
//! existing Node backend stays untouched and remains the default.
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
//! The exposed [`BackendEpoch`] advances on [`Backend::invalidate`] and when a
//! fatal native failure drops the session. A normal lazy start or snapshot
//! update does not advance it: the server has already stamped the in-flight
//! request, and input changes are guarded by [`RequestIdentity`] plus the C0
//! project/view/source revision checks. Advancing during a successful request
//! would make the server discard a valid result.
//!
//! # Blocked capabilities
//!
//! `definition`, rename edits and script symbols remain explicitly
//! `Unsupported`. The native API has no `getQuickInfoAtPosition`,
//! `getDefinitionAtPosition`, `getDefinitionAndBoundSpan`, `getRenameInfo` or
//! `getEditsForRename`; the AST range completion those combinations need is not
//! implemented. The adapter does not fake locations or edits and does not call
//! a Node fallback.
//!
//! # Platform limits
//!
//! The native input layer accepts Unix absolute project roots and
//! `/`-separated relative paths only. Windows drive-letter/UNC roots and
//! non-UTF-8 project roots are rejected as `Unavailable`; cross-platform path
//! support remains BLOCKED until the native contract and assets cover it.
//!
//! # Test seams
//!
//! [`ViewProvider`] and [`SessionFactory`] are internal seams. Unit tests use a
//! fake session to exercise hover/diagnostics/completion mapping, no-false-clean
//! statuses and epoch invalidation without a native binary. The optional real
//! engine test is gated by `TSGO_BIN` and `WXML_REQUIRE_REAL_ENGINE=1`.

#![allow(
    dead_code,
    reason = "P4-C implements the opt-in adapter; no server or config path selects it"
)]

pub(crate) mod completion;
pub(crate) mod diagnostics;
pub(crate) mod hover;
pub(crate) mod input;
pub(crate) mod mapping;
pub(crate) mod session;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::input::{AnalysisInput, ProjectKey, SnapshotRevision};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::session::SessionOptions;
use wxml_types::{
    Backend, BackendCapabilities, BackendEpoch, BackendError, BackendId, CapabilityStatus,
    CompletionKind, CompletionRequest, CompletionResult, DefinitionRequest, DefinitionResult,
    DiagnosticsRequest, DiagnosticsResult, HoverRequest, HoverResult, SymbolRequest, SymbolResult,
};

use self::input::{RequestIdentity, RequestPlan, ViewProvider, build_request_plan};
use self::session::{NativeSessionFactory, QuerySession, SessionFactory};

/// Stable backend identity used in [`BackendEpoch`].
pub(crate) const BACKEND_ID: &str = "tsgo-native";

/// Reason returned for definition queries until native range completion exists.
pub(crate) const DEFINITION_UNSUPPORTED: &str =
    "native DefinitionAtPosition/AST range completion is not implemented";

/// Reason returned for symbols until script symbol facts are wired.
pub(crate) const SYMBOLS_UNSUPPORTED: &str =
    "native script symbol extraction is not wired into the backend";

/// Opt-in adapter between the LSP project host and the native session.
///
/// Construction is explicit. No default server path constructs it, so adding
/// this type does not select the native backend.
pub(crate) struct TsgoBackendAdapter {
    engine: PinnedEngine,
    views: Arc<dyn ViewProvider>,
    sessions: Arc<dyn SessionFactory>,
    state: Mutex<AdapterState>,
    epoch: AtomicU64,
}

impl std::fmt::Debug for TsgoBackendAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TsgoBackendAdapter")
            .field("engine", &self.engine)
            .field("epoch", &self.epoch.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl TsgoBackendAdapter {
    /// Creates an adapter around a project host and verified session options.
    ///
    /// This is the production constructor for the later P6 selection point.
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

    /// Creates an adapter with explicit view/session seams.
    #[must_use]
    pub(crate) fn new(
        engine: PinnedEngine,
        views: Arc<dyn ViewProvider>,
        sessions: Arc<dyn SessionFactory>,
    ) -> Self {
        Self {
            engine,
            views,
            sessions,
            state: Mutex::new(AdapterState::new(1)),
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
        build_request_plan(self.views.as_ref(), context)
    }

    fn with_session<R>(
        &self,
        plan: &RequestPlan,
        operation: impl FnOnce(&mut dyn QuerySession) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.ensure_open(&mut state, plan)?;
        let session = state.session.as_mut().ok_or_else(|| {
            BackendError::Unavailable("native session was not established".to_owned())
        })?;
        let outcome = operation(session.as_mut());
        let current = self.epoch.load(Ordering::SeqCst);
        let invalidated = state.generation != current;
        let fatal = matches!(
            &outcome,
            Err(BackendError::Unavailable(_) | BackendError::Protocol(_) | BackendError::Timeout)
        );
        let dead = if invalidated {
            let session = state.session.take();
            state.open = None;
            state.generation = current;
            session
        } else if fatal {
            let next = self.advance_epoch();
            state.generation = next;
            let session = state.session.take();
            state.open = None;
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

        if state.session.is_none() {
            return self.start_and_open(state, plan, &input);
        }
        let same_project = state.open.as_ref().is_some_and(|open| {
            open.project == *input.project() && open.root == input.root().as_str()
        });
        if same_project {
            let Some(session) = state.session.as_mut() else {
                return Err(BackendError::Unavailable(
                    "native session disappeared".to_owned(),
                ));
            };
            if session.update_project(&input).is_ok() {
                if self.epoch.load(Ordering::SeqCst) != state.generation {
                    self.drop_invalidated(state);
                    return Err(BackendError::Cancelled);
                }
                state.open = Some(OpenProject::new(plan, &input));
                return Ok(());
            }
        }
        // A failed update or a project/root change cannot be repaired in place:
        // stop the old session and open the new input from scratch.
        if let Some(session) = state.session.take() {
            session.shutdown();
        }
        state.open = None;
        self.start_and_open(state, plan, &input)
    }

    /// Drops a session whose generation was invalidated while it was busy.
    fn drop_invalidated(&self, state: &mut AdapterState) {
        let current = self.epoch.load(Ordering::SeqCst);
        if state.generation == current {
            return;
        }
        state.generation = current;
        state.open = None;
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
            return Err(BackendError::Cancelled);
        }
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
        let degradation = plan.degradation.clone();
        self.with_session(&plan, |session| {
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
        let degradation = plan.degradation.clone();
        let syntax = merged_syntax(&plan.syntax_diagnostics, request.context.source());
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if self.ensure_open(&mut state, &plan).is_err() {
            return Ok(unavailable_diagnostics(syntax, "type backend unavailable"));
        }
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
        Ok(DefinitionResult {
            locations: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: DEFINITION_UNSUPPORTED.to_owned(),
            },
        })
    }

    fn completion(&self, request: CompletionRequest) -> Result<CompletionResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        if request.kind != CompletionKind::ExpressionMember {
            return Ok(CompletionResult {
                items: Vec::new(),
                status: CapabilityStatus::Unsupported {
                    reason:
                        "only expression-member completion is implemented for the native backend"
                            .to_owned(),
                },
                truncated: false,
            });
        }
        let plan = self.prepare(&request.context)?;
        let script_path = plan.identity.script_path.clone();
        let degradation = plan.degradation.clone();
        self.with_session(&plan, |session| {
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
                state.session.take()
            },
        );
        if let Some(session) = session {
            // Forced invalidation must not run the graceful release path on
            // the protocol thread; dropping the session closes/kills it.
            drop(session);
        }
    }

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
            definition: CapabilityStatus::Unsupported {
                reason: DEFINITION_UNSUPPORTED.to_owned(),
            },
            completion: CapabilityStatus::Partial {
                reason:
                    "expression-member completion is native; entry details, event-handler completion and incomplete/unclosed expressions are not implemented"
                        .to_owned(),
            },
        }
    }
}

struct AdapterState {
    session: Option<Box<dyn QuerySession>>,
    open: Option<OpenProject>,
    next_revision: u64,
    generation: u64,
}

impl AdapterState {
    const fn new(generation: u64) -> Self {
        Self {
            session: None,
            open: None,
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
