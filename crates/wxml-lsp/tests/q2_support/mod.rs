//! Shared in-process JSON-RPC harness for Q2 scheduling/recovery tests.
//!
//! Each test runs the real protocol loop and project host over a Unix socket
//! pair. The Node transport is either a fake C1 responder (fast, failure
//! injection) or the real backend in `real_node.rs`/`performance_bench.rs`.

#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    unreachable_pub
)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig, NodeTransport, NodeTransportError};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::run_with_host;

use crate::common::TempProject;

pub const WXML: &str = "pages/index/index.wxml";
pub const SCRIPT: &str = "pages/index/index.ts";
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// What the fake Node transport does on its next request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    None,
    TimeoutOnce,
    CrashOnce,
    ProtocolOnce,
}

#[derive(Clone)]
pub struct Shared {
    pub requests: Arc<Mutex<Vec<String>>>,
    pub connects: Arc<AtomicU64>,
    pub failure: Arc<Mutex<Failure>>,
    pub delay: Arc<Mutex<Duration>>,
    pub symbols_delay: Arc<Mutex<Duration>>,
    pub bad_version: Arc<AtomicBool>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            connects: Arc::new(AtomicU64::new(0)),
            failure: Arc::new(Mutex::new(Failure::None)),
            delay: Arc::new(Mutex::new(Duration::ZERO)),
            symbols_delay: Arc::new(Mutex::new(Duration::ZERO)),
            bad_version: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Shared {
    pub fn new(failure: Failure, delay: Duration) -> Self {
        Self {
            failure: Arc::new(Mutex::new(failure)),
            delay: Arc::new(Mutex::new(delay)),
            ..Self::default()
        }
    }

    pub fn request_count(&self) -> usize {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn last_request(&self) -> Option<String> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .cloned()
    }

    pub fn set_failure(&self, failure: Failure) {
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = failure;
    }

    pub fn set_delay(&self, delay: Duration) {
        *self.delay.lock().unwrap_or_else(PoisonError::into_inner) = delay;
    }

    pub fn set_symbols_delay(&self, delay: Duration) {
        *self
            .symbols_delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = delay;
    }
}

struct FakeTransport {
    shared: Shared,
}

impl NodeTransport for FakeTransport {
    fn request(&self, line: &str, _timeout: Duration) -> Result<String, NodeTransportError> {
        self.shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_owned());
        let delay = *self
            .shared
            .delay
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        if serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|request| request["method"].as_str().map(str::to_owned))
            .as_deref()
            == Some("symbols")
        {
            let symbols_delay = *self
                .shared
                .symbols_delay
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !symbols_delay.is_zero() {
                thread::sleep(symbols_delay);
            }
        }
        let mut failure = self
            .shared
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mode = *failure;
        if mode != Failure::None {
            *failure = Failure::None;
        }
        drop(failure);
        match mode {
            Failure::TimeoutOnce => Err(NodeTransportError::Timeout),
            Failure::CrashOnce => Err(NodeTransportError::Exited),
            Failure::ProtocolOnce => Ok(respond_with_version(line, 3)),
            Failure::None => Ok(respond(line)),
        }
    }

    fn shutdown(&self) {}
}

/// A transport that is never used by configuration/recovery tests.
pub struct NoopTransport;

impl NodeTransport for NoopTransport {
    fn request(&self, _line: &str, _timeout: Duration) -> Result<String, NodeTransportError> {
        Err(NodeTransportError::NotConfigured)
    }

    fn shutdown(&self) {}
}

/// Starts an in-process server whose backend uses the fake transport.
pub fn start_fake(
    project: &TempProject,
    failure: Failure,
    delay: Duration,
) -> (
    Client,
    thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>,
    Shared,
) {
    let shared = Shared::new(failure, delay);
    let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
    let server_read = server_stream.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend_shared = shared.clone();
    let backend = Arc::new(NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        move || {
            backend_shared.connects.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeTransport {
                shared: backend_shared.clone(),
            }))
        },
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server_stream, backend, host));
    let mut client = Client::new(client_stream);
    client.initialize(
        project,
        json!({"wxml": {"diagnostics": {"debounceMs": 120}}}),
    );
    (client, handle, shared)
}

pub fn app_fixture() -> TempProject {
    let project = TempProject::new("wxml-q2");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write(WXML, "{{ user.name }}");
    project.write(SCRIPT, "Page({ data: { user: { name: 'DISK_Q2' } } });");
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    project
}

pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    pending: VecDeque<Value>,
}

impl Client {
    pub fn new(stream: UnixStream) -> Self {
        let reader_stream = stream.try_clone().expect("clone stream");
        reader_stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .expect("read timeout");
        Self {
            writer: stream,
            reader: BufReader::new(reader_stream),
            next_id: 1,
            pending: VecDeque::new(),
        }
    }

    pub fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("json");
        write!(self.writer, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        self.writer.write_all(&body).expect("body");
        self.writer.flush().expect("flush");
    }

    pub fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    pub fn request_with_id(&mut self, id: i64, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn recv(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).expect("header read");
            assert!(read != 0, "server closed the stream");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let length = length.expect("content length");
        let mut body = vec![0_u8; length];
        self.reader.read_exact(&mut body).expect("body read");
        serde_json::from_slice(&body).expect("json")
    }

    pub fn next_matching(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.pending.iter().position(&mut predicate) {
            return self.pending.remove(index).expect("pending index");
        }
        loop {
            let message = self.recv();
            if std::env::var_os("WXML_Q2_DEBUG").is_some() {
                eprintln!("q2-message: {message}");
            }
            if predicate(&message) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    pub fn response(&mut self, id: i64) -> Value {
        self.next_matching(|message| {
            message.get("id").and_then(Value::as_i64) == Some(id) && message.get("method").is_none()
        })
    }

    pub fn initialize(&mut self, project: &TempProject, options: Value) {
        let id = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": project.root_uri(),
                "capabilities": {"workspace": {"workspaceEdit": {"documentChanges": true}}},
                "initializationOptions": options,
            }),
        );
        let response = self.response(id);
        assert!(response.get("result").is_some(), "{response}");
        self.notify("initialized", json!({}));
    }

    pub fn initialize_response(&mut self, project: &TempProject, options: Value) -> Value {
        let id = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": project.root_uri(),
                "capabilities": {"workspace": {"workspaceEdit": {"documentChanges": true}}},
                "initializationOptions": options,
            }),
        );
        self.response(id)
    }

    pub fn open(&mut self, uri: &str, text: &str, version: u64) {
        self.open_language(uri, "wxml", text, version);
    }

    pub fn open_language(&mut self, uri: &str, language_id: &str, text: &str, version: u64) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        );
    }

    pub fn change(&mut self, uri: &str, text: &str, version: u64) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    pub fn save(&mut self, uri: &str) {
        self.notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    pub fn close(&mut self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    pub fn watched(&mut self, uri: &str, kind: u64) {
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": [{"uri": uri, "type": kind}]}),
        );
    }

    pub fn wait_diagnostics(&mut self, uri: &str, min_version: u64) -> Value {
        self.next_matching(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && message["params"]["uri"].as_str() == Some(uri)
                && message["params"]["version"]
                    .as_u64()
                    .is_some_and(|version| version >= min_version)
        })
    }

    pub fn wait_diagnostics_matching(
        &mut self,
        uri: &str,
        mut predicate: impl FnMut(&Value) -> bool,
    ) -> Value {
        self.next_matching(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && message["params"]["uri"].as_str() == Some(uri)
                && predicate(message)
        })
    }

    pub fn wait_next_diagnostics(&mut self, uri: &str) -> Value {
        self.next_matching(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && message["params"]["uri"].as_str() == Some(uri)
        })
    }

    pub fn wait_diagnostics_after(&mut self, uri: &str, after: Instant) -> Value {
        loop {
            let message = self.next_matching(|message| {
                message.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                    && message["params"]["uri"].as_str() == Some(uri)
            });
            if Instant::now() >= after {
                return message;
            }
        }
    }

    pub fn hover(&mut self, uri: &str, line: u32, character: u32) -> Value {
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        self.response(id)
    }

    pub fn completion(&mut self, uri: &str, line: u32, character: u32) -> Value {
        let id = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        self.response(id)
    }

    pub fn take_pending(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending).into_iter().collect()
    }

    pub fn finish(mut self) {
        let shutdown = self.request("shutdown", json!(null));
        let response = self.response(shutdown);
        assert!(response.get("result").is_some(), "{response}");
        self.notify("exit", json!(null));
    }

    pub fn finish_without_initialize(mut self) {
        self.notify("exit", json!(null));
    }
}

pub fn diagnostic_messages(notification: &Value) -> Vec<String> {
    notification["params"]["diagnostics"]
        .as_array()
        .map_or_else(Vec::new, |diagnostics| {
            diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic["message"].as_str().map(str::to_owned))
                .collect()
        })
}

pub fn diagnostic_message(notification: &Value) -> String {
    diagnostic_messages(notification)
        .first()
        .cloned()
        .unwrap_or_default()
}

fn marker_from_files(params: &Value) -> String {
    let mut marker = "disk".to_owned();
    let Some(files) = params["files"].as_array() else {
        return marker;
    };
    for file in files {
        let Some(text) = file["text"].as_str() else {
            continue;
        };
        for word in
            text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            if word.starts_with("ZED_") || word.starts_with("DISK_") || word.starts_with("NEW_") {
                word.clone_into(&mut marker);
            }
        }
    }
    marker
}

pub fn respond(line: &str) -> String {
    respond_with_version(line, 2)
}

pub fn respond_with_version(line: &str, protocol_version: u64) -> String {
    let request: Value = serde_json::from_str(line).expect("request json");
    let id = &request["id"];
    let params = &request["params"];
    let request_id = params["requestId"].as_str().unwrap_or("r");
    let project_revision = params["analysisStamp"]["projectRevision"]
        .as_u64()
        .unwrap_or(1);
    let view_revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .unwrap_or(1);
    let script_file = params["scriptFileId"].as_str().unwrap_or("s");
    let marker = marker_from_files(params);
    let fragments = params["fragments"]
        .as_array()
        .map_or_else(Vec::new, |fragments| {
            fragments
                .iter()
                .map(|fragment| {
                    let start = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3) + 5;
                    json!({
                        "fragmentId": fragment["fragmentId"],
                        "hover": {
                            "typeText": marker,
                            "typeOnly": marker,
                            "quality": "known",
                            "documentation": format!("{marker}:{project_revision}:{view_revision}"),
                            "documentationSource": "quickinfo"
                        },
                        "diagnostics": [{
                            "code": 2339,
                            "range": {"start": start, "end": start + 4},
                            "message": format!("{marker}:{project_revision}:{view_revision}"),
                            "attribution": "source"
                        }],
                        "definition": {
                            "kind": "script",
                            "fileId": script_file,
                            "range": {"start": 0, "end": 4}
                        }
                    })
                })
                .collect()
        });
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": protocol_version,
            "requestId": request_id,
            "jsCheckApplied": "pragma",
            "registration": {"kind": "page", "count": 1, "topLevel": true},
            "states": {
                "hover": {"status": "supported"},
                "diagnostics": {"status": "supported"},
                "definition": {"status": "supported"}
            },
            "insertions": [{"viewRevision": view_revision, "backendFileVersion": 1}],
            "mounts": [],
            "fragments": fragments,
            "moduleKind": {"before": "script", "after": "script"},
            "suppressed": {"count": 0, "details": []}
        }
    }))
    .expect("response json")
}

pub fn request_count_after(shared: &Shared, baseline: usize) -> usize {
    shared.request_count().saturating_sub(baseline)
}
