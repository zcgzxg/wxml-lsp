#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::unwrap_used
)]

//! Real JSON-RPC diagnostics for local style/resource checks.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::run_with_host;
use wxml_types::{Backend, MockBackend};

mod common;

use common::TempProject;

fn fixture() -> TempProject {
    let project = TempProject::new("wxml-style-diagnostics");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({})");
    project.write("pages/index/index.wxss", ".known { color: red; }\n");
    project
}

struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        let reader_stream = stream.try_clone().expect("clone stream");
        reader_stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        Self {
            writer: stream,
            reader: BufReader::new(reader_stream),
            next_id: 1,
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

    fn response(&mut self, id: i64) -> Value {
        loop {
            let message = self.recv();
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
        }
    }

    fn diagnostics(&mut self) -> Value {
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                return message;
            }
        }
    }
}

fn start(project: &TempProject, options: Value) -> (Client, thread::JoinHandle<()>) {
    let (server, client) = UnixStream::pair().expect("socket pair");
    let server_read = server.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
    let handle = thread::spawn(move || {
        let _ = run_with_host(server_read, server, backend, host);
    });
    let mut client = Client::new(client);
    let id = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "capabilities": {},
            "initializationOptions": options,
        }),
    );
    let response = client.response(id);
    assert!(response.get("result").is_some(), "{response}");
    client.notify("initialized", json!({}));
    (client, handle)
}

fn diagnostic_codes(message: &Value) -> Vec<String> {
    message["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn style_diagnostics_publish_and_clear_for_resources_and_classes() {
    let project = fixture();
    let (mut client, handle) = start(
        &project,
        json!({"wxml": {"styleDiagnostics": {"resources": true, "classes": true}}}),
    );
    let uri = project.uri("pages/index/index.wxml");
    let text = "<view class=\"known unknown\" background=\"red\"><image src=\"images/missing.png\"/></view>";
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": text,
            }
        }),
    );
    let published = client.diagnostics();
    assert_eq!(published["params"]["uri"], json!(uri));
    let codes = diagnostic_codes(&published);
    assert!(
        codes.contains(&"style.missing-resource".to_owned()),
        "{published}"
    );
    assert!(
        codes.contains(&"style.unknown-class".to_owned()),
        "{published}"
    );
    for diagnostic in published["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics")
    {
        assert_eq!(diagnostic["source"], json!("wxml-lsp"));
        assert_eq!(diagnostic["severity"], json!(2));
        assert!(diagnostic["range"]["start"].is_object());
    }

    // Defining the class in an unsaved overlay clears only the class code.
    let style_uri = project.uri("pages/index/index.wxss");
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": style_uri,
                "languageId": "wxss",
                "version": 1,
                "text": ".known { } .unknown { }\n",
            }
        }),
    );
    let refreshed = client.diagnostics();
    let codes = diagnostic_codes(&refreshed);
    assert!(
        !codes.contains(&"style.unknown-class".to_owned()),
        "{refreshed}"
    );
    assert!(
        codes.contains(&"style.missing-resource".to_owned()),
        "{refreshed}"
    );

    // Removing the broken path clears the resource code on the next change.
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": "<view class=\"known unknown\"></view>"}],
        }),
    );
    let cleared = client.diagnostics();
    assert!(
        diagnostic_codes(&cleared).is_empty(),
        "diagnostics must clear: {cleared}"
    );

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": style_uri}}),
    );
    let id = client.request("shutdown", Value::Null);
    let _ = client.response(id);
    client.notify("exit", Value::Null);
    handle.join().expect("server thread");
}

#[test]
fn class_diagnostics_are_off_by_default() {
    let project = fixture();
    let (mut client, handle) = start(&project, json!({}));
    let uri = project.uri("pages/index/index.wxml");
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": "<view class=\"not-defined\"></view>",
            }
        }),
    );
    let published = client.diagnostics();
    assert!(
        diagnostic_codes(&published).is_empty(),
        "class diagnostics must be conservative by default: {published}"
    );
    let id = client.request("shutdown", Value::Null);
    let _ = client.response(id);
    client.notify("exit", Value::Null);
    handle.join().expect("server thread");
}

#[test]
fn style_diagnostics_switches_are_independent() {
    let project = fixture();
    let (mut client, handle) = start(
        &project,
        json!({"wxml": {"styleDiagnostics": {"resources": false, "classes": true}}}),
    );
    let uri = project.uri("pages/index/index.wxml");
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": "<view class=\"unknown\"><image src=\"images/missing.png\"/></view>",
            }
        }),
    );
    let published = client.diagnostics();
    let codes = diagnostic_codes(&published);
    assert!(
        codes.contains(&"style.unknown-class".to_owned()),
        "{published}"
    );
    assert!(
        !codes.contains(&"style.missing-resource".to_owned()),
        "resources are disabled by configuration: {published}"
    );
    let id = client.request("shutdown", Value::Null);
    let _ = client.response(id);
    client.notify("exit", Value::Null);
    handle.join().expect("server thread");
}
