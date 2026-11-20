//! `wxml_types::Backend` adapter for the native `tsgo` backend.
//!
//! # Scope and safety
//!
//! This module is constructed by the `--stdio` selectors in `main.rs` and is
//! never selected by `server.rs`, `project_host.rs` or the configuration
//! defaults. `--stdio` without `--backend` uses [`run_stdio_native_default`]:
//! it serves the verified native engine when the running executable is inside
//! an installed package and falls back to [`run_stdio_host_only`] **only** when
//! discovery reports [`EngineDiscoveryError::NotInstalled`]. Every other
//! discovery or asset failure fails closed. `--backend native` uses the strict
//! [`run_stdio_native`] selector. There is no legacy Node backend: `--backend
//! node` is rejected and the runtime never constructs a Node process.
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
//! A template owner that the in-place shadow deliberately cannot analyze (for
//! example a non-literal `Page(options)` registration) is a static language
//! limit: every template capability answers `Unsupported` with a path-free
//! reason and no native session is started. Host or native-input failures stay
//! `Unavailable`, so a static limit is never confused with a backend outage.
//!
//! # Type environment
//!
//! [`run_stdio_native`] and [`run_stdio_native_default`] load the pinned type
//! assets once through `TsgoBackendAdapter::with_install`, before any sandbox
//! or session exists. A missing, unsafe or pin-mismatched tree aborts startup;
//! there is no Node or legacy fallback. `HostOnly` never loads assets and never
//! reads the legacy Node environment: it serves the static host layer with a
//! session factory that cannot start an engine.
//!
//! [`run_stdio_host_only`] exists only for the default selector's
//! `NotInstalled` state. Its type queries return `Unavailable` with a stable
//! reason; template syntax, formatting, structure, event facts and script
//! symbol facts keep working. It is not a Node fallback.
//!
//! `new` and `with_host` remain legacy test/probe constructors and are not
//! reachable from the production selectors.
//!
//! Only Linux x86-64 with POSIX absolute roots is supported: Windows
//! drive-letter/UNC roots, non-UTF-8 roots, symlinks, hard links and
//! non-regular files are rejected (fail closed).
//! The exposed [`BackendEpoch`] advances on [`Backend::invalidate`] and fatal
//! native failures, not on a lazy start or snapshot update: the in-flight
//! request is already stamped and [`RequestIdentity`] guards input changes.
//!
//! # Capabilities and blocked operations
//!
//! Rename edits remain explicitly `Unsupported`: the native API has no
//! `getRenameInfo` or `getEditsForRename`. Script symbol facts
//! (`DataRoots`/`DataPath`/`EventHandler`) are wired from the Rust script
//! overlay through [`Backend::symbols`] and do not open a native
//! session; they are conservative and report `Partial` whenever a dynamic or
//! cross-file shape is not proven. Definition is limited to expression-member
//! targets that the native referenced-symbol path can decode and map back to
//! original text; the cursor must be inside a mounted template expression. The
//! adapter does not fake locations or edits and does not call a Node fallback.
//!
//! Standalone script diagnostics are implemented at the adapter seam
//! ([`TsgoBackendAdapter::script_diagnostics`]) without a shadow or template
//! syntax. [`Backend::script_diagnostics`] exposes that seam to the LSP server,
//! which routes project `.ts`/`.js`/`.mts`/`.cts`/`.mjs`/`.cjs` documents here.
//! An unsupported or unavailable result is never published as an empty,
//! checked script.
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
    reason = "native selectors: legacy constructors/query seams and manifest \
              projections are test- or forward-compatibility-only"
)]

pub(crate) mod completion;
pub(crate) mod component_bindings;
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
pub(crate) mod symbols;

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

use self::diagnostics::DiagnosticsRun;
use self::engine::DiscoveredInstall;
use self::environment::Environment;
use self::event_completion::EventCandidateCache;
use self::input::{
    OpenPlan, PlanError, PlanIdentity, RequestPlan, ScriptViewError, ViewProvider,
    build_request_plan, build_script_request_plan, merge_reason,
};
use self::sandbox::SandboxSessionFactory;
use self::session::{HostOnlySessionFactory, NativeSessionFactory, QuerySession, SessionFactory};

/// Stable backend identity used in [`BackendEpoch`].
pub(crate) const BACKEND_ID: &str = "tsgo-native";

/// Stable reason reported by the `HostOnly` selector.
///
/// The text is path-free and shared by request results and the startup log, so
/// a user can tell that static WXML features are available while type checking
/// is not.
pub(crate) const HOST_ONLY_REASON: &str = "native engine is not installed";

/// Capability reason advertised by `HostOnly`.
///
/// `HostOnly` still answers static template, event and script-fact queries, so
/// the capability stays `Partial` rather than `Supported` or `Unavailable`.
const HOST_ONLY_CAPABILITY_REASON: &str = "native engine is not installed; static template, event and script-fact queries remain available, while type-backed queries are unavailable";

/// Why a native startup path could not begin serving.
///
/// Messages are path-free and stable enough for stderr. A failure never
/// constructs the Node backend or probes a JavaScript backend; the selectors
/// fail closed.
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

/// Serves the LSP over stdio with the strict native backend.
///
/// This is the explicit `--backend native` selector: it never degrades to
/// `HostOnly` and never constructs the Node backend. Discovery starts at the
/// running executable and requires
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

/// Serves the LSP over stdio with the default native selector.
///
/// The running executable is discovered as `<package>/bin/wxml-lsp` and the
/// verified engine plus pinned type assets are loaded exactly like
/// [`run_stdio_native`]. The only degradation is
/// [`EngineDiscoveryError::NotInstalled`]: the executable is not inside an
/// installed package, so the process starts [`run_stdio_host_only`] instead of
/// failing. Every other discovery, asset, sandbox or transport error is
/// returned unchanged; there is no Node or legacy fallback.
pub fn run_stdio_native_default() -> Result<crate::ServerOutcome, NativeStartupError> {
    let install = match engine::discover_from_current_exe() {
        Ok(install) => install,
        Err(EngineDiscoveryError::NotInstalled) => {
            crate::server::log(format_args!(
                "{HOST_ONLY_REASON}; serving static WXML features only"
            ));
            return run_stdio_host_only().map_err(NativeStartupError::Server);
        }
        Err(error) => return Err(NativeStartupError::Discovery(error)),
    };
    let host = Arc::new(crate::project_host::ProjectState::new());
    let adapter = TsgoBackendAdapter::with_install(host.clone(), &install)?;
    crate::server::run_stdio_with_host(Arc::new(adapter), host).map_err(NativeStartupError::Server)
}

/// Serves the LSP over stdio with the native `HostOnly` adapter.
///
/// `HostOnly` never discovers an engine, never loads type assets, never creates a
/// sandbox and never reads the removed `WXML_NODE`, `WXML_TYPE_BACKEND` or
/// `WXML_BACKEND_DIR` variables. Static template, style, event and
/// script-symbol features keep working; every session-backed query returns
/// `Unavailable` with `native engine is not installed`. It is not a Node fallback.
///
/// # Errors
///
/// Returns [`crate::ServerError`] only for transport or thread startup failures.
pub fn run_stdio_host_only() -> Result<crate::ServerOutcome, crate::ServerError> {
    let host = Arc::new(crate::project_host::ProjectState::new());
    let adapter = TsgoBackendAdapter::without_engine(host.clone());
    crate::server::run_stdio_with_host(Arc::new(adapter), host)
}

/// One reusable session operation for the restart retry path.
type SessionOperation<'a, R> =
    dyn FnMut(&mut dyn QuerySession, Option<String>) -> Result<R, BackendError> + 'a;

/// Adapter between the LSP project host and the native session.
///
/// Construction is explicit. Only the `--stdio` native selectors construct it,
/// and only the `HostOnly` selector leaves `engine_unavailable` set.
pub(crate) struct TsgoBackendAdapter {
    engine: PinnedEngine,
    views: Arc<dyn ViewProvider>,
    sessions: Arc<dyn SessionFactory>,
    environment: Environment,
    /// When set, every session-backed query fails closed with this reason.
    ///
    /// `HostOnly` sets it because discovery reported `NotInstalled`; the static
    /// host layer never opens a session, so no engine process or type asset is
    /// reached.
    engine_unavailable: Option<&'static str>,
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
    /// Engine manifest projection and asset sets are verified independently:
    /// the delivery reader validates `typeAssets`/`binaries`/`builtAt` before
    /// activation, while this constructor re-verifies every mounted
    /// declaration byte against the crate pin.
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

    /// Creates the `HostOnly` adapter for a package that is not installed.
    ///
    /// No engine is discovered, no type assets are loaded and no sandbox is
    /// created. The legacy [`Environment::Legacy`] mode is reused deliberately:
    /// it carries no assets, so a session can never be opened with a partial
    /// type environment. [`Self::ensure_open`] short-circuits every
    /// session-backed query with `native engine is not installed`.
    #[must_use]
    pub(crate) fn without_engine(host: Arc<dyn crate::project_host::ProjectHost>) -> Self {
        Self {
            engine: PinnedEngine::pinned(),
            views: Arc::new(input::HostViewProvider::new(host)),
            sessions: Arc::new(HostOnlySessionFactory),
            environment: Environment::legacy(),
            engine_unavailable: Some(HOST_ONLY_REASON),
            state: Mutex::new(AdapterState::new(1)),
            event_cache: Mutex::new(None),
            epoch: AtomicU64::new(1),
        }
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
            engine_unavailable: None,
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

    fn prepare(&self, context: &wxml_types::RequestContext) -> Result<RequestPlan, PlanError> {
        build_request_plan(self.views.as_ref(), context, &self.environment)
    }

    fn prepare_script(
        &self,
        context: &wxml_types::RequestContext,
    ) -> Result<input::ScriptRequestPlan, ScriptViewError> {
        build_script_request_plan(self.views.as_ref(), context, &self.environment)
    }

    /// Runs one purely syntactic query without the session environment reason.
    ///
    /// `document_symbols` cannot be affected by a degraded type environment,
    /// so this path neither clones nor merges `environment_reason`.
    fn with_session<R>(
        &self,
        plan: &dyn OpenPlan,
        mut operation: impl FnMut(&mut dyn QuerySession) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        self.with_session_mode(plan, false, |session, _degradation| operation(session))
    }

    /// Runs one typed query and merges the open-session environment reason.
    ///
    /// The reason is read after [`Self::ensure_open`] so the first request that
    /// opens a degraded VFS already reports `Partial`.
    fn with_session_environment<R>(
        &self,
        plan: &dyn OpenPlan,
        operation: impl FnMut(&mut dyn QuerySession, Option<String>) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        self.with_session_mode(plan, true, operation)
    }

    /// Runs one session operation with a single dead-session restart retry.
    ///
    /// A fatal result whose engine process is already dead is retried once on
    /// a freshly started session. The retry deliberately does not advance the
    /// adapter epoch when it succeeds: the request's own stamp is still
    /// current, and the old process cannot publish a competing result because
    /// the adapter lock is held for the whole operation. An external
    /// invalidation during the query is never retried and keeps the historical
    /// `Cancelled` result. Non-fatal errors, `Ok(None)` and `Unsupported`
    /// results are returned without a retry.
    fn with_session_mode<R>(
        &self,
        plan: &dyn OpenPlan,
        include_environment: bool,
        mut operation: impl FnMut(&mut dyn QuerySession, Option<String>) -> Result<R, BackendError>,
    ) -> Result<R, BackendError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.ensure_open(&mut state, plan)?;
        let first =
            Self::run_session_operation(&mut state, plan, include_environment, &mut operation);

        let current = self.epoch.load(Ordering::SeqCst);
        if state.generation != current {
            let dead = Self::take_session(&mut state);
            state.generation = current;
            drop(state);
            if let Some(session) = dead {
                session.shutdown();
            }
            return Err(BackendError::Cancelled);
        }

        let first_is_fatal = first.as_ref().is_err_and(fatal_error);
        let dead_session = first_is_fatal
            && state
                .session
                .as_mut()
                .is_some_and(|session| !session.is_alive());
        if !dead_session {
            let dead = if first_is_fatal {
                self.fail_session(&mut state)
            } else {
                None
            };
            drop(state);
            if let Some(session) = dead {
                session.shutdown();
            }
            return first;
        }

        // The engine died during the query. Drop it without advancing the
        // epoch and make exactly one fresh attempt. If an external
        // invalidation races with the restart, the request is stale and must
        // not be retried.
        let dead = Self::take_session(&mut state);
        drop(state);
        if let Some(session) = dead {
            session.shutdown();
        }

        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let current = self.epoch.load(Ordering::SeqCst);
        if state.generation != current {
            let dead = Self::take_session(&mut state);
            state.generation = current;
            drop(state);
            if let Some(session) = dead {
                session.shutdown();
            }
            return Err(BackendError::Cancelled);
        }
        if let Err(error) = self.ensure_open(&mut state, plan) {
            let dead = self.fail_session(&mut state);
            drop(state);
            if let Some(session) = dead {
                session.shutdown();
            }
            return Err(error);
        }
        let retry =
            Self::run_session_operation(&mut state, plan, include_environment, &mut operation);
        let current = self.epoch.load(Ordering::SeqCst);
        if state.generation != current {
            let dead = Self::take_session(&mut state);
            state.generation = current;
            drop(state);
            if let Some(session) = dead {
                session.shutdown();
            }
            return Err(BackendError::Cancelled);
        }
        let retry_is_fatal = retry.as_ref().is_err_and(fatal_error);
        let dead = if retry_is_fatal {
            self.fail_session(&mut state)
        } else {
            None
        };
        drop(state);
        if let Some(session) = dead {
            session.shutdown();
        }
        retry
    }

    /// Runs one operation against the currently open session.
    fn run_session_operation<R>(
        state: &mut AdapterState,
        plan: &dyn OpenPlan,
        include_environment: bool,
        operation: &mut SessionOperation<'_, R>,
    ) -> Result<R, BackendError> {
        let mut degradation = plan.degradation();
        if include_environment {
            merge_reason(&mut degradation, plan.environment_degradation());
            merge_reason(&mut degradation, state.environment_reason.clone());
        }
        let session = state.session.as_mut().ok_or_else(|| {
            BackendError::Unavailable("native session was not established".to_owned())
        })?;
        operation(session.as_mut(), degradation)
    }

    /// Removes the open session without changing the epoch.
    fn take_session(state: &mut AdapterState) -> Option<Box<dyn QuerySession>> {
        state.open = None;
        state.environment_reason = None;
        state.session.take()
    }

    /// Advances the epoch, removes the open session and returns it for shutdown.
    fn fail_session(&self, state: &mut AdapterState) -> Option<Box<dyn QuerySession>> {
        let next = self.advance_epoch();
        state.generation = next;
        Self::take_session(state)
    }

    fn ensure_open(
        &self,
        state: &mut AdapterState,
        plan: &dyn OpenPlan,
    ) -> Result<(), BackendError> {
        if let Some(reason) = self.engine_unavailable {
            return Err(BackendError::Unavailable(reason.to_owned()));
        }
        self.drop_invalidated(state);
        if let Some(open) = &state.open {
            if open.identity == plan.plan_identity() && state.session.is_some() {
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
        plan: &dyn OpenPlan,
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
        plan: &dyn OpenPlan,
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

    /// Returns native diagnostics for one standalone project script.
    ///
    /// [`Backend::script_diagnostics`] is the LSP-facing trait seam and calls
    /// this method. It never merges template syntax diagnostics, never opens a
    /// shadow and never invents an empty `Supported` result:
    ///
    /// - a non-script/non-project document is `Unsupported` with no entries;
    /// - a missing engine or failed session is `Unavailable`;
    /// - diagnostics for other files (for example external declarations) are
    ///   excluded and degrade the result to `Partial`;
    /// - an engine-owned empty list for both sources is a genuine
    ///   `Supported` clean result.
    pub(crate) fn script_diagnostics(
        &self,
        request: &DiagnosticsRequest,
    ) -> Result<DiagnosticsResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = match self.prepare_script(&request.context) {
            Ok(plan) => plan,
            Err(ScriptViewError::Unsupported(reason)) => {
                return Ok(DiagnosticsResult {
                    diagnostics: Vec::new(),
                    status: CapabilityStatus::Unsupported { reason },
                });
            }
            Err(ScriptViewError::Unavailable(reason)) => {
                return Ok(DiagnosticsResult {
                    diagnostics: Vec::new(),
                    status: CapabilityStatus::Unavailable { reason },
                });
            }
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if self.ensure_open(&mut state, &plan).is_err() {
            return Ok(unavailable_script_diagnostics("type backend unavailable"));
        }
        let mut degradation = plan.degradation.clone();
        merge_reason(&mut degradation, plan.environment_degradation.clone());
        merge_reason(&mut degradation, state.environment_reason.clone());
        let Some(session) = state.session.as_mut() else {
            return Ok(unavailable_script_diagnostics("type backend unavailable"));
        };
        let run = diagnostics::script_diagnostics(
            session.as_mut(),
            &plan.identity.project_root,
            &plan.path,
            request.context.source(),
            degradation.as_deref(),
        );
        Ok(self.finish_diagnostics(state, run, || {
            unavailable_script_diagnostics("request cancelled")
        }))
    }

    /// Drops a dead session after one diagnostics run and returns the result.
    fn finish_diagnostics(
        &self,
        mut state: std::sync::MutexGuard<'_, AdapterState>,
        run: DiagnosticsRun,
        cancelled: impl FnOnce() -> DiagnosticsResult,
    ) -> DiagnosticsResult {
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
            return cancelled();
        }
        run.result
    }

    /// Answers one member completion through a completion-only shadow plan.
    ///
    /// `prefix` is the bare data root before a trailing dot (`user` in
    /// `{{ user.`) or before an unclosed typed member prefix (`user` in
    /// `{{ user.na`). The script overlay decides whether the root is a declared
    /// data field first. An unknown root is an explicit `Unsupported`/`Partial`
    /// answer; it never falls through to a silent empty native list. A mounted
    /// prefix is queried at a synthetic `.` while the published replacement
    /// range stays the original typed prefix.
    fn completion_member(
        &self,
        request: &CompletionRequest,
        prefix: String,
        prefix_range: wxml_types::SourceRange,
        replace: wxml_types::SourceRange,
    ) -> Result<CompletionResult, BackendError> {
        let Some(root) = completion::member_prefix_root(&prefix) else {
            return Ok(completion::unsupported_completion(
                "the expression before the trailing dot has no data root",
            ));
        };
        match completion::member_root(self.views.as_ref(), request, root)? {
            completion::MemberRoot::Known => {}
            completion::MemberRoot::UnknownComplete => {
                return Ok(completion::unsupported_completion(&format!(
                    "`{root}` is not a data root of the template owner"
                )));
            }
            completion::MemberRoot::UnknownIncomplete(reason) => {
                return Ok(completion::partial_completion(reason));
            }
            completion::MemberRoot::Unavailable(reason) => {
                return Ok(completion::unsupported_completion(&reason));
            }
        }
        let fragment = input::CompletionFragment {
            expr: prefix,
            wxml_range: prefix_range,
        };
        let completion_plan = match input::build_completion_request_plan(
            self.views.as_ref(),
            &request.context,
            &self.environment,
            &fragment,
        ) {
            Ok(plan) => plan,
            Err(
                PlanError::Unsupported(reason)
                | PlanError::Backend(BackendError::Unavailable(reason)),
            ) => {
                return Ok(completion::unsupported_completion(&format!(
                    "the incomplete member expression cannot be mounted: {reason}"
                )));
            }
            Err(PlanError::Backend(error)) => return Err(error),
        };
        let Some(position) = completion_plan.completion_query else {
            return Ok(completion::unsupported_completion(
                "the incomplete member query position is unavailable",
            ));
        };
        let script_path = completion_plan.identity.script_path.clone();
        self.with_session_environment(&completion_plan, |session, degradation| {
            completion::completion(
                session,
                &completion_plan.layout,
                &script_path,
                degradation.as_deref(),
                request,
                Some(position),
                Some(replace),
            )
        })
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
        let plan = match self.prepare(&request.context) {
            Ok(plan) => plan,
            Err(PlanError::Unsupported(reason)) => return Ok(unsupported_hover(&reason)),
            Err(PlanError::Backend(error)) => return Err(error),
        };
        let script_path = plan.identity.script_path.clone();
        self.with_session_environment(&plan, |session, degradation| {
            hover::hover(
                session,
                &plan.layout,
                &script_path,
                &plan.declared_properties,
                degradation.as_deref(),
                &request,
            )
        })
    }

    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = match self.prepare(&request.context) {
            Ok(plan) => plan,
            Err(PlanError::Unsupported(reason)) => {
                // A statically unanalyzable owner still publishes host syntax
                // evidence; only the type-backed part is `Unsupported`.
                return Ok(unsupported_diagnostics(
                    crate::project_host::parse_syntax_diagnostics(request.context.source()),
                    &reason,
                ));
            }
            Err(PlanError::Backend(_)) => {
                return Ok(unavailable_diagnostics(
                    crate::project_host::parse_syntax_diagnostics(request.context.source()),
                    "type backend unavailable",
                ));
            }
        };
        let script_path = plan.identity.script_path.clone();
        let syntax = merged_syntax(&plan.syntax_diagnostics, request.context.source());
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if self.ensure_open(&mut state, &plan).is_err() {
            return Ok(unavailable_diagnostics(syntax, "type backend unavailable"));
        }
        let mut degradation = plan.degradation.clone();
        merge_reason(&mut degradation, plan.environment_degradation.clone());
        merge_reason(&mut degradation, state.environment_reason.clone());
        let Some(session) = state.session.as_mut() else {
            return Ok(unavailable_diagnostics(syntax, "type backend unavailable"));
        };
        let run = diagnostics::diagnostics(
            session.as_mut(),
            &plan.layout,
            &plan.view,
            &script_path,
            &plan.identity.project_root,
            &syntax,
            degradation.as_deref(),
        );
        Ok(self.finish_diagnostics(state, run, || {
            unavailable_diagnostics(syntax, "request cancelled")
        }))
    }

    fn script_diagnostics(
        &self,
        request: DiagnosticsRequest,
    ) -> Result<DiagnosticsResult, BackendError> {
        Self::script_diagnostics(self, &request)
    }

    fn definition(&self, request: DefinitionRequest) -> Result<DefinitionResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = match self.prepare(&request.context) {
            Ok(plan) => plan,
            Err(PlanError::Unsupported(reason)) => return Ok(unsupported_definition(&reason)),
            Err(PlanError::Backend(error)) => return Err(error),
        };
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
        let text = request.context.source().text();
        let site = completion::expression_site(text, request.offset);
        if let Some(completion::ExpressionSite::Root { replace }) = &site {
            // Data roots are Rust script facts: no shadow and no native session.
            return completion::root_completion(self.views.as_ref(), &request, *replace);
        }
        if let Some(completion::ExpressionSite::Member {
            prefix,
            prefix_range,
            replace,
        }) = &site
        {
            // A trailing dot can never be a mounted fragment; answer it without
            // requiring the normal shadow plan to be representable.
            return self.completion_member(&request, prefix.clone(), *prefix_range, *replace);
        }
        let plan = match self.prepare(&request.context) {
            Ok(plan) => plan,
            Err(PlanError::Unsupported(reason)) => {
                return Ok(completion::unsupported_completion(&reason));
            }
            Err(PlanError::Backend(error)) => return Err(error),
        };
        let script_path = plan.identity.script_path.clone();
        if let Some(position) = completion::query_position(&plan.layout, request.offset)? {
            return self.with_session_environment(&plan, |session, degradation| {
                completion::completion(
                    session,
                    &plan.layout,
                    &script_path,
                    degradation.as_deref(),
                    &request,
                    Some(position),
                    None,
                )
            });
        }
        match site {
            Some(completion::ExpressionSite::Unsupported(reason)) => {
                Ok(completion::unsupported_completion(reason))
            }
            _ => Ok(completion::unsupported_completion(
                "the cursor is not in a completable template expression",
            )),
        }
    }

    fn symbols(&self, request: SymbolRequest) -> Result<SymbolResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        symbols::symbols(self.views.as_ref(), &request)
    }

    fn document_symbols(
        &self,
        request: DocumentSymbolsRequest,
    ) -> Result<DocumentSymbolsResult, BackendError> {
        if !self.epoch_matches(&request.context) {
            return Err(BackendError::Cancelled);
        }
        let plan = match self.prepare(&request.context) {
            Ok(plan) => plan,
            Err(PlanError::Unsupported(reason)) => {
                return Ok(DocumentSymbolsResult::unsupported(reason));
            }
            Err(PlanError::Backend(error)) => return Err(error),
        };
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
        if self.engine_unavailable.is_some() {
            // Static template, event and script-fact queries still answer, so
            // this is `Partial`, not `Supported`: every type-backed request is
            // `Unavailable` and no result claims a completed type check.
            let status = CapabilityStatus::Partial {
                reason: HOST_ONLY_CAPABILITY_REASON.to_owned(),
            };
            return BackendCapabilities {
                hover: status.clone(),
                diagnostics: status.clone(),
                definition: status.clone(),
                completion: status,
            };
        }
        BackendCapabilities {
            hover: CapabilityStatus::Partial {
                reason:
                    "native expression-member hover is wired; event-handler and component semantics are served by the static layer"
                        .to_owned(),
            },
            diagnostics: CapabilityStatus::Partial {
                reason:
                    "host syntax plus native fragment diagnostics are wired; standalone project script diagnostics are routed through the separate script seam; event-handler diagnostics are served by host static semantics; local component property binding diagnostics are wired, while composite expressions, array/object structure and unresolved component declarations may degrade to Partial"
                        .to_owned(),
            },
            definition: CapabilityStatus::Partial {
                reason:
                    "native expression-member definition is wired; event-handler definitions are served by host static semantics; component and non-expression targets are not covered"
                        .to_owned(),
            },
            completion: CapabilityStatus::Partial {
                reason:
                    "data-root completion from the Rust script overlay and native expression-member completion are wired; a bare data root before a trailing dot (`{{ user. }}`) and an unclosed typed member prefix (`{{ user.na`) are mounted through a completion-only shadow, while member chains before the dot (`user.name.`) and template-local roots (`item.` inside `wx:for`) are not resolved; prepared auto-import entries are queried but the pinned API exposes no origin or edit, so auto-import items stay Partial and no import edit is offered; the pinned API also has no per-entry documentation, so non-empty native results stay Partial; event-handler completion is served from the Rust script method table without a native session and can be Supported; `this.`/method-body completion, custom component event signatures and handler signature help are not implemented"
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
    identity: PlanIdentity,
    project: ProjectKey,
    root: String,
}

impl OpenProject {
    fn new(plan: &dyn OpenPlan, input: &AnalysisInput) -> Self {
        Self {
            identity: plan.plan_identity(),
            project: input.project().clone(),
            root: input.root().as_str().to_owned(),
        }
    }
}

/// Returns whether the open snapshot can be advanced in place.
///
/// The environment identity and project/root must be byte-identical; only
/// source/view revisions may differ.
fn same_open_project(state: &AdapterState, plan: &dyn OpenPlan, input: &AnalysisInput) -> bool {
    state.open.as_ref().is_some_and(|open| {
        open.project == *input.project()
            && open.root == input.root().as_str()
            && open.identity.environment() == plan.environment_identity()
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

/// Returns an empty `Unsupported` hover; the owner shape has no type probe.
fn unsupported_hover(reason: &str) -> HoverResult {
    HoverResult {
        contents: String::new(),
        markup: wxml_types::MarkupKind::Markdown,
        range: None,
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
        quality: wxml_types::TypeQuality::Unknown,
    }
}

/// Returns an empty `Unsupported` definition; no location is invented.
fn unsupported_definition(reason: &str) -> DefinitionResult {
    DefinitionResult {
        locations: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
    }
}

/// Returns host syntax evidence with an explicit `Unsupported` type status.
fn unsupported_diagnostics(
    diagnostics: Vec<wxml_types::Diagnostic>,
    reason: &str,
) -> DiagnosticsResult {
    DiagnosticsResult {
        diagnostics,
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
    }
}

fn unavailable_script_diagnostics(reason: &str) -> DiagnosticsResult {
    unavailable_diagnostics(Vec::new(), reason)
}

/// Returns whether an error is a transport/session failure that can justify
/// one dead-session restart retry.
const fn fatal_error(error: &BackendError) -> bool {
    matches!(
        error,
        BackendError::Unavailable(_) | BackendError::Protocol(_) | BackendError::Timeout
    )
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
