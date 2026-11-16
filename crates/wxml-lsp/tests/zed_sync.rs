#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

//! Zed synchronization tests (wave5 WT-D).
//!
//! The server runs with a real `ProjectState` and a `NodeBackend` whose
//! transport is fake, so unsaved script/JSON buffers, `didSave` and
//! `workspace/didChangeWatchedFiles` are exercised end to end without a Node
//! process.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver};
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

fn fixture() -> TempProject {
    let project = TempProject::new("wxml-zed-sync");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write(WXML, "{{ user.name }}");
    project.write(SCRIPT, "Page({ data: { user: { name: 'Ada' } } });");
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    project.write("package.json", r#"{"name":"zed-fixture"}"#);
    project.write("node_modules/pkg/index.js", "module.exports = 1;");
    project
}

fn multi_app_fixture() -> TempProject {
    let project = TempProject::new("wxml-zed-multi-app");
    for app in ["a", "b"] {
        project.write(
            &format!("apps/{app}/src/app.json"),
            r#"{"pages":["pages/index/index"]}"#,
        );
        project.write(&format!("apps/{app}/src/{WXML}"), "{{ user.name }}");
        project.write(
            &format!("apps/{app}/src/{SCRIPT}"),
            &format!(
                "Page({{ data: {{ user: {{ name: 'ZED_{}_DISK' }} }} }});",
                app.to_uppercase()
            ),
        );
        project.write(
            &format!("apps/{app}/tsconfig.json"),
            &json!({"compilerOptions": {"strict": app == "a"}}).to_string(),
        );
    }
    project
}

fn multi_app_options() -> Value {
    json!({"apps": [
        {"root": "apps/a/src", "projectRoot": "apps/a"},
        {"root": "apps/b/src", "projectRoot": "apps/b"},
    ]})
}

/// Finds the `ZED_*` marker in the owning script, even with a shared project root.
fn overlay_marker(params: &Value) -> String {
    let mut marker = "disk".to_owned();
    let Some(files) = params["files"].as_array() else {
        return marker;
    };
    for file in files {
        if file["fileId"] != params["scriptFileId"] {
            continue;
        }
        let Some(text) = file["text"].as_str() else {
            continue;
        };
        for word in
            text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            if word.starts_with("ZED_") {
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
    let revision = params["analysisStamp"]["viewRevision"]
        .as_u64()
        .expect("viewRevision");
    let marker = overlay_marker(params);
    let fragments = params["fragments"]
        .as_array()
        .expect("fragments")
        .iter()
        .map(|fragment| {
            let start = fragment["wxmlRange"]["start"].as_u64().unwrap_or(3) + 5;
            json!({
                "fragmentId": fragment["fragmentId"],
                "hover": {
                    "typeText": format!("value:{marker}"),
                    "typeOnly": format!("value:{marker}"),
                    "quality": "known",
                    "documentation": format!("value:{marker}"),
                    "documentationSource": "quickinfo"
                },
                "diagnostics": [{
                    "code": 1,
                    "range": {"start": start, "end": start + 4},
                    "message": format!("check:{marker}"),
                    "attribution": "source"
                }],
                "definition": {
                    "kind": "script",
                    "fileId": params["scriptFileId"],
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
            "insertions": [{"viewRevision": revision, "backendFileVersion": 1}],
            "mounts": [],
            "fragments": fragments,
            "moduleKind": {"before": "script", "after": "script"},
            "suppressed": {"count": 0, "details": []}
        }
    }))
    .expect("response json")
}

type FirstRequestGate = Arc<Mutex<Option<Receiver<()>>>>;

struct Fake {
    requests: Arc<Mutex<Vec<String>>>,
    sender: mpsc::Sender<String>,
    first_request_gate: Option<FirstRequestGate>,
}

impl NodeTransport for Fake {
    fn request(&self, line: &str, timeout: Duration) -> Result<String, NodeTransportError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_owned());
        let _ = self.sender.send(line.to_owned());
        if let Some(receiver) = self
            .first_request_gate
            .as_ref()
            .and_then(|gate| gate.lock().unwrap_or_else(PoisonError::into_inner).take())
        {
            receiver
                .recv_timeout(timeout)
                .map_err(|_| NodeTransportError::Timeout)?;
        }
        Ok(respond(line))
    }

    fn shutdown(&self) {}
}

struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: i64,
    received: Vec<Value>,
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
            received: Vec::new(),
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
        let message = self.read_message().expect("server closed the stream");
        self.received.push(message.clone());
        message
    }

    /// Reads one framed message, or `None` on EOF/timeout.
    fn recv_optional(&mut self) -> Option<Value> {
        let message = self.read_message()?;
        self.received.push(message.clone());
        Some(message)
    }

    fn read_message(&mut self) -> Option<Value> {
        let mut length = None;
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).ok()?;
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
        self.reader.read_exact(&mut body).ok()?;
        serde_json::from_slice(&body).ok()
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

type ServerJoin = thread::JoinHandle<Result<wxml_lsp::ServerOutcome, wxml_lsp::ServerError>>;

struct Session {
    client: Client,
    requests: Arc<Mutex<Vec<String>>>,
    events: Receiver<String>,
    handle: ServerJoin,
    registration: Value,
    wxml_uri: String,
}

impl Session {
    fn start(project: &TempProject) -> Self {
        Self::start_configured(project, None)
    }

    fn start_with_options(project: &TempProject, options: Value) -> Self {
        Self::start_configured(project, Some(options))
    }

    fn start_configured(project: &TempProject, options: Option<Value>) -> Self {
        let mut session = Self::uninitialized(project);
        let response = session.initialize(project, options);
        assert_eq!(
            response["result"]["capabilities"]["hoverProvider"], true,
            "{response}"
        );
        session.initialized();
        session
    }

    fn uninitialized(project: &TempProject) -> Self {
        Self::uninitialized_with_gate(project, None)
    }

    fn uninitialized_with_gate(
        project: &TempProject,
        first_request_gate: Option<FirstRequestGate>,
    ) -> Self {
        let (server, client) = UnixStream::pair().expect("socket pair");
        let server_read = server.try_clone().expect("clone");
        let host = Arc::new(ProjectState::new());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let factory_requests = Arc::clone(&requests);
        let (sender, events) = mpsc::channel();
        let backend = Arc::new(NodeBackend::with_transport_factory(
            NodeBackendConfig::default(),
            host.clone(),
            move || {
                Ok(Box::new(Fake {
                    requests: Arc::clone(&factory_requests),
                    sender: sender.clone(),
                    first_request_gate: first_request_gate.clone(),
                }))
            },
        ));
        let handle = thread::spawn(move || run_with_host(server_read, server, backend, host));

        Self {
            client: Client::new(client),
            requests,
            events,
            handle,
            registration: Value::Null,
            wxml_uri: project.uri(WXML),
        }
    }

    fn initialize(&mut self, project: &TempProject, options: Option<Value>) -> Value {
        let mut params = json!({
            "processId": null,
            "rootUri": project.root_uri(),
            "capabilities": {
                "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": true}}
            }
        });
        if let Some(options) = options {
            params["initializationOptions"] = options;
        }
        let id = self.client.request("initialize", params);
        self.client.response(id)
    }

    fn initialized(&mut self) {
        self.client.notify("initialized", json!({}));
        let registration = self.client.recv();
        assert_eq!(registration["method"], json!("client/registerCapability"));
        self.client.send(&json!({
            "jsonrpc": "2.0",
            "id": registration["id"].clone(),
            "result": null,
        }));
        self.registration = registration;
    }

    fn open(&mut self, uri: &str, language: &str, text: &str, version: u64) {
        self.client.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language,
                    "version": version,
                    "text": text,
                }
            }),
        );
    }

    fn change(&mut self, uri: &str, text: &str, version: u64) {
        self.client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    fn save(&mut self, uri: &str, text: Option<&str>) {
        let mut params = json!({"textDocument": {"uri": uri}});
        if let Some(text) = text {
            params["text"] = json!(text);
        }
        self.client.notify("textDocument/didSave", params);
    }

    fn close(&mut self, uri: &str) {
        self.client.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
    }

    fn watched(&mut self, changes: &[Value]) {
        self.client.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": changes}),
        );
    }

    fn hover(&mut self) -> Value {
        let uri = self.wxml_uri.clone();
        self.hover_at(&uri)
    }

    fn hover_at(&mut self, uri: &str) -> Value {
        self.location_request("textDocument/hover", uri)
    }

    fn assert_hover(&mut self, uri: &str, marker: &str) {
        let hover = self.hover_at(uri);
        assert_eq!(hover["result"]["contents"]["kind"], "markdown", "{hover}");
        let contents = hover["result"]["contents"]["value"]
            .as_str()
            .expect("hover contents");
        let expected = format!("value:{marker}");
        assert_eq!(contents.lines().nth(1), Some(expected.as_str()), "{hover}");
    }

    fn location_request(&mut self, method: &str, uri: &str) -> Value {
        let id = self.client.request(
            method,
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": 0, "character": 10},
            }),
        );
        self.client.response(id)
    }

    fn diagnostics_after(&mut self, since: usize, uri: &str, marker: &str) {
        let expected = json!(format!("check:{marker}"));
        let mut index = since;
        loop {
            for message in &self.client.received[index..] {
                if message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["diagnostics"]
                        .as_array()
                        .is_some_and(|diagnostics| {
                            diagnostics
                                .iter()
                                .any(|diagnostic| diagnostic["message"] == expected)
                        })
                {
                    assert_eq!(message["params"]["uri"], uri, "{message}");
                    return;
                }
            }
            index = self.client.received.len();
            assert!(
                self.client.recv_optional().is_some(),
                "no diagnostics for {uri} with {marker}; received: {:?}",
                &self.client.received[since..],
            );
        }
    }

    fn next_request(&self, timeout: Duration) -> String {
        match self.events.recv_timeout(timeout) {
            Ok(line) => line,
            Err(error) => panic!("no backend request arrived: {error}"),
        }
    }

    fn drained_requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn drain_requests(&self) {
        while self.events.try_recv().is_ok() {}
    }

    fn finish(mut self) -> Vec<Value> {
        let shutdown = self.client.request("shutdown", json!(null));
        assert_eq!(self.client.response(shutdown)["result"], json!(null));
        self.client.notify("exit", json!(null));
        // Close the write half so the server reader thread observes EOF and
        // the join cannot hang on a detached blocking read.
        let _ = self.client.writer.shutdown(std::net::Shutdown::Write);
        let outcome = self.handle.join().expect("server thread").expect("outcome");
        assert!(outcome.clean_shutdown);
        while self.client.recv_optional().is_some() {}
        self.client.received
    }
}

fn script_uri(project: &TempProject) -> String {
    project.uri(SCRIPT)
}

#[test]
fn initialized_registers_watched_files_for_a_project() {
    let project = fixture();
    let session = Session::start(&project);
    let registration = &session.registration;
    assert_eq!(registration["id"], json!("wxml-lsp-watch"));

    let registrations = registration["params"]["registrations"]
        .as_array()
        .expect("registrations");
    assert_eq!(registrations.len(), 1);
    assert_eq!(
        registrations[0]["method"],
        json!("workspace/didChangeWatchedFiles")
    );
    let watchers = registrations[0]["registerOptions"]["watchers"]
        .as_array()
        .expect("watchers");
    let globs: Vec<&str> = watchers
        .iter()
        .filter_map(|watcher| watcher["globPattern"].as_str())
        .collect();
    assert_eq!(
        globs,
        vec![
            "**/*.wxml",
            "**/*.json",
            "**/*.ts",
            "**/*.tsx",
            "**/*.js",
            "**/*.jsx",
            "**/*.mts",
            "**/*.cts",
            "**/*.mjs",
            "**/*.cjs",
            "**/*.wxss",
        ]
    );
    assert!(watchers.iter().all(|watcher| watcher["kind"] == json!(7)));

    let received = session.finish();
    assert!(
        received
            .iter()
            .all(|message| message["method"] != json!("textDocument/publishDiagnostics")),
        "no documents were opened"
    );
}

#[test]
fn unsaved_script_overlay_reaches_wxml_analysis() {
    let project = fixture();
    let mut session = Session::start(&project);
    let wxml = session.wxml_uri.clone();
    let script = script_uri(&project);

    session.open(&wxml, "wxml", "{{ user.name }}", 1);
    session.next_request(Duration::from_secs(10));
    session.drain_requests();

    session.open(
        &script,
        "typescript",
        "Page({ data: { user: { name: 'ZED_OPEN' } } });",
        1,
    );
    session.change(
        &script,
        "Page({ data: { user: { name: 'ZED_CHANGED' } } });",
        2,
    );

    let hover = session.hover();
    let contents = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover contents");
    assert!(contents.contains("ZED_CHANGED"), "{hover}");

    let requests = session.drained_requests();
    assert!(
        requests.iter().any(|line| line.contains("ZED_CHANGED")),
        "the unsaved script overlay never reached the backend request"
    );

    let received = session.finish();
    let diagnostics = received
        .iter()
        .rfind(|message| message["method"] == json!("textDocument/publishDiagnostics"))
        .expect("diagnostics for the WXML view");
    assert_eq!(diagnostics["params"]["uri"], json!(wxml));
    assert!(
        diagnostics["params"]["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("ZED_CHANGED")),
        "{diagnostics}"
    );
}

#[test]
fn unreferenced_documents_do_not_publish_or_change_analysis() {
    let project = fixture();
    let mut session = Session::start(&project);
    let wxml = session.wxml_uri.clone();
    let package_json = project.uri("package.json");
    let unreferenced = project.uri("node_modules/pkg/index.js");

    session.open(&wxml, "wxml", "{{ user.name }}", 1);
    session.next_request(Duration::from_secs(10));

    session.open(&package_json, "json", r#"{"name":"ZED_PKG"}"#, 1);
    session.change(&package_json, r#"{"name":"ZED_PKG_CHANGED"}"#, 2);
    session.open(
        &unreferenced,
        "javascript",
        "module.exports = 'ZED_UNUSED';",
        1,
    );
    session.change(&unreferenced, "module.exports = 'ZED_UNUSED_CHANGED';", 2);

    let hover = session.hover();
    let contents = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover contents");
    assert!(!contents.contains("ZED_"), "{hover}");

    let requests = session.drained_requests();
    assert!(
        !requests
            .iter()
            .any(|line| line.contains("ZED_PKG") || line.contains("ZED_UNUSED")),
        "an ignored document overlay reached the backend"
    );

    let received = session.finish();
    for message in &received {
        if message["method"] != json!("textDocument/publishDiagnostics") {
            continue;
        }
        let uri = message["params"]["uri"].as_str().expect("diagnostics uri");
        assert_ne!(uri, package_json, "{message}");
        assert_ne!(uri, unreferenced, "{message}");
    }
}

#[test]
fn did_save_reschedules_wxml_diagnostics() {
    let project = fixture();
    let mut session = Session::start(&project);
    let wxml = session.wxml_uri.clone();
    let script = script_uri(&project);

    session.open(&wxml, "wxml", "{{ user.name }}", 1);
    session.next_request(Duration::from_secs(10));
    session.open(
        &script,
        "typescript",
        "Page({ data: { user: { name: 'ZED_SAVE' } } });",
        1,
    );
    session.next_request(Duration::from_secs(10));
    session.drain_requests();

    // Zed sends `didSave` without `text` by default.
    session.save(&script, None);
    let line = session.next_request(Duration::from_secs(10));
    assert!(line.contains("ZED_SAVE"), "{line}");

    let diagnostics = session
        .client
        .notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["params"]["uri"], json!(wxml));
    assert!(
        diagnostics["params"]["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("ZED_SAVE")),
        "{diagnostics}"
    );

    session.finish();
}

#[test]
fn watched_file_changes_trigger_reanalysis() {
    let project = fixture();
    let mut session = Session::start(&project);
    let wxml = session.wxml_uri.clone();

    session.open(&wxml, "wxml", "{{ user.name }}", 1);
    session.next_request(Duration::from_secs(10));

    // A changed owning script on disk is read again.
    project.write(
        SCRIPT,
        "Page({ data: { user: { name: 'ZED_DISK_CHANGE' } } });",
    );
    session.watched(&[json!({"uri": project.uri(SCRIPT), "type": 2})]);
    let line = session.next_request(Duration::from_secs(10));
    assert!(line.contains("ZED_DISK_CHANGE"), "{line}");
    let diagnostics = session
        .client
        .notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["params"]["uri"], json!(wxml));

    // A JSON config change adds a page and must be rediscovered.
    project.write("pages/other/other.wxml", "<view>{{ other }}</view>");
    project.write("pages/other/other.ts", "Page({ data: { other: 1 } });");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index","pages/other/other"]}"#,
    );
    session.watched(&[json!({"uri": project.uri("app.json"), "type": 2})]);
    let line = session.next_request(Duration::from_secs(10));
    assert!(line.contains("analysisStamp"), "{line}");
    let _ = session
        .client
        .notification("textDocument/publishDiagnostics");

    // Creation and deletion events must not panic and must keep the session
    // usable.
    session.watched(&[
        json!({"uri": project.uri(SCRIPT), "type": 3}),
        json!({"uri": project.uri("scripts/generated.ts"), "type": 1}),
        json!({"uri": "file:///outside/root.ts", "type": 2}),
    ]);
    let hover = session.hover();
    assert!(hover.get("result").is_some(), "{hover}");

    session.finish();
}

#[test]
fn watched_changes_keep_an_open_buffer_overlay() {
    let project = fixture();
    let mut session = Session::start(&project);
    let wxml = session.wxml_uri.clone();
    let script = script_uri(&project);

    session.open(&wxml, "wxml", "{{ user.name }}", 1);
    session.next_request(Duration::from_secs(10));
    session.open(
        &script,
        "typescript",
        "Page({ data: { user: { name: 'ZED_OPEN' } } });",
        1,
    );
    session.drain_requests();

    // A delayed save event for the same buffer must not replace the overlay
    // with the on-disk text.
    project.write(SCRIPT, "Page({ data: { user: { name: 'ZED_DISK' } } });");
    session.watched(&[json!({"uri": project.uri(SCRIPT), "type": 2})]);

    let hover = session.hover();
    let contents = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover contents");
    assert!(contents.contains("ZED_OPEN"), "{hover}");
    assert!(!contents.contains("ZED_DISK"), "{hover}");
    session.finish();
}

#[test]
fn explicit_multi_app_options_isolate_hover_definition_and_diagnostics() {
    let project = multi_app_fixture();
    let mut session = Session::start_with_options(&project, multi_app_options());
    let a_wxml = project.uri(&format!("apps/a/src/{WXML}"));
    let b_wxml = project.uri(&format!("apps/b/src/{WXML}"));
    let a_script = project.uri(&format!("apps/a/src/{SCRIPT}"));
    let b_script = project.uri(&format!("apps/b/src/{SCRIPT}"));

    for (uri, marker) in [(&a_wxml, "ZED_A_DISK"), (&b_wxml, "ZED_B_DISK")] {
        let since = session.client.received.len();
        session.open(uri, "wxml", "{{ user.name }}", 1);
        session.diagnostics_after(since, uri, marker);
    }
    for (script, wxml, marker) in [(&a_script, &a_wxml, "ZED_A"), (&b_script, &b_wxml, "ZED_B")] {
        let since = session.client.received.len();
        session.open(
            script,
            "typescript",
            &format!("Page({{ data: {{ user: {{ name: '{marker}' }} }} }});"),
            1,
        );
        session.diagnostics_after(since, wxml, marker);
    }

    for (wxml, script, marker) in [
        (&b_wxml, &b_script, "ZED_B"),
        (&a_wxml, &a_script, "ZED_A"),
        (&b_wxml, &b_script, "ZED_B"),
    ] {
        session.assert_hover(wxml, marker);
        let definition = session.location_request("textDocument/definition", wxml);
        assert_eq!(definition["result"][0]["uri"], *script, "{definition}");
        assert_eq!(
            definition["result"][0]["range"],
            json!({
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 4},
            })
        );
    }

    let requests = session.drained_requests();
    for app in ["a", "b"] {
        let root = project.root().join(format!("apps/{app}"));
        assert!(
            requests.iter().any(|line| {
                let request: Value = serde_json::from_str(line).expect("backend request");
                request["params"]["project"]["projectRoot"] == root.to_string_lossy().as_ref()
                    && request["params"]["project"]["compilerOptions"]["strict"] == (app == "a")
                    && request["params"]["scriptFileId"] == format!("src/{SCRIPT}")
            }),
            "no analysis used the explicit projectRoot for app {app}"
        );
    }

    // Both documents have the same project-relative path. Updating A while B
    // is active must schedule A, without needing a hover to trigger analysis.
    let since = session.client.received.len();
    session.change(
        &a_script,
        "Page({ data: { user: { name: 'ZED_A_CHANGED' } } });",
        2,
    );
    session.diagnostics_after(since, &a_wxml, "ZED_A_CHANGED");
    let received = session.finish();
    assert!(
        received[since..].iter().all(|message| {
            message["method"] != "textDocument/publishDiagnostics"
                || message["params"]["uri"] == a_wxml
        }),
        "editing A scheduled another app: {:?}",
        &received[since..],
    );

    for message in received {
        if message["method"] != "textDocument/publishDiagnostics" {
            continue;
        }
        let Some(diagnostics) = message["params"]["diagnostics"].as_array() else {
            panic!("invalid diagnostics: {message}");
        };
        for diagnostic in diagnostics {
            let marker = diagnostic["message"].as_str().expect("diagnostic message");
            let expected_uri = if marker.starts_with("check:ZED_A") {
                &a_wxml
            } else if marker.starts_with("check:ZED_B") {
                &b_wxml
            } else {
                panic!("unexpected diagnostic: {message}");
            };
            assert_eq!(message["params"]["uri"], *expected_uri, "{message}");
        }
    }
}

#[test]
fn explicit_multi_app_overlays_survive_save_watch_and_close() {
    let project = multi_app_fixture();
    let mut session = Session::start_with_options(&project, multi_app_options());
    let a_wxml = project.uri(&format!("apps/a/src/{WXML}"));
    let b_wxml = project.uri(&format!("apps/b/src/{WXML}"));
    let a_script = project.uri(&format!("apps/a/src/{SCRIPT}"));
    let b_script = project.uri(&format!("apps/b/src/{SCRIPT}"));
    for (uri, marker) in [(&a_wxml, "ZED_A_DISK"), (&b_wxml, "ZED_B_DISK")] {
        let since = session.client.received.len();
        session.open(uri, "wxml", "{{ user.name }}", 1);
        session.diagnostics_after(since, uri, marker);
    }
    for (script, wxml, marker) in [(&a_script, &a_wxml, "ZED_A"), (&b_script, &b_wxml, "ZED_B")] {
        let since = session.client.received.len();
        session.open(
            script,
            "typescript",
            &format!("Page({{ data: {{ user: {{ name: '{marker}' }} }} }});"),
            1,
        );
        session.diagnostics_after(since, wxml, marker);
    }

    // B is active. A's save must select A and keep its unsaved text.
    let since = session.client.received.len();
    session.save(&a_script, None);
    session.diagnostics_after(since, &a_wxml, "ZED_A");
    project.write(
        &format!("apps/a/src/{SCRIPT}"),
        "Page({ data: { user: { name: 'ZED_A_SAVED' } } });",
    );
    session.watched(&[json!({"uri": a_script, "type": 2})]);
    session.assert_hover(&a_wxml, "ZED_A");
    session.assert_hover(&b_wxml, "ZED_B");

    // Closing A while B is active restores A's disk text; later switches must
    // not replay the closed overlay.
    let since = session.client.received.len();
    session.close(&a_script);
    session.diagnostics_after(since, &a_wxml, "ZED_A_SAVED");
    session.assert_hover(&b_wxml, "ZED_B");
    session.assert_hover(&a_wxml, "ZED_A_SAVED");

    // A closed script accepts watched disk changes even while B is active.
    session.assert_hover(&b_wxml, "ZED_B");
    project.write(
        &format!("apps/a/src/{SCRIPT}"),
        "Page({ data: { user: { name: 'ZED_A_WATCHED' } } });",
    );
    let since = session.client.received.len();
    session.watched(&[json!({"uri": a_script, "type": 2})]);
    session.diagnostics_after(since, &a_wxml, "ZED_A_WATCHED");
    session.assert_hover(&b_wxml, "ZED_B");
    session.finish();
}

#[test]
fn explicit_multi_app_watched_batch_refreshes_both_closed_scripts() {
    let project = multi_app_fixture();
    let mut session = Session::start_with_options(&project, multi_app_options());
    let a_wxml = project.uri(&format!("apps/a/src/{WXML}"));
    let b_wxml = project.uri(&format!("apps/b/src/{WXML}"));
    let a_script = project.uri(&format!("apps/a/src/{SCRIPT}"));
    let b_script = project.uri(&format!("apps/b/src/{SCRIPT}"));
    for (wxml, script, marker) in [
        (&a_wxml, &a_script, "ZED_A_DISK"),
        (&b_wxml, &b_script, "ZED_B_DISK"),
    ] {
        let since = session.client.received.len();
        session.open(wxml, "wxml", "{{ user.name }}", 1);
        session.diagnostics_after(since, wxml, marker);
        let since = session.client.received.len();
        session.open(
            script,
            "typescript",
            "Page({ data: { user: { name: 'ZED_OPEN' } } });",
            1,
        );
        session.diagnostics_after(since, wxml, "ZED_OPEN");
        let since = session.client.received.len();
        session.close(script);
        session.diagnostics_after(since, wxml, marker);
    }

    for (app, marker) in [("a", "ZED_A_BATCH"), ("b", "ZED_B_BATCH")] {
        project.write(
            &format!("apps/{app}/src/{SCRIPT}"),
            &format!("Page({{ data: {{ user: {{ name: '{marker}' }} }} }});"),
        );
    }
    let since = session.client.received.len();
    session.watched(&[
        json!({"uri": a_script, "type": 2}),
        json!({"uri": b_script, "type": 2}),
    ]);
    // Either app may finish first. Both waits inspect the same notification
    // window, and neither hover nor another notification can trigger analysis.
    session.diagnostics_after(since, &a_wxml, "ZED_A_BATCH");
    session.diagnostics_after(since, &b_wxml, "ZED_B_BATCH");
    session.assert_hover(&a_wxml, "ZED_A_BATCH");
    session.assert_hover(&b_wxml, "ZED_B_BATCH");
    session.finish();
}

#[test]
fn rapid_multi_app_opens_keep_every_queued_diagnostic() {
    let project = multi_app_fixture();
    project.write(
        "apps/a/src/app.json",
        r#"{"pages":["pages/index/index","pages/second/second"]}"#,
    );
    project.write("apps/a/src/pages/second/second.wxml", "{{ user.name }}");
    project.write(
        "apps/a/src/pages/second/second.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    let (release, blocked) = mpsc::channel();
    let gate = Arc::new(Mutex::new(Some(blocked)));
    let mut session = Session::uninitialized_with_gate(&project, Some(gate));
    let response = session.initialize(&project, Some(multi_app_options()));
    assert_eq!(
        response["result"]["capabilities"]["hoverProvider"], true,
        "{response}"
    );
    session.initialized();

    let a_first = project.uri(&format!("apps/a/src/{WXML}"));
    let b_page = project.uri(&format!("apps/b/src/{WXML}"));
    let a_second = project.uri("apps/a/src/pages/second/second.wxml");
    let since = session.client.received.len();
    session.open(&a_first, "wxml", "{{ user.name }}", 1);
    let request: Value = serde_json::from_str(&session.next_request(Duration::from_secs(10)))
        .expect("first backend request");
    assert_eq!(
        request["params"]["project"]["projectRoot"],
        project.root().join("apps/a").to_string_lossy().as_ref()
    );
    assert_eq!(request["params"]["scriptFileId"], format!("src/{SCRIPT}"));
    session.open(&b_page, "wxml", "{{ user.name }}", 1);
    session.open(&a_second, "wxml", "{{ user.name }}", 1);

    // An unknown request is answered on the protocol thread. Its response
    // proves both later opens were handled while A/page1's backend was gated.
    // It neither runs analysis nor requests a diagnostics refresh.
    let barrier = session.client.request("wxml-test/barrier", json!({}));
    assert_eq!(session.client.response(barrier)["error"]["code"], -32601);
    release.send(()).expect("release first analysis");

    let expected = std::collections::BTreeSet::from([a_first, b_page, a_second]);
    let mut published = std::collections::BTreeSet::new();
    let mut index = since;
    while published != expected {
        for message in &session.client.received[index..] {
            if message["method"] != "textDocument/publishDiagnostics" {
                continue;
            }
            let uri = message["params"]["uri"].as_str().expect("diagnostics URI");
            assert!(expected.contains(uri), "unexpected diagnostics: {message}");
            assert_eq!(message["params"]["version"], 1, "{message}");
            assert!(
                !message["params"]["diagnostics"]
                    .as_array()
                    .expect("diagnostics")
                    .is_empty(),
                "{message}"
            );
            published.insert(uri.to_owned());
        }
        if published == expected {
            break;
        }
        index = session.client.received.len();
        assert!(
            session.client.recv_optional().is_some(),
            "queued diagnostics were lost across app switches; expected {expected:?}, published {published:?}; received: {:?}",
            &session.client.received[since..]
        );
    }
    session.finish();
}

#[test]
fn shared_project_root_watched_batch_preserves_foreground_config_routing() {
    // Separate sessions compare the single-event baseline with the same
    // tsconfig deletion batched before a change in B. Both start foreground A.
    for batched in [false, true] {
        let project = TempProject::new("wxml-zed-shared-root");
        project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
        for app in ["a", "b"] {
            project.write(
                &format!("{app}/app.json"),
                r#"{"pages":["pages/index/index"]}"#,
            );
            project.write(&format!("{app}/{WXML}"), "{{ user.name }}");
            project.write(
                &format!("{app}/{SCRIPT}"),
                &format!(
                    "Page({{ data: {{ user: {{ name: 'ZED_{}_DISK' }} }} }});",
                    app.to_uppercase()
                ),
            );
        }
        let options = json!({"apps": [
            {"root": "a", "projectRoot": "."},
            {"root": "b", "projectRoot": "."},
        ]});
        let mut session = Session::start_with_options(&project, options);
        let a_wxml = project.uri(&format!("a/{WXML}"));
        let b_wxml = project.uri(&format!("b/{WXML}"));
        // Open B before A and consume both initial results. The config and
        // scripts themselves stay closed; no hover changes the foreground.
        for (uri, marker) in [(&b_wxml, "ZED_B_DISK"), (&a_wxml, "ZED_A_DISK")] {
            let since = session.client.received.len();
            session.open(uri, "wxml", "{{ user.name }}", 1);
            session.diagnostics_after(since, uri, marker);
        }
        let before = session.drained_requests();
        for app in ["a", "b"] {
            assert!(
                before.iter().any(|line| {
                    let request: Value = serde_json::from_str(line).expect("backend request");
                    request["params"]["scriptFileId"] == format!("{app}/{SCRIPT}")
                        && request["params"]["project"]["compilerOptions"]["strict"] == true
                }),
                "app {app} did not initially load the shared tsconfig"
            );
        }

        std::fs::remove_file(project.root().join("tsconfig.json")).expect("delete shared tsconfig");
        let mut changes = vec![json!({"uri": project.uri("tsconfig.json"), "type": 3})];
        if batched {
            project.write(
                &format!("b/{SCRIPT}"),
                "Page({ data: { user: { name: 'ZED_B_BATCH' } } });",
            );
            changes.push(json!({"uri": project.uri(&format!("b/{SCRIPT}")), "type": 2}));
        }
        let since = session.client.received.len();
        session.watched(&changes);
        // A must be refreshed by the config deletion in either form. In the
        // batch B must also refresh; either completion order is accepted.
        session.diagnostics_after(since, &a_wxml, "ZED_A_DISK");
        if batched {
            session.diagnostics_after(since, &b_wxml, "ZED_B_BATCH");
        }
        let after = session.drained_requests();
        for app in if batched { &["a", "b"][..] } else { &["a"][..] } {
            assert!(
                after[before.len()..].iter().any(|line| {
                    let request: Value = serde_json::from_str(line).expect("backend request");
                    request["params"]["scriptFileId"] == format!("{app}/{SCRIPT}")
                        && request["params"]["project"]["compilerOptions"]
                            .get("strict")
                            .is_none()
                }),
                "app {app} did not reanalyze after deleting the shared tsconfig (batched={batched})"
            );
        }
        session.finish();
    }
}

#[test]
fn invalid_initialization_options_allow_a_later_valid_initialize() {
    let project = fixture();
    let mut session = Session::uninitialized(&project);
    for options in [
        json!({"apps": "src"}),
        json!({"apps": [{"root": "../outside"}]}),
    ] {
        let response = session.initialize(&project, Some(options));
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(response.get("result").is_none(), "{response}");
    }

    // Do not wait for registration after a rejected initialize: only the
    // successful retry enters the initialized lifecycle.
    let response = session.initialize(&project, Some(json!({"apps": [{"root": "."}]})));
    assert_eq!(
        response["result"]["capabilities"]["hoverProvider"], true,
        "{response}"
    );
    session.initialized();
    let wxml = session.wxml_uri.clone();
    let since = session.client.received.len();
    session.open(&wxml, "wxml", "{{ user.name }}", 1);
    session.diagnostics_after(since, &wxml, "disk");
    session.assert_hover(&wxml, "disk");
    let received = session.finish();
    assert_eq!(
        received
            .iter()
            .filter(|message| message["method"] == "client/registerCapability")
            .count(),
        1
    );
}
