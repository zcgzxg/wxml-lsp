//! Pratt parser and AST for `{{ ... }}` template expressions.
//!
//! The expression grammar is a small, side-effect-free subset of JavaScript
//! that WXML uses for interpolation and bindings. It is parsed independently
//! from the surrounding markup so that a broken interpolation does not affect
//! tag structure.
//!
//! Precedence, from tightest to loosest:
//!
//! 1. member access, indexing and calls
//! 2. unary `!`, `-`, `+`
//! 3. `*`, `/`, `%`
//! 4. `+`, `-`
//! 5. `<`, `<=`, `>`, `>=`
//! 6. `==`, `!=`, `===`, `!==`
//! 7. `&&`
//! 8. `||`
//! 9. `condition ? then : else`
//!
//! Parentheses override the order. Array and object literals are parsed so
//! that templates can use them in bindings; calls are parsed but remain the
//! caller's responsibility to reject when a binding does not allow them.

mod ast;
pub(crate) mod lexer;
mod parser;

pub use ast::{
    BinaryOp, Expr, ExprKind, ExprParseResult, LogicalOp, ObjectEntry, ObjectKey, UnaryOp, Walk,
};
pub use parser::parse_expression;

/// Maximum nesting depth accepted by the expression parser.
///
/// The limit is shared by parentheses, arrays, objects, calls, indexing and
/// unary chains. Inputs beyond it produce a
/// [`ParseErrorKind::ExpressionTooDeep`](crate::ParseErrorKind::ExpressionTooDeep)
/// error instead of recursing further.
pub const MAX_EXPRESSION_DEPTH: usize = 256;

/// Maximum number of tokens accepted in a single mustache expression.
///
/// Longer expressions produce a
/// [`ParseErrorKind::ExpressionTooManyTokens`](crate::ParseErrorKind::ExpressionTooManyTokens)
/// error. The remaining text is ignored.
pub const MAX_EXPRESSION_TOKENS: usize = 4096;
