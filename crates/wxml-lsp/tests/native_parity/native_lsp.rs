//! Minimal LSP client for the first-wave native gates.
//!
//! The client spawns the package binary with the committed opt-in selector
//! `--stdio --backend native`, drives real JSON-RPC frames over pipes, and
//! fails on timeouts instead of sleeping. The child environment is scrubbed of
//! the Node backend and gate variables, poisoned with obviously wrong values,
//! and given a PATH that only contains tripwire shims.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::native_package::NativeGate;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Environment variables that must never influence the native child.
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

/// Evidence captured after a clean LSP shutdown.
#[derive(Debug)]
pub(crate) struct ShutdownEvidence {
    /// Server child PID before it exited.
    pub(crate) pid: u32,
    /// Full captured stderr.
    pub(crate) stderr: String,
    /// Wall time from `shutdown` request to a reaped child.
    pub(crate) elapsed: Duration,
}

/// One live native server process.
pub(crate) struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    pending: VecDeque<Value>,
    stderr: Arc<Mutex<String>>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    next_id: i64,
}

impl Session {
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
            .name("native-gate-reader".to_owned())
            .spawn(move || read_messages(stdout, &sender))
            .expect("stdout reader");
        let stderr_sink = Arc::new(Mutex::new(String::new()));
        let stderr_for_thread = Arc::clone(&stderr_sink);
        let stderr_thread = thread::Builder::new()
            .name("native-gate-stderr".to_owned())
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
                // `rename` and the missing-handler quick fix answer a
                // `WorkspaceEdit.documentChanges`; without this capability the
                // server correctly refuses both and a gate would see a false
                // empty result.
                "capabilities": {
                    "workspace": {"workspaceEdit": {"documentChanges": true}},
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

    #[allow(
        clippy::needless_pass_by_value,
        reason = "JSON-RPC frames own their params"
    )]
    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    /// Sends one notification.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "JSON-RPC frames own their params"
    )]
    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn recv(&self) -> Value {
        match self.messages.recv_timeout(REQUEST_TIMEOUT) {
            Ok(value) => value,
            Err(error) => panic!(
                "timed out waiting for an LSP message ({error}); server stderr:\n{}",
                self.stderr_text()
            ),
        }
    }

    fn next_matching(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.pending.iter().position(&mut predicate) {
            return self.pending.remove(index).expect("pending index");
        }
        loop {
            let message = self.recv();
            if predicate(&message) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    fn response(&mut self, id: i64) -> Value {
        self.next_matching(|message| {
            message.get("id").and_then(Value::as_i64) == Some(id) && message.get("method").is_none()
        })
    }

    /// Sends `shutdown`/`exit` and asserts a clean process exit.
    pub(crate) fn shutdown(self) {
        let evidence = self.shutdown_with_evidence();
        assert!(
            !evidence.stderr.contains("detaching it"),
            "server detached a worker during shutdown; stderr:\n{}",
            evidence.stderr
        );
    }

    /// Sends `shutdown`/`exit`, asserts a clean exit and returns the evidence.
    ///
    /// The caller can additionally check that the PID and every descendant are
    /// gone after shutdown; a leaked engine process must fail the gate.
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

    /// Returns the server child PID.
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Returns every descendant PID of the server.
    pub(crate) fn descendant_pids(&self) -> Vec<u32> {
        descendants(self.child.id())
    }

    /// Opens a WXML document and waits for its current diagnostics.
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
        self.wait_diagnostics(uri, Some(version))
    }

    /// Applies one full-text WXML change and waits for diagnostics.
    pub(crate) fn change_wxml(&mut self, uri: &str, text: &str, version: u64) -> Value {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
        self.wait_diagnostics(uri, Some(version))
    }

    /// Applies one full-text overlay change without waiting for diagnostics.
    ///
    /// Non-WXML overlays do not publish a diagnostics answer of their own; the
    /// next hover/definition call observes the updated text.
    pub(crate) fn change_document(&mut self, uri: &str, text: &str, version: u64) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    fn wait_diagnostics(&mut self, uri: &str, min_version: Option<u64>) -> Value {
        loop {
            let message = self.next_matching(|message| {
                message.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                    && message["params"]["uri"].as_str() == Some(uri)
            });
            let version = message["params"]["version"].as_u64();
            let current =
                min_version.is_none_or(|min| version.is_some_and(|version| version >= min));
            if current {
                return message["params"]["diagnostics"].clone();
            }
        }
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

    /// Requests completion and returns the response frame.
    pub(crate) fn completion(&mut self, uri: &str, position: (u32, u32)) -> Value {
        let id = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        );
        self.response(id)
    }

    /// Requests definition and returns the response frame.
    pub(crate) fn definition(&mut self, uri: &str, position: (u32, u32)) -> Value {
        let id = self.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        );
        self.response(id)
    }

    /// Requests references and returns the response frame.
    pub(crate) fn references(
        &mut self,
        uri: &str,
        position: (u32, u32),
        include_declaration: bool,
    ) -> Value {
        let id = self.request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
                "context": {"includeDeclaration": include_declaration},
            }),
        );
        self.response(id)
    }

    /// Requests prepareRename and returns the response frame.
    pub(crate) fn prepare_rename(&mut self, uri: &str, position: (u32, u32)) -> Value {
        let id = self.request(
            "textDocument/prepareRename",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        );
        self.response(id)
    }

    /// Requests rename and returns the response frame.
    pub(crate) fn rename(&mut self, uri: &str, position: (u32, u32), new_name: &str) -> Value {
        let id = self.request(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
                "newName": new_name,
            }),
        );
        self.response(id)
    }

    /// Requests code actions and returns the response frame.
    pub(crate) fn code_action(&mut self, uri: &str, range: ((u32, u32), (u32, u32))) -> Value {
        let id = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": {
                    "start": {"line": range.0.0, "character": range.0.1},
                    "end": {"line": range.1.0, "character": range.1.1},
                },
                "context": {"diagnostics": [], "only": ["quickfix"]},
            }),
        );
        self.response(id)
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

    /// Asserts that no direct or transitive child is a Node executable.
    ///
    /// The PATH tripwire covers relative lookups; `/proc` covers an absolute
    /// Node invocation.
    pub(crate) fn assert_no_node_exec(&self) {
        #[cfg(target_os = "linux")]
        {
            for pid in descendants(self.child.id()) {
                if let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe")) {
                    let name = exe.file_name().and_then(OsStr::to_str).unwrap_or_default();
                    assert!(
                        !matches!(name, "node" | "nodejs" | "npm" | "npx"),
                        "the native server spawned Node (pid {pid}, exe {})",
                        exe.display()
                    );
                }
            }
        }
    }

    /// Asserts that no child process comes from the package (the engine).
    pub(crate) fn assert_no_engine_exec(&self, gate: &NativeGate) {
        #[cfg(target_os = "linux")]
        {
            for pid in descendants(self.child.id()) {
                if let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe")) {
                    assert!(
                        !exe.starts_with(gate.package_root()),
                        "the native server started the package engine (pid {pid}, exe {}); \
                         this gate must not perform a type query",
                        exe.display()
                    );
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = gate;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
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

/// Returns the LSP UTF-16 position of the first `needle`.
pub(crate) fn lsp_position(text: &str, needle: &str) -> (u32, u32) {
    position_at(text, text.find(needle).expect("needle in text"))
}

/// Returns the LSP UTF-16 position of one UTF-8 byte offset in `text`.
pub(crate) fn position_at(text: &str, byte: usize) -> (u32, u32) {
    let prefix = &text[..byte];
    let line =
        u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).expect("line fits");
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = u32::try_from(
        text[line_start..byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>(),
    )
    .expect("column fits");
    (line, character)
}

/// Returns hover markdown or panics with the exact response and stderr.
///
/// A missing `contents.value` is a real gate failure, not a skip: the caller
/// must see the full response and decide whether the native evidence is
/// absent, degraded or wrong.
pub(crate) fn hover_markdown(gate_id: &str, feature: &str, response: &Value) -> String {
    let Some(text) = response["result"]["contents"]["value"].as_str() else {
        panic!("FAILED({gate_id}): {feature} returned no hover. Response: {response}");
    };
    text.to_owned()
}

/// Returns definition locations or panics with the exact response.
pub(crate) fn definition_locations(gate_id: &str, feature: &str, response: &Value) -> Vec<Value> {
    let Some(locations) = response["result"].as_array() else {
        panic!("FAILED({gate_id}): {feature} returned no definition. Response: {response}");
    };
    locations.clone()
}

/// True when the response carries no hover contents.
pub(crate) fn hover_is_empty(response: &Value) -> bool {
    response["result"].is_null()
        || response["result"]["contents"].is_null()
        || response["result"]["contents"]["value"]
            .as_str()
            .is_some_and(str::is_empty)
}

/// Reads the on-disk text of one project file.
pub(crate) fn read_project_file(gate: &NativeGate, relative: &str) -> String {
    let path = gate.project_file(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Returns every descendant PID of `root` using `/proc` (Linux only).
#[cfg(not(target_os = "linux"))]
pub(crate) fn descendants(_root: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(target_os = "linux")]
pub(crate) fn descendants(root: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let children = Path::new("/proc")
                .join(pid.to_string())
                .join("task")
                .join(task.file_name())
                .join("children");
            let Ok(text) = fs::read_to_string(&children) else {
                continue;
            };
            for child in text
                .split_whitespace()
                .filter_map(|text| text.parse::<u32>().ok())
            {
                found.push(child);
                queue.push(child);
            }
        }
    }
    found
}
