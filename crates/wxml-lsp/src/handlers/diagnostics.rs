//! Document synchronization and diagnostics publication.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use wxml_types::{BackendEpoch, BackendError, CapabilityStatus, Diagnostic, ViewRevision};

use super::{
    content_changes, sanitized_message, sanitized_reason, severity, source_range_value,
    status_label, string, text_document,
};
use crate::jsonrpc::Outgoing;
use crate::project_host::{ChangeDocument, DocumentRoute, HostUpdate, OpenDocument};
use crate::scheduler::{Job, JobKind, Outcome, WorkDone};
use crate::server::{Server, log};
use crate::workspace::Document;

/// Method name used for pushed diagnostics.
const PUBLISH: &str = "textDocument/publishDiagnostics";

/// Converts one backend diagnostic, returning `None` for an invalid range.
fn diagnostic_value(document: &Document, diagnostic: &Diagnostic) -> Option<Value> {
    let range = source_range_value(document, diagnostic.range)?;
    Some(json!({
        "range": range,
        "severity": severity(diagnostic.severity),
        "source": "wxml-lsp",
        "message": sanitized_message(&diagnostic.message),
    }))
}

/// Builds the zero-width warning that reports a degraded check.
fn status_diagnostic(status: &CapabilityStatus) -> Value {
    let message = status.reason().map_or_else(
        || format!("type checking is {}", status_label(status)),
        |reason| {
            format!(
                "type checking is {}: {}",
                status_label(status),
                sanitized_reason(reason)
            )
        },
    );
    json!({
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 0},
        },
        "severity": 2,
        "source": "wxml-lsp",
        "message": message,
    })
}

/// Builds the warning published when the backend request itself failed.
///
/// The detailed [`BackendError`] text stays in the log: `Protocol` and
/// `Internal` messages may contain private paths or protocol details.
fn failure_diagnostic() -> Value {
    json!({
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 0},
        },
        "severity": 2,
        "source": "wxml-lsp",
        "message": "type checking is unavailable; see the server log",
    })
}

/// Sends one error-level `window/logMessage` to the client.
fn client_error(message: &str) -> Outgoing {
    Outgoing::notification("window/logMessage", json!({"type": 1, "message": message}))
}

/// Tells the client that a change was rejected and a full resync is needed.
///
/// The document state stayed at the old revision, so incremental changes based
/// on the rejected text would silently drift. `window/logMessage` is the only
/// portable channel for this without inventing a request.
fn rejected_change(reason: &str) -> Outgoing {
    client_error(&format!(
        "wxml-lsp rejected a document change ({reason}); the document may be out of sync. Send a full-text change to resynchronize."
    ))
}

/// Tells the client that an open request was rejected.
fn rejected_open(reason: &str) -> Outgoing {
    client_error(&format!(
        "wxml-lsp rejected opening a document ({reason}); the document is not tracked. Close it or reopen it with supported content."
    ))
}

/// Describes a backend failure for stderr only.
const fn failure_message(error: &BackendError) -> &'static str {
    match error {
        BackendError::Unavailable(_) => "backend unavailable",
        BackendError::Protocol(_) => "backend protocol error",
        BackendError::Timeout => "backend timed out",
        BackendError::Cancelled => "backend request cancelled",
        BackendError::Internal(_) => "backend internal error",
        _ => "backend failed",
    }
}

impl Server {
    /// Handles `textDocument/didOpen`.
    pub(crate) fn did_open(&mut self, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            log(format_args!("ignoring didOpen without parameters"));
            return Vec::new();
        };
        let Some(document) = text_document(&params) else {
            log(format_args!("ignoring didOpen without a text document"));
            return Vec::new();
        };
        let Some(uri) = string(document, "uri") else {
            log(format_args!("ignoring didOpen without a uri"));
            return Vec::new();
        };
        let Some(text) = string(document, "text") else {
            log(format_args!("ignoring didOpen without text"));
            return Vec::new();
        };
        let version = document.get("version").and_then(Value::as_u64);
        let language_id = document.get("languageId").and_then(Value::as_str);
        let text = text.to_owned();
        let route = self.host.route(uri);
        self.workspace
            .set_project(self.host.project_id(), self.host.project_revision());
        match route {
            DocumentRoute::Wxml(path) => self.open_wxml(uri, &path, text, version),
            DocumentRoute::Script(_) | DocumentRoute::Json(_) => {
                self.open_overlay(uri, text, version);
                Vec::new()
            }
            DocumentRoute::Unknown => {
                if let Err(error) = self.workspace.open(uri, text, version) {
                    log(format_args!("rejected didOpen: {error}"));
                    return vec![rejected_open(&error.to_string())];
                }
                if language_id == Some("wxml") {
                    self.synthetic_templates.insert(uri.to_owned());
                } else {
                    self.synthetic_templates.remove(uri);
                }
                if super::template_document(self, uri) {
                    self.schedule_diagnostics(uri)
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Opens a WXML template with its real project identity.
    fn open_wxml(
        &mut self,
        uri: &str,
        path: &wxml_project::ProjectPath,
        text: String,
        version: Option<u64>,
    ) -> Vec<Outgoing> {
        let file = self.host.file_id(path).ok();
        let view_revision = self
            .host
            .view_for(path)
            .map_or_else(|_| ViewRevision::new(1), |target| target.view_revision);
        if let Some(file) = file {
            match self
                .workspace
                .open_file(uri, file.clone(), text.clone(), version, view_revision)
            {
                Ok(()) => {
                    if let Some(document) = self.workspace.get(uri) {
                        self.host
                            .set_wxml_snapshot(file, Arc::clone(document.snapshot()));
                    }
                    if let Ok(target) = self.host.view_for(path) {
                        self.workspace.set_view_revision(uri, target.view_revision);
                    }
                    return self.schedule_diagnostics(uri);
                }
                Err(error) => log(format_args!("real document open failed: {error}")),
            }
        }
        if let Err(error) = self.workspace.open(uri, text, version) {
            log(format_args!("rejected didOpen: {error}"));
            return vec![rejected_open(&error.to_string())];
        }
        self.schedule_diagnostics(uri)
    }

    /// Stores a script/JSON overlay and schedules affected open views.
    fn open_overlay(&mut self, uri: &str, text: String, version: Option<u64>) {
        let path = match self.host.route(uri) {
            DocumentRoute::Script(path) | DocumentRoute::Json(path) => Some(path),
            _ => None,
        };
        if let Some(path) = path {
            if let Ok(file) = self.host.file_id(&path) {
                if let Err(error) =
                    self.workspace
                        .open_file(uri, file, text.clone(), version, ViewRevision::new(0))
                {
                    log(format_args!("side document open failed: {error}"));
                }
            }
        }
        match self.host.open_document(OpenDocument {
            uri: uri.to_owned(),
            text,
            version,
        }) {
            Ok(update) => self.apply_host_update(&update),
            Err(error) => log(format_args!("overlay open failed: {error}")),
        }
    }

    /// Revisions and dirty views after a script/JSON overlay update.
    pub(crate) fn apply_host_update(&mut self, update: &HostUpdate) {
        let Some(revision) = &update.project_revision else {
            return;
        };
        self.workspace.set_project_revision(revision);
        let mut uris = Vec::new();
        for path in &update.dirty_views {
            let Ok(file) = self.host.file_id_in(revision.project(), path) else {
                continue;
            };
            let Some(uri) = self
                .workspace
                .get_by_file(&file)
                .map(|document| document.uri().to_owned())
            else {
                continue;
            };
            if let Ok(target) = self.host.view_for_file(&file) {
                self.workspace.set_view_revision(&uri, target.view_revision);
            }
            uris.push(uri);
        }
        for uri in uris {
            let _ = self.schedule_diagnostics(&uri);
        }
    }

    /// Handles `textDocument/didChange`.
    pub(crate) fn did_change(&mut self, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            log(format_args!("ignoring didChange without parameters"));
            return Vec::new();
        };
        let Some(document) = text_document(&params) else {
            log(format_args!("ignoring didChange without a text document"));
            return Vec::new();
        };
        let Some(uri) = string(document, "uri") else {
            log(format_args!("ignoring didChange without a uri"));
            return Vec::new();
        };
        let Some(version) = document.get("version").and_then(Value::as_u64) else {
            log(format_args!("rejected didChange without a version"));
            return vec![rejected_change("missing version")];
        };
        let Some(changes) = content_changes(&params) else {
            log(format_args!(
                "rejected didChange with malformed content changes"
            ));
            return vec![rejected_change("malformed content changes")];
        };
        let route = self.host.route(uri);
        match self.workspace.change(uri, version, &changes) {
            Ok(()) => match route {
                DocumentRoute::Wxml(path) => {
                    let file = self
                        .workspace
                        .get(uri)
                        .map(|document| (document.file().clone(), Arc::clone(document.snapshot())));
                    if let Some((file, snapshot)) = file {
                        self.host.set_wxml_snapshot(file, snapshot);
                    }
                    if let Ok(target) = self.host.view_for(&path) {
                        self.workspace.set_view_revision(uri, target.view_revision);
                    }
                    self.schedule_diagnostics(uri)
                }
                DocumentRoute::Script(_) | DocumentRoute::Json(_) => {
                    let text = self
                        .workspace
                        .get(uri)
                        .map(|document| document.text().to_owned());
                    if let Some(text) = text {
                        match self.host.change_document(ChangeDocument {
                            uri: uri.to_owned(),
                            text,
                            version,
                        }) {
                            Ok(update) => self.apply_host_update(&update),
                            Err(error) => log(format_args!("overlay change failed: {error}")),
                        }
                    }
                    Vec::new()
                }
                DocumentRoute::Unknown => {
                    if super::template_document(self, uri) {
                        self.schedule_diagnostics(uri)
                    } else {
                        Vec::new()
                    }
                }
            },
            Err(error) => {
                log(format_args!("rejected didChange: {error}"));
                vec![rejected_change(&error.to_string())]
            }
        }
    }

    /// Handles `textDocument/didClose`.
    pub(crate) fn did_close(&mut self, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            log(format_args!("ignoring didClose without parameters"));
            return Vec::new();
        };
        let Some(document) = text_document(&params) else {
            log(format_args!("ignoring didClose without a text document"));
            return Vec::new();
        };
        let Some(uri) = string(document, "uri") else {
            log(format_args!("ignoring didClose without a uri"));
            return Vec::new();
        };
        if uri.is_empty() || uri.len() > crate::workspace::MAX_URI_BYTES {
            log(format_args!("ignoring didClose with an invalid URI"));
            return Vec::new();
        }
        let route = self.host.route(uri);
        let was_template = super::template_document(self, uri);
        self.synthetic_templates.remove(uri);
        let file = self
            .workspace
            .get(uri)
            .map(|document| document.file().clone());
        self.workspace.close(uri);
        self.clear_dirty(uri);
        if let Some(cancel) = self.diagnostics_cancel.remove(uri) {
            cancel.store(true, Ordering::SeqCst);
        }
        self.cancel_document_requests(uri);
        match route {
            DocumentRoute::Wxml(_) => {
                if let Some(file) = file {
                    self.host.remove_wxml_snapshot(&file);
                }
            }
            DocumentRoute::Script(_) | DocumentRoute::Json(_) => {
                match self.host.close_document(uri) {
                    Ok(update) => self.apply_host_update(&update),
                    Err(error) => log(format_args!("overlay close failed: {error}")),
                }
            }
            DocumentRoute::Unknown => {}
        }
        if was_template {
            vec![Outgoing::notification(
                PUBLISH,
                json!({"uri": uri, "diagnostics": []}),
            )]
        } else {
            Vec::new()
        }
    }

    /// Queues a diagnostics job for the current revision of `uri`.
    ///
    /// A full queue is not an error: the URI is remembered and retried after
    /// the next completed job, so the newest revision is not left
    /// unpublished behind older queued work.
    pub(crate) fn schedule_diagnostics(&mut self, uri: &str) -> Vec<Outgoing> {
        let epoch = self.refresh_epoch();
        self.schedule_diagnostics_with_epoch(uri, &epoch)
    }

    /// Queues diagnostics using an already observed backend epoch.
    pub(crate) fn schedule_diagnostics_with_epoch(
        &mut self,
        uri: &str,
        epoch: &BackendEpoch,
    ) -> Vec<Outgoing> {
        let Some(document) = self.workspace.get(uri) else {
            self.clear_dirty(uri);
            return Vec::new();
        };
        let Some(context) = document.request_context(epoch) else {
            self.clear_dirty(uri);
            log(format_args!("document context does not match the request"));
            return Vec::new();
        };
        let stamp = document.stamp(epoch);
        // A newer edit makes every older diagnostics request for this
        // document obsolete. Cancelling the queued job before moving the
        // pointer as well as after it closes the race with the worker.
        if let Some(previous) = self.diagnostics_cancel.remove(uri) {
            previous.store(true, Ordering::SeqCst);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            key: None,
            kind: JobKind::Diagnostics,
            context,
            offset: 0,
            stamp,
            cancel: Arc::clone(&cancel),
        };
        match self.dispatch.submit(job) {
            Ok(()) => {
                self.diagnostics_cancel.insert(uri.to_owned(), cancel);
                self.clear_dirty(uri);
            }
            Err(error) => {
                log(format_args!("diagnostics job queued for retry: {error:?}"));
                self.mark_dirty(uri);
            }
        }
        Vec::new()
    }

    /// Publishes one finished diagnostics job.
    pub(crate) fn diagnostics_done(
        &mut self,
        done: WorkDone,
        epoch: &BackendEpoch,
    ) -> Vec<Outgoing> {
        if done.cancelled {
            return Vec::new();
        }
        if done.stamp.epoch != *epoch {
            let current = self.workspace.is_current(&done.stamp);
            // A failed Node request returns a syntax-only `Unavailable` result
            // under the pre-failure epoch. Publishing it is safe and avoids a
            // failure -> epoch bump -> drop -> reschedule loop.
            let failed_fallback = matches!(
                &done.outcome,
                Ok(Outcome::Diagnostics(result))
                    if matches!(result.status, CapabilityStatus::Unavailable { .. })
            );
            if !(failed_fallback && current) {
                log(format_args!(
                    "dropping diagnostics for an old backend epoch and rescheduling"
                ));
                // The document may not have changed, but the backend restarted;
                // the new generation has not checked this snapshot yet. Mark it
                // dirty so the retry that follows this completed job schedules a
                // fresh request with the new epoch.
                if current {
                    if let Some(uri) = self
                        .workspace
                        .get_by_file(&done.stamp.file)
                        .map(|document| document.uri().to_owned())
                    {
                        // Reuse the epoch already read for this event instead
                        // of asking the backend again.
                        let _ = self.schedule_diagnostics_with_epoch(&uri, epoch);
                    }
                }
                return Vec::new();
            }
        }
        if !self.workspace.is_current(&done.stamp) {
            // Every content, view or project revision change that affects this
            // document schedules a fresh job itself (`didChange`,
            // `set_wxml_snapshot`, `apply_host_update`). Publishing this stale
            // result would either duplicate that job or race a `didClose`
            // acknowledgement, so drop it.
            log(format_args!("dropping diagnostics for an old revision"));
            return Vec::new();
        }
        let Some(document) = self.workspace.get_by_file(&done.stamp.file) else {
            return Vec::new();
        };
        let mut diagnostics = Vec::new();
        let status = match done.outcome {
            Ok(Outcome::Diagnostics(result)) => {
                for diagnostic in &result.diagnostics {
                    match diagnostic_value(document, diagnostic) {
                        Some(value) => diagnostics.push(value),
                        None => log(format_args!(
                            "skipping diagnostic with an invalid source range"
                        )),
                    }
                }
                Some(result.status)
            }
            Ok(_) => {
                log(format_args!(
                    "diagnostics job returned the wrong result kind"
                ));
                None
            }
            Err(error) => {
                log(format_args!(
                    "diagnostics failed: {}",
                    failure_message(&error)
                ));
                None
            }
        };
        match status {
            Some(status) if !status.is_supported() => {
                diagnostics.push(status_diagnostic(&status));
            }
            Some(_) => {}
            None => diagnostics.push(failure_diagnostic()),
        }
        let params = json!({
            "uri": document.uri(),
            "version": document.client_version().get(),
            "diagnostics": diagnostics,
        });
        vec![Outgoing::notification(PUBLISH, params)]
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
}
