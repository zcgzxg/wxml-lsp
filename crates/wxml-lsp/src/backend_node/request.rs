//! Builds C1.1 `AnalyzeRequest` JSON from a prepared view.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use wxml_text::SourceRange;
use wxml_types::{
    CheckFragment, CompletionKind, ComponentAttribute, ComponentUsage, EventHandler, GuardChain,
    LocalBinding, RequestContext,
};

use super::config::NodeBackendConfig;
use super::protocol::{PROTOCOL_VERSION, ProtocolError};
use crate::project_host::PreparedView;

/// Where one fragment's target token lives in the WXML source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FragmentInfo {
    /// Request fragment id.
    pub fragment_id: String,
    /// Chosen target identifier, when one could be found.
    pub target: Option<String>,
    /// Byte range of the target token in the WXML source.
    pub target_range: Option<SourceRange>,
    /// Byte range of the whole mustache expression in the WXML source.
    pub expr_range: SourceRange,
    /// Structured `wx:for` bindings in scope at the fragment.
    pub bindings: Vec<LocalBinding>,
}

/// Where one requested event handler lives in the WXML source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventInfo {
    /// Request event id, `e0`, `e1`, ... in source order.
    pub event_id: String,
    /// Raw attribute name, for example `bind:tap`.
    pub attribute: String,
    /// Normalized event name, for example `tap`.
    pub event: String,
    /// Static handler name as written, trimmed.
    pub handler: String,
    /// Byte range of the whole attribute in the WXML source.
    pub attribute_range: SourceRange,
    /// Byte range of the handler token in the WXML source.
    pub handler_range: SourceRange,
}

/// A serialized C1 request plus the metadata needed to map the response.
#[derive(Clone, Debug)]
pub(crate) struct BuiltRequest {
    /// One NDJSON line.
    pub line: String,
    /// C1 request id, echoed by Node.
    pub request_id: String,
    /// Per-fragment target metadata.
    pub fragments: BTreeMap<String, FragmentInfo>,
    /// Requested events in the same order as their `e0`, `e1`, ... ids.
    pub events: Vec<EventInfo>,
}

/// Builds the request for one view.
///
/// Returns `Ok(None)` when the view has no usable owner/script or no
/// identifier target; the caller should not start Node in that case.
pub(crate) fn build(
    view: &PreparedView,
    context: &RequestContext,
    config: &NodeBackendConfig,
    request_id: String,
) -> Result<Option<BuiltRequest>, ProtocolError> {
    let Some(target) = view.target.as_ref() else {
        return Ok(None);
    };
    let script_path = target.script_file.path();
    if !view
        .files
        .iter()
        .any(|file| file.path.as_str() == script_path)
    {
        return Ok(None);
    }

    let mut fragments = Vec::with_capacity(view.fragments.len());
    let mut infos = BTreeMap::new();
    for fragment in view.fragments.iter() {
        let (selected, target_range) = fragment_target(fragment);
        infos.insert(
            fragment.fragment_id.clone(),
            FragmentInfo {
                fragment_id: fragment.fragment_id.clone(),
                target: selected.clone(),
                target_range,
                expr_range: fragment.wxml_range,
                bindings: fragment.bindings.clone(),
            },
        );
        fragments.push(fragment_value(fragment, selected.as_deref()));
    }

    let components: Vec<Value> = view.components.iter().map(component_value).collect();

    let events = event_infos(&view.events);
    let event_values: Vec<Value> = events.iter().map(event_value).collect();

    let files: Vec<Value> = view
        .files
        .iter()
        .map(|file| {
            json!({
                "fileId": file.file.path(),
                "path": file.path.as_str(),
                "kind": file.kind,
                "text": file.text.as_ref(),
                "sourceRevision": file.source_revision,
            })
        })
        .collect();

    let params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "requestId": request_id,
        "analysisStamp": {
            "snapshotId": context.source().snapshot().get(),
            "projectRevision": context.project_revision().get(),
            "viewRevision": context.view_revision().get(),
            "backendEpoch": context.backend_epoch().get(),
        },
        "view": {
            "viewId": view.view_id,
            "owner": {
                "kind": target.owner.as_str(),
                "fileId": target.script_file.path(),
            },
        },
        "project": {
            "projectRoot": view.project_root.to_string_lossy(),
            "compilerOptions": Value::Object(view.compiler_options.clone()),
            "typings": {
                "package": config.typings.package,
                "version": config.typings.version,
            },
        },
        "scriptFileId": target.script_file.path(),
        "files": files,
        "fragments": fragments,
        // Protocol v2 optional inputs.
        "components": components,
        "events": event_values,
        "policy": {
            "jsCheck": config.js_check.as_str(),
            "propertiesOverlay": "declaration",
        },
        "limits": {
            "maxFiles": config.limits.max_files,
            "maxTotalBytes": config.limits.max_total_bytes,
            "maxFragments": config.limits.max_fragments,
        },
    });
    let line = super::protocol::request_line(&request_id, "analyze", Some(&params))
        .map_err(|_| ProtocolError::Json)?;
    Ok(Some(BuiltRequest {
        line,
        request_id,
        fragments: infos,
        events,
    }))
}

/// Serializes one fragment for the protocol v2 `analyze` request.
///
/// `bindings` is sent as-is (full chain, duplicates included). `locals` is
/// kept for C1.1 clients: it derives first-occurrence binding names, while the
/// backend resolves types from the full chain.
fn fragment_value(fragment: &CheckFragment, selected: Option<&str>) -> Value {
    let mut value = json!({
        "fragmentId": fragment.fragment_id,
        "exprText": fragment.expr_text,
        "locals": effective_locals(&fragment.bindings),
        "bindings": fragment
            .bindings
            .iter()
            .map(local_binding_value)
            .collect::<Vec<Value>>(),
        "guards": fragment
            .guards
            .iter()
            .map(guard_chain_value)
            .collect::<Vec<Value>>(),
        "wxmlRange": {
            "start": fragment.wxml_range.start(),
            "end": fragment.wxml_range.end(),
        },
        "fragmentRevision": fragment.fragment_revision,
        "root": fragment.root.as_str(),
    });
    if let Some(selected) = selected {
        if let Some(object) = value.as_object_mut() {
            object.insert("target".to_owned(), Value::String(selected.to_owned()));
        }
    }
    value
}

/// Serializes one structured template binding for protocol v2.
fn local_binding_value(binding: &LocalBinding) -> Value {
    json!({
        "name": binding.name,
        "role": binding.role.as_str(),
        "sourceExpr": binding.source_expr,
        "sourceRange": {
            "start": binding.source_range.start(),
            "end": binding.source_range.end(),
        },
    })
}

/// Serializes one `wx:if`/`wx:elif`/`wx:else` chain for protocol v2.
fn guard_chain_value(chain: &GuardChain) -> Value {
    let conditions = chain
        .conditions
        .iter()
        .map(|condition| {
            json!({
                "branch": condition.branch.as_str(),
                "exprText": condition.expr_text,
                "range": {
                    "start": condition.range.start(),
                    "end": condition.range.end(),
                },
            })
        })
        .collect::<Vec<Value>>();
    json!({
        "conditions": conditions,
        "selected": chain.selected,
    })
}

/// Returns the effective binding names in scope, one per name and in order.
fn effective_locals(bindings: &[LocalBinding]) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if !names.iter().any(|name| name == &binding.name) {
            names.push(binding.name.clone());
        }
    }
    names
}

/// Serializes one custom-component usage for protocol v2.
fn component_value(usage: &ComponentUsage) -> Value {
    let mut value = json!({
        "tag": usage.tag,
        "attributes": usage
            .attributes
            .iter()
            .map(component_attribute_value)
            .collect::<Vec<Value>>(),
    });
    if let Some(file) = usage.component_file.as_ref() {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "componentFileId".to_owned(),
                Value::String(file.path().to_owned()),
            );
        }
    }
    value
}

/// Serializes one component attribute; `exprText`/`exprRange` are optional.
fn component_attribute_value(attribute: &ComponentAttribute) -> Value {
    let mut value = json!({
        "name": attribute.name,
        "valueKind": attribute.value_kind.as_str(),
        "attributeRange": byte_range(attribute.attribute_range),
    });
    if let Some(object) = value.as_object_mut() {
        if let Some(expr_text) = attribute.expr_text.as_ref() {
            object.insert("exprText".to_owned(), Value::String(expr_text.clone()));
        }
        if let Some(range) = attribute.expr_range {
            object.insert("exprRange".to_owned(), byte_range(range));
        }
    }
    value
}

/// Assigns stable ids `e0`, `e1`, ... and keeps the ranges for response mapping.
fn event_infos(events: &[EventHandler]) -> Vec<EventInfo> {
    events
        .iter()
        .enumerate()
        .map(|(index, event)| EventInfo {
            event_id: format!("e{index}"),
            attribute: event.attribute.clone(),
            event: event.event.clone(),
            handler: event.handler.clone(),
            attribute_range: event.attribute_range,
            handler_range: event.handler_range,
        })
        .collect()
}

/// Serializes one static event binding for protocol v2.
fn event_value(info: &EventInfo) -> Value {
    json!({
        "eventId": info.event_id,
        "attribute": info.attribute,
        "event": info.event,
        "handler": info.handler,
        "attributeRange": byte_range(info.attribute_range),
        "handlerRange": byte_range(info.handler_range),
    })
}

/// Serializes a byte range in the protocol `{start,end}` shape.
fn byte_range(range: SourceRange) -> Value {
    json!({"start": range.start(), "end": range.end()})
}

/// Selects the P6 target or the default right-value identifier.
fn fragment_target(fragment: &CheckFragment) -> (Option<String>, Option<SourceRange>) {
    let ranges = identifier_ranges(&fragment.expr_text);
    let chosen = fragment
        .target
        .as_ref()
        .and_then(|target| {
            ranges
                .iter()
                .find(|(_, _, text)| *text == target.as_str())
                .map(|(start, end, text)| (*start, *end, (*text).to_owned()))
        })
        .or_else(|| {
            ranges
                .iter()
                .rev()
                .find(|(_, _, text)| !is_keyword(text))
                .map(|(start, end, text)| (*start, *end, (*text).to_owned()))
        });
    let Some((start, end, target)) = chosen else {
        return (None, None);
    };
    let start = u32::try_from(start).ok();
    let end = u32::try_from(end).ok();
    let target_range = start
        .zip(end)
        .and_then(|(start, end)| SourceRange::from_bounds(start, end).ok())
        .and_then(|range| {
            let wxml = fragment.wxml_range;
            let offset = wxml.start();
            let start = offset.checked_add(range.start())?;
            let end = offset.checked_add(range.end())?;
            SourceRange::from_bounds(start, end).ok()
        });
    (Some(target), target_range)
}

/// Returns identifier tokens `(start, end, text)` outside strings/comments.
fn identifier_ranges(expr: &str) -> Vec<(usize, usize, &str)> {
    let bytes = expr.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0_usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\'' || byte == b'"' {
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = index.saturating_add(2);
                    continue;
                }
                if bytes[index] == byte {
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if is_identifier_start(byte) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_identifier_continue(bytes[index]) {
                index += 1;
            }
            if let Some(text) = expr.get(start..index) {
                ranges.push((start, index, text));
            }
            continue;
        }
        index += 1;
    }
    ranges
}

const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

const fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn is_keyword(text: &str) -> bool {
    matches!(
        text,
        "true" | "false" | "null" | "undefined" | "this" | "new" | "typeof" | "void"
    )
}

/// Returns the `{{ ...` expression start and the text up to `offset`.
fn expression_prefix(text: &str, offset: u32) -> (u32, String) {
    const MAX_EXPRESSION_BYTES: usize = 4_096;
    let end = usize::try_from(offset)
        .unwrap_or(text.len())
        .min(text.len());
    let head = text.get(..end).unwrap_or_default();
    let mut start = head.rfind("{{").map_or(end, |index| index + 2);
    // Only the tail of a very long, unclosed expression can affect member
    // completion; keep the request bounded and start on a char boundary.
    if end.saturating_sub(start) > MAX_EXPRESSION_BYTES {
        let mut clipped = end - MAX_EXPRESSION_BYTES;
        while clipped < end && !text.is_char_boundary(clipped) {
            clipped += 1;
        }
        start = clipped;
    }
    let expression = head.get(start..).unwrap_or_default();
    if expression.is_empty() {
        // Protocol v2 rejects empty `exprText`; one space trims to an empty
        // expression in Node and keeps the range self-consistent.
        (u32::try_from(start).unwrap_or(offset), " ".to_owned())
    } else {
        (
            u32::try_from(start).unwrap_or(offset),
            expression.to_owned(),
        )
    }
}

/// Builds a protocol v2 `complete` request line.
///
/// The expression prefix is taken from the snapshot rather than the analyzer
/// fragment, because a fragment may omit a trailing `.` and an empty mustache
/// has no fragment at all. Node then synthesizes a parseable expression.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_completion(
    view: &PreparedView,
    context: &RequestContext,
    config: &NodeBackendConfig,
    request_id: &str,
    offset: u32,
    kind: CompletionKind,
    prefix: &str,
) -> Result<Option<String>, ProtocolError> {
    let Some(target) = view.target.as_ref() else {
        return Ok(None);
    };
    let script_path = target.script_file.path();
    if !view
        .files
        .iter()
        .any(|file| file.path.as_str() == script_path)
    {
        return Ok(None);
    }

    let template = context.source().text();
    let (expression_start, expr_text) = expression_prefix(template, offset);
    let analyzer_fragment = view.fragments.iter().find(|fragment| {
        fragment.wxml_range.start() <= offset && offset <= fragment.wxml_range.end()
    });
    let bindings = analyzer_fragment.map_or_else(Vec::new, |fragment| fragment.bindings.clone());
    let guards = analyzer_fragment.map_or_else(Vec::new, |fragment| fragment.guards.clone());
    let locals = effective_locals(&bindings);
    let fragment_revision = analyzer_fragment.map_or(1, |fragment| fragment.fragment_revision);
    let range_end = expression_start
        .checked_add(u32::try_from(expr_text.len()).unwrap_or(0))
        .unwrap_or(offset);
    // Event-handler completion never mounts a fragment; keep the request
    // minimal so template text is not sent for a pure script query.
    let fragments: Vec<Value> = if matches!(kind, CompletionKind::ExpressionMember) {
        vec![json!({
            "fragmentId": "complete-0",
            "exprText": expr_text,
            "locals": locals,
            "bindings": bindings.iter().map(local_binding_value).collect::<Vec<Value>>(),
            "guards": guards.iter().map(guard_chain_value).collect::<Vec<Value>>(),
            "wxmlRange": {"start": expression_start, "end": range_end},
            "fragmentRevision": fragment_revision,
            "root": target.owner.as_str(),
        })]
    } else {
        Vec::new()
    };
    let files: Vec<Value> = view
        .files
        .iter()
        .map(|file| {
            json!({
                "fileId": file.file.path(),
                "path": file.path.as_str(),
                "kind": file.kind,
                "text": file.text.as_ref(),
                "sourceRevision": file.source_revision,
            })
        })
        .collect();

    let params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "requestId": request_id,
        "analysisStamp": {
            "snapshotId": context.source().snapshot().get(),
            "projectRevision": context.project_revision().get(),
            "viewRevision": context.view_revision().get(),
            "backendEpoch": context.backend_epoch().get(),
        },
        "view": {
            "viewId": view.view_id,
            "owner": {
                "kind": target.owner.as_str(),
                "fileId": target.script_file.path(),
            },
        },
        "project": {
            "projectRoot": view.project_root.to_string_lossy(),
            "compilerOptions": Value::Object(view.compiler_options.clone()),
            "typings": {
                "package": config.typings.package,
                "version": config.typings.version,
            },
        },
        "scriptFileId": target.script_file.path(),
        "files": files,
        "fragments": fragments,
        "components": Vec::<Value>::new(),
        "events": Vec::<Value>::new(),
        "policy": {
            "jsCheck": config.js_check.as_str(),
            "propertiesOverlay": "declaration",
        },
        "limits": {
            "maxFiles": config.limits.max_files,
            "maxTotalBytes": config.limits.max_total_bytes,
            "maxFragments": config.limits.max_fragments,
        },
        "offset": offset,
        "kind": kind.as_str(),
        "prefix": prefix,
    });
    super::protocol::request_line(request_id, "complete", Some(&params)).map(Some)
}

/// Local validation mirroring the Node validator's query bounds.
fn valid_symbol_query(query: &wxml_types::SymbolQuery) -> bool {
    fn segment_ok(segment: &str) -> bool {
        !segment.is_empty()
            && segment.len() <= wxml_types::MAX_SYMBOL_NAME_BYTES
            && !segment
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            && !segment
                .chars()
                .any(|character| matches!(character, '.' | '[' | ']' | '\'' | '"' | '`'))
    }
    match query {
        wxml_types::SymbolQuery::DataRoots => true,
        wxml_types::SymbolQuery::DataPath { path } => {
            !path.is_empty()
                && path.len() <= wxml_types::MAX_SYMBOL_PATH
                && path.iter().all(|segment| segment_ok(segment))
        }
        wxml_types::SymbolQuery::EventHandler { name } => {
            !name.is_empty() && name.len() <= wxml_types::MAX_SYMBOL_NAME_BYTES && {
                let mut chars = name.chars();
                chars.next().is_some_and(|first| {
                    (first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
                        && chars.all(|character| {
                            character.is_ascii_alphanumeric() || matches!(character, '_' | '$')
                        })
                })
            }
        }
        _ => false,
    }
}

/// Builds a protocol `symbols` request line.
///
/// The view supplies the same file snapshot used by `analyze`; the subject
/// file must be one of those files so a symbol query can never resolve a path
/// outside the current project snapshot. `Ok(None)` means the view has no
/// usable owner or the subject is not part of it.
pub(crate) fn build_symbols(
    view: &PreparedView,
    context: &RequestContext,
    config: &NodeBackendConfig,
    request_id: &str,
    subject: &wxml_types::FileId,
    query: &wxml_types::SymbolQuery,
) -> Result<Option<String>, ProtocolError> {
    let Some(target) = view.target.as_ref() else {
        return Ok(None);
    };
    if !view.files.iter().any(|file| file.file == *subject) {
        return Ok(None);
    }
    if !valid_symbol_query(query) {
        return Ok(None);
    }
    let files: Vec<Value> = view
        .files
        .iter()
        .map(|file| {
            json!({
                "fileId": file.file.path(),
                "path": file.path.as_str(),
                "kind": file.kind,
                "text": file.text.as_ref(),
                "sourceRevision": file.source_revision,
            })
        })
        .collect();
    let query = match query {
        wxml_types::SymbolQuery::DataRoots => json!({"kind": "dataRoots"}),
        wxml_types::SymbolQuery::DataPath { path } => {
            json!({"kind": "dataPath", "path": path})
        }
        wxml_types::SymbolQuery::EventHandler { name } => {
            json!({"kind": "eventHandler", "name": name})
        }
        _ => return Ok(None),
    };
    let params = json!({
        "protocolVersion": PROTOCOL_VERSION,
        "requestId": request_id,
        "project": {
            "projectRoot": view.project_root.to_string_lossy(),
            "compilerOptions": Value::Object(view.compiler_options.clone()),
            "typings": {
                "package": config.typings.package,
                "version": config.typings.version,
            },
        },
        "files": files,
        "subjectFileId": subject.path(),
        "sourceComplete": view.source_complete,
        "root": target.owner.as_str(),
        "query": query,
        "limits": {
            "maxFiles": config.limits.max_files,
            "maxTotalBytes": config.limits.max_total_bytes,
            "maxFragments": config.limits.max_fragments,
        },
    });
    let _ = context;
    super::protocol::request_line(request_id, "symbols", Some(&params)).map(Some)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use wxml_types::{
        AttributeValueKind, BackendEpoch, BackendId, CheckFragment, ComponentAttribute,
        ComponentUsage, EventBindingKind, EventHandler, FileId, ProjectId, ProjectRevision,
        RootKind, SnapshotId, SourceRevision, SourceSnapshot, ViewRevision,
    };

    use super::*;
    use crate::project_host::{PreparedFile, ViewTarget};

    fn fragment(expr: &str, start: u32) -> CheckFragment {
        CheckFragment {
            fragment_id: "f0".to_owned(),
            expr_text: expr.to_owned(),
            target: None,
            bindings: Vec::new(),
            guards: Vec::new(),
            wxml_range: SourceRange::from_bounds(
                start,
                start + u32::try_from(expr.len()).expect("len"),
            )
            .expect("range"),
            fragment_revision: 1,
            root: RootKind::Page,
        }
    }

    fn view(fragment: CheckFragment) -> PreparedView {
        let project = ProjectId::new("proj-test");
        let script = FileId::new(project, "pages/index/index.ts").expect("script");
        PreparedView {
            view_id: "pages/index/index.wxml:1".to_owned(),
            target: Some(ViewTarget {
                wxml: wxml_project::ProjectPath::new("pages/index/index.wxml").expect("path"),
                owner: RootKind::Page,
                script: wxml_project::ProjectPath::new("pages/index/index.ts").expect("script"),
                script_file: script.clone(),
                view_revision: ViewRevision::new(1),
            }),
            files: Arc::from(vec![PreparedFile {
                file: script,
                path: wxml_project::ProjectPath::new("pages/index/index.ts").expect("script"),
                kind: "ts",
                text: Arc::from("Page({ data: {} });"),
                source_revision: 1,
            }]),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(vec![fragment]),
            components: Arc::from(Vec::new()),
            events: Arc::from(Vec::new()),
            project_root: PathBuf::from("/tmp/project"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        }
    }

    fn context() -> RequestContext {
        let project = ProjectId::new("proj-test");
        let file = FileId::new(project.clone(), "pages/index/index.wxml").expect("wxml");
        RequestContext::new(
            ProjectRevision::new(project, 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("node"), 1),
            Arc::new(SourceSnapshot::new(
                file,
                SourceRevision::new(1),
                SnapshotId::new(1),
                "{{ user.name }}",
            )),
        )
        .expect("context")
    }

    #[test]
    fn defaults_to_the_last_identifier() {
        for (expr, expected) in [
            ("user.name", "name"),
            ("!title", "title"),
            ("(title)", "title"),
            ("title + user.name", "name"),
            ("user.nmae", "nmae"),
            ("true", "true"),
        ] {
            let ranges = identifier_ranges(expr);
            let chosen = ranges
                .iter()
                .rev()
                .find(|(_, _, text)| !is_keyword(text))
                .map(|(_, _, text)| *text);
            assert_eq!(
                chosen,
                if expected == "true" {
                    None
                } else {
                    Some(expected)
                },
                "{expr}"
            );
        }
    }

    #[test]
    fn ignores_identifiers_in_strings_and_comments() {
        let ranges = identifier_ranges(
            r#""ignore.me" + // comment.name
            user.name /* other.nope */"#,
        );
        let texts: Vec<&str> = ranges.iter().map(|(_, _, text)| *text).collect();
        assert_eq!(texts, vec!["user", "name"]);
    }

    #[test]
    fn maps_the_target_back_to_wxml_bytes() {
        let view = view(fragment("user.name", 10));
        let request = build(
            &view,
            &context(),
            &NodeBackendConfig::default(),
            "req".to_owned(),
        )
        .expect("request")
        .expect("some");
        let info = request.fragments.get("f0").expect("info");
        assert_eq!(info.target.as_deref(), Some("name"));
        let range = info.target_range.expect("range");
        assert_eq!(range.start(), 15);
        assert_eq!(range.end(), 19);
        let value: Value = serde_json::from_str(&request.line).expect("json");
        assert_eq!(value["method"], "analyze");
        let params = &value["params"];
        assert_eq!(params["protocolVersion"], 2);
        assert_eq!(params["view"]["owner"]["kind"], "page");
        assert_eq!(params["files"][0]["fileId"], "pages/index/index.ts");
        assert_eq!(params["fragments"][0]["wxmlRange"]["start"], 10);
        assert_eq!(params["fragments"][0]["target"], "name");
        assert!(
            params["fragments"][0]["bindings"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );
        assert!(
            params["fragments"][0]["guards"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );
        assert!(params["components"].as_array().is_some_and(Vec::is_empty));
        assert!(params["events"].as_array().is_some_and(Vec::is_empty));
        assert_eq!(params["policy"]["propertiesOverlay"], "declaration");
    }

    #[test]
    fn serializes_v2_bindings_and_guard_chains() {
        let mut fragment = fragment("item.name", 10);
        fragment.bindings = vec![
            LocalBinding {
                name: "item".to_owned(),
                role: wxml_types::LocalRole::Item,
                source_expr: "groups".to_owned(),
                source_range: SourceRange::from_bounds(100, 106).expect("range"),
            },
            LocalBinding {
                name: "index".to_owned(),
                role: wxml_types::LocalRole::Index,
                source_expr: "groups".to_owned(),
                source_range: SourceRange::from_bounds(100, 106).expect("range"),
            },
        ];
        fragment.guards = vec![GuardChain {
            conditions: vec![wxml_types::GuardCondition {
                branch: wxml_types::GuardBranch::If,
                expr_text: "state.ready".to_owned(),
                range: SourceRange::from_bounds(200, 211).expect("range"),
            }],
            selected: 0,
        }];
        let view = view(fragment);
        let request = build(
            &view,
            &context(),
            &NodeBackendConfig::default(),
            "req".to_owned(),
        )
        .expect("request")
        .expect("some");
        let value: Value = serde_json::from_str(&request.line).expect("json");
        let params = &value["params"];
        assert_eq!(params["protocolVersion"], 2);
        assert_eq!(params["fragments"][0]["locals"], json!(["item", "index"]));
        assert_eq!(params["fragments"][0]["bindings"][0]["name"], "item");
        assert_eq!(params["fragments"][0]["bindings"][0]["role"], "item");
        assert_eq!(
            params["fragments"][0]["bindings"][0]["sourceExpr"],
            "groups"
        );
        assert_eq!(
            params["fragments"][0]["bindings"][0]["sourceRange"]["start"],
            100
        );
        assert_eq!(params["fragments"][0]["bindings"][1]["role"], "index");
        assert_eq!(
            params["fragments"][0]["guards"][0]["conditions"][0]["branch"],
            "if"
        );
        assert_eq!(
            params["fragments"][0]["guards"][0]["conditions"][0]["exprText"],
            "state.ready"
        );
        assert_eq!(params["fragments"][0]["guards"][0]["selected"], 0);
        assert_eq!(
            params["fragments"][0]["guards"][0]["conditions"][0]["range"]["end"],
            211
        );
    }

    #[test]
    fn shadowed_loop_bindings_keep_the_full_chain() {
        let range = |start: u32, end: u32| SourceRange::from_bounds(start, end).expect("range");
        let mut fragment = fragment("item.name", 10);
        fragment.bindings = vec![
            LocalBinding {
                name: "item".to_owned(),
                role: wxml_types::LocalRole::Item,
                source_expr: "groups".to_owned(),
                source_range: range(100, 106),
            },
            LocalBinding {
                name: "itemIndex".to_owned(),
                role: wxml_types::LocalRole::Index,
                source_expr: "groups".to_owned(),
                source_range: range(100, 106),
            },
            LocalBinding {
                name: "item".to_owned(),
                role: wxml_types::LocalRole::Item,
                source_expr: "item.members".to_owned(),
                source_range: range(120, 132),
            },
            LocalBinding {
                name: "index".to_owned(),
                role: wxml_types::LocalRole::Index,
                source_expr: "item.members".to_owned(),
                source_range: range(120, 132),
            },
        ];
        let view = view(fragment);
        let request = build(
            &view,
            &context(),
            &NodeBackendConfig::default(),
            "req".to_owned(),
        )
        .expect("request")
        .expect("some");
        let value: Value = serde_json::from_str(&request.line).expect("json");
        let params = &value["params"];
        // `locals` keeps only the first occurrence of each name.
        assert_eq!(
            params["fragments"][0]["locals"],
            json!(["item", "itemIndex", "index"])
        );
        // `bindings` keeps the full outermost-to-innermost chain, duplicates
        // included, so the backend can resolve the shadowing inner loop.
        let bindings = params["fragments"][0]["bindings"]
            .as_array()
            .expect("bindings");
        assert_eq!(bindings.len(), 4);
        assert_eq!(bindings[0]["sourceExpr"], "groups");
        assert_eq!(bindings[1]["role"], "index");
        assert_eq!(bindings[2]["name"], "item");
        assert_eq!(bindings[2]["sourceExpr"], "item.members");
        assert_eq!(bindings[3]["role"], "index");
        assert_eq!(bindings[3]["sourceExpr"], "item.members");
    }

    #[test]
    fn serializes_components_with_component_file_and_attributes() {
        let mut view = view(fragment("item.name", 10));
        let project = ProjectId::new("proj-test");
        view.components = Arc::from(vec![ComponentUsage {
            tag: "card".to_owned(),
            component_file: Some(
                FileId::new(project, "components/card/card.ts").expect("component file"),
            ),
            attributes: vec![
                ComponentAttribute {
                    name: "title".to_owned(),
                    value_kind: AttributeValueKind::Mustache,
                    expr_text: Some("pageTitle".to_owned()),
                    expr_range: Some(SourceRange::from_bounds(30, 39).expect("expr range")),
                    attribute_range: SourceRange::from_bounds(20, 41).expect("attribute range"),
                },
                ComponentAttribute {
                    name: "disabled".to_owned(),
                    value_kind: AttributeValueKind::Boolean,
                    expr_text: None,
                    expr_range: None,
                    attribute_range: SourceRange::from_bounds(42, 50).expect("attribute range"),
                },
            ],
        }]);
        let request = build(
            &view,
            &context(),
            &NodeBackendConfig::default(),
            "req".to_owned(),
        )
        .expect("request")
        .expect("some");
        let value: Value = serde_json::from_str(&request.line).expect("json");
        let component = &value["params"]["components"][0];
        assert_eq!(component["tag"], "card");
        assert_eq!(component["componentFileId"], "components/card/card.ts");
        assert_eq!(component["attributes"][0]["name"], "title");
        assert_eq!(component["attributes"][0]["valueKind"], "mustache");
        assert_eq!(component["attributes"][0]["exprText"], "pageTitle");
        assert_eq!(component["attributes"][0]["exprRange"]["start"], 30);
        assert_eq!(component["attributes"][0]["exprRange"]["end"], 39);
        assert_eq!(component["attributes"][0]["attributeRange"]["start"], 20);
        assert_eq!(component["attributes"][1]["valueKind"], "boolean");
        assert!(component["attributes"][1].get("exprText").is_none());
        assert!(component["attributes"][1].get("exprRange").is_none());
    }

    #[test]
    fn serializes_events_with_stable_ids_and_byte_ranges() {
        let mut view = view(fragment("item.name", 10));
        let range = |start: u32, end: u32| SourceRange::from_bounds(start, end).expect("range");
        view.events = Arc::from(vec![
            EventHandler {
                attribute: "bind:tap".to_owned(),
                binding: EventBindingKind::Bind,
                event: "tap".to_owned(),
                handler: "onTap".to_owned(),
                range: range(20, 34),
                attribute_range: range(20, 34),
                handler_range: range(30, 35),
            },
            EventHandler {
                attribute: "catch:touchstart".to_owned(),
                binding: EventBindingKind::CaptureCatch,
                event: "touchstart".to_owned(),
                handler: "onTouch".to_owned(),
                range: range(40, 65),
                attribute_range: range(40, 65),
                handler_range: range(59, 66),
            },
        ]);
        let request = build(
            &view,
            &context(),
            &NodeBackendConfig::default(),
            "req".to_owned(),
        )
        .expect("request")
        .expect("some");
        assert_eq!(request.events.len(), 2);
        assert_eq!(request.events[0].event_id, "e0");
        assert_eq!(request.events[0].handler_range, range(30, 35));
        assert_eq!(request.events[1].event_id, "e1");
        let value: Value = serde_json::from_str(&request.line).expect("json");
        let events = value["params"]["events"].as_array().expect("events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["eventId"], "e0");
        assert_eq!(events[0]["attribute"], "bind:tap");
        assert_eq!(events[0]["event"], "tap");
        assert_eq!(events[0]["handler"], "onTap");
        assert_eq!(events[0]["attributeRange"]["start"], 20);
        assert_eq!(events[0]["attributeRange"]["end"], 34);
        assert_eq!(events[0]["handlerRange"]["start"], 30);
        assert_eq!(events[0]["handlerRange"]["end"], 35);
        assert_eq!(events[1]["eventId"], "e1");
        assert_eq!(events[1]["handler"], "onTouch");
    }

    #[test]
    fn components_without_a_script_omit_the_file_id() {
        let mut view = view(fragment("item.name", 10));
        view.components = Arc::from(vec![ComponentUsage {
            tag: "card".to_owned(),
            component_file: None,
            attributes: Vec::new(),
        }]);
        let request = build(
            &view,
            &context(),
            &NodeBackendConfig::default(),
            "req".to_owned(),
        )
        .expect("request")
        .expect("some");
        let value: Value = serde_json::from_str(&request.line).expect("json");
        let component = &value["params"]["components"][0];
        assert_eq!(component["tag"], "card");
        assert!(component.get("componentFileId").is_none());
    }
    #[test]
    fn over_deep_symbol_queries_are_rejected_locally() {
        use wxml_types::SymbolQuery;

        let view = view(fragment("user.name", 10));
        let context = context();
        let mut path = Vec::new();
        for index in 0..=wxml_types::MAX_SYMBOL_PATH {
            path.push(format!("s{index}"));
        }
        let line = build_symbols(
            &view,
            &context,
            &NodeBackendConfig::default(),
            "r1",
            &view.target.as_ref().expect("target").script_file,
            &SymbolQuery::DataPath { path },
        )
        .expect("build");
        assert!(line.is_none());
    }
}
