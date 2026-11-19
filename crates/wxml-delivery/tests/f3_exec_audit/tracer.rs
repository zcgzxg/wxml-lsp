//! Minimal Linux x86_64 `ptrace` exec auditor (no Cargo dependency).
//!
//! The tracer follows `strace -f -e trace=execve,execveat` semantics: the
//! target is forked by the tracer thread, calls `PTRACE_TRACEME`, stops itself
//! with `SIGSTOP`, and is resumed with fork/vfork/clone/exec events enabled.
//! On every exec event the tracer snapshots `/proc/<pid>/exe`,
//! `/proc/<pid>/cmdline`, `/proc/<pid>/comm` and the environment keys from
//! `/proc/<pid>/environ`, so an absolute-path `node` invocation that bypasses
//! `PATH` is still recorded.
//!
//! Safety: the child path after `fork` only calls async-signal-safe libc
//! entry points (`setpgid`, `close`, `dup2`, `chdir`, `ptrace`, `write`,
//! `raise`, `execve`, `_exit`). Every C string/pointer array is built before
//! the fork. All `unsafe` blocks below exist only to call those libc and
//! `ptrace` entry points; the rest of the test harness stays safe Rust.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::mem::size_of;
use std::os::fd::FromRawFd;
use std::os::fd::{AsRawFd, RawFd};
use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PTRACE_TRACEME: c_uint = 0;
const PTRACE_CONT: c_uint = 7;
const PTRACE_SETOPTIONS: c_uint = 0x4200;
const PTRACE_GETEVENTMSG: c_uint = 0x4201;

const PTRACE_O_TRACEFORK: usize = 0x0000_0002;
const PTRACE_O_TRACEVFORK: usize = 0x0000_0004;
const PTRACE_O_TRACECLONE: usize = 0x0000_0008;
const PTRACE_O_TRACEEXEC: usize = 0x0000_0010;
const PTRACE_O_EXITKILL: usize = 0x0010_0000;

const PTRACE_EVENT_FORK: c_int = 1;
const PTRACE_EVENT_VFORK: c_int = 2;
const PTRACE_EVENT_CLONE: c_int = 3;
const PTRACE_EVENT_EXEC: c_int = 4;
const PTRACE_EVENT_VFORK_DONE: c_int = 5;

/// `__WALL` from `<linux/wait.h>`: wait for all children, including threads.
const __WALL: c_int = 0x4000_0000;
const WNOHANG: c_int = 1;

const SIGTRAP: c_int = 5;
const SIGKILL: c_int = 9;
const SIGCHLD: c_int = 17;
const SIGCONT: c_int = 18;
const SIGSTOP: c_int = 19;
const SIGTSTP: c_int = 20;
const SIGTTIN: c_int = 21;
const SIGTTOU: c_int = 22;

/// A child that stops after the root process exited is killed after this grace.
const POST_EXIT_GRACE: Duration = Duration::from_secs(5);
/// How long the tracer drains wait events after killing the process group.
const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// Poll interval when no tracee produced a wait event.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

unsafe extern "C" {
    fn fork() -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn ptrace(request: c_uint, ...) -> c_long;
    fn raise(sig: c_int) -> c_int;
    fn execve(path: *const c_char, argv: *const *const c_char, envp: *const *const c_char)
    -> c_int;
    fn setpgid(pid: c_int, pgid: c_int) -> c_int;
    fn chdir(path: *const c_char) -> c_int;
    fn dup2(old_fd: c_int, new_fd: c_int) -> c_int;
    #[link_name = "pipe"]
    fn c_pipe(fds: *mut c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn _exit(code: c_int) -> !;
    fn __errno_location() -> *mut c_int;
}

/// Why the trace could not produce evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    /// The host does not permit ptrace (`seccomp`, `Yama`, missing `/proc`).
    /// Tests turn this into a loud `BLOCKED` unless a hard gate is set.
    Blocked(String),
    /// The traced program failed to start or the tracer hit an internal error.
    Failed(String),
}

impl TraceError {
    pub fn reason(&self) -> &str {
        match self {
            Self::Blocked(reason) | Self::Failed(reason) => reason,
        }
    }
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(reason) => write!(formatter, "exec audit blocked: {reason}"),
            Self::Failed(reason) => write!(formatter, "exec audit failed: {reason}"),
        }
    }
}

/// Exit state of the traced root process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
    Unknown,
}

impl Exit {
    pub fn success(self) -> bool {
        matches!(self, Self::Code(0))
    }
}

/// One observed `execve`/`execveat`.
#[derive(Debug, Clone)]
pub struct ExecRecord {
    /// Thread/process id that exec'd (the execing thread becomes the tgid).
    pub pid: i32,
    /// Parent process observed through a fork event, or `/proc/<pid>/status`.
    pub ppid: i32,
    /// True for the root process (the node the tracer started).
    pub root: bool,
    /// Absolute path from `/proc/<pid>/exe` after the exec completed.
    pub exe: PathBuf,
    /// `/proc/<pid>/comm`, the kernel task name.
    pub comm: String,
    /// NUL-split `/proc/<pid>/cmdline` after the exec.
    pub argv: Vec<String>,
    /// Sorted environment keys of the exec'd process; values are not kept.
    pub env_keys: Vec<String>,
}

impl ExecRecord {
    /// Lower-case executable basename.
    pub fn exe_name(&self) -> String {
        self.exe.file_name().map_or_else(String::new, |name| {
            name.to_string_lossy().to_ascii_lowercase()
        })
    }

    /// Lower-case `argv[0]` basename.
    pub fn argv0_name(&self) -> String {
        self.argv
            .first()
            .and_then(|arg| Path::new(arg).file_name())
            .map_or_else(String::new, |name| {
                name.to_string_lossy().to_ascii_lowercase()
            })
    }

    /// One `EXEC` log line.
    pub fn line(&self) -> String {
        format!(
            "EXEC pid={} ppid={} root={} comm={} basename={} exe={} argv={:?} env_keys={}",
            self.pid,
            self.ppid,
            self.root,
            self.comm,
            self.exe_name(),
            self.exe.display(),
            self.argv,
            self.env_keys.len()
        )
    }
}

/// One observed fork/vfork/clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForkRecord {
    pub parent: i32,
    pub child: i32,
    pub kind: &'static str,
}

/// Complete exec/fork log of one traced process tree.
#[derive(Debug, Clone, Default)]
pub struct ExecLog {
    pub root_pid: i32,
    pub execs: Vec<ExecRecord>,
    pub forks: Vec<ForkRecord>,
}

impl ExecLog {
    pub fn merge(&mut self, other: &Self) {
        self.execs.extend(other.execs.iter().cloned());
        self.forks.extend(other.forks.iter().copied());
    }

    pub fn exec_count(&self) -> usize {
        self.execs.len()
    }

    /// True when any exec's absolute path or argv covers `suffix`.
    pub fn covers(&self, suffix: &str) -> bool {
        self.execs.iter().any(|record| {
            record.exe.to_string_lossy().ends_with(suffix)
                || record.argv.iter().any(|arg| arg.ends_with(suffix))
        })
    }

    pub fn summary(&self) -> String {
        let mut lines = Vec::with_capacity(self.execs.len() + self.forks.len() + 1);
        lines.push(format!(
            "root_pid={} execs={} forks={}",
            self.root_pid,
            self.execs.len(),
            self.forks.len()
        ));
        for record in &self.execs {
            lines.push(record.line());
        }
        for fork in &self.forks {
            lines.push(format!(
                "FORK parent={} child={} kind={}",
                fork.parent, fork.child, fork.kind
            ));
        }
        lines.join("\n")
    }
}

/// Result of tracing one process tree to completion.
#[derive(Debug, Clone)]
pub struct TraceOutcome {
    pub log: ExecLog,
    pub exit: Exit,
    pub timed_out: bool,
    pub survivors: Vec<i32>,
    pub elapsed: Duration,
}

impl TraceOutcome {
    fn empty(root_pid: i32) -> Self {
        Self {
            log: ExecLog {
                root_pid,
                ..ExecLog::default()
            },
            exit: Exit::Unknown,
            timed_out: false,
            survivors: Vec::new(),
            elapsed: Duration::ZERO,
        }
    }
}

/// How the child's standard input is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinMode {
    /// `/dev/null`.
    Null,
    /// A pipe: the parent writes through [`TracedChild::stdin`].
    Piped,
}

/// One traced invocation: an absolute program, argv, an explicit environment
/// (no inheritance), a working directory and a stdin mode.
#[derive(Debug, Clone)]
pub struct Spec {
    pub program: PathBuf,
    pub argv: Vec<std::ffi::OsString>,
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub cwd: PathBuf,
    pub stdin: StdinMode,
}

impl Spec {
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        let program = program.into();
        let argv0 = program.as_os_str().to_owned();
        Self {
            program,
            argv: vec![argv0],
            env: Vec::new(),
            cwd: cwd.into(),
            stdin: StdinMode::Null,
        }
    }

    pub fn arg(mut self, argument: impl Into<std::ffi::OsString>) -> Self {
        self.argv.push(argument.into());
        self
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<std::ffi::OsString>,
    {
        self.argv.extend(arguments.into_iter().map(Into::into));
        self
    }

    pub fn env(
        mut self,
        key: impl Into<std::ffi::OsString>,
        value: impl Into<std::ffi::OsString>,
    ) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn envs<I, K, V>(mut self, variables: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<std::ffi::OsString>,
        V: Into<std::ffi::OsString>,
    {
        self.env.extend(
            variables
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    pub fn piped_stdin(mut self) -> Self {
        self.stdin = StdinMode::Piped;
        self
    }

    fn validate(&self) -> Result<(), TraceError> {
        if !self.program.is_absolute() {
            return Err(TraceError::Failed(format!(
                "program must be absolute: {}",
                self.program.display()
            )));
        }
        if !self.cwd.is_absolute() {
            return Err(TraceError::Failed(format!(
                "working directory must be absolute: {}",
                self.cwd.display()
            )));
        }
        Ok(())
    }
}

/// A running traced process tree.
pub struct TracedChild {
    pub pid: i32,
    pub stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
    log: Arc<Mutex<ExecLog>>,
    tracer: Option<JoinHandle<TraceOutcome>>,
    finished: Arc<AtomicBool>,
}

impl TracedChild {
    pub fn take_stdout(&mut self) -> Option<File> {
        self.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<File> {
        self.stderr.take()
    }

    pub fn snapshot(&self) -> ExecLog {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Kills the whole process group started by the tracer, if it still runs.
    pub fn kill_group(&self) {
        if self.finished.load(Ordering::SeqCst) {
            return;
        }
        let _ = unsafe { kill(-self.pid, SIGKILL) };
        let _ = unsafe { kill(self.pid, SIGKILL) };
    }

    /// Joins the tracer thread. Bounded by the timeout given to [`spawn`].
    pub fn join_tracer(mut self) -> TraceOutcome {
        self.stdin.take();
        match self.tracer.take() {
            Some(handle) => handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            None => TraceOutcome::empty(self.pid),
        }
    }

    /// Collects both output streams and the outcome. The streams are drained
    /// on helper threads so a chatty child cannot fill a pipe and deadlock.
    pub fn wait_with_output(mut self) -> (TraceOutcome, Vec<u8>, Vec<u8>) {
        let stdout = self.take_stdout().map(collect_stream);
        let stderr = self.take_stderr().map(collect_stream);
        let outcome = self.join_tracer();
        let stdout = stdout.map_or_else(Vec::new, Stream::finish);
        let stderr = stderr.map_or_else(Vec::new, Stream::finish);
        (outcome, stdout, stderr)
    }
}

impl Drop for TracedChild {
    fn drop(&mut self) {
        if self.tracer.is_some() {
            self.kill_group();
            if let Some(handle) = self.tracer.take() {
                let _ = handle.join();
            }
        }
    }
}

struct Setup {
    pid: i32,
    stdin: Option<File>,
    stdout: File,
    stderr: File,
    log: Arc<Mutex<ExecLog>>,
}

/// Forks, traces and returns the live handle once the first stop is handled.
pub fn spawn(spec: Spec, timeout: Duration) -> Result<TracedChild, TraceError> {
    spec.validate()?;
    let (sender, receiver) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&finished);
    let handle = thread::Builder::new()
        .name("f3-exec-tracer".to_owned())
        .spawn(move || {
            let outcome = trace_main(spec, timeout, sender);
            flag.store(true, Ordering::SeqCst);
            outcome
        })
        .map_err(|error| TraceError::Failed(format!("cannot start tracer thread: {error}")))?;
    match receiver.recv() {
        Ok(Ok(setup)) => Ok(TracedChild {
            pid: setup.pid,
            stdin: setup.stdin,
            stdout: Some(setup.stdout),
            stderr: Some(setup.stderr),
            log: setup.log,
            tracer: Some(handle),
            finished,
        }),
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            let _ = handle.join();
            Err(TraceError::Failed(
                "tracer thread stopped before reporting setup".to_owned(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Tracer thread
// ---------------------------------------------------------------------------

fn trace_main(
    spec: Spec,
    timeout: Duration,
    setup_tx: Sender<Result<Setup, TraceError>>,
) -> TraceOutcome {
    let started = Instant::now();
    let prepared = match Prepared::new(&spec) {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = setup_tx.send(Err(error));
            return TraceOutcome::empty(0);
        }
    };
    let Prepared {
        plan,
        stdin_child,
        stdin_parent,
        stdout_child,
        stdout_parent,
        stderr_child,
        stderr_parent,
        status_read,
        status_write,
    } = prepared;

    let stdin_fd = stdin_child.as_raw_fd();
    let stdout_fd = stdout_child.as_raw_fd();
    let stderr_fd = stderr_child.as_raw_fd();
    let status_read_fd = status_read.as_raw_fd();
    let status_write_fd = status_write.as_raw_fd();
    let stdin_parent_fd = stdin_parent.as_ref().map_or(-1, AsRawFd::as_raw_fd);
    let stdout_parent_fd = stdout_parent.as_raw_fd();
    let stderr_parent_fd = stderr_parent.as_raw_fd();
    let program_ptr = plan.program.as_ptr();
    let argv_ptr = plan.argv.as_ptr();
    let envp_ptr = plan.envp.as_ptr();
    let cwd_ptr = plan.cwd.as_ptr();

    let pid = unsafe { fork() };
    if pid < 0 {
        let error = TraceError::Failed(format!("fork failed: {}", io::Error::last_os_error()));
        let _ = setup_tx.send(Err(error));
        return TraceOutcome::empty(0);
    }
    if pid == 0 {
        // SAFETY: post-fork child, only async-signal-safe libc calls below.
        child_exec(
            stdin_parent_fd,
            stdout_parent_fd,
            stderr_parent_fd,
            stdin_fd,
            stdout_fd,
            stderr_fd,
            status_read_fd,
            status_write_fd,
            program_ptr,
            argv_ptr,
            envp_ptr,
            cwd_ptr,
        );
    }

    // Parent: only the parent-side ends stay open.
    drop(stdin_child);
    drop(stdout_child);
    drop(stderr_child);
    drop(status_write);

    let mut status_read = status_read;
    let mut code = [0_u8; 1];
    let status_code = match status_read.read_exact(&mut code) {
        Ok(()) => code[0],
        Err(error) => {
            let mut status = 0;
            unsafe { waitpid(pid, &raw mut status, 0) };
            let error = TraceError::Failed(format!("tracee did not report ptrace setup ({error})"));
            let _ = setup_tx.send(Err(error));
            return TraceOutcome::empty(pid);
        }
    };
    if status_code != 0 {
        let mut status = 0;
        unsafe { waitpid(pid, &raw mut status, 0) };
        let reason = if status_code == 1 {
            "ptrace(PTRACE_TRACEME) is not permitted on this host".to_owned()
        } else {
            format!(
                "tracee setup failed (code {status_code}); ptrace may be restricted by seccomp/Yama"
            )
        };
        let _ = setup_tx.send(Err(TraceError::Blocked(reason)));
        return TraceOutcome::empty(pid);
    }

    let mut status = 0;
    let waited = unsafe { waitpid(pid, &raw mut status, 0) };
    if waited != pid {
        let error = TraceError::Failed(format!(
            "initial waitpid failed: {}",
            io::Error::last_os_error()
        ));
        let _ = setup_tx.send(Err(error));
        return TraceOutcome::empty(pid);
    }
    if !wif_stopped(status) {
        let error = TraceError::Failed(format!(
            "tracee exited before the initial stop (status {status:#x}); execve probably failed"
        ));
        let _ = setup_tx.send(Err(error));
        return TraceOutcome::empty(pid);
    }

    let options = PTRACE_O_TRACEFORK
        | PTRACE_O_TRACEVFORK
        | PTRACE_O_TRACECLONE
        | PTRACE_O_TRACEEXEC
        | PTRACE_O_EXITKILL;
    if unsafe { ptrace(PTRACE_SETOPTIONS, pid, 0_usize, options) } < 0 {
        let errno = io::Error::last_os_error();
        let mut status = 0;
        unsafe {
            kill(pid, SIGKILL);
            waitpid(pid, &raw mut status, 0);
        }
        let error = classify_errno(errno.raw_os_error(), "PTRACE_SETOPTIONS");
        let _ = setup_tx.send(Err(error));
        return TraceOutcome::empty(pid);
    }

    let log = Arc::new(Mutex::new(ExecLog {
        root_pid: pid,
        ..ExecLog::default()
    }));
    if setup_tx
        .send(Ok(Setup {
            pid,
            stdin: stdin_parent,
            stdout: stdout_parent,
            stderr: stderr_parent,
            log: Arc::clone(&log),
        }))
        .is_err()
    {
        // The test dropped the handle before setup completed; stop the child.
        let _ = unsafe { kill(-pid, SIGKILL) };
        let _ = unsafe { kill(pid, SIGKILL) };
        return TraceOutcome::empty(pid);
    }

    resume(pid, 0);

    let mut state = LoopState {
        live: BTreeSet::from([pid]),
        root_pid: pid,
        root_exit: Exit::Unknown,
        root_exited_at: None,
        parents: BTreeMap::new(),
        warnings: Vec::new(),
    };
    let deadline = started + timeout;
    let mut timed_out = false;
    let mut last_progress = Instant::now();

    loop {
        if state.live.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            kill_group(pid);
            break;
        }
        if state
            .root_exited_at
            .is_some_and(|at| at.elapsed() >= POST_EXIT_GRACE)
        {
            kill_group(pid);
            break;
        }
        // A newly auto-attached thread can be returned as ECHILD by a
        // too-early waitpid and then be dropped from `live`; it would stay in
        // ptrace-stop forever and deadlock the whole traced process. Re-adopt
        // every thread listed under a traced thread group before polling.
        let mut progressed = discover_threads(&mut state, &log);
        let snapshot: Vec<i32> = state.live.iter().copied().collect();
        for task in snapshot {
            let mut task_status = 0;
            let waited = unsafe { waitpid(task, &raw mut task_status, __WALL | WNOHANG) };
            if waited == 0 {
                continue;
            }
            if waited < 0 {
                let errno = io::Error::last_os_error();
                if trace_enabled() {
                    eprintln!("TRACE wait-error task={task} errno={errno}");
                }
                if errno.raw_os_error() != Some(10) {
                    // ECHILD (10) means the task is gone and needs no warning.
                    state
                        .warnings
                        .push(format!("waitpid({task}) failed: {errno}"));
                }
                state.live.remove(&task);
                continue;
            }
            progressed = true;
            handle_wait(task, task_status, &mut state, &log);
        }
        if progressed {
            last_progress = Instant::now();
        } else {
            if trace_enabled() && last_progress.elapsed() >= Duration::from_secs(3) {
                last_progress = Instant::now();
                for task in &state.live {
                    let stat = fs::read_to_string(format!("/proc/{task}/stat"))
                        .unwrap_or_else(|_| "<gone>".to_owned());
                    let state_char = stat.split_whitespace().nth(2).unwrap_or("?");
                    eprintln!(
                        "TRACE stall root={} task={task} state={state_char}",
                        state.root_pid
                    );
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    if !state.live.is_empty() {
        kill_group(pid);
        drain(&mut state);
    }
    let survivors: Vec<i32> = state.live.iter().copied().collect();
    for warning in &state.warnings {
        eprintln!("EXEC-AUDIT: tracer warning: {warning}");
    }
    let log = log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    TraceOutcome {
        log,
        exit: state.root_exit,
        timed_out,
        survivors,
        elapsed: started.elapsed(),
    }
}

struct LoopState {
    live: BTreeSet<i32>,
    root_pid: i32,
    root_exit: Exit,
    root_exited_at: Option<Instant>,
    parents: BTreeMap<i32, i32>,
    warnings: Vec<String>,
}

fn handle_wait(task: i32, status: c_int, state: &mut LoopState, log: &Arc<Mutex<ExecLog>>) {
    if trace_enabled() {
        eprintln!(
            "TRACE wait task={task} status={status:#x} stopped={} sig={} event={}",
            wif_stopped(status),
            wstopsig(status),
            status >> 16
        );
    }
    if wif_exited(status) || wif_signaled(status) {
        if trace_enabled() {
            eprintln!(
                "TRACE exit task={task} status={status:#x} code={} signal={}",
                wexit_status(status),
                wtermsig(status)
            );
        }
        state.live.remove(&task);
        if task == state.root_pid {
            state.root_exit = if wif_exited(status) {
                Exit::Code(wexit_status(status))
            } else {
                Exit::Signal(wtermsig(status))
            };
            state.root_exited_at = Some(Instant::now());
        }
        return;
    }
    if !wif_stopped(status) {
        return;
    }
    let signal = wstopsig(status);
    let event = status >> 16;
    if event == PTRACE_EVENT_EXEC {
        handle_exec(task, state, log);
        resume(task, 0);
    } else if matches!(
        event,
        PTRACE_EVENT_FORK | PTRACE_EVENT_VFORK | PTRACE_EVENT_CLONE
    ) {
        if let Some(child) = event_message(task) {
            if child > 0 {
                // `task` may be a helper thread; record the parent process id
                // so the log matches `/proc/<pid>/status` PPid semantics.
                state
                    .parents
                    .insert(child, process_tgid(task).unwrap_or(task));
                state.live.insert(child);
                let kind = match event {
                    PTRACE_EVENT_FORK => "fork",
                    PTRACE_EVENT_VFORK => "vfork",
                    PTRACE_EVENT_CLONE => "clone",
                    _ => "unknown",
                };
                if trace_enabled() {
                    eprintln!("TRACE fork parent={task} child={child} kind={kind}");
                }
                log.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .forks
                    .push(ForkRecord {
                        parent: task,
                        child,
                        kind,
                    });
            }
        }
        resume(task, 0);
    } else if event == PTRACE_EVENT_VFORK_DONE {
        resume(task, 0);
    } else {
        resume(task, delivered_signal(signal));
    }
}

fn handle_exec(task: i32, state: &mut LoopState, log: &Arc<Mutex<ExecLog>>) {
    // A non-leader thread that execs becomes the new thread-group leader; the
    // kernel reports the former tid so the parent relation can be remapped.
    if let Some(former) = event_message(task) {
        if former > 0 && former != task {
            if let Some(parent) = state.parents.remove(&former) {
                state.parents.insert(task, parent);
            }
            state.live.remove(&former);
            if state.root_pid == former {
                state.root_pid = task;
            }
        }
    }
    state.live.insert(task);
    let ppid = state
        .parents
        .get(&task)
        .copied()
        .or_else(|| process_ppid(task))
        .unwrap_or(0);
    let record = read_exec_record(task, ppid, task == state.root_pid);
    let mut log = log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if task == state.root_pid {
        log.root_pid = task;
    }
    log.execs.push(record);
}

/// Re-adopts threads of traced thread groups that are missing from `live`.
///
/// `PTRACE_O_TRACECLONE` normally reports every new thread, but a waitpid race
/// (ECHILD before the auto-attach is complete) can drop the tid from `live`.
/// The kernel keeps such a thread stopped until the tracer resumes it, so the
/// thread-group listing is the authoritative recovery path.
fn discover_threads(state: &mut LoopState, log: &Arc<Mutex<ExecLog>>) -> bool {
    let tgids: BTreeSet<i32> = state
        .live
        .iter()
        .filter_map(|task| process_tgid(*task))
        .collect();
    let mut progressed = false;
    for tgid in tgids {
        let Ok(entries) = fs::read_dir(format!("/proc/{tgid}/task")) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(tid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            if state.live.contains(&tid) {
                continue;
            }
            let mut status = 0;
            let waited = unsafe { waitpid(tid, &raw mut status, __WALL | WNOHANG) };
            if waited == tid {
                if trace_enabled() {
                    eprintln!("TRACE adopt task={tid} status={status:#x}");
                }
                state.live.insert(tid);
                handle_wait(tid, status, state, log);
                progressed = true;
            } else if waited == 0 {
                // A tracee without a pending stop; track it for its exit.
                state.live.insert(tid);
            }
        }
    }
    progressed
}

fn drain(state: &mut LoopState) {
    let deadline = Instant::now() + DRAIN_GRACE;
    while !state.live.is_empty() && Instant::now() < deadline {
        let snapshot: Vec<i32> = state.live.iter().copied().collect();
        let mut reaped = false;
        for task in snapshot {
            let mut status = 0;
            let waited = unsafe { waitpid(task, &raw mut status, __WALL | WNOHANG) };
            if waited == task {
                reaped = true;
                if wif_exited(status) || wif_signaled(status) {
                    state.live.remove(&task);
                } else {
                    resume(task, 0);
                }
            } else if waited < 0 {
                state.live.remove(&task);
            }
        }
        if !reaped {
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn trace_enabled() -> bool {
    std::env::var_os("WXML_EXEC_AUDIT_TRACE").is_some()
}

fn resume(task: i32, signal: c_int) {
    let result = unsafe { ptrace(PTRACE_CONT, task, 0_usize, signal as usize) };
    if trace_enabled() {
        eprintln!("TRACE continue task={task} signal={signal} result={result}");
    }
    if result < 0 {
        let errno = io::Error::last_os_error();
        if errno.raw_os_error() != Some(3) {
            // ESRCH is expected when the task exited under us.
            eprintln!("EXEC-AUDIT: PTRACE_CONT({task}) failed: {errno}");
        }
    }
}

fn event_message(task: i32) -> Option<i32> {
    let mut message: c_long = 0;
    let result = unsafe {
        ptrace(
            PTRACE_GETEVENTMSG,
            task,
            0_usize,
            std::ptr::addr_of_mut!(message),
        )
    };
    if result < 0 {
        None
    } else {
        i32::try_from(message).ok()
    }
}

fn kill_group(pid: i32) {
    let _ = unsafe { kill(-pid, SIGKILL) };
    let _ = unsafe { kill(pid, SIGKILL) };
}

/// Signals that stop the tracee but are part of normal ptrace/fork handshakes.
fn delivered_signal(signal: c_int) -> c_int {
    if matches!(
        signal,
        SIGSTOP | SIGTRAP | SIGCHLD | SIGCONT | SIGTSTP | SIGTTIN | SIGTTOU
    ) {
        0
    } else {
        signal
    }
}

fn classify_errno(errno: Option<i32>, operation: &str) -> TraceError {
    let message = format!(
        "{operation} failed with errno {} ({})",
        errno.map_or_else(|| "?".to_owned(), |code| code.to_string()),
        errno.map_or_else(
            || "unknown".to_owned(),
            |code| io::Error::from_raw_os_error(code).to_string()
        )
    );
    match errno {
        // EPERM/EACCES: Yama, seccomp or another LSM denied ptrace.
        // ENOSYS: the kernel has no ptrace.
        Some(1 | 13 | 38) => TraceError::Blocked(message),
        _ => TraceError::Failed(message),
    }
}

// ---------------------------------------------------------------------------
// /proc snapshot
// ---------------------------------------------------------------------------

fn read_exec_record(pid: i32, ppid: i32, root: bool) -> ExecRecord {
    let exe = fs::read_link(format!("/proc/{pid}/exe"))
        .unwrap_or_else(|_| PathBuf::from(format!("<gone:{pid}>")));
    let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
        .map_or_else(|_| "<gone>".to_owned(), |text| text.trim().to_owned());
    let argv = read_cmdline(pid);
    let env_keys = read_env_keys(pid);
    ExecRecord {
        pid,
        ppid,
        root,
        exe,
        comm,
        argv,
        env_keys,
    }
}

fn read_cmdline(pid: i32) -> Vec<String> {
    let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return Vec::new();
    };
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .take(64)
        .map(|part| String::from_utf8_lossy(&part[..part.len().min(4096)]).into_owned())
        .collect()
}

fn read_env_keys(pid: i32) -> Vec<String> {
    let Ok(bytes) = fs::read(format!("/proc/{pid}/environ")) else {
        return Vec::new();
    };
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for entry in bytes.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let key = entry
            .split(|byte| *byte == b'=')
            .next()
            .map(|key| String::from_utf8_lossy(&key[..key.len().min(256)]).into_owned())
            .unwrap_or_default();
        if !key.is_empty() {
            keys.insert(key);
        }
    }
    keys.into_iter().collect()
}

fn process_ppid(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("PPid:")
            .and_then(|rest| rest.trim().parse::<i32>().ok())
    })
}

/// Thread-group id of a pid/tid from `/proc/<pid>/status`.
fn process_tgid(pid: i32) -> Option<i32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("Tgid:")
            .and_then(|rest| rest.trim().parse::<i32>().ok())
    })
}

// ---------------------------------------------------------------------------
// Forked child
// ---------------------------------------------------------------------------

struct Prepared {
    plan: ChildPlan,
    stdin_child: File,
    stdin_parent: Option<File>,
    stdout_child: File,
    stdout_parent: File,
    stderr_child: File,
    stderr_parent: File,
    status_read: File,
    status_write: File,
}

impl Prepared {
    fn new(spec: &Spec) -> Result<Self, TraceError> {
        let plan = ChildPlan::new(spec)?;
        let (stdin_child, stdin_parent) = match spec.stdin {
            StdinMode::Null => (
                File::open("/dev/null")
                    .map_err(|error| TraceError::Failed(format!("open /dev/null: {error}")))?,
                None,
            ),
            StdinMode::Piped => {
                let (reader, writer) = pipe().map_err(pipe_error)?;
                (reader, Some(writer))
            }
        };
        let (stdout_parent, stdout_child) = pipe().map_err(pipe_error)?;
        let (stderr_parent, stderr_child) = pipe().map_err(pipe_error)?;
        let (status_read, status_write) = pipe().map_err(pipe_error)?;
        Ok(Self {
            plan,
            stdin_child,
            stdin_parent,
            stdout_child,
            stdout_parent,
            stderr_child,
            stderr_parent,
            status_read,
            status_write,
        })
    }
}

/// Anonymous pipe as two `File`s.
fn pipe() -> io::Result<(File, File)> {
    let mut fds = [0 as c_int; 2];
    // SAFETY: `fds` is a valid two-element array; `pipe` writes both ends.
    let result = unsafe { c_pipe(fds.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe` returned two freshly owned descriptors.
    let reader = unsafe { File::from_raw_fd(fds[0]) };
    // SAFETY: same as above for the write end.
    let writer = unsafe { File::from_raw_fd(fds[1]) };
    Ok((reader, writer))
}

fn pipe_error(error: io::Error) -> TraceError {
    TraceError::Failed(format!("cannot create trace pipes: {error}"))
}

/// Pre-built C strings and NUL-terminated pointer arrays.
struct ChildPlan {
    program: CString,
    cwd: CString,
    /// NUL-terminated argument vector.
    argv: Vec<*const c_char>,
    /// NUL-terminated environment vector.
    envp: Vec<*const c_char>,
    _argv_storage: Vec<CString>,
    _env_storage: Vec<CString>,
}

impl ChildPlan {
    fn new(spec: &Spec) -> Result<Self, TraceError> {
        let program = CString::new(spec.program.as_os_str().as_bytes())
            .map_err(|_| TraceError::Failed("program path contains NUL".to_owned()))?;
        let cwd = CString::new(spec.cwd.as_os_str().as_bytes())
            .map_err(|_| TraceError::Failed("working directory contains NUL".to_owned()))?;
        let mut argv_storage = Vec::with_capacity(spec.argv.len() + 1);
        for argument in &spec.argv {
            argv_storage.push(
                CString::new(argument.as_os_str().as_bytes())
                    .map_err(|_| TraceError::Failed("argv contains NUL".to_owned()))?,
            );
        }
        let mut env_storage = Vec::with_capacity(spec.env.len() + 1);
        for (key, value) in &spec.env {
            let mut entry = Vec::new();
            entry.extend_from_slice(key.as_os_str().as_bytes());
            entry.push(b'=');
            entry.extend_from_slice(value.as_os_str().as_bytes());
            env_storage.push(
                CString::new(entry)
                    .map_err(|_| TraceError::Failed("environment contains NUL".to_owned()))?,
            );
        }
        let argv: Vec<*const c_char> = argv_storage
            .iter()
            .map(|argument| argument.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let envp: Vec<*const c_char> = env_storage
            .iter()
            .map(|variable| variable.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        Ok(Self {
            program,
            cwd,
            argv,
            envp,
            _argv_storage: argv_storage,
            _env_storage: env_storage,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn child_exec(
    stdin_parent_fd: RawFd,
    stdout_parent_fd: RawFd,
    stderr_parent_fd: RawFd,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    status_read_fd: RawFd,
    status_write_fd: RawFd,
    program: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
    cwd: *const c_char,
) -> ! {
    // SAFETY: the process is a post-fork child holding only one thread. Every
    // call below is async-signal-safe; no Rust allocation or panic can occur.
    unsafe {
        if setpgid(0, 0) != 0 {
            _exit(125);
        }
        for fd in [
            stdin_parent_fd,
            stdout_parent_fd,
            stderr_parent_fd,
            status_read_fd,
        ] {
            if fd > 2 {
                close(fd);
            }
        }
        move_fd(stdin_fd, 0);
        move_fd(stdout_fd, 1);
        move_fd(stderr_fd, 2);
        if chdir(cwd) != 0 {
            write_code(status_write_fd, 201);
            _exit(125);
        }
        if ptrace(PTRACE_TRACEME, 0 as c_int, 0_usize, 0_usize) != 0 {
            let errno = *__errno_location();
            write_code(status_write_fd, u8::try_from(errno).unwrap_or(255));
            close(status_write_fd);
            _exit(126);
        }
        write_code(status_write_fd, 0);
        close(status_write_fd);
        raise(SIGSTOP);
        execve(program, argv, envp);
        _exit(127);
    }
}

/// `dup2` a child-side fd onto a standard fd, closing the original if needed.
fn move_fd(from: RawFd, to: RawFd) {
    if from == to {
        return;
    }
    if from < 0 {
        return;
    }
    let result = unsafe { dup2(from, to) };
    if result >= 0 && from > 2 {
        let _ = unsafe { close(from) };
    }
}

fn write_code(fd: RawFd, code: u8) {
    if fd < 0 {
        return;
    }
    let _ = unsafe {
        write(
            fd,
            std::ptr::addr_of!(code).cast::<c_void>(),
            size_of::<u8>(),
        )
    };
}

// ---------------------------------------------------------------------------
// wait status decoding
// ---------------------------------------------------------------------------

fn wif_exited(status: c_int) -> bool {
    status & 0x7f == 0
}

fn wexit_status(status: c_int) -> i32 {
    (status >> 8) & 0xff
}

fn wif_signaled(status: c_int) -> bool {
    ((((status & 0x7f) + 1) as i8) >> 1) > 0
}

fn wtermsig(status: c_int) -> i32 {
    status & 0x7f
}

fn wif_stopped(status: c_int) -> bool {
    status & 0xff == 0x7f
}

fn wstopsig(status: c_int) -> c_int {
    (status >> 8) & 0xff
}

// ---------------------------------------------------------------------------
// Output stream collection
// ---------------------------------------------------------------------------

struct Stream {
    receiver: Receiver<Vec<u8>>,
    handle: JoinHandle<()>,
}

fn collect_stream(mut reader: impl Read + Send + 'static) -> Stream {
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut collected = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    // Bound memory for a chatty child while still draining.
                    if collected.len() < 8 * 1024 * 1024 {
                        collected.extend_from_slice(&buffer[..read]);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = sender.send(collected);
    });
    Stream { receiver, handle }
}

impl Stream {
    fn finish(self) -> Vec<u8> {
        let collected = match self.receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(collected) => collected,
            Err(error) => panic!("traced output stream did not close: {error}"),
        };
        let _ = self.handle.join();
        collected
    }
}
