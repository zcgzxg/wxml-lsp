//! LSP handlers for references, rename, code actions and the read-only
//! command bridge.
//!
//! Local targets (tag pairs, template names, loop locals, classes) are answered
//! from WXML facts without Node. Data fields, component properties and event
//! handlers additionally ask the type backend for exact script tokens; an
//! unavailable or incomplete script answer never becomes an edit.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use wxml_text::{LineIndex, Position, SourceRange};
use wxml_types::{BackendError, FileId, SymbolQuery, SymbolReferenceRole, SymbolResult};

use super::edit_plan::{EditPlan, EditRule, is_identifier};
use super::facts::{ViewFacts, view_facts};
use super::target::{
    SymbolToken, Target, TokenStyle, Tokens, camel_case, class_in_use, collect_class_refs,
    collect_data_refs, collect_event_refs, collect_loop_refs, collect_property_refs,
    collect_tag_refs, collect_template_refs, kebab_case, target_at,
};
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::scheduler::{Job, JobKind, Outcome, SubmitError, WorkDone};
use crate::server::{Inflight, Server, log};

/// Maximum locations returned by one references answer.
const MAX_REFERENCE_LOCATIONS: usize = 4096;
/// Maximum target files indexed for one references answer.
const MAX_REFERENCE_FILES: usize = 64;
/// Maximum estimated text/index bytes read for references.
const MAX_REFERENCE_INDEX_BYTES: usize = 64 * 1024 * 1024;
/// Maximum bounded edit distance considered by a typo quick fix.
const MAX_SUGGESTION_DISTANCE: usize = 2;

/// Why a script symbol query is pending.
#[derive(Clone, Debug)]
pub(crate) enum PendingIntent {
    /// `textDocument/prepareRename` for a script-backed target.
    PrepareRename { target: Target },
    /// `textDocument/references` or `wxml.references`.
    References {
        target: Target,
        include_declaration: bool,
        command: bool,
    },
    /// `textDocument/rename`.
    Rename { target: Target, new_name: String },
    /// A missing event handler quick fix.
    CodeActionHandler { handler: String },
    /// A data field typo quick fix.
    CodeActionTypo { typo: String, token: SourceRange },
}

/// One pending semantic request.
#[derive(Clone, Debug)]
pub(crate) struct PendingSemantic {
    /// Open document the request came from.
    pub(crate) uri: String,
    /// What to answer once the script facts arrive.
    pub(crate) intent: PendingIntent,
}

impl Server {
    /// Handles `textDocument/prepareRename`.
    pub(crate) fn prepare_rename(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
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
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, Value::Null)];
        }
        let Some(document) = self.workspace.get(uri).cloned() else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some(offset) = document.offset_at(position) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some(facts) = view_facts(self, &document) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some(target) = target_at(self, &facts, offset) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        if let Some((subject, query)) = node_query(&target) {
            return self.schedule_symbol_query(
                id,
                uri,
                &document,
                PendingIntent::PrepareRename {
                    target: target.clone(),
                },
                subject,
                query,
            );
        }
        prepare_result(id, &document, &facts, &target)
    }

    /// Handles `textDocument/references`.
    pub(crate) fn references(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        self.references_inner(id, &params, false)
    }

    /// Shared references path for `textDocument/references` and
    /// `workspace/executeCommand`.
    fn references_inner(&mut self, id: Value, params: &Value, command: bool) -> Vec<Outgoing> {
        let Some((uri, position)) = crate::handlers::hover_target(params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument or position",
            )];
        };
        let include_declaration = params
            .get("context")
            .and_then(|context| context.get("includeDeclaration"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, json!([]))];
        }
        let Some(document) = self.workspace.get(uri).cloned() else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(offset) = document.offset_at(position) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(facts) = view_facts(self, &document) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(target) = target_at(self, &facts, offset) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        if let Some((subject, query)) = node_query(&target) {
            return self.schedule_symbol_query(
                id,
                uri,
                &document,
                PendingIntent::References {
                    target,
                    include_declaration,
                    command,
                },
                subject,
                query,
            );
        }
        let tokens = local_reference_tokens(self, &facts, &target, include_declaration);
        let _ = command;
        let locations = locations_value(self, &tokens);
        vec![Outgoing::success(id, json!(locations))]
    }

    /// Handles `textDocument/rename`.
    pub(crate) fn rename(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
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
        let Some(new_name) = params.get("newName").and_then(Value::as_str) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing newName",
            )];
        };
        if new_name.is_empty() || new_name.len() > wxml_types::MAX_SYMBOL_NAME_BYTES {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid new name",
            )];
        }
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "rename is only available for WXML templates",
            )];
        }
        let Some(document) = self.workspace.get(uri).cloned() else {
            return vec![Outgoing::success(id, json!({"documentChanges": []}))];
        };
        let Some(offset) = document.offset_at(position) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid position",
            )];
        };
        let Some(facts) = view_facts(self, &document) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the template could not be parsed completely",
            )];
        };
        let Some(target) = target_at(self, &facts, offset) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "rename is not available for this token",
            )];
        };
        if let Some(reason) = prevalidate_name(&target, new_name) {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                reason,
            )];
        }
        if let Some(old) = target.placeholder_text(&facts.doc) {
            if old == new_name {
                return vec![Outgoing::success(id, json!({"documentChanges": []}))];
            }
        }
        if let Some((subject, query)) = node_query(&target) {
            return self.schedule_symbol_query(
                id,
                uri,
                &document,
                PendingIntent::Rename {
                    target,
                    new_name: new_name.to_owned(),
                },
                subject,
                query,
            );
        }
        match local_rename_plan(self, &facts, &target, new_name) {
            Ok(value) => vec![Outgoing::success(id, value)],
            Err(error) => vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                error,
            )],
        }
    }

    /// Schedules one script symbol query for an async semantic handler.
    fn schedule_symbol_query(
        &mut self,
        id: Value,
        uri: &str,
        document: &crate::workspace::Document,
        intent: PendingIntent,
        subject: FileId,
        query: SymbolQuery,
    ) -> Vec<Outgoing> {
        let epoch = self.refresh_epoch();
        let Some(context) = document.request_context(&epoch) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the document changed before the query was scheduled",
            )];
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
        self.semantic_pending.insert(
            key.clone(),
            PendingSemantic {
                uri: uri.to_owned(),
                intent,
            },
        );
        let job = Job {
            key: Some(key.clone()),
            kind: JobKind::Symbols { subject, query },
            context,
            offset: 0,
            stamp,
            cancel,
        };
        match self.dispatch.submit(job) {
            Ok(()) => Vec::new(),
            Err(error) => {
                self.inflight.remove(&key);
                self.semantic_pending.remove(&key);
                log(format_args!("dispatch rejected a symbols job: {error:?}"));
                vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INTERNAL_ERROR,
                    submit_message(error),
                )]
            }
        }
    }

    /// Answers one finished script symbol query.
    pub(crate) fn symbols_done(
        &mut self,
        id: Value,
        done: WorkDone,
        epoch: &wxml_types::BackendEpoch,
    ) -> Vec<Outgoing> {
        let Some(key) = done.key.clone() else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INTERNAL_ERROR,
                "script symbol query has no id",
            )];
        };
        let pending = self.semantic_pending.remove(&key);
        let Some(pending) = pending else {
            log(format_args!(
                "dropping a symbol result without a pending intent"
            ));
            return Vec::new();
        };
        if done.cancelled {
            return cancelled(&pending, id);
        }
        if done.stamp.epoch != *epoch {
            return stale(&pending, id);
        }
        let Some(document) = self.workspace.get(&pending.uri) else {
            return stale(&pending, id);
        };
        if document.stamp(epoch) != done.stamp {
            return stale(&pending, id);
        }
        let Some(facts) = view_facts(self, document) else {
            return stale(&pending, id);
        };
        let result = match done.outcome {
            Ok(Outcome::Symbols(result)) => *result,
            Ok(_) => {
                log(format_args!("symbol job returned the wrong result kind"));
                return failed(
                    &pending,
                    id,
                    "the script query returned an unexpected result",
                );
            }
            Err(error) => {
                log(format_args!("script symbol query failed: {error}"));
                return failed(&pending, id, failure_message(&error));
            }
        };
        match pending.intent.clone() {
            PendingIntent::PrepareRename { target } => {
                if result.complete && result.declaration.is_some() {
                    prepare_result(id, document, &facts, &target)
                } else {
                    vec![Outgoing::success(id, Value::Null)]
                }
            }
            PendingIntent::References {
                target,
                include_declaration,
                command,
            } => {
                if !result.complete {
                    log(format_args!(
                        "references are partial: {}",
                        crate::handlers::log_safe_text(
                            result.reason.as_deref().unwrap_or("coverage is incomplete"),
                            200
                        )
                    ));
                }
                let tokens =
                    node_reference_tokens(self, &facts, &target, &result, include_declaration);
                let _ = command;
                let locations = locations_value(self, &tokens);
                vec![Outgoing::success(id, json!(locations))]
            }
            PendingIntent::Rename { target, new_name } => {
                match node_rename_plan(self, &facts, &target, &new_name, &result) {
                    Ok(value) => vec![Outgoing::success(id, value)],
                    Err(error) => vec![Outgoing::failure(
                        id,
                        jsonrpc::error_codes::REQUEST_FAILED,
                        error,
                    )],
                }
            }
            PendingIntent::CodeActionHandler { handler } => {
                match handler_code_action(self, &facts, &handler, &result) {
                    Some(edit) => vec![Outgoing::success(id, json!([edit]))],
                    None => vec![Outgoing::success(id, json!([]))],
                }
            }
            PendingIntent::CodeActionTypo { typo, token } => {
                match typo_code_action(self, &facts, &typo, token, &result) {
                    Some(edit) => vec![Outgoing::success(id, json!([edit]))],
                    None => vec![Outgoing::success(id, json!([]))],
                }
            }
        }
    }

    /// Handles `textDocument/codeAction`.
    #[allow(clippy::too_many_lines)] // one selection dispatches two bounded quick fixes
    pub(crate) fn code_action(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some((uri, range)) = crate::handlers::range_target(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument or range",
            )];
        };
        if let Some(only) = params
            .get("context")
            .and_then(|context| context.get("only"))
            .and_then(Value::as_array)
            && !only
                .iter()
                .filter_map(Value::as_str)
                .any(|kind| kind == "quickfix" || kind == "source")
        {
            return vec![Outgoing::success(id, json!([]))];
        }
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, json!([]))];
        }
        let Some(document) = self.workspace.get(uri).cloned() else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let (Some(start), Some(end)) = (document.offset_at(range.0), document.offset_at(range.1))
        else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(facts) = view_facts(self, &document) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        // 1. A static event handler token: ask Node whether the method exists.
        if let Some(event) = facts.analysis.events.iter().find(|event| {
            overlaps(
                event.handler_range.start(),
                event.handler_range.end(),
                start,
                end,
            )
        }) {
            let Some(subject) = facts
                .target
                .as_ref()
                .map(|target| target.script_file.clone())
            else {
                return vec![Outgoing::success(id, json!([]))];
            };
            return self.schedule_symbol_query(
                id,
                uri,
                &document,
                PendingIntent::CodeActionHandler {
                    handler: event.handler.clone(),
                },
                subject,
                SymbolQuery::EventHandler {
                    name: event.handler.clone(),
                },
            );
        }
        // 2. A `Property 'x' does not exist` diagnostic over a data path.
        let Some(diagnostic) = code_action_diagnostic(&params, range) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(typo) = diagnostic_typo(&diagnostic) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(diagnostic_range) = code_action_diagnostic_range(&diagnostic) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let (Some(start), Some(_end)) = (
            document.offset_at(diagnostic_range.0),
            document.offset_at(diagnostic_range.1),
        ) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(Target::Data {
            subject,
            root: _,
            path,
            token,
        }) = target_at(self, &facts, start)
        else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let Some(token_text) = facts.doc.text().get(
            usize::try_from(token.start()).unwrap_or(usize::MAX)
                ..usize::try_from(token.end()).unwrap_or(usize::MAX),
        ) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        if token_text != typo || path.last().is_none_or(|last| last != &typo) {
            return vec![Outgoing::success(id, json!([]))];
        }
        let parent = path[..path.len().saturating_sub(1)].to_vec();
        let query = if parent.is_empty() {
            SymbolQuery::DataRoots
        } else {
            SymbolQuery::DataPath { path: parent }
        };
        self.schedule_symbol_query(
            id,
            uri,
            &document,
            PendingIntent::CodeActionTypo { typo, token },
            subject,
            query,
        )
    }
}

/// Builds the synchronous prepareRename response for a proven local target.
fn prepare_result(
    id: Value,
    document: &crate::workspace::Document,
    facts: &ViewFacts,
    target: &Target,
) -> Vec<Outgoing> {
    let Some(range) = crate::handlers::source_range_value(document, target.placeholder()) else {
        return vec![Outgoing::success(id, Value::Null)];
    };
    let Some(placeholder) = target.placeholder_text(&facts.doc) else {
        return vec![Outgoing::success(id, Value::Null)];
    };
    vec![Outgoing::success(
        id,
        json!({"range": range, "placeholder": placeholder}),
    )]
}

/// Builds the Node query for a script-backed WXML target.
fn node_query(target: &Target) -> Option<(FileId, SymbolQuery)> {
    match target {
        Target::Data { subject, path, .. } => Some((
            subject.clone(),
            SymbolQuery::DataPath { path: path.clone() },
        )),
        Target::Property {
            subject, declared, ..
        } => Some((
            subject.clone(),
            SymbolQuery::DataPath {
                path: vec![declared.clone()],
            },
        )),
        Target::Event { subject, name, .. } => Some((
            subject.clone(),
            SymbolQuery::EventHandler { name: name.clone() },
        )),
        _ => None,
    }
}

/// Returns a client-safe failure for a cancelled pending request.
fn cancelled(pending: &PendingSemantic, id: Value) -> Vec<Outgoing> {
    if matches!(pending.intent, PendingIntent::PrepareRename { .. }) {
        return vec![Outgoing::success(id, Value::Null)];
    }
    vec![Outgoing::failure(
        id,
        jsonrpc::error_codes::REQUEST_CANCELLED,
        "request cancelled",
    )]
}

/// Returns a client-safe failure for a stale pending request.
fn stale(pending: &PendingSemantic, id: Value) -> Vec<Outgoing> {
    match pending.intent {
        PendingIntent::PrepareRename { .. } => vec![Outgoing::success(id, Value::Null)],
        PendingIntent::References { .. } => {
            vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the document changed before references could be resolved",
            )]
        }
        PendingIntent::CodeActionHandler { .. } | PendingIntent::CodeActionTypo { .. } => {
            vec![Outgoing::success(id, json!([]))]
        }
        PendingIntent::Rename { .. } => vec![Outgoing::failure(
            id,
            jsonrpc::error_codes::CONTENT_MODIFIED,
            "the document changed before rename could be completed",
        )],
    }
}

fn failed(pending: &PendingSemantic, id: Value, message: &str) -> Vec<Outgoing> {
    match pending.intent {
        PendingIntent::PrepareRename { .. } => vec![Outgoing::success(id, Value::Null)],
        PendingIntent::References { .. } => vec![Outgoing::success(id, json!([]))],
        PendingIntent::CodeActionHandler { .. } | PendingIntent::CodeActionTypo { .. } => {
            vec![Outgoing::success(id, json!([]))]
        }
        PendingIntent::Rename { .. } => {
            vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                message,
            )]
        }
    }
}

/// Short client-safe failure message for a backend error.
const fn failure_message(error: &BackendError) -> &'static str {
    match error {
        BackendError::Unavailable(_) => "the script symbol backend is unavailable",
        BackendError::Protocol(_) => "the script symbol backend returned an invalid response",
        BackendError::Timeout => "the script symbol query timed out",
        BackendError::Cancelled => "the script symbol query was cancelled",
        _ => "the script symbol query failed",
    }
}

/// Builds the placeholder/name validation for a target.
fn prevalidate_name(target: &Target, new_name: &str) -> Option<&'static str> {
    match target {
        Target::Data { .. } | Target::Event { .. } | Target::Loop { .. } => {
            (!is_identifier(new_name)).then_some("the new name is not a valid identifier")
        }
        Target::Property { .. } => {
            let declared: Option<String> = if new_name.contains('-') {
                camel_case(new_name)
            } else {
                Some(new_name.to_owned())
            };
            match declared {
                Some(declared) if is_identifier(&declared) => None,
                _ => Some("the new property name is not a valid camelCase or kebab-case name"),
            }
        }
        Target::Tag { .. } => {
            let mut chars = new_name.chars();
            let valid = chars.next().is_some_and(|first| first.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !crate::metadata::is_builtin_tag(new_name);
            (!valid).then_some("the new tag name is not a valid custom-component tag")
        }
        Target::Template { .. } => {
            let valid = super::target::is_template_name_public(new_name);
            (!valid).then_some("the new template name is not valid")
        }
        Target::Class { .. } => {
            let valid = super::target::is_class_name(new_name);
            (!valid).then_some("the new class name is not valid")
        }
    }
}

/// Submits the error message for a full dispatch queue.
const fn submit_message(error: SubmitError) -> &'static str {
    match error {
        SubmitError::Busy => "server is busy",
        SubmitError::Stopped => "server is not running",
    }
}

/// Returns `true` when two byte ranges overlap (inclusive on the selection).
const fn overlaps(start: u32, end: u32, range_start: u32, range_end: u32) -> bool {
    start <= range_end && range_start <= end
}

/* ------------------------------------------------------------- locations */

/// Reads the current effective text of one target file.
fn file_text(server: &Server, file: &FileId) -> Option<Arc<str>> {
    if let Some(document) = server.workspace.get_by_file(file) {
        return Some(Arc::from(document.text()));
    }
    server.host.file_text(file)
}

/// Maps exact tokens to LSP locations with per-file budgets.
fn locations_value(server: &Server, tokens: &Tokens) -> Vec<Value> {
    let mut grouped: BTreeMap<FileId, Vec<SourceRange>> = BTreeMap::new();
    for token in &tokens.items {
        let entry = grouped.entry(token.file.clone()).or_default();
        if !entry.contains(&token.range) {
            entry.push(token.range);
        }
    }
    let mut locations = Vec::new();
    let mut bytes = 0_usize;
    let mut exhausted = false;
    for (file_index, (file, ranges)) in grouped.into_iter().enumerate() {
        if file_index >= MAX_REFERENCE_FILES || exhausted {
            break;
        }
        let target = if let Some(document) = server.workspace.get_by_file(&file) {
            LocationTarget {
                uri: document.uri().to_owned(),
                line_index: document.snapshot().line_index().clone(),
            }
        } else {
            let Some(uri) = server.host.uri_for(&file) else {
                continue;
            };
            let Some(text) = server.host.file_text(&file) else {
                continue;
            };
            let cost = text
                .len()
                .saturating_add(8_usize.saturating_mul(text.match_indices('\n').count()));
            if bytes.saturating_add(cost) > MAX_REFERENCE_INDEX_BYTES {
                break;
            }
            bytes = bytes.saturating_add(cost);
            LocationTarget {
                uri,
                line_index: LineIndex::new(text),
            }
        };
        for range in ranges {
            if locations.len() >= MAX_REFERENCE_LOCATIONS {
                exhausted = true;
                break;
            }
            let Some(value) = crate::handlers::line_index_range_value(&target.line_index, range)
            else {
                continue;
            };
            locations.push(json!({"uri": target.uri, "range": value}));
        }
    }
    locations.sort_by(|left, right| {
        let left_uri = left.get("uri").and_then(Value::as_str).unwrap_or_default();
        let right_uri = right.get("uri").and_then(Value::as_str).unwrap_or_default();
        left_uri.cmp(right_uri).then_with(|| {
            let left_start = left["range"]["start"]["line"].as_u64().unwrap_or_default();
            let right_start = right["range"]["start"]["line"].as_u64().unwrap_or_default();
            left_start.cmp(&right_start)
        })
    });
    locations
}

struct LocationTarget {
    uri: String,
    line_index: LineIndex,
}

/* --------------------------------------------------------- token helpers */

fn push_script_token(tokens: &mut Tokens, token: &wxml_types::SymbolToken, style: TokenStyle) {
    tokens.push(&token.file, token.range, style);
}

/// WXML + script tokens for one target.
fn node_reference_tokens(
    server: &Server,
    facts: &ViewFacts,
    target: &Target,
    result: &SymbolResult,
    include_declaration: bool,
) -> Tokens {
    if result.declaration.is_none() && result.references.is_empty() && result.occurrences.is_empty()
    {
        // A symbol with no script declaration is not proven; WXML tokens with
        // the same spelling must not be presented as its references.
        return Tokens::new();
    }
    let mut tokens = match target {
        Target::Data { path, .. } => collect_data_refs(facts, path),
        Target::Event { name, .. } => collect_event_refs(facts, name),
        Target::Property {
            declared,
            component,
            ..
        } => {
            let collected = collect_property_refs(server, facts, declared, component);
            let mut tokens = Tokens {
                items: collected.items,
                complete: collected.complete,
                reason: collected.reason,
            };
            if !collected.complete {
                tokens.complete = false;
            }
            tokens
        }
        _ => Tokens::new(),
    };
    if include_declaration {
        if let Some(token) = result.declaration.as_ref() {
            push_script_token(&mut tokens, token, TokenStyle::Identifier);
        }
    }
    for reference in &result.references {
        if reference.role == SymbolReferenceRole::Declaration && !include_declaration {
            continue;
        }
        push_script_token(&mut tokens, &reference.token, TokenStyle::Identifier);
    }
    for occurrence in &result.occurrences {
        push_script_token(&mut tokens, &occurrence.token, TokenStyle::PathSegment);
    }
    tokens
}

/// Local tokens for a target that does not need script facts.
fn local_reference_tokens(
    server: &Server,
    facts: &ViewFacts,
    target: &Target,
    include_declaration: bool,
) -> Tokens {
    match target {
        Target::Loop { identity, name, .. } => {
            let collected = collect_loop_refs(facts, identity, name);
            let mut tokens = Tokens::new();
            if include_declaration {
                if let Some(declaration) = collected.declaration {
                    tokens.push(&declaration.file, declaration.range, TokenStyle::Identifier);
                }
            }
            for reference in collected.references {
                tokens.push(&reference.file, reference.range, TokenStyle::Identifier);
            }
            tokens
        }
        Target::Tag { start, end, .. } => collect_tag_refs(facts, *start, *end),
        Target::Template { name, .. } => {
            let collected = collect_template_refs(server, facts, name);
            let mut tokens = Tokens::new();
            if include_declaration {
                if let Some(definition) = collected.definition {
                    tokens.push(&definition.file, definition.range, TokenStyle::Template);
                }
            }
            for call in collected.calls {
                tokens.push(&call.file, call.range, TokenStyle::Template);
            }
            tokens
        }
        Target::Class { name, .. } => {
            let collected = collect_class_refs(server, facts, name);
            let mut tokens = Tokens::new();
            if include_declaration {
                for definition in collected.definitions {
                    tokens.push(&definition.file, definition.range, TokenStyle::Class);
                }
            }
            for usage in collected.usages {
                tokens.push(&usage.file, usage.range, TokenStyle::Class);
            }
            tokens
        }
        _ => Tokens::new(),
    }
}

/* -------------------------------------------------------------- rename */

/// Adds one exact token replacement to a plan.
fn add_token_edit(
    plan: &mut EditPlan,
    server: &Server,
    token: &SymbolToken,
    replacement: &str,
    rule: EditRule,
) -> Result<(), String> {
    let text = file_text(server, &token.file)
        .ok_or_else(|| "the edit target is unavailable".to_owned())?;
    let start = usize::try_from(token.range.start())
        .map_err(|_| "the edit target has an invalid range".to_owned())?;
    let end = usize::try_from(token.range.end())
        .map_err(|_| "the edit target has an invalid range".to_owned())?;
    let expected = text
        .get(start..end)
        .ok_or_else(|| "the edit target has an invalid range".to_owned())?;
    plan.add(&token.file, token.range, expected, replacement, rule)
        .map_err(|error| error.message().to_owned())
}

fn plan_value(plan: EditPlan, server: &Server) -> Result<Value, String> {
    plan.document_changes(server)
        .map_err(|error| error.message().to_owned())
}

/// Builds a local rename plan (tag/template/loop/class).
#[allow(clippy::too_many_lines)] // one plan covers the static target variants
fn local_rename_plan(
    server: &Server,
    facts: &ViewFacts,
    target: &Target,
    new_name: &str,
) -> Result<Value, String> {
    let mut plan = EditPlan::new(facts.doc.file.clone());
    match target {
        Target::Tag { name, start, end } => {
            if name == new_name {
                return Ok(json!({"documentChanges": []}));
            }
            let mut conflict = false;
            super::target::for_each_element(facts.doc.tree(), &mut |element| {
                if element.name() == Some(new_name) {
                    conflict = true;
                }
            });
            if conflict {
                return Err("the new tag name is already used in this template".to_owned());
            }
            add_token_edit(
                &mut plan,
                server,
                &SymbolToken {
                    file: facts.doc.file.clone(),
                    range: *start,
                    style: TokenStyle::Tag,
                },
                new_name,
                EditRule::Tag,
            )?;
            add_token_edit(
                &mut plan,
                server,
                &SymbolToken {
                    file: facts.doc.file.clone(),
                    range: *end,
                    style: TokenStyle::Tag,
                },
                new_name,
                EditRule::Tag,
            )?;
        }
        Target::Template { name, .. } => {
            let current = collect_template_refs(server, facts, name);
            if !current.complete {
                return Err(current
                    .reason
                    .unwrap_or("template coverage is incomplete")
                    .to_owned());
            }
            let Some(definition) = current.definition else {
                return Err("the template has no reachable definition".to_owned());
            };
            match super::target::template_name_in_use(server, facts, new_name) {
                Some(false) => {}
                Some(true) => {
                    return Err("the new template name is already in use".to_owned());
                }
                None => {
                    return Err(
                        "the new template name could not be proven conflict-free".to_owned()
                    );
                }
            }
            add_token_edit(&mut plan, server, &definition, new_name, EditRule::Template)?;
            for call in current.calls {
                add_token_edit(&mut plan, server, &call, new_name, EditRule::Template)?;
            }
        }
        Target::Loop {
            name,
            identity,
            loop_index,
            ..
        } => {
            let Some(loop_info) = facts.loops.get(*loop_index) else {
                return Err("the loop scope is unavailable".to_owned());
            };
            for (index, other) in facts.loops.iter().enumerate() {
                let is_target = index == *loop_index;
                for binding in [&other.item, &other.index] {
                    if binding.name == new_name && !(is_target && binding.name == *name) {
                        return Err("the new name conflicts with another loop binding".to_owned());
                    }
                }
            }
            let collected = collect_loop_refs(facts, identity, name);
            if !collected.complete {
                return Err(collected
                    .reason
                    .unwrap_or("loop references are incomplete")
                    .to_owned());
            }
            if let Some(declaration) = collected.declaration {
                add_token_edit(
                    &mut plan,
                    server,
                    &declaration,
                    new_name,
                    EditRule::Identifier,
                )?;
            } else {
                let attribute = match identity.role {
                    wxml_types::LocalRole::Item => "wx:for-item",
                    wxml_types::LocalRole::Index => "wx:for-index",
                    _ => return Err("the loop binding role is unknown".to_owned()),
                };
                let insertion = format!(" {attribute}=\"{new_name}\"");
                let range =
                    SourceRange::from_bounds(loop_info.tag_name_end, loop_info.tag_name_end)
                        .map_err(|_| "the loop tag anchor is invalid".to_owned())?;
                plan.add(
                    &facts.doc.file,
                    range,
                    "",
                    insertion,
                    EditRule::Preformatted,
                )
                .map_err(|error| error.message().to_owned())?;
            }
            for reference in collected.references {
                add_token_edit(
                    &mut plan,
                    server,
                    &reference,
                    new_name,
                    EditRule::Identifier,
                )?;
            }
        }
        Target::Class { name, .. } => {
            let collected = collect_class_refs(server, facts, name);
            if !collected.complete || !collected.rename_safe {
                return Err(collected
                    .reason
                    .unwrap_or("the style class cannot be renamed completely")
                    .to_owned());
            }
            if let Some(style) = server.host.style_facts_for(&facts.doc.file) {
                if !style.definitions(new_name).is_empty() {
                    return Err("the new class name already has a definition".to_owned());
                }
            }
            if class_in_use(facts, new_name) {
                return Err("the new class name is already used in this template".to_owned());
            }
            for definition in collected.definitions {
                add_token_edit(
                    &mut plan,
                    server,
                    &SymbolToken {
                        file: definition.file,
                        range: definition.range,
                        style: TokenStyle::Class,
                    },
                    new_name,
                    EditRule::Class,
                )?;
            }
            for usage in collected.usages {
                add_token_edit(&mut plan, server, &usage, new_name, EditRule::Class)?;
            }
        }
        Target::Data { .. } | Target::Property { .. } | Target::Event { .. } => {
            return Err("the target requires a script symbol query".to_owned());
        }
    }
    plan_value(plan, server)
}

/// Builds a rename plan from script facts plus WXML occurrences.
#[allow(clippy::too_many_lines)] // one plan covers data, event and component targets
fn node_rename_plan(
    server: &Server,
    facts: &ViewFacts,
    target: &Target,
    new_name: &str,
    result: &SymbolResult,
) -> Result<Value, String> {
    if !result.complete {
        return Err(result
            .reason
            .as_deref()
            .map_or("script symbol coverage is incomplete", str::trim)
            .to_owned());
    }
    let Some(declaration) = result.declaration.as_ref() else {
        return Err("the symbol has no provable declaration".to_owned());
    };
    let (Target::Data { subject, .. }
    | Target::Property { subject, .. }
    | Target::Event { subject, .. }) = target
    else {
        return Err("the target does not accept a script rename".to_owned());
    };
    for reference in &result.references {
        if reference.token.file != *subject && reference.token.file != declaration.file {
            return Err(
                "the symbol is referenced from another script; cross-view edits are not supported"
                    .to_owned(),
            );
        }
    }
    let mut plan = EditPlan::new(facts.doc.file.clone());
    match target {
        Target::Data { path, .. } => {
            let old = path.last().cloned().unwrap_or_default();
            if new_name != old && result.fields.iter().any(|field| field.name == new_name) {
                return Err("the new field name already exists".to_owned());
            }
            let wxml = collect_data_refs(facts, path);
            if !wxml.complete {
                return Err(wxml
                    .reason
                    .unwrap_or("WXML data references are incomplete")
                    .to_owned());
            }
            add_token_edit(
                &mut plan,
                server,
                &script_token(declaration),
                new_name,
                EditRule::Identifier,
            )?;
            for reference in &result.references {
                add_token_edit(
                    &mut plan,
                    server,
                    &script_token(&reference.token),
                    new_name,
                    EditRule::Identifier,
                )?;
            }
            for occurrence in &result.occurrences {
                add_token_edit(
                    &mut plan,
                    server,
                    &occurrence_token(occurrence),
                    new_name,
                    EditRule::PathSegment,
                )?;
            }
            for token in wxml.items {
                add_token_edit(&mut plan, server, &token, new_name, EditRule::Identifier)?;
            }
        }
        Target::Event { name, .. } => {
            if new_name != *name && result.fields.iter().any(|field| field.name == new_name) {
                return Err("the new handler name already exists".to_owned());
            }
            let wxml = collect_event_refs(facts, name);
            if !wxml.complete {
                return Err(wxml
                    .reason
                    .unwrap_or("WXML event references are incomplete")
                    .to_owned());
            }
            add_token_edit(
                &mut plan,
                server,
                &script_token(declaration),
                new_name,
                EditRule::Identifier,
            )?;
            for reference in &result.references {
                add_token_edit(
                    &mut plan,
                    server,
                    &script_token(&reference.token),
                    new_name,
                    EditRule::Identifier,
                )?;
            }
            for token in wxml.items {
                add_token_edit(&mut plan, server, &token, new_name, EditRule::Identifier)?;
            }
        }
        Target::Property {
            declared,
            component,
            ..
        } => {
            let declared_new = if new_name.contains('-') {
                camel_case(new_name).ok_or_else(|| "the new property name is invalid".to_owned())?
            } else {
                new_name.to_owned()
            };
            if declared_new != *declared
                && result.fields.iter().any(|field| field.name == declared_new)
            {
                return Err("the new property name already exists".to_owned());
            }
            match super::target::property_name_conflict(server, facts, component, &declared_new) {
                Some(false) => {}
                Some(true) => {
                    return Err(
                        "the new property name is already used by this component".to_owned()
                    );
                }
                None => {
                    return Err(
                        "the new property name could not be proven conflict-free".to_owned()
                    );
                }
            }
            let wxml = collect_property_refs(server, facts, declared, component);
            if !wxml.complete {
                return Err(wxml
                    .reason
                    .unwrap_or("component property references are incomplete")
                    .to_owned());
            }
            add_token_edit(
                &mut plan,
                server,
                &script_token(declaration),
                &declared_new,
                EditRule::Identifier,
            )?;
            for reference in &result.references {
                add_token_edit(
                    &mut plan,
                    server,
                    &script_token(&reference.token),
                    &declared_new,
                    EditRule::Identifier,
                )?;
            }
            for occurrence in &result.occurrences {
                add_token_edit(
                    &mut plan,
                    server,
                    &occurrence_token(occurrence),
                    &declared_new,
                    EditRule::PathSegment,
                )?;
            }
            for token in wxml.items {
                let Some(text) = token_text(server, &token) else {
                    return Err("a component attribute target is unavailable".to_owned());
                };
                let replacement = if text.contains('-') {
                    kebab_case(&declared_new)
                } else if text == *declared {
                    declared_new.clone()
                } else {
                    return Err("a component attribute spelling is not recognized".to_owned());
                };
                add_token_edit(
                    &mut plan,
                    server,
                    &token,
                    &replacement,
                    EditRule::AttributeName,
                )?;
            }
        }
        _ => return Err("the target does not accept a script rename".to_owned()),
    }
    plan_value(plan, server)
}

fn script_token(token: &wxml_types::SymbolToken) -> SymbolToken {
    SymbolToken {
        file: token.file.clone(),
        range: token.range,
        style: TokenStyle::Identifier,
    }
}

fn occurrence_token(occurrence: &wxml_types::SymbolOccurrence) -> SymbolToken {
    SymbolToken {
        file: occurrence.token.file.clone(),
        range: occurrence.token.range,
        style: TokenStyle::PathSegment,
    }
}

fn token_text(server: &Server, token: &SymbolToken) -> Option<String> {
    let text = file_text(server, &token.file)?;
    let start = usize::try_from(token.range.start()).ok()?;
    let end = usize::try_from(token.range.end()).ok()?;
    text.get(start..end).map(str::to_owned)
}

/* ---------------------------------------------------------- code actions */

/// Returns the diagnostics selected by a code-action range.
fn code_action_diagnostic(params: &Value, range: (Position, Position)) -> Option<Value> {
    let diagnostics = params.get("context")?.get("diagnostics")?.as_array()?;
    let allowed = params
        .get("context")
        .and_then(|context| context.get("only"))
        .and_then(Value::as_array);
    if let Some(allowed) = allowed {
        if !allowed
            .iter()
            .filter_map(Value::as_str)
            .any(|kind| kind == "quickfix" || kind == "source")
        {
            return None;
        }
    }
    diagnostics.iter().find_map(|diagnostic| {
        let diagnostic_range = code_action_diagnostic_range(diagnostic)?;
        if !position_overlaps(diagnostic_range, range) {
            return None;
        }
        let message = diagnostic.get("message")?.as_str()?;
        diagnostic_typo_message(message)?;
        Some(diagnostic.clone())
    })
}

fn position_overlaps(left: (Position, Position), right: (Position, Position)) -> bool {
    left.0 <= right.1 && right.0 <= left.1
}

fn code_action_diagnostic_range(diagnostic: &Value) -> Option<(Position, Position)> {
    let range = diagnostic.get("range")?.as_object()?;
    let start = crate::handlers::position(range.get("start")?)?;
    let end = crate::handlers::position(range.get("end")?)?;
    Some((start, end))
}

/// Extracts `x` from a `Property 'x' does not exist` diagnostic.
fn diagnostic_typo(diagnostic: &Value) -> Option<String> {
    let message = diagnostic.get("message")?.as_str()?;
    diagnostic_typo_message(message)
}

fn diagnostic_typo_message(message: &str) -> Option<String> {
    let start = message.find("Property '")? + "Property '".len();
    let rest = message.get(start..)?;
    let end = rest.find('\'')?;
    let typo = rest.get(..end)?;
    if typo.is_empty() || !is_identifier(typo) {
        return None;
    }
    message
        .get(start + end..)?
        .starts_with("' does not exist")
        .then(|| typo.to_owned())
}

/// Offers a quick fix that creates a missing event handler.
fn handler_code_action(
    server: &Server,
    facts: &ViewFacts,
    handler: &str,
    result: &SymbolResult,
) -> Option<Value> {
    if !result.complete || result.declaration.is_some() {
        return None;
    }
    let insertion = result.insertion.as_ref()?;
    if count_identifier_occurrences(&insertion.text, handler) != 1 {
        return None;
    }
    let mut plan = EditPlan::new(facts.doc.file.clone());
    let range = SourceRange::from_bounds(insertion.offset, insertion.offset).ok()?;
    plan.add(
        &insertion.file,
        range,
        "",
        insertion.text.clone(),
        EditRule::Preformatted,
    )
    .ok()?;
    let edit = plan.document_changes(server).ok()?;
    Some(json!({
        "title": format!("Create event handler '{handler}'"),
        "kind": "quickfix",
        "isPreferred": true,
        "edit": edit,
    }))
}

fn count_identifier_occurrences(text: &str, name: &str) -> usize {
    let mut count = 0_usize;
    let mut start = 0_usize;
    while let Some(index) = text.get(start..).and_then(|rest| rest.find(name)) {
        let absolute = start + index;
        let before = text[..absolute].chars().next_back();
        let after = text[absolute + name.len()..].chars().next();
        let before_ok = before.is_none_or(|c| !is_identifier_char(c));
        let after_ok = after.is_none_or(|c| !is_identifier_char(c));
        if before_ok && after_ok {
            count += 1;
        }
        start = absolute + name.len();
    }
    count
}

const fn is_identifier_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '$')
}

/// Offers a quick fix that replaces a data field typo with its closest sibling.
fn typo_code_action(
    server: &Server,
    facts: &ViewFacts,
    typo: &str,
    token: SourceRange,
    result: &SymbolResult,
) -> Option<Value> {
    if !result.complete {
        return None;
    }
    let mut candidates: Vec<(&str, usize)> = result
        .fields
        .iter()
        .filter(|field| field.name != typo && is_identifier(&field.name))
        .map(|field| (field.name.as_str(), edit_distance(typo, &field.name)))
        .filter(|(_, distance)| *distance <= MAX_SUGGESTION_DISTANCE)
        .collect();
    candidates.sort_by_key(|(name, distance)| (*distance, *name));
    let (best, distance) = candidates.first().copied()?;
    if candidates
        .iter()
        .skip(1)
        .any(|(_, other)| *other == distance)
    {
        return None;
    }
    let mut plan = EditPlan::new(facts.doc.file.clone());
    let target = SymbolToken {
        file: facts.doc.file.clone(),
        range: token,
        style: TokenStyle::Identifier,
    };
    add_token_edit(&mut plan, server, &target, best, EditRule::Identifier).ok()?;
    let edit = plan.document_changes(server).ok()?;
    Some(json!({
        "title": format!("Replace '{typo}' with '{best}'"),
        "kind": "quickfix",
        "isPreferred": true,
        "edit": edit,
    }))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > MAX_SUGGESTION_DISTANCE {
        return MAX_SUGGESTION_DISTANCE + 1;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len() + 1];
    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(left_char != right_char);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

impl Server {
    /// Handles `textDocument/semanticTokens/full`.
    pub(crate) fn semantic_tokens_full(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(uri) = crate::handlers::text_document(&params)
            .and_then(|document| crate::handlers::string(document, "uri"))
        else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument URI",
            )];
        };
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, json!({"data": []}))];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, json!({"data": []}))];
        };
        let value = super::tokens::semantic_tokens(self, document);
        vec![Outgoing::success(id, value)]
    }

    /// Handles `workspace/symbol`.
    #[allow(clippy::needless_pass_by_value)] // dispatch passes the request params by value
    pub(crate) fn workspace_symbol(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let query = params
            .as_ref()
            .and_then(|params| params.get("query"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let value = super::hints::workspace_symbols(self, query);
        vec![Outgoing::success(id, value)]
    }

    /// Handles `textDocument/inlayHint`.
    pub(crate) fn inlay_hint(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some((uri, range)) = crate::handlers::range_target(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument or range",
            )];
        };
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, json!([]))];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        // LSP clients commonly request a range past the end of a short file;
        // clamp the end to the document instead of dropping every hint.
        let clamped = document.offset_at(range.0).map(|start| {
            let end = document
                .offset_at(range.1)
                .unwrap_or_else(|| u32::try_from(document.text().len()).unwrap_or(u32::MAX));
            (
                document
                    .snapshot()
                    .line_index()
                    .position(usize::try_from(start).unwrap_or(usize::MAX))
                    .unwrap_or(Position::new(0, 0)),
                document
                    .snapshot()
                    .line_index()
                    .position(usize::try_from(end).unwrap_or(usize::MAX))
                    .unwrap_or(Position::new(u32::MAX, 0)),
            )
        });
        let value = super::hints::inlay_hints(self, document, clamped);
        vec![Outgoing::success(id, value)]
    }

    /// Handles `textDocument/signatureHelp`.
    pub(crate) fn signature_help(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
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
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, Value::Null)];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let value = super::hints::signature_help(self, document, position);
        vec![Outgoing::success(id, value)]
    }

    /// Handles `textDocument/codeLens`.
    pub(crate) fn code_lens(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(uri) = crate::handlers::text_document(&params)
            .and_then(|document| crate::handlers::string(document, "uri"))
        else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument URI",
            )];
        };
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, json!([]))];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let value = super::hints::code_lens(self, document);
        vec![Outgoing::success(id, value)]
    }

    /// Handles `workspace/executeCommand` with the one read-only command.
    pub(crate) fn execute_command(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(command) = params.get("command").and_then(Value::as_str) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing command",
            )];
        };
        if command != "wxml.references" {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "unknown command",
            )];
        }
        let Some(argument) = params
            .get("arguments")
            .and_then(Value::as_array)
            .and_then(|arguments| arguments.first())
        else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "wxml.references requires one argument",
            )];
        };
        let Some(uri) = argument.get("uri").and_then(Value::as_str) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "wxml.references argument has no uri",
            )];
        };
        let Some(position) = argument.get("position") else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "wxml.references argument has no position",
            )];
        };
        let include_declaration = argument
            .get("includeDeclaration")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let params = json!({
            "textDocument": {"uri": uri},
            "position": position,
            "context": {"includeDeclaration": include_declaration},
        });
        self.references_inner(id, &params, true)
    }
}
