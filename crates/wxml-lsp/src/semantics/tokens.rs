//! `textDocument/semanticTokens/full`.
//!
//! Tokens come from the lossless syntax tree and T's static WXS facts. The
//! legend is fixed in `initialize`; positions are UTF-16 and the encoder never
//! emits overlapping or multiline tokens. A token budget truncates the tail
//! instead of failing the request.

use serde_json::{Value, json};
use wxml_syntax::{AstNode, ExprKind, NodeKind};
use wxml_text::TextRange;

use super::facts::{ViewFacts, scan_chains, view_facts};
use crate::server::log;
use crate::workspace::Document;

/// Token type names in legend order.
const TOKEN_TYPES: [&str; 23] = [
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "wxs",
];

const NAMESPACE: u8 = 0;
const VARIABLE: u8 = 8;
const PROPERTY: u8 = 9;
const EVENT: u8 = 11;
const FUNCTION: u8 = 12;
const KEYWORD: u8 = 15;
const COMMENT: u8 = 17;
const STRING: u8 = 18;
const NUMBER: u8 = 19;
const WXS: u8 = 22;

/// Returns the semantic token legend value for `initialize`.
pub(crate) fn legend() -> Value {
    json!({
        "tokenTypes": TOKEN_TYPES,
        "tokenModifiers": [],
    })
}

/// Encodes semantic tokens for one open template.
pub(crate) fn semantic_tokens(server: &crate::server::Server, document: &Document) -> Value {
    let Some(facts) = view_facts(server, document) else {
        log(format_args!(
            "semantic tokens skipped: the template could not be parsed completely"
        ));
        return json!({"data": []});
    };
    let mut raw = RawTokens::default();
    collect_structure(&facts, &mut raw);
    collect_mustaches(&facts, &mut raw);
    collect_wxs(&facts, &mut raw);
    let data = encode(&facts, raw.tokens);
    json!({"data": data})
}

#[derive(Default)]
struct RawTokens {
    tokens: Vec<(u32, u32, u8)>,
}

impl RawTokens {
    fn push(&mut self, range: TextRange, token_type: u8) {
        if range.is_empty() {
            return;
        }
        self.tokens.push((range.start(), range.end(), token_type));
    }
}

fn collect_structure(facts: &ViewFacts, raw: &mut RawTokens) {
    let tree = facts.doc.tree();
    for child in tree.document().children() {
        walk_node(child, raw);
    }
    for (_, node) in tree.nodes() {
        if node.kind() == NodeKind::Comment {
            let range = node.text_range();
            let text = tree.text();
            let start = usize::try_from(range.start()).unwrap_or(usize::MAX);
            let end = usize::try_from(range.end()).unwrap_or(usize::MAX);
            let Some(slice) = text.get(start..end) else {
                continue;
            };
            let mut offset = range.start();
            for line in slice.split_inclusive('\n') {
                let content = line.trim_end_matches('\n');
                if !content.is_empty()
                    && let Ok(length) = u32::try_from(content.len())
                    && let Ok(line_range) = TextRange::new(offset, offset.saturating_add(length))
                {
                    raw.push(line_range, COMMENT);
                }
                offset = offset.saturating_add(u32::try_from(line.len()).unwrap_or(0));
            }
        }
    }
}

fn walk_node(node: AstNode<'_>, raw: &mut RawTokens) {
    let AstNode::Element(element) = node else {
        return;
    };
    if let Some(range) = element.name_range() {
        raw.push(range, NAMESPACE);
    }
    for attribute in element.attributes() {
        let Some(name) = attribute.name() else {
            continue;
        };
        if let Some(range) = attribute.name_range() {
            let token = if name.starts_with("wx:") {
                KEYWORD
            } else if crate::completion::catalog::is_event_attribute(name) {
                EVENT
            } else {
                PROPERTY
            };
            raw.push(range, token);
        }
        let Some(value) = attribute.value() else {
            continue;
        };
        let function_value = element.name() == Some("template") && name == "name";
        for child in value.children() {
            if let AstNode::Text(text) = child {
                let range = text.text_range();
                raw.push(range, if function_value { FUNCTION } else { STRING });
            }
        }
    }
    for child in element.children() {
        walk_node(child, raw);
    }
}

fn collect_mustaches(facts: &ViewFacts, raw: &mut RawTokens) {
    let tree = facts.doc.tree();
    for (id, node) in tree.nodes() {
        if node.kind() != NodeKind::MustacheExpr {
            continue;
        }
        let Some(expr) = tree.expr(id) else {
            continue;
        };
        let mut property_ranges = Vec::new();
        let scan = scan_chains(expr);
        for chain in &scan.chains {
            for (_, range) in chain.segments.iter().skip(1) {
                property_ranges.push(range.range());
            }
        }
        for node in expr.walk() {
            match node.kind() {
                ExprKind::Identifier { name } => {
                    let token = if property_ranges.contains(name) {
                        PROPERTY
                    } else {
                        VARIABLE
                    };
                    raw.push(*name, token);
                }
                ExprKind::String { content, .. } => raw.push(*content, STRING),
                ExprKind::Number => raw.push(node.range(), NUMBER),
                ExprKind::Boolean(_) | ExprKind::Null => raw.push(node.range(), KEYWORD),
                _ => {}
            }
        }
    }
}

fn collect_wxs(facts: &ViewFacts, raw: &mut RawTokens) {
    let text = facts.doc.text();
    for block in &facts.semantics.wxs {
        let Some(body) = block.body_range else {
            continue;
        };
        let start = usize::try_from(body.start()).unwrap_or(usize::MAX);
        let end = usize::try_from(body.end()).unwrap_or(usize::MAX);
        let Some(body_text) = text.get(start..end) else {
            continue;
        };
        let mut line_offset = 0_usize;
        for line in body_text.split_inclusive('\n') {
            let line_start = start.saturating_add(line_offset);
            let trimmed_start = line.len().saturating_sub(line.trim_start().len());
            let trimmed = line.trim();
            if !trimmed.is_empty()
                && let (Ok(token_start), Ok(token_len)) = (
                    u32::try_from(line_start.saturating_add(trimmed_start)),
                    u32::try_from(trimmed.len()),
                )
                && let Ok(range) =
                    TextRange::new(token_start, token_start.saturating_add(token_len))
            {
                raw.push(range, WXS);
            }
            line_offset = line_offset.saturating_add(line.len());
        }
    }
}

fn encode(facts: &ViewFacts, mut tokens: Vec<(u32, u32, u8)>) -> Vec<u64> {
    tokens.sort_by_key(|(start, end, _)| (*start, *end));
    tokens.dedup();
    let mut cursor = super::position::Utf16Cursor::new(facts.doc.text());
    let mut data = Vec::with_capacity(tokens.len().saturating_mul(5));
    let mut last_line = 0_u32;
    let mut last_character = 0_u32;
    let mut last_end = 0_u32;
    for (start, end, token_type) in tokens {
        if start < last_end {
            continue;
        }
        let Ok(start_usize) = usize::try_from(start) else {
            continue;
        };
        let Ok(end_usize) = usize::try_from(end) else {
            continue;
        };
        let (Some(start_position), Some(end_position)) = (
            cursor.position(u32::try_from(start_usize).unwrap_or(u32::MAX)),
            cursor.position(u32::try_from(end_usize).unwrap_or(u32::MAX)),
        ) else {
            log(format_args!("semantic token position is not mappable"));
            break;
        };
        if end_position.line != start_position.line {
            continue;
        }
        let length = end_position
            .character
            .saturating_sub(start_position.character);
        if length == 0 {
            continue;
        }
        let delta_line = start_position.line.saturating_sub(last_line);
        let delta_start = if delta_line == 0 {
            start_position.character.saturating_sub(last_character)
        } else {
            start_position.character
        };
        data.extend([
            u64::from(delta_line),
            u64::from(delta_start),
            u64::from(length),
            u64::from(token_type),
            0,
        ]);
        last_line = start_position.line;
        last_character = start_position.character;
        last_end = end;
    }
    data
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn legend_is_stable_and_ordered() {
        let value = legend();
        assert_eq!(value["tokenTypes"][0], "namespace");
        assert_eq!(value["tokenTypes"][NAMESPACE as usize], "namespace");
        assert_eq!(value["tokenTypes"][VARIABLE as usize], "variable");
        assert_eq!(value["tokenTypes"][WXS as usize], "wxs");
        assert_eq!(value["tokenModifiers"], json!([]));
    }
}
