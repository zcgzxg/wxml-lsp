//! End-to-end kebab/camel spelling equivalence for declared component members.
//!
//! This drives the real `wxml-lsp --stdio` binary in its default `HostOnly`
//! mode: no engine install, no Node, no pinned type assets. The project writes
//! a `components.d.ts` next to `tsconfig.json`, so component properties and
//! events come from the declaration parser in `wxml-project`.
//!
//! Covered here:
//!
//! - declared `cardTitle` written as `card-title` and declared `badge-label`
//!   written as `badgeLabel` resolve to the same declaration for hover,
//!   definition, inlay hints and completion (both insertion spellings);
//! - every `bind`/`catch`/`capture-bind`/`capture-catch`/`mut-bind` colon and
//!   compact spelling, with kebab and camel event names, maps to the declared
//!   event and stays quiet; only the misspelling warns, on the event token;
//! - event hover and completion read the declared event name through the same
//!   equivalence;
//! - a component without a declaration and a component with an incomplete
//!   declaration keep the old behaviour: no undeclared-event warning, no
//!   declared-event completion, and script-derived properties still complete.
//!
//! Pre-existing coverage this file deliberately does not duplicate:
//!
//! - `crates/wxml-project/src/typing.rs` unit-tests the canonicalisation and
//!   both lookup directions;
//! - `crates/wxml-lsp/src/semantics/event_diagnostics.rs`,
//!   `crates/wxml-lsp/src/metadata/docs.rs` and
//!   `crates/wxml-lsp/src/completion/static_items.rs` unit-test the same rules
//!   at their own seams;
//! - `tests/fixtures/components-typings` (see `tests/fixtures/README.md` §11)
//!   is the shared declaration fixture; this file writes its own small project
//!   at runtime because it needs kebab-named declarations and both event
//!   spellings, which that fixture does not express.
//!
//! Property *diagnostics* for undeclared attributes are owned by the native
//! engine adapter (`backend_tsgo/component_bindings.rs`); `HostOnly` publishes
//! no attribute diagnostics at all, so this file proves the shared lookup
//! through the static providers and asserts that no local warning is invented
//! for either spelling. The engine-side path is covered by the component
//! bindings unit tests.
#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use std::io::{BufReader, Write};
use std::path::Path;
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
const DECLARATION: &str = "components.d.ts";

/// Declarations for the two components that carry spelling variants.
///
/// `cardTitle` is camel-declared, `badge-label` is kebab-declared; events use
/// the same two shapes. `broken-card` maps to a declaration with unsupported
/// heritage, which the parser must mark incomplete and ignore.
const DECLARATIONS: &str = concat!(
    "declare global {\n",
    "  interface WxmlComponentMap {\n",
    "    'spell-card': SpellCardProps;\n",
    "    'broken-card': BrokenProps;\n",
    "  }\n",
    "\n",
    "  interface SpellCardProps {\n",
    "    props: {\n",
    "      cardTitle?: string;\n",
    "      'badge-label'?: string;\n",
    "    };\n",
    "    events: {\n",
    "      someEvent: { detail: { index: number } };\n",
    "      'other-event': { detail: { value: string } };\n",
    "    };\n",
    "  }\n",
    "\n",
    "  // Unsupported heritage keeps the whole component incomplete, so none of\n",
    "  // its members may become authoritative facts.\n",
    "  interface BrokenProps extends MissingBase {\n",
    "    props: {\n",
    "      ghostProp: string;\n",
    "    };\n",
    "    events: {\n",
    "      ghostEvent: { detail: { id: number } };\n",
    "    };\n",
    "  }\n",
    "}\n",
    "\n",
    "export {};\n",
);

const PAGE_SCRIPT: &str = concat!(
    "Page({\n",
    "  onSome(event: WechatMiniprogram.CustomEvent<{ index: number }>) { void event; },\n",
    "  onOther(event: WechatMiniprogram.CustomEvent<{ value: string }>) { void event; },\n",
    "});\n",
);

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: i64,
    version: i64,
}

impl Client {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(BIN)
            .arg("--stdio")
            .current_dir(root)
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

    /// Waits for the publish whose version and URI both match the document.
    fn recv_diagnostics(&self, uri: &str, version: i64) -> Vec<Value> {
        loop {
            let message = self
                .messages
                .recv_timeout(TIMEOUT)
                .expect("diagnostics before timeout");
            if message.get("method") != Some(&json!("textDocument/publishDiagnostics")) {
                continue;
            }
            let params = &message["params"];
            if params["version"] == json!(version) && params["uri"] == json!(uri) {
                return params["diagnostics"]
                    .as_array()
                    .expect("diagnostics array")
                    .clone();
            }
        }
    }

    fn initialize(&mut self, root: &Path) {
        let id = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": format!("file://{}", root.display()),
                "capabilities": {
                    "textDocument": {"inlayHint": {"dynamicRegistration": false}},
                },
            }),
        );
        let response = self.recv_response(id);
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(
            response["result"]["capabilities"]["hoverProvider"],
            json!(true),
            "{response}"
        );
        self.notify("initialized", json!({}));
    }

    fn open(&mut self, uri: &str, text: &str) {
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
    }

    fn change(&mut self, uri: &str, text: &str) {
        self.version += 1;
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": self.version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    fn result(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        let response = self.recv_response(id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    /// Replaces the document with `text` and runs a point request at `byte`.
    fn at(&mut self, method: &str, uri: &str, text: &str, byte: usize) -> Value {
        self.change(uri, text);
        self.result(
            method,
            json!({
                "textDocument": {"uri": uri},
                "position": position(text, byte),
            }),
        )
    }

    fn shutdown(&mut self) {
        let id = self.request("shutdown", json!(null));
        let response = self.recv_response(id);
        assert_eq!(response["result"], json!(null), "{response}");
        self.notify("exit", json!(null));
    }

    fn finish(mut self) -> std::process::ExitStatus {
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

/// Writes the small mini-program project used by every test in this file.
fn project() -> TempProject {
    let project = TempProject::new("component-spelling");
    project.write(
        "app.json",
        concat!(
            "{\n",
            "  \"pages\": [\"pages/index/index\"],\n",
            "  \"usingComponents\": {\n",
            "    \"spell-card\": \"/components/spell-card/spell-card\",\n",
            "    \"plain-card\": \"/components/plain-card/plain-card\",\n",
            "    \"broken-card\": \"/components/broken-card/broken-card\"\n",
            "  }\n",
            "}\n",
        ),
    );
    project.write(
        "tsconfig.json",
        concat!(
            "{\n",
            "  \"compilerOptions\": {\n",
            "    \"strict\": true,\n",
            "    \"target\": \"ES2020\",\n",
            "    \"module\": \"CommonJS\"\n",
            "  },\n",
            "  \"include\": [\"**/*.ts\"]\n",
            "}\n",
        ),
    );
    project.write(DECLARATION, DECLARATIONS);
    project.write("pages/index/index.json", "{}\n");
    project.write("pages/index/index.ts", PAGE_SCRIPT);
    project.write(PAGE, "<!-- the test opens its own template text -->\n");
    project.write(
        "components/spell-card/spell-card.json",
        "{\"component\": true}\n",
    );
    project.write("components/spell-card/spell-card.ts", "Component({});\n");
    project.write("components/spell-card/spell-card.wxml", "<view />\n");
    // No declaration entry: only script facts exist.
    project.write(
        "components/plain-card/plain-card.json",
        "{\"component\": true}\n",
    );
    project.write(
        "components/plain-card/plain-card.ts",
        "Component({ properties: { plainProp: { type: String, value: '' } } });\n",
    );
    project.write("components/plain-card/plain-card.wxml", "<view />\n");
    // Declared, but the declaration is structurally unusable.
    project.write(
        "components/broken-card/broken-card.json",
        "{\"component\": true}\n",
    );
    project.write(
        "components/broken-card/broken-card.ts",
        "Component({ properties: { scriptProp: { type: String, value: '' } } });\n",
    );
    project.write("components/broken-card/broken-card.wxml", "<view />\n");
    project
}

/// Returns the LSP position (UTF-16) of one byte offset.
fn position(text: &str, offset: usize) -> Value {
    assert!(text.is_char_boundary(offset), "offset {offset}");
    let prefix = &text[..offset];
    let line = prefix.matches('\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = prefix[line_start..].encode_utf16().count();
    json!({"line": line, "character": character})
}

/// Returns the byte range of one LSP range in `text`.
fn byte_range(text: &str, range: &Value) -> (usize, usize) {
    let offset = |position: &Value| -> usize {
        let line = usize::try_from(position["line"].as_u64().expect("line")).expect("line");
        let character =
            usize::try_from(position["character"].as_u64().expect("character")).expect("character");
        let mut offset = 0;
        for (index, value) in text.split('\n').enumerate() {
            if index == line {
                let rest = value;
                let mut units = 0;
                for (byte, value_char) in rest.char_indices() {
                    if units == character {
                        return offset + byte;
                    }
                    units += value_char.len_utf16();
                }
                assert_eq!(units, rest.encode_utf16().count(), "line width");
                return offset + rest.len();
            }
            offset += value.len() + 1;
        }
        panic!("line {line} is outside the document");
    };
    (offset(&range["start"]), offset(&range["end"]))
}

fn hover_text(hover: &Value) -> &str {
    hover["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("hover markdown: {hover}"))
}

fn completion_items(result: &Value) -> &[Value] {
    result["items"]
        .as_array()
        .unwrap_or_else(|| panic!("completion items: {result}"))
}

fn labels(result: &Value) -> Vec<&str> {
    completion_items(result)
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect()
}

/// Finds the one item whose insertion text is exactly `insert`.
fn item_with_insert<'a>(result: &'a Value, insert: &str) -> &'a Value {
    completion_items(result)
        .iter()
        .find(|item| item["insertText"] == json!(insert))
        .unwrap_or_else(|| {
            panic!(
                "completion item inserting {insert:?} is missing; labels: {:?}",
                labels(result)
            )
        })
}

/// Returns diagnostics whose byte range overlaps `needle` in `text`.
fn diagnostics_in<'a>(diagnostics: &'a [Value], text: &str, needle: &str) -> Vec<&'a Value> {
    let start = text
        .find(needle)
        .unwrap_or_else(|| panic!("marker {needle:?} is not in the template"));
    let end = start + needle.len();
    diagnostics
        .iter()
        .filter(|diagnostic| {
            let (diagnostic_start, diagnostic_end) = byte_range(text, &diagnostic["range"]);
            diagnostic_start < end && start < diagnostic_end
        })
        .collect()
}

/// Asserts a property resolves to `declared` in the declaration file, and
/// returns the hover markdown plus the definition so both spellings can be
/// compared byte-for-byte.
fn property_resolution(
    client: &mut Client,
    uri: &str,
    text: &str,
    written: &str,
    declared: &str,
) -> (String, Value) {
    let start = text
        .find(written)
        .unwrap_or_else(|| panic!("attribute {written:?} is not in {text:?}"));
    let offset = start + 1;
    let hover = client.at("textDocument/hover", uri, text, offset);
    let markdown = hover_text(&hover).to_owned();
    assert!(
        markdown.contains(declared),
        "{written}: hover must name the declaration {declared}: {markdown}"
    );
    assert!(
        markdown.contains("components.d.ts"),
        "{written}: hover must point at the declaration file: {markdown}"
    );
    let definition = client.at("textDocument/definition", uri, text, offset);
    let locations = definition
        .as_array()
        .unwrap_or_else(|| panic!("{written}: definition locations: {definition}"));
    assert_eq!(locations.len(), 1, "{written}: {definition}");
    let location_uri = locations[0]["uri"].as_str().expect("definition uri");
    assert!(
        location_uri.ends_with(DECLARATION),
        "{written}: definition must land in {DECLARATION}: {location_uri}"
    );
    assert_eq!(
        locations[0]["range"]["start"],
        json!({"line": 0, "character": 0}),
        "{written}: declared members anchor at the declaration file start: {definition}"
    );
    (markdown, definition)
}

#[test]
fn property_spellings_resolve_to_the_same_declaration() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(project.root());
    let uri = project.uri(PAGE);

    // Both directions in one document: camel declaration used kebab, kebab
    // declaration used camel. `HostOnly` publishes no attribute diagnostics,
    // so this checks that no *local* warning is invented either; the engine
    // attribute check is covered by the component bindings unit tests.
    let template = concat!(
        "<spell-card card-title=\"hello\" badgeLabel=\"hi\" />\n",
        "<spell-card cardTitle=\"hello\" badge-label=\"hi\" />\n",
    );
    client.open(&uri, template);
    let diagnostics = client.recv_diagnostics(&uri, 1);
    for written in [
        "card-title=\"hello\"",
        "badgeLabel=\"hi\"",
        "cardTitle=\"hello\"",
        "badge-label=\"hi\"",
    ] {
        let hits = diagnostics_in(&diagnostics, template, written);
        assert!(
            hits.is_empty(),
            "{written}: unexpected diagnostics: {hits:?}"
        );
    }
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("not declared"))),
        "{diagnostics:?}"
    );

    // camel-declared property, kebab-vs-camel use resolves identically.
    let (kebab_hover, kebab_definition) = property_resolution(
        &mut client,
        &uri,
        "<spell-card card-title=\"x\" />",
        "card-title",
        "cardTitle",
    );
    let (camel_hover, camel_definition) = property_resolution(
        &mut client,
        &uri,
        "<spell-card cardTitle=\"x\" />",
        "cardTitle",
        "cardTitle",
    );
    assert_eq!(kebab_hover, camel_hover);
    assert_eq!(kebab_definition, camel_definition);
    assert!(kebab_hover.contains("string"), "{kebab_hover}");

    // kebab-declared property, camel-vs-kebab use resolves identically.
    let (camel_use_hover, camel_use_definition) = property_resolution(
        &mut client,
        &uri,
        "<spell-card badgeLabel=\"x\" />",
        "badgeLabel",
        "badge-label",
    );
    let (kebab_use_hover, kebab_use_definition) = property_resolution(
        &mut client,
        &uri,
        "<spell-card badge-label=\"x\" />",
        "badge-label",
        "badge-label",
    );
    assert_eq!(camel_use_hover, kebab_use_hover);
    assert_eq!(camel_use_definition, kebab_use_definition);
    assert!(camel_use_hover.contains("string"), "{camel_use_hover}");

    // Inlay hints read the same declaration through both spellings.
    let hints_text = "<spell-card card-title=\"x\" badgeLabel=\"y\" />";
    client.change(&uri, hints_text);
    let hints = client.result(
        "textDocument/inlayHint",
        json!({
            "textDocument": {"uri": uri},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": position(hints_text, hints_text.len()),
            },
        }),
    );
    for (attribute, hint_offset) in [
        ("card-title=\"x\"", "card-title=\"x\"".len()),
        ("badgeLabel=\"y\"", "badgeLabel=\"y\"".len()),
    ] {
        let end = hints_text.find(attribute).expect("attribute") + hint_offset;
        assert!(
            hints.as_array().expect("hints").iter().any(|hint| {
                hint["label"] == json!(": string") && hint["position"] == position(hints_text, end)
            }),
            "{attribute}: expected a `: string` hint at its end: {hints}"
        );
    }

    // Completion follows the spelling the user already typed, for both
    // declaration shapes.
    let cases = [
        ("<spell-card card-", "card-title"),
        ("<spell-card cardT", "cardTitle"),
        ("<spell-card badge-", "badge-label"),
        ("<spell-card badgeL", "badgeLabel"),
    ];
    for (text, insert) in cases {
        let result = client.at("textDocument/completion", &uri, text, text.len());
        let item = item_with_insert(&result, insert);
        assert_eq!(item["textEdit"]["newText"], json!(insert), "{text}: {item}");
        assert_eq!(
            item["textEdit"]["range"]["start"],
            position(
                text,
                text.len() - text.rsplit_once(' ').expect("prefix").1.len()
            ),
            "{text}: the whole typed prefix is replaced: {item}"
        );
    }

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn every_event_binding_spelling_resolves_and_only_a_typo_warns() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(project.root());
    let uri = project.uri(PAGE);

    // `someEvent` is camel-declared, `other-event` is kebab-declared. Every
    // binding family is used in its colon and compact form, with both name
    // spellings; the last three lines are the negative and quiet controls.
    let template = concat!(
        "<spell-card bind:some-event=\"onSome\" />\n",
        "<spell-card bind:someEvent=\"onSome\" />\n",
        "<spell-card bindsome-event=\"onSome\" />\n",
        "<spell-card bindsomeEvent=\"onSome\" />\n",
        "<spell-card catch:some-event=\"onSome\" />\n",
        "<spell-card catchsomeEvent=\"onSome\" />\n",
        "<spell-card capture-bind:some-event=\"onSome\" />\n",
        "<spell-card capture-bindsome-event=\"onSome\" />\n",
        "<spell-card capture-catch:someEvent=\"onSome\" />\n",
        "<spell-card mut-bind:some-event=\"onSome\" />\n",
        "<spell-card mut-bindsomeEvent=\"onSome\" />\n",
        "<spell-card bind:other-event=\"onOther\" />\n",
        "<spell-card bind:otherEvent=\"onOther\" />\n",
        "<spell-card bindother-event=\"onOther\" />\n",
        "<spell-card bindotherEvent=\"onOther\" />\n",
        "<spell-card bindsomeEvent=\"{{ handler }}\" />\n",
        "<spell-card bind:someevnt=\"onSome\" />\n",
    );
    client.open(&uri, template);
    let diagnostics = client.recv_diagnostics(&uri, 1);

    let undeclared: Vec<&Value> = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("is not declared by component"))
        })
        .collect();
    assert_eq!(
        undeclared.len(),
        1,
        "only `bind:someevnt` may be undeclared; every other spelling must map to the declaration: {diagnostics:?}"
    );
    let warning = undeclared[0];
    assert_eq!(warning["severity"], json!(2), "{warning}");
    assert_eq!(
        warning["message"],
        json!("Event \"someevnt\" is not declared by component \"spell-card\""),
        "{warning}"
    );
    let (start, end) = byte_range(template, &warning["range"]);
    assert_eq!(
        &template[start..end],
        "someevnt",
        "the warning must cover the event token, not the binding prefix: {warning}"
    );
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("does not match event"))),
        "compatible handlers must not produce detail errors: {diagnostics:?}"
    );

    // Event hover reads the declared detail contract for both directions and
    // both binding spellings.
    let hover_cases = [
        (
            "<spell-card bind:some-event=\"onSome\" />",
            "some-event",
            "{ index: number }",
        ),
        (
            "<spell-card bindsomeEvent=\"onSome\" />",
            "someEvent",
            "{ index: number }",
        ),
        (
            "<spell-card bind:otherEvent=\"onOther\" />",
            "otherEvent",
            "{ value: string }",
        ),
        (
            "<spell-card bindother-event=\"onOther\" />",
            "other-event",
            "{ value: string }",
        ),
    ];
    for (text, written, detail) in hover_cases {
        let offset = text.find(written).expect("event token") + 1;
        let hover = client.at("textDocument/hover", &uri, text, offset);
        let markdown = hover_text(&hover);
        assert!(
            markdown.contains(&format!("CustomEvent<{detail}>")),
            "{written}: hover must use the declared detail: {markdown}"
        );
        assert!(
            markdown.contains("声明的事件处理函数签名"),
            "{written}: {markdown}"
        );
    }

    // Event-name completion keeps the prefix and inserts the spelling the
    // user is typing.
    let completion_cases = [
        ("<spell-card bind:some-", "some-event", "some-"),
        ("<spell-card bindsomeE", "someEvent", "someE"),
        ("<spell-card bindsome-", "some-event", "some-"),
        ("<spell-card bind:otherE", "otherEvent", "otherE"),
        ("<spell-card bind:other-", "other-event", "other-"),
    ];
    for (text, insert, fragment) in completion_cases {
        let result = client.at("textDocument/completion", &uri, text, text.len());
        let item = item_with_insert(&result, insert);
        assert_eq!(item["textEdit"]["newText"], json!(insert), "{text}: {item}");
        assert_eq!(
            item["textEdit"]["range"]["start"],
            position(text, text.len() - fragment.len()),
            "{text}: only the event-name fragment may be replaced: {item}"
        );
        assert!(
            item["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("CustomEvent")),
            "{text}: declared completion must carry the event contract: {item}"
        );
    }

    client.shutdown();
    assert!(client.finish().success());
}

#[test]
fn components_without_usable_declarations_keep_the_old_behavior() {
    let project = project();
    let mut client = Client::start(project.root());
    client.initialize(project.root());
    let uri = project.uri(PAGE);

    // `plain-card` has no declaration; `broken-card`'s declaration is
    // incomplete. Neither may become authoritative for event names.
    let template = concat!(
        "<plain-card plainProp=\"x\" bind:missingThing=\"onSome\" />\n",
        "<broken-card scriptProp=\"x\" bind:ghostEvent=\"onSome\" />\n",
    );
    client.open(&uri, template);
    let diagnostics = client.recv_diagnostics(&uri, 1);
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("is not declared by component"))),
        "a missing or incomplete declaration must not produce undeclared-event warnings: {diagnostics:?}"
    );

    // Script-derived properties still complete for both components.
    for (text, insert) in [
        ("<plain-card plainP", "plainProp"),
        ("<broken-card scriptP", "scriptProp"),
    ] {
        let result = client.at("textDocument/completion", &uri, text, text.len());
        let item = item_with_insert(&result, insert);
        assert_eq!(item["textEdit"]["newText"], json!(insert), "{text}: {item}");
    }

    // No declared-event items: the declared list is only for a complete
    // declaration. `broken-card` does declare `ghostEvent`, so its absence
    // proves the incomplete declaration is ignored.
    for text in ["<plain-card bind:", "<broken-card bind:ghost"] {
        let result = client.at("textDocument/completion", &uri, text, text.len());
        assert!(
            !labels(&result).contains(&"ghostEvent"),
            "{text}: incomplete declarations must not contribute declared events: {result}"
        );
        assert!(
            completion_items(&result)
                .iter()
                .all(|item| item["kind"] != json!(23)),
            "{text}: no declared-event completion is available: {result}"
        );
    }

    client.shutdown();
    assert!(client.finish().success());
}
