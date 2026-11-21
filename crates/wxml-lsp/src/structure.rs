//! Structural facts and handlers: linked tag names, folding, selection ranges
//! and document symbols.
//!
//! Every function in this module works on the lossless syntax tree from
//! `wxml-syntax`; the parser is not reimplemented and no regular expressions
//! are used to rediscover structure. Requests are answered from the open
//! document snapshot, so these features keep working when the Node type
//! backend is unavailable.
//!
//! [`StructureFacts`] is the crate-internal single source of structural
//! queries. It parses once and exposes linked-editing pairs, selection parent
//! chains, folding ranges and document symbols. S must reuse
//! [`StructureFacts::document_symbols`] for `documentSymbol` instead of parsing
//! the document a second time; T can combine its ordered scope frames with the
//! ranges returned here.
//!
//! Linked editing is intentionally separate from rename: this module never
//! claims `textDocument/rename`, which belongs to the semantic package.

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    /// Counts parses on the current test thread.
    static STRUCTURE_PARSE_CALLS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn bump_parse_count() {
    STRUCTURE_PARSE_CALLS.with(|count| count.set(count.get().saturating_add(1)));
}

#[cfg(test)]
fn reset_parse_count() {
    STRUCTURE_PARSE_CALLS.with(|count| count.set(0));
}

#[cfg(test)]
fn parse_count() -> usize {
    STRUCTURE_PARSE_CALLS.with(Cell::get)
}

use serde_json::{Value, json};
use wxml_syntax::parser::{NodeId, NodeKind, SyntaxTree};
use wxml_text::{LineIndex, SourceRange, TextRange};
use wxml_types::{
    BackendEpoch, BackendError, CapabilityStatus, DocumentSymbolKind, DocumentSymbolsResult,
    RequestContext,
};

use crate::handlers::line_index_range_value;
use crate::jsonrpc::{self, Outgoing, RequestKey};
use crate::project_host::DocumentRoute;
use crate::scheduler::{DocumentSymbolsScope, Job, JobKind, Outcome, SubmitError, WorkDone};
use crate::server::{Server, log};

/// Largest number of positions accepted by one `selectionRange` request.
///
/// Multi-cursor selection requests commonly exceed a handful of positions;
/// 256 covers realistic editor use while keeping the one-parse request bounded.
pub(crate) const MAX_SELECTION_POSITIONS: usize = 256;

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

/// Parses a document once and exposes reusable structural queries.
///
/// Parsing is rejected only when a resource budget truncated the tree, because
/// a partial tree cannot produce trustworthy ranges. Ordinary syntax errors
/// are tolerated: structural ranges stay inside the nodes the parser recovered.
#[derive(Debug)]
pub(crate) struct StructureFacts {
    tree: SyntaxTree,
}

impl StructureFacts {
    /// Parses `text` into reusable facts.
    #[must_use]
    pub(crate) fn parse(text: &str) -> Option<Self> {
        #[cfg(test)]
        bump_parse_count();
        let parsed = wxml_syntax::parse(text);
        if parsed.is_truncated() {
            log(format_args!(
                "structure skipped: parse was truncated by a resource budget"
            ));
            return None;
        }
        Some(Self { tree: parsed.tree })
    }

    /// Wraps an already parsed tree so structural consumers do not parse twice.
    #[must_use]
    pub(crate) const fn from_tree(tree: SyntaxTree) -> Self {
        Self { tree }
    }

    /// Returns the parsed source text.
    #[must_use]
    pub(crate) fn text(&self) -> &str {
        self.tree.text()
    }

    /// Returns the parsed tree for structural consumers that must not parse
    /// the document again.
    #[must_use]
    pub(crate) const fn tree(&self) -> &SyntaxTree {
        &self.tree
    }

    /// Returns the paired tag-name ranges for linked editing.
    #[must_use]
    pub(crate) fn linked_editing_range(&self, offset: u32) -> Option<(TextRange, TextRange)> {
        linked_editing_range_in(&self.tree, offset)
    }

    /// Returns the selection ancestor chain for `offset`, innermost first.
    #[must_use]
    pub(crate) fn selection_chain(&self, offset: u32) -> Vec<TextRange> {
        selection_chain_in(&self.tree, offset)
    }

    /// Returns folding ranges for elements, multiline start tags, comments and
    /// whole WXS elements.
    #[must_use]
    pub(crate) fn folding_ranges(&self, line_index: &LineIndex) -> Vec<Fold> {
        folding_ranges_in(&self.tree, line_index)
    }

    /// Builds hierarchical document symbols.
    ///
    /// This is the single outline implementation; the semantic package must
    /// reuse it rather than performing a second structural parse.
    #[must_use]
    pub(crate) fn document_symbols(&self) -> Vec<DocumentSymbol> {
        document_symbols_in(&self.tree)
    }
}

/// Converts a byte range into a line-only folding range.
fn add_fold(
    text: &str,
    line_index: &LineIndex,
    folds: &mut Vec<Fold>,
    range: TextRange,
    kind: &'static str,
) {
    if range.is_empty() {
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

/// Folding implementation over an already parsed tree.
fn folding_ranges_in(tree: &SyntaxTree, line_index: &LineIndex) -> Vec<Fold> {
    let text = tree.text();
    let mut folds = Vec::new();
    for id in tree.descendants(tree.root()) {
        match tree.kind(id) {
            Some(NodeKind::Element) => {
                let Some(element) = tree.ast(id).and_then(as_element) else {
                    continue;
                };
                if element.is_closed() || element.is_self_closing() {
                    add_fold(text, line_index, &mut folds, element.text_range(), "region");
                }
                // A multiline start tag is foldable even for an unclosed or
                // self-closing element; dedup removes the duplicate range.
                if let Some(start_tag) = start_tag_id(tree, id)
                    && let Some(range) = tree.text_range(start_tag)
                {
                    add_fold(text, line_index, &mut folds, range, "region");
                }
            }
            Some(NodeKind::Comment) => {
                if let Some(range) = tree.text_range(id) {
                    add_fold(text, line_index, &mut folds, range, "comment");
                }
            }
            _ => {}
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

/// Linked-editing implementation over an already parsed tree.
fn linked_editing_range_in(tree: &SyntaxTree, offset: u32) -> Option<(TextRange, TextRange)> {
    for id in tree.descendants(tree.root()) {
        let Some(kind) = tree.kind(id) else {
            continue;
        };
        if !matches!(kind, NodeKind::StartTag | NodeKind::EndTag) {
            continue;
        }
        let Some(name) = tree.name_range(id) else {
            continue;
        };
        if offset < name.start() || offset > name.end() {
            continue;
        }
        let Some(element) = tree.parent(id) else {
            continue;
        };
        if tree.kind(element) != Some(NodeKind::Element) {
            continue;
        }
        let other = if kind == NodeKind::StartTag {
            end_tag_id(tree, element)
        } else {
            start_tag_id(tree, element)
        };
        let Some(other) = other else {
            continue;
        };
        let Some(other_name) = tree.name_range(other) else {
            continue;
        };
        let Some(start_name) = tree.name(id) else {
            continue;
        };
        let Some(end_name) = tree.name(other) else {
            continue;
        };
        if start_name != end_name {
            continue;
        }
        return Some(if name.start() <= other_name.start() {
            (name, other_name)
        } else {
            (other_name, name)
        });
    }
    None
}

/// Selection implementation over an already parsed tree.
fn selection_chain_in(tree: &SyntaxTree, offset: u32) -> Vec<TextRange> {
    let text = tree.text();
    let Ok(offset_usize) = usize::try_from(offset) else {
        return Vec::new();
    };
    if offset_usize > text.len() || !text.is_char_boundary(offset_usize) {
        return Vec::new();
    }
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
    // At a boundary, prefer the right-side node unless a token ending at the
    // cursor is a descendant of it, or the right-side node is a tag delimiter
    // and the left-side node is a content token. This keeps `x|</view>` on the
    // text token without selecting `view` at the start of that text.
    let deepest = match (interior, ending) {
        (Some(interior), Some(ending)) => {
            let interior_range = tree.text_range(interior.1);
            let ending_range = tree.text_range(ending.1);
            let interior_contains_ending = matches!(
                (interior_range, ending_range),
                (Some(interior_range), Some(ending_range))
                    if interior_range.start() <= ending_range.start()
                        && interior_range.end() >= ending_range.end()
            );
            let interior_is_tag = matches!(
                tree.kind(interior.1),
                Some(NodeKind::StartTag | NodeKind::EndTag)
            );
            let ending_is_content = matches!(
                tree.kind(ending.1),
                Some(
                    NodeKind::Text
                        | NodeKind::AttributeValue
                        | NodeKind::Mustache
                        | NodeKind::Comment
                )
            );
            if interior_contains_ending || (interior_is_tag && ending_is_content) {
                Some(ending)
            } else {
                Some(interior)
            }
        }
        (interior, ending) => interior.or(ending),
    };
    let (_, mut current) = deepest.unwrap_or_else(|| (0, tree.root()));
    let mut chain: Vec<TextRange> = Vec::new();
    loop {
        let kind = tree.kind(current);
        let range = if matches!(kind, Some(NodeKind::StartTag | NodeKind::EndTag)) {
            // Tag names are selection tokens too; the whole start/end tag is
            // not useful because it includes attributes or the delimiter.
            tree.name_range(current)
        } else {
            tree.text_range(current)
        };
        if let Some(range) = range {
            let keep = chain.last().is_none_or(|inner| {
                range != *inner && range.start() <= inner.start() && range.end() >= inner.end()
            });
            if keep {
                chain.push(range);
            }
        }
        let Some(parent) = tree.parent(current) else {
            break;
        };
        current = parent;
    }
    chain
}

/// Document-symbol implementation over an already parsed tree.
fn document_symbols_in(tree: &SyntaxTree) -> Vec<DocumentSymbol> {
    let mut budget = wxml_types::MAX_DOCUMENT_SYMBOLS;
    let mut truncated = false;
    let mut symbols = Vec::new();
    for node in tree.document().children() {
        let wxml_syntax::AstNode::Element(element) = node else {
            continue;
        };
        if let Some(symbol) = element_symbol(element, 0, &mut budget, &mut truncated) {
            symbols.push(symbol);
        }
        if budget == 0 {
            truncated = true;
            break;
        }
    }
    if truncated {
        log(format_args!(
            "document symbols truncated at {} symbols or {} levels",
            wxml_types::MAX_DOCUMENT_SYMBOLS,
            wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH
        ));
    }
    symbols
}

/// Builds one symbol and its nested elements.
fn element_symbol(
    element: wxml_syntax::Element<'_>,
    depth: usize,
    budget: &mut usize,
    truncated: &mut bool,
) -> Option<DocumentSymbol> {
    if *budget == 0 {
        *truncated = true;
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
    if depth < wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH {
        for node in element.children() {
            let wxml_syntax::AstNode::Element(child) = node else {
                continue;
            };
            if let Some(symbol) = element_symbol(child, depth.saturating_add(1), budget, truncated)
            {
                children.push(symbol);
            }
            if *budget == 0 {
                break;
            }
        }
    } else if element
        .children()
        .any(|node| matches!(node, wxml_syntax::AstNode::Element(_)))
    {
        *truncated = true;
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

/// Largest number of characters shown in an outline detail.
const MAX_DETAIL_CHARS: usize = 120;

/// Largest number of class tokens shown in an outline detail.
const MAX_DETAIL_CLASSES: usize = 20;

/// Summarizes literal `id` and `class` attributes for the outline.
///
/// This is a plain-text detail, not a backend reason: it is bounded locally
/// and never logged. Control characters are dropped instead of being passed
/// through a display sanitizer that would write user source to stderr.
fn element_detail(element: wxml_syntax::Element<'_>) -> Option<String> {
    let mut detail = String::new();
    if let Some(id) = element
        .attribute("id")
        .and_then(wxml_syntax::Attribute::value)
        .and_then(wxml_syntax::AttributeValue::literal_text)
        .and_then(|value| value.split_whitespace().next())
        .and_then(clean_token)
    {
        detail.push('#');
        detail.push_str(&id);
    }
    if let Some(classes) = element
        .attribute("class")
        .and_then(wxml_syntax::Attribute::value)
        .and_then(wxml_syntax::AttributeValue::literal_text)
    {
        for class in classes.split_whitespace().take(MAX_DETAIL_CLASSES) {
            let Some(class) = clean_token(class) else {
                continue;
            };
            if !detail.is_empty() {
                detail.push(' ');
            }
            detail.push('.');
            detail.push_str(&class);
        }
    }
    if detail.is_empty() {
        return None;
    }
    detail = truncate_chars(&detail, MAX_DETAIL_CHARS);
    Some(detail)
}

/// Removes control characters from one detail token.
fn clean_token(token: &str) -> Option<String> {
    let cleaned: String = token
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Truncates by characters and appends an ellipsis.
fn truncate_chars(text: &str, limit: usize) -> String {
    let mut bounded: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        bounded.push('…');
    }
    bounded
}

/// Reads the `textDocument.uri` of a document-only request.
fn document_uri(params: &Value) -> Option<&str> {
    let document = crate::handlers::text_document(params)?;
    crate::handlers::string(document, "uri")
}

/// Serializes a hierarchical symbol and all of its descendants.
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
        let mut children = Vec::with_capacity(symbol.children.len());
        for child in &symbol.children {
            children.push(hierarchical_symbol(document, child)?);
        }
        object.insert("children".to_owned(), Value::Array(children));
    }
    Some(value)
}

/// Serializes one flat `SymbolInformation` and its descendants.
///
/// Returns `false` if any range cannot be represented; the caller then drops
/// the whole result instead of returning a partial outline.
fn flatten_symbol(
    document: &crate::workspace::Document,
    uri: &str,
    symbol: &DocumentSymbol,
    container: Option<&str>,
    output: &mut Vec<Value>,
) -> bool {
    if output.len() >= wxml_types::MAX_DOCUMENT_SYMBOLS {
        return false;
    }
    let Some(range) = crate::handlers::source_range_value(document, SourceRange::new(symbol.range))
    else {
        return false;
    };
    let mut value = json!({
        "name": symbol.name,
        "kind": symbol.kind,
        "location": {"uri": uri, "range": range},
    });
    if let Some(container) = container {
        let Some(object) = value.as_object_mut() else {
            return false;
        };
        object.insert(
            "containerName".to_owned(),
            Value::String(container.to_owned()),
        );
    }
    output.push(value);
    for child in &symbol.children {
        if !flatten_symbol(document, uri, child, Some(&symbol.name), output) {
            return false;
        }
    }
    true
}

/// Serializes a backend document-symbol result for one script document.
///
/// Returns `None` if any kind or range cannot be represented, so the caller
/// returns `[]` instead of a half-serialized outline. Hierarchical clients get
/// `DocumentSymbol` with `range`, `selectionRange` and `children`; other clients
/// get flat `SymbolInformation` whose `location.uri` is the requested script.
///
/// Truncation and gaps are not padded with placeholders: the decoded subset is
/// returned exactly as the backend reported it. The backend owns the
/// partial/unsupported status; this serializer only refuses invalid positions.
fn script_symbol_values(
    result: &DocumentSymbolsResult,
    line_index: &LineIndex,
    uri: &str,
    hierarchical: bool,
) -> Option<Vec<Value>> {
    let mut values = Vec::with_capacity(result.symbols.len());
    for symbol in &result.symbols {
        if hierarchical {
            values.push(hierarchical_script_symbol(line_index, symbol)?);
        } else if !flatten_script_symbol(line_index, uri, symbol, None, &mut values) {
            return None;
        }
    }
    Some(values)
}

/// Serializes one hierarchical `DocumentSymbol` and its descendants.
fn hierarchical_script_symbol(
    line_index: &LineIndex,
    symbol: &wxml_types::DocumentSymbol,
) -> Option<Value> {
    if !script_symbol_ranges_are_valid(symbol) {
        return None;
    }
    let kind = script_symbol_kind(symbol.kind)?;
    let range = line_index_range_value(line_index, symbol.range)?;
    let selection_range = line_index_range_value(line_index, symbol.selection_range)?;
    let mut children = Vec::with_capacity(symbol.children.len());
    for child in &symbol.children {
        children.push(hierarchical_script_symbol(line_index, child)?);
    }
    let mut value = json!({
        "name": symbol.name,
        "kind": kind,
        "range": range,
        "selectionRange": selection_range,
    });
    if !children.is_empty() {
        value
            .as_object_mut()?
            .insert("children".to_owned(), Value::Array(children));
    }
    Some(value)
}

/// Serializes one flat `SymbolInformation` and its descendants.
///
/// Returns `false` if any kind or range is invalid; the caller then drops the
/// whole result instead of returning a partial outline.
fn flatten_script_symbol(
    line_index: &LineIndex,
    uri: &str,
    symbol: &wxml_types::DocumentSymbol,
    container: Option<&str>,
    output: &mut Vec<Value>,
) -> bool {
    if !script_symbol_ranges_are_valid(symbol) {
        return false;
    }
    let Some(kind) = script_symbol_kind(symbol.kind) else {
        return false;
    };
    let Some(range) = line_index_range_value(line_index, symbol.range) else {
        return false;
    };
    let mut value = json!({
        "name": symbol.name,
        "kind": kind,
        "location": {"uri": uri, "range": range},
    });
    if let Some(container) = container {
        let Some(object) = value.as_object_mut() else {
            return false;
        };
        object.insert(
            "containerName".to_owned(),
            Value::String(container.to_owned()),
        );
    }
    output.push(value);
    for child in &symbol.children {
        if !flatten_script_symbol(line_index, uri, child, Some(&symbol.name), output) {
            return false;
        }
    }
    true
}

/// Requires LSP's `selectionRange` containment and parent/child containment.
fn script_symbol_ranges_are_valid(symbol: &wxml_types::DocumentSymbol) -> bool {
    if symbol.selection_range.start() < symbol.range.start()
        || symbol.selection_range.end() > symbol.range.end()
    {
        return false;
    }
    symbol.children.iter().all(|child| {
        child.range.start() >= symbol.range.start()
            && child.range.end() <= symbol.range.end()
            && script_symbol_ranges_are_valid(child)
    })
}

/// Maps the C0 symbol kind to the LSP `SymbolKind` number.
///
/// LSP has no type-alias kind, so [`DocumentSymbolKind::TypeAlias`] maps to
/// `Interface` (11) while the C0 DTO keeps the distinction. A future C0 kind
/// with no proven LSP mapping fails serialization instead of guessing.
#[allow(clippy::match_same_arms)] // TypeAlias intentionally maps to Interface.
const fn script_symbol_kind(kind: DocumentSymbolKind) -> Option<u32> {
    match kind {
        DocumentSymbolKind::Module => Some(2),
        DocumentSymbolKind::Class => Some(5),
        DocumentSymbolKind::Method => Some(6),
        DocumentSymbolKind::Property => Some(7),
        DocumentSymbolKind::Enum => Some(10),
        DocumentSymbolKind::Interface => Some(11),
        DocumentSymbolKind::TypeAlias => Some(11),
        DocumentSymbolKind::Function => Some(12),
        DocumentSymbolKind::Variable => Some(13),
        DocumentSymbolKind::EnumMember => Some(22),
        _ => None,
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
        let Some(facts) = StructureFacts::parse(document.text()) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        let Some((first, second)) = facts.linked_editing_range(offset) else {
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
        let Some(facts) = StructureFacts::parse(document.text()) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let folds = facts.folding_ranges(document.snapshot().line_index());
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
        let Some(facts) = StructureFacts::parse(document.text()) else {
            return vec![Outgoing::success(id, Value::Null)];
        };
        // One parse for every cursor; a bad position falls back to the
        // document range so the result stays aligned with the request.
        let mut values = Vec::with_capacity(parsed_positions.len());
        for position in parsed_positions {
            let chain = document
                .offset_at(position)
                .map_or_else(Vec::new, |offset| facts.selection_chain(offset));
            values.push(
                selection_value(document, &chain).unwrap_or_else(|| root_selection_value(document)),
            );
        }
        vec![Outgoing::success(id, json!(values))]
    }

    /// Handles `textDocument/documentSymbol`.
    ///
    /// E is the single owner of the static WXML outline. S must reuse
    /// [`StructureFacts::document_symbols`] and must not parse the document a
    /// second time for its symbol queries.
    ///
    /// A script document is answered by [`wxml_types::Backend::document_symbols`]
    /// on the background worker and is never merged with the static WXML
    /// outline: script ranges belong to the requested script, not to a
    /// template. The WXML outline stays synchronous and never calls a backend.
    pub(crate) fn document_symbol(&mut self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
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
        match self.host.route(uri) {
            DocumentRoute::Script(_) => self.schedule_script_document_symbol(id, uri),
            DocumentRoute::Wxml(_) => self.static_document_symbol(id, uri),
            DocumentRoute::Json(_) | DocumentRoute::Style(_) => {
                vec![Outgoing::success(id, json!([]))]
            }
            DocumentRoute::Unknown => {
                // Synthetic templates have no project route; the URI is the
                // only signal that they are WXML.
                if crate::handlers::template_document(self, uri) {
                    self.static_document_symbol(id, uri)
                } else {
                    vec![Outgoing::success(id, json!([]))]
                }
            }
        }
    }

    /// Answers the static WXML outline for one open template.
    fn static_document_symbol(&self, id: Value, uri: &str) -> Vec<Outgoing> {
        let Some(document) = self.workspace.get(uri) else {
            log(format_args!(
                "WXML document symbols failed: the template is not open"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the template is not open",
            )];
        };
        let Some(facts) = StructureFacts::parse(document.text()) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the template outline is unavailable",
            )];
        };
        let symbols = facts.document_symbols();
        let values: Option<Vec<Value>> = if self.client.hierarchical_symbols {
            let mut values = Vec::with_capacity(symbols.len());
            let mut complete = true;
            for symbol in &symbols {
                if let Some(value) = hierarchical_symbol(document, symbol) {
                    values.push(value);
                } else {
                    complete = false;
                    break;
                }
            }
            if complete { Some(values) } else { None }
        } else {
            let mut values = Vec::new();
            let mut complete = true;
            for symbol in &symbols {
                if !flatten_symbol(document, uri, symbol, None, &mut values) {
                    complete = false;
                    break;
                }
            }
            if complete { Some(values) } else { None }
        };
        let Some(values) = values else {
            log(format_args!(
                "WXML document symbols failed: a range could not be serialized"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the template outline could not be serialized",
            )];
        };
        vec![Outgoing::success(id, json!(values))]
    }

    /// Validates one script outline request and queues its backend job.
    ///
    /// The scheduling check is fail-closed: a missing owning template, missing
    /// script text, or a script/overlay mismatch rejects the request instead of
    /// returning an empty outline. The backend query itself runs on the
    /// scheduler worker; `document_symbols_done` publishes the result only if
    /// the captured script/view/template identity is still current.
    #[allow(clippy::too_many_lines)] // one linear request validation and schedule
    fn schedule_script_document_symbol(&mut self, id: Value, uri: &str) -> Vec<Outgoing> {
        let Some(document) = self.workspace.get(uri) else {
            log(format_args!(
                "script document symbols failed: the script is not open"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the script is not open",
            )];
        };
        let subject = document.file().clone();
        let view = match self.host.view_for_script(&subject) {
            Ok(view) => view,
            Err(error) => {
                log(format_args!(
                    "script document symbols failed: {}",
                    crate::handlers::log_safe_text(&error.to_string(), 200)
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::REQUEST_FAILED,
                    "the script is not an owning page or component script",
                )];
            }
        };
        let template = match self.host.file_id_in(subject.project(), &view.wxml) {
            Ok(file) => file,
            Err(error) => {
                log(format_args!(
                    "script document symbols failed: {}",
                    crate::handlers::log_safe_text(&error.to_string(), 200)
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::REQUEST_FAILED,
                    "the owning template path is invalid",
                )];
            }
        };
        let Some(snapshot) = self.host.source(&template) else {
            log(format_args!(
                "script document symbols failed: the owning template is not open"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the owning template is not open",
            )];
        };
        let Some(host_text) = self.host.file_text(&subject) else {
            log(format_args!(
                "script document symbols failed: the script text is unavailable"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the script text is unavailable",
            )];
        };
        if host_text.as_ref() != document.text() {
            log(format_args!(
                "script document symbols failed: the workspace and host script text disagree"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the script text changed before the outline was requested",
            )];
        }
        let script_hash = script_text_hash(document.text());
        let template_snapshot = snapshot.snapshot();
        let epoch = self.refresh_epoch();
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the script was closed before the outline was scheduled",
            )];
        };
        let stamp = document.stamp(&epoch);
        let context = match RequestContext::new(
            self.host.project_revision(),
            view.view_revision,
            epoch,
            snapshot,
        ) {
            Ok(context) => context,
            Err(error) => {
                log(format_args!(
                    "script document symbols failed: {}",
                    crate::handlers::log_safe_text(&error.to_string(), 200)
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::CONTENT_MODIFIED,
                    "the owning template snapshot does not match the request",
                )];
            }
        };
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
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let attempt = self.next_attempt();
        self.inflight.insert(
            key.clone(),
            crate::server::Inflight {
                id: id.clone(),
                attempt,
                cancel: std::sync::Arc::clone(&cancel),
                uri: uri.to_owned(),
            },
        );
        let job = Job {
            key: Some(key.clone()),
            kind: JobKind::DocumentSymbols {
                subject,
                scope: DocumentSymbolsScope {
                    template,
                    template_snapshot,
                    view_revision: view.view_revision,
                    script_hash,
                },
            },
            context,
            offset: 0,
            stamp,
            attempt,
            cancel,
        };
        match self.dispatch.submit(job) {
            Ok(()) => Vec::new(),
            Err(error) => {
                self.inflight.remove(&key);
                log(format_args!(
                    "dispatch rejected a document symbols job: {error:?}"
                ));
                vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INTERNAL_ERROR,
                    submit_message(error),
                )]
            }
        }
    }

    /// Answers one finished script outline query.
    ///
    /// Every stale, cancelled or unsupported outcome is an explicit protocol
    /// error. Only a `Supported` empty outline and a non-empty `Partial`
    /// outline are published; a backend status that does not prove an outline
    /// never becomes `[]`.
    #[allow(clippy::too_many_lines)] // one ordered publication-barrier matrix
    pub(crate) fn document_symbols_done(
        &self,
        id: Value,
        uri: &str,
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
        let JobKind::DocumentSymbols { subject, scope } = &done.kind else {
            log(format_args!(
                "a document symbol result carried the wrong job kind"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INTERNAL_ERROR,
                "the script outline query returned an unexpected result",
            )];
        };
        if done.stamp.epoch != *epoch || !self.workspace.is_current(&done.stamp) {
            log(format_args!(
                "dropping a script outline result for an old revision"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the document changed before the outline was published",
            )];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the script was closed before the outline was published",
            )];
        };
        if document.file() != subject || script_text_hash(document.text()) != scope.script_hash {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the script text changed before the outline was published",
            )];
        }
        match self.host.file_text(subject) {
            Some(text) if script_text_hash(&text) == scope.script_hash => {}
            _ => {
                log(format_args!(
                    "dropping a script outline result: the host script text changed"
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::CONTENT_MODIFIED,
                    "the script text changed before the outline was published",
                )];
            }
        }
        let view = match self.host.view_for_script(subject) {
            Ok(view) => view,
            Err(error) => {
                log(format_args!(
                    "dropping a script outline result: {}",
                    crate::handlers::log_safe_text(&error.to_string(), 200)
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::CONTENT_MODIFIED,
                    "the owning view changed before the outline was published",
                )];
            }
        };
        if view.view_revision != scope.view_revision || view.script_file != *subject {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the owning view changed before the outline was published",
            )];
        }
        let Ok(template) = self.host.file_id_in(subject.project(), &view.wxml) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the owning view changed before the outline was published",
            )];
        };
        if template != scope.template {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::CONTENT_MODIFIED,
                "the owning view changed before the outline was published",
            )];
        }
        match self.host.source(&scope.template) {
            Some(snapshot) if snapshot.snapshot() == scope.template_snapshot => {}
            _ => {
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::CONTENT_MODIFIED,
                    "the owning template changed before the outline was published",
                )];
            }
        }

        let result = match done.outcome {
            Ok(Outcome::DocumentSymbols(result)) => *result,
            Ok(_) => {
                log(format_args!(
                    "document symbol job returned the wrong result kind"
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::INTERNAL_ERROR,
                    "the script outline query returned an unexpected result",
                )];
            }
            Err(BackendError::Cancelled) => {
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::CONTENT_MODIFIED,
                    "the script outline request was cancelled",
                )];
            }
            Err(error) => {
                log(format_args!(
                    "script document symbols failed: {}",
                    crate::handlers::log_safe_text(&error.to_string(), 200)
                ));
                return vec![Outgoing::failure(
                    id,
                    jsonrpc::error_codes::REQUEST_FAILED,
                    backend_failure_message(&error),
                )];
            }
        };
        if !result.status.is_supported()
            && !matches!(result.status, CapabilityStatus::Partial { .. })
        {
            log(format_args!(
                "script document symbols are {}: {} symbol(s), {} skipped, truncated={}",
                crate::handlers::status_label(&result.status),
                result.symbols.len(),
                result.skipped,
                result.truncated
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the script outline is unavailable",
            )];
        }
        if result.symbols.is_empty() && !result.status.is_supported() {
            log(format_args!(
                "script document symbols are {} with no symbols: {} skipped, truncated={}",
                crate::handlers::status_label(&result.status),
                result.skipped,
                result.truncated
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the script outline is incomplete",
            )];
        }
        if let Some(reason) = result.status.reason() {
            log(format_args!(
                "script document symbols are {}: {}",
                crate::handlers::status_label(&result.status),
                crate::handlers::log_safe_text(reason, 200)
            ));
        }
        let Some(values) = script_symbol_values(
            &result,
            document.snapshot().line_index(),
            uri,
            self.client.hierarchical_symbols,
        ) else {
            log(format_args!(
                "script document symbols failed: a range or kind could not be serialized"
            ));
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::REQUEST_FAILED,
                "the script outline could not be serialized",
            )];
        };
        vec![Outgoing::success(id, json!(values))]
    }
}

/// FNV-1a over the complete script text.
///
/// The scheduler compares this hash with both the workspace document and the
/// host's effective script text before publishing ranges produced for the
/// prepared view. A mismatch means the outline ranges no longer describe the
/// text the LSP layer would convert them against.
fn script_text_hash(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Client-safe message for one failed backend outline query.
const fn backend_failure_message(error: &BackendError) -> &'static str {
    match error {
        BackendError::Timeout => "the type backend timed out",
        BackendError::Unavailable(_) => "the type backend is unavailable",
        BackendError::Protocol(_) => "the type backend violated its protocol",
        BackendError::Internal(_) => "the type backend failed internally",
        BackendError::Cancelled => "the script outline request was cancelled",
        _ => "the type backend failed",
    }
}

/// Message for a rejected scheduler submission.
const fn submit_message(error: SubmitError) -> &'static str {
    match error {
        SubmitError::Busy => "server is busy",
        SubmitError::Stopped => "server is not running",
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

/// Returns the whole-document selection range used for invalid positions.
fn root_selection_value(document: &crate::workspace::Document) -> Value {
    let end = u32::try_from(document.text().len()).unwrap_or(0);
    let range = TextRange::new(0, end)
        .ok()
        .and_then(|range| crate::handlers::source_range_value(document, SourceRange::new(range)));
    range.map_or_else(
        || {
            json!({
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 0},
                }
            })
        },
        |range| json!({"range": range}),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::assert_is_empty)]
mod tests {
    use super::*;
    use wxml_types::CapabilityStatus;

    /// Parses `text` into facts; unit tests only use texts the parser accepts.
    fn facts(text: &str) -> StructureFacts {
        StructureFacts::parse(text).expect("structure facts")
    }

    #[test]
    fn folds_elements_multiline_tags_comments_and_wxs() {
        let text = "<view\n  class=\"a\">\n  <text>x</text>\n  <!-- a\n  b -->\n  <wxs module=\"m\">\nvar x = 1;\n</wxs>\n</view>";
        let line_index = LineIndex::new(text);
        let folds = facts(text).folding_ranges(&line_index);
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
        assert!(facts(text).folding_ranges(&line_index).is_empty());
    }

    #[test]
    fn unclosed_regions_do_not_swallow_siblings() {
        let text = "<view>\n<text>a\nb";
        let line_index = LineIndex::new(text);
        assert!(facts(text).folding_ranges(&line_index).is_empty());
    }

    #[test]
    fn folds_a_multiline_self_closing_tag() {
        let text = "<image\n  src=\"x\"\n/>";
        let line_index = LineIndex::new(text);
        let folds = facts(text).folding_ranges(&line_index);
        assert_eq!(
            folds,
            vec![Fold {
                start_line: 0,
                end_line: 2,
                kind: "region"
            }]
        );
    }

    #[test]
    fn folding_is_sorted() {
        let text = format!(
            "{}\n{}",
            (0..40)
                .map(|index| format!("<view id=\"v{index}\">\n</view>"))
                .collect::<Vec<_>>()
                .join("\n"),
            "<!--\na\n-->"
        );
        let line_index = LineIndex::new(text.as_str());
        let folds = facts(&text).folding_ranges(&line_index);
        assert!(folds.windows(2).all(|pair| {
            pair[0].start_line < pair[1].start_line
                || (pair[0].start_line == pair[1].start_line
                    && pair[0].end_line >= pair[1].end_line)
        }));
    }

    #[test]
    fn links_nested_same_name_tags_to_their_own_pair() {
        let text = "<view><view>x</view></view>";
        let outer = facts(text).linked_editing_range(2).expect("outer pair");
        assert_eq!(
            &text[outer.0.start() as usize..outer.0.end() as usize],
            "view"
        );
        assert_eq!(outer.0.start(), 1);
        assert_eq!(outer.1.start(), 22);

        let inner = facts(text).linked_editing_range(16).expect("inner pair");
        assert_eq!(inner.0.start(), 7);
        assert_eq!(inner.1.start(), 15);
    }

    #[test]
    fn linked_editing_requires_a_complete_pair() {
        for text in ["<view/>", "<view>x", "<view></VIEW>", "plain text"] {
            assert!(
                facts(text).linked_editing_range(2).is_none(),
                "{text:?} must not link"
            );
        }
    }

    #[test]
    fn linked_editing_ignores_a_position_outside_the_name() {
        let text = "<view>x</view>";
        assert!(facts(text).linked_editing_range(6).is_none());
        assert!(facts(text).linked_editing_range(14).is_none());
    }

    #[test]
    fn selection_chain_keeps_label_tokens_and_ancestors() {
        let text = "<view class=\"a\">\n  {{ user.name }}\n</view>";
        let offset = u32::try_from(text.find("user").expect("user") + 2).expect("offset");
        let chain = facts(text).selection_chain(offset);
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
        let chain = facts(text).selection_chain(value_offset);
        let names: Vec<&str> = chain
            .iter()
            .map(|range| &text[range.start() as usize..range.end() as usize])
            .collect();
        assert_eq!(
            names,
            vec!["a", "\"a\"", "class=\"a\"", "<view class=\"a\">text</view>"]
        );

        let text_offset = u32::try_from(text.find("text").expect("text")).expect("offset");
        let chain = facts(text).selection_chain(text_offset);
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
        assert!(
            facts(text)
                .selection_chain(u32::try_from(text.len() + 1).expect("offset"))
                .is_empty()
        );
        let chain = facts(text).selection_chain(2);
        assert_eq!(chain.len(), 3);
        assert_eq!(
            &text[chain[0].start() as usize..chain[0].end() as usize],
            "view"
        );
        assert_eq!(
            &text[chain[2].start() as usize..chain[2].end() as usize],
            text
        );
    }

    #[test]
    fn selection_chain_keeps_a_token_that_ends_at_the_cursor() {
        let text = "<view>x</view>";
        let offset = u32::try_from(text.find('x').expect("x") + 1).expect("offset");
        let chain = facts(text).selection_chain(offset);
        assert_eq!(
            &text[chain[0].start() as usize..chain[0].end() as usize],
            "x"
        );

        let text = "<view class=\"a\">x</view>";
        let offset = u32::try_from(text.rfind('"').expect("quote")).expect("offset");
        let chain = facts(text).selection_chain(offset);
        assert_eq!(
            &text[chain[0].start() as usize..chain[0].end() as usize],
            "a"
        );
    }

    #[test]
    fn selection_chain_starts_with_a_tag_name_token() {
        let text = "<view>x</view>";
        let chain = facts(text).selection_chain(2);
        assert_eq!(
            &text[chain[0].start() as usize..chain[0].end() as usize],
            "view"
        );
        assert_strictly_nested(&chain);
    }

    #[test]
    fn document_symbols_nest_elements_and_summarize_ids_and_classes() {
        let text = "<view id=\"page\" class=\"a b\"><text>hi</text><image src=\"x\"/></view>";
        let symbols = facts(text).document_symbols();
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
    fn document_symbols_are_bounded_and_detail_is_truncated() {
        let mut text = String::new();
        for _ in 0..(wxml_types::MAX_DOCUMENT_SYMBOLS + 20) {
            text.push_str("<view></view>");
        }
        let symbols = facts(&text).document_symbols();
        assert!(
            symbols.len() <= wxml_types::MAX_DOCUMENT_SYMBOLS,
            "{}",
            symbols.len()
        );

        let classes = (0..40)
            .map(|index| format!("class{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let text = format!("<view class=\"{classes}\"></view>");
        let symbols = facts(&text).document_symbols();
        let detail = symbols[0].detail.as_deref().expect("detail");
        assert!(detail.chars().count() <= MAX_DETAIL_CHARS + 1);
        assert!(detail.starts_with(".class0"));
    }

    #[test]
    fn document_symbols_skip_dynamic_id_and_class_values() {
        let text = "<view id=\"{{ id }}\" class=\"a {{ b }}\"></view>";
        let symbols = facts(text).document_symbols();
        assert_eq!(symbols[0].detail, None);
    }

    #[test]
    fn document_symbols_include_malformed_siblings_but_stay_valid() {
        let text = "<view><text>a</text><view";
        let symbols = facts(text).document_symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "view");
        assert!(symbols[0].children.iter().any(|child| child.name == "text"));
    }

    fn script_range(start: usize, end: usize) -> SourceRange {
        SourceRange::from_bounds(
            u32::try_from(start).expect("range start"),
            u32::try_from(end).expect("range end"),
        )
        .expect("script range")
    }

    fn script_symbol(
        name: &str,
        kind: DocumentSymbolKind,
        range: (usize, usize),
        selection: (usize, usize),
        children: Vec<wxml_types::DocumentSymbol>,
    ) -> wxml_types::DocumentSymbol {
        wxml_types::DocumentSymbol {
            name: name.to_owned(),
            kind,
            range: script_range(range.0, range.1),
            selection_range: script_range(selection.0, selection.1),
            children,
        }
    }

    fn script_symbols(text: &str) -> DocumentSymbolsResult {
        let class_start = text.find("class").expect("class");
        let class_end = text.find("}\ntype Shape").expect("class end") + 1;
        let alias_start = text.find("type Shape").expect("alias");
        let alias_end = alias_start + "type Shape = User;".len();
        let name_start = text.find("name:").expect("name");
        let greet_start = text.find("greet").expect("greet");
        DocumentSymbolsResult {
            symbols: vec![
                script_symbol(
                    "用户",
                    DocumentSymbolKind::Class,
                    (class_start, class_end),
                    (
                        text.find("用户").expect("class name"),
                        text.find("用户").expect("class name") + "用户".len(),
                    ),
                    vec![
                        script_symbol(
                            "name",
                            DocumentSymbolKind::Property,
                            (name_start, name_start + "name: string;".len()),
                            (name_start, name_start + "name".len()),
                            Vec::new(),
                        ),
                        script_symbol(
                            "greet😀",
                            DocumentSymbolKind::Method,
                            (greet_start, greet_start + "greet😀(): void {}".len()),
                            (greet_start, greet_start + "greet😀".len()),
                            Vec::new(),
                        ),
                    ],
                ),
                script_symbol(
                    "Shape",
                    DocumentSymbolKind::TypeAlias,
                    (alias_start, alias_end),
                    (
                        alias_start + "type ".len(),
                        alias_start + "type Shape".len(),
                    ),
                    Vec::new(),
                ),
            ],
            status: CapabilityStatus::Supported,
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        }
    }

    #[test]
    fn script_document_symbols_are_hierarchical_with_selection_ranges() {
        let text = "// 😀 文档\nclass 用户 {\n  name: string;\n  greet😀(): void {}\n}\ntype Shape = User;\n";
        let result = script_symbols(text);
        let values = script_symbol_values(&result, &LineIndex::new(text), "file:///a.ts", true)
            .expect("hierarchical script symbols");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["name"], json!("用户"));
        assert_eq!(values[0]["kind"], json!(5));
        assert_eq!(
            values[0]["range"]["start"],
            json!({"line": 1, "character": 0})
        );
        assert_eq!(
            values[0]["range"]["end"],
            json!({"line": 4, "character": 1})
        );
        assert_eq!(
            values[0]["selectionRange"]["start"],
            json!({"line": 1, "character": 6})
        );
        assert_eq!(
            values[0]["selectionRange"]["end"],
            json!({"line": 1, "character": 8})
        );
        let children = values[0]["children"].as_array().expect("children");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["name"], json!("name"));
        assert_eq!(children[0]["kind"], json!(7));
        assert_eq!(
            children[1]["selectionRange"]["start"],
            json!({"line": 3, "character": 2})
        );
        assert_eq!(
            children[1]["selectionRange"]["end"],
            json!({"line": 3, "character": 9})
        );
        // TypeAlias stays distinct in C0 but maps to LSP Interface.
        assert_eq!(values[1]["name"], json!("Shape"));
        assert_eq!(values[1]["kind"], json!(11));
    }

    #[test]
    fn script_document_symbols_flatten_with_container_names() {
        let text = "// 😀 文档\nclass 用户 {\n  name: string;\n  greet😀(): void {}\n}\ntype Shape = User;\n";
        let result = script_symbols(text);
        let values = script_symbol_values(&result, &LineIndex::new(text), "file:///a.ts", false)
            .expect("flat script symbols");
        assert_eq!(values.len(), 4);
        assert_eq!(values[0]["name"], json!("用户"));
        assert_eq!(values[0]["location"]["uri"], json!("file:///a.ts"));
        assert_eq!(
            values[0]["location"]["range"]["start"],
            json!({"line": 1, "character": 0})
        );
        assert!(values[0].get("selectionRange").is_none());
        assert_eq!(values[1]["containerName"], json!("用户"));
        assert_eq!(values[2]["containerName"], json!("用户"));
        assert_eq!(values[3]["name"], json!("Shape"));
        assert!(values[3].get("containerName").is_none());
    }

    #[test]
    fn script_document_symbols_do_not_pad_gaps_or_truncation() {
        let text = "class C {}\n";
        let class_start = text.find("class").expect("class");
        let mut result = DocumentSymbolsResult {
            symbols: vec![script_symbol(
                "C",
                DocumentSymbolKind::Class,
                (class_start, class_start + "class C {}".len()),
                (class_start + "class ".len(), class_start + "class C".len()),
                Vec::new(),
            )],
            status: CapabilityStatus::Partial {
                reason: "the document symbol evidence is incomplete".to_owned(),
            },
            truncated: true,
            skipped: 3,
            gaps: vec![wxml_types::DocumentSymbolGap {
                start: None,
                reason: "name slot is not an Identifier".to_owned(),
            }],
        };
        let values = script_symbol_values(&result, &LineIndex::new(text), "file:///a.ts", true)
            .expect("partial script symbols");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["name"], json!("C"));
        result.symbols.clear();
        let values = script_symbol_values(&result, &LineIndex::new(text), "file:///a.ts", true)
            .expect("empty partial result");
        assert!(values.is_empty());
    }

    #[test]
    fn script_document_symbols_fail_closed_on_invalid_ranges() {
        let text = "class C {}\n";
        let out_of_bounds = DocumentSymbolsResult {
            symbols: vec![script_symbol(
                "C",
                DocumentSymbolKind::Class,
                (0, text.len() + 1),
                (0, 1),
                Vec::new(),
            )],
            status: CapabilityStatus::Supported,
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        };
        assert!(
            script_symbol_values(&out_of_bounds, &LineIndex::new(text), "file:///a.ts", true)
                .is_none()
        );
        let selection_outside = DocumentSymbolsResult {
            symbols: vec![script_symbol(
                "C",
                DocumentSymbolKind::Class,
                (0, 2),
                (0, 9),
                Vec::new(),
            )],
            status: CapabilityStatus::Supported,
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        };
        assert!(
            script_symbol_values(
                &selection_outside,
                &LineIndex::new(text),
                "file:///a.ts",
                false
            )
            .is_none()
        );
    }

    #[test]
    fn selection_chains_include_every_ancestor() {
        let depth = 40_usize;
        let mut text = String::new();
        for _ in 0..depth {
            text.push_str("<view>");
        }
        text.push('x');
        for _ in 0..depth {
            text.push_str("</view>");
        }
        let offset = u32::try_from(text.find('x').expect("content")).expect("offset");
        let chain = facts(&text).selection_chain(offset);
        assert_eq!(chain.len(), depth + 1);
        assert_strictly_nested(&chain);
    }

    #[test]
    fn folding_ranges_cover_every_multiline_element() {
        use std::fmt::Write as _;

        let count = 2_010_usize;
        let mut text = String::new();
        for index in 0..count {
            writeln!(text, "<view id=\"v{index}\">").expect("write");
            text.push_str("</view>");
            text.push('\n');
        }
        let line_index = LineIndex::new(text.as_str());
        let folds = facts(&text).folding_ranges(&line_index);
        assert_eq!(folds.len(), count);
    }

    #[test]
    fn selection_reuses_one_parse_for_many_positions() {
        let text = "<view><text>x</text><image src=\"a\"/></view>";
        reset_parse_count();
        let facts = StructureFacts::parse(text).expect("facts");
        let offsets = [2_u32, 6, 11, 16, 25];
        for offset in offsets {
            assert!(!facts.selection_chain(offset).is_empty());
        }
        assert_eq!(parse_count(), 1);
    }

    #[test]
    fn many_positions_stay_well_below_a_second_in_debug_builds() {
        use std::fmt::Write as _;

        let mut text = String::new();
        for index in 0..2_000 {
            writeln!(text, "<view id=\"v{index}\"><text>x</text></view>").expect("write");
        }
        let facts = StructureFacts::parse(&text).expect("facts");
        let offsets: Vec<u32> = (0..32)
            .map(|index| u32::try_from(index * 97 + 3).expect("offset"))
            .collect();
        let started = std::time::Instant::now();
        for offset in offsets {
            let _ = facts.selection_chain(offset);
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "selection chains took {:?}",
            started.elapsed()
        );
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
