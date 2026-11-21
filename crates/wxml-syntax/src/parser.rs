//! Tolerant, lossless WXML parser and concrete syntax tree.
//!
//! [`parse`] always returns a tree plus diagnostics. Malformed markup never
//! aborts the document: unclosed elements, mismatched tags, stray closing tags
//! and broken mustaches become recoverable nodes with errors attached.
//!
//! Parsing is bounded. Diagnostics are capped at [`crate::MAX_ERRORS`], nodes at
//! [`MAX_NODES`] and markup tokens at [`MAX_MARKUP_TOKENS`]. When a budget is
//! reached the parser stops, records a resource diagnostic and sets
//! [`ParseResult::truncated`]; it never returns a tree that silently looks
//! complete. `WXS` bodies are preserved as raw text and are not parsed as
//! markup or evaluated.
//!
//! Nodes keep exact byte ranges into the original text and the tree owns that
//! text, so `tree.source_text(id)` is always the original slice. This makes the
//! tree usable for lossless edits and for mapping expression ranges back to the
//! template.

use std::fmt;
use std::sync::Arc;

use wxml_text::TextRange;

use crate::error::{ErrorSink, span};
use crate::expr::{Expr, ExprKind};
use crate::lexer::{Token, TokenKind, tokenize_bounded};
use crate::{ParseError, ParseErrorKind};

/// Maximum element nesting depth accepted by the parser.
///
/// Deeper input is flattened into text and reported as
/// [`ParseErrorKind::NestingTooDeep`] instead of recursing further.
pub const MAX_ELEMENT_DEPTH: usize = 256;

/// Maximum number of nodes a syntax tree may contain.
///
/// The document root counts toward the limit. When it is reached, parsing stops
/// with [`ParseErrorKind::TooManyNodes`] and [`ParseResult::truncated`] is set.
pub const MAX_NODES: usize = 100_000;

/// Maximum number of markup tokens the parser will lex.
///
/// Longer input is tokenized up to this limit, reported as
/// [`ParseErrorKind::TooManyTokens`] and flagged as truncated. This keeps the
/// token buffer proportional to a fixed bound instead of the input size.
pub const MAX_MARKUP_TOKENS: usize = 500_000;

/// Parses WXML source into a [`SyntaxTree`] plus diagnostics.
///
/// The result is deterministic: parsing the same text twice yields the same
/// node ranges, kinds and errors. Element nesting is capped at
/// [`MAX_ELEMENT_DEPTH`]; deeper input is flattened into text and reported,
/// never recursed into. Node, token and diagnostic counts are bounded by
/// [`MAX_NODES`], [`MAX_MARKUP_TOKENS`] and [`crate::MAX_ERRORS`].
///
/// `WXS` script bodies are preserved verbatim as a text child and are not
/// parsed as markup; `WXS` evaluation and template/import semantics belong to
/// later phases. Only a lowercase `<wxs>` element starts raw-text mode;
/// `<WXS>` is treated as an ordinary element name.
/// Input larger than the 4 GiB byte-range limit is rejected before
/// copying it into an owned tree; that result has an empty tree and
/// [`ParseResult::truncated`] set.
#[must_use]
pub fn parse(text: &str) -> ParseResult {
    parse_with_limits(text, u32::MAX, MAX_MARKUP_TOKENS)
}

#[cfg(test)]
fn parse_inner(text: &str, byte_limit: u32) -> ParseResult {
    parse_with_limits(text, byte_limit, MAX_MARKUP_TOKENS)
}

#[cfg(test)]
fn parse_with_token_limit(text: &str, token_limit: usize) -> ParseResult {
    parse_with_limits(text, u32::MAX, token_limit)
}

fn parse_with_limits(text: &str, byte_limit: u32, token_limit: usize) -> ParseResult {
    let Ok(text_len) = u32::try_from(text.len()) else {
        return input_too_large();
    };
    if text_len > byte_limit {
        return input_too_large();
    }
    let (tokens, tokens_truncated) = tokenize_bounded(text, token_limit);
    let eof = tokens
        .last()
        .copied()
        .unwrap_or_else(|| Token::new(TokenKind::Eof, span(text_len, text_len)));
    let mut parser = Parser {
        text,
        tokens,
        index: 0,
        eof,
        nodes: vec![document_node()],
        errors: ErrorSink::new(),
        last_end: 0,
        open_names: Vec::new(),
        depth_reported: false,
        node_budget_exhausted: false,
        pending_slots: 0,
        truncated: tokens_truncated,
    };
    if tokens_truncated {
        let cut = parser.eof.range();
        parser.error(
            ParseErrorKind::TooManyTokens,
            cut,
            "markup token budget exhausted; input truncated",
        );
    }
    let mut children = Vec::new();
    while parser.peek().kind() != TokenKind::Eof {
        let Some(node) = parser.parse_node(0) else {
            break;
        };
        children.push(node);
    }
    parser.finish_root(text_len, children);
    ParseResult {
        tree: SyntaxTree {
            text: Arc::from(text),
            nodes: parser.nodes,
            root: NodeId(0),
        },
        errors: parser.errors.into_vec(),
        truncated: parser.truncated,
    }
}

fn input_too_large() -> ParseResult {
    ParseResult {
        tree: SyntaxTree {
            text: Arc::from(""),
            nodes: vec![document_node()],
            root: NodeId(0),
        },
        errors: vec![ParseError::new(
            ParseErrorKind::InputTooLarge,
            span(0, 0),
            "input is larger than the supported 4 GiB byte range",
        )],
        truncated: true,
    }
}

fn document_node() -> Node {
    Node {
        kind: NodeKind::Document,
        range: span(0, 0),
        parent: None,
        children: Vec::new(),
        name: None,
        expr: None,
        closed: false,
        self_closing: false,
    }
}

/// Identifies a node inside a [`SyntaxTree`].
///
/// An id is only meaningful for the tree that produced it. Ids from another
/// tree are usually out of range and return `None`, but a same-index id may
/// alias a different node; there is no generation check. Never mix ids between
/// trees or revisions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(u32);

impl NodeId {
    /// Returns the arena index. Stable only within one tree.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// The category of a syntax node.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    /// The root node covering the whole document.
    Document,
    /// A complete or unclosed element, including its start and end tags.
    Element,
    /// The opening tag, including all attributes.
    StartTag,
    /// The closing tag, when the element is closed.
    EndTag,
    /// An attribute, from its name to the end of its value.
    Attribute,
    /// An attribute value including surrounding quotes when present.
    AttributeValue,
    /// Raw text outside tags or inside a quoted attribute value.
    Text,
    /// A comment including `<!--` and `-->`.
    Comment,
    /// A complete or unclosed `{{ ... }}` interpolation.
    Mustache,
    /// The expression inside a mustache, trimmed of surrounding whitespace.
    MustacheExpr,
}

/// One node of the concrete syntax tree.
#[derive(Debug)]
pub struct Node {
    kind: NodeKind,
    range: TextRange,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    name: Option<TextRange>,
    expr: Option<Box<Expr>>,
    closed: bool,
    self_closing: bool,
}

impl Node {
    /// Returns the node category.
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        self.kind
    }

    /// Returns the node's exact source range.
    #[must_use]
    pub const fn text_range(&self) -> TextRange {
        self.range
    }

    /// Returns the parent node, or `None` for the document root.
    #[must_use]
    pub const fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Returns direct children in source order.
    #[must_use]
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    /// Returns the range of this node's name, for elements, tags and
    /// attributes.
    #[must_use]
    pub const fn name_range(&self) -> Option<TextRange> {
        self.name
    }

    /// Returns the parsed expression for a [`NodeKind::MustacheExpr`] node.
    #[must_use]
    pub fn expr(&self) -> Option<&Expr> {
        self.expr.as_deref()
    }

    /// Returns whether the parser saw the construct's closing delimiter.
    ///
    /// For [`NodeKind::Element`] this means an explicit end tag was present
    /// (self-closing elements return `false`). For [`NodeKind::Mustache`] it
    /// means `}}` was found. The flag comes from parsing, not from inspecting
    /// the source text afterwards, so literals such as `{{ {a: {b: 1}}` are not
    /// mistaken for closed constructs.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// Returns whether an element's start tag ended with `/>`.
    #[must_use]
    pub const fn is_self_closing(&self) -> bool {
        self.self_closing
    }
}

/// Owns the parsed document and all of its nodes.
///
/// Nodes are stored in an arena and refer to each other by [`NodeId`]. The tree
/// owns an `Arc<str>` snapshot of the text it was parsed from, so ranges never
/// refer to a different revision.
pub struct SyntaxTree {
    text: Arc<str>,
    nodes: Vec<Node>,
    root: NodeId,
}

impl SyntaxTree {
    /// Returns the parsed source text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the document root.
    #[must_use]
    pub const fn root(&self) -> NodeId {
        self.root
    }

    /// Returns a node by id, or `None` when the index is out of range.
    ///
    /// Ids carry no tree identity. Passing an id from another tree can return a
    /// different node at the same index; only use ids with the tree that
    /// produced them.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(node_index(id))
    }

    /// Returns the kind of a node.
    #[must_use]
    pub fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.node(id).map(Node::kind)
    }

    /// Returns the exact source range of a node.
    #[must_use]
    pub fn text_range(&self, id: NodeId) -> Option<TextRange> {
        self.node(id).map(Node::text_range)
    }

    /// Returns the direct children of a node, or an empty slice for an unknown
    /// id.
    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.node(id).map_or(&[], |node| node.children())
    }

    /// Returns the parent of a node.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).and_then(Node::parent)
    }

    /// Returns the name range of an element, tag or attribute.
    #[must_use]
    pub fn name_range(&self, id: NodeId) -> Option<TextRange> {
        self.node(id).and_then(Node::name_range)
    }

    /// Returns the name of an element, tag or attribute.
    #[must_use]
    pub fn name(&self, id: NodeId) -> Option<&str> {
        let range = self.name_range(id)?;
        self.slice(range)
    }

    /// Returns the original source text covered by a node.
    #[must_use]
    pub fn source_text(&self, id: NodeId) -> Option<&str> {
        let range = self.text_range(id)?;
        self.slice(range)
    }

    /// Returns the parsed expression of a mustache expression node.
    #[must_use]
    pub fn expr(&self, id: NodeId) -> Option<&Expr> {
        self.node(id).and_then(Node::expr)
    }

    /// Iterates over every node and its id in arena order.
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node_id(index), node))
    }

    /// Iterates over a node and all of its descendants in pre-order.
    #[must_use]
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            tree: self,
            stack: vec![id],
        }
    }

    /// Iterates over every mustache node in the document.
    pub fn mustaches(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.descendants(self.root)
            .filter(|id| self.kind(*id) == Some(NodeKind::Mustache))
    }

    fn slice(&self, range: TextRange) -> Option<&str> {
        let start = usize::try_from(range.start()).ok()?;
        let end = usize::try_from(range.end()).ok()?;
        self.text.get(start..end)
    }
}

impl fmt::Debug for SyntaxTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyntaxTree")
            .field("nodes", &self.nodes.len())
            .field("root", &self.root)
            .field("text_len", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// Pre-order iterator over a node and its descendants.
#[derive(Clone, Debug)]
pub struct Descendants<'a> {
    tree: &'a SyntaxTree,
    stack: Vec<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.stack.pop()?;
        self.stack
            .extend(self.tree.children(id).iter().rev().copied());
        Some(id)
    }
}

/// Result of [`parse`].
#[derive(Debug)]
pub struct ParseResult {
    /// The syntax tree, always usable even when errors were reported.
    pub tree: SyntaxTree,
    /// Problems found while parsing, in discovery order, capped at
    /// [`crate::MAX_ERRORS`] plus a final [`ParseErrorKind::TooManyErrors`] sentinel.
    pub errors: Vec<ParseError>,
    /// `true` when the token or node budget stopped parsing early.
    ///
    /// The tree then contains only the nodes committed before the cut and must
    /// not be presented as a complete parse. Errors contain a resource
    /// diagnostic unless the error budget was already exhausted.
    ///
    /// Diagnostic truncation is separate: it is reported by a final
    /// [`ParseErrorKind::TooManyErrors`] entry and does not set this flag.
    pub truncated: bool,
}

impl ParseResult {
    /// Returns `true` when the document parsed without errors or truncation.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.errors.is_empty() && !self.truncated
    }

    /// Returns `true` when a resource budget stopped parsing early.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }
}

fn node_index(id: NodeId) -> usize {
    usize::try_from(id.0).unwrap_or(usize::MAX)
}

fn node_id(index: usize) -> NodeId {
    NodeId(u32::try_from(index).unwrap_or(u32::MAX))
}

fn u32_from(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

enum CloseAction {
    Closed(u32),
    Unclosed(u32),
    Text,
}

struct StartTagInfo {
    node: NodeId,
    end: u32,
    self_closing: bool,
    terminated: bool,
}

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    index: usize,
    eof: Token,
    nodes: Vec<Node>,
    errors: ErrorSink,
    last_end: u32,
    open_names: Vec<Option<TextRange>>,
    depth_reported: bool,
    node_budget_exhausted: bool,
    pending_slots: usize,
    truncated: bool,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Token {
        self.tokens.get(self.index).copied().unwrap_or(self.eof)
    }

    fn peek_at(&self, offset: usize) -> Token {
        self.tokens
            .get(self.index.saturating_add(offset))
            .copied()
            .unwrap_or(self.eof)
    }

    fn bump(&mut self) -> Token {
        let token = self.peek();
        if token.kind() != TokenKind::Eof {
            self.index += 1;
            self.last_end = token.range().end();
        }
        token
    }

    fn slice(&self, range: TextRange) -> &'a str {
        let start = usize::try_from(range.start()).unwrap_or(usize::MAX);
        let end = usize::try_from(range.end()).unwrap_or(usize::MAX);
        self.text.get(start..end).unwrap_or_default()
    }

    fn error(&mut self, kind: ParseErrorKind, range: TextRange, message: &'static str) {
        self.errors.push(kind, range, message);
    }

    fn note_node_budget(&mut self, range: TextRange) {
        if self.node_budget_exhausted {
            return;
        }
        self.node_budget_exhausted = true;
        self.truncated = true;
        self.error(
            ParseErrorKind::TooManyNodes,
            range,
            "node budget exhausted; tree truncated",
        );
    }

    /// Creates a node that consumes the regular budget.
    fn finish_node(
        &mut self,
        kind: NodeKind,
        range: TextRange,
        name: Option<TextRange>,
        expr: Option<Box<Expr>>,
        children: Vec<NodeId>,
    ) -> Option<NodeId> {
        if self.node_budget_exhausted {
            return None;
        }
        if self.nodes.len().saturating_add(self.pending_slots) >= MAX_NODES {
            self.note_node_budget(range);
            return None;
        }
        Some(self.push_node(kind, range, name, expr, children))
    }

    /// Creates a node covered by a reserved slot for an open element.
    ///
    /// Reserving the end tag and element slots keeps complete ancestors
    /// constructible after the regular budget is exhausted, so a truncated
    /// tree retains committed children instead of collapsing to the root.
    fn finish_reserved_node(
        &mut self,
        kind: NodeKind,
        range: TextRange,
        name: Option<TextRange>,
        expr: Option<Box<Expr>>,
        children: Vec<NodeId>,
    ) -> Option<NodeId> {
        if self.nodes.len() >= MAX_NODES {
            self.note_node_budget(range);
            return None;
        }
        Some(self.push_node(kind, range, name, expr, children))
    }

    fn push_node(
        &mut self,
        kind: NodeKind,
        range: TextRange,
        name: Option<TextRange>,
        expr: Option<Box<Expr>>,
        children: Vec<NodeId>,
    ) -> NodeId {
        let id = node_id(self.nodes.len());
        for child in &children {
            if let Some(node) = self.nodes.get_mut(node_index(*child)) {
                node.parent = Some(id);
            }
        }
        self.nodes.push(Node {
            kind,
            range,
            parent: None,
            children,
            name,
            expr,
            closed: false,
            self_closing: false,
        });
        id
    }

    fn rollback(&mut self, mark: usize) {
        self.nodes.truncate(mark);
    }

    fn set_closed(&mut self, id: NodeId, closed: bool) {
        if let Some(node) = self.nodes.get_mut(node_index(id)) {
            node.closed = closed;
        }
    }

    fn set_self_closing(&mut self, id: NodeId, self_closing: bool) {
        if let Some(node) = self.nodes.get_mut(node_index(id)) {
            node.self_closing = self_closing;
        }
    }

    fn finish_root(&mut self, text_len: u32, children: Vec<NodeId>) {
        for child in &children {
            if let Some(node) = self.nodes.get_mut(node_index(*child)) {
                node.parent = Some(NodeId(0));
            }
        }
        if let Some(root) = self.nodes.get_mut(0) {
            root.range = span(0, text_len);
            root.children = children;
        }
    }

    fn node_range(&self, id: NodeId) -> TextRange {
        self.nodes
            .get(node_index(id))
            .map_or_else(|| span(0, 0), Node::text_range)
    }

    /// Parses one node at `depth`. Every branch consumes at least one token and
    /// returns `None` only when the node budget is exhausted.
    fn parse_node(&mut self, depth: usize) -> Option<NodeId> {
        match self.peek().kind() {
            TokenKind::Text | TokenKind::RawText => {
                let token = self.bump();
                self.finish_node(NodeKind::Text, token.range(), None, None, Vec::new())
            }
            TokenKind::Comment => {
                let token = self.bump();
                self.finish_node(NodeKind::Comment, token.range(), None, None, Vec::new())
            }
            TokenKind::UnterminatedComment => {
                let token = self.bump();
                self.error(
                    ParseErrorKind::UnclosedComment,
                    token.range(),
                    "comment is not terminated",
                );
                self.finish_node(NodeKind::Comment, token.range(), None, None, Vec::new())
            }
            TokenKind::Mustache | TokenKind::UnterminatedMustache => self.parse_mustache(),
            TokenKind::TagOpen => {
                if depth >= MAX_ELEMENT_DEPTH {
                    self.degrade_tag()
                } else {
                    self.parse_element(depth)
                }
            }
            TokenKind::TagClose => self.consume_closing_as_text(),
            _ => {
                let token = self.bump();
                self.error(
                    ParseErrorKind::UnexpectedToken,
                    token.range(),
                    "unexpected token in document content",
                );
                self.finish_node(NodeKind::Text, token.range(), None, None, Vec::new())
            }
        }
    }

    /// Flattens an over-deep opening tag into text so recursion stays bounded.
    fn degrade_tag(&mut self) -> Option<NodeId> {
        let start = self.peek().range();
        if !self.depth_reported {
            self.depth_reported = true;
            self.error(
                ParseErrorKind::NestingTooDeep,
                start,
                "element nesting exceeds the supported depth",
            );
        }
        let mut end = start.end();
        loop {
            match self.peek().kind() {
                TokenKind::Eof => break,
                TokenKind::TagEnd | TokenKind::TagSelfClose => {
                    end = self.bump().range().end();
                    break;
                }
                _ => {
                    end = self.bump().range().end();
                }
            }
        }
        self.finish_node(
            NodeKind::Text,
            span(start.start(), end),
            None,
            None,
            Vec::new(),
        )
    }

    fn consume_closing_as_text(&mut self) -> Option<NodeId> {
        let close = self.bump();
        let start = close.range().start();
        let mut end = close.range().end();
        let name = self.peek();
        if name.kind() == TokenKind::Name {
            end = self.bump().range().end();
        }
        if self.peek().kind() == TokenKind::TagEnd {
            end = self.bump().range().end();
        }
        self.error(
            ParseErrorKind::UnexpectedClosingTag,
            span(start, end),
            "closing tag has no matching open element",
        );
        self.finish_node(NodeKind::Text, span(start, end), None, None, Vec::new())
    }

    fn parse_element(&mut self, depth: usize) -> Option<NodeId> {
        let mark = self.nodes.len();
        self.pending_slots = self.pending_slots.saturating_add(2);
        let open = self.bump();
        let name = self.parse_tag_name();
        let element_start = open.range().start();
        let Some(start) = self.parse_start_tag(element_start, name) else {
            self.pending_slots = self.pending_slots.saturating_sub(2);
            self.rollback(mark);
            return None;
        };
        let mut children = vec![start.node];
        let (end, closed) = if start.self_closing || !start.terminated {
            (start.end, false)
        } else if name.is_some_and(|range| self.slice(range) == "wxs") {
            self.parse_wxs_content(name, &mut children)
        } else {
            self.parse_element_content(name, depth, &mut children)
        };
        let element = self.finish_reserved_node(
            NodeKind::Element,
            span(element_start, end),
            name,
            None,
            children,
        );
        self.pending_slots = self.pending_slots.saturating_sub(2);
        let Some(element) = element else {
            self.rollback(mark);
            return None;
        };
        self.set_closed(element, closed);
        self.set_self_closing(element, start.self_closing);
        Some(element)
    }

    fn parse_tag_name(&mut self) -> Option<TextRange> {
        if self.peek().kind() == TokenKind::Name {
            return Some(self.bump().range());
        }
        self.error(
            ParseErrorKind::UnexpectedToken,
            self.peek().range(),
            "tag is missing a name",
        );
        None
    }

    fn parse_start_tag(
        &mut self,
        element_start: u32,
        name: Option<TextRange>,
    ) -> Option<StartTagInfo> {
        let mark = self.nodes.len();
        let mut attributes = Vec::new();
        let (end, self_closing, terminated) = loop {
            match self.peek().kind() {
                TokenKind::Name => {
                    let Some(attribute) = self.parse_attribute() else {
                        break (self.last_end, false, false);
                    };
                    attributes.push(attribute);
                }
                TokenKind::TagEnd => break (self.bump().range().end(), false, true),
                TokenKind::TagSelfClose => break (self.bump().range().end(), true, true),
                TokenKind::Eof => {
                    self.error(
                        ParseErrorKind::UnclosedTag,
                        name.unwrap_or_else(|| span(element_start, self.last_end)),
                        "tag is not terminated",
                    );
                    break (self.last_end, false, false);
                }
                _ => {
                    let token = self.bump();
                    self.error(
                        ParseErrorKind::UnexpectedToken,
                        token.range(),
                        "unexpected token inside a tag",
                    );
                }
            }
        };
        let Some(node) = self.finish_node(
            NodeKind::StartTag,
            span(element_start, end),
            name,
            None,
            attributes,
        ) else {
            self.rollback(mark);
            return None;
        };
        Some(StartTagInfo {
            node,
            end,
            self_closing,
            terminated,
        })
    }

    fn parse_element_content(
        &mut self,
        name: Option<TextRange>,
        depth: usize,
        children: &mut Vec<NodeId>,
    ) -> (u32, bool) {
        self.open_names.push(name);
        let mut closed = false;
        let end = loop {
            match self.peek().kind() {
                TokenKind::Eof => {
                    if let Some(range) = name {
                        self.error(
                            ParseErrorKind::UnclosedTag,
                            range,
                            "element is not closed before the end of the input",
                        );
                    }
                    break self.last_end;
                }
                TokenKind::TagClose => match self.handle_closing_tag(name, children) {
                    CloseAction::Closed(end) => {
                        closed = true;
                        break end;
                    }
                    CloseAction::Unclosed(end) => break end,
                    CloseAction::Text => {}
                },
                _ => {
                    let Some(child) = self.parse_node(depth.saturating_add(1)) else {
                        break self.last_end;
                    };
                    children.push(child);
                }
            }
        };
        self.open_names.pop();
        (end, closed)
    }

    /// Consumes the raw body token and closing tag of a `WXS` element.
    ///
    /// The lexer emits one [`TokenKind::RawText`] token for the body, so this
    /// never re-lexes the document and cannot swallow following siblings.
    /// Returns the element end offset and whether the closing tag was found.
    ///
    /// If the token budget stopped before the raw body token, the element is
    /// reported as unclosed; [`ParseResult::truncated`] is already set, so the
    /// partial tree is not presented as complete.
    fn parse_wxs_content(
        &mut self,
        name: Option<TextRange>,
        children: &mut Vec<NodeId>,
    ) -> (u32, bool) {
        let mut end = self.last_end;
        if self.peek().kind() == TokenKind::RawText {
            let token = self.bump();
            end = end.max(token.range().end());
            if !token.range().is_empty()
                && let Some(text) =
                    self.finish_node(NodeKind::Text, token.range(), None, None, Vec::new())
            {
                children.push(text);
            }
        }
        if self.peek().kind() == TokenKind::TagClose {
            let close_start = self.peek().range().start();
            self.bump();
            if self.peek().kind() == TokenKind::Name {
                let close_name = self.bump();
                let name_range = close_name.range();
                let mut tag_end = name_range.end();
                match self.peek().kind() {
                    TokenKind::TagEnd | TokenKind::TagSelfClose => {
                        tag_end = self.bump().range().end();
                    }
                    _ => {
                        self.error(
                            ParseErrorKind::UnexpectedToken,
                            self.peek().range(),
                            "closing tag is missing '>'",
                        );
                    }
                }
                let Some(end_tag) = self.finish_reserved_node(
                    NodeKind::EndTag,
                    span(close_start, tag_end),
                    Some(name_range),
                    None,
                    Vec::new(),
                ) else {
                    self.last_end = self.last_end.max(tag_end);
                    return (tag_end, false);
                };
                children.push(end_tag);
                self.last_end = self.last_end.max(tag_end);
                return (tag_end, true);
            }
        }
        while self.peek().kind() != TokenKind::Eof {
            self.bump();
        }
        end = end.max(u32_from(self.text.len()));
        self.last_end = self.last_end.max(end);
        if let Some(range) = name {
            self.error(
                ParseErrorKind::UnclosedTag,
                range,
                "element is not closed before the end of the input",
            );
        }
        (end, false)
    }

    fn handle_closing_tag(
        &mut self,
        name: Option<TextRange>,
        children: &mut Vec<NodeId>,
    ) -> CloseAction {
        let close = self.peek();
        let close_name = self.peek_at(1);
        if close_name.kind() != TokenKind::Name {
            self.bump();
            self.error(
                ParseErrorKind::UnexpectedClosingTag,
                close.range(),
                "closing tag has no matching open element",
            );
            return CloseAction::Text;
        }
        let close_name_range = close_name.range();
        if name.is_some_and(|range| self.names_equal(range, close_name_range)) {
            let close_start = close.range().start();
            self.bump();
            self.bump();
            let end_tag_end = if self.peek().kind() == TokenKind::TagEnd {
                self.bump().range().end()
            } else {
                self.error(
                    ParseErrorKind::UnexpectedToken,
                    self.peek().range(),
                    "closing tag is missing '>'",
                );
                close_name_range.end()
            };
            let Some(end_tag) = self.finish_reserved_node(
                NodeKind::EndTag,
                span(close_start, end_tag_end),
                Some(close_name_range),
                None,
                Vec::new(),
            ) else {
                return CloseAction::Unclosed(self.last_end);
            };
            children.push(end_tag);
            return CloseAction::Closed(end_tag_end);
        }
        if self.is_ancestor(close_name_range) {
            if let Some(range) = name {
                self.error(
                    ParseErrorKind::UnclosedTag,
                    range,
                    "element is not closed before its parent closes",
                );
            }
            return CloseAction::Unclosed(self.last_end);
        }
        let mismatch_start = close.range().start();
        let mismatch_range = span(mismatch_start, close_name_range.end());
        self.bump();
        self.bump();
        let mismatch_end = if self.peek().kind() == TokenKind::TagEnd {
            self.bump().range().end()
        } else {
            close_name_range.end()
        };
        self.error(
            ParseErrorKind::MismatchedTag,
            mismatch_range,
            "closing tag does not match the open element",
        );
        let Some(text_node) = self.finish_node(
            NodeKind::Text,
            span(mismatch_start, mismatch_end),
            None,
            None,
            Vec::new(),
        ) else {
            return CloseAction::Unclosed(self.last_end);
        };
        children.push(text_node);
        CloseAction::Text
    }

    fn is_ancestor(&self, range: TextRange) -> bool {
        self.open_names
            .iter()
            .rev()
            .skip(1)
            .flatten()
            .any(|open| self.names_equal(*open, range))
    }

    fn names_equal(&self, left: TextRange, right: TextRange) -> bool {
        self.slice(left) == self.slice(right)
    }

    fn parse_attribute(&mut self) -> Option<NodeId> {
        let mark = self.nodes.len();
        let token = self.bump();
        let name = token.range();
        let start = name.start();
        let mut end = name.end();
        let mut value = None;
        if self.peek().kind() == TokenKind::Equals {
            let equals = self.bump();
            if let Some(value_node) = self.parse_attribute_value() {
                end = self.node_range(value_node).end();
                value = Some(value_node);
            } else if self.node_budget_exhausted {
                self.rollback(mark);
                return None;
            } else {
                self.error(
                    ParseErrorKind::MissingAttributeValue,
                    equals.range(),
                    "attribute is missing a value",
                );
                end = equals.range().end();
            }
        }
        let attribute = self.finish_node(
            NodeKind::Attribute,
            span(start, end),
            Some(name),
            None,
            value.into_iter().collect(),
        );
        if attribute.is_none() {
            self.rollback(mark);
        }
        attribute
    }

    fn parse_attribute_value(&mut self) -> Option<NodeId> {
        match self.peek().kind() {
            TokenKind::Quote(quote) => self.parse_quoted_value(quote),
            TokenKind::Mustache | TokenKind::UnterminatedMustache | TokenKind::BareValue => {
                self.parse_unquoted_value()
            }
            _ => None,
        }
    }

    fn parse_quoted_value(&mut self, quote: char) -> Option<NodeId> {
        let mark = self.nodes.len();
        let open = self.bump();
        let start = open.range().start();
        let mut children = Vec::new();
        let mut end = open.range().end();
        loop {
            match self.peek().kind() {
                TokenKind::Quote(found) if found == quote => {
                    end = self.bump().range().end();
                    break;
                }
                TokenKind::Eof => {
                    self.error(
                        ParseErrorKind::UnclosedAttributeValue,
                        span(start, end),
                        "quoted attribute value is not terminated",
                    );
                    break;
                }
                TokenKind::ValueText | TokenKind::Quote(_) => {
                    let token = self.bump();
                    end = token.range().end();
                    let Some(text) =
                        self.finish_node(NodeKind::Text, token.range(), None, None, Vec::new())
                    else {
                        self.rollback(mark);
                        return None;
                    };
                    children.push(text);
                }
                TokenKind::Mustache | TokenKind::UnterminatedMustache => {
                    let Some(node) = self.parse_mustache() else {
                        self.rollback(mark);
                        return None;
                    };
                    end = self.node_range(node).end();
                    children.push(node);
                }
                _ => {
                    let token = self.bump();
                    end = token.range().end();
                    self.error(
                        ParseErrorKind::UnexpectedToken,
                        token.range(),
                        "unexpected token inside an attribute value",
                    );
                }
            }
        }
        let value = self.finish_node(
            NodeKind::AttributeValue,
            span(start, end),
            None,
            None,
            children,
        );
        if value.is_none() {
            self.rollback(mark);
        }
        value
    }

    fn parse_unquoted_value(&mut self) -> Option<NodeId> {
        let mark = self.nodes.len();
        let start = self.peek().range().start();
        let mut children = Vec::new();
        let mut end = start;
        loop {
            match self.peek().kind() {
                TokenKind::BareValue | TokenKind::ValueText => {
                    let token = self.bump();
                    end = token.range().end();
                    let Some(text) =
                        self.finish_node(NodeKind::Text, token.range(), None, None, Vec::new())
                    else {
                        self.rollback(mark);
                        return None;
                    };
                    children.push(text);
                }
                TokenKind::Mustache | TokenKind::UnterminatedMustache => {
                    let Some(node) = self.parse_mustache() else {
                        self.rollback(mark);
                        return None;
                    };
                    end = self.node_range(node).end();
                    children.push(node);
                }
                _ => break,
            }
        }
        let value = self.finish_node(
            NodeKind::AttributeValue,
            span(start, end),
            None,
            None,
            children,
        );
        if value.is_none() {
            self.rollback(mark);
        }
        value
    }

    fn parse_mustache(&mut self) -> Option<NodeId> {
        let mark = self.nodes.len();
        let token = self.bump();
        let range = token.range();
        let terminated = token.kind() == TokenKind::Mustache;
        if !terminated {
            self.error(
                ParseErrorKind::UnclosedMustache,
                range,
                "mustache is not terminated",
            );
        }
        let inner_end = if terminated {
            range.end().saturating_sub(2)
        } else {
            range.end()
        };
        let inner_start = range.start().saturating_add(2);
        let inner = span(inner_start.min(inner_end), inner_end);
        let trimmed = self.trim_range(inner);
        let expr = if trimmed.is_empty() {
            if terminated {
                self.error(
                    ParseErrorKind::EmptyMustache,
                    trimmed,
                    "mustache contains no expression",
                );
            }
            Expr::new(ExprKind::Missing, trimmed)
        } else {
            let result = crate::expr::parse_expression(self.slice(trimmed), trimmed.start());
            self.errors.extend(result.errors);
            result.expr
        };
        let Some(expr_node) = self.finish_node(
            NodeKind::MustacheExpr,
            trimmed,
            None,
            Some(Box::new(expr)),
            Vec::new(),
        ) else {
            self.rollback(mark);
            return None;
        };
        let mustache = self.finish_node(NodeKind::Mustache, range, None, None, vec![expr_node]);
        if let Some(id) = mustache {
            self.set_closed(id, terminated);
        } else {
            self.rollback(mark);
        }
        mustache
    }

    fn trim_range(&self, range: TextRange) -> TextRange {
        let slice = self.slice(range);
        let leading = slice.len().saturating_sub(slice.trim_start().len());
        let trailing = slice.len().saturating_sub(slice.trim_end().len());
        span(
            range.start().saturating_add(u32_from(leading)),
            range.end().saturating_sub(u32_from(trailing)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_MARKUP_TOKENS, MAX_NODES, NodeId, NodeKind, SyntaxTree, parse, parse_inner,
        parse_with_token_limit,
    };
    use crate::ParseErrorKind;
    use crate::ast::Mustache;
    use crate::error::MAX_ERRORS;
    use crate::expr::ExprKind;
    use wxml_text::TextRange;

    fn at(offset: u32) -> usize {
        usize::try_from(offset).expect("offset fits in usize")
    }

    fn u32_len(text: &str) -> u32 {
        u32::try_from(text.len()).expect("test input fits in u32")
    }

    fn parse_clean(source: &str) -> SyntaxTree {
        let result = parse(source);
        assert!(
            result.errors.is_empty(),
            "expected no errors for {source:?}, got {:?}",
            result.errors
        );
        result.tree
    }

    fn child_kinds(tree: &SyntaxTree, parent: NodeId) -> Vec<NodeKind> {
        tree.children(parent)
            .iter()
            .filter_map(|id| tree.kind(*id))
            .collect()
    }

    fn find_child(tree: &SyntaxTree, parent: NodeId, kind: NodeKind) -> Option<NodeId> {
        tree.children(parent)
            .iter()
            .copied()
            .find(|id| tree.kind(*id) == Some(kind))
    }

    fn children_of_kind(tree: &SyntaxTree, parent: NodeId, kind: NodeKind) -> Vec<NodeId> {
        tree.children(parent)
            .iter()
            .copied()
            .filter(|id| tree.kind(*id) == Some(kind))
            .collect()
    }

    /// Flattens the tree in pre-order together with each node's raw source
    /// text; this is the golden representation used by structure tests.
    fn flatten(tree: &SyntaxTree) -> Vec<(NodeKind, &str)> {
        tree.descendants(tree.root())
            .map(|id| {
                (
                    tree.kind(id).expect("node kind"),
                    tree.source_text(id).expect("node text"),
                )
            })
            .collect()
    }

    fn assert_ranges(tree: &SyntaxTree) {
        let source = tree.text();
        let reachable: std::collections::HashSet<NodeId> = tree.descendants(tree.root()).collect();
        for (id, node) in tree.nodes() {
            assert!(reachable.contains(&id), "orphan node {id:?}");
            let range = node.text_range();
            assert!(range.start() <= range.end());
            assert!(at(range.end()) <= source.len());
            assert!(source.is_char_boundary(at(range.start())));
            assert!(source.is_char_boundary(at(range.end())));
            if let Some(parent) = node.parent() {
                let parent_range = tree.text_range(parent).expect("parent range");
                assert!(
                    parent_range.start() <= range.start() && range.end() <= parent_range.end(),
                    "child {range:?} outside parent {parent_range:?}"
                );
            }
            for child in node.children() {
                let child_range = tree.text_range(*child).expect("child range");
                assert!(
                    range.start() <= child_range.start() && child_range.end() <= range.end(),
                    "child {child_range:?} outside node {range:?}"
                );
            }
        }
    }

    #[test]
    fn empty_input_produces_an_empty_document() {
        let result = parse("");
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let tree = result.tree;
        assert_eq!(tree.text(), "");
        assert_eq!(tree.kind(tree.root()), Some(NodeKind::Document));
        assert_eq!(
            tree.text_range(tree.root()),
            Some(TextRange::new(0, 0).expect("range"))
        );
        assert!(
            tree.children(tree.root()).is_empty(),
            "unexpected children: {:?}",
            tree.children(tree.root())
        );
    }

    #[test]
    fn builds_element_start_and_end_tags() {
        let source = r#"<view id="a">hi</view>"#;
        let tree = parse_clean(source);
        let root = tree.root();
        assert_eq!(tree.children(root).len(), 1);
        let element = tree.children(root)[0];
        assert_eq!(tree.kind(element), Some(NodeKind::Element));
        assert_eq!(tree.name(element), Some("view"));
        assert_eq!(
            child_kinds(&tree, element),
            vec![NodeKind::StartTag, NodeKind::Text, NodeKind::EndTag]
        );
        assert_eq!(tree.source_text(element), Some(source));

        let start = tree.children(element)[0];
        assert_eq!(tree.source_text(start), Some(r#"<view id="a">"#));
        let attribute = tree.children(start)[0];
        assert_eq!(tree.kind(attribute), Some(NodeKind::Attribute));
        assert_eq!(tree.name(attribute), Some("id"));
        assert_eq!(tree.source_text(attribute), Some(r#"id="a""#));
        let value = tree.children(attribute)[0];
        assert_eq!(tree.kind(value), Some(NodeKind::AttributeValue));
        assert_eq!(tree.source_text(value), Some(r#""a""#));
        let text_node = tree.children(element)[1];
        assert_eq!(tree.source_text(text_node), Some("hi"));
        let end = tree.children(element)[2];
        assert_eq!(tree.name(end), Some("view"));
        assert_eq!(tree.source_text(end), Some("</view>"));
        assert_ranges(&tree);
    }

    #[test]
    fn self_closing_elements_have_no_end_tag() {
        let tree = parse_clean("<input/>");
        let element = tree.children(tree.root())[0];
        assert_eq!(tree.name(element), Some("input"));
        assert_eq!(child_kinds(&tree, element), vec![NodeKind::StartTag]);
        assert!(
            tree.source_text(element)
                .is_some_and(|text| text.ends_with("/>"))
        );
        assert_ranges(&tree);
    }

    #[test]
    fn nested_elements_and_mustache_expressions() {
        let source = "<view>{{ user.name }}</view>";
        let tree = parse_clean(source);
        let view = tree.children(tree.root())[0];
        let mustache = find_child(&tree, view, NodeKind::Mustache).expect("mustache node");
        assert_eq!(tree.source_text(mustache), Some("{{ user.name }}"));

        let expr_node = tree.children(mustache)[0];
        assert_eq!(tree.kind(expr_node), Some(NodeKind::MustacheExpr));
        assert_eq!(tree.source_text(expr_node), Some("user.name"));
        let expr = tree.expr(expr_node).expect("parsed expression");
        let ExprKind::Member {
            object, property, ..
        } = expr.kind()
        else {
            panic!("expected member access, got {:?}", expr.kind());
        };
        let ExprKind::Identifier { name } = object.kind() else {
            panic!("expected an identifier, got {:?}", object.kind());
        };
        assert_eq!(&source[at(name.start())..at(name.end())], "user");
        let ExprKind::Identifier { name } = property.kind() else {
            panic!("expected an identifier, got {:?}", property.kind());
        };
        assert_eq!(&source[at(name.start())..at(name.end())], "name");
        assert_ranges(&tree);
    }

    #[test]
    fn golden_document_shape() {
        let source = "<!-- header -->\n<view class=\"page\" wx:if=\"{{ user.active }}\">\n  <text>{{ user.name }}</text>\n</view>";
        let tree = parse_clean(source);
        let expected = vec![
            (NodeKind::Document, source),
            (NodeKind::Comment, "<!-- header -->"),
            (NodeKind::Text, "\n"),
            (
                NodeKind::Element,
                "<view class=\"page\" wx:if=\"{{ user.active }}\">\n  <text>{{ user.name }}</text>\n</view>",
            ),
            (
                NodeKind::StartTag,
                "<view class=\"page\" wx:if=\"{{ user.active }}\">",
            ),
            (NodeKind::Attribute, "class=\"page\""),
            (NodeKind::AttributeValue, "\"page\""),
            (NodeKind::Text, "page"),
            (NodeKind::Attribute, "wx:if=\"{{ user.active }}\""),
            (NodeKind::AttributeValue, "\"{{ user.active }}\""),
            (NodeKind::Mustache, "{{ user.active }}"),
            (NodeKind::MustacheExpr, "user.active"),
            (NodeKind::Text, "\n  "),
            (NodeKind::Element, "<text>{{ user.name }}</text>"),
            (NodeKind::StartTag, "<text>"),
            (NodeKind::Mustache, "{{ user.name }}"),
            (NodeKind::MustacheExpr, "user.name"),
            (NodeKind::EndTag, "</text>"),
            (NodeKind::Text, "\n"),
            (NodeKind::EndTag, "</view>"),
        ];
        assert_eq!(flatten(&tree), expected);
        assert_ranges(&tree);
    }

    #[test]
    fn unclosed_elements_report_errors_but_stay_in_the_tree() {
        let result = parse("<view>");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnclosedTag)
        );
        let tree = result.tree;
        let element = tree.children(tree.root())[0];
        assert_eq!(tree.name(element), Some("view"));
        assert_eq!(
            tree.text_range(element),
            Some(TextRange::new(0, u32_len("<view>")).expect("range"))
        );
        assert_eq!(child_kinds(&tree, element), vec![NodeKind::StartTag]);
        assert_ranges(&tree);
    }

    #[test]
    fn unclosed_nested_element_unwinds_to_its_parent() {
        let source = "<view><text></view>";
        let result = parse(source);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnclosedTag)
        );
        let tree = result.tree;
        let view = tree.children(tree.root())[0];
        assert_eq!(tree.name(view), Some("view"));
        let text_element = find_child(&tree, view, NodeKind::Element).expect("text element");
        assert_eq!(tree.name(text_element), Some("text"));
        assert_eq!(child_kinds(&tree, text_element), vec![NodeKind::StartTag]);
        let end = find_child(&tree, view, NodeKind::EndTag).expect("view end tag");
        assert_eq!(tree.name(end), Some("view"));
        assert_ranges(&tree);
    }

    #[test]
    fn mismatched_closing_tags_become_text_and_errors() {
        let source = "<view></text></view>";
        let result = parse(source);
        let mismatches = result
            .errors
            .iter()
            .filter(|error| error.kind() == ParseErrorKind::MismatchedTag)
            .count();
        assert_eq!(mismatches, 1, "errors: {:?}", result.errors);
        let tree = result.tree;
        let view = tree.children(tree.root())[0];
        assert!(
            children_of_kind(&tree, view, NodeKind::Text)
                .iter()
                .any(|id| tree.source_text(*id) == Some("</text>"))
        );
        assert_eq!(tree.name(view), Some("view"));
        assert_ranges(&tree);
    }

    #[test]
    fn unexpected_closing_tags_at_top_level_are_reported() {
        let source = "</view>";
        let result = parse(source);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnexpectedClosingTag)
        );
        let tree = result.tree;
        assert_eq!(tree.source_text(tree.root()), Some(source));
        assert_ranges(&tree);
    }

    #[test]
    fn comment_tokens_become_comment_nodes() {
        let tree = parse_clean("a<!-- note -->b");
        let root = tree.root();
        assert_eq!(
            child_kinds(&tree, root),
            vec![NodeKind::Text, NodeKind::Comment, NodeKind::Text]
        );
        assert_eq!(
            tree.source_text(tree.children(root)[1]),
            Some("<!-- note -->")
        );
    }

    #[test]
    fn unterminated_comments_are_reported() {
        let result = parse("<!-- note");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnclosedComment)
        );
        assert_eq!(
            result.tree.kind(result.tree.root()),
            Some(NodeKind::Document)
        );
        assert_ranges(&result.tree);
    }

    #[test]
    fn crlf_is_preserved_in_text_nodes() {
        let source = "a\r\n<view>x\r\n</view>";
        let tree = parse_clean(source);
        let root = tree.root();
        assert_eq!(tree.source_text(tree.children(root)[0]), Some("a\r\n"));
        assert_eq!(tree.text(), source);
        assert_ranges(&tree);
    }

    #[test]
    fn mustache_strings_may_contain_double_braces() {
        let source = "<view>{{ \"}}\" }}</view>";
        let tree = parse_clean(source);
        let view = tree.children(tree.root())[0];
        let mustache = find_child(&tree, view, NodeKind::Mustache).expect("mustache");
        assert_eq!(tree.source_text(mustache), Some("{{ \"}}\" }}"));
        let expr_node = tree.children(mustache)[0];
        assert!(matches!(
            tree.expr(expr_node).map(crate::expr::Expr::kind),
            Some(ExprKind::String { .. })
        ));
        assert_ranges(&tree);
    }

    #[test]
    fn attributes_can_mix_text_and_mustaches() {
        let source = r#"<view class="a {{b}} c"></view>"#;
        let tree = parse_clean(source);
        let view = tree.children(tree.root())[0];
        let start = find_child(&tree, view, NodeKind::StartTag).expect("start tag");
        let attribute = find_child(&tree, start, NodeKind::Attribute).expect("attribute");
        let value = find_child(&tree, attribute, NodeKind::AttributeValue).expect("value");
        assert_eq!(tree.source_text(value), Some(r#""a {{b}} c""#));
        assert_eq!(
            child_kinds(&tree, value),
            vec![NodeKind::Text, NodeKind::Mustache, NodeKind::Text]
        );
        let mustache = tree.children(value)[1];
        assert_eq!(tree.source_text(mustache), Some("{{b}}"));
        assert_ranges(&tree);
    }

    #[test]
    fn unquoted_attribute_values_are_parsed() {
        let source = "<view a=1 b={{x}} />";
        let tree = parse_clean(source);
        let view = tree.children(tree.root())[0];
        let start = find_child(&tree, view, NodeKind::StartTag).expect("start tag");
        let attributes = children_of_kind(&tree, start, NodeKind::Attribute);
        assert_eq!(attributes.len(), 2);
        assert_eq!(tree.source_text(attributes[0]), Some("a=1"));
        assert_eq!(tree.source_text(attributes[1]), Some("b={{x}}"));
        let value = find_child(&tree, attributes[1], NodeKind::AttributeValue).expect("value");
        assert_eq!(tree.source_text(value), Some("{{x}}"));
        assert_ranges(&tree);
    }

    #[test]
    fn empty_mustaches_are_reported() {
        let result = parse("{{}}");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::EmptyMustache)
        );
        let tree = result.tree;
        let mustache = tree.children(tree.root())[0];
        assert_eq!(tree.kind(mustache), Some(NodeKind::Mustache));
        let expr_node = tree.children(mustache)[0];
        assert!(
            tree.expr(expr_node)
                .is_some_and(crate::expr::Expr::is_missing)
        );
        assert_ranges(&tree);
    }

    #[test]
    fn unclosed_mustaches_are_reported_with_a_partial_expression() {
        let source = "{{ user.name";
        let result = parse(source);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnclosedMustache)
        );
        let tree = result.tree;
        let mustache = tree.children(tree.root())[0];
        assert_eq!(tree.source_text(mustache), Some(source));
        let expr_node = tree.children(mustache)[0];
        assert!(matches!(
            tree.expr(expr_node).map(crate::expr::Expr::kind),
            Some(ExprKind::Member { .. })
        ));
        assert_ranges(&tree);
    }

    #[test]
    fn unclosed_attribute_values_are_reported() {
        let result = parse("<view a=\"x>");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnclosedAttributeValue)
        );
        assert_ranges(&result.tree);
    }

    #[test]
    fn missing_attribute_values_are_reported() {
        let result = parse("<view a=>");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::MissingAttributeValue)
        );
        let tree = result.tree;
        let view = tree.children(tree.root())[0];
        assert_eq!(tree.name(view), Some("view"));
        assert_ranges(&tree);
    }

    #[test]
    fn stray_tag_tokens_are_reported_and_skipped() {
        let result = parse("<view / >");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnexpectedToken)
        );
        let tree = result.tree;
        let view = tree.children(tree.root())[0];
        assert_eq!(tree.name(view), Some("view"));
        assert_ranges(&tree);
    }

    #[test]
    fn deeply_nested_input_hits_the_depth_limit_without_panicking() {
        let source: String = "<a>".repeat(300);
        let result = parse(&source);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::NestingTooDeep),
            "expected a depth error, got {:?}",
            result.errors
        );
        let tree = result.tree;
        assert!(tree.nodes().count() < 2000);
        assert_ranges(&tree);
    }

    #[test]
    fn repeated_parses_are_stable() {
        let source = r#"<view a="{{x}}"><!--c--><text>{{ y ? 1 : 2 }}</text></view>"#;
        let first = parse(source);
        let second = parse(source);
        assert_eq!(first.errors, second.errors);
        assert_eq!(flatten(&first.tree), flatten(&second.tree));
    }

    #[test]
    fn parsing_is_lossless_for_every_node() {
        let source = "  <a  b = \"c\" >d<!--e-->{{ f }}<g/>  </a> ";
        let tree = parse_clean(source);
        assert_eq!(tree.text(), source);
        assert_ranges(&tree);
    }

    #[test]
    fn names_with_namespaces_are_kept_verbatim() {
        let tree = parse_clean("<my-comp wx:for-item=\"item\"/>");
        let element = tree.children(tree.root())[0];
        assert_eq!(tree.name(element), Some("my-comp"));
        let start = find_child(&tree, element, NodeKind::StartTag).expect("start tag");
        let attribute = tree.children(start)[0];
        assert_eq!(tree.name(attribute), Some("wx:for-item"));
    }

    #[test]
    fn wxs_bodies_are_raw_text_and_keep_following_siblings() {
        let source = r#"<wxs module="m">if (a<b) { x = 1 }</wxs><view>{{x}}</view>"#;
        let tree = parse_clean(source);
        let root = tree.root();
        let elements: Vec<_> = tree
            .children(root)
            .iter()
            .copied()
            .filter(|id| tree.kind(*id) == Some(NodeKind::Element))
            .collect();
        assert_eq!(elements.len(), 2);
        let wxs = elements[0];
        assert_eq!(tree.name(wxs), Some("wxs"));
        let text = children_of_kind(&tree, wxs, NodeKind::Text);
        assert_eq!(text.len(), 1);
        assert_eq!(tree.source_text(text[0]), Some("if (a<b) { x = 1 }"));
        assert_eq!(
            tree.name(find_child(&tree, wxs, NodeKind::EndTag).expect("wxs end tag")),
            Some("wxs")
        );
        assert!(
            tree.descendants(wxs).all(|id| tree.name(id) != Some("b")),
            "the wxs body must not produce markup elements"
        );
        let view = elements[1];
        assert_eq!(tree.name(view), Some("view"));
        assert!(
            tree.document()
                .mustaches()
                .any(|mustache| mustache.expr_source() == "x")
        );
        assert_ranges(&tree);
    }

    #[test]
    fn unclosed_wxs_bodies_consume_the_rest_of_the_input() {
        let source = "<wxs>if (a<b) { x = 1 }";
        let result = parse(source);
        assert!(!result.truncated);
        let unclosed = result
            .errors
            .iter()
            .filter(|error| error.kind() == ParseErrorKind::UnclosedTag)
            .count();
        assert_eq!(unclosed, 1, "errors: {:?}", result.errors);
        let tree = result.tree;
        let wxs = tree.children(tree.root())[0];
        let text = children_of_kind(&tree, wxs, NodeKind::Text);
        assert_eq!(
            text.first().and_then(|id| tree.source_text(*id)),
            Some("if (a<b) { x = 1 }")
        );
        assert_ranges(&tree);
    }

    #[test]
    fn unterminated_open_tag_reports_a_single_error() {
        let result = parse("<view");
        assert_eq!(result.errors.len(), 1, "errors: {:?}", result.errors);
        assert_eq!(result.errors[0].kind(), ParseErrorKind::UnclosedTag);
        assert!(!result.truncated);
        assert_ranges(&result.tree);
    }

    #[test]
    fn expression_error_flood_is_capped() {
        let mut source = String::from("{{");
        source.push_str(&"@".repeat(1024 * 1024));
        source.push_str("}}");
        let result = parse(&source);
        assert_eq!(result.errors.len(), MAX_ERRORS + 1);
        assert_eq!(
            result.errors.last().map(crate::ParseError::kind),
            Some(ParseErrorKind::TooManyErrors)
        );
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::InvalidCharacter)
        );
    }

    #[test]
    fn tag_error_flood_is_capped() {
        let mut source = String::from("<a ");
        source.push_str(&"@".repeat(1024 * 1024));
        source.push('>');
        let result = parse(&source);
        assert!(result.truncated);
        assert_eq!(result.errors.len(), MAX_ERRORS + 1);
        assert_eq!(
            result.errors.last().map(crate::ParseError::kind),
            Some(ParseErrorKind::TooManyErrors)
        );
    }

    #[test]
    fn lone_text_flood_hits_the_node_budget() {
        let source = "<".repeat(1024 * 1024);
        let result = parse(&source);
        assert!(result.truncated);
        assert!(result.tree.nodes().count() <= MAX_NODES);
        assert!(result.errors.len() <= MAX_ERRORS + 1);
        assert!(result.errors.iter().any(|error| matches!(
            error.kind(),
            ParseErrorKind::TooManyNodes | ParseErrorKind::TooManyTokens
        )));
        assert_ranges(&result.tree);
    }

    #[test]
    fn token_flood_is_truncated_at_the_markup_token_limit() {
        let source = "<a ".to_owned() + &"@".repeat(MAX_MARKUP_TOKENS + 1024) + ">";
        let result = parse(&source);
        assert!(result.truncated);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::TooManyTokens)
        );
    }

    #[test]
    fn oversized_input_is_rejected_before_building_a_tree() {
        let result = parse_inner("hello", 4);
        assert!(result.truncated);
        assert_eq!(result.tree.text(), "");
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].kind(), ParseErrorKind::InputTooLarge);
    }

    #[test]
    fn node_ids_from_different_trees_can_alias() {
        let first = parse_clean("<view/>");
        let second = parse_clean("<view></view>");
        let element = first.children(first.root())[0];
        assert_eq!(first.kind(element), Some(NodeKind::Element));
        assert_eq!(second.kind(element), Some(NodeKind::EndTag));
    }

    #[test]
    fn closed_flags_come_from_parsing_not_text_suffixes() {
        let tree = parse("{{ {a: {b: 1}}").tree;
        let mustache = tree.mustaches().next().expect("mustache");
        assert!(!tree.node(mustache).expect("mustache node").is_closed());

        let tree = parse("<view a=\"/>").tree;
        let view = tree.children(tree.root())[0];
        assert!(!tree.node(view).expect("view node").is_self_closing());
    }

    #[test]
    fn wxs_raw_text_does_not_swallow_mustaches_or_siblings() {
        // The `{{` in the wxs body would otherwise lex an unterminated mustache
        // that swallows the closing tag and the sibling element.
        let source = "<wxs>{{</wxs><view>{{x}}</view>";
        let result = parse(source);
        assert!(!result.truncated, "errors: {:?}", result.errors);
        let tree = result.tree;
        let elements: Vec<_> = tree
            .children(tree.root())
            .iter()
            .copied()
            .filter(|id| tree.kind(*id) == Some(NodeKind::Element))
            .collect();
        assert_eq!(elements.len(), 2, "errors: {:?}", result.errors);
        let wxs = elements[0];
        let text = children_of_kind(&tree, wxs, NodeKind::Text);
        assert_eq!(
            text.first().and_then(|id| tree.source_text(*id)),
            Some("{{")
        );
        let view = elements[1];
        assert_eq!(tree.name(view), Some("view"));
        assert_eq!(
            tree.document()
                .mustaches()
                .map(Mustache::expr_source)
                .collect::<Vec<_>>(),
            vec!["x"]
        );
        assert_ranges(&tree);
    }

    fn element_named(tree: &SyntaxTree, name: &str) -> NodeId {
        tree.descendants(tree.root())
            .find(|id| tree.kind(*id) == Some(NodeKind::Element) && tree.name(*id) == Some(name))
            .expect("element exists")
    }

    fn assert_contains(tree: &SyntaxTree, parent: NodeId, child: NodeId) {
        let parent_range = tree.text_range(parent).expect("parent range");
        let child_range = tree.text_range(child).expect("child range");
        assert!(
            parent_range.start() <= child_range.start() && child_range.end() <= parent_range.end(),
            "child {child_range:?} outside parent {parent_range:?}"
        );
    }

    #[test]
    fn unclosed_parent_ranges_contain_wxs_children() {
        let tree = parse("<div><wxs>x</wxs>").tree;
        let div = element_named(&tree, "div");
        let wxs = element_named(&tree, "wxs");
        assert_contains(&tree, div, wxs);
        assert_eq!(tree.source_text(wxs), Some("<wxs>x</wxs>"));
        assert_ranges(&tree);

        let tree = parse("<a><b><wxs>x</wxs></a>").tree;
        let b = element_named(&tree, "b");
        let wxs = element_named(&tree, "wxs");
        assert_contains(&tree, b, wxs);
        assert_ranges(&tree);
    }

    #[test]
    fn token_truncated_wxs_body_keeps_parent_ranges_valid() {
        // `<div>` uses three tokens and `<wxs>` uses three more. The raw body
        // token is cut by the budget, so the wxs element is unclosed; its end
        // still has to include the parsed bytes and fit inside the parent.
        let source = "<div><wxs>abc</wxs></div>";
        let result = parse_with_token_limit(source, 6);
        assert!(result.truncated);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::TooManyTokens)
        );
        let tree = result.tree;
        let div = element_named(&tree, "div");
        let wxs = element_named(&tree, "wxs");
        assert_contains(&tree, div, wxs);
        assert_ranges(&tree);
    }

    #[test]
    fn node_budget_keeps_committed_subtrees() {
        let result = parse(&"<a>".repeat(200_000));
        assert!(result.truncated);
        let nodes = result.tree.nodes().count();
        assert!(
            nodes > 50_000,
            "budget truncation collapsed the tree to {nodes} nodes"
        );
        assert!(nodes <= MAX_NODES);
        assert_ranges(&result.tree);
    }

    #[test]
    fn many_wxs_elements_stay_linear() {
        let source = "<wxs></wxs>".repeat(10_000);
        let start = std::time::Instant::now();
        let result = parse(&source);
        let elapsed = start.elapsed();
        assert!(!result.truncated);
        assert_eq!(result.tree.document().elements().count(), 10_000);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        // Generous bound: the removed suffix re-lexing was O(n^2) and took
        // seconds for a comparable input; lexer-level raw text is linear.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "parsing 10,000 wxs elements took {elapsed:?}"
        );
    }

    #[test]
    fn uppercase_wxs_is_an_ordinary_element() {
        let tree = parse_clean("<WXS>text</WXS>");
        let element = element_named(&tree, "WXS");
        assert_eq!(
            children_of_kind(&tree, element, NodeKind::Text)
                .first()
                .and_then(|id| tree.source_text(*id)),
            Some("text")
        );
        assert!(find_child(&tree, element, NodeKind::EndTag).is_some());
    }

    #[test]
    fn wxs_closing_tag_accepts_self_closing_form() {
        let tree = parse_clean("<wxs>x</wxs />");
        let wxs = element_named(&tree, "wxs");
        let end = find_child(&tree, wxs, NodeKind::EndTag).expect("end tag");
        assert_eq!(tree.name(end), Some("wxs"));
        assert_eq!(tree.source_text(end), Some("</wxs />"));
    }

    #[test]
    fn eight_mib_of_lone_angles_stays_bounded() {
        // Security regression: the public parse path must never tokenize the
        // whole input without a cap.
        let source = "<".repeat(8 * 1024 * 1024);
        let result = parse(&source);
        assert!(result.truncated);
        assert!(result.tree.nodes().count() <= MAX_NODES);
        assert!(result.errors.len() <= MAX_ERRORS + 1);
        assert!(result.errors.iter().any(|error| matches!(
            error.kind(),
            ParseErrorKind::TooManyNodes | ParseErrorKind::TooManyTokens
        )));
    }
}
