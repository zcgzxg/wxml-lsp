//! Document synchronization and diagnostics publication.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use wxml_project::ProjectPath;
use wxml_types::{BackendEpoch, BackendError, CapabilityStatus, Diagnostic, ViewRevision};

use super::{
    content_changes, sanitized_message, sanitized_reason, severity, source_range_value,
    status_label, string, text_document,
};
use crate::jsonrpc::Outgoing;
use crate::project_host::{ChangeDocument, DocumentRoute, HostUpdate, OpenDocument};
use crate::scheduler::{Job, JobKind, Outcome, WorkDone};
use crate::semantics::event_diagnostics::EventDiagnostics;
use crate::server::{CachedDiagnostics, MAX_DEBOUNCED_DIAGNOSTICS, Server, log};
use crate::workspace::Document;

/// Method name used for pushed diagnostics.
const PUBLISH: &str = "textDocument/publishDiagnostics";

/// Converts one backend diagnostic, returning `None` for an invalid range.
fn diagnostic_value(document: &Document, diagnostic: &Diagnostic) -> Option<Value> {
    let range = source_range_value(document, diagnostic.range)?;
    let mut value = json!({
        "range": range,
        "severity": severity(diagnostic.severity),
        "source": "wxml-lsp",
        "message": sanitized_message(&diagnostic.message),
    });
    if let Some(code) = diagnostic.code.as_ref() {
        if let Some(object) = value.as_object_mut() {
            object.insert("code".to_owned(), Value::String(sanitized_reason(code)));
        }
    }
    Some(value)
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

/// Builds the publish payload from backend and local diagnostics.
///
/// Diagnostic order is deterministic: the merged list is stably sorted by
/// source range, so a payload does not depend on which source produced a
/// diagnostic. Duplicates are removed by exact range and message so a backend
/// that reports the same missing handler cannot appear twice.
fn publish_value(
    document: &Document,
    backend: &[Diagnostic],
    local: &[Diagnostic],
    event: &[Diagnostic],
    status: Option<&CapabilityStatus>,
    failure: bool,
) -> Value {
    let mut mapped = Vec::new();
    let mut seen = HashSet::new();
    for diagnostic in backend.iter().chain(local.iter()).chain(event.iter()) {
        if !seen.insert((
            diagnostic.range.start(),
            diagnostic.range.end(),
            diagnostic.message.as_str(),
        )) {
            continue;
        }
        match diagnostic_value(document, diagnostic) {
            Some(value) => mapped.push((diagnostic.range.start(), diagnostic.range.end(), value)),
            None => log(format_args!(
                "skipping diagnostic with an invalid source range"
            )),
        }
    }
    mapped.sort_by_key(|(start, end, _)| (*start, *end));
    let mut diagnostics: Vec<Value> = mapped.into_iter().map(|(_, _, value)| value).collect();
    match status {
        Some(status) if !status.is_supported() => {
            diagnostics.push(status_diagnostic(status));
        }
        None if failure => diagnostics.push(failure_diagnostic()),
        _ => {}
    }
    json!({
        "uri": document.uri(),
        "version": document.client_version().get(),
        "diagnostics": diagnostics,
    })
}

/// Merges the local event coverage into the backend diagnostics status.
///
/// The backend reason wins whenever it is already degraded: publishing the
/// local event reason instead would hide why the rest of the template was not
/// checked. When the backend claimed a complete check, the local event gap is
/// the only degradation to report.
fn merged_status(
    backend: Option<CapabilityStatus>,
    event: Option<CapabilityStatus>,
) -> Option<CapabilityStatus> {
    match (backend, event) {
        (backend, None) => backend,
        (None, event) => event,
        (Some(backend), Some(event)) => {
            if backend.is_supported() {
                Some(event)
            } else {
                Some(backend)
            }
        }
    }
}

/// Runs the local missing-event-handler check for one open template.
///
/// Node publishes event-handler diagnostics itself and reports a `Supported`
/// diagnostics capability, so the local check only fills the gap for a
/// degraded or absent backend. A host without a prepared view (synthetic or
/// project-less documents) stays quiet instead of failing the publish.
fn local_event_diagnostics(server: &Server, document: &Document) -> EventDiagnostics {
    if server.capabilities.diagnostics.is_supported() {
        return EventDiagnostics::empty();
    }
    let Ok(view) = server.host.analysis(document.snapshot()) else {
        return EventDiagnostics::empty();
    };
    crate::semantics::event_diagnostics::diagnostics(&view, document.text())
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
        self.cancel_debounced_diagnostics(uri);
        let version = document.get("version").and_then(Value::as_u64);
        let language_id = document.get("languageId").and_then(Value::as_str);
        let text = text.to_owned();
        let route = self.host.route(uri);
        self.workspace
            .set_project(self.host.project_id(), self.host.project_revision());
        match route {
            DocumentRoute::Wxml(path) => self.open_wxml(uri, &path, text, version),
            DocumentRoute::Script(_) | DocumentRoute::Json(_) | DocumentRoute::Style(_) => {
                self.open_overlay(uri, text, version)
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
        path: &ProjectPath,
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

    /// Stores a script, JSON or style overlay and schedules affected open views.
    fn open_overlay(&mut self, uri: &str, text: String, version: Option<u64>) -> Vec<Outgoing> {
        let path = match self.host.route(uri) {
            DocumentRoute::Script(path)
            | DocumentRoute::Json(path)
            | DocumentRoute::Style(path) => Some(path),
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
            Err(error) => {
                log(format_args!("overlay open failed: {error}"));
                Vec::new()
            }
        }
    }

    /// Revisions and dirty views after a script/JSON overlay update.
    pub(crate) fn apply_host_update(&mut self, update: &HostUpdate) -> Vec<Outgoing> {
        if let Some(revision) = &update.project_revision {
            self.workspace.set_project_revision(revision);
        }
        let mut uris = Vec::new();
        if let Some(revision) = &update.project_revision {
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
        }
        for uri in uris {
            // Script/JSON/style overlays are authoritative recovery inputs.
            // They schedule immediately instead of joining the typing
            // debounce: an unsaved type change must be visible without another
            // template keystroke. The scheduler still coalesces duplicate
            // diagnostics jobs per document.
            self.cancel_debounced_diagnostics(&uri);
            let _ = self.schedule_diagnostics(&uri);
        }
        let style_paths: HashSet<&str> =
            update.style_views.iter().map(ProjectPath::as_str).collect();
        let style_uris: Vec<String> = self
            .workspace
            .documents()
            .filter(|document| {
                style_paths.contains(document.file().path())
                    && update
                        .project
                        .as_ref()
                        .is_none_or(|project| document.file().project() == project)
            })
            .map(|document| document.uri().to_owned())
            .collect();
        let mut outgoing = Vec::new();
        for uri in style_uris {
            outgoing.extend(self.refresh_style_diagnostics(&uri));
        }
        outgoing
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
                    self.debounce_diagnostics(uri)
                }
                DocumentRoute::Script(_) | DocumentRoute::Json(_) | DocumentRoute::Style(_) => {
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
                            Ok(update) => return self.apply_host_update(&update),
                            Err(error) => log(format_args!("overlay change failed: {error}")),
                        }
                    }
                    Vec::new()
                }
                DocumentRoute::Unknown => {
                    if super::template_document(self, uri) {
                        self.debounce_diagnostics(uri)
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
        self.cancel_debounced_diagnostics(uri);
        if let Some(cancel) = self.diagnostics_cancel.remove(uri) {
            cancel.store(true, Ordering::SeqCst);
        }
        self.backend_diagnostics.remove(uri);
        self.cancel_document_requests(uri);
        let mut outgoing = match route {
            DocumentRoute::Wxml(_) => {
                if let Some(file) = file {
                    self.host.remove_wxml_snapshot(&file);
                }
                Vec::new()
            }
            DocumentRoute::Script(_) | DocumentRoute::Json(_) | DocumentRoute::Style(_) => {
                match self.host.close_document(uri) {
                    Ok(update) => self.apply_host_update(&update),
                    Err(error) => {
                        log(format_args!("overlay close failed: {error}"));
                        Vec::new()
                    }
                }
            }
            DocumentRoute::Unknown => Vec::new(),
        };
        if was_template {
            outgoing.push(Outgoing::notification(
                PUBLISH,
                json!({"uri": uri, "diagnostics": []}),
            ));
        }
        outgoing
    }

    /// Defers diagnostics for one typed edit until the quiet period ends.
    ///
    /// Opening, saving and dependency/config updates schedule immediately.
    /// The pending set is bounded; when it is full the newest document is
    /// scheduled immediately rather than growing the timer map.
    pub(crate) fn debounce_diagnostics(&mut self, uri: &str) -> Vec<Outgoing> {
        if self.diagnostics_debounce.is_zero() {
            return self.schedule_diagnostics(uri);
        }
        if let Some(previous) = self.diagnostics_cancel.remove(uri) {
            previous.store(true, Ordering::SeqCst);
        }
        if !self.debounced_diagnostics.contains_key(uri)
            && self.debounced_diagnostics.len() >= MAX_DEBOUNCED_DIAGNOSTICS
        {
            log(format_args!(
                "diagnostics debounce is full; scheduling the newest edit immediately"
            ));
            return self.schedule_diagnostics(uri);
        }
        self.debounced_diagnostics
            .insert(uri.to_owned(), Instant::now() + self.diagnostics_debounce);
        Vec::new()
    }

    /// Cancels a pending debounce for one document.
    pub(crate) fn cancel_debounced_diagnostics(&mut self, uri: &str) {
        self.debounced_diagnostics.remove(uri);
    }

    /// Schedules every diagnostics deadline that has passed.
    pub(crate) fn flush_due_diagnostics(&mut self, now: Instant) -> Vec<Outgoing> {
        if self.debounced_diagnostics.is_empty() {
            return Vec::new();
        }
        let due: Vec<String> = self
            .debounced_diagnostics
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(uri, _)| uri.clone())
            .collect();
        if due.is_empty() {
            return Vec::new();
        }
        let epoch = self.refresh_epoch();
        for uri in due {
            self.debounced_diagnostics.remove(&uri);
            let _ = self.schedule_diagnostics_with_epoch(&uri, &epoch);
        }
        Vec::new()
    }

    /// Flushes every pending debounce regardless of its deadline.
    #[cfg(test)]
    pub(crate) fn flush_all_debounced_diagnostics(&mut self) {
        let now = Instant::now() + self.diagnostics_debounce;
        let _ = self.flush_due_diagnostics(now);
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
        self.debounced_diagnostics.remove(uri);
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
            attempt: 0,
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
        let local = self.host.style_diagnostics(document.snapshot());
        let event = local_event_diagnostics(self, document);
        let (backend, status, failure) = match done.outcome {
            Ok(Outcome::Diagnostics(result)) => {
                self.backend_diagnostics.insert(
                    document.uri().to_owned(),
                    CachedDiagnostics {
                        stamp: done.stamp.clone(),
                        diagnostics: result.diagnostics.clone(),
                        status: result.status.clone(),
                    },
                );
                (result.diagnostics, Some(result.status), false)
            }
            Ok(_) => {
                log(format_args!(
                    "diagnostics job returned the wrong result kind"
                ));
                let status = CapabilityStatus::Unavailable {
                    reason: "see the server log".to_owned(),
                };
                self.backend_diagnostics.insert(
                    document.uri().to_owned(),
                    CachedDiagnostics {
                        stamp: done.stamp.clone(),
                        diagnostics: Vec::new(),
                        status: status.clone(),
                    },
                );
                (Vec::new(), Some(status), false)
            }
            Err(error) => {
                log(format_args!(
                    "diagnostics failed: {}",
                    failure_message(&error)
                ));
                let status = CapabilityStatus::Unavailable {
                    reason: "see the server log".to_owned(),
                };
                self.backend_diagnostics.insert(
                    document.uri().to_owned(),
                    CachedDiagnostics {
                        stamp: done.stamp.clone(),
                        diagnostics: Vec::new(),
                        status: status.clone(),
                    },
                );
                (Vec::new(), Some(status), false)
            }
        };
        let mut params = publish_value(
            document,
            &backend,
            &local,
            &event.diagnostics,
            merged_status(status, event.status).as_ref(),
            failure,
        );
        let mode = super::mode_diagnostics::diagnostics(document);
        if !mode.is_empty() {
            if let Some(diagnostics) = params.get_mut("diagnostics").and_then(Value::as_array_mut) {
                diagnostics.extend(mode);
            }
        }
        vec![Outgoing::notification(PUBLISH, params)]
    }

    /// Re-publishes cached backend diagnostics plus fresh local style
    /// diagnostics after a style/config change.
    pub(crate) fn refresh_style_diagnostics(&self, uri: &str) -> Vec<Outgoing> {
        let Some(document) = self.workspace.get(uri) else {
            return Vec::new();
        };
        let local = self.host.style_diagnostics(document.snapshot());
        let event = local_event_diagnostics(self, document);
        let cached = self
            .backend_diagnostics
            .get(uri)
            .filter(|cached| self.workspace.is_current(&cached.stamp))
            .cloned();
        let mode = super::mode_diagnostics::diagnostics(document);
        if cached.is_none() && local.is_empty() && event.diagnostics.is_empty() && mode.is_empty() {
            return Vec::new();
        }
        let status = merged_status(
            cached.as_ref().map(|cached| cached.status.clone()),
            event.status,
        );
        let mut params = cached.map_or_else(
            || {
                publish_value(
                    document,
                    &[],
                    &local,
                    &event.diagnostics,
                    status.as_ref(),
                    false,
                )
            },
            |cached| {
                publish_value(
                    document,
                    &cached.diagnostics,
                    &local,
                    &event.diagnostics,
                    status.as_ref(),
                    false,
                )
            },
        );
        if !mode.is_empty() {
            if let Some(diagnostics) = params.get_mut("diagnostics").and_then(Value::as_array_mut) {
                diagnostics.extend(mode);
            }
        }
        vec![Outgoing::notification(PUBLISH, params)]
    }
}

#[cfg(test)]
mod tests;
