//! Native process supervision for the pinned engine.
//!
//! The supervisor is the single lifecycle authority. It builds the argv from
//! [`PinnedEngine::launch_plan`] exactly, verifies the executable SHA-256
//! against the pinned identity, spawns the executable directly with piped
//! `stdin`/`stdout`/`stderr` (no shell, no Node, no `--runExternalCode`),
//! drains `stderr` into a bounded ring buffer, and moves frames through three
//! dedicated threads:
//!
//! - an `stdin` writer thread fed by a bounded queue, so
//!   [`ProcessSession::send`] never blocks on a full pipe;
//! - an `stdout` reader thread that parses frames into a bounded queue whose
//!   total payload is capped at 96 MiB;
//! - an `stderr` drain thread.
//!
//! The child environment is cleared and rebuilt with only `LANG=C` and
//! `LC_ALL=C`; `cwd` is the validated empty sandbox root. The sandbox should
//! also be non-writable by the engine process as defence in depth (F0-17);
//! `P3-N` verifies it is empty and not a symlink but does not change its mode.
//!
//! A blocking read can only be released by process death. Therefore
//! [`ProcessSession::recv_until`] poisons the session on timeout: every later
//! `send`/`recv` returns [`NativeError::Unavailable`] with
//! [`UnavailableReason::SessionPoisoned`], and the caller must abort or shut
//! down the session. [`ProcessSession::poison`] and
//! [`ProcessSession::abort_for_timeout`] make that state explicit.
//!
//! Beginning a session does **not** perform the F0 handshake or probe: after
//! [`EngineSupervisor::spawn`] the lifecycle is [`Lifecycle::Starting`], and
//! only an explicit [`EngineSupervisor::mark_ready`] after a successful probe
//! may move it to [`Lifecycle::Ready`]. `mark_ready` also re-checks the child
//! and fails if it already exited. There is no automatic restart loop in
//! `P3-N`; [`RestartPolicy`] is metadata only.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::{LaunchError, LaunchPlan, PinnedEngine};
use crate::native::framing::{
    DEFAULT_FRAME_LIMITS, Frame, FrameError, FrameLimits, FrameReader, FrameRole, FrameWriter,
};
use crate::native::{NativeError, UnavailableReason};

/// Handshake/probe deadline (contract section 3.1).
pub const HANDSHAKE_DEADLINE_MS: u64 = 10_000;

/// Filesystem callback deadline.
pub const CALLBACK_DEADLINE_MS: u64 = 2_000;

/// Deadline for the `close stdin -> wait -> SIGKILL -> reap` shutdown path.
pub const SHUTDOWN_DEADLINE_MS: u64 = 5_000;

/// Deadline for releasing one lease or snapshot.
pub const RELEASE_DEADLINE_MS: u64 = 1_000;

/// Bytes retained in the `stderr` ring buffer.
pub const STDERR_RING_BYTES: usize = 64 * 1024;

/// Bytes exposed from the ring buffer for exit diagnostics.
pub const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// Maximum bytes retained from one `stderr` line before it is truncated.
const STDERR_LINE_BYTES: usize = 8 * 1024;

/// Maximum start attempts including the initial one (contract section 6.6).
pub const MAX_START_ATTEMPTS: u32 = 3;

/// Restart attempts allowed for a protocol or version mismatch.
pub const PROTOCOL_RESTART_LIMIT: u32 = 1;

/// Backoff before the first, second and third start attempt after a failure.
pub const RESTART_BACKOFF_MS: [u64; 3] = [100, 500, 2_000];

/// Completed frames buffered between the reader thread and the session.
const INBOUND_CHANNEL_CAPACITY: usize = 8;

/// Aggregate bytes that may sit in the inbound frame queue.
///
/// This is larger than one maximum response (64 MiB) so a single frame always
/// fits, but it stops a child from queueing eight 64 MiB frames. The channel
/// count and this byte budget are both enforced.
const MAX_INBOUND_QUEUED_BYTES: usize = 96 * 1024 * 1024;

/// Frames buffered between the session and the `stdin` writer thread.
const OUTBOUND_CHANNEL_CAPACITY: usize = 4;

/// Maximum time `send` waits for space in the outbound queue.
const OUTBOUND_ENQUEUE_DEADLINE_MS: u64 = 1_000;

/// Bytes read per hashing pass during executable verification.
const EXECUTABLE_HASH_CHUNK_BYTES: usize = 64 * 1024;

/// Poll interval while waiting for a thread to finish during shutdown.
const THREAD_JOIN_POLL_MS: u64 = 5;

/// Environment variables kept for the child.
const CHILD_ENVIRONMENT: [(&str, &str); 2] = [("LANG", "C"), ("LC_ALL", "C")];

/// Lifecycle marker owned by the supervisor.
///
/// This is the only lifecycle authority. `wxml-backend-tsgo`'s
/// `BackendState` is a projection of this enum, not a second source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// No child process exists.
    NotStarted,
    /// The child is running but the handshake/probe has not passed.
    Starting,
    /// Probe passed and the session can serve requests.
    Ready,
    /// The child exited or the probe failed.
    Failed,
    /// Shutdown completed.
    Stopped,
}

/// Absolute point in time after which an operation must be abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    at: Instant,
    timeout: Duration,
}

impl Deadline {
    /// Handshake/probe deadline (10 s).
    #[must_use]
    pub fn handshake() -> Self {
        Self::after(Duration::from_millis(HANDSHAKE_DEADLINE_MS))
    }

    /// Filesystem callback deadline (2 s).
    #[must_use]
    pub fn callback() -> Self {
        Self::after(Duration::from_millis(CALLBACK_DEADLINE_MS))
    }

    /// Default request deadline from the frozen frame budgets.
    #[must_use]
    pub fn request(limits: FrameLimits) -> Self {
        Self::after(Duration::from_millis(limits.request_deadline_ms))
    }

    /// Snapshot create/update deadline from the frozen frame budgets.
    #[must_use]
    pub fn snapshot(limits: FrameLimits) -> Self {
        Self::after(Duration::from_millis(limits.snapshot_deadline_ms))
    }

    /// Lease/snapshot release deadline (1 s).
    #[must_use]
    pub fn release() -> Self {
        Self::after(Duration::from_millis(RELEASE_DEADLINE_MS))
    }

    /// Starts a deadline of `timeout` from now.
    ///
    /// Overflow of `Instant` arithmetic is clamped to 24 hours instead of
    /// panicking; real deadlines are seconds.
    #[must_use]
    pub fn after(timeout: Duration) -> Self {
        let now = Instant::now();
        let at = now
            .checked_add(timeout)
            .or_else(|| now.checked_add(Duration::from_secs(24 * 60 * 60)))
            .unwrap_or(now);
        Self { at, timeout }
    }

    /// Returns the configured timeout used for error reporting.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// Returns the timeout in whole milliseconds, saturating at `u64::MAX`.
    #[must_use]
    pub fn timeout_ms(self) -> u64 {
        u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX)
    }

    /// Returns the time left, or zero once expired.
    #[must_use]
    pub fn remaining(self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }

    /// Returns whether the deadline has passed.
    #[must_use]
    pub fn is_expired(self) -> bool {
        Instant::now() >= self.at
    }

    /// Returns the absolute instant the deadline expires.
    #[must_use]
    pub const fn at(self) -> Instant {
        self.at
    }
}

/// Frozen restart metadata.
///
/// `P3-N` performs no automatic restart: a session layer must consult this
/// policy, decide to restart and call [`EngineSupervisor::spawn`] again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Maximum start attempts including the initial one.
    pub max_start_attempts: u32,
    /// Milliseconds to wait before retry 1, 2 and 3.
    pub start_backoff_ms: [u64; 3],
    /// Restarts allowed for a protocol or version mismatch.
    pub protocol_restarts: u32,
}

impl RestartPolicy {
    /// Policy frozen by F1-09.
    pub const PINNED: Self = Self {
        max_start_attempts: MAX_START_ATTEMPTS,
        start_backoff_ms: RESTART_BACKOFF_MS,
        protocol_restarts: PROTOCOL_RESTART_LIMIT,
    };

    /// Returns the backoff before retry `attempt` (1-based).
    ///
    /// Returns `None` once the bounded retry budget is exhausted.
    #[must_use]
    pub fn backoff(self, attempt: u32) -> Option<Duration> {
        let index = usize::try_from(attempt).ok()?.checked_sub(1)?;
        self.start_backoff_ms
            .get(index)
            .copied()
            .map(Duration::from_millis)
    }
}

/// Aggregate byte accounting for frames waiting in the inbound queue.
#[derive(Debug)]
struct InboundBudget {
    queued: AtomicUsize,
    limit: usize,
    closed: AtomicBool,
}

impl InboundBudget {
    const fn new(limit: usize) -> Self {
        Self {
            queued: AtomicUsize::new(0),
            limit,
            closed: AtomicBool::new(false),
        }
    }

    /// Returns the aggregate byte limit.
    const fn limit(&self) -> usize {
        self.limit
    }

    /// Reserves `bytes`, returning false when the aggregate budget is full.
    fn try_reserve(&self, bytes: usize) -> bool {
        let mut current = self.queued.load(Ordering::SeqCst);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return false;
            };
            if next > self.limit {
                return false;
            }
            match self
                .queued
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// Releases a reservation after the frame was consumed or dropped.
    fn release(&self, bytes: usize) {
        let mut current = self.queued.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_sub(bytes);
            match self
                .queued
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    #[cfg(test)]
    fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// Bounded `stderr` ring buffer.
#[derive(Debug)]
struct StderrRing {
    bytes: VecDeque<u8>,
    capacity: usize,
    discarded: u64,
    line_bytes: usize,
}

impl StderrRing {
    const fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity,
            discarded: 0,
            line_bytes: 0,
        }
    }

    /// Appends a chunk, dropping the oldest bytes and truncating long lines.
    fn push(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if byte == b'\n' {
                self.line_bytes = 0;
            } else {
                self.line_bytes = self.line_bytes.saturating_add(1);
                if self.line_bytes > STDERR_LINE_BYTES {
                    self.discarded = self.discarded.saturating_add(1);
                    continue;
                }
            }

            if self.capacity == 0 {
                self.discarded = self.discarded.saturating_add(1);
                continue;
            }
            if self.bytes.len() >= self.capacity {
                self.bytes.pop_front();
                self.discarded = self.discarded.saturating_add(1);
            }
            self.bytes.push_back(byte);
        }
    }

    /// Returns the last `limit` bytes as sanitized, lossy UTF-8.
    fn tail(&self, limit: usize) -> String {
        let start = self.bytes.len().saturating_sub(limit);
        let collected: Vec<u8> = self.bytes.iter().skip(start).copied().collect();
        sanitize_text(&String::from_utf8_lossy(&collected))
    }

    /// Returns how many bytes were dropped by eviction or line truncation.
    const fn discarded(&self) -> u64 {
        self.discarded
    }
}

/// Replaces control characters except newline and tab with spaces.
fn sanitize_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() && character != '\n' && character != '\t' {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// Replaces every control character, including newlines, for single-line text.
fn sanitize_single_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// Framing or pipe failure observed by the reader thread.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("native transport failed: {message}")]
pub struct TransportFault {
    message: String,
}

impl TransportFault {
    /// Creates a bounded, single-line fault summary.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        let message = sanitize_single_line(&message.into());
        let bounded: String = message.chars().take(512).collect();
        Self { message: bounded }
    }

    /// Returns the sanitized summary.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<FrameError> for TransportFault {
    fn from(error: FrameError) -> Self {
        Self::new(error.to_string())
    }
}

/// Event produced by the `stdout` reader thread.
#[derive(Debug)]
pub enum InboundEvent {
    /// One complete frame body from the engine.
    Frame(Frame),
    /// Framing or pipe failure; the session must be terminated.
    Fault(TransportFault),
    /// `stdout` reached EOF without a framing error.
    Closed,
}

/// One frame waiting for the `stdin` writer thread.
#[derive(Clone, PartialEq, Eq)]
struct OutboundFrame {
    role: FrameRole,
    body: Vec<u8>,
}

impl std::fmt::Debug for OutboundFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutboundFrame")
            .field("role", &self.role)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Validated command line for one engine process.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpawnSpec {
    program: PathBuf,
    args: Vec<String>,
    sandbox_root: PathBuf,
    environment: Vec<(&'static str, &'static str)>,
}

impl SpawnSpec {
    /// Builds the frozen argv for `install_root`/`sandbox_root`.
    ///
    /// Artifact verification is a separate preflight in
    /// [`EngineSupervisor::spawn`]; this function only validates paths and
    /// constructs the command.
    fn for_engine(
        engine: &PinnedEngine,
        install_root: &Path,
        sandbox_root: &Path,
    ) -> Result<Self, SpawnError> {
        validate_install_root(install_root)?;
        validate_sandbox_root(engine, sandbox_root)?;

        let root = sandbox_root.to_str().ok_or(SpawnError::SandboxNotUtf8)?;
        let plan = engine.launch_plan(root)?;

        Ok(Self {
            program: install_root.join(plan.executable),
            args: plan.args,
            sandbox_root: sandbox_root.to_path_buf(),
            environment: CHILD_ENVIRONMENT.to_vec(),
        })
    }

    /// Builds the process command with piped stdio and a cleared environment.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command.current_dir(&self.sandbox_root);
        command.env_clear();
        for &(key, value) in &self.environment {
            command.env(key, value);
        }
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command
    }
}

/// Failure to prepare, verify or spawn a native process.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// The sandbox root failed the frozen launch-plan validation.
    #[error("invalid sandbox root: {0}")]
    SandboxRoot(#[from] LaunchError),
    /// The sandbox root is not valid UTF-8, so it cannot be passed as `--cwd`.
    #[error("sandbox root must be valid UTF-8")]
    SandboxNotUtf8,
    /// The sandbox root does not exist.
    #[error("sandbox root does not exist")]
    SandboxMissing,
    /// The sandbox root is not a directory.
    #[error("sandbox root must be a directory")]
    SandboxNotDirectory,
    /// The sandbox root is a symlink instead of a backend-owned directory.
    #[error("sandbox root must not be a symlink")]
    SandboxSymlink,
    /// The sandbox root already contains entries.
    #[error("sandbox root must be empty")]
    SandboxNotEmpty,
    /// The install root is not an absolute, normalized path.
    #[error("install root must be an absolute, normalized path")]
    InvalidInstallRoot,
    /// The pinned executable does not exist.
    #[error("pinned engine executable does not exist")]
    ExecutableMissing,
    /// The pinned executable is a symlink.
    #[error("pinned engine executable must not be a symlink")]
    ExecutableSymlink,
    /// The pinned executable is not a regular file.
    #[error("pinned engine executable must be a regular file")]
    ExecutableNotFile,
    /// The pinned executable is missing its executable bit.
    #[error("pinned engine executable is not executable")]
    ExecutableNotExecutable,
    /// The executable hash does not match the pinned identity.
    #[error("engine executable hash does not match the pinned identity")]
    ExecutableMismatch,
    /// The executable could not be opened or read.
    #[error("failed to read the engine executable: {0}")]
    ExecutableIo(#[source] io::Error),
    /// The engine executable could not be spawned.
    #[error("failed to spawn the native engine: {0}")]
    Spawn(#[source] io::Error),
    /// An I/O thread could not be started.
    #[error("failed to start an engine I/O thread: {0}")]
    Thread(#[source] io::Error),
}

/// Rejects install roots that cannot name a pinned installation tree.
fn validate_install_root(root: &Path) -> Result<(), SpawnError> {
    if !root.is_absolute() {
        return Err(SpawnError::InvalidInstallRoot);
    }
    if root
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(SpawnError::InvalidInstallRoot);
    }
    Ok(())
}

/// Requires an existing, empty, non-symlink sandbox directory.
fn validate_sandbox_root(engine: &PinnedEngine, root: &Path) -> Result<(), SpawnError> {
    let text = root.to_str().ok_or(SpawnError::SandboxNotUtf8)?;
    engine.launch_plan(text)?;

    let metadata = std::fs::symlink_metadata(root).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            SpawnError::SandboxMissing
        } else {
            SpawnError::Spawn(error)
        }
    })?;

    if metadata.file_type().is_symlink() {
        return Err(SpawnError::SandboxSymlink);
    }
    if !metadata.is_dir() {
        return Err(SpawnError::SandboxNotDirectory);
    }

    let mut entries = std::fs::read_dir(root).map_err(SpawnError::Spawn)?;
    let first = entries.next().transpose().map_err(SpawnError::Spawn)?;
    if first.is_some() {
        return Err(SpawnError::SandboxNotEmpty);
    }

    Ok(())
}

/// Verifies that `path` is the pinned executable by size-independent SHA-256.
///
/// The path must be a regular, non-symlink file with an executable bit on
/// Unix, and its streamed SHA-256 must equal `expected_sha256`. Reading the
/// whole binary (about 24 MB for the pinned artifact) happens before spawn and
/// before any project content is sent.
fn verify_executable(path: &Path, expected_sha256: &str) -> Result<(), SpawnError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            SpawnError::ExecutableMissing
        } else {
            SpawnError::ExecutableIo(error)
        }
    })?;

    if metadata.file_type().is_symlink() {
        return Err(SpawnError::ExecutableSymlink);
    }
    if !metadata.is_file() {
        return Err(SpawnError::ExecutableNotFile);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(SpawnError::ExecutableNotExecutable);
        }
    }

    let mut file = File::open(path).map_err(SpawnError::ExecutableIo)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; EXECUTABLE_HASH_CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(SpawnError::ExecutableIo)?;
        if read == 0 {
            break;
        }
        let Some(chunk) = buffer.get(..read) else {
            return Err(SpawnError::ExecutableNotFile);
        };
        hasher.update(chunk);
    }

    if hex_lower(&hasher.finalize()) != expected_sha256 {
        return Err(SpawnError::ExecutableMismatch);
    }
    Ok(())
}

/// Encodes bytes as lowercase hexadecimal without an extra dependency.
fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        if let (Some(&high), Some(&low)) = (
            DIGITS.get(usize::from(byte >> 4)),
            DIGITS.get(usize::from(byte & 0x0f)),
        ) {
            encoded.push(char::from(high));
            encoded.push(char::from(low));
        }
    }
    encoded
}

/// Supervisor failure outside the frame/router layer.
#[derive(Debug, Error)]
pub enum SupervisorError {
    /// The process could not be prepared, verified or spawned.
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// A session is already attached.
    #[error("native process is already running")]
    AlreadyRunning,
    /// No session is attached.
    #[error("native process is not running")]
    NotRunning,
    /// The lifecycle is not in [`Lifecycle::Starting`].
    #[error("native process is not in the starting state")]
    NotStarting,
    /// The child exited before the handshake/probe completed.
    #[error("native process exited before the probe completed")]
    ProcessExited,
    /// A session operation failed.
    #[error(transparent)]
    Native(#[from] NativeError),
}

impl From<SupervisorError> for NativeError {
    fn from(error: SupervisorError) -> Self {
        match error {
            SupervisorError::Native(native) => native,
            SupervisorError::Spawn(_)
            | SupervisorError::AlreadyRunning
            | SupervisorError::NotRunning
            | SupervisorError::NotStarting
            | SupervisorError::ProcessExited => Self::Unavailable {
                reason: UnavailableReason::ProcessExited,
            },
        }
    }
}

/// Result of an explicit shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    exit_status: Option<ExitStatus>,
    forced_kill: bool,
    threads_detached: bool,
    stderr_tail: String,
    stderr_discarded_bytes: u64,
}

impl ShutdownReport {
    /// Returns the exit status when the child could be reaped.
    #[must_use]
    pub const fn exit_status(&self) -> Option<ExitStatus> {
        self.exit_status
    }

    /// Returns whether the graceful wait expired and `SIGKILL` was required.
    #[must_use]
    pub const fn forced_kill(&self) -> bool {
        self.forced_kill
    }

    /// Returns whether at least one reader/writer thread outlived the deadline.
    ///
    /// This can only happen when a descendant process keeps a pipe open after
    /// the direct child died; the thread is detached so shutdown stays bounded.
    #[must_use]
    pub const fn threads_detached(&self) -> bool {
        self.threads_detached
    }

    /// Returns the sanitized last [`STDERR_TAIL_BYTES`] of `stderr`.
    #[must_use]
    pub fn stderr_tail(&self) -> &str {
        &self.stderr_tail
    }

    /// Returns how many `stderr` bytes were dropped by the bounded buffer.
    #[must_use]
    pub const fn stderr_discarded_bytes(&self) -> u64 {
        self.stderr_discarded_bytes
    }
}

/// One live child process plus its frame channels.
#[derive(Debug)]
pub struct ProcessSession {
    child: Option<Child>,
    outbound: Option<SyncSender<OutboundFrame>>,
    frames: Option<Receiver<InboundEvent>>,
    stderr: Arc<Mutex<StderrRing>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
    closed: Arc<AtomicBool>,
    stdout_done: Arc<AtomicBool>,
    stderr_done: Arc<AtomicBool>,
    writer_done: Arc<AtomicBool>,
    writer_abort: Arc<AtomicBool>,
    stdin_failed: Arc<AtomicBool>,
    inbound: Arc<InboundBudget>,
    limits: FrameLimits,
    poisoned: bool,
}

impl ProcessSession {
    /// Spawns `spec` and starts the writer/reader threads.
    fn spawn(spec: &SpawnSpec, limits: FrameLimits) -> Result<Self, SpawnError> {
        let mut child = spec.command().spawn().map_err(SpawnError::Spawn)?;

        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            reap(&mut child);
            return Err(SpawnError::Spawn(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "engine stdio pipe is missing",
            )));
        };

        let (outbound, outbound_receiver) = sync_channel(OUTBOUND_CHANNEL_CAPACITY);
        let (inbound_sender, inbound_receiver) = sync_channel(INBOUND_CHANNEL_CAPACITY);
        let inbound_budget = Arc::new(InboundBudget::new(MAX_INBOUND_QUEUED_BYTES));
        let closed = Arc::new(AtomicBool::new(false));
        let stdout_done = Arc::new(AtomicBool::new(false));
        let stderr_done = Arc::new(AtomicBool::new(false));
        let writer_done = Arc::new(AtomicBool::new(false));
        let writer_abort = Arc::new(AtomicBool::new(false));
        let stdin_failed = Arc::new(AtomicBool::new(false));
        let stderr_ring = Arc::new(Mutex::new(StderrRing::new(STDERR_RING_BYTES)));

        let writer_thread = match spawn_writer_thread(
            stdin,
            limits,
            outbound_receiver,
            Arc::clone(&writer_abort),
            Arc::clone(&stdin_failed),
            Arc::clone(&writer_done),
        ) {
            Ok(handle) => handle,
            Err(error) => {
                reap(&mut child);
                return Err(SpawnError::Thread(error));
            }
        };

        let stdout_thread = match spawn_stdout_thread(
            stdout,
            limits,
            inbound_sender,
            Arc::clone(&closed),
            Arc::clone(&stdout_done),
            Arc::clone(&inbound_budget),
        ) {
            Ok(handle) => handle,
            Err(error) => {
                writer_abort.store(true, Ordering::SeqCst);
                inbound_budget.close();
                reap(&mut child);
                drop(writer_thread);
                return Err(SpawnError::Thread(error));
            }
        };

        let stderr_thread =
            match spawn_stderr_thread(stderr, Arc::clone(&stderr_ring), Arc::clone(&stderr_done)) {
                Ok(handle) => handle,
                Err(error) => {
                    writer_abort.store(true, Ordering::SeqCst);
                    inbound_budget.close();
                    reap(&mut child);
                    drop(writer_thread);
                    drop(stdout_thread);
                    return Err(SpawnError::Thread(error));
                }
            };

        Ok(Self {
            child: Some(child),
            outbound: Some(outbound),
            frames: Some(inbound_receiver),
            stderr: stderr_ring,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            writer_thread: Some(writer_thread),
            closed,
            stdout_done,
            stderr_done,
            writer_done,
            writer_abort,
            stdin_failed,
            inbound: inbound_budget,
            limits,
            poisoned: false,
        })
    }

    /// Writes one already encoded frame to the engine.
    ///
    /// The frame is copied into a bounded outbound queue; this call waits at
    /// most one second for space and poisons the session
    /// if the queue stays full.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Protocol`] for an inbound role or a body above
    /// the role budget, [`NativeError::Timeout`] when the queue stays full,
    /// and [`NativeError::Unavailable`] when the session is poisoned or the
    /// writer is gone.
    pub fn send(&mut self, role: FrameRole, body: &[u8]) -> Result<(), NativeError> {
        self.send_until(
            role,
            body,
            Deadline::after(Duration::from_millis(OUTBOUND_ENQUEUE_DEADLINE_MS)),
        )
    }

    /// Like [`Self::send`] with a caller-supplied enqueue deadline.
    ///
    /// # Errors
    ///
    /// See [`Self::send`]; a timeout poisons the session.
    pub fn send_until(
        &mut self,
        role: FrameRole,
        body: &[u8],
        deadline: Deadline,
    ) -> Result<(), NativeError> {
        if self.poisoned {
            return Err(poisoned_error());
        }
        if self.stdin_failed.load(Ordering::SeqCst) {
            return Err(process_exited_error());
        }
        if !role.is_outbound() {
            return Err(NativeError::Protocol(
                "frame role is not written by the client".to_owned(),
            ));
        }

        let limit = self.limits.body_limit(role);
        if body.len() > limit {
            return Err(NativeError::Protocol(format!(
                "frame body of {} bytes exceeds the {limit}-byte budget",
                body.len()
            )));
        }

        let Some(sender) = self.outbound.clone() else {
            return Err(process_exited_error());
        };
        let mut frame = OutboundFrame {
            role,
            body: body.to_vec(),
        };

        loop {
            match sender.try_send(frame) {
                Ok(()) => {
                    if self.stdin_failed.load(Ordering::SeqCst) {
                        return Err(process_exited_error());
                    }
                    return Ok(());
                }
                Err(TrySendError::Full(returned)) => {
                    frame = returned;
                    if deadline.is_expired() {
                        self.poison();
                        return Err(NativeError::Timeout(deadline.timeout_ms()));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.stdin_failed.store(true, Ordering::SeqCst);
                    return Err(process_exited_error());
                }
            }
        }
    }

    /// Waits for the next inbound event until `deadline`.
    ///
    /// A timeout cannot interrupt a blocked `read`; it poisons the session so
    /// every later operation fails with `SessionPoisoned`. The blocked reader
    /// thread is released only when the process dies, so callers must abort or
    /// shut the session down after a timeout.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Timeout`] on expiry and
    /// [`NativeError::Unavailable`] when the session is poisoned or the reader
    /// thread is gone.
    pub fn recv_until(&mut self, deadline: Deadline) -> Result<InboundEvent, NativeError> {
        if self.poisoned {
            return Err(poisoned_error());
        }
        if deadline.is_expired() {
            self.poison();
            return Err(NativeError::Timeout(deadline.timeout_ms()));
        }

        let outcome = {
            let Some(receiver) = self.frames.as_ref() else {
                return Err(process_exited_error());
            };
            receiver.recv_timeout(deadline.remaining())
        };

        match outcome {
            Ok(event) => {
                if let InboundEvent::Frame(frame) = &event {
                    self.inbound.release(frame.len());
                }
                if matches!(event, InboundEvent::Fault(_) | InboundEvent::Closed) {
                    self.poison();
                }
                Ok(event)
            }
            Err(RecvTimeoutError::Timeout) => {
                self.poison();
                Err(NativeError::Timeout(deadline.timeout_ms()))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.poison();
                Err(process_exited_error())
            }
        }
    }

    /// Marks the session unusable without touching the process.
    ///
    /// The caller must still drop or shut down the session to release the
    /// child and any blocked reader.
    pub const fn poison(&mut self) {
        self.poisoned = true;
    }

    /// Returns whether the session was poisoned by a timeout or terminal event.
    #[must_use]
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Reports the child's exit status without blocking.
    ///
    /// # Errors
    ///
    /// Returns [`NativeError::Unavailable`] when the status cannot be read.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, NativeError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        child.try_wait().map_err(|_| process_exited_error())
    }

    /// Returns whether the `stdout` reader observed EOF or a framing fault.
    #[must_use]
    pub fn stdout_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Returns the sanitized last [`STDERR_TAIL_BYTES`] of `stderr`.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        lock_ring(&self.stderr).tail(STDERR_TAIL_BYTES)
    }

    /// Returns how many `stderr` bytes were dropped by the bounded buffer.
    #[must_use]
    pub fn stderr_discarded_bytes(&self) -> u64 {
        lock_ring(&self.stderr).discarded()
    }

    /// Poisons the session and shuts it down with the default deadline.
    #[must_use]
    pub fn abort_for_timeout(self) -> ShutdownReport {
        self.abort_for_timeout_with(Duration::from_millis(SHUTDOWN_DEADLINE_MS))
    }

    /// Poisons the session and shuts it down with an explicit deadline.
    #[must_use]
    pub fn abort_for_timeout_with(mut self, timeout: Duration) -> ShutdownReport {
        self.poison();
        self.shutdown_with_timeout(timeout)
    }

    /// Closes `stdin`, waits up to [`SHUTDOWN_DEADLINE_MS`], then kills and
    /// reaps the child; reader/writer threads get the same deadline to finish.
    #[must_use]
    pub fn shutdown(self) -> ShutdownReport {
        self.shutdown_with_timeout(Duration::from_millis(SHUTDOWN_DEADLINE_MS))
    }

    /// Explicit shutdown with a caller-supplied deadline for both the graceful
    /// wait and the thread joins.
    ///
    /// Threads still running after the deadline are detached, which can only
    /// happen when a descendant process keeps a pipe open.
    #[must_use]
    pub fn shutdown_with_timeout(mut self, timeout: Duration) -> ShutdownReport {
        self.poison();
        // Contract order: close stdin -> wait -> SIGKILL -> reap.
        self.writer_abort.store(true, Ordering::SeqCst);
        self.inbound.close();
        self.outbound.take();
        self.frames.take();

        let deadline = Deadline::after(timeout);
        let mut exit_status = None;
        let mut forced_kill = false;

        if let Some(child) = self.child.as_mut() {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        exit_status = Some(status);
                        break;
                    }
                    Ok(None) => {
                        if deadline.is_expired() {
                            forced_kill = true;
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        forced_kill = true;
                        break;
                    }
                }
            }

            if forced_kill {
                let _ = child.kill();
                exit_status = child.wait().ok();
            }
        }

        // After reap the direct child's pipe ends are closed. A descendant can
        // still keep them open, so joins are bounded and detach on timeout.
        let join_deadline = Deadline::after(timeout);
        let mut threads_detached = false;
        threads_detached |=
            !join_with_deadline(self.stdout_thread.take(), &self.stdout_done, join_deadline);
        threads_detached |=
            !join_with_deadline(self.stderr_thread.take(), &self.stderr_done, join_deadline);
        threads_detached |=
            !join_with_deadline(self.writer_thread.take(), &self.writer_done, join_deadline);

        let stderr_tail = self.stderr_tail();
        let stderr_discarded_bytes = self.stderr_discarded_bytes();
        self.child = None;

        ShutdownReport {
            exit_status,
            forced_kill,
            threads_detached,
            stderr_tail,
            stderr_discarded_bytes,
        }
    }
}

impl Drop for ProcessSession {
    fn drop(&mut self) {
        self.poison();
        self.writer_abort.store(true, Ordering::SeqCst);
        self.inbound.close();
        self.outbound.take();
        self.frames.take();

        if let Some(mut child) = self.child.take() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        // Threads are detached: after `kill` plus `wait` their pipes reach EOF
        // and they exit without blocking the drop path. A descendant that
        // keeps a pipe open merely delays its own reader thread.
    }
}

/// Kills and reaps a child after a failed spawn sequence.
fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_writer_thread(
    stdin: ChildStdin,
    limits: FrameLimits,
    receiver: Receiver<OutboundFrame>,
    abort: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tsgo-stdin".to_owned())
        .spawn(move || writer_loop(stdin, limits, &receiver, &abort, &failed, &done))
}

fn writer_loop(
    stdin: ChildStdin,
    limits: FrameLimits,
    receiver: &Receiver<OutboundFrame>,
    abort: &AtomicBool,
    failed: &AtomicBool,
    done: &AtomicBool,
) {
    let mut writer = FrameWriter::new(stdin, limits);
    while let Ok(frame) = receiver.recv() {
        if abort.load(Ordering::SeqCst) {
            break;
        }
        if writer.write_frame(frame.role, &frame.body).is_err() {
            failed.store(true, Ordering::SeqCst);
            break;
        }
    }
    done.store(true, Ordering::SeqCst);
}

fn spawn_stdout_thread(
    stdout: ChildStdout,
    limits: FrameLimits,
    sender: SyncSender<InboundEvent>,
    closed: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    budget: Arc<InboundBudget>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tsgo-stdout".to_owned())
        .spawn(move || stdout_loop(stdout, limits, &sender, &closed, &done, &budget))
}

fn spawn_stderr_thread(
    stderr: ChildStderr,
    ring: Arc<Mutex<StderrRing>>,
    done: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tsgo-stderr".to_owned())
        .spawn(move || stderr_loop(stderr, &ring, &done))
}

/// Parses frames from `reader` until EOF, a fault, or a closed consumer.
fn stdout_loop<R: Read>(
    reader: R,
    limits: FrameLimits,
    sender: &SyncSender<InboundEvent>,
    closed: &AtomicBool,
    done: &AtomicBool,
    budget: &InboundBudget,
) {
    let mut reader = FrameReader::new(reader, limits);

    loop {
        match reader.read_frame() {
            Ok(Some(frame)) => {
                let bytes = frame.len();
                if bytes > budget.limit() {
                    closed.store(true, Ordering::SeqCst);
                    let _ = sender.send(InboundEvent::Fault(TransportFault::new(
                        "frame exceeds the inbound queue budget",
                    )));
                    break;
                }
                if !reserve_inbound(budget, bytes) {
                    break;
                }
                if sender.send(InboundEvent::Frame(frame)).is_err() {
                    budget.release(bytes);
                    break;
                }
            }
            Ok(None) => {
                closed.store(true, Ordering::SeqCst);
                let _ = sender.send(InboundEvent::Closed);
                break;
            }
            Err(error) => {
                closed.store(true, Ordering::SeqCst);
                let _ = sender.send(InboundEvent::Fault(TransportFault::from(error)));
                break;
            }
        }
    }

    done.store(true, Ordering::SeqCst);
}

/// Waits for aggregate queue space, returning false when the session closes.
fn reserve_inbound(budget: &InboundBudget, bytes: usize) -> bool {
    loop {
        if budget.try_reserve(bytes) {
            return true;
        }
        if budget.is_closed() {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn stderr_loop(mut stderr: ChildStderr, ring: &Mutex<StderrRing>, done: &AtomicBool) {
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if let Some(chunk) = buffer.get(..read) {
                    lock_ring(ring).push(chunk);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    done.store(true, Ordering::SeqCst);
}

/// Waits for a thread to finish until `deadline`, detaching on timeout.
fn join_with_deadline(
    handle: Option<JoinHandle<()>>,
    done: &AtomicBool,
    deadline: Deadline,
) -> bool {
    let Some(handle) = handle else {
        return true;
    };
    while !done.load(Ordering::SeqCst) {
        if deadline.is_expired() {
            drop(handle);
            return false;
        }
        thread::sleep(Duration::from_millis(THREAD_JOIN_POLL_MS));
    }
    let _ = handle.join();
    true
}

/// Locks the ring buffer, recovering from a poisoned lock instead of panicking.
fn lock_ring(ring: &Mutex<StderrRing>) -> MutexGuard<'_, StderrRing> {
    ring.lock().unwrap_or_else(PoisonError::into_inner)
}

const fn process_exited_error() -> NativeError {
    NativeError::Unavailable {
        reason: UnavailableReason::ProcessExited,
    }
}

const fn poisoned_error() -> NativeError {
    NativeError::Unavailable {
        reason: UnavailableReason::SessionPoisoned,
    }
}

/// Lifecycle authority for the pinned native engine.
#[derive(Debug)]
pub struct EngineSupervisor {
    engine: PinnedEngine,
    lifecycle: Lifecycle,
    session: Option<ProcessSession>,
    restart_policy: RestartPolicy,
}

impl EngineSupervisor {
    /// Creates the supervisor without starting anything.
    #[must_use]
    pub const fn new(engine: PinnedEngine) -> Self {
        Self {
            engine,
            lifecycle: Lifecycle::NotStarted,
            session: None,
            restart_policy: RestartPolicy::PINNED,
        }
    }

    /// Returns the pinned identity the session must verify.
    #[must_use]
    pub const fn engine(&self) -> &PinnedEngine {
        &self.engine
    }

    /// Returns the current lifecycle marker.
    #[must_use]
    pub const fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// Returns the frozen restart metadata.
    #[must_use]
    pub const fn restart_policy(&self) -> RestartPolicy {
        self.restart_policy
    }

    /// Returns the frozen launch plan for review; this does not spawn.
    ///
    /// # Errors
    ///
    /// Propagates [`LaunchError`] for an invalid sandbox root.
    pub fn launch_plan(&self, sandbox_root: &str) -> Result<LaunchPlan, LaunchError> {
        self.engine.launch_plan(sandbox_root)
    }

    /// Verifies the pinned executable and spawns it with the frozen argv.
    ///
    /// The sandbox root must be an existing, empty, non-symlink directory. The
    /// executable at `<install_root>/engine/lib/tsc` must be a regular,
    /// executable, non-symlink file whose SHA-256 matches the pinned identity.
    /// After a successful spawn the lifecycle is [`Lifecycle::Starting`]; the
    /// caller must run the handshake/probe and then call [`Self::mark_ready`].
    ///
    /// # Errors
    ///
    /// Returns [`SupervisorError`] when a session already exists, the roots or
    /// artifact are invalid, or the process/threads cannot be started.
    pub fn spawn(
        &mut self,
        install_root: &Path,
        sandbox_root: &Path,
    ) -> Result<(), SupervisorError> {
        let spec = SpawnSpec::for_engine(&self.engine, install_root, sandbox_root)?;
        verify_executable(&spec.program, self.engine.executable_sha256())?;
        self.start(&spec)
    }

    /// Starts a session for an already validated spec.
    fn start(&mut self, spec: &SpawnSpec) -> Result<(), SupervisorError> {
        if self.session.is_some() {
            return Err(SupervisorError::AlreadyRunning);
        }

        let session = ProcessSession::spawn(spec, DEFAULT_FRAME_LIMITS)?;
        self.session = Some(session);
        self.lifecycle = Lifecycle::Starting;
        Ok(())
    }

    /// Returns the live session, if any.
    #[must_use]
    pub const fn session(&mut self) -> Option<&mut ProcessSession> {
        self.session.as_mut()
    }

    /// Marks the handshake/probe as passed.
    ///
    /// Re-checks the child first: if it already exited, the lifecycle becomes
    /// [`Lifecycle::Failed`] and this returns
    /// [`SupervisorError::ProcessExited`] instead of reporting a ready
    /// session. A failed lifecycle can never be overwritten by `mark_ready`.
    ///
    /// # Errors
    ///
    /// Requires a running session in [`Lifecycle::Starting`] whose child has
    /// not exited.
    pub fn mark_ready(&mut self) -> Result<(), SupervisorError> {
        if self.lifecycle != Lifecycle::Starting {
            return Err(SupervisorError::NotStarting);
        }
        let Some(session) = self.session.as_mut() else {
            return Err(SupervisorError::NotStarting);
        };

        if matches!(session.try_wait(), Ok(None)) {
            self.lifecycle = Lifecycle::Ready;
            Ok(())
        } else {
            self.lifecycle = Lifecycle::Failed;
            Err(SupervisorError::ProcessExited)
        }
    }

    /// Marks the session as failed without killing the child.
    ///
    /// The lifecycle becomes [`Lifecycle::Failed`] and cannot be moved back to
    /// [`Lifecycle::Ready`]; the caller must drop or shut down the session.
    pub const fn mark_failed(&mut self) {
        self.lifecycle = Lifecycle::Failed;
    }

    /// Detects process exit and updates the lifecycle.
    ///
    /// Any exit observed outside an explicit shutdown is a failure; the
    /// session layer must consult [`Self::restart_policy`] and decide whether
    /// to restart.
    ///
    /// # Errors
    ///
    /// Returns the session's [`NativeError`] when the status cannot be read.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, SupervisorError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(None);
        };
        let status = session.try_wait()?;
        if status.is_some() {
            self.lifecycle = Lifecycle::Failed;
        }
        Ok(status)
    }

    /// Returns the sanitized `stderr` tail of the live session.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.session
            .as_ref()
            .map_or_else(String::new, ProcessSession::stderr_tail)
    }

    /// Returns how many `stderr` bytes the live session dropped.
    #[must_use]
    pub fn stderr_discarded_bytes(&self) -> u64 {
        self.session
            .as_ref()
            .map_or(0, ProcessSession::stderr_discarded_bytes)
    }

    /// Closes stdin, waits, kills and reaps the child.
    ///
    /// Returns `None` when no session existed. The lifecycle becomes
    /// [`Lifecycle::Stopped`] in both cases.
    pub fn shutdown(&mut self) -> Option<ShutdownReport> {
        self.shutdown_with_timeout(Duration::from_millis(SHUTDOWN_DEADLINE_MS))
    }

    /// Like [`Self::shutdown`] with a caller-supplied deadline.
    ///
    /// Returns `None` when no session existed. The lifecycle becomes
    /// [`Lifecycle::Stopped`] in both cases.
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> Option<ShutdownReport> {
        let report = self
            .session
            .take()
            .map(|session| session.shutdown_with_timeout(timeout));
        self.lifecycle = Lifecycle::Stopped;
        report
    }

    /// Test hook: starts a session from an arbitrary spec.
    #[cfg(test)]
    fn start_for_test(&mut self, spec: &SpawnSpec) -> Result<(), SupervisorError> {
        self.start(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::atomic::AtomicU64;

    const HELPER_ENV: &str = "WXML_TSGO_HELPER_MODE";
    const HELPER_TEST: &str = "native::supervisor::tests::helper_child";

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("wxml-tsgo-{tag}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    /// Child-process entry point selected through [`HELPER_ENV`].
    ///
    /// In a normal test run the variable is absent and the test is a no-op.
    /// The supervisor tests spawn the current test binary with `--exact` and a
    /// mode value to exercise pipes, exit detection and `stderr` draining
    /// without depending on optional `/bin` helpers.
    #[test]
    fn helper_child() {
        let Ok(mode) = std::env::var(HELPER_ENV) else {
            return;
        };
        match mode.as_str() {
            "sleep" => {
                thread::sleep(Duration::from_secs(60));
                std::process::exit(0);
            }
            "sleep-short" => {
                thread::sleep(Duration::from_secs(2));
                std::process::exit(0);
            }
            "exit" => std::process::exit(0),
            "exit-failure" => std::process::exit(1),
            "dump-env" => {
                for name in [
                    "LANG",
                    "LC_ALL",
                    "PATH",
                    "HOME",
                    "NODE_OPTIONS",
                    "NODE_PATH",
                    "TS_NODE_PROJECT",
                ] {
                    let value = std::env::var(name).unwrap_or_else(|_| "<absent>".to_owned());
                    eprintln!("{name}={value}");
                }
                std::process::exit(0);
            }
            "flood-stderr" => {
                let chunk = vec![b'x'; 8 * 1024];
                let mut stderr = io::stderr();
                for _ in 0..20 {
                    let _ = stderr.write_all(&chunk);
                }
                let _ = stderr.write_all(b"\nstderr-sentinel\n");
                let _ = stderr.flush();
                std::process::exit(0);
            }
            "descendant" => {
                let Ok(executable) = std::env::current_exe() else {
                    std::process::exit(0);
                };
                // Hold the parent's stderr pipe but not its stdout pipe: the
                // reader thread closes stdout after the libtest banner, and a
                // descendant writing there would die before proving the point.
                let _ = Command::new(executable)
                    .args(["--exact", HELPER_TEST, "--test-threads=1", "--nocapture"])
                    .env(HELPER_ENV, "sleep-short")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn();
                eprintln!("descendant-started");
                let _ = io::stderr().flush();
                std::process::exit(0);
            }
            _ => {}
        }
    }

    fn helper_spec(mode: &'static str) -> SpawnSpec {
        let executable = std::env::current_exe().expect("test executable");
        SpawnSpec {
            program: executable,
            args: vec![
                "--exact".to_owned(),
                HELPER_TEST.to_owned(),
                "--test-threads=1".to_owned(),
                "--nocapture".to_owned(),
            ],
            sandbox_root: std::env::temp_dir(),
            environment: vec![("LANG", "C"), ("LC_ALL", "C"), (HELPER_ENV, mode)],
        }
    }

    fn spawn_helper(mode: &'static str) -> ProcessSession {
        ProcessSession::spawn(&helper_spec(mode), DEFAULT_FRAME_LIMITS).expect("spawn helper")
    }

    fn wait_for_exit(session: &mut ProcessSession, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Deadline::after(timeout);
        loop {
            match session.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if !deadline.is_expired() => {
                    thread::sleep(Duration::from_millis(5));
                }
                _ => return None,
            }
        }
    }

    fn wait_for_stderr(session: &ProcessSession, needle: &str, timeout: Duration) -> String {
        let deadline = Deadline::after(timeout);
        loop {
            let tail = session.stderr_tail();
            if tail.contains(needle) || deadline.is_expired() {
                return tail;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_flag(flag: &AtomicBool, timeout: Duration) -> bool {
        let deadline = Deadline::after(timeout);
        while !flag.load(Ordering::SeqCst) {
            if deadline.is_expired() {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn builds_only_frozen_startup_args() {
        let sandbox = temp_dir("argv");
        let engine = PinnedEngine::pinned();
        let install_root = Path::new("/opt/tsgo");

        let spec = SpawnSpec::for_engine(&engine, install_root, &sandbox).expect("spawn spec");
        let plan = engine
            .launch_plan(sandbox.to_str().expect("utf8 sandbox"))
            .expect("launch plan");

        assert_eq!(spec.program, install_root.join("engine/lib/tsc"));
        assert_eq!(spec.args, plan.args);
        assert_eq!(spec.sandbox_root, sandbox);
        assert_eq!(spec.environment, vec![("LANG", "C"), ("LC_ALL", "C")]);

        for forbidden in [
            "--runExternalCode",
            "--pipe",
            "--timing",
            "--callbacks=appendFile",
            "appendFile",
            "chtimes",
            "node",
        ] {
            assert!(
                !spec.args.iter().any(|arg| arg.contains(forbidden)),
                "argv must not contain {forbidden}"
            );
        }

        let command = spec.command();
        assert_eq!(command.get_program(), spec.program.as_os_str());
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        let expected: Vec<&std::ffi::OsStr> = plan
            .args
            .iter()
            .map(String::as_str)
            .map(std::ffi::OsStr::new)
            .collect();
        assert_eq!(args, expected);
        assert_eq!(command.get_current_dir(), Some(spec.sandbox_root.as_path()));

        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            envs,
            vec![
                ("LANG".to_owned(), Some("C".to_owned())),
                ("LC_ALL".to_owned(), Some("C".to_owned())),
            ]
        );

        std::fs::remove_dir_all(sandbox).expect("cleanup");
    }

    #[test]
    fn sandbox_root_validation_is_strict() {
        let engine = PinnedEngine::pinned();

        let empty = temp_dir("sandbox-empty");
        assert!(validate_sandbox_root(&engine, &empty).is_ok());

        let populated = temp_dir("sandbox-populated");
        std::fs::write(populated.join("canary.txt"), b"x").expect("write canary");
        assert!(matches!(
            validate_sandbox_root(&engine, &populated),
            Err(SpawnError::SandboxNotEmpty)
        ));

        let file = temp_dir("sandbox-file").join("not-a-dir");
        std::fs::write(&file, b"x").expect("write file");
        assert!(matches!(
            validate_sandbox_root(&engine, &file),
            Err(SpawnError::SandboxNotDirectory)
        ));

        let missing = std::env::temp_dir().join("wxml-tsgo-does-not-exist");
        assert!(matches!(
            validate_sandbox_root(&engine, &missing),
            Err(SpawnError::SandboxMissing)
        ));

        assert!(matches!(
            validate_sandbox_root(&engine, Path::new("relative/path")),
            Err(SpawnError::SandboxRoot(_))
        ));
        assert!(matches!(
            validate_sandbox_root(&engine, Path::new("/tmp/../tmp")),
            Err(SpawnError::SandboxRoot(_))
        ));

        #[cfg(unix)]
        {
            let link = temp_dir("sandbox-link");
            let target = temp_dir("sandbox-target");
            let link_path = link.join("root");
            std::os::unix::fs::symlink(&target, &link_path).expect("create symlink");
            assert!(matches!(
                validate_sandbox_root(&engine, &link_path),
                Err(SpawnError::SandboxSymlink)
            ));
            std::fs::remove_dir_all(link).expect("cleanup link");
            std::fs::remove_dir_all(target).expect("cleanup target");
        }

        assert!(matches!(
            validate_install_root(Path::new("relative/install")),
            Err(SpawnError::InvalidInstallRoot)
        ));
        assert!(matches!(
            validate_install_root(Path::new("/opt/../opt/tsgo")),
            Err(SpawnError::InvalidInstallRoot)
        ));
        assert!(validate_install_root(Path::new("/opt/tsgo")).is_ok());

        std::fs::remove_dir_all(empty).expect("cleanup");
        std::fs::remove_dir_all(populated).expect("cleanup");
        std::fs::remove_dir_all(file.parent().expect("parent")).expect("cleanup");
    }

    #[test]
    fn deadlines_are_saturating_and_report_milliseconds() {
        let expired = Deadline::after(Duration::ZERO);
        assert!(expired.is_expired());
        assert_eq!(expired.remaining(), Duration::ZERO);
        assert_eq!(expired.timeout_ms(), 0);

        let pending = Deadline::after(Duration::from_millis(50));
        assert!(!pending.is_expired());
        assert!(pending.remaining() <= Duration::from_millis(50));
        assert_eq!(pending.timeout_ms(), 50);

        // Absurd durations must not panic in `Instant` arithmetic.
        let absurd = Deadline::after(Duration::MAX);
        assert!(!absurd.is_expired());

        assert!(
            Deadline::after(Duration::from_millis(HANDSHAKE_DEADLINE_MS)).at() > Instant::now()
        );

        assert_eq!(Deadline::handshake().timeout_ms(), HANDSHAKE_DEADLINE_MS);
        assert_eq!(Deadline::callback().timeout_ms(), CALLBACK_DEADLINE_MS);
        assert_eq!(Deadline::release().timeout_ms(), RELEASE_DEADLINE_MS);
        assert_eq!(Deadline::request(DEFAULT_FRAME_LIMITS).timeout_ms(), 15_000);
        assert_eq!(
            Deadline::snapshot(DEFAULT_FRAME_LIMITS).timeout_ms(),
            30_000
        );
    }

    #[test]
    fn restart_policy_is_bounded_metadata() {
        let policy = RestartPolicy::PINNED;
        assert_eq!(policy.max_start_attempts, 3);
        assert_eq!(policy.start_backoff_ms, [100, 500, 2_000]);
        assert_eq!(policy.protocol_restarts, 1);

        assert_eq!(policy.backoff(1), Some(Duration::from_millis(100)));
        assert_eq!(policy.backoff(2), Some(Duration::from_millis(500)));
        assert_eq!(policy.backoff(3), Some(Duration::from_millis(2_000)));
        assert_eq!(policy.backoff(4), None);
        assert_eq!(policy.backoff(0), None);
    }

    #[test]
    fn stderr_ring_stays_bounded_and_sanitized() {
        let mut ring = StderrRing::new(1024);
        for _ in 0..4096 {
            ring.push(b"a");
        }
        assert_eq!(ring.bytes.len(), 1024);
        assert!(ring.discarded() > 0);

        ring.push(b"\npanic: boom\0\x07\n");
        let tail = ring.tail(64);
        assert!(!tail.contains('\0'));
        assert!(!tail.contains('\x07'));
        assert!(tail.contains("panic: boom"));

        let mut long_line = StderrRing::new(2048);
        let long = vec![b'x'; STDERR_LINE_BYTES * 3];
        long_line.push(&long);
        long_line.push(b"\nkept\n");
        let tail = long_line.tail(16);
        assert!(tail.contains("kept"));
    }

    #[test]
    fn inbound_budget_accounts_and_closes() {
        let budget = InboundBudget::new(10);
        assert!(budget.try_reserve(6));
        assert!(budget.try_reserve(4));
        assert!(!budget.try_reserve(1), "aggregate limit is enforced");
        budget.release(3);
        assert_eq!(budget.queued(), 7);
        assert!(budget.try_reserve(3));

        budget.close();
        assert!(budget.is_closed());
        assert!(!reserve_inbound(&budget, 1));
    }

    #[test]
    fn stdout_loop_reports_faults_and_eof() {
        let limits = DEFAULT_FRAME_LIMITS;
        let budget = InboundBudget::new(1024);
        let closed = AtomicBool::new(false);
        let done = AtomicBool::new(false);

        let (sender, receiver) = sync_channel(8);
        stdout_loop(
            io::Cursor::new(b"not-a-header\r\n\r\n".to_vec()),
            limits,
            &sender,
            &closed,
            &done,
            &budget,
        );
        match receiver.try_recv().expect("fault event") {
            InboundEvent::Fault(fault) => {
                assert!(!fault.message().contains('\n'));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(closed.load(Ordering::SeqCst));
        assert!(done.load(Ordering::SeqCst));

        let closed = AtomicBool::new(false);
        let done = AtomicBool::new(false);
        let (sender, receiver) = sync_channel(8);
        let mut payload = b"Content-Length: 2\r\n\r\n".to_vec();
        payload.extend_from_slice(b"{}");
        stdout_loop(
            io::Cursor::new(payload),
            limits,
            &sender,
            &closed,
            &done,
            &budget,
        );
        assert!(matches!(
            receiver.try_recv().expect("frame"),
            InboundEvent::Frame(_)
        ));
        assert!(matches!(
            receiver.try_recv().expect("closed"),
            InboundEvent::Closed
        ));
        assert!(closed.load(Ordering::SeqCst));
        assert!(done.load(Ordering::SeqCst));
    }

    #[test]
    fn stdout_loop_stops_when_the_receiver_is_dropped() {
        let budget = InboundBudget::new(1024);
        let closed = AtomicBool::new(false);
        let done = AtomicBool::new(false);
        let (sender, receiver) = sync_channel(1);
        drop(receiver);

        let mut payload = b"Content-Length: 2\r\n\r\n".to_vec();
        payload.extend_from_slice(b"{}");
        stdout_loop(
            io::Cursor::new(payload),
            DEFAULT_FRAME_LIMITS,
            &sender,
            &closed,
            &done,
            &budget,
        );

        assert!(done.load(Ordering::SeqCst));
        assert_eq!(budget.queued(), 0, "reservation is released on failure");
    }

    #[test]
    fn stdout_loop_applies_inbound_backpressure() {
        let budget = Arc::new(InboundBudget::new(2));
        assert!(budget.try_reserve(2));

        let closed = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (sender, _receiver) = sync_channel(1);
        let mut payload = b"Content-Length: 2\r\n\r\n".to_vec();
        payload.extend_from_slice(b"{}");

        let thread_budget = Arc::clone(&budget);
        let thread_closed = Arc::clone(&closed);
        let thread_done = Arc::clone(&done);
        let handle = thread::spawn(move || {
            stdout_loop(
                io::Cursor::new(payload),
                DEFAULT_FRAME_LIMITS,
                &sender,
                &thread_closed,
                &thread_done,
                &thread_budget,
            );
        });

        thread::sleep(Duration::from_millis(30));
        assert!(!done.load(Ordering::SeqCst), "reader must block on budget");
        budget.close();
        handle.join().expect("reader thread");
        assert!(done.load(Ordering::SeqCst));
        budget.release(2);
    }

    #[test]
    fn verifies_the_pinned_executable_hash() {
        let directory = temp_dir("hash");
        let executable = directory.join("tsc");
        std::fs::write(&executable, b"not the pinned engine").expect("write fake executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&executable)
                .expect("metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).expect("chmod");
        }

        assert!(matches!(
            verify_executable(&executable, "0000"),
            Err(SpawnError::ExecutableMismatch)
        ));

        let missing = directory.join("missing");
        assert!(matches!(
            verify_executable(&missing, "0000"),
            Err(SpawnError::ExecutableMissing)
        ));

        std::fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn supervisor_refuses_a_mismatched_executable_before_spawn() {
        let install = temp_dir("install");
        let sandbox = temp_dir("install-sandbox");
        let engine_dir = install.join("engine/lib");
        std::fs::create_dir_all(&engine_dir).expect("engine dir");
        let executable = engine_dir.join("tsc");
        std::fs::write(&executable, b"definitely not the engine").expect("fake executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&executable)
                .expect("metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).expect("chmod");
        }

        let mut supervisor = EngineSupervisor::new(PinnedEngine::pinned());
        let error = supervisor
            .spawn(&install, &sandbox)
            .expect_err("hash mismatch must fail");
        assert!(matches!(
            error,
            SupervisorError::Spawn(SpawnError::ExecutableMismatch)
        ));
        assert_eq!(supervisor.lifecycle(), Lifecycle::NotStarted);

        std::fs::remove_dir_all(install).expect("cleanup");
        std::fs::remove_dir_all(sandbox).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn writes_and_reads_a_frame_through_a_local_process() {
        let cat = Path::new("/bin/cat");
        assert!(cat.is_file(), "this test requires /bin/cat on Linux");
        let spec = SpawnSpec {
            program: cat.to_path_buf(),
            args: Vec::new(),
            sandbox_root: std::env::temp_dir(),
            environment: CHILD_ENVIRONMENT.to_vec(),
        };
        let mut session = ProcessSession::spawn(&spec, DEFAULT_FRAME_LIMITS).expect("spawn cat");
        let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;

        session
            .send(FrameRole::Request, body)
            .expect("write request frame");
        let event = session
            .recv_until(Deadline::after(Duration::from_secs(2)))
            .expect("read echoed frame");
        match event {
            InboundEvent::Frame(frame) => assert_eq!(frame.body(), body),
            other => panic!("unexpected inbound event: {other:?}"),
        }

        // A reader-side role must never be written by the client.
        assert!(matches!(
            session.send(FrameRole::Response, body),
            Err(NativeError::Protocol(_))
        ));

        let report = session.shutdown_with_timeout(Duration::from_millis(100));
        assert!(!report.threads_detached());
    }

    #[cfg(unix)]
    #[test]
    fn child_malformed_stdout_is_a_fault() {
        let echo = Path::new("/bin/echo");
        assert!(echo.is_file(), "this test requires /bin/echo on Linux");
        let spec = SpawnSpec {
            program: echo.to_path_buf(),
            args: vec!["not-a-frame".to_owned()],
            sandbox_root: std::env::temp_dir(),
            environment: CHILD_ENVIRONMENT.to_vec(),
        };
        let mut session = ProcessSession::spawn(&spec, DEFAULT_FRAME_LIMITS).expect("spawn echo");

        let event = session
            .recv_until(Deadline::after(Duration::from_secs(2)))
            .expect("inbound event");
        assert!(
            matches!(event, InboundEvent::Fault(_)),
            "non-frame stdout must be a fault: {event:?}"
        );
        assert!(session.stdout_closed());
        let _ = session.shutdown_with_timeout(Duration::from_millis(100));
    }

    #[cfg(unix)]
    #[test]
    fn child_clean_stdout_eof_is_closed() {
        let true_binary = Path::new("/bin/true");
        assert!(
            true_binary.is_file(),
            "this test requires /bin/true on Linux"
        );
        let spec = SpawnSpec {
            program: true_binary.to_path_buf(),
            args: Vec::new(),
            sandbox_root: std::env::temp_dir(),
            environment: CHILD_ENVIRONMENT.to_vec(),
        };
        let mut session = ProcessSession::spawn(&spec, DEFAULT_FRAME_LIMITS).expect("spawn true");

        let event = session
            .recv_until(Deadline::after(Duration::from_secs(2)))
            .expect("inbound event");
        assert!(
            matches!(event, InboundEvent::Closed),
            "clean stdout EOF must be closed: {event:?}"
        );
        assert!(session.stdout_closed());
    }

    #[cfg(unix)]
    #[test]
    fn recv_timeout_poisons_the_session() {
        let sleep = Path::new("/bin/sleep");
        assert!(sleep.is_file(), "this test requires /bin/sleep on Linux");
        let spec = SpawnSpec {
            program: sleep.to_path_buf(),
            args: vec!["30".to_owned()],
            sandbox_root: std::env::temp_dir(),
            environment: CHILD_ENVIRONMENT.to_vec(),
        };
        let mut session = ProcessSession::spawn(&spec, DEFAULT_FRAME_LIMITS).expect("spawn sleep");

        let error = session
            .recv_until(Deadline::after(Duration::from_millis(20)))
            .expect_err("no frame can arrive");
        assert!(matches!(error, NativeError::Timeout(20)));
        assert!(session.is_poisoned());

        assert!(matches!(
            session.send(FrameRole::Request, b"{}"),
            Err(NativeError::Unavailable {
                reason: UnavailableReason::SessionPoisoned
            })
        ));
        assert!(matches!(
            session.recv_until(Deadline::after(Duration::from_millis(10))),
            Err(NativeError::Unavailable {
                reason: UnavailableReason::SessionPoisoned
            })
        ));

        let report = session.abort_for_timeout_with(Duration::from_millis(50));
        assert!(report.forced_kill());
    }

    #[test]
    fn send_backpressure_poisons_without_leaking_the_writer() {
        let mut session = spawn_helper("sleep");
        let writer_done = Arc::clone(&session.writer_done);
        let body = vec![b'x'; 16 * 1024];
        let mut timed_out = false;

        for _ in 0..64 {
            match session.send_until(
                FrameRole::Request,
                &body,
                Deadline::after(Duration::from_millis(20)),
            ) {
                Ok(()) => {}
                Err(NativeError::Timeout(_)) => {
                    timed_out = true;
                    break;
                }
                Err(other) => panic!("unexpected send error: {other}"),
            }
        }

        assert!(timed_out, "the writer pipe must fill");
        assert!(session.is_poisoned());

        let started = Instant::now();
        drop(session);
        assert!(
            wait_for_flag(&writer_done, Duration::from_secs(2)),
            "writer thread must exit after kill-on-drop"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn explicit_shutdown_closes_stdin_then_kills_and_reaps() {
        let session = spawn_helper("sleep");
        let report = session.shutdown_with_timeout(Duration::from_millis(50));

        assert!(report.forced_kill());
        assert!(report.exit_status().is_some());
        assert!(!report.threads_detached());
        assert_eq!(report.stderr_tail(), "");
    }

    #[test]
    fn shutdown_detaches_threads_held_by_a_descendant() {
        let session = spawn_helper("descendant");
        let tail = wait_for_stderr(&session, "descendant-started", Duration::from_secs(2));
        assert!(tail.contains("descendant-started"));

        let started = Instant::now();
        let report = session.shutdown_with_timeout(Duration::from_millis(50));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            report.threads_detached(),
            "stderr is held by the descendant and must be detached"
        );
    }

    #[test]
    fn process_exit_is_detected_without_blocking() {
        let mut session = spawn_helper("exit");
        let status = wait_for_exit(&mut session, Duration::from_secs(5)).expect("child exit");
        assert!(status.success());
    }

    #[test]
    fn mark_ready_refuses_an_exited_child() {
        let mut supervisor = EngineSupervisor::new(PinnedEngine::pinned());
        supervisor
            .start_for_test(&helper_spec("exit"))
            .expect("start");
        assert_eq!(supervisor.lifecycle(), Lifecycle::Starting);

        // Wait through the session (not the supervisor) so the lifecycle is
        // still Starting when `mark_ready` performs its own exit check.
        let deadline = Deadline::after(Duration::from_secs(5));
        let mut exited = false;
        while !exited && !deadline.is_expired() {
            exited = supervisor
                .session()
                .expect("session")
                .try_wait()
                .expect("try_wait")
                .is_some();
            if !exited {
                thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(exited, "the helper child must exit");

        assert!(matches!(
            supervisor.mark_ready(),
            Err(SupervisorError::ProcessExited)
        ));
        assert_eq!(supervisor.lifecycle(), Lifecycle::Failed);
        assert!(supervisor.shutdown().is_some());
    }

    #[test]
    fn nonzero_exit_is_observed() {
        let mut session = spawn_helper("exit-failure");
        let status = wait_for_exit(&mut session, Duration::from_secs(5)).expect("child exit");
        assert!(!status.success());
    }

    #[test]
    fn mark_failed_cannot_be_overwritten() {
        let mut supervisor = EngineSupervisor::new(PinnedEngine::pinned());
        supervisor
            .start_for_test(&helper_spec("sleep"))
            .expect("start");
        supervisor.mark_failed();
        assert_eq!(supervisor.lifecycle(), Lifecycle::Failed);
        assert!(matches!(
            supervisor.mark_ready(),
            Err(SupervisorError::NotStarting)
        ));
        assert_eq!(supervisor.lifecycle(), Lifecycle::Failed);
        assert!(
            supervisor
                .shutdown_with_timeout(Duration::from_millis(50))
                .is_some()
        );
    }

    #[test]
    fn supervisor_lifecycle_requires_an_explicit_probe() {
        let mut supervisor = EngineSupervisor::new(PinnedEngine::pinned());
        assert_eq!(supervisor.lifecycle(), Lifecycle::NotStarted);
        assert!(matches!(
            supervisor.mark_ready(),
            Err(SupervisorError::NotStarting)
        ));
        assert!(supervisor.shutdown().is_none());

        supervisor
            .start_for_test(&helper_spec("sleep"))
            .expect("start");
        assert_eq!(supervisor.lifecycle(), Lifecycle::Starting);
        supervisor.mark_ready().expect("probe passed");
        assert_eq!(supervisor.lifecycle(), Lifecycle::Ready);

        assert!(
            supervisor
                .shutdown_with_timeout(Duration::from_millis(50))
                .is_some()
        );
        assert_eq!(supervisor.lifecycle(), Lifecycle::Stopped);
    }

    #[test]
    fn drains_stderr_and_surfaces_truncation() {
        let session = spawn_helper("flood-stderr");
        let tail = wait_for_stderr(&session, "stderr-sentinel", Duration::from_secs(5));
        assert!(tail.contains("stderr-sentinel"));
        assert!(tail.len() <= STDERR_TAIL_BYTES);
        assert!(!tail.contains('\0'));
        assert!(
            session.stderr_discarded_bytes() > 0,
            "the ring must report dropped bytes"
        );

        let report = session.shutdown_with_timeout(Duration::from_millis(100));
        assert!(report.stderr_discarded_bytes() > 0);
    }

    #[test]
    fn clears_the_child_environment_to_lang_and_lc_all() {
        let session = spawn_helper("dump-env");
        let tail = wait_for_stderr(&session, "TS_NODE_PROJECT=", Duration::from_secs(5));
        assert!(tail.contains("LANG=C"), "unexpected stderr: {tail}");
        assert!(tail.contains("LC_ALL=C"), "unexpected stderr: {tail}");
        for absent in [
            "PATH=<absent>",
            "HOME=<absent>",
            "NODE_OPTIONS=<absent>",
            "NODE_PATH=<absent>",
            "TS_NODE_PROJECT=<absent>",
        ] {
            assert!(tail.contains(absent), "expected {absent} in: {tail}");
        }
        let _ = session.shutdown_with_timeout(Duration::from_millis(100));
    }

    #[test]
    fn hex_encoding_is_lowercase() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }
}
