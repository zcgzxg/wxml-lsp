//! End-to-end static component intelligence over the real stdio server.
//!
//! The Node backend is deliberately pointed at a nonexistent executable: tag
//! completion, static hover and component definition must work without it.
//! Only static result shapes are asserted; no Node process is required.
#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::unwrap_used
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_wxml-lsp");
const TIMEOUT: Duration = Duration::from_secs(10);

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: i64,
    version: i64,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(BIN)
            .arg("--stdio")
            .env("WXML_NODE", "/nonexistent/opencode-node")
            .env(
                "WXML_TYPE_BACKEND",
                "/nonexistent/opencode-backend/server.js",
            )
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
            next_id: 1,
            version: 1,
        }
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize");
        let stdin = self.stdin.as_mut().expect("stdin");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        stdin.write_all(&body).expect("body");
        stdin.flush().expect("flush");
    }

    fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn recv_response(&self, id: i64) -> Value {
        loop {
            let message = self
                .messages
                .recv_timeout(TIMEOUT)
                .expect("server response before timeout");
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return message;
            }
        }
    }

    fn initialize(&mut self, root: &Path) {
        let id = self.request(
            "initialize",
            json!({"processId": null, "rootUri": format!("file://{}", root.display())}),
        );
        let response = self.recv_response(id);
        assert!(response.get("result").is_some(), "{response}");
        assert_eq!(
            response["result"]["capabilities"]["hoverProvider"],
            json!(true),
            "static hover is always advertised"
        );
        self.notify("initialized", json!({}));
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": self.version,
                    "text": text,
                },
            }),
        );
    }

    fn change(&mut self, uri: &str, text: &str) {
        self.version += 1;
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": self.version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    fn at(&mut self, method: &str, uri: &str, text: &str, byte: usize) -> Value {
        self.change(uri, text);
        let id = self.request(
            method,
            json!({
                "textDocument": {"uri": uri},
                "position": lsp_position(text, byte),
            }),
        );
        let response = self.recv_response(id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn shutdown(&mut self) {
        let id = self.request("shutdown", json!(null));
        let response = self.recv_response(id);
        assert_eq!(response["result"], json!(null), "{response}");
        self.notify("exit", json!(null));
    }

    fn finish(mut self) -> std::process::ExitStatus {
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
    let mut body = vec![0_u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn wait_with_timeout(child: &mut Child) -> std::process::ExitStatus {
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

fn lsp_position(text: &str, byte: usize) -> Value {
    assert!(
        text.is_char_boundary(byte),
        "offset {byte} is not a boundary"
    );
    let prefix = &text[..byte];
    let line = prefix.matches('\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = prefix[line_start..].encode_utf16().count();
    json!({"line": line, "character": character})
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/component-intelligence")
}

fn page_uri(root: &Path) -> String {
    format!("file://{}/pages/index/index.wxml", root.display())
}

fn labels(result: &Value) -> Vec<String> {
    result["items"]
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str().map(str::to_owned))
        .collect()
}

fn hover_text(result: &Value) -> String {
    result["contents"]["value"]
        .as_str()
        .expect("hover markdown")
        .to_owned()
}

fn definition_uri(result: &Value) -> Option<&str> {
    result
        .as_array()
        .and_then(|locations| locations.first())
        .and_then(|location| location["uri"].as_str())
}

#[test]
fn component_completion_hover_and_definition() {
    let root = fixture();
    let uri = page_uri(&root);
    let mut client = Client::start();
    client.initialize(&root);
    client.open(&uri, "<view />");

    let text = "<outer-card ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let labels = labels(&items);
    for expected in ["count", "title", "active", "class", "bindtap"] {
        assert!(labels.contains(&expected.to_owned()), "{labels:?}");
    }
    let count = items["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["label"] == "count")
        .expect("count property");
    assert_eq!(count["detail"], json!("number"));
    assert!(
        count["documentation"]
            .as_str()
            .is_some_and(|documentation| documentation.contains("计数")),
        "{count}"
    );

    let text = "<outer-card count=\"1\" />";
    let offset = text.find("outer-card").expect("tag") + 2;
    let hover = client.at("textDocument/hover", &uri, text, offset);
    let tag_hover = hover_text(&hover);
    assert!(tag_hover.contains("Custom component"), "{tag_hover}");
    assert!(
        tag_hover.contains("components/outer-card/outer-card.wxml"),
        "{tag_hover}"
    );
    assert!(tag_hover.contains("count"), "{tag_hover}");
    assert!(tag_hover.contains("标题"), "{tag_hover}");
    assert!(
        !tag_hover.contains(root.to_str().expect("root")),
        "hover must not expose the absolute project root: {tag_hover}"
    );

    let offset = text.find("count").expect("attribute") + 2;
    let hover = client.at("textDocument/hover", &uri, text, offset);
    let property_hover = hover_text(&hover);
    assert!(property_hover.contains("number"), "{property_hover}");
    assert!(property_hover.contains("计数"), "{property_hover}");
    assert!(
        property_hover.contains("components/outer-card/outer-card.ts"),
        "{property_hover}"
    );
    assert_eq!(
        hover["range"]["start"],
        json!({"line": 0, "character": 12}),
        "{hover}"
    );

    let text = "<outer-card />";
    let offset = text.find("outer-card").expect("tag") + 2;
    let definition = client.at("textDocument/definition", &uri, text, offset);
    assert!(
        definition_uri(&definition)
            .is_some_and(|uri| uri.ends_with("components/outer-card/outer-card.wxml")),
        "{definition}"
    );

    let text = "<outer-card count=\"1\" />";
    let offset = text.find("count").expect("attribute") + 2;
    let definition = client.at("textDocument/definition", &uri, text, offset);
    let uri = definition_uri(&definition).expect("property definition");
    assert!(
        uri.ends_with("components/outer-card/outer-card.ts"),
        "{uri}"
    );
    assert_eq!(
        definition[0]["range"]["start"],
        json!({"line": 3, "character": 4}),
        "{definition}"
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn picker_modes_and_duplicate_attributes() {
    let root = fixture();
    let uri = page_uri(&root);
    let mut client = Client::start();
    client.initialize(&root);
    client.open(&uri, "<view />");

    let text = "<picker mode=\"selector\" range=\"a\" ra";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"range-key".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"range".to_owned()), "{visible:?}");

    let text = "<picker mode=\"date\" ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    for expected in ["start", "end", "fields"] {
        assert!(visible.contains(&expected.to_owned()), "{visible:?}");
    }
    assert!(!visible.contains(&"range".to_owned()), "{visible:?}");
    assert!(
        !visible.contains(&"bindcolumnchange".to_owned()),
        "{visible:?}"
    );

    let text = "<picker mode=\"selector\" ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"range".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"start".to_owned()), "{visible:?}");

    let text = "<picker mode=\"{{ pickerMode }}\" ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"range".to_owned()), "{visible:?}");
    assert!(visible.contains(&"start".to_owned()), "{visible:?}");

    let text = "<picker mode=\"date\" mode";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    assert!(
        labels(&items).contains(&"mode".to_owned()),
        "editing the current attribute must keep it"
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn component_own_using_components_and_local_override() {
    let root = fixture();
    let uri = page_uri(&root);
    let mut client = Client::start();
    client.initialize(&root);
    client.open(&uri, "<view />");

    let text = "<shared-card ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"pageOnly".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"globalOnly".to_owned()), "{visible:?}");

    let text = "<shared-card />";
    let offset = text.find("shared-card").expect("tag") + 2;
    let hover = client.at("textDocument/hover", &uri, text, offset);
    let text = hover_text(&hover);
    assert!(
        text.contains("components/page-card/page-card.wxml"),
        "{text}"
    );
    assert!(text.contains("pages/index/index.json"), "{text}");
    assert!(!text.contains("global-card"), "{text}");

    let component_uri = format!(
        "file://{}/components/outer-card/outer-card.wxml",
        root.display()
    );
    client.open(&component_uri, "<view />");
    let text = "<inner";
    let items = client.at("textDocument/completion", &component_uri, text, text.len());
    assert!(
        labels(&items).contains(&"inner-badge".to_owned()),
        "component-own usingComponents must complete"
    );

    let text = "<inner-badge />";
    let offset = text.find("inner-badge").expect("tag") + 2;
    let hover = client.at("textDocument/hover", &component_uri, text, offset);
    assert!(
        hover_text(&hover).contains("components/inner-badge/inner-badge.wxml"),
        "{hover}"
    );

    let text = "<shared-card ";
    let items = client.at("textDocument/completion", &component_uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"pageOnly".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"globalOnly".to_owned()), "{visible:?}");

    let text = "<shared-card />";
    let offset = text.find("shared-card").expect("tag") + 2;
    let hover = client.at("textDocument/hover", &component_uri, text, offset);
    let text = hover_text(&hover);
    assert!(
        text.contains("components/outer-card/outer-card.json"),
        "component-local declaration must win: {text}"
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn unsaved_component_properties_update_static_queries() {
    let root = fixture();
    let uri = page_uri(&root);
    let mut client = Client::start();
    client.initialize(&root);
    client.open(&uri, "<view />");

    let script_uri = format!(
        "file://{}/components/outer-card/outer-card.ts",
        root.display()
    );
    client.open(
        &script_uri,
        "Component({ properties: { freshCount: { type: Number, value: 0 } } });",
    );

    let text = "<outer-card ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"freshCount".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"count".to_owned()), "{visible:?}");

    let text = "<outer-card freshCount=\"1\" />";
    let offset = text.find("freshCount").expect("attribute") + 2;
    let hover = client.at("textDocument/hover", &uri, text, offset);
    let hover = hover_text(&hover);
    assert!(hover.contains("number"), "{hover}");
    assert!(
        hover.contains("components/outer-card/outer-card.ts"),
        "{hover}"
    );

    let definition = client.at("textDocument/definition", &uri, text, offset);
    let definition_uri = definition_uri(&definition).expect("property definition");
    assert!(
        definition_uri.ends_with("components/outer-card/outer-card.ts"),
        "{definition_uri}"
    );
    assert_eq!(definition[0]["range"]["start"]["line"], 0, "{definition}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn unknown_external_components_degrade_honestly() {
    let root = fixture();
    let uri = page_uri(&root);
    let mut client = Client::start();
    client.initialize(&root);
    client.open(&uri, "<view />");

    let text = "<remote";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let remote = items["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["label"] == "remote-card")
        .expect("plugin tag");
    assert!(
        remote["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Plugin")),
        "{remote}"
    );

    let text = "<remote-card ";
    let items = client.at("textDocument/completion", &uri, text, text.len());
    let visible = labels(&items);
    assert!(visible.contains(&"id".to_owned()), "{visible:?}");
    assert!(visible.contains(&"bindtap".to_owned()), "{visible:?}");
    assert!(!visible.contains(&"globalOnly".to_owned()), "{visible:?}");

    let text = "<remote-card />";
    let offset = text.find("remote-card").expect("tag") + 2;
    let hover = client.at("textDocument/hover", &uri, text, offset);
    let plugin_hover = hover_text(&hover);
    assert!(plugin_hover.contains("Plugin component"), "{plugin_hover}");
    assert!(plugin_hover.contains("unavailable"), "{plugin_hover}");
    let definition = client.at("textDocument/definition", &uri, text, offset);
    assert_eq!(definition, Value::Null, "{definition}");

    let text = "<pkg-card />";
    let offset = text.find("pkg-card").expect("tag") + 2;
    let hover = client.at("textDocument/hover", &uri, text, offset);
    assert!(hover_text(&hover).contains("Package component"), "{hover}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn hover_range_is_utf16_after_an_emoji() {
    let root = fixture();
    let uri = page_uri(&root);
    let mut client = Client::start();
    client.initialize(&root);
    client.open(&uri, "<view />");

    let text = "<!-- 😀 --><outer-card count=\"1\" />";
    let offset = text.find("count").expect("attribute");
    let hover = client.at("textDocument/hover", &uri, text, offset + 2);
    assert_eq!(
        hover["range"]["start"],
        json!({"line": 0, "character": 23}),
        "{hover}"
    );
    assert_eq!(
        hover["range"]["end"],
        json!({"line": 0, "character": 28}),
        "{hover}"
    );

    client.shutdown();
    assert!(client.finish().success());
}
