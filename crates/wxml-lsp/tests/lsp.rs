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
fn unknown_methods_and_malformed_json_are_reported() {
    let mut client = Client::start();
    client.initialize();

    client.request(json!(2), "textDocument/formatting", json!({}));
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
    let suspect = "file:///../../etc/passwd";
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
