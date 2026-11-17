//! Workspace symbols, inlay hints, signature help and code lens.
//!
//! Every value is derived from existing facts: template semantics from T,
//! component property declarations from C, the static WXS summary from T and
//! the lossless tree from E. Each request builds its project/position indexes
//! once; no method parses a project view per symbol.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use wxml_text::{Position, SourceRange, TextRange};

use super::facts::{ImportCache, parse_doc, project_views, view_facts};
use super::position::Utf16Cursor;
use super::target::{collect_template_refs_with_views, for_each_element};
use crate::server::Server;
use crate::workspace::Document;

/// Maximum symbols returned by `workspace/symbol`.
const MAX_WORKSPACE_SYMBOLS: usize = 512;
/// Maximum inlay hints returned for one document.
const MAX_INLAY_HINTS: usize = 1_000;
/// Maximum template lens entries before the response is marked partial.
const MAX_TEMPLATE_LENSES: usize = 64;
/// Maximum component lens entries.
const MAX_COMPONENT_LENSES: usize = 64;
/// Maximum project WXML documents inspected for one workspace query.
const MAX_SYMBOL_DOCUMENTS: usize = 1_024;

#[derive(Clone, Debug)]
struct RawSymbol {
    file: wxml_types::FileId,
    name: String,
    kind: u8,
    range: SourceRange,
}

/// Returns `workspace/symbol` entries across every loaded project.
#[allow(clippy::too_many_lines)] // one bounded project scan
pub(crate) fn workspace_symbols(server: &Server, query: &str) -> Value {
    let needle = query.to_lowercase();
    let mut raw = Vec::new();
    let mut texts: BTreeMap<wxml_types::FileId, std::sync::Arc<str>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut complete = true;
    let mut inspected = 0_usize;
    let mut imports = ImportCache::default();
    let original_project = server.host.project_id();
    for project in server.host.project_ids() {
        if server.host.activate_project(&project).is_err() {
            complete = false;
            continue;
        }
        let Ok(probe) = wxml_types::FileId::new(project, "__workspace_symbol__") else {
            complete = false;
            continue;
        };
        let Some(templates) = server.host.view_templates(&probe) else {
            continue;
        };
        if !templates.complete {
            complete = false;
            break;
        }
        for template in templates.templates {
            if inspected >= MAX_SYMBOL_DOCUMENTS {
                complete = false;
                break;
            }
            inspected += 1;
            let Some(doc) = parse_doc(template.file.clone(), &template.text) else {
                complete = false;
                break;
            };
            texts.insert(template.file.clone(), template.text.clone());
            let semantics = wxml_types::analyze_template_semantics(doc.tree());
            add_template_definitions(
                server,
                &doc,
                &semantics,
                &mut imports,
                &mut raw,
                &mut texts,
                &mut seen,
                &mut complete,
            );
            let components = server.host.view_components_for(&template.file);
            let mut component_names = BTreeSet::new();
            for component in components {
                if !component_names.insert(component.name.clone()) {
                    continue;
                }
                if !matches_query(&component.name, &needle) {
                    continue;
                }
                let Some(range) = first_tag_range(&doc, &component.name) else {
                    continue;
                };
                push_raw(
                    &mut raw,
                    &mut seen,
                    RawSymbol {
                        file: template.file.clone(),
                        name: component.name,
                        kind: 5,
                        range: SourceRange::new(range),
                    },
                );
            }
        }
        if !complete {
            break;
        }
    }
    let _ = server.host.activate_project(&original_project);
    if !complete {
        crate::server::log(format_args!(
            "workspace symbols omitted: a project WXML index is incomplete"
        ));
        return json!([]);
    }
    raw.retain(|symbol| matches_query(&symbol.name, &needle));
    raw.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then_with(|| left.range.start().cmp(&right.range.start()))
            .then_with(|| left.name.cmp(&right.name))
    });
    raw.dedup_by(|left, right| {
        left.file == right.file && left.range == right.range && left.name == right.name
    });
    if raw.len() > MAX_WORKSPACE_SYMBOLS {
        raw.truncate(MAX_WORKSPACE_SYMBOLS);
        crate::server::log(format_args!(
            "workspace symbols truncated at {MAX_WORKSPACE_SYMBOLS}"
        ));
    }
    let mut symbols = Vec::with_capacity(raw.len());
    let mut current_file = None;
    let mut cursor: Option<Utf16Cursor<'_>> = None;
    for symbol in raw {
        if current_file.as_ref() != Some(&symbol.file) {
            let Some(text) = texts.get(&symbol.file) else {
                continue;
            };
            cursor = Some(Utf16Cursor::new(text));
            current_file = Some(symbol.file.clone());
        }
        let Some(cursor) = cursor.as_mut() else {
            continue;
        };
        let (Some(start), Some(end)) = (cursor.position(symbol.range.start()), {
            // Positions must advance monotonically; a second cursor is not
            // needed because symbols in one file are sorted by start.
            cursor.position(symbol.range.end())
        }) else {
            continue;
        };
        let Some(uri) = server.host.uri_for(&symbol.file) else {
            continue;
        };
        symbols.push(json!({
            "name": symbol.name,
            "kind": symbol.kind,
            "location": {
                "uri": uri,
                "range": {
                    "start": crate::handlers::position_value(start),
                    "end": crate::handlers::position_value(end),
                },
            },
        }));
    }
    json!(symbols)
}

#[allow(clippy::too_many_arguments)] // collected output parameters
fn add_template_definitions(
    server: &Server,
    doc: &super::facts::QueryDoc,
    semantics: &wxml_types::TemplateSemantics,
    imports: &mut ImportCache,
    raw: &mut Vec<RawSymbol>,
    texts: &mut BTreeMap<wxml_types::FileId, std::sync::Arc<str>>,
    seen: &mut BTreeSet<(wxml_types::FileId, String, u32, u32)>,
    complete: &mut bool,
) {
    for definition in semantics.distinct_definitions() {
        push_raw(
            raw,
            seen,
            RawSymbol {
                file: doc.file.clone(),
                name: definition.name.clone(),
                kind: 12,
                range: definition.name_range,
            },
        );
    }
    let mut visited = BTreeSet::new();
    visited.insert(doc.file.clone());
    let mut queue: Vec<(wxml_types::FileId, wxml_types::TemplateSemantics)> =
        vec![(doc.file.clone(), semantics.clone())];
    let mut budget = 64_usize;
    while let Some((file, semantics)) = queue.pop() {
        for import in &semantics.imports {
            if budget == 0 {
                *complete = false;
                return;
            }
            budget = budget.saturating_sub(1);
            let Some(path) = wxml_types::resolve_static_specifier(file.path(), &import.src) else {
                *complete = false;
                continue;
            };
            let Some(imported) = imports.load(server, file.project(), &path) else {
                *complete = false;
                if imports.is_exhausted() {
                    return;
                }
                continue;
            };
            if !visited.insert(imported.file.clone()) {
                continue;
            }
            texts.insert(imported.file.clone(), imported.text.clone());
            for definition in imported.semantics.distinct_definitions() {
                push_raw(
                    raw,
                    seen,
                    RawSymbol {
                        file: imported.file.clone(),
                        name: definition.name.clone(),
                        kind: 12,
                        range: definition.name_range,
                    },
                );
            }
            queue.push((imported.file.clone(), imported.semantics.clone()));
        }
    }
}

fn push_raw(
    raw: &mut Vec<RawSymbol>,
    seen: &mut BTreeSet<(wxml_types::FileId, String, u32, u32)>,
    symbol: RawSymbol,
) {
    if seen.insert((
        symbol.file.clone(),
        symbol.name.clone(),
        symbol.range.start(),
        symbol.range.end(),
    )) {
        raw.push(symbol);
    }
}

fn matches_query(name: &str, needle: &str) -> bool {
    needle.is_empty() || name.to_lowercase().contains(needle)
}

fn first_tag_range(doc: &super::facts::QueryDoc, tag: &str) -> Option<TextRange> {
    let mut found = None;
    for_each_element(doc.tree(), &mut |element| {
        if found.is_none() && element.name() == Some(tag) {
            found = element.name_range();
        }
    });
    found
}

/// Builds `textDocument/inlayHint` for one view.
pub(crate) fn inlay_hints(
    server: &Server,
    document: &Document,
    range: Option<(Position, Position)>,
) -> Value {
    if !server.semantic_tools.inlay_hints {
        return json!([]);
    }
    let Some(facts) = view_facts(server, document) else {
        return json!([]);
    };
    let (Some(start), Some(end)) = (
        range.and_then(|range| document.offset_at(range.0)),
        range.and_then(|range| document.offset_at(range.1)),
    ) else {
        return json!([]);
    };
    let start = start.min(end);
    let end = start.max(end);
    let mut raw: Vec<(u32, String)> = Vec::new();
    for_each_element(facts.doc.tree(), &mut |element| {
        if raw.len() >= MAX_INLAY_HINTS {
            return;
        }
        let Some(name) = element.name() else {
            return;
        };
        let Some(component) = facts
            .components
            .iter()
            .find(|component| component.name == name)
        else {
            return;
        };
        if !component.property_facts.complete {
            return;
        }
        for attribute in element.attributes() {
            if raw.len() >= MAX_INLAY_HINTS {
                return;
            }
            let Some(attribute_name) = attribute.name() else {
                continue;
            };
            let attribute_range = attribute.text_range();
            if attribute_range.end() < start || attribute_range.start() > end {
                continue;
            }
            if crate::completion::catalog::is_event_attribute(attribute_name) {
                let Some(event) = facts.analysis.events.iter().find(|event| {
                    event.attribute_range == SourceRange::new(attribute.text_range())
                }) else {
                    continue;
                };
                if event.handler_range.end() < start || event.handler_range.start() > end {
                    continue;
                }
                raw.push((attribute_range.end(), ": event".to_owned()));
                continue;
            }
            if attribute_name.starts_with("wx:")
                || matches!(attribute_name, "class" | "id" | "style")
            {
                continue;
            }
            let Some(property) = component.property_facts.properties.iter().find(|property| {
                attribute_name == property.name
                    || (attribute_name.contains('-')
                        && attribute_name == super::target::kebab_case(&property.name))
            }) else {
                continue;
            };
            if property.type_text.trim().is_empty() {
                continue;
            }
            raw.push((
                attribute_range.end(),
                format!(": {}", property.type_text.trim()),
            ));
        }
    });
    raw.sort_by_key(|(offset, _)| *offset);
    let mut cursor = Utf16Cursor::new(document.text());
    let mut hints = Vec::with_capacity(raw.len());
    for (offset, label) in raw {
        let Some(position) = cursor.position(offset) else {
            crate::server::log(format_args!(
                "inlay hints stopped at the position scan budget"
            ));
            break;
        };
        hints.push(json!({
            "position": crate::handlers::position_value(position),
            "label": label,
            "kind": 1,
            "paddingLeft": false,
            "paddingRight": false,
        }));
    }
    json!(hints)
}

/// Builds `textDocument/signatureHelp` from static WXS facts.
pub(crate) fn signature_help(server: &Server, document: &Document, position: Position) -> Value {
    let Some(offset) = document.offset_at(position) else {
        return Value::Null;
    };
    let access = crate::static_semantics::host_access_in(server, document.file().project());
    let path = document.file().path();
    let Some(signature) =
        crate::static_semantics::wxs_call_signature(path, document.text(), offset, &access)
    else {
        return Value::Null;
    };
    let parameters: Vec<Value> = signature
        .parameters
        .iter()
        .map(|parameter| json!({"label": parameter}))
        .collect();
    let mut entry = json!({
        "label": signature.label,
        "parameters": parameters,
    });
    if let Some(documentation) = signature.documentation {
        if let Some(object) = entry.as_object_mut() {
            object.insert(
                "documentation".to_owned(),
                json!({"kind": "markdown", "value": documentation}),
            );
        }
    }
    let active = signature
        .active_parameter
        .min(u32::try_from(parameters.len().saturating_sub(1)).unwrap_or(0));
    json!({
        "signatures": [entry],
        "activeSignature": 0,
        "activeParameter": active,
    })
}

/// Builds reference-count code lenses for templates and component aliases.
#[allow(clippy::too_many_lines)] // one bounded project view pass
pub(crate) fn code_lens(server: &Server, document: &Document) -> Value {
    if !server.semantic_tools.code_lens {
        return json!([]);
    }
    let Some(facts) = view_facts(server, document) else {
        return json!([]);
    };
    let views = project_views(server, &facts);
    let mut imports = ImportCache::default();
    let mut raw: Vec<(u32, String, Value)> = Vec::new();
    let mut seen_templates = BTreeSet::new();
    let mut template_count = 0_usize;
    let template_truncated = facts.semantics.distinct_definitions().count() > MAX_TEMPLATE_LENSES;
    for definition in facts.semantics.distinct_definitions() {
        if !seen_templates.insert(definition.name.clone()) {
            continue;
        }
        if template_count >= MAX_TEMPLATE_LENSES {
            break;
        }
        template_count += 1;
        let collected = collect_template_refs_with_views(
            server,
            &facts,
            &views,
            &mut imports,
            &definition.name,
        );
        let count = collected.calls.len();
        let mut title = if collected.complete {
            format!("{count} template call{}", plural(count))
        } else {
            format!("{count}+ indexed template call{}", plural(count))
        };
        if template_truncated {
            title.push_str(" (lens list truncated)");
        }
        raw.push((
            definition.name_range.start(),
            title,
            json!({
                "kind": "template",
                "name": definition.name,
                "count": count,
                "complete": collected.complete && !template_truncated,
            }),
        ));
    }
    let mut seen_components = BTreeSet::new();
    let mut component_count = 0_usize;
    for component in &facts.components {
        if !seen_components.insert(component.name.clone()) {
            continue;
        }
        if component_count >= MAX_COMPONENT_LENSES {
            break;
        }
        component_count += 1;
        let mut file_count = 0_usize;
        let mut first_in_file = None;
        let mut first_include = None;
        for_each_element(facts.doc.tree(), &mut |element| {
            if element.name() == Some("include") && first_include.is_none() {
                first_include = element.name_range();
            }
            if element.name() == Some(component.name.as_str()) {
                file_count += 1;
                if first_in_file.is_none() {
                    first_in_file = element.name_range();
                }
            }
        });
        let mut included_count = 0_usize;
        for include in &facts.includes {
            for_each_element(include.tree(), &mut |element| {
                if element.name() == Some(component.name.as_str()) {
                    included_count += 1;
                }
            });
        }
        let count = file_count.saturating_add(included_count);
        if count == 0 {
            continue;
        }
        // A lens range must be a position in the current document. When the
        // component only appears through includes, anchor the lens at the
        // current file's `<include>` element instead of an include offset.
        let Some(anchor) = first_in_file.or(first_include) else {
            continue;
        };
        let incomplete = !facts.includes_complete;
        let suffix = if incomplete { "+ indexed" } else { "" };
        let location = match (file_count, included_count) {
            (0, _) => "in included files".to_owned(),
            (_, 0) => "in this file".to_owned(),
            _ => format!("({included_count} in included files)"),
        };
        let title = format!("{count}{suffix} component use{} {location}", plural(count));
        raw.push((
            anchor.start(),
            title,
            json!({
                "kind": "component",
                "name": component.name,
                "count": count,
                "fileCount": file_count,
                "includedCount": included_count,
                "complete": !incomplete,
            }),
        ));
    }
    raw.sort_by_key(|(offset, _, _)| *offset);
    let mut cursor = Utf16Cursor::new(document.text());
    let mut lenses = Vec::with_capacity(raw.len());
    for (offset, title, data) in raw {
        let Some(position) = cursor.position(offset) else {
            crate::server::log(format_args!(
                "code lenses stopped at the position scan budget"
            ));
            break;
        };
        let start = crate::handlers::position_value(position);
        let command = json!({
            "title": title,
            "command": "wxml.references",
            "arguments": [{
                "uri": document.uri(),
                "position": start,
                "includeDeclaration": true,
            }],
        });
        lenses.push(json!({
            "range": {"start": start, "end": start},
            "data": data,
            "command": command,
        }));
    }
    json!(lenses)
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
