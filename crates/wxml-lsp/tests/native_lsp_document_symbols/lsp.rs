//! Minimal Content-Length JSON-RPC client and fail-closed startup runner.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    missing_docs
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::package::{BrokenPackage, Fixture};
use super::{EXIT_TIMEOUT, MAX_FRAME_BYTES, PACKAGE_BIN, REQUEST_TIMEOUT, STDERR_LIMIT, Tripwire};

/// One live native server process speaking LSP over stdio.
pub(crate) struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    stderr: Arc<Mutex<String>>,
    stdout_errors: Arc<Mutex<Vec<String>>>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    next_id: i64,
}

impl Session {
    pub(crate) fn start(fixture: &Fixture) -> Self {
        let mut command = native_command(
            fixture.bin(),
            fixture.project(),
            fixture.private_tmp(),
            fixture.tripwire(),
        );
        let mut child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "spawn {} --stdio --backend native: {error}",
                fixture.bin().display()
            )
        });
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let (sender, messages) = mpsc::channel();
        let stdout_errors = Arc::new(Mutex::new(Vec::new()));
        let errors_for_thread = Arc::clone(&stdout_errors);
        let stdout_thread = thread::Builder::new()
            .name("native-ds-stdout".to_owned())
            .spawn(move || read_frames(stdout, &sender, &errors_for_thread))
            .expect("stdout reader");
        let stderr_sink = Arc::new(Mutex::new(String::new()));
        let stderr_for_thread = Arc::clone(&stderr_sink);
        let stderr_thread = thread::Builder::new()
            .name("native-ds-stderr".to_owned())
            .spawn(move || read_stderr(stderr, &stderr_for_thread))
            .expect("stderr reader");
        Self {
            child,
            stdin: Some(stdin),
            messages,
            stderr: stderr_sink,
            stdout_errors,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            next_id: 1,
        }
    }

    pub(crate) fn initialize(&mut self, root_uri: &str) -> Value {
        let response = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": true}
                    }
                }
            }),
        );
        assert!(
            response.get("error").is_none(),
            "initialize failed: {}",
            self.describe(&response)
        );
        self.notify("initialized", json!({}));
        response
    }

    pub(crate) fn did_open(&mut self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            }),
        );
    }

    pub(crate) fn document_symbol(&mut self, uri: &str) -> Value {
        self.request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        )
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        write_frame(stdin, value).expect("write frame");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.response(id)
    }

    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn response(&self, id: i64) -> Value {
        loop {
            let message = match self.messages.recv_timeout(REQUEST_TIMEOUT) {
                Ok(message) => message,
                Err(error) => panic!(
                    "timed out waiting for LSP response {id} ({error}); {}\nserver stderr:\n{}",
                    self.stdout_error_text(),
                    self.stderr_text()
                ),
            };
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
        }
    }

    pub(crate) fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn stdout_error_text(&self) -> String {
        let errors = {
            let guard = self
                .stdout_errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.clone()
        };
        if errors.is_empty() {
            "stdout: no framing errors".to_owned()
        } else {
            format!("stdout framing errors: {}", errors.join("; "))
        }
    }

    fn describe(&self, value: &Value) -> String {
        format!(
            "{value}\n{}\nserver stderr:\n{}",
            self.stdout_error_text(),
            self.stderr_text()
        )
    }

    pub(crate) fn shutdown(mut self) {
        let response = self.request("shutdown", Value::Null);
        assert_eq!(
            response["result"],
            Value::Null,
            "{}",
            self.describe(&response)
        );
        self.notify("exit", Value::Null);
        self.stdin.take();
        self.wait_for_exit();
        self.join_threads();
        let errors = self
            .stdout_errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        assert!(
            errors.is_empty(),
            "stdout carried non-JSON-RPC output (for example a Node/server.js log): {errors:?}"
        );
        assert!(
            !self.stderr_text().contains("server.js"),
            "stderr mentions server.js; the native path must not fall back to Node:\n{}",
            self.stderr_text()
        );
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(
                        status.success(),
                        "the native server exited with {status}; stderr:\n{}",
                        self.stderr_text()
                    );
                    return;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    kill_process_group(self.child.id());
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!(
                        "the native server did not exit after exit; stderr:\n{}",
                        self.stderr_text()
                    );
                }
                Err(error) => panic!("wait failed: {error}"),
            }
        }
    }

    fn join_threads(&mut self) {
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stdin.take();
        // A normal shutdown already reaped the child. Only signal a process
        // group that is still running; otherwise the numeric PGID could have
        // been reused after `wait`.
        if matches!(self.child.try_wait(), Ok(None)) {
            kill_process_group(self.child.id());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.join_threads();
    }
}

/// Kills the child's process group when the platform `kill` utility is present.
///
/// The server runs in its own process group, so this also reaps a native
/// engine child on a timeout or a panic path. A missing `kill` utility falls
/// back to the direct `Child::kill` call at the call site.
fn kill_process_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .arg("-9")
        .arg(format!("-{pid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn native_command(
    bin: std::path::PathBuf,
    project: &Path,
    private_tmp: &Path,
    tripwire: &Tripwire,
) -> Command {
    let mut command = Command::new(bin);
    command
        .arg("--stdio")
        .arg("--backend")
        .arg("native")
        .current_dir(project)
        .env_clear()
        .env("TMPDIR", private_tmp)
        .env("PATH", tripwire.dir())
        .env("WXML_TRIPWIRE_MARKER", tripwire.marker())
        .env("WXML_TYPE_BACKEND", "/nonexistent/poison-server.js")
        .env("WXML_BACKEND_DIR", "/nonexistent")
        .env("WXML_NODE", "wxml-poison-node")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn write_frame(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).expect("JSON");
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn read_frames(
    stdout: ChildStdout,
    sender: &mpsc::Sender<Value>,
    errors: &Arc<Mutex<Vec<String>>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let Some(length) = read_header(&mut reader, errors) else {
            return;
        };
        let mut body = vec![0_u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        if let Ok(value) = serde_json::from_slice(&body) {
            if sender.send(value).is_err() {
                return;
            }
        } else {
            errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push("stdout frame is not JSON".to_owned());
            return;
        }
    }
}

fn read_header(reader: &mut impl BufRead, errors: &Arc<Mutex<Vec<String>>>) -> Option<usize> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return length;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            match value.trim().parse::<usize>() {
                Ok(parsed) if parsed <= MAX_FRAME_BYTES => length = Some(parsed),
                _ => {
                    errors
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(format!(
                            "Content-Length is invalid or exceeds the {MAX_FRAME_BYTES}-byte cap"
                        ));
                    return None;
                }
            }
        } else {
            errors
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("unexpected stdout line: {line}"));
            return None;
        }
    }
}

fn read_stderr(stderr: std::process::ChildStderr, sink: &Arc<Mutex<String>>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let mut sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
                if sink.len() < STDERR_LIMIT {
                    sink.push_str(&line);
                }
            }
        }
    }
}

/// Result of one process that must fail before answering `initialize`.
#[derive(Debug)]
pub(crate) struct StartupFailure {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Spawns the package, sends one initialize frame, and requires an early exit.
pub(crate) fn run_startup_failure(broken: &BrokenPackage) -> StartupFailure {
    let mut command = native_command(
        broken.package().join(PACKAGE_BIN),
        broken.project(),
        broken.private_tmp(),
        broken.tripwire(),
    );
    let mut child = command.spawn().expect("spawn broken package");
    {
        let stdin = child.stdin.as_mut().expect("broken stdin");
        let _ = write_frame(
            stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"processId": null, "rootUri": null, "capabilities": {}}
            }),
        );
    }
    child.stdin.take();
    let status = wait_for_child(&mut child, &broken.package().join(PACKAGE_BIN));
    let stdout = read_pipe(child.stdout.take().expect("broken stdout"));
    let stderr = read_pipe(child.stderr.take().expect("broken stderr"));
    StartupFailure {
        status,
        stdout,
        stderr,
    }
}

fn wait_for_child(child: &mut Child, bin: &Path) -> ExitStatus {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                kill_process_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                panic!("{} did not exit after discovery failure", bin.display());
            }
            Err(error) => panic!("wait failed: {error}"),
        }
    }
}

fn read_pipe(mut pipe: impl Read) -> String {
    let mut text = String::new();
    match pipe.read_to_string(&mut text) {
        Ok(_) => text,
        Err(error) => format!("{text}\n<pipe read error: {error}>"),
    }
}
