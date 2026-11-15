//! Hover, definition and cancellation of point queries.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use wxml_types::{BackendError, CapabilityStatus, HoverResult, MarkupKind, TypeQuality};

use super::{
    hover_target, line_index_range_value, markdown_escape, quality_label, sanitized_reason,
    source_range_value, status_label,
};
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::scheduler::{Job, JobKind, Outcome, SubmitError, WorkDone};
use crate::server::{Inflight, Server, log};
use crate::workspace::AnalysisStamp;
use wxml_types::BackendEpoch;

/// Returns the JSON markup kind for a hover.
const fn markup_kind(kind: MarkupKind) -> &'static str {
    match kind {
        MarkupKind::Markdown => "markdown",
        MarkupKind::PlainText => "plaintext",
    }
}

/// Adds explicit degradation notes to backend hover text.
fn decorated_contents(result: &HoverResult) -> String {
    let mut text = result.contents.clone();
    let mut notes = Vec::new();
    if !result.status.is_supported() {
        let label = status_label(&result.status);
        notes.push(result.status.reason().map_or_else(
            || format!("check status: {label}"),
            |reason| {
                format!(
                    "check status: {label} ({})",
                    if matches!(result.markup, MarkupKind::Markdown) {
                        markdown_escape(&sanitized_reason(reason))
                    } else {
                        sanitized_reason(reason)
                    }
                )
            },
        ));
    }
    if result.quality != TypeQuality::Known {
        notes.push(format!("type quality: {}", quality_label(result.quality)));
    }
    if notes.is_empty() {
        return text;
    }
    match result.markup {
        MarkupKind::Markdown => {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str("---\n\n");
            for note in notes {
                text.push('*');
                text.push_str(&note);
                text.push('*');
                text.push('\n');
            }
            text.pop();
        }
        MarkupKind::PlainText => {
            for note in notes {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push('[');
                text.push_str(&note);
                text.push(']');
            }
        }
    }
    text
}

/// Maps a backend hover to the LSP result shape.
fn hover_value(server: &Server, stamp: &AnalysisStamp, result: &HoverResult) -> Option<Value> {
    let document = server.workspace.get_by_file(&stamp.file)?;
    let mut value = json!({
        "contents": {
            "kind": markup_kind(result.markup),
            "value": decorated_contents(result),
        },
    });
    if let Some(range) = result
        .range
        .and_then(|range| source_range_value(document, range))
    {
        if let Some(object) = value.as_object_mut() {
            object.insert("range".to_owned(), range);
        }
    }
    Some(value)
}

/// Maps backend errors to a short message that never contains internal paths.
const fn failure_message(error: &BackendError) -> &'static str {
    match error {
        BackendError::Unavailable(_) => "type checking is unavailable; see the server log",
        BackendError::Protocol(_) => "type backend protocol error; see the server log",
        BackendError::Timeout => "type checking timed out",
        BackendError::Cancelled => "type checking was cancelled",
        _ => "type checking failed; see the server log",
    }
}

/// Maximum number of distinct definition target files read per request.
const MAX_DEFINITION_FILES: usize = 8;

/// Maximum estimated text + line-index bytes read for definition targets.
const MAX_DEFINITION_INDEX_BYTES: usize = 64 * 1024 * 1024;

/// Mapped LSP locations plus whether any target could not be mapped.
struct MappedDefinition {
    locations: Vec<Value>,
    unmappable: bool,
}

/// Builds definition locations, skipping targets that cannot be mapped.
///
/// The per-request budget bounds disk reads and `LineIndex` allocations even
/// when a malicious backend returns many distinct target files.
fn definition_value(
    server: &Server,
    stamp: &AnalysisStamp,
    result: &wxml_types::DefinitionResult,
) -> MappedDefinition {
    let Some(document) = server.workspace.get_by_file(&stamp.file) else {
        return MappedDefinition {
            locations: Vec::new(),
            unmappable: true,
        };
    };
    let mut locations = Vec::new();
    let mut unmappable = false;
    let mut indexes: HashMap<wxml_types::FileId, Option<Arc<wxml_text::LineIndex>>> =
        HashMap::new();
    let mut index_bytes = 0_usize;
    let mut budget_exhausted = false;
    for location in &result.locations {
        if location.file == stamp.file {
            let Some(range) = source_range_value(document, location.range) else {
                log(format_args!("definition target has an invalid range"));
                unmappable = true;
                continue;
            };
            locations.push(json!({
                // The open document's real client URI, never the synthetic FileId.
                "uri": document.uri(),
                "range": range,
            }));
            continue;
        }
        let Some(uri) = server.host.uri_for(&location.file) else {
            log(format_args!(
                "definition target is outside the project and has no client URI"
            ));
            unmappable = true;
            continue;
        };
        let line_index = if let Some(index) = indexes.get(&location.file) {
            let Some(index) = index else {
                unmappable = true;
                continue;
            };
            Arc::clone(index)
        } else {
            if indexes.len() >= MAX_DEFINITION_FILES || budget_exhausted {
                indexes.insert(location.file.clone(), None);
                log(format_args!("definition target budget reached"));
                unmappable = true;
                continue;
            }
            let Some(text) = server.host.file_text(&location.file) else {
                indexes.insert(location.file.clone(), None);
                log(format_args!("definition target text is unavailable"));
                unmappable = true;
                continue;
            };
            let cost = text
                .len()
                .saturating_add(8_usize.saturating_mul(newline_count(&text)));
            if index_bytes.saturating_add(cost) > MAX_DEFINITION_INDEX_BYTES {
                // No later file is read once the budget is exhausted.
                budget_exhausted = true;
                indexes.insert(location.file.clone(), None);
                log(format_args!("definition target byte budget reached"));
                unmappable = true;
                continue;
            }
            index_bytes = index_bytes.saturating_add(cost);
            let index = Arc::new(wxml_text::LineIndex::new(text));
            indexes.insert(location.file.clone(), Some(Arc::clone(&index)));
            index
        };
        let Some(range) = line_index_range_value(&line_index, location.range) else {
            log(format_args!("definition target has an invalid range"));
            unmappable = true;
            continue;
        };
        locations.push(json!({"uri": uri, "range": range}));
    }
    MappedDefinition {
        locations,
        unmappable,
    }
}

/// Counts line starts for the definition index budget.
fn newline_count(text: &str) -> usize {
    text.match_indices('\n').count()
}

/// Short message for a full or stopped dispatch queue.
const fn submit_message(error: SubmitError) -> &'static str {
    match error {
        SubmitError::Busy => "server is busy",
        SubmitError::Stopped => "server is not running",
    }
}

impl Server {
    /// Handles `textDocument/hover`.
    pub(crate) fn hover(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        if !self.capabilities.hover.is_supported() {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::METHOD_NOT_FOUND,
                "hover is not supported",
            )];
        }
        self.schedule_point(id, JobKind::Hover, params)
    }

    /// Handles `textDocument/definition`.
    pub(crate) fn definition(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        if !self.capabilities.definition.is_supported() {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::METHOD_NOT_FOUND,
                "definition is not supported",
            )];
        }
        self.schedule_point(id, JobKind::Definition, params)
    }

    /// Validates a point request and queues its backend job.
    fn schedule_point(&mut self, id: Value, kind: JobKind, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some((uri, position)) = hover_target(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument or position",
            )];
        };
        let epoch = self.refresh_epoch();
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some(offset) = document.offset_at(position) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some(context) = document.request_context(&epoch) else {
            log(format_args!("document context does not match the request"));
            return vec![Outgoing::success(id, Value::Null)];
        };
        let stamp = document.stamp(&epoch);
        let Some(key) = RequestKey::from_id(&id) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "request id must be a string or a number",
            )];
        };
        if self.inflight.contains_key(&key) {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_REQUEST,
                "request id is already in flight",
            )];
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.inflight.insert(
            key.clone(),
            Inflight {
                id: id.clone(),
                cancel: Arc::clone(&cancel),
                uri: uri.to_owned(),
            },
        );
        let job = Job {
            key: Some(key.clone()),
            kind,
            context,
            offset,
            stamp,
            cancel,
        };
        match self.dispatch.submit(job) {
            Ok(()) => Vec::new(),
            Err(error) => {
                self.inflight.remove(&key);
                log(format_args!("dispatch rejected a {kind:?} job: {error:?}"));
                vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INTERNAL_ERROR,
                    submit_message(error),
                )]
            }
        }
    }

    /// Answers one finished hover job.
    pub(crate) fn hover_done(
        &self,
        id: Value,
        done: WorkDone,
        epoch: &BackendEpoch,
    ) -> Vec<Outgoing> {
        if done.cancelled {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_CANCELLED,
                "request cancelled",
            )];
        }
        if done.stamp.epoch != *epoch || !self.workspace.is_current(&done.stamp) {
            log(format_args!("dropping a hover result for an old revision"));
            return vec![Outgoing::success(id, Value::Null)];
        }
        match done.outcome {
            Ok(Outcome::Hover(result)) => match hover_value(self, &done.stamp, &result) {
                Some(value) => vec![Outgoing::success(id, value)],
                None => vec![Outgoing::success(id, Value::Null)],
            },
            Ok(_) => {
                log(format_args!("hover job returned the wrong result kind"));
                vec![Outgoing::success(id, Value::Null)]
            }
            Err(error) => {
                log(format_args!("hover failed: {error}"));
                vec![Outgoing::success(id, Value::Null)]
            }
        }
    }

    /// Answers one finished definition job.
    pub(crate) fn definition_done(
        &self,
        id: Value,
        done: WorkDone,
        epoch: &BackendEpoch,
    ) -> Vec<Outgoing> {
        if done.cancelled {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_CANCELLED,
                "request cancelled",
            )];
        }
        if done.stamp.epoch != *epoch || !self.workspace.is_current(&done.stamp) {
            log(format_args!(
                "dropping a definition result for an old revision"
            ));
            return vec![Outgoing::success(id, Value::Null)];
        }
        match done.outcome {
            Ok(Outcome::Definition(result)) => {
                if !result.status.is_supported()
                    && !matches!(result.status, CapabilityStatus::Partial { .. })
                {
                    // An unsupported check is not the same as "checked, no
                    // definition"; LSP has no status field, so use `null`.
                    log(format_args!(
                        "definition check is {}",
                        status_label(&result.status)
                    ));
                    return vec![Outgoing::success(id, Value::Null)];
                }
                let mapped = definition_value(self, &done.stamp, &result);
                if mapped.locations.is_empty()
                    && (!result.status.is_supported() || mapped.unmappable)
                {
                    return vec![Outgoing::success(id, Value::Null)];
                }
                vec![Outgoing::success(id, Value::Array(mapped.locations))]
            }
            Ok(_) => {
                log(format_args!(
                    "definition job returned the wrong result kind"
                ));
                vec![Outgoing::success(id, Value::Null)]
            }
            Err(error) => {
                log(format_args!(
                    "definition failed: {}",
                    failure_message(&error)
                ));
                vec![Outgoing::success(id, Value::Null)]
            }
        }
    }

    /// Marks every in-flight request for `uri` as cancelled.
    pub(crate) fn cancel_document_requests(&self, uri: &str) {
        for inflight in self.inflight.values() {
            if inflight.uri == uri {
                inflight.cancel.store(true, Ordering::SeqCst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wxml_text::SourceRange;
    use wxml_types::CapabilityStatus;

    use super::*;

    #[test]
    fn hover_notes_make_degradation_explicit() {
        let known = HoverResult {
            contents: "string".to_owned(),
            markup: MarkupKind::Markdown,
            range: None,
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Known,
        };
        assert_eq!(decorated_contents(&known), "string");

        let degraded = HoverResult {
            contents: String::new(),
            markup: MarkupKind::PlainText,
            range: None,
            status: CapabilityStatus::Partial {
                reason: "wxs\u{1b}[31m".to_owned(),
            },
            quality: TypeQuality::Any,
        };
        let text = decorated_contents(&degraded);
        assert!(text.contains("partial"), "{text}");
        assert!(text.contains("wxs [31m"), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
        assert!(text.contains("type quality: any"), "{text}");
        assert!(text.starts_with("[check status:"), "{text}");
        assert!(!text.contains('*'), "{text}");
    }

    #[test]
    fn markdown_status_notes_escape_links_and_images() {
        let degraded = HoverResult {
            contents: "string".to_owned(),
            markup: MarkupKind::Markdown,
            range: None,
            status: CapabilityStatus::Partial {
                reason: "x](command:y) ![i](https://x)".to_owned(),
            },
            quality: TypeQuality::Known,
        };
        let text = decorated_contents(&degraded);
        assert!(!text.contains("](command:"), "{text}");
        assert!(!text.contains("![i]("), "{text}");
        assert!(text.contains("partial"), "{text}");
    }

    #[test]
    fn backend_failures_do_not_expose_details() {
        let unavailable = BackendError::Unavailable("/secret/node not found".to_owned());
        assert!(!failure_message(&unavailable).contains("secret"));
        assert!(failure_message(&unavailable).contains("unavailable"));
        assert_eq!(
            failure_message(&BackendError::Timeout),
            "type checking timed out"
        );
    }

    #[test]
    fn range_values_are_optional() {
        let mut workspace = crate::workspace::Workspace::new();
        workspace
            .open("file:///a.wxml", "abc".to_owned(), Some(1))
            .expect("open");
        let document = workspace.get("file:///a.wxml").expect("document");
        let valid = SourceRange::from_bounds(0, 2).expect("range");
        assert_eq!(
            source_range_value(document, valid),
            Some(json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}}))
        );
        let invalid = SourceRange::from_bounds(0, 99).expect("range");
        assert_eq!(source_range_value(document, invalid), None);
    }
}
