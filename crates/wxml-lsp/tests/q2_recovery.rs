#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation
)]

//! IT-15/IT-18 and AC-20 failure recovery over real JSON-RPC framing.
//!
//! Fake C1 transports inject timeout, crash and protocol-version failures. A
//! real child-process script covers startup protocol mismatch and immediate
//! exit without requiring Node. Resource deletion and app/root removal are
//! exercised against the real project host.

mod common;
mod q2_support;

use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::{Duration, Instant};

use q2_support::{
    Failure, SCRIPT, WXML, app_fixture, diagnostic_message, diagnostic_messages, start_fake,
};
use serde_json::Value;
use wxml_lsp::backend_node::protocol::ProtocolError;
use wxml_lsp::backend_node::{NodeBackendConfig, NodeTransportError, StdioNode};

fn wait_for_symbols_request(shared: &q2_support::Shared, baseline: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let requests = shared
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(line) = requests.iter().skip(baseline).find(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|request| request["method"].as_str().map(str::to_owned))
                .as_deref()
                == Some("symbols")
        }) {
            return line.clone();
        }
        assert!(
            Instant::now() < deadline,
            "no symbols request was dispatched"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn has_unavailable(messages: &[String]) -> bool {
    messages.iter().any(|message| {
        let lower = message.to_ascii_lowercase();
        lower.contains("unavailable")
            || lower.contains("timed out")
            || lower.contains("unable")
            || lower.contains("failed")
    })
}

#[test]
fn timeout_crash_and_protocol_mismatch_degrade_then_recover() {
    for (failure, label) in [
        (Failure::TimeoutOnce, "timeout"),
        (Failure::CrashOnce, "crash"),
        (Failure::ProtocolOnce, "protocol"),
    ] {
        let project = app_fixture();
        let (mut client, handle, shared) = start_fake(&project, failure, Duration::ZERO);
        let uri = project.uri(WXML);
        let text = "<view cla>{{ user.name }}</view>";
        client.open(&uri, text, 1);
        let degraded = client.wait_diagnostics(&uri, 1);
        let messages = diagnostic_messages(&degraded);
        assert!(
            has_unavailable(&messages),
            "{label}: a failed backend must be visible, got {degraded}"
        );
        assert!(
            !messages
                .iter()
                .any(|message| message.starts_with("DISK_Q2")),
            "{label}: stale backend diagnostics must not be published"
        );

        // Static features never need Node.
        let completion = client.completion(&uri, 0, "<view cla".len() as u32);
        assert!(
            completion["result"]["items"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{label}: {completion}"
        );

        // Replay an unsaved script overlay, then edit the template. The next
        // analysis must contain the overlay text and a supported check.
        client.open_language(
            &project.uri(SCRIPT),
            "typescript",
            "Page({ data: { user: { name: 'ZED_RECOVERED' } } });",
            1,
        );
        client.change(&uri, "<view class=\"card\">{{ user.name }}</view>", 2);
        let recovered = client.wait_diagnostics(&uri, 2);
        let messages = diagnostic_messages(&recovered);
        assert!(
            !has_unavailable(&messages),
            "{label}: recovery must not keep the degraded status: {recovered}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("ZED_RECOVERED")),
            "{label}: the replayed overlay must reach the recovered backend: {recovered}"
        );
        assert!(
            shared.connects.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "{label}: a failed generation must start a new transport"
        );

        let hover = client.hover(
            &uri,
            0,
            "<view class=\"card\">{{ user.".chars().count() as u32,
        );
        assert!(
            hover.to_string().contains("ZED_RECOVERED") || hover["result"].is_null(),
            "{label}: recovered hover must use the new generation: {hover}"
        );

        client.finish();
        let outcome = handle.join().expect("server thread").expect("outcome");
        assert!(outcome.clean_shutdown, "{label}");
    }
}

#[test]
fn a_real_child_process_with_a_bad_protocol_or_immediate_exit_is_rejected() {
    let project = app_fixture();
    let mismatch = project.root().join("fake-node-mismatch.sh");
    std::fs::write(
        &mismatch,
        "#!/bin/sh\nread line\nprintf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":\"ping\",\"result\":{\"ok\":true,\"protocolVersion\":1}}'\nsleep 2\n",
    )
    .expect("script");
    std::fs::set_permissions(&mismatch, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let config = NodeBackendConfig {
        node: mismatch.clone(),
        server: Some(mismatch),
        backend_dir: Some(project.root().to_path_buf()),
        timeout: Duration::from_secs(5),
        ..NodeBackendConfig::default()
    };
    let error = StdioNode::start(&config).expect_err("protocol 1 must be rejected");
    assert!(
        matches!(error, NodeTransportError::Protocol(ProtocolError::Version)),
        "{error:?}"
    );

    let crash = project.root().join("fake-node-crash.sh");
    std::fs::write(&crash, "#!/bin/sh\nexit 7\n").expect("script");
    std::fs::set_permissions(&crash, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let config = NodeBackendConfig {
        node: crash.clone(),
        server: Some(crash),
        backend_dir: Some(project.root().to_path_buf()),
        timeout: Duration::from_secs(5),
        ..NodeBackendConfig::default()
    };
    let error = StdioNode::start(&config).expect_err("an exited backend must not start");
    assert!(
        matches!(
            error,
            NodeTransportError::Exited | NodeTransportError::Io(_) | NodeTransportError::Timeout
        ),
        "{error:?}"
    );
}

#[test]
fn deleting_and_restoring_a_resource_updates_diagnostics_without_stale_results() {
    let project = app_fixture();
    project.write(WXML, "<view><image src=\"images/logo.png\" /></view>");
    let (mut client, handle, _shared) = start_fake(&project, Failure::None, Duration::ZERO);
    let uri = project.uri(WXML);
    client.open(&uri, "<view><image src=\"images/logo.png\" /></view>", 1);
    let missing = client.wait_diagnostics(&uri, 1);
    assert!(
        diagnostic_messages(&missing)
            .iter()
            .any(|message| message.contains("does not exist")),
        "{missing}"
    );

    project.write("pages/index/images/logo.png", "png");
    client.watched(&project.uri("pages/index/images/logo.png"), 1);
    let restored = client.wait_diagnostics_matching(&uri, |message| {
        diagnostic_messages(message)
            .iter()
            .all(|text| !text.contains("does not exist"))
    });
    assert!(
        diagnostic_messages(&restored)
            .iter()
            .all(|message| !message.contains("does not exist")),
        "a created resource must clear the missing-file diagnostic: {restored}"
    );

    std::fs::remove_file(project.root().join("pages/index/images/logo.png")).expect("delete");
    client.watched(&project.uri("pages/index/images/logo.png"), 3);
    let deleted = client.wait_diagnostics_matching(&uri, |message| {
        diagnostic_messages(message)
            .iter()
            .any(|text| text.contains("does not exist"))
    });
    assert!(
        diagnostic_messages(&deleted)
            .iter()
            .any(|message| message.contains("does not exist")),
        "a deleted resource must be reported again: {deleted}"
    );

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn removing_a_page_from_app_json_clears_stale_type_diagnostics() {
    let project = app_fixture();
    let (mut client, handle, _shared) = start_fake(&project, Failure::None, Duration::ZERO);
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    let initial = client.wait_diagnostics(&uri, 1);
    assert!(
        diagnostic_messages(&initial)
            .iter()
            .any(|message| message.starts_with("DISK_Q2")),
        "{initial}"
    );

    client.open_language(&project.uri("app.json"), "json", r#"{"pages":[]}"#, 1);
    let removed = client.wait_next_diagnostics(&uri);
    assert!(
        diagnostic_messages(&removed)
            .iter()
            .all(|message| !message.starts_with("DISK_Q2")),
        "a removed page must not keep stale type diagnostics: {removed}"
    );

    client.close(&project.uri("app.json"));
    let restored = client.wait_next_diagnostics(&uri);
    assert!(
        diagnostic_messages(&restored)
            .iter()
            .any(|message| message.starts_with("DISK_Q2")),
        "closing the app.json overlay must restore the page analysis: {restored}"
    );

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn removing_and_readding_the_workspace_root_drops_project_state() {
    let project = app_fixture();
    let (mut client, handle, _shared) = start_fake(&project, Failure::None, Duration::ZERO);
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    let initial = client.wait_diagnostics(&uri, 1);
    assert!(diagnostic_message(&initial).starts_with("DISK_Q2"));

    let root = project.root_uri();
    client.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {
                "added": [],
                "removed": [{"uri": root, "name": "root"}]
            }
        }),
    );
    let cleared = client.wait_next_diagnostics(&uri);
    assert_eq!(
        cleared["params"]["diagnostics"],
        serde_json::json!([]),
        "{cleared}"
    );

    // No stale result may arrive from the old project after the removal.
    thread::sleep(Duration::from_millis(200));
    for message in client.take_pending() {
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && message["params"]["uri"].as_str() == Some(uri.as_str())
        {
            assert!(
                message["params"]["diagnostics"]
                    .as_array()
                    .is_some_and(|diagnostics| diagnostics.iter().all(|diagnostic| {
                        !diagnostic["message"]
                            .as_str()
                            .unwrap_or_default()
                            .starts_with("DISK_Q2")
                    })),
                "stale project diagnostics after removal: {message}"
            );
        }
    }
    let missing_hover = client.hover(&uri, 0, 10);
    assert!(
        missing_hover["result"].is_null(),
        "removed project must not answer with stale types: {missing_hover}"
    );

    client.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {
                "added": [{"uri": root, "name": "root"}],
                "removed": []
            }
        }),
    );
    client.change(&uri, "{{ user.name }}\n", 2);
    let recovered = client.wait_diagnostics(&uri, 2);
    assert!(
        diagnostic_message(&recovered).starts_with("DISK_Q2"),
        "re-adding the root must make project analysis available again: {recovered}"
    );

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn a_root_removal_answers_an_inflight_semantic_request_exactly_once() {
    let project = app_fixture();
    let (mut client, handle, shared) = start_fake(&project, Failure::None, Duration::ZERO);
    shared.set_symbols_delay(Duration::from_millis(300));
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    client.wait_diagnostics(&uri, 1);
    let baseline = shared.request_count();
    let rename = client.request(
        "textDocument/rename",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 10},
            "newName": "account"
        }),
    );
    let _ = wait_for_symbols_request(&shared, baseline);
    let root = project.root_uri();
    client.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {"added": [], "removed": [{"uri": root, "name": "root"}]}
        }),
    );
    let mut responses = 0;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && responses == 0 {
        let message = client.recv();
        if message.get("id").and_then(Value::as_i64) == Some(rename)
            && message.get("method").is_none()
        {
            responses += 1;
        }
    }
    assert_eq!(
        responses, 1,
        "an in-flight semantic request must answer once"
    );
    // The late symbol result must not produce a second response.
    thread::sleep(Duration::from_millis(350));
    for message in client.take_pending() {
        assert!(
            message.get("id").and_then(Value::as_i64) != Some(rename)
                || message.get("method").is_some(),
            "duplicate response after root removal: {message}"
        );
    }
    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn cancelling_semantic_requests_releases_pending_intents() {
    let project = app_fixture();
    let (mut client, handle, shared) = start_fake(&project, Failure::None, Duration::ZERO);
    shared.set_symbols_delay(Duration::from_millis(150));
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    client.wait_diagnostics(&uri, 1);
    let mut ids = Vec::new();
    for _ in 0..20 {
        let id = client.request(
            "textDocument/rename",
            serde_json::json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 10},
                "newName": "account"
            }),
        );
        client.notify("$/cancelRequest", serde_json::json!({"id": id}));
        ids.push(id);
    }
    let mut cancelled = 0;
    while cancelled < ids.len() {
        let id = client.request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 10}
            }),
        );
        let response = client.response(id);
        assert!(response.get("result").is_some(), "{response}");
        for message in client.take_pending() {
            if message.get("method").is_none()
                && message["error"]["code"] == serde_json::json!(-32800)
            {
                cancelled += 1;
            }
        }
    }
    let root = project.root_uri();
    client.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {"added": [], "removed": [{"uri": root, "name": "root"}]}
        }),
    );
    thread::sleep(Duration::from_millis(400));
    for message in client.take_pending() {
        let id = message.get("id").and_then(Value::as_i64);
        assert!(
            id.is_none_or(|id| !ids.contains(&id)) || message.get("method").is_some(),
            "a cancelled semantic id received a second response: {message}"
        );
    }
    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn a_reused_request_id_receives_the_new_result_not_the_old_cancellation() {
    let project = app_fixture();
    let (mut client, handle, _shared) =
        start_fake(&project, Failure::None, Duration::from_millis(100));
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    let first = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    thread::sleep(Duration::from_millis(20));
    client.notify("$/cancelRequest", serde_json::json!({"id": first}));
    let cancelled = client.response(first);
    assert_eq!(cancelled["error"]["code"], serde_json::json!(-32800));
    client.request_with_id(
        first,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    let response = client.response(first);
    assert!(
        response.get("result").is_some(),
        "the reused id must receive its own result: {response}"
    );
    assert_ne!(response["error"]["code"], serde_json::json!(-32800));
    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn replaying_snapshots_and_overlays_after_root_readd_uses_unsaved_text() {
    let project = app_fixture();
    let (mut client, handle, shared) = start_fake(&project, Failure::None, Duration::ZERO);
    let uri = project.uri(WXML);
    let script_uri = project.uri(SCRIPT);
    let unsaved = "<view>{{ user.name }}";
    client.open(&uri, unsaved, 1);
    let initial = client.wait_diagnostics(&uri, 1);
    assert!(
        diagnostic_messages(&initial)
            .iter()
            .any(|message| message.contains("unclosed") || message.contains("expected"))
            || !diagnostic_messages(&initial).is_empty(),
        "the unsaved malformed template must produce a diagnostic: {initial}"
    );
    client.open_language(
        &script_uri,
        "typescript",
        "Page({ data: { user: { name: 'ZED_OVERLAY' } } });",
        2,
    );
    let _ = client
        .wait_diagnostics_matching(&uri, |message| message.to_string().contains("ZED_OVERLAY"));

    let root = project.root_uri();
    client.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {"added": [], "removed": [{"uri": root, "name": "root"}]}
        }),
    );
    client.wait_diagnostics_matching(&uri, |message| {
        message["params"]["diagnostics"] == serde_json::json!([])
    });
    client.notify(
        "workspace/didChangeWorkspaceFolders",
        serde_json::json!({
            "event": {"added": [{"uri": root, "name": "root"}], "removed": []}
        }),
    );
    let _ = client.wait_next_diagnostics(&uri);
    let hover = client.hover(&uri, 0, 17);
    assert!(
        hover.to_string().contains("ZED_OVERLAY"),
        "hover after root readd must use the replayed overlay: {hover}"
    );
    let baseline = shared.request_count();
    let rename = client.request(
        "textDocument/rename",
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 17},
            "newName": "renamed"
        }),
    );
    let symbols = wait_for_symbols_request(&shared, baseline);
    assert!(
        symbols.contains("ZED_OVERLAY"),
        "symbols after root readd must use the replayed overlay: {symbols}"
    );
    let _ = client.response(rename);

    // Close and reopen both buffers: the second open must still use the
    // overlay text, not the disk fallback.
    client.close(&script_uri);
    client.close(&uri);
    client.open_language(
        &script_uri,
        "typescript",
        "Page({ data: { user: { name: 'ZED_OVERLAY' } } });",
        3,
    );
    client.open(&uri, unsaved, 2);
    let _ = client.wait_diagnostics(&uri, 2);
    let reopened = client.hover(&uri, 0, 17);
    assert!(
        reopened.to_string().contains("ZED_OVERLAY"),
        "reopened buffers must keep overlay text: {reopened}"
    );

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn script_overlay_edits_update_dependents_without_a_template_edit() {
    let project = app_fixture();
    let (mut client, handle, _shared) = start_fake(&project, Failure::None, Duration::ZERO);
    let uri = project.uri(WXML);
    let script_uri = project.uri(SCRIPT);
    client.open(&uri, "{{ user.name }}", 1);
    client.wait_diagnostics(&uri, 1);
    client.open_language(
        &script_uri,
        "typescript",
        "Page({ data: { user: { name: 'ZED_FIRST' } } });",
        2,
    );
    let _ =
        client.wait_diagnostics_matching(&uri, |message| message.to_string().contains("ZED_FIRST"));
    client.change(
        &script_uri,
        "Page({ data: { user: { name: 'ZED_SECOND' } } });",
        3,
    );
    let _ = client
        .wait_diagnostics_matching(&uri, |message| message.to_string().contains("ZED_SECOND"));
    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn debounce_publishes_during_a_continuous_message_stream() {
    let project = app_fixture();
    let (mut client, handle, _shared) = start_fake(&project, Failure::None, Duration::ZERO);
    let uri = project.uri(WXML);
    client.open(&uri, "{{ user.name }}", 1);
    client.wait_diagnostics(&uri, 1);
    client.change(&uri, "{{ user.name }}\n<!-- debounce flood -->", 2);
    let started = Instant::now();
    let deadline = started + Duration::from_millis(700);
    let mut published = false;
    while Instant::now() < deadline {
        let id = client.request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 3}
            }),
        );
        let _ = client.response(id);
        for message in client.take_pending() {
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
                && message["params"]["version"]
                    .as_u64()
                    .is_some_and(|version| version >= 2)
            {
                published = true;
                break;
            }
        }
        if published {
            break;
        }
    }
    let elapsed = started.elapsed();
    if !published {
        // Drain the eventual publish so the failure message carries the timing.
        let _ = client.wait_diagnostics(&uri, 2);
    }
    assert!(
        published,
        "diagnostics were starved by a continuous message stream ({elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "debounced diagnostics arrived too late: {elapsed:?}"
    );
    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn an_invalid_startup_configuration_is_rejected_and_keeps_the_server_uninitialized() {
    let project = app_fixture();
    let (client, handle, _shared) = start_fake(&project, Failure::None, Duration::ZERO);
    // `start_fake` already initialized successfully; start a second raw session
    // for the invalid configuration case so the first server is unaffected.
    client.finish();
    let _ = handle.join().expect("server thread").expect("outcome");

    let (server_stream, client_stream) = std::os::unix::net::UnixStream::pair().expect("pair");
    let server_read = server_stream.try_clone().expect("clone");
    let host = std::sync::Arc::new(wxml_lsp::project_host::ProjectState::new());
    let backend = std::sync::Arc::new(wxml_lsp::backend_node::NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        || Ok(Box::new(q2_support::NoopTransport)),
    ));
    let handle =
        thread::spawn(move || wxml_lsp::run_with_host(server_read, server_stream, backend, host));
    let mut raw = q2_support::Client::new(client_stream);
    let response = raw.initialize_response(
        &project,
        serde_json::json!({"apps": [{"root": "/absolute"}]}),
    );
    assert_eq!(
        response["error"]["code"],
        serde_json::json!(-32602),
        "{response}"
    );
    let request = raw.request(
        "textDocument/hover",
        serde_json::json!({"textDocument": {"uri": project.uri(WXML)}, "position": {"line": 0, "character": 0}}),
    );
    let response = raw.response(request);
    assert_eq!(
        response["error"]["code"],
        serde_json::json!(-32002),
        "{response}"
    );
    raw.finish_without_initialize();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(!outcome.clean_shutdown);
}
