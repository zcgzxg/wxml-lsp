//! Protocol state machine and stdio event loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufReader};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;
use wxml_types::{
    Backend, BackendCapabilities, BackendEpoch, BackendId, CapabilityStatus, Diagnostic,
};

use crate::configuration::InitializationOptions;
use crate::jsonrpc::{self, FrameError, Incoming, Outgoing, RequestKey};
use crate::project_host::{NullHost, ProjectHost};
use crate::scheduler::{Job, JobKind, Scheduler, SubmitError, WorkDone};
use crate::workspace::Workspace;

/// What the server did while serving one stdio session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerOutcome {
    /// `true` when the client sent `shutdown` before `exit`, matching the
    /// process exit code required by the protocol.
    pub clean_shutdown: bool,
}

/// A fatal server startup or transport failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServerError {
    /// Reading from or writing to the stdio transport failed.
    #[error("stdio transport failed: {0}")]
    Io(#[from] io::Error),
    /// The backend worker or reader thread could not be started.
    #[error("failed to start a server thread: {0}")]
    Worker(#[source] io::Error),
}

/// Where dispatch sends backend work.
pub(crate) trait Dispatch: Send + Sync {
    /// Queues one job.
    fn submit(&self, job: Job) -> Result<(), SubmitError>;

    /// Drops a queued interactive job after its request was cancelled.
    ///
    /// The server answers the request before calling this, so no completion is
    /// expected. A running job is stopped by the shared cancel flag instead.
    fn cancel(&self, _key: &RequestKey) {}
}

impl Dispatch for Scheduler {
    fn submit(&self, job: Job) -> Result<(), SubmitError> {
        Self::submit(self, job)
    }

    fn cancel(&self, key: &RequestKey) {
        Self::cancel(self, key);
    }
}

/// Lifecycle phase of the protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lifecycle {
    /// No `initialize` has been received.
    Uninitialized,
    /// Normal request handling.
    Running,
    /// `shutdown` was received; only `exit` and errors are allowed.
    ShuttingDown,
}

/// Number of protocol events that may wait between the reader/worker and the
/// protocol loop. Bounding this channel is what keeps a client that sends
/// frames faster than the server writes responses from growing memory without
/// limit.
pub(crate) const EVENT_QUEUE_CAPACITY: usize = 8;

/// Maximum number of pending diagnostics retried after one completed job.
const MAX_DIAGNOSTICS_RETRIES: usize = 8;

/// Default quiet period before a typed edit schedules diagnostics.
///
/// A burst of keystrokes therefore runs at most one backend analysis instead
/// of one per character. Opening, saving and configuration changes bypass the
/// debounce and schedule immediately.
pub(crate) const DEFAULT_DIAGNOSTICS_DEBOUNCE: Duration = Duration::from_millis(120);

/// Largest accepted debounce configured through
/// `WXML_DIAGNOSTICS_DEBOUNCE_MS`.
const MAX_DIAGNOSTICS_DEBOUNCE_MS: u64 = 5_000;

/// Maximum number of documents waiting for their debounce deadline.
pub(crate) const MAX_DEBOUNCED_DIAGNOSTICS: usize = 256;

/// A request whose response has not been produced yet.
#[derive(Clone, Debug)]
pub(crate) struct Inflight {
    /// Id to echo in the response.
    pub(crate) id: serde_json::Value,
    /// Monotonic attempt for this request id.
    pub(crate) attempt: u64,
    /// Set by `$/cancelRequest`.
    pub(crate) cancel: Arc<AtomicBool>,
    /// Document the request belongs to, so `didClose` can cancel it.
    pub(crate) uri: String,
}

/// Client capabilities that change server behavior.
#[allow(clippy::struct_excessive_bools)] // independent protocol feature flags
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClientFlags {
    /// The client declared dynamic watched-file registrations.
    pub(crate) watched_files: bool,
    /// The client accepts LSP snippets
    /// (`textDocument.completion.completionItem.snippetSupport`).
    pub(crate) snippets: bool,
    /// The client accepts hierarchical `DocumentSymbol` results
    /// (`textDocument.documentSymbol.hierarchicalDocumentSymbolSupport`).
    pub(crate) hierarchical_symbols: bool,
    /// The client accepts `WorkspaceEdit.documentChanges`.
    pub(crate) document_changes: bool,
}

/// One connected client.
pub(crate) struct Server {
    /// Open documents.
    pub(crate) workspace: Workspace,
    /// Project routing and analysis inputs.
    pub(crate) host: Arc<dyn ProjectHost>,
    /// Protocol phase.
    pub(crate) lifecycle: Lifecycle,
    /// Backend used to refresh the generation before scheduling/publishing.
    pub(crate) backend: Arc<dyn Backend>,
    /// Backend capabilities advertised to the client.
    pub(crate) capabilities: BackendCapabilities,
    /// Last observed backend generation.
    pub(crate) epoch: BackendEpoch,
    /// Sink for backend work.
    pub(crate) dispatch: Arc<dyn Dispatch>,
    /// Requests waiting for their response.
    pub(crate) inflight: HashMap<RequestKey, Inflight>,
    /// Documents whose diagnostics could not be queued because the worker was
    /// busy. They are retried in rotation after completed jobs.
    pub(crate) dirty: VecDeque<String>,
    /// Cancellation flag of the newest queued diagnostics job per document.
    ///
    /// Every edit supersedes the previous diagnostics request. Cancelling the
    /// old flag keeps a burst of keystrokes from queueing one multi-second
    /// analysis per character ahead of hover and completion requests. Script
    /// documents use the same per-URI slot because a document is either a
    /// template or a script.
    pub(crate) diagnostics_cancel: HashMap<String, Arc<AtomicBool>>,
    /// Script diagnostics jobs currently queued or running, keyed by URI.
    ///
    /// A request for the exact same stamp is not queued twice while its result
    /// is still in flight. The entry is removed when that stamp completes,
    /// fails or is superseded.
    pub(crate) script_diagnostics_inflight: HashMap<String, crate::workspace::AnalysisStamp>,
    /// Script documents with a published supported/partial result.
    ///
    /// Used only to clear the client when such a document closes. A script that
    /// never published (for example the default Node backend) sends nothing.
    pub(crate) script_diagnostics_published: HashSet<String>,
    /// Set by `exit` or a fatal transport error.
    pub(crate) stop: bool,
    /// Set by `shutdown`.
    pub(crate) clean_shutdown: bool,
    /// Capability flags read once from `initialize`.
    pub(crate) client: ClientFlags,
    /// Workspace/user snippet configuration and cache.
    pub(crate) snippets: crate::completion::user_snippets::Store,
    /// Synthetic (non-project) documents the client opened as WXML.
    ///
    /// Their route is `Unknown`, so the URI extension alone cannot tell a
    /// template from another language (for example `untitled:` buffers).
    pub(crate) synthetic_templates: HashSet<String>,
    /// Runtime semantic-tool switches.
    pub(crate) semantic_tools: crate::semantics::SemanticToolsConfig,
    /// Pending references/rename/code-action intentions waiting for a
    /// script symbol query.
    ///
    /// [`Self::inflight`] remains the only owner of response delivery. Pending
    /// intents are auxiliary state and are removed on completion, cancellation
    /// or workspace removal without sending a second response.
    pub(crate) semantic_pending: HashMap<RequestKey, crate::semantics::handlers::PendingSemantic>,
    /// Next request attempt; never reused within one session.
    pub(crate) next_attempt: u64,
    /// Last backend diagnostics for one document, keyed by client URI.
    ///
    /// Style changes refresh only the local diagnostics; the cached backend
    /// set is re-published with them when the document snapshot is unchanged.
    pub(crate) backend_diagnostics: HashMap<String, CachedDiagnostics>,
    /// Quiet period before a typed edit schedules diagnostics.
    pub(crate) diagnostics_debounce: Duration,
    /// Documents waiting for their diagnostics debounce deadline.
    pub(crate) debounced_diagnostics: HashMap<String, Instant>,
    /// Root URI passed to `initialize`, retained for workspace removal cleanup.
    pub(crate) workspace_root: Option<String>,
    /// Initialization options retained for an explicit re-open after a
    /// workspace-folder removal.
    pub(crate) initialization: InitializationOptions,
}

/// Backend diagnostics cached for local style refreshes.
#[derive(Clone, Debug)]
pub(crate) struct CachedDiagnostics {
    /// Full analysis stamp the result belongs to.
    pub(crate) stamp: crate::workspace::AnalysisStamp,
    /// Backend diagnostics without local style/resource diagnostics.
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Backend check status.
    pub(crate) status: CapabilityStatus,
}

impl Server {
    /// Creates a server for one backend.
    ///
    /// A backend that panics while reporting its epoch is degraded to a fixed
    /// fallback generation; capabilities are sanitized by the caller with
    /// [`safe_capabilities`].
    #[cfg(test)]
    pub(crate) fn new(
        backend: Arc<dyn Backend>,
        capabilities: BackendCapabilities,
        dispatch: Arc<dyn Dispatch>,
    ) -> Self {
        Self::new_with_host(backend, capabilities, dispatch, Arc::new(NullHost))
    }

    /// Creates a server bound to a project host.
    pub(crate) fn new_with_host(
        backend: Arc<dyn Backend>,
        capabilities: BackendCapabilities,
        dispatch: Arc<dyn Dispatch>,
        host: Arc<dyn ProjectHost>,
    ) -> Self {
        let epoch = safe_epoch(&backend, None);
        Self {
            workspace: Workspace::new(),
            host,
            lifecycle: Lifecycle::Uninitialized,
            backend,
            capabilities,
            epoch,
            dispatch,
            inflight: HashMap::new(),
            dirty: VecDeque::new(),
            diagnostics_cancel: HashMap::new(),
            script_diagnostics_inflight: HashMap::new(),
            script_diagnostics_published: HashSet::new(),
            stop: false,
            clean_shutdown: false,
            client: ClientFlags::default(),
            snippets: crate::completion::user_snippets::Store::default(),
            synthetic_templates: HashSet::new(),
            semantic_tools: crate::semantics::SemanticToolsConfig::default(),
            semantic_pending: HashMap::new(),
            next_attempt: 0,
            backend_diagnostics: HashMap::new(),
            diagnostics_debounce: diagnostics_debounce_from_env(),
            debounced_diagnostics: HashMap::new(),
            workspace_root: None,
            initialization: InitializationOptions::default(),
        }
    }

    /// Reads the backend epoch again before scheduling or publishing.
    ///
    /// A backend restart bumps its epoch; results produced for an older
    /// generation must not be published. `Backend::epoch` implementations
    /// must be O(1), stable unless the backend instance is actually replaced,
    /// and must not advance the epoch as a side effect of being read. The
    /// protocol paths call this at most once per scheduling/publishing event.
    pub(crate) fn refresh_epoch(&mut self) -> BackendEpoch {
        self.epoch = safe_epoch(&self.backend, Some(&self.epoch));
        self.epoch.clone()
    }

    /// Marks a document for a diagnostics retry.
    pub(crate) fn mark_dirty(&mut self, uri: &str) {
        if !self.dirty.iter().any(|dirty| dirty == uri) {
            self.dirty.push_back(uri.to_owned());
        }
    }

    /// Removes a document from the diagnostics retry queue.
    pub(crate) fn clear_dirty(&mut self, uri: &str) {
        self.dirty.retain(|dirty| dirty != uri);
    }

    /// Allocates the next request attempt.
    pub(crate) const fn next_attempt(&mut self) -> u64 {
        self.next_attempt = self.next_attempt.saturating_add(1);
        self.next_attempt
    }

    /// Returns the earliest pending diagnostics deadline.
    pub(crate) fn next_diagnostics_deadline(&self) -> Option<Instant> {
        self.debounced_diagnostics.values().copied().min()
    }

    /// Handles one parsed client message.
    pub(crate) fn handle_incoming(&mut self, incoming: Incoming) -> Vec<Outgoing> {
        match incoming {
            Incoming::Request { id, method, params } => self.on_request(id, &method, params),
            Incoming::Notification { method, params } => self.on_notification(&method, params),
            Incoming::Response { .. } => {
                log(format_args!(
                    "ignoring a response to a server-initiated request"
                ));
                Vec::new()
            }
        }
    }

    /// Handles one finished backend job.
    pub(crate) fn handle_work(&mut self, done: WorkDone) -> Vec<Outgoing> {
        // Read the backend generation once for this event and share it with
        // the result handler and the diagnostics retry batch.
        let epoch = self.refresh_epoch();
        let script = matches!(&done.kind, JobKind::ScriptDiagnostics { .. });
        if script || done.kind == JobKind::Diagnostics {
            let outgoing = if script {
                self.script_diagnostics_done(done, &epoch)
            } else {
                self.diagnostics_done(done, &epoch)
            };
            self.retry_pending_diagnostics_with_epoch(&epoch);
            return outgoing;
        }
        let Some(key) = done.key.clone() else {
            log(format_args!("dropping a request result without an id"));
            self.retry_pending_diagnostics_with_epoch(&epoch);
            return Vec::new();
        };
        let Some(inflight) = self.inflight.get(&key) else {
            // A cancelled semantic request may still have a late worker
            // result. Pending intents are removed by the cancellation itself,
            // but clear defensively so a future cancellation path cannot leak.
            self.semantic_pending.remove(&key);
            log(format_args!("dropping a result for an unknown request"));
            self.retry_pending_diagnostics_with_epoch(&epoch);
            return Vec::new();
        };
        if inflight.attempt != done.attempt {
            // The id was reused after the old request was answered. Keep the
            // new in-flight entry and drop this late result.
            log(format_args!(
                "dropping a result for a superseded request attempt"
            ));
            self.retry_pending_diagnostics_with_epoch(&epoch);
            return Vec::new();
        }
        let Some(inflight) = self.inflight.remove(&key) else {
            // The entry disappeared between the check and removal; treating it
            // as unknown is the same safe outcome as a late result.
            self.retry_pending_diagnostics_with_epoch(&epoch);
            return Vec::new();
        };
        let Inflight { id, uri, .. } = inflight;
        let outgoing = match done.kind {
            JobKind::Hover => self.hover_done(id, done, &epoch),
            JobKind::Definition => self.definition_done(id, done, &epoch),
            JobKind::Completion(_) => self.completion_done(id, done, &epoch),
            JobKind::Diagnostics | JobKind::ScriptDiagnostics { .. } => Vec::new(),
            JobKind::DocumentSymbols { .. } => self.document_symbols_done(id, &uri, done, &epoch),
            JobKind::Symbols { .. } => self.symbols_done(id, done, &epoch),
        };
        self.retry_pending_diagnostics_with_epoch(&epoch);
        outgoing
    }

    /// Retries up to [`MAX_DIAGNOSTICS_RETRIES`] pending diagnostics.
    ///
    /// Failed attempts are pushed back, so repeated completions rotate through
    /// the set instead of retrying the same first documents forever.
    #[cfg(test)]
    pub(crate) fn retry_pending_diagnostics(&mut self) {
        let epoch = self.refresh_epoch();
        self.retry_pending_diagnostics_with_epoch(&epoch);
    }

    /// Retries pending diagnostics with an epoch already observed for this
    /// event.
    pub(crate) fn retry_pending_diagnostics_with_epoch(&mut self, epoch: &BackendEpoch) {
        if self.dirty.is_empty() {
            return;
        }
        for _ in 0..MAX_DIAGNOSTICS_RETRIES {
            let Some(uri) = self.dirty.pop_front() else {
                break;
            };
            // Retries never produce outgoing messages; only the completion
            // path answers a request or publishes diagnostics.
            let _ = self.schedule_diagnostics_with_epoch(&uri, epoch);
        }
    }

    /// Returns `true` once the event loop should stop.
    pub(crate) const fn should_stop(&self) -> bool {
        self.stop
    }

    /// Returns the final outcome of the session.
    pub(crate) const fn outcome(&self) -> ServerOutcome {
        ServerOutcome {
            clean_shutdown: self.clean_shutdown,
        }
    }
}

/// Runs the server over stdin/stdout until the client exits or the transport
/// fails.
///
/// Only JSON-RPC frames are written to stdout; diagnostics and errors go to
/// stderr. The client must send `shutdown` before `exit` for
/// [`ServerOutcome::clean_shutdown`] to be `true`, which the binary maps to a
/// zero exit code.
///
/// # Errors
///
/// Returns [`ServerError::Io`] when the stdio transport cannot be read or
/// written, and [`ServerError::Worker`] when a server thread cannot be
/// started.
///
/// The reader thread performs a blocking `stdin` read; when the server stops
/// because of `exit`, that thread may stay blocked until the process exits or
/// the input is closed. This is intentional for the stdio binary; an embedder
/// that reuses the process must close its input instead of relying on the
/// thread being joined.
pub fn run_stdio(backend: Arc<dyn Backend>) -> Result<ServerOutcome, ServerError> {
    run_stdio_with_host(backend, Arc::new(NullHost))
}

/// Runs the server over stdin/stdout with a project host.
///
/// # Errors
///
/// Returns [`ServerError`] for transport or thread startup failures.
pub fn run_stdio_with_host(
    backend: Arc<dyn Backend>,
    host: Arc<dyn ProjectHost>,
) -> Result<ServerOutcome, ServerError> {
    run_with_host(io::stdin(), io::stdout(), backend, host)
}

/// Runs the server over arbitrary byte streams with a project host.
///
/// This is the embedding/test entry point; [`run_stdio`] and
/// [`run_stdio_with_host`] wrap the standard streams.
///
/// # Errors
///
/// Returns [`ServerError`] for transport or thread startup failures.
pub fn run_with_host<R, W>(
    input: R,
    output: W,
    backend: Arc<dyn Backend>,
    host: Arc<dyn ProjectHost>,
) -> Result<ServerOutcome, ServerError>
where
    R: io::Read + Send + 'static,
    W: io::Write,
{
    run_server(input, output, backend, host)
}

/// Event loop over generic byte streams.
fn run_server<R, W>(
    input: R,
    output: W,
    backend: Arc<dyn Backend>,
    host: Arc<dyn ProjectHost>,
) -> Result<ServerOutcome, ServerError>
where
    R: io::Read + Send + 'static,
    W: io::Write,
{
    let (events, receiver) = event_channel();
    let reader_events = events.clone();
    thread::Builder::new()
        .name("wxml-lsp-reader".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(input);
            loop {
                let frame = jsonrpc::read_frame(&mut reader);
                // A framing error cannot be resynchronised: report it and let
                // the protocol loop stop after answering. Only `Ok(Some)`
                // keeps the stream open.
                let stop = !matches!(frame, Ok(Some(_)));
                if reader_events.send(Message::Frame(frame)).is_err() {
                    break;
                }
                if stop {
                    break;
                }
            }
        })
        .map_err(ServerError::Worker)?;

    let worker_events = events;
    let capabilities = safe_capabilities(&backend);
    let scheduler = Arc::new(
        Scheduler::start(Arc::clone(&backend), move |done| {
            let _ = worker_events.send(Message::Work(Box::new(done)));
        })
        .map_err(ServerError::Worker)?,
    );
    let mut server = Server::new_with_host(backend, capabilities, scheduler.clone(), host);
    let mut writer = io::BufWriter::new(output);

    // Run the loop in a helper so a write error returns here and still drops
    // the receiver before the scheduler shutdown. Otherwise a worker blocked
    // on a full event channel would make shutdown wait for its 2 s deadline.
    let result = drive_loop(&receiver, &mut server, &mut writer);
    drop(receiver);
    scheduler.shutdown();
    result?;
    Ok(server.outcome())
}

/// Consumes protocol events until the client exits or a write fails.
fn drive_loop(
    receiver: &Receiver<Message>,
    server: &mut Server,
    writer: &mut impl io::Write,
) -> Result<(), ServerError> {
    loop {
        // Check expired debounce deadlines before blocking and after every
        // handled message. A client that keeps a <1 ms message stream alive
        // must not postpone diagnostics until the stream stops.
        let outgoing = server.flush_due_diagnostics(Instant::now());
        write_outgoing(writer, outgoing)?;
        if server.should_stop() {
            break;
        }
        let wait = server.next_diagnostics_deadline().map_or_else(
            || Duration::from_secs(3_600),
            |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1))
            },
        );
        let message = match receiver.recv_timeout(wait) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let outgoing = match message {
            Message::Frame(Ok(None)) => break,
            Message::Frame(Ok(Some(body))) => match jsonrpc::parse(&body) {
                Ok(incoming) => server.handle_incoming(incoming),
                Err(error) => {
                    log(format_args!("rejected message: {error}"));
                    vec![Outgoing::failure(
                        serde_json::Value::Null,
                        error.code(),
                        error.response_message(),
                    )]
                }
            },
            Message::Frame(Err(error)) => {
                log(format_args!("frame error: {error}"));
                write_outgoing(
                    writer,
                    vec![Outgoing::failure(
                        serde_json::Value::Null,
                        jsonrpc::error_codes::PARSE_ERROR,
                        "malformed transport frame",
                    )],
                )?;
                break;
            }
            Message::Work(done) => server.handle_work(*done),
        };
        write_outgoing(writer, outgoing)?;
        if server.should_stop() {
            break;
        }
    }
    Ok(())
}

/// Bounded channel used by both the reader thread and the worker.
fn event_channel() -> (SyncSender<Message>, Receiver<Message>) {
    mpsc::sync_channel(EVENT_QUEUE_CAPACITY)
}

/// Reads the optional diagnostics debounce from the trusted environment.
///
/// Invalid or oversized values fall back to [`DEFAULT_DIAGNOSTICS_DEBOUNCE`];
/// an explicit zero disables the debounce (used by focused tests/benchmarks).
fn diagnostics_debounce_from_env() -> Duration {
    let Some(value) = std::env::var_os("WXML_DIAGNOSTICS_DEBOUNCE_MS") else {
        return DEFAULT_DIAGNOSTICS_DEBOUNCE;
    };
    let Some(text) = value.to_str() else {
        return DEFAULT_DIAGNOSTICS_DEBOUNCE;
    };
    let Ok(milliseconds) = text.parse::<u64>() else {
        return DEFAULT_DIAGNOSTICS_DEBOUNCE;
    };
    Duration::from_millis(milliseconds.min(MAX_DIAGNOSTICS_DEBOUNCE_MS))
}

/// Reads the backend epoch, degrading to a fallback generation on panic.
fn safe_epoch(backend: &Arc<dyn Backend>, fallback: Option<&BackendEpoch>) -> BackendEpoch {
    catch_unwind(AssertUnwindSafe(|| backend.epoch())).unwrap_or_else(|_| {
        log(format_args!("backend panicked while reporting its epoch"));
        fallback
            .cloned()
            .unwrap_or_else(|| BackendEpoch::new(BackendId::new("unavailable-backend"), 0))
    })
}

/// Reads backend capabilities, degrading to `Unavailable` on panic.
pub(crate) fn safe_capabilities(backend: &Arc<dyn Backend>) -> BackendCapabilities {
    catch_unwind(AssertUnwindSafe(|| backend.capabilities())).unwrap_or_else(|_| {
        log(format_args!(
            "backend panicked while reporting capabilities"
        ));
        unavailable_capabilities()
    })
}

/// Every feature unavailable because the backend failed during initialization.
fn unavailable_capabilities() -> BackendCapabilities {
    let status = || CapabilityStatus::Unavailable {
        reason: "backend failed to report capabilities".to_owned(),
    };
    BackendCapabilities {
        hover: status(),
        diagnostics: status(),
        definition: status(),
        completion: status(),
    }
}

/// One event consumed by the protocol loop.
enum Message {
    /// A frame read from the client, or the error that stopped it.
    Frame(Result<Option<Vec<u8>>, FrameError>),
    /// A finished backend job.
    Work(Box<WorkDone>),
}

/// Writes all messages in order, aborting on the first I/O failure.
fn write_outgoing(writer: &mut impl io::Write, messages: Vec<Outgoing>) -> Result<(), ServerError> {
    for message in messages {
        jsonrpc::write_frame(writer, &message.into_value())?;
    }
    Ok(())
}

/// Writes a line to stderr. Never touches stdout and never panics on a closed
/// stderr, because logging must not take the protocol loop down.
pub(crate) fn log(arguments: std::fmt::Arguments<'_>) {
    use std::io::Write as _;

    let _ = writeln!(io::stderr().lock(), "[wxml-lsp] {arguments}");
}

#[cfg(test)]
#[allow(clippy::assert_is_empty)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::TrySendError;
    use std::sync::{Mutex, PoisonError};

    use serde_json::json;
    use wxml_text::{Position, SourceRange};
    use wxml_types::{
        BackendCapabilities, BackendError, BackendId, CapabilityStatus, CompletionItem,
        CompletionItemKind, CompletionKind, CompletionResult, DefinitionResult, Diagnostic,
        DiagnosticSeverity, DiagnosticsRequest, DiagnosticsResult, DocumentSymbol,
        DocumentSymbolKind, DocumentSymbolsRequest, DocumentSymbolsResult, FileId, HoverResult,
        InsertTextFormat, MarkupKind, MockBackend, ProjectId, ProjectRevision, RootKind,
        SnapshotId, SourceLocation, SourceRevision, SourceSnapshot, TypeQuality, ViewRevision,
    };

    use super::*;
    use crate::jsonrpc::ResponseOutcome;
    use crate::project_host::{
        ChangeDocument, DocumentRoute, HostError, HostUpdate, OpenDocument, ProjectHost,
        ProjectState, ViewTarget,
    };
    use crate::scheduler::{JobKind, Outcome};

    const URI: &str = "file:///pages/index/index.wxml";
    const SCRIPT_URI: &str = "file:///pages/index/index.ts";
    const SCRIPT_OUTLINE_TEXT: &str = "class 用户 {\n  name: string;\n}\n";

    #[derive(Default)]
    struct ManualDispatch {
        jobs: Mutex<Vec<Job>>,
        failures: Mutex<usize>,
        stopped: AtomicBool,
        busy: AtomicBool,
        attempts: AtomicUsize,
    }

    impl ManualDispatch {
        fn take(&self) -> Vec<Job> {
            std::mem::take(&mut *self.jobs.lock().unwrap_or_else(PoisonError::into_inner))
        }

        fn fail_next(&self, count: usize) {
            *self.failures.lock().unwrap_or_else(PoisonError::into_inner) = count;
        }

        fn stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
        }

        fn set_busy(&self, busy: bool) {
            self.busy.store(busy, Ordering::SeqCst);
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    impl Dispatch for ManualDispatch {
        fn submit(&self, job: Job) -> Result<(), SubmitError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.stopped.load(Ordering::SeqCst) {
                return Err(SubmitError::Stopped);
            }
            if self.busy.load(Ordering::SeqCst) {
                return Err(SubmitError::Busy);
            }
            let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
            if *failures > 0 {
                *failures -= 1;
                return Err(SubmitError::Busy);
            }
            drop(failures);
            self.jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(job);
            Ok(())
        }
    }

    /// Host with one template/script view and an open owning template.
    struct ScriptHost {
        project: ProjectId,
        wxml: FileId,
        script: FileId,
        wxml_path: wxml_project::ProjectPath,
        script_path: wxml_project::ProjectPath,
        snapshot: Mutex<Arc<SourceSnapshot>>,
        script_text: Mutex<Arc<str>>,
        view_revision: Mutex<ViewRevision>,
        project_revision: Mutex<ProjectRevision>,
        template_open: AtomicBool,
        shared_owner: bool,
    }

    impl ScriptHost {
        fn new(script_text: &str) -> Self {
            Self::with_template(script_text, true)
        }

        fn with_template(script_text: &str, template_open: bool) -> Self {
            let project = ProjectId::new("script-outline");
            let wxml = FileId::new(project.clone(), "pages/index/index.wxml").expect("wxml");
            let script = FileId::new(project.clone(), "pages/index/index.ts").expect("script");
            let snapshot = Arc::new(SourceSnapshot::new(
                wxml.clone(),
                SourceRevision::new(1),
                SnapshotId::new(1),
                "<view>{{ user.name }}</view>",
            ));
            let project_revision = ProjectRevision::new(project.clone(), 1);
            Self {
                project,
                wxml,
                script,
                wxml_path: wxml_project::ProjectPath::new("pages/index/index.wxml")
                    .expect("wxml path"),
                script_path: wxml_project::ProjectPath::new("pages/index/index.ts")
                    .expect("script path"),
                snapshot: Mutex::new(snapshot),
                script_text: Mutex::new(Arc::from(script_text)),
                view_revision: Mutex::new(ViewRevision::new(2)),
                project_revision: Mutex::new(project_revision),
                template_open: AtomicBool::new(template_open),
                shared_owner: false,
            }
        }

        fn shared(script_text: &str) -> Self {
            Self {
                shared_owner: true,
                ..Self::new(script_text)
            }
        }

        fn view(&self) -> ViewTarget {
            ViewTarget {
                wxml: self.wxml_path.clone(),
                owner: RootKind::Page,
                script: self.script_path.clone(),
                script_file: self.script.clone(),
                view_revision: *self
                    .view_revision
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            }
        }

        fn close_template(&self) {
            self.template_open.store(false, Ordering::SeqCst);
        }

        fn replace_template_snapshot(&self) {
            let wxml = self.wxml.clone();
            *self.snapshot.lock().unwrap_or_else(PoisonError::into_inner) =
                Arc::new(SourceSnapshot::new(
                    wxml,
                    SourceRevision::new(2),
                    SnapshotId::new(2),
                    "<view>{{ user.name }}</view>",
                ));
        }

        fn bump_view_revision(&self) {
            let mut revision = self
                .view_revision
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *revision = revision.next();
        }

        fn set_script_text(&self, text: &str) {
            *self
                .script_text
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Arc::from(text);
        }
    }

    impl ProjectHost for ScriptHost {
        fn open_root(&self, _root_uri: Option<&str>) -> Result<(), HostError> {
            Ok(())
        }

        fn project_id(&self) -> ProjectId {
            self.project.clone()
        }

        fn project_revision(&self) -> ProjectRevision {
            self.project_revision
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn path_for_uri(&self, uri: &str) -> Result<wxml_project::ProjectPath, HostError> {
            let path = uri.strip_prefix("file:///").ok_or(HostError::InvalidUri)?;
            wxml_project::ProjectPath::new(path).map_err(|_| HostError::InvalidUri)
        }

        fn uri_for(&self, file: &FileId) -> Option<String> {
            Some(format!("file:///{}", file.path()))
        }

        fn file_id(&self, path: &wxml_project::ProjectPath) -> Result<FileId, HostError> {
            FileId::new(self.project.clone(), path.as_str()).map_err(|_| HostError::InvalidUri)
        }

        fn route(&self, uri: &str) -> DocumentRoute {
            let Ok(path) = self.path_for_uri(uri) else {
                return DocumentRoute::Unknown;
            };
            match path.extension().map(str::to_ascii_lowercase).as_deref() {
                Some("wxml") => DocumentRoute::Wxml(path),
                Some("ts") => DocumentRoute::Script(path),
                Some("json") => DocumentRoute::Json(path),
                _ => DocumentRoute::Unknown,
            }
        }

        fn view_for(&self, wxml: &wxml_project::ProjectPath) -> Result<ViewTarget, HostError> {
            if *wxml == self.wxml_path {
                Ok(self.view())
            } else {
                Err(HostError::Unsupported {
                    reason: "unknown template",
                })
            }
        }

        fn view_for_script(&self, file: &FileId) -> Result<ViewTarget, HostError> {
            if self.shared_owner {
                return Err(HostError::Unsupported {
                    reason: "script is shared by several views",
                });
            }
            if *file == self.script {
                Ok(self.view())
            } else {
                Err(HostError::Unsupported {
                    reason: "script is not an owning page or component script",
                })
            }
        }

        fn open_document(&self, open: OpenDocument) -> Result<HostUpdate, HostError> {
            if self.route(&open.uri) != DocumentRoute::Json(self.path_for_uri(&open.uri)?) {
                return Ok(HostUpdate::default());
            }
            let mut revision = self
                .project_revision
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let next = revision.clone().next();
            *revision = next;
            Ok(HostUpdate {
                project_revision: Some(revision.clone()),
                project: Some(self.project.clone()),
                ..HostUpdate::default()
            })
        }

        fn change_document(&self, _change: ChangeDocument) -> Result<HostUpdate, HostError> {
            Ok(HostUpdate::default())
        }

        fn close_document(&self, _uri: &str) -> Result<HostUpdate, HostError> {
            Ok(HostUpdate::default())
        }

        fn set_wxml_snapshot(&self, _file: FileId, _snapshot: Arc<SourceSnapshot>) {}

        fn remove_wxml_snapshot(&self, _file: &FileId) {}

        fn source(&self, file: &FileId) -> Option<Arc<SourceSnapshot>> {
            (*file == self.wxml && self.template_open.load(Ordering::SeqCst))
                .then(|| Arc::clone(&self.snapshot.lock().unwrap_or_else(PoisonError::into_inner)))
        }

        fn file_text(&self, file: &FileId) -> Option<Arc<str>> {
            (*file == self.script).then(|| {
                Arc::clone(
                    &self
                        .script_text
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner),
                )
            })
        }

        fn analysis(
            &self,
            _snapshot: &SourceSnapshot,
        ) -> Result<crate::project_host::PreparedView, HostError> {
            Err(HostError::Unsupported {
                reason: "test host has no analysis",
            })
        }
    }

    /// Backend wrapper that records document-symbol calls and returns one answer.
    struct ScriptSymbolsBackend {
        inner: MockBackend,
        calls: AtomicUsize,
        answer: Mutex<Option<DocumentSymbolsResult>>,
        requests: Mutex<Vec<DocumentSymbolsRequest>>,
        fail: bool,
    }

    impl ScriptSymbolsBackend {
        fn new(answer: DocumentSymbolsResult) -> Self {
            Self {
                inner: MockBackend::new(),
                calls: AtomicUsize::new(0),
                answer: Mutex::new(Some(answer)),
                requests: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                inner: MockBackend::new(),
                calls: AtomicUsize::new(0),
                answer: Mutex::new(None),
                requests: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn requests(&self) -> Vec<DocumentSymbolsRequest> {
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Backend for ScriptSymbolsBackend {
        fn epoch(&self) -> BackendEpoch {
            self.inner.epoch()
        }

        fn hover(&self, request: wxml_types::HoverRequest) -> Result<HoverResult, BackendError> {
            self.inner.hover(request)
        }

        fn diagnostics(
            &self,
            request: DiagnosticsRequest,
        ) -> Result<DiagnosticsResult, BackendError> {
            self.inner.diagnostics(request)
        }

        fn document_symbols(
            &self,
            request: DocumentSymbolsRequest,
        ) -> Result<DocumentSymbolsResult, BackendError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            if self.fail {
                return Err(BackendError::Unavailable(
                    "document symbols failed".to_owned(),
                ));
            }
            self.answer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or_else(|| BackendError::Internal("missing answer".to_owned()))
        }

        fn capabilities(&self) -> BackendCapabilities {
            self.inner.capabilities()
        }
    }

    /// Backend wrapper that records standalone script diagnostics requests.
    struct ScriptDiagnosticsBackend {
        inner: MockBackend,
        requests: Mutex<Vec<DiagnosticsRequest>>,
        answer: Mutex<Option<DiagnosticsResult>>,
    }

    impl ScriptDiagnosticsBackend {
        fn new(answer: DiagnosticsResult) -> Self {
            Self {
                inner: MockBackend::new(),
                requests: Mutex::new(Vec::new()),
                answer: Mutex::new(Some(answer)),
            }
        }

        fn requests(&self) -> Vec<DiagnosticsRequest> {
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Backend for ScriptDiagnosticsBackend {
        fn epoch(&self) -> BackendEpoch {
            self.inner.epoch()
        }

        fn hover(&self, request: wxml_types::HoverRequest) -> Result<HoverResult, BackendError> {
            self.inner.hover(request)
        }

        fn diagnostics(
            &self,
            request: DiagnosticsRequest,
        ) -> Result<DiagnosticsResult, BackendError> {
            self.inner.diagnostics(request)
        }

        fn script_diagnostics(
            &self,
            request: DiagnosticsRequest,
        ) -> Result<DiagnosticsResult, BackendError> {
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            self.answer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or_else(|| BackendError::Internal("missing script answer".to_owned()))
        }

        fn capabilities(&self) -> BackendCapabilities {
            self.inner.capabilities()
        }
    }

    fn script_outline(truncated: bool, skipped: usize) -> DocumentSymbolsResult {
        let text = SCRIPT_OUTLINE_TEXT;
        let class_start = text.find("class").expect("class");
        let class_end = text.rfind('}').expect("class end") + 1;
        let name_start = text.find("name:").expect("name");
        DocumentSymbolsResult {
            symbols: vec![DocumentSymbol {
                name: "用户".to_owned(),
                kind: DocumentSymbolKind::Class,
                range: SourceRange::from_bounds(
                    u32::try_from(class_start).expect("start"),
                    u32::try_from(class_end).expect("end"),
                )
                .expect("range"),
                selection_range: SourceRange::from_bounds(
                    u32::try_from(text.find("用户").expect("name")).expect("name start"),
                    u32::try_from(text.find("用户").expect("name") + "用户".len())
                        .expect("name end"),
                )
                .expect("selection"),
                children: vec![DocumentSymbol {
                    name: "name".to_owned(),
                    kind: DocumentSymbolKind::Property,
                    range: SourceRange::from_bounds(
                        u32::try_from(name_start).expect("prop start"),
                        u32::try_from(name_start + "name: string;".len()).expect("prop end"),
                    )
                    .expect("prop range"),
                    selection_range: SourceRange::from_bounds(
                        u32::try_from(name_start).expect("prop name start"),
                        u32::try_from(name_start + "name".len()).expect("prop name end"),
                    )
                    .expect("prop selection"),
                    children: Vec::new(),
                }],
            }],
            status: if truncated || skipped > 0 {
                CapabilityStatus::Partial {
                    reason: "the document symbol evidence is incomplete".to_owned(),
                }
            } else {
                CapabilityStatus::Supported
            },
            truncated,
            skipped,
            gaps: Vec::new(),
        }
    }

    fn initialize_with_hierarchy(server: &mut Server, hierarchical: bool) {
        let outgoing = server.handle_incoming(request(
            json!(1),
            "initialize",
            json!({
                "capabilities": {
                    "textDocument": {
                        "documentSymbol": {
                            "hierarchicalDocumentSymbolSupport": hierarchical,
                        }
                    }
                }
            }),
        ));
        assert_eq!(outgoing.len(), 1);
    }

    fn open_script_document(server: &mut Server, text: &str) {
        let outgoing = server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": SCRIPT_URI,
                    "languageId": "typescript",
                    "version": 1,
                    "text": text,
                }
            }),
        ));
        assert!(outgoing.is_empty());
    }

    fn document_symbol_request(id: u64, uri: &str) -> Incoming {
        request(
            json!(id),
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    fn take_script_diagnostics_job(dispatch: &ManualDispatch) -> Job {
        let mut found = None;
        for job in dispatch.take() {
            if matches!(job.kind, JobKind::ScriptDiagnostics { .. }) {
                assert!(found.is_none(), "one script diagnostics job per event");
                found = Some(job);
            }
        }
        found.expect("a script diagnostics job")
    }

    fn finish_script_diagnostics_job(
        server: &mut Server,
        job: &Job,
        result: Result<DiagnosticsResult, BackendError>,
    ) -> Vec<Outgoing> {
        server.handle_work(WorkDone {
            key: job.key.clone(),
            kind: job.kind.clone(),
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: false,
            outcome: result.map(|result| Outcome::Diagnostics(Box::new(result))),
        })
    }

    fn finish_cancelled_script_diagnostics_job(server: &mut Server, job: &Job) -> Vec<Outgoing> {
        server.handle_work(WorkDone {
            key: job.key.clone(),
            kind: job.kind.clone(),
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: true,
            outcome: Err(BackendError::Cancelled),
        })
    }

    fn run_script_diagnostics(backend: &ScriptDiagnosticsBackend, job: &Job) -> DiagnosticsResult {
        backend
            .script_diagnostics(DiagnosticsRequest {
                context: job.context.clone(),
            })
            .expect("script diagnostics answer")
    }

    fn script_diagnostics_server(
        backend: Arc<ScriptDiagnosticsBackend>,
        host: Arc<ScriptHost>,
        text: &str,
    ) -> (Server, Arc<ManualDispatch>) {
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend,
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        server.diagnostics_debounce = Duration::ZERO;
        initialize(&mut server);
        open_script_document(&mut server, text);
        (server, dispatch)
    }

    fn publish_params(outgoing: &Outgoing) -> &serde_json::Value {
        let Outgoing::Notification { method, params } = outgoing else {
            panic!("expected a diagnostics notification");
        };
        assert_eq!(method, "textDocument/publishDiagnostics");
        params
    }

    fn take_document_symbol_job(dispatch: &ManualDispatch) -> Job {
        let mut found = None;
        for job in dispatch.take() {
            if matches!(job.kind, JobKind::DocumentSymbols { .. }) {
                assert!(found.is_none(), "one document symbol job per request");
                found = Some(job);
            }
        }
        found.expect("a document symbol job")
    }

    fn finish_document_symbol_job(
        server: &mut Server,
        job: &Job,
        result: Result<DocumentSymbolsResult, BackendError>,
    ) -> Vec<Outgoing> {
        finish_document_symbol_job_outcome(
            server,
            job,
            result.map(|result| Outcome::DocumentSymbols(Box::new(result))),
        )
    }

    fn finish_document_symbol_job_outcome(
        server: &mut Server,
        job: &Job,
        outcome: Result<Outcome, BackendError>,
    ) -> Vec<Outgoing> {
        server.handle_work(WorkDone {
            key: job.key.clone(),
            kind: job.kind.clone(),
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: false,
            outcome,
        })
    }

    fn finish_cancelled_document_symbol_job(server: &mut Server, job: &Job) -> Vec<Outgoing> {
        server.handle_work(WorkDone {
            key: job.key.clone(),
            kind: job.kind.clone(),
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: true,
            outcome: Err(BackendError::Cancelled),
        })
    }

    fn epoch() -> BackendEpoch {
        BackendEpoch::new(BackendId::new("mock"), 1)
    }

    fn setup() -> (Server, Arc<ManualDispatch>) {
        setup_with(MockBackend::new().capabilities())
    }

    fn setup_with(capabilities: BackendCapabilities) -> (Server, Arc<ManualDispatch>) {
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new(backend, capabilities, dispatch.clone());
        // Focused protocol tests drive scheduling directly; debounce behavior
        // has its own tests below.
        server.diagnostics_debounce = Duration::ZERO;
        (server, dispatch)
    }

    fn setup_with_backend(
        backend: Arc<MockBackend>,
        capabilities: BackendCapabilities,
    ) -> (Server, Arc<ManualDispatch>) {
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new(backend, capabilities, dispatch.clone());
        server.diagnostics_debounce = Duration::ZERO;
        (server, dispatch)
    }

    fn initialize(server: &mut Server) {
        let outgoing = server.handle_incoming(request(json!(1), "initialize", json!({})));
        assert_eq!(outgoing.len(), 1);
    }

    fn open_document(server: &mut Server, text: &str, version: u64) {
        let outgoing = server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "wxml",
                    "version": version,
                    "text": text,
                }
            }),
        ));
        assert!(outgoing.is_empty());
    }

    fn request(id: serde_json::Value, method: &str, params: serde_json::Value) -> Incoming {
        Incoming::Request {
            id,
            method: method.to_owned(),
            params: Some(params),
        }
    }

    fn notification(method: &str, params: serde_json::Value) -> Incoming {
        Incoming::Notification {
            method: method.to_owned(),
            params: Some(params),
        }
    }

    fn hover_params(position: (u32, u32)) -> serde_json::Value {
        json!({
            "textDocument": {"uri": URI},
            "position": {"line": position.0, "character": position.1},
        })
    }

    fn only(outgoing: Vec<Outgoing>) -> Outgoing {
        assert_eq!(outgoing.len(), 1, "expected exactly one message");
        outgoing.into_iter().next().expect("one message")
    }

    fn result(outgoing: &Outgoing) -> &serde_json::Value {
        match outgoing {
            Outgoing::Response {
                outcome: ResponseOutcome::Success(result),
                ..
            } => result,
            other => panic!("expected a success response, got {other:?}"),
        }
    }

    fn error_code(outgoing: &Outgoing) -> i64 {
        match outgoing {
            Outgoing::Response {
                outcome: ResponseOutcome::Failure(error),
                ..
            } => error.code,
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    fn hover_result() -> HoverResult {
        HoverResult {
            contents: "string".to_owned(),
            markup: MarkupKind::Markdown,
            range: Some(SourceRange::from_bounds(1, 2).expect("range")),
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Known,
        }
    }

    fn completed_hover(job: &Job, result: HoverResult) -> WorkDone {
        WorkDone {
            key: job.key.clone(),
            kind: JobKind::Hover,
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Hover(Box::new(result))),
        }
    }

    fn completed_diagnostics(job: &Job, result: DiagnosticsResult) -> WorkDone {
        WorkDone {
            key: job.key.clone(),
            kind: JobKind::Diagnostics,
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Diagnostics(Box::new(result))),
        }
    }

    #[test]
    fn completion_results_map_items_and_stale_ones_are_dropped() {
        let capabilities = BackendCapabilities {
            completion: CapabilityStatus::Supported,
            ..MockBackend::new().capabilities()
        };
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "<view>{{ user.na }}</view>", 1);
        dispatch.take();

        server.handle_incoming(request(
            json!("c1"),
            "textDocument/completion",
            hover_params((0, 16)),
        ));
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        let completion = CompletionResult {
            items: vec![CompletionItem {
                label: "name".to_owned(),
                kind: CompletionItemKind::Property,
                detail: Some("string".to_owned()),
                documentation: Some("The user name.".to_owned()),
                insert_text: "name".to_owned(),
                insert_text_format: InsertTextFormat::PlainText,
                replace: SourceRange::from_bounds(14, 16).expect("range"),
                sort_text: Some("m_name".to_owned()),
                filter_text: None,
            }],
            status: CapabilityStatus::Supported,
            truncated: false,
        };
        let outgoing = only(server.handle_work(WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Completion(CompletionKind::ExpressionMember),
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Ok(Outcome::Completion(Box::new(completion.clone()))),
        }));
        let response = result(&outgoing);
        assert_eq!(response["isIncomplete"], json!(false));
        assert_eq!(response["items"][0]["label"], json!("name"));
        assert_eq!(
            response["items"][0]["textEdit"]["range"]["start"],
            json!({"line": 0, "character": 14})
        );
        assert_eq!(
            response["items"][0]["textEdit"]["range"]["end"],
            json!({"line": 0, "character": 16})
        );
        assert_eq!(response["items"][0]["sortText"], json!("m_name"));

        server.handle_incoming(request(
            json!("c2"),
            "textDocument/completion",
            hover_params((0, 16)),
        ));
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        let mut stale = jobs[0].stamp.clone();
        stale.epoch = epoch().next();
        let outgoing = only(server.handle_work(WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Completion(CompletionKind::ExpressionMember),
            stamp: stale,
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Ok(Outcome::Completion(Box::new(completion))),
        }));
        assert_eq!(result(&outgoing)["items"], json!([]));
    }

    #[test]
    fn initialize_reports_backend_capabilities() {
        let (mut server, _) = setup();
        let outgoing = only(server.handle_incoming(request(json!(1), "initialize", json!({}))));
        let Outgoing::Response { id, .. } = &outgoing else {
            panic!("expected response");
        };
        assert_eq!(id, &json!(1));
        let result = result(&outgoing);
        assert_eq!(
            result["capabilities"]["textDocumentSync"]["openClose"],
            json!(true)
        );
        assert_eq!(
            result["capabilities"]["textDocumentSync"]["change"],
            json!(2)
        );
        assert_eq!(
            result["capabilities"]["textDocumentSync"]["save"]["includeText"],
            json!(false)
        );
        assert_eq!(result["capabilities"]["positionEncoding"], json!("utf-16"));
        assert_eq!(result["capabilities"]["hoverProvider"], json!(true));
        assert_eq!(result["capabilities"]["definitionProvider"], json!(false));
        assert_eq!(result["serverInfo"]["name"], json!("wxml-lsp"));
        assert_eq!(result["serverInfo"]["version"], json!(crate::version()));
    }

    #[test]
    fn requests_before_initialize_are_rejected() {
        let (mut server, _) = setup();
        let outgoing = only(server.handle_incoming(request(
            json!(2),
            "textDocument/hover",
            hover_params((0, 0)),
        )));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::SERVER_NOT_INITIALIZED
        );
        assert!(
            server
                .handle_incoming(notification("initialized", json!({})))
                .is_empty()
        );
    }

    #[test]
    fn shutdown_before_initialize_is_invalid_request() {
        let (mut server, _) = setup();
        let outgoing = only(server.handle_incoming(request(json!(33), "shutdown", json!(null))));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_REQUEST);
        assert!(!server.should_stop());
    }

    #[test]
    fn initialize_twice_is_rejected() {
        let (mut server, _) = setup();
        initialize(&mut server);
        let outgoing = only(server.handle_incoming(request(json!(2), "initialize", json!({}))));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_REQUEST);
    }

    #[test]
    fn hover_schedules_a_job_with_the_byte_offset() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        let outgoing = server.handle_incoming(request(
            json!(3),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        assert!(outgoing.is_empty(), "hover response is deferred");

        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Hover);
        assert_eq!(jobs[0].offset, 1);
        assert_eq!(
            jobs[0].key,
            Some(RequestKey::from_id(&json!(3)).expect("key"))
        );

        let outgoing = only(server.handle_work(completed_hover(&jobs[0], hover_result())));
        let result = result(&outgoing);
        assert_eq!(result["contents"]["kind"], json!("markdown"));
        assert_eq!(result["contents"]["value"], json!("string"));
        assert_eq!(result["range"]["start"], json!({"line": 0, "character": 1}));
        assert_eq!(result["range"]["end"], json!({"line": 0, "character": 2}));
    }

    #[test]
    fn hover_converts_utf16_positions_to_byte_offsets() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        // 中 (1 UTF-16 unit, 3 bytes), 😀 (2 units, 4 bytes), a, CRLF, b.
        open_document(&mut server, "中😀a\r\nb", 1);
        dispatch.take();

        for (index, (position, expected)) in [((0, 0), 0), ((0, 3), 7), ((1, 0), 10), ((1, 1), 11)]
            .into_iter()
            .enumerate()
        {
            let outgoing = server.handle_incoming(request(
                json!(4 + index),
                "textDocument/hover",
                hover_params(position),
            ));
            assert!(outgoing.is_empty(), "position {position:?}");
            let jobs = dispatch.take();
            assert_eq!(jobs.len(), 1, "position {position:?}");
            assert_eq!(jobs[0].offset, expected, "position {position:?}");
        }

        // Inside the surrogate pair: no backend call, null result.
        let outgoing = only(server.handle_incoming(request(
            json!(5),
            "textDocument/hover",
            hover_params((0, 2)),
        )));
        assert_eq!(result(&outgoing), &json!(null));
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn hover_without_an_open_document_returns_null() {
        let (mut server, _) = setup();
        initialize(&mut server);
        let outgoing = only(server.handle_incoming(request(
            json!(6),
            "textDocument/hover",
            hover_params((0, 0)),
        )));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn stale_hover_results_are_dropped() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(7),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);

        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"text": "abcd"}],
            }),
        ));

        let outgoing = only(server.handle_work(completed_hover(&jobs[0], hover_result())));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn results_from_an_old_backend_epoch_are_dropped() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let diagnostics_job = dispatch.take().remove(0);
        server.handle_incoming(request(
            json!(32),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let hover_job = dispatch.take().remove(0);

        let mut stale_hover = completed_hover(&hover_job, hover_result());
        stale_hover.stamp.epoch = epoch().next();
        assert_eq!(result(&only(server.handle_work(stale_hover))), &json!(null));

        let mut stale_diagnostics = completed_diagnostics(
            &diagnostics_job,
            DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            },
        );
        stale_diagnostics.stamp.epoch = epoch().next();
        assert!(server.handle_work(stale_diagnostics).is_empty());
    }

    #[test]
    fn cancelled_hover_reports_request_cancelled() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(8),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Hover,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: true,
            outcome: Err(BackendError::Cancelled),
        };
        let outgoing = only(server.handle_work(done));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );
    }

    #[test]
    fn cancel_notification_sets_the_job_flag() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!("hover-1"),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        let outgoing =
            only(server.handle_incoming(notification("$/cancelRequest", json!({"id": "hover-1"}))));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );
        assert!(jobs[0].cancel.load(Ordering::SeqCst));
        assert!(server.inflight.is_empty());
        // The late worker result is dropped, not answered twice.
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Hover,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: true,
            outcome: Err(BackendError::Cancelled),
        };
        assert!(server.handle_work(done).is_empty());
        assert!(
            server
                .handle_incoming(notification("$/cancelRequest", json!({"id": "unknown"})))
                .is_empty()
        );
    }

    #[test]
    fn a_late_cancelled_attempt_does_not_touch_a_reused_request_id() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        server.handle_incoming(request(
            json!(7),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let first = dispatch.take().remove(0);
        assert_eq!(first.attempt, 1);
        let cancelled =
            only(server.handle_incoming(notification("$/cancelRequest", json!({"id": 7}))));
        assert_eq!(
            error_code(&cancelled),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );

        server.handle_incoming(request(
            json!(7),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let second = dispatch.take().remove(0);
        assert_eq!(second.attempt, 2);
        assert!(
            server
                .inflight
                .contains_key(first.key.as_ref().expect("key"))
        );

        // The old synchronous call returns after the client reused the id.
        assert!(
            server
                .handle_work(completed_hover(&first, hover_result()))
                .is_empty(),
            "the superseded attempt must not answer or remove the new inflight"
        );
        assert!(
            server
                .inflight
                .contains_key(first.key.as_ref().expect("key"))
        );
        let fresh = only(server.handle_work(completed_hover(&second, hover_result())));
        assert_eq!(result(&fresh)["contents"]["value"], json!("string"));
    }

    #[test]
    fn hover_backend_errors_return_null() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(9),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Hover,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Err(BackendError::Timeout),
        };
        let outgoing = only(server.handle_work(done));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn unsupported_hover_returns_null_instead_of_an_error() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.hover = CapabilityStatus::Unsupported {
            reason: "no hover".to_owned(),
        };
        let (mut server, _) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let outgoing = only(server.handle_incoming(request(
            json!(10),
            "textDocument/hover",
            hover_params((0, 1)),
        )));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn unsupported_definition_returns_null_instead_of_an_error() {
        let (mut server, _) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let outgoing = only(server.handle_incoming(request(
            json!(11),
            "textDocument/definition",
            hover_params((0, 1)),
        )));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn static_tag_hover_works_when_backend_hover_is_unsupported() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.hover = CapabilityStatus::Unsupported {
            reason: "no hover".to_owned(),
        };
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "<view />", 1);
        dispatch.take();
        let outgoing = only(server.handle_incoming(request(
            json!(43),
            "textDocument/hover",
            hover_params((0, 2)),
        )));
        let result = result(&outgoing);
        assert_eq!(result["contents"]["kind"], json!("markdown"));
        assert!(
            result["contents"]["value"]
                .as_str()
                .is_some_and(|value| value.contains("视图容器")),
            "{result}"
        );
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn static_component_definition_works_when_backend_definition_is_unsupported() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/component-intelligence");
        let uri = format!("file://{}/pages/index/index.wxml", root.display());
        let host = Arc::new(ProjectState::new());
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            Arc::new(MockBackend::new()),
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        let outgoing = only(server.handle_incoming(request(
            json!(1),
            "initialize",
            json!({"rootUri": format!("file://{}", root.display())}),
        )));
        assert_eq!(
            result(&outgoing)["capabilities"]["definitionProvider"].as_bool(),
            Some(true)
        );
        server.handle_incoming(notification("initialized", json!({})));
        server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": "<outer-card />",
                },
            }),
        ));
        dispatch.take();
        let outgoing = only(server.handle_incoming(request(
            json!(2),
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 3},
            }),
        )));
        let locations = result(&outgoing).as_array().expect("locations");
        assert_eq!(locations.len(), 1, "{outgoing:?}");
        assert!(
            locations[0]["uri"]
                .as_str()
                .is_some_and(|uri| uri.ends_with("components/outer-card/outer-card.wxml")),
            "{outgoing:?}"
        );
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn supported_definition_maps_locations() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.definition = CapabilityStatus::Supported;
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        let outgoing = server.handle_incoming(request(
            json!(12),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        assert!(outgoing.is_empty());
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Definition);
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Definition,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
                locations: vec![SourceLocation {
                    file: FileId::new(ProjectId::new("lsp"), "documents/1.wxml").expect("file"),
                    range: SourceRange::from_bounds(0, 1).expect("range"),
                }],
                status: CapabilityStatus::Supported,
            }))),
        };
        let outgoing = only(server.handle_work(done));
        let result = result(&outgoing);
        let locations = result.as_array().expect("array");
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0]["uri"], json!(URI));
        assert_eq!(
            locations[0]["range"]["start"],
            json!({"line": 0, "character": 0})
        );
    }

    #[test]
    fn unmappable_definition_targets_return_null() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.definition = CapabilityStatus::Supported;
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(19),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Definition,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
                locations: vec![SourceLocation {
                    file: FileId::new(ProjectId::new("lsp"), "documents/2.wxml").expect("file"),
                    range: SourceRange::from_bounds(0, 1).expect("range"),
                }],
                status: CapabilityStatus::Supported,
            }))),
        };
        let outgoing = only(server.handle_work(done));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn unsupported_definition_results_are_null() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.definition = CapabilityStatus::Supported;
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(18),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Definition,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
                locations: Vec::new(),
                status: CapabilityStatus::Unsupported {
                    reason: "no symbols".to_owned(),
                },
            }))),
        };
        let outgoing = only(server.handle_work(done));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn did_open_schedules_diagnostics_and_publishes_versioned_results() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Diagnostics);
        assert!(jobs[0].key.is_none());
        let outgoing = only(server.handle_work(completed_diagnostics(
            &jobs[0],
            DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            },
        )));
        let Outgoing::Notification { method, params } = outgoing else {
            panic!("expected notification");
        };
        assert_eq!(method, "textDocument/publishDiagnostics");
        assert_eq!(params["uri"], json!(URI));
        assert_eq!(params["version"], json!(1));
        assert_eq!(params["diagnostics"], json!([]));
    }

    #[test]
    fn degraded_diagnostics_publish_a_status_warning() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        let outgoing = only(server.handle_work(completed_diagnostics(
            &jobs[0],
            DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Partial {
                    reason: "wxs is not analysed".to_owned(),
                },
            },
        )));
        let Outgoing::Notification { params, .. } = outgoing else {
            panic!("expected notification");
        };
        let diagnostics = params["diagnostics"].as_array().expect("array");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["severity"], json!(2));
        let message = diagnostics[0]["message"].as_str().expect("message");
        assert!(message.contains("partial"), "{message}");
        assert!(message.contains("wxs is not analysed"), "{message}");
    }

    #[test]
    fn diagnostic_ranges_use_utf16_positions() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "中😀a", 1);
        let jobs = dispatch.take();
        let outgoing = only(server.handle_work(completed_diagnostics(
            &jobs[0],
            DiagnosticsResult {
                diagnostics: vec![Diagnostic {
                    range: SourceRange::from_bounds(3, 7).expect("range"),
                    severity: DiagnosticSeverity::Error,
                    message: "bad".to_owned(),
                    code: None,
                }],
                status: CapabilityStatus::Supported,
            },
        )));
        let Outgoing::Notification { params, .. } = outgoing else {
            panic!("expected notification");
        };
        let diagnostic = &params["diagnostics"][0];
        assert_eq!(
            diagnostic["range"]["start"],
            json!({"line": 0, "character": 1})
        );
        assert_eq!(
            diagnostic["range"]["end"],
            json!({"line": 0, "character": 3})
        );
        assert_eq!(diagnostic["severity"], json!(1));
        assert_eq!(diagnostic["message"], json!("bad"));
    }

    #[test]
    fn diagnostic_messages_are_sanitized() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        let dirty = format!("bad\u{1b}[31m\n{}", "x".repeat(600));
        let outgoing = only(server.handle_work(completed_diagnostics(
            &jobs[0],
            DiagnosticsResult {
                diagnostics: vec![Diagnostic {
                    range: SourceRange::from_bounds(0, 1).expect("range"),
                    severity: DiagnosticSeverity::Error,
                    message: dirty,
                    code: None,
                }],
                status: CapabilityStatus::Supported,
            },
        )));
        let Outgoing::Notification { params, .. } = outgoing else {
            panic!("expected notification");
        };
        let message = params["diagnostics"][0]["message"]
            .as_str()
            .expect("message");
        assert!(!message.contains('\u{1b}'), "{message}");
        assert!(!message.contains('\n'), "{message}");
        assert!(message.starts_with("bad [31m x"), "{message}");
        assert_eq!(message.chars().count(), 513);
    }

    #[test]
    fn stale_diagnostics_are_dropped() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"text": "abcd"}],
            }),
        ));
        assert!(
            server
                .handle_work(completed_diagnostics(
                    &jobs[0],
                    DiagnosticsResult {
                        diagnostics: Vec::new(),
                        status: CapabilityStatus::Supported,
                    },
                ))
                .is_empty()
        );
    }

    #[test]
    fn diagnostics_backend_errors_publish_a_warning() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        let done = WorkDone {
            key: None,
            kind: JobKind::Diagnostics,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Err(BackendError::Unavailable("no node".to_owned())),
        };
        let outgoing = only(server.handle_work(done));
        let Outgoing::Notification { params, .. } = outgoing else {
            panic!("expected notification");
        };
        let diagnostics = params["diagnostics"].as_array().expect("array");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["severity"], json!(2));
        let message = diagnostics[0]["message"].as_str().expect("message");
        assert!(message.contains("unavailable"), "{message}");
        assert!(!message.contains("no node"), "leaked detail: {message}");
    }

    #[test]
    fn rapid_changes_cancel_superseded_diagnostics() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let opened = dispatch.take();
        assert_eq!(opened.len(), 1);

        for (version, text) in [(2, "ab"), (3, "abc"), (4, "abcd")] {
            server.handle_incoming(notification(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": URI, "version": version},
                    "contentChanges": [{"text": text}],
                }),
            ));
        }

        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 3, "one diagnostics job per change is queued");
        assert!(
            jobs[0].cancel.load(Ordering::SeqCst),
            "the first superseded change is cancelled"
        );
        assert!(
            jobs[1].cancel.load(Ordering::SeqCst),
            "the second superseded change is cancelled"
        );
        assert!(
            !jobs[2].cancel.load(Ordering::SeqCst),
            "the newest change stays live"
        );
        assert!(
            server.workspace.is_current(&jobs[2].stamp),
            "the newest diagnostics job matches the document revision"
        );
    }

    #[test]
    fn typed_changes_are_debounced_and_coalesced() {
        let (mut server, dispatch) = setup();
        server.diagnostics_debounce = Duration::from_millis(120);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let opened = dispatch.take();
        assert_eq!(opened.len(), 1);

        for (version, text) in [(2, "ab"), (3, "abc"), (4, "abcd")] {
            assert!(
                server
                    .handle_incoming(notification(
                        "textDocument/didChange",
                        json!({
                            "textDocument": {"uri": URI, "version": version},
                            "contentChanges": [{"text": text}],
                        }),
                    ))
                    .is_empty()
            );
        }
        assert!(
            dispatch.take().is_empty(),
            "typing must not queue one analysis per character"
        );
        assert_eq!(server.debounced_diagnostics.len(), 1);
        assert!(opened[0].cancel.load(Ordering::SeqCst));

        server.flush_all_debounced_diagnostics();
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1, "one coalesced diagnostics job");
        assert_eq!(jobs[0].kind, JobKind::Diagnostics);
        assert!(
            server.workspace.is_current(&jobs[0].stamp),
            "the debounced job uses the newest revision"
        );
    }

    #[test]
    fn did_save_bypasses_the_diagnostics_debounce() {
        let (mut server, dispatch) = setup();
        server.diagnostics_debounce = Duration::from_millis(120);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"text": "abcd"}],
            }),
        ));
        assert!(dispatch.take().is_empty());
        server.handle_incoming(notification(
            "textDocument/didSave",
            json!({"textDocument": {"uri": URI}}),
        ));
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Diagnostics);
        assert!(server.debounced_diagnostics.is_empty());
    }

    #[test]
    fn did_close_cancels_a_pending_debounce() {
        let (mut server, dispatch) = setup();
        server.diagnostics_debounce = Duration::from_millis(120);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"text": "abcd"}],
            }),
        ));
        server.handle_incoming(notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": URI}}),
        ));
        assert!(server.debounced_diagnostics.is_empty());
        assert!(dispatch.take().is_empty());
        server.flush_all_debounced_diagnostics();
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn hover_after_an_incremental_change_uses_the_new_revision() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "{{ title }}", 1);
        dispatch.take();

        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{
                    "range": {
                        "start": {"line": 0, "character": 3},
                        "end": {"line": 0, "character": 8},
                    },
                    "text": "user",
                }],
            }),
        ));
        let diagnostics = dispatch.take();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, JobKind::Diagnostics);

        server.handle_incoming(request(
            json!(7),
            "textDocument/hover",
            hover_params((0, 5)),
        ));
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Hover);
        assert_eq!(jobs[0].offset, 5);
        assert!(
            server.workspace.is_current(&jobs[0].stamp),
            "hover sees the document text after the incremental edit"
        );
        let outgoing = only(server.handle_work(completed_hover(&jobs[0], hover_result())));
        assert_eq!(result(&outgoing)["contents"]["value"], json!("string"));
    }

    #[test]
    fn busy_diagnostics_are_retried_after_work() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let first = dispatch.take();
        assert_eq!(first.len(), 1);

        dispatch.fail_next(1);
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"text": "abcd"}],
            }),
        ));
        assert!(dispatch.take().is_empty(), "queue was reported busy");
        assert!(server.dirty.iter().any(|dirty| dirty == URI));

        // Finishing the obsolete job frees a slot and retries the newest one.
        let _ = server.handle_work(completed_diagnostics(
            &first[0],
            DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            },
        ));
        let retried = dispatch.take();
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].kind, JobKind::Diagnostics);
        assert!(server.dirty.is_empty());
    }

    #[test]
    fn did_close_clears_diagnostics_and_cancels_requests() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(13),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        assert!(server.workspace.get(URI).is_some());

        let outgoing = only(server.handle_incoming(notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": URI}}),
        )));
        let Outgoing::Notification { method, params } = outgoing else {
            panic!("expected notification");
        };
        assert_eq!(method, "textDocument/publishDiagnostics");
        assert_eq!(params["uri"], json!(URI));
        assert_eq!(params["diagnostics"], json!([]));
        assert!(params.get("version").is_none());
        assert!(jobs[0].cancel.load(Ordering::SeqCst));
        assert!(server.workspace.get(URI).is_none());
    }

    #[test]
    fn did_change_rejects_version_regressions() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        let outgoing = server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 1},
                "contentChanges": [{"text": "nope"}],
            }),
        ));
        let Outgoing::Notification { method, params } = only(outgoing) else {
            panic!("expected a client log message");
        };
        assert_eq!(method, "window/logMessage");
        assert_eq!(params["type"], json!(1));
        assert!(
            params["message"]
                .as_str()
                .expect("message")
                .contains("out of sync")
        );
        assert!(dispatch.take().is_empty());
        assert_eq!(server.workspace.get(URI).expect("document").text(), "abc");
    }

    #[test]
    fn did_change_applies_incremental_edits() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{
                    "range": {
                        "start": {"line": 0, "character": 1},
                        "end": {"line": 0, "character": 2},
                    },
                    "text": "X",
                }],
            }),
        ));
        assert_eq!(server.workspace.get(URI).expect("document").text(), "aXc");
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Diagnostics);
    }

    #[test]
    fn shutdown_blocks_new_requests_and_exit_stops_the_server() {
        let (mut server, _) = setup();
        initialize(&mut server);
        let outgoing = only(server.handle_incoming(request(json!(14), "shutdown", json!(null))));
        assert_eq!(result(&outgoing), &json!(null));
        let outgoing = only(server.handle_incoming(request(
            json!(15),
            "textDocument/hover",
            hover_params((0, 0)),
        )));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_REQUEST);
        assert!(
            server
                .handle_incoming(notification("exit", json!(null)))
                .is_empty()
        );
        assert!(server.should_stop());
        assert!(server.outcome().clean_shutdown);
    }

    #[test]
    fn exit_without_shutdown_is_not_clean() {
        let (mut server, _) = setup();
        initialize(&mut server);
        assert!(
            server
                .handle_incoming(notification("exit", json!(null)))
                .is_empty()
        );
        assert!(server.should_stop());
        assert!(!server.outcome().clean_shutdown);
    }

    #[test]
    fn unknown_methods_and_client_responses_are_handled() {
        let (mut server, _) = setup();
        initialize(&mut server);
        let outgoing = only(server.handle_incoming(request(
            json!(16),
            "textDocument/documentColor",
            json!({}),
        )));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::METHOD_NOT_FOUND
        );
        assert!(
            server
                .handle_incoming(notification("$/setTrace", json!({})))
                .is_empty()
        );
        assert!(
            server
                .handle_incoming(Incoming::Response { id: json!(1) })
                .is_empty()
        );
    }

    #[test]
    fn case_server_0150_json_rpc_notifications_never_produce_a_response() {
        // Oracle: `server.test.ts:150`. Notifications may change state and may
        // schedule diagnostics, but they never answer; the first reply belongs
        // to the request that follows them.
        let (mut server, _) = setup();
        initialize(&mut server);

        assert!(
            server
                .handle_incoming(notification("nope", json!({})))
                .is_empty(),
            "an unknown notification is ignored"
        );
        assert!(
            server
                .handle_incoming(notification("textDocument/didOpen", json!({})))
                .is_empty(),
            "a malformed notification is ignored"
        );
        assert!(
            server
                .handle_incoming(notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": URI,
                            "languageId": "wxml",
                            "version": 1,
                            "text": "<view>{{ title }}</view>",
                        }
                    }),
                ))
                .is_empty(),
            "a valid notification only schedules work"
        );

        let outgoing = only(server.handle_incoming(request(json!(7), "shutdown", json!(null))));
        match &outgoing {
            Outgoing::Response { id, .. } => {
                assert_eq!(id, &json!(7), "the first reply must belong to the request");
            }
            other => panic!("expected the shutdown response, got {other:?}"),
        }
        assert!(
            server
                .handle_incoming(notification("exit", json!(null)))
                .is_empty()
        );
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        let (mut server, _) = setup();
        initialize(&mut server);
        let outgoing = only(server.handle_incoming(request(
            json!(17),
            "textDocument/hover",
            json!({"textDocument": {}, "position": "nope"}),
        )));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_PARAMS);
    }

    #[test]
    fn cancel_flags_use_the_shared_atomic() {
        // Compile-time witness that `Inflight` stays cheap to clone.
        let flag = Arc::new(AtomicBool::new(false));
        let inflight = Inflight {
            id: json!(1),
            attempt: 1,
            cancel: Arc::clone(&flag),
            uri: URI.to_owned(),
        };
        inflight.cancel.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn malformed_document_notifications_do_not_change_state() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        let malformed = [
            notification("textDocument/didOpen", json!(null)),
            notification("textDocument/didOpen", json!({})),
            notification(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": URI}}),
            ),
            notification(
                "textDocument/didOpen",
                json!({"textDocument": {"text": "x"}}),
            ),
            notification("textDocument/didChange", json!({})),
            notification("textDocument/didClose", json!({})),
            Incoming::Notification {
                method: "textDocument/didOpen".to_owned(),
                params: None,
            },
            Incoming::Notification {
                method: "textDocument/didChange".to_owned(),
                params: None,
            },
            Incoming::Notification {
                method: "textDocument/didClose".to_owned(),
                params: None,
            },
        ];
        for incoming in malformed {
            assert!(server.handle_incoming(incoming).is_empty());
        }
        assert!(dispatch.take().is_empty());
        assert!(server.workspace.get(URI).is_none());

        for rejected in [
            json!({"textDocument": {"uri": URI}}),
            json!({"textDocument": {"uri": URI, "version": 2}}),
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"range": {}}],
            }),
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"text": "orphan"}],
            }),
        ] {
            let outgoing = server.handle_incoming(notification("textDocument/didChange", rejected));
            let Outgoing::Notification { method, params } = only(outgoing) else {
                panic!("expected a client log message");
            };
            assert_eq!(method, "window/logMessage");
            assert_eq!(params["type"], json!(1));
        }
    }

    #[test]
    fn oversized_document_uris_are_rejected() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        let uri = "u".repeat(crate::workspace::MAX_URI_BYTES + 1);
        let outgoing = server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": "x",
                },
            }),
        ));
        let Outgoing::Notification { method, params } = only(outgoing) else {
            panic!("expected a client log message");
        };
        assert_eq!(method, "window/logMessage");
        assert_eq!(params["type"], json!(1));
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn scheduling_diagnostics_for_unknown_documents_is_a_noop() {
        let (mut server, dispatch) = setup();
        assert!(server.schedule_diagnostics(URI).is_empty());
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn initialized_emits_no_registration_without_a_project() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        assert!(
            server
                .handle_incoming(notification("initialized", json!({})))
                .is_empty()
        );
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn malformed_save_and_watched_file_notifications_are_ignored() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        let malformed = [
            notification("textDocument/didSave", json!(null)),
            notification("textDocument/didSave", json!({})),
            notification("textDocument/didSave", json!({"textDocument": {}})),
            notification("textDocument/didSave", json!({"textDocument": {"uri": 1}})),
            notification("workspace/didChangeWatchedFiles", json!(null)),
            notification("workspace/didChangeWatchedFiles", json!({})),
            notification("workspace/didChangeWatchedFiles", json!({"changes": {}})),
            notification(
                "workspace/didChangeWatchedFiles",
                json!({"changes": [null, {"uri": 1}, {"uri": URI}, {"type": 2}]}),
            ),
            Incoming::Notification {
                method: "textDocument/didSave".to_owned(),
                params: None,
            },
            Incoming::Notification {
                method: "workspace/didChangeWatchedFiles".to_owned(),
                params: None,
            },
        ];
        for incoming in malformed {
            assert!(server.handle_incoming(incoming).is_empty());
        }
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn did_save_for_a_synthetic_wxml_document_schedules_diagnostics() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "{{ user.name }}", 1);
        dispatch.take();

        let outgoing = server.handle_incoming(notification(
            "textDocument/didSave",
            json!({"textDocument": {"uri": URI}}),
        ));
        assert!(outgoing.is_empty());
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Diagnostics);
        assert!(jobs[0].key.is_none());
    }

    #[test]
    fn did_save_with_text_refreshes_and_reschedules() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "{{ user.name }}", 1);
        dispatch.take();

        let outgoing = server.handle_incoming(notification(
            "textDocument/didSave",
            json!({"textDocument": {"uri": URI}, "text": "{{ user.age }}"}),
        ));
        assert!(outgoing.is_empty());
        let jobs = dispatch.take();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, JobKind::Diagnostics);
    }

    #[test]
    fn untitled_wxml_documents_schedule_but_other_untitled_documents_do_not() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": "untitled:Untitled-1",
                "languageId": "wxml",
                "version": 1,
                "text": "<view>{{ user.name }}</view>",
            }}),
        ));
        assert_eq!(dispatch.take().len(), 1);

        server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": "untitled:Untitled-2",
                "languageId": "plaintext",
                "version": 1,
                "text": "not a template",
            }}),
        ));
        assert!(dispatch.take().is_empty());

        let outgoing = only(server.handle_incoming(notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": "untitled:Untitled-1"}}),
        )));
        let Outgoing::Notification { method, params } = outgoing else {
            panic!("expected notification");
        };
        assert_eq!(method, "textDocument/publishDiagnostics");
        assert_eq!(params["uri"], json!("untitled:Untitled-1"));
        assert_eq!(params["diagnostics"], json!([]));
    }

    #[test]
    fn point_requests_for_non_wxml_documents_return_null() {
        let capabilities = BackendCapabilities {
            definition: CapabilityStatus::Supported,
            ..MockBackend::new().capabilities()
        };
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        let uri = "file:///scripts/a.ts";
        server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "typescript", "version": 1, "text": "const x = 1;"}}),
        ));
        dispatch.take();

        for method in ["textDocument/hover", "textDocument/definition"] {
            let outgoing = only(server.handle_incoming(request(
                json!(8),
                method,
                json!({
                    "textDocument": {"uri": uri},
                    "position": {"line": 0, "character": 6},
                }),
            )));
            assert_eq!(result(&outgoing), &json!(null), "{method}");
        }
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn cancelled_diagnostics_are_not_published() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        let done = WorkDone {
            key: None,
            kind: JobKind::Diagnostics,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: true,
            outcome: Err(BackendError::Cancelled),
        };
        assert!(server.handle_work(done).is_empty());
    }

    #[test]
    fn diagnostics_wrong_kind_and_timeout_publish_warnings() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let job = dispatch.take().remove(0);

        let wrong_kind = WorkDone {
            key: None,
            kind: JobKind::Diagnostics,
            stamp: job.stamp.clone(),
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Hover(Box::new(hover_result()))),
        };
        let Outgoing::Notification { params, .. } = only(server.handle_work(wrong_kind)) else {
            panic!("expected a diagnostics notification");
        };
        assert_eq!(params["diagnostics"].as_array().expect("array").len(), 1);

        let timed_out = WorkDone {
            key: None,
            kind: JobKind::Diagnostics,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Err(BackendError::Timeout),
        };
        let Outgoing::Notification { params, .. } = only(server.handle_work(timed_out)) else {
            panic!("expected a diagnostics notification");
        };
        assert_eq!(params["diagnostics"].as_array().expect("array").len(), 1);
    }

    #[test]
    fn duplicate_request_ids_are_rejected() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        assert!(
            server
                .handle_incoming(request(
                    json!(20),
                    "textDocument/hover",
                    hover_params((0, 1)),
                ))
                .is_empty()
        );
        let outgoing = only(server.handle_incoming(request(
            json!(20),
            "textDocument/hover",
            hover_params((0, 1)),
        )));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_REQUEST);
        assert_eq!(dispatch.take().len(), 1);
    }

    #[test]
    fn invalid_request_ids_are_rejected() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        let incoming = Incoming::Request {
            id: json!(true),
            method: "textDocument/hover".to_owned(),
            params: Some(hover_params((0, 0))),
        };
        let outgoing = only(server.handle_incoming(incoming));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_PARAMS);
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn hover_parameter_variants_are_rejected() {
        let (mut server, _) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let invalid = [
            None,
            Some(json!(null)),
            Some(json!({})),
            Some(json!({"textDocument": {"uri": URI}})),
            Some(json!({"textDocument": {"uri": 7}, "position": {"line": 0, "character": 0}})),
            Some(json!({"textDocument": {"uri": URI}, "position": {"line": 0}})),
        ];
        for params in invalid {
            let incoming = Incoming::Request {
                id: json!(21),
                method: "textDocument/hover".to_owned(),
                params,
            };
            let outgoing = only(server.handle_incoming(incoming));
            assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_PARAMS);
        }
    }

    #[test]
    fn dispatch_failures_return_internal_errors() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        dispatch.fail_next(1);
        let busy = only(server.handle_incoming(request(
            json!(22),
            "textDocument/hover",
            hover_params((0, 1)),
        )));
        assert_eq!(error_code(&busy), jsonrpc::error_codes::INTERNAL_ERROR);

        dispatch.stop();
        let stopped = only(server.handle_incoming(request(
            json!(23),
            "textDocument/hover",
            hover_params((0, 1)),
        )));
        assert_eq!(error_code(&stopped), jsonrpc::error_codes::INTERNAL_ERROR);
    }

    #[test]
    fn hover_results_with_the_wrong_kind_are_null() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(24),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let jobs = dispatch.take();
        let done = WorkDone {
            key: jobs[0].key.clone(),
            kind: JobKind::Hover,
            stamp: jobs[0].stamp.clone(),
            attempt: jobs[0].attempt,
            cancelled: false,
            outcome: Ok(Outcome::Diagnostics(Box::new(DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            }))),
        };
        let outgoing = only(server.handle_work(done));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn hover_respects_plaintext_markup() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(25),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let done = completed_hover(
            &job,
            HoverResult {
                contents: "plain".to_owned(),
                markup: MarkupKind::PlainText,
                range: None,
                status: CapabilityStatus::Supported,
                quality: TypeQuality::Known,
            },
        );
        let outgoing = only(server.handle_work(done));
        let result = result(&outgoing);
        assert_eq!(result["contents"]["kind"], json!("plaintext"));
        assert_eq!(result["contents"]["value"], json!("plain"));
    }

    #[test]
    fn definition_done_handles_cancellation_and_backend_errors() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.definition = CapabilityStatus::Supported;
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        server.handle_incoming(request(
            json!(26),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let cancelled = WorkDone {
            key: job.key.clone(),
            kind: JobKind::Definition,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: true,
            outcome: Err(BackendError::Cancelled),
        };
        assert_eq!(
            error_code(&only(server.handle_work(cancelled))),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );

        server.handle_incoming(request(
            json!(27),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let failed = WorkDone {
            key: job.key.clone(),
            kind: JobKind::Definition,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Err(BackendError::Timeout),
        };
        assert_eq!(result(&only(server.handle_work(failed))), &json!(null));

        server.handle_incoming(request(
            json!(28),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let wrong_kind = WorkDone {
            key: job.key.clone(),
            kind: JobKind::Definition,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Hover(Box::new(hover_result()))),
        };
        assert_eq!(result(&only(server.handle_work(wrong_kind))), &json!(null));
    }

    #[test]
    fn definition_done_validates_ranges_and_empty_results() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.definition = CapabilityStatus::Supported;
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        server.handle_incoming(request(
            json!(29),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let invalid = WorkDone {
            key: job.key.clone(),
            kind: JobKind::Definition,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
                locations: vec![SourceLocation {
                    file: FileId::new(ProjectId::new("lsp"), "documents/1.wxml").expect("file"),
                    range: SourceRange::from_bounds(0, 99).expect("range"),
                }],
                status: CapabilityStatus::Supported,
            }))),
        };
        assert_eq!(result(&only(server.handle_work(invalid))), &json!(null));

        server.handle_incoming(request(
            json!(30),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let empty = WorkDone {
            key: job.key.clone(),
            kind: JobKind::Definition,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
                locations: Vec::new(),
                status: CapabilityStatus::Supported,
            }))),
        };
        assert_eq!(result(&only(server.handle_work(empty))), &json!([]));
    }

    #[test]
    fn notifications_after_shutdown_are_ignored() {
        let (mut server, _) = setup();
        initialize(&mut server);
        server.handle_incoming(request(json!(31), "shutdown", json!(null)));
        assert!(
            server
                .handle_incoming(notification(
                    "textDocument/didOpen",
                    json!({"textDocument": {"uri": URI, "text": "abc"}}),
                ))
                .is_empty()
        );
        assert!(server.workspace.get(URI).is_none());
    }

    #[test]
    fn malformed_cancel_requests_are_ignored() {
        let (mut server, _) = setup();
        initialize(&mut server);
        for params in [None, Some(json!({})), Some(json!({"id": true}))] {
            let incoming = Incoming::Notification {
                method: "$/cancelRequest".to_owned(),
                params,
            };
            assert!(server.handle_incoming(incoming).is_empty());
        }
    }

    #[test]
    fn the_event_channel_is_bounded() {
        let (sender, _receiver) = event_channel();
        for _ in 0..EVENT_QUEUE_CAPACITY {
            sender
                .try_send(Message::Frame(Ok(Some(Vec::new()))))
                .expect("within capacity");
        }
        assert!(matches!(
            sender.try_send(Message::Frame(Ok(Some(Vec::new())))),
            Err(TrySendError::Full(_))
        ));
    }

    #[test]
    fn epoch_changes_are_seen_before_scheduling_and_publishing() {
        let backend = Arc::new(MockBackend::new());
        let (mut server, dispatch) =
            setup_with_backend(Arc::clone(&backend), MockBackend::new().capabilities());
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();

        server.handle_incoming(request(
            json!(40),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let first = dispatch.take().remove(0);
        assert_eq!(first.stamp.epoch, epoch());

        let restarted = epoch().next();
        backend.set_epoch(restarted.clone());
        assert_eq!(
            result(&only(
                server.handle_work(completed_hover(&first, hover_result()))
            )),
            &json!(null)
        );

        server.handle_incoming(request(
            json!(41),
            "textDocument/hover",
            hover_params((0, 1)),
        ));
        let second = dispatch.take().remove(0);
        assert_eq!(second.stamp.epoch, restarted);
    }

    #[test]
    fn failed_diagnostics_from_an_old_epoch_are_published_without_a_loop() {
        let backend = Arc::new(MockBackend::new());
        let (mut server, dispatch) =
            setup_with_backend(Arc::clone(&backend), MockBackend::new().capabilities());
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let job = dispatch.take().remove(0);

        // Simulate the backend bumping its epoch after a transport failure.
        backend.set_epoch(epoch().next());
        let done = WorkDone {
            key: None,
            kind: JobKind::Diagnostics,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Diagnostics(Box::new(DiagnosticsResult {
                diagnostics: vec![Diagnostic {
                    range: SourceRange::from_bounds(0, 1).expect("range"),
                    severity: DiagnosticSeverity::Error,
                    message: "syntax".to_owned(),
                    code: None,
                }],
                status: CapabilityStatus::Unavailable {
                    reason: "type backend unavailable".to_owned(),
                },
            }))),
        };
        let outgoing = only(server.handle_work(done));
        let Outgoing::Notification { params, .. } = outgoing else {
            panic!("expected diagnostics");
        };
        let diagnostics = params["diagnostics"].as_array().expect("diagnostics");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic["message"] == json!("syntax")),
            "{params}"
        );
        assert!(
            dispatch.take().is_empty(),
            "a failed fallback must not reschedule"
        );
    }

    #[test]
    fn old_epoch_diagnostics_are_rescheduled() {
        let backend = Arc::new(MockBackend::new());
        let (mut server, dispatch) =
            setup_with_backend(Arc::clone(&backend), MockBackend::new().capabilities());
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let first = dispatch.take().remove(0);
        assert_eq!(first.stamp.epoch, epoch());

        let restarted = epoch().next();
        backend.set_epoch(restarted.clone());
        let done = completed_diagnostics(
            &first,
            DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            },
        );
        assert!(server.handle_work(done).is_empty());

        let rescheduled = dispatch.take();
        assert_eq!(rescheduled.len(), 1);
        assert_eq!(rescheduled[0].kind, JobKind::Diagnostics);
        assert_eq!(rescheduled[0].stamp.epoch, restarted);
    }

    #[test]
    fn definition_results_from_an_old_epoch_are_dropped() {
        let mut capabilities = MockBackend::new().capabilities();
        capabilities.definition = CapabilityStatus::Supported;
        let (mut server, dispatch) = setup_with(capabilities);
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(request(
            json!(44),
            "textDocument/definition",
            hover_params((0, 1)),
        ));
        let job = dispatch.take().remove(0);
        let mut done = WorkDone {
            key: job.key.clone(),
            kind: JobKind::Definition,
            stamp: job.stamp,
            attempt: job.attempt,
            cancelled: false,
            outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
                locations: vec![SourceLocation {
                    file: FileId::new(ProjectId::new("lsp"), "documents/1.wxml").expect("file"),
                    range: SourceRange::from_bounds(0, 1).expect("range"),
                }],
                status: CapabilityStatus::Supported,
            }))),
        };
        done.stamp.epoch = epoch().next();
        assert_eq!(result(&only(server.handle_work(done))), &json!(null));
    }

    #[test]
    fn diagnostics_retries_are_bounded_and_rotate() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        dispatch.set_busy(true);
        for index in 0..10 {
            let uri = format!("file:///doc{index}.wxml");
            server.handle_incoming(notification(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "wxml",
                        "version": 1,
                        "text": "x",
                    },
                }),
            ));
        }
        assert_eq!(server.dirty.len(), 10);
        let before = dispatch.attempts();
        server.retry_pending_diagnostics();
        assert_eq!(dispatch.attempts() - before, MAX_DIAGNOSTICS_RETRIES);
        server.retry_pending_diagnostics();
        assert_eq!(dispatch.attempts() - before, MAX_DIAGNOSTICS_RETRIES * 2);
        assert_eq!(server.dirty.len(), 10);
    }

    #[test]
    fn did_close_rejects_oversized_uris() {
        let (mut server, _) = setup();
        initialize(&mut server);
        let uri = "u".repeat(crate::workspace::MAX_URI_BYTES + 1);
        let outgoing = server.handle_incoming(notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        ));
        assert!(outgoing.is_empty());
    }

    #[test]
    fn null_change_ranges_apply_full_text() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        dispatch.take();
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": [{"range": null, "text": "new"}],
            }),
        ));
        assert_eq!(server.workspace.get(URI).expect("document").text(), "new");
        assert_eq!(dispatch.take().len(), 1);
    }

    #[test]
    fn panicking_backend_initialization_degrades() {
        struct PanicBackend;

        impl Backend for PanicBackend {
            fn epoch(&self) -> BackendEpoch {
                panic!("epoch panic");
            }

            fn hover(
                &self,
                _request: wxml_types::HoverRequest,
            ) -> Result<HoverResult, BackendError> {
                Err(BackendError::Unavailable("panicked".to_owned()))
            }

            fn diagnostics(
                &self,
                _request: DiagnosticsRequest,
            ) -> Result<DiagnosticsResult, BackendError> {
                Ok(DiagnosticsResult {
                    diagnostics: Vec::new(),
                    status: CapabilityStatus::Unavailable {
                        reason: "panicked".to_owned(),
                    },
                })
            }

            fn capabilities(&self) -> BackendCapabilities {
                panic!("capabilities panic");
            }
        }

        let backend: Arc<dyn Backend> = Arc::new(PanicBackend);
        let capabilities = safe_capabilities(&backend);
        for status in [
            &capabilities.hover,
            &capabilities.diagnostics,
            &capabilities.definition,
            &capabilities.completion,
        ] {
            assert!(!status.is_supported());
            assert_eq!(
                status.reason(),
                Some("backend failed to report capabilities")
            );
        }
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new(backend, capabilities, dispatch);
        initialize(&mut server);
        let outgoing = only(server.handle_incoming(request(
            json!(42),
            "textDocument/hover",
            hover_params((0, 0)),
        )));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn edit_work_budget_rejects_large_changes_atomically() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        let text = "a".repeat(2 * 1024 * 1024);
        server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                },
            }),
        ));
        dispatch.take();

        let too_many: Vec<serde_json::Value> = (0..1_024).map(|_| json!({"text": ""})).collect();
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": too_many,
            }),
        ));
        assert!(dispatch.take().is_empty());
        assert_eq!(
            server.workspace.get(URI).expect("document").text().len(),
            text.len()
        );

        let expensive: Vec<serde_json::Value> = (0..256)
            .map(|_| {
                json!({
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 0},
                    },
                    "text": "x",
                })
            })
            .collect();
        let outgoing = server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": expensive,
            }),
        ));
        let Outgoing::Notification { method, params } = only(outgoing) else {
            panic!("expected a client log message");
        };
        assert_eq!(method, "window/logMessage");
        assert_eq!(params["type"], json!(1));
        assert!(
            params["message"]
                .as_str()
                .expect("message")
                .contains("resynchronize")
        );
        assert!(dispatch.take().is_empty());
        assert_eq!(
            server.workspace.get(URI).expect("document").text().len(),
            text.len()
        );

        let noops: Vec<serde_json::Value> = (0..200)
            .map(|_| {
                json!({
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 0},
                    },
                    "text": "",
                })
            })
            .collect();
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": 2},
                "contentChanges": noops,
            }),
        ));
        assert_eq!(dispatch.take().len(), 1);

        server.handle_incoming(request(
            json!(43),
            "textDocument/hover",
            hover_params((0, 0)),
        ));
        assert_eq!(dispatch.take().len(), 1);
        server.handle_incoming(notification("exit", json!(null)));
        assert!(server.should_stop());
    }

    #[test]
    fn positions_round_trip_through_the_document_snapshot() {
        let (mut server, _) = setup();
        initialize(&mut server);
        open_document(&mut server, "a😀b", 1);
        let document = server.workspace.get(URI).expect("document");
        assert_eq!(document.offset_at(Position::new(0, 1)), Some(1));
        assert_eq!(document.offset_at(Position::new(0, 3)), Some(5));
        assert_eq!(document.offset_at(Position::new(0, 2)), None);
    }

    fn script_outline_file() -> FileId {
        FileId::new(ProjectId::new("script-outline"), "pages/index/index.ts").expect("script file")
    }

    fn template_outline_file() -> FileId {
        FileId::new(ProjectId::new("script-outline"), "pages/index/index.wxml")
            .expect("template file")
    }

    fn outline_server(
        backend: Arc<ScriptSymbolsBackend>,
        host: Arc<ScriptHost>,
        hierarchical: bool,
        text: &str,
    ) -> (Server, Arc<ManualDispatch>) {
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend,
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        initialize_with_hierarchy(&mut server, hierarchical);
        open_script_document(&mut server, text);
        dispatch.take();
        (server, dispatch)
    }

    fn run_outline(
        backend: &ScriptSymbolsBackend,
        job: &Job,
    ) -> Result<DocumentSymbolsResult, BackendError> {
        let JobKind::DocumentSymbols { subject, .. } = &job.kind else {
            panic!("expected a document symbol job");
        };
        backend.document_symbols(DocumentSymbolsRequest {
            context: job.context.clone(),
            subject: subject.clone(),
        })
    }

    #[test]
    fn script_document_symbol_request_runs_on_the_worker_and_publishes() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, true, SCRIPT_OUTLINE_TEXT);

        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty(),
            "the protocol loop must not answer before the worker finishes"
        );
        let job = take_document_symbol_job(&dispatch);
        let JobKind::DocumentSymbols { subject, scope } = &job.kind else {
            panic!("expected a document symbol job");
        };
        assert_eq!(subject, &script_outline_file());
        assert_eq!(scope.template, template_outline_file());
        assert_eq!(scope.template_snapshot, SnapshotId::new(1));
        assert_eq!(scope.view_revision, ViewRevision::new(2));
        assert_ne!(scope.script_hash, 0);

        let answer = run_outline(&backend, &job).expect("backend answer");
        let requests = backend.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.subject, *subject);
        assert_eq!(
            request.context.project_revision(),
            ProjectRevision::new(ProjectId::new("script-outline"), 1)
        );
        assert_eq!(request.context.view_revision(), ViewRevision::new(2));
        assert_eq!(request.context.backend_epoch(), epoch());
        assert_eq!(request.context.source().file(), &template_outline_file());
        assert_eq!(request.context.source().snapshot(), SnapshotId::new(1));
        assert_eq!(
            request.context.source().text(),
            "<view>{{ user.name }}</view>"
        );

        let outgoing = only(finish_document_symbol_job(&mut server, &job, Ok(answer)));
        let values = result(&outgoing).as_array().expect("outline array");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["name"], json!("用户"));
        assert_eq!(values[0]["kind"], json!(5));
        assert_eq!(
            values[0]["range"]["start"],
            json!({"line": 0, "character": 0})
        );
        assert_eq!(
            values[0]["selectionRange"]["start"],
            json!({"line": 0, "character": 6})
        );
        assert_eq!(
            values[0]["selectionRange"]["end"],
            json!({"line": 0, "character": 8})
        );
        let children = values[0]["children"].as_array().expect("children");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["name"], json!("name"));
        assert_eq!(children[0]["kind"], json!(7));
        assert_eq!(backend.calls(), 1);
    }

    #[test]
    fn script_document_symbols_flatten_without_hierarchical_support() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(true, 2)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, false, SCRIPT_OUTLINE_TEXT);

        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let answer = run_outline(&backend, &job).expect("backend answer");
        let outgoing = only(finish_document_symbol_job(&mut server, &job, Ok(answer)));
        let values = result(&outgoing).as_array().expect("outline array");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["name"], json!("用户"));
        assert_eq!(values[0]["location"]["uri"], json!(SCRIPT_URI));
        assert_eq!(
            values[0]["location"]["range"]["start"],
            json!({"line": 0, "character": 0})
        );
        assert!(values[0].get("selectionRange").is_none());
        assert_eq!(values[1]["name"], json!("name"));
        assert_eq!(values[1]["containerName"], json!("用户"));
        assert!(values[1].get("selectionRange").is_none());
        assert_eq!(backend.calls(), 1);
    }

    #[test]
    fn script_document_symbols_checked_empty_is_the_only_empty_success() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let mut empty = script_outline(false, 0);
        empty.symbols.clear();
        empty.status = CapabilityStatus::Supported;
        let outgoing = only(finish_document_symbol_job(&mut server, &job, Ok(empty)));
        assert_eq!(result(&outgoing), &json!([]));
    }

    #[test]
    fn script_document_symbols_unsupported_with_symbols_are_a_request_failure() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let mut unsupported = script_outline(false, 0);
        unsupported.status = CapabilityStatus::Unsupported {
            reason: "document symbols are not implemented".to_owned(),
        };
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(unsupported),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::REQUEST_FAILED,
            "an unsupported status must not publish its symbols"
        );
    }

    #[test]
    fn script_document_symbols_partial_empty_is_a_request_failure() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let mut partial = script_outline(false, 0);
        partial.symbols.clear();
        partial.status = CapabilityStatus::Partial {
            reason: "the document symbol evidence is incomplete".to_owned(),
        };
        let outgoing = only(finish_document_symbol_job(&mut server, &job, Ok(partial)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
    }

    #[test]
    fn script_document_symbols_partial_with_symbols_publish_them() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(true, 2)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(true, 2)),
        ));
        let values = result(&outgoing).as_array().expect("outline array");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["name"], json!("用户"));
    }

    #[test]
    fn script_document_symbols_backend_errors_are_request_failures() {
        let backend = Arc::new(ScriptSymbolsBackend::failing());
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let error = run_outline(&backend, &job).expect_err("backend failure");
        let outgoing = only(finish_document_symbol_job(&mut server, &job, Err(error)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
        assert_eq!(backend.calls(), 1);

        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(3, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Err(BackendError::Timeout),
        ));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
    }

    #[test]
    fn script_document_symbols_wrong_result_kind_is_an_internal_error() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let outgoing = only(finish_document_symbol_job_outcome(
            &mut server,
            &job,
            Ok(Outcome::Hover(Box::new(hover_result()))),
        ));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INTERNAL_ERROR);
    }

    #[test]
    fn script_document_symbols_cancelled_backend_error_is_content_modified() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Err(BackendError::Cancelled),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );
    }

    #[test]
    fn script_document_symbols_serialization_failure_is_a_request_failure() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let mut invalid = script_outline(false, 0);
        invalid.symbols[0].range = SourceRange::from_bounds(
            0,
            u32::try_from(SCRIPT_OUTLINE_TEXT.len() + 8).expect("range end"),
        )
        .expect("range");
        let outgoing = only(finish_document_symbol_job(&mut server, &job, Ok(invalid)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
    }

    #[test]
    fn script_document_symbols_stale_epoch_and_revision_are_content_modified() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let mut job = take_document_symbol_job(&dispatch);
        job.stamp.epoch = BackendEpoch::new(BackendId::new("mock"), 2);
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );

        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(3, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": SCRIPT_URI, "version": 2},
                "contentChanges": [{"text": "class Changed {}\n"}],
            }),
        ));
        dispatch.take();
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );
    }

    #[test]
    fn script_document_symbols_stale_scope_is_content_modified() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend, host.clone(), true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        host.close_template();
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );

        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend, host.clone(), true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(3, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        host.bump_view_revision();
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );

        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend, host.clone(), true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(4, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        host.replace_template_snapshot();
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );
    }

    #[test]
    fn script_document_symbols_host_text_change_is_content_modified() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend, host.clone(), true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        host.set_script_text("class Changed {}\n");
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &job,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );
    }

    #[test]
    fn script_document_symbols_reject_mismatched_script_text_before_scheduling() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, true, "class Other {}\n");
        let outgoing = only(server.handle_incoming(document_symbol_request(2, SCRIPT_URI)));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::CONTENT_MODIFIED
        );
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_document_symbols_for_an_unopened_script_fail() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend.clone(),
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        initialize_with_hierarchy(&mut server, true);
        let outgoing = only(server.handle_incoming(document_symbol_request(2, SCRIPT_URI)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_document_symbols_require_the_owning_template_to_be_open() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::with_template(SCRIPT_OUTLINE_TEXT, false));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, true, SCRIPT_OUTLINE_TEXT);
        let outgoing = only(server.handle_incoming(document_symbol_request(2, SCRIPT_URI)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_document_symbols_for_a_non_owning_script_fail() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, true, SCRIPT_OUTLINE_TEXT);
        let other_uri = "file:///pages/index/other.ts";
        let outgoing = server.handle_incoming(notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": other_uri,
                    "languageId": "typescript",
                    "version": 1,
                    "text": "const other = 1;\n",
                }
            }),
        ));
        assert!(outgoing.is_empty());
        dispatch.take();
        let outgoing = only(server.handle_incoming(document_symbol_request(2, other_uri)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_document_symbols_for_a_shared_owner_fail() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::shared(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) =
            outline_server(backend.clone(), host, true, SCRIPT_OUTLINE_TEXT);
        let outgoing = only(server.handle_incoming(document_symbol_request(2, SCRIPT_URI)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_document_symbols_outside_a_project_are_empty() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new(
            backend.clone(),
            MockBackend::new().capabilities(),
            dispatch.clone(),
        );
        initialize_with_hierarchy(&mut server, true);
        open_script_document(&mut server, SCRIPT_OUTLINE_TEXT);
        dispatch.take();
        let outgoing = only(server.handle_incoming(document_symbol_request(2, SCRIPT_URI)));
        assert_eq!(
            result(&outgoing),
            &json!([]),
            "a document with no project route is not a script outline"
        );
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_document_symbols_reject_a_duplicate_inflight_id() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let outgoing = only(server.handle_incoming(document_symbol_request(2, SCRIPT_URI)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::INVALID_REQUEST);
        assert_eq!(dispatch.take().len(), 1);
    }

    #[test]
    fn script_document_symbols_queued_cancellation_answers_once() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let outgoing =
            only(server.handle_incoming(notification("$/cancelRequest", json!({"id": 2}))));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );
        assert!(
            finish_document_symbol_job(&mut server, &job, Ok(script_outline(false, 0))).is_empty(),
            "a late result for a cancelled request must be dropped"
        );
    }

    #[test]
    fn script_document_symbols_running_cancellation_is_request_cancelled() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        let outgoing = only(finish_cancelled_document_symbol_job(&mut server, &job));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );
    }

    #[test]
    fn script_document_symbols_keep_inflight_state_across_did_close() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let job = take_document_symbol_job(&dispatch);
        server.handle_incoming(notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": SCRIPT_URI}}),
        ));
        let outgoing = only(finish_cancelled_document_symbol_job(&mut server, &job));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::REQUEST_CANCELLED
        );
    }

    #[test]
    fn script_document_symbols_reused_id_drops_the_old_attempt() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let (mut server, dispatch) = outline_server(backend, host, true, SCRIPT_OUTLINE_TEXT);
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let old = take_document_symbol_job(&dispatch);
        let _ = only(server.handle_incoming(notification("$/cancelRequest", json!({"id": 2}))));
        assert!(
            server
                .handle_incoming(document_symbol_request(2, SCRIPT_URI))
                .is_empty()
        );
        let new = take_document_symbol_job(&dispatch);
        assert_ne!(old.attempt, new.attempt);
        assert!(
            finish_document_symbol_job(&mut server, &old, Ok(script_outline(false, 0))).is_empty(),
            "the old attempt must not answer the new request"
        );
        let outgoing = only(finish_document_symbol_job(
            &mut server,
            &new,
            Ok(script_outline(false, 0)),
        ));
        assert_eq!(result(&outgoing).as_array().expect("outline").len(), 1);
    }

    #[test]
    fn wxml_document_symbols_stay_static_and_never_call_the_backend() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend.clone(),
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        initialize_with_hierarchy(&mut server, true);
        open_document(&mut server, "<view id=\"a\"><text>x</text></view>", 1);
        dispatch.take();

        let outgoing = only(server.handle_incoming(document_symbol_request(2, URI)));
        let values = result(&outgoing).as_array().expect("outline array");
        assert_eq!(values[0]["name"], json!("view"));
        assert_eq!(values[0]["kind"], json!(5));
        assert_eq!(backend.calls(), 0);
        assert!(dispatch.take().is_empty());
    }

    #[test]
    fn wxml_document_symbols_for_an_unopened_template_fail() {
        let backend = Arc::new(ScriptSymbolsBackend::new(script_outline(false, 0)));
        let host = Arc::new(ScriptHost::new(SCRIPT_OUTLINE_TEXT));
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend.clone(),
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        initialize_with_hierarchy(&mut server, true);
        let outgoing = only(server.handle_incoming(document_symbol_request(2, URI)));
        assert_eq!(error_code(&outgoing), jsonrpc::error_codes::REQUEST_FAILED);
        assert!(dispatch.take().is_empty());
        assert_eq!(backend.calls(), 0);
    }

    #[test]
    fn script_diagnostics_publish_exact_uri_version_and_utf16_ranges() {
        const TEXT: &str = "// 😀 中文\nconst 名字: string = 1;\n";
        let start = u32::try_from(TEXT.find("名字: string").expect("name token")).expect("start");
        let end = start + u32::try_from("名字: string".len()).expect("length");
        let answer = DiagnosticsResult {
            diagnostics: vec![Diagnostic {
                range: SourceRange::from_bounds(start, end).expect("range"),
                severity: DiagnosticSeverity::Error,
                message: "Type 'number' is not assignable to type 'string'.".to_owned(),
                code: Some("2322".to_owned()),
            }],
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new(TEXT));
        let (mut server, dispatch) = script_diagnostics_server(backend.clone(), host, TEXT);
        let job = take_script_diagnostics_job(&dispatch);
        assert_eq!(job.context.file().path(), "pages/index/index.ts");
        assert_eq!(job.context.source().text(), TEXT);
        assert_eq!(job.context.source().revision(), job.stamp.revision);
        assert!(matches!(
            job.kind,
            JobKind::ScriptDiagnostics { script_hash } if script_hash != 0
        ));

        let answer = run_script_diagnostics(&backend, &job);
        let requests = backend.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].context.file().path(), "pages/index/index.ts");
        assert_eq!(requests[0].context.source().text(), TEXT);
        assert_eq!(requests[0].context.source().revision(), job.stamp.revision);

        let outgoing = only(finish_script_diagnostics_job(&mut server, &job, Ok(answer)));
        let params = publish_params(&outgoing);
        assert_eq!(params["uri"], json!(SCRIPT_URI));
        assert_eq!(params["version"], json!(1));
        let diagnostic = &params["diagnostics"][0];
        assert_eq!(
            diagnostic["range"]["start"],
            json!({"line": 1, "character": 6})
        );
        assert_eq!(
            diagnostic["range"]["end"],
            json!({"line": 1, "character": 16})
        );
        assert_eq!(diagnostic["severity"], json!(1));
        assert_eq!(diagnostic["code"], json!("2322"));
    }

    #[test]
    fn stale_script_diagnostics_are_dropped_and_the_latest_publishes() {
        let answer = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x: string = 1;\n"));
        let (mut server, dispatch) =
            script_diagnostics_server(backend.clone(), host, "const x: string = 1;\n");
        let old = take_script_diagnostics_job(&dispatch);

        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": SCRIPT_URI, "version": 2},
                "contentChanges": [{"text": "const y: string = 2;\n"}],
            }),
        ));
        let latest = take_script_diagnostics_job(&dispatch);
        assert!(
            old.cancel.load(Ordering::SeqCst),
            "a newer edit supersedes the queued job"
        );

        let stale = DiagnosticsResult {
            diagnostics: vec![Diagnostic {
                range: SourceRange::from_bounds(6, 13).expect("range"),
                severity: DiagnosticSeverity::Error,
                message: "stale".to_owned(),
                code: Some("2322".to_owned()),
            }],
            status: CapabilityStatus::Supported,
        };
        assert!(
            finish_script_diagnostics_job(&mut server, &old, Ok(stale)).is_empty(),
            "a result for the previous revision must not publish"
        );

        let outgoing = only(finish_script_diagnostics_job(
            &mut server,
            &latest,
            Ok(run_script_diagnostics(&backend, &latest)),
        ));
        let params = publish_params(&outgoing);
        assert_eq!(params["uri"], json!(SCRIPT_URI));
        assert_eq!(params["version"], json!(2));
        assert_eq!(params["diagnostics"], json!([]));
    }

    #[test]
    fn script_diagnostics_retry_after_an_unrelated_project_revision() {
        let answer = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x: string = 1;\n"));
        let (mut server, dispatch) =
            script_diagnostics_server(backend.clone(), host, "const x: string = 1;\n");
        let old = take_script_diagnostics_job(&dispatch);

        // Opening an unrelated JSON overlay advances the project revision. The
        // script text is unchanged, so its in-flight check must be retried
        // instead of silently disappearing.
        assert!(
            server
                .handle_incoming(notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": "file:///pages/index/app.json",
                            "languageId": "json",
                            "version": 1,
                            "text": "{\"pages\":[]}",
                        }
                    }),
                ))
                .is_empty()
        );
        assert!(
            finish_script_diagnostics_job(
                &mut server,
                &old,
                Ok(run_script_diagnostics(&backend, &old))
            )
            .is_empty(),
            "the stale result must not publish directly"
        );

        let retry = take_script_diagnostics_job(&dispatch);
        assert_eq!(retry.attempt, 1);
        assert_eq!(retry.stamp.revision, old.stamp.revision);
        let outgoing = only(finish_script_diagnostics_job(
            &mut server,
            &retry,
            Ok(run_script_diagnostics(&backend, &retry)),
        ));
        let params = publish_params(&outgoing);
        assert_eq!(params["uri"], json!(SCRIPT_URI));
        assert_eq!(params["diagnostics"], json!([]));
    }

    #[test]
    fn stale_script_diagnostics_epoch_is_dropped_and_rescheduled() {
        let answer = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x: string = 1;\n"));
        let (mut server, dispatch) =
            script_diagnostics_server(backend.clone(), host, "const x: string = 1;\n");
        let job = take_script_diagnostics_job(&dispatch);

        let next_epoch = BackendEpoch::new(BackendId::new("mock"), 2);
        backend.inner.set_epoch(next_epoch.clone());
        assert!(
            finish_script_diagnostics_job(
                &mut server,
                &job,
                Ok(run_script_diagnostics(&backend, &job))
            )
            .is_empty(),
            "a result from the previous backend generation must not publish"
        );
        let retried = take_script_diagnostics_job(&dispatch);
        assert_eq!(
            retried.stamp.epoch, next_epoch,
            "the retry must use the new backend generation"
        );
        assert!(
            server.dirty.is_empty(),
            "the retry consumed the dirty marker"
        );
    }

    #[test]
    fn unsupported_script_diagnostics_publish_nothing() {
        let backend = Arc::new(MockBackend::new());
        let host = Arc::new(ScriptHost::new("const x: string = 1;\n"));
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend.clone(),
            backend.capabilities(),
            dispatch.clone(),
            host,
        );
        server.diagnostics_debounce = Duration::ZERO;
        initialize(&mut server);
        open_script_document(&mut server, "const x: string = 1;\n");

        let job = take_script_diagnostics_job(&dispatch);
        let answer = Backend::script_diagnostics(
            &*backend,
            DiagnosticsRequest {
                context: job.context.clone(),
            },
        )
        .expect("default script diagnostics");
        assert!(matches!(
            answer.status,
            CapabilityStatus::Unsupported { .. }
        ));
        assert!(
            finish_script_diagnostics_job(&mut server, &job, Ok(answer)).is_empty(),
            "Unsupported must not be published as an empty, checked script"
        );
        assert!(
            server
                .handle_incoming(notification(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": SCRIPT_URI}}),
                ))
                .is_empty(),
            "a script that never published does not need a close clear"
        );
    }

    #[test]
    fn partial_script_diagnostics_publish_their_entries() {
        let answer = DiagnosticsResult {
            diagnostics: vec![Diagnostic {
                range: SourceRange::from_bounds(0, 5).expect("range"),
                severity: DiagnosticSeverity::Warning,
                message: "partial script warning".to_owned(),
                code: None,
            }],
            status: CapabilityStatus::Partial {
                reason: "the project source list is incomplete".to_owned(),
            },
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x = 1;\n"));
        let (mut server, dispatch) =
            script_diagnostics_server(backend.clone(), host, "const x = 1;\n");
        let job = take_script_diagnostics_job(&dispatch);
        let outgoing = only(finish_script_diagnostics_job(
            &mut server,
            &job,
            Ok(run_script_diagnostics(&backend, &job)),
        ));
        let params = publish_params(&outgoing);
        let diagnostics = params["diagnostics"].as_array().expect("diagnostics");
        assert_eq!(
            diagnostics.len(),
            2,
            "the entry plus a partial status warning"
        );
        assert_eq!(diagnostics[0]["message"], json!("partial script warning"));
        let status = diagnostics[1]["message"].as_str().expect("status message");
        assert!(status.contains("partial"), "{status}");
        assert!(status.contains("incomplete"), "{status}");
    }

    #[test]
    fn did_save_schedules_script_diagnostics_immediately() {
        let answer = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x = 1;\n"));
        let dispatch = Arc::new(ManualDispatch::default());
        let mut server = Server::new_with_host(
            backend,
            MockBackend::new().capabilities(),
            dispatch.clone(),
            host,
        );
        server.diagnostics_debounce = Duration::from_millis(120);
        initialize(&mut server);
        open_script_document(&mut server, "const x = 1;\n");
        dispatch.take();

        server.handle_incoming(notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": SCRIPT_URI, "version": 2},
                "contentChanges": [{"text": "const x = 2;\n"}],
            }),
        ));
        assert!(dispatch.take().is_empty(), "typing is debounced");
        assert_eq!(server.debounced_diagnostics.len(), 1);

        server.handle_incoming(notification(
            "textDocument/didSave",
            json!({"textDocument": {"uri": SCRIPT_URI}}),
        ));
        let job = take_script_diagnostics_job(&dispatch);
        assert_eq!(job.context.source().text(), "const x = 2;\n");
        assert!(server.debounced_diagnostics.is_empty());
    }

    #[test]
    fn the_same_script_stamp_is_not_queued_twice_while_in_flight() {
        let answer = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x = 1;\n"));
        let (mut server, dispatch) = script_diagnostics_server(backend, host, "const x = 1;\n");
        let job = take_script_diagnostics_job(&dispatch);

        assert!(server.schedule_script_diagnostics(SCRIPT_URI).is_empty());
        assert!(server.schedule_script_diagnostics(SCRIPT_URI).is_empty());
        assert!(
            dispatch.take().is_empty(),
            "the exact same stamp is already in flight"
        );

        let _ = finish_script_diagnostics_job(
            &mut server,
            &job,
            Ok(DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            }),
        );
        assert!(server.schedule_script_diagnostics(SCRIPT_URI).is_empty());
        assert_eq!(dispatch.take().len(), 1, "a completed stamp may be retried");
    }

    #[test]
    fn script_diagnostics_never_merge_template_diagnostics() {
        let answer = DiagnosticsResult {
            diagnostics: vec![Diagnostic {
                range: SourceRange::from_bounds(0, 5).expect("range"),
                severity: DiagnosticSeverity::Error,
                message: "script only".to_owned(),
                code: Some("2322".to_owned()),
            }],
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x: string = 1;\n"));
        let (mut server, dispatch) =
            script_diagnostics_server(backend.clone(), host, "const x: string = 1;\n");
        let script_job = take_script_diagnostics_job(&dispatch);

        open_document(&mut server, "<view>{{ user.name }}</view>", 1);
        let template_job = dispatch
            .take()
            .into_iter()
            .find(|job| job.kind == JobKind::Diagnostics)
            .expect("a template diagnostics job");

        let outgoing = only(finish_script_diagnostics_job(
            &mut server,
            &script_job,
            Ok(run_script_diagnostics(&backend, &script_job)),
        ));
        let params = publish_params(&outgoing);
        assert_eq!(params["uri"], json!(SCRIPT_URI));
        let diagnostics = params["diagnostics"].as_array().expect("diagnostics");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["code"], json!("2322"));
        assert_eq!(diagnostics[0]["message"], json!("script only"));

        let outgoing = only(finish_script_diagnostics_job(
            &mut server,
            &template_job,
            Ok(DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            }),
        ));
        let params = publish_params(&outgoing);
        assert_eq!(params["uri"], json!(URI));
        assert!(
            !params["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .iter()
                .any(|diagnostic| diagnostic["code"] == json!("2322")),
            "a script diagnostic must not appear in the template payload: {params}"
        );
    }

    #[test]
    fn closing_a_published_script_cancels_and_clears_it() {
        let answer = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        let backend = Arc::new(ScriptDiagnosticsBackend::new(answer));
        let host = Arc::new(ScriptHost::new("const x = 1;\n"));
        let (mut server, dispatch) =
            script_diagnostics_server(backend.clone(), host, "const x = 1;\n");
        let first = take_script_diagnostics_job(&dispatch);
        let outgoing = only(finish_script_diagnostics_job(
            &mut server,
            &first,
            Ok(run_script_diagnostics(&backend, &first)),
        ));
        assert_eq!(publish_params(&outgoing)["uri"], json!(SCRIPT_URI));

        server.handle_incoming(notification(
            "textDocument/didSave",
            json!({"textDocument": {"uri": SCRIPT_URI}}),
        ));
        let pending = take_script_diagnostics_job(&dispatch);
        let outgoing = only(server.handle_incoming(notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": SCRIPT_URI}}),
        )));
        let params = publish_params(&outgoing);
        assert_eq!(params["uri"], json!(SCRIPT_URI));
        assert_eq!(params["diagnostics"], json!([]));
        assert!(params.get("version").is_none());
        assert!(
            pending.cancel.load(Ordering::SeqCst),
            "closing must cancel the pending script job"
        );
        assert!(
            finish_cancelled_script_diagnostics_job(&mut server, &pending).is_empty(),
            "a cancelled script job must not publish"
        );
        assert!(server.workspace.get(SCRIPT_URI).is_none());
    }
}
