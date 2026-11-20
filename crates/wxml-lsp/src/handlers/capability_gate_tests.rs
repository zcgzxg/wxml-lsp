//! Capability-gate dispatch tests for point and completion handlers.
//!
//! A backend that reports `Partial` answers some inputs and reports the limits
//! on each result, so the handler must dispatch to it. Only `Supported` and
//! `Partial` are available; `Unsupported`, `Disabled`, `Unavailable` and
//! `Unknown` must not start backend work. These tests drive the public server
//! entry point with a fake backend and a recording dispatcher.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wxml_types::{
    Backend, BackendCapabilities, CapabilityStatus, CompletionKind, CompletionResult,
    DefinitionResult, HoverResult, MarkupKind, MockBackend, TypeQuality,
};

use crate::jsonrpc::{Incoming, Outgoing, ResponseOutcome};
use crate::scheduler::{Job, JobKind, Outcome, SubmitError, WorkDone};
use crate::server::{Dispatch, Server};

use super::backend_available;

const URI: &str = "file:///pages/index/index.wxml";
const POINT_DOCUMENT: &str = "abc";
const WXS_TEXT: &str = concat!(
    "<wxs module=\"utils\">/** Formats a value. */\n",
    "function format(value) { return \"x\"; }\n",
    "module.exports = { format };</wxs>\n",
    "<view>{{ utils.format(pageTitle) }}</view>",
);
const TEMPLATE_TEXT: &str = concat!(
    "<template name=\"card\"><view>{{ title }}</view></template>\n",
    "<template is=\"card\" />",
);
const WXS_SOURCE_URI: &str = "file:///pages/index/utils.wxs";
const WXS_SOURCE_TEXT: &str = concat!(
    "/** Formats a name for display.\n",
    " * @param {string} name input name\n",
    " * @returns {string} formatted name\n",
    " */\n",
    "function someFunc(name) { return name; }\n",
    "module.exports = { someFunc };\n",
);
const SCRIPT_URI: &str = "file:///pages/index/utils.ts";
const SCRIPT_TEXT: &str = concat!(
    "/** Formats a name. */\n",
    "function someFunc(name: string): string { return name; }\n",
);

#[derive(Default)]
struct RecordingDispatch {
    jobs: Mutex<Vec<Job>>,
}

impl RecordingDispatch {
    fn take(&self) -> Vec<Job> {
        std::mem::take(&mut *self.jobs.lock().expect("dispatch jobs"))
    }
}

impl Dispatch for RecordingDispatch {
    fn submit(&self, job: Job) -> Result<(), SubmitError> {
        self.jobs.lock().expect("dispatch jobs").push(job);
        Ok(())
    }
}

fn request(id: Value, method: &str, params: Value) -> Incoming {
    Incoming::Request {
        id,
        method: method.to_owned(),
        params: Some(params),
    }
}

fn server_with(capabilities: BackendCapabilities) -> (Server, Arc<RecordingDispatch>) {
    let backend: Arc<dyn Backend> = Arc::new(MockBackend::new());
    let dispatch = Arc::new(RecordingDispatch::default());
    let mut server = Server::new(backend, capabilities, dispatch.clone());
    server.diagnostics_debounce = std::time::Duration::ZERO;
    let initialized = server.handle_incoming(request(json!(1), "initialize", json!({})));
    assert_eq!(initialized.len(), 1, "initialize response");
    (server, dispatch)
}

fn open(server: &mut Server, text: &str) {
    open_at(server, URI, "wxml", text);
}

fn open_at(server: &mut Server, uri: &str, language_id: &str, text: &str) {
    let outgoing = server.handle_incoming(Incoming::Notification {
        method: "textDocument/didOpen".to_owned(),
        params: Some(json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            },
        })),
    });
    assert!(outgoing.is_empty(), "didOpen returned {outgoing:?}");
}

fn point_params(line: u32, character: u32) -> Value {
    point_params_at(URI, line, character)
}

fn point_params_at(uri: &str, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": line, "character": character},
    })
}

fn only(outgoing: Vec<Outgoing>) -> Outgoing {
    assert_eq!(outgoing.len(), 1, "expected exactly one message");
    outgoing.into_iter().next().expect("one message")
}

fn success_value(outgoing: &Outgoing) -> &Value {
    match outgoing {
        Outgoing::Response {
            outcome: ResponseOutcome::Success(value),
            ..
        } => value,
        other => panic!("expected a success response, got {other:?}"),
    }
}

fn empty_completion_list() -> Value {
    json!({
        "isIncomplete": false,
        "items": [],
    })
}

fn position_at(text: &str, offset: usize) -> (u32, u32) {
    let prefix = text.get(..offset).expect("character boundary");
    let line = u32::try_from(prefix.matches('\n').count()).expect("line");
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = u32::try_from(prefix[line_start..].encode_utf16().count()).expect("character");
    (line, character)
}

fn wxs_member_position(text: &str) -> (u32, u32) {
    let start = text.find("utils.format").expect("WXS member use") + "utils.".len();
    position_at(text, start + 1)
}

fn template_call_position(text: &str) -> (u32, u32) {
    let start = text.find("is=\"card\"").expect("template call") + "is=\"".len();
    position_at(text, start + 1)
}

fn wxs_source_declaration_position() -> (u32, u32) {
    let start = WXS_SOURCE_TEXT
        .find("function someFunc")
        .expect("WXS declaration")
        + "function ".len();
    position_at(WXS_SOURCE_TEXT, start + 1)
}

fn wxs_source_keyword_position() -> (u32, u32) {
    let start = WXS_SOURCE_TEXT
        .find("return name")
        .expect("WXS return keyword");
    position_at(WXS_SOURCE_TEXT, start + 1)
}

#[test]
fn only_supported_and_partial_backends_are_available() {
    assert!(backend_available(&CapabilityStatus::Supported));
    assert!(backend_available(&CapabilityStatus::Partial {
        reason: "subset".to_owned(),
    }));
    for status in [
        CapabilityStatus::Unsupported {
            reason: "missing".to_owned(),
        },
        CapabilityStatus::Disabled {
            reason: "policy".to_owned(),
        },
        CapabilityStatus::Unavailable {
            reason: "stopped".to_owned(),
        },
        CapabilityStatus::Unknown { reason: None },
    ] {
        assert!(!backend_available(&status), "{status:?}");
    }
}

#[test]
fn hover_dispatches_only_available_capabilities() {
    let cases = [
        ("supported", CapabilityStatus::Supported, true),
        (
            "partial",
            CapabilityStatus::Partial {
                reason: "native expression hover only".to_owned(),
            },
            true,
        ),
        (
            "unsupported",
            CapabilityStatus::Unsupported {
                reason: "no hover".to_owned(),
            },
            false,
        ),
        (
            "unavailable",
            CapabilityStatus::Unavailable {
                reason: "not started".to_owned(),
            },
            false,
        ),
        (
            "disabled",
            CapabilityStatus::Disabled {
                reason: "policy".to_owned(),
            },
            false,
        ),
        ("unknown", CapabilityStatus::Unknown { reason: None }, false),
    ];
    for (name, status, dispatched) in cases {
        let capabilities = BackendCapabilities {
            hover: status,
            ..MockBackend::new().capabilities()
        };
        let (mut server, dispatch) = server_with(capabilities);
        open(&mut server, POINT_DOCUMENT);
        dispatch.take();
        let outgoing =
            server.handle_incoming(request(json!(2), "textDocument/hover", point_params(0, 1)));
        let jobs = dispatch.take();
        if dispatched {
            assert!(outgoing.is_empty(), "{name}: {outgoing:?}");
            assert_eq!(jobs.len(), 1, "{name}");
            assert_eq!(jobs[0].kind, JobKind::Hover, "{name}");
        } else {
            assert!(jobs.is_empty(), "{name}");
            assert_eq!(success_value(&only(outgoing)), &json!(null), "{name}");
        }
    }
}

#[test]
fn definition_dispatches_only_available_capabilities() {
    let cases = [
        ("supported", CapabilityStatus::Supported, true),
        (
            "partial",
            CapabilityStatus::Partial {
                reason: "native expression definition only".to_owned(),
            },
            true,
        ),
        (
            "unsupported",
            CapabilityStatus::Unsupported {
                reason: "no definition".to_owned(),
            },
            false,
        ),
        (
            "unavailable",
            CapabilityStatus::Unavailable {
                reason: "not started".to_owned(),
            },
            false,
        ),
        (
            "disabled",
            CapabilityStatus::Disabled {
                reason: "policy".to_owned(),
            },
            false,
        ),
        ("unknown", CapabilityStatus::Unknown { reason: None }, false),
    ];
    for (name, status, dispatched) in cases {
        let capabilities = BackendCapabilities {
            definition: status,
            ..MockBackend::new().capabilities()
        };
        let (mut server, dispatch) = server_with(capabilities);
        open(&mut server, POINT_DOCUMENT);
        dispatch.take();
        let outgoing = server.handle_incoming(request(
            json!(3),
            "textDocument/definition",
            point_params(0, 1),
        ));
        let jobs = dispatch.take();
        if dispatched {
            assert!(outgoing.is_empty(), "{name}: {outgoing:?}");
            assert_eq!(jobs.len(), 1, "{name}");
            assert_eq!(jobs[0].kind, JobKind::Definition, "{name}");
        } else {
            assert!(jobs.is_empty(), "{name}");
            assert_eq!(success_value(&only(outgoing)), &json!(null), "{name}");
        }
    }
}

#[test]
fn completion_dispatches_only_available_capabilities() {
    let cases = [
        ("supported", CapabilityStatus::Supported, true),
        (
            "partial",
            CapabilityStatus::Partial {
                reason: "native expression members only".to_owned(),
            },
            true,
        ),
        (
            "unsupported",
            CapabilityStatus::Unsupported {
                reason: "no completion".to_owned(),
            },
            false,
        ),
        (
            "unavailable",
            CapabilityStatus::Unavailable {
                reason: "not started".to_owned(),
            },
            false,
        ),
        (
            "disabled",
            CapabilityStatus::Disabled {
                reason: "policy".to_owned(),
            },
            false,
        ),
        ("unknown", CapabilityStatus::Unknown { reason: None }, false),
    ];
    for (name, status, dispatched) in cases {
        let capabilities = BackendCapabilities {
            completion: status,
            ..MockBackend::new().capabilities()
        };
        let (mut server, dispatch) = server_with(capabilities);
        let text = "<view>{{ user.na }}</view>";
        open(&mut server, text);
        dispatch.take();
        let outgoing = server.handle_incoming(request(
            json!(4),
            "textDocument/completion",
            point_params(0, 16),
        ));
        let jobs = dispatch.take();
        if dispatched {
            assert!(outgoing.is_empty(), "{name}: {outgoing:?}");
            assert_eq!(jobs.len(), 1, "{name}");
            assert_eq!(
                jobs[0].kind,
                JobKind::Completion(CompletionKind::ExpressionMember),
                "{name}"
            );
        } else {
            assert!(jobs.is_empty(), "{name}");
            assert_eq!(
                success_value(&only(outgoing)),
                &empty_completion_list(),
                "{name}"
            );
        }
    }
}

#[test]
fn partial_hover_answers_static_wxs_without_a_backend_job() {
    let capabilities = BackendCapabilities {
        hover: CapabilityStatus::Partial {
            reason: "native expression hover only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open(&mut server, WXS_TEXT);
    dispatch.take();
    let (line, character) = wxs_member_position(WXS_TEXT);
    let outgoing = server.handle_incoming(request(
        json!(5),
        "textDocument/hover",
        point_params(line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "static WXS hover must not start the backend"
    );
    let response = only(outgoing);
    let contents = success_value(&response)["contents"]["value"]
        .as_str()
        .expect("markdown hover");
    assert!(contents.contains("utils.format"), "{contents}");
    assert!(contents.contains("Formats a value."), "{contents}");
}

#[test]
fn partial_hover_answers_open_wxs_source_document() {
    let capabilities = BackendCapabilities {
        hover: CapabilityStatus::Partial {
            reason: "native expression hover only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open_at(&mut server, WXS_SOURCE_URI, "wxs", WXS_SOURCE_TEXT);
    dispatch.take();
    let (line, character) = wxs_source_declaration_position();
    let outgoing = server.handle_incoming(request(
        json!(12),
        "textDocument/hover",
        point_params_at(WXS_SOURCE_URI, line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "a .wxs hover must not start the backend"
    );
    let response = only(outgoing);
    let value = success_value(&response);
    let contents = value["contents"]["value"].as_str().expect("markdown hover");
    assert!(contents.contains("someFunc"), "{contents}");
    assert!(contents.contains("(name: string) => string"), "{contents}");
    assert!(contents.contains("@param {string} name"), "{contents}");
    assert!(!contents.contains("\\{"), "{contents}");
    // The range covers the hovered declaration token.
    assert_eq!(value["range"]["start"]["line"], json!(4), "{value}");
    assert_eq!(
        value["range"]["start"]["character"],
        json!("function ".len()),
        "{value}"
    );
    assert_eq!(
        value["range"]["end"]["character"],
        json!("function someFunc".len()),
        "{value}"
    );
}

#[test]
fn wxs_hover_returns_null_for_non_member_word() {
    let capabilities = BackendCapabilities {
        hover: CapabilityStatus::Partial {
            reason: "native expression hover only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open_at(&mut server, WXS_SOURCE_URI, "wxs", WXS_SOURCE_TEXT);
    dispatch.take();
    let (line, character) = wxs_source_keyword_position();
    let outgoing = server.handle_incoming(request(
        json!(13),
        "textDocument/hover",
        point_params_at(WXS_SOURCE_URI, line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "an unhandled .wxs word must not start the backend"
    );
    assert_eq!(success_value(&only(outgoing)), &json!(null));
}

#[test]
fn hover_does_not_enter_wxs_branch_for_script_documents() {
    // A `Partial` hover capability keeps `.ts` on the script/template gate:
    // `schedule_point` rejects non-template documents before dispatch and the
    // protocol-correct empty result is `null`. Asserting "no WXS payload and
    // no backend job" is the stable invariant here; the script document holds
    // a WXS-shaped declaration so a leaked static answer would be visible.
    let capabilities = BackendCapabilities {
        hover: CapabilityStatus::Partial {
            reason: "native expression hover only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open_at(&mut server, SCRIPT_URI, "typescript", SCRIPT_TEXT);
    dispatch.take();
    let (line, character) =
        position_at(SCRIPT_TEXT, SCRIPT_TEXT.find("someFunc").expect("name") + 1);
    let outgoing = server.handle_incoming(request(
        json!(14),
        "textDocument/hover",
        point_params_at(SCRIPT_URI, line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "a script document must not start the backend"
    );
    assert_eq!(success_value(&only(outgoing)), &json!(null));
}

#[test]
fn partial_definition_answers_static_wxs_without_a_backend_job() {
    let capabilities = BackendCapabilities {
        definition: CapabilityStatus::Partial {
            reason: "native expression definition only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open(&mut server, WXS_TEXT);
    dispatch.take();
    let (line, character) = wxs_member_position(WXS_TEXT);
    let outgoing = server.handle_incoming(request(
        json!(6),
        "textDocument/definition",
        point_params(line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "static WXS definition must not start the backend"
    );
    let response = only(outgoing);
    let locations = success_value(&response).as_array().expect("locations");
    assert_eq!(locations.len(), 1, "{response:?}");
    assert_eq!(
        locations[0]["range"]["start"]["line"],
        json!(1),
        "{response:?}"
    );
}

#[test]
fn partial_hover_answers_static_template_semantics() {
    let capabilities = BackendCapabilities {
        hover: CapabilityStatus::Partial {
            reason: "native expression hover only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open(&mut server, TEMPLATE_TEXT);
    dispatch.take();
    let (line, character) = template_call_position(TEMPLATE_TEXT);
    let outgoing = server.handle_incoming(request(
        json!(7),
        "textDocument/hover",
        point_params(line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "static template hover must not start the backend"
    );
    let response = only(outgoing);
    let contents = success_value(&response)["contents"]["value"]
        .as_str()
        .expect("markdown hover");
    assert!(contents.contains("template `card`"), "{contents}");
}

#[test]
fn partial_definition_answers_static_template_semantics() {
    let capabilities = BackendCapabilities {
        definition: CapabilityStatus::Partial {
            reason: "native expression definition only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open(&mut server, TEMPLATE_TEXT);
    dispatch.take();
    let (line, character) = template_call_position(TEMPLATE_TEXT);
    let outgoing = server.handle_incoming(request(
        json!(8),
        "textDocument/definition",
        point_params(line, character),
    ));
    assert!(
        dispatch.take().is_empty(),
        "static template definition must not start the backend"
    );
    let response = only(outgoing);
    let locations = success_value(&response).as_array().expect("locations");
    assert_eq!(locations.len(), 1, "{response:?}");
    assert_eq!(
        locations[0]["range"]["start"]["line"],
        json!(0),
        "{response:?}"
    );
}

#[test]
fn partial_hover_keeps_a_request_level_unsupported_result_empty() {
    let capabilities = BackendCapabilities {
        hover: CapabilityStatus::Partial {
            reason: "native expression hover only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open(&mut server, POINT_DOCUMENT);
    dispatch.take();
    assert_eq!(
        server
            .handle_incoming(request(json!(9), "textDocument/hover", point_params(0, 1),))
            .len(),
        0,
        "the hover response is deferred"
    );
    let job = dispatch.take().remove(0);
    let outgoing = server.handle_work(WorkDone {
        key: job.key.clone(),
        kind: JobKind::Hover,
        stamp: job.stamp.clone(),
        attempt: job.attempt,
        cancelled: false,
        outcome: Ok(Outcome::Hover(Box::new(HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status: CapabilityStatus::Unsupported {
                reason: "the cursor is not inside a mounted expression".to_owned(),
            },
            quality: TypeQuality::Unknown,
        }))),
    });
    assert_eq!(success_value(&only(outgoing)), &json!(null));
}

#[test]
fn partial_definition_keeps_a_request_level_unsupported_result_empty() {
    let capabilities = BackendCapabilities {
        definition: CapabilityStatus::Partial {
            reason: "native expression definition only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    open(&mut server, POINT_DOCUMENT);
    dispatch.take();
    assert_eq!(
        server
            .handle_incoming(request(
                json!(10),
                "textDocument/definition",
                point_params(0, 1),
            ))
            .len(),
        0,
        "the definition response is deferred"
    );
    let job = dispatch.take().remove(0);
    let outgoing = server.handle_work(WorkDone {
        key: job.key.clone(),
        kind: JobKind::Definition,
        stamp: job.stamp.clone(),
        attempt: job.attempt,
        cancelled: false,
        outcome: Ok(Outcome::Definition(Box::new(DefinitionResult {
            locations: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: "the cursor is not inside a mounted template expression".to_owned(),
            },
        }))),
    });
    assert_eq!(success_value(&only(outgoing)), &json!(null));
}

#[test]
fn partial_completion_keeps_a_request_level_unsupported_result_empty() {
    let capabilities = BackendCapabilities {
        completion: CapabilityStatus::Partial {
            reason: "native expression members only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let (mut server, dispatch) = server_with(capabilities);
    let text = "<view>{{ user.na }}</view>";
    open(&mut server, text);
    dispatch.take();
    assert_eq!(
        server
            .handle_incoming(request(
                json!(11),
                "textDocument/completion",
                point_params(0, 16),
            ))
            .len(),
        0,
        "the completion response is deferred"
    );
    let job = dispatch.take().remove(0);
    let outgoing = server.handle_work(WorkDone {
        key: job.key.clone(),
        kind: JobKind::Completion(CompletionKind::ExpressionMember),
        stamp: job.stamp.clone(),
        attempt: job.attempt,
        cancelled: false,
        outcome: Ok(Outcome::Completion(Box::new(CompletionResult {
            items: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: "the cursor is not inside a mounted template expression".to_owned(),
            },
            truncated: false,
        }))),
    });
    assert_eq!(success_value(&only(outgoing)), &empty_completion_list());
}
