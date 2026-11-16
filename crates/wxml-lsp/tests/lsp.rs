//! End-to-end stdio protocol tests for `wxml-lsp --stdio`.
//!
//! Each test starts the real binary, writes `Content-Length` frames, and reads
//! responses through a reader thread with a bounded timeout. Nothing here
//! sleeps waiting for a notification; every wait is a channel receive with a
//! deadline.

// This is an integration test crate: panicking helpers are the assertion
// mechanism, and passing JSON values by value keeps call sites readable.
#![allow(
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::unwrap_used
)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_wxml-lsp");
const TIMEOUT: Duration = Duration::from_secs(10);
const MANY_REQUESTS: i64 = 200;
const URI: &str = "file:///pages/index/index.wxml";

/// A running server process plus its framed stdout channel.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(BIN)
            .arg("--stdio")
            .arg("--mock")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn wxml-lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(value) = read_frame(&mut reader) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            messages,
        }
    }

    fn start_real() -> Self {
        let mut child = Command::new(BIN)
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn wxml-lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(value) = read_frame(&mut reader) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            messages,
        }
    }

    fn write_body(&mut self, body: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        stdin.write_all(body).expect("body");
        stdin.flush().expect("flush");
    }

    fn write_header(&mut self, header: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        stdin.write_all(header.as_bytes()).expect("header");
        stdin.flush().expect("flush");
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize");
        self.write_body(&body);
    }

    fn request(&mut self, id: Value, method: &str, params: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn recv(&self) -> Value {
        self.messages
            .recv_timeout(TIMEOUT)
            .expect("server message before timeout")
    }

    fn recv_response(&self, id: &Value) -> Value {
        loop {
            let message = self.recv();
            if message.get("id") == Some(id) && message.get("method").is_none() {
                return message;
            }
        }
    }

    fn recv_notification(&self, method: &str) -> Value {
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str) == Some(method) {
                return message;
            }
        }
    }

    fn initialize(&mut self) {
        self.request(json!(1), "initialize", json!({}));
        let response = self.recv_response(&json!(1));
        assert!(response.get("result").is_some(), "{response}");
        self.notify("initialized", json!({}));
    }

    fn open(&mut self, text: &str, version: u64) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "wxml",
                    "version": version,
                    "text": text,
                },
            }),
        );
    }

    fn shutdown(&mut self) {
        self.request(json!(999), "shutdown", json!(null));
        let response = self.recv_response(&json!(999));
        assert_eq!(response["result"], json!(null), "{response}");
        self.notify("exit", json!(null));
    }

    fn finish(mut self) -> ExitStatus {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }
        wait_with_timeout(&mut self.child)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stdin.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length?;
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn wait_with_timeout(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("server did not exit before the timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn initialize_hover_shutdown_flow() {
    let mut client = Client::start();
    client.request(json!(1), "initialize", json!({"processId": null}));
    let response = client.recv_response(&json!(1));
    let capabilities = &response["result"]["capabilities"];
    assert_eq!(capabilities["textDocumentSync"]["openClose"], json!(true));
    assert_eq!(capabilities["textDocumentSync"]["change"], json!(2));
    assert_eq!(
        capabilities["textDocumentSync"]["save"]["includeText"],
        json!(false)
    );
    assert_eq!(capabilities["hoverProvider"], json!(true));
    assert_eq!(capabilities["definitionProvider"], json!(false));
    assert_eq!(response["result"]["serverInfo"]["name"], json!("wxml-lsp"));
    client.notify("initialized", json!({}));

    client.open("{{ user.name }}", 1);
    let diagnostics = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["params"]["uri"], json!(URI));
    assert_eq!(diagnostics["params"]["version"], json!(1));
    assert_eq!(diagnostics["params"]["diagnostics"], json!([]));

    client.request(
        json!(2),
        "textDocument/hover",
        json!({
            "textDocument": {"uri": URI},
            "position": {"line": 0, "character": 3},
        }),
    );
    let hover = client.recv_response(&json!(2));
    assert_eq!(hover["result"]["contents"]["kind"], json!("markdown"));
    assert_eq!(hover["result"]["contents"]["value"], json!("string"));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn did_change_and_close_publish_versioned_diagnostics() {
    let mut client = Client::start();
    client.initialize();
    client.open("abc", 1);
    let first = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(first["params"]["version"], json!(1));

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 2},
            "contentChanges": [{"text": "abcdef"}],
        }),
    );
    let second = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(second["params"]["version"], json!(2));

    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 3},
            "contentChanges": [{
                "range": {
                    "start": {"line": 0, "character": 1},
                    "end": {"line": 0, "character": 2},
                },
                "text": "X",
            }],
        }),
    );
    let third = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(third["params"]["version"], json!(3));

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": URI}}),
    );
    let closed = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(closed["params"]["uri"], json!(URI));
    assert_eq!(closed["params"]["diagnostics"], json!([]));
    assert!(closed["params"].get("version").is_none());

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn did_save_and_watched_files_are_no_ops_that_keep_the_session_alive() {
    let mut client = Client::start();
    client.initialize();
    client.open("{{ user.name }}", 1);
    let diagnostics = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["params"]["version"], json!(1));

    client.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": URI}}),
    );
    // Malformed notifications must be ignored instead of closing the stream.
    client.notify("textDocument/didSave", json!({}));
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": URI, "type": 2}]}),
    );
    client.notify("workspace/didChangeWatchedFiles", json!({"changes": {}}));

    client.request(
        json!(2),
        "textDocument/hover",
        json!({
            "textDocument": {"uri": URI},
            "position": {"line": 0, "character": 3},
        }),
    );
    let hover = client.recv_response(&json!(2));
    assert_eq!(hover["result"]["contents"]["value"], json!("string"));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn unknown_methods_and_malformed_json_are_reported() {
    let mut client = Client::start();
    client.initialize();

    client.request(json!(2), "textDocument/documentColor", json!({}));
    let error = client.recv_response(&json!(2));
    assert_eq!(error["error"]["code"], json!(-32601));

    client.write_body(b"{oops");
    let parse_error = client.recv_response(&Value::Null);
    assert_eq!(parse_error["error"]["code"], json!(-32700));
    assert_eq!(parse_error["id"], json!(null));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn invalid_utf8_and_deep_nesting_are_parse_errors() {
    let mut client = Client::start();
    client.initialize();

    client.write_body(&[0xff, 0xfe]);
    let invalid_utf8 = client.recv_response(&Value::Null);
    assert_eq!(invalid_utf8["error"]["code"], json!(-32700));

    let depth = 200;
    let mut nested = "[".repeat(depth);
    nested.push_str(&"]".repeat(depth));
    client.write_body(nested.as_bytes());
    let too_deep = client.recv_response(&Value::Null);
    assert_eq!(too_deep["error"]["code"], json!(-32700));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn structurally_invalid_messages_are_invalid_requests() {
    let mut client = Client::start();
    client.initialize();
    client.write_body(b"[]");
    let error = client.recv_response(&Value::Null);
    assert_eq!(error["error"]["code"], json!(-32600));
    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn oversized_content_length_is_rejected_without_panicking() {
    let mut client = Client::start();
    client.write_header("Content-Length: 999999999\r\n\r\n");
    let error = client.recv_response(&Value::Null);
    assert_eq!(error["error"]["code"], json!(-32700));
    // The frame is fatal, so the server stops reading and exits by itself.
    let status = client.finish();
    assert!(!status.success());
}

#[test]
fn path_traversal_uris_do_not_escape_the_server() {
    let mut client = Client::start();
    client.initialize();
    let suspect = "file:///../../etc/passwd.wxml";
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": suspect,
                "languageId": "wxml",
                "version": 1,
                "text": "abc",
            },
        }),
    );
    let diagnostics = client.recv_notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["params"]["uri"], json!(suspect));

    client.request(
        json!(3),
        "textDocument/hover",
        json!({
            "textDocument": {"uri": suspect},
            "position": {"line": 0, "character": 0},
        }),
    );
    let hover = client.recv_response(&json!(3));
    assert_eq!(hover["result"]["contents"]["value"], json!("string"));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn requests_before_initialize_are_rejected() {
    let mut client = Client::start();
    client.request(
        json!(1),
        "textDocument/hover",
        json!({
            "textDocument": {"uri": URI},
            "position": {"line": 0, "character": 0},
        }),
    );
    let error = client.recv_response(&json!(1));
    assert_eq!(error["error"]["code"], json!(-32002));

    client.initialize();
    client.request(json!(2), "initialize", json!({}));
    let again = client.recv_response(&json!(2));
    assert_eq!(again["error"]["code"], json!(-32600));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn requests_after_shutdown_are_rejected() {
    let mut client = Client::start();
    client.initialize();
    client.request(json!(2), "shutdown", json!(null));
    let shutdown = client.recv_response(&json!(2));
    assert_eq!(shutdown["result"], json!(null));

    client.request(
        json!(3),
        "textDocument/hover",
        json!({
            "textDocument": {"uri": URI},
            "position": {"line": 0, "character": 0},
        }),
    );
    let error = client.recv_response(&json!(3));
    assert_eq!(error["error"]["code"], json!(-32600));

    client.notify("exit", json!(null));
    assert!(client.finish().success());
}

#[test]
fn exit_without_shutdown_is_not_clean() {
    let mut client = Client::start();
    client.notify("exit", json!(null));
    assert!(!client.finish().success());
}

// This covers transport ordering and the bounded event channel (no frame
// loss). It does not force OS-level stdout backpressure: shrinking the pipe
// would need a platform-specific test, so that case stays a documented gap.
#[test]
fn many_frames_without_reading_do_not_drop_responses() {
    let mut client = Client::start();
    client.initialize();
    client.open("abc", 1);
    client.recv_notification("textDocument/publishDiagnostics");

    for index in 0..MANY_REQUESTS {
        client.request(
            json!(1_000 + index),
            "textDocument/hover",
            json!({
                "textDocument": {"uri": URI},
                "position": {"line": 0, "character": 0},
            }),
        );
    }
    client.request(json!(999), "shutdown", json!(null));

    let mut seen = std::collections::HashSet::new();
    let mut shutdown_seen = false;
    while !shutdown_seen || seen.len() < usize::try_from(MANY_REQUESTS).expect("count") {
        let message = client.recv();
        if message.get("method").is_some() {
            continue;
        }
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            continue;
        };
        if id == 999 {
            shutdown_seen = true;
        } else {
            seen.insert(id);
        }
    }
    assert_eq!(seen.len(), usize::try_from(MANY_REQUESTS).expect("count"));
    client.notify("exit", json!(null));
    assert!(client.finish().success());
}

#[test]
fn cancellation_is_harmless() {
    let mut client = Client::start();
    client.initialize();
    client.open("abc", 1);
    client.recv_notification("textDocument/publishDiagnostics");

    client.request(
        json!("hover"),
        "textDocument/hover",
        json!({
            "textDocument": {"uri": URI},
            "position": {"line": 0, "character": 0},
        }),
    );
    client.notify("$/cancelRequest", json!({"id": "hover"}));
    client.notify("$/cancelRequest", json!({"id": "missing"}));
    let response = client.recv_response(&json!("hover"));
    assert!(
        response.get("result").is_some() || response["error"]["code"] == json!(-32800),
        "{response}"
    );

    client.shutdown();
    assert!(client.finish().success());
}

/// UTF-16 position of a byte offset in a single-line or multi-line document.
fn completion_position(text: &str, offset: usize) -> Value {
    let head = &text[..offset];
    let line = head.matches('\n').count();
    let line_start = head.rfind('\n').map_or(0, |index| index + 1);
    let character: usize = head[line_start..].encode_utf16().count();
    json!({"line": line, "character": character})
}

fn completion_items(client: &mut Client, id: i64, text: &str, offset: usize) -> Vec<Value> {
    client.request(
        json!(id),
        "textDocument/completion",
        json!({
            "textDocument": {"uri": URI},
            "position": completion_position(text, offset),
        }),
    );
    let response = client.recv_response(&json!(id));
    assert!(response.get("error").is_none(), "{response}");
    response["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn item_labels(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item["label"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn completion_provider_is_advertised_and_tags_complete() {
    let mut client = Client::start();
    client.request(json!(1), "initialize", json!({}));
    let response = client.recv_response(&json!(1));
    let provider = &response["result"]["capabilities"]["completionProvider"];
    assert_eq!(provider["resolveProvider"], json!(false));
    assert!(provider["triggerCharacters"].as_array().is_some());
    client.notify("initialized", json!({}));

    let text = "<vi";
    client.open(text, 1);
    let items = completion_items(&mut client, 2, text, text.len());
    let labels = item_labels(&items);
    assert!(labels.contains(&"view".to_owned()), "{labels:?}");
    let view = items
        .iter()
        .find(|item| item["label"] == json!("view"))
        .expect("view item");
    assert_eq!(
        view["textEdit"]["range"]["start"],
        json!({"line": 0, "character": 1})
    );
    assert_eq!(
        view["textEdit"]["range"]["end"],
        json!({"line": 0, "character": 3})
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn attribute_names_values_directives_and_events_complete() {
    let mut client = Client::start();
    client.initialize();

    let text = "<button ";
    client.open(text, 1);
    let labels = item_labels(&completion_items(&mut client, 2, text, text.len()));
    assert!(labels.contains(&"type".to_owned()), "{labels:?}");
    assert!(labels.contains(&"bindtap".to_owned()), "{labels:?}");

    let text = "<button type=\"";
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 2},
            "contentChanges": [{"text": text}],
        }),
    );
    let items = completion_items(&mut client, 3, text, text.len());
    let labels = item_labels(&items);
    assert!(labels.contains(&"primary".to_owned()), "{labels:?}");
    assert!(labels.contains(&"warn".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"selector".to_owned()), "{labels:?}");

    let text = "<picker mode=\"";
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 3},
            "contentChanges": [{"text": text}],
        }),
    );
    let labels = item_labels(&completion_items(&mut client, 4, text, text.len()));
    assert!(labels.contains(&"selector".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"primary".to_owned()), "{labels:?}");

    let text = "<view wx:";
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 4},
            "contentChanges": [{"text": text}],
        }),
    );
    let labels = item_labels(&completion_items(&mut client, 5, text, text.len()));
    assert!(labels.contains(&"wx:if".to_owned()), "{labels:?}");
    assert!(labels.contains(&"wx:for".to_owned()), "{labels:?}");

    let text = "<view bindt";
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": URI, "version": 5},
            "contentChanges": [{"text": text}],
        }),
    );
    let labels = item_labels(&completion_items(&mut client, 6, text, text.len()));
    assert!(labels.contains(&"bindtap".to_owned()), "{labels:?}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn closing_tags_only_suggest_opened_elements() {
    let mut client = Client::start();
    client.initialize();
    let text = "<view><text></";
    client.open(text, 1);
    let labels = item_labels(&completion_items(&mut client, 2, text, text.len()));
    assert!(labels.contains(&"view".to_owned()), "{labels:?}");
    assert!(labels.contains(&"text".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"button".to_owned()), "{labels:?}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn input_type_values_and_event_attribute_forms_complete() {
    let mut client = Client::start();
    client.initialize();
    let text = "<input type=\"\"\n<view bind:tap=\"\"";
    client.open(text, 1);

    let input_offset = text.find("type=\"").expect("input") + "type=\"".len();
    let labels = item_labels(&completion_items(&mut client, 2, text, input_offset));
    assert!(labels.contains(&"number".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"primary".to_owned()), "{labels:?}");

    let bind_offset = text.find("bind:tap").expect("bind") + "bind:".len();
    let labels = item_labels(&completion_items(&mut client, 3, text, bind_offset));
    assert!(labels.contains(&"bind:tap".to_owned()), "{labels:?}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn text_comments_and_wxs_have_no_completion() {
    let mut client = Client::start();
    client.initialize();
    let text = "<view>plain text\n<!-- <vi -->\n<wxs module=\"m\">var x</wxs>";
    client.open(text, 1);

    for (id, needle) in [(2_i64, "plain"), (3, "<!--"), (4, "var x")] {
        let offset = text.find(needle).expect("needle") + needle.len();
        let items = completion_items(&mut client, id, text, offset);
        assert!(items.is_empty(), "completion at `{needle}`: {items:?}");
    }

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn backend_completion_degrades_quietly_with_the_mock_backend() {
    let mut client = Client::start();
    client.initialize();
    let text = "<view>{{ user.na }}</view>\n<view bindtap=\"onTap\" />";
    client.open(text, 1);

    let offset = text.find("na").expect("member") + 2;
    let items = completion_items(&mut client, 2, text, offset);
    assert!(items.is_empty(), "{items:?}");

    let offset = text.find("onTap").expect("handler") + 5;
    let items = completion_items(&mut client, 3, text, offset);
    assert!(items.is_empty(), "{items:?}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn invalid_completion_parameters_are_rejected() {
    let mut client = Client::start();
    client.initialize();
    client.request(json!(2), "textDocument/completion", json!({}));
    let error = client.recv_response(&json!(2));
    assert_eq!(error["error"]["code"], json!(-32602));

    client.request(
        json!(3),
        "textDocument/completion",
        json!({"textDocument": {"uri": URI}, "position": {"line": "x", "character": 0}}),
    );
    let error = client.recv_response(&json!(3));
    assert_eq!(error["error"]["code"], json!(-32602));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn non_wxml_documents_have_no_completion() {
    let mut client = Client::start();
    client.initialize();
    let uri = "file:///pages/index/index.ts";
    let text = "<vi";
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "typescript",
                "version": 1,
                "text": text,
            },
        }),
    );
    client.request(
        json!(2),
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": completion_position(text, text.len()),
        }),
    );
    let response = client.recv_response(&json!(2));
    assert_eq!(response["result"]["items"], json!([]));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn completion_positions_are_utf16_after_unicode_text() {
    let mut client = Client::start();
    client.initialize();
    let text = "中文\n<vi";
    client.open(text, 1);
    let offset = text.len();
    let items = completion_items(&mut client, 2, text, offset);
    let view = items
        .iter()
        .find(|item| item["label"] == json!("view"))
        .expect("view item");
    assert_eq!(
        view["textEdit"]["range"]["start"],
        json!({"line": 1, "character": 1})
    );
    assert_eq!(
        view["textEdit"]["range"]["end"],
        json!({"line": 1, "character": 3})
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn unquoted_attribute_values_insert_quoted_text() {
    let mut client = Client::start();
    client.initialize();
    let text = "<button type=";
    client.open(text, 1);
    let items = completion_items(&mut client, 2, text, text.len());
    let default = items
        .iter()
        .find(|item| item["label"] == json!("default"))
        .expect("default item");
    assert_eq!(default["insertText"], json!("\"default\""));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn custom_components_complete_from_using_components() {
    let mut client = Client::start_real();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/completion-components");
    let uri = format!("file://{}/index.wxml", root.display());
    client.request(
        json!(1),
        "initialize",
        json!({"rootUri": format!("file://{}", root.display())}),
    );
    let response = client.recv_response(&json!(1));
    assert!(response.get("result").is_some(), "{response}");
    client.notify("initialized", json!({}));

    let text = "<my";
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": text,
            },
        }),
    );
    client.request(
        json!(2),
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": completion_position(text, text.len()),
        }),
    );
    let response = client.recv_response(&json!(2));
    let labels = item_labels(response["result"]["items"].as_array().expect("items"));
    assert!(labels.contains(&"my-card".to_owned()), "{labels:?}");

    client.shutdown();
    assert!(client.finish().success());
}
