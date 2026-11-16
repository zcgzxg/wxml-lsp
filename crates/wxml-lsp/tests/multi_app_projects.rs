#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

//! Explicit multi-app behavior over real JSON-RPC framing.
//!
//! The in-process sessions run the real [`ProjectState`] and a `NodeBackend`
//! whose transport is fake, so routing, overlays, diagnostics and definition
//! mapping are exercised end to end without a Node process. The binary test
//! starts `wxml-lsp --stdio` and verifies that invalid initialization options
//! return `-32602` without entering the running state.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, Stdio};
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

fn app_fixture(root: &TempProject, app: &str) {
    root.write(
        &format!("{app}/app.json"),
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    root.write(&format!("{app}/{WXML}"), "{{ user.name }}");
    root.write(
        &format!("{app}/{SCRIPT}"),
        &format!(
            "Page({{ data: {{ user: {{ name: 'DISK_{}' }} }} }});",
            app.to_uppercase()
        ),
    );
}

type Responder = Arc<dyn Fn(&str) -> Result<String, NodeTransportError> + Send + Sync>;

struct Fake {
    requests: Arc<Mutex<Vec<String>>>,
    responder: Responder,
}

impl NodeTransport for Fake {
    fn request(&self, line: &str, _timeout: Duration) -> Result<String, NodeTransportError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_owned());
        (self.responder)(line)
    }

    fn shutdown(&self) {}
}

fn marker_from_files(params: &Value) -> String {
    let mut marker = "disk".to_owned();
    let Some(files) = params["files"].as_array() else {
        return marker;
    };
    for file in files {
        let Some(text) = file["text"].as_str() else {
            continue;
        };
        for word in
            text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            if word.starts_with("ZED_") || word.starts_with("DISK_") {
                word.clone_into(&mut marker);
            }
        }
    }
    marker
}

fn respond(line: &str) -> String {
    let request: Value = serde_json::from_str(line).expect("request json");
    let id = &request["id"];
    let params = &request["params"];
    let request_id = params["requestId"].as_str().expect("requestId");
    let project_revision = params["analysisStamp"]["projectRevision"]
        .as_u64()
        .expect("projectRevision");
    let view_revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .expect("viewRevision");
    let script_file = params["scriptFileId"].as_str().expect("scriptFileId");
    let marker = marker_from_files(params);
    let fragments = params["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|fragment| {
            let start = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3) + 5;
            json!({
                "fragmentId": fragment["fragmentId"],
                "hover": {
                    "typeText": marker,
                    "typeOnly": marker,
                    "quality": "known",
                    "documentation": format!("{marker}:{project_revision}:{view_revision}"),
                    "documentationSource": "quickinfo"
                },
                "diagnostics": [{
                    "code": 2339,
                    "range": {"start": start, "end": start + 4},
                    "message": format!("{marker}:{project_revision}:{view_revision}"),
                    "attribution": "source"
                }],
                "definition": {
                    "kind": "script",
                    "fileId": script_file,
                    "range": {"start": 0, "end": 4}
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": 2,
            "requestId": request_id,
            "jsCheckApplied": "pragma",
            "registration": {"kind": "page", "count": 1, "topLevel": true},
            "states": {
                "hover": {"status": "supported"},
                "diagnostics": {"status": "supported"},
                "definition": {"status": "supported"}
            },
            "insertions": [{"viewRevision": view_revision, "backendFileVersion": 1}],
            "mounts": [],
            "fragments": fragments,
            "moduleKind": {"before": "script", "after": "script"},
            "suppressed": {"count": 0, "details": []}
        }
    }))
    .expect("response json")
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

    fn diagnostics_for(&mut self, uri: &str) -> Value {
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
                && message["params"]["uri"].as_str() == Some(uri)
            {
                return message;
            }
        }
    }

    fn hover_text(&mut self, uri: &str) -> String {
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 10}
            }),
        );
        let response = self.response(id);
        response["result"]["contents"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn finish(mut self) {
        let shutdown = self.request("shutdown", json!(null));
        assert_eq!(self.response(shutdown)["result"], json!(null));
        self.notify("exit", json!(null));
    }
}

fn diagnostic_message(notification: &Value) -> &str {
    notification["params"]["diagnostics"][0]["message"]
        .as_str()
        .expect("diagnostic message")
}

fn open_wxml(client: &mut Client, uri: &str) {
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": "{{ user.name }}"
            }
        }),
    );
}

fn start_session(
    project: &TempProject,
    initialization_options: Value,
) -> (
    Client,
    thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>,
) {
    let (server, client_stream) = UnixStream::pair().expect("socket pair");
    let server_read = server.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        || {
            Ok(Box::new(Fake {
                requests: Arc::new(Mutex::new(Vec::new())),
                responder: Arc::new(|line| Ok(respond(line))),
            }))
        },
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server, backend, host));
    let mut client = Client::new(client_stream);
    let initialize = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "initializationOptions": initialization_options
        }),
    );
    let response = client.response(initialize);
    assert!(
        response.get("error").is_none(),
        "initialize failed: {response}"
    );
    client.notify("initialized", json!({}));
    (client, handle)
}

#[test]
fn explicit_apps_isolate_same_paths_overlays_and_definitions() {
    let project = TempProject::new("wxml-multi-app");
    app_fixture(&project, "a");
    app_fixture(&project, "b");
    let (mut client, handle) =
        start_session(&project, json!({"apps": [{"root": "a"}, {"root": "b"}]}));

    let a_wxml = project.uri(&format!("a/{WXML}"));
    let b_wxml = project.uri(&format!("b/{WXML}"));
    open_wxml(&mut client, &b_wxml);
    assert!(diagnostic_message(&client.diagnostics_for(&b_wxml)).starts_with("DISK_B:"));
    open_wxml(&mut client, &a_wxml);
    assert!(diagnostic_message(&client.diagnostics_for(&a_wxml)).starts_with("DISK_A:"));

    // Configuration is consumed at initialize only; this notification must
    // not change the active app roots or enter a hot-update path.
    client.notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"initializationOptions": {"apps": [{"root": "b"}]}}}),
    );

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": project.uri(&format!("a/{SCRIPT}")),
                "languageId": "typescript",
                "version": 1,
                "text": "Page({ data: { user: { name: 'ZED_A_OVERLAY' } } });"
            }
        }),
    );
    assert!(diagnostic_message(&client.diagnostics_for(&a_wxml)).starts_with("ZED_A_OVERLAY:"));
    let b_hover = client.hover_text(&b_wxml);
    assert!(
        b_hover.contains("DISK_B") && !b_hover.contains("ZED_A_OVERLAY"),
        "app B must not see app A's overlay: {b_hover:?}"
    );

    for (uri, script) in [
        (&a_wxml, format!("a/{SCRIPT}")),
        (&b_wxml, format!("b/{SCRIPT}")),
    ] {
        let definition = client.request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 10}
            }),
        );
        let definition = client.response(definition);
        assert_eq!(
            definition["result"][0]["uri"],
            json!(project.uri(&script)),
            "definition must stay in the owning app"
        );
    }

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn unsaved_app_json_removal_clears_the_page_diagnostics() {
    let project = TempProject::new("wxml-json-removal");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write(WXML, "{{ user.name }}");
    project.write(SCRIPT, "Page({ data: { user: { name: 'DISK_JSON' } } });");
    let (mut client, handle) = start_session(&project, json!({"apps": [{"root": "."}]}));
    let wxml = project.uri(WXML);
    open_wxml(&mut client, &wxml);
    let diagnostics = client.diagnostics_for(&wxml);
    assert!(
        !diagnostics["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty()
    );

    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": project.uri("app.json"),
                "languageId": "json",
                "version": 1,
                "text": r#"{"pages":[]}"#
            }
        }),
    );
    let cleared = client.diagnostics_for(&wxml);
    let diagnostics = cleared["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert!(
        diagnostics.iter().all(|diagnostic| !diagnostic["message"]
            .as_str()
            .unwrap_or_default()
            .contains("DISK_JSON")),
        "old type diagnostics must be cleared: {cleared}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["severity"] != json!(1)),
        "a removed page must not keep error diagnostics: {cleared}"
    );

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": project.uri("app.json")}}),
    );
    let restored = client.diagnostics_for(&wxml);
    assert!(
        !restored["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .is_empty(),
        "closing the overlay must restore the page"
    );

    client.finish();
    let outcome = handle.join().expect("server thread").expect("outcome");
    assert!(outcome.clean_shutdown);
}

#[test]
fn stdio_binary_routes_two_explicit_apps_for_syntax_diagnostics() {
    let project = TempProject::new("wxml-multi-app-stdio");
    for app in ["a", "b"] {
        project.write(&format!("{app}/app.json"), r#"{"pages":[]}"#);
    }
    let mut client = ProcessClient::start();
    let id = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "initializationOptions": {"apps": [{"root": "a"}, {"root": "b"}]}
        }),
    );
    let response = client.response(id);
    assert!(response.get("result").is_some(), "{response}");
    client.notify("initialized", json!({}));
    for (app, text) in [("a", "<view>"), ("b", "<view><view>")] {
        client.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": project.uri(&format!("{app}/{WXML}")),
                    "languageId": "wxml",
                    "version": 1,
                    "text": text
                }
            }),
        );
        let notification = client.notification("textDocument/publishDiagnostics");
        assert_eq!(
            notification["params"]["uri"],
            json!(project.uri(&format!("{app}/{WXML}")))
        );
        assert!(
            !notification["params"]["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .is_empty(),
            "unclosed markup must publish a syntax diagnostic in app {app}"
        );
    }
    let shutdown = client.request("shutdown", json!(null));
    assert_eq!(client.response(shutdown)["result"], json!(null));
    client.send(&json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
    let _ = client.child.wait();
}

/// A minimal framed client for a spawned binary.
struct ProcessClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    next_id: i64,
}

impl ProcessClient {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wxml-lsp"))
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn wxml-lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("json");
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        self.stdin.write_all(&body).expect("body");
        self.stdin.flush().expect("flush");
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

    fn notification(&mut self, method: &str) -> Value {
        loop {
            let mut length = None;
            loop {
                let mut line = String::new();
                let read = self.reader.read_line(&mut line).expect("header");
                assert!(read != 0, "server closed stdout");
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
            let message: Value = serde_json::from_slice(&body).expect("json");
            if message.get("method").and_then(Value::as_str) == Some(method) {
                return message;
            }
        }
    }

    fn response(&mut self, id: i64) -> Value {
        loop {
            let mut length = None;
            loop {
                let mut line = String::new();
                let read = self.reader.read_line(&mut line).expect("header");
                assert!(read != 0, "server closed stdout");
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
            let message: Value = serde_json::from_slice(&body).expect("json");
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
        }
    }
}

#[test]
fn stdio_binary_rejects_invalid_initialization_options_with_invalid_params() {
    let project = TempProject::new("wxml-init-errors");
    project.write("app.json", r#"{"pages":[]}"#);
    project.write("app/pages/.keep", "");
    let mut client = ProcessClient::start();
    let absolute = project.root().to_string_lossy().into_owned();
    let invalid = vec![
        json!({"apps": "src"}),
        json!({"apps": [{"root": absolute}]}),
        json!({"apps": [{"root": "../outside"}]}),
        json!({"apps": [{"root": "missing"}]}),
        json!({"apps": [{"root": "."}, {"root": "./"}]}),
        json!({"apps": [{"root": ".", "projectRoot": "app"}]}),
        json!({"apps": vec![json!({"root": "."}); 65]}),
    ];
    for options in invalid {
        let id = client.request(
            "initialize",
            json!({"processId": null, "rootUri": project.root_uri(), "initializationOptions": options}),
        );
        let response = client.response(id);
        assert_eq!(
            response["error"]["code"],
            json!(-32602),
            "invalid options were accepted: {response}"
        );
    }

    let id = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "initializationOptions": {"apps": [{"root": "."}]}
        }),
    );
    let response = client.response(id);
    assert!(
        response.get("result").is_some(),
        "valid initialize after failures: {response}"
    );
    let shutdown = client.request("shutdown", json!(null));
    assert_eq!(client.response(shutdown)["result"], json!(null));
    client.send(&json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
    let _ = client.child.wait();
}
