//! Structural facts and handlers: linked tag names, folding, selection ranges
//! and document symbols.
//!
//! Every function in this module works on the lossless syntax tree from
//! `wxml-syntax`; the parser is not reimplemented and no regular expressions
//! are used to rediscover structure. Requests are answered from the open
//! document snapshot, so these features keep working when the Node type
//! backend is unavailable.

use serde_json::{Value, json};
use wxml_syntax::parser::{NodeId, NodeKind, SyntaxTree};
use wxml_text::{LineIndex, SourceRange, TextRange};

use crate::jsonrpc::{self, Outgoing};
use crate::server::{Server, log};

/// Largest number of folding ranges returned for one document.
pub(crate) const MAX_FOLDING_RANGES: usize = 2_000;

/// Largest number of positions accepted by one `selectionRange` request.
pub(crate) const MAX_SELECTION_POSITIONS: usize = 32;

/// Largest ancestor chain returned for one selection position.
pub(crate) const MAX_SELECTION_DEPTH: usize = 32;

/// Largest number of document symbols returned for one document.
pub(crate) const MAX_DOCUMENT_SYMBOLS: usize = 2_000;

/// Largest element nesting depth represented in document symbols.
pub(crate) const MAX_DOCUMENT_SYMBOL_DEPTH: usize = 32;

/// One folding range in zero-based lines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Fold {
    /// First folded line.
    pub(crate) start_line: u32,
    /// Last folded line.
    pub(crate) end_line: u32,
    /// LSP folding kind (`comment` or `region`).
    pub(crate) kind: &'static str,
}

/// One document symbol with nested children.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentSymbol {
    /// Tag name shown in the outline.
    pub(crate) name: String,
    /// Optional `#id`/`.class` summary.
    pub(crate) detail: Option<String>,
    /// LSP `SymbolKind` (5 = Class, the HTML-like outline category).
    pub(crate) kind: u8,
    /// Whole element range.
    pub(crate) range: TextRange,
    /// Tag-name range.
    pub(crate) selection: TextRange,
    /// Nested elements in source order.
    pub(crate) children: Vec<Self>,
}

/// Converts a byte range into a line-only folding range.
fn add_fold(
    text: &str,
    line_index: &LineIndex,
    folds: &mut Vec<Fold>,
    range: TextRange,
    kind: &'static str,
) {
    if range.is_empty() || folds.len() >= MAX_FOLDING_RANGES {
        return;
    }
    let Ok(start) = usize::try_from(range.start()) else {
        return;
    };
    let Ok(end) = usize::try_from(range.end()) else {
        return;
    };
    let Some(start_position) = line_index.position(start) else {
        return;
    };
    let Some(end_position) = line_index.position(end) else {
        return;
    };
    let mut end_line = end_position.line;
    if end_line > start_position.line
        && end_position.character == 0
        && text.as_bytes().get(end.saturating_sub(1)) == Some(&b'\n')
    {
        end_line = end_line.saturating_sub(1);
    }
    if start_position.line < end_line {
        folds.push(Fold {
            start_line: start_position.line,
            end_line,
            kind,
        });
    }
}

/// Returns folding ranges for elements, multiline start tags, comments and
/// whole WXS elements.
pub(crate) fn folding_ranges(text: &str, line_index: &LineIndex) -> Vec<Fold> {
    let parsed = wxml_syntax::parse(text);
    if parsed.is_truncated() {
        log(format_args!(
            "folding skipped: parse was truncated by a resource budget"
        ));
        return Vec::new();
    }
    let tree = &parsed.tree;
    let mut folds = Vec::new();
    for id in tree.descendants(tree.root()) {
        match tree.kind(id) {
            Some(NodeKind::Element) => {
                let Some(element) = tree.ast(id) else {
                    continue;
                };
                let Some(element) = as_element(element) else {
                    continue;
                };
                if !element.is_closed() {
                    continue;
                }
                add_fold(text, line_index, &mut folds, element.text_range(), "region");
                if let Some(start_tag) = start_tag_id(tree, id) {
                    if let Some(range) = tree.text_range(start_tag) {
                        add_fold(text, line_index, &mut folds, range, "region");
                    }
                }
            }
            Some(NodeKind::Comment) => {
                if let Some(range) = tree.text_range(id) {
                    add_fold(text, line_index, &mut folds, range, "comment");
                }
            }
            _ => {}
        }
        if folds.len() >= MAX_FOLDING_RANGES {
            break;
        }
    }
    folds.sort_by(|left, right| {
        left.start_line
            .cmp(&right.start_line)
            .then_with(|| right.end_line.cmp(&left.end_line))
    });
    folds.dedup_by(|left, right| {
        left.start_line == right.start_line && left.end_line == right.end_line
    });
    folds.truncate(MAX_FOLDING_RANGES);
    folds
}

/// Unwraps an AST node into an element, if it is one.
const fn as_element(node: wxml_syntax::AstNode<'_>) -> Option<wxml_syntax::Element<'_>> {
    match node {
        wxml_syntax::AstNode::Element(element) => Some(element),
        _ => None,
    }
}

/// Returns the start-tag child of an element.
fn start_tag_id(tree: &SyntaxTree, element: NodeId) -> Option<NodeId> {
    tree.children(element)
        .iter()
        .copied()
        .find(|id| tree.kind(*id) == Some(NodeKind::StartTag))
}

/// Returns the end-tag child of an element.
fn end_tag_id(tree: &SyntaxTree, element: NodeId) -> Option<NodeId> {
    tree.children(element)
        .iter()
        .copied()
        .find(|id| tree.kind(*id) == Some(NodeKind::EndTag))
}

/// Returns the paired tag-name ranges for linked editing at `offset`.
///
/// The pair always belongs to one element, so nested elements with the same
/// name and unclosed tags never borrow a different sibling's name.
pub(crate) fn linked_editing_range(text: &str, offset: u32) -> Option<(TextRange, TextRange)> {
    let parsed = wxml_syntax::parse(text);
    if parsed.is_truncated() {
        return None;
    }
    let tree = &parsed.tree;
    for id in tree.descendants(tree.root()) {
        let kind = tree.kind(id)?;
        if !matches!(kind, NodeKind::StartTag | NodeKind::EndTag) {
            continue;
        }
        let Some(name) = tree.name_range(id) else {
            continue;
        };
        if offset < name.start() || offset > name.end() {
            continue;
        }
        let element = tree.parent(id)?;
        if tree.kind(element) != Some(NodeKind::Element) {
            continue;
        }
        let other = if kind == NodeKind::StartTag {
            end_tag_id(tree, element)?
        } else {
            start_tag_id(tree, element)?
        };
        let other_name = tree.name_range(other)?;
        let start_name = tree.name(id)?;
        let end_name = tree.name(other)?;
        if start_name != end_name {
            continue;
        }
        if name.start() <= other_name.start() {
            return Some((name, other_name));
        }
        return Some((other_name, name));
    }
    None
}

/// Returns the selection ancestor chain for `offset`, innermost first.
///
/// The chain is strict: equal consecutive ranges are collapsed, so an element
/// that covers the whole document does not duplicate the document range.
pub(crate) fn selection_chain(text: &str, offset: u32) -> Vec<TextRange> {
    let parsed = wxml_syntax::parse(text);
    if parsed.is_truncated() {
        return Vec::new();
    }
    let Ok(offset_usize) = usize::try_from(offset) else {
        return Vec::new();
    };
    if offset_usize > text.len() || !text.is_char_boundary(offset_usize) {
        return Vec::new();
    }
    let tree = &parsed.tree;
    let mut interior: Option<(u32, NodeId)> = None;
    let mut ending: Option<(u32, NodeId)> = None;
    for (id, node) in tree.nodes() {
        if node.kind() == NodeKind::Document {
            continue;
        }
        let range = node.text_range();
        if range.start() <= offset && offset < range.end() {
            if interior.is_none_or(|(index, _)| id.index() < index) {
                interior = Some((id.index(), id));
            }
        } else if offset == range.end() && ending.is_none_or(|(index, _)| id.index() < index) {
            ending = Some((id.index(), id));
        }
    }
    let (_, mut current) = interior.or(ending).unwrap_or_else(|| (0, tree.root()));
    let mut chain: Vec<TextRange> = Vec::new();
    loop {
        let kind = tree.kind(current);
        if !matches!(kind, Some(NodeKind::StartTag | NodeKind::EndTag)) {
            if let Some(range) = tree.text_range(current) {
                let keep = chain.last().is_none_or(|inner| {
                    range != *inner && range.start() <= inner.start() && range.end() >= inner.end()
                });
                if keep {
                    if chain.len() >= MAX_SELECTION_DEPTH {
                        break;
                    }
                    chain.push(range);
                }
            }
        }
        let Some(parent) = tree.parent(current) else {
            break;
        };
        current = parent;
    }
    chain
}

/// Builds hierarchical document symbols for every top-level element.
pub(crate) fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let parsed = wxml_syntax::parse(text);
    if parsed.is_truncated() {
        log(format_args!(
            "document symbols skipped: parse was truncated by a resource budget"
        ));
        return Vec::new();
    }
    let tree = &parsed.tree;
    let mut budget = MAX_DOCUMENT_SYMBOLS;
    let mut symbols = Vec::new();
    for node in tree.document().children() {
        let wxml_syntax::AstNode::Element(element) = node else {
            continue;
        };
        if let Some(symbol) = element_symbol(element, 0, &mut budget) {
            symbols.push(symbol);
        }
        if budget == 0 {
            break;
        }
    }
    symbols
}

/// Builds one symbol and its nested elements.
fn element_symbol(
    element: wxml_syntax::Element<'_>,
    depth: usize,
    budget: &mut usize,
) -> Option<DocumentSymbol> {
    if *budget == 0 {
        return None;
    }
    let name = element.name()?.to_owned();
    if name.is_empty() {
        return None;
    }
    let selection = element.name_range()?;
    let range = element.text_range();
    if range.start() > selection.start() || range.end() < selection.end() {
        return None;
    }
    *budget = budget.saturating_sub(1);
    let detail = element_detail(element);
    let mut children = Vec::new();
    if depth < MAX_DOCUMENT_SYMBOL_DEPTH {
        for node in element.children() {
            let wxml_syntax::AstNode::Element(child) = node else {
                continue;
            };
            if let Some(symbol) = element_symbol(child, depth.saturating_add(1), budget) {
                children.push(symbol);
            }
            if *budget == 0 {
                break;
            }
        }
    }
    Some(DocumentSymbol {
        name,
        detail,
        kind: 5,
        range,
        selection,
        children,
    })
}

/// Summarizes literal `id` and `class` attributes for the outline.
fn element_detail(element: wxml_syntax::Element<'_>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(id) = element
        .attribute("id")
        .and_then(wxml_syntax::Attribute::value)
        .and_then(wxml_syntax::AttributeValue::literal_text)
    {
        if !id.is_empty() {
            parts.push(format!("#{id}"));
        }
    }
    if let Some(classes) = element
        .attribute("class")
        .and_then(wxml_syntax::Attribute::value)
        .and_then(wxml_syntax::AttributeValue::literal_text)
    {
        let classes: Vec<String> = classes
            .split_whitespace()
            .map(|class| format!(".{class}"))
            .collect();
        if !classes.is_empty() {
            parts.push(classes.join(" "));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(crate::handlers::sanitized_reason(&parts.join(" ")))
    }
}

/// Reads the `textDocument.uri` of a document-only request.
fn document_uri(params: &Value) -> Option<&str> {
    let document = crate::handlers::text_document(params)?;
    crate::handlers::string(document, "uri")
}

/// Serializes a hierarchical symbol.
fn hierarchical_symbol(
    document: &crate::workspace::Document,
    symbol: &DocumentSymbol,
) -> Option<Value> {
    let range = crate::handlers::source_range_value(document, SourceRange::new(symbol.range))?;
    let selection_range =
        crate::handlers::source_range_value(document, SourceRange::new(symbol.selection))?;
    let mut value = json!({
        "name": symbol.name,
        "kind": symbol.kind,
        "range": range,
        "selectionRange": selection_range,
    });
    let object = value.as_object_mut()?;
    if let Some(detail) = symbol.detail.as_ref() {
        object.insert("detail".to_owned(), Value::String(detail.clone()));
    }
    if !symbol.children.is_empty() {
        let children: Vec<Value> = symbol
            .children
            .iter()
            .filter_map(|child| hierarchical_symbol(document, child))
            .collect();
        object.insert("children".to_owned(), Value::Array(children));
    }
    Some(value)
}

/// Serializes one flat `SymbolInformation` and its descendants.
fn flatten_symbol(
    document: &crate::workspace::Document,
    uri: &str,
    symbol: &DocumentSymbol,
    container: Option<&str>,
    output: &mut Vec<Value>,
) {
    if output.len() >= MAX_DOCUMENT_SYMBOLS {
        return;
    }
    let Some(range) = crate::handlers::source_range_value(document, SourceRange::new(symbol.range))
    else {
        return;
    };
    let mut value = json!({
        "name": symbol.name,
        "kind": symbol.kind,
        "location": {"uri": uri, "range": range},
    });
    if let Some(container) = container {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "containerName".to_owned(),
                Value::String(container.to_owned()),
            );
        }
    }
    output.push(value);
    for child in &symbol.children {
        flatten_symbol(document, uri, child, Some(&symbol.name), output);
        if output.len() >= MAX_DOCUMENT_SYMBOLS {
            break;
        }
    }
}

impl Server {
    /// Handles `textDocument/linkedEditingRange`.
    pub(crate) fn linked_editing(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
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
        let Some(offset) = document.offset_at(position) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some((first, second)) = linked_editing_range(document.text(), offset) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let first = crate::handlers::source_range_value(document, SourceRange::new(first));
        let second = crate::handlers::source_range_value(document, SourceRange::new(second));
        let (Some(first), Some(second)) = (first, second) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        vec![Outgoing::success(id, json!({"ranges": [first, second]}))]
    }

    /// Handles `textDocument/foldingRange`.
    pub(crate) fn folding_range(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(uri) = document_uri(&params) else {
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
        let folds = folding_ranges(document.text(), document.snapshot().line_index());
        let values: Vec<Value> = folds
            .iter()
            .map(|fold| {
                json!({
                    "startLine": fold.start_line,
                    "endLine": fold.end_line,
                    "kind": fold.kind,
                })
            })
            .collect();
        vec![Outgoing::success(id, json!(values))]
    }

    /// Handles `textDocument/selectionRange`.
    pub(crate) fn selection_range(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(uri) = document_uri(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument URI",
            )];
        };
        let Some(positions) = params.get("positions").and_then(Value::as_array) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing positions",
            )];
        };
        if positions.len() > MAX_SELECTION_POSITIONS {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "too many positions",
            )];
        }
        let mut parsed_positions = Vec::with_capacity(positions.len());
        for position in positions {
            let Some(position) = crate::handlers::position(position) else {
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INVALID_PARAMS,
                    "invalid position",
                )];
            };
            parsed_positions.push(position);
        }
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, Value::Null)];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let mut values = Vec::with_capacity(parsed_positions.len());
        for position in parsed_positions {
            let Some(offset) = document.offset_at(position) else {
                return vec![Outgoing::success(id, Value::Null)];
            };
            let chain = selection_chain(document.text(), offset);
            let Some(value) = selection_value(document, &chain) else {
                return vec![Outgoing::success(id, Value::Null)];
            };
            values.push(value);
        }
        vec![Outgoing::success(id, json!(values))]
    }

    /// Handles `textDocument/documentSymbol`.
    pub(crate) fn document_symbol(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(uri) = document_uri(&params) else {
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
        let symbols = document_symbols(document.text());
        let values: Vec<Value> = if self.client_hierarchical_symbols {
            symbols
                .iter()
                .filter_map(|symbol| hierarchical_symbol(document, symbol))
                .collect()
        } else {
            let mut output = Vec::new();
            for symbol in &symbols {
                flatten_symbol(document, uri, symbol, None, &mut output);
            }
            output
        };
        vec![Outgoing::success(id, json!(values))]
    }
}

/// Builds the nested `SelectionRange` value for one innermost-first chain.
fn selection_value(document: &crate::workspace::Document, chain: &[TextRange]) -> Option<Value> {
    let mut parent: Option<Value> = None;
    for range in chain.iter().rev() {
        let range = crate::handlers::source_range_value(document, SourceRange::new(*range))?;
        let mut node = json!({"range": range});
        if let Some(parent) = parent.take() {
            node.as_object_mut()?.insert("parent".to_owned(), parent);
        }
        parent = Some(node);
    }
    parent
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::assert_is_empty)]
mod tests {
    use super::*;

    #[test]
    fn folds_elements_multiline_tags_comments_and_wxs() {
        let text = "<view\n  class=\"a\">\n  <text>x</text>\n  <!-- a\n  b -->\n  <wxs module=\"m\">\nvar x = 1;\n</wxs>\n</view>";
        let line_index = LineIndex::new(text);
        let folds = folding_ranges(text, &line_index);
        assert_eq!(
            folds,
            vec![
                Fold {
                    start_line: 0,
                    end_line: 8,
                    kind: "region"
                },
                Fold {
                    start_line: 0,
                    end_line: 1,
                    kind: "region"
                },
                Fold {
                    start_line: 3,
                    end_line: 4,
                    kind: "comment"
                },
                Fold {
                    start_line: 5,
                    end_line: 7,
                    kind: "region"
                },
            ]
        );
        for fold in &folds {
            assert!(fold.start_line < fold.end_line);
            assert!((fold.end_line as usize) < line_index.line_count());
        }
    }

    #[test]
    fn single_line_constructs_do_not_produce_empty_folds() {
        let text = "<view><text>x</text><!-- c --></view>";
        let line_index = LineIndex::new(text);
        assert!(folding_ranges(text, &line_index).is_empty());
    }

    #[test]
    fn unclosed_regions_do_not_swallow_siblings() {
        let text = "<view>\n<text>a\nb";
        let line_index = LineIndex::new(text);
        assert!(folding_ranges(text, &line_index).is_empty());
    }

    #[test]
    fn folding_is_bounded_and_sorted() {
        let text = format!(
            "{}\n{}",
            (0..40)
                .map(|index| format!("<view id=\"v{index}\">\n</view>"))
                .collect::<Vec<_>>()
                .join("\n"),
            "<!--\na\n-->"
        );
        let line_index = LineIndex::new(text.as_str());
        let folds = folding_ranges(&text, &line_index);
        assert!(folds.len() <= MAX_FOLDING_RANGES);
        assert!(folds.windows(2).all(|pair| {
            pair[0].start_line < pair[1].start_line
                || (pair[0].start_line == pair[1].start_line
                    && pair[0].end_line >= pair[1].end_line)
        }));
    }

    #[test]
    fn links_nested_same_name_tags_to_their_own_pair() {
        let text = "<view><view>x</view></view>";
        let outer = linked_editing_range(text, 2).expect("outer pair");
        assert_eq!(
            &text[outer.0.start() as usize..outer.0.end() as usize],
            "view"
        );
        assert_eq!(outer.0.start(), 1);
        assert_eq!(outer.1.start(), 22);

        let inner = linked_editing_range(text, 16).expect("inner pair");
        assert_eq!(inner.0.start(), 7);
        assert_eq!(inner.1.start(), 15);
    }

    #[test]
    fn linked_editing_requires_a_complete_pair() {
        for text in ["<view/>", "<view>x", "<view></VIEW>", "plain text"] {
            assert!(
                linked_editing_range(text, 2).is_none(),
                "{text:?} must not link"
            );
        }
    }

    #[test]
    fn linked_editing_ignores_a_position_outside_the_name() {
        let text = "<view>x</view>";
        assert!(linked_editing_range(text, 6).is_none());
        assert!(linked_editing_range(text, 14).is_none());
    }

    #[test]
    fn selection_chain_keeps_label_tokens_and_ancestors() {
        let text = "<view class=\"a\">\n  {{ user.name }}\n</view>";
        let offset = u32::try_from(text.find("user").expect("user") + 2).expect("offset");
        let chain = selection_chain(text, offset);
        let names: Vec<&str> = chain
            .iter()
            .map(|range| &text[range.start() as usize..range.end() as usize])
            .collect();
        assert_eq!(
            names,
            vec![
                "user.name",
                "{{ user.name }}",
                "<view class=\"a\">\n  {{ user.name }}\n</view>",
            ]
        );
        assert_strictly_nested(&chain);
    }

    #[test]
    fn selection_chain_covers_attribute_values_and_text() {
        let text = "<view class=\"a\">text</view>";
        let value_offset = u32::try_from(text.find("\"a\"").expect("value") + 1).expect("offset");
        let chain = selection_chain(text, value_offset);
        let names: Vec<&str> = chain
            .iter()
            .map(|range| &text[range.start() as usize..range.end() as usize])
            .collect();
        assert_eq!(
            names,
            vec!["a", "\"a\"", "class=\"a\"", "<view class=\"a\">text</view>"]
        );

        let text_offset = u32::try_from(text.find("text").expect("text")).expect("offset");
        let chain = selection_chain(text, text_offset);
        assert_eq!(
            &text[chain[0].start() as usize..chain[0].end() as usize],
            "text"
        );
        assert!(chain.len() >= 2);
        assert_strictly_nested(&chain);
    }

    #[test]
    fn selection_chain_rejects_positions_outside_the_document() {
        let text = "<view>x</view>\n";
        assert!(selection_chain(text, u32::try_from(text.len() + 1).expect("offset")).is_empty());
        let chain = selection_chain(text, 2);
        assert_eq!(chain.len(), 2);
        assert_eq!(
            &text[chain[1].start() as usize..chain[1].end() as usize],
            text
        );
    }

    #[test]
    fn document_symbols_nest_elements_and_summarize_ids_and_classes() {
        let text = "<view id=\"page\" class=\"a b\"><text>hi</text><image src=\"x\"/></view>";
        let symbols = document_symbols(text);
        assert_eq!(symbols.len(), 1);
        let view = &symbols[0];
        assert_eq!(view.name, "view");
        assert_eq!(view.detail.as_deref(), Some("#page .a .b"));
        assert_eq!(view.kind, 5);
        let children: Vec<&str> = view
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(children, vec!["text", "image"]);
        assert!(view.range.start() <= view.selection.start());
        assert!(view.selection.end() <= view.range.end());
        for child in &view.children {
            assert!(child.range.start() >= view.range.start());
            assert!(child.range.end() <= view.range.end());
        }
    }

    #[test]
    fn document_symbols_skip_dynamic_id_and_class_values() {
        let text = "<view id=\"{{ id }}\" class=\"a {{ b }}\"></view>";
        let symbols = document_symbols(text);
        assert_eq!(symbols[0].detail, None);
    }

    #[test]
    fn document_symbols_include_malformed_siblings_but_stay_valid() {
        let text = "<view><text>a</text><view";
        let symbols = document_symbols(text);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "view");
        assert!(symbols[0].children.iter().any(|child| child.name == "text"));
    }

    #[test]
    fn selection_chains_are_depth_bounded() {
        let depth = MAX_SELECTION_DEPTH + 8;
        let mut text = String::new();
        for _ in 0..depth {
            text.push_str("<view>");
        }
        text.push('x');
        for _ in 0..depth {
            text.push_str("</view>");
        }
        let offset = u32::try_from(text.find('x').expect("content")).expect("offset");
        let chain = selection_chain(&text, offset);
        assert!(chain.len() <= MAX_SELECTION_DEPTH, "{}", chain.len());
        assert_strictly_nested(&chain);
    }

    #[test]
    fn folding_ranges_are_capped() {
        use std::fmt::Write as _;

        let mut text = String::new();
        for index in 0..(MAX_FOLDING_RANGES + 10) {
            writeln!(text, "<view id=\"v{index}\">").expect("write");
            text.push_str("</view>");
            text.push('\n');
        }
        let line_index = LineIndex::new(text.as_str());
        let folds = folding_ranges(&text, &line_index);
        assert!(folds.len() <= MAX_FOLDING_RANGES, "{}", folds.len());
        assert!(!folds.is_empty());
    }

    fn assert_strictly_nested(chain: &[TextRange]) {
        for pair in chain.windows(2) {
            let inner = pair[0];
            let outer = pair[1];
            assert!(outer.start() <= inner.start(), "{chain:?}");
            assert!(outer.end() >= inner.end(), "{chain:?}");
            assert_ne!(outer, inner, "duplicate range in {chain:?}");
        }
    }
}
