#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

//! G1 real end-to-end acceptance over the actual `wxml-lsp --stdio` binary.
//!
//! Every test is ignored by default. Run them with:
//!
//! ```bash
//! (cd backends/typescript && npm ci && npm run build)
//! WXML_TEST_NODE=1 cargo test -p wxml-lsp --test stdio_type_loop -- --ignored --nocapture
//! ```
//!
//! `#[ignore]` means the gate must be requested explicitly. Running the file
//! with `--ignored` without `WXML_TEST_NODE=1`, or with the environment set but
//! the built backend missing, is a hard failure rather than a silent skip.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;

use common::TempProject;

const BIN: &str = env!("CARGO_BIN_EXE_wxml-lsp");
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

struct BackendPaths {
    server: PathBuf,
    backend_dir: PathBuf,
}

/// Resolves the real backend, failing closed when the gate was not requested.
///
/// The tests are `#[ignore]`, so `--ignored` is an explicit request to run the
/// real Node gate. A missing environment variable or backend must fail instead
/// of reporting a green skip.
fn backend_paths() -> BackendPaths {
    assert!(
        std::env::var("WXML_TEST_NODE").as_deref() == Ok("1"),
        "set WXML_TEST_NODE=1 to run the real Node G1 gate"
    );
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf();
    let server = std::env::var_os("WXML_TYPE_BACKEND").map_or_else(
        || repo.join("backends/typescript/dist/server.js"),
        PathBuf::from,
    );
    let backend_dir = std::env::var_os("WXML_BACKEND_DIR")
        .map_or_else(|| repo.join("backends/typescript"), PathBuf::from);
    assert!(
        server.is_file(),
        "WXML_TEST_NODE=1 but {} is missing; run `npm ci && npm run build` in backends/typescript",
        server.display()
    );
    assert!(
        backend_dir.is_dir(),
        "WXML_TEST_NODE=1 but backend dir {} is missing",
        backend_dir.display()
    );
    BackendPaths {
        server,
        backend_dir,
    }
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    pending: VecDeque<Value>,
    stderr: Arc<Mutex<String>>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    next_id: i64,
}

impl Session {
    fn start(project: &TempProject, paths: &BackendPaths) -> Self {
        let mut child = Command::new(BIN)
            .arg("--stdio")
            .current_dir(project.root())
            .env("WXML_TYPE_BACKEND", &paths.server)
            .env("WXML_BACKEND_DIR", &paths.backend_dir)
            .env("WXML_ANALYZE_TIMEOUT_MS", "30000")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn wxml-lsp --stdio");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (sender, messages) = mpsc::channel();
        let stdout_thread = thread::Builder::new()
            .name("g1-lsp-reader".to_owned())
            .spawn(move || read_messages(stdout, &sender))
            .expect("stdout reader");
        let stderr_sink = Arc::new(Mutex::new(String::new()));
        let stderr_for_thread = Arc::clone(&stderr_sink);
        let stderr_thread = thread::Builder::new()
            .name("g1-lsp-stderr".to_owned())
            .spawn(move || read_stderr(stderr, &stderr_for_thread))
            .expect("stderr reader");

        let mut session = Self {
            child,
            stdin: Some(stdin),
            messages,
            pending: VecDeque::new(),
            stderr: stderr_sink,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            next_id: 1,
        };
        let initialize = session.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": project.root_uri(),
                "capabilities": {},
            }),
        );
        let response = session.response(initialize);
        assert_eq!(
            response["result"]["capabilities"]["positionEncoding"],
            json!("utf-16"),
            "{response}"
        );
        assert_eq!(
            response["result"]["capabilities"]["hoverProvider"],
            json!(true),
            "{response}"
        );
        session.notify("initialized", json!({}));
        session
    }

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("json");
        let stdin = self.stdin.as_mut().expect("stdin open");
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("header");
        stdin.write_all(&body).expect("body");
        stdin.flush().expect("flush");
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
        if let Some(value) = self.pending.pop_front() {
            return value;
        }
        match self.messages.recv_timeout(REQUEST_TIMEOUT) {
            Ok(value) => value,
            Err(error) => panic!(
                "timed out waiting for an LSP message ({error}); server stderr:\n{}",
                self.stderr_text()
            ),
        }
    }

    fn next_matching(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.pending.iter().position(&mut predicate) {
            return self.pending.remove(index).expect("pending index");
        }
        loop {
            let message = self.recv();
            if predicate(&message) {
                return message;
            }
            self.pending.push_back(message);
        }
    }

    fn response(&mut self, id: i64) -> Value {
        self.next_matching(|message| {
            message.get("id").and_then(Value::as_i64) == Some(id) && message.get("method").is_none()
        })
    }

    fn open_wxml(&mut self, uri: &str, text: &str, version: u64) -> Value {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "wxml",
                    "version": version,
                    "text": text,
                }
            }),
        );
        self.wait_diagnostics(uri, Some(version))
    }

    fn change_wxml(&mut self, uri: &str, text: &str, version: u64) -> Value {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
        self.wait_diagnostics(uri, Some(version))
    }

    fn wait_diagnostics(&mut self, uri: &str, min_version: Option<u64>) -> Value {
        loop {
            let message = self.next_matching(|message| {
                message.get("method").and_then(Value::as_str)
                    == Some("textDocument/publishDiagnostics")
                    && message["params"]["uri"].as_str() == Some(uri)
            });
            let version = message["params"]["version"].as_u64();
            let current =
                min_version.is_none_or(|min| version.is_some_and(|version| version >= min));
            if current {
                return message["params"]["diagnostics"].clone();
            }
        }
    }

    fn hover(&mut self, uri: &str, position: (u32, u32)) -> Value {
        let id = self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": position.0, "character": position.1},
            }),
        );
        self.response(id)
    }

    fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn shutdown(mut self) {
        let id = self.request("shutdown", Value::Null);
        let response = self.response(id);
        assert_eq!(response["result"], Value::Null, "{response}");
        self.notify("exit", Value::Null);
        self.stdin.take();

        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(
                        status.success(),
                        "server exited with {status}; stderr:\n{}",
                        self.stderr_text()
                    );
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("server did not exit; stderr:\n{}", self.stderr_text());
                }
                Err(error) => panic!("wait failed: {error}"),
            }
        }
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}

fn read_messages(stdout: ChildStdout, sender: &mpsc::Sender<Value>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let Some(length) = read_header(&mut reader) else {
            return;
        };
        let mut body = vec![0_u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice(&body) else {
            return;
        };
        if sender.send(value).is_err() {
            return;
        }
    }
}

fn read_header(reader: &mut impl BufRead) -> Option<usize> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return length;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
}

fn read_stderr(stderr: ChildStderr, sink: &Arc<Mutex<String>>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let mut sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
                if sink.len() < 256 * 1024 {
                    sink.push_str(&line);
                }
            }
        }
    }
}

/// Returns the LSP UTF-16 position of the first `needle`.
fn lsp_position(text: &str, needle: &str) -> (u32, u32) {
    let byte = text.find(needle).expect("needle in text");
    let prefix = &text[..byte];
    let line =
        u32::try_from(prefix.bytes().filter(|byte| *byte == b'\n').count()).expect("line fits");
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = u32::try_from(
        text[line_start..byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>(),
    )
    .expect("column fits");
    (line, character)
}

fn assert_diagnostic_at(
    diagnostics: &Value,
    text: &str,
    needle: &str,
    message_contains: &str,
    context: &str,
) {
    let values = diagnostics.as_array().expect("diagnostics array");
    let (line, character) = lsp_position(text, needle);
    let end = character + u32::try_from(needle.encode_utf16().count()).expect("needle fits");
    let found = values.iter().any(|diagnostic| {
        diagnostic["range"]["start"]["line"] == json!(line)
            && diagnostic["range"]["start"]["character"] == json!(character)
            && diagnostic["range"]["end"]["line"] == json!(line)
            && diagnostic["range"]["end"]["character"] == json!(end)
            && diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains(message_contains))
    });
    assert!(
        found,
        "{context}: no diagnostic at {line}:{character}-{line}:{end} containing {message_contains:?} in {diagnostics}"
    );
}

/// Fails when a clean page also carries a degradation status warning.
fn assert_no_status_diagnostic(diagnostics: &Value, context: &str) {
    let status = diagnostics
        .as_array()
        .expect("diagnostics array")
        .iter()
        .find(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is"))
        });
    assert!(
        status.is_none(),
        "{context}: unexpected degradation status: {status:?} in {diagnostics}"
    );
}

fn hover_value(response: &Value) -> &str {
    response["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("hover without markdown contents: {response}"))
}

fn write_page_fixture(project: &TempProject, wxml: &str) {
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{}}"#,
    );
    project.write("pages/index/index.wxml", wxml);
    project.write(
        "pages/index/index.ts",
        r"
Page({
  data: {
    user: {
      /** 用户显示名 */
      name: 'Ada'
    },
    /** 页面标题 */
    title: 'Hello'
  }
});
",
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
}

#[test]
#[ignore = "requires WXML_TEST_NODE=1 and a built backends/typescript/dist/server.js"]
fn ac001_page_data_and_jsdoc_hover() {
    let paths = backend_paths();
    let project = TempProject::new("wxml-g1");
    let wxml = "{{ user.name }} {{ title }}";
    write_page_fixture(&project, wxml);
    let mut session = Session::start(&project, &paths);
    let uri = project.uri("pages/index/index.wxml");

    let diagnostics = session.open_wxml(&uri, wxml, 1);
    eprintln!("AC-001 diagnostics: {diagnostics}");
    assert_eq!(
        diagnostics,
        json!([]),
        "a valid page data binding must publish no diagnostics: {diagnostics}"
    );

    let name_position = lsp_position(wxml, "name");
    let name = session.hover(&uri, name_position);
    let name_value = hover_value(&name);
    eprintln!("AC-001 hover(user.name): {name}");
    assert!(
        name_value.contains("string"),
        "user.name must be string: {name}"
    );
    assert!(
        name_value.contains("用户显示名"),
        "user.name hover must include the JSDoc text: {name}"
    );
    assert_eq!(
        name["result"]["range"]["start"],
        json!({"line": name_position.0, "character": name_position.1}),
        "hover range must point at `name`: {name}"
    );
    assert_eq!(
        name["result"]["range"]["end"],
        json!({"line": name_position.0, "character": name_position.1 + 4}),
        "hover range must point at `name`: {name}"
    );

    // The whole mustache expression, not only the target token, answers hover.
    let user_position = lsp_position(wxml, "user");
    let user = session.hover(&uri, user_position);
    assert!(
        hover_value(&user).contains("string"),
        "hover over the expression root must use the target type: {user}"
    );
    assert_eq!(
        user["result"]["range"]["start"],
        json!({"line": name_position.0, "character": name_position.1}),
        "the reported range must stay on the target token: {user}"
    );

    let name_len = u32::try_from("user.name".len()).expect("expression fits in u32");
    let expression_end = (user_position.0, user_position.1 + name_len);
    let end = session.hover(&uri, expression_end);
    assert!(
        hover_value(&end).contains("string"),
        "the expression end is a valid hover point: {end}"
    );

    let outside = session.hover(&uri, (0, 0));
    assert!(
        outside["result"].is_null(),
        "plain markup outside a mustache must not produce an unknown tooltip: {outside}"
    );

    let title_position = lsp_position(wxml, "title");
    let title = session.hover(&uri, title_position);
    let title_value = hover_value(&title);
    eprintln!("AC-001 hover(title): {title}");
    assert!(
        title_value.contains("string"),
        "title must be string: {title}"
    );
    assert_eq!(
        title["result"]["range"]["start"],
        json!({"line": title_position.0, "character": title_position.1}),
        "hover range must point at `title`: {title}"
    );
    assert_eq!(
        title["result"]["range"]["end"],
        json!({"line": title_position.0, "character": title_position.1 + 5}),
        "hover range must point at `title`: {title}"
    );

    session.shutdown();
}

#[test]
#[ignore = "requires WXML_TEST_NODE=1 and a built backends/typescript/dist/server.js"]
fn ac002_field_typo_diagnostic_and_fix_clear() {
    let paths = backend_paths();
    let project = TempProject::new("wxml-g1");
    let broken = "{{ user.nmae }}";
    write_page_fixture(&project, broken);
    let mut session = Session::start(&project, &paths);
    let uri = project.uri("pages/index/index.wxml");

    let diagnostics = session.open_wxml(&uri, broken, 1);
    eprintln!("AC-002 broken diagnostics: {diagnostics}");
    assert_diagnostic_at(
        &diagnostics,
        broken,
        "nmae",
        "does not exist",
        "AC-002 typo",
    );
    assert_no_status_diagnostic(&diagnostics, "AC-002 typo");
    let values = diagnostics.as_array().expect("diagnostics");
    let errors = values
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "the typo must produce exactly one error diagnostic: {diagnostics}"
    );
    let (line, character) = lsp_position(broken, "nmae");
    assert_eq!(
        errors[0]["range"],
        json!({
            "start": {"line": line, "character": character},
            "end": {"line": line, "character": character + 4},
        }),
        "the only error must sit exactly on `nmae`: {diagnostics}"
    );

    let fixed = "{{ user.name }}";
    let cleared = session.change_wxml(&uri, fixed, 2);
    eprintln!("AC-002 fixed diagnostics: {cleared}");
    assert_eq!(
        cleared,
        json!([]),
        "AC-002 diagnostics must clear after didChange: {cleared}"
    );

    session.shutdown();
}

#[test]
#[ignore = "requires WXML_TEST_NODE=1 and a built backends/typescript/dist/server.js"]
fn ac003_unicode_and_crlf_positions() {
    let paths = backend_paths();
    let project = TempProject::new("wxml-g1");
    let wxml = "标题😀 {{ user.nmae }}\r\n第二行 {{ user.nmae }}\r\n";
    write_page_fixture(&project, wxml);
    let mut session = Session::start(&project, &paths);
    let uri = project.uri("pages/index/index.wxml");

    let diagnostics = session.open_wxml(&uri, wxml, 1);
    eprintln!("AC-003 diagnostics: {diagnostics}");
    assert_no_status_diagnostic(&diagnostics, "AC-003 unicode");
    let values = diagnostics.as_array().expect("diagnostics");
    let errors = values
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == json!(1))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        2,
        "both `nmae` occurrences must be diagnosed: {diagnostics}"
    );
    let first = errors
        .iter()
        .find(|diagnostic| diagnostic["range"]["start"]["line"] == json!(0))
        .unwrap_or_else(|| panic!("no line-0 diagnostic: {diagnostics}"));
    assert_eq!(first["range"]["start"]["character"], json!(13), "{first}");
    assert_eq!(first["range"]["end"]["line"], json!(0), "{first}");
    assert_eq!(first["range"]["end"]["character"], json!(17), "{first}");
    assert!(
        first["message"]
            .as_str()
            .is_some_and(|message| message.contains("nmae")),
        "{first}"
    );

    let second = errors
        .iter()
        .find(|diagnostic| diagnostic["range"]["start"]["line"] == json!(1))
        .unwrap_or_else(|| panic!("no line-1 diagnostic: {diagnostics}"));
    assert_eq!(second["range"]["start"]["character"], json!(12), "{second}");
    assert_eq!(second["range"]["end"]["line"], json!(1), "{second}");
    assert_eq!(second["range"]["end"]["character"], json!(16), "{second}");

    session.shutdown();
}

#[test]
#[ignore = "requires WXML_TEST_NODE=1 and a built backends/typescript/dist/server.js"]
fn ac013_component_properties_hover() {
    let paths = backend_paths();
    let project = TempProject::new("wxml-g1");
    project.write(
        "app.json",
        r#"{"pages":["pages/index/index"],"usingComponents":{"card":"./components/card/card"}}"#,
    );
    project.write(
        "pages/index/index.ts",
        "Page({ data: { user: { name: 'Ada' } } });",
    );
    project.write(
        "pages/index/index.wxml",
        r#"<card title="{{ user.name }}" />"#,
    );
    project.write("components/card/card.wxml", "<view>{{ title }}</view>");
    project.write(
        "components/card/card.ts",
        r"
Component({
  properties: {
    title: { type: String, value: '' }
  }
});
",
    );
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);

    let mut session = Session::start(&project, &paths);
    let uri = project.uri("components/card/card.wxml");
    let text = "<view>{{ title }}</view>";
    let diagnostics = session.open_wxml(&uri, text, 1);
    eprintln!("AC-013 component diagnostics: {diagnostics}");
    let values = diagnostics.as_array().expect("diagnostics");
    assert!(
        values
            .iter()
            .all(|diagnostic| diagnostic["severity"] != json!(1)),
        "a declared component property must not produce a type error: {diagnostics}"
    );
    // P5 currently marks a component with only `properties` as Partial because
    // there is no `data` root. Keep the reason visible rather than pretending
    // the check was fully Supported; hover must still answer the property.
    let status = values
        .iter()
        .find(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is partial"))
        })
        .unwrap_or_else(|| {
            panic!("properties-only component must report its Partial status: {diagnostics}")
        });
    assert!(
        status["message"]
            .as_str()
            .is_some_and(|message| message.contains("registration has no `data` member")),
        "{status}"
    );

    let hover = session.hover(&uri, lsp_position(text, "title"));
    let value = hover_value(&hover);
    eprintln!("AC-013 hover(title): {hover}");
    assert!(
        value.contains("string"),
        "component property title must be string: {hover}"
    );

    session.shutdown();
}

#[test]
#[ignore = "requires WXML_TEST_NODE=1 and a built backends/typescript/dist/server.js"]
fn ac011_unknown_owner_degrades_honestly() {
    let paths = backend_paths();
    let project = TempProject::new("wxml-g1");
    write_page_fixture(&project, "{{ user.name }}");
    let orphan = "{{ unknown.value }}";
    project.write("orphan.wxml", orphan);

    let mut session = Session::start(&project, &paths);
    let uri = project.uri("orphan.wxml");
    let diagnostics = session.open_wxml(&uri, orphan, 1);
    eprintln!("AC-011 orphan diagnostics: {diagnostics}");
    let warning = diagnostics
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| {
            diagnostic["severity"] == json!(2)
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("type checking is"))
        })
        .unwrap_or_else(|| {
            panic!("unknown owner must publish a degradation warning: {diagnostics}")
        });
    assert!(
        warning["message"].as_str().is_some_and(
            |message| message.contains("unsupported") || message.contains("unavailable")
        ),
        "{warning}"
    );

    let hover = session.hover(&uri, lsp_position(orphan, "value"));
    eprintln!("AC-011 hover: {hover}");
    let hover_empty = hover["result"].is_null()
        || hover["result"]["contents"].is_null()
        || hover["result"]["contents"]["value"]
            .as_str()
            .is_some_and(str::is_empty);
    assert!(
        hover_empty,
        "an unknown owner must not fabricate a hover type: {hover}"
    );
    assert!(
        diagnostics
            .as_array()
            .expect("diagnostics")
            .iter()
            .all(|diagnostic| diagnostic["severity"] != json!(1)),
        "an unknown owner must not fabricate a type diagnostic: {diagnostics}"
    );

    session.shutdown();
}
