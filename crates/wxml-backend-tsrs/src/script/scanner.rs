//! Conservative token-level scanner shared by the script semantics.
//!
//! The scanner reuses the shadow expression lexer and delimiter matching
//! instead of inventing a second tokenizer. It locates registration calls,
//! first-level object members and `JSDoc` comments; it is deliberately not a
//! TypeScript parser. Anything outside this grammar is reported as
//! [`super::ScriptError::Invalid`] or kept as an opaque member, never guessed.

use crate::shadow::ShadowError;
use crate::shadow::expression::{self, Token, TokenKind};
use crate::shadow::mapping::SourceRange;

use super::ScriptError;
use super::symbols::{ScriptRange, Utf16Index};

/// One lexed script plus delimiter pairs.
#[derive(Debug, Clone)]
pub(crate) struct Script<'a> {
    /// Exact original text.
    pub(crate) text: &'a str,
    /// Lexed tokens with UTF-8 byte offsets.
    pub(crate) tokens: Vec<Token>,
    /// Matching delimiter token index for `(`, `[`, `{` and template starts.
    pub(crate) pairs: Vec<Option<usize>>,
    /// Reusable UTF-8 → UTF-16 boundary index.
    utf16: Utf16Index,
    /// Delimiter depth before each token.
    depths: Vec<usize>,
}

impl<'a> Script<'a> {
    /// Lexes `text` and matches every delimiter.
    pub(crate) fn new(text: &'a str) -> Result<Self, ScriptError> {
        let tokens = expression::lex_script(text).map_err(scan_error)?;
        let pairs = expression::matching_pairs_for_script(&tokens).map_err(scan_error)?;
        let depths = compute_depths(&tokens);
        Ok(Self {
            text,
            tokens,
            pairs,
            utf16: Utf16Index::new(text),
            depths,
        })
    }

    /// Returns the kind of one token.
    pub(crate) fn kind(&self, index: usize) -> Option<TokenKind> {
        self.tokens.get(index).map(|token| token.kind)
    }

    /// Returns the exact text of one token.
    pub(crate) fn token_text(&self, index: usize) -> Option<&'a str> {
        self.tokens.get(index).map(|token| token.text(self.text))
    }

    /// Returns whether a token is the given punctuator.
    pub(crate) fn is_punct(&self, index: usize, character: char) -> bool {
        self.kind(index) == Some(TokenKind::Punctuator(character))
    }

    /// Returns whether a token is the given identifier text.
    pub(crate) fn is_ident(&self, index: usize, name: &str) -> bool {
        self.token_text(index) == Some(name) && self.kind(index) == Some(TokenKind::Identifier)
    }

    /// Returns the matching close token index.
    pub(crate) fn close_of(&self, index: usize) -> Result<usize, ScriptError> {
        self.pairs
            .get(index)
            .copied()
            .flatten()
            .ok_or_else(|| ScriptError::Invalid {
                feature: "script.scan",
                reason: "unbalanced delimiter".to_owned(),
            })
    }

    /// Returns the UTF-8 byte start of a token.
    pub(crate) fn token_start(&self, index: usize) -> Option<usize> {
        self.tokens.get(index).map(|token| token.start)
    }

    /// Returns the exclusive UTF-8 byte end of a token.
    pub(crate) fn token_end(&self, index: usize) -> Option<usize> {
        self.tokens.get(index).map(|token| token.end)
    }

    /// Converts a recorded source range with the cached UTF-16 index.
    pub(crate) fn source_range(&self, range: SourceRange) -> Result<ScriptRange, ScriptError> {
        ScriptRange::from_index(&self.utf16, self.text, range)
    }

    /// Returns the delimiter depth before each token.
    pub(crate) fn depths(&self) -> &[usize] {
        &self.depths
    }
}

fn compute_depths(tokens: &[Token]) -> Vec<usize> {
    let mut depths = Vec::with_capacity(tokens.len());
    let mut depth = 0_usize;
    for token in tokens {
        if matches!(
            token.kind,
            TokenKind::Punctuator(')' | ']' | '}') | TokenKind::TemplateExpressionEnd
        ) {
            depth = depth.saturating_sub(1);
        }
        depths.push(depth);
        if matches!(
            token.kind,
            TokenKind::Punctuator('(' | '[' | '{') | TokenKind::TemplateExpressionStart
        ) {
            depth += 1;
        }
    }
    depths
}

fn scan_error(error: ShadowError) -> ScriptError {
    match error {
        ShadowError::BadExpression { detail } => ScriptError::Invalid {
            feature: "script.scan",
            reason: detail,
        },
        other => ScriptError::Invalid {
            feature: "script.scan",
            reason: other.to_string(),
        },
    }
}

pub(crate) use super::members::{
    Member, MemberKind, doc_before, member_last, object_bounds, scan_members, unquote,
};

impl super::members::Members for Script<'_> {
    fn kind_at(&self, index: usize) -> Option<TokenKind> {
        self.kind(index)
    }

    fn span_at(&self, index: usize) -> Option<(usize, usize)> {
        Some((self.token_start(index)?, self.token_end(index)?))
    }

    fn text_at(&self, index: usize) -> Option<&str> {
        self.token_text(index)
    }

    fn close_at(&self, index: usize) -> Option<usize> {
        self.close_of(index).ok()
    }

    fn punct_at(&self, index: usize, character: char) -> bool {
        self.is_punct(index, character)
    }

    fn ident_at(&self, index: usize, name: &str) -> bool {
        self.is_ident(index, name)
    }
}

/// Renders a one-line member signature.
///
/// A method ends before its body; an arrow/function property ends before its
/// body and gets a trailing `...`, mirroring the TypeScript event signature
/// snippet. Whitespace is collapsed but tokens are not otherwise rewritten.
pub(crate) fn signature_of(script: &Script<'_>, member: &Member) -> String {
    let text = match member.kind {
        MemberKind::Method | MemberKind::Accessor => {
            let mut end = member.value_end;
            let mut cursor = member.value_start;
            while cursor < member.value_end && !script.is_punct(cursor, '(') {
                cursor += 1;
            }
            if cursor < member.value_end
                && let Ok(close) = script.close_of(cursor)
            {
                // Methods may carry a return type between `)` and `{`.
                let mut after = close + 1;
                while after < member.value_end {
                    if script.is_punct(after, '{') {
                        end = after;
                        break;
                    }
                    if script.is_punct(after, ';') {
                        break;
                    }
                    after += 1;
                }
            }
            script
                .token_range_text_between(member.key_start, end)
                .unwrap_or("")
                .to_owned()
        }
        MemberKind::Property => {
            let mut end = member.value_end;
            let mut arrow = false;
            let mut cursor = member.value_start;
            while cursor < member.value_end {
                if script.is_punct(cursor, '=') && script.is_punct(cursor + 1, '>') {
                    end = cursor + 2;
                    arrow = true;
                    break;
                }
                if script.is_punct(cursor, '{') {
                    end = cursor;
                    break;
                }
                cursor += 1;
            }
            let base = script
                .token_range_text_between(member.key_start, end)
                .unwrap_or("");
            if arrow || end < member.value_end {
                format!("{base}...")
            } else {
                base.to_owned()
            }
        }
        _ => script
            .token_range_text_between(member.key_start, member.value_end)
            .unwrap_or("")
            .to_owned(),
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl<'a> Script<'a> {
    fn token_range_text_between(&self, start_byte: usize, end_token: usize) -> Option<&'a str> {
        let end = self.token_start(end_token)?;
        self.text.get(start_byte..end)
    }
}
