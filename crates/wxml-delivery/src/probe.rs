//! Sanitized probe execution with bounded output and process-group cleanup.
//!
//! Version probes run before activation, so the runner must not let a hostile
//! or broken binary block the installer forever or fill memory. Each probe runs
//! in its own process group on Unix; on timeout the whole group is killed and
//! both pipes are drained concurrently into fixed-size buffers.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{DeliveryError, Result};

/// Maximum bytes retained from each probe stream (stdout and stderr separately).
pub const MAX_PROBE_OUTPUT_BYTES: usize = 64 * 1024;

/// Runs a command with piped output, a hard timeout and a process-group kill.
///
/// # Errors
///
/// Returns an error when the process cannot start, times out, exits with an
/// unreadable status/output or produces more than the bounded output limit.
pub fn run_probe(command: &mut Command, timeout: Duration) -> Result<Output> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| DeliveryError::Runtime(format!("could not execute a probe: {error}")))?;
    let pid = child.id();
    let stdout_reader = child.stdout.take().map(spawn_reader);
    let stderr_reader = child.stderr.take().map(spawn_reader);
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The leader has exited. Pipes normally close immediately; if a
                // grandchild still holds one open, kill the process group after
                // a short grace period. The external kill helper is not spawned
                // on the normal path, keeping the probe's exec set minimal.
                wait_for_readers(pid, stdout_reader.as_ref(), stderr_reader.as_ref());
                let stdout = join_reader(stdout_reader)?;
                let stderr = join_reader(stderr_reader)?;
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                kill_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                kill_group(pid);
                let _ = join_reader(stdout_reader);
                let _ = join_reader(stderr_reader);
                return Err(DeliveryError::Runtime(
                    "version probe timed out; the probe process did not exit".to_owned(),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                kill_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                kill_group(pid);
                let _ = join_reader(stdout_reader);
                let _ = join_reader(stderr_reader);
                return Err(DeliveryError::Runtime(format!(
                    "could not wait for a probe process: {error}"
                )));
            }
        }
    }
}

fn spawn_reader(mut reader: impl Read + Send + 'static) -> JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut total = 0_usize;
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    total = total.saturating_add(read);
                    if output.len() < MAX_PROBE_OUTPUT_BYTES {
                        let remaining = MAX_PROBE_OUTPUT_BYTES - output.len();
                        output.extend_from_slice(&buffer[..read.min(remaining)]);
                    }
                }
            }
        }
        (output, total > MAX_PROBE_OUTPUT_BYTES)
    })
}

fn join_reader(reader: Option<JoinHandle<(Vec<u8>, bool)>>) -> Result<Vec<u8>> {
    let Some(reader) = reader else {
        return Ok(Vec::new());
    };
    let (bytes, overflow) = reader
        .join()
        .map_err(|_| DeliveryError::Runtime("probe output reader panicked".to_owned()))?;
    if overflow {
        return Err(DeliveryError::Runtime(format!(
            "probe output exceeded {MAX_PROBE_OUTPUT_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn wait_for_readers(
    pid: u32,
    stdout: Option<&JoinHandle<(Vec<u8>, bool)>>,
    stderr: Option<&JoinHandle<(Vec<u8>, bool)>>,
) {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let stdout_done = stdout.is_none_or(JoinHandle::is_finished);
        let stderr_done = stderr.is_none_or(JoinHandle::is_finished);
        if stdout_done && stderr_done {
            return;
        }
        if Instant::now() >= deadline {
            kill_group(pid);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    use std::process::Stdio;
    let group = format!("-{pid}");
    for candidate in ["/bin/kill", "/usr/bin/kill"] {
        if Path::new(candidate).exists() {
            let _ = Command::new(candidate)
                .args(["-KILL", "--", &group])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            return;
        }
    }
    let _ = Command::new("kill")
        .args(["-KILL", "--", &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_out_promptly_and_kills_the_group() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let started = Instant::now();
        let error = run_probe(&mut command, Duration::from_millis(100)).unwrap_err();
        assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn rejects_oversized_probe_output() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 200000 ]; do echo x; i=$((i+1)); done",
        ]);
        let error = run_probe(&mut command, Duration::from_secs(5)).unwrap_err();
        assert!(error.to_string().contains("exceeded"), "{error}");
    }

    #[test]
    fn kills_grandchildren_on_timeout() {
        let marker = std::path::PathBuf::from("/tmp/opencode").join(format!(
            "wxml-probe-orphan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let script = format!(
            "rm -f '{}'; (sleep 1; touch '{}') & sleep 30",
            marker.display(),
            marker.display()
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &script]);
        let error = run_probe(&mut command, Duration::from_millis(100)).unwrap_err();
        assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            !marker.exists(),
            "a grandchild survived the process-group kill"
        );
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn captures_normal_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'hello\\n'"]);
        let output = run_probe(&mut command, Duration::from_secs(5)).unwrap();
        assert_eq!(output.stdout, b"hello\n");
        assert!(output.status.success());
    }
}
