//! Single background worker for backend requests.
//!
//! The backend boundary is synchronous and potentially slow, so the protocol
//! loop never calls it directly. Jobs carry the exact [`AnalysisStamp`] they
//! were created for; the server checks that stamp before publishing a result.
//! Cancellation is a shared atomic flag: `$/cancelRequest` sets it, the server
//! answers immediately, and the worker checks it before and after the backend
//! call.
//!
//! The queue is a bounded two-class priority queue:
//!
//! * interactive requests (hover, definition, completion, symbols) run before
//!   queued diagnostics, so a diagnostics burst cannot make the pointer hang;
//! * diagnostics are coalesced per document, so a burst of edits for one file
//!   keeps at most one queued job for that file;
//! * after [`MAX_INTERACTIVE_BURST`] interactive jobs the worker runs one
//!   diagnostics job when any is queued, so a continuous hover stream cannot
//!   starve diagnostics either.
//!
//! A synchronous [`Backend`] call cannot be interrupted without leaking a
//! thread per call. A running TypeScript call therefore keeps the worker until
//! it returns or its backend timeout kills the process; cancellation latency
//! for *queued* jobs is bounded by the protocol loop, and running-call
//! occupancy is measured separately by the Q2 benchmark.

use std::collections::VecDeque;
use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use wxml_types::{
    Backend, BackendError, CompletionKind, CompletionRequest, CompletionResult, DefinitionRequest,
    DefinitionResult, DiagnosticsRequest, DiagnosticsResult, FileId, HoverRequest, HoverResult,
    RequestContext, SymbolQuery, SymbolRequest, SymbolResult,
};

use crate::jsonrpc::RequestKey;
use crate::server::log;
use crate::workspace::AnalysisStamp;

/// Maximum number of queued interactive jobs before the server reports itself
/// as busy.
///
/// Pointer hovers arrive in bursts while the first analysis is still running.
/// The queue is large enough to absorb a normal mouse sweep so the popup does
/// not disappear with `server is busy`. Jobs for one document share the same
/// `Arc<SourceSnapshot>`, so a burst on one revision does not multiply its
/// text; the worst case with many documents and revisions stays bounded.
pub(crate) const MAX_PENDING_JOBS: usize = 128;

/// Maximum number of queued diagnostics jobs.
///
/// Diagnostics are coalesced per document, and the server debounces edits, so
/// a normal workspace stays far below this. The separate small bound keeps
/// diagnostics from ever occupying the interactive admission capacity.
pub(crate) const MAX_PENDING_DIAGNOSTICS: usize = 8;

/// How many interactive jobs may run before a pending diagnostics job gets a
/// turn. Guarantees diagnostics progress under a continuous hover stream.
pub(crate) const MAX_INTERACTIVE_BURST: usize = 8;

/// How long [`Scheduler::shutdown`] waits for the worker before detaching it.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Poll interval while waiting for the worker to finish.
const SHUTDOWN_POLL: Duration = Duration::from_millis(5);

/// What kind of backend query a job performs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JobKind {
    /// A hover query that will answer a request.
    Hover,
    /// A diagnostics query that will publish a notification.
    Diagnostics,
    /// A definition query that will answer a request.
    Definition,
    /// A completion query that will answer a request.
    Completion(CompletionKind),
    /// A script symbol query used by references, rename or code actions.
    Symbols {
        /// Script the query is about.
        subject: FileId,
        /// Query identity.
        query: SymbolQuery,
    },
}

impl JobKind {
    /// Returns `true` for request/response jobs that need interactive latency.
    const fn is_interactive(&self) -> bool {
        !matches!(self, Self::Diagnostics)
    }
}

/// One unit of backend work.
#[derive(Clone, Debug)]
pub(crate) struct Job {
    /// Request id for request/response jobs; `None` for diagnostics.
    pub(crate) key: Option<RequestKey>,
    /// Query to run.
    pub(crate) kind: JobKind,
    /// Versions and snapshot the query depends on.
    pub(crate) context: RequestContext,
    /// UTF-8 byte offset for hover/definition.
    pub(crate) offset: u32,
    /// Stamp the result must match before it is published.
    pub(crate) stamp: AnalysisStamp,
    /// Set by `$/cancelRequest`.
    pub(crate) cancel: Arc<AtomicBool>,
}

/// A successful backend answer.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Hover text.
    Hover(Box<HoverResult>),
    /// Diagnostics.
    Diagnostics(Box<DiagnosticsResult>),
    /// Definition locations.
    Definition(Box<DefinitionResult>),
    /// Completion candidates.
    Completion(Box<CompletionResult>),
    /// Script symbol facts.
    Symbols(Box<SymbolResult>),
}

/// The completed (or cancelled) result of one job.
#[derive(Debug)]
pub(crate) struct WorkDone {
    /// Request id for request/response jobs.
    pub(crate) key: Option<RequestKey>,
    /// Query that ran.
    pub(crate) kind: JobKind,
    /// Stamp captured when the job was created.
    pub(crate) stamp: AnalysisStamp,
    /// Whether the job was cancelled before or during the backend call.
    pub(crate) cancelled: bool,
    /// Backend answer. A pre-call cancellation reports
    /// [`BackendError::Cancelled`].
    pub(crate) outcome: Result<Outcome, BackendError>,
}

/// Why a job could not be queued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubmitError {
    /// The queue is full; the server is busy.
    Busy,
    /// The worker is gone.
    Stopped,
}

/// Bounded job queues plus the worker wake-up condition.
struct Queue {
    interactive: VecDeque<Box<Job>>,
    diagnostics: VecDeque<Box<Job>>,
    consecutive_interactive: usize,
    stopping: bool,
    max_interactive: usize,
    max_diagnostics: usize,
}

impl Queue {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            interactive: VecDeque::new(),
            diagnostics: VecDeque::new(),
            consecutive_interactive: 0,
            stopping: false,
            max_interactive: capacity,
            max_diagnostics: capacity.min(MAX_PENDING_DIAGNOSTICS),
        }
    }
}

/// Shared queue handle used by the dispatcher and the worker.
type QueueHandle = Arc<(Mutex<Queue>, Condvar)>;

struct Worker {
    queue: QueueHandle,
    handle: JoinHandle<()>,
}

/// A thread that runs backend jobs by class and priority.
pub(crate) struct Scheduler {
    worker: Mutex<Option<Worker>>,
}

impl Scheduler {
    /// Starts the worker with [`MAX_PENDING_JOBS`] interactive queue slots.
    pub(crate) fn start<S>(backend: Arc<dyn Backend>, sink: S) -> std::io::Result<Self>
    where
        S: Fn(WorkDone) + Send + 'static,
    {
        Self::start_with_capacity(backend, MAX_PENDING_JOBS, sink)
    }

    /// Starts the worker with an explicit interactive queue capacity.
    fn start_with_capacity<S>(
        backend: Arc<dyn Backend>,
        capacity: usize,
        sink: S,
    ) -> std::io::Result<Self>
    where
        S: Fn(WorkDone) + Send + 'static,
    {
        let queue: QueueHandle = Arc::new((Mutex::new(Queue::new(capacity)), Condvar::new()));
        let worker_queue = Arc::clone(&queue);
        let handle = thread::Builder::new()
            .name("wxml-lsp-worker".to_owned())
            .spawn(move || worker_loop(&*backend, &worker_queue, &sink))?;
        Ok(Self {
            worker: Mutex::new(Some(Worker { queue, handle })),
        })
    }

    /// Queues one job, failing fast when its class is full.
    ///
    /// Diagnostics for a file that already has a queued diagnostics job
    /// replace that job, so the newest revision is the one that runs.
    // The outer lock is held until the inner queue lock is acquired so a
    // concurrent shutdown cannot take the worker between lookup and insert.
    #[allow(clippy::significant_drop_tightening)]
    pub(crate) fn submit(&self, job: Job) -> Result<(), SubmitError> {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        let worker = worker.as_mut().ok_or(SubmitError::Stopped)?;
        let (lock, condvar) = &*worker.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.stopping {
            return Err(SubmitError::Stopped);
        }
        if job.kind.is_interactive() {
            if queue.interactive.len() >= queue.max_interactive {
                return Err(SubmitError::Busy);
            }
            queue.interactive.push_back(Box::new(job));
        } else {
            if let Some(position) = queue
                .diagnostics
                .iter()
                .position(|queued| queued.stamp.file == job.stamp.file)
            {
                if let Some(previous) = queue.diagnostics.remove(position) {
                    previous.cancel.store(true, Ordering::SeqCst);
                }
            } else if queue.diagnostics.len() >= queue.max_diagnostics {
                return Err(SubmitError::Busy);
            }
            queue.diagnostics.push_back(Box::new(job));
        }
        condvar.notify_one();
        Ok(())
    }

    /// Drops a queued interactive job whose request was cancelled.
    ///
    /// The server has already answered the request, so no synthetic completion
    /// is needed; a running job is handled by its shared cancel flag.
    #[allow(clippy::significant_drop_tightening)]
    pub(crate) fn cancel(&self, key: &RequestKey) {
        let worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(worker) = worker.as_ref() else {
            return;
        };
        let (lock, _condvar) = &*worker.queue;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(position) = queue
            .interactive
            .iter()
            .position(|job| job.key.as_ref() == Some(key))
        {
            if let Some(job) = queue.interactive.remove(position) {
                job.cancel.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Stops the worker and waits at most [`SHUTDOWN_TIMEOUT`] for the current
    /// backend call.
    ///
    /// Queued jobs are discarded. If a backend violates the non-blocking
    /// contract, the thread is detached instead of blocking process exit
    /// forever.
    pub(crate) fn shutdown(&self) {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(Worker { queue, handle }) = worker else {
            return;
        };
        {
            let (lock, condvar) = &*queue;
            let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
            queue.stopping = true;
            queue.interactive.clear();
            queue.diagnostics.clear();
            drop(queue);
            condvar.notify_all();
        }
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                log(format_args!(
                    "worker did not stop within {SHUTDOWN_TIMEOUT:?}; detaching it"
                ));
                return;
            }
            thread::sleep(SHUTDOWN_POLL);
        }
        let _ = handle.join();
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Body of the worker thread.
fn worker_loop<S>(backend: &dyn Backend, queue: &QueueHandle, sink: &S)
where
    S: Fn(WorkDone),
{
    let (lock, condvar) = &**queue;
    loop {
        let job = {
            let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if state.stopping {
                    return;
                }
                let diagnostics_turn = state.interactive.is_empty()
                    || (state.consecutive_interactive >= MAX_INTERACTIVE_BURST
                        && !state.diagnostics.is_empty());
                if diagnostics_turn {
                    if let Some(job) = state.diagnostics.pop_front() {
                        state.consecutive_interactive = 0;
                        break job;
                    }
                }
                if let Some(job) = state.interactive.pop_front() {
                    state.consecutive_interactive = state.consecutive_interactive.saturating_add(1);
                    break job;
                }
                state = condvar.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
        };
        if job.cancel.load(Ordering::SeqCst) {
            sink(cancelled(&job));
            continue;
        }
        let outcome = run(backend, &job);
        {
            let state = lock.lock().unwrap_or_else(PoisonError::into_inner);
            if state.stopping {
                return;
            }
        }
        let cancelled = job.cancel.load(Ordering::SeqCst);
        sink(WorkDone {
            key: job.key.clone(),
            kind: job.kind,
            stamp: job.stamp.clone(),
            cancelled,
            outcome,
        });
    }
}

/// Runs one backend call, mapping a panic into an internal error so the
/// worker survives a misbehaving backend.
fn run(backend: &dyn Backend, job: &Job) -> Result<Outcome, BackendError> {
    let Ok(result) = catch_unwind(std::panic::AssertUnwindSafe(|| match &job.kind {
        JobKind::Hover => backend
            .hover(HoverRequest {
                context: job.context.clone(),
                offset: job.offset,
            })
            .map(|hover| Outcome::Hover(Box::new(hover))),
        JobKind::Diagnostics => backend
            .diagnostics(DiagnosticsRequest {
                context: job.context.clone(),
            })
            .map(|diagnostics| Outcome::Diagnostics(Box::new(diagnostics))),
        JobKind::Definition => backend
            .definition(DefinitionRequest {
                context: job.context.clone(),
                offset: job.offset,
            })
            .map(|definition| Outcome::Definition(Box::new(definition))),
        JobKind::Completion(kind) => backend
            .completion(CompletionRequest {
                context: job.context.clone(),
                offset: job.offset,
                kind: *kind,
            })
            .map(|completion| Outcome::Completion(Box::new(completion))),
        JobKind::Symbols { subject, query } => backend
            .symbols(SymbolRequest {
                context: job.context.clone(),
                subject: subject.clone(),
                query: query.clone(),
            })
            .map(|symbols| Outcome::Symbols(Box::new(symbols))),
    })) else {
        return Err(BackendError::Internal("backend panicked".to_owned()));
    };
    result
}

/// Builds the result for a job cancelled before its backend call.
fn cancelled(job: &Job) -> WorkDone {
    WorkDone {
        key: job.key.clone(),
        kind: job.kind.clone(),
        stamp: job.stamp.clone(),
        cancelled: true,
        outcome: Err(BackendError::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Condvar, PoisonError, mpsc};
    use std::time::Duration;

    use serde_json::json;
    use wxml_text::SourceRange;
    use wxml_types::{
        BackendCapabilities, BackendEpoch, BackendId, CapabilityStatus, DefinitionResult,
        Diagnostic, DiagnosticSeverity, HoverResult, MarkupKind, MockBackend, SourceLocation,
        TypeQuality,
    };

    use super::*;
    use crate::workspace::Workspace;

    const URI: &str = "file:///pages/index/index.wxml";

    fn epoch() -> BackendEpoch {
        BackendEpoch::new(BackendId::new("mock"), 1)
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

    fn context_and_stamp(offset: u32) -> (RequestContext, AnalysisStamp) {
        let mut workspace = Workspace::new();
        workspace
            .open(URI, "abc".to_owned(), Some(1))
            .expect("open");
        let document = workspace.get(URI).expect("document");
        let context = document.request_context(&epoch()).expect("context");
        let stamp = document.stamp(&epoch());
        let _ = offset;
        (context, stamp)
    }

    fn job(kind: JobKind, key: Option<&str>, offset: u32) -> Job {
        let (context, stamp) = context_and_stamp(offset);
        Job {
            key: key.map(|key| RequestKey::from_id(&json!(key)).expect("valid id")),
            kind,
            context,
            offset,
            stamp,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn receive(receiver: &mpsc::Receiver<WorkDone>) -> WorkDone {
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("worker result")
    }

    #[test]
    fn executes_hover_jobs_in_submission_order() {
        let backend = Arc::new(MockBackend::new().with_hover(hover_result()));
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend.clone(), move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("one"), 1))
            .expect("first");
        scheduler
            .submit(job(JobKind::Hover, Some("two"), 2))
            .expect("second");

        let first = receive(&receiver);
        let second = receive(&receiver);
        assert_eq!(first.key, RequestKey::from_id(&json!("one")));
        assert_eq!(second.key, RequestKey::from_id(&json!("two")));
        assert!(!first.cancelled);
        assert!(
            matches!(first.outcome, Ok(Outcome::Hover(ref hover)) if hover.contents == "string")
        );
        assert_eq!(backend.calls().len(), 2);
        scheduler.shutdown();
        assert_eq!(
            scheduler.submit(job(JobKind::Hover, Some("three"), 3)),
            Err(SubmitError::Stopped)
        );
    }

    #[test]
    fn executes_diagnostics_and_definition_jobs() {
        let backend = Arc::new(
            MockBackend::new()
                .with_diagnostics(DiagnosticsResult {
                    diagnostics: vec![Diagnostic {
                        range: SourceRange::from_bounds(1, 2).expect("range"),
                        severity: DiagnosticSeverity::Error,
                        message: "bad".to_owned(),
                        code: None,
                    }],
                    status: CapabilityStatus::Supported,
                })
                .with_definition(DefinitionResult {
                    locations: vec![SourceLocation {
                        file: FileId::new(wxml_types::ProjectId::new("lsp"), "documents/1.wxml")
                            .expect("file"),
                        range: SourceRange::from_bounds(0, 1).expect("range"),
                    }],
                    status: CapabilityStatus::Supported,
                }),
        );
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Diagnostics, None, 0))
            .expect("diagnostics");
        scheduler
            .submit(job(JobKind::Definition, Some("def"), 1))
            .expect("definition");

        // The worker may start the first job before the second is queued, so
        // this test only requires both classes to complete.
        let first = receive(&receiver);
        let second = receive(&receiver);
        let mut saw_definition = false;
        let mut saw_diagnostics = false;
        for done in [first, second] {
            match done.outcome {
                Ok(Outcome::Definition(result)) => {
                    assert_eq!(result.locations.len(), 1);
                    saw_definition = true;
                }
                Ok(Outcome::Diagnostics(result)) => {
                    assert_eq!(result.diagnostics.len(), 1);
                    saw_diagnostics = true;
                }
                other => panic!("unexpected result: {other:?}"),
            }
        }
        assert!(saw_definition && saw_diagnostics);
    }

    #[test]
    fn survives_backend_errors_and_continues() {
        let backend = Arc::new(MockBackend::new().with_error(BackendError::Timeout));
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend.clone(), move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("one"), 1))
            .expect("first");
        let failed = receive(&receiver);
        assert!(matches!(failed.outcome, Err(BackendError::Timeout)));

        backend.clear_errors();
        backend.set_hover(hover_result());
        scheduler
            .submit(job(JobKind::Hover, Some("two"), 1))
            .expect("second");
        let recovered = receive(&receiver);
        assert!(matches!(recovered.outcome, Ok(Outcome::Hover(_))));
    }

    /// A backend whose hover blocks until the test releases it.
    struct GatedBackend {
        entered: mpsc::Sender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        calls: AtomicUsize,
        diagnostics_calls: AtomicUsize,
    }

    struct Gate {
        entered: mpsc::Receiver<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl GatedBackend {
        fn new() -> (Arc<Self>, Gate) {
            let (entered_tx, entered_rx) = mpsc::channel();
            let gate = Arc::new((Mutex::new(false), Condvar::new()));
            (
                Arc::new(Self {
                    entered: entered_tx,
                    gate: Arc::clone(&gate),
                    calls: AtomicUsize::new(0),
                    diagnostics_calls: AtomicUsize::new(0),
                }),
                Gate {
                    entered: entered_rx,
                    gate,
                },
            )
        }
    }

    impl Backend for GatedBackend {
        fn epoch(&self) -> BackendEpoch {
            epoch()
        }

        #[allow(clippy::significant_drop_tightening)] // the lock guards the wait
        fn hover(&self, _request: HoverRequest) -> Result<HoverResult, BackendError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.entered.send(());
            let (lock, condvar) = &*self.gate;
            let mut released = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while !*released {
                released = condvar
                    .wait(released)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            Ok(hover_result())
        }

        fn diagnostics(
            &self,
            _request: DiagnosticsRequest,
        ) -> Result<DiagnosticsResult, BackendError> {
            self.diagnostics_calls.fetch_add(1, Ordering::SeqCst);
            Ok(DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            })
        }

        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities::none()
        }
    }

    impl Gate {
        fn wait_until_entered(&self) {
            self.entered
                .recv_timeout(Duration::from_secs(10))
                .expect("backend entered");
        }

        #[allow(clippy::significant_drop_tightening)] // held until notify
        fn release(&self) {
            let (lock, condvar) = &*self.gate;
            let mut released = lock.lock().unwrap_or_else(PoisonError::into_inner);
            *released = true;
            condvar.notify_all();
        }
    }

    #[test]
    fn skips_the_backend_call_for_pre_cancelled_jobs() {
        let (backend, gate) = GatedBackend::new();
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend.clone(), move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("blocking"), 1))
            .expect("first");
        gate.wait_until_entered();

        let cancelled = job(JobKind::Hover, Some("cancelled"), 1);
        let flag = Arc::clone(&cancelled.cancel);
        scheduler.submit(cancelled).expect("second");
        flag.store(true, Ordering::SeqCst);
        gate.release();

        let first = receive(&receiver);
        assert!(!first.cancelled);
        let second = receive(&receiver);
        assert!(second.cancelled);
        assert!(matches!(second.outcome, Err(BackendError::Cancelled)));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn marks_results_cancelled_after_the_backend_call() {
        let (backend, gate) = GatedBackend::new();
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        let running = job(JobKind::Hover, Some("running"), 1);
        let flag = Arc::clone(&running.cancel);
        scheduler.submit(running).expect("submit");
        gate.wait_until_entered();
        flag.store(true, Ordering::SeqCst);
        gate.release();

        let done = receive(&receiver);
        assert!(done.cancelled);
        assert!(matches!(done.outcome, Ok(Outcome::Hover(_))));
    }

    #[test]
    fn shutdown_is_bounded_when_the_backend_hangs() {
        let (backend, gate) = GatedBackend::new();
        let (sender, _receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("hanging"), 1))
            .expect("submitted");
        gate.wait_until_entered();

        let start = Instant::now();
        scheduler.shutdown();
        let elapsed = start.elapsed();
        assert!(
            elapsed < SHUTDOWN_TIMEOUT + Duration::from_secs(1),
            "shutdown took {elapsed:?}"
        );
        assert_eq!(
            scheduler.submit(job(JobKind::Hover, Some("after"), 1)),
            Err(SubmitError::Stopped)
        );
        gate.release();
    }

    #[test]
    fn default_queue_absorbs_a_hover_burst() {
        let (backend, gate) = GatedBackend::new();
        let (sender, _receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();
        for index in 0..64 {
            scheduler
                .submit(job(JobKind::Hover, Some(&format!("queued-{index}")), 1))
                .expect("a hover burst must not report the server busy");
        }
        gate.release();
    }

    #[test]
    fn rejects_jobs_when_the_queue_is_full() {
        let (backend, gate) = GatedBackend::new();
        let (sender, _receiver) = mpsc::channel();
        let scheduler = Scheduler::start_with_capacity(backend, 1, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();
        scheduler
            .submit(job(JobKind::Hover, Some("queued"), 1))
            .expect("queued");
        assert_eq!(
            scheduler.submit(job(JobKind::Hover, Some("rejected"), 1)),
            Err(SubmitError::Busy)
        );
        gate.release();
    }

    #[test]
    fn diagnostics_are_coalesced_per_document() {
        let (backend, gate) = GatedBackend::new();
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend.clone(), move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();

        let first = job(JobKind::Diagnostics, None, 0);
        let second = job(JobKind::Diagnostics, None, 0);
        let first_stamp = first.stamp.clone();
        scheduler.submit(first).expect("first diagnostics");
        scheduler.submit(second).expect("second diagnostics");
        gate.release();

        let hover = receive(&receiver);
        assert!(!hover.cancelled);
        let diagnostics = receive(&receiver);
        assert_eq!(diagnostics.kind, JobKind::Diagnostics);
        assert_eq!(diagnostics.stamp, first_stamp);
        assert_eq!(
            backend.diagnostics_calls.load(Ordering::SeqCst),
            1,
            "the superseded queued diagnostics job must not call the backend"
        );
    }

    #[test]
    fn interactive_jobs_run_before_queued_diagnostics() {
        let (backend, gate) = GatedBackend::new();
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();
        scheduler
            .submit(job(JobKind::Diagnostics, None, 0))
            .expect("diagnostics");
        scheduler
            .submit(job(JobKind::Hover, Some("pointer"), 1))
            .expect("pointer");
        gate.release();

        let first = receive(&receiver);
        assert_eq!(first.kind, JobKind::Hover);
        let second = receive(&receiver);
        assert_eq!(
            second.kind,
            JobKind::Hover,
            "a queued diagnostics job must not delay an interactive request"
        );
        let third = receive(&receiver);
        assert_eq!(third.kind, JobKind::Diagnostics);
    }

    #[test]
    fn a_diagnostics_job_gets_a_turn_under_a_hover_burst() {
        let (backend, gate) = GatedBackend::new();
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();
        scheduler
            .submit(job(JobKind::Diagnostics, None, 0))
            .expect("diagnostics");
        for index in 0..(MAX_INTERACTIVE_BURST * 2) {
            scheduler
                .submit(job(JobKind::Hover, Some(&format!("hover-{index}")), 1))
                .expect("hover");
        }
        gate.release();

        let mut diagnostics_at = None;
        for index in 0..(MAX_INTERACTIVE_BURST * 2 + 2) {
            let done = receive(&receiver);
            if done.kind == JobKind::Diagnostics {
                diagnostics_at = Some(index);
                break;
            }
        }
        let diagnostics_at = diagnostics_at.expect("diagnostics must not be starved");
        assert!(
            diagnostics_at <= MAX_INTERACTIVE_BURST,
            "diagnostics ran after {diagnostics_at} interactive jobs"
        );
    }

    #[test]
    fn cancel_removes_a_queued_interactive_job() {
        let (backend, gate) = GatedBackend::new();
        let (sender, receiver) = mpsc::channel();
        let scheduler = Scheduler::start(backend, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();
        let queued = job(JobKind::Hover, Some("queued"), 1);
        let key = queued.key.clone().expect("key");
        scheduler.submit(queued).expect("queued");
        scheduler.cancel(&key);
        gate.release();

        let first = receive(&receiver);
        assert!(!first.cancelled);
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_err(),
            "a cancelled queued job must not run or answer"
        );
    }

    #[test]
    fn diagnostics_cap_keeps_interactive_admission_free() {
        let (backend, gate) = GatedBackend::new();
        let (sender, _receiver) = mpsc::channel();
        let scheduler = Scheduler::start_with_capacity(backend, 2, move |done| {
            let _ = sender.send(done);
        })
        .expect("worker");

        scheduler
            .submit(job(JobKind::Hover, Some("running"), 1))
            .expect("running");
        gate.wait_until_entered();
        // The explicit capacity also caps diagnostics; fill it, then verify an
        // interactive job still fits in its own class.
        scheduler
            .submit(job(JobKind::Diagnostics, None, 0))
            .expect("diagnostics one");
        let mut second = job(JobKind::Diagnostics, None, 0);
        second.stamp.file =
            FileId::new(wxml_types::ProjectId::new("lsp"), "documents/2.wxml").expect("file");
        scheduler.submit(second).expect("diagnostics two");
        scheduler
            .submit(job(JobKind::Hover, Some("interactive"), 1))
            .expect("interactive must not be blocked by diagnostics");
        gate.release();
    }
}
