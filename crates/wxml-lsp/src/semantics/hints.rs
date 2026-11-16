//! Workspace symbols, inlay hints, signature help and code lens.
//!
//! Every value is derived from existing facts: template semantics from T,
//! component property declarations from C, the static WXS summary from T and
//! the lossless tree from E. No method here guesses a type or a reference.

use serde_json::{Value, json};
use wxml_text::{Position, SourceRange};

use super::facts::{parse_doc, view_facts};
use super::target::for_each_element;
use crate::server::Server;
use crate::workspace::Document;

/// Maximum symbols returned by `workspace/symbol`.
const MAX_WORKSPACE_SYMBOLS: usize = 512;
/// Maximum inlay hints returned for one document.
const MAX_INLAY_HINTS: usize = 1_000;

/// Returns `workspace/symbol` entries.
pub(crate) fn workspace_symbols(server: &Server, query: &str) -> Value {
    let project = server.host.project_id();
    let Ok(probe) = wxml_types::FileId::new(project, "__workspace_symbol__") else {
        return json!([]);
    };
    let Some(templates) = server.host.view_templates(&probe) else {
        return json!([]);
    };
    let needle = query.to_lowercase();
    let mut symbols = Vec::new();
    for template in templates.templates {
        let Some(doc) = parse_doc(template.file.clone(), &template.text) else {
            continue;
        };
        let semantics = wxml_types::analyze_template_semantics(doc.tree());
        for definition in semantics.distinct_definitions() {
            if symbols.len() >= MAX_WORKSPACE_SYMBOLS {
                break;
            }
            if !matches_query(&definition.name, &needle) {
                continue;
            }
            let Some(range) = crate::handlers::line_index_range_value(
                &wxml_text::LineIndex::new(template.text.clone()),
                definition.name_range,
            ) else {
                continue;
            };
            let Some(uri) = server.host.uri_for(&template.file) else {
                continue;
            };
            symbols.push(json!({
                "name": definition.name,
                "kind": 12,
                "location": {"uri": uri, "range": range},
            }));
        }
        if symbols.len() >= MAX_WORKSPACE_SYMBOLS {
            break;
        }
        let components = server.host.view_components_for(&template.file);
        let mut seen = std::collections::HashSet::new();
        for component in components {
            if !seen.insert(component.name.clone()) || !matches_query(&component.name, &needle) {
                continue;
            }
            let Some(range) = first_tag_range(&doc, &component.name) else {
                continue;
            };
            let Some(range) = crate::handlers::line_index_range_value(
                &wxml_text::LineIndex::new(template.text.clone()),
                SourceRange::new(range),
            ) else {
                continue;
            };
            let Some(uri) = server.host.uri_for(&template.file) else {
                continue;
            };
            symbols.push(json!({
                "name": component.name,
                "kind": 5,
                "location": {"uri": uri, "range": range},
            }));
            if symbols.len() >= MAX_WORKSPACE_SYMBOLS {
                break;
            }
        }
    }
    json!(symbols)
}

fn matches_query(name: &str, needle: &str) -> bool {
    needle.is_empty() || name.to_lowercase().contains(needle)
}

fn first_tag_range(doc: &super::facts::QueryDoc, tag: &str) -> Option<wxml_text::TextRange> {
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
    let mut hints = Vec::new();
    for_each_element(facts.doc.tree(), &mut |element| {
        if hints.len() >= MAX_INLAY_HINTS {
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
        for attribute in element.attributes() {
            if hints.len() >= MAX_INLAY_HINTS {
                return;
            }
            let Some(attribute_name) = attribute.name() else {
                continue;
            };
            if crate::completion::catalog::is_event_attribute(attribute_name) {
                let Some(event) = facts.analysis.events.iter().find(|event| {
                    event.attribute_range == SourceRange::new(attribute.text_range())
                }) else {
                    continue;
                };
                if event.handler_range.end() < start || event.handler_range.start() > end {
                    continue;
                }
                let Some(position) = document
                    .snapshot()
                    .line_index()
                    .position(usize::try_from(event.handler_range.end()).unwrap_or(usize::MAX))
                else {
                    continue;
                };
                hints.push(json!({
                    "position": crate::handlers::position_value(position),
                    "label": ": event",
                    "kind": 1,
                    "paddingLeft": false,
                    "paddingRight": false,
                }));
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
            let attribute_range = attribute.text_range();
            if attribute_range.end() < start || attribute_range.start() > end {
                continue;
            }
            let Some(position) = document
                .snapshot()
                .line_index()
                .position(usize::try_from(attribute_range.end()).unwrap_or(usize::MAX))
            else {
                continue;
            };
            hints.push(json!({
                "position": crate::handlers::position_value(position),
                "label": format!(": {}", property.type_text.trim()),
                "kind": 1,
                "paddingLeft": false,
                "paddingRight": false,
            }));
        }
    });
    json!(hints)
}

/// Builds `textDocument/signatureHelp` from static WXS facts.
pub(crate) fn signature_help(server: &Server, document: &Document, position: Position) -> Value {
    let Some(offset) = document.offset_at(position) else {
        return Value::Null;
    };
    let access = crate::static_semantics::host_access(server);
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
pub(crate) fn code_lens(server: &Server, document: &Document) -> Value {
    if !server.semantic_tools.code_lens {
        return json!([]);
    }
    let Some(facts) = view_facts(server, document) else {
        return json!([]);
    };
    let mut lenses = Vec::new();
    let mut seen_templates = std::collections::HashSet::new();
    for definition in facts.semantics.distinct_definitions() {
        if !seen_templates.insert(definition.name.clone()) {
            continue;
        }
        let collected = super::target::collect_template_refs(server, &facts, &definition.name);
        let count = collected.calls.len();
        let Some(range) = crate::handlers::source_range_value(document, definition.name_range)
        else {
            continue;
        };
        let title = if collected.complete {
            format!("{count} template call{}", plural(count))
        } else {
            format!("{count}+ indexed template call{}", plural(count))
        };
        let command = read_only_command(document, &range, &title);
        lenses.push(json!({"range": range, "data": {"kind": "template", "name": definition.name, "count": count, "complete": collected.complete}, "command": command}));
    }
    let mut seen_components = std::collections::HashSet::new();
    for component in &facts.components {
        if !seen_components.insert(component.name.clone()) {
            continue;
        }
        let mut count = 0_usize;
        let mut first = None;
        for_each_element(facts.doc.tree(), &mut |element| {
            if element.name() == Some(component.name.as_str()) {
                count += 1;
                if first.is_none() {
                    first = element.name_range();
                }
            }
        });
        if count == 0 {
            continue;
        }
        let Some(range) = first.and_then(|range| {
            crate::handlers::source_range_value(document, SourceRange::new(range))
        }) else {
            continue;
        };
        let incomplete = !facts.includes.is_empty() || !facts.includes_complete;
        let title = if incomplete {
            format!("{count}+ indexed component use{}", plural(count))
        } else {
            format!("{count} component use{}", plural(count))
        };
        let command = read_only_command(document, &range, &title);
        lenses.push(json!({
            "range": range,
            "data": {"kind": "component", "name": component.name, "count": count, "complete": true},
            "command": command,
        }));
    }
    json!(lenses)
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Builds a read-only `wxml.references` command for one lens title.
fn read_only_command(document: &Document, range: &Value, title: &str) -> Value {
    let Some(start) = range.get("start") else {
        return json!({"title": title, "command": "", "arguments": []});
    };
    json!({
        "title": title,
        "command": "wxml.references",
        "arguments": [{
            "uri": document.uri(),
            "position": start,
            "includeDeclaration": true,
        }],
    })
}
