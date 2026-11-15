//! Tokenizer for the `{{ ... }}` expression subset.
//!
//! The lexer is independent from the markup lexer: the mustache scanner first
//! finds the closing `}}` (respecting strings and brackets), then this module
//! splits the inner text into tokens whose ranges are absolute in the original
//! document. The number of tokens is capped, so a hostile expression cannot
//! force unbounded parser work.

use crate::error::{ErrorSink, ParseErrorKind, span};
use crate::expr::MAX_EXPRESSION_TOKENS;
use wxml_text::TextRange;

/// Punctuation and operator tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Punct {
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Equal,
    NotEqual,
    StrictEqual,
    StrictNotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    AndAnd,
    OrOr,
    Bang,
    Question,
    Colon,
    Dot,
    Comma,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
}

/// Literal keywords with dedicated AST nodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Keyword {
    True,
    False,
    Null,
}

/// A token kind with all payloads copied by value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TokenKind {
    /// A bare identifier.
    Ident,
    /// A numeric literal.
    Number,
    /// A quoted string literal.
    String {
        /// Opening quote character.
        quote: char,
        /// Range of the raw content between the quotes.
        content: TextRange,
        /// `false` when the input ended before the closing quote.
        terminated: bool,
    },
    /// A literal keyword.
    Keyword(Keyword),
    /// Punctuation or an operator.
    Punct(Punct),
    /// End of the expression.
    Eof,
}

/// A token with its absolute source range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Token {
    pub kind: TokenKind,
    pub range: TextRange,
}

/// Tokens plus lexical diagnostics.
pub(super) struct LexResult {
    pub tokens: Vec<Token>,
    pub errors: ErrorSink,
}

/// Tokenizes `text`, treating `base` as the offset of `text[0]` in the
/// original document.
pub(super) fn lex(text: &str, base: u32) -> LexResult {
    let mut lexer = Lexer {
        text,
        base,
        offset: 0,
        tokens: Vec::new(),
        errors: ErrorSink::new(),
    };
    lexer.run();
    lexer.finish()
}

struct Lexer<'a> {
    text: &'a str,
    base: u32,
    offset: usize,
    tokens: Vec<Token>,
    errors: ErrorSink,
}

impl<'a> Lexer<'a> {
    fn run(&mut self) {
        while self.offset < self.text.len() {
            if self.tokens.len() >= MAX_EXPRESSION_TOKENS {
                self.error(
                    ParseErrorKind::ExpressionTooManyTokens,
                    self.offset,
                    self.text.len(),
                    "expression contains too many tokens",
                );
                self.offset = self.text.len();
                break;
            }
            if !self.next_token() {
                break;
            }
        }
    }

    fn finish(self) -> LexResult {
        let end = self.absolute(self.text.len());
        let mut tokens = self.tokens;
        tokens.push(Token {
            kind: TokenKind::Eof,
            range: span(end, end),
        });
        LexResult {
            tokens,
            errors: self.errors,
        }
    }

    fn absolute(&self, offset: usize) -> u32 {
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        self.base.saturating_add(offset)
    }

    fn remaining(&self) -> &'a str {
        self.text.get(self.offset..).unwrap_or_default()
    }

    fn peek_char(&self) -> Option<(char, usize)> {
        let mut chars = self.remaining().chars();
        let c = chars.next()?;
        Some((c, c.len_utf8()))
    }

    fn peek_char_after(&self, offset: usize) -> Option<(char, usize)> {
        let mut chars = self.text.get(offset..)?.chars();
        let c = chars.next()?;
        Some((c, c.len_utf8()))
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            range: span(self.absolute(start), self.absolute(self.offset)),
        });
    }

    fn error(&mut self, kind: ParseErrorKind, start: usize, end: usize, message: &'static str) {
        self.errors.push(
            kind,
            span(self.absolute(start), self.absolute(end)),
            message,
        );
    }

    /// Consumes one token. Returns `false` when there is nothing left.
    fn next_token(&mut self) -> bool {
        self.skip_whitespace();
        let Some((first, first_len)) = self.peek_char() else {
            return false;
        };
        if is_identifier_start(first) {
            self.scan_identifier();
            return true;
        }
        if first.is_ascii_digit() {
            self.scan_number();
            return true;
        }
        if first == '.'
            && self
                .peek_char_after(self.offset + first_len)
                .is_some_and(|(c, _)| c.is_ascii_digit())
        {
            self.scan_number();
            return true;
        }
        if first == '\'' || first == '"' {
            self.scan_string(first);
            return true;
        }
        if self.scan_operator() {
            return true;
        }
        let start = self.offset;
        self.offset += first_len;
        self.error(
            ParseErrorKind::InvalidCharacter,
            start,
            self.offset,
            "character is not part of a template expression",
        );
        true
    }

    fn skip_whitespace(&mut self) {
        while let Some((c, len)) = self.peek_char() {
            if !c.is_whitespace() {
                break;
            }
            self.offset += len;
        }
    }

    fn scan_identifier(&mut self) {
        let start = self.offset;
        while let Some((c, len)) = self.peek_char() {
            if !is_identifier_continue(c) {
                break;
            }
            self.offset += len;
        }
        let name = self.text.get(start..self.offset).unwrap_or_default();
        let kind = match name {
            "true" => TokenKind::Keyword(Keyword::True),
            "false" => TokenKind::Keyword(Keyword::False),
            "null" => TokenKind::Keyword(Keyword::Null),
            _ => TokenKind::Ident,
        };
        self.push(kind, start);
    }

    fn scan_number(&mut self) {
        let start = self.offset;
        let rest = self.remaining();
        let mut radix_digits: Option<fn(char) -> bool> = None;
        if rest.len() > 2 && rest.starts_with('0') {
            let prefix = rest.as_bytes().get(1).copied();
            let digit_ok: Option<fn(char) -> bool> = match prefix {
                Some(b'x' | b'X') => Some(|c: char| c.is_ascii_hexdigit()),
                Some(b'b' | b'B') => Some(|c: char| matches!(c, '0' | '1')),
                Some(b'o' | b'O') => Some(|c: char| matches!(c, '0'..='7')),
                _ => None,
            };
            if let Some(digit_ok) = digit_ok {
                if self
                    .peek_char_after(self.offset + 2)
                    .is_some_and(|(c, _)| digit_ok(c))
                {
                    radix_digits = Some(digit_ok);
                    self.offset += 2;
                }
            }
        }
        if let Some(digit_ok) = radix_digits {
            while let Some((c, len)) = self.peek_char() {
                if !digit_ok(c) {
                    break;
                }
                self.offset += len;
            }
            self.push(TokenKind::Number, start);
            return;
        }
        self.consume_ascii_digits();
        if self.remaining().starts_with('.')
            && self
                .peek_char_after(self.offset + 1)
                .is_some_and(|(c, _)| c.is_ascii_digit())
        {
            self.offset += 1;
            self.consume_ascii_digits();
        }
        if let Some(('e' | 'E', len)) = self.peek_char() {
            if self.exponent_has_digits(self.offset + len) {
                self.offset += len;
                if let Some(('+' | '-', sign_len)) = self.peek_char() {
                    self.offset += sign_len;
                }
                self.consume_ascii_digits();
            }
        }
        self.push(TokenKind::Number, start);
    }

    fn consume_ascii_digits(&mut self) {
        while let Some((c, len)) = self.peek_char() {
            if !c.is_ascii_digit() {
                break;
            }
            self.offset += len;
        }
    }

    fn exponent_has_digits(&self, offset: usize) -> bool {
        let mut chars = self.text.get(offset..).unwrap_or_default().chars();
        let first = chars.next();
        let candidate = match first {
            Some('+' | '-') => chars.next(),
            other => other,
        };
        candidate.is_some_and(|c| c.is_ascii_digit())
    }

    fn scan_string(&mut self, quote: char) {
        let start = self.offset;
        self.offset += quote.len_utf8();
        let content_start = self.offset;
        let mut terminated = false;
        while let Some((c, len)) = self.peek_char() {
            if c == '\\' {
                self.offset += len;
                if let Some((_, escaped_len)) = self.peek_char() {
                    self.offset += escaped_len;
                }
                continue;
            }
            self.offset += len;
            if c == quote {
                terminated = true;
                break;
            }
        }
        let content_end = if terminated {
            self.offset.saturating_sub(quote.len_utf8())
        } else {
            self.offset
        };
        if !terminated {
            self.error(
                ParseErrorKind::InvalidExpression,
                start,
                self.offset,
                "string literal is not terminated",
            );
        }
        self.push(
            TokenKind::String {
                quote,
                content: span(self.absolute(content_start), self.absolute(content_end)),
                terminated,
            },
            start,
        );
    }

    /// Scans the longest operator at the cursor. Returns `false` when the
    /// character is not an operator and must be reported as invalid.
    fn scan_operator(&mut self) -> bool {
        let start = self.offset;
        let rest = self.remaining();
        let (punct, len) = if rest.starts_with("===") {
            (Punct::StrictEqual, 3)
        } else if rest.starts_with("!==") {
            (Punct::StrictNotEqual, 3)
        } else if rest.starts_with("==") {
            (Punct::Equal, 2)
        } else if rest.starts_with("!=") {
            (Punct::NotEqual, 2)
        } else if rest.starts_with("<=") {
            (Punct::LessOrEqual, 2)
        } else if rest.starts_with(">=") {
            (Punct::GreaterOrEqual, 2)
        } else if rest.starts_with("&&") {
            (Punct::AndAnd, 2)
        } else if rest.starts_with("||") {
            (Punct::OrOr, 2)
        } else {
            let (c, len) = self.peek_char().unwrap_or(('\0', 1));
            let punct = match c {
                '+' => Punct::Plus,
                '-' => Punct::Minus,
                '*' => Punct::Star,
                '/' => Punct::Slash,
                '%' => Punct::Percent,
                '<' => Punct::Less,
                '>' => Punct::Greater,
                '!' => Punct::Bang,
                '?' => Punct::Question,
                ':' => Punct::Colon,
                '.' => Punct::Dot,
                ',' => Punct::Comma,
                '(' => Punct::LeftParen,
                ')' => Punct::RightParen,
                '[' => Punct::LeftBracket,
                ']' => Punct::RightBracket,
                '{' => Punct::LeftBrace,
                '}' => Punct::RightBrace,
                _ => return false,
            };
            (punct, len)
        };
        self.offset += len;
        self.push(TokenKind::Punct(punct), start);
        true
    }
}

fn is_identifier_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

fn is_identifier_continue(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source, 0)
            .tokens
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    fn puncts(source: &str) -> Vec<Punct> {
        lex(source, 0)
            .tokens
            .into_iter()
            .filter_map(|token| match token.kind {
                TokenKind::Punct(punct) => Some(punct),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn tokenizes_identifiers_and_keywords() {
        assert_eq!(
            kinds("name true false null"),
            vec![
                TokenKind::Ident,
                TokenKind::Keyword(Keyword::True),
                TokenKind::Keyword(Keyword::False),
                TokenKind::Keyword(Keyword::Null),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_unicode_identifiers() {
        assert_eq!(kinds("用户名"), vec![TokenKind::Ident, TokenKind::Eof]);
    }

    #[test]
    fn tokenizes_number_forms() {
        for source in [
            "0", "42", "1.5", ".5", "1e3", "1E+3", "1.5e-2", "0xFF", "0b101", "0o17",
        ] {
            let tokens = lex(source, 0);
            let errors = tokens.errors.into_vec();
            assert!(errors.is_empty(), "errors for {source:?}: {errors:?}");
            assert_eq!(
                tokens.tokens.first().map(|token| token.kind),
                Some(TokenKind::Number),
                "source {source:?}"
            );
        }
        // A number stops before an exponent marker that has no digits.
        let tokens = lex("1e", 0);
        assert_eq!(tokens.tokens[0].kind, TokenKind::Number);
        assert_eq!(tokens.tokens[1].kind, TokenKind::Ident);
    }

    #[test]
    fn tokenizes_escaped_strings_verbatim() {
        let tokens = lex(r#" "a\"b" 'c\\' "#, 0);
        let errors = tokens.errors.into_vec();
        assert!(errors.is_empty(), "errors: {errors:?}");
        match tokens.tokens[0].kind {
            TokenKind::String {
                quote,
                content,
                terminated,
            } => {
                assert_eq!(quote, '"');
                assert!(terminated);
                assert_eq!(
                    &r#" "a\"b" 'c\\' "#[content.start() as usize..content.end() as usize],
                    r#"a\"b"#
                );
            }
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn reports_unterminated_strings() {
        let tokens = lex("'abc", 0);
        let errors = tokens.errors.into_vec();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind(), ParseErrorKind::InvalidExpression);
        assert!(matches!(
            tokens.tokens[0].kind,
            TokenKind::String {
                terminated: false,
                ..
            }
        ));
    }

    #[test]
    fn tokenizes_all_operators() {
        assert_eq!(
            puncts("+ - * / % == != === !== < <= > >= && || ! ? : . , ( ) [ ] { }"),
            vec![
                Punct::Plus,
                Punct::Minus,
                Punct::Star,
                Punct::Slash,
                Punct::Percent,
                Punct::Equal,
                Punct::NotEqual,
                Punct::StrictEqual,
                Punct::StrictNotEqual,
                Punct::Less,
                Punct::LessOrEqual,
                Punct::Greater,
                Punct::GreaterOrEqual,
                Punct::AndAnd,
                Punct::OrOr,
                Punct::Bang,
                Punct::Question,
                Punct::Colon,
                Punct::Dot,
                Punct::Comma,
                Punct::LeftParen,
                Punct::RightParen,
                Punct::LeftBracket,
                Punct::RightBracket,
                Punct::LeftBrace,
                Punct::RightBrace,
            ]
        );
    }

    #[test]
    fn reports_invalid_characters_and_skips_them() {
        let tokens = lex("a @ b", 0);
        let errors = tokens.errors.into_vec();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind(), ParseErrorKind::InvalidCharacter);
        assert_eq!(errors[0].range().start(), 2);
        assert_eq!(errors[0].range().end(), 3);
        assert_eq!(
            kinds("a @ b")[..3],
            [TokenKind::Ident, TokenKind::Ident, TokenKind::Eof]
        );
    }

    #[test]
    fn ranges_are_shifted_by_the_base_offset() {
        let tokens = lex("a + 1", 100);
        assert_eq!(tokens.tokens[0].range, span(100, 101));
        assert_eq!(tokens.tokens[1].range, span(102, 103));
        assert_eq!(tokens.tokens[2].range, span(104, 105));
        assert_eq!(tokens.tokens[3].range, span(105, 105));
    }

    #[test]
    fn caps_the_number_of_tokens() {
        let source = "a+".repeat(MAX_EXPRESSION_TOKENS);
        let tokens = lex(&source, 0);
        let errors = tokens.errors.into_vec();
        assert!(
            errors
                .iter()
                .any(|error| error.kind() == ParseErrorKind::ExpressionTooManyTokens)
        );
        assert!(tokens.tokens.len() <= MAX_EXPRESSION_TOKENS + 1);
        assert_eq!(
            tokens.tokens.last().map(|token| token.kind),
            Some(TokenKind::Eof)
        );
    }
}
