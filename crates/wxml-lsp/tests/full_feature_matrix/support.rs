//! Shared JSON-RPC session and text/position helpers for the full-feature
//! integration matrix.
//!
//! Every test talks to the real [`wxml_lsp::run_with_host`] event loop over a
//! `UnixStream` pair. TCP/stdio framing, request ids, diagnostics
//! notifications, document synchronization and capability dispatch are the
//! production paths; only the type backend is substituted.

#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::unwrap_used,
    dead_code,
    unreachable_pub
)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use wxml_lsp::ServerOutcome;
use wxml_lsp::project_host::{ProjectHost, ProjectState};
use wxml_lsp::run_with_host;
use wxml_types::Backend;

const TIMEOUT: Duration = Duration::from_secs(15);

/// A temporary project rooted under `TMPDIR` (or `/tmp/opencode`), removed on
/// drop.
pub struct TempProject {
    dir: tempfile::TempDir,
}

impl TempProject {
    /// Creates a unique project directory below `TMPDIR` (or `/tmp/opencode`).
    pub fn new(prefix: &str) -> Self {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(base)
            .expect("temp project");
        Self { dir }
    }

    /// Returns the project root.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Writes one project file, creating parent directories.
    pub fn write(&self, path: &str, text: &str) {
        let target = self.root().join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(target, text).expect("fixture");
    }

    /// Removes one project file, ignoring an already absent path.
    pub fn remove(&self, path: &str) {
        let target = self.root().join(path);
        match std::fs::remove_file(target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove fixture: {error}"),
        }
    }

    /// Returns the `file://` URI of the project root.
    pub fn root_uri(&self) -> String {
        format!("file://{}", self.root().display())
    }

    /// Returns the `file://` URI of a project-relative path.
    pub fn uri(&self, path: &str) -> String {
        format!("file://{}", self.root().join(path).display())
    }
}

/// A live LSP session backed by the real server event loop.
pub struct Session {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    handle: Option<thread::JoinHandle<Result<ServerOutcome, wxml_lsp::ServerError>>>,
    diagnostics: VecDeque<Value>,
    /// The real project host shared with the server.
    pub host: Arc<ProjectState>,
    /// The `initialize` result.
    pub initialize: Value,
}

impl Session {
    /// Starts a session with the in-process mock backend.
    pub fn start(project: &TempProject, options: Value) -> Self {
        Self::start_with(project, options, json!({}), |_| {
            Arc::new(wxml_types::MockBackend::new()) as Arc<dyn Backend>
        })
    }

    /// Starts a session with a backend factory that can see the real host.
    pub fn start_with<F>(
        project: &TempProject,
        options: Value,
        capabilities: Value,
        factory: F,
    ) -> Self
    where
        F: FnOnce(Arc<ProjectState>) -> Arc<dyn Backend>,
    {
        let (server, client) = UnixStream::pair().expect("socket pair");
        let server_read = server.try_clone().expect("clone server stream");
        client
            .set_read_timeout(Some(TIMEOUT))
            .expect("client read timeout");
        let host = Arc::new(ProjectState::new());
        let backend = factory(Arc::clone(&host));
        let server_host: Arc<dyn ProjectHost> = host.clone();
        let handle =
            thread::spawn(move || run_with_host(server_read, server, backend, server_host));
        let mut session = Self {
            writer: client.try_clone().expect("clone client stream"),
            reader: BufReader::new(client),
            next_id: 1,
            handle: Some(handle),
            diagnostics: VecDeque::new(),
            host,
            initialize: Value::Null,
        };
        let id = session.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": project.root_uri(),
                "capabilities": capabilities,
                "initializationOptions": options,
            }),
        );
        let response = session.response(id);
        assert!(
            response.get("error").is_none(),
            "initialize failed: {response}"
        );
        session.initialize = response["result"].clone();
        session.notify("initialized", json!({}));
        session
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize request");
        write!(self.writer, "Content-Length: {}\r\n\r\n", body.len()).expect("frame header");
        self.writer.write_all(&body).expect("frame body");
        self.writer.flush().expect("frame flush");
    }

    /// Sends a request and returns its id.
    pub fn request(&mut self, method: &str, params: Value) -> i64 {
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

    /// Sends a notification.
    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
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
        let length = length.expect("Content-Length");
        let mut body = vec![0_u8; length];
        self.reader.read_exact(&mut body).expect("body read");
        serde_json::from_slice(&body).expect("message json")
    }

    /// Waits for the response with `id`, skipping notifications and server
    /// requests in between. Diagnostics seen on the way are kept for a later
    /// [`Self::diagnostics_version`] call instead of being dropped.
    pub fn response(&mut self, id: i64) -> Value {
        loop {
            let message = self.recv();
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                self.diagnostics.push_back(message);
            }
        }
    }

    /// Waits for the response with `id` and returns its `result`.
    pub fn result(&mut self, id: i64) -> Value {
        let response = self.response(id);
        response
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("missing result for {id}: {response}"))
    }

    /// Opens a WXML document.
    pub fn open_wxml(&mut self, uri: &str, text: &str, version: u64) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": version,
                    "text": text,
                },
            }),
        );
    }

    /// Replaces an open document's full text.
    pub fn change(&mut self, uri: &str, text: &str, version: u64) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    /// Waits for diagnostics published for `uri` at `version`.
    pub fn diagnostics_version(&mut self, uri: &str, version: u64) -> Value {
        if let Some(index) = self.diagnostics.iter().position(|message| {
            message["params"]["uri"] == json!(uri) && message["params"]["version"] == json!(version)
        }) {
            return self.diagnostics.remove(index).expect("queued diagnostics");
        }
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                if message["params"]["uri"] == json!(uri)
                    && message["params"]["version"] == json!(version)
                {
                    return message;
                }
                self.diagnostics.push_back(message);
            }
        }
    }

    /// Waits for the next diagnostics published for `uri`.
    pub fn diagnostics(&mut self, uri: &str) -> Value {
        if let Some(index) = self
            .diagnostics
            .iter()
            .position(|message| message["params"]["uri"] == json!(uri))
        {
            return self.diagnostics.remove(index).expect("queued diagnostics");
        }
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                if message["params"]["uri"] == json!(uri) {
                    return message;
                }
                self.diagnostics.push_back(message);
            }
        }
    }

    /// Shuts the server down, waits for the thread and asserts a clean exit.
    pub fn finish(mut self) -> ServerOutcome {
        let id = self.request("shutdown", Value::Null);
        let response = self.response(id);
        assert_eq!(response["result"], Value::Null, "{response}");
        self.notify("exit", Value::Null);
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
        let handle = self.handle.take().expect("server thread");
        let outcome = handle
            .join()
            .expect("server thread")
            .expect("server outcome");
        assert!(outcome.clean_shutdown, "unclean shutdown: {outcome:?}");
        outcome
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
        // Detach instead of joining: a panicking test must not hang here.
        self.handle.take();
    }
}

/// Converts a byte offset into an LSP UTF-16 position.
pub fn position_at(text: &str, offset: usize) -> Value {
    assert!(
        text.is_char_boundary(offset),
        "offset {offset} not a boundary"
    );
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let mut content = &text[line_start..offset];
    if let Some(stripped) = content.strip_suffix('\r') {
        content = stripped;
    }
    json!({"line": line, "character": content.encode_utf16().count()})
}

/// Converts an LSP position into a byte offset.
pub fn offset_at(text: &str, position: &Value) -> usize {
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

/// Applies ordered LSP text edits and returns the new text.
pub fn apply_edits(text: &str, edits: &Value) -> String {
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

/// Expands the simple `$n` / `${n:default}` snippet placeholders used by the
/// server's completion items.
pub fn expand_snippet(body: &str) -> String {
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
                let default = inner.split_once(':').map(|(_, value)| value);
                if let Some(default) = default {
                    output.push_str(default);
                }
            }
            Some('0'..='9') => {
                chars.next();
            }
            _ => output.push('$'),
        }
    }
    output
}

/// Applies one completion item's `textEdit` to `text`.
pub fn apply_completion_item(text: &str, item: &Value) -> String {
    let edit = item
        .get("textEdit")
        .unwrap_or_else(|| panic!("completion item has no textEdit: {item}"));
    let start = offset_at(text, &edit["range"]["start"]);
    let end = offset_at(text, &edit["range"]["end"]);
    let mut new_text = edit["newText"].as_str().expect("newText").to_owned();
    if item["insertTextFormat"] == json!(2) {
        new_text = expand_snippet(&new_text);
    }
    let mut output = String::with_capacity(text.len());
    output.push_str(&text[..start]);
    output.push_str(&new_text);
    output.push_str(&text[end..]);
    output
}

/// Asserts that `text` parses without errors or truncation.
pub fn assert_parses_cleanly(text: &str) {
    let parsed = wxml_syntax::parse(text);
    assert!(
        parsed.errors.is_empty() && !parsed.is_truncated(),
        "invalid WXML after edit: {text:?} ({:?})",
        parsed.errors
    );
}

/// Returns completion labels from a completion `result`.
pub fn labels(result: &Value) -> Vec<String> {
    result["items"]
        .as_array()
        .expect("completion items")
        .iter()
        .filter_map(|item| item["label"].as_str().map(str::to_owned))
        .collect()
}

/// Returns one completion item by label.
pub fn item<'a>(result: &'a Value, label: &str) -> &'a Value {
    result["items"]
        .as_array()
        .expect("completion items")
        .iter()
        .find(|item| item["label"] == json!(label))
        .unwrap_or_else(|| panic!("missing completion item {label}: {result}"))
}

/// Returns diagnostic codes in a publish payload.
pub fn diagnostic_codes(notification: &Value) -> Vec<String> {
    notification["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|diagnostic| diagnostic["code"].as_str().map(str::to_owned))
        .collect()
}

/// Returns diagnostic messages in a publish payload.
pub fn diagnostic_messages(notification: &Value) -> Vec<String> {
    notification["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter_map(|diagnostic| diagnostic["message"].as_str().map(str::to_owned))
        .collect()
}

/// Returns document-link target strings.
pub fn link_targets(links: &Value) -> Vec<String> {
    links
        .as_array()
        .expect("document links")
        .iter()
        .filter_map(|link| link["target"].as_str().map(str::to_owned))
        .collect()
}

/// Returns definition location URIs.
pub fn definition_uris(locations: &Value) -> Vec<String> {
    locations
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|location| location["uri"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
