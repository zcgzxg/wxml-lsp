//! Node + TypeScript type backend.
//!
//! [`NodeBackend`] implements the C0 [`wxml_types::Backend`] boundary on top of
//! the C1.1 NDJSON JSON-RPC process. It caches one `analyze` response per
//! view revision and all three capabilities reuse it. A miss or a transport
//! failure starts a new session and advances the backend epoch so the LSP layer
//! drops results from the old generation.
//!
//! The process boundary is the [`NodeTransport`] trait; tests use a fake
//! transport, while [`StdioNode`] runs `node server.js`.

pub mod completion;
pub mod config;
pub mod process;
pub mod protocol;
pub mod request;
pub mod response;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use wxml_types::{
    Backend, BackendCapabilities, BackendEpoch, BackendError, BackendId, CapabilityStatus,
    CompletionItem, CompletionRequest, CompletionResult, DefinitionRequest, DefinitionResult,
    DiagnosticsRequest, DiagnosticsResult, FileId, HoverRequest, HoverResult, MarkupKind,
    ProjectRevision, RequestContext, SourceRevision, TypeQuality, ViewRevision,
};

use crate::project_host::{PreparedView, ProjectHost};
use request::FragmentInfo;
use response::AnalyzeResponse;

pub use config::{JsCheckMode, NodeBackendConfig, RequestLimits, Typings};
pub use process::{NodeTransport, NodeTransportError, StdioNode};

/// A cache key for one analysis input set.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheKey {
    file: FileId,
    source_revision: SourceRevision,
    view_revision: ViewRevision,
    project_revision: ProjectRevision,
    epoch: u64,
}

/// One cached analysis response (or a syntax-only view without Node).
#[derive(Clone, Debug)]
struct CachedAnalysis {
    key: CacheKey,
    view: Arc<PreparedView>,
    response: Option<Arc<AnalyzeResponse>>,
    fragments: BTreeMap<String, FragmentInfo>,
}

#[derive(Default)]
struct BackendState {
    transport: Option<Box<dyn NodeTransport>>,
    cache: Option<CachedAnalysis>,
    next_request: u64,
}

type TransportFactory =
    Arc<dyn Fn() -> Result<Box<dyn NodeTransport>, NodeTransportError> + Send + Sync>;

/// A failed analysis; keeps the prepared view when one was already built so
/// diagnostics can preserve the P2/P6 diagnostics.
#[derive(Clone, Debug)]
struct AnalysisFailure {
    error: BackendError,
    view: Option<Arc<PreparedView>>,
}

/// A C1 Node backend.
pub struct NodeBackend {
    config: NodeBackendConfig,
    host: Arc<dyn ProjectHost>,
    state: Mutex<BackendState>,
    epoch: AtomicU64,
    id: BackendId,
    factory: TransportFactory,
    failures: AtomicU32,
    cooldown: Duration,
    cooldown_until: Mutex<Option<Instant>>,
}

impl fmt::Debug for NodeBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeBackend")
            .field("epoch", &self.epoch.load(Ordering::SeqCst))
            .field("failures", &self.failures.load(Ordering::SeqCst))
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl NodeBackend {
    /// Creates a backend that starts `config.server` on first use.
    #[must_use]
    pub fn new(config: NodeBackendConfig, host: Arc<dyn ProjectHost>) -> Self {
        if let Some(reason) = config.unavailable_reason() {
            crate::server::log(format_args!("type backend disabled: {reason}"));
        }
        let factory_config = config.clone();
        let factory: TransportFactory = Arc::new(move || {
            StdioNode::start(&factory_config).map(|node| Box::new(node) as Box<dyn NodeTransport>)
        });
        Self::with_transport_factory(config, host, move || factory())
    }

    /// Creates a backend with an explicit transport factory. Used by tests and
    /// embedders that own process supervision.
    #[must_use]
    pub fn with_transport_factory<F>(
        config: NodeBackendConfig,
        host: Arc<dyn ProjectHost>,
        factory: F,
    ) -> Self
    where
        F: Fn() -> Result<Box<dyn NodeTransport>, NodeTransportError> + Send + Sync + 'static,
    {
        let cooldown = config.restart_cooldown;
        Self {
            config,
            host,
            state: Mutex::new(BackendState::default()),
            epoch: AtomicU64::new(1),
            id: BackendId::new("node"),
            factory: Arc::new(factory),
            failures: AtomicU32::new(0),
            cooldown,
            cooldown_until: Mutex::new(None),
        }
    }

    /// Returns the current cache key for `context`.
    fn key(&self, context: &RequestContext) -> CacheKey {
        CacheKey {
            file: context.source().file().clone(),
            source_revision: context.source().revision(),
            view_revision: context.view_revision(),
            project_revision: context.project_revision(),
            epoch: self.epoch.load(Ordering::SeqCst),
        }
    }

    /// Analyzes one snapshot, using the cache when possible.
    // The state lock serializes transport, cache and epoch updates; the
    // request itself may block for the configured timeout.
    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    fn analyze(&self, context: &RequestContext) -> Result<Arc<CachedAnalysis>, AnalysisFailure> {
        let key = self.key(context);
        {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = state.cache.as_ref() {
                if cached.key == key {
                    return Ok(Arc::new(cached.clone()));
                }
            }
        }

        let view = match self.host.analysis(context.source()) {
            Ok(view) => Arc::new(view),
            Err(error) => {
                crate::server::log(format_args!("project analysis unavailable: {error}"));
                return Err(AnalysisFailure {
                    error: BackendError::Unavailable(
                        "no project view for this document".to_owned(),
                    ),
                    view: None,
                });
            }
        };

        if view.target.is_none() || (view.fragments.is_empty() && view.components.is_empty()) {
            let cached = CachedAnalysis {
                key,
                view,
                response: None,
                fragments: BTreeMap::new(),
            };
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.cache = Some(cached.clone());
            return Ok(Arc::new(cached));
        }
        let cooldown_until = *self
            .cooldown_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(until) = cooldown_until {
            if Instant::now() < until {
                return Err(AnalysisFailure {
                    error: BackendError::Unavailable(
                        "type backend is cooling down after repeated failures".to_owned(),
                    ),
                    view: Some(view),
                });
            }
        }

        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_request = state.next_request.saturating_add(1);
        let request_id = format!(
            "{}:{}:{}",
            view.view_id,
            self.epoch.load(Ordering::SeqCst),
            state.next_request
        );
        let built = match request::build(&view, context, &self.config, request_id) {
            Ok(Some(built)) => built,
            Ok(None) => {
                let cached = CachedAnalysis {
                    key,
                    view,
                    response: None,
                    fragments: BTreeMap::new(),
                };
                state.cache = Some(cached.clone());
                return Ok(Arc::new(cached));
            }
            Err(error) => {
                self.invalidate(&mut state);
                return Err(AnalysisFailure {
                    error: BackendError::Protocol(error.to_string()),
                    view: Some(view),
                });
            }
        };

        if state.transport.is_none() {
            match (self.factory)() {
                Ok(transport) => state.transport = Some(transport),
                Err(error) => {
                    self.invalidate(&mut state);
                    return Err(AnalysisFailure {
                        error: map_transport_error(&error),
                        view: Some(view),
                    });
                }
            }
        }
        let Some(transport) = state.transport.as_ref() else {
            return Err(AnalysisFailure {
                error: BackendError::Unavailable("node transport is not running".to_owned()),
                view: Some(view),
            });
        };
        let result = transport.request(&built.line, self.config.timeout);
        let line = match result {
            Ok(line) => line,
            Err(error) => {
                self.invalidate(&mut state);
                return Err(AnalysisFailure {
                    error: map_transport_error(&error),
                    view: Some(view),
                });
            }
        };
        let response =
            match response::parse_line(&line, &built.request_id, context.view_revision().get()) {
                Ok(response) => response,
                Err(error) => {
                    crate::server::log(format_args!(
                        "node C1 response rejected: {error}; line={}",
                        crate::handlers::log_safe_text(&line, 240)
                    ));
                    self.invalidate(&mut state);
                    return Err(AnalysisFailure {
                        error: BackendError::Protocol(error.to_string()),
                        view: Some(view),
                    });
                }
            };
        let cached = CachedAnalysis {
            key,
            view,
            response: Some(Arc::new(response)),
            fragments: built.fragments,
        };
        state.cache = Some(cached.clone());
        self.failures.store(0, Ordering::SeqCst);
        *self
            .cooldown_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        Ok(Arc::new(cached))
    }

    /// Drops the session/cache and advances the epoch after a failure.
    fn invalidate(&self, state: &mut BackendState) {
        if let Some(transport) = state.transport.take() {
            transport.shutdown();
        }
        state.cache = None;
        let _ = self.epoch.fetch_add(1, Ordering::SeqCst);
        let failures = self
            .failures
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        if failures >= 3 {
            *self
                .cooldown_until
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now() + self.cooldown);
        }
    }

    /// Returns the syntax-only status for a view with no Node response.
    fn hint_status(view: &PreparedView) -> CapabilityStatus {
        view.status_hint
            .clone()
            .unwrap_or(CapabilityStatus::Supported)
    }
}

/// Empty result carrying the reason a completion query could not run.
const fn degraded(status: CapabilityStatus) -> CompletionResult {
    CompletionResult {
        items: Vec::<CompletionItem>::new(),
        status,
        truncated: false,
    }
}

/// Maps a transport error to a degraded capability status.
fn unavailable(error: &NodeTransportError) -> CapabilityStatus {
    CapabilityStatus::Unavailable {
        reason: match error {
            NodeTransportError::NotConfigured => "type backend is not configured".to_owned(),
            NodeTransportError::Timeout => "type backend timed out".to_owned(),
            _ => "type backend is unavailable".to_owned(),
        },
    }
}

/// Maximum bytes of hint text sent for a completion prefix.
const MAX_COMPLETION_PREFIX_BYTES: usize = 256;

/// Identifier prefix immediately before `offset` (bounded to 256 bytes).
fn completion_prefix(text: &str, offset: u32) -> String {
    let Ok(offset) = usize::try_from(offset) else {
        return String::new();
    };
    let Some(head) = text.get(..offset) else {
        return String::new();
    };
    let bytes = head.as_bytes();
    let mut start = bytes.len();
    while start > 0 {
        let byte = bytes[start - 1];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'-' | b':') {
            start -= 1;
        } else {
            break;
        }
    }
    let token = head.get(start..).unwrap_or_default();
    if token.len() <= MAX_COMPLETION_PREFIX_BYTES {
        return token.to_owned();
    }
    // Keep the tail: the replacement range is derived from this prefix, so
    // dropping the *end* of the token would move the edit away from the cursor.
    let mut cut = token.len() - MAX_COMPLETION_PREFIX_BYTES;
    while cut < token.len() && !token.is_char_boundary(cut) {
        cut += 1;
    }
    token.get(cut..).unwrap_or_default().to_owned()
}

fn map_transport_error(error: &NodeTransportError) -> BackendError {
    match error {
        NodeTransportError::Timeout => BackendError::Timeout,
        NodeTransportError::Protocol(error) => BackendError::Protocol(error.to_string()),
        NodeTransportError::Spawn(_) => {
            BackendError::Unavailable("node failed to start".to_owned())
        }
        NodeTransportError::Io(_) => BackendError::Unavailable("node transport failed".to_owned()),
        NodeTransportError::Exited => BackendError::Unavailable("node exited".to_owned()),
        NodeTransportError::LineTooLarge => {
            BackendError::Protocol("node response line is too large".to_owned())
        }
        NodeTransportError::NotConfigured => {
            BackendError::Unavailable("type backend is not configured".to_owned())
        }
    }
}

impl Backend for NodeBackend {
    fn epoch(&self) -> BackendEpoch {
        BackendEpoch::new(self.id.clone(), self.epoch.load(Ordering::SeqCst))
    }

    fn hover(&self, request: HoverRequest) -> Result<HoverResult, BackendError> {
        let cached = self
            .analyze(&request.context)
            .map_err(|failure| failure.error)?;
        if cached.view.target.is_none() {
            return Err(BackendError::Unavailable(
                "no project view for this document".to_owned(),
            ));
        }
        if let Some(response) = cached.response.as_ref() {
            Ok(response::hover(
                response,
                &cached.view,
                &cached.fragments,
                request.offset,
            ))
        } else {
            Ok(HoverResult {
                contents: String::new(),
                markup: MarkupKind::Markdown,
                range: None,
                status: Self::hint_status(&cached.view),
                quality: TypeQuality::Unknown,
            })
        }
    }

    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError> {
        match self.analyze(&request.context) {
            Ok(cached) => {
                if let Some(response) = cached.response.as_ref() {
                    Ok(response::diagnostics(
                        response,
                        &cached.view,
                        request.context.source(),
                    ))
                } else {
                    Ok(DiagnosticsResult {
                        diagnostics: cached.view.syntax_diagnostics.clone(),
                        status: Self::hint_status(&cached.view),
                    })
                }
            }
            Err(failure) => {
                crate::server::log(format_args!(
                    "type backend diagnostics failed: {}",
                    failure.error
                ));
                let diagnostics = failure.view.as_ref().map_or_else(
                    || crate::project_host::parse_syntax_diagnostics(request.context.source()),
                    |view| view.syntax_diagnostics.clone(),
                );
                Ok(DiagnosticsResult {
                    diagnostics,
                    status: CapabilityStatus::Unavailable {
                        reason: "type backend unavailable".to_owned(),
                    },
                })
            }
        }
    }

    fn definition(&self, request: DefinitionRequest) -> Result<DefinitionResult, BackendError> {
        let cached = self
            .analyze(&request.context)
            .map_err(|failure| failure.error)?;
        if cached.view.target.is_none() {
            return Err(BackendError::Unavailable(
                "no project view for this document".to_owned(),
            ));
        }
        if let Some(response) = cached.response.as_ref() {
            Ok(response::definition(
                response,
                &cached.view,
                &cached.fragments,
                request.offset,
                self.host.as_ref(),
            ))
        } else {
            Ok(DefinitionResult {
                locations: Vec::new(),
                status: Self::hint_status(&cached.view),
            })
        }
    }

    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)] // one linear query
    fn completion(&self, request: CompletionRequest) -> Result<CompletionResult, BackendError> {
        let view = match self.host.analysis(request.context.source()) {
            Ok(view) => Arc::new(view),
            Err(error) => {
                crate::server::log(format_args!("completion view unavailable: {error}"));
                return Ok(degraded(CapabilityStatus::Unavailable {
                    reason: "no project view for this document".to_owned(),
                }));
            }
        };
        if view.target.is_none() {
            return Ok(degraded(CapabilityStatus::Unsupported {
                reason: "template has no page or component script".to_owned(),
            }));
        }
        let cooldown_until = *self
            .cooldown_until
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if cooldown_until.is_some_and(|until| Instant::now() < until) {
            return Ok(degraded(CapabilityStatus::Unavailable {
                reason: "type backend is cooling down after repeated failures".to_owned(),
            }));
        }
        let text = request.context.source().text();
        let offset = request
            .offset
            .min(u32::try_from(text.len()).unwrap_or(u32::MAX));
        let prefix = completion_prefix(text, offset);

        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_request = state.next_request.saturating_add(1);
        let request_id = format!(
            "{}:{}:{}",
            view.view_id,
            self.epoch.load(Ordering::SeqCst),
            state.next_request
        );
        let line = match request::build_completion(
            &view,
            &request.context,
            &self.config,
            &request_id,
            offset,
            request.kind,
            &prefix,
        ) {
            Ok(Some(line)) => line,
            Ok(None) => {
                return Ok(degraded(CapabilityStatus::Unsupported {
                    reason: "no script is available for this view".to_owned(),
                }));
            }
            Err(error) => {
                self.invalidate(&mut state);
                return Ok(degraded(CapabilityStatus::Unknown {
                    reason: Some(format!("completion protocol error: {error}")),
                }));
            }
        };
        if state.transport.is_none() {
            match (self.factory)() {
                Ok(transport) => state.transport = Some(transport),
                Err(error) => {
                    self.invalidate(&mut state);
                    return Ok(degraded(unavailable(&error)));
                }
            }
        }
        let Some(transport) = state.transport.as_ref() else {
            return Ok(degraded(CapabilityStatus::Unavailable {
                reason: "node transport is not running".to_owned(),
            }));
        };
        let response = match transport.request(&line, self.config.timeout) {
            Ok(line) => line,
            Err(error) => {
                self.invalidate(&mut state);
                return Ok(degraded(unavailable(&error)));
            }
        };
        let value = match protocol::parse_response(&response, &request_id) {
            Ok(value) => value,
            Err(error) => {
                self.invalidate(&mut state);
                return Ok(degraded(CapabilityStatus::Unknown {
                    reason: Some(format!("completion protocol error: {error}")),
                }));
            }
        };
        let text_len = u32::try_from(text.len()).unwrap_or(u32::MAX);
        match completion::parse(value, &request_id, text_len) {
            Ok(result) => {
                self.failures.store(0, Ordering::SeqCst);
                *self
                    .cooldown_until
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
                Ok(result)
            }
            Err(error) => {
                self.invalidate(&mut state);
                Ok(degraded(CapabilityStatus::Unknown {
                    reason: Some(format!("completion protocol error: {error}")),
                }))
            }
        }
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            hover: CapabilityStatus::Supported,
            diagnostics: CapabilityStatus::Supported,
            definition: CapabilityStatus::Supported,
            completion: CapabilityStatus::Supported,
        }
    }
}

impl Drop for NodeBackend {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(transport) = state.transport.take() {
            transport.shutdown();
        }
    }
}

#[cfg(test)]
mod tests;
