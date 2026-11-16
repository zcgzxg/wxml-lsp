//! Single background worker for backend requests.
//!
//! The backend boundary is synchronous and potentially slow, so the protocol
//! loop never calls it directly. Jobs carry the exact [`AnalysisStamp`] they
//! were created for; the server checks that stamp before publishing a result.
//! Cancellation is a shared atomic flag: `$/cancelRequest` sets it, and the
//! worker checks it before and after the backend call.
//!
//! There is deliberately no per-request deadline yet. The synchronous
//! [`Backend`] trait cannot be interrupted safely without leaking a thread per
//! call, so a backend that violates its non-blocking contract stalls hover and
//! diagnostics until it returns. Request-level timeouts and backend process
//! isolation arrive with the C1 protocol and the real backend (P5); shutdown
//! itself remains bounded because it detaches after two seconds.

use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use wxml_types::{
    Backend, BackendError, CompletionKind, CompletionRequest, CompletionResult, DefinitionRequest,
    DefinitionResult, DiagnosticsRequest, DiagnosticsResult, HoverRequest, HoverResult,
    RequestContext,
};

use crate::jsonrpc::RequestKey;
use crate::server::log;
use crate::workspace::AnalysisStamp;

/// Maximum number of queued jobs before the server reports itself as busy.
///
/// Each job pins an `Arc<SourceSnapshot>` for an old revision. Those snapshots
/// are outside the workspace's 64 MiB text-plus-line-index budget; the queue
/// plus the running job bound the extra pinned text to at most 33 × 8 MiB
/// (≈264 MiB) in the worst case.
pub(crate) const MAX_PENDING_JOBS: usize = 32;

/// How long [`Scheduler::shutdown`] waits for the worker before detaching it.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Poll interval while waiting for the worker to finish.
const SHUTDOWN_POLL: Duration = Duration::from_millis(5);

/// What kind of backend query a job performs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JobKind {
    /// A hover query that will answer a request.
    Hover,
    /// A diagnostics query that will publish a notification.
    Diagnostics,
    /// A definition query that will answer a request.
    Definition,
    /// A completion query that will answer a request.
    Completion(CompletionKind),
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

/// Worker state behind an `Arc` so the scheduler is `Send + Sync` and can be
/// shared with the protocol loop as a dispatcher.
struct Worker {
    sender: SyncSender<Box<Job>>,
    handle: JoinHandle<()>,
}

/// A thread that runs backend jobs in submission order.
pub(crate) struct Scheduler {
    worker: Mutex<Option<Worker>>,
    /// Set before the sender is dropped so the worker stops after the current
    /// backend call and queued jobs are discarded.
    stopping: Arc<AtomicBool>,
}

impl Scheduler {
    /// Starts the worker with [`MAX_PENDING_JOBS`] queue slots.
    pub(crate) fn start<S>(backend: Arc<dyn Backend>, sink: S) -> std::io::Result<Self>
    where
        S: Fn(WorkDone) + Send + 'static,
    {
        Self::start_with_capacity(backend, MAX_PENDING_JOBS, sink)
    }

    /// Starts the worker with an explicit queue capacity.
    fn start_with_capacity<S>(
        backend: Arc<dyn Backend>,
        capacity: usize,
        sink: S,
    ) -> std::io::Result<Self>
    where
        S: Fn(WorkDone) + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let handle = thread::Builder::new()
            .name("wxml-lsp-worker".to_owned())
            .spawn(move || worker_loop(&*backend, &receiver, &sink, &worker_stopping))?;
        Ok(Self {
            worker: Mutex::new(Some(Worker { sender, handle })),
            stopping,
        })
    }

    /// Queues one job, failing fast when the queue is full.
    // The lock is held until the send completes so a concurrent shutdown
    // cannot take the sender between the lookup and the send.
    #[allow(clippy::significant_drop_tightening)]
    pub(crate) fn submit(&self, job: Job) -> Result<(), SubmitError> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(SubmitError::Stopped);
        }
        let mut guard = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        let worker = guard.as_mut().ok_or(SubmitError::Stopped)?;
        match worker.sender.try_send(Box::new(job)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SubmitError::Busy),
            Err(TrySendError::Disconnected(_)) => {
                guard.take();
                Err(SubmitError::Stopped)
            }
        }
    }

    /// Stops the worker and waits at most [`SHUTDOWN_TIMEOUT`] for the current
    /// backend call.
    ///
    /// Queued jobs are discarded: the stopping flag makes the worker break
    /// before it runs the next job, and dropping the sender closes the channel.
    /// If a backend violates the non-blocking contract, the thread is detached
    /// instead of blocking process exit forever.
    pub(crate) fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(Worker { sender, handle }) = worker else {
            return;
        };
        drop(sender);
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
fn worker_loop<S>(
    backend: &dyn Backend,
    receiver: &Receiver<Box<Job>>,
    sink: &S,
    stopping: &AtomicBool,
) where
    S: Fn(WorkDone),
{
    while let Ok(job) = receiver.recv() {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        if job.cancel.load(Ordering::SeqCst) {
            sink(cancelled(&job));
            continue;
        }
        let outcome = run(backend, &job);
        if stopping.load(Ordering::SeqCst) {
            break;
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
    let Ok(result) = catch_unwind(std::panic::AssertUnwindSafe(|| match job.kind {
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
                kind,
            })
            .map(|completion| Outcome::Completion(Box::new(completion))),
    })) else {
        return Err(BackendError::Internal("backend panicked".to_owned()));
    };
    result
}

/// Builds the result for a job cancelled before its backend call.
fn cancelled(job: &Job) -> WorkDone {
    WorkDone {
        key: job.key.clone(),
        kind: job.kind,
        stamp: job.stamp.clone(),
        cancelled: true,
        outcome: Err(BackendError::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Condvar, PoisonError};
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

    fn receive(receiver: &Receiver<WorkDone>) -> WorkDone {
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
                    }],
                    status: CapabilityStatus::Supported,
                })
                .with_definition(DefinitionResult {
                    locations: vec![SourceLocation {
                        file: wxml_types::FileId::new(
                            wxml_types::ProjectId::new("lsp"),
                            "documents/1.wxml",
                        )
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

        let diagnostics = receive(&receiver);
        assert!(matches!(
            diagnostics.outcome,
            Ok(Outcome::Diagnostics(ref result)) if result.diagnostics.len() == 1
        ));
        let definition = receive(&receiver);
        assert!(matches!(
            definition.outcome,
            Ok(Outcome::Definition(ref result)) if result.locations.len() == 1
        ));
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
    }

    struct Gate {
        entered: Receiver<()>,
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
                }),
                Gate {
                    entered: entered_rx,
                    gate,
                },
            )
        }
    }

    impl Backend for GatedBackend {
        #[allow(clippy::significant_drop_tightening)] // the lock guards the wait
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
}
