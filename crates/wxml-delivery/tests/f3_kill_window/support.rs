//! Gates, preflight and evidence helpers for the F3-06/F3-09 kill windows.
//!
//! The hard gate is `WXML_REQUIRE_KILL_WINDOW=1`; the two existing F3 gates
//! (`WXML_REQUIRE_REAL_ENGINE=1`, `WXML_REQUIRE_EXEC_AUDIT=1`) also fail closed
//! so a CI matrix cannot silently turn a blocked ptrace host into a pass.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::gate::KillGate;
use crate::tracer::{
    self, Decision, Exit, Gate, Outcome, Phase, Spec, SyscallEvent, SyscallFamily, TraceError,
};

/// Hard gate for this suite.
pub const REQUIRE_KILL_WINDOW_ENV: &str = "WXML_REQUIRE_KILL_WINDOW";
/// Upper bound for one traced installer process.
pub const KILL_TIMEOUT: Duration = Duration::from_secs(300);

static PREFLIGHT: OnceLock<Result<(), TraceError>> = OnceLock::new();
static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn hard_gate_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value.is_empty() => false,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
        Ok(value) => panic!("{name} must be unset, empty, `1` or `true`; got `{value}`"),
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} must be valid UTF-8"),
    }
}

/// True when a missing capability must fail instead of skip.
pub fn required_mode() -> bool {
    hard_gate_enabled(REQUIRE_KILL_WINDOW_ENV)
        || hard_gate_enabled("WXML_REQUIRE_REAL_ENGINE")
        || hard_gate_enabled("WXML_REQUIRE_EXEC_AUDIT")
}

/// Prints one `F3-EVIDENCE` line.
pub fn evidence(id: &str, verdict: &str, detail: impl std::fmt::Display) {
    println!("F3-EVIDENCE: {id} {verdict} :: {detail}");
}

/// Records an explicitly skipped real-asset or fixture case.
pub fn skip(id: &str, detail: impl std::fmt::Display) {
    println!("F3-EVIDENCE: {id} SKIP :: {detail}");
}

/// Reports a blocked host; a hard gate turns the block into a failure.
pub fn blocked(id: &str, reason: &str) {
    assert!(
        !required_mode(),
        "{id}: KILL-WINDOW BLOCKED and {} / WXML_REQUIRE_REAL_ENGINE is set: {reason}",
        REQUIRE_KILL_WINDOW_ENV
    );
    println!("F3-EVIDENCE: {id} BLOCKED :: {reason}");
}

/// Gate that never matches, used by the preflight and the control run.
struct NeverGate;

impl Gate for NeverGate {
    fn interested(&self, _family: SyscallFamily, _phase: Phase) -> bool {
        false
    }

    fn evaluate(&mut self, _event: &SyscallEvent) -> Decision {
        Decision::Ignore
    }
}

fn scratch_paths(label: &str) -> (PathBuf, PathBuf) {
    let directory = std::env::var_os("WXML_TEST_TMPDIR")
        .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
    let _ = fs::create_dir_all(&directory);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let count = OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let safe_label: String = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let base = directory.join(format!(
        "kill-window-{safe_label}-{}-{count}-{nonce}",
        std::process::id()
    ));
    (base.with_extension("stdout"), base.with_extension("stderr"))
}

fn preflight() -> Result<(), TraceError> {
    if std::env::var_os("WXML_KILL_WINDOW_FORCE_BLOCKED").is_some() {
        return Err(TraceError::Blocked(
            "forced by WXML_KILL_WINDOW_FORCE_BLOCKED".to_owned(),
        ));
    }
    let (stdout_path, stderr_path) = scratch_paths("preflight");
    let cwd = std::env::temp_dir();
    let spec = Spec::new("/bin/true", &cwd)
        .envs([("PATH", "/usr/bin:/bin"), ("LANG", "C")])
        .output_paths(stdout_path.clone(), stderr_path.clone());
    let result = tracer::run(&spec, &mut NeverGate, Duration::from_secs(10));
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    let outcome = result?;
    if !outcome.exit.success() {
        return Err(TraceError::Failed(format!(
            "preflight /bin/true exited {:?}; stderr={}",
            outcome.exit,
            outcome.stderr_text()
        )));
    }
    if outcome.stops == 0 {
        return Err(TraceError::Failed(
            "preflight observed no syscall stop; the tracer would be vacuous".to_owned(),
        ));
    }
    Ok(())
}

/// Runs the tracer preflight once. Returns false after printing `BLOCKED`.
pub fn ensure_tracer(id: &str) -> bool {
    // Evaluate the gates eagerly: an invalid value must fail closed.
    let _ = required_mode();
    match PREFLIGHT.get_or_init(preflight) {
        Ok(()) => true,
        Err(TraceError::Blocked(reason)) => {
            blocked(id, reason);
            false
        }
        Err(TraceError::Failed(reason)) => {
            panic!("{id}: kill-window tracer preflight failed: {reason}")
        }
    }
}

/// Resolves the shared pinned real channel.
pub fn real_channel(id: &str) -> Option<&'static crate::shared::SharedChannel> {
    crate::shared::shared_channel(id)
}

/// Runs the production installer under the syscall tracer.
pub fn traced_installer(
    label: &str,
    install_bin: &Path,
    args: &[&str],
    tripwires: &crate::shared::Tripwires,
    work: &Path,
    gate: &mut KillGate,
) -> Outcome {
    let mut command = crate::release::clean_command(install_bin, tripwires, work);
    command.args(args);
    let env: Vec<(OsString, OsString)> = command
        .get_envs()
        .map(|(key, value)| (key.to_os_string(), value.unwrap_or_default().to_os_string()))
        .collect();
    let (stdout_path, stderr_path) = scratch_paths(label);
    let spec = Spec::new(install_bin, work)
        .args(args.iter().copied())
        .envs(env)
        .output_paths(stdout_path.clone(), stderr_path.clone());
    let outcome = match tracer::run(&spec, gate, KILL_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(TraceError::Blocked(reason)) => {
            blocked(label, &reason);
            panic!("{label}: the traced installer could not start after a successful preflight")
        }
        Err(TraceError::Failed(reason)) => {
            panic!("{label}: cannot start the traced installer: {reason}")
        }
    };
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    outcome
}

/// Prints the observed trace summary for one run.
pub fn print_outcome(label: &str, outcome: &Outcome) {
    println!(
        "KILL-WINDOW: {label}: pid={} exit={:?} killed={} stops={} syscalls={} timed_out={} elapsed={:?}",
        outcome.pid,
        outcome.exit,
        outcome.killed,
        outcome.stops,
        outcome.syscall_summary(),
        outcome.timed_out,
        outcome.elapsed
    );
    for line in outcome.stdout_text().lines() {
        println!("KILL-WINDOW: {label}: stdout: {line}");
    }
    for line in outcome.stderr_text().lines() {
        println!("KILL-WINDOW: {label}: stderr: {line}");
    }
}

/// Asserts the tracee was killed by an external SIGKILL and never reported
/// success.
pub fn assert_killed(label: &str, outcome: &Outcome) {
    assert!(
        !outcome.timed_out,
        "{label}: traced installer timed out; the kill window was not reached"
    );
    assert!(
        outcome.killed,
        "{label}: the gate never fired; last syscalls={}",
        outcome.syscall_summary()
    );
    assert_eq!(
        outcome.exit,
        Exit::Signal(9),
        "{label}: tracee exit {:?}, expected SIGKILL; stdout={} stderr={}",
        outcome.exit,
        outcome.stdout_text(),
        outcome.stderr_text()
    );
    let text = format!("{}\n{}", outcome.stdout_text(), outcome.stderr_text());
    for forbidden in ["upgraded to", "migrated to", "rolled back to", "installed "] {
        assert!(
            !text.contains(forbidden),
            "{label}: the killed process printed `{forbidden}`; it was not killed in the \
             intended window: {text}"
        );
    }
}

/// Formats the first (and only) hit of a gate.
pub fn hit_detail(gate: &KillGate) -> String {
    gate.hits()
        .first()
        .map_or_else(|| "<no hit>".to_owned(), crate::gate::Hit::line)
}

/// Runs `recover` and returns its output.
pub fn recover(
    install_bin: &Path,
    root: &Path,
    tripwires: &crate::shared::Tripwires,
    work: &Path,
) -> std::process::Output {
    crate::shared::run_installer(
        install_bin,
        &["recover", "--root", root.to_str().expect("UTF-8 root")],
        tripwires,
        work,
        "recover after SIGKILL",
    )
}
