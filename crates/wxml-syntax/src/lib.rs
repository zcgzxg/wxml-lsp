//! WXML syntax: tolerant lexing, parsing and template expressions.
//!
//! This crate turns WXML source into an edit-friendly syntax tree with byte
//! ranges, including `{{ ... }}` mustache expressions. It knows nothing about
//! editors, project files or TypeScript types; the only dependency is
//! [`wxml_text`] for byte ranges.
//!
//! The pipeline has four layers:
//!
//! 1. [`lexer`] splits WXML markup into tokens with exact source ranges.
//! 2. [`parser`] builds a lossless concrete syntax tree ([`SyntaxTree`]) and
//!    recovers from malformed input instead of failing the whole document.
//! 3. [`expr`] parses the inside of each `{{ ... }}` with a Pratt parser.
//! 4. [`source_map`] records segmented template-to-fragment mappings for the
//!    template layer that generates check fragments.
//!
//! [`ast`] adds typed views over the concrete tree for consumers that want
//! elements, attributes, values and mustaches rather than raw nodes.
//!
//! All ranges are UTF-8 byte ranges over the original source text. Parsing is
//! bounded: element nesting, node count, markup token count and expression
//! complexity have explicit limits so that hostile or truncated input cannot
//! exhaust the stack or memory. Truncation is visible through
//! [`ParseResult::truncated`] and a resource diagnostic; it is never reported
//! as a clean parse. Parsing the same text twice produces the same node kinds,
//! ranges and errors.
//!
//! ```rust
//! use wxml_syntax::parse;
//!
//! let result = parse(r#"<view class="page">{{ user.name }}</view>"#);
//! assert!(result.errors.is_empty());
//! let tree = result.tree;
//! let document = tree.document();
//! if let Some(view) = document.elements().next() {
//!     assert_eq!(view.name(), Some("view"));
//!     assert_eq!(
//!         view.attribute("class")
//!             .and_then(|attribute| attribute.value())
//!             .map(|value| value.raw()),
//!         Some(r#""page""#)
//!     );
//!     if let Some(mustache) = view.mustaches().next() {
//!         assert_eq!(mustache.expr_source(), "user.name");
//!     }
//! }
//! ```

pub mod ast;
pub mod error;
pub mod expr;
pub mod lexer;
pub mod parser;
pub mod source_map;

pub use ast::{
    Attribute, AttributeValue, Comment, Document, Element, Mustache, Node as AstNode, Text,
};
pub use error::{MAX_ERRORS, ParseError, ParseErrorKind};
pub use expr::{
    BinaryOp, Expr, ExprKind, ExprParseResult, LogicalOp, MAX_EXPRESSION_DEPTH,
    MAX_EXPRESSION_TOKENS, ObjectEntry, ObjectKey, UnaryOp, parse_expression,
};
pub use lexer::{Token, TokenKind};
pub use parser::{
    MAX_ELEMENT_DEPTH, MAX_MARKUP_TOKENS, MAX_NODES, Node, NodeId, NodeKind, ParseResult,
    SyntaxTree, parse,
};
pub use source_map::{FragmentBuilder, SegmentKind, SourceMap, SourceMapError, SourceMapSegment};
