//! In-process LSP client used by every query benchmark.
//!
//! A benchmark sample is one full request/response round trip through the real
//! server event loop: JSON-RPC framing, dispatch, workspace lookup, shadow
//! mapping and the engine/style query behind it. Process start and the
//! installed-binary transport are deliberately outside the measured window;
//! the cold (`first-query/*`) group adds session setup back in.
//!
//! The server is served over a `UnixStream` pair with
//! [`wxml_lsp::run_with_host`], exactly like the integration tests, so the
//! benchmark measures production code paths rather than a test-only entry.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use wxml_lsp::bench_support::BenchBackend;
use wxml_lsp::{ServerError, ServerOutcome, run_with_host};

/// Upper bound for one framed message. Exceeding it aborts the benchmark
/// instead of hanging.
const TIMEOUT: Duration = Duration::from_secs(60);

/// A live server plus the client half of its socket pair.
pub(crate) struct Session {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    handle: Option<JoinHandle<Result<ServerOutcome, ServerError>>>,
}

impl Session {
    /// Starts a server with `backend` and `root` as the workspace root.
    ///
    /// Diagnostics are published without debounce (`debounceMs: 0`) so the
    /// first-query group does not silently measure a timer instead of work.
    pub(crate) fn start(backend: BenchBackend, root: &Path) -> Self {
        let (server, client) = UnixStream::pair().expect("socket pair");
        let server_read = server.try_clone().expect("clone server stream");
        client
            .set_read_timeout(Some(TIMEOUT))
            .expect("client read timeout");
        let handle = thread::spawn(move || {
            run_with_host(server_read, server, backend.backend, backend.host)
        });
        let mut session = Self {
            writer: client.try_clone().expect("clone client stream"),
            reader: BufReader::new(client),
            next_id: 1,
            handle: Some(handle),
        };
        let id = session.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": uri_for(root),
                "capabilities": {},
                "initializationOptions": {
                    "wxml": {"diagnostics": {"debounceMs": 0}},
                },
            }),
        );
        let response = session.response(id);
        assert!(
            response.get("error").is_none(),
            "initialize failed: {response}"
        );
        session.notify("initialized", json!({}));
        session
    }

    /// Opens `path` as an overlay with the fixture text and waits until the
    /// first diagnostics for it have been published.
    pub(crate) fn open(&mut self, path: &Path, text: &str) {
        let uri = uri_for(path);
        self.notify(
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
        self.wait_diagnostics(&uri);
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialize message");
        write!(self.writer, "Content-Length: {}\r\n\r\n", body.len()).expect("frame header");
        self.writer.write_all(&body).expect("frame body");
        self.writer.flush().expect("frame flush");
    }

    /// Sends a request and returns its id without waiting.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> i64 {
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
    pub(crate) fn notify(&mut self, method: &str, params: Value) {
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

    /// Waits for the response with `id`, buffering notifications.
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

    /// Waits until the diagnostics for `uri` have been published.
    fn wait_diagnostics(&mut self, uri: &str) {
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
                && message.pointer("/params/uri").and_then(Value::as_str) == Some(uri)
            {
                return;
            }
        }
    }

    /// Sends `method` and returns its `result` value.
    ///
    /// # Panics
    ///
    /// Panics when the server answers with a JSON-RPC error.
    pub(crate) fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        let response = self.response(id);
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response.get("result").cloned().unwrap_or(Value::Null)
    }

    /// Shuts the server down and joins its thread.
    pub(crate) fn shutdown(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if self.handle.is_none() {
            return;
        }
        let id = self.request("shutdown", Value::Null);
        let _ = self.response(id);
        self.notify("exit", Value::Null);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Returns the `file://` URI of an absolute path.
pub(crate) fn uri_for(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// Builds a `textDocument/position` parameter.
pub(crate) fn position_params(uri: &str, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": line, "character": character},
    })
}
