//! Shared first-level object-literal member grammar.
//!
//! Both the script scanner ([`super::scanner`]) and the in-place shadow builder
//! (`crate::shadow::layout`) group script tokens into members. They lex with the
//! same tokenizer, so the member grammar lives here once and the two callers
//! only provide token access through [`Members`].
#![allow(clippy::redundant_pub_crate)]

use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

const MAX_DOC_LOOKBACK_BYTES: usize = 4096;

/// Read-only token surface the member grammar needs from a lexed script.
///
/// The two script representations keep their own token storage, error type and
/// extra tables (`UTF-16` offsets, delimiter pairs); only this narrow view is
/// shared.
pub(crate) trait Members {
    /// Returns the kind of the token at `index`, `None` when out of range.
    fn kind_at(&self, index: usize) -> Option<TokenKind>;
    /// Returns the UTF-8 byte span of the token at `index`.
    fn span_at(&self, index: usize) -> Option<(usize, usize)>;
    /// Returns the source text of the token at `index`.
    fn text_at(&self, index: usize) -> Option<&str>;
    /// Returns the index of the token closing the delimiter at `index`.
    fn close_at(&self, index: usize) -> Option<usize>;
    /// Returns whether the token at `index` is the punctuator `character`.
    fn punct_at(&self, index: usize, character: char) -> bool;
    /// Returns whether the token at `index` is the identifier `name`.
    fn ident_at(&self, index: usize, name: &str) -> bool;
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
    pub(crate) fn value_range(&self, tokens: &impl Members) -> Option<SourceRange> {
        let (start, _) = tokens.span_at(self.value_start)?;
        let (end, _) = tokens.span_at(self.value_end)?;
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
pub(crate) fn scan_members(tokens: &impl Members, start: usize, end: usize) -> Vec<Member> {
    let mut members = Vec::new();
    let mut index = start;
    while index < end {
        while index < end && tokens.punct_at(index, ',') {
            index += 1;
        }
        if index >= end {
            break;
        }
        let segment_end = member_end(tokens, index, end);
        if segment_end <= index {
            break;
        }
        members.push(parse_member(tokens, index, segment_end));
        index = segment_end;
    }
    members
}

pub(crate) fn member_end(tokens: &impl Members, start: usize, end: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < end {
        match tokens.kind_at(index) {
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

fn parse_member(tokens: &impl Members, start: usize, end: usize) -> Member {
    let unknown = Member {
        name: None,
        kind: MemberKind::Unknown,
        key_start: tokens.span_at(start).map_or(0, |span| span.0),
        key_end: tokens.span_at(start).map_or(0, |span| span.1),
        value_start: start,
        value_end: end,
    };
    if start >= end {
        return unknown;
    }
    if tokens.punct_at(start, '.')
        && tokens.punct_at(start + 1, '.')
        && tokens.punct_at(start + 2, '.')
    {
        return Member {
            kind: MemberKind::Spread,
            value_start: start + 3,
            ..unknown
        };
    }
    if tokens.punct_at(start, '[') {
        return Member {
            kind: MemberKind::Computed,
            ..unknown
        };
    }
    if tokens.punct_at(start, '*') {
        return parse_named_member(tokens, start + 1, end, MemberKind::Method).unwrap_or(unknown);
    }
    if tokens.ident_at(start, "async") {
        return parse_named_member(tokens, start + 1, end, MemberKind::Method).unwrap_or(unknown);
    }
    if tokens.ident_at(start, "get") || tokens.ident_at(start, "set") {
        if let Some(next) = parse_accessor(tokens, start + 1, end) {
            return next;
        }
    }
    parse_named_member(tokens, start, end, MemberKind::Unknown).unwrap_or(unknown)
}

fn parse_accessor(tokens: &impl Members, name_index: usize, end: usize) -> Option<Member> {
    let member = parse_named_member(tokens, name_index, end, MemberKind::Accessor)?;
    if member.kind == MemberKind::Accessor {
        Some(member)
    } else {
        None
    }
}

fn parse_named_member(
    tokens: &impl Members,
    start: usize,
    end: usize,
    fallback: MemberKind,
) -> Option<Member> {
    if start >= end {
        return None;
    }
    let kind_at_start = tokens.kind_at(start)?;
    if !matches!(
        kind_at_start,
        TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number
    ) {
        return None;
    }
    let text = tokens.text_at(start)?;
    let name = match kind_at_start {
        TokenKind::StringLiteral => unquote(text),
        TokenKind::Identifier => Some(text.to_owned()),
        _ => None,
    };
    let (key_start, key_end) = tokens.span_at(start)?;
    let second = start + 1;
    if second >= end {
        return Some(Member {
            name,
            kind: MemberKind::Shorthand,
            key_start,
            key_end,
            value_start: start,
            value_end: end,
        });
    }
    let kind = match tokens.kind_at(second) {
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
        key_start,
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
pub(crate) fn object_bounds(tokens: &impl Members, member: &Member) -> Option<(usize, usize)> {
    if member.kind != MemberKind::Property || member.value_start >= member.value_end {
        return None;
    }
    if !tokens.punct_at(member.value_start, '{') {
        return None;
    }
    let close = tokens.close_at(member.value_start)?;
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

/// Returns whether `open` starts a `function` parameter list rather than a call
/// or control-flow condition.
pub(crate) fn function_keyword_before(tokens: &impl Members, open: usize) -> bool {
    let mut cursor = open;
    while cursor > 0 {
        cursor -= 1;
        if tokens.ident_at(cursor, "function") {
            return true;
        }
        if tokens.ident_at(cursor, "if")
            || tokens.ident_at(cursor, "while")
            || tokens.ident_at(cursor, "switch")
            || tokens.ident_at(cursor, "for")
            || tokens.ident_at(cursor, "with")
            || tokens.ident_at(cursor, "catch")
        {
            return false;
        }
        if open - cursor > 3 {
            break;
        }
    }
    false
}

/// Returns whether `open` is a parameter list introduced by a method name.
pub(crate) fn method_name_before(tokens: &impl Members, open: usize) -> bool {
    let Some(previous) = open.checked_sub(1) else {
        return false;
    };
    if !matches!(
        tokens.kind_at(previous),
        Some(TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number)
    ) {
        return false;
    }
    !matches!(
        tokens.text_at(previous),
        Some("if" | "while" | "switch" | "for" | "with" | "catch")
    )
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
