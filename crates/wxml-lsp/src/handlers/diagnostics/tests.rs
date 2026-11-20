use std::sync::Arc;

use serde_json::json;
use wxml_types::{Backend, BackendCapabilities, DiagnosticSeverity, MockBackend, SourceRange};

use super::*;
use crate::scheduler::SubmitError;
use crate::server::Dispatch;
use crate::workspace::Workspace;

/// A dispatch that accepts and drops every job.
struct NoopDispatch;

impl Dispatch for NoopDispatch {
    fn submit(&self, _job: Job) -> Result<(), SubmitError> {
        Ok(())
    }
}

fn range(start: u32, end: u32) -> SourceRange {
    SourceRange::from_bounds(start, end).expect("range")
}

fn diagnostic(start: u32, end: u32, severity: DiagnosticSeverity, message: &str) -> Diagnostic {
    Diagnostic {
        range: range(start, end),
        severity,
        message: message.to_owned(),
        code: None,
    }
}

fn document_for(text: &str) -> Workspace {
    let mut workspace = Workspace::new();
    workspace
        .open("file:///a.wxml", text.to_owned(), Some(1))
        .expect("open");
    workspace
}

#[test]
fn event_diagnostics_are_sorted_and_deduplicated() {
    let workspace = document_for("<view bindtap=\"onMissing\" />");
    let document = workspace.get("file:///a.wxml").expect("document");
    let backend = vec![diagnostic(
        12,
        21,
        DiagnosticSeverity::Error,
        "Event handler \"onMissing\" for \"tap\" is not defined on this page",
    )];
    let local = vec![Diagnostic {
        code: Some("style.missing-resource".to_owned()),
        ..diagnostic(0, 1, DiagnosticSeverity::Warning, "missing image")
    }];
    let event = vec![
        diagnostic(
            12,
            21,
            DiagnosticSeverity::Error,
            "Event handler \"onMissing\" for \"tap\" is not defined on this page",
        ),
        diagnostic(
            24,
            28,
            DiagnosticSeverity::Error,
            "Event handler \"onOther\" for \"tap\" is not defined on this page",
        ),
    ];
    let value = publish_value(document, &backend, &local, &event, None, false);
    let diagnostics = value["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 3, "{value}");
    assert_eq!(diagnostics[0]["code"], json!("style.missing-resource"));
    assert_eq!(diagnostics[1]["message"], json!(backend[0].message.clone()));
    assert_eq!(
        diagnostics[2]["message"],
        json!("Event handler \"onOther\" for \"tap\" is not defined on this page")
    );
    assert_eq!(diagnostics[1]["source"], json!("wxml-lsp"));
    assert!(diagnostics[1].get("code").is_none(), "{value}");
    assert_eq!(diagnostics[0]["range"]["start"]["character"], json!(0));
}

#[test]
fn an_event_only_template_publishes_errors_without_a_status_note() {
    let workspace = document_for("<view bindtap=\"onMissing\" />");
    let document = workspace.get("file:///a.wxml").expect("document");
    let event = vec![diagnostic(
        12,
        21,
        DiagnosticSeverity::Error,
        "Event handler \"onMissing\" for \"tap\" is not defined on this page",
    )];
    let value = publish_value(document, &[], &[], &event, None, false);
    let diagnostics = value["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1, "{value}");
    assert_eq!(diagnostics[0]["severity"], json!(1));
    assert_eq!(diagnostics[0]["source"], json!("wxml-lsp"));
    assert!(diagnostics[0].get("code").is_none(), "{value}");
}

#[test]
fn event_status_only_downgrades_a_complete_backend() {
    let supported = CapabilityStatus::Supported;
    let event = CapabilityStatus::Partial {
        reason: "event table is partial".to_owned(),
    };
    let backend = CapabilityStatus::Partial {
        reason: "backend reason".to_owned(),
    };
    assert_eq!(
        merged_status(Some(supported.clone()), None),
        Some(supported.clone())
    );
    assert_eq!(
        merged_status(None, Some(event.clone())),
        Some(event.clone())
    );
    assert_eq!(
        merged_status(Some(supported), Some(event.clone())),
        Some(event.clone())
    );
    assert_eq!(
        merged_status(Some(backend.clone()), Some(event)),
        Some(backend)
    );
}

#[test]
fn local_event_diagnostics_without_a_prepared_view_stay_empty() {
    let capabilities = BackendCapabilities {
        diagnostics: CapabilityStatus::Partial {
            reason: "native diagnostics only".to_owned(),
        },
        ..MockBackend::new().capabilities()
    };
    let mut server = Server::new(
        Arc::new(MockBackend::new()) as Arc<dyn Backend>,
        capabilities,
        Arc::new(NoopDispatch),
    );
    server
        .workspace
        .open(
            "file:///documents/1.wxml",
            "<view bindtap=\"onMissing\" />".to_owned(),
            Some(1),
        )
        .expect("open");
    let document = server
        .workspace
        .get("file:///documents/1.wxml")
        .expect("document");
    assert_eq!(
        local_event_diagnostics(&server, document),
        EventDiagnostics::empty()
    );
}

#[test]
fn a_supported_backend_keeps_its_own_event_diagnostics() {
    let capabilities = BackendCapabilities {
        diagnostics: CapabilityStatus::Supported,
        ..MockBackend::new().capabilities()
    };
    let mut server = Server::new(
        Arc::new(MockBackend::new()) as Arc<dyn Backend>,
        capabilities,
        Arc::new(NoopDispatch),
    );
    server
        .workspace
        .open(
            "file:///documents/1.wxml",
            "<view bindtap=\"onMissing\" />".to_owned(),
            Some(1),
        )
        .expect("open");
    let document = server
        .workspace
        .get("file:///documents/1.wxml")
        .expect("document");
    assert_eq!(
        local_event_diagnostics(&server, document),
        EventDiagnostics::empty()
    );
}

#[test]
fn status_diagnostics_explain_the_degradation() {
    let value = status_diagnostic(&CapabilityStatus::Disabled {
        reason: "policy".to_owned(),
    });
    assert_eq!(value["severity"], json!(2));
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("disabled")
    );
    assert!(
        value["message"]
            .as_str()
            .expect("message")
            .contains("policy")
    );
    let dirty = status_diagnostic(&CapabilityStatus::Partial {
        reason: "x\u{1b}[2Jy".to_owned(),
    });
    let message = dirty["message"].as_str().expect("message");
    assert!(!message.contains('\u{1b}'), "{message}");
    assert!(message.contains("x [2Jy"), "{message}");
    let bare = status_diagnostic(&CapabilityStatus::Unknown { reason: None });
    assert_eq!(bare["message"], json!("type checking is unknown"));
}

#[test]
fn failure_diagnostics_hide_backend_details() {
    let value = failure_diagnostic();
    let message = value["message"].as_str().expect("message");
    assert!(message.contains("unavailable"), "{message}");
    assert!(!message.contains('/'), "{message}");
    assert_eq!(failure_message(&BackendError::Timeout), "backend timed out");
    assert_eq!(
        failure_message(&BackendError::Protocol("frame".to_owned())),
        "backend protocol error"
    );
    assert_eq!(
        failure_message(&BackendError::Cancelled),
        "backend request cancelled"
    );
    assert_eq!(
        failure_message(&BackendError::Internal("bug".to_owned())),
        "backend internal error"
    );
}
