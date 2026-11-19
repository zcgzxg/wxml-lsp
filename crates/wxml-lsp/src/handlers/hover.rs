//! Hover, definition and cancellation of point queries.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use wxml_text::SourceRange;
use wxml_types::{
    BackendError, CapabilityStatus, HoverResult, MarkupKind, SourceLocation, TypeQuality,
};

use super::{
    hover_target, line_index_range_value, markdown_escape, quality_label, sanitized_reason,
    source_range_value, status_label, template_document,
};
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::metadata::{self, DefinitionAnswer, HoverAnswer, StaticLocation};
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
///
/// Notes only appear when the backend produced no text. A successful hover
/// (signature, type or `JSDoc`) is more useful than a coverage disclaimer, and
/// appending one turned every answer into a wall of status text.
fn decorated_contents(result: &HoverResult) -> String {
    let mut text = result.contents.clone();
    if !text.trim().is_empty() {
        return text;
    }
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

/// True when the backend found no expression at the requested point.
///
/// Returning an empty markdown block here would make every ordinary WXML
/// position show a misleading "type quality: unknown" tooltip.
fn is_empty_hover(result: &HoverResult) -> bool {
    result.contents.is_empty()
        && result.range.is_none()
        && matches!(result.quality, TypeQuality::Unknown)
}

/// Maps a backend hover to the LSP result shape.
fn hover_value(server: &Server, stamp: &AnalysisStamp, result: &HoverResult) -> Option<Value> {
    if is_empty_hover(result) {
        return None;
    }
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

/// Returns the custom components registered for one template identity.
fn view_components(
    server: &Server,
    file: &wxml_types::FileId,
) -> Vec<crate::project_host::ViewComponent> {
    server.host.view_components_for(file)
}

/// Refreshes the one component named by `target` from the effective script.
fn refresh_component_properties(
    server: &Server,
    file: &wxml_types::FileId,
    target: &metadata::Target,
    components: &mut [crate::project_host::ViewComponent],
) {
    let tag = match target {
        metadata::Target::Tag { name, .. } => name,
        metadata::Target::Attribute { tag, .. } | metadata::Target::Value { tag, .. } => tag,
    };
    if metadata::is_builtin_tag(tag) {
        return;
    }
    if let Some(component) = components.iter_mut().find(|entry| entry.name == *tag) {
        if let Some(facts) = server.host.component_properties_for(file, tag) {
            component.apply_live_facts(facts);
        }
    }
}

/// Maps one project-relative static location through the host.
///
/// The target never leaves the project: project-bound `file_id_in` validates
/// the path, `uri_for` builds the client URI, and `file_text` supplies the
/// current overlay or disk text for the line index.
fn static_location_value(
    server: &Server,
    project: &wxml_types::ProjectId,
    location: &StaticLocation,
) -> Option<Value> {
    let file = server.host.file_id_in(project, &location.path).ok()?;
    let uri = server.host.uri_for(&file)?;
    let text = server.host.file_text(&file)?;
    let target_range = if location.range.is_empty() && location.range.start() == 0 {
        wxml_syntax::parse(&text)
            .tree
            .document()
            .elements()
            .next()
            .and_then(wxml_syntax::Element::name_range)
            .unwrap_or(location.range)
    } else {
        location.range
    };
    let line_index = wxml_text::LineIndex::new(&*text);
    let range = line_index_range_value(&line_index, SourceRange::new(target_range))?;
    Some(json!({"uri": uri, "range": range}))
}

/// Maps one exact script declaration through the host line index.
///
/// Event-handler declarations are name tokens, not element starts, so this
/// never widens an empty range. A missing URI, text or range stays `None` and
/// the caller answers with a checked empty result.
fn event_location_value(server: &Server, location: &SourceLocation) -> Option<Value> {
    let uri = server.host.uri_for(&location.file)?;
    let text = server.host.file_text(&location.file)?;
    let line_index = wxml_text::LineIndex::new(&*text);
    let range = line_index_range_value(&line_index, location.range)?;
    Some(json!({"uri": uri, "range": range}))
}

/// Short message for a full or stopped dispatch queue.
const fn submit_message(error: SubmitError) -> &'static str {
    match error {
        SubmitError::Busy => "server is busy",
        SubmitError::Stopped => "server is not running",
    }
}

impl Server {
    /// Answers a hover statically when the cursor is on a tag, attribute or
    /// documented value.
    ///
    /// Mustaches and event values return `None`, so the backend keeps its
    /// existing role for data fields and event handlers.
    fn static_hover(&self, id: &Value, params: Option<&Value>) -> Option<Vec<Outgoing>> {
        let params = params?;
        let (uri, position) = hover_target(params)?;
        if !template_document(self, uri) {
            return None;
        }
        let document = self.workspace.get(uri)?;
        let offset = document.offset_at(position)?;
        let context = document.request_context(&self.epoch)?;
        let target = metadata::target_at(context.source().text(), offset)?;
        let file = context.source().file();
        let mut components = if metadata::needs_components(&target) {
            view_components(self, file)
        } else {
            Vec::new()
        };
        refresh_component_properties(self, file, &target, &mut components);
        let HoverAnswer::Document { markdown, range } = metadata::hover(&components, &target)
        else {
            return None;
        };
        let range = source_range_value(document, range)?;
        Some(vec![Outgoing::success(
            id.clone(),
            json!({
                "contents": {"kind": "markdown", "value": markdown},
                "range": range,
            }),
        )])
    }

    /// Answers a component definition statically when one is provable.
    ///
    /// An unresolved component or undeclared property returns `null` instead
    /// of a fabricated target; tokens that are not component metadata return
    /// `None` and keep the backend definition path.
    fn static_definition(&self, id: &Value, params: Option<&Value>) -> Option<Vec<Outgoing>> {
        let params = params?;
        let (uri, position) = hover_target(params)?;
        if !template_document(self, uri) {
            return None;
        }
        let document = self.workspace.get(uri)?;
        let offset = document.offset_at(position)?;
        let context = document.request_context(&self.epoch)?;
        let target = metadata::target_at(context.source().text(), offset)?;
        let file = context.source().file();
        let needs_components = metadata::needs_components(&target);
        let mut components = if needs_components {
            view_components(self, file)
        } else {
            Vec::new()
        };
        if needs_components && matches!(target, metadata::Target::Attribute { .. }) {
            refresh_component_properties(self, file, &target, &mut components);
        }
        match metadata::definition(&components, &target) {
            DefinitionAnswer::NotStatic => None,
            DefinitionAnswer::None => Some(vec![Outgoing::success(id.clone(), Value::Null)]),
            DefinitionAnswer::Location(location) => {
                let value =
                    static_location_value(self, file.project(), &location).unwrap_or(Value::Null);
                if value.is_null() {
                    Some(vec![Outgoing::success(id.clone(), Value::Null)])
                } else {
                    Some(vec![Outgoing::success(
                        id.clone(),
                        Value::Array(vec![value]),
                    )])
                }
            }
        }
    }

    /// Handles `textDocument/hover`.
    pub(crate) fn hover(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        if let Some(outgoing) = self.static_hover(&id, params.as_ref()) {
            return outgoing;
        }
        if let Some(response) = self.local_hover_response(&id, params.as_ref()) {
            return response;
        }
        if !super::backend_available(&self.capabilities.hover) {
            log(format_args!(
                "hover backend is unavailable; returning a static-only empty result"
            ));
            return vec![Outgoing::success(id, Value::Null)];
        }
        self.schedule_point(id, JobKind::Hover, params)
    }

    /// Answers an event-handler definition from Rust script facts.
    ///
    /// This runs before the static component and backend definition paths and
    /// never schedules a native session. `None` means the cursor is not inside
    /// an event-binding value, so the caller keeps the existing routing; an
    /// event value that cannot name a static handler returns `null` instead of
    /// falling through to a backend that could fabricate a target.
    fn local_event_definition(&self, id: &Value, params: Option<&Value>) -> Option<Vec<Outgoing>> {
        // The Node backend already answers event definitions with its own
        // oracle; only fill the gap for a degraded/unsupported definition
        // capability so the default backend behavior stays unchanged.
        if self.capabilities.definition.is_supported() {
            return None;
        }
        let params = params?;
        let (uri, position) = hover_target(params)?;
        if !template_document(self, uri) {
            return None;
        }
        let document = self.workspace.get(uri)?;
        let offset = document.offset_at(position)?;
        let site = crate::semantics::event_definition::event_site(document.text(), offset)?;
        let handler = match site {
            crate::semantics::event_definition::EventValueSite::Handler { handler, .. } => handler,
            crate::semantics::event_definition::EventValueSite::NotStatic => {
                return Some(vec![Outgoing::success(id.clone(), Value::Null)]);
            }
        };
        // A synthetic or project-less document has no prepared script view;
        // keep the backend fallback instead of claiming a checked empty.
        let Ok(view) = self.host.analysis(document.snapshot()) else {
            return None;
        };
        let answer = crate::semantics::event_definition::resolve(&view, &handler);
        if let Some(location) = answer.location {
            let value = event_location_value(self, &location).unwrap_or(Value::Null);
            if value.is_null() {
                log(format_args!(
                    "event handler declaration could not be mapped to a client location"
                ));
                return Some(vec![Outgoing::success(id.clone(), Value::Null)]);
            }
            return Some(vec![Outgoing::success(
                id.clone(),
                Value::Array(vec![value]),
            )]);
        }
        log(format_args!(
            "event handler definition is {}: {handler}",
            status_label(&answer.status)
        ));
        Some(vec![Outgoing::success(id.clone(), Value::Null)])
    }

    /// Handles `textDocument/definition`.
    pub(crate) fn definition(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        if let Some(outgoing) = self.local_event_definition(&id, params.as_ref()) {
            return outgoing;
        }
        if let Some(outgoing) = self.static_definition(&id, params.as_ref()) {
            return outgoing;
        }
        if let Some(response) = self.local_definition_response(&id, params.as_ref()) {
            return response;
        }
        if !super::backend_available(&self.capabilities.definition) {
            log(format_args!(
                "definition backend is unavailable; returning a static-only empty result"
            ));
            return vec![Outgoing::success(id, Value::Null)];
        }
        self.schedule_point(id, JobKind::Definition, params)
    }

    /// Validates a point request and queues its backend job.
    #[allow(clippy::needless_pass_by_value)] // the enum is logged after being cloned into the job
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
        // Hover and definition are template features. Script and JSON buffers
        // are tracked for overlays, but a point query on them is not answered
        // by WXML analysis; `null` is the protocol-correct empty result.
        if !template_document(self, uri) {
            return vec![Outgoing::success(id, Value::Null)];
        }
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

        // Static template/WXS facts (declarations, imports, WXS exports) are
        // answered without the type backend. A point outside those constructs
        // falls through to the normal mustache type query.
        {
            let access = crate::static_semantics::host_access(self);
            let path = context.source().file().path();
            let text = context.source().text();
            match kind {
                JobKind::Hover => {
                    if let Some(result) =
                        crate::static_semantics::hover(path, text, offset, &access)
                    {
                        let value = crate::static_semantics::hover_value(document, &result);
                        return vec![Outgoing::success(id, value)];
                    }
                }
                JobKind::Definition => {
                    if let Some(positions) =
                        crate::static_semantics::definition(path, text, offset, &access)
                    {
                        let values =
                            crate::static_semantics::definition_value(self, document, &positions);
                        return vec![Outgoing::success(id, Value::Array(values))];
                    }
                }
                _ => {}
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let attempt = self.next_attempt();
        self.inflight.insert(
            key.clone(),
            Inflight {
                id: id.clone(),
                attempt,
                cancel: Arc::clone(&cancel),
                uri: uri.to_owned(),
            },
        );
        let job = Job {
            key: Some(key.clone()),
            context,
            offset,
            stamp,
            attempt,
            cancel,
            kind: kind.clone(),
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

        let documented = HoverResult {
            contents: "```ts\nonTap(): void\n```\n\n文档".to_owned(),
            status: CapabilityStatus::Partial {
                reason: "component behaviors are partially resolved".to_owned(),
            },
            quality: TypeQuality::Missing,
            ..known
        };
        assert_eq!(decorated_contents(&documented), documented.contents);

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
    fn empty_hover_without_a_range_is_not_rendered() {
        let empty = HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Unknown,
        };
        assert!(is_empty_hover(&empty));

        let with_text = HoverResult {
            contents: "unknown".to_owned(),
            ..empty.clone()
        };
        assert!(!is_empty_hover(&with_text));

        let with_range = HoverResult {
            range: Some(SourceRange::from_bounds(0, 1).expect("range")),
            ..empty
        };
        assert!(!is_empty_hover(&with_range));
    }

    #[test]
    fn markdown_status_notes_escape_links_and_images() {
        let degraded = HoverResult {
            contents: String::new(),
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
