#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation
)]

//! IT-14 scheduling behavior over the real JSON-RPC protocol loop.
//!
//! The backend is a fake C1 responder so timing is deterministic; the real
//! protocol, project host and `NodeBackend` cache barriers are exercised.

mod common;
mod q2_support;

use std::thread;
use std::time::{Duration, Instant};

use q2_support::{Failure, WXML, app_fixture, diagnostic_message, start_fake};

#[test]
fn rapid_edits_publish_only_the_latest_diagnostics() {
    let project = app_fixture();
    let (mut client, handle, shared) =
        start_fake(&project, Failure::None, Duration::from_millis(20));
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}\n<!-- NEW_0 -->", 1);
    let opened = client.wait_diagnostics(&uri, 1);
    assert!(diagnostic_message(&opened).starts_with("DISK_Q2"));
    let baseline = shared.request_count();

    for version in 2..=21 {
        client.change(
            &uri,
            &format!("{{{{ user.name }}}}\n<!-- NEW_{version} -->"),
            version,
        );
    }
    let latest = client.wait_diagnostics(&uri, 21);
    let last_request = shared.last_request().expect("an analysis request");
    let request: serde_json::Value = serde_json::from_str(&last_request).expect("request json");
    let view_revision = request["params"]["analysisStamp"]["viewRevision"]
        .as_u64()
        .expect("view revision");
    assert_eq!(
        diagnostic_message(&latest),
        format!("DISK_Q2:1:{view_revision}"),
        "the newest revision must win: {latest}"
    );
    let analyses = shared.request_count().saturating_sub(baseline);
    assert!(
        analyses <= 2,
        "a typing burst must coalesce into at most one or two analyses, ran {analyses}"
    );

    // Any older publication left in the stream would be a stale result; the
    // server must drop results whose document revision changed.
    for message in client.take_pending() {
        if message.get("method").and_then(serde_json::Value::as_str)
            == Some("textDocument/publishDiagnostics")
            && message["params"]["uri"].as_str() == Some(uri.as_str())
            && message["params"]["version"]
                .as_u64()
                .is_some_and(|version| version > 1 && version < 21)
        {
            panic!(
                "stale diagnostics published for version {}: {message}",
                message["params"]["version"]
            );
        }
    }

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn cancelling_a_queued_request_answers_without_waiting_for_the_backend() {
    let project = app_fixture();
    let (mut client, handle, shared) =
        start_fake(&project, Failure::None, Duration::from_millis(300));
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    // The opening diagnostics request occupies the slow worker.
    thread::sleep(Duration::from_millis(40));

    let running = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    thread::sleep(Duration::from_millis(20));
    let queued = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    thread::sleep(Duration::from_millis(20));
    let started = Instant::now();
    client.notify("$/cancelRequest", serde_json::json!({"id": queued}));
    let cancelled = client.response(queued);
    let latency = started.elapsed();
    assert_eq!(
        cancelled["error"]["code"],
        serde_json::json!(-32_800),
        "{cancelled}"
    );
    assert!(
        latency < Duration::from_millis(150),
        "queued cancellation latency {latency:?}"
    );
    let running = client.response(running);
    assert!(
        running.get("result").is_some() || running.get("error").is_some(),
        "{running}"
    );
    assert!(
        shared.request_count() <= 2,
        "a cancelled queued hover must not reach the backend: {} requests",
        shared.request_count()
    );

    // A running call cannot be interrupted silently; the client still gets an
    // immediate cancellation response, and the late result is dropped.
    client.change(&uri, "{{ user.name }}\n<!-- NEW_C -->", 2);
    thread::sleep(Duration::from_millis(150));
    let running = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    thread::sleep(Duration::from_millis(20));
    let started = Instant::now();
    client.notify("$/cancelRequest", serde_json::json!({"id": running}));
    let cancelled = client.response(running);
    let latency = started.elapsed();
    assert_eq!(
        cancelled["error"]["code"],
        serde_json::json!(-32_800),
        "{cancelled}"
    );
    assert!(
        latency < Duration::from_millis(150),
        "running cancellation response latency {latency:?}"
    );
    // Let the non-interruptible fake call return before shutting the session
    // down, so its late result is dropped while the transport is still open.
    thread::sleep(Duration::from_millis(400));

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn static_features_stay_responsive_while_the_backend_is_slow() {
    let project = app_fixture();
    let (mut client, handle, _shared) =
        start_fake(&project, Failure::None, Duration::from_millis(500));
    let uri = project.uri(WXML);
    client.open(&uri, "<view cla", 1);
    let started = Instant::now();
    let completion = client.completion(&uri, 0, "<view cla".len() as u32);
    assert!(
        completion["result"]["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "{completion}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "static completion must not wait for the slow backend: {:?}",
        started.elapsed()
    );
    // Drain the slow opening diagnostics before shutdown; otherwise the late
    // publish can race the transport close in this in-process harness.
    client.wait_diagnostics(&uri, 1);
    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}
