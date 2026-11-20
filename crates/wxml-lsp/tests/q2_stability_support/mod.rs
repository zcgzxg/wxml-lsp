//! Raw JSON-RPC session for the `G-P5-STABILITY-01` scenario assertions.
//!
//! The shared `native_lsp::Session` is deliberately synchronous: one request,
//! one response, diagnostics tied to a text version. Four stability scenarios
//! need protocol shapes that helper cannot express:
//!
//! - ten `didChange` notifications followed by one request and an immediate
//!   `$/cancelRequest` by id;
//! - diagnostics published by `workspace/didChangeWatchedFiles` without a new
//!   text version;
//! - a hover probe between an `app.json` overlay open and close;
//! - forty hover requests held in flight before their responses are read.
//!
//! This module keeps that raw capability next to the gate. The shared
//! native-parity harness and the production default are unchanged. The child
//! environment, PATH tripwire and private `TMPDIR` are the same as
//! `native_lsp::Session`; only the diagnostics debounce is pinned to the
//! retired stability gate's `120 ms` through initialization options.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::native_lsp::ShutdownEvidence;
use crate::native_package::NativeGate;

/// One request must answer; a hung server is a gate failure, not a skip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Clean LSP shutdown must reap the server.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Environment variables that must never influence the native child.
///
/// Mirrors `native_lsp::SCRUBBED_ENV`; this module is intentionally local so
/// the shared native-parity harness is not edited by the stability gate. A
/// future addition to the shared list must be copied here as well.
const SCRUBBED_ENV: &[&str] = &[
    "WXML_TYPE_BACKEND",
    "WXML_BACKEND_DIR",
    "WXML_NODE",
    "WXML_TEMPLATE_CHECK",
    "WXML_TEST_NATIVE",
    "WXML_TEST_NATIVE_PACKAGE",
    "WXML_REQUIRE_REAL_ENGINE",
    "WXML_TRIPWIRE_MARKER",
    "TSGO_BIN",
    "TSGO_ASSET_ROOT",
    "TSGO_TYPINGS_ROOT",
];

/// One live native server process with raw request/response control.
pub(crate) struct RawSession {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    pending: VecDeque<Value>,
    stderr: Arc<Mutex<String>>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    next_id: i64,
}

impl RawSession {
    /// Spawns `<package>/bin/wxml-lsp --stdio --backend native` and initializes.
    pub(crate) fn start(gate: &NativeGate) -> Self {
        let mut command = Command::new(gate.bin());
        command
            .arg("--stdio")
            .arg("--backend")
            .arg("native")
            .current_dir(gate.project_root());
        for variable in SCRUBBED_ENV {
            command.env_remove(variable);
        }
        command
            .env("WXML_TYPE_BACKEND", "/nonexistent/poison-server.js")
            .env("WXML_BACKEND_DIR", "/nonexistent")
            .env("WXML_NODE", "wxml-poison-node")
            .env("WXML_ANALYZE_TIMEOUT_MS", "30000")
            // The scenario gate pins the retired stability debounce through
            // initialization options; the env stays zero as a poison check.
            .env("WXML_DIAGNOSTICS_DEBOUNCE_MS", "0")
            .env("TMPDIR", gate.private_tmp())
            .env("PATH", gate.tripwire().dir())
            .env("WXML_TRIPWIRE_MARKER", gate.tripwire().marker())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "spawn {} --stdio --backend native: {error}",
                gate.bin().display()
            )
        });
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (sender, messages) = mpsc::channel();
        let stdout_thread = thread::Builder::new()
            .name("q2-stability-reader".to_owned())
            .spawn(move || read_messages(stdout, &sender))
            .expect("stdout reader");
        let stderr_sink = Arc::new(Mutex::new(String::new()));
        let stderr_for_thread = Arc::clone(&stderr_sink);
        let stderr_thread = thread::Builder::new()
            .name("q2-stability-stderr".to_owned())
            .spawn(move || read_stderr(stderr, &stderr_for_thread))
            .expect("stderr reader");

        let mut session = Self {
            child,
            stdin: Some(stdin),
            messages,
            pending: VecDeque::new(),
            stderr: stderr_sink,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            next_id: 1,
        };
        let initialize = session.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": gate.project_uri(),
                "capabilities": {
                    "workspace": {"workspaceEdit": {"documentChanges": true}},
                },
                "initializationOptions": {
                    "wxml": {"diagnostics": {"debounceMs": 120}},
                },
            }),
        );
        let response = session.response(initialize);
        assert_eq!(
            response["result"]["capabilities"]["positionEncoding"],
            json!("utf-16"),
            "{}",
            session.describe(&response)
        );
        assert_eq!(
            response["result"]["capabilities"]["hoverProvider"],
            json!(true),
            "{}",
            session.describe(&response)
        );
        session.notify("initialized", json!({}));
        session
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("json");
        let stdin = self.stdin.as_mut().expect("stdin open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        stdin.write_all(&body).expect("body");
        stdin.flush().expect("flush");
    }

    /// Sends one notification.
    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Sends one request and returns its id without waiting for a response.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<Value> {
        match self.messages.recv_timeout(timeout) {
            Ok(value) => Some(value),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!(
                    "the native server closed stdout before answering; stderr:\n{}",
                    self.stderr_text()
                )
            }
        }
    }

    fn next_matching(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.pending.iter().position(&mut predicate) {
            return self.pending.remove(index).expect("pending index");
        }
        loop {
            let message = self.recv_timeout(REQUEST_TIMEOUT).unwrap_or_else(|| {
                panic!(
                    "timed out waiting for an LSP message; server stderr:\n{}",
                    self.stderr_text()
                )
            });
            if predicate(&message) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    /// Waits for the response frame of one request id.
    pub(crate) fn response(&mut self, id: i64) -> Value {
        self.next_matching(|message| {
            message.get("id").and_then(Value::as_i64) == Some(id) && message.get("method").is_none()
        })
    }

    /// Opens a WXML document and waits for diagnostics at least at `version`.
    pub(crate) fn open_wxml(&mut self, uri: &str, text: &str, version: u64) -> Value {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": version,
                    "text": text,
                }
            }),
        );
        self.wait_diagnostics_matching(uri, REQUEST_TIMEOUT, |message| {
            message["params"]["version"]
                .as_u64()
                .is_some_and(|seen| seen >= version)
        })
        .unwrap_or_else(|| {
            panic!(
                "no diagnostics for {uri} at version {version}; server stderr:\n{}",
                self.stderr_text()
            )
        })
        .get("params")
        .and_then(|params| params.get("diagnostics"))
        .cloned()
        .unwrap_or(Value::Null)
    }

    /// Requests hover and returns the response frame.
    pub(crate) fn hover(&mut self, uri: &str, position: (u32, u32)) -> Value {
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        );
        self.response(id)
    }

    /// Waits for one diagnostics notification matching `predicate`.
    ///
    /// This is the watched-files seam: the notification can arrive without a
    /// new text version, so the caller supplies the bound and the predicate.
    pub(crate) fn wait_diagnostics_matching(
        &mut self,
        uri: &str,
        timeout: Duration,
        mut predicate: impl FnMut(&Value) -> bool,
    ) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(index) = self
                .pending
                .iter()
                .position(|message| is_diagnostics_for(message, uri) && predicate(message))
            {
                return Some(self.pending.remove(index).expect("pending index"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let message = self.recv_timeout(remaining)?;
            if is_diagnostics_for(&message, uri) && predicate(&message) {
                return Some(message);
            }
            self.pending.push_back(message);
        }
    }

    /// Returns the server child PID.
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Returns every descendant PID of the server.
    pub(crate) fn descendant_pids(&self) -> Vec<u32> {
        crate::native_lsp::descendants(self.child.id())
    }

    /// Sends `shutdown`/`exit` and returns the clean-exit evidence.
    pub(crate) fn shutdown_with_evidence(mut self) -> ShutdownEvidence {
        let pid = self.child.id();
        let started = Instant::now();
        let id = self.request("shutdown", Value::Null);
        let response = self.response(id);
        assert_eq!(
            response["result"],
            Value::Null,
            "{}",
            self.describe(&response)
        );
        self.notify("exit", Value::Null);
        self.stdin.take();

        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(
                        status.success(),
                        "server exited with {status}; stderr:\n{}",
                        self.stderr_text()
                    );
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("server did not exit; stderr:\n{}", self.stderr_text());
                }
                Err(error) => panic!("wait failed: {error}"),
            }
        }
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        ShutdownEvidence {
            pid,
            stderr: self.stderr_text(),
            elapsed: started.elapsed(),
        }
    }

    /// Returns captured stderr.
    pub(crate) fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Formats a response with stderr for assertion messages.
    pub(crate) fn describe(&self, value: &Value) -> String {
        format!("{value}\nserver stderr:\n{}", self.stderr_text())
    }
}

impl Drop for RawSession {
    fn drop(&mut self) {
        // A panicking scenario must not leave the engine behind. A clean
        // `shutdown_with_evidence` has already reaped the child and every
        // descendant, so this loop is empty on the success path.
        let descendants = crate::native_lsp::descendants(self.child.id());
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for pid in descendants {
            let _ = Command::new("/bin/kill")
                .arg("-9")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}

fn is_diagnostics_for(message: &Value, uri: &str) -> bool {
    message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        && message["params"]["uri"].as_str() == Some(uri)
}

fn read_messages(stdout: ChildStdout, sender: &mpsc::Sender<Value>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let Some(length) = read_header(&mut reader) else {
            return;
        };
        let mut body = vec![0_u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice(&body) else {
            return;
        };
        if sender.send(value).is_err() {
            return;
        }
    }
}

fn read_header(reader: &mut impl BufRead) -> Option<usize> {
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
            length = value.trim().parse::<usize>().ok();
        }
    }
}

fn read_stderr(stderr: ChildStderr, sink: &Arc<Mutex<String>>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let mut sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
                if sink.len() < 256 * 1024 {
                    sink.push_str(&line);
                }
            }
        }
    }
}
