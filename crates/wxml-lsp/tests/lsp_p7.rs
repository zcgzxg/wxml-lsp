#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

//! End-to-end P7 protocol tests over an in-process `UnixStream` pair.
//!
//! The server runs with a real `ProjectState` and a `NodeBackend` whose
//! transport is fake, so the tests cover the full LSP -> host -> C1 request ->
//! response -> LSP path without a Node process.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
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

fn fixture() -> TempProject {
    let project = TempProject::new("wxml-p7-lsp");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write("pages/index/index.wxml", "{{ user.name }}");
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    project
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

fn respond(line: &str) -> String {
    let request: Value = serde_json::from_str(line).expect("request json");
    let id = &request["id"];
    let params = &request["params"];
    let request_id = params["requestId"].as_str().expect("requestId");
    let revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .expect("viewRevision");
    let fragments = params["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|fragment| {
            let start = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3) + 5;
            json!({
                "fragmentId": fragment["fragmentId"],
                "hover": {
                    "typeText": "string",
                    "typeOnly": "string",
                    "quality": "known",
                    "documentation": "the name",
                    "documentationSource": "quickinfo"
                },
                "diagnostics": [{
                    "code": 2339,
                    "range": {"start": start, "end": start + 4},
                    "message": "Property 'nmae' does not exist",
                    "attribution": "source"
                }],
                "definition": {
                    "kind": "script",
                    "fileId": "pages/index/index.ts",
                    "range": {"start": 0, "end": 4}
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": 1,
            "requestId": request_id,
            "jsCheckApplied": "pragma",
            "registration": {"kind": "page", "count": 1, "topLevel": true},
            "states": {
                "hover": {"status": "supported"},
                "diagnostics": {"status": "supported"},
                "definition": {"status": "supported"}
            },
            "insertions": [{"viewRevision": revision, "backendFileVersion": 1}],
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

    fn notification(&mut self, method: &str) -> Value {
        loop {
            let message = self.recv();
            if message.get("method").and_then(Value::as_str) == Some(method) {
                return message;
            }
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn project_flow_serves_diagnostics_hover_and_cross_file_definition() {
    let project = fixture();
    let (server, client) = UnixStream::pair().expect("socket pair");
    let server_read = server.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let factory_requests = Arc::clone(&requests);
    let backend = Arc::new(NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        move || {
            let requests = Arc::clone(&factory_requests);
            Ok(Box::new(Fake {
                requests,
                responder: Arc::new(|line| Ok(respond(line))),
            }))
        },
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server, backend, host));
    let mut client = Client::new(client);

    let initialize = client.request(
        "initialize",
        json!({"processId": null, "rootUri": project.root_uri()}),
    );
    let response = client.response(initialize);
    assert_eq!(response["result"]["capabilities"]["hoverProvider"], true);
    client.notify("initialized", json!({}));

    let wxml_uri = project.uri("pages/index/index.wxml");
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": wxml_uri,
                "languageId": "wxml",
                "version": 1,
                "text": "{{ user.name }}"
            }
        }),
    );
    let diagnostics = client.notification("textDocument/publishDiagnostics");
    let values = diagnostics["params"]["diagnostics"]
        .as_array()
        .expect("diagnostics");
    assert_eq!(values.len(), 1, "{diagnostics}");
    assert_eq!(values[0]["severity"], 1);
    assert_eq!(values[0]["range"]["start"]["character"], 8);
    assert_eq!(values[0]["range"]["end"]["character"], 12);

    let hover_id = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": wxml_uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    let hover = client.response(hover_id);
    assert!(
        hover["result"]["contents"]["value"]
            .as_str()
            .expect("contents")
            .contains("string"),
        "{hover}"
    );
    assert_eq!(hover["result"]["range"]["start"]["character"], 8);
    assert_eq!(hover["result"]["range"]["end"]["character"], 12);

    let definition_id = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": wxml_uri},
            "position": {"line": 0, "character": 10}
        }),
    );
    let definition = client.response(definition_id);
    let locations = definition["result"].as_array().expect("locations");
    assert_eq!(locations.len(), 1, "{definition}");
    assert_eq!(locations[0]["uri"], project.uri("pages/index/index.ts"));
    assert_eq!(locations[0]["range"]["start"]["character"], 0);
    assert_eq!(locations[0]["range"]["end"]["character"], 4);

    // A script overlay change must mark the page dirty and re-run diagnostics.
    let script_uri = project.uri("pages/index/index.ts");
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": script_uri,
                "languageId": "typescript",
                "version": 1,
                "text": "Page({ data: { user: { name: 'Ada' } } });"
            }
        }),
    );
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": script_uri, "version": 2},
            "contentChanges": [{"text": "Page({ data: { user: { name: 'Changed' } } });"}]
        }),
    );
    let refreshed = client.notification("textDocument/publishDiagnostics");
    assert_eq!(refreshed["params"]["uri"], wxml_uri);

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": wxml_uri}}),
    );
    let closed = client.notification("textDocument/publishDiagnostics");
    assert_eq!(closed["params"]["diagnostics"], json!([]));
    assert!(closed["params"].get("version").is_none());

    let shutdown = client.request("shutdown", json!(null));
    assert_eq!(client.response(shutdown)["result"], json!(null));
    client.notify("exit", json!(null));
    drop(client);
    let outcome = handle
        .join()
        .expect("server thread")
        .expect("server outcome");
    assert!(outcome.clean_shutdown);
    assert_ne!(
        requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        0
    );
}

/// Replaces the response fragment list with one fragment per definition,
/// keeping the request fragment id so every entry participates in mapping.
fn definition_response(line: &str, definitions: &[Value]) -> String {
    let mut value: Value = serde_json::from_str(&respond(line)).expect("response");
    let template = value["result"]["fragments"][0].clone();
    let fragments = definitions
        .iter()
        .map(|definition| {
            let mut fragment = template.clone();
            fragment["definition"] = definition.clone();
            fragment["diagnostics"] = json!([]);
            fragment
        })
        .collect::<Vec<_>>();
    value["result"]["fragments"] = Value::Array(fragments);
    serde_json::to_string(&value).expect("response json")
}

/// Runs initialize -> didOpen -> definition -> shutdown and returns the LSP
/// `result` value.
fn run_definition(project: &TempProject, responder: Responder) -> Value {
    let (server, client) = UnixStream::pair().expect("socket pair");
    let server_read = server.try_clone().expect("clone");
    let host = Arc::new(ProjectState::new());
    let backend = Arc::new(NodeBackend::with_transport_factory(
        NodeBackendConfig::default(),
        host.clone(),
        move || {
            Ok(Box::new(Fake {
                requests: Arc::new(Mutex::new(Vec::new())),
                responder: Arc::clone(&responder),
            }))
        },
    ));
    let handle = thread::spawn(move || run_with_host(server_read, server, backend, host));
    let mut client = Client::new(client);

    let initialize = client.request(
        "initialize",
        json!({"processId": null, "rootUri": project.root_uri()}),
    );
    let response = client.response(initialize);
    assert_eq!(
        response["result"]["capabilities"]["definitionProvider"],
        true
    );
    client.notify("initialized", json!({}));
    client.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": project.uri("pages/index/index.wxml"),
                "languageId": "wxml",
                "version": 1,
                "text": "{{ user.name }}"
            }
        }),
    );
    let _ = client.notification("textDocument/publishDiagnostics");
    let definition_id = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": project.uri("pages/index/index.wxml")},
            "position": {"line": 0, "character": 10}
        }),
    );
    let result = client.response(definition_id)["result"].clone();
    let shutdown = client.request("shutdown", json!(null));
    assert_eq!(client.response(shutdown)["result"], json!(null));
    client.notify("exit", json!(null));
    drop(client);
    let outcome = handle
        .join()
        .expect("server thread")
        .expect("server outcome");
    assert!(outcome.clean_shutdown);
    result
}

#[test]
fn partial_definition_result_is_returned_as_locations() {
    let project = fixture();
    let definitions = (0..40)
        .map(|index| {
            json!({
                "kind": "script",
                "fileId": "pages/index/index.ts",
                "range": {"start": index, "end": index + 1}
            })
        })
        .collect::<Vec<_>>();
    let result = run_definition(
        &project,
        Arc::new(move |line| Ok(definition_response(line, &definitions))),
    );
    let locations = result.as_array().expect("locations, not null");
    assert_eq!(locations.len(), 32, "{result}");
    assert_eq!(locations[0]["uri"], project.uri("pages/index/index.ts"));
}

#[test]
fn duplicate_definition_result_is_deduplicated_but_not_null() {
    let project = fixture();
    let definition = json!({
        "kind": "script",
        "fileId": "pages/index/index.ts",
        "range": {"start": 0, "end": 4}
    });
    let definitions = vec![definition; 300];
    let result = run_definition(
        &project,
        Arc::new(move |line| Ok(definition_response(line, &definitions))),
    );
    let locations = result.as_array().expect("locations, not null");
    assert_eq!(locations.len(), 1, "{result}");
    assert_eq!(locations[0]["uri"], project.uri("pages/index/index.ts"));
}

#[test]
fn empty_definition_results_respect_supported_and_partial_status() {
    let project = fixture();
    let result = run_definition(
        &project,
        Arc::new(|line| Ok(definition_response(line, &[]))),
    );
    assert_eq!(result, json!([]));

    let dropped = vec![json!({"kind": "dropped"})];
    let result = run_definition(
        &project,
        Arc::new(move |line| Ok(definition_response(line, &dropped))),
    );
    assert_eq!(result, Value::Null);
}

#[test]
fn definition_file_count_is_capped_at_eight() {
    let project = fixture();
    let mut definitions = Vec::new();
    for index in 0..32 {
        let path = format!("generated/tiny-{index}.ts");
        project.write(&path, "export const x = 1;\n");
        definitions.push(json!({
            "kind": "script",
            "fileId": path,
            "range": {"start": 0, "end": 1}
        }));
    }
    let result = run_definition(
        &project,
        Arc::new(move |line| Ok(definition_response(line, &definitions))),
    );
    let locations = result.as_array().expect("locations, not null");
    assert_eq!(
        locations.len(),
        8,
        "at most 8 distinct definition files: {result}"
    );
}

#[test]
fn definition_index_budget_limits_distinct_target_files() {
    let project = fixture();
    let dense = "\n".repeat(2 * 1024 * 1024);
    let mut definitions = Vec::new();
    for index in 0..32 {
        let path = format!("generated/dense-{index}.ts");
        project.write(&path, &dense);
        definitions.push(json!({
            "kind": "script",
            "fileId": path,
            "range": {"start": 0, "end": 1}
        }));
    }
    let response_definitions = definitions;
    let started = std::time::Instant::now();
    let result = run_definition(
        &project,
        Arc::new(move |line| Ok(definition_response(line, &response_definitions))),
    );
    let elapsed = started.elapsed();
    let locations = result.as_array().expect("locations, not null");
    assert_eq!(
        locations.len(),
        3,
        "2 MiB newline-dense files must hit the byte budget after 3 targets: {result}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "budgeted definition mapping took {elapsed:?}"
    );
}
