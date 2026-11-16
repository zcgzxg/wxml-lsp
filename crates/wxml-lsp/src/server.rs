//! Protocol state machine and stdio event loop.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use thiserror::Error;
use wxml_types::{Backend, BackendCapabilities, BackendEpoch, BackendId, CapabilityStatus};

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
}

impl Dispatch for Scheduler {
    fn submit(&self, job: Job) -> Result<(), SubmitError> {
        Self::submit(self, job)
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

/// A request whose response has not been produced yet.
#[derive(Clone, Debug)]
pub(crate) struct Inflight {
    /// Id to echo in the response.
    pub(crate) id: serde_json::Value,
    /// Set by `$/cancelRequest`.
    pub(crate) cancel: Arc<AtomicBool>,
    /// Document the request belongs to, so `didClose` can cancel it.
    pub(crate) uri: String,
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
    /// Set by `exit` or a fatal transport error.
    pub(crate) stop: bool,
    /// Set by `shutdown`.
    pub(crate) clean_shutdown: bool,
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
            stop: false,
            clean_shutdown: false,
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

    /// Handles one parsed client message.
    pub(crate) fn handle_incoming(&mut self, incoming: Incoming) -> Vec<Outgoing> {
        match incoming {
            Incoming::Request { id, method, params } => self.on_request(id, &method, params),
            Incoming::Notification { method, params } => self.on_notification(&method, params),
            Incoming::Response { .. } => {
                log(format_args!(
                    "ignoring a response from the client (no server requests are outstanding)"
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
        if done.kind == JobKind::Diagnostics {
            let outgoing = self.diagnostics_done(done, &epoch);
            self.retry_pending_diagnostics_with_epoch(&epoch);
            return outgoing;
        }
        let Some(key) = done.key.clone() else {
            log(format_args!("dropping a request result without an id"));
            return Vec::new();
        };
        let Some(inflight) = self.inflight.remove(&key) else {
            log(format_args!("dropping a result for an unknown request"));
            return Vec::new();
        };
        let outgoing = match done.kind {
            JobKind::Hover => self.hover_done(inflight.id, done, &epoch),
            JobKind::Definition => self.definition_done(inflight.id, done, &epoch),
            JobKind::Completion(_) => self.completion_done(inflight.id, done, &epoch),
            JobKind::Diagnostics => Vec::new(),
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
            let _ = worker_events.send(Message::Work(done));
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
    while let Ok(message) = receiver.recv() {
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
            Message::Work(done) => server.handle_work(done),
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
    Work(WorkDone),
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
        DiagnosticSeverity, DiagnosticsResult, FileId, HoverResult, InsertTextFormat, MarkupKind,
        MockBackend, ProjectId, SourceLocation, TypeQuality,
    };

    use super::*;
    use crate::jsonrpc::ResponseOutcome;
    use crate::scheduler::{JobKind, Outcome};

    const URI: &str = "file:///pages/index/index.wxml";

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

    fn epoch() -> BackendEpoch {
        BackendEpoch::new(BackendId::new("mock"), 1)
    }

    fn setup() -> (Server, Arc<ManualDispatch>) {
        setup_with(MockBackend::new().capabilities())
    }

    fn setup_with(capabilities: BackendCapabilities) -> (Server, Arc<ManualDispatch>) {
        let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
        let dispatch = Arc::new(ManualDispatch::default());
        let server = Server::new(backend, capabilities, dispatch.clone());
        (server, dispatch)
    }

    fn setup_with_backend(
        backend: Arc<MockBackend>,
        capabilities: BackendCapabilities,
    ) -> (Server, Arc<ManualDispatch>) {
        let dispatch = Arc::new(ManualDispatch::default());
        let server = Server::new(backend, capabilities, dispatch.clone());
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
            cancelled: false,
            outcome: Ok(Outcome::Hover(Box::new(result))),
        }
    }

    fn completed_diagnostics(job: &Job, result: DiagnosticsResult) -> WorkDone {
        WorkDone {
            key: job.key.clone(),
            kind: JobKind::Diagnostics,
            stamp: job.stamp.clone(),
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
        assert!(
            server
                .handle_incoming(notification("$/cancelRequest", json!({"id": "hover-1"}),))
                .is_empty()
        );
        assert!(jobs[0].cancel.load(Ordering::SeqCst));
        assert!(
            server
                .handle_incoming(notification("$/cancelRequest", json!({"id": "unknown"})))
                .is_empty()
        );
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
            cancelled: false,
            outcome: Err(BackendError::Timeout),
        };
        let outgoing = only(server.handle_work(done));
        assert_eq!(result(&outgoing), &json!(null));
    }

    #[test]
    fn unsupported_hover_is_not_registered() {
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
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::METHOD_NOT_FOUND
        );
    }

    #[test]
    fn definition_is_method_not_found_when_unsupported() {
        let (mut server, _) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let outgoing = only(server.handle_incoming(request(
            json!(11),
            "textDocument/definition",
            hover_params((0, 1)),
        )));
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::METHOD_NOT_FOUND
        );
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
        let outgoing =
            only(server.handle_incoming(request(json!(16), "textDocument/formatting", json!({}))));
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
    fn cancelled_diagnostics_are_not_published() {
        let (mut server, dispatch) = setup();
        initialize(&mut server);
        open_document(&mut server, "abc", 1);
        let jobs = dispatch.take();
        let done = WorkDone {
            key: None,
            kind: JobKind::Diagnostics,
            stamp: jobs[0].stamp.clone(),
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
            cancelled: false,
            outcome: Ok(Outcome::Diagnostics(Box::new(DiagnosticsResult {
                diagnostics: vec![Diagnostic {
                    range: SourceRange::from_bounds(0, 1).expect("range"),
                    severity: DiagnosticSeverity::Error,
                    message: "syntax".to_owned(),
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
                _request: wxml_types::DiagnosticsRequest,
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
        assert_eq!(
            error_code(&outgoing),
            jsonrpc::error_codes::METHOD_NOT_FOUND
        );
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
}
