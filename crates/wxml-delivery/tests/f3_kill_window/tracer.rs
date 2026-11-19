//! Minimal Linux x86_64 `ptrace` syscall-stop tracer (no Cargo dependency).
//!
//! Unlike the F3-02 exec auditor, this tracer follows a *single* root process
//! and stops it at every system-call entry and exit with `PTRACE_SYSCALL`.
//! That gives the parent two kernel-defined boundaries for each syscall:
//!
//! - entry-stop: before the kernel executes the syscall;
//! - exit-stop: after the syscall completed, before the tracee runs user code.
//!
//! The parent reads the register file (`PTRACE_GETREGS`) and the path strings
//! (`PTRACE_PEEKDATA`), hands the decoded event to a [`Gate`], and — when the
//! gate returns [`Decision::Kill`] — sends an *external* `SIGKILL` to the
//! process group. The tracee never gets to run cleanup code, which is exactly
//! the process-crash window the F3-06/F3-09 evidence needs.
//!
//! Safety: every `unsafe` block exists only to call `fork`/`ptrace`/`execve`/
//! `waitpid`/`kill`/signal helpers. The forked child only calls
//! async-signal-safe libc entry points; all C strings and pointer arrays are
//! built before the fork.

use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const PTRACE_TRACEME: c_uint = 0;
const PTRACE_PEEKDATA: c_uint = 2;
const PTRACE_SYSCALL: c_uint = 24;
const PTRACE_GETREGS: c_uint = 12;
const PTRACE_SETOPTIONS: c_uint = 0x4200;

const PTRACE_O_TRACESYSGOOD: usize = 0x0000_0001;
const PTRACE_O_EXITKILL: usize = 0x0010_0000;

/// `__WALL` from `<linux/wait.h>`: wait for all children, including threads.
const __WALL: c_int = 0x4000_0000;

const SIGTRAP: c_int = 5;
const SIGKILL: c_int = 9;
const SIGCHLD: c_int = 17;
const SIGCONT: c_int = 18;
const SIGSTOP: c_int = 19;
const SIGTSTP: c_int = 20;
const SIGTTIN: c_int = 21;
const SIGTTOU: c_int = 22;

/// `SIGTRAP | 0x80`, the marker `PTRACE_O_TRACESYSGOOD` adds to syscall stops.
const SYSCALL_TRAP: c_int = SIGTRAP | 0x80;

/// x86_64 syscall numbers the gate can observe.
const SYS_RENAME: i64 = 82;
const SYS_SYMLINK: i64 = 88;
const SYS_RENAMEAT: i64 = 264;
const SYS_SYMLINKAT: i64 = 266;
const SYS_RENAMEAT2: i64 = 316;
const SYS_FSYNC: i64 = 74;

/// `AT_FDCWD` from `<fcntl.h>`.
const AT_FDCWD: i64 = -100;

const PATH_MAX: usize = 4096;

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

/// The `user_regs_struct` layout used by `PTRACE_GETREGS` on x86_64.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UserRegs {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub eflags: u64,
    pub rsp: u64,
    pub ss: u64,
    pub fs_base: u64,
    pub gs_base: u64,
    pub ds: u64,
    pub es: u64,
    pub fs: u64,
    pub gs: u64,
}

/// Why the trace could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    /// ptrace/`/proc` unavailable (`seccomp`, `Yama`, `ptrace_scope=2`).
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
            Self::Blocked(reason) => write!(formatter, "kill-window tracer blocked: {reason}"),
            Self::Failed(reason) => write!(formatter, "kill-window tracer failed: {reason}"),
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

/// Which side of a syscall stop the parent observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Entry,
    Exit,
}

impl Phase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Exit => "exit",
        }
    }
}

/// The syscall families the gate understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyscallFamily {
    Rename,
    Symlink,
    Fsync,
    Other,
}

impl SyscallFamily {
    pub const fn from_number(number: i64) -> Self {
        match number {
            SYS_RENAME | SYS_RENAMEAT | SYS_RENAMEAT2 => Self::Rename,
            SYS_SYMLINK | SYS_SYMLINKAT => Self::Symlink,
            SYS_FSYNC => Self::Fsync,
            _ => Self::Other,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Symlink => "symlink",
            Self::Fsync => "fsync",
            Self::Other => "other",
        }
    }
}

/// One decoded syscall stop handed to the gate.
#[derive(Debug, Clone)]
pub struct SyscallEvent {
    pub pid: i32,
    pub phase: Phase,
    pub family: SyscallFamily,
    pub number: i64,
    /// First path argument (`rename` old, `symlink` target, `fsync` none).
    pub old_path: Option<PathBuf>,
    /// Second path argument (`rename` new, `symlink` link path).
    pub new_path: Option<PathBuf>,
    /// Raw first two register arguments (also the `fsync` fd in `args[0]`).
    pub args: [u64; 4],
}

impl SyscallEvent {
    pub fn syscall_label(&self) -> &'static str {
        match self.number {
            SYS_RENAME => "rename",
            SYS_RENAMEAT => "renameat",
            SYS_RENAMEAT2 => "renameat2",
            SYS_SYMLINK => "symlink",
            SYS_SYMLINKAT => "symlinkat",
            SYS_FSYNC => "fsync",
            _ => "other",
        }
    }

    pub fn paths_summary(&self) -> String {
        match (&self.old_path, &self.new_path) {
            (Some(old), Some(new)) => format!("{} -> {}", old.display(), new.display()),
            (None, Some(new)) => new.display().to_string(),
            (Some(old), None) => old.display().to_string(),
            (None, None) => format!("fd={}", self.args[0]),
        }
    }
}

/// What the tracer should do after a gate evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Not this stop; keep tracing.
    Ignore,
    /// The window matches; send SIGKILL immediately.
    Kill,
    /// The path matched but the disk state did not; keep tracing and fail later.
    Reject(String),
}

/// A gate decides which syscall stop is the kill window.
pub trait Gate {
    /// True when this family/phase pair can be a kill window. Used to avoid
    /// decoding paths for unrelated syscalls.
    fn interested(&self, family: SyscallFamily, phase: Phase) -> bool;

    /// Evaluate one matching stop. The gate records its own evidence.
    fn evaluate(&mut self, event: &SyscallEvent) -> Decision;
}

/// One traced invocation.
#[derive(Debug, Clone)]
pub struct Spec {
    pub program: PathBuf,
    pub argv: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
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
            stdout_path: PathBuf::new(),
            stderr_path: PathBuf::new(),
        }
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.argv.extend(arguments.into_iter().map(Into::into));
        self
    }

    pub fn envs<I, K, V>(mut self, variables: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.env.extend(
            variables
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    pub fn output_paths(
        mut self,
        stdout_path: impl Into<PathBuf>,
        stderr_path: impl Into<PathBuf>,
    ) -> Self {
        self.stdout_path = stdout_path.into();
        self.stderr_path = stderr_path.into();
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
        if self.stdout_path.as_os_str().is_empty() || self.stderr_path.as_os_str().is_empty() {
            return Err(TraceError::Failed(
                "stdout_path and stderr_path are required".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Result of tracing one invocation.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub pid: i32,
    pub exit: Exit,
    pub timed_out: bool,
    /// True when a gate returned [`Decision::Kill`].
    pub killed: bool,
    pub stops: u64,
    /// Counts of observed interesting syscalls by family name.
    pub syscall_counts: BTreeMap<String, u64>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

impl Outcome {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    pub fn syscall_summary(&self) -> String {
        self.syscall_counts
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Traces `spec` to completion, killing it when `gate` requests a kill.
///
/// # Errors
///
/// Returns [`TraceError::Blocked`] when this host denies `PTRACE_TRACEME` or
/// `PTRACE_SETOPTIONS`, and [`TraceError::Failed`] for internal errors.
pub fn run(spec: &Spec, gate: &mut dyn Gate, timeout: Duration) -> Result<Outcome, TraceError> {
    spec.validate()?;
    let prepared = Prepared::new(spec)?;
    let started = Instant::now();
    let Prepared {
        plan,
        stdin,
        stdout,
        stderr,
        status_read,
        status_write,
    } = prepared;
    let stdin_fd = stdin.as_raw_fd();
    let stdout_fd = stdout.as_raw_fd();
    let stderr_fd = stderr.as_raw_fd();
    let status_write_fd = status_write.as_raw_fd();
    let program = plan.program.as_ptr();
    let argv = plan.argv.as_ptr();
    let envp = plan.envp.as_ptr();
    let cwd = plan.cwd.as_ptr();

    let pid = unsafe { fork() };
    if pid < 0 {
        return Err(TraceError::Failed(format!(
            "fork failed: {}",
            io::Error::last_os_error()
        )));
    }
    if pid == 0 {
        // SAFETY: post-fork child, only async-signal-safe libc calls below.
        child_exec(
            stdin_fd,
            stdout_fd,
            stderr_fd,
            status_write_fd,
            program,
            argv,
            envp,
            cwd,
        );
    }
    drop(stdin);
    drop(stdout);
    drop(stderr);
    drop(status_write);
    // Keep the pre-built C strings alive for the whole trace.
    let plan_storage = plan;

    let mut guard = ChildGuard::new(pid);
    let mut status_read = status_read;
    let mut setup_code = [0_u8; 1];
    match status_read.read_exact(&mut setup_code) {
        Ok(()) => {}
        Err(error) => {
            let mut status = 0;
            unsafe { waitpid(pid, &raw mut status, 0) };
            return Err(TraceError::Failed(format!(
                "tracee did not report ptrace setup: {error}"
            )));
        }
    }
    if setup_code[0] != 0 {
        let mut status = 0;
        unsafe { waitpid(pid, &raw mut status, 0) };
        return Err(if setup_code[0] == 1 {
            TraceError::Blocked("ptrace(PTRACE_TRACEME) is not permitted on this host".to_owned())
        } else {
            TraceError::Blocked(format!(
                "tracee setup failed (code {}); ptrace may be restricted by seccomp/Yama",
                setup_code[0]
            ))
        });
    }

    let mut status = 0;
    let waited = unsafe { waitpid(pid, &raw mut status, 0) };
    if waited != pid {
        return Err(TraceError::Failed(format!(
            "initial waitpid failed: {}",
            io::Error::last_os_error()
        )));
    }
    if !wif_stopped(status) {
        return Err(TraceError::Failed(format!(
            "tracee exited before the initial stop (status {status:#x}); execve probably failed"
        )));
    }
    let options = PTRACE_O_TRACESYSGOOD | PTRACE_O_EXITKILL;
    if unsafe { ptrace(PTRACE_SETOPTIONS, pid, 0_usize, options) } < 0 {
        let errno = io::Error::last_os_error();
        let mut status = 0;
        unsafe {
            kill(pid, SIGKILL);
            waitpid(pid, &raw mut status, 0);
        }
        return Err(classify_errno(errno.raw_os_error(), "PTRACE_SETOPTIONS"));
    }

    let expired = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watchdog_expired = Arc::clone(&expired);
    let watchdog = thread::Builder::new()
        .name("f3-kill-window-watchdog".to_owned())
        .spawn(move || {
            if done_rx.recv_timeout(timeout).is_err() {
                watchdog_expired.store(true, Ordering::SeqCst);
                kill_group(pid);
            }
        })
        .map_err(|error| TraceError::Failed(format!("cannot start watchdog: {error}")))?;

    let mut stops = 0_u64;
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut killed = false;
    let mut pending: Option<PendingStop> = None;
    let mut exit = Exit::Unknown;

    // Resume the tracee from the initial `SIGSTOP` into its `execve`.
    if gate_stop(pid, 0).is_err() {
        let _ = done_tx.send(());
        let _ = watchdog.join();
        guard.disarm();
        return Err(TraceError::Failed(
            "cannot resume the tracee after the initial stop".to_owned(),
        ));
    }

    loop {
        let mut task_status = 0;
        let waited = wait_pid(pid, &mut task_status);
        if waited <= 0 {
            break;
        }
        if wif_exited(task_status) {
            exit = Exit::Code(wexit_status(task_status));
            break;
        }
        if wif_signaled(task_status) {
            exit = Exit::Signal(wtermsig(task_status));
            break;
        }
        if !wif_stopped(task_status) {
            continue;
        }
        let signal = wstopsig(task_status);
        let event = task_status >> 16;
        let mut deliver = 0;
        if signal == SYSCALL_TRAP {
            stops = stops.saturating_add(1);
            let Some(regs) = get_regs(pid) else {
                pending = None;
                if gate_stop(pid, 0).is_err() {
                    break;
                }
                continue;
            };
            let number = i64::try_from(regs.orig_rax).unwrap_or(-1);
            let family = SyscallFamily::from_number(number);
            if family != SyscallFamily::Other {
                *counts.entry(family.as_str().to_owned()).or_insert(0) += 1;
            }
            // x86_64 marks a syscall-entry stop with `rax = -ENOSYS`.
            let phase = if regs.rax as i64 == -38 {
                Phase::Entry
            } else {
                Phase::Exit
            };
            let decoded = if phase == Phase::Entry {
                let decoded = decode_paths(pid, &regs, family, number);
                pending = Some(PendingStop {
                    number,
                    old_path: decoded.old_path.clone(),
                    new_path: decoded.new_path.clone(),
                });
                decoded
            } else {
                pending
                    .take()
                    .filter(|stop| stop.number == number)
                    .map_or_else(
                        || decode_paths(pid, &regs, family, number),
                        |stop| DecodedPaths {
                            old_path: stop.old_path,
                            new_path: stop.new_path,
                        },
                    )
            };
            if trace_enabled() && family != SyscallFamily::Other {
                eprintln!(
                    "TRACE syscall pid={pid} {} #{} phase={} rax={:#x} rdi={:#x} rsi={:#x} rdx={:#x} r10={:#x} old={:?} new={:?}",
                    syscall_name(number),
                    number,
                    phase.as_str(),
                    regs.rax,
                    regs.rdi,
                    regs.rsi,
                    regs.rdx,
                    regs.r10,
                    decoded.old_path,
                    decoded.new_path
                );
            }
            if family != SyscallFamily::Other && gate.interested(family, phase) {
                let event = SyscallEvent {
                    pid,
                    phase,
                    family,
                    number,
                    old_path: decoded.old_path,
                    new_path: decoded.new_path,
                    args: [regs.rdi, regs.rsi, regs.rdx, regs.r10],
                };
                match gate.evaluate(&event) {
                    Decision::Kill => {
                        killed = true;
                        kill_group(pid);
                    }
                    Decision::Reject(reason) => {
                        eprintln!("KILL-WINDOW: rejected candidate: {reason}");
                    }
                    Decision::Ignore => {}
                }
            }
        } else if event == 0 {
            // Signal-delivery stop: swallow ptrace/process-group handshakes.
            deliver = delivered_signal(signal);
            pending = None;
        } else {
            pending = None;
        }
        if killed {
            break;
        }
        if gate_stop(pid, deliver).is_err() {
            break;
        }
    }

    if killed {
        let mut kill_status = 0;
        loop {
            let waited = wait_pid(pid, &mut kill_status);
            if waited <= 0 || wif_exited(kill_status) || wif_signaled(kill_status) {
                break;
            }
            // Any stop other than death must be resumed before reaping.
            let _ = gate_stop(pid, 0);
        }
        // Clear untraced probe children that share the tracee's process group.
        kill_group(pid);
        if wif_signaled(kill_status) {
            exit = Exit::Signal(wtermsig(kill_status));
        } else if wif_exited(kill_status) {
            exit = Exit::Code(wexit_status(kill_status));
        }
    } else if !expired.load(Ordering::SeqCst) {
        kill_group(pid);
    }

    let _ = done_tx.send(());
    let _ = watchdog.join();
    let timed_out = expired.load(Ordering::SeqCst);
    guard.disarm();

    drop(plan_storage);
    let stdout = fs::read(&spec.stdout_path).unwrap_or_default();
    let stderr = fs::read(&spec.stderr_path).unwrap_or_default();
    Ok(Outcome {
        pid,
        exit,
        timed_out,
        killed,
        stops,
        syscall_counts: counts,
        stdout,
        stderr,
        elapsed: started.elapsed(),
    })
}

/// Resumes a stopped tracee with `PTRACE_SYSCALL`.
fn gate_stop(pid: i32, signal: c_int) -> Result<(), ()> {
    let result = unsafe { ptrace(PTRACE_SYSCALL, pid, 0_usize, signal as usize) };
    if result < 0 {
        let errno = io::Error::last_os_error();
        if errno.raw_os_error() != Some(3) {
            // ESRCH is expected when the task exited under us.
            eprintln!("KILL-WINDOW: PTRACE_SYSCALL({pid}) failed: {errno}");
        }
        return Err(());
    }
    Ok(())
}

fn wait_pid(pid: i32, status: &mut c_int) -> i32 {
    loop {
        let waited = unsafe { waitpid(pid, status, __WALL) };
        if waited >= 0 {
            return waited;
        }
        let errno = io::Error::last_os_error();
        if errno.raw_os_error() != Some(4) {
            // EINTR (4) is retried; everything else ends the trace.
            return -1;
        }
    }
}

fn get_regs(pid: i32) -> Option<UserRegs> {
    let mut regs = UserRegs::default();
    let result = unsafe {
        ptrace(
            PTRACE_GETREGS,
            pid,
            0_usize,
            std::ptr::addr_of_mut!(regs).cast::<c_void>(),
        )
    };
    (result == 0).then_some(regs)
}

struct DecodedPaths {
    old_path: Option<PathBuf>,
    new_path: Option<PathBuf>,
}

struct PendingStop {
    number: i64,
    old_path: Option<PathBuf>,
    new_path: Option<PathBuf>,
}

fn decode_paths(pid: i32, regs: &UserRegs, family: SyscallFamily, number: i64) -> DecodedPaths {
    match family {
        SyscallFamily::Rename => {
            if number == SYS_RENAME {
                DecodedPaths {
                    old_path: read_path(pid, regs.rdi),
                    new_path: read_path(pid, regs.rsi),
                }
            } else {
                DecodedPaths {
                    old_path: read_path_at(pid, regs.rdi, regs.rsi),
                    new_path: read_path_at(pid, regs.rdx, regs.r10),
                }
            }
        }
        SyscallFamily::Symlink => {
            if number == SYS_SYMLINK {
                DecodedPaths {
                    old_path: read_path(pid, regs.rdi),
                    new_path: read_path(pid, regs.rsi),
                }
            } else {
                DecodedPaths {
                    old_path: read_path(pid, regs.rdi),
                    new_path: read_path_at(pid, regs.rsi, regs.rdx),
                }
            }
        }
        SyscallFamily::Fsync | SyscallFamily::Other => DecodedPaths {
            old_path: None,
            new_path: None,
        },
    }
}

/// Resolves a `*at` syscall path against the tracee's `dirfd`.
fn read_path_at(pid: i32, dirfd: u64, path: u64) -> Option<PathBuf> {
    let raw = read_path_bytes(pid, path)?;
    let path = PathBuf::from(OsString::from_vec(raw));
    if path.is_absolute() {
        return Some(path);
    }
    let dirfd = dirfd as i64;
    let base = if dirfd == AT_FDCWD {
        fs::read_link(format!("/proc/{pid}/cwd")).ok()?
    } else {
        fs::read_link(format!("/proc/{pid}/fd/{dirfd}")).ok()?
    };
    Some(base.join(path))
}

fn read_path(pid: i32, pointer: u64) -> Option<PathBuf> {
    read_path_bytes(pid, pointer).map(|bytes| PathBuf::from(OsString::from_vec(bytes)))
}

/// Reads a NUL-terminated string from tracee memory with `PTRACE_PEEKDATA`.
fn read_path_bytes(pid: i32, pointer: u64) -> Option<Vec<u8>> {
    if pointer == 0 {
        return None;
    }
    let mut bytes = Vec::new();
    let mut offset = 0_u64;
    while bytes.len() < PATH_MAX {
        let address = pointer.checked_add(offset)?;
        let mut word = 0_u64;
        // SAFETY: PTRACE_PEEKDATA reads one aligned word from the tracee's
        // address space. Linux returns the word in the ptrace result (the
        // fourth argument is ignored); some kernels also write it through the
        // out-pointer, so both are accepted.
        unsafe { *__errno_location() = 0 };
        let result = unsafe {
            ptrace(
                PTRACE_PEEKDATA,
                pid,
                usize::try_from(address).ok()?,
                std::ptr::addr_of_mut!(word).cast::<c_void>(),
            )
        };
        let errno = unsafe { *__errno_location() };
        if result == -1 && errno != 0 {
            return None;
        }
        let word = if word == 0 && result > 0 {
            result as u64
        } else {
            word
        };
        for byte in word.to_ne_bytes() {
            if byte == 0 {
                return Some(bytes);
            }
            bytes.push(byte);
            if bytes.len() >= PATH_MAX {
                return Some(bytes);
            }
        }
        offset = offset.checked_add(8)?;
    }
    Some(bytes)
}

fn trace_enabled() -> bool {
    std::env::var_os("WXML_KILL_WINDOW_TRACE").is_some()
}

const fn syscall_name(number: i64) -> &'static str {
    match number {
        SYS_RENAME => "rename",
        SYS_RENAMEAT => "renameat",
        SYS_RENAMEAT2 => "renameat2",
        SYS_SYMLINK => "symlink",
        SYS_SYMLINKAT => "symlinkat",
        SYS_FSYNC => "fsync",
        _ => "other",
    }
}

fn kill_group(pid: i32) {
    let _ = unsafe { kill(-pid, SIGKILL) };
    let _ = unsafe { kill(pid, SIGKILL) };
}

/// Signals that stop the tracee but belong to the ptrace/fork handshake.
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

/// Kills the tracee if the tracer unwinds or panics mid-run.
struct ChildGuard {
    pid: i32,
    armed: bool,
}

impl ChildGuard {
    const fn new(pid: i32) -> Self {
        Self { pid, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_group(self.pid);
        }
    }
}

// ---------------------------------------------------------------------------
// Forked child
// ---------------------------------------------------------------------------

struct Prepared {
    plan: ChildPlan,
    stdin: File,
    stdout: File,
    stderr: File,
    status_read: File,
    status_write: File,
}

impl Prepared {
    fn new(spec: &Spec) -> Result<Self, TraceError> {
        let plan = ChildPlan::new(spec)?;
        let stdin = File::open("/dev/null")
            .map_err(|error| TraceError::Failed(format!("open /dev/null: {error}")))?;
        let stdout = File::create(&spec.stdout_path).map_err(|error| {
            TraceError::Failed(format!("create {}: {error}", spec.stdout_path.display()))
        })?;
        let stderr = File::create(&spec.stderr_path).map_err(|error| {
            TraceError::Failed(format!("create {}: {error}", spec.stderr_path.display()))
        })?;
        let (status_read, status_write) = pipe().map_err(pipe_error)?;
        Ok(Self {
            plan,
            stdin,
            stdout,
            stderr,
            status_read,
            status_write,
        })
    }
}

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
    TraceError::Failed(format!("cannot create trace status pipe: {error}"))
}

/// Pre-built C strings and NUL-terminated pointer arrays.
struct ChildPlan {
    program: CString,
    cwd: CString,
    argv: Vec<*const c_char>,
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

fn child_exec(
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
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

fn move_fd(from: RawFd, to: RawFd) {
    if from == to || from < 0 {
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
