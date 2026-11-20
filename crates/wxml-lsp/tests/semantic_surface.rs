//! LSP-level coverage for the native semantic surface.
//!
//! The deleted `semantic_tools.rs` drove these handlers through a fake C1
//! transport. This file drives the real `wxml-lsp --stdio` binary in its
//! default `HostOnly` mode instead, so it proves the native protocol surface
//! without Node and without a pinned engine:
//!
//! - capability negotiation for `semanticTokens`, `inlayHint`, `codeLens`,
//!   `workspace/symbol` and `signatureHelp`, including the configuration
//!   switches that remove the two optional providers;
//! - UTF-16 semantic token positions with CJK, emoji and CRLF;
//! - component property inlay hints, WXS signature help, template/component
//!   code lenses and workspace symbols;
//! - a local template rename through `documentChanges`.
//!
//! These are static native facts. They do not claim engine-backed type
//! queries; the opt-in real-engine tests cover those.

#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::unwrap_used
)]

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

mod common;

use common::{TempProject, read_frame, wait_with_timeout};

const BIN: &str = env!("CARGO_BIN_EXE_wxml-lsp");
const TIMEOUT: Duration = Duration::from_secs(10);
const PAGE: &str = "pages/index/index.wxml";
const CRLF_PAGE: &str = "pages/crlf/crlf.wxml";
const WXS_PAGE: &str = "pages/index/utils.wxs";

const PAGE_TEXT: &str = concat!(
    "<template name=\"row\"><view>😀 {{ 名字 }}</view></template>\n",
    "<wxs module=\"u\">\n",
    "function greet(name, count) { return name; }\n",
    "module.exports = { greet: greet };\n",
    "</wxs>\n",
    "<my-card count=\"{{ 1 }}\" title=\"卡片\" />\n",
    "<template is=\"row\" />\n",
    "<view class=\"card\">{{ u.greet(名字, 1) }}</view>\n",
);

const CRLF_TEXT: &str = "<view>😀 {{ 名字 }}</view>\r\n<text>{{ 标题 }}</text>\r\n";

const WXS_TEXT: &str = concat!(
    "/**\n",
    " * Formats a name for display.\n",
    " * @param {string} name input name\n",
    " * @returns {string} formatted name\n",
    " */\n",
    "function someFunc(name) { return name; }\n",
    "module.exports = { someFunc };\n",
);

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: i64,
    version: i64,
    root: PathBuf,
}

impl Client {
    fn start(root: &Path) -> Self {
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
            root: root.to_path_buf(),
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

    fn result(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        let response = self.recv_response(id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn initialize(&mut self, options: Value) -> Value {
        let id = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": format!("file://{}", self.root.display()),
                "capabilities": {
                    "workspace": {"workspaceEdit": {"documentChanges": true}},
                    "textDocument": {
                        "semanticTokens": {"requests": {"full": true}},
                        "inlayHint": {"dynamicRegistration": false},
                        "codeLens": {"dynamicRegistration": false},
                    },
                },
                "initializationOptions": options,
            }),
        );
        let response = self.recv_response(id);
        assert!(response.get("error").is_none(), "{response}");
        self.notify("initialized", json!({}));
        response["result"].clone()
    }

    fn open(&mut self, path: &str, text: &str) -> String {
        let uri = self.uri(path);
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
        uri
    }

    fn uri(&self, path: &str) -> String {
        format!("file://{}", self.root.join(path).display())
    }

    fn finish(mut self) {
        let id = self.request("shutdown", json!(null));
        let response = self.recv_response(id);
        assert_eq!(response["result"], json!(null), "{response}");
        self.notify("exit", json!(null));
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }
        wait_with_timeout(&mut self.child);
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

fn project() -> TempProject {
    let project = TempProject::new("semantic-surface");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/crlf/crlf"],"usingComponents":{"my-card":"/components/card/card"}}"#,
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    project.write("components/card/card.json", r#"{"component":true}"#);
    project.write(
        "components/card/card.ts",
        "Component({ properties: { count: { type: Number, value: 0 }, title: { type: String, value: '' } } });",
    );
    project.write("components/card/card.wxml", "<view>{{ count }}</view>");
    project.write(
        "pages/index/index.json",
        r#"{"usingComponents":{"my-card":"../../components/card/card"}}"#,
    );
    project.write(
        "pages/index/index.ts",
        "Page({ data: { 名字: '首页', 标题: '标题' } });",
    );
    project.write(PAGE, PAGE_TEXT);
    project.write(CRLF_PAGE, CRLF_TEXT);
    project.write(WXS_PAGE, WXS_TEXT);
    project.write(
        "pages/crlf/crlf.ts",
        "Page({ data: { 名字: 'a', 标题: 'b' } });",
    );
    project
}

/// Returns the LSP position (UTF-16) of one byte offset.
fn position(text: &str, offset: usize) -> (u32, u32) {
    assert!(text.is_char_boundary(offset), "offset {offset}");
    let prefix = &text[..offset];
    let line = u32::try_from(prefix.matches('\n').count()).expect("line");
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = u32::try_from(prefix[line_start..].encode_utf16().count()).expect("character");
    (line, character)
}

fn position_value(text: &str, offset: usize) -> Value {
    let (line, character) = position(text, offset);
    json!({"line": line, "character": character})
}

fn full_range(text: &str) -> Value {
    let (line, character) = position(text, text.len());
    json!({
        "start": {"line": 0, "character": 0},
        "end": {"line": line, "character": character},
    })
}

/// Decodes the semantic-token relative encoding into absolute entries.
fn decode_tokens(data: &[Value]) -> Vec<(u32, u32, u32, u64)> {
    let mut line = 0_u32;
    let mut start = 0_u32;
    let mut decoded = Vec::with_capacity(data.len() / 5);
    for chunk in data.chunks(5) {
        let delta_line = u32::try_from(chunk[0].as_u64().expect("delta line")).expect("delta line");
        let delta_start =
            u32::try_from(chunk[1].as_u64().expect("delta start")).expect("delta start");
        let length = u32::try_from(chunk[2].as_u64().expect("length")).expect("length");
        let token_type = chunk[3].as_u64().expect("token type");
        if delta_line == 0 {
            start += delta_start;
        } else {
            line += delta_line;
            start = delta_start;
        }
        decoded.push((line, start, length, token_type));
    }
    decoded
}

fn labels(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|entry| entry["label"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn native_semantic_capabilities_follow_configuration() {
    let project = project();
    let mut client = Client::start(project.root());
    let initialized = client.initialize(json!(null));
    let capabilities = &initialized["capabilities"];
    for provider in [
        "semanticTokensProvider",
        "workspaceSymbolProvider",
        "signatureHelpProvider",
        "inlayHintProvider",
        "codeLensProvider",
        "executeCommandProvider",
    ] {
        assert!(
            capabilities.get(provider).is_some(),
            "missing {provider}: {capabilities}"
        );
    }
    assert_eq!(
        capabilities["semanticTokensProvider"]["full"],
        json!(true),
        "{capabilities}"
    );
    assert_eq!(
        capabilities["semanticTokensProvider"]["legend"]["tokenTypes"][0],
        json!("namespace"),
        "{capabilities}"
    );
    let uri = client.open(PAGE, PAGE_TEXT);
    client.finish();

    let mut disabled = Client::start(project.root());
    let initialized = disabled.initialize(json!({
        "semanticTools": {"inlayHints": false, "codeLens": false},
    }));
    let capabilities = &initialized["capabilities"];
    assert!(
        capabilities.get("inlayHintProvider").is_none(),
        "inlay hints must disappear when disabled: {capabilities}"
    );
    assert!(
        capabilities.get("codeLensProvider").is_none(),
        "code lens must disappear when disabled: {capabilities}"
    );
    assert!(
        capabilities.get("semanticTokensProvider").is_some(),
        "semantic tokens are not configurable: {capabilities}"
    );
    disabled.open(PAGE, PAGE_TEXT);
    let hints = disabled.result(
        "textDocument/inlayHint",
        json!({"textDocument": {"uri": uri}, "range": full_range(PAGE_TEXT)}),
    );
    assert_eq!(hints, json!([]), "disabled inlay hints must stay empty");
    let lenses = disabled.result(
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(lenses, json!([]), "disabled code lens must stay empty");
    disabled.finish();
}

#[test]
fn native_semantic_tokens_are_utf16_for_cjk_emoji_and_crlf() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(json!(null));
    let uri = client.open(CRLF_PAGE, CRLF_TEXT);
    let result = client.result(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": uri}}),
    );
    let data = result["data"].as_array().expect("semantic token data");
    assert!(!data.is_empty(), "expected tokens for {CRLF_TEXT:?}");
    assert_eq!(data.len() % 5, 0, "data must be 5-tuples: {data:?}");
    let decoded = decode_tokens(data);

    let name = CRLF_TEXT.find("名字").expect("CJK name");
    let (name_line, name_character) = position(CRLF_TEXT, name);
    assert!(
        decoded.contains(&(name_line, name_character, 2, 8)),
        "CJK `名字` must be a two-unit variable token at {name_line}:{name_character}: {decoded:?}"
    );

    let tag = CRLF_TEXT.find("view").expect("view tag");
    let (tag_line, tag_character) = position(CRLF_TEXT, tag);
    assert!(
        decoded.contains(&(tag_line, tag_character, 4, 0)),
        "`view` must be a four-unit namespace token at {tag_line}:{tag_character}: {decoded:?}"
    );

    // The emoji is two UTF-16 code units. A byte- or char-based encoder would
    // place `名字` one or three columns to the left of the LSP answer.
    let emoji = CRLF_TEXT.find('😀').expect("emoji");
    let (_, emoji_character) = position(CRLF_TEXT, emoji);
    let between = CRLF_TEXT[emoji..name].encode_utf16().count();
    assert_eq!(
        name_character,
        emoji_character + u32::try_from(between).expect("utf16 distance"),
        "the CJK name follows the emoji by its UTF-16 width"
    );

    // Every token must stay inside one line and inside its UTF-16 line width.
    for (line, start, length, _) in &decoded {
        let text_line = CRLF_TEXT.split('\n').nth(*line as usize).expect("line");
        let width =
            u32::try_from(text_line.trim_end_matches('\r').encode_utf16().count()).expect("width");
        assert!(
            start.saturating_add(*length) <= width,
            "token {line}:{start}+{length} exceeds line width {width}"
        );
    }
    client.finish();
}

#[test]
fn native_inlay_hints_signature_help_code_lens_and_workspace_symbols() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(json!(null));
    let uri = client.open(PAGE, PAGE_TEXT);

    let hints = client.result(
        "textDocument/inlayHint",
        json!({"textDocument": {"uri": uri}, "range": full_range(PAGE_TEXT)}),
    );
    let hint_labels = labels(&hints);
    assert!(
        hint_labels.iter().any(|label| label == ": number"),
        "component `count` must produce a number hint: {hints}"
    );
    assert!(
        hint_labels.iter().any(|label| label == ": string"),
        "component `title` must produce a string hint: {hints}"
    );
    let count_end = PAGE_TEXT
        .find("count=\"{{ 1 }}\"")
        .expect("count attribute")
        + "count=\"{{ 1 }}\"".len();
    let count_position = position_value(PAGE_TEXT, count_end);
    assert!(
        hints.as_array().expect("hints").iter().any(|hint| {
            hint["label"] == json!(": number") && hint["position"] == count_position
        }),
        "the number hint must sit at the end of the count attribute: {hints}"
    );

    let call = PAGE_TEXT.find("u.greet(名字, ").expect("signature call") + "u.greet(名字, ".len();
    let signature = client.result(
        "textDocument/signatureHelp",
        json!({"textDocument": {"uri": uri}, "position": position_value(PAGE_TEXT, call)}),
    );
    let entry = &signature["signatures"][0];
    assert!(
        entry["label"]
            .as_str()
            .is_some_and(|label| label.starts_with('(')),
        "WXS signature label: {signature}"
    );
    assert_eq!(
        entry["parameters"].as_array().expect("parameters").len(),
        2,
        "{signature}"
    );
    assert_eq!(signature["activeParameter"], json!(1), "{signature}");

    let lenses = client.result(
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    let lenses = lenses.as_array().expect("code lenses");
    assert!(
        lenses.iter().any(|lens| {
            lens["data"]["kind"] == json!("template")
                && lens["data"]["name"] == json!("row")
                && lens["data"]["count"] == json!(1)
                && lens["command"]["command"] == json!("wxml.references")
        }),
        "template call lens: {lenses:?}"
    );
    assert!(
        lenses.iter().any(|lens| {
            lens["data"]["kind"] == json!("component")
                && lens["data"]["name"] == json!("my-card")
                && lens["data"]["count"] == json!(1)
        }),
        "component use lens: {lenses:?}"
    );

    let symbols = client.result("workspace/symbol", json!({"query": ""}));
    let names: Vec<&str> = symbols
        .as_array()
        .expect("workspace symbols")
        .iter()
        .filter_map(|symbol| symbol["name"].as_str())
        .collect();
    assert!(names.contains(&"row"), "template symbol: {symbols}");
    assert!(names.contains(&"my-card"), "component symbol: {symbols}");
    let filtered = client.result("workspace/symbol", json!({"query": "row"}));
    let filtered: Vec<&Value> = filtered
        .as_array()
        .expect("filtered symbols")
        .iter()
        .collect();
    assert!(
        !filtered.is_empty() && filtered.iter().all(|symbol| symbol["name"] == json!("row")),
        "query filtering: {filtered:?}"
    );
    assert!(
        filtered[0]["location"]["uri"]
            .as_str()
            .is_some_and(|location| location.ends_with(PAGE)),
        "template symbol location: {filtered:?}"
    );
    client.finish();
}

#[test]
fn native_template_rename_returns_document_changes_for_definition_and_call() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(json!(null));
    let uri = client.open(PAGE, PAGE_TEXT);
    let call = PAGE_TEXT.find("is=\"row\"").expect("template call") + "is=\"".len();
    let prepared = client.result(
        "textDocument/prepareRename",
        json!({"textDocument": {"uri": uri}, "position": position_value(PAGE_TEXT, call)}),
    );
    assert!(
        prepared.get("range").is_some(),
        "prepareRename must return a range: {prepared}"
    );
    assert_eq!(prepared["placeholder"], json!("row"), "{prepared}");

    let renamed = client.result(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": position_value(PAGE_TEXT, call),
            "newName": "card-row",
        }),
    );
    let changes = renamed["documentChanges"]
        .as_array()
        .expect("documentChanges");
    assert_eq!(changes.len(), 1, "{renamed}");
    assert_eq!(changes[0]["textDocument"]["uri"], json!(uri), "{renamed}");
    let edits = changes[0]["edits"].as_array().expect("edits");
    assert_eq!(
        edits.len(),
        2,
        "definition + call must be renamed: {renamed}"
    );
    assert!(
        edits
            .iter()
            .all(|edit| edit["newText"] == json!("card-row")),
        "{renamed}"
    );
    client.finish();
}

#[test]
fn hover_reads_open_wxs_source_document() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(json!(null));
    let uri = client.open(WXS_PAGE, WXS_TEXT);

    let declaration = WXS_TEXT.find("function someFunc").expect("declaration") + "function ".len();
    let hover = client.result(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_value(WXS_TEXT, declaration),
        }),
    );
    assert_eq!(
        hover["contents"]["kind"],
        json!("markdown"),
        "WXS source hover must be markdown: {hover}"
    );
    let value = hover["contents"]["value"]
        .as_str()
        .expect("markdown hover value");
    assert!(value.contains("someFunc"), "member name: {value}");
    assert!(
        value.contains("(name: string) => string"),
        "signature must come from the JSDoc types: {value}"
    );
    assert!(
        value.contains("@param {string} name"),
        "multi-line JSDoc must be readable: {value}"
    );
    assert!(
        !value.contains("\\{"),
        "type braces must not be escaped: {value}"
    );
    let (line, character) = position(WXS_TEXT, declaration);
    let range = &hover["range"];
    assert_eq!(
        range["start"],
        json!({"line": line, "character": character}),
        "range must cover the declaration token: {hover}"
    );
    assert_eq!(
        range["end"],
        json!({"line": line, "character": character + 8}),
        "range must cover exactly `someFunc`: {hover}"
    );

    let keyword = WXS_TEXT.find("return").expect("return keyword");
    let none = client.result(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_value(WXS_TEXT, keyword),
        }),
    );
    assert_eq!(
        none,
        Value::Null,
        "a non-member word must return null hover"
    );
    client.finish();
}

#[test]
fn wxs_dot_completion_lists_exports_with_whitespace_after_the_dot() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(json!(null));

    let inline = concat!(
        "<wxs module=\"m1\">\n",
        "function fmt(value) { return value; }\n",
        "module.exports = { fmt };\n",
        "</wxs>\n",
        "<view>{{ m1.  }}</view>\n",
    );
    let uri = client.open("pages/index/completion-inline.wxml", inline);
    let offset = inline.find("m1.  ").expect("inline site") + "m1.  ".len();
    let completion = client.result(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": position_value(inline, offset)}),
    );
    let items = completion["items"].as_array().expect("inline items");
    let fmt = items
        .iter()
        .find(|item| item["label"] == json!("fmt"))
        .unwrap_or_else(|| panic!("inline export must be suggested: {completion}"));
    assert!(
        fmt["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("=>")),
        "detail must carry the signature: {completion}"
    );
    assert_eq!(
        fmt["textEdit"]["range"]["start"],
        position_value(inline, offset),
        "whitespace must not enter the edit range: {completion}"
    );

    let sourced = concat!(
        "<wxs module=\"m1\" src=\"./utils.wxs\" />\n",
        "<view>{{ m1.  }}</view>\n",
    );
    let uri = client.open("pages/index/completion-src.wxml", sourced);
    let offset = sourced.find("m1.  ").expect("src site") + "m1.  ".len();
    let completion = client.result(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": position_value(sourced, offset)}),
    );
    let items = completion["items"].as_array().expect("src items");
    let some_func = items
        .iter()
        .find(|item| item["label"] == json!("someFunc"))
        .unwrap_or_else(|| panic!("src export must be suggested: {completion}"));
    assert_eq!(
        some_func["detail"],
        json!("(name: string) => string"),
        "{completion}"
    );

    client.finish();
}

#[test]
fn mustache_module_hover_targets_the_token() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(json!(null));
    let uri = client.open(PAGE, PAGE_TEXT);

    let token = PAGE_TEXT
        .find("u.greet(名字, 1)")
        .expect("mustache module reference");
    let hover = client.result(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": position_value(PAGE_TEXT, token),
        }),
    );
    assert_eq!(
        hover["contents"]["kind"],
        json!("markdown"),
        "module hover must be markdown: {hover}"
    );
    let value = hover["contents"]["value"]
        .as_str()
        .expect("markdown hover value");
    assert!(value.contains("WXS module `u`"), "module header: {value}");
    assert!(value.contains("Exports:"), "export list: {value}");

    let (line, character) = position(PAGE_TEXT, token);
    let range = &hover["range"];
    assert_eq!(
        range["start"],
        json!({"line": line, "character": character}),
        "range must start at the mustache `u`, not the `<wxs module>` attribute: {hover}"
    );
    assert_eq!(
        range["end"],
        json!({"line": line, "character": character + 1}),
        "range must cover only the one-character `u` token: {hover}"
    );
    client.finish();
}
