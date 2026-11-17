#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

//! Repeatable Linux performance benchmark for the Q2 scheduling work.
//!
//! The benchmark is `#[ignore]`d so ordinary `cargo test` runs stay fast. It is
//! an explicit gate:
//!
//! ```bash
//! (cd backends/typescript && npm ci && npm run build)
//! WXML_TEST_NODE=1 WXML_PERF_LABEL=after WXML_PERF_REQUIRE_CLEAN=1 \
//! WXML_PERF_REQUIRE_BASELINE=1 WXML_PERF_BASELINE=/tmp/opencode/q2-bench/w3-baseline-r2.json \
//!   CARGO_TARGET_DIR=/home/zxg/.cache/wxml-targets/full-performance-local \
//!   cargo test -p wxml-lsp --test performance_bench -- --ignored --nocapture
//! ```
//!
//! Raw samples are written as JSON to `WXML_PERF_OUT` (default
//! `/tmp/opencode/q2-bench/<label>.json`). The output is evidence, never a
//! committed fixture. Absolute targets, p95 regression and parent RSS are
//! asserted by the test; a baseline JSON is required for the W4 gate. Set
//! `WXML_PERF_BASELINE_ONLY=1` only when producing that baseline from W3.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig, NodeTransport, NodeTransportError};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::run_with_host;

mod common;

use common::TempProject;

const WXML: &str = "pages/index/index.wxml";
const SCRIPT: &str = "pages/index/index.ts";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

fn sample_count() -> usize {
    std::env::var("WXML_PERF_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value >= 10)
        .unwrap_or(200)
}

fn label() -> String {
    std::env::var("WXML_PERF_LABEL").unwrap_or_else(|_| "run".to_owned())
}

fn output_path(label: &str) -> PathBuf {
    std::env::var_os("WXML_PERF_OUT").map_or_else(
        || PathBuf::from(format!("/tmp/opencode/q2-bench/{label}.json")),
        PathBuf::from,
    )
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn backend_paths() -> (PathBuf, PathBuf) {
    let repo = repo_root();
    let server = std::env::var_os("WXML_TEST_BACKEND_SERVER").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    let backend_dir = std::env::var_os("WXML_TEST_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);
    assert!(
        server.is_file(),
        "build the Node backend first: {}",
        server.display()
    );
    (server, backend_dir)
}

fn project_fixture() -> TempProject {
    let project = TempProject::new("wxml-q2-bench");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write(WXML, "<view class=\"card\">{{ user.name }}</view>");
    project.write(
        SCRIPT,
        "Page({ data: { user: { name: 'Ada', age: 36 } } });",
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    project
}

struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    pending: VecDeque<Value>,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
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

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("json");
        write!(self.writer, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        self.writer.write_all(&body).expect("body");
        self.writer.flush().expect("flush");
    }

    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn recv(&mut self) -> Value {
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

    fn wait_diagnostics(&mut self, uri: &str, min_version: u64) -> Value {
        self.next_matching(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && message["params"]["uri"].as_str() == Some(uri)
                && message["params"]["version"]
                    .as_u64()
                    .is_some_and(|version| version >= min_version)
        })
    }

    fn completion(&mut self, uri: &str, line: u32, character: u32) -> Duration {
        let started = Instant::now();
        let id = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        let response = self.response(id);
        assert!(
            response.get("result").is_some(),
            "completion failed: {response}"
        );
        started.elapsed()
    }

    fn hover(&mut self, uri: &str, line: u32, character: u32) -> Duration {
        let started = Instant::now();
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        let response = self.response(id);
        assert!(response.get("result").is_some(), "hover failed: {response}");
        started.elapsed()
    }

    fn initialize(&mut self, project: &TempProject) {
        let id = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": project.root_uri(),
                "capabilities": {},
                "initializationOptions": {"wxml": {"diagnostics": {"debounceMs": 120}}},
            }),
        );
        let response = self.response(id);
        assert!(response.get("result").is_some(), "{response}");
        self.notify("initialized", json!({}));
    }

    fn open(&mut self, uri: &str, text: &str, version: u64) {
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
    }

    fn finish(mut self) {
        let shutdown = self.request("shutdown", json!(null));
        let response = self.response(shutdown);
        assert!(response.get("result").is_some(), "{response}");
        self.notify("exit", json!(null));
    }
}

struct Session {
    client: Client,
    handle: thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>,
}

fn start_real_node(project: &TempProject) -> Session {
    let (server, backend_dir) = backend_paths();
    let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
    let server_read = server_stream.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::new(
        NodeBackendConfig {
            server: Some(server),
            backend_dir: Some(backend_dir),
            timeout: Duration::from_secs(30),
            ..NodeBackendConfig::default()
        },
        host.clone(),
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server_stream, backend, host));
    let mut client = Client::new(client_stream);
    client.initialize(project);
    Session { client, handle }
}

impl Session {
    fn finish(self) {
        self.client.finish();
        let outcome = self.handle.join().expect("server thread").expect("outcome");
        assert!(outcome.clean_shutdown);
    }
}

fn percentile(samples: &mut [f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let index = (fraction * samples.len() as f64).ceil() as usize;
    let index = index.clamp(1, samples.len()) - 1;
    samples[index]
}

fn summary(samples: &mut [f64]) -> Value {
    let p50 = percentile(samples, 0.50);
    let p95 = percentile(samples, 0.95);
    let p99 = percentile(samples, 0.99);
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    json!({
        "count": samples.len(),
        "mean_ms": round(mean),
        "p50_ms": round(p50),
        "p95_ms": round(p95),
        "p99_ms": round(p99),
        "max_ms": round(samples.iter().copied().fold(0.0, f64::max)),
    })
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map_or(0, |entries| entries.filter_map(Result::ok).count())
}

fn node_version() -> String {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unavailable".to_owned(), |value| value.trim().to_owned())
}

fn git_head() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root())
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |value| value.trim().to_owned())
}

fn git_dirty() -> String {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_root())
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |value| value.trim().to_owned())
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |value| value.trim().to_owned())
}

fn rustc_version() -> String {
    command_line("rustc", &["-vV"])
        .lines()
        .next()
        .unwrap_or("unknown")
        .to_owned()
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("model name\t: "))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn cpu_count() -> usize {
    thread::available_parallelism().map_or(0, std::num::NonZero::get)
}

fn mem_total_kib() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0)
}

fn os_pretty_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                .map(|value| value.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn kernel_release() -> String {
    command_line("uname", &["-r"])
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .map_or_else(|_| "unknown".to_owned(), |text| text.trim().to_owned())
}

fn static_completion(project: &TempProject) -> Value {
    const CALLS_PER_OUTER_ITERATION: usize = 50;
    let mut session = start_real_node(project);
    let uri = project.uri(WXML);
    let text = "<view class=\"card\">{{ user.name }}</view>";
    session.client.open(&uri, text, 1);
    session.client.wait_diagnostics(&uri, 1);
    // `<view cla` is an attribute-name site answered without Node. A single
    // request is ~0.2 ms, below the resolution needed for a 10% gate. Each
    // outer iteration issues 50 independent calls and every individual call
    // time is summarized, so the reported p95 is per independent call.
    let character = u32::try_from("<view cla".chars().count()).expect("characters");
    for _ in 0..20 {
        session.client.completion(&uri, 0, character);
    }
    let mut samples = Vec::with_capacity(sample_count().saturating_mul(CALLS_PER_OUTER_ITERATION));
    for _ in 0..sample_count() {
        for _ in 0..CALLS_PER_OUTER_ITERATION {
            samples.push(millis(session.client.completion(&uri, 0, character)));
        }
    }
    session.finish();
    let mut summary = summary(&mut samples);
    if let Some(object) = summary.as_object_mut() {
        object.insert(
            "independent_calls_per_outer_iteration".to_owned(),
            json!(CALLS_PER_OUTER_ITERATION),
        );
    }
    summary
}

fn semantic_completion(project: &TempProject) -> (Value, Value) {
    let mut session = start_real_node(project);
    let uri = project.uri(WXML);
    // Distinct partial member prefixes exercise the path a user typing
    // `{{ user.na`, `{{ user.nam`, ... takes. The document is not edited, so
    // this isolates Node project reuse from LSP document/debounce overhead.
    let text = (0..32)
        .map(|index| format!("<view>{{{{ user.na{index} }}}}</view>"))
        .collect::<Vec<_>>()
        .join("\n");
    session.client.open(&uri, &text, 1);
    session.client.wait_diagnostics(&uri, 1);
    let positions: Vec<(u32, u32)> = text
        .lines()
        .enumerate()
        .map(|(line, value)| {
            let character =
                u32::try_from(value.find("user.").expect("member") + "user.".len()).expect("chars");
            (u32::try_from(line).expect("line"), character)
        })
        .collect();
    for (line, character) in positions.iter().take(8) {
        session.client.completion(&uri, *line, *character);
    }
    let mut samples = Vec::with_capacity(sample_count());
    for index in 0..sample_count() {
        let (line, character) = positions[index % positions.len()];
        samples.push(millis(session.client.completion(&uri, line, character)));
    }
    let mut hovers = Vec::with_capacity(sample_count());
    for index in 0..sample_count() {
        let (line, character) = positions[index % positions.len()];
        hovers.push(millis(session.client.hover(&uri, line, character)));
    }
    session.finish();
    (summary(&mut samples), summary(&mut hovers))
}

fn diagnostics_stop(project: &TempProject) -> Value {
    let mut session = start_real_node(project);
    let uri = project.uri(WXML);
    session
        .client
        .open(&uri, "<view class=\"card\">{{ user.name }}</view>", 1);
    session.client.wait_diagnostics(&uri, 1);
    let mut version = 2_u64;
    for _ in 0..10 {
        let text =
            format!("<view class=\"card\">{{{{ user.name }}}}</view>\n<!-- warm {version} -->");
        session.client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
        session.client.wait_diagnostics(&uri, version);
        version += 1;
    }
    let mut samples = Vec::with_capacity(sample_count());
    for _ in 0..sample_count() {
        let text =
            format!("<view class=\"card\">{{{{ user.name }}}}</view>\n<!-- edit {version} -->");
        let started = Instant::now();
        session.client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
        session.client.wait_diagnostics(&uri, version);
        samples.push(millis(started.elapsed()));
        version += 1;
    }
    session.finish();
    summary(&mut samples)
}

/// A transport that blocks every analyze/symbol request for a fixed delay.
struct SlowTransport {
    delay: Duration,
    requests: AtomicU64,
}

impl NodeTransport for SlowTransport {
    fn request(&self, line: &str, _timeout: Duration) -> Result<String, NodeTransportError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        thread::sleep(self.delay);
        Ok(slow_response(line))
    }

    fn shutdown(&self) {}
}

fn slow_response(line: &str) -> String {
    let request: Value = serde_json::from_str(line).expect("request");
    let id = &request["id"];
    let params = &request["params"];
    let request_id = params["requestId"].as_str().unwrap_or("r");
    let view_revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .unwrap_or(1);
    let project_revision = params["analysisStamp"]["projectRevision"]
        .as_u64()
        .unwrap_or(1);
    let script_file = params["scriptFileId"].as_str().unwrap_or("s");
    let fragments = params["fragments"]
        .as_array()
        .map_or_else(Vec::new, |fragments| {
            fragments
                .iter()
                .map(|fragment| {
                    json!({
                        "fragmentId": fragment["fragmentId"],
                        "hover": {
                            "typeText": "string",
                            "typeOnly": "string",
                            "quality": "known",
                            "documentation": "synthetic",
                            "documentationSource": "quickinfo"
                        },
                        "diagnostics": [],
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
            "protocolVersion": 2,
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
            "suppressed": {"count": 0, "details": []},
            "projectRevision": project_revision
        }
    }))
    .expect("response")
}

fn slow_session(project: &TempProject) -> Session {
    let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
    let server_read = server_stream.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        || {
            Ok(Box::new(SlowTransport {
                delay: Duration::from_millis(400),
                requests: AtomicU64::new(0),
            }))
        },
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server_stream, backend, host));
    let mut client = Client::new(client_stream);
    client.initialize(project);
    Session { client, handle }
}

fn cancel_latency(project: &TempProject) -> (Value, Value) {
    let mut session = slow_session(project);
    let uri = project.uri(WXML);
    // `{{ user. }}` is a Node-backed member completion; completion never uses
    // the analysis cache, so each request reaches the slow transport.
    let text = "<view class=\"card\">{{ user. }}</view>";
    session.client.open(&uri, text, 1);
    let character =
        u32::try_from(text.find("user.").expect("member") + "user.".len()).expect("chars");

    let mut queue_samples = Vec::with_capacity(sample_count());
    let mut running_samples = Vec::with_capacity(sample_count());
    for round in 0..sample_count() {
        // A running backend call is not interruptible. The cancellation
        // response is immediate; the actual call occupancy is measured by
        // `slow_call_occupancy`, not by this response latency.
        let running_id = session.client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": character},
            }),
        );
        thread::sleep(Duration::from_millis(5));
        let running_started = Instant::now();
        session
            .client
            .notify("$/cancelRequest", json!({"id": running_id}));
        let running = session.client.response(running_id);
        running_samples.push(millis(running_started.elapsed()));
        assert!(
            running.get("error").is_some(),
            "a cancelled running request must answer with an error: round {round} {running}"
        );

        // The first completion occupies the worker; the second waits in the
        // queue. Cancelling the queued request must not wait for the running
        // TypeScript call.
        let busy_id = session.client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": character},
            }),
        );
        thread::sleep(Duration::from_millis(5));
        let queued_id = session.client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": character},
            }),
        );
        thread::sleep(Duration::from_millis(5));
        let cancel_started = Instant::now();
        session
            .client
            .notify("$/cancelRequest", json!({"id": queued_id}));
        let queued = session.client.response(queued_id);
        queue_samples.push(millis(cancel_started.elapsed()));
        assert!(
            queued.get("error").is_some(),
            "cancelled request must answer with an error: round {round} {queued}"
        );
        let busy = session.client.response(busy_id);
        assert!(
            busy.get("result").is_some() || busy.get("error").is_some(),
            "running request must answer: round {round} {busy}"
        );
    }
    session.finish();
    (summary(&mut queue_samples), summary(&mut running_samples))
}

fn slow_call_occupancy(project: &TempProject) -> Value {
    let mut session = slow_session(project);
    let uri = project.uri(WXML);
    let text = "<view class=\"card\">{{ user. }}</view>";
    session.client.open(&uri, text, 1);
    let character =
        u32::try_from(text.find("user.").expect("member") + "user.".len()).expect("chars");
    let count = std::env::var("WXML_PERF_TS_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value >= 5)
        .unwrap_or(20);
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        samples.push(millis(session.client.completion(&uri, 0, character)));
    }
    session.finish();
    summary(&mut samples)
}

fn p95(summary: &Value) -> f64 {
    summary["p95_ms"].as_f64().unwrap_or(f64::INFINITY)
}

fn check_limit(failures: &mut Vec<String>, name: &str, value: f64, limit: f64) {
    let pass = value <= limit;
    println!("threshold {name}: {value:.3} <= {limit:.3} -> {pass}");
    if !pass {
        failures.push(format!("{name}: {value:.3} > {limit:.3}"));
    }
}

fn check_regression(
    failures: &mut Vec<String>,
    name: &str,
    after: f64,
    baseline: f64,
    allowed_fraction: f64,
) {
    let limit = baseline * (1.0 + allowed_fraction);
    let pass = after <= limit;
    println!(
        "regression {name}: after {after:.3} <= baseline {baseline:.3} * {:.2} = {limit:.3} -> {pass}",
        1.0 + allowed_fraction
    );
    if !pass {
        failures.push(format!(
            "{name}: after {after:.3} > baseline {baseline:.3} * {:.2}",
            1.0 + allowed_fraction
        ));
    }
}

#[test]
#[ignore = "performance gate: set WXML_TEST_NODE=1 and run with --ignored"]
fn q2_performance_benchmark() {
    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the performance gate"
    );
    let require_clean = std::env::var("WXML_PERF_REQUIRE_CLEAN").as_deref() == Ok("1");
    let baseline_only = std::env::var("WXML_PERF_BASELINE_ONLY").as_deref() == Ok("1");
    let head = git_head();
    let dirty = git_dirty();
    if require_clean {
        assert!(
            dirty.is_empty(),
            "performance gate requires a clean worktree; dirty:\n{dirty}"
        );
    }
    let project = project_fixture();
    let label = label();
    let started = Instant::now();
    let rss_before = rss_kib();
    let threads_before = threads();
    let load_start = loadavg();

    let static_summary = static_completion(&project);
    println!("static_completion: {static_summary}");
    let (semantic_summary, hover_summary) = semantic_completion(&project);
    println!("semantic_completion: {semantic_summary}");
    println!("semantic_hover: {hover_summary}");
    let diagnostics_summary = diagnostics_stop(&project);
    println!("diagnostics_stop: {diagnostics_summary}");
    let (cancel_queue_summary, cancel_running_summary) = cancel_latency(&project);
    println!("cancel_queued_request: {cancel_queue_summary}");
    println!("cancel_running_response: {cancel_running_summary}");
    let ts_call_summary = slow_call_occupancy(&project);
    println!("single_ts_call_occupancy: {ts_call_summary}");

    let rss_after = rss_kib();
    let threads_after = threads();
    let load_end = loadavg();
    let debounce_ms = 120_u64; // pinned by the initialization options in start_real_node

    let mut failures = Vec::new();
    check_limit(
        &mut failures,
        "static_completion_p95",
        p95(&static_summary),
        50.0,
    );
    check_limit(
        &mut failures,
        "semantic_completion_p95",
        p95(&semantic_summary),
        200.0,
    );
    check_limit(
        &mut failures,
        "diagnostics_stop_p95",
        p95(&diagnostics_summary),
        500.0,
    );
    check_limit(
        &mut failures,
        "cancel_queued_request_p95",
        p95(&cancel_queue_summary),
        100.0,
    );

    let baseline_value = std::env::var_os("WXML_PERF_BASELINE").and_then(|path| {
        std::fs::read_to_string(&path).ok().map(|text| {
            let parsed: Value = serde_json::from_str(&text).expect("baseline performance JSON");
            (PathBuf::from(path), parsed)
        })
    });
    if let Some((_baseline_path, baseline)) = baseline_value.as_ref() {
        for (name, after) in [
            ("static_completion", p95(&static_summary)),
            ("semantic_completion", p95(&semantic_summary)),
            ("semantic_hover", p95(&hover_summary)),
            ("diagnostics_stop", p95(&diagnostics_summary)),
            ("cancel_queued_request", p95(&cancel_queue_summary)),
        ] {
            let Some(before) = baseline["scenarios"][name]["p95_ms"].as_f64() else {
                failures.push(format!("baseline {name} is missing"));
                continue;
            };
            check_regression(&mut failures, name, after, before, 0.10);
        }
        if let (Some(before), Some(after)) = (
            baseline["resources"]["rss_kib_after"].as_u64(),
            Some(rss_after),
        ) {
            check_regression(
                &mut failures,
                "parent_rss_kib",
                after as f64,
                before as f64,
                0.15,
            );
        } else {
            failures.push("baseline parent RSS is missing".to_owned());
        }
    } else if std::env::var("WXML_PERF_REQUIRE_BASELINE").as_deref() == Ok("1") {
        failures.push("WXML_PERF_BASELINE is required but missing/unreadable".to_owned());
    }

    let report = json!({
        "label": label,
        "harness": "q2-r2-batch",
        "baseline_only": baseline_only,
        "git_head": head,
        "git_dirty": dirty,
        "generated_at_unix_s": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        "environment": {
            "node": node_version(),
            "rustc": rustc_version(),
            "target_dir": std::env::var("CARGO_TARGET_DIR").unwrap_or_default(),
            "samples_per_scenario": sample_count(),
            "static_outer_iterations": sample_count(),
            "static_independent_calls": sample_count().saturating_mul(50),
            "warmups": {
                "static_completion_calls": 20,
                "semantic_completion_calls": 8,
                "semantic_hover_calls": 0,
                "diagnostics_edits": 10,
                "cancel_rounds": 0,
                "single_ts_call_calls": 0
            },
            "diagnostics_debounce_ms": debounce_ms,
            "diagnostics_backend_estimate_ms": round(
                (p95(&diagnostics_summary) - debounce_ms as f64).max(0.0)
            ),
            "outliers": "max_ms and p99_ms include outliers; raw percentile method is nearest-rank",
        },
        "machine": {
            "os": os_pretty_name(),
            "kernel": kernel_release(),
            "cpu": cpu_model(),
            "cpu_count": cpu_count(),
            "mem_total_kib": mem_total_kib(),
            "loadavg_start": load_start,
            "loadavg_end": load_end,
            "concurrency": "single benchmark process, no other benchmark running"
        },
        "baseline": baseline_value.as_ref().map(|(path, _)| path.display().to_string()),
        "scenarios": {
            "static_completion": static_summary,
            "semantic_completion": semantic_summary,
            "semantic_hover": hover_summary,
            "diagnostics_stop": diagnostics_summary,
            "cancel_queued_request": cancel_queue_summary,
            "cancel_running_response": cancel_running_summary,
            "single_ts_call_occupancy": ts_call_summary,
        },
        "resources": {
            "rss_kib_before": rss_before,
            "rss_kib_after": rss_after,
            "threads_before": threads_before,
            "threads_after": threads_after,
            "wall_time_ms": millis(started.elapsed()),
        },
        "thresholds": {
            "static_completion_p95_ms": 50.0,
            "semantic_completion_p95_ms": 200.0,
            "diagnostics_stop_p95_ms": 500.0,
            "cancel_queued_request_p95_ms": 100.0,
            "p95_regression_fraction": 0.10,
            "parent_rss_regression_fraction": 0.15,
            "pass": failures.is_empty(),
            "failures": failures,
        },
        "notes": [
            "cancel_queued_request measures the LSP response path for a job that has not started yet.",
            "cancel_running_response is the immediate protocol response for a running call; a synchronous TypeScript call is not interruptible.",
            "single_ts_call_occupancy measures one uncached slow backend call and is reported separately.",
            "semantic_hover exercises the Rust analysis-cache hit path; it is not a cold Node hover measurement.",
            "static_completion reports p95 across 10,000 independent calls (50 per outer iteration, 200 outer iterations).",
            "diagnostics_stop is measured from the final didChange notification to the matching publishDiagnostics version."
        ]
    });
    let path = output_path(report["label"].as_str().unwrap_or("run"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("output directory");
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&report).expect("json")).expect("write report");
    println!("performance report: {}", path.display());
    println!("{}", serde_json::to_string_pretty(&report).expect("json"));
    if !baseline_only {
        assert!(
            report["thresholds"]["pass"].as_bool() == Some(true),
            "performance thresholds failed: {}",
            report["thresholds"]["failures"]
        );
    }
}
