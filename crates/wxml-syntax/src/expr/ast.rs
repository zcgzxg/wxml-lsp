//! Expression AST types shared by the parser and its consumers.
//!
//! Every node keeps an absolute byte [`TextRange`] into the original WXML
//! document, so a template layer can rewrite identifiers or map diagnostics
//! back to the exact source token. The AST is immutable.

use wxml_text::TextRange;

/// A parsed template expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    kind: ExprKind,
    range: TextRange,
}

impl Expr {
    pub(crate) const fn new(kind: ExprKind, range: TextRange) -> Self {
        Self { kind, range }
    }

    /// Returns the expression shape.
    #[must_use]
    pub const fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Returns the source range covered by this expression, including
    /// parentheses and operators but not surrounding whitespace.
    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }

    /// Returns `true` when the parser could not build a real node here.
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        matches!(self.kind, ExprKind::Missing)
    }

    /// Iterates over the direct child expressions in source order.
    pub fn children(&self) -> impl DoubleEndedIterator<Item = &Self> + '_ {
        let mut children: Vec<&Self> = Vec::new();
        match &self.kind {
            ExprKind::Member {
                object, property, ..
            } => {
                children.push(object);
                children.push(property);
            }
            ExprKind::Index { object, index } => {
                children.push(object);
                children.push(index);
            }
            ExprKind::Call { callee, arguments } => {
                children.push(callee);
                children.extend(arguments.iter());
            }
            ExprKind::Unary { operand, .. } => children.push(operand),
            ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
                children.push(left);
                children.push(right);
            }
            ExprKind::Conditional {
                condition,
                then_expr,
                else_expr,
            } => {
                children.push(condition);
                children.push(then_expr);
                children.push(else_expr);
            }
            ExprKind::Parenthesized { expr } => children.push(expr),
            ExprKind::Array(items) => children.extend(items.iter()),
            ExprKind::Object(entries) => {
                children.extend(entries.iter().map(|entry| &entry.value));
            }
            ExprKind::Missing
            | ExprKind::Identifier { .. }
            | ExprKind::String { .. }
            | ExprKind::Number
            | ExprKind::Boolean(_)
            | ExprKind::Null => {}
        }
        children.into_iter()
    }

    /// Walks this expression and all descendants in pre-order.
    #[must_use]
    pub fn walk(&self) -> Walk<'_> {
        Walk { stack: vec![self] }
    }
}

/// Pre-order iterator produced by [`Expr::walk`].
#[derive(Clone, Debug)]
pub struct Walk<'a> {
    stack: Vec<&'a Expr>,
}

impl<'a> Iterator for Walk<'a> {
    type Item = &'a Expr;

    fn next(&mut self) -> Option<Self::Item> {
        let expr = self.stack.pop()?;
        self.stack.extend(expr.children().rev());
        Some(expr)
    }
}

/// The shape of a parsed expression.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExprKind {
    /// A missing expression produced by error recovery.
    Missing,
    /// A bare name, such as `user` or `wx:if` condition identifiers.
    Identifier {
        /// Range of the name token, without surrounding whitespace.
        name: TextRange,
    },
    /// A quoted string literal.
    String {
        /// The opening quote character, `'` or `"`.
        quote: char,
        /// Range of the raw content between the quotes. Escapes are kept
        /// verbatim; consumers that need the decoded value must unescape.
        content: TextRange,
    },
    /// A numeric literal; the range covers the literal exactly.
    Number,
    /// A `true` or `false` literal.
    Boolean(bool),
    /// A `null` literal.
    Null,
    /// Member access such as `user.name` or `user?.name`.
    Member {
        /// The value being accessed.
        object: Box<Expr>,
        /// The property expression; normally an [`ExprKind::Identifier`].
        property: Box<Expr>,
        /// `true` for optional chaining (`?.`); currently always `false`.
        optional: bool,
    },
    /// Index access such as `items[0]`.
    Index {
        /// The value being indexed.
        object: Box<Expr>,
        /// The index expression.
        index: Box<Expr>,
    },
    /// A function call such as `format(user.name)`.
    Call {
        /// The callee expression.
        callee: Box<Expr>,
        /// Argument expressions in source order.
        arguments: Vec<Expr>,
    },
    /// A prefix unary operation.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// The operand.
        operand: Box<Expr>,
    },
    /// An arithmetic, relational or equality operation.
    Binary {
        /// The operator.
        op: BinaryOp,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
    },
    /// A short-circuiting logical operation.
    Logical {
        /// The operator.
        op: LogicalOp,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
    },
    /// The ternary conditional `condition ? then : else`.
    Conditional {
        /// The tested expression.
        condition: Box<Expr>,
        /// The value when the condition is truthy.
        then_expr: Box<Expr>,
        /// The value when the condition is falsy.
        else_expr: Box<Expr>,
    },
    /// An expression wrapped in parentheses; the range includes the
    /// parentheses so edits can preserve them.
    Parenthesized {
        /// The inner expression.
        expr: Box<Expr>,
    },
    /// An array literal, including any trailing comma.
    Array(Vec<Expr>),
    /// An object literal, including any trailing comma.
    Object(Vec<ObjectEntry>),
}

/// Prefix unary operators.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UnaryOp {
    /// Logical negation `!`.
    Not,
    /// Arithmetic negation `-`.
    Negate,
    /// Unary plus `+`.
    Plus,
}

/// Non-logical binary operators.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `%`
    Remainder,
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterOrEqual,
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `===`
    StrictEqual,
    /// `!==`
    StrictNotEqual,
}

/// Short-circuiting binary operators.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LogicalOp {
    /// `&&`
    And,
    /// `||`
    Or,
}

/// One `key: value` pair or shorthand property of an object literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectEntry {
    /// The key as written.
    pub key: ObjectKey,
    /// The value; for a shorthand entry this is an identifier with the same
    /// range as the key.
    pub value: Expr,
    /// `true` when the entry was written as a bare key (`{ name }`).
    pub shorthand: bool,
    /// Range from the start of the key to the end of the value.
    pub range: TextRange,
}

/// A key in an object literal.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectKey {
    /// A bare identifier key.
    Identifier {
        /// Range of the key name.
        name: TextRange,
    },
    /// A quoted string key.
    String {
        /// The opening quote character.
        quote: char,
        /// Range of the raw content between the quotes.
        content: TextRange,
    },
    /// A numeric key.
    Number {
        /// Range of the literal.
        literal: TextRange,
    },
}

/// Result of parsing a single mustache expression.
///
/// Errors do not prevent a usable tree: recovered expressions contain
/// [`ExprKind::Missing`] nodes and the surrounding nodes keep their ranges.
#[derive(Clone, Debug)]
pub struct ExprParseResult {
    /// The parsed (possibly partial) expression.
    pub expr: Expr,
    /// Problems found while lexing and parsing, in discovery order.
    ///
    /// At most [`crate::MAX_ERRORS`] regular entries are kept; when the cap is
    /// reached the last entry is a
    /// [`ParseErrorKind::TooManyErrors`](crate::ParseErrorKind::TooManyErrors)
    /// sentinel.
    pub errors: Vec<crate::error::ParseError>,
}

impl ExprParseResult {
    /// Returns `true` when no problems were found.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}
