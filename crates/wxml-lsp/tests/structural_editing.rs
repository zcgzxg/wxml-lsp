//! End-to-end tests for structural editing features: formatting, linked
//! editing, folding, selection ranges, document symbols and snippets.
//!
//! Each test starts the real `wxml-lsp --stdio --mock` binary and talks JSON-RPC
//! over `Content-Length` frames. Formatting edits are applied to the source
//! text before they are asserted, so a wrong range fails the test instead of
//! only looking plausible in JSON.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_wxml-lsp");
const TIMEOUT: Duration = Duration::from_secs(10);
const URI: &str = "file:///pages/index/index.wxml";

/// A running server plus its framed stdout channel.
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

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize");
        let stdin = self.stdin.as_mut().expect("stdin is open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        stdin.write_all(&body).expect("body");
        stdin.flush().expect("flush");
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

    fn initialize(&mut self, capabilities: Value) -> Value {
        self.request(
            json!(1),
            "initialize",
            json!({"capabilities": capabilities}),
        );
        let response = self.recv_response(&json!(1));
        assert!(response.get("result").is_some(), "{response}");
        self.notify("initialized", json!({}));
        response
    }

    fn open(&mut self, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": URI,
                    "languageId": "wxml",
                    "version": 1,
                    "text": text,
                },
            }),
        );
    }

    fn change(&mut self, text: &str, version: u64) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": URI, "version": version},
                "contentChanges": [{"text": text}],
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

/// Converts a byte offset into an LSP UTF-16 position.
fn position_at(text: &str, offset: usize) -> Value {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let character = text[line_start..offset].encode_utf16().count();
    json!({"line": line, "character": character})
}

/// Converts an LSP position into a byte offset.
fn offset_at(text: &str, position: &Value) -> usize {
    let line = usize::try_from(position["line"].as_u64().expect("line")).expect("line");
    let character =
        usize::try_from(position["character"].as_u64().expect("character")).expect("character");
    let mut line_start = 0_usize;
    for _ in 0..line {
        let newline = text[line_start..].find('\n').expect("line start");
        line_start += newline + 1;
    }
    let mut units = 0_usize;
    let mut offset = line_start;
    for current in text[line_start..].chars() {
        if current == '\n' || units >= character {
            break;
        }
        let width = current.len_utf16();
        assert!(
            units + width <= character,
            "position splits a surrogate pair"
        );
        units += width;
        offset += current.len_utf8();
    }
    assert_eq!(units, character, "position past the end of the line");
    offset
}

/// Applies formatting edits in descending order and returns the new text.
fn apply_edits(text: &str, edits: &Value) -> String {
    let edits = edits.as_array().expect("edits array");
    let mut ranges = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = &edit["range"];
        let start = offset_at(text, &range["start"]);
        let end = offset_at(text, &range["end"]);
        assert!(start <= end, "{range}");
        assert!(end <= text.len(), "{range}");
        ranges.push((start, end, edit["newText"].as_str().expect("newText")));
    }
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0_usize;
    for (start, end, replacement) in ranges {
        assert!(start >= cursor, "overlapping edits");
        output.push_str(&text[cursor..start]);
        output.push_str(replacement);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output
}

/// Extracts the byte range of a serialized LSP range.
fn byte_range(text: &str, range: &Value) -> (usize, usize) {
    (
        offset_at(text, &range["start"]),
        offset_at(text, &range["end"]),
    )
}

/// Walks the nested parent chain of a serialized `SelectionRange`.
fn chain_ranges(text: &str, value: &Value) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut current = value;
    loop {
        ranges.push(byte_range(text, &current["range"]));
        let Some(parent) = current.get("parent") else {
            break;
        };
        current = parent;
    }
    ranges
}

fn snippet_capabilities() -> Value {
    json!({
        "textDocument": {
            "completion": {
                "completionItem": {"snippetSupport": true},
            },
            "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
            "foldingRange": {"lineFoldingOnly": true},
        },
    })
}

#[test]
fn initialize_declares_local_editing_and_structure_capabilities() {
    let mut client = Client::start();
    let response = client.initialize(snippet_capabilities());
    let capabilities = &response["result"]["capabilities"];
    for provider in [
        "documentFormattingProvider",
        "documentRangeFormattingProvider",
        "linkedEditingRangeProvider",
        "foldingRangeProvider",
        "selectionRangeProvider",
        "documentSymbolProvider",
    ] {
        assert_eq!(capabilities[provider], json!(true), "{provider}");
    }
    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn document_formatting_is_faithful_idempotent_and_crlf_aware() {
    let source = "<view   class=\"中文😀\"   id='a'   >\r\n  <text>hi &amp; {{ name }}</text>\r\n  <!-- 注释 -->\r\n  <wxs module=\"m\">var x = 1 < 2;</wxs>\r\n  <view class=\"x\"\r\n";
    let expected = "<view class=\"中文😀\" id='a'>\r\n  <text>hi &amp; {{ name }}</text>\r\n  <!-- 注释 -->\r\n  <wxs module=\"m\">var x = 1 < 2;</wxs>\r\n  <view class=\"x\"\r\n";

    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(source);
    client.request(
        json!(2),
        "textDocument/formatting",
        json!({
            "textDocument": {"uri": URI},
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let response = client.recv_response(&json!(2));
    let edits = &response["result"];
    assert!(edits.is_array(), "{response}");
    assert!(!edits.as_array().expect("edits").is_empty());
    let formatted = apply_edits(source, edits);
    assert_eq!(formatted, expected);
    assert!(formatted.contains("中文😀"));
    assert!(formatted.contains("&amp;"));
    assert!(formatted.contains("{{ name }}"));
    assert!(formatted.contains("<!-- 注释 -->"));
    assert!(formatted.contains("var x = 1 < 2;"));

    client.change(&formatted, 2);
    client.request(
        json!(3),
        "textDocument/formatting",
        json!({
            "textDocument": {"uri": URI},
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let again = client.recv_response(&json!(3));
    assert_eq!(again["result"], json!([]), "{again}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn range_formatting_respects_the_requested_boundary() {
    let source = "<view   a=\"1\"   >\n  <text   b=\"2\"   >x</text>\n</view>";
    let second_line_start = source.find("  <text").expect("text line");
    let second_line_end = source[second_line_start..]
        .find('\n')
        .map_or(source.len(), |offset| second_line_start + offset);

    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(source);

    client.request(
        json!(2),
        "textDocument/rangeFormatting",
        json!({
            "textDocument": {"uri": URI},
            "range": {
                "start": position_at(source, second_line_start),
                "end": position_at(source, second_line_end),
            },
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let response = client.recv_response(&json!(2));
    let formatted = apply_edits(source, &response["result"]);
    assert!(formatted.starts_with("<view   a=\"1\"   >"), "{formatted}");
    assert!(formatted.contains("<text b=\"2\">x</text>"), "{formatted}");

    // A range inside one tag must not edit that tag.
    client.request(
        json!(3),
        "textDocument/rangeFormatting",
        json!({
            "textDocument": {"uri": URI},
            "range": {
                "start": position_at(source, 2),
                "end": position_at(source, 8),
            },
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let inside = client.recv_response(&json!(3));
    assert_eq!(inside["result"], json!([]), "{inside}");

    // An out-of-bounds line is rejected without touching the document.
    client.request(
        json!(4),
        "textDocument/rangeFormatting",
        json!({
            "textDocument": {"uri": URI},
            "range": {
                "start": {"line": 99, "character": 0},
                "end": {"line": 99, "character": 1},
            },
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let out_of_bounds = client.recv_response(&json!(4));
    assert_eq!(out_of_bounds["result"], json!([]), "{out_of_bounds}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn linked_editing_pairs_tags_without_crossing_nested_siblings() {
    let text = "<view><view>x</view></view>";
    let mut client = Client::start();
    client.initialize(json!({}));
    client.open(text);

    client.request(
        json!(2),
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(text, 2),
        }),
    );
    let outer = client.recv_response(&json!(2));
    let ranges = outer["result"]["ranges"].as_array().expect("ranges");
    assert_eq!(ranges.len(), 2);
    assert_eq!(byte_range(text, &ranges[0]), (1, 5));
    assert_eq!(byte_range(text, &ranges[1]), (22, 26));

    client.request(
        json!(3),
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(text, 16),
        }),
    );
    let inner = client.recv_response(&json!(3));
    let ranges = inner["result"]["ranges"].as_array().expect("ranges");
    assert_eq!(byte_range(text, &ranges[0]), (7, 11));
    assert_eq!(byte_range(text, &ranges[1]), (15, 19));

    client.request(
        json!(4),
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(text, 6),
        }),
    );
    let outside = client.recv_response(&json!(4));
    assert_eq!(outside["result"], Value::Null, "{outside}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn folding_range_covers_elements_comments_and_wxs() {
    let text = "<view\n  class=\"a\">\n  <text>x</text>\n  <!-- a\n  b -->\n  <wxs module=\"m\">\nvar x = 1;\n</wxs>\n</view>";
    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(text);
    client.request(
        json!(2),
        "textDocument/foldingRange",
        json!({"textDocument": {"uri": URI}}),
    );
    let response = client.recv_response(&json!(2));
    let folds = response["result"].as_array().expect("folds");
    let actual: Vec<(u64, u64, &str)> = folds
        .iter()
        .map(|fold| {
            assert!(fold.get("startCharacter").is_none(), "{fold}");
            assert!(fold.get("endCharacter").is_none(), "{fold}");
            (
                fold["startLine"].as_u64().expect("startLine"),
                fold["endLine"].as_u64().expect("endLine"),
                fold["kind"].as_str().expect("kind"),
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            (0, 8, "region"),
            (0, 1, "region"),
            (3, 4, "comment"),
            (5, 7, "region"),
        ]
    );
    assert!(
        folds
            .iter()
            .all(|fold| { fold["startLine"].as_u64() < fold["endLine"].as_u64() })
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn selection_range_returns_ordered_strict_parent_chains() {
    let text = "<view class=\"a\">\n  {{ user.name }}\n  hello\n</view>";
    let mustache = text.find("user").expect("user") + 2;
    let value = text.find("\"a\"").expect("value") + 1;
    let content = text.find("hello").expect("hello");
    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(text);
    client.request(
        json!(2),
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": URI},
            "positions": [
                position_at(text, mustache),
                position_at(text, value),
                position_at(text, content),
            ],
        }),
    );
    let response = client.recv_response(&json!(2));
    let values = response["result"].as_array().expect("selection ranges");
    assert_eq!(values.len(), 3, "{response}");

    let mustache_chain = chain_ranges(text, &values[0]);
    assert_eq!(&text[mustache_chain[0].0..mustache_chain[0].1], "user.name");
    assert_eq!(
        &text[mustache_chain[1].0..mustache_chain[1].1],
        "{{ user.name }}"
    );
    assert!(mustache_chain.len() >= 3);

    let value_chain = chain_ranges(text, &values[1]);
    assert_eq!(&text[value_chain[0].0..value_chain[0].1], "a");
    assert_eq!(&text[value_chain[1].0..value_chain[1].1], "\"a\"");
    assert_eq!(&text[value_chain[2].0..value_chain[2].1], "class=\"a\"");

    let content_chain = chain_ranges(text, &values[2]);
    assert!(
        text[content_chain[0].0..content_chain[0].1].contains("hello"),
        "{content_chain:?}"
    );
    assert!(content_chain.len() >= 2);

    for chain in [&mustache_chain, &value_chain, &content_chain] {
        for pair in chain.windows(2) {
            assert!(pair[1].0 <= pair[0].0, "{chain:?}");
            assert!(pair[1].1 >= pair[0].1, "{chain:?}");
            assert_ne!(pair[0], pair[1], "{chain:?}");
        }
    }

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn document_symbol_returns_a_hierarchy_when_the_client_supports_it() {
    let text = "<view id=\"page\" class=\"a b\"><text>hi</text><image src=\"x\"/></view>";
    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(text);
    client.request(
        json!(2),
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": URI}}),
    );
    let response = client.recv_response(&json!(2));
    let symbols = response["result"].as_array().expect("symbols");
    assert_eq!(symbols.len(), 1, "{response}");
    let view = &symbols[0];
    assert_eq!(view["name"], json!("view"));
    assert_eq!(view["detail"], json!("#page .a .b"));
    let children = view["children"].as_array().expect("children");
    let names: Vec<&str> = children
        .iter()
        .map(|child| child["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["text", "image"]);
    let range = byte_range(text, &view["range"]);
    let selection = byte_range(text, &view["selectionRange"]);
    assert!(range.0 <= selection.0 && selection.1 <= range.1);

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn document_symbol_falls_back_to_flat_locations_without_hierarchical_support() {
    let text = "<view id=\"page\"><text>hi</text></view>";
    let mut client = Client::start();
    client.initialize(json!({}));
    client.open(text);
    client.request(
        json!(2),
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": URI}}),
    );
    let response = client.recv_response(&json!(2));
    let symbols = response["result"].as_array().expect("symbols");
    assert_eq!(symbols.len(), 2, "{response}");
    assert_eq!(symbols[0]["name"], json!("view"));
    assert_eq!(symbols[1]["name"], json!("text"));
    assert_eq!(symbols[1]["containerName"], json!("view"));
    assert_eq!(symbols[0]["location"]["uri"], json!(URI));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn completion_snippets_are_gated_on_client_support() {
    let text = "<vi";
    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(text);
    client.request(
        json!(2),
        "textDocument/completion",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(text, text.len()),
        }),
    );
    let response = client.recv_response(&json!(2));
    let items = response["result"]["items"].as_array().expect("items");
    let snippet = items
        .iter()
        .find(|item| item["label"] == json!("view (snippet)"))
        .expect("view snippet");
    assert_eq!(snippet["insertTextFormat"], json!(2));
    let new_text = snippet["textEdit"]["newText"].as_str().expect("newText");
    assert!(new_text.starts_with("view>"), "{new_text}");
    assert!(new_text.contains("$0"), "{new_text}");
    assert!(new_text.contains("</view>"), "{new_text}");

    let attribute_text = "<view wx:f";
    client.change(attribute_text, 2);
    client.request(
        json!(3),
        "textDocument/completion",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(attribute_text, attribute_text.len()),
        }),
    );
    let response = client.recv_response(&json!(3));
    let items = response["result"]["items"].as_array().expect("items");
    let snippet = items
        .iter()
        .find(|item| item["label"] == json!("wx:for (snippet)"))
        .expect("wx:for snippet");
    assert_eq!(snippet["insertTextFormat"], json!(2));
    assert!(
        snippet["textEdit"]["newText"]
            .as_str()
            .expect("newText")
            .starts_with("wx:for=\"{{")
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn clients_without_snippet_support_receive_plain_items_only() {
    let text = "<vi";
    let mut client = Client::start();
    client.initialize(json!({}));
    client.open(text);
    client.request(
        json!(2),
        "textDocument/completion",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(text, text.len()),
        }),
    );
    let response = client.recv_response(&json!(2));
    let items = response["result"]["items"].as_array().expect("items");
    assert!(!items.is_empty(), "{response}");
    assert!(
        items
            .iter()
            .all(|item| item["insertTextFormat"] != json!(2)),
        "{response}"
    );

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn selection_range_rejects_too_many_positions() {
    let text = "<view>x</view>";
    let mut client = Client::start();
    client.initialize(json!({}));
    client.open(text);
    let positions: Vec<Value> = (0..33).map(|_| position_at(text, 6)).collect();
    client.request(
        json!(2),
        "textDocument/selectionRange",
        json!({"textDocument": {"uri": URI}, "positions": positions}),
    );
    let response = client.recv_response(&json!(2));
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    client.shutdown();
    assert!(client.finish().success());
}
