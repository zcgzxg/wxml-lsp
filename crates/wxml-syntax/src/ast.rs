//! Typed AST views over the lossless concrete syntax tree.
//!
//! The concrete tree keeps every token; these wrappers expose the semantic
//! nodes a template layer usually needs: documents, elements, attributes,
//! values, text, comments and mustaches. Wrappers are cheap `Copy` views and
//! their fields stay private, so a wrapper always refers to a node of the
//! expected kind.

use wxml_text::TextRange;

use crate::error::span;
use crate::expr::Expr;
use crate::parser::{Node as SyntaxNode, NodeId, NodeKind, SyntaxTree};

impl SyntaxTree {
    /// Returns the document wrapper for this tree.
    #[must_use]
    pub const fn document(&self) -> Document<'_> {
        Document {
            tree: self,
            id: self.root(),
        }
    }

    /// Returns a typed AST node for `id`, or `None` when the id is unknown or
    /// the node kind has no AST wrapper (start tags, end tags and mustache
    /// expression nodes are reached through their parent wrapper instead).
    #[must_use]
    pub fn ast(&self, id: NodeId) -> Option<Node<'_>> {
        Node::new(self, id)
    }
}

/// The document root.
#[derive(Clone, Copy, Debug)]
pub struct Document<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> Document<'a> {
    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the document's source range.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree.text_range(self.id).unwrap_or_else(|| span(0, 0))
    }

    /// Iterates over every element in the document, including nested ones, in
    /// pre-order.
    pub fn elements(self) -> impl Iterator<Item = Element<'a>> {
        let tree = self.tree;
        tree.descendants(self.id)
            .filter_map(move |id| Element::new(tree, id))
    }

    /// Iterates over direct AST children of the document.
    pub fn children(self) -> impl Iterator<Item = Node<'a>> {
        node_children(self.tree, self.id)
    }

    /// Iterates over every mustache in the document, including attribute
    /// values.
    pub fn mustaches(self) -> impl Iterator<Item = Mustache<'a>> {
        let tree = self.tree;
        tree.mustaches()
            .filter_map(move |id| Mustache::new(tree, id))
    }
}

/// An element, including its start tag, content and end tag.
#[derive(Clone, Copy, Debug)]
pub struct Element<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> Element<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        (tree.kind(id) == Some(NodeKind::Element)).then_some(Self { tree, id })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the element's source range.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree.text_range(self.id).unwrap_or_else(|| span(0, 0))
    }

    /// Returns the tag name, for example `view` or `my-component`.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.tree.name(self.id)
    }

    /// Returns the range of the tag name.
    #[must_use]
    pub fn name_range(self) -> Option<TextRange> {
        self.tree.name_range(self.id)
    }

    /// Iterates over the element's attributes in source order.
    pub fn attributes(self) -> impl Iterator<Item = Attribute<'a>> {
        let tree = self.tree;
        let start_tag = self
            .tree
            .children(self.id)
            .iter()
            .copied()
            .find(|id| self.tree.kind(*id) == Some(NodeKind::StartTag));
        start_tag.into_iter().flat_map(move |start| {
            tree.children(start)
                .iter()
                .copied()
                .filter_map(move |id| Attribute::new(tree, id))
        })
    }

    /// Returns the first attribute with the given name.
    #[must_use]
    pub fn attribute(self, name: &str) -> Option<Attribute<'a>> {
        self.attributes()
            .find(|attribute| attribute.name() == Some(name))
    }

    /// Iterates over content children, skipping the start and end tags.
    pub fn children(self) -> impl Iterator<Item = Node<'a>> {
        node_children(self.tree, self.id)
    }

    /// Iterates over mustaches that appear directly in element content.
    /// Mustaches inside attribute values belong to their
    /// [`AttributeValue`], not to this iterator.
    pub fn mustaches(self) -> impl Iterator<Item = Mustache<'a>> {
        let tree = self.tree;
        self.tree
            .children(self.id)
            .iter()
            .copied()
            .filter_map(move |id| Mustache::new(tree, id))
    }

    /// Returns `true` when the start tag ended with `/>`.
    ///
    /// The flag is recorded while parsing, so an unterminated quoted value that
    /// happens to end in `/>` does not count as self-closing.
    #[must_use]
    pub fn is_self_closing(self) -> bool {
        self.tree
            .node(self.id)
            .is_some_and(SyntaxNode::is_self_closing)
    }

    /// Returns `true` when the element has an explicit end tag.
    #[must_use]
    pub fn is_closed(self) -> bool {
        self.tree.node(self.id).is_some_and(SyntaxNode::is_closed)
    }
}

/// An attribute of an element.
#[derive(Clone, Copy, Debug)]
pub struct Attribute<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> Attribute<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        (tree.kind(id) == Some(NodeKind::Attribute)).then_some(Self { tree, id })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the attribute's source range, from its name to the end of its
    /// value.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree.text_range(self.id).unwrap_or_else(|| span(0, 0))
    }

    /// Returns the attribute name, for example `class` or `wx:if`.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.tree.name(self.id)
    }

    /// Returns the range of the attribute name.
    #[must_use]
    pub fn name_range(self) -> Option<TextRange> {
        self.tree.name_range(self.id)
    }

    /// Returns the attribute value, when the attribute has one.
    #[must_use]
    pub fn value(self) -> Option<AttributeValue<'a>> {
        let tree = self.tree;
        self.tree
            .children(self.id)
            .iter()
            .copied()
            .find_map(|id| AttributeValue::new(tree, id))
    }
}

/// The value of an attribute, including surrounding quotes when present.
#[derive(Clone, Copy, Debug)]
pub struct AttributeValue<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> AttributeValue<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        (tree.kind(id) == Some(NodeKind::AttributeValue)).then_some(Self { tree, id })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the value's source range.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree.text_range(self.id).unwrap_or_else(|| span(0, 0))
    }

    /// Returns the raw value text, including quotes when the value is quoted.
    #[must_use]
    pub fn raw(self) -> &'a str {
        self.tree.source_text(self.id).unwrap_or_default()
    }

    /// Returns `true` when the value is wrapped in single or double quotes.
    #[must_use]
    pub fn is_quoted(self) -> bool {
        let raw = self.raw();
        raw.starts_with('"') || raw.starts_with('\'')
    }

    /// Returns the literal text when the value has no mustache and exactly one
    /// text segment, with quotes removed.
    ///
    /// An empty value such as `a=""` returns `Some("")`. Values mixing text and
    /// mustaches return `None`; use [`AttributeValue::mustaches`] and
    /// [`AttributeValue::children`] for those.
    #[must_use]
    pub fn literal_text(self) -> Option<&'a str> {
        let mut children = self.children();
        let Some(first) = children.next() else {
            return Some("");
        };
        if children.next().is_some() {
            return None;
        }
        match first {
            Node::Text(text) => Some(text.text()),
            _ => None,
        }
    }

    /// Iterates over mustaches embedded in the value.
    pub fn mustaches(self) -> impl Iterator<Item = Mustache<'a>> {
        let tree = self.tree;
        self.tree
            .children(self.id)
            .iter()
            .copied()
            .filter_map(move |id| Mustache::new(tree, id))
    }

    /// Iterates over the value's AST children (text and mustaches).
    pub fn children(self) -> impl Iterator<Item = Node<'a>> {
        node_children(self.tree, self.id)
    }
}

/// Raw text outside tags or inside a quoted attribute value.
#[derive(Clone, Copy, Debug)]
pub struct Text<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> Text<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        (tree.kind(id) == Some(NodeKind::Text)).then_some(Self { tree, id })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the text exactly as written.
    #[must_use]
    pub fn text(self) -> &'a str {
        self.tree.source_text(self.id).unwrap_or_default()
    }

    /// Returns the text's source range.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree.text_range(self.id).unwrap_or_else(|| span(0, 0))
    }
}

/// A `<!-- ... -->` comment.
#[derive(Clone, Copy, Debug)]
pub struct Comment<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> Comment<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        (tree.kind(id) == Some(NodeKind::Comment)).then_some(Self { tree, id })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the comment including its delimiters.
    #[must_use]
    pub fn text(self) -> &'a str {
        self.tree.source_text(self.id).unwrap_or_default()
    }

    /// Returns the comment text without `<!--` and `-->`; unterminated
    /// comments return the full source text.
    #[must_use]
    pub fn content(self) -> &'a str {
        let text = self.text();
        text.strip_prefix("<!--")
            .and_then(|rest| rest.strip_suffix("-->"))
            .unwrap_or(text)
    }
}

/// A `{{ ... }}` interpolation.
#[derive(Clone, Copy, Debug)]
pub struct Mustache<'a> {
    tree: &'a SyntaxTree,
    id: NodeId,
}

impl<'a> Mustache<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        (tree.kind(id) == Some(NodeKind::Mustache)).then_some(Self { tree, id })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Returns the whole mustache range, including `{{` and `}}`.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree.text_range(self.id).unwrap_or_else(|| span(0, 0))
    }

    /// Returns `true` when the closing `}}` was present.
    ///
    /// The flag is recorded while parsing, so a nested-brace expression such as
    /// `{{ {a: {b: 1}}` is correctly reported as not closed.
    #[must_use]
    pub fn is_closed(self) -> bool {
        self.tree.node(self.id).is_some_and(SyntaxNode::is_closed)
    }

    /// Returns the range of the trimmed expression inside the braces.
    #[must_use]
    pub fn inner_range(self) -> Option<TextRange> {
        self.expr_node().and_then(|id| self.tree.text_range(id))
    }

    /// Returns the trimmed expression source, without `{{` and `}}`.
    #[must_use]
    pub fn expr_source(self) -> &'a str {
        self.expr_node()
            .and_then(|id| self.tree.source_text(id))
            .unwrap_or_default()
    }

    /// Returns the parsed expression, if one was produced.
    #[must_use]
    pub fn expr(self) -> Option<&'a Expr> {
        self.expr_node().and_then(|id| self.tree.expr(id))
    }

    fn expr_node(self) -> Option<NodeId> {
        self.tree
            .children(self.id)
            .iter()
            .copied()
            .find(|id| self.tree.kind(*id) == Some(NodeKind::MustacheExpr))
    }
}

/// A typed AST node.
#[derive(Clone, Copy, Debug)]
pub enum Node<'a> {
    /// The document root.
    Document(Document<'a>),
    /// An element.
    Element(Element<'a>),
    /// An element attribute.
    Attribute(Attribute<'a>),
    /// An attribute value.
    AttributeValue(AttributeValue<'a>),
    /// Raw text.
    Text(Text<'a>),
    /// A comment.
    Comment(Comment<'a>),
    /// A mustache interpolation.
    Mustache(Mustache<'a>),
}

impl<'a> Node<'a> {
    fn new(tree: &'a SyntaxTree, id: NodeId) -> Option<Self> {
        Some(match tree.kind(id)? {
            NodeKind::Document => Self::Document(Document { tree, id }),
            NodeKind::Element => Self::Element(Element { tree, id }),
            NodeKind::Attribute => Self::Attribute(Attribute { tree, id }),
            NodeKind::AttributeValue => Self::AttributeValue(AttributeValue { tree, id }),
            NodeKind::Text => Self::Text(Text::new(tree, id)?),
            NodeKind::Comment => Self::Comment(Comment::new(tree, id)?),
            NodeKind::Mustache => Self::Mustache(Mustache { tree, id }),
            NodeKind::StartTag | NodeKind::EndTag | NodeKind::MustacheExpr => return None,
        })
    }

    /// Returns the underlying node id.
    #[must_use]
    pub const fn id(self) -> NodeId {
        match self {
            Self::Document(node) => node.id,
            Self::Element(node) => node.id,
            Self::Attribute(node) => node.id,
            Self::AttributeValue(node) => node.id,
            Self::Text(node) => node.id,
            Self::Comment(node) => node.id,
            Self::Mustache(node) => node.id,
        }
    }

    /// Returns the node kind.
    #[must_use]
    pub const fn kind(self) -> NodeKind {
        match self {
            Self::Document(_) => NodeKind::Document,
            Self::Element(_) => NodeKind::Element,
            Self::Attribute(_) => NodeKind::Attribute,
            Self::AttributeValue(_) => NodeKind::AttributeValue,
            Self::Text(_) => NodeKind::Text,
            Self::Comment(_) => NodeKind::Comment,
            Self::Mustache(_) => NodeKind::Mustache,
        }
    }

    /// Returns the node's source range.
    #[must_use]
    pub fn text_range(self) -> TextRange {
        self.tree()
            .text_range(self.id())
            .unwrap_or_else(|| span(0, 0))
    }

    /// Returns the source text covered by the node.
    #[must_use]
    pub fn source_text(self) -> &'a str {
        self.tree().source_text(self.id()).unwrap_or_default()
    }

    /// Iterates over direct AST children, skipping structural tag nodes.
    pub fn children(self) -> impl Iterator<Item = Node<'a>> {
        node_children(self.tree(), self.id())
    }

    const fn tree(self) -> &'a SyntaxTree {
        match self {
            Self::Document(node) => node.tree,
            Self::Element(node) => node.tree,
            Self::Attribute(node) => node.tree,
            Self::AttributeValue(node) => node.tree,
            Self::Text(node) => node.tree,
            Self::Comment(node) => node.tree,
            Self::Mustache(node) => node.tree,
        }
    }
}

fn node_children(tree: &SyntaxTree, id: NodeId) -> impl Iterator<Item = Node<'_>> + '_ {
    tree.children(id)
        .iter()
        .copied()
        .filter_map(move |child| Node::new(tree, child))
}

#[cfg(test)]
mod tests {
    use crate::ast::{Attribute, AttributeValue, Mustache, Node};
    use crate::parser::{NodeKind, parse};

    fn parse_clean(source: &str) -> crate::parser::SyntaxTree {
        let result = parse(source);
        assert!(
            result.errors.is_empty(),
            "expected no errors for {source:?}, got {:?}",
            result.errors
        );
        result.tree
    }

    #[test]
    fn document_lists_all_elements() {
        let tree = parse_clean("<view><text>hi</text></view>");
        let document = tree.document();
        let names: Vec<&str> = document
            .elements()
            .map(|element| element.name().unwrap_or_default())
            .collect();
        assert_eq!(names, vec!["view", "text"]);
        assert_eq!(
            document.text_range(),
            tree.text_range(tree.root()).expect("range")
        );
    }

    #[test]
    fn elements_expose_attributes_and_values() {
        let tree = parse_clean(r#"<view id="page" class='x y' hidden empty="">z</view>"#);
        let view = tree.document().elements().next().expect("view element");
        assert!(view.attributes().count() >= 4);
        let id = view.attribute("id").expect("id attribute");
        assert_eq!(id.name(), Some("id"));
        let value = id.value().expect("id value");
        assert!(value.is_quoted());
        assert_eq!(value.literal_text(), Some("page"));
        assert_eq!(value.raw(), "\"page\"");
        let hidden = view.attribute("hidden").expect("hidden attribute");
        assert!(hidden.value().is_none());

        let class = view.attribute("class").expect("class attribute");
        assert_eq!(
            class.value().and_then(AttributeValue::literal_text),
            Some("x y")
        );
        assert_eq!(
            view.text_range(),
            tree.text_range(tree.children(tree.root())[0])
                .expect("range")
        );
    }

    #[test]
    fn element_children_skip_start_and_end_tags() {
        let tree = parse_clean("<view>a<text/>b</view>");
        let view = tree.document().elements().next().expect("view");
        let children: Vec<NodeKind> = view.children().map(Node::kind).collect();
        assert_eq!(
            children,
            vec![NodeKind::Text, NodeKind::Element, NodeKind::Text]
        );
        assert!(!view.is_self_closing());
        assert!(view.is_closed());
        let inner = tree.document().elements().nth(1).expect("text element");
        assert!(inner.is_self_closing());
        assert!(!inner.is_closed());
    }

    #[test]
    fn element_mustaches_only_include_content() {
        let tree = parse_clean(r#"<view class="{{a}}">{{b}}<text>{{c}}</text></view>"#);
        let view = tree.document().elements().next().expect("view");
        let exprs: Vec<&str> = view.mustaches().map(Mustache::expr_source).collect();
        assert_eq!(exprs, vec!["b"]);
        assert_eq!(tree.document().mustaches().count(), 3);
    }

    #[test]
    fn attribute_values_mix_literals_and_mustaches() {
        let tree = parse_clean(r#"<view class="a {{b}} c"></view>"#);
        let view = tree.document().elements().next().expect("view");
        let value = view
            .attribute("class")
            .and_then(Attribute::value)
            .expect("class value");
        assert!(value.is_quoted());
        assert_eq!(value.literal_text(), None);
        let mustaches: Vec<&str> = value.mustaches().map(Mustache::expr_source).collect();
        assert_eq!(mustaches, vec!["b"]);
    }

    #[test]
    fn text_and_comment_wrappers_return_source_slices() {
        let tree = parse_clean("hello<!-- note -->");
        let document = tree.document();
        let children: Vec<Node<'_>> = document.children().collect();
        assert_eq!(children.len(), 2);
        let Node::Text(text) = children[0] else {
            panic!("expected text, got {:?}", children[0]);
        };
        assert_eq!(text.text(), "hello");
        let Node::Comment(comment) = children[1] else {
            panic!("expected comment, got {:?}", children[1]);
        };
        assert_eq!(comment.text(), "<!-- note -->");
        assert_eq!(comment.content(), " note ");
    }

    #[test]
    fn mustache_wrapper_exposes_expression_and_inner_range() {
        let tree = parse_clean("{{ user.name }}");
        let document = tree.document();
        let mustache = document.mustaches().next().expect("mustache");
        assert!(mustache.is_closed());
        assert_eq!(mustache.expr_source(), "user.name");
        let inner = mustache.inner_range().expect("inner range");
        assert_eq!(
            &tree.text()[inner.start() as usize..inner.end() as usize],
            "user.name"
        );
        assert!(mustache.expr().is_some());
    }

    #[test]
    fn ast_dispatch_returns_none_for_ids_from_other_trees() {
        let empty = parse_clean("");
        let other = parse_clean("<view/>");
        let element_id = other.children(other.root())[0];
        assert!(empty.ast(element_id).is_none());
        assert_eq!(
            other.ast(other.root()).map(Node::kind),
            Some(NodeKind::Document)
        );
    }

    #[test]
    fn ast_node_children_are_typed() {
        let tree = parse_clean(r#"<view a="1">{{b}}</view>"#);
        let view = tree.document().elements().next().expect("view");
        let kinds: Vec<NodeKind> = view.children().map(Node::kind).collect();
        assert_eq!(kinds, vec![NodeKind::Mustache]);
        let Node::Mustache(mustache) = view.children().next().expect("mustache") else {
            panic!("expected a mustache");
        };
        assert_eq!(mustache.expr_source(), "b");
    }

    #[test]
    fn empty_quoted_attribute_value_is_an_empty_literal() {
        let tree = parse_clean("<view a=\"\"/>");
        let view = tree.document().elements().next().expect("view");
        let value = view
            .attribute("a")
            .and_then(Attribute::value)
            .expect("attribute value");
        assert!(value.is_quoted());
        assert_eq!(value.raw(), "\"\"");
        assert_eq!(value.literal_text(), Some(""));
    }

    #[test]
    fn mustache_closed_flag_is_exact() {
        let result = parse("{{ {a: {b: 1}}");
        assert!(
            !result.errors.is_empty(),
            "expected a parse error, got none"
        );
        let tree = result.tree;
        let mustache = tree.document().mustaches().next().expect("mustache");
        assert!(!mustache.is_closed());
    }

    #[test]
    fn element_self_closing_flag_is_exact() {
        let result = parse("<view a=\"/>");
        assert!(
            !result.errors.is_empty(),
            "expected a parse error, got none"
        );
        let tree = result.tree;
        let view = tree.document().elements().next().expect("view");
        assert!(!view.is_self_closing());
        assert!(!view.is_closed());
    }
}
