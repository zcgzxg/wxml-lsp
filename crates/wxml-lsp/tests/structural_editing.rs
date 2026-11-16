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

mod common;

use common::TempProject;

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
        self.initialize_with(capabilities, None, None)
    }

    fn initialize_with(
        &mut self,
        capabilities: Value,
        root_uri: Option<&str>,
        initialization_options: Option<Value>,
    ) -> Value {
        let mut params = json!({"capabilities": capabilities});
        if let Some(root_uri) = root_uri {
            params["rootUri"] = json!(root_uri);
        }
        if let Some(options) = initialization_options {
            params["initializationOptions"] = options;
        }
        self.request(json!(1), "initialize", params);
        let response = self.recv_response(&json!(1));
        assert!(response.get("result").is_some(), "{response}");
        self.notify("initialized", json!({}));
        response
    }

    fn open(&mut self, text: &str) {
        self.open_uri(URI, "wxml", text);
    }

    fn open_uri(&mut self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
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

    fn completion_at(&mut self, id: Value, version: u64, text: &str, offset: usize) -> Vec<Value> {
        self.change(text, version);
        self.request(
            id.clone(),
            "textDocument/completion",
            json!({
                "textDocument": {"uri": URI},
                "position": position_at(text, offset),
            }),
        );
        let response = self.recv_response(&id);
        response["result"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
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
    let mut content = &text[line_start..offset];
    // LSP columns exclude the `\r` of a CRLF line ending.
    if let Some(stripped) = content.strip_suffix('\r') {
        content = stripped;
    }
    let character = content.encode_utf16().count();
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

/// Expands the simple `$n` / `${n:default}` placeholders used by snippets.
fn expand_snippet(body: &str) -> String {
    let mut output = String::new();
    let mut chars = body.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '$' {
            output.push(character);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();
                let mut inner = String::new();
                for next in chars.by_ref() {
                    if next == '}' {
                        break;
                    }
                    inner.push(next);
                }
                output.push_str(inner.split_once(':').map_or("", |(_, value)| value));
            }
            Some(next) if next.is_ascii_digit() => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            _ => output.push('$'),
        }
    }
    output
}

/// Applies one completion item's `textEdit` after expanding snippet
/// placeholders, and returns the resulting document.
fn apply_completion_item(text: &str, item: &Value) -> String {
    let edit = &item["textEdit"];
    let range = &edit["range"];
    let (start, end) = byte_range(text, range);
    let new_text = edit["newText"].as_str().expect("newText");
    let mut output = String::with_capacity(text.len());
    output.push_str(&text[..start]);
    output.push_str(&expand_snippet(new_text));
    output.push_str(&text[end..]);
    output
}

/// Asserts that the result parses without errors or truncation.
fn assert_parses_cleanly(text: &str) {
    let parsed = wxml_syntax::parse(text);
    assert!(
        parsed.errors.is_empty() && !parsed.is_truncated(),
        "invalid WXML after edit: {text:?} ({:?})",
        parsed.errors
    );
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
    let source = "<view   class=\"中文😀\"   id='a'   >\r\n  <text>hi &amp; {{ name }}</text>\r\n  <!-- 注释 -->\r\n  <wxs module=\"m\">var x = 1 < 2;</wxs>\r\n</view>\r\n";
    let expected = "<view class=\"中文😀\" id='a'>\r\n  <text>hi &amp; {{ name }}</text>\r\n  <!-- 注释 -->\r\n  <wxs module=\"m\">var x = 1 < 2;</wxs>\r\n</view>\r\n";

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
fn formatting_refuses_documents_with_parse_errors() {
    let source = "<view   a=\"1\"   ><text>ok</text><view class=\"x\"";
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
    assert_eq!(response["result"], json!([]), "{response}");

    client.request(
        json!(3),
        "textDocument/rangeFormatting",
        json!({
            "textDocument": {"uri": URI},
            "range": {
                "start": position_at(source, 0),
                "end": position_at(source, 18),
            },
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let response = client.recv_response(&json!(3));
    assert_eq!(response["result"], json!([]), "{response}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn range_formatting_handles_crlf_and_emoji_only_inside_the_range() {
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
fn range_formatting_on_crlf_and_emoji_changes_only_the_target_tag() {
    let source = "<view   a=\"😀\"   >\r\n  <text   b=\"中文\"   >x</text>\r\n</view>";
    let line_start = source.find("  <text").expect("text line");
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |offset| line_start + offset);
    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open(source);
    client.request(
        json!(2),
        "textDocument/rangeFormatting",
        json!({
            "textDocument": {"uri": URI},
            "range": {
                "start": position_at(source, line_start),
                "end": position_at(source, line_end),
            },
            "options": {"tabSize": 2, "insertSpaces": true},
        }),
    );
    let response = client.recv_response(&json!(2));
    let formatted = apply_edits(source, &response["result"]);
    assert!(formatted.starts_with("<view   a=\"😀\"   >"), "{formatted}");
    assert!(
        formatted.contains("<text b=\"中文\">x</text>"),
        "{formatted}"
    );
    assert_parses_cleanly(&formatted);

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
#[allow(clippy::too_many_lines)] // One table-driven regression matrix.
fn snippet_edits_apply_to_real_documents_without_breaking_wxml() {
    struct Case {
        text: &'static str,
        offset: usize,
        label: Option<&'static str>,
        expected: Option<&'static str>,
        parse_clean: bool,
        forbidden: &'static [&'static str],
    }

    let cases = [
        Case {
            text: "<vi",
            offset: 3,
            label: Some("view (snippet)"),
            expected: Some("<view>\n  \n</view>"),
            parse_clean: true,
            forbidden: &[],
        },
        Case {
            text: "<vi>",
            offset: 3,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["view (snippet)"],
        },
        Case {
            text: "<vi>text</vi>",
            offset: 3,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["view (snippet)"],
        },
        Case {
            text: "<view>x</view>",
            offset: 3,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["view (snippet)"],
        },
        Case {
            text: "<view class=\"a\"",
            offset: 5,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["view (snippet)"],
        },
        Case {
            text: "<view class=\"a\" >x</view>",
            offset: 16,
            label: Some("wx:for (snippet)"),
            expected: Some("<view class=\"a\" wx:for=\"{{items}}\" wx:key=\"index\">x</view>"),
            parse_clean: true,
            forbidden: &[],
        },
        Case {
            text: "<view class=\"a\">x</view>",
            offset: 15,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["wx:for (snippet)"],
        },
        Case {
            text: "<text>x</text>",
            offset: 5,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["text (snippet)"],
        },
        Case {
            text: "<view wx:f=\"\">x</view>",
            offset: 10,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["wx:for (snippet)"],
        },
        Case {
            text: "<view wx:f=\"\">x</view>",
            offset: 12,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["wx:for (snippet)", "wx:if (snippet)"],
        },
        Case {
            text: "<view wx:f bindtap=\"x\">x</view>",
            offset: 10,
            label: Some("wx:for (snippet)"),
            expected: Some("<view wx:for=\"{{items}}\" wx:key=\"index\" bindtap=\"x\">x</view>"),
            parse_clean: true,
            forbidden: &["bindtap (snippet)"],
        },
        Case {
            text: "<view wx:for-item=\"x\" wx:key=\"id\" wx:f>x</view>",
            offset: 38,
            label: Some("wx:for (snippet)"),
            expected: Some("<view wx:for-item=\"x\" wx:key=\"id\" wx:for=\"{{items}}\">x</view>"),
            parse_clean: true,
            forbidden: &[],
        },
        Case {
            text: "<view wx:for=\"{{x}}\" wx:key=\"id\" wx:f>x</view>",
            offset: 35,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["wx:for (snippet)"],
        },
        Case {
            text: "<!--x-->\r\n  <vi",
            offset: 15,
            label: Some("view (snippet)"),
            expected: Some("<!--x-->\r\n  <view>\r\n    \r\n  </view>"),
            parse_clean: true,
            forbidden: &[],
        },
        Case {
            text: "<wxs ></wxs>",
            offset: 5,
            label: None,
            expected: None,
            parse_clean: false,
            forbidden: &["wx:for (snippet)", "wx:if (snippet)", "bindtap (snippet)"],
        },
    ];

    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open("");
    for (index, case) in cases.iter().enumerate() {
        let version = u64::try_from(index).expect("index") + 2;
        let id = json!(10 + index);
        let items = client.completion_at(id, version, case.text, case.offset);
        for forbidden in case.forbidden {
            assert!(
                items.iter().all(|item| item["label"] != json!(forbidden)),
                "case {index} ({:?}) offered {forbidden}",
                case.text
            );
        }
        match case.label {
            Some(label) => {
                let snippet = items
                    .iter()
                    .find(|item| item["label"] == json!(label))
                    .unwrap_or_else(|| {
                        panic!("case {index} ({:?}) did not offer {label}", case.text)
                    });
                assert_eq!(snippet["insertTextFormat"], json!(2), "case {index}");
                let output = apply_completion_item(case.text, snippet);
                if let Some(expected) = case.expected {
                    assert_eq!(output, expected, "case {index}");
                }
                if case.parse_clean {
                    assert_parses_cleanly(&output);
                }
            }
            None => {
                assert!(
                    items
                        .iter()
                        .all(|item| item["insertTextFormat"] != json!(2)),
                    "case {index} ({:?}) offered a snippet",
                    case.text
                );
            }
        }
    }

    // The unsafe `<vi|>` state still has the plain `view` fallback and the
    // replacement must not add a second `>`.
    let items = client.completion_at(json!(90), 40, "<vi>", 3);
    let plain = items
        .iter()
        .find(|item| item["label"] == json!("view"))
        .expect("plain view item");
    assert_eq!(plain["insertTextFormat"], json!(1));
    let output = apply_completion_item("<vi>", plain);
    assert_eq!(output, "<view>");
    assert!(!output.contains(">>"));

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn workspace_snippets_load_override_and_reload_without_restart() {
    let project = TempProject::new("wxml-snippet-config");
    let config = project.root().join(".wxml-lsp/custom-snippets.json");
    std::fs::create_dir_all(config.parent().expect("parent")).expect("dir");
    std::fs::write(
        &config,
        r#"{
            "snippets": [
                {"id": "tag.view", "label": "view (custom)", "prefix": "view",
                 "body": "view class=\"card\">$0</view>"}
            ]
        }"#,
    )
    .expect("write snippets");

    let mut client = Client::start();
    client.initialize_with(
        snippet_capabilities(),
        Some(&project.root_uri()),
        Some(json!({"snippetsPath": ".wxml-lsp/custom-snippets.json"})),
    );
    client.open("<vi");
    let items = client.completion_at(json!(2), 2, "<vi", 3);
    assert!(
        items
            .iter()
            .all(|item| item["label"] != json!("view (snippet)")),
        "{items:?}"
    );
    let custom = items
        .iter()
        .find(|item| item["label"] == json!("view (custom)"))
        .expect("custom snippet");
    let output = apply_completion_item("<vi", custom);
    assert_eq!(output, "<view class=\"card\"></view>");
    assert_parses_cleanly(&output);

    std::fs::write(
        &config,
        r#"{
            "snippets": [
                {"id": "tag.view", "label": "view (custom)", "prefix": "view",
                 "body": "view class=\"second\">$0</view>"}
            ]
        }"#,
    )
    .expect("rewrite");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": format!("file://{}", config.display()), "type": 2}]}),
    );
    let items = client.completion_at(json!(3), 3, "<vi", 3);
    let custom = items
        .iter()
        .find(|item| item["label"] == json!("view (custom)"))
        .expect("reloaded snippet");
    assert_eq!(
        apply_completion_item("<vi", custom),
        "<view class=\"second\"></view>"
    );

    std::fs::write(&config, r#"{"enabled": false}"#).expect("disable");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": format!("file://{}", config.display()), "type": 2}]}),
    );
    let items = client.completion_at(json!(4), 4, "<vi", 3);
    assert!(
        items
            .iter()
            .all(|item| item["insertTextFormat"] != json!(2)),
        "{items:?}"
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
fn selection_range_handles_many_positions_and_bounds_the_request() {
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
    let values = response["result"].as_array().expect("selection ranges");
    assert_eq!(values.len(), 33, "{response}");

    let positions: Vec<Value> = (0..257).map(|_| position_at(text, 6)).collect();
    client.request(
        json!(3),
        "textDocument/selectionRange",
        json!({"textDocument": {"uri": URI}, "positions": positions}),
    );
    let response = client.recv_response(&json!(3));
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn non_wxml_documents_get_empty_structural_results() {
    let text = "<view>x</view>";
    let uri = "file:///project/pages/index.ts";
    let mut client = Client::start();
    client.initialize(snippet_capabilities());
    client.open_uri(uri, "typescript", text);

    for (id, method, params) in [
        (
            json!(2),
            "textDocument/formatting",
            json!({
                "textDocument": {"uri": uri},
                "options": {"tabSize": 2, "insertSpaces": true},
            }),
        ),
        (
            json!(3),
            "textDocument/rangeFormatting",
            json!({
                "textDocument": {"uri": uri},
                "range": {
                    "start": position_at(text, 0),
                    "end": position_at(text, text.len()),
                },
                "options": {"tabSize": 2, "insertSpaces": true},
            }),
        ),
        (
            json!(4),
            "textDocument/foldingRange",
            json!({"textDocument": {"uri": uri}}),
        ),
        (
            json!(5),
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        ),
    ] {
        client.request(id.clone(), method, params);
        let response = client.recv_response(&id);
        assert_eq!(response["result"], json!([]), "{method}: {response}");
    }

    client.request(
        json!(6),
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": uri},
            "position": position_at(text, 2),
        }),
    );
    let response = client.recv_response(&json!(6));
    assert_eq!(response["result"], Value::Null, "{response}");

    client.request(
        json!(7),
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": uri},
            "positions": [position_at(text, 6)],
        }),
    );
    let response = client.recv_response(&json!(7));
    assert_eq!(response["result"], Value::Null, "{response}");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn linked_and_selection_ranges_handle_emoji_and_crlf_positions() {
    let text = "<view>中文😀<text>x</text></view>";
    let mut client = Client::start();
    client.initialize(json!({}));
    client.open(text);

    let emoji = text.find('😀').expect("emoji");
    let inner = text.find("<text").expect("inner") + 2;
    client.request(
        json!(2),
        "textDocument/linkedEditingRange",
        json!({
            "textDocument": {"uri": URI},
            "position": position_at(text, inner),
        }),
    );
    let response = client.recv_response(&json!(2));
    let ranges = response["result"]["ranges"].as_array().expect("ranges");
    assert_eq!(
        byte_range(text, &ranges[0]),
        (
            text.find("<text").expect("open") + 1,
            text.find("<text").expect("open") + 5
        )
    );
    assert_eq!(
        byte_range(text, &ranges[1]),
        (
            text.find("</text>").expect("close") + 2,
            text.find("</text>").expect("close") + 6
        )
    );

    client.request(
        json!(3),
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": URI},
            "positions": [position_at(text, emoji)],
        }),
    );
    let response = client.recv_response(&json!(3));
    let values = response["result"].as_array().expect("selection");
    let chain = chain_ranges(text, &values[0]);
    assert_eq!(&text[chain[0].0..chain[0].1], "中文😀");

    let crlf = "<view>\r\n  <text>x</text>\r\n</view>";
    client.change(crlf, 2);
    let inner_line = crlf.find("  <text").expect("inner line") + 3;
    client.request(
        json!(4),
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": URI},
            "positions": [position_at(crlf, inner_line)],
        }),
    );
    let response = client.recv_response(&json!(4));
    let values = response["result"].as_array().expect("selection");
    let chain = chain_ranges(crlf, &values[0]);
    assert_eq!(&crlf[chain[0].0..chain[0].1], "text");

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn selection_range_keeps_other_cursors_when_one_position_is_invalid() {
    let text = "<view>x</view>";
    let mut client = Client::start();
    client.initialize(json!({}));
    client.open(text);
    client.request(
        json!(2),
        "textDocument/selectionRange",
        json!({
            "textDocument": {"uri": URI},
            "positions": [
                {"line": 99, "character": 0},
                position_at(text, 6),
            ],
        }),
    );
    let response = client.recv_response(&json!(2));
    let values = response["result"].as_array().expect("selection ranges");
    assert_eq!(values.len(), 2, "{response}");
    let fallback = byte_range(text, &values[0]["range"]);
    assert_eq!(fallback, (0, text.len()));
    let chain = chain_ranges(text, &values[1]);
    assert!(!chain.is_empty());
    client.shutdown();
    assert!(client.finish().success());
}
