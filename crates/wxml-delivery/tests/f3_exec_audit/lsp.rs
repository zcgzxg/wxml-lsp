//! Minimal native stdio LSP client whose transport is a traced child.
//!
//! The client speaks the same `Content-Length` framing as the existing F3
//! harness, but the process is started by [`crate::tracer`] so every `execve`
//! of the server and its native engine is recorded. It intentionally covers
//! only `initialize`/`documentSymbol`/`hover`/`shutdown`.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::tracer::{self, ExecLog, Spec, TraceError, TraceOutcome, TracedChild};

/// Upper bound for one JSON-RPC response.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(120);
/// Upper bound for the whole traced session.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(180);

/// WXML fixture opened by the native query smoke.
pub const PROJECT_WXML: &str = concat!(
    "<view class=\"root\">{{ user.name }}</view>\n",
    "<button bindtap=\"onTap\">tap</button>\n",
);
/// JavaScript fixture that the native engine puts in the project program.
pub const PROJECT_JS: &str = concat!(
    "const user = { name: 'Ada' };\n",
    "function onTap() {}\n",
    "class PageHelpers {\n",
    "  formatName() { return user.name; }\n",
    "}\n",
    "Page({ data: { user: user }, onTap: onTap });\n",
);

/// Project fixture with the minimum files a native session may touch.
pub fn write_project(root: &Path) {
    std::fs::create_dir_all(root.join("pages/index")).expect("create project pages");
    std::fs::write(
        root.join("app.json"),
        b"{\"pages\":[\"pages/index/index\"]}\n",
    )
    .expect("app.json");
    std::fs::write(
        root.join("tsconfig.json"),
        b"{\"compilerOptions\":{\"allowJs\":true}}\n",
    )
    .expect("tsconfig.json");
    std::fs::write(root.join("pages/index/index.wxml"), PROJECT_WXML).expect("index.wxml");
    std::fs::write(root.join("pages/index/index.js"), PROJECT_JS).expect("index.js");
}

/// One live traced language-server process.
pub struct Session {
    child: Option<TracedChild>,
    stdin: Option<File>,
    messages: Receiver<Value>,
    stderr: Arc<Mutex<String>>,
    next_id: i64,
    reader: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts `spec` (a `--stdio --backend native` invocation) and completes
    /// the LSP `initialize` exchange.
    pub fn start(spec: Spec, project: &Path) -> Result<Self, TraceError> {
        let mut child = tracer::spawn(spec, SESSION_TIMEOUT)?;
        let stdin = child.stdin.take().expect("traced LSP stdin is piped");
        let stdout = child.take_stdout().expect("traced LSP stdout is piped");
        let stderr = child.take_stderr().expect("traced LSP stderr is piped");
        let (sender, messages) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("f3-exec-audit-lsp-reader".to_owned())
            .spawn(move || read_messages(stdout, &sender))
            .expect("LSP stdout reader");
        let stderr_sink = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr_sink);
        thread::Builder::new()
            .name("f3-exec-audit-lsp-stderr".to_owned())
            .spawn(move || read_stderr(stderr, &sink))
            .expect("LSP stderr reader");
        let mut session = Self {
            child: Some(child),
            stdin: Some(stdin),
            messages,
            stderr: stderr_sink,
            next_id: 1,
            reader: Some(reader),
        };
        session.initialize(project);
        Ok(session)
    }

    fn initialize(&mut self, project: &Path) {
        let id = self.request(
            "initialize",
            &json!({
                "processId": null,
                "rootUri": format!("file://{}", project.display()),
                "capabilities": {},
            }),
        );
        let response = self.response(id);
        assert_eq!(
            response["result"]["capabilities"]["positionEncoding"],
            json!("utf-16"),
            "{}",
            self.describe(&response)
        );
        assert_eq!(
            response["result"]["capabilities"]["hoverProvider"],
            json!(true),
            "{}",
            self.describe(&response)
        );
        self.notify("initialized", &json!({}));
    }

    pub fn did_open(&mut self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            &json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        );
    }

    pub fn document_symbol(&mut self, uri: &str) -> Value {
        let id = self.request(
            "textDocument/documentSymbol",
            &json!({"textDocument": {"uri": uri}}),
        );
        self.response(id)
    }

    pub fn hover(&mut self, uri: &str, line: u32, character: u32) -> Value {
        let id = self.request(
            "textDocument/hover",
            &json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        self.response(id)
    }

    /// Snapshot of every exec recorded so far.
    pub fn exec_log(&self) -> ExecLog {
        self.child
            .as_ref()
            .map_or_else(ExecLog::default, TracedChild::snapshot)
    }

    pub fn describe(&self, value: &Value) -> String {
        format!(
            "response={value} stderr={} exec_log:\n{}",
            self.stderr_text(),
            self.exec_log().summary()
        )
    }

    pub fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize LSP frame");
        let stdin = self.stdin.as_mut().expect("LSP stdin open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("LSP header");
        stdin.write_all(&body).expect("LSP body");
        stdin.flush().expect("LSP flush");
    }

    fn request(&mut self, method: &str, params: &Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn response(&self, id: i64) -> Value {
        let deadline = Instant::now() + HEADER_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "LSP response {id} timed out; stderr={} exec_log:\n{}",
                self.stderr_text(),
                self.exec_log().summary()
            );
            match self.messages.recv_timeout(remaining) {
                Ok(value) if value["id"].as_i64() == Some(id) => return value,
                Ok(_) => {}
                Err(error) => {
                    panic!(
                        "LSP response {id} failed: {error}; stderr={} exec_log:\n{}",
                        self.stderr_text(),
                        self.exec_log().summary()
                    );
                }
            }
        }
    }

    /// Sends `shutdown`/`exit`, closes stdin and returns the executable log.
    pub fn shutdown(mut self) -> TraceOutcome {
        let id = self.request("shutdown", &json!(null));
        let _ = self.response(id);
        self.notify("exit", &json!(null));
        self.stdin.take();
        let child = self.child.take().expect("live traced session");
        let outcome = child.join_tracer();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        outcome
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stdin.take();
        // Dropping the traced child kills the process group and joins the tracer.
        self.child.take();
    }
}

fn read_messages(stdout: File, sender: &mpsc::Sender<Value>) {
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

fn read_stderr(stderr: File, sink: &Arc<Mutex<String>>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let mut sink = sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if sink.len() < 256 * 1024 {
                    sink.push_str(&line);
                }
            }
        }
    }
}
