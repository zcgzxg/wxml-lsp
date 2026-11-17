#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

//! End-to-end semantic tool tests over real JSON-RPC framing.
//!
//! The in-process sessions use `ProjectState` and a `NodeBackend` with a fake
//! transport. The fake is intentionally dumb (test-script markers), while the
//! Rust side exercises target resolution, multi-app isolation, `EditPlan`
//! validation, UTF-16 mapping and zero-write behavior.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use wxml_lsp::backend_node::{NodeBackend, NodeBackendConfig, NodeTransport, NodeTransportError};
use wxml_lsp::project_host::ProjectState;
use wxml_lsp::run_with_host;

mod common;

use common::TempProject;

const WXML: &str = "pages/index/index.wxml";
const SCRIPT: &str = "pages/index/index.ts";
const CARD_WXML: &str = "components/card/card.wxml";
const CARD_SCRIPT: &str = "components/card/card.ts";

type Responder = Arc<dyn Fn(&Value) -> Result<String, NodeTransportError> + Send + Sync>;

struct Fake {
    responder: Responder,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl NodeTransport for Fake {
    fn request(&self, line: &str, _timeout: Duration) -> Result<String, NodeTransportError> {
        let request: Value = serde_json::from_str(line).expect("request json");
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        (self.responder)(&request)
    }

    fn shutdown(&self) {}
}

#[allow(clippy::unnecessary_wraps)] // keeps the fake transport call sites uniform
fn response(id: &Value, result: Value) -> Result<String, NodeTransportError> {
    Ok(serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
    .expect("response json"))
}

fn file_text(params: &Value, file: &str) -> String {
    params["files"]
        .as_array()
        .and_then(|files| {
            files.iter().find_map(|entry| {
                (entry["fileId"].as_str() == Some(file))
                    .then(|| entry["text"].as_str().unwrap_or_default().to_owned())
            })
        })
        .unwrap_or_default()
}

const fn byte_len(text: &str) -> usize {
    text.len()
}

fn find_byte(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .find(needle)
        .map(|index| byte_len(&haystack[..index]))
}

fn marker_list(script: &str, marker: &str) -> Vec<String> {
    script
        .split(marker)
        .nth(1)
        .and_then(|rest| rest.lines().next())
        .map(|line| {
            line.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn symbols_marker(script: &str) -> Vec<String> {
    let mut names = marker_list(script, "// SYMBOL_FIELDS:");
    for name in marker_list(script, "// PROPERTY_FIELDS:") {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn reserved_marker(script: &str) -> Vec<String> {
    let mut names = marker_list(script, "// RESERVED_NAMES:");
    for marker in ["// DATA_FIELDS:", "// PROPERTY_FIELDS:"] {
        for name in marker_list(script, marker) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// A fake `symbols` answer that follows test-script markers.
#[allow(clippy::too_many_lines)] // one fake query dispatcher
fn symbol_response(params: &Value) -> Value {
    let subject = params["subjectFileId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let script = file_text(params, &subject);
    let complete = !script.contains("DYNAMIC_FOR_TEST");
    let state = if complete {
        json!({"status": "supported"})
    } else {
        json!({"status": "partial", "reason": "test marker declares incomplete coverage"})
    };
    let fields = symbols_marker(&script);
    let field_entries: Vec<Value> = fields
        .iter()
        .map(|name| {
            let start = find_byte(&script, &format!("{name}:")).unwrap_or(0);
            json!({"name": name, "quoted": false, "declaration": {"fileId": subject, "start": start, "end": start + name.len()}})
        })
        .collect();
    let query = &params["query"];
    let kind = query["kind"].as_str().unwrap_or_default();
    let mut references = Vec::new();
    let mut occurrences = Vec::new();
    let mut declaration = Value::Null;
    let mut insertion = Value::Null;
    if kind == "dataPath" {
        let path: Vec<String> = query["path"]
            .as_array()
            .map(|segments| {
                segments
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let leaf = path.last().cloned().unwrap_or_default();
        let declaration_needle = format!("{leaf}:");
        if let Some(start) = find_byte(&script, &declaration_needle) {
            declaration = json!({"fileId": subject, "start": start, "end": start + leaf.len()});
            references.push(json!({"fileId": subject, "start": start, "end": start + leaf.len(), "role": "declaration"}));
        }
        let access = format!("this.data.{}", path.join("."));
        let mut from = 0_usize;
        while let Some(index) = script[from..].find(&access) {
            let absolute = from + index;
            let start = byte_len(&script[..absolute + access.len() - leaf.len()]);
            references.push(json!({"fileId": subject, "start": start, "end": start + leaf.len(), "role": "read"}));
            from = absolute + access.len();
        }
        let key_path = if path.len() == 1 {
            format!("{}.name", path[0])
        } else {
            path.join(".")
        };
        let quoted = format!("\"{key_path}\"");
        let key_matches = if path.len() == 1 {
            key_path.starts_with(&format!("{}.", path[0]))
        } else {
            key_path == path.join(".")
        };
        if key_matches && let Some(index) = find_byte(&script, &quoted) {
            let content_start = index + 1;
            let mut segment_start = content_start;
            for segment in &path[..path.len().saturating_sub(1)] {
                segment_start += segment.len() + 1;
            }
            occurrences.push(json!({
                "fileId": subject,
                "start": segment_start,
                "end": segment_start + leaf.len(),
                "kind": "setDataKey",
                "path": path,
                "segment": path.len().saturating_sub(1),
            }));
        }
    } else if kind == "eventHandler" {
        let name = query["name"].as_str().unwrap_or_default();
        if let Some(start) = find_byte(&script, &format!("{name}("))
            .or_else(|| find_byte(&script, &format!("{name}:")))
        {
            declaration = json!({"fileId": subject, "start": start, "end": start + name.len()});
            references.push(json!({"fileId": subject, "start": start, "end": start + name.len(), "role": "declaration"}));
        } else if let Some(marker) = find_byte(&script, "// INSERT_HERE") {
            insertion = json!({
                "fileId": subject,
                "offset": marker,
                "text": format!("\n  {name}() {{\n  }},\n"),
            });
        }
    }
    json!({
        "protocolVersion": 2,
        "requestId": params["requestId"],
        "state": state,
        "complete": complete,
        "references": references,
        "occurrences": occurrences,
        "fields": field_entries,
        "reserved": reserved_marker(&script),
        "declaration": declaration,
        "insertion": insertion,
    })
}

fn default_responder(request: &Value) -> Result<String, NodeTransportError> {
    let id = &request["id"];
    let method = request["method"].as_str().unwrap_or_default();
    if method == "symbols" {
        return response(id, symbol_response(&request["params"]));
    }
    if method == "analyze" {
        let params = &request["params"];
        return response(
            id,
            json!({
                "protocolVersion": 2,
                "requestId": params["requestId"],
                "jsCheckApplied": "pragma",
                "registration": {"kind": "page", "count": 1, "topLevel": true},
                "states": {
                    "hover": {"status": "supported"},
                    "diagnostics": {"status": "supported"},
                    "definition": {"status": "supported"},
                },
                "insertions": [],
                "mounts": [],
                "fragments": [],
                "moduleKind": {"before": "script", "after": "script"},
                "suppressed": {"count": 0, "details": []},
            }),
        );
    }
    response(id, json!({"ok": true}))
}

struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    initialize_result: Value,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        let reader_stream = stream.try_clone().expect("clone");
        reader_stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        Self {
            writer: stream,
            reader: BufReader::new(reader_stream),
            next_id: 1,
            initialize_result: Value::Null,
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
            let read = self.reader.read_line(&mut line).expect("header");
            assert!(read != 0, "server closed");
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
        self.reader.read_exact(&mut body).expect("body");
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

    fn open(&mut self, uri: &str, version: u64, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "wxml", "version": version, "text": text}}),
        );
    }

    fn change(&mut self, uri: &str, version: u64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        );
    }

    fn finish(mut self) {
        let shutdown = self.request("shutdown", json!(null));
        assert_eq!(self.response(shutdown)["result"], json!(null));
        self.notify("exit", json!(null));
        // Closing the write half makes the server reader observe EOF and drop
        // its clone of the stream; otherwise draining for EOF would wait for
        // the client's own still-open write half forever.
        let _ = self.writer.shutdown(Shutdown::Write);
        // Drain until the server closes its side so a queued diagnostics write
        // cannot race the client drop and fail the server thread.
        loop {
            let mut line = String::new();
            let Ok(read) = self.reader.read_line(&mut line) else {
                break;
            };
            if read == 0 {
                break;
            }
            let Some(length) = line
                .trim_end()
                .strip_prefix("Content-Length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
            else {
                continue;
            };
            let mut body = vec![0_u8; length];
            if self.reader.read_exact(&mut body).is_err() {
                break;
            }
        }
    }
}

fn app_fixture(project: &TempProject) {
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{"my-card":"/components/card/card"}}"#,
    );
    project.write(
        CARD_SCRIPT,
        r"Component({ properties: { title: { type: String } } });",
    );
    project.write("components/card/card.json", r#"{"component":true}"#);
    project.write(CARD_WXML, "<view>{{ title }}</view>");
    project.write(
        SCRIPT,
        "Page({ data: { user: { name: 'x' }, count: 1 }, onTap() {} });",
    );
    project.write(WXML, "<view>{{ user.name }}</view>");
}

fn start_session(
    project: &TempProject,
    responder: Responder,
    initialization: Value,
) -> (
    Client,
    thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>,
) {
    start_session_with(
        project,
        responder,
        initialization,
        json!({"workspace": {"workspaceEdit": {"documentChanges": true}}}),
    )
}

fn start_session_with(
    project: &TempProject,
    responder: Responder,
    initialization: Value,
    capabilities: Value,
) -> (
    Client,
    thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>,
) {
    let (server, client_stream) = UnixStream::pair().expect("socket");
    let server_read = server.try_clone().expect("clone");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        {
            let requests = Arc::clone(&requests);
            move || {
                Ok(Box::new(Fake {
                    responder: Arc::clone(&responder),
                    requests: Arc::clone(&requests),
                }))
            }
        },
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server, backend, host));
    let mut client = Client::new(client_stream);
    let options = json!({
        "processId": null,
        "rootUri": project.root_uri(),
        "capabilities": capabilities,
        "initializationOptions": initialization,
    });
    let id = client.request("initialize", options);
    let response = client.response(id);
    assert!(
        response.get("error").is_none(),
        "initialize failed: {response}"
    );
    client.notify("initialized", json!({}));
    client.initialize_result = response;
    (client, handle)
}

fn position_at(text: &str, needle: &str) -> (u64, u64) {
    let index = text.find(needle).expect("needle");
    let before = &text[..index];
    let line = before.matches('\n').count() as u64;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let character: usize = before[line_start..].encode_utf16().count();
    (line, u64::try_from(character).unwrap())
}

fn apply_edits(text: &str, edits: &[Value]) -> String {
    let mut ranges: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|edit| {
            let start = usize::try_from(edit["range"]["start"]["line"].as_u64().expect("line"))
                .expect("fits");
            let start_char =
                usize::try_from(edit["range"]["start"]["character"].as_u64().expect("char"))
                    .expect("fits");
            let end = usize::try_from(edit["range"]["end"]["line"].as_u64().expect("line"))
                .expect("fits");
            let end_char =
                usize::try_from(edit["range"]["end"]["character"].as_u64().expect("char"))
                    .expect("fits");
            (
                byte_from_position(text, start, start_char),
                byte_from_position(text, end, end_char),
                edit["newText"].as_str().expect("newText").to_owned(),
            )
        })
        .collect();
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    let mut output = text.to_owned();
    for (start, end, replacement) in ranges.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    output
}

fn byte_from_position(text: &str, line: usize, character: usize) -> usize {
    let mut offset = 0_usize;
    for (index, segment) in text.split_inclusive('\n').enumerate() {
        if index == line {
            let mut units = 0_usize;
            for (byte, ch) in segment.char_indices() {
                if units == character {
                    return offset + byte;
                }
                units += ch.len_utf16();
                if units > character {
                    return offset + byte;
                }
            }
            return offset + segment.len();
        }
        offset += segment.len();
    }
    offset
}

fn edit_for_uri<'a>(response: &'a Value, uri: &str) -> &'a Value {
    response["result"]["documentChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("no documentChanges: {response}"))
        .iter()
        .find(|change| change["textDocument"]["uri"].as_str() == Some(uri))
        .unwrap_or_else(|| panic!("no edit for {uri}: {response}"))
}

#[test]
fn capabilities_match_handlers_and_follow_configuration() {
    let project = TempProject::new("wxml-semantic-capabilities");
    app_fixture(&project);
    let (client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let capabilities = &client.initialize_result["result"]["capabilities"];
    assert_eq!(capabilities["referencesProvider"], json!(true));
    assert_eq!(
        capabilities["renameProvider"]["prepareProvider"],
        json!(true)
    );
    assert_eq!(
        capabilities["codeActionProvider"]["resolveProvider"],
        json!(false)
    );
    assert_eq!(capabilities["semanticTokensProvider"]["full"], json!(true));
    assert!(
        capabilities["semanticTokensProvider"]["legend"]["tokenTypes"]
            .as_array()
            .is_some_and(|types| types.iter().any(|value| value == "wxs"))
    );
    assert_eq!(capabilities["workspaceSymbolProvider"], json!(true));
    assert_eq!(
        capabilities["inlayHintProvider"]["resolveProvider"],
        json!(false)
    );
    assert_eq!(
        capabilities["signatureHelpProvider"]["triggerCharacters"][0],
        "("
    );
    assert_eq!(
        capabilities["codeLensProvider"]["resolveProvider"],
        json!(false)
    );
    assert_eq!(
        capabilities["executeCommandProvider"]["commands"],
        json!(["wxml.references"])
    );
    client.finish();
    handle.join().expect("thread").expect("server");

    let project = TempProject::new("wxml-semantic-disabled");
    app_fixture(&project);
    let (client, handle) = start_session(
        &project,
        Arc::new(default_responder),
        json!({"semanticTools": {"inlayHints": false, "codeLens": false}}),
    );
    let capabilities = &client.initialize_result["result"]["capabilities"];
    assert!(capabilities.get("inlayHintProvider").is_none());
    assert!(capabilities.get("codeLensProvider").is_none());
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn semantic_tokens_are_utf16_and_cover_unicode_crlf_and_wxs() {
    let project = TempProject::new("wxml-semantic-tokens");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let wxml = "<!-- 注释😀 -->\r\n<view class=\"a\" bindtap=\"onTap\">标题😀 {{ user.name }}</view>\r\n<wxs module=\"u\">\nfunction add(a) { return a; }\n</wxs>\r\n";
    let uri = project.uri(WXML);
    client.open(&uri, 1, wxml);
    let id = client.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": uri}}),
    );
    let response = client.response(id);
    let data = response["result"]["data"].as_array().expect("data");
    assert_eq!(data.len() % 5, 0);
    assert!(!data.is_empty());
    let mut line = 0_u64;
    let mut character = 0_u64;
    let mut types = std::collections::HashSet::new();
    for token in data.chunks(5) {
        let delta_line = token[0].as_u64().unwrap();
        let delta_start = token[1].as_u64().unwrap();
        let length = token[2].as_u64().unwrap();
        let token_type = token[3].as_u64().unwrap();
        assert!(length > 0);
        if delta_line == 0 {
            character += delta_start;
        } else {
            line += delta_line;
            character = delta_start;
        }
        types.insert(token_type);
        let line_text = wxml
            .split_inclusive('\n')
            .nth(usize::try_from(line).expect("fits"))
            .unwrap_or("");
        assert!(
            character + length <= line_text.encode_utf16().count() as u64,
            "token out of line bounds: {token:?}"
        );
    }
    for expected in [0_u64, 8, 9, 11, 17, 22] {
        assert!(
            types.contains(&expected),
            "missing token type {expected}: {types:?}"
        );
    }
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn local_rename_applies_tag_template_and_loop_edits_atomically() {
    let project = TempProject::new("wxml-semantic-local");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<template name=\"row\"><view wx:for=\"{{ items }}\" wx:for-item=\"item\">{{ item.name }}</view></template>\n<template is=\"row\" data=\"{{ items: list }}\"/>\n<my-card></my-card>";
    client.open(&uri, 1, wxml);

    let (line, character) = position_at(wxml, "<my-card");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 1},
            "newName": "fancy-card",
        }),
    );
    let response = client.response(id);
    assert_eq!(
        response["error"]["code"], -32803,
        "custom component tag rename must be rejected without alias coverage: {response}"
    );
    assert!(response.get("result").is_none(), "{response}");

    let (line, character) = position_at(wxml, "is=\"row\"");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "rowItem",
        }),
    );
    let response = client.response(id);
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    assert_eq!(edits.len(), 2, "{response}");
    let renamed = apply_edits(wxml, edits);
    assert!(renamed.contains("<template name=\"rowItem\">"), "{renamed}");
    assert!(renamed.contains("is=\"rowItem\""), "{renamed}");

    let (line, character) = position_at(wxml, "wx:for-item=\"item\"");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 13},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    assert_eq!(edits.len(), 2, "{response}");
    let renamed = apply_edits(wxml, edits);
    assert!(renamed.contains("wx:for-item=\"entry\""), "{renamed}");
    assert!(renamed.contains("{{ entry.name }}"), "{renamed}");

    let prepare = client.request(
        "textDocument/prepareRename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 13},
        }),
    );
    let prepare = client.response(prepare);
    assert_eq!(prepare["result"]["placeholder"], "item");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn default_loop_binding_rename_inserts_the_binding_attribute() {
    let project = TempProject::new("wxml-semantic-default-loop");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view wx:for=\"{{ items }}\"><text>{{ item }}</text></view>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "{{ item }}");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    let renamed = apply_edits(wxml, edits);
    assert!(renamed.contains("wx:for-item=\"entry\""), "{renamed}");
    assert!(renamed.contains("{{ entry }}"), "{renamed}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn data_rename_updates_open_wxml_and_closed_script_without_writing_disk() {
    let project = TempProject::new("wxml-semantic-data");
    app_fixture(&project);
    let script = "Page({\n  // SYMBOL_FIELDS: user, count\n  data: { user: { name: \"x\" }, count: 1 },\n  onLoad() {\n    const label = this.data.user.name;\n    this.setData({ \"user.name\": label });\n  },\n});";
    project.write(SCRIPT, script);
    let disk_before = std::fs::read(project.root().join(SCRIPT)).expect("script");
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "标题😀\r\n<view>{{ user.name }}</view>\r\n";
    client.open(&uri, 7, wxml);

    let references = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 9},
            "context": {"includeDeclaration": true},
        }),
    );
    let references = client.response(references);
    let locations = references["result"].as_array().expect("locations");
    assert!(
        locations
            .iter()
            .any(|location| { location["uri"].as_str() == Some(project.uri(WXML).as_str()) })
    );
    assert!(
        locations
            .iter()
            .any(|location| { location["uri"].as_str() == Some(project.uri(SCRIPT).as_str()) })
    );

    let prepare = client.request(
        "textDocument/prepareRename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 9},
        }),
    );
    let prepare = client.response(prepare);
    assert_eq!(prepare["result"]["placeholder"], "user", "{prepare}");

    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 1, "character": 9},
            "newName": "account",
        }),
    );
    let response = client.response(id);
    let wxml_edits = edit_for_uri(&response, &project.uri(WXML));
    assert_eq!(wxml_edits["textDocument"]["version"], 7);
    let script_edits = edit_for_uri(&response, &project.uri(SCRIPT));
    assert_eq!(script_edits["textDocument"]["version"], Value::Null);
    let renamed_wxml = apply_edits(wxml, wxml_edits["edits"].as_array().expect("wxml edits"));
    assert!(
        renamed_wxml.contains("{{ account.name }}"),
        "{renamed_wxml}"
    );
    let renamed_script = apply_edits(
        script,
        script_edits["edits"].as_array().expect("script edits"),
    );
    assert!(
        renamed_script.contains("account: { name: \"x\" }"),
        "{renamed_script}"
    );
    assert!(
        renamed_script.contains("this.data.account.name"),
        "{renamed_script}"
    );
    assert!(
        renamed_script.contains("\"account.name\""),
        "{renamed_script}"
    );
    // The server returned edits only; the closed file stays byte-identical.
    assert_eq!(
        std::fs::read(project.root().join(SCRIPT)).expect("script"),
        disk_before
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn multi_app_rename_stays_in_the_document_project() {
    let project = TempProject::new("wxml-semantic-multi-app");
    for app in ["a", "b"] {
        project.write(
            &format!("{app}/app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
        );
        project.write(&format!("{app}/{WXML}"), "{{ user.name }}");
        project.write(
            &format!("{app}/{SCRIPT}"),
            &format!("Page({{ data: {{ user: {{ name: '{app}' }} }} }});"),
        );
    }
    let (mut client, handle) = start_session(
        &project,
        Arc::new(default_responder),
        json!({"apps": [{"root": "a"}, {"root": "b"}]}),
    );
    let b_uri = project.uri(&format!("b/{WXML}"));
    let b_text = "{{ user.name }}";
    client.open(&b_uri, 1, b_text);
    let (line, character) = position_at(b_text, "user");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": b_uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let response = client.response(id);
    let changes = response["result"]["documentChanges"]
        .as_array()
        .expect("changes");
    assert!(!changes.is_empty());
    for change in changes {
        let uri = change["textDocument"]["uri"].as_str().expect("uri");
        assert!(uri.contains("/b/"), "cross-app edit: {uri}");
    }
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn code_actions_create_handlers_and_fix_field_typos() {
    let project = TempProject::new("wxml-semantic-actions");
    app_fixture(&project);
    let script = "Page({\n  // SYMBOL_FIELDS: name\n  data: { name: 'x' },\n  // INSERT_HERE\n});";
    project.write(SCRIPT, script);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view bindtap=\"onMissing\">{{ nmae }}</view>";
    client.open(&uri, 1, wxml);

    let (line, character) = position_at(wxml, "onMissing");
    let id = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": line, "character": character},
                "end": {"line": line, "character": character + "onMissing".len() as u64},
            },
            "context": {"diagnostics": []},
        }),
    );
    let response = client.response(id);
    eprintln!("code action response: {response}");
    let actions = response["result"].as_array().expect("actions");
    assert_eq!(actions.len(), 1, "{response}");
    assert_eq!(actions[0]["title"], "Create event handler 'onMissing'");
    let edits = actions[0]["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .expect("edits");
    let applied = apply_edits(script, edits);
    assert!(applied.contains("onMissing()"), "{applied}");
    assert_eq!(
        std::fs::read_to_string(project.root().join(SCRIPT)).expect("script"),
        script
    );

    let diagnostic_range = {
        let (line, character) = position_at(wxml, "nmae");
        json!({
            "start": {"line": line, "character": character},
            "end": {"line": line, "character": character + 4},
        })
    };
    let id = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": {"uri": uri},
            "range": diagnostic_range,
            "context": {
                "diagnostics": [{
                    "range": diagnostic_range,
                    "severity": 1,
                    "source": "wxml-lsp",
                    "message": "Property 'nmae' does not exist on type '{ name: string; }'.",
                }],
                "only": ["quickfix"],
            },
        }),
    );
    let response = client.response(id);
    let actions = response["result"].as_array().expect("actions");
    assert!(
        actions
            .iter()
            .any(|action| action["title"] == "Replace 'nmae' with 'name'"),
        "{response}"
    );
    let action = actions
        .iter()
        .find(|action| action["title"] == "Replace 'nmae' with 'name'")
        .expect("typo action");
    let edits = action["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .expect("edits");
    let applied = apply_edits(wxml, edits);
    assert!(applied.contains("{{ name }}"), "{applied}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn signature_help_inlay_lens_symbols_and_command_use_static_facts() {
    let project = TempProject::new("wxml-semantic-hints");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<wxs module=\"u\">\nfunction add(a, b) { return a + b; }\nmodule.exports = { add: add };\n</wxs>\n<template name=\"row\"><view>{{ u.add(1, 2) }}</view></template>\n<template is=\"row\" data=\"{{}}\"/>\n<my-card title=\"x\" bindtap=\"onTap\"></my-card>";
    client.open(&uri, 1, wxml);

    let (line, character) = position_at(wxml, "u.add(1, ");
    let id = client.request(
        "textDocument/signatureHelp",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + "u.add(1, ".len() as u64},
        }),
    );
    let response = client.response(id);
    let signature = &response["result"]["signatures"][0];
    assert_eq!(
        signature["parameters"]
            .as_array()
            .expect("parameters")
            .len(),
        2
    );
    assert_eq!(response["result"]["activeParameter"], 1, "{response}");

    let id = client.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 20, "character": 0},
            },
        }),
    );
    let response = client.response(id);
    let hints = response["result"].as_array().expect("hints");
    assert!(
        hints.iter().any(|hint| hint["label"] == ": string"),
        "{response}"
    );

    let id = client.request(
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    let response = client.response(id);
    let lenses = response["result"].as_array().expect("lenses");
    assert!(
        lenses.iter().any(|lens| lens["command"]["title"]
            .as_str()
            .is_some_and(|title| title.contains("template call"))),
        "{response}"
    );
    assert!(
        lenses.iter().any(|lens| lens["command"]["title"]
            .as_str()
            .is_some_and(|title| title.contains("component use"))),
        "{response}"
    );

    let id = client.request("workspace/symbol", json!({"query": "row"}));
    let response = client.response(id);
    let symbols = response["result"].as_array().expect("symbols");
    assert!(
        symbols.iter().any(|symbol| symbol["name"] == "row"),
        "{response}"
    );

    let (line, character) = position_at(wxml, "is=\"row\"");
    let id = client.request(
        "workspace/executeCommand",
        json!({
            "command": "wxml.references",
            "arguments": [{
                "uri": uri,
                "position": {"line": line, "character": character + 4},
                "includeDeclaration": true,
            }],
        }),
    );
    let response = client.response(id);
    let locations = response["result"]["locations"]
        .as_array()
        .expect("locations");
    assert_eq!(
        response["result"]["coverage"]["status"], "supported",
        "{response}"
    );
    assert!(locations.len() >= 2, "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn incomplete_script_facts_reject_rename_and_keep_references_empty() {
    let project = TempProject::new("wxml-semantic-incomplete");
    app_fixture(&project);
    project.write(
        SCRIPT,
        "Page({ data: { user: { name: 'x' } } }); // DYNAMIC_FOR_TEST",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "{{ user.name }}";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user");
    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let response = client.response(rename);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    assert!(response.get("result").is_none());
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn cancelled_and_stale_semantic_requests_return_errors_without_edits() {
    let project = TempProject::new("wxml-semantic-cancel");
    app_fixture(&project);
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let gate_responder = Arc::clone(&gate);
    let responder: Responder = Arc::new(move |request: &Value| {
        if request["method"] == "symbols" {
            let (lock, condvar) = &*gate_responder;
            let mut released = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while !*released {
                released = condvar
                    .wait(released)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            drop(released);
            return response(&request["id"], symbol_response(&request["params"]));
        }
        default_responder(request)
    });
    let (mut client, handle) = start_session(&project, responder, json!({}));
    let uri = project.uri(WXML);
    let wxml = "{{ user.name }}";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user");
    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    client.notify("$/cancelRequest", json!({"id": rename}));
    let (lock, condvar) = &*gate;
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
    condvar.notify_all();
    let response = client.response(rename);
    assert_eq!(response["error"]["code"], -32800, "{response}");
    assert!(response.get("result").is_none());

    // A late result for an edited document must not produce an edit.
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = false;
    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    client.change(&uri, 2, "{{ user.title }}");
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
    condvar.notify_all();
    let response = client.response(rename);
    assert_eq!(response["error"]["code"], -32801, "{response}");
    assert!(response.get("result").is_none());
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn component_property_rename_updates_declaration_and_wxml_attribute() {
    let project = TempProject::new("wxml-semantic-property");
    app_fixture(&project);
    project.write(
        CARD_SCRIPT,
        "Component({ properties: { title: { type: String } } });",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<my-card title=\"x\"></my-card>";
    client.open(&uri, 2, wxml);
    let (line, character) = position_at(wxml, "title");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "heading",
        }),
    );
    let response = client.response(id);
    let wxml_edits = edit_for_uri(&response, &uri);
    let renamed = apply_edits(wxml, wxml_edits["edits"].as_array().expect("edits"));
    assert!(renamed.contains("heading=\"x\""), "{renamed}");
    let script_edits = edit_for_uri(&response, &project.uri(CARD_SCRIPT));
    let renamed_script = apply_edits(
        "Component({ properties: { title: { type: String } } });",
        script_edits["edits"].as_array().expect("edits"),
    );
    assert!(renamed_script.contains("heading:"), "{renamed_script}");
    let template_edits = edit_for_uri(&response, &project.uri(CARD_WXML));
    let renamed_template = apply_edits(
        "<view>{{ title }}</view>",
        template_edits["edits"].as_array().expect("edits"),
    );
    assert!(
        renamed_template.contains("{{ heading }}"),
        "{renamed_template}"
    );
    let parsed = wxml_syntax::parse(&renamed_template);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn event_handler_rename_updates_script_and_bindings() {
    let project = TempProject::new("wxml-semantic-event");
    app_fixture(&project);
    let script = "Page({ onTap() {} });";
    project.write(SCRIPT, script);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view bindtap=\"onTap\"></view>";
    client.open(&uri, 3, wxml);
    let (line, character) = position_at(wxml, "onTap");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "handleTap",
        }),
    );
    let response = client.response(id);
    let wxml_edits = edit_for_uri(&response, &uri);
    let renamed = apply_edits(wxml, wxml_edits["edits"].as_array().expect("edits"));
    assert!(renamed.contains("bindtap=\"handleTap\""), "{renamed}");
    assert!(wxml_syntax::parse(&renamed).errors.is_empty());
    let script_edits = edit_for_uri(&response, &project.uri(SCRIPT));
    let renamed_script = apply_edits(script, script_edits["edits"].as_array().expect("edits"));
    assert!(renamed_script.contains("handleTap()"), "{renamed_script}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn local_class_rename_updates_wxml_and_local_wxss() {
    let project = TempProject::new("wxml-semantic-class");
    app_fixture(&project);
    project.write("pages/index/index.wxss", ".hero { color: red; }");
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view class=\"hero big\"></view>";
    client.open(&uri, 4, wxml);
    let (line, character) = position_at(wxml, "hero");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "banner",
        }),
    );
    let response = client.response(id);
    let wxml_edits = edit_for_uri(&response, &uri);
    let renamed = apply_edits(wxml, wxml_edits["edits"].as_array().expect("edits"));
    assert!(renamed.contains("class=\"banner big\""), "{renamed}");
    assert!(wxml_syntax::parse(&renamed).errors.is_empty());
    let style_edits = edit_for_uri(&response, &project.uri("pages/index/index.wxss"));
    let renamed_style = apply_edits(
        ".hero { color: red; }",
        style_edits["edits"].as_array().expect("edits"),
    );
    assert!(renamed_style.contains(".banner"), "{renamed_style}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn template_rename_crosses_import_boundary() {
    let project = TempProject::new("wxml-semantic-template-import");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({});");
    project.write(
        "pages/index/index.wxml",
        "<import src=\"../../templates/row.wxml\"/><template is=\"row\"/>",
    );
    project.write(
        "templates/row.wxml",
        "<template name=\"row\"><view/></template>",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<import src=\"../../templates/row.wxml\"/><template is=\"row\"/>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "is=\"row\"");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "rowItem",
        }),
    );
    let response = client.response(id);
    let call_edits = edit_for_uri(&response, &uri);
    let renamed = apply_edits(wxml, call_edits["edits"].as_array().expect("edits"));
    assert!(renamed.contains("is=\"rowItem\""), "{renamed}");
    let definition_edits = edit_for_uri(&response, &project.uri("templates/row.wxml"));
    let renamed_definition = apply_edits(
        "<template name=\"row\"><view/></template>",
        definition_edits["edits"].as_array().expect("edits"),
    );
    assert!(
        renamed_definition.contains("name=\"rowItem\""),
        "{renamed_definition}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn references_honor_include_declaration_and_unsaved_script_version() {
    let project = TempProject::new("wxml-semantic-overlay");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "{{ user.name }}";
    client.open(&uri, 1, wxml);
    let script_uri = project.uri(SCRIPT);
    let script = "Page({\n  // SYMBOL_FIELDS: user\n  data: { user: { name: 'unsaved' } },\n  onLoad() { const x = this.data.user; },\n});";
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": script_uri, "languageId": "typescript", "version": 5, "text": script}}),
    );
    let (line, character) = position_at(wxml, "user");
    let with_declaration = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "context": {"includeDeclaration": true},
        }),
    );
    let with_declaration = client.response(with_declaration);
    let with_count = with_declaration["result"]
        .as_array()
        .expect("locations")
        .len();
    let without_declaration = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "context": {"includeDeclaration": false},
        }),
    );
    let without_declaration = client.response(without_declaration);
    let without_count = without_declaration["result"]
        .as_array()
        .expect("locations")
        .len();
    assert_eq!(
        with_count,
        without_count + 1,
        "{with_declaration} {without_declaration}"
    );

    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let response = client.response(rename);
    let script_edits = edit_for_uri(&response, &script_uri);
    assert_eq!(script_edits["textDocument"]["version"], 5);
    let renamed = apply_edits(script, script_edits["edits"].as_array().expect("edits"));
    assert!(
        renamed.contains("account: { name: 'unsaved' }"),
        "{renamed}"
    );
    assert!(renamed.contains("this.data.account"), "{renamed}");
    assert!(
        std::fs::read_to_string(project.root().join(SCRIPT))
            .expect("disk script")
            .contains("user: { name: 'x' }"),
        "the server must not write the unsaved overlay to disk"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn unsupported_symbol_backend_rejects_rename_and_returns_empty_references() {
    let project = TempProject::new("wxml-semantic-unsupported");
    app_fixture(&project);
    let responder: Responder = Arc::new(|request: &Value| {
        if request["method"] == "symbols" {
            return Ok(serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "error": {"code": -32601, "message": "Method not found: symbols"},
            }))
            .expect("json"));
        }
        default_responder(request)
    });
    let (mut client, handle) = start_session(&project, responder, json!({}));
    let uri = project.uri(WXML);
    let wxml = "{{ user.name }}";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user");
    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let response = client.response(rename);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    let references = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "context": {"includeDeclaration": true},
        }),
    );
    let references = client.response(references);
    assert_eq!(references["result"], json!([]), "{references}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn incomplete_syntax_keeps_tokens_symbols_and_hints_bounded() {
    let project = TempProject::new("wxml-semantic-incomplete-syntax");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view><text>{{ user.name }}\r\n<my-card title=\"x\"";
    client.open(&uri, 1, wxml);

    let tokens = client.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": uri}}),
    );
    let tokens = client.response(tokens);
    let data = tokens["result"]["data"].as_array().expect("data");
    assert_eq!(data.len() % 5, 0);

    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    let symbols = client.response(symbols);
    assert!(symbols["result"].is_array(), "{symbols}");

    let hints = client.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 9, "character": 0},
            },
        }),
    );
    let hints = client.response(hints);
    assert!(hints["result"].is_array(), "{hints}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn component_property_kebab_and_camel_spellings_stay_consistent() {
    let project = TempProject::new("wxml-semantic-property-kebab");
    app_fixture(&project);
    project.write(
        CARD_SCRIPT,
        "Component({ properties: { displayTitle: { type: String } } });",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<my-card display-title=\"x\"></my-card>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "display-title");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 2},
            "newName": "heading-text",
        }),
    );
    let response = client.response(id);
    let renamed = apply_edits(
        wxml,
        edit_for_uri(&response, &uri)["edits"]
            .as_array()
            .expect("wxml edits"),
    );
    assert!(renamed.contains("heading-text=\"x\""), "{renamed}");
    let renamed_script = apply_edits(
        "Component({ properties: { displayTitle: { type: String } } });",
        edit_for_uri(&response, &project.uri(CARD_SCRIPT))["edits"]
            .as_array()
            .expect("script edits"),
    );
    assert!(renamed_script.contains("headingText:"), "{renamed_script}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn semantic_tokens_single_long_line_stays_linear() {
    let project = TempProject::new("wxml-semantic-long-line");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let mut wxml = String::new();
    wxml.push_str(&"a".repeat(1024 * 1024));
    for _ in 0..4_000 {
        wxml.push_str("{{a}}");
    }
    let started = std::time::Instant::now();
    client.open(&uri, 1, &wxml);
    let id = client.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": uri}}),
    );
    let response = client.response(id);
    let elapsed = started.elapsed();
    assert!(response["result"]["data"].is_array(), "{response}");
    assert!(
        elapsed < Duration::from_secs(5),
        "single-line semantic tokens took {elapsed:?}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn component_property_rename_rejects_component_data_conflict() {
    let project = TempProject::new("wxml-semantic-property-conflict");
    app_fixture(&project);
    project.write(
        CARD_SCRIPT,
        "Component({\n  // PROPERTY_FIELDS: title\n  // DATA_FIELDS: heading\n  data: { heading: 'x' },\n  properties: { title: { type: String } },\n});",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<my-card title=\"x\"></my-card>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "title");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "heading",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    assert!(response.get("result").is_none(), "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn reserved_and_scope_capturing_names_are_rejected() {
    let project = TempProject::new("wxml-semantic-reserved");
    app_fixture(&project);
    project.write(
        SCRIPT,
        "Page({ // DATA_FIELDS: items\n data: { user: { name: 'x' }, items: [] } });",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view wx:for=\"{{ items }}\" wx:for-item=\"entry\">{{ user.name }}</view>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user.name");

    for (new_name, expected) in [("entry", -32803), ("this", -32602), ("null", -32602)] {
        let id = client.request(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
                "newName": new_name,
            }),
        );
        let response = client.response(id);
        assert_eq!(
            response["error"]["code"], expected,
            "{new_name}: {response}"
        );
        assert!(response.get("result").is_none(), "{response}");
    }

    let (line, character) = position_at(wxml, "wx:for-item=\"entry\"");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 13},
            "newName": "user",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn loop_rename_keeps_self_shadowing_and_template_definition_bodies() {
    let project = TempProject::new("wxml-semantic-loop-self");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);

    let wxml = "<view wx:for=\"{{ items }}\" wx:for-item=\"items\">{{ items.name }}</view>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "wx:for-item=\"items\"");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 13},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    let renamed = apply_edits(wxml, edits);
    assert!(renamed.contains("wx:for=\"{{ items }}\""), "{renamed}");
    assert!(renamed.contains("wx:for-item=\"entry\""), "{renamed}");
    assert!(renamed.contains("{{ entry.name }}"), "{renamed}");
    assert!(wxml_syntax::parse(&renamed).errors.is_empty());

    let wxml = "<view wx:for=\"{{ items }}\" wx:for-item=\"item\"><template name=\"row\"><view>{{ item }}</view></template></view>";
    client.change(&uri, 2, wxml);
    let (line, character) = position_at(wxml, "wx:for-item=\"item\"");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 13},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    assert_eq!(edits.len(), 1, "{response}");
    let renamed = apply_edits(wxml, edits);
    assert!(
        renamed.contains("<template name=\"row\"><view>{{ item }}</view></template>"),
        "{renamed}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn template_data_reference_blocks_data_rename() {
    let project = TempProject::new("wxml-semantic-template-data");
    app_fixture(&project);
    project.write(
        SCRIPT,
        "Page({ // DATA_FIELDS: pageTitle\n data: { pageTitle: 'x' } });",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<template is=\"row\" data=\"{{ pageTitle }}\"/><view>{{ pageTitle }}</view>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "{{ pageTitle }}");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "headline",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    assert!(response.get("result").is_none(), "{response}");

    let (line, character) = position_at(wxml, "{{ pageTitle }}</view>");
    let references = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "context": {"includeDeclaration": true},
        }),
    );
    let references = client.response(references);
    assert_eq!(references["result"], json!([]), "{references}");
    let command = client.request(
        "workspace/executeCommand",
        json!({
            "command": "wxml.references",
            "arguments": [{
                "uri": uri,
                "position": {"line": line, "character": character + 4},
                "includeDeclaration": true,
            }],
        }),
    );
    let command = client.response(command);
    assert_eq!(
        command["result"]["coverage"]["status"], "partial",
        "{command}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn code_lens_is_bounded_for_many_template_definitions() {
    use std::fmt::Write as _;

    let project = TempProject::new("wxml-semantic-lens-cap");
    let mut pages = Vec::new();
    for page in 0..32 {
        let route = format!("pages/p{page}/p{page}");
        pages.push(route.clone());
        project.write(&format!("{route}.ts"), "Page({});");
        let mut wxml = String::new();
        let definitions = if page == 0 { 512 } else { 16 };
        for index in 0..definitions {
            write!(
                wxml,
                "<template name=\"r{page}_{index}\"><view/></template><template is=\"r{page}_{index}\"/>"
            )
            .expect("write");
        }
        project.write(&format!("{route}.wxml"), &wxml);
    }
    project.write(
        "app.json",
        &format!(
            "{{\"pages\":[{}]}}",
            pages
                .iter()
                .map(|page| format!("\"{page}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri("pages/p0/p0.wxml");
    let text = std::fs::read_to_string(project.root().join("pages/p0/p0.wxml")).expect("wxml");
    client.open(&uri, 1, &text);
    let started = std::time::Instant::now();
    let id = client.request(
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    let response = client.response(id);
    let elapsed = started.elapsed();
    assert!(response["result"].is_array());
    assert!(
        elapsed < Duration::from_secs(5),
        "codeLens with 512 project templates took {elapsed:?}"
    );
    assert!(
        response["result"].as_array().expect("lenses").len() <= 64,
        "lens cap"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn workspace_symbol_aggregates_projects_and_imported_templates() {
    let project = TempProject::new("wxml-semantic-workspace-symbols");
    for app in ["a", "b"] {
        project.write(
            &format!("{app}/app.json"),
            r#"{"pages":["pages/index/index"]}"#,
        );
        project.write(&format!("{app}/pages/index/index.ts"), "Page({});");
        project.write(
            &format!("{app}/pages/index/index.wxml"),
            &format!("<template name=\"row-{app}\"><view/></template>"),
        );
    }
    let (mut client, handle) = start_session(
        &project,
        Arc::new(default_responder),
        json!({"apps": [{"root": "a"}, {"root": "b"}]}),
    );
    let invalid = client.request("workspace/symbol", json!({}));
    let invalid = client.response(invalid);
    assert_eq!(invalid["error"]["code"], -32602, "{invalid}");
    let id = client.request("workspace/symbol", json!({"query": "row"}));
    let response = client.response(id);
    let names: Vec<&str> = response["result"]
        .as_array()
        .expect("symbols")
        .iter()
        .filter_map(|symbol| symbol["name"].as_str())
        .collect();
    assert!(names.contains(&"row-a"), "{names:?}");
    assert!(names.contains(&"row-b"), "{names:?}");
    client.finish();
    handle.join().expect("thread").expect("server");

    let project = TempProject::new("wxml-semantic-workspace-import");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({});");
    project.write(
        "pages/index/index.wxml",
        "<import src=\"../../templates/row.wxml\"/><template is=\"row\"/>",
    );
    project.write(
        "templates/row.wxml",
        "<template name=\"row\"><view/></template>",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let id = client.request("workspace/symbol", json!({"query": "row"}));
    let response = client.response(id);
    assert!(
        response["result"]
            .as_array()
            .expect("symbols")
            .iter()
            .any(|symbol| symbol["name"] == "row"),
        "{response}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn closed_script_change_after_analysis_rejects_rename() {
    let project = TempProject::new("wxml-semantic-toctou");
    app_fixture(&project);
    project.write(SCRIPT, "Page({ data: { alpha: 1, beta: 2 } });");
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let gate_responder = Arc::clone(&gate);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let responder: Responder = Arc::new(move |request: &Value| {
        if request["method"] == "symbols" {
            let _ = entered_tx.send(());
            let (lock, condvar) = &*gate_responder;
            let mut released = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while !*released {
                released = condvar
                    .wait(released)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            drop(released);
            return response(&request["id"], symbol_response(&request["params"]));
        }
        default_responder(request)
    });
    let (mut client, handle) = start_session(&project, responder, json!({}));
    let uri = project.uri(WXML);
    let wxml = "{{ alpha }}";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "alpha");
    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "renamed_alpha",
        }),
    );
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("symbol query entered");
    project.write(SCRIPT, "Page({ data: { gamma: 1, alpha: 1, beta: 2 } });");
    let (lock, condvar) = &*gate;
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
    condvar.notify_all();
    let response = client.response(rename);
    assert!(
        matches!(response["error"]["code"].as_i64(), Some(-32801 | -32803)),
        "{response}"
    );
    assert!(response.get("result").is_none(), "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn signature_help_uses_the_document_owner_project() {
    let project = TempProject::new("wxml-semantic-signature-apps");
    for app in ["a", "b"] {
        project.write(
            &format!("{app}/app.json"),
            r#"{"pages":["pages/index/index"]}"#,
        );
        project.write(&format!("{app}/pages/index/index.ts"), "Page({});");
        project.write(
            &format!("{app}/pages/index/index.wxml"),
            "<wxs module=\"u\" src=\"./u.wxs\"/>{{ u.add(1, 2) }}",
        );
        let wxs = if app == "a" {
            "function add(value) { return value; }\nmodule.exports = { add: add };"
        } else {
            "function add(left, right) { return left + right; }\nmodule.exports = { add: add };"
        };
        project.write(&format!("{app}/pages/index/u.wxs"), wxs);
    }
    let (mut client, handle) = start_session(
        &project,
        Arc::new(default_responder),
        json!({"apps": [{"root": "a"}, {"root": "b"}]}),
    );
    for (app, expected) in [("b", 2), ("a", 1)] {
        let uri = project.uri(&format!("{app}/pages/index/index.wxml"));
        let text = "<wxs module=\"u\" src=\"./u.wxs\"/>{{ u.add(1, 2) }}";
        client.open(&uri, 1, text);
        let (line, character) = position_at(text, "u.add(1, ");
        let id = client.request(
            "textDocument/signatureHelp",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character + "u.add(1, ".len() as u64},
            }),
        );
        let response = client.response(id);
        assert_eq!(
            response["result"]["signatures"][0]["parameters"]
                .as_array()
                .expect("parameters")
                .len(),
            expected,
            "{app}: {response}"
        );
    }
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn document_changes_capability_is_negotiated() {
    let project = TempProject::new("wxml-semantic-document-changes");
    app_fixture(&project);
    let (mut client, handle) = start_session_with(
        &project,
        Arc::new(default_responder),
        json!({}),
        json!({"workspace": {"workspaceEdit": {"documentChanges": false}}}),
    );
    let capabilities = &client.initialize_result["result"]["capabilities"];
    assert!(
        capabilities.get("renameProvider").is_none(),
        "{capabilities}"
    );
    assert!(
        capabilities.get("codeActionProvider").is_none(),
        "{capabilities}"
    );
    let uri = project.uri(WXML);
    let wxml = "{{ user.name }}";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[cfg(unix)]
#[test]
fn symlink_and_generated_targets_are_rejected() {
    use std::os::unix::fs::symlink;

    let project = TempProject::new("wxml-semantic-path-boundary");
    app_fixture(&project);
    let outside = std::path::PathBuf::from("/tmp/opencode/wxml-semantic-outside.wxml");
    std::fs::write(&outside, "<template name=\"row\"/>").expect("outside");
    let link = project.root().join("pages/index/link.wxml");
    let _ = std::fs::remove_file(&link);
    symlink(&outside, &link).expect("symlink");
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri("pages/index/link.wxml");
    client.open(&uri, 1, "<template name=\"row\"/>");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": 17},
            "newName": "rowItem",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");

    let generated_uri = project.uri("generated/page.wxml");
    client.open(&generated_uri, 1, "<template name=\"row\"/>");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": generated_uri},
            "position": {"line": 0, "character": 17},
            "newName": "rowItem",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
    let _ = std::fs::remove_file(outside);
}

#[test]
fn component_property_rename_covers_static_include_markup() {
    let project = TempProject::new("wxml-semantic-property-include");
    app_fixture(&project);
    project.write(
        CARD_SCRIPT,
        "Component({ properties: { title: { type: String } } });",
    );
    project.write("pages/index/part.wxml", "<my-card title=\"included\"/>");
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<include src=\"./part.wxml\"/><my-card title=\"caller\"/>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "<my-card title=\"caller\"");
    let title_index = wxml[..wxml.find("<my-card title=\"caller\"").expect("tag") + 12].len();
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": (title_index - wxml[..title_index].rfind('\n').map_or(0, |index| index + 1)) as u64},
            "newName": "heading",
        }),
    );
    let response = client.response(id);
    let caller = edit_for_uri(&response, &uri);
    assert!(
        apply_edits(wxml, caller["edits"].as_array().expect("edits"))
            .contains("heading=\"caller\"")
    );
    let include_uri = project.uri("pages/index/part.wxml");
    let include = edit_for_uri(&response, &include_uri);
    assert_eq!(include["textDocument"]["version"], Value::Null);
    let renamed = apply_edits(
        "<my-card title=\"included\"/>",
        include["edits"].as_array().expect("edits"),
    );
    assert!(renamed.contains("heading=\"included\""), "{renamed}");
    let _ = character;
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn imported_template_bodies_block_event_rename() {
    let project = TempProject::new("wxml-semantic-imported-event");
    project.write("app.json", r#"{"pages":["pages/index/index"]}"#);
    project.write("pages/index/index.ts", "Page({ onTap() {} });");
    project.write(
        "templates/row.wxml",
        "<template name=\"row\"><view bindtap=\"onTap\"/></template>",
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<import src=\"../../templates/row.wxml\"/><view bindtap=\"onTap\"/>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "onTap");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "handleTap",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    assert!(response.get("result").is_none(), "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

fn real_node_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)?
        .to_path_buf();
    let backend_dir = repo.join("backends/typescript");
    let server = backend_dir.join("dist/server.js");
    server.exists().then_some((server, backend_dir))
}

#[test]
#[allow(clippy::too_many_lines)] // one end-to-end LSP session
fn real_node_lsp_component_property_edit_applies_and_parses() {
    let Some((server, backend_dir)) = real_node_paths() else {
        eprintln!("skipping real Node LSP test: build backends/typescript first");
        return;
    };
    let project = TempProject::new("wxml-semantic-real-lsp");
    app_fixture(&project);
    project.write(
        CARD_SCRIPT,
        "Component({ properties: { title: { type: String } } });",
    );
    let (socket, client_stream) = UnixStream::pair().expect("socket");
    let server_read = socket.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::new(
        NodeBackendConfig {
            server: Some(server),
            backend_dir: Some(backend_dir.clone()),
            timeout: Duration::from_secs(20),
            ..NodeBackendConfig::default()
        },
        host.clone(),
    ));
    let handle = thread::spawn(move || run_with_host(server_read, socket, backend, host));
    let mut client = Client::new(client_stream);
    let id = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "capabilities": {"workspace": {"workspaceEdit": {"documentChanges": true}}},
            "initializationOptions": {},
        }),
    );
    let response = client.response(id);
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({}));
    let uri = project.uri(WXML);
    let wxml = "<my-card title=\"x\"></my-card>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "title");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "heading",
        }),
    );
    let response = client.response(id);
    assert!(response.get("result").is_some(), "{response}");
    let caller = edit_for_uri(&response, &uri);
    let renamed = apply_edits(wxml, caller["edits"].as_array().expect("edits"));
    assert!(renamed.contains("heading=\"x\""), "{renamed}");
    assert!(wxml_syntax::parse(&renamed).errors.is_empty());
    let template = edit_for_uri(&response, &project.uri(CARD_WXML));
    let renamed_template = apply_edits(
        "<view>{{ title }}</view>",
        template["edits"].as_array().expect("edits"),
    );
    assert!(
        renamed_template.contains("{{ heading }}"),
        "{renamed_template}"
    );
    assert!(wxml_syntax::parse(&renamed_template).errors.is_empty());

    let script = "Component({ properties: { title: { type: String } } });";
    let script_edits = edit_for_uri(&response, &project.uri(CARD_SCRIPT));
    let renamed_script = apply_edits(
        script,
        script_edits["edits"].as_array().expect("script edits"),
    );
    assert!(renamed_script.contains("heading:"), "{renamed_script}");
    // Compile the applied edits in an isolated copy so the live LSP project
    // (and its overlays) is not modified by the verification step.
    let applied = TempProject::new("wxml-semantic-real-lsp-applied");
    applied.write(CARD_SCRIPT, &renamed_script);
    applied.write(CARD_WXML, &renamed_template);
    applied.write(WXML, &renamed);
    applied.write(
        SCRIPT,
        "Page({ data: { user: { name: 'x' }, count: 1 }, onTap() {} });",
    );
    applied.write(
        "globals.d.ts",
        "declare function Page(options: any): void;\ndeclare function Component(options: any): void;\n",
    );
    let tsc = backend_dir.join("node_modules/typescript/bin/tsc");
    if tsc.exists() {
        let output = std::process::Command::new("node")
            .arg(&tsc)
            .args([
                "--noEmit",
                "--strict",
                "--skipLibCheck",
                "--target",
                "ES2020",
                "--module",
                "CommonJS",
                "globals.d.ts",
                CARD_SCRIPT,
                SCRIPT,
            ])
            .current_dir(applied.root())
            .output()
            .expect("run tsc");
        assert!(
            output.status.success(),
            "tsc failed after applying edits:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        eprintln!("tsc not installed; skipped post-edit type check");
    }

    let (line, character) = position_at(wxml, "<my-card");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 1},
            "newName": "fancy-card",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");

    let script_uri = project.uri(SCRIPT);
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": script_uri,
                "languageId": "typescript",
                "version": 2,
                "text": "Page({ data: { user: { name: 'x' } }, onLoad() { const { user } = this.data; return user.name; } });",
            },
        }),
    );
    let shorthand_wxml = "{{ user.name }}";
    client.change(&uri, 2, shorthand_wxml);
    let (line, character) = position_at(shorthand_wxml, "user");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let response = client.response(id);
    assert_eq!(
        response["error"]["code"], -32803,
        "destructuring shorthand must reject the real Node rename: {response}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn empty_loop_binding_value_is_replaced_not_duplicated() {
    let project = TempProject::new("wxml-semantic-empty-loop");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<view wx:for=\"{{ items }}\" wx:for-item=\"\">{{ item }}</view>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "{{ item }}");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    assert!(response.get("result").is_some(), "{response}");
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    let renamed = apply_edits(wxml, edits);
    assert_eq!(renamed.matches("wx:for-item=").count(), 1, "{renamed}");
    assert!(renamed.contains("wx:for-item=\"entry\""), "{renamed}");
    assert!(wxml_syntax::parse(&renamed).errors.is_empty());
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn data_rename_rejects_component_property_conflict() {
    let project = TempProject::new("wxml-semantic-data-property-conflict");
    app_fixture(&project);
    project.write(
        CARD_SCRIPT,
        "Component({\n  // DATA_FIELDS: user\n  // RESERVED_NAMES: heading\n  properties: { heading: { type: String } },\n  data: { user: { name: 'x' } },\n});",
    );
    project.write(CARD_WXML, "<view>{{ user.name }}</view>");
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(CARD_WXML);
    let wxml = "<view>{{ user.name }}</view>";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "heading",
        }),
    );
    let response = client.response(id);
    assert_eq!(response["error"]["code"], -32803, "{response}");
    assert!(response.get("result").is_none(), "{response}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn oversized_sibling_script_makes_source_snapshot_incomplete() {
    let project = TempProject::new("wxml-semantic-oversized-sibling");
    app_fixture(&project);
    let mut big = String::from("export const big = \"");
    big.push_str(&"x".repeat(17 * 1024 * 1024));
    big.push_str("\";\n");
    project.write("big.ts", &big);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "{{ user.name }}";
    client.open(&uri, 1, wxml);
    let (line, character) = position_at(wxml, "user");
    let prepare = client.request(
        "textDocument/prepareRename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    );
    let prepare = client.response(prepare);
    assert_eq!(prepare["result"], Value::Null, "{prepare}");
    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "newName": "account",
        }),
    );
    let rename = client.response(rename);
    assert_eq!(rename["error"]["code"], -32801, "{rename}");
    assert!(rename.get("result").is_none(), "{rename}");
    let references = client.request(
        "workspace/executeCommand",
        json!({
            "command": "wxml.references",
            "arguments": [{
                "uri": uri,
                "position": {"line": line, "character": character},
                "includeDeclaration": true,
            }],
        }),
    );
    let references = client.response(references);
    assert_eq!(
        references["result"]["coverage"]["status"], "partial",
        "{references}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn component_code_lens_uses_current_document_anchor_for_includes() {
    let project = TempProject::new("wxml-semantic-include-lens");
    app_fixture(&project);
    project.write("pages/index/part.wxml", "<my-card title=\"x\"/>");
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let wxml = "<include src=\"./part.wxml\"/>";
    client.open(&uri, 1, wxml);
    let id = client.request(
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    let response = client.response(id);
    let lens = response["result"]
        .as_array()
        .expect("lenses")
        .iter()
        .find(|lens| lens["data"]["kind"] == "component")
        .expect("component lens");
    assert_eq!(lens["range"]["start"]["line"], 0, "{lens}");
    assert_eq!(lens["range"]["start"]["character"], 1, "{lens}");
    assert_eq!(lens["data"]["fileCount"], 0, "{lens}");
    assert_eq!(lens["data"]["includedCount"], 1, "{lens}");
    assert_eq!(lens["data"]["complete"], true, "{lens}");
    let title = lens["command"]["title"].as_str().expect("title");
    assert!(title.contains("in included files"), "{title}");
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn code_lens_with_shared_import_is_bounded() {
    use std::fmt::Write as _;

    let project = TempProject::new("wxml-semantic-import-lens");
    let mut pages = Vec::new();
    for page in 0..64 {
        let route = format!("pages/p{page}/p{page}");
        pages.push(route.clone());
        project.write(&format!("{route}.ts"), "Page({});");
        if page == 0 {
            let mut wxml = String::new();
            for index in 0..64 {
                write!(
                    wxml,
                    "<import src=\"../../templates/common.wxml\"/><template name=\"r{index}\"><view/></template><template is=\"r{index}\"/>"
                )
                .expect("write");
            }
            project.write(&format!("{route}.wxml"), &wxml);
        } else {
            project.write(
                &format!("{route}.wxml"),
                "<import src=\"../../templates/common.wxml\"/><view/>",
            );
        }
    }
    project.write(
        "app.json",
        &format!(
            "{{\"pages\":[{}]}}",
            pages
                .iter()
                .map(|page| format!("\"{page}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    let mut common = String::from("<template name=\"shared\"><view>");
    common.push_str(&"x".repeat(64 * 1024));
    common.push_str("</view></template>");
    project.write("templates/common.wxml", &common);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri("pages/p0/p0.wxml");
    let text = std::fs::read_to_string(project.root().join("pages/p0/p0.wxml")).expect("wxml");
    client.open(&uri, 1, &text);
    let started = std::time::Instant::now();
    let id = client.request(
        "textDocument/codeLens",
        json!({"textDocument": {"uri": uri}}),
    );
    let response = client.response(id);
    let elapsed = started.elapsed();
    let lenses = response["result"].as_array().expect("lenses");
    assert_eq!(lenses.len(), 64, "{response}");
    assert!(
        elapsed < Duration::from_secs(2),
        "codeLens with 64 shared imports took {elapsed:?}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn workspace_symbol_with_many_imports_is_bounded() {
    use std::fmt::Write as _;

    let project = TempProject::new("wxml-semantic-import-symbols");
    let mut pages = Vec::new();
    for page in 0..20 {
        let route = format!("pages/p{page}/p{page}");
        pages.push(route.clone());
        project.write(&format!("{route}.ts"), "Page({});");
        let mut wxml = String::new();
        for index in 0..20 {
            write!(wxml, "<import src=\"../../templates/t{index}.wxml\"/>").expect("write");
        }
        wxml.push_str("<view/>");
        project.write(&format!("{route}.wxml"), &wxml);
    }
    for index in 0..20 {
        let mut template = format!("<template name=\"t{index}\"><view>");
        template.push_str(&"x".repeat(256 * 1024));
        template.push_str("</view></template>");
        project.write(&format!("templates/t{index}.wxml"), &template);
    }
    project.write(
        "app.json",
        &format!(
            "{{\"pages\":[{}]}}",
            pages
                .iter()
                .map(|page| format!("\"{page}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let started = std::time::Instant::now();
    let id = client.request("workspace/symbol", json!({"query": "zzz"}));
    let response = client.response(id);
    let elapsed = started.elapsed();
    assert!(response["result"].is_array(), "{response}");
    assert!(
        elapsed < Duration::from_secs(3),
        "workspace/symbol with shared imports took {elapsed:?}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn loop_rename_is_linear_for_many_directives() {
    let project = TempProject::new("wxml-semantic-quadratic");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let mut wxml = String::with_capacity(60_000);
    for _ in 0..1_000 {
        wxml.push_str("<view wx:for=\"{{ items }}\" wx:for-item=\"item\">{{ item }}</view>");
    }
    client.open(&uri, 1, &wxml);
    let (line, character) = position_at(&wxml, "{{ item }}");
    let started = std::time::Instant::now();
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    let elapsed = started.elapsed();
    assert!(response.get("result").is_some(), "{response}");
    let edits = edit_for_uri(&response, &uri)["edits"]
        .as_array()
        .expect("edits");
    assert_eq!(edits.len(), 2, "binding plus the target loop body");
    assert!(
        elapsed < Duration::from_secs(1),
        "1000-directive rename took {elapsed:?}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
fn loop_rename_stays_bounded_after_the_fragment_budget() {
    let project = TempProject::new("wxml-semantic-quadratic-budget");
    app_fixture(&project);
    let (mut client, handle) = start_session(&project, Arc::new(default_responder), json!({}));
    let uri = project.uri(WXML);
    let mut wxml = String::with_capacity(480_000);
    for _ in 0..8_000 {
        wxml.push_str("<view wx:for=\"{{ items }}\" wx:for-item=\"item\">{{ item }}</view>");
    }
    client.open(&uri, 1, &wxml);
    let (line, character) = position_at(&wxml, "{{ item }}");
    let started = std::time::Instant::now();
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 4},
            "newName": "entry",
        }),
    );
    let response = client.response(id);
    let elapsed = started.elapsed();
    assert!(
        response.get("result").is_some() || response["error"]["code"] == -32803,
        "{response}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "8000-directive rename took {elapsed:?}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}

#[test]
#[allow(clippy::too_many_lines)] // one end-to-end LSP session covers four rejections
fn real_node_lsp_rejects_cross_namespace_and_dynamic_edits() {
    let Some((server, backend_dir)) = real_node_paths() else {
        eprintln!("skipping real Node LSP test: build backends/typescript first");
        return;
    };
    let project = TempProject::new("wxml-semantic-real-lsp-conflicts");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{"my-card":"/components/card/card"}}"#,
    );
    project.write("components/card/card.json", r#"{"component":true}"#);
    project.write(
        CARD_SCRIPT,
        "Component({\n  properties: { title: { type: String }, heading: { type: String } },\n  data: { local: 'x', user: { name: 'x' } },\n  methods: { onTap() {} },\n});",
    );
    project.write(CARD_WXML, "<view>{{ title }} {{ user.name }}</view>");
    project.write(SCRIPT, "Page({ data: { user: { name: 'x' }, alpha: 1 } });");
    project.write(
        "other.ts",
        "export function update(page: any) { page.setData({ user: { name: 'y' } }); }",
    );
    let (socket, client_stream) = UnixStream::pair().expect("socket");
    let server_read = socket.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::new(
        NodeBackendConfig {
            server: Some(server),
            backend_dir: Some(backend_dir),
            timeout: Duration::from_secs(20),
            ..NodeBackendConfig::default()
        },
        host.clone(),
    ));
    let handle = thread::spawn(move || run_with_host(server_read, socket, backend, host));
    let mut client = Client::new(client_stream);
    let id = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "capabilities": {"workspace": {"workspaceEdit": {"documentChanges": true}}},
            "initializationOptions": {},
        }),
    );
    let response = client.response(id);
    assert!(response.get("error").is_none(), "{response}");
    client.notify("initialized", json!({}));

    let main_uri = project.uri(WXML);
    let caller = "<my-card title=\"x\"></my-card>";
    client.open(&main_uri, 1, caller);
    let (line, character) = position_at(caller, "title");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": main_uri},
            "position": {"line": line, "character": character},
            "newName": "local",
        }),
    );
    let response = client.response(id);
    assert_eq!(
        response["error"]["code"], -32803,
        "property -> component data must reject: {response}"
    );

    let card_uri = project.uri(CARD_WXML);
    let card = "<view>{{ user.name }}</view>";
    client.open(&card_uri, 1, card);
    let (line, character) = position_at(card, "user");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": card_uri},
            "position": {"line": line, "character": character},
            "newName": "heading",
        }),
    );
    let response = client.response(id);
    assert_eq!(
        response["error"]["code"], -32803,
        "component data -> property must reject: {response}"
    );

    let page = "{{ user.name }}";
    client.change(&main_uri, 2, page);
    let (line, character) = position_at(page, "name");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": main_uri},
            "position": {"line": line, "character": character},
            "newName": "title",
        }),
    );
    let response = client.response(id);
    assert_eq!(
        response["error"]["code"], -32803,
        "another script's nested setData must reject: {response}"
    );

    let script_uri = project.uri(SCRIPT);
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": script_uri,
                "languageId": "typescript",
                "version": 3,
                "text": "Page({ data: { user: { name: 'x' }, alpha: 1 }, onLoad() { const that = this; const key = 'alpha'; return that.data[key]; } });",
            },
        }),
    );
    let alias = "{{ alpha }}";
    client.change(&main_uri, 4, alias);
    let (line, character) = position_at(alias, "alpha");
    let id = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": main_uri},
            "position": {"line": line, "character": character},
            "newName": "gamma",
        }),
    );
    let response = client.response(id);
    assert_eq!(
        response["error"]["code"], -32803,
        "this-alias dynamic data access must reject: {response}"
    );
    client.finish();
    handle.join().expect("thread").expect("server");
}
