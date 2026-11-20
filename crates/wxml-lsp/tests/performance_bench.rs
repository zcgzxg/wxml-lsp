#![cfg(target_os = "linux")]
#![allow(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

//! P7 native-only shell for `G-P5-PERF-01`.
//!
//! The frozen gate coordinate is kept, but the old Node process boundary is
//! replaced by the real `wxml-lsp --stdio --backend native` package from
//! `native_parity/native_package.rs`. The shell has two segments:
//!
//! 1. **real engine**: cold start, 10,000 independent static completions,
//!    semantic completion, hover and stop-typing diagnostics through the
//!    pinned 7.1.0-beta engine, plus server-tree RSS and one-engine accounting;
//! 2. **scheduler**: an in-process server over a 400 ms test `SlowBackend`
//!    reproduces the old `SlowTransport` cancel/queue/occupancy scenarios. It
//!    never starts Node or the engine and does not claim native engine cancel
//!    latency.
//!
//! The deep native oracle stays in
//! `native_parity/native_engine_real.rs::native_q2_performance_benchmark`; this
//! shell does not duplicate it.
//!
//! ```bash
//! WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
//! TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
//! TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
//! TSGO_TYPINGS_ROOT=/tmp/opencode/tsgo-native-pkg/typings/miniprogram-api-typings \
//! cargo test -p wxml-lsp --release --test performance_bench -- \
//!   --ignored --test-threads=1
//! ```
//!
//! A debug run is rejected: the absolute p95 budgets are release baselines.
//! Raw samples are written to `WXML_PERF_OUT` (default
//! `/tmp/opencode/q2-bench/<label>.json`). `WXML_PERF_BASELINE` is an optional
//! pre-recorded JSON regression baseline; `WXML_PERF_REQUIRE_BASELINE=1`
//! makes it mandatory. Generated evidence is never committed.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wxml_backend_tsgo::config::{AST_PROTOCOL_VERSION, ENGINE_VERSION, PinnedEngine};
use wxml_lsp::project_host::{ProjectHost, ProjectState};
use wxml_lsp::run_with_host;
use wxml_types::{
    Backend, BackendCapabilities, BackendEpoch, BackendError, BackendId, CapabilityStatus,
    CompletionItem, CompletionItemKind, CompletionRequest, CompletionResult, DiagnosticsRequest,
    DiagnosticsResult, HoverRequest, HoverResult, InsertTextFormat, MarkupKind, SourceRange,
    TypeQuality,
};

#[path = "native_parity/native_gate_env.rs"]
mod native_gate_env;
#[path = "native_parity/native_lsp.rs"]
mod native_lsp;
#[path = "native_parity/native_package.rs"]
mod native_package;

#[cfg(target_os = "linux")]
#[path = "native_parity/native_measure.rs"]
mod native_measure;

/// The harness modules only need the workspace root from this crate's ledger.
mod ledger {
    use std::path::PathBuf;

    /// Returns the nearest ancestor containing the workspace `Cargo.toml`.
    pub(crate) fn workspace_root() -> PathBuf {
        let start = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for ancestor in start.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            if text.lines().any(|line| line.trim() == "[workspace]") {
                return ancestor.to_path_buf();
            }
        }
        panic!("workspace root not found above {}", start.display());
    }
}

use native_gate_env::PINNED_ENGINE_SIZE;
use native_lsp::{Session as NativeSession, hover_is_empty};
use native_measure::{
    Stats, check_p95, elapsed_ms, engine_children, growth_percent, rss_kib, sample_tree, threads,
    wait_gone,
};
use native_package::NativeGate;

const INDEX_WXML: &str = "pages/index/index.wxml";
const COMPLETE_WXML: &str = "pages/complete/complete.wxml";
const PAGE_TEXT: &str = r#"<view class="card">{{ user.name }} {{ title }}</view>"#;
const COMPLETE: &str = "<view>{{ user. }}</view>";
const SCRIPT: &str = "Page({ data: { user: { name: 'Ada', age: 36 }, title: 'Hello' } });";
const STATIC_SITE: &str = "<view cla";
const HOVER_P95_MS: f64 = 200.0;
const COMPLETION_P95_MS: f64 = 200.0;
const DIAGNOSTICS_P95_MS: f64 = 500.0;
const STATIC_COMPLETION_P95_MS: f64 = 50.0;
const CANCEL_P95_MS: f64 = 100.0;
const RSS_GROWTH_PERCENT_MAX: f64 = 15.0;
const OCCUPANCY_P50_FLOOR_MS: f64 = 300.0;
const SLOW_BACKEND_DELAY: Duration = Duration::from_millis(400);
const CALLS_PER_OUTER_ITERATION: usize = 50;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

fn sample_count() -> usize {
    std::env::var("WXML_PERF_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value >= 10)
        .unwrap_or(200)
}

fn slow_call_count() -> usize {
    std::env::var("WXML_PERF_TS_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value >= 5)
        .unwrap_or(20)
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

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn write_fixture(gate: &NativeGate) {
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/complete/complete"],"usingComponents":{}}"#,
    );
    gate.write(INDEX_WXML, PAGE_TEXT);
    gate.write("pages/index/index.ts", SCRIPT);
    gate.write(COMPLETE_WXML, COMPLETE);
    gate.write("pages/complete/complete.ts", SCRIPT);
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
}

fn command_line(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |value| value.trim().to_owned())
}

fn git_output(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .current_dir(repo_root())
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

fn parent_rss_kib() -> u64 {
    rss_kib(std::process::id()).unwrap_or(0)
}

fn parent_threads() -> usize {
    threads(std::process::id()).unwrap_or(0)
}

fn engine_identity() -> Value {
    let pin = PinnedEngine::pinned();
    json!({
        "version": ENGINE_VERSION,
        "git_head": pin.git_head(),
        "ast_protocol": AST_PROTOCOL_VERSION,
        "executable_sha256": pin.executable_sha256(),
        "executable_size_bytes": PINNED_ENGINE_SIZE,
    })
}

fn require_hover(
    session: &mut NativeSession,
    uri: &str,
    position: (u32, u32),
    context: &str,
) -> f64 {
    let started = Instant::now();
    let response = session.hover(uri, position);
    let elapsed = elapsed_ms(started);
    assert!(
        response.get("result").is_some(),
        "{context} failed: {}",
        session.describe(&response)
    );
    assert!(
        !hover_is_empty(&response),
        "{context} returned an empty hover: {}",
        session.describe(&response)
    );
    elapsed
}

fn require_completion(
    session: &mut NativeSession,
    uri: &str,
    position: (u32, u32),
    context: &str,
) -> f64 {
    let started = Instant::now();
    let response = session.completion(uri, position);
    let elapsed = elapsed_ms(started);
    let items = response["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("{context} returned no completion list: {response}"));
    assert!(
        !items.is_empty(),
        "{context} returned an empty completion list: {response}"
    );
    elapsed
}

fn real_engine_segment(gate: &NativeGate, failures: &mut Vec<String>) -> Value {
    let index_uri = gate.uri(INDEX_WXML);
    let complete_uri = gate.uri(COMPLETE_WXML);
    let name_position = native_lsp::lsp_position(PAGE_TEXT, "name");
    let member = native_lsp::lsp_position(COMPLETE, "user.");
    let member_position = (member.0, member.1 + 5);
    // `<view cla` is answered without the engine (static attribute names). One
    // request is far below the 10% regression resolution, so 50 independent
    // calls are timed per outer iteration while every call remains a sample.
    let static_character = u32::try_from(STATIC_SITE.chars().count()).expect("characters");

    let started = Instant::now();
    let mut session = NativeSession::start(gate);
    let initialize_ms = elapsed_ms(started);

    let open_started = Instant::now();
    let diagnostics = session.open_wxml(&index_uri, PAGE_TEXT, 1);
    let first_page_ms = elapsed_ms(open_started);
    assert_eq!(
        diagnostics,
        json!([]),
        "the performance page must start clean: {diagnostics}"
    );
    let cold_open_total_ms = elapsed_ms(started);

    let first_hover_started = Instant::now();
    require_hover(&mut session, &index_uri, name_position, "cold first hover");
    let first_engine_hover_ms = elapsed_ms(first_hover_started);

    session.open_wxml(&complete_uri, COMPLETE, 1);

    let mut version = 1_u64;
    for iteration in 0..5 {
        require_hover(&mut session, &index_uri, name_position, "hover warm-up");
        require_completion(
            &mut session,
            &complete_uri,
            member_position,
            "completion warm-up",
        );
        version += 1;
        let text = format!("{PAGE_TEXT}\n<!-- warm {iteration} -->\n");
        let diagnostics = session.change_wxml(&index_uri, &text, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "diagnostics warm-up must stay clean: {diagnostics}"
        );
    }

    let parent_rss_start = parent_rss_kib();
    let baseline = sample_tree(session.pid());
    assert!(
        baseline.rss_kib > 0,
        "the native server tree must expose a non-zero RSS baseline"
    );

    let mut static_samples = Vec::with_capacity(sample_count() * CALLS_PER_OUTER_ITERATION);
    for _ in 0..sample_count() {
        for _ in 0..CALLS_PER_OUTER_ITERATION {
            static_samples.push(require_completion(
                &mut session,
                &index_uri,
                (0, static_character),
                "static completion",
            ));
        }
    }

    let mut hover_samples = Vec::with_capacity(sample_count());
    for _ in 0..sample_count() {
        hover_samples.push(require_hover(
            &mut session,
            &index_uri,
            name_position,
            "hot hover",
        ));
    }

    let mut completion_samples = Vec::with_capacity(sample_count());
    for _ in 0..sample_count() {
        completion_samples.push(require_completion(
            &mut session,
            &complete_uri,
            member_position,
            "hot completion",
        ));
    }

    let mut diagnostics_samples = Vec::with_capacity(sample_count());
    for sample in 0..sample_count() {
        version += 1;
        let text = format!("{PAGE_TEXT}\n<!-- edit {sample} -->\n");
        let started = Instant::now();
        let diagnostics = session.change_wxml(&index_uri, &text, version);
        diagnostics_samples.push(elapsed_ms(started));
        assert_eq!(
            diagnostics,
            json!([]),
            "a clean edit must publish no diagnostics: {diagnostics}"
        );
    }

    let final_sample = sample_tree(session.pid());
    let parent_rss_end = parent_rss_kib();
    let engines = engine_children(session.pid(), gate.package_root());
    assert_eq!(
        engines.len(),
        1,
        "the performance run must keep exactly one engine child: {engines:?}"
    );
    let descendants = session.descendant_pids();
    assert!(
        descendants.len() <= 1,
        "the performance run leaked child processes: {descendants:?}"
    );
    session.assert_no_node_exec();
    let pid = session.pid();
    let evidence = session.shutdown_with_evidence();
    assert!(
        !evidence.stderr.contains("detaching it"),
        "server detached a worker on shutdown; stderr:\n{}",
        evidence.stderr
    );
    wait_gone(&[pid], "server after perf shutdown");
    wait_gone(&descendants, "engine after perf shutdown");
    gate.assert_no_native_sandbox();
    gate.tripwire().assert_absent();

    let static_stats = Stats::from_samples(&static_samples);
    let hover = Stats::from_samples(&hover_samples);
    let completion = Stats::from_samples(&completion_samples);
    let diagnostics = Stats::from_samples(&diagnostics_samples);
    check_p95(
        failures,
        "static_completion",
        &static_stats,
        STATIC_COMPLETION_P95_MS,
    );
    check_p95(failures, "semantic_hover", &hover, HOVER_P95_MS);
    check_p95(
        failures,
        "semantic_completion",
        &completion,
        COMPLETION_P95_MS,
    );
    check_p95(
        failures,
        "diagnostics_stop",
        &diagnostics,
        DIAGNOSTICS_P95_MS,
    );

    let rss_growth = growth_percent(baseline.rss_kib, final_sample.rss_kib);
    println!(
        "server tree RSS: baseline {} KiB, final {} KiB, growth {rss_growth:.2}%",
        baseline.rss_kib, final_sample.rss_kib
    );
    if !rss_growth.is_finite() {
        failures.push("server tree RSS baseline is zero".to_owned());
    } else if rss_growth > RSS_GROWTH_PERCENT_MAX {
        failures.push(format!(
            "server tree RSS grew {rss_growth:.2}% (cap {RSS_GROWTH_PERCENT_MAX:.0}%)"
        ));
    }

    json!({
        "transport": "--stdio --backend native",
        "cold_start": {
            "spawn_initialize_ms": round(initialize_ms),
            "open_and_first_diagnostics_ms": round(first_page_ms),
            "first_page_total_ms": round(cold_open_total_ms),
            "first_engine_hover_ms": round(first_engine_hover_ms),
            "budget": "recorded only; the frozen gate defines no cold-start p95",
        },
        "metrics": {
            "static_completion": static_stats.to_json(),
            "semantic_completion": completion.to_json(),
            "semantic_hover": hover.to_json(),
            "diagnostics_stop": diagnostics.to_json(),
        },
        "samples": {
            "static_completion_calls": static_samples.len(),
            "static_calls_per_outer_iteration": CALLS_PER_OUTER_ITERATION,
            "semantic_calls_per_metric": sample_count(),
            "warmup_calls_per_metric": 5,
            "diagnostics_debounce_ms": 0,
            "diagnostics_debounce_note": "the native harness pins 0 ms, which is stricter than the old 120 ms gate",
        },
        "resources": {
            "parent_rss_kib_start": parent_rss_start,
            "parent_rss_kib_end": parent_rss_end,
            "server_tree_rss_kib_baseline": baseline.rss_kib,
            "server_tree_rss_kib_final": final_sample.rss_kib,
            "server_tree_rss_growth_percent": round(rss_growth),
            "server_tree_cpu_ticks_delta": final_sample.cpu_ticks.saturating_sub(baseline.cpu_ticks),
            "engine_processes": engines.len(),
        },
        "shutdown": {
            "server_pid": evidence.pid,
            "elapsed_ms": round(millis(evidence.elapsed)),
            "detached_worker": false,
            "sandbox_left": false,
            "processes_left": 0,
        },
    })
}

/// A backend that sleeps on every completion, like the retired 400 ms
/// `SlowTransport`. It is an in-process test double: it never starts Node or
/// the engine, and it is used only for scheduler cancellation/queue timing.
struct SlowBackend {
    delay: Duration,
    completions: AtomicU64,
    hovers: AtomicU64,
}

impl SlowBackend {
    const fn new(delay: Duration) -> Self {
        Self {
            delay,
            completions: AtomicU64::new(0),
            hovers: AtomicU64::new(0),
        }
    }

    fn completion_calls(&self) -> u64 {
        self.completions.load(Ordering::SeqCst)
    }
}

impl Backend for SlowBackend {
    fn epoch(&self) -> BackendEpoch {
        BackendEpoch::new(BackendId::new("p7-perf-slow"), 1)
    }

    fn hover(&self, _request: HoverRequest) -> Result<HoverResult, BackendError> {
        self.hovers.fetch_add(1, Ordering::SeqCst);
        thread::sleep(self.delay);
        Ok(HoverResult {
            contents: "string".to_owned(),
            markup: MarkupKind::Markdown,
            range: None,
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Known,
        })
    }

    fn diagnostics(&self, _request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError> {
        Ok(DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        })
    }

    fn completion(&self, request: CompletionRequest) -> Result<CompletionResult, BackendError> {
        self.completions.fetch_add(1, Ordering::SeqCst);
        thread::sleep(self.delay);
        let end = request.offset;
        let start = end.saturating_sub(1);
        Ok(CompletionResult {
            items: vec![CompletionItem {
                label: "name".to_owned(),
                kind: CompletionItemKind::Property,
                detail: Some("string".to_owned()),
                documentation: None,
                insert_text: "name".to_owned(),
                insert_text_format: InsertTextFormat::PlainText,
                replace: SourceRange::from_bounds(start, end).expect("valid slow completion range"),
                sort_text: None,
                filter_text: None,
            }],
            status: CapabilityStatus::Supported,
            truncated: false,
        })
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            hover: CapabilityStatus::Supported,
            diagnostics: CapabilityStatus::Supported,
            definition: CapabilityStatus::Unsupported {
                reason: "definition is not implemented".to_owned(),
            },
            completion: CapabilityStatus::Supported,
        }
    }
}

/// Minimal JSON-RPC client for the in-process scheduler segment.
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

    fn completion(&mut self, uri: &str, line: u32, character: u32, case: &str) -> Duration {
        let started = Instant::now();
        let id = self.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        let response = self.response(id);
        let elapsed = started.elapsed();
        assert!(
            response.get("result").is_some(),
            "{case} completion failed: {response}"
        );
        elapsed
    }

    fn initialize(&mut self, root_uri: &str) {
        let id = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": root_uri,
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

struct SlowSession {
    client: Client,
    backend: Arc<SlowBackend>,
    handle: thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>,
}

impl SlowSession {
    fn finish(self) {
        self.client.finish();
        let outcome = self.handle.join().expect("server thread").expect("outcome");
        assert!(outcome.clean_shutdown);
    }
}

fn start_slow(project_uri: &str) -> SlowSession {
    let backend = Arc::new(SlowBackend::new(SLOW_BACKEND_DELAY));
    let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
    let server_read = server_stream.try_clone().expect("clone");
    let host: Arc<dyn ProjectHost> = Arc::new(ProjectState::new());
    let backend_for_server: Arc<dyn Backend> = backend.clone();
    let handle =
        thread::spawn(move || run_with_host(server_read, server_stream, backend_for_server, host));
    let mut client = Client::new(client_stream);
    client.initialize(project_uri);
    SlowSession {
        client,
        backend,
        handle,
    }
}

fn scheduler_segment(gate: &NativeGate, failures: &mut Vec<String>) -> Value {
    let mut session = start_slow(&gate.project_uri());
    let uri = gate.uri(COMPLETE_WXML);
    let text = COMPLETE;
    session.client.open(&uri, text, 1);
    let member = native_lsp::lsp_position(text, "user.");
    let character = member.1 + 5;

    let mut queue_samples = Vec::with_capacity(sample_count());
    let mut running_samples = Vec::with_capacity(sample_count());
    for round in 0..sample_count() {
        // A running backend call is not interruptible. The cancellation
        // response is immediate; the call occupancy is measured separately.
        let running_id = session.client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": member.0, "character": character},
            }),
        );
        thread::sleep(Duration::from_millis(5));
        let running_started = Instant::now();
        session
            .client
            .notify("$/cancelRequest", json!({"id": running_id}));
        let running = session.client.response(running_id);
        running_samples.push(millis(running_started.elapsed()));
        assert_cancelled(&running, "running", round);

        // The first completion occupies the worker; the second waits in the
        // queue. Cancelling the queued request must not start it.
        let busy_id = session.client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": member.0, "character": character},
            }),
        );
        thread::sleep(Duration::from_millis(5));
        let queued_id = session.client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": member.0, "character": character},
            }),
        );
        thread::sleep(Duration::from_millis(5));
        let cancel_started = Instant::now();
        session
            .client
            .notify("$/cancelRequest", json!({"id": queued_id}));
        let queued = session.client.response(queued_id);
        queue_samples.push(millis(cancel_started.elapsed()));
        assert_cancelled(&queued, "queued", round);
        let busy = session.client.response(busy_id);
        assert!(
            busy.get("result").is_some() || busy.get("error").is_some(),
            "running request must answer: round {round} {busy}"
        );
    }
    let backend_completion_calls = session.backend.completion_calls();

    let occupancy_samples = {
        let count = slow_call_count();
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            samples.push(millis(session.client.completion(
                &uri,
                member.0,
                character,
                "slow occupancy",
            )));
        }
        samples
    };

    session.finish();

    let queue = Stats::from_samples(&queue_samples);
    let running = Stats::from_samples(&running_samples);
    let occupancy = Stats::from_samples(&occupancy_samples);
    check_p95(failures, "cancel_queued_request", &queue, CANCEL_P95_MS);
    check_p95(failures, "cancel_running_response", &running, CANCEL_P95_MS);
    let occupancy_floor = occupancy.p50_ms >= OCCUPANCY_P50_FLOOR_MS;
    println!(
        "threshold single_ts_call_occupancy_p50: {:.3} ms >= {OCCUPANCY_P50_FLOOR_MS:.3} ms -> {occupancy_floor}",
        occupancy.p50_ms
    );
    if !occupancy_floor {
        failures.push(format!(
            "single_ts_call_occupancy_p50: {:.3} ms < {OCCUPANCY_P50_FLOOR_MS:.3} ms; \
             the synthetic delay was not observed",
            occupancy.p50_ms
        ));
    }

    json!({
        "transport": "in-process run_with_host",
        "backend": "test SlowBackend; 400 ms completion delay; no engine and no Node process",
        "cancel_queued_request": queue.to_json(),
        "cancel_running_response": running.to_json(),
        "single_ts_call_occupancy": occupancy.to_json(),
        "backend_completion_calls": backend_completion_calls,
        "rounds": sample_count(),
        "occupancy_calls": slow_call_count(),
        "scope": "scheduler cancellation/queue/occupancy only; this is not a native engine cancel-latency claim",
    })
}

fn assert_cancelled(response: &Value, case: &str, round: usize) {
    let code = response["error"]["code"].as_i64();
    assert_eq!(
        code,
        Some(-32800),
        "a cancelled {case} request must answer REQUEST_CANCELLED: round {round} {response}"
    );
}

fn p95(summary: &Value) -> f64 {
    summary["p95_ms"].as_f64().unwrap_or(f64::INFINITY)
}

fn check_regression(
    failures: &mut Vec<String>,
    name: &str,
    after: f64,
    baseline: f64,
    allowed_fraction: f64,
) {
    // Sub-millisecond p95 values are below the timer's practical resolution on
    // a loaded desktop. Keep the absolute latency targets strict, but allow a
    // 0.5 ms absolute noise floor before treating a relative change as a
    // regression. Larger metrics still use the plain percentage limit.
    let noise_floor = if baseline < 1.0 { 0.5 } else { 0.0 };
    let limit = baseline.mul_add(1.0 + allowed_fraction, noise_floor);
    let pass = after <= limit;
    println!(
        "regression {name}: after {after:.3} <= baseline {baseline:.3} * {:.2} + floor {noise_floor:.3} = {limit:.3} -> {pass}",
        1.0 + allowed_fraction
    );
    if !pass {
        failures.push(format!(
            "{name}: after {after:.3} > baseline {baseline:.3} * {:.2} + floor {noise_floor:.3}",
            1.0 + allowed_fraction
        ));
    }
}

#[test]
#[allow(
    clippy::assertions_on_constants,
    reason = "the release profile is a constant prerequisite of the baseline"
)]
#[ignore = "P7 native perf gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn q2_performance_benchmark() {
    let Some(gate) = NativeGate::require("G-P5-PERF-01") else {
        return;
    };
    assert!(
        !cfg!(debug_assertions),
        "G-P5-PERF-01 is a release baseline: run `cargo test -p wxml-lsp --release --test performance_bench -- --ignored`; \
         a debug build cannot be compared with the frozen p95 budgets"
    );
    let require_clean = std::env::var("WXML_PERF_REQUIRE_CLEAN").as_deref() == Ok("1");
    let baseline_only = std::env::var("WXML_PERF_BASELINE_ONLY").as_deref() == Ok("1");
    let head = git_output(&["rev-parse", "HEAD"]);
    let dirty = git_output(&["status", "--porcelain"]);
    if require_clean {
        assert!(
            dirty.is_empty(),
            "performance gate requires a clean worktree; dirty:\n{dirty}"
        );
    }
    write_fixture(&gate);
    let label = label();
    let started = Instant::now();
    let load_start = loadavg();
    let threads_before = parent_threads();

    let mut failures = Vec::new();
    let real = real_engine_segment(&gate, &mut failures);
    println!("real_engine: {real}");
    let scheduler = scheduler_segment(&gate, &mut failures);
    println!("scheduler: {scheduler}");

    let scenarios = json!({
        "static_completion": real["metrics"]["static_completion"],
        "semantic_completion": real["metrics"]["semantic_completion"],
        "semantic_hover": real["metrics"]["semantic_hover"],
        "diagnostics_stop": real["metrics"]["diagnostics_stop"],
        "cancel_queued_request": scheduler["cancel_queued_request"],
        "cancel_running_response": scheduler["cancel_running_response"],
        "single_ts_call_occupancy": scheduler["single_ts_call_occupancy"],
    });

    let baseline_value = std::env::var_os("WXML_PERF_BASELINE").and_then(|path| {
        std::fs::read_to_string(&path).ok().map(|text| {
            let parsed: Value = serde_json::from_str(&text).expect("baseline performance JSON");
            (PathBuf::from(path), parsed)
        })
    });
    if let Some((_baseline_path, baseline)) = baseline_value.as_ref() {
        for name in [
            "static_completion",
            "semantic_completion",
            "semantic_hover",
            "diagnostics_stop",
            "cancel_queued_request",
        ] {
            let Some(before) = baseline["scenarios"][name]["p95_ms"].as_f64() else {
                failures.push(format!("baseline {name} is missing"));
                continue;
            };
            check_regression(&mut failures, name, p95(&scenarios[name]), before, 0.10);
        }
        if let Some(before) = baseline["resources"]["rss_kib_after"].as_u64() {
            check_regression(
                &mut failures,
                "parent_rss_kib",
                real["resources"]["parent_rss_kib_end"]
                    .as_u64()
                    .unwrap_or(0) as f64,
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
        "harness": "p7-native-retarget-perf",
        "gate": "G-P5-PERF-01",
        "baseline_only": baseline_only,
        "git_head": head,
        "git_dirty": dirty,
        "generated_at_unix_s": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        "engine": engine_identity(),
        "environment": {
            "transport": "--stdio --backend native (real engine) + in-process scheduler test double",
            "profile": "release (debug is rejected by the gate)",
            "samples_per_metric": sample_count(),
            "static_outer_iterations": sample_count(),
            "static_independent_calls": sample_count().saturating_mul(CALLS_PER_OUTER_ITERATION),
            "rustc": rustc_version(),
            "target_dir": std::env::var("CARGO_TARGET_DIR").unwrap_or_default(),
        },
        "machine": {
            "os": os_pretty_name(),
            "kernel": kernel_release(),
            "cpu": cpu_model(),
            "cpu_count": cpu_count(),
            "mem_total_kib": mem_total_kib(),
            "loadavg_start": load_start,
            "loadavg_end": loadavg(),
            "concurrency": "single benchmark process, no other benchmark running",
        },
        "baseline": baseline_value.as_ref().map(|(path, _)| path.display().to_string()),
        "scenarios": scenarios,
        "real_engine": real,
        "scheduler": scheduler,
        "resources": {
            "rss_kib_before": real["resources"]["parent_rss_kib_start"],
            "rss_kib_after": real["resources"]["parent_rss_kib_end"],
            "threads_before": threads_before,
            "threads_after": parent_threads(),
            "wall_time_ms": round(millis(started.elapsed())),
        },
        "thresholds": {
            "static_completion_p95_ms": STATIC_COMPLETION_P95_MS,
            "semantic_completion_p95_ms": COMPLETION_P95_MS,
            "semantic_hover_p95_ms": HOVER_P95_MS,
            "diagnostics_stop_p95_ms": DIAGNOSTICS_P95_MS,
            "cancel_queued_request_p95_ms": CANCEL_P95_MS,
            "cancel_running_response_p95_ms": CANCEL_P95_MS,
            "single_ts_call_occupancy_p50_min_ms": OCCUPANCY_P50_FLOOR_MS,
            "rss_growth_percent_max": RSS_GROWTH_PERCENT_MAX,
            "p95_regression_fraction": 0.10,
            "parent_rss_regression_fraction": 0.15,
            "pass": failures.is_empty(),
            "failures": failures,
        },
        "notes": [
            "The old `#[ignore]` gate coordinate and test name are preserved; the Node boundary is replaced by `--stdio --backend native`.",
            "The deep native oracle stays in native_parity/native_engine_real.rs::native_q2_performance_benchmark.",
            "static_completion reports p95 across 10,000 independent calls (50 per outer iteration, 200 outer iterations).",
            "semantic_hover and semantic_completion are measured around the JSON-RPC call and include serialization and server queueing.",
            "diagnostics_stop is measured from didChange to the matching publishDiagnostics version; the native harness pins a 0 ms debounce, stricter than the old 120 ms gate.",
            "cancel/queue/occupancy run against an in-process 400 ms SlowBackend, exactly like the retired SlowTransport; they are scheduler measurements, not native engine cancel-latency claims.",
            "RSS is the summed VmRSS of the server tree sampled from /proc with a hard process cap.",
            "No Node process, Node shim or external sampler is used.",
        ],
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
