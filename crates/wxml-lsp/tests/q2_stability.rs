#![cfg(target_os = "linux")]
#![allow(
    dead_code,
    clippy::cast_precision_loss,
    clippy::expect_used,
    clippy::needless_pass_by_value,
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

const WXML: &str = "pages/index/index.wxml";
const IMAGE: &str = "pages/index/images/logo.png";
const ROUNDS: usize = 10;
const RSS_GROWTH_PERCENT_MAX: f64 = 15.0;
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

    let rss_growth = growth_percent(baseline_rss, final_rss);
    let mut failures = Vec::new();
    if baseline_rss == 0 {
        failures.push("round-1 RSS baseline is zero".to_owned());
    } else if rss_growth > RSS_GROWTH_PERCENT_MAX {
        failures.push(format!(
            "server tree RSS grew {rss_growth:.2}% across {ROUNDS} cycles (cap {RSS_GROWTH_PERCENT_MAX:.0}%)"
        ));
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
        },
        "rounds": rounds,
        "summary": {
            "baseline_rss_kib": baseline_rss,
            "final_rss_kib": final_rss,
            "rss_growth_percent": round(rss_growth),
            "engine_processes": final_engines.len(),
            "cpu_ticks": cpu_ticks,
            "cpu_seconds_at_100hz": round(u64_to_f64(cpu_ticks) / 100.0),
            "wall_ms": round(started.elapsed().as_secs_f64() * 1000.0),
        },
        "shutdown": {
            "server_pid": evidence.pid,
            "elapsed_ms": round(evidence.elapsed.as_secs_f64() * 1000.0),
            "detached_worker": detached,
            "sandbox_left": false,
            "processes_left": 0,
        },
        "thresholds": {
            "in_run_rss_growth_percent_max": RSS_GROWTH_PERCENT_MAX,
            "pass": failures.is_empty(),
            "failures": failures,
        },
        "blocked": [
            {
                "scenario": "typing burst plus in-flight cancel",
                "status": "BLOCKED",
                "reason": "the retarget shell does not duplicate the old 10-edit burst and immediate cancellation; the deep native oracle covers update cadence and the scheduler gate covers cancellation separately",
            },
            {
                "scenario": "resource delete/restore and watched files",
                "status": "BLOCKED",
                "reason": "no native gate currently asserts the missing-resource diagnostic round-trip; this is a named follow-up, not a relaxed threshold",
            },
            {
                "scenario": "app.json overlay removal/restore",
                "status": "BLOCKED",
                "reason": "the old gate was the only hover-null assertion for a removed page; native ownerless-hover is proven by G-P5-E2E-09 but the app overlay combination is unproven",
            },
            {
                "scenario": "40-request bounded queue burst",
                "status": "BLOCKED",
                "reason": "the deep native oracle keeps 16 serial hovers; a concurrent 40-request burst is not re-proven here",
            },
            {
                "scenario": "external RSS baseline ratio",
                "status": "BLOCKED",
                "reason": "the retired baseline mixes Rust parent RSS with a Node child; server-tree RSS is not comparable, so only the in-run 15% cap is asserted",
            },
        ],
        "notes": [
            "The old `#[ignore]` gate coordinate and test name are preserved; the Node boundary is replaced by `--stdio --backend native`.",
            "The deep native oracle stays in native_parity/native_engine_real.rs::native_ten_round_lifecycle_stability (12 cycles, unsaved overlay replay, settle tail).",
            "One server process performs 10 cycles; each cycle restarts the engine, not the server. This matches the retired gate shape.",
            "RSS is the summed VmRSS of the server tree sampled from /proc with a hard process cap; the baseline is round 1 to exclude the one-time cold working set.",
            "Strict assertions are always on; the retired WXML_STABILITY_LENIENT mode is not supported.",
            "No Node process, Node shim or external sampler is used.",
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
