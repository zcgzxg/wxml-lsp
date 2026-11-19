//! Minimal native stdio LSP session plus `/proc` process-tree observation.
//!
//! This is deliberately small: it only proves that the installed native binary
//! completes an `initialize`/`shutdown` handshake with a PATH that contains no
//! Node. It is not a full LSP conformance suite and it does not replace the
//! `strace` audit that F3-02 still requires.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::support::{Tripwires, clean_command};

const HEADER_TIMEOUT: Duration = Duration::from_secs(60);

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

/// One live native language-server process.
pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    stderr: Arc<Mutex<String>>,
    next_id: i64,
    reader: Option<thread::JoinHandle<()>>,
}

impl Session {
    pub fn start(binary: &Path, project: &Path, tripwires: &Tripwires, work: &Path) -> Self {
        let mut command = clean_command(binary, tripwires, work);
        command
            .args(["--stdio", "--backend", "native"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "spawn {} --stdio --backend native: {error}",
                binary.display()
            )
        });
        let stdin = child.stdin.take().expect("lsp stdin");
        let stdout = child.stdout.take().expect("lsp stdout");
        let stderr = child.stderr.take().expect("lsp stderr");
        let (sender, messages) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("f3-lsp-reader".to_owned())
            .spawn(move || read_messages(stdout, &sender))
            .expect("lsp stdout reader");
        let stderr_sink = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr_sink);
        thread::Builder::new()
            .name("f3-lsp-stderr".to_owned())
            .spawn(move || read_stderr(stderr, &sink))
            .expect("lsp stderr reader");
        let mut session = Self {
            child,
            stdin: Some(stdin),
            messages,
            stderr: stderr_sink,
            next_id: 1,
            reader: Some(reader),
        };
        session.initialize(project);
        session
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

    /// Opens one document in the live session.
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

    /// Waits for the diagnostics notification of one open document.
    pub fn wait_diagnostics(&self, uri: &str) -> Value {
        let deadline = Instant::now() + HEADER_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "LSP diagnostics for {uri} timed out; stderr={}",
                self.stderr_text()
            );
            match self.messages.recv_timeout(remaining) {
                Ok(value)
                    if value["method"] == json!("textDocument/publishDiagnostics")
                        && value["params"]["uri"].as_str() == Some(uri) =>
                {
                    return value["params"]["diagnostics"].clone();
                }
                Ok(_) => {}
                Err(error) => panic!("LSP diagnostics for {uri} failed: {error}"),
            }
        }
    }

    /// Requests the document outline of one open document.
    pub fn document_symbol(&mut self, uri: &str) -> Value {
        let id = self.request(
            "textDocument/documentSymbol",
            &json!({"textDocument": {"uri": uri}}),
        );
        self.response(id)
    }

    /// Requests hover at a zero-based UTF-16 line/character position.
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

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize LSP frame");
        let stdin = self.stdin.as_mut().expect("lsp stdin open");
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
            assert!(!remaining.is_zero(), "LSP response {id} timed out");
            match self.messages.recv_timeout(remaining) {
                Ok(value) if value["id"].as_i64() == Some(id) => return value,
                Ok(_) => {}
                Err(error) => panic!("LSP response {id} failed: {error}"),
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn describe(&self, value: &Value) -> String {
        format!("response={value} stderr={}", self.stderr_text())
    }

    pub fn shutdown(mut self) {
        let id = self.request("shutdown", &json!(null));
        let _ = self.response(id);
        self.notify("exit", &json!(null));
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(
                        status.success(),
                        "native LSP exited with {status}; stderr={}",
                        self.stderr_text()
                    );
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!(
                        "native LSP did not exit after shutdown; stderr={}",
                        self.stderr_text()
                    );
                }
                Err(error) => panic!("wait for native LSP: {error}"),
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stdin.take();
    }
}

fn read_messages(stdout: std::process::ChildStdout, sender: &mpsc::Sender<Value>) {
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

fn read_stderr(stderr: std::process::ChildStderr, sink: &Arc<Mutex<String>>) {
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

/// Observes the live process subtree below `root_pid` for a short window.
///
/// The returned rows contain `pid`, `comm` and `/proc/<pid>/exe`. This is
/// auxiliary evidence: it observes processes instead of tracing `execve`.
pub fn observe_process_tree(root_pid: u32, window: Duration) -> Vec<String> {
    let deadline = Instant::now() + window;
    let mut seen = BTreeSet::new();
    while Instant::now() < deadline {
        collect_descendants(root_pid, &mut seen);
        thread::sleep(Duration::from_millis(10));
    }
    seen.into_iter().map(describe_process).collect()
}

fn collect_descendants(pid: u32, seen: &mut BTreeSet<u32>) {
    if !seen.insert(pid) {
        return;
    }
    for child in read_children(pid) {
        collect_descendants(child, seen);
    }
}

fn read_children(pid: u32) -> Vec<u32> {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for task in tasks.flatten() {
        let path = task.path().join("children");
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for value in text.split_whitespace() {
            if let Ok(child) = value.parse::<u32>() {
                children.push(child);
            }
        }
    }
    children
}

fn describe_process(pid: u32) -> String {
    let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
        .map_or_else(|_| "<gone>".to_owned(), |text| text.trim().to_owned());
    let exe = fs::read_link(format!("/proc/{pid}/exe"))
        .map_or_else(|_| "<gone>".to_owned(), |path| path.display().to_string());
    format!("pid={pid} comm={comm} exe={exe}")
}

/// Fails when an observed process is a Node-family runtime.
pub fn assert_no_node_process(lines: &[String]) {
    for line in lines {
        let lower = line.to_ascii_lowercase();
        let comm = lower
            .split(" comm=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default();
        let exe = lower.split(" exe=").nth(1).unwrap_or_default();
        let exe_name = Path::new(exe)
            .file_name()
            .map_or("", |name| name.to_str().unwrap_or_default());
        assert!(
            !matches!(
                comm,
                "node" | "nodejs" | "npm" | "npx" | "pnpm" | "yarn" | "bun"
            ),
            "observed a Node-family process: {line}"
        );
        assert!(
            !matches!(
                exe_name,
                "node" | "nodejs" | "npm" | "npx" | "pnpm" | "yarn" | "bun"
            ),
            "observed a Node-family executable: {line}"
        );
    }
}

/// Project fixture with the minimum files a native session may touch.
pub fn write_project(root: &Path) {
    fs::create_dir_all(root.join("pages/index")).expect("create project pages");
    fs::write(
        root.join("app.json"),
        b"{\"pages\":[\"pages/index/index\"]}\n",
    )
    .expect("app.json");
    fs::write(
        root.join("tsconfig.json"),
        b"{\"compilerOptions\":{\"allowJs\":true}}\n",
    )
    .expect("tsconfig.json");
    fs::write(root.join("pages/index/index.wxml"), PROJECT_WXML).expect("index.wxml");
    fs::write(root.join("pages/index/index.js"), PROJECT_JS).expect("index.js");
}

/// A filesystem path that is outside the repository and does not exist yet.
pub fn project_uri(project: &Path) -> String {
    format!("file://{}", project.display())
}

/// Kept for diagnostics and to make the module's path dependency explicit.
pub fn debug_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}
