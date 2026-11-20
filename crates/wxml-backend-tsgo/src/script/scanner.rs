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

/// Bound for a `JSDoc` lookback; comments directly before a member are short,
/// and this prevents O(prefix) rescans for thousands of members.
const MAX_DOC_LOOKBACK_BYTES: usize = 4096;
/// Maximum signature length in characters.
pub(crate) const MAX_SIGNATURE_CHARS: usize = 512;

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

/// Kind of one first-level object member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberKind {
    /// `name: value` assignment.
    Property,
    /// `name() {}`, `async name() {}` or `*name() {}`.
    Method,
    /// `name` shorthand.
    Shorthand,
    /// `...value` spread.
    Spread,
    /// `[expression]: value` computed member.
    Computed,
    /// `get name()` / `set name(value)`.
    Accessor,
    /// Any shape this scanner does not classify.
    Unknown,
}

/// One first-level object-literal member.
#[derive(Debug, Clone)]
pub(crate) struct Member {
    /// Unquoted member name when it is a static identifier/string key.
    pub(crate) name: Option<String>,
    /// Member shape.
    pub(crate) kind: MemberKind,
    /// Inclusive UTF-8 byte start of the key token.
    pub(crate) key_start: usize,
    /// Exclusive UTF-8 byte end of the key token.
    pub(crate) key_end: usize,
    /// Token index where the value starts.
    pub(crate) value_start: usize,
    /// Token index where the member ends.
    pub(crate) value_end: usize,
}

impl Member {
    /// Returns the exact original key token range.
    pub(crate) fn key_range(&self) -> SourceRange {
        to_range(self.key_start, self.key_end)
    }

    /// Returns the exact original value token range.
    pub(crate) fn value_range(&self, script: &Script<'_>) -> Option<SourceRange> {
        let start = script.token_start(self.value_start)?;
        let end = script.token_start(self.value_end)?;
        Some(to_range(start, end))
    }
}

fn to_range(start: usize, end: usize) -> SourceRange {
    SourceRange::new(
        u32::try_from(start).unwrap_or(u32::MAX),
        u32::try_from(end).unwrap_or(u32::MAX),
    )
}

/// Scans first-level members of an object literal body (`start..end`).
pub(crate) fn scan_members(script: &Script<'_>, start: usize, end: usize) -> Vec<Member> {
    let mut members = Vec::new();
    let mut index = start;
    while index < end {
        while index < end && script.is_punct(index, ',') {
            index += 1;
        }
        if index >= end {
            break;
        }
        let segment_end = member_end(script, index, end);
        if segment_end <= index {
            break;
        }
        members.push(parse_member(script, index, segment_end));
        index = segment_end;
    }
    members
}

fn member_end(script: &Script<'_>, start: usize, end: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < end {
        match script.kind(index) {
            Some(TokenKind::TemplateExpressionStart | TokenKind::Punctuator('(' | '[' | '{')) => {
                depth += 1;
            }
            Some(TokenKind::TemplateExpressionEnd | TokenKind::Punctuator(')' | ']' | '}')) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Some(TokenKind::Punctuator(',')) if depth == 0 => break,
            _ => {}
        }
        index += 1;
    }
    index
}

fn parse_member(script: &Script<'_>, start: usize, end: usize) -> Member {
    let unknown = Member {
        name: None,
        kind: MemberKind::Unknown,
        key_start: script.token_start(start).unwrap_or(0),
        key_end: script.token_end(start).unwrap_or(0),
        value_start: start,
        value_end: end,
    };
    if start >= end {
        return unknown;
    }
    if script.is_punct(start, '.')
        && script.is_punct(start + 1, '.')
        && script.is_punct(start + 2, '.')
    {
        return Member {
            kind: MemberKind::Spread,
            value_start: start + 3,
            ..unknown
        };
    }
    if script.is_punct(start, '[') {
        return Member {
            kind: MemberKind::Computed,
            ..unknown
        };
    }
    if script.is_punct(start, '*') {
        return parse_named_member(script, start + 1, end, MemberKind::Method).unwrap_or(unknown);
    }
    if script.is_ident(start, "async") {
        return parse_named_member(script, start + 1, end, MemberKind::Method).unwrap_or(unknown);
    }
    if script.is_ident(start, "get") || script.is_ident(start, "set") {
        if let Some(next) = parse_accessor(script, start + 1, end) {
            return next;
        }
    }
    parse_named_member(script, start, end, MemberKind::Unknown).unwrap_or(unknown)
}

fn parse_accessor(script: &Script<'_>, name_index: usize, end: usize) -> Option<Member> {
    let member = parse_named_member(script, name_index, end, MemberKind::Accessor)?;
    if member.kind == MemberKind::Accessor {
        Some(member)
    } else {
        None
    }
}

fn parse_named_member(
    script: &Script<'_>,
    start: usize,
    end: usize,
    fallback: MemberKind,
) -> Option<Member> {
    if start >= end {
        return None;
    }
    let token = script.tokens.get(start)?;
    if !matches!(
        token.kind,
        TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number
    ) {
        return None;
    }
    let name = match token.kind {
        TokenKind::StringLiteral => unquote(token.text(script.text)),
        TokenKind::Identifier => Some(token.text(script.text).to_owned()),
        _ => None,
    };
    let key_end = token.end;
    let second = start + 1;
    if second >= end {
        return Some(Member {
            name,
            kind: MemberKind::Shorthand,
            key_start: token.start,
            key_end,
            value_start: start,
            value_end: end,
        });
    }
    let kind = match script.kind(second) {
        Some(TokenKind::Punctuator(':')) => MemberKind::Property,
        Some(TokenKind::Punctuator('(')) if fallback == MemberKind::Accessor => {
            MemberKind::Accessor
        }
        Some(TokenKind::Punctuator('(')) => MemberKind::Method,
        _ if fallback == MemberKind::Method => MemberKind::Method,
        _ if fallback == MemberKind::Accessor => MemberKind::Accessor,
        _ => MemberKind::Unknown,
    };
    Some(Member {
        name,
        kind,
        key_start: token.start,
        key_end,
        value_start: if kind == MemberKind::Property {
            second + 1
        } else {
            second
        },
        value_end: end,
    })
}

/// Returns the object bounds of a `name: { ... }` member.
///
/// The value must be exactly one object literal; a wrapped expression returns
/// `None` so callers can report the shape instead of scanning a sub-object.
pub(crate) fn object_bounds(script: &Script<'_>, member: &Member) -> Option<(usize, usize)> {
    if member.kind != MemberKind::Property || member.value_start >= member.value_end {
        return None;
    }
    if !script.is_punct(member.value_start, '{') {
        return None;
    }
    let close = script.close_of(member.value_start).ok()?;
    if close + 1 != member.value_end {
        return None;
    }
    Some((member.value_start, close))
}

/// Returns the last member with `name`.
pub(crate) fn member_last<'a>(members: &'a [Member], name: &str) -> Option<&'a Member> {
    members
        .iter()
        .rev()
        .find(|member| member.name.as_deref() == Some(name))
}

/// Returns the `JSDoc` comment directly before `key_start`.
pub(crate) fn doc_before(text: &str, key_start: usize) -> String {
    let key_start = key_start.min(text.len());
    let window_start = key_start.saturating_sub(MAX_DOC_LOOKBACK_BYTES);
    let prefix = text.get(window_start..key_start).unwrap_or("");
    let trimmed = prefix.trim_end();
    if !trimmed.ends_with("*/") {
        return String::new();
    }
    let Some(start) = trimmed[..trimmed.len() - 2].rfind("/**") else {
        return String::new();
    };
    let comment = &trimmed[start + 3..trimmed.len() - 2];
    comment
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Unquotes a JavaScript string literal if it is a simple single-line literal.
pub(crate) fn unquote(text: &str) -> Option<String> {
    let mut characters = text.chars();
    let quote = characters.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    let mut characters = characters.peekable();
    while let Some(character) = characters.next() {
        if escaped {
            match character {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'u' => value.push(read_unicode_escape(&mut characters)?),
                'x' => value.push(read_hex_escape(&mut characters, 2)?),
                other => value.push(other),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            return Some(value);
        } else {
            value.push(character);
        }
    }
    None
}

fn read_unicode_escape(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    if characters.peek() == Some(&'{') {
        characters.next();
        let mut digits = String::new();
        while let Some(&character) = characters.peek() {
            if character == '}' {
                characters.next();
                break;
            }
            digits.push(character);
            characters.next();
        }
        if digits.is_empty() || digits.len() > 6 {
            return None;
        }
        return u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32);
    }
    read_hex_escape(characters, 4)
}

fn read_hex_escape(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    count: usize,
) -> Option<char> {
    let mut digits = String::new();
    for _ in 0..count {
        let character = characters.next()?;
        if !character.is_ascii_hexdigit() {
            return None;
        }
        digits.push(character);
    }
    u32::from_str_radix(&digits, 16)
        .ok()
        .and_then(char::from_u32)
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
            if cursor < member.value_end {
                if let Ok(close) = script.close_of(cursor) {
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
    cap_signature(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn cap_signature(text: String) -> String {
    if text.chars().count() <= MAX_SIGNATURE_CHARS {
        return text;
    }
    let mut capped = text
        .chars()
        .take(MAX_SIGNATURE_CHARS - 1)
        .collect::<String>();
    capped.truncate(capped.trim_end().len());
    capped.push('…');
    capped
}

impl<'a> Script<'a> {
    fn token_range_text_between(&self, start_byte: usize, end_token: usize) -> Option<&'a str> {
        let end = self.token_start(end_token)?;
        self.text.get(start_byte..end)
    }
}
