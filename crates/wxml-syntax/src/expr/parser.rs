//! Pratt parser for `{{ ... }}` expressions.
//!
//! The parser is a precedence-climbing (Pratt) parser over the token stream
//! produced by [`super::lexer`]. It always returns an expression: malformed
//! input becomes [`ExprKind::Missing`] nodes with attached diagnostics, and
//! nesting is capped so recursive descent cannot overflow the stack.

use super::MAX_EXPRESSION_DEPTH;
use super::ast::{
    BinaryOp, Expr, ExprKind, ExprParseResult, LogicalOp, ObjectEntry, ObjectKey, UnaryOp,
};
use super::lexer::{Keyword, LexResult, Punct, Token, TokenKind, lex};
use crate::error::{ErrorSink, ParseErrorKind, span};
use wxml_text::TextRange;

/// Parses a single template expression.
///
/// `text` is the expression source without the surrounding `{{` and `}}`.
/// `base` is the byte offset of `text[0]` in the original document, so all
/// ranges in the result are absolute source ranges. Input beyond
/// [`super::MAX_EXPRESSION_TOKENS`] is truncated with an error.
#[must_use]
pub fn parse_expression(text: &str, base: u32) -> ExprParseResult {
    let LexResult { tokens, errors } = lex(text, base);
    let eof = tokens.last().copied().unwrap_or_else(|| Token {
        kind: TokenKind::Eof,
        range: span(base, base),
    });
    let mut parser = Parser {
        tokens,
        index: 0,
        eof,
        depth: 0,
        too_deep: false,
        last_end: base,
        errors,
    };
    let expr = parser.parse_root();
    ExprParseResult {
        expr,
        errors: parser.errors.into_vec(),
    }
}

/// A binary operator together with its precedence.
#[derive(Clone, Copy)]
enum Operator {
    Binary(BinaryOp),
    Logical(LogicalOp),
}

const fn binary_operator(kind: TokenKind) -> Option<(Operator, u8)> {
    let TokenKind::Punct(punct) = kind else {
        return None;
    };
    Some(match punct {
        Punct::Star => (Operator::Binary(BinaryOp::Multiply), 10),
        Punct::Slash => (Operator::Binary(BinaryOp::Divide), 10),
        Punct::Percent => (Operator::Binary(BinaryOp::Remainder), 10),
        Punct::Plus => (Operator::Binary(BinaryOp::Add), 9),
        Punct::Minus => (Operator::Binary(BinaryOp::Subtract), 9),
        Punct::Less => (Operator::Binary(BinaryOp::Less), 8),
        Punct::LessOrEqual => (Operator::Binary(BinaryOp::LessOrEqual), 8),
        Punct::Greater => (Operator::Binary(BinaryOp::Greater), 8),
        Punct::GreaterOrEqual => (Operator::Binary(BinaryOp::GreaterOrEqual), 8),
        Punct::Equal => (Operator::Binary(BinaryOp::Equal), 7),
        Punct::NotEqual => (Operator::Binary(BinaryOp::NotEqual), 7),
        Punct::StrictEqual => (Operator::Binary(BinaryOp::StrictEqual), 7),
        Punct::StrictNotEqual => (Operator::Binary(BinaryOp::StrictNotEqual), 7),
        Punct::AndAnd => (Operator::Logical(LogicalOp::And), 6),
        Punct::OrOr => (Operator::Logical(LogicalOp::Or), 5),
        _ => return None,
    })
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    eof: Token,
    depth: usize,
    too_deep: bool,
    last_end: u32,
    errors: ErrorSink,
}

impl Parser {
    fn parse_root(&mut self) -> Expr {
        let expr = self.parse_expr();
        if !self.too_deep && !self.at_eof() {
            let range = self.peek().range;
            self.error(
                ParseErrorKind::UnexpectedToken,
                range,
                "unexpected token after expression",
            );
        }
        expr
    }

    fn peek(&self) -> Token {
        self.tokens.get(self.index).copied().unwrap_or(self.eof)
    }

    fn bump(&mut self) -> Token {
        let token = self.peek();
        if token.kind != TokenKind::Eof {
            self.index += 1;
            self.last_end = token.range.end();
        }
        token
    }

    fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn at_punct(&self, punct: Punct) -> bool {
        matches!(self.peek().kind, TokenKind::Punct(found) if found == punct)
    }

    fn eat_punct(&mut self, punct: Punct) -> bool {
        if self.at_punct(punct) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, punct: Punct, message: &'static str) -> Option<Token> {
        if self.at_punct(punct) {
            return Some(self.bump());
        }
        if !self.too_deep {
            let range = self.peek().range;
            self.error(ParseErrorKind::InvalidExpression, range, message);
        }
        None
    }

    fn error(&mut self, kind: ParseErrorKind, range: TextRange, message: &'static str) {
        self.errors.push(kind, range, message);
    }

    fn enter(&mut self) -> bool {
        if self.depth >= MAX_EXPRESSION_DEPTH {
            if !self.too_deep {
                self.too_deep = true;
                let range = self.peek().range;
                self.error(
                    ParseErrorKind::ExpressionTooDeep,
                    range,
                    "expression nesting exceeds the supported depth",
                );
            }
            return false;
        }
        self.depth += 1;
        true
    }

    const fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn missing_here(&self) -> Expr {
        let offset = self.peek().range.start();
        Expr::new(ExprKind::Missing, span(offset, offset))
    }

    fn parse_expr(&mut self) -> Expr {
        if self.too_deep {
            return self.missing_here();
        }
        if !self.enter() {
            return self.missing_here();
        }
        let mut lhs = self.parse_binary(0);
        if !self.too_deep && self.eat_punct(Punct::Question) {
            let then_expr = self.parse_expr();
            if !self.eat_punct(Punct::Colon) && !self.too_deep {
                let range = self.peek().range;
                self.error(
                    ParseErrorKind::InvalidExpression,
                    range,
                    "conditional expression is missing ':'",
                );
            }
            let else_expr = self.parse_expr();
            let range = span(lhs.range().start(), else_expr.range().end());
            lhs = Expr::new(
                ExprKind::Conditional {
                    condition: Box::new(lhs),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                },
                range,
            );
        }
        self.leave();
        lhs
    }

    fn parse_binary(&mut self, min_binding: u8) -> Expr {
        if self.too_deep {
            return self.missing_here();
        }
        let mut lhs = self.parse_unary();
        while !self.too_deep {
            let Some((operator, binding)) = binary_operator(self.peek().kind) else {
                break;
            };
            if binding < min_binding {
                break;
            }
            self.bump();
            if !self.enter() {
                break;
            }
            let rhs = self.parse_binary(binding + 1);
            self.leave();
            let range = span(lhs.range().start(), rhs.range().end());
            lhs = match operator {
                Operator::Binary(op) => Expr::new(
                    ExprKind::Binary {
                        op,
                        left: Box::new(lhs),
                        right: Box::new(rhs),
                    },
                    range,
                ),
                Operator::Logical(op) => Expr::new(
                    ExprKind::Logical {
                        op,
                        left: Box::new(lhs),
                        right: Box::new(rhs),
                    },
                    range,
                ),
            };
        }
        lhs
    }

    fn parse_unary(&mut self) -> Expr {
        if self.too_deep {
            return self.missing_here();
        }
        let token = self.peek();
        let op = match token.kind {
            TokenKind::Punct(Punct::Bang) => Some(UnaryOp::Not),
            TokenKind::Punct(Punct::Minus) => Some(UnaryOp::Negate),
            TokenKind::Punct(Punct::Plus) => Some(UnaryOp::Plus),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            if !self.enter() {
                return self.missing_here();
            }
            let operand = self.parse_unary();
            self.leave();
            let range = span(token.range.start(), operand.range().end());
            return Expr::new(
                ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
                range,
            );
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut expr = self.parse_primary();
        loop {
            if self.too_deep || self.at_eof() {
                break;
            }
            match self.peek().kind {
                TokenKind::Punct(Punct::Dot) => {
                    self.bump();
                    let property = self.parse_property();
                    let range = span(expr.range().start(), property.range().end());
                    expr = Expr::new(
                        ExprKind::Member {
                            object: Box::new(expr),
                            property: Box::new(property),
                            optional: false,
                        },
                        range,
                    );
                }
                TokenKind::Punct(Punct::LeftBracket) => {
                    self.bump();
                    if !self.enter() {
                        break;
                    }
                    let index = self.parse_expr();
                    self.leave();
                    let end = self
                        .expect_punct(Punct::RightBracket, "index expression is missing ']'")
                        .map_or_else(|| index.range().end(), |token| token.range.end());
                    let range = span(expr.range().start(), end);
                    expr = Expr::new(
                        ExprKind::Index {
                            object: Box::new(expr),
                            index: Box::new(index),
                        },
                        range,
                    );
                }
                TokenKind::Punct(Punct::LeftParen) => {
                    self.bump();
                    if !self.enter() {
                        break;
                    }
                    let arguments = self.parse_arguments();
                    self.leave();
                    let end = self
                        .expect_punct(Punct::RightParen, "call is missing ')'")
                        .map_or_else(|| self.last_end, |token| token.range.end());
                    let range = span(expr.range().start(), end);
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            arguments,
                        },
                        range,
                    );
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_property(&mut self) -> Expr {
        let token = self.peek();
        if token.kind == TokenKind::Ident {
            self.bump();
            return Expr::new(ExprKind::Identifier { name: token.range }, token.range);
        }
        self.error(
            ParseErrorKind::InvalidExpression,
            token.range,
            "expected a property name after '.'",
        );
        self.missing_here()
    }

    fn parse_arguments(&mut self) -> Vec<Expr> {
        let mut arguments = Vec::new();
        loop {
            if self.too_deep || self.at_eof() || self.at_punct(Punct::RightParen) {
                break;
            }
            let before = self.index;
            arguments.push(self.parse_expr());
            if self.eat_punct(Punct::Comma) {
                continue;
            }
            if self.index == before {
                // The expression parser made no progress; skip the token so
                // the loop always terminates.
                self.bump();
            }
            break;
        }
        arguments
    }

    fn parse_primary(&mut self) -> Expr {
        let token = self.peek();
        match token.kind {
            TokenKind::Ident => {
                self.bump();
                Expr::new(ExprKind::Identifier { name: token.range }, token.range)
            }
            TokenKind::Number => {
                self.bump();
                Expr::new(ExprKind::Number, token.range)
            }
            TokenKind::String {
                quote,
                content,
                terminated,
            } => {
                self.bump();
                if !terminated {
                    self.error(
                        ParseErrorKind::InvalidExpression,
                        token.range,
                        "string literal is not terminated",
                    );
                }
                Expr::new(ExprKind::String { quote, content }, token.range)
            }
            TokenKind::Keyword(keyword) => {
                self.bump();
                let kind = match keyword {
                    Keyword::True => ExprKind::Boolean(true),
                    Keyword::False => ExprKind::Boolean(false),
                    Keyword::Null => ExprKind::Null,
                };
                Expr::new(kind, token.range)
            }
            TokenKind::Punct(Punct::LeftParen) => self.parse_parenthesized(),
            TokenKind::Punct(Punct::LeftBracket) => self.parse_array(),
            TokenKind::Punct(Punct::LeftBrace) => self.parse_object(),
            _ => {
                self.error(
                    ParseErrorKind::InvalidExpression,
                    token.range,
                    "expected an expression",
                );
                self.missing_here()
            }
        }
    }

    fn parse_parenthesized(&mut self) -> Expr {
        let open = self.bump();
        if !self.enter() {
            return Expr::new(
                ExprKind::Parenthesized {
                    expr: Box::new(self.missing_here()),
                },
                open.range,
            );
        }
        let inner = self.parse_expr();
        self.leave();
        let end = self
            .expect_punct(Punct::RightParen, "parenthesized expression is missing ')'")
            .map_or_else(|| inner.range().end(), |token| token.range.end());
        Expr::new(
            ExprKind::Parenthesized {
                expr: Box::new(inner),
            },
            span(open.range.start(), end),
        )
    }

    fn parse_array(&mut self) -> Expr {
        let open = self.bump();
        let mut items = Vec::new();
        let mut closed = false;
        if self.enter() {
            loop {
                if self.too_deep || self.at_eof() {
                    break;
                }
                if self.at_punct(Punct::RightBracket) {
                    self.bump();
                    closed = true;
                    break;
                }
                let before = self.index;
                items.push(self.parse_expr());
                if self.eat_punct(Punct::Comma) {
                    continue;
                }
                if self.at_punct(Punct::RightBracket) {
                    self.bump();
                    closed = true;
                    break;
                }
                if self.index == before {
                    self.bump();
                }
                break;
            }
            self.leave();
        }
        if !closed && !self.too_deep {
            let range = self.peek().range;
            self.error(
                ParseErrorKind::InvalidExpression,
                range,
                "array literal is missing ']'",
            );
        }
        let range = span(open.range.start(), self.last_end.max(open.range.end()));
        Expr::new(ExprKind::Array(items), range)
    }

    fn parse_object(&mut self) -> Expr {
        let open = self.bump();
        let mut entries = Vec::new();
        let mut closed = false;
        if self.enter() {
            loop {
                if self.too_deep || self.at_eof() {
                    break;
                }
                if self.at_punct(Punct::RightBrace) {
                    self.bump();
                    closed = true;
                    break;
                }
                let before = self.index;
                let Some(entry) = self.parse_object_entry() else {
                    break;
                };
                entries.push(entry);
                if self.eat_punct(Punct::Comma) {
                    continue;
                }
                if self.at_punct(Punct::RightBrace) {
                    self.bump();
                    closed = true;
                    break;
                }
                if self.index == before {
                    self.bump();
                }
                break;
            }
            self.leave();
        }
        if !closed && !self.too_deep {
            let range = self.peek().range;
            self.error(
                ParseErrorKind::InvalidExpression,
                range,
                "object literal is missing '}'",
            );
        }
        let range = span(open.range.start(), self.last_end.max(open.range.end()));
        Expr::new(ExprKind::Object(entries), range)
    }

    fn parse_object_entry(&mut self) -> Option<ObjectEntry> {
        let token = self.peek();
        let key = match token.kind {
            TokenKind::Ident => {
                self.bump();
                ObjectKey::Identifier { name: token.range }
            }
            TokenKind::String { quote, content, .. } => {
                self.bump();
                ObjectKey::String { quote, content }
            }
            TokenKind::Number => {
                self.bump();
                ObjectKey::Number {
                    literal: token.range,
                }
            }
            _ => {
                self.error(
                    ParseErrorKind::InvalidExpression,
                    token.range,
                    "expected an object key",
                );
                return None;
            }
        };
        if self.eat_punct(Punct::Colon) {
            let value = self.parse_expr();
            let range = span(token.range.start(), value.range().end());
            return Some(ObjectEntry {
                key,
                value,
                shorthand: false,
                range,
            });
        }
        if let ObjectKey::Identifier { name } = key {
            let value = Expr::new(ExprKind::Identifier { name }, name);
            return Some(ObjectEntry {
                key: ObjectKey::Identifier { name },
                value,
                shorthand: true,
                range: name,
            });
        }
        self.error(
            ParseErrorKind::InvalidExpression,
            token.range,
            "object entry is missing ':'",
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::parse_expression;
    use crate::ParseErrorKind;
    use crate::expr::{BinaryOp, Expr, ExprKind, ExprParseResult, LogicalOp, ObjectKey, UnaryOp};
    use wxml_text::TextRange;

    fn parse(source: &str) -> ExprParseResult {
        parse_expression(source, 0)
    }

    fn parse_clean(source: &str) -> Expr {
        let result = parse(source);
        assert!(
            result.errors.is_empty(),
            "expected no errors for {source:?}, got {:?}",
            result.errors
        );
        result.expr
    }

    fn text_of(source: &str, range: TextRange) -> String {
        source[range.start() as usize..range.end() as usize].to_owned()
    }

    fn u32_len(text: impl AsRef<str>) -> u32 {
        u32::try_from(text.as_ref().len()).expect("test input fits in u32")
    }

    fn identifier_name<'a>(expr: &Expr, source: &'a str) -> Option<&'a str> {
        match expr.kind() {
            ExprKind::Identifier { name } => source.get(name.start() as usize..name.end() as usize),
            _ => None,
        }
    }

    fn binary(expr: &Expr) -> (BinaryOp, &Expr, &Expr) {
        match expr.kind() {
            ExprKind::Binary { op, left, right } => (*op, left, right),
            other => panic!("expected a binary expression, got {other:?}"),
        }
    }

    fn logical(expr: &Expr) -> (LogicalOp, &Expr, &Expr) {
        match expr.kind() {
            ExprKind::Logical { op, left, right } => (*op, left, right),
            other => panic!("expected a logical expression, got {other:?}"),
        }
    }

    fn range_of(expr: &Expr) -> (u32, u32) {
        (expr.range().start(), expr.range().end())
    }

    #[test]
    fn parses_an_identifier_with_an_exact_range() {
        let expr = parse_clean("user_name");
        assert_eq!(identifier_name(&expr, "user_name"), Some("user_name"));
        assert_eq!(range_of(&expr), (0, 9));
        assert_eq!(
            expr.kind(),
            &ExprKind::Identifier {
                name: TextRange::new(0, 9).expect("valid range")
            }
        );
    }

    #[test]
    fn parses_unicode_identifiers() {
        let source = "用户名";
        let expr = parse_clean(source);
        assert_eq!(identifier_name(&expr, source), Some(source));
        assert_eq!(range_of(&expr), (0, 9));
    }

    #[test]
    fn parses_number_literals() {
        for source in ["0", "42", "1.5", ".5", "1e3", "1.5e-2", "0xFF", "0b101"] {
            let expr = parse_clean(source);
            assert_eq!(expr.kind(), &ExprKind::Number, "source {source:?}");
            assert_eq!(range_of(&expr), (0, u32_len(source)), "source {source:?}");
        }
    }

    #[test]
    fn parses_string_literals_and_keeps_the_content_range() {
        let source = r"'a\'b'";
        let expr = parse_clean(source);
        match expr.kind() {
            ExprKind::String { quote, content } => {
                assert_eq!(*quote, '\'');
                assert_eq!(text_of(source, *content), r"a\'b");
                assert_eq!(range_of(&expr), (0, u32_len(source)));
            }
            other => panic!("expected a string literal, got {other:?}"),
        }

        let source = r#""double""#;
        let expr = parse_clean(source);
        match expr.kind() {
            ExprKind::String { quote, content } => {
                assert_eq!(*quote, '"');
                assert_eq!(text_of(source, *content), "double");
            }
            other => panic!("expected a string literal, got {other:?}"),
        }
    }

    #[test]
    fn parses_boolean_and_null_literals() {
        assert_eq!(parse_clean("true").kind(), &ExprKind::Boolean(true));
        assert_eq!(parse_clean("false").kind(), &ExprKind::Boolean(false));
        assert_eq!(parse_clean("null").kind(), &ExprKind::Null);
    }

    #[test]
    fn member_access_binds_tighter_than_binary_operators() {
        let source = "user.age + 1";
        let expr = parse_clean(source);
        let (op, left, right) = binary(&expr);
        assert_eq!(op, BinaryOp::Add);
        assert_eq!(range_of(right), (11, 12));

        match left.kind() {
            ExprKind::Member {
                object,
                property,
                optional,
            } => {
                assert!(!optional);
                assert_eq!(identifier_name(object, source), Some("user"));
                assert_eq!(identifier_name(property, source), Some("age"));
            }
            other => panic!("expected member access, got {other:?}"),
        }
    }

    #[test]
    fn postfix_operations_chain_left_to_right() {
        let source = "a.b[c].d(1, 2)";
        let expr = parse_clean(source);

        let ExprKind::Call { callee, arguments } = expr.kind() else {
            panic!("expected a call, got {:?}", expr.kind());
        };
        assert_eq!(arguments.len(), 2);
        assert_eq!(range_of(&expr), (0, u32_len(source)));

        let ExprKind::Member {
            object,
            property,
            optional,
        } = callee.kind()
        else {
            panic!("expected member access, got {:?}", callee.kind());
        };
        assert!(!optional);
        assert_eq!(identifier_name(property, source), Some("d"));

        let ExprKind::Index { object, index } = object.kind() else {
            panic!("expected an index expression, got {:?}", object.kind());
        };
        assert_eq!(identifier_name(index, source), Some("c"));

        let ExprKind::Member {
            object,
            property,
            optional,
        } = object.kind()
        else {
            panic!("expected nested member access, got {:?}", object.kind());
        };
        assert!(!optional);
        assert_eq!(identifier_name(object, source), Some("a"));
        assert_eq!(identifier_name(property, source), Some("b"));
    }

    #[test]
    fn multiplicative_binds_tighter_than_additive() {
        let expr = parse_clean("a + b * c");
        let (op, left, right) = binary(&expr);
        assert_eq!(op, BinaryOp::Add);
        assert_eq!(identifier_name(left, "a + b * c"), Some("a"));
        let (inner_op, inner_left, inner_right) = binary(right);
        assert_eq!(inner_op, BinaryOp::Multiply);
        assert_eq!(identifier_name(inner_left, "a + b * c"), Some("b"));
        assert_eq!(identifier_name(inner_right, "a + b * c"), Some("c"));

        let expr = parse_clean("a * b + c");
        let (op, left, _) = binary(&expr);
        assert_eq!(op, BinaryOp::Add);
        assert_eq!(binary(left).0, BinaryOp::Multiply);
    }

    #[test]
    fn comparison_binds_tighter_than_equality() {
        let source = "a + b < c == d";
        let expr = parse_clean(source);
        let (op, left, right) = binary(&expr);
        assert_eq!(op, BinaryOp::Equal);
        assert_eq!(identifier_name(right, source), Some("d"));

        let (op, left, right) = binary(left);
        assert_eq!(op, BinaryOp::Less);
        assert_eq!(binary(left).0, BinaryOp::Add);
        assert_eq!(identifier_name(right, source), Some("c"));
    }

    #[test]
    fn logical_and_binds_tighter_than_logical_or() {
        let source = "a || b && c";
        let expr = parse_clean(source);
        let (op, left, right) = logical(&expr);
        assert_eq!(op, LogicalOp::Or);
        assert_eq!(identifier_name(left, source), Some("a"));
        let (inner_op, inner_left, inner_right) = logical(right);
        assert_eq!(inner_op, LogicalOp::And);
        assert_eq!(identifier_name(inner_left, source), Some("b"));
        assert_eq!(identifier_name(inner_right, source), Some("c"));
    }

    #[test]
    fn logical_operators_accept_comparisons() {
        let source = "a > 1 && b !== 2";
        let expr = parse_clean(source);
        let (op, left, right) = logical(&expr);
        assert_eq!(op, LogicalOp::And);
        assert_eq!(binary(left).0, BinaryOp::Greater);
        assert_eq!(binary(right).0, BinaryOp::StrictNotEqual);
    }

    #[test]
    fn unary_operators_bind_after_member_access() {
        let source = "!user.active";
        let expr = parse_clean(source);
        let ExprKind::Unary { op, operand } = expr.kind() else {
            panic!("expected a unary expression, got {:?}", expr.kind());
        };
        assert_eq!(*op, UnaryOp::Not);
        assert!(matches!(operand.kind(), ExprKind::Member { .. }));

        let expr = parse_clean("-a + +b");
        let (op, left, right) = binary(&expr);
        assert_eq!(op, BinaryOp::Add);
        assert!(matches!(
            left.kind(),
            ExprKind::Unary {
                op: UnaryOp::Negate,
                ..
            }
        ));
        assert!(matches!(
            right.kind(),
            ExprKind::Unary {
                op: UnaryOp::Plus,
                ..
            }
        ));
    }

    #[test]
    fn conditional_is_the_loosest_operator_and_right_associative() {
        let source = "a ? b : c ? d : e";
        let expr = parse_clean(source);
        let ExprKind::Conditional {
            condition,
            then_expr,
            else_expr,
        } = expr.kind()
        else {
            panic!("expected a conditional, got {:?}", expr.kind());
        };
        assert_eq!(identifier_name(condition, source), Some("a"));
        assert_eq!(identifier_name(then_expr, source), Some("b"));
        let ExprKind::Conditional {
            condition,
            then_expr,
            else_expr,
        } = else_expr.kind()
        else {
            panic!("expected a nested conditional, got {:?}", else_expr.kind());
        };
        assert_eq!(identifier_name(condition, source), Some("c"));
        assert_eq!(identifier_name(then_expr, source), Some("d"));
        assert_eq!(identifier_name(else_expr, source), Some("e"));
    }

    #[test]
    fn parentheses_override_precedence() {
        let source = "(a + b) * c";
        let expr = parse_clean(source);
        let (op, left, right) = binary(&expr);
        assert_eq!(op, BinaryOp::Multiply);
        assert_eq!(identifier_name(right, source), Some("c"));
        let ExprKind::Parenthesized { expr: inner } = left.kind() else {
            panic!("expected parentheses, got {:?}", left.kind());
        };
        assert_eq!(binary(inner).0, BinaryOp::Add);
        assert_eq!(range_of(left), (0, 7));
    }

    #[test]
    fn parses_array_literals_with_trailing_commas() {
        let source = "[1, 'two', true,]";
        let expr = parse_clean(source);
        let ExprKind::Array(items) = expr.kind() else {
            panic!("expected an array literal, got {:?}", expr.kind());
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind(), &ExprKind::Number);
        assert_eq!(items[2].kind(), &ExprKind::Boolean(true));
        assert_eq!(range_of(&expr), (0, u32_len(source)));

        assert_eq!(parse_clean("[]").kind(), &ExprKind::Array(Vec::new()));
    }

    #[test]
    fn parses_nested_object_and_array_literals() {
        let source = "{ a: { b: [1, 2] }, c: true }";
        let expr = parse_clean(source);
        let ExprKind::Object(entries) = expr.kind() else {
            panic!("expected an object literal, got {:?}", expr.kind());
        };
        assert_eq!(entries.len(), 2);
        let ExprKind::Object(inner) = entries[0].value.kind() else {
            panic!(
                "expected a nested object, got {:?}",
                entries[0].value.kind()
            );
        };
        let ExprKind::Array(items) = inner[0].value.kind() else {
            panic!("expected an array, got {:?}", inner[0].value.kind());
        };
        assert_eq!(items.len(), 2);
        assert_eq!(entries[1].value.kind(), &ExprKind::Boolean(true));
    }

    #[test]
    fn parses_object_literals_with_shorthand_keys() {
        let source = r#"{ a: 1, b, "c": null }"#;
        let expr = parse_clean(source);
        let ExprKind::Object(entries) = expr.kind() else {
            panic!("expected an object literal, got {:?}", expr.kind());
        };
        assert_eq!(entries.len(), 3);
        assert!(!entries[0].shorthand);
        assert!(entries[1].shorthand);
        assert_eq!(
            entries[1].value.kind(),
            &ExprKind::Identifier {
                name: TextRange::new(8, 9).expect("valid range"),
            }
        );
        match &entries[2].key {
            ObjectKey::String { quote, content } => {
                assert_eq!(*quote, '"');
                assert_eq!(text_of(source, *content), "c");
            }
            other => panic!("expected a string key, got {other:?}"),
        }
        assert_eq!(range_of(&expr), (0, u32_len(source)));
    }

    #[test]
    fn empty_input_is_a_missing_expression() {
        let result = parse("");
        assert_eq!(result.expr.kind(), &ExprKind::Missing);
        assert!(result.expr.range().is_empty());
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::InvalidExpression)
        );
    }

    #[test]
    fn reports_trailing_tokens_without_dropping_the_expression() {
        let result = parse("a b");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::UnexpectedToken)
        );
        assert_eq!(identifier_name(&result.expr, "a b"), Some("a"));
    }

    #[test]
    fn reports_unterminated_strings_and_keeps_the_literal() {
        let result = parse("'abc");
        assert!(
            result
                .errors
                .iter()
                .any(|error| matches!(error.kind(), ParseErrorKind::InvalidExpression))
        );
        assert!(matches!(result.expr.kind(), ExprKind::String { .. }));
    }

    #[test]
    fn reports_invalid_characters_and_keeps_parsing() {
        let result = parse("a @ b");
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::InvalidCharacter)
        );
        assert!(matches!(result.expr.kind(), ExprKind::Identifier { .. }));
    }

    #[test]
    fn reports_incomplete_member_access() {
        let result = parse("a.");
        assert!(!result.errors.is_empty(), "expected at least one error");
        assert!(matches!(result.expr.kind(), ExprKind::Member { .. }));
    }

    #[test]
    fn rejects_expression_nesting_beyond_the_limit_without_an_error_storm() {
        let source = format!("{}1{}", "(".repeat(300), ")".repeat(300));
        let result = parse(&source);
        let depth_errors = result
            .errors
            .iter()
            .filter(|error| error.kind() == ParseErrorKind::ExpressionTooDeep)
            .count();
        assert_eq!(depth_errors, 1, "errors: {:?}", result.errors);
        assert_eq!(result.errors.len(), 1, "errors: {:?}", result.errors);
        assert!(result.expr.range().end() <= u32_len(&source));
    }

    #[test]
    fn rejects_expression_token_counts_beyond_the_limit() {
        let source = "a+".repeat(3000);
        let result = parse(&source);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::ExpressionTooManyTokens),
            "errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn ranges_are_absolute_when_a_base_offset_is_given() {
        let result = parse_expression("a + 1", 100);
        let expr = result.expr;
        assert_eq!(range_of(&expr), (100, 105));
        let (_, left, right) = binary(&expr);
        assert_eq!(range_of(left), (100, 101));
        assert_eq!(range_of(right), (104, 105));
    }

    #[test]
    fn walks_every_expression_node_in_preorder() {
        let expr = parse_clean("a + b.c");
        let kinds: Vec<&ExprKind> = expr.walk().map(Expr::kind).collect();
        assert_eq!(kinds.len(), 5);
        assert!(matches!(kinds[0], ExprKind::Binary { .. }));
        assert!(matches!(kinds[1], ExprKind::Identifier { .. }));
        assert!(matches!(kinds[2], ExprKind::Member { .. }));
        assert!(matches!(kinds[3], ExprKind::Identifier { .. }));
        assert!(matches!(kinds[4], ExprKind::Identifier { .. }));
    }

    #[test]
    fn children_exposes_direct_operands() {
        let expr = parse_clean("f(a, b)");
        assert_eq!(expr.children().count(), 3);
        let expr = parse_clean("a");
        assert_eq!(expr.children().count(), 0);
    }
}
