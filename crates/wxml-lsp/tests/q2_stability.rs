#![cfg(target_os = "linux")]
#![allow(
    dead_code,
    clippy::cast_precision_loss,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

//! P7 native-only shell for `G-P5-STABILITY-01`.
//!
//! The frozen gate name and `#[ignore]` coordinate are kept. The old Node
//! child is replaced by the real `wxml-lsp --stdio --backend native` package
//! from `native_parity/native_package.rs`:
//!
//! - one fresh server process;
//! - 10 open/edit/query cycles;
//! - one engine `SIGKILL` per cycle, a fresh engine PID, and a replayed query;
//! - per-cycle server-tree RSS, thread and process accounting with the frozen
//!   15% growth cap;
//! - a close/reopen of the template every cycle;
//! - clean `shutdown`/`exit` with no detached worker, leaked sandbox or child.
//!
//! After the lifecycle rounds, four previously BLOCKED scenarios run as
//! independent assertion blocks inside the same ignored test:
//!
//! 1. a ten-edit typing burst plus `$/cancelRequest` of an in-flight
//!    completion, with clean final diagnostics;
//! 2. an image delete/restore through bounded `didChangeWatchedFiles`, with
//!    the missing-resource diagnostic and its removal both observed;
//! 3. an unsaved `app.json` overlay that removes and restores the page, with
//!    hover empty during removal and typed after the close;
//! 4. forty in-flight hover requests answered without `server is busy`.
//!
//! The fifth scenario, an external server-tree RSS baseline ratio, has a
//! fail-closed seam (`WXML_NATIVE_STABILITY_BASELINE`) but remains explicitly
//! BLOCKED in the report until an independent native baseline artifact exists.
//! The retired `WXML_STABILITY_RSS_BASELINE` name is deliberately not reused:
//! that artifact mixes Rust parent RSS with a Node child and is not comparable
//! to server-tree RSS.
//!
//! All five assertions stay inside this one `#[ignore]` function because
//! `native_parity/native_gates.rs` freezes the workspace ignored-gate count at
//! 23; adding a new ignored test would be gate drift, not coverage.
//!
//! The deep native oracle stays in
//! `native_parity/native_engine_real.rs::native_ten_round_lifecycle_stability`
//! (12 cycles, unsaved script overlay replay, settle-RSS tail); this shell does
//! not duplicate it.
//!
//! ```bash
//! WXML_TEST_NATIVE=1 WXML_REQUIRE_REAL_ENGINE=1 \
//! TSGO_BIN=/tmp/opencode/tsgo-native-pkg/engine/lib/tsc \
//! TSGO_ASSET_ROOT=/tmp/opencode/tsgo-native-pkg/engine \
//! TSGO_TYPINGS_ROOT=/tmp/opencode/tsgo-native-pkg/typings/miniprogram-api-typings \
//! cargo test -p wxml-lsp --release --test q2_stability -- \
//!   --ignored --test-threads=1
//! ```
//!
//! Strict semantics are the only mode: the retired `WXML_STABILITY_LENIENT`
//! escape hatch is not supported. A dirty worktree fails by default; set
//! `WXML_STABILITY_ALLOW_DIRTY=1` only for a local evidence run. Raw rounds are
//! written to `WXML_STABILITY_OUT` (default
//! `/tmp/opencode/q2-bench/stability-<label>.json`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wxml_backend_tsgo::config::{AST_PROTOCOL_VERSION, ENGINE_VERSION, PinnedEngine};

#[path = "native_parity/native_gate_env.rs"]
mod native_gate_env;
#[path = "native_parity/native_lsp.rs"]
mod native_lsp;
#[path = "native_parity/native_package.rs"]
mod native_package;
mod q2_stability_support;

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

use native_lsp::Session;
use native_measure::{
    engine_children, git_head, growth_percent, kill_process, sample_tree, u64_to_f64, wait_gone,
};
use native_package::NativeGate;
use q2_stability_support::RawSession;

const WXML: &str = "pages/index/index.wxml";
const IMAGE: &str = "pages/index/images/logo.png";
const ROUNDS: usize = 10;
const RSS_GROWTH_PERCENT_MAX: f64 = 15.0;
const STEADY_RSS_RATIO_MAX: f64 = 1.15;
const BURST_EDITS: usize = 10;
const QUEUE_BURST_REQUESTS: usize = 40;
const SCENARIO_WAIT: Duration = Duration::from_secs(5);
const NAME_RETRY_ATTEMPTS: usize = 5;

fn page_text(comment: &str) -> String {
    format!("<view><image src=\"images/logo.png\" />{{{{ user.name }}}}</view>{comment}")
}

fn write_fixture(gate: &NativeGate) {
    gate.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    gate.write(WXML, &page_text(""));
    gate.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    gate.write(IMAGE, "png");
    gate.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn output_path(label: &str) -> PathBuf {
    std::env::var_os("WXML_STABILITY_OUT").map_or_else(
        || PathBuf::from(format!("/tmp/opencode/q2-bench/stability-{label}.json")),
        PathBuf::from,
    )
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

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn engine_identity() -> Value {
    let pin = PinnedEngine::pinned();
    json!({
        "version": ENGINE_VERSION,
        "git_head": pin.git_head(),
        "ast_protocol": AST_PROTOCOL_VERSION,
        "executable_sha256": pin.executable_sha256(),
    })
}

/// Retries the post-restart hover until it observes the disk type.
///
/// The first request after `SIGKILL` may legitimately arrive before the engine
/// respawns. The gate records that class and requires the replay, not a
/// zero-latency first answer.
fn hover_until_string(
    session: &mut Session,
    uri: &str,
    position: (u32, u32),
) -> (Value, Vec<String>) {
    let mut classes = Vec::with_capacity(NAME_RETRY_ATTEMPTS);
    let mut last = Value::Null;
    for _ in 0..NAME_RETRY_ATTEMPTS {
        last = session.hover(uri, position);
        let class = if last.get("error").is_some() {
            "error".to_owned()
        } else if native_lsp::hover_is_empty(&last) {
            "empty".to_owned()
        } else if last["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("string"))
        {
            "string".to_owned()
        } else {
            "other".to_owned()
        };
        classes.push(class.clone());
        if class == "string" {
            return (last, classes);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    (last, classes)
}

fn hover_text(response: &Value) -> Option<&str> {
    response["result"]["contents"]["value"].as_str()
}

fn hover_is_string(response: &Value) -> bool {
    hover_text(response).is_some_and(|text| text.contains("string"))
}

/// Polls hover until `predicate` accepts the response or the bound expires.
///
/// The protocol loop is ordered, so the first request after a notification
/// normally observes it; polling tolerates the host-update scheduling without
/// turning a missing removal into an unbounded wait.
fn hover_until(
    session: &mut RawSession,
    uri: &str,
    position: (u32, u32),
    timeout: Duration,
    context: &str,
    mut predicate: impl FnMut(&Value) -> bool,
) -> (Value, usize) {
    let deadline = Instant::now() + timeout;
    let mut attempts = 0_usize;
    loop {
        attempts += 1;
        let response = session.hover(uri, position);
        if predicate(&response) {
            return (response, attempts);
        }
        assert!(
            Instant::now() < deadline,
            "{context}: hover did not reach the expected state after {attempts} attempt(s): {}",
            session.describe(&response)
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Asserts resource/process accounting, then shuts one scenario session down.
fn scenario_shutdown(
    gate: &NativeGate,
    session: RawSession,
    expected_engines: usize,
    context: &str,
) -> Value {
    let pid = session.pid();
    let descendants = session.descendant_pids();
    let engines = engine_children(pid, gate.package_root());
    assert_eq!(
        engines.len(),
        expected_engines,
        "{context}: expected {expected_engines} engine process(es), found {engines:?}"
    );
    let sample = sample_tree(pid);
    assert!(
        sample.rss_kib > 0,
        "{context}: server tree RSS must be non-zero"
    );
    let evidence = session.shutdown_with_evidence();
    let detached = evidence.stderr.contains("detaching it");
    assert!(
        !detached,
        "{context}: server detached a worker on shutdown; stderr:\n{}",
        evidence.stderr
    );
    wait_gone(&[pid], context);
    wait_gone(&descendants, context);
    gate.assert_no_native_sandbox();
    gate.tripwire().assert_absent();
    json!({
        "server_pid": evidence.pid,
        "engine_processes": engines.len(),
        "server_tree_rss_kib": sample.rss_kib,
        "shutdown_ms": round(evidence.elapsed.as_secs_f64() * 1000.0),
        "detached_worker": detached,
        "sandbox_left": false,
        "processes_left": 0,
    })
}

/// Scenario 1: ten unsaved edits in one burst, then cancel a completion.
fn run_typing_burst_and_cancel(gate: &NativeGate) -> Value {
    let uri = gate.uri(WXML);
    let text = page_text("");
    let position = native_lsp::lsp_position(&text, "name");
    let mut session = RawSession::start(gate);
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_eq!(
        diagnostics,
        json!([]),
        "typing burst: the initial page must be clean: {diagnostics}"
    );

    let mut version = 1_u64;
    for edit in 0..BURST_EDITS {
        version += 1;
        let update = page_text(&format!("\n<!-- burst {edit} v{version} -->"));
        session.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": update}],
            }),
        );
    }
    let id = session.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": position.0, "character": position.1},
        }),
    );
    session.notify("$/cancelRequest", json!({"id": id}));
    let response = session.response(id);
    let outcome = if let Some(error) = response.get("error") {
        assert_eq!(
            error["code"],
            json!(-32_800),
            "typing burst: an error answer must be REQUEST_CANCELLED: {}",
            session.describe(&response)
        );
        assert_eq!(
            error["message"],
            json!("request cancelled"),
            "typing burst: the cancellation message must be exact: {}",
            session.describe(&response)
        );
        "cancelled"
    } else {
        assert!(
            response.get("result").is_some(),
            "typing burst: completion must answer with a result or REQUEST_CANCELLED: {}",
            session.describe(&response)
        );
        "completed"
    };

    let final_message = session
        .wait_diagnostics_matching(&uri, SCENARIO_WAIT, |message| {
            message["params"]["version"]
                .as_u64()
                .is_some_and(|seen| seen >= version)
        })
        .unwrap_or_else(|| {
            panic!(
                "typing burst: no diagnostics for final version {version} within {SCENARIO_WAIT:?}; \
                 stderr:\n{}",
                session.stderr_text()
            )
        });
    let final_diagnostics = final_message["params"]["diagnostics"].clone();
    assert_eq!(
        final_diagnostics,
        json!([]),
        "typing burst: final version {version} must be clean: {final_diagnostics}"
    );
    let hover = session.hover(&uri, position);
    assert!(
        hover_is_string(&hover),
        "typing burst: the page lost its disk type after the burst: {}",
        session.describe(&hover)
    );

    let mut record = json!({
        "scenario": "typing burst plus in-flight cancel",
        "status": "IMPLEMENTED",
        "typing_edits": BURST_EDITS,
        "final_version": version,
        "cancel_request_id": id,
        "cancel_outcome": outcome,
        "final_diagnostics": final_diagnostics,
        "final_hover": "string",
    });
    record["shutdown"] = scenario_shutdown(gate, session, 1, "typing burst");
    record
}

/// Scenario 2: delete/restore the page image through watched files only.
fn run_resource_watched_round_trip(gate: &NativeGate) -> Value {
    let uri = gate.uri(WXML);
    let image_uri = gate.uri(IMAGE);
    let image_path = gate.project_file(IMAGE);
    let text = page_text("");
    let position = native_lsp::lsp_position(&text, "name");
    let mut session = RawSession::start(gate);
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_eq!(
        diagnostics,
        json!([]),
        "resource delete/restore: the initial page must be clean: {diagnostics}"
    );

    std::fs::remove_file(&image_path)
        .unwrap_or_else(|error| panic!("delete {}: {error}", image_path.display()));
    session.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": image_uri, "type": 3}]}),
    );
    let missing = session
        .wait_diagnostics_matching(&uri, SCENARIO_WAIT, |message| {
            message["params"]["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| {
                    diagnostics.iter().any(|diagnostic| {
                        diagnostic["message"]
                            .as_str()
                            .is_some_and(|text| text.contains("does not exist"))
                    })
                })
        })
        .unwrap_or_else(|| {
            panic!(
                "resource delete/restore: the deleted image did not publish a missing-resource \
                 diagnostic within {SCENARIO_WAIT:?}; stderr:\n{}",
                session.stderr_text()
            )
        });
    assert_eq!(
        missing["params"]["version"],
        json!(1),
        "resource delete/restore: watched files must not need a text version bump: {missing}"
    );
    let missing_diagnostics = missing["params"]["diagnostics"].clone();
    let missing_values = missing_diagnostics.as_array().expect("diagnostics array");
    assert_eq!(
        missing_values.len(),
        1,
        "resource delete/restore: exactly one missing-resource diagnostic: {missing_diagnostics}"
    );
    assert_eq!(
        missing_values[0]["code"],
        json!("style.missing-resource"),
        "resource delete/restore: wrong diagnostic code: {}",
        missing_values[0]
    );
    assert_eq!(
        missing_values[0]["severity"],
        json!(2),
        "resource delete/restore: the missing resource must be a warning: {}",
        missing_values[0]
    );
    assert_eq!(
        missing_values[0]["message"],
        json!("local Image target `images/logo.png` does not exist"),
        "resource delete/restore: the exact missing-resource message must be published"
    );

    gate.write(IMAGE, "png");
    session.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": image_uri, "type": 1}]}),
    );
    let restored = session
        .wait_diagnostics_matching(&uri, SCENARIO_WAIT, |message| {
            message["params"]["diagnostics"]
                .as_array()
                .is_some_and(Vec::is_empty)
        })
        .unwrap_or_else(|| {
            panic!(
                "resource delete/restore: the restored image did not publish clean diagnostics \
                 within {SCENARIO_WAIT:?}; stderr:\n{}",
                session.stderr_text()
            )
        });
    assert_eq!(
        restored["params"]["version"],
        json!(1),
        "resource delete/restore: the restore must stay on the open version: {restored}"
    );
    assert_eq!(
        restored["params"]["diagnostics"],
        json!([]),
        "resource delete/restore: the missing-resource diagnostic must clear: {restored}"
    );
    let hover = session.hover(&uri, position);
    assert!(
        hover_is_string(&hover),
        "resource delete/restore: the page lost its disk type after restore: {}",
        session.describe(&hover)
    );

    let mut record = json!({
        "scenario": "resource delete/restore and watched files",
        "status": "IMPLEMENTED",
        "watched_change_types": [3, 1],
        "missing_diagnostic": missing_values[0].clone(),
        "missing_diagnostics_version": 1,
        "restored_diagnostics": [],
        "restored_diagnostics_version": 1,
    });
    record["shutdown"] = scenario_shutdown(gate, session, 1, "resource delete/restore");
    record
}

/// Scenario 3: remove and restore the page through an unsaved app.json overlay.
fn run_app_overlay_round_trip(gate: &NativeGate) -> Value {
    let uri = gate.uri(WXML);
    let app_uri = gate.uri("app.json");
    let text = page_text("");
    let position = native_lsp::lsp_position(&text, "name");
    let mut session = RawSession::start(gate);
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_eq!(
        diagnostics,
        json!([]),
        "app overlay: the initial page must be clean: {diagnostics}"
    );
    let initial = session.hover(&uri, position);
    assert!(
        hover_is_string(&initial),
        "app overlay: the initial hover must be string: {}",
        session.describe(&initial)
    );

    session.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": app_uri,
                "languageId": "json",
                "version": 1,
                "text": r#"{"pages":[]}"#,
            }
        }),
    );
    let (removed, removed_attempts) = hover_until(
        &mut session,
        &uri,
        position,
        SCENARIO_WAIT,
        "app overlay removal",
        native_lsp::hover_is_empty,
    );
    assert!(
        native_lsp::hover_is_empty(&removed),
        "app overlay: a removed page must not answer with types: {}",
        session.describe(&removed)
    );

    session.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": app_uri}}),
    );
    let (restored, restored_attempts) = hover_until(
        &mut session,
        &uri,
        position,
        SCENARIO_WAIT,
        "app overlay restore",
        hover_is_string,
    );
    assert!(
        hover_is_string(&restored),
        "app overlay: the restored page lost its disk type: {}",
        session.describe(&restored)
    );

    let mut record = json!({
        "scenario": "app.json overlay removal/restore",
        "status": "IMPLEMENTED",
        "removed_hover": "empty",
        "removed_hover_attempts": removed_attempts,
        "restored_hover": "string",
        "restored_hover_attempts": restored_attempts,
        "barrier": "the hover request is ordered after didOpen/didClose on the same stdio stream",
    });
    record["shutdown"] = scenario_shutdown(gate, session, 1, "app overlay");
    record
}

/// Scenario 4: forty hovers in flight, then every response collected.
fn run_bounded_queue_burst(gate: &NativeGate) -> Value {
    let uri = gate.uri(WXML);
    let text = page_text("");
    let position = native_lsp::lsp_position(&text, "name");
    let mut session = RawSession::start(gate);
    let diagnostics = session.open_wxml(&uri, &text, 1);
    assert_eq!(
        diagnostics,
        json!([]),
        "queue burst: the initial page must be clean: {diagnostics}"
    );

    let mut ids = Vec::with_capacity(QUEUE_BURST_REQUESTS);
    for _ in 0..QUEUE_BURST_REQUESTS {
        ids.push(session.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        ));
    }
    let mut answered = 0_usize;
    let mut null_results = 0_usize;
    for id in &ids {
        let response = session.response(*id);
        assert!(
            response.get("error").is_none(),
            "queue burst: request {id} was rejected instead of queued: {}",
            session.describe(&response)
        );
        assert!(
            response.get("result").is_some(),
            "queue burst: request {id} answered without a result: {}",
            session.describe(&response)
        );
        answered += 1;
        if response["result"].is_null() {
            null_results += 1;
        }
    }
    assert_eq!(
        answered, QUEUE_BURST_REQUESTS,
        "queue burst: every request must be answered"
    );
    let after = session.hover(&uri, position);
    assert!(
        hover_is_string(&after),
        "queue burst: the page lost its disk type after the burst: {}",
        session.describe(&after)
    );

    let mut record = json!({
        "scenario": "40-request bounded queue burst",
        "status": "IMPLEMENTED",
        "requests": QUEUE_BURST_REQUESTS,
        "answered": answered,
        "server_is_busy": 0,
        "null_results": null_results,
        "final_hover": "string",
    });
    record["shutdown"] = scenario_shutdown(gate, session, 1, "queue burst");
    record
}

/// Scenario 5: compare final server-tree RSS with an external native baseline.
///
/// The retired baseline mixed the Rust parent process with a Node child, so it
/// is not comparable with server-tree RSS. This seam accepts a native baseline
/// artifact, but no such artifact is committed; without one the report keeps
/// the scenario explicitly BLOCKED instead of substituting the in-run cap.
fn external_rss_baseline(final_rss: u64) -> (Value, Option<String>) {
    // The retired `WXML_STABILITY_RSS_BASELINE` artifact mixed the Rust parent
    // with a Node child. Reusing that name would silently compare incomparable
    // numbers, so only the native server-tree report is accepted.
    let path = std::env::var_os("WXML_NATIVE_STABILITY_BASELINE");
    let required = std::env::var("WXML_NATIVE_STABILITY_REQUIRE_BASELINE").as_deref() == Ok("1")
        || std::env::var("WXML_STABILITY_REQUIRE_BASELINE").as_deref() == Ok("1");
    let Some(path) = path else {
        let failure = required.then(|| {
            "WXML_NATIVE_STABILITY_REQUIRE_BASELINE=1 but no native server-tree RSS baseline is \
             available; set WXML_NATIVE_STABILITY_BASELINE to a native stability report"
                .to_owned()
        });
        return (
            json!({
                "scenario": "external RSS baseline ratio",
                "status": if required { "FAILED" } else { "BLOCKED" },
                "final_rss_kib": final_rss,
                "baseline_final_rss_kib": Value::Null,
                "ratio": Value::Null,
                "ratio_max": STEADY_RSS_RATIO_MAX,
                "reason": "the retired baseline mixes Rust parent RSS with a Node child; server-tree RSS is not comparable, and no independent native baseline artifact is committed",
                "missing_seam": "an external native stability report with summary.rss_metric=server_tree_rss_kib and summary.server_tree_rss_kib from a dedicated run",
                "seam": "WXML_NATIVE_STABILITY_BASELINE=<json>; WXML_NATIVE_STABILITY_REQUIRE_BASELINE=1 (legacy alias: WXML_STABILITY_REQUIRE_BASELINE) makes absence fatal",
            }),
            failure,
        );
    };
    let path = PathBuf::from(path);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read RSS baseline {}: {error}", path.display()));
    let baseline: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse RSS baseline {}: {error}", path.display()));
    assert_eq!(
        baseline["summary"]["rss_metric"],
        json!("server_tree_rss_kib"),
        "RSS baseline {} is not a native server-tree report; refusing to compare \
         incomparable metrics",
        path.display()
    );
    let baseline_rss = baseline["summary"]["server_tree_rss_kib"]
        .as_u64()
        .or_else(|| baseline["summary"]["final_rss_kib"].as_u64())
        .unwrap_or_else(|| {
            panic!(
                "RSS baseline {} has no summary.server_tree_rss_kib or summary.final_rss_kib",
                path.display()
            )
        });
    assert!(
        baseline_rss > 0,
        "RSS baseline {} is zero; refusing a zero-denominator ratio",
        path.display()
    );
    let ratio = u64_to_f64(final_rss) / u64_to_f64(baseline_rss);
    let failure = (ratio > STEADY_RSS_RATIO_MAX).then(|| {
        format!(
            "server-tree RSS {final_rss} KiB is {ratio:.3}x external baseline {baseline_rss} KiB \
             (cap {STEADY_RSS_RATIO_MAX:.2}x)"
        )
    });
    (
        json!({
            "scenario": "external RSS baseline ratio",
            "status": if failure.is_some() { "FAILED" } else { "IMPLEMENTED" },
            "baseline_file": path.display().to_string(),
            "baseline_final_rss_kib": baseline_rss,
            "final_rss_kib": final_rss,
            "ratio": round(ratio),
            "ratio_max": STEADY_RSS_RATIO_MAX,
            "reason": Value::Null,
            "missing_seam": Value::Null,
        }),
        failure,
    )
}

/// Runs scenarios 1-4, each on its own server and engine, and returns evidence.
fn run_stability_scenarios(gate: &NativeGate) -> Value {
    let typing = run_typing_burst_and_cancel(gate);
    let resource = run_resource_watched_round_trip(gate);
    let overlay = run_app_overlay_round_trip(gate);
    let queue = run_bounded_queue_burst(gate);
    json!([typing, resource, overlay, queue])
}

#[test]
#[ignore = "P7 native stability gate: set WXML_TEST_NATIVE=1 and WXML_REQUIRE_REAL_ENGINE=1 with a native package"]
fn ten_round_lifecycle_stability() {
    let Some(gate) = NativeGate::require("G-P5-STABILITY-01") else {
        return;
    };
    write_fixture(&gate);
    let allow_dirty = std::env::var("WXML_STABILITY_ALLOW_DIRTY").as_deref() == Ok("1");
    let dirty = git_output(&["status", "--porcelain"]);
    assert!(
        allow_dirty || dirty.is_empty(),
        "strict stability gate requires a clean worktree; dirty:\n{dirty}\n\
         set WXML_STABILITY_ALLOW_DIRTY=1 only for a local evidence run"
    );

    let uri = gate.uri(WXML);
    let text = page_text("");
    let name_position = native_lsp::lsp_position(&text, "name");

    let started = Instant::now();
    let mut session = Session::start(&gate);
    let mut version = 1_u64;
    let diagnostics = session.open_wxml(&uri, &text, version);
    assert_eq!(
        diagnostics,
        json!([]),
        "the stability page must start clean: {diagnostics}"
    );
    let initial = session.hover(&uri, name_position);
    assert!(
        initial["result"]["contents"]["value"]
            .as_str()
            .is_some_and(|text| text.contains("string")),
        "the initial disk hover must be string: {}",
        session.describe(&initial)
    );

    let cpu_start = sample_tree(session.pid()).cpu_ticks;
    let mut rounds = Vec::with_capacity(ROUNDS);
    let mut baseline_rss = 0_u64;
    let mut final_rss = 0_u64;
    for round_index in 0..ROUNDS {
        let round_started = Instant::now();
        version += 1;
        let update = page_text(&format!("\n<!-- cycle {round_index} update -->"));
        let diagnostics = session.change_wxml(&uri, &update, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "round {round_index}: update must stay clean: {diagnostics}"
        );
        let disk = session.hover(&uri, name_position);
        assert!(
            disk["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains("string")),
            "round {round_index}: disk hover must stay string: {}",
            session.describe(&disk)
        );

        let engines = engine_children(session.pid(), gate.package_root());
        assert_eq!(
            engines.len(),
            1,
            "round {round_index}: expected one engine before restart: {engines:?}"
        );
        let old_engine = engines[0];
        kill_process(old_engine);
        wait_gone(&[old_engine], "killed engine");

        let (replay, first_request_classes) = hover_until_string(&mut session, &uri, name_position);
        assert!(
            replay["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains("string")),
            "round {round_index}: hover did not recover after engine restart \
             (observed {first_request_classes:?}): {}",
            session.describe(&replay)
        );
        let new_engines = engine_children(session.pid(), gate.package_root());
        assert_eq!(
            new_engines.len(),
            1,
            "round {round_index}: restart did not leave one live engine: {new_engines:?}"
        );
        assert_ne!(
            new_engines[0], old_engine,
            "round {round_index}: restart reused the killed engine process"
        );

        // Close and reopen the template: no old revision may survive the close.
        session.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
        version += 1;
        let reopened = page_text(&format!("\n<!-- cycle {round_index} reopened -->"));
        let diagnostics = session.open_wxml(&uri, &reopened, version);
        assert_eq!(
            diagnostics,
            json!([]),
            "round {round_index}: a reopened template must start clean: {diagnostics}"
        );
        let reopened_hover = session.hover(&uri, name_position);
        assert!(
            reopened_hover["result"]["contents"]["value"]
                .as_str()
                .is_some_and(|text| text.contains("string")),
            "round {round_index}: a reopened template lost its disk type: {}",
            session.describe(&reopened_hover)
        );

        let sample = sample_tree(session.pid());
        assert!(
            sample.rss_kib > 0,
            "round {round_index}: server tree RSS must be non-zero"
        );
        if round_index == 1 {
            baseline_rss = sample.rss_kib;
        }
        final_rss = sample.rss_kib;
        let growth = growth_percent(baseline_rss, sample.rss_kib);
        rounds.push(json!({
            "round": round_index,
            "version": version,
            "restart_first_request_classes": first_request_classes,
            "rss_kib": sample.rss_kib,
            "rss_growth_from_round1_percent": round(growth),
            "threads": sample.threads,
            "processes": sample.pids.len(),
            "engine_pid": new_engines[0],
            "wall_ms": round(round_started.elapsed().as_secs_f64() * 1000.0),
        }));
        println!(
            "stability round {round_index}: v{version} rss={} KiB growth={growth:.2}% engine={} wall={:.1} ms",
            sample.rss_kib,
            new_engines[0],
            round_started.elapsed().as_secs_f64() * 1000.0
        );
    }

    let final_engines = engine_children(session.pid(), gate.package_root());
    assert_eq!(
        final_engines.len(),
        1,
        "the final cycle must keep exactly one engine: {final_engines:?}"
    );
    let final_sample = sample_tree(session.pid());
    assert!(
        final_sample.rss_kib > 0,
        "the final server tree RSS must be non-zero"
    );

    let pid = session.pid();
    let descendants = session.descendant_pids();
    let evidence = session.shutdown_with_evidence();
    let detached = evidence.stderr.contains("detaching it");
    assert!(
        !detached,
        "server detached a worker on shutdown; stderr:\n{}",
        evidence.stderr
    );
    wait_gone(&[pid], "server after stability shutdown");
    wait_gone(&descendants, "engine after stability shutdown");
    gate.assert_no_native_sandbox();
    gate.tripwire().assert_absent();

    // Scenarios 1-4 each start and stop their own real server and engine. The
    // lifecycle RSS measurement above is already closed, so these sessions
    // cannot change the frozen in-run baseline.
    let mut scenarios = run_stability_scenarios(&gate);
    gate.assert_no_native_sandbox();
    gate.tripwire().assert_absent();

    let rss_growth = growth_percent(baseline_rss, final_rss);
    let (external_baseline, external_failure) = external_rss_baseline(final_rss);
    if let Some(array) = scenarios.as_array_mut() {
        array.push(external_baseline.clone());
    }
    let scenario_values = scenarios.as_array().expect("scenario evidence array");
    let implemented = scenario_values
        .iter()
        .filter(|scenario| scenario["status"] == json!("IMPLEMENTED"))
        .count();
    let blocked = scenario_values
        .iter()
        .filter(|scenario| scenario["status"] == json!("BLOCKED"))
        .count();
    let failed = scenario_values
        .iter()
        .filter(|scenario| scenario["status"] == json!("FAILED"))
        .count();
    let blocked_scenarios = scenario_values
        .iter()
        .filter(|scenario| scenario["status"] == json!("BLOCKED"))
        .cloned()
        .collect::<Vec<Value>>();

    let mut failures = Vec::new();
    if baseline_rss == 0 {
        failures.push("round-1 RSS baseline is zero".to_owned());
    } else if rss_growth > RSS_GROWTH_PERCENT_MAX {
        failures.push(format!(
            "server tree RSS grew {rss_growth:.2}% across {ROUNDS} cycles (cap {RSS_GROWTH_PERCENT_MAX:.0}%)"
        ));
    }
    if let Some(failure) = external_failure {
        failures.push(failure);
    }
    let cpu_ticks = final_sample.cpu_ticks.saturating_sub(cpu_start);
    let report = json!({
        "label": std::env::var("WXML_STABILITY_LABEL").unwrap_or_else(|_| "run".to_owned()),
        "harness": "p7-native-retarget-stability",
        "gate": "G-P5-STABILITY-01",
        "git_head": git_head(&ledger::workspace_root()),
        "git_dirty": dirty,
        "strict_semantics": true,
        "allow_dirty": allow_dirty,
        "engine": engine_identity(),
        "environment": {
            "transport": "--stdio --backend native",
            "cycles": ROUNDS,
            "restart": "SIGKILL the engine child every cycle; the replay must observe the disk type and a new engine PID",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "diagnostics_debounce_ms": {"lifecycle": 0, "scenarios": 120},
            "scenario_sessions": 4,
        },
        "rounds": rounds,
        "summary": {
            "baseline_rss_kib": baseline_rss,
            "final_rss_kib": final_rss,
            "server_tree_rss_kib": final_rss,
            "rss_metric": "server_tree_rss_kib",
            "rss_growth_percent": round(rss_growth),
            "engine_processes": final_engines.len(),
            "cpu_ticks": cpu_ticks,
            "cpu_seconds_at_100hz": round(u64_to_f64(cpu_ticks) / 100.0),
            "wall_ms": round(started.elapsed().as_secs_f64() * 1000.0),
            "stability_scenarios": {
                "implemented": implemented,
                "blocked": blocked,
                "failed": failed,
            },
        },
        "shutdown": {
            "server_pid": evidence.pid,
            "elapsed_ms": round(evidence.elapsed.as_secs_f64() * 1000.0),
            "detached_worker": detached,
            "sandbox_left": false,
            "processes_left": 0,
        },
        "scenarios": scenarios,
        "external_rss_baseline": external_baseline,
        "thresholds": {
            "in_run_rss_growth_percent_max": RSS_GROWTH_PERCENT_MAX,
            "steady_rss_ratio_max": STEADY_RSS_RATIO_MAX,
            "pass": failures.is_empty(),
            "failures": failures,
        },
        "blocked": blocked_scenarios,
        "notes": [
            "The old `#[ignore]` gate coordinate and test name are preserved; the Node boundary is replaced by `--stdio --backend native`.",
            "The deep native oracle stays in native_parity/native_engine_real.rs::native_ten_round_lifecycle_stability (12 cycles, unsaved overlay replay, settle tail).",
            "One server process performs 10 cycles; each cycle restarts the engine, not the server. This matches the retired gate shape.",
            "RSS is the summed VmRSS of the server tree sampled from /proc with a hard process cap; the baseline is round 1 to exclude the one-time cold working set.",
            "Strict assertions are always on; the retired WXML_STABILITY_LENIENT mode is not supported.",
            "No Node process, Node shim or external sampler is used.",
            "Scenarios 1-4 run after the lifecycle measurement, each on a fresh server and engine; scenario 5 stays BLOCKED until an external native RSS baseline artifact exists.",
            "All five scenario assertions stay inside this one ignored test because native_parity/native_gates.rs freezes the workspace ignored-gate count at 23.",
        ],
    });
    let path = output_path(report["label"].as_str().unwrap_or("run"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("output dir");
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&report).expect("json")).expect("write");
    println!("stability report: {}", path.display());
    println!("{}", serde_json::to_string_pretty(&report).expect("json"));
    assert!(
        report["thresholds"]["pass"].as_bool() == Some(true),
        "stability thresholds failed: {}",
        report["thresholds"]["failures"]
    );
}
