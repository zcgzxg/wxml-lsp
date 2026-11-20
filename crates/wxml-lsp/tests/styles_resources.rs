#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

//! JSON-RPC integration tests for class/resource completion, hover,
//! definition and document links.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::run_with_host;
use wxml_text::LineIndex;
use wxml_types::{Backend, MockBackend};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new() -> Self {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let root = base.join(format!("wxml-lsp-styles-{stamp}-{id}"));
        std::fs::create_dir_all(&root).expect("create temp project");
        Self { root }
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(target, text).expect("write fixture");
    }

    fn uri(&self, path: &str) -> String {
        format!("file://{}", self.root.join(path).display())
    }

    fn root_uri(&self) -> String {
        format!("file://{}", self.root.display())
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fixture() -> TempProject {
    let project = TempProject::new();
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write(
        "app.wxss",
        "/* global doc */\n.app-global { color: black; }\n",
    );
    project.write(
        "shared/shared.scss",
        "/* shared doc */\n.shared { color: red; }\n.card { color: orange; }\n",
    );
    project.write(
        "pages/index/index.wxss",
        "@import \"../../shared/shared\";\n.page { color: red; }\n/* card doc */\n.card {\n  &-header { color: blue; }\n}\n",
    );
    project.write("pages/index/index.ts", "Page({})");
    project.write("images/logo.png", "png");
    project.write("images/my logo.png", "png");
    std::fs::write(project.root.join("images/bin.png"), [0xff, 0xfe, 0x00]).expect("binary image");
    project.write("styles/extra.wxss", ".configured { color: gray; }\n");
    project
}

const fn wxml_text() -> &'static str {
    "<!-- 😀 中文 -->\r\n<view class=\"card \">\r\n  <image src=\"../../images/logo.png\" />\r\n  <image src=\"../../images/my logo.png\" />\r\n  <image src=\"../../images/bin.png\" />\r\n  <image src=\"../../images/lo\" />\r\n  <image src=\"https://example.com/a.png\" />\r\n  <image src=\"../../images/missing.png\" />\r\n  <navigator url=\"../../images/logo.png\" />\r\n</view>\r\n"
}

fn completion_offset(text: &str) -> usize {
    text.find("class=\"card ").expect("class attribute") + "class=\"card ".len()
}

fn byte_position(text: &str, byte: usize) -> Value {
    let index = LineIndex::new(text);
    let position = index.position(byte).expect("byte offset is a position");
    json!({"line": position.line, "character": position.character})
}

fn value_range(text: &str, start: usize, end: usize) -> Value {
    json!({"start": byte_position(text, start), "end": byte_position(text, end)})
}

struct Session {
    messages: Vec<Value>,
}

fn frame(value: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).expect("serialize");
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(&body);
    bytes
}

fn run_session(_project: &TempProject, messages: &[Value]) -> Session {
    let mut input = Vec::new();
    for message in messages {
        input.extend(frame(message));
    }
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = SharedWriter(Arc::clone(&output));
    let host = Arc::new(ProjectState::new());
    let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
    run_with_host(Cursor::new(input), writer, backend, host).expect("run server");
    let bytes = output.lock().expect("output").clone();
    Session {
        messages: parse_frames(&bytes),
    }
}

struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("output").extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn parse_frames(bytes: &[u8]) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut index = 0_usize;
    while index < bytes.len() {
        let Some(header_end) = bytes[index..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        else {
            break;
        };
        let header = std::str::from_utf8(&bytes[index..index + header_end]).expect("header");
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .expect("content length");
        let body_start = index + header_end + 4;
        let body_end = body_start + length;
        let value = serde_json::from_slice(&bytes[body_start..body_end]).expect("json frame");
        messages.push(value);
        index = body_end;
    }
    messages
}

impl Session {
    fn response(&self, id: &Value) -> &Value {
        self.messages
            .iter()
            .find(|message| message.get("id") == Some(id) && message.get("method").is_none())
            .unwrap_or_else(|| panic!("missing response {id}: {:?}", self.messages))
    }

    fn result(&self, id: &Value) -> &Value {
        let response = self.response(id);
        response
            .get("result")
            .unwrap_or_else(|| panic!("missing result: {response}"))
    }
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn response(id: i64, method: &str, params: Value) -> Value {
    request(id, method, params)
}

fn document_position(text: &str, uri: &str, byte: usize) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": byte_position(text, byte),
    })
}

fn completion_at(uri: &str, text: &str, byte: usize) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": byte_position(text, byte),
    })
}

fn labels(result: &Value) -> Vec<String> {
    result["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["label"].as_str().map(str::to_owned))
        .collect()
}

fn item<'a>(result: &'a Value, label: &str) -> &'a Value {
    result["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["label"] == json!(label))
        .unwrap_or_else(|| panic!("missing item {label}: {result}"))
}

#[test]
fn class_completion_hover_and_definition_use_project_styles() {
    let project = fixture();
    let uri = project.uri("pages/index/index.wxml");
    let text = wxml_text();
    let act = completion_offset(text);
    let card = text.find("card").expect("card") + 2;

    let messages = vec![
        request(
            1,
            "initialize",
            json!({
                "rootUri": project.root_uri(),
                "capabilities": {},
                "initializationOptions": {
                    "wxml": {"globalStyles": ["styles/extra.wxss"]}
                },
            }),
        ),
        notification("initialized", json!({})),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                }
            }),
        ),
        response(2, "textDocument/completion", completion_at(&uri, text, act)),
        response(3, "textDocument/hover", document_position(text, &uri, card)),
        response(
            4,
            "textDocument/definition",
            document_position(text, &uri, card),
        ),
        request(99, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ];
    let session = run_session(&project, &messages);

    let capabilities = session.result(&json!(1));
    assert_eq!(
        capabilities["capabilities"]["documentLinkProvider"]["resolveProvider"],
        json!(false)
    );
    assert_eq!(
        capabilities["capabilities"]["definitionProvider"],
        json!(true)
    );

    let completion = session.result(&json!(2));
    let labels = labels(completion);
    assert!(labels.contains(&"card-header".to_owned()), "{labels:?}");
    assert!(labels.contains(&"shared".to_owned()), "{labels:?}");
    assert!(labels.contains(&"app-global".to_owned()), "{labels:?}");
    assert!(labels.contains(&"configured".to_owned()), "{labels:?}");
    assert!(
        !labels.contains(&"card".to_owned()),
        "existing class: {labels:?}"
    );
    let header = item(completion, "card-header");
    assert_eq!(header["textEdit"]["range"], value_range(text, act, act));
    assert!(
        header["textEdit"]["newText"] == json!("card-header"),
        "{header}"
    );

    let hover = session.result(&json!(3));
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(contents.contains("WXSS class"), "{contents}");
    assert!(contents.contains("card doc"), "{contents}");
    assert!(!contents.contains("pages/index/index.wxss"), "{contents}");
    assert!(!contents.contains("style index is partial"), "{contents}");
    assert_eq!(hover["range"], value_range(text, card - 2, card + 2));

    let definition = session.result(&json!(4));
    let locations = definition.as_array().expect("locations");
    assert!(!locations.is_empty(), "{definition}");
    let page_location = locations
        .iter()
        .find(|location| {
            location["uri"]
                .as_str()
                .is_some_and(|uri| uri.ends_with("pages/index/index.wxss"))
        })
        .expect("page style location");
    let style_text =
        std::fs::read_to_string(project.root.join("pages/index/index.wxss")).expect("style text");
    let style_card = style_text.find(".card").expect("card selector") + 1;
    assert_eq!(
        page_location["range"],
        value_range(&style_text, style_card, style_card + 4)
    );
    assert!(
        locations.iter().any(|location| location["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("shared/shared.scss"))),
        "{definition}"
    );
}

#[test]
fn class_hover_keeps_hyphens_unescaped() {
    let project = fixture();
    project.write(
        "pages/index/index.wxss",
        ".right-fx-input { color: red; }\n",
    );
    let uri = project.uri("pages/index/index.wxml");
    let text = "<view class=\"right-fx-input\"></view>";
    let class = text.find("right-fx-input").expect("class");

    let messages = vec![
        request(
            21,
            "initialize",
            json!({"rootUri": project.root_uri(), "capabilities": {}}),
        ),
        notification("initialized", json!({})),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                }
            }),
        ),
        response(
            22,
            "textDocument/hover",
            document_position(text, &uri, class),
        ),
        request(23, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ];
    let session = run_session(&project, &messages);

    let hover = session.result(&json!(22));
    let contents = hover["contents"]["value"].as_str().expect("hover text");
    assert!(
        contents.contains("`right-fx-input` — WXSS class"),
        "{contents}"
    );
    assert!(!contents.contains("right\\-fx\\-input"), "{contents}");
}

#[test]
fn resource_links_completion_and_definition_are_local_only() {
    let project = fixture();
    let uri = project.uri("pages/index/index.wxml");
    let text = wxml_text();
    let typed = text.find("../../images/lo\"").expect("typed") + "../../images/".len();
    let logo = text.find("../../images/logo.png").expect("logo") + 2;
    let binary = text.find("../../images/bin.png").expect("binary") + 2;

    let messages = vec![
        request(
            10,
            "initialize",
            json!({"rootUri": project.root_uri(), "capabilities": {}}),
        ),
        notification("initialized", json!({})),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                }
            }),
        ),
        response(
            11,
            "textDocument/documentLink",
            json!({"textDocument": {"uri": uri}}),
        ),
        response(
            12,
            "textDocument/completion",
            completion_at(&uri, text, typed),
        ),
        response(
            13,
            "textDocument/definition",
            document_position(text, &uri, logo),
        ),
        response(
            14,
            "textDocument/definition",
            document_position(text, &uri, binary),
        ),
        request(99, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ];
    let session = run_session(&project, &messages);

    let links = session.result(&json!(11)).as_array().expect("links");
    let targets: Vec<_> = links
        .iter()
        .filter_map(|link| link["target"].as_str())
        .collect();
    assert_eq!(targets.len(), 3, "{links:?}");
    assert!(
        targets
            .iter()
            .any(|target| target.ends_with("/images/logo.png")),
        "{targets:?}"
    );
    assert!(
        targets
            .iter()
            .any(|target| target.contains("images/my%20logo.png")),
        "space must be percent-encoded: {targets:?}"
    );
    assert!(
        links
            .iter()
            .all(|link| !link["target"].as_str().unwrap_or_default().contains("http")),
        "remote URLs must not become links: {links:?}"
    );
    let logo_link = links
        .iter()
        .find(|link| {
            link["target"]
                .as_str()
                .is_some_and(|target| target.ends_with("/images/logo.png"))
        })
        .expect("logo link");
    let value_start = text.find("../../images/logo.png").expect("logo value");
    assert_eq!(
        logo_link["range"],
        value_range(
            text,
            value_start,
            value_start + "../../images/logo.png".len()
        )
    );

    let completion = session.result(&json!(12));
    let labels = labels(completion);
    assert!(labels.contains(&"logo.png".to_owned()), "{labels:?}");
    assert!(labels.contains(&"my logo.png".to_owned()), "{labels:?}");
    let logo_item = item(completion, "logo.png");
    let value_start = text.find("../../images/lo\"").expect("typed");
    let value_end = value_start + "../../images/lo".len();
    assert_eq!(
        logo_item["textEdit"]["range"],
        value_range(text, value_start, value_end)
    );
    assert_eq!(
        logo_item["textEdit"]["newText"],
        json!("../../images/logo.png")
    );

    let definition = session.result(&json!(13));
    let location = definition
        .as_array()
        .expect("locations")
        .first()
        .expect("resource location");
    assert!(
        location["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("/images/logo.png")),
        "{definition}"
    );
    assert_eq!(
        location["range"]["start"],
        json!({"line": 0, "character": 0})
    );
}

#[test]
fn unsaved_style_overlays_and_watched_files_refresh_classes() {
    let project = fixture();
    let wxml_uri = project.uri("pages/index/index.wxml");
    let style_uri = project.uri("pages/index/index.wxss");
    let text = wxml_text();
    let act = completion_offset(text);

    let messages = vec![
        request(
            20,
            "initialize",
            json!({"rootUri": project.root_uri(), "capabilities": {}}),
        ),
        notification("initialized", json!({})),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": wxml_uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                }
            }),
        ),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": style_uri,
                    "languageId": "wxss",
                    "version": 1,
                    "text": ".unsaved { color: red; }\n",
                }
            }),
        ),
        notification(
            "workspace/didChangeWatchedFiles",
            json!({"changes": [{"uri": style_uri, "type": 2}]}),
        ),
        response(
            21,
            "textDocument/completion",
            completion_at(&wxml_uri, text, act),
        ),
        notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": style_uri, "version": 2},
                "contentChanges": [{"text": ".changed { color: blue; }\n"}],
            }),
        ),
        response(
            22,
            "textDocument/completion",
            completion_at(&wxml_uri, text, act),
        ),
        notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": style_uri}}),
        ),
        response(
            23,
            "textDocument/completion",
            completion_at(&wxml_uri, text, act),
        ),
        request(99, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ];
    let session = run_session(&project, &messages);

    let unsaved = labels(session.result(&json!(21)));
    assert!(unsaved.contains(&"unsaved".to_owned()), "{unsaved:?}");
    assert!(!unsaved.contains(&"page".to_owned()), "{unsaved:?}");

    let changed = labels(session.result(&json!(22)));
    assert!(changed.contains(&"changed".to_owned()), "{changed:?}");
    assert!(!changed.contains(&"unsaved".to_owned()), "{changed:?}");

    let closed = labels(session.result(&json!(23)));
    assert!(closed.contains(&"page".to_owned()), "{closed:?}");
    assert!(!closed.contains(&"changed".to_owned()), "{closed:?}");

    // A watched style change removes the overlay and re-reads disk.
    std::fs::write(
        project.root.join("pages/index/index.wxss"),
        ".disk { color: green; }\n",
    )
    .expect("write disk style");
    let messages = vec![
        request(
            25,
            "initialize",
            json!({"rootUri": project.root_uri(), "capabilities": {}}),
        ),
        notification("initialized", json!({})),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": wxml_uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                }
            }),
        ),
        notification(
            "workspace/didChangeWatchedFiles",
            json!({"changes": [{"uri": style_uri, "type": 2}]}),
        ),
        response(
            26,
            "textDocument/completion",
            completion_at(&wxml_uri, text, act),
        ),
        request(98, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ];
    let session = run_session(&project, &messages);
    let watched = labels(session.result(&json!(26)));
    assert!(watched.contains(&"disk".to_owned()), "{watched:?}");
    assert!(!watched.contains(&"page".to_owned()), "{watched:?}");
}

#[test]
fn unknown_and_dynamic_classes_are_not_invented() {
    let project = fixture();
    let uri = project.uri("pages/index/index.wxml");
    let text = "<view class=\"unknown\"></view><view class=\"{{dynamic}}\"></view>";
    let unknown = text.find("unknown").expect("unknown") + 2;
    let messages = vec![
        request(
            30,
            "initialize",
            json!({"rootUri": project.root_uri(), "capabilities": {}}),
        ),
        notification("initialized", json!({})),
        notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                }
            }),
        ),
        response(
            31,
            "textDocument/hover",
            document_position(text, &uri, unknown),
        ),
        response(
            32,
            "textDocument/definition",
            document_position(text, &uri, unknown),
        ),
        request(99, "shutdown", Value::Null),
        notification("exit", Value::Null),
    ];
    let session = run_session(&project, &messages);
    assert_eq!(session.result(&json!(31)), &Value::Null);
    assert_eq!(session.result(&json!(32)), &json!([]));
}
