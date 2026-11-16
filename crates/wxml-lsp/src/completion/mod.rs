//! LSP completion: tolerant context detection, the static catalog and the
//! Node-backed expression/event-handler route.
//!
//! Static tag/attribute/value completion never starts Node and is therefore
//! available even when the type backend is disabled. Only Mustache member
//! completion and event-handler completion are scheduled through the backend;
//! when it is unavailable they degrade to an empty list, never a protocol
//! error.

pub(crate) mod catalog;
pub(crate) mod context;
pub(crate) mod expression;
pub(crate) mod rules;
pub(crate) mod snippets;
pub(crate) mod static_items;
pub(crate) mod user_snippets;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use wxml_types::{
    BackendEpoch, CapabilityStatus, CompletionItem, CompletionItemKind, CompletionKind,
    CompletionResult, InsertTextFormat, MAX_COMPLETION_ITEMS,
};

use self::context::Context as Detected;
use self::expression::Route;
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::scheduler::{Job, JobKind, Outcome, SubmitError, WorkDone};
use crate::server::{Inflight, Server, log};
use crate::workspace::AnalysisStamp;

/// Detects a completion site in `text` at byte `offset`.
pub(crate) fn detect(text: &str, offset: u32) -> Option<Detected> {
    context::detect(text, offset)
}

/// Builds the static completion result for a detected context.
pub(crate) fn static_completion(
    server: &Server,
    context: &wxml_types::RequestContext,
    detected: &Detected,
) -> CompletionResult {
    // Tag and attribute-name sites need `usingComponents` (custom component
    // tags and their property names); value sites avoid a discovery lookup on
    // every keystroke.
    let wants_components = match &detected.site {
        context::Site::Tag { .. } => true,
        context::Site::AttributeName { tag, .. } => catalog::tag(tag).is_none(),
        context::Site::AttributeValue { .. } | context::Site::Mustache => false,
    };
    let file = context.source().file();
    let mut components = if wants_components {
        server.host.view_components_for(file)
    } else {
        Vec::new()
    };
    if let context::Site::AttributeName { tag, .. } = &detected.site {
        // Discovery may predate an unsaved component script edit; refresh the
        // one tag being completed from the effective script text.
        if let Some(component) = components.iter_mut().find(|entry| entry.name == *tag) {
            if let Some(facts) = server.host.component_properties_for(file, tag) {
                component.apply_live_facts(facts);
            }
        }
    }
    let text = context.source().text();
    let snippet = server.client.snippets.then(|| {
        (
            snippets::SnippetEnv::new(text, detected.replace.start()),
            server.snippets.catalog(),
        )
    });
    let snippet_context = snippet
        .as_ref()
        .map(|(env, catalog)| snippets::SnippetContext { env, catalog });
    let (items, truncated) =
        static_items::items_with_snippets(detected, &components, snippet_context.as_ref(), text);
    CompletionResult {
        items,
        status: CapabilityStatus::Supported,
        truncated,
    }
}

/// Maps a C0 item kind to the numeric LSP `CompletionItemKind`.
///
/// The enum is `#[non_exhaustive]`, so future variants default to `Text` (1)
/// rather than an arbitrary field kind.
#[allow(clippy::match_same_arms)] // Attribute and Property are both fields.
const fn lsp_kind(kind: CompletionItemKind) -> u8 {
    match kind {
        // Class is the closest LSP kind for a tag/component name.
        CompletionItemKind::Tag => 7,
        CompletionItemKind::Attribute => 10,
        CompletionItemKind::Value => 12,
        CompletionItemKind::Property => 10,
        CompletionItemKind::Method => 2,
        CompletionItemKind::Event => 23,
        CompletionItemKind::Keyword => 14,
        _ => 1,
    }
}

/// Maps one completion item to the LSP wire shape.
fn item_value(document: &crate::workspace::Document, item: &CompletionItem) -> Option<Value> {
    let range = crate::handlers::source_range_value(document, item.replace)?;
    let mut value = json!({
        "label": item.label,
        "kind": lsp_kind(item.kind),
        "insertText": item.insert_text,
        "insertTextFormat": match item.insert_text_format {
            InsertTextFormat::PlainText => 1,
            InsertTextFormat::Snippet => 2,
        },
        "textEdit": {
            "range": range,
            "newText": item.insert_text,
        },
    });
    let object = value.as_object_mut()?;
    if let Some(detail) = item.detail.as_ref() {
        object.insert("detail".to_owned(), Value::String(detail.clone()));
    }
    if let Some(documentation) = item.documentation.as_ref() {
        object.insert(
            "documentation".to_owned(),
            Value::String(documentation.clone()),
        );
    }
    if let Some(sort_text) = item.sort_text.as_ref() {
        object.insert("sortText".to_owned(), Value::String(sort_text.clone()));
    }
    if let Some(filter_text) = item.filter_text.as_ref() {
        object.insert("filterText".to_owned(), Value::String(filter_text.clone()));
    }
    Some(value)
}

/// Builds a `CompletionList` from a C0 result.
fn completion_list(document: &crate::workspace::Document, result: &CompletionResult) -> Value {
    let mut items = Vec::new();
    for item in result.items.iter().take(MAX_COMPLETION_ITEMS) {
        if let Some(value) = item_value(document, item) {
            items.push(value);
        }
    }
    json!({
        "isIncomplete": false,
        "items": items,
    })
}

/// Empty completion list for quiet degradation.
fn empty_list() -> Value {
    json!({
        "isIncomplete": false,
        "items": [],
    })
}

/// Short message for a full or stopped dispatch queue.
const fn submit_message(error: SubmitError) -> &'static str {
    match error {
        SubmitError::Busy => "server is busy",
        SubmitError::Stopped => "server is not running",
    }
}

impl Server {
    /// Handles `textDocument/completion`.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn completion(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some((uri, position)) = crate::handlers::hover_target(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument or position",
            )];
        };
        let epoch = self.refresh_epoch();
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, empty_list())];
        };
        // Completion is a template feature only; scripts and JSON files are
        // routed elsewhere and must not be treated as WXML text.
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, empty_list())];
        }
        let Some(offset) = document.offset_at(position) else {
            return vec![Outgoing::success(id, empty_list())];
        };
        let Some(context) = document.request_context(&epoch) else {
            log(format_args!(
                "completion context does not match the request"
            ));
            return vec![Outgoing::success(id, empty_list())];
        };
        let text = context.source().text();
        // Static template names, template-instance data keys and WXS module
        // members never start the type backend.
        {
            let access = crate::static_semantics::host_access(self);
            if let Some(result) = crate::static_semantics::completion(
                context.source().file().path(),
                text,
                offset,
                &access,
            ) {
                let value = crate::static_semantics::completion_values(document, &result);
                return vec![Outgoing::success(id, value)];
            }
        }
        let Some(detected) = detect(text, offset) else {
            return vec![Outgoing::success(id, empty_list())];
        };
        if let Some(value) = self.class_completion_value(uri, document, text, offset, &detected) {
            return vec![Outgoing::success(id, value)];
        }
        if let Some(value) = self.resource_completion_value(uri, document, text, offset, &detected)
        {
            return vec![Outgoing::success(id, value)];
        }

        match expression::route(&detected) {
            Route::Static => {
                let result = static_completion(self, &context, &detected);
                vec![Outgoing::success(id, completion_list(document, &result))]
            }
            Route::Backend(kind) => {
                if !self.capabilities.completion.is_supported() {
                    log(format_args!(
                        "completion backend is unavailable; returning an empty list"
                    ));
                    return vec![Outgoing::success(id, empty_list())];
                }
                let stamp = document.stamp(&epoch);
                self.schedule_completion(id, kind, uri, offset, stamp, context)
            }
        }
    }

    /// Queues one backend completion and records it as in flight.
    fn schedule_completion(
        &mut self,
        id: Value,
        kind: CompletionKind,
        uri: &str,
        offset: u32,
        stamp: AnalysisStamp,
        context: wxml_types::RequestContext,
    ) -> Vec<Outgoing> {
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
            kind: JobKind::Completion(kind),
            context,
            offset,
            stamp,
            cancel,
        };
        match self.dispatch.submit(job) {
            Ok(()) => Vec::new(),
            Err(error) => {
                self.inflight.remove(&key);
                log(format_args!(
                    "dispatch rejected a completion job: {error:?}"
                ));
                vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INTERNAL_ERROR,
                    submit_message(error),
                )]
            }
        }
    }

    /// Answers one finished completion job.
    pub(crate) fn completion_done(
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
                "dropping a completion result for an old revision"
            ));
            return vec![Outgoing::success(id, empty_list())];
        }
        match done.outcome {
            Ok(Outcome::Completion(result)) => {
                if !result.status.is_supported()
                    && !matches!(result.status, CapabilityStatus::Partial { .. })
                {
                    log(format_args!(
                        "completion is {}",
                        crate::handlers::status_label(&result.status)
                    ));
                    return vec![Outgoing::success(id, empty_list())];
                }
                let Some(document) = self.workspace.get_by_file(&done.stamp.file) else {
                    return vec![Outgoing::success(id, empty_list())];
                };
                vec![Outgoing::success(id, completion_list(document, &result))]
            }
            Ok(_) => {
                log(format_args!(
                    "completion job returned the wrong result kind"
                ));
                vec![Outgoing::success(id, empty_list())]
            }
            Err(error) => {
                log(format_args!("completion failed: {error}"));
                vec![Outgoing::success(id, empty_list())]
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn attribute_value_before_the_closing_quote_is_a_site() {
        let text = "<input type=\"\"\n<view bind:tap=\"\"";
        let detected = detect(text, 13).expect("input type value");
        assert!(matches!(
            detected.site,
            context::Site::AttributeValue { ref tag, ref attribute, quoted: true }
                if tag == "input" && attribute == "type"
        ));
        assert_eq!(detected.replace.start(), 13);
        assert_eq!(detected.prefix, "");
    }

    #[test]
    fn detects_and_classifies_each_site() {
        let text = "<view cla";
        let detected = detect(text, 9).expect("attribute");
        assert_eq!(expression::route(&detected), Route::Static);

        let text = "<view>{{ user.";
        let detected = detect(text, u32::try_from(text.len()).unwrap_or(0)).expect("mustache");
        assert_eq!(
            expression::route(&detected),
            Route::Backend(CompletionKind::ExpressionMember)
        );

        let text = "<view bindtap=\"on";
        let detected = detect(text, u32::try_from(text.len()).unwrap_or(0)).expect("event");
        assert_eq!(
            expression::route(&detected),
            Route::Backend(CompletionKind::EventHandler)
        );

        let text = "<view>text";
        assert!(detect(text, u32::try_from(text.len()).unwrap_or(0)).is_none());
    }
}
