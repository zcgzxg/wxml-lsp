#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

//! 10-round lifecycle stability gate: edits, cancellation, Node restart,
//! overlay replay, resource delete/restore, app removal and close/reopen.
//!
//! ```bash
//! (cd backends/typescript && npm ci && npm run build)
//! WXML_TEST_NODE=1 WXML_STABILITY_REQUIRE_CLEAN=1 WXML_STABILITY_LABEL=strict \
//! WXML_STABILITY_OUT=/tmp/opencode/q2-bench/stability-strict.json \
//!   CARGO_TARGET_DIR=/home/zxg/.cache/wxml-targets/full-performance-local \
//!   cargo test -p wxml-lsp --test q2_stability -- --ignored --nocapture
//! ```
//!
//! Strict recovery assertions are the default. `WXML_STABILITY_LENIENT=1` is
//! only for the W3 old-code comparison and disables the overlay/restart/page
//! assertions. `WXML_STABILITY_RSS_BASELINE=<json>` only selects an RSS
//! comparison file and never changes strictness.
//!
//! Raw round data is written to `WXML_STABILITY_OUT` (default
//! `/tmp/opencode/q2-bench/stability-<label>.json`). Generated evidence is
//! never committed.

mod common;
mod q2_support;

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig};
use wxml_lsp::project_host::ProjectState;

use q2_support::{Client, WXML, app_fixture};

const SCRIPT: &str = "pages/index/index.ts";
const IMAGE: &str = "pages/index/images/logo.png";

fn page_text(comment: &str) -> String {
    format!("<view><image src=\"images/logo.png\" />{{{{ user.name }}}}</view>{comment}")
}

fn name_position() -> u32 {
    let text = page_text("");
    u32::try_from(text.find("name").expect("name") + 2).expect("position")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root")
        .to_path_buf()
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

/// A process started by this test whose command line mentions the backend.
struct ChildInfo {
    pid: u32,
    rss_kib: u64,
    threads: usize,
}

fn node_children() -> Vec<ChildInfo> {
    let parent = std::process::id();
    let mut children = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return children;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let mut fields = stat[close + 1..].split_whitespace();
        let _state = fields.next();
        let Some(parent_pid) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if parent_pid != parent {
            continue;
        }
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&cmdline);
        if !cmdline.contains("dist/server.js") {
            continue;
        }
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let rss_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let threads = std::fs::read_dir(format!("/proc/{pid}/task"))
            .map_or(0, |entries| entries.filter_map(Result::ok).count());
        children.push(ChildInfo {
            pid,
            rss_kib,
            threads,
        });
    }
    children
}

fn kill_node_children() -> usize {
    let children = node_children();
    for child in &children {
        let _ = std::process::Command::new("kill")
            .args(["-9", &child.pid.to_string()])
            .status();
    }
    children.len()
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

fn wait_for_completion(client: &mut Client, uri: &str, position: (u32, u32)) -> Value {
    let id = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": position.0, "character": position.1},
        }),
    );
    client.response(id)
}

fn wait_for_hover(client: &mut Client, uri: &str, position: (u32, u32)) -> Value {
    let id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": position.0, "character": position.1},
        }),
    );
    client.response(id)
}

fn wait_for_hover_until(
    client: &mut Client,
    uri: &str,
    position: (u32, u32),
    mut predicate: impl FnMut(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = wait_for_hover(client, uri, position);
        if predicate(&value) || Instant::now() >= deadline {
            return value;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "stability gate: set WXML_TEST_NODE=1 and run with --ignored"]
fn ten_round_lifecycle_stability() {
    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the stability gate"
    );
    // Strict is the default and final-gate mode. Only the W3 old-code
    // comparison may set LENIENT=1; the RSS baseline path is independent.
    let lenient = std::env::var("WXML_STABILITY_LENIENT").as_deref() == Ok("1");
    let strict_semantics = !lenient;
    let repo = repo_root();
    let server_path = std::env::var_os("WXML_TEST_BACKEND_SERVER").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    let backend_dir = std::env::var_os("WXML_TEST_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);
    assert!(server_path.is_file(), "build the Node backend first");

    let project = app_fixture();
    project.write(IMAGE, "png");
    let (server_stream, client_stream) = UnixStream::pair().expect("socket pair");
    let server_read = server_stream.try_clone().expect("clone");
    let host = std::sync::Arc::new(ProjectState::new());
    let backend = std::sync::Arc::new(NodeBackend::new(
        NodeBackendConfig {
            server: Some(server_path),
            backend_dir: Some(backend_dir),
            timeout: Duration::from_secs(20),
            ..NodeBackendConfig::default()
        },
        host.clone(),
    ));
    let handle =
        thread::spawn(move || wxml_lsp::run_with_host(server_read, server_stream, backend, host));
    let mut client = Client::new(client_stream);
    client.initialize(
        &project,
        json!({"wxml": {"diagnostics": {"debounceMs": 120}}}),
    );

    let uri = project.uri(WXML);
    let script_uri = project.uri(SCRIPT);
    let image_uri = project.uri(IMAGE);
    let app_uri = project.uri("app.json");
    let mut version = 1_u64;
    client.open(&uri, &page_text(""), version);
    client.wait_diagnostics(&uri, version);

    let started = Instant::now();
    let mut rounds = Vec::new();
    let mut baseline_rss = 0_u64;
    let mut baseline_node_rss = 0_u64;
    for round in 0..10 {
        let round_started = Instant::now();
        // Typing burst plus cancellation of an in-flight backend completion.
        for _ in 0..10 {
            version += 1;
            client.change(
                &uri,
                &page_text(&format!("\n<!-- round {round} v{version} -->")),
                version,
            );
        }
        let id = client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": name_position()},
            }),
        );
        client.notify("$/cancelRequest", json!({"id": id}));
        let cancelled = client.response(id);
        assert!(
            cancelled.get("result").is_some() || cancelled.get("error").is_some(),
            "round {round}: cancellation answer: {cancelled}"
        );
        client.wait_diagnostics(&uri, version);

        // Unsaved script overlay: `name` becomes a number, which is visible in
        // hover. It must survive the backend restart below.
        version += 1;
        client.open_language(
            &script_uri,
            "typescript",
            &format!("Page({{ data: {{ user: {{ name: {round} }} }} }});"),
            version,
        );
        version += 1;
        client.change(
            &uri,
            &page_text(&format!("\n<!-- overlay {round} -->")),
            version,
        );
        client.wait_diagnostics(&uri, version);
        let overlay_hover =
            wait_for_hover_until(&mut client, &uri, (0, name_position()), |value| {
                !strict_semantics || value.to_string().contains("number")
            });
        if strict_semantics {
            assert!(
                overlay_hover.to_string().contains("number"),
                "round {round}: overlay not visible in hover: {overlay_hover}"
            );
        }

        // Kill the real Node process and recover through the epoch barrier.
        let killed = kill_node_children();
        assert!(killed >= 1, "round {round}: no Node child to kill");
        thread::sleep(Duration::from_millis(50));
        version += 1;
        client.change(
            &uri,
            &page_text(&format!("\n<!-- crash {round} -->")),
            version,
        );
        let _ = client.wait_diagnostics(&uri, version);
        version += 1;
        client.change(
            &uri,
            &page_text(&format!("\n<!-- recovered {round} -->")),
            version,
        );
        let recovered = if strict_semantics {
            client.wait_diagnostics_matching(&uri, |message| {
                message["params"]["version"]
                    .as_u64()
                    .is_some_and(|seen| seen >= version)
                    && message["params"]["diagnostics"]
                        .as_array()
                        .is_some_and(|diagnostics| {
                            diagnostics.iter().all(|diagnostic| {
                                !diagnostic["message"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_ascii_lowercase()
                                    .contains("unavailable")
                            })
                        })
            })
        } else {
            client.wait_diagnostics(&uri, version)
        };
        let recovered_hover =
            wait_for_hover_until(&mut client, &uri, (0, name_position()), |value| {
                !strict_semantics || value.to_string().contains("number")
            });
        if strict_semantics {
            assert!(
                recovered_hover.to_string().contains("number"),
                "round {round}: overlay not replayed after restart: {recovered} / {recovered_hover}"
            );
        }

        // Resource deletion and restoration.
        std::fs::remove_file(project.root().join(IMAGE)).expect("delete resource");
        client.watched(&image_uri, 3);
        let missing = client.wait_diagnostics_matching(&uri, |message| {
            message.to_string().contains("does not exist")
        });
        assert!(missing.to_string().contains("does not exist"));
        project.write(IMAGE, "png");
        client.watched(&image_uri, 1);
        let _ = client.wait_diagnostics_matching(&uri, |message| {
            !message.to_string().contains("does not exist")
        });

        // App removal and restore through an unsaved app.json overlay. A
        // removed page has no hover target; restoring it brings the overlay
        // type back.
        client.open_language(&app_uri, "json", r#"{"pages":[]}"#, 1);
        let _ = client.wait_next_diagnostics(&uri);
        let removed_hover =
            wait_for_hover_until(&mut client, &uri, (0, name_position()), |value| {
                value["result"].is_null() || !strict_semantics
            });
        assert!(
            removed_hover["result"].is_null() || !strict_semantics,
            "round {round}: removed page answered with types: {removed_hover}"
        );
        client.close(&app_uri);
        let _ = client.wait_next_diagnostics(&uri);
        let restored_hover =
            wait_for_hover_until(&mut client, &uri, (0, name_position()), |value| {
                !strict_semantics || value.to_string().contains("number")
            });
        if strict_semantics {
            assert!(
                restored_hover.to_string().contains("number"),
                "round {round}: restored page lost its overlay type: {restored_hover}"
            );
        }

        // Close/reopen the template; no state may survive the close.
        client.close(&uri);
        version += 1;
        client.open(&uri, &page_text(""), version);
        client.wait_diagnostics(&uri, version);

        let children = node_children();
        assert!(
            children.len() <= 1,
            "round {round}: leaked Node processes: {children:?}",
            children = children.iter().map(|child| child.pid).collect::<Vec<_>>()
        );
        let node_rss = children.first().map_or(0, |child| child.rss_kib);
        if round == 1 {
            baseline_rss = rss_kib();
            baseline_node_rss = node_rss;
        }
        rounds.push(json!({
            "round": round,
            "rss_kib": rss_kib(),
            "node_rss_kib": node_rss,
            "threads": threads(),
            "node_threads": children.first().map_or(0, |child| child.threads),
            "node_processes": children.len(),
            "wall_ms": round_started.elapsed().as_secs_f64() * 1000.0,
            "version": version,
        }));
        println!(
            "round {round}: rss={} KiB node_rss={} KiB threads={} took {:?}",
            rss_kib(),
            node_rss,
            threads(),
            round_started.elapsed()
        );
    }

    // Bounded queue: a final burst must fit without `server is busy`.
    let position = (0_u32, name_position());
    let mut ids = Vec::new();
    for _ in 0..40 {
        ids.push(client.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        ));
    }
    for id in ids {
        let response = client.response(id);
        assert!(
            response.get("error").is_none()
                || response["error"]["message"] != json!("server is busy"),
            "queue rejected a bounded hover burst: {response}"
        );
    }
    let _ = wait_for_completion(&mut client, &uri, position);
    let _ = wait_for_hover(&mut client, &uri, position);

    let final_rss = rss_kib();
    let final_children = node_children();
    let final_node_rss = final_children.first().map_or(0, |child| child.rss_kib);
    let rss_growth = if baseline_rss == 0 {
        0.0
    } else {
        (final_rss as f64 - baseline_rss as f64) / baseline_rss as f64 * 100.0
    };
    let node_rss_growth = if baseline_node_rss == 0 {
        0.0
    } else {
        (final_node_rss as f64 - baseline_node_rss as f64) / baseline_node_rss as f64 * 100.0
    };
    let dirty = git_output(&["status", "--porcelain"]);
    if strict_semantics || std::env::var("WXML_STABILITY_REQUIRE_CLEAN").as_deref() == Ok("1") {
        assert!(
            dirty.is_empty(),
            "strict stability gate requires a clean worktree; dirty:\n{dirty}"
        );
    }
    let mut failures = Vec::new();
    if rss_growth > 15.0 {
        failures.push(format!("rust rss grew {rss_growth:.2}% in 10 rounds"));
    }
    if node_rss_growth > 15.0 {
        failures.push(format!("node rss grew {node_rss_growth:.2}% in 10 rounds"));
    }
    let baseline_value = std::env::var_os("WXML_STABILITY_RSS_BASELINE").and_then(|path| {
        std::fs::read_to_string(&path).ok().map(|text| {
            let parsed: Value = serde_json::from_str(&text).expect("baseline stability JSON");
            (PathBuf::from(path), parsed)
        })
    });
    let mut baseline_ratios = json!(null);
    if let Some((_path, baseline)) = baseline_value.as_ref() {
        let baseline_final_rss = baseline["summary"]["final_rss_kib"].as_u64().unwrap_or(0);
        let baseline_final_node = baseline["summary"]["final_node_rss_kib"]
            .as_u64()
            .unwrap_or(0);
        let rust_ratio = if baseline_final_rss == 0 {
            1.0
        } else {
            final_rss as f64 / baseline_final_rss as f64
        };
        let node_ratio = if baseline_final_node == 0 {
            1.0
        } else {
            final_node_rss as f64 / baseline_final_node as f64
        };
        if rust_ratio > 1.15 {
            failures.push(format!(
                "rust steady RSS {final_rss} KiB is {rust_ratio:.3}x baseline {baseline_final_rss} KiB"
            ));
        }
        if node_ratio > 1.15 {
            failures.push(format!(
                "node steady RSS {final_node_rss} KiB is {node_ratio:.3}x baseline {baseline_final_node} KiB"
            ));
        }
        baseline_ratios = json!({
            "rust_final_ratio": rust_ratio,
            "node_final_ratio": node_ratio,
            "baseline_final_rss_kib": baseline_final_rss,
            "baseline_final_node_rss_kib": baseline_final_node,
        });
    } else if std::env::var("WXML_STABILITY_REQUIRE_BASELINE").as_deref() == Ok("1") {
        failures.push("WXML_STABILITY_RSS_BASELINE is required but missing/unreadable".to_owned());
    }
    let report = json!({
        "label": std::env::var("WXML_STABILITY_LABEL").unwrap_or_else(|_| "run".to_owned()),
        "harness": "q2-r3-stability",
        "git_head": git_output(&["rev-parse", "HEAD"]),
        "git_dirty": dirty,
        "strict_semantics": strict_semantics,
        "lenient": lenient,
        "rss_baseline": baseline_value.as_ref().map(|(path, _)| path.display().to_string()),
        "rounds": rounds,
        "summary": {
            "baseline_rss_kib": baseline_rss,
            "final_rss_kib": final_rss,
            "rss_growth_percent": rss_growth,
            "baseline_node_rss_kib": baseline_node_rss,
            "final_node_rss_kib": final_node_rss,
            "node_rss_growth_percent": node_rss_growth,
            "node_processes": final_children.len(),
            "wall_ms": started.elapsed().as_secs_f64() * 1000.0,
        },
        "baseline_ratios": baseline_ratios,
        "thresholds": {
            "in_run_rss_growth_percent_max": 15.0,
            "steady_rss_ratio_max": 1.15,
            "pass": failures.is_empty(),
            "failures": failures,
        }
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

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}
