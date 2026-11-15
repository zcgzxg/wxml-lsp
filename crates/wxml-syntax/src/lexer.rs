//! Tolerant lexer for WXML markup.
//!
//! The lexer never fails: malformed input becomes a token that the parser can
//! report and recover from. Each token covers the exact bytes of one meaningful
//! source fragment; whitespace between tokens is not tokenized but stays in the
//! source text and inside parent node ranges, so the syntax tree remains
//! lossless.
//!
//! Two contexts share one token stream:
//!
//! - **text** — everything outside tags. `{{ ... }}` and `<!-- ... -->` are
//!   recognised here; a lone `<` that cannot start a tag stays text.
//! - **tag** — names, `=`, quoted or unquoted values and the tag delimiters.
//!   Mustaches are allowed inside attribute values.
//! - **raw text** — the body of a lowercase `<wxs>` element. It is emitted as a
//!   single [`TokenKind::RawText`] token so `<`, `{{` and quotes inside the
//!   script have no markup meaning. The closing tag is the first `</wxs>`
//!   (optionally `</wxs />`), matching HTML raw-text behavior; `<WXS>` is an
//!   ordinary element name and is case-sensitive.

use wxml_text::TextRange;

/// A lexical token with its exact source range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    kind: TokenKind,
    range: TextRange,
}

impl Token {
    pub(crate) const fn new(kind: TokenKind, range: TextRange) -> Self {
        Self { kind, range }
    }

    /// Returns the token kind.
    #[must_use]
    pub const fn kind(self) -> TokenKind {
        self.kind
    }

    /// Returns the source range covered by the token.
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }
}

/// Token categories produced by the lexer.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    /// Raw text outside tags.
    Text,
    /// A complete `<!-- ... -->` comment.
    Comment,
    /// A comment that reached the end of the input without `-->`.
    UnterminatedComment,
    /// A complete `{{ ... }}` interpolation.
    Mustache,
    /// A `{{` that reached the end of the input without `}}`.
    UnterminatedMustache,
    /// The `<` that starts an opening tag.
    TagOpen,
    /// The `</` that starts a closing tag.
    TagClose,
    /// The `>` that ends a tag.
    TagEnd,
    /// The `/>` that ends a self-closing tag.
    TagSelfClose,
    /// The `=` between an attribute name and its value.
    Equals,
    /// A tag or attribute name.
    Name,
    /// A single quote or double quote delimiting an attribute value.
    Quote(char),
    /// Raw text inside a quoted attribute value.
    ValueText,
    /// An unquoted attribute value.
    BareValue,
    /// Raw text inside a `wxs` element.
    ///
    /// The body is not tokenized as markup: `<`, `{{` and quotes have no
    /// meaning until the closing `</wxs>` tag. One token covers the whole body
    /// and counts as one markup token regardless of its length.
    RawText,
    /// A `/` inside a tag that does not close it.
    Slash,
    /// A character that has no meaning in the current context.
    Unknown,
    /// End of the token stream; its range is empty and located at the end of
    /// the input or at the truncation point.
    Eof,
}

/// Splits WXML source into tokens, always ending with [`TokenKind::Eof`].
///
/// This entry point does not cap the token count; it allocates in proportion to
/// the input. The parser uses the bounded variant instead. Ranges assume the
/// input fits in the 4 GiB limit of [`TextRange`]; offsets beyond it saturate at
/// `u32::MAX` rather than wrapping.
#[cfg(test)]
#[must_use]
fn tokenize(text: &str) -> Vec<Token> {
    tokenize_bounded(text, usize::MAX).0
}

/// Splits WXML source into at most `limit` tokens plus an end marker.
///
/// Returns the tokens and whether the input was truncated. When truncated, the
/// final [`TokenKind::Eof`] is placed at the truncation point instead of the
/// end of the text, so the parser stops there.
pub(crate) fn tokenize_bounded(text: &str, limit: usize) -> (Vec<Token>, bool) {
    let mut lexer = Lexer {
        text,
        offset: 0,
        mode: Mode::Text,
        tokens: Vec::new(),
        truncated: false,
        tag: TagState {
            name_seen: false,
            is_wxs: false,
            closing: false,
        },
    };
    while lexer.offset < text.len() {
        if lexer.tokens.len() >= limit {
            lexer.truncated = true;
            break;
        }
        lexer.next_token();
    }
    let end = absolute_offset(lexer.offset);
    lexer.tokens.push(Token {
        kind: TokenKind::Eof,
        range: crate::error::span(end, end),
    });
    (lexer.tokens, lexer.truncated)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Text,
    Tag,
    BeforeValue,
    Quoted(char),
    RawText,
    RawClose,
}

struct Lexer<'a> {
    text: &'a str,
    offset: usize,
    mode: Mode,
    tokens: Vec<Token>,
    truncated: bool,
    tag: TagState,
}

/// Per-tag state used to recognize a lowercase `<wxs>` opening tag.
#[derive(Clone, Copy)]
struct TagState {
    name_seen: bool,
    is_wxs: bool,
    closing: bool,
}

impl<'a> Lexer<'a> {
    fn remaining(&self) -> &'a str {
        self.text.get(self.offset..).unwrap_or_default()
    }

    fn peek_char(&self) -> Option<(char, usize)> {
        let mut chars = self.remaining().chars();
        let c = chars.next()?;
        Some((c, c.len_utf8()))
    }

    fn char_at(&self, offset: usize) -> Option<(char, usize)> {
        let mut chars = self.text.get(offset..)?.chars();
        let c = chars.next()?;
        Some((c, c.len_utf8()))
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            range: crate::error::span(absolute_offset(start), absolute_offset(self.offset)),
        });
    }

    fn is_tag_start(&self, offset: usize) -> bool {
        self.char_at(offset).is_some_and(|(c, _)| is_name_start(c))
    }

    fn next_token(&mut self) {
        match self.mode {
            Mode::Text => self.next_text(),
            Mode::Tag => self.next_tag(),
            Mode::BeforeValue => self.next_before_value(),
            Mode::Quoted(quote) => self.next_quoted(quote),
            Mode::RawText => self.next_raw_text(),
            Mode::RawClose => self.next_raw_close(),
        }
    }

    fn next_text(&mut self) {
        let start = self.offset;
        if self.remaining().starts_with("<!--") {
            self.scan_comment();
            return;
        }
        if self.remaining().starts_with("{{") {
            self.scan_mustache();
            return;
        }
        if self.remaining().starts_with("</") && self.is_tag_start(self.offset + 2) {
            self.offset += 2;
            self.push(TokenKind::TagClose, start);
            self.mode = Mode::Tag;
            self.reset_tag();
            self.tag.closing = true;
            return;
        }
        if self.remaining().starts_with('<') && self.is_tag_start(self.offset + 1) {
            self.offset += 1;
            self.push(TokenKind::TagOpen, start);
            self.mode = Mode::Tag;
            self.reset_tag();
            return;
        }
        if self.remaining().starts_with('<') {
            // A lone `<` in text, such as `a < b`.
            self.consume_char();
            self.push(TokenKind::Text, start);
            return;
        }
        while self.offset < self.text.len() {
            if self.remaining().starts_with("{{") || self.remaining().starts_with('<') {
                break;
            }
            self.consume_char();
        }
        if self.offset == start {
            // The loop cannot stall, but consume one character if it ever did.
            self.consume_char();
        }
        self.push(TokenKind::Text, start);
    }

    fn scan_comment(&mut self) {
        let start = self.offset;
        if let Some(relative) = self.remaining().find("-->") {
            self.offset += relative + 3;
            self.push(TokenKind::Comment, start);
        } else {
            self.offset = self.text.len();
            self.push(TokenKind::UnterminatedComment, start);
        }
    }

    fn scan_mustache(&mut self) {
        let start = self.offset;
        if let Some(end) = self.mustache_end(start) {
            self.offset = end;
            self.push(TokenKind::Mustache, start);
        } else {
            self.offset = self.text.len();
            self.push(TokenKind::UnterminatedMustache, start);
        }
    }

    /// Returns the offset just after the closing `}}`, or `None` when the
    /// interpolation is not closed. Brackets and strings are tracked so that
    /// `{{ "}}" }}` and `{{ {a: 1} }}` close at the right place.
    fn mustache_end(&self, start: usize) -> Option<usize> {
        let mut offset = start.checked_add(2)?;
        let mut parens = 0u32;
        let mut brackets = 0u32;
        let mut braces = 0u32;
        while offset < self.text.len() {
            let (c, len) = self.char_at(offset)?;
            match c {
                '"' | '\'' | '`' => {
                    offset = self.string_end(offset, c)?;
                }
                '(' => {
                    parens = parens.saturating_add(1);
                    offset += len;
                }
                ')' => {
                    parens = parens.saturating_sub(1);
                    offset += len;
                }
                '[' => {
                    brackets = brackets.saturating_add(1);
                    offset += len;
                }
                ']' => {
                    brackets = brackets.saturating_sub(1);
                    offset += len;
                }
                '{' => {
                    braces = braces.saturating_add(1);
                    offset += len;
                }
                '}' => {
                    if braces > 0 {
                        braces -= 1;
                        offset += len;
                    } else if self
                        .text
                        .get(offset..)
                        .is_some_and(|rest| rest.starts_with("}}"))
                    {
                        return Some(offset + 2);
                    } else {
                        offset += len;
                    }
                }
                _ => offset += len,
            }
        }
        None
    }

    /// Skips a quoted string inside a mustache, honouring backslash escapes.
    /// Returns `None` when the string is not terminated.
    fn string_end(&self, start: usize, quote: char) -> Option<usize> {
        let mut offset = start + quote.len_utf8();
        while offset < self.text.len() {
            let (c, len) = self.char_at(offset)?;
            if c == '\\' {
                offset += len;
                offset += self
                    .char_at(offset)
                    .map_or(0, |(_, escaped_len)| escaped_len);
                continue;
            }
            offset += len;
            if c == quote {
                return Some(offset);
            }
        }
        None
    }

    fn next_tag(&mut self) {
        self.skip_whitespace();
        let Some((c, len)) = self.peek_char() else {
            return;
        };
        let start = self.offset;
        match c {
            '>' => {
                self.offset += len;
                self.push(TokenKind::TagEnd, start);
                self.ends_tag(false);
            }
            '/' if self.remaining().starts_with("/>") => {
                self.offset += 2;
                self.push(TokenKind::TagSelfClose, start);
                self.ends_tag(true);
            }
            '/' => {
                self.offset += len;
                self.push(TokenKind::Slash, start);
            }
            '=' => {
                self.offset += len;
                self.push(TokenKind::Equals, start);
                self.mode = Mode::BeforeValue;
            }
            '{' if self.remaining().starts_with("{{") => {
                self.scan_mustache();
            }
            c if is_name_char(c) => {
                while let Some((c, len)) = self.peek_char() {
                    if !is_name_char(c) {
                        break;
                    }
                    self.offset += len;
                }
                self.push(TokenKind::Name, start);
                if !self.tag.name_seen {
                    self.tag.name_seen = true;
                    self.tag.is_wxs = self.text.get(start..self.offset) == Some("wxs");
                }
            }
            _ => {
                self.offset += len;
                self.push(TokenKind::Unknown, start);
            }
        }
    }

    fn next_before_value(&mut self) {
        self.skip_whitespace();
        let Some((c, len)) = self.peek_char() else {
            return;
        };
        let start = self.offset;
        match c {
            '>' => {
                self.offset += len;
                self.push(TokenKind::TagEnd, start);
                self.ends_tag(false);
            }
            '/' if self.remaining().starts_with("/>") => {
                self.offset += 2;
                self.push(TokenKind::TagSelfClose, start);
                self.ends_tag(true);
            }
            '"' | '\'' => {
                self.offset += len;
                self.push(TokenKind::Quote(c), start);
                self.mode = Mode::Quoted(c);
            }
            '{' if self.remaining().starts_with("{{") => {
                self.scan_mustache();
                self.mode = Mode::Tag;
            }
            _ => {
                self.scan_bare_value();
                self.push(TokenKind::BareValue, start);
                self.mode = Mode::Tag;
            }
        }
    }

    /// Resets per-tag name tracking after the tag has ended.
    const fn reset_tag(&mut self) {
        self.tag = TagState {
            name_seen: false,
            is_wxs: false,
            closing: false,
        };
    }

    /// Handles a tag terminator and switches to raw-text mode for `<wxs>`.
    const fn ends_tag(&mut self, self_closing: bool) {
        let starts_raw = !self_closing && self.tag.is_wxs && !self.tag.closing;
        self.reset_tag();
        self.mode = if starts_raw {
            Mode::RawText
        } else {
            Mode::Text
        };
    }

    /// Emits the raw body of a `wxs` element as one token.
    fn next_raw_text(&mut self) {
        let start = self.offset;
        if let Some((close_start, _)) = self.raw_close(start) {
            self.offset = close_start;
            self.push(TokenKind::RawText, start);
            self.mode = Mode::RawClose;
        } else {
            self.offset = self.text.len();
            self.push(TokenKind::RawText, start);
            self.mode = Mode::Text;
        }
    }

    /// Emits `</` for the `wxs` closing tag detected by [`Self::raw_close`].
    fn next_raw_close(&mut self) {
        let start = self.offset;
        if self
            .text
            .get(start..)
            .is_some_and(|rest| rest.starts_with("</"))
        {
            self.offset += 2;
            self.push(TokenKind::TagClose, start);
            self.mode = Mode::Tag;
            self.reset_tag();
            self.tag.closing = true;
        } else {
            self.mode = Mode::Text;
            self.consume_char();
            self.push(TokenKind::Unknown, start);
        }
    }

    /// Finds the first HTML-style raw-text closing tag, `</wxs>`, with optional
    /// whitespace and an optional `/` before `>`.
    fn raw_close(&self, from: usize) -> Option<(usize, usize)> {
        let mut search = from;
        while let Some(relative) = self.text.get(search..)?.find("</wxs") {
            let close_start = search + relative;
            let after_name = close_start + 5;
            let mut offset = after_name;
            while let Some((c, len)) = self.char_at(offset) {
                if !c.is_whitespace() {
                    break;
                }
                offset += len;
            }
            if self
                .text
                .get(offset..)
                .is_some_and(|rest| rest.starts_with('/'))
            {
                offset += 1;
                while let Some((c, len)) = self.char_at(offset) {
                    if !c.is_whitespace() {
                        break;
                    }
                    offset += len;
                }
            }
            if self
                .text
                .get(offset..)
                .is_some_and(|rest| rest.starts_with('>'))
            {
                return Some((close_start, offset + 1));
            }
            search = after_name;
        }
        None
    }

    fn scan_bare_value(&mut self) {
        let start = self.offset;
        while let Some((c, len)) = self.peek_char() {
            if c.is_whitespace() || c == '>' {
                break;
            }
            if c == '/' && self.remaining().starts_with("/>") {
                break;
            }
            self.offset += len;
        }
        if self.offset == start {
            self.consume_char();
        }
    }

    fn next_quoted(&mut self, quote: char) {
        if self.remaining().starts_with("{{") {
            self.scan_mustache();
            return;
        }
        let start = self.offset;
        while let Some((c, len)) = self.peek_char() {
            if c == quote || self.remaining().starts_with("{{") {
                break;
            }
            self.offset += len;
        }
        if self.offset == start {
            // Close the quote, or consume one character if the loop stalled.
            if self.peek_char().is_some_and(|(c, _)| c == quote) {
                self.consume_char();
                self.push(TokenKind::Quote(quote), start);
                self.mode = Mode::Tag;
            } else {
                self.consume_char();
                self.push(TokenKind::ValueText, start);
            }
            return;
        }
        self.push(TokenKind::ValueText, start);
    }

    fn skip_whitespace(&mut self) {
        while let Some((c, len)) = self.peek_char() {
            if !c.is_whitespace() {
                break;
            }
            self.offset += len;
        }
    }

    fn consume_char(&mut self) {
        if let Some((_, len)) = self.peek_char() {
            self.offset += len;
        }
    }
}

fn absolute_offset(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

const fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

const fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source).into_iter().map(Token::kind).collect()
    }

    fn token_text<'a>(source: &'a str, token: &Token) -> &'a str {
        &source[token.range().start() as usize..token.range().end() as usize]
    }

    #[test]
    fn empty_input_has_only_eof() {
        let tokens = tokenize("");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind(), TokenKind::Eof);
        assert!(tokens[0].range().is_empty());
        assert_eq!(tokens[0].range().start(), 0);
    }

    #[test]
    fn tokenizes_text() {
        let tokens = tokenize("hello world");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].kind(), TokenKind::Text);
        assert_eq!(token_text("hello world", &tokens[0]), "hello world");
    }

    #[test]
    fn tokenizes_an_element_with_attributes() {
        let source = r#"<view class="a" wx:if="{{x}}">"#;
        let tokens = tokenize(source);
        assert_eq!(
            tokens.iter().map(|token| token.kind()).collect::<Vec<_>>(),
            vec![
                TokenKind::TagOpen,
                TokenKind::Name,
                TokenKind::Name,
                TokenKind::Equals,
                TokenKind::Quote('"'),
                TokenKind::ValueText,
                TokenKind::Quote('"'),
                TokenKind::Name,
                TokenKind::Equals,
                TokenKind::Quote('"'),
                TokenKind::Mustache,
                TokenKind::Quote('"'),
                TokenKind::TagEnd,
                TokenKind::Eof,
            ]
        );
        assert_eq!(token_text(source, &tokens[1]), "view");
        assert_eq!(token_text(source, &tokens[2]), "class");
        assert_eq!(token_text(source, &tokens[10]), "{{x}}");
    }

    #[test]
    fn tokenizes_closing_and_self_closing_tags() {
        assert_eq!(
            kinds("</view>"),
            vec![
                TokenKind::TagClose,
                TokenKind::Name,
                TokenKind::TagEnd,
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("<input/>"),
            vec![
                TokenKind::TagOpen,
                TokenKind::Name,
                TokenKind::TagSelfClose,
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("<input />"),
            vec![
                TokenKind::TagOpen,
                TokenKind::Name,
                TokenKind::TagSelfClose,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn tokenizes_names_with_namespaces_and_dashes() {
        let source = "<my-component bind:tap=\"go\" data-id=\"1\">";
        let names: Vec<String> = tokenize(source)
            .iter()
            .filter(|token| token.kind() == TokenKind::Name)
            .map(|token| token_text(source, token).to_owned())
            .collect();
        assert_eq!(names, vec!["my-component", "bind:tap", "data-id"]);
    }

    #[test]
    fn tokenizes_comments() {
        assert_eq!(
            kinds("a<!-- note -->b"),
            vec![
                TokenKind::Text,
                TokenKind::Comment,
                TokenKind::Text,
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("<!-- note"),
            vec![TokenKind::UnterminatedComment, TokenKind::Eof]
        );
    }

    #[test]
    fn mustache_ends_at_the_matching_double_brace() {
        let source = r#"{{ a ? "}}" : b }}"#;
        let tokens = tokenize(source);
        assert_eq!(tokens[0].kind(), TokenKind::Mustache);
        assert_eq!(token_text(source, &tokens[0]), source);
    }

    #[test]
    fn mustache_tracks_nested_brackets_and_braces() {
        let source = "{{ {a: {b: 1}} }}";
        let tokens = tokenize(source);
        assert_eq!(tokens[0].kind(), TokenKind::Mustache);
        assert_eq!(token_text(source, &tokens[0]), source);

        let source = "{{ (a + b)[0] }}";
        let tokens = tokenize(source);
        assert_eq!(tokens[0].kind(), TokenKind::Mustache);
        assert_eq!(token_text(source, &tokens[0]), source);
    }

    #[test]
    fn unclosed_mustache_runs_to_the_end() {
        let source = "a{{ b";
        let tokens = tokenize(source);
        assert_eq!(tokens[0].kind(), TokenKind::Text);
        assert_eq!(tokens[1].kind(), TokenKind::UnterminatedMustache);
        assert_eq!(token_text(source, &tokens[1]), "{{ b");
    }

    #[test]
    fn preserves_crlf_in_text_and_values() {
        let source = "a\r\nb";
        let tokens = tokenize(source);
        assert_eq!(tokens[0].kind(), TokenKind::Text);
        assert_eq!(token_text(source, &tokens[0]), "a\r\nb");

        let source = "<view a=\"x\r\ny\">";
        let tokens = tokenize(source);
        let value = tokens
            .iter()
            .find(|token| token.kind() == TokenKind::ValueText)
            .expect("value token");
        assert_eq!(token_text(source, value), "x\r\ny");
    }

    #[test]
    fn keeps_lone_angle_brackets_as_text() {
        let source = "a < b > c";
        let tokens = tokenize(source);
        assert_eq!(
            tokens.last().copied().map(Token::kind),
            Some(TokenKind::Eof)
        );
        let text: String = tokens
            .iter()
            .filter(|token| token.kind() == TokenKind::Text)
            .map(|token| token_text(source, token))
            .collect();
        assert_eq!(text, source);
        assert!(
            !tokens
                .iter()
                .any(|token| token.kind() == TokenKind::TagOpen)
        );
    }

    #[test]
    fn tokenizes_unquoted_values() {
        let source = "<view a=1 b={{x}} c=d/>";
        assert_eq!(
            kinds(source),
            vec![
                TokenKind::TagOpen,
                TokenKind::Name,
                TokenKind::Name,
                TokenKind::Equals,
                TokenKind::BareValue,
                TokenKind::Name,
                TokenKind::Equals,
                TokenKind::Mustache,
                TokenKind::Name,
                TokenKind::Equals,
                TokenKind::BareValue,
                TokenKind::TagSelfClose,
                TokenKind::Eof,
            ]
        );
        let tokens = tokenize(source);
        let values: Vec<&str> = tokens
            .iter()
            .filter(|token| token.kind() == TokenKind::BareValue)
            .map(|token| token_text(source, token))
            .collect();
        assert_eq!(values, vec!["1", "d"]);
    }

    #[test]
    fn tokenizes_single_quoted_values() {
        let source = "<view a='x y'>";
        let tokens = tokenize(source);
        assert!(
            tokens
                .iter()
                .any(|token| token.kind() == TokenKind::Quote('\''))
        );
        let value = tokens
            .iter()
            .find(|token| token.kind() == TokenKind::ValueText)
            .expect("value token");
        assert_eq!(token_text(source, value), "x y");
    }

    #[test]
    fn reports_unknown_tag_characters_without_stopping() {
        let source = "<view @x a=1>";
        let tokens = tokenize(source);
        assert!(
            tokens
                .iter()
                .any(|token| token.kind() == TokenKind::Unknown)
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.kind() == TokenKind::BareValue)
        );
        assert_eq!(
            tokens.last().copied().map(Token::kind),
            Some(TokenKind::Eof)
        );
    }

    #[test]
    fn token_ranges_stay_inside_the_input_and_on_char_boundaries() {
        let source = "a\r\n<view 中文=\"值\">{{用户.名字}}</view>";
        let tokens = tokenize(source);
        for token in &tokens {
            let range = token.range();
            assert!(range.end() as usize <= source.len());
            assert!(source.is_char_boundary(range.start() as usize));
            assert!(source.is_char_boundary(range.end() as usize));
        }
    }

    #[test]
    fn text_and_mustache_cover_the_whole_input() {
        let source = "before {{ a.b }} after";
        let tokens = tokenize(source);
        let covered: String = tokens
            .iter()
            .filter(|token| token.kind() != TokenKind::Eof)
            .map(|token| token_text(source, token))
            .collect();
        assert_eq!(covered, source);
    }

    #[test]
    fn tokenizes_wxs_bodies_as_single_raw_text_tokens() {
        let source = r#"<wxs module="m">if (a<b) { x = "}}" }</wxs>"#;
        let tokens = tokenize(source);
        assert_eq!(
            tokens.iter().map(|token| token.kind()).collect::<Vec<_>>(),
            vec![
                TokenKind::TagOpen,
                TokenKind::Name,
                TokenKind::Name,
                TokenKind::Equals,
                TokenKind::Quote('"'),
                TokenKind::ValueText,
                TokenKind::Quote('"'),
                TokenKind::TagEnd,
                TokenKind::RawText,
                TokenKind::TagClose,
                TokenKind::Name,
                TokenKind::TagEnd,
                TokenKind::Eof,
            ]
        );
        let raw = tokens
            .iter()
            .find(|token| token.kind() == TokenKind::RawText)
            .expect("raw text token");
        assert_eq!(token_text(source, raw), r#"if (a<b) { x = "}}" }"#);
    }

    #[test]
    fn wxs_body_does_not_emit_markup_tokens() {
        let source = "<wxs><view>{{x}}</view></wxs>";
        let tokens = tokenize(source);
        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.kind() == TokenKind::RawText)
                .count(),
            1
        );
        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.kind() == TokenKind::TagOpen)
                .count(),
            1
        );
        assert!(
            !tokens
                .iter()
                .any(|token| token.kind() == TokenKind::Mustache)
        );
    }

    #[test]
    fn self_closing_wxs_is_not_raw_text() {
        assert!(!kinds("<wxs/>").contains(&TokenKind::RawText));
        assert!(!kinds("<wxs />").contains(&TokenKind::RawText));
        assert!(!kinds("<WXS>x</WXS>").contains(&TokenKind::RawText));
    }

    #[test]
    fn unclosed_wxs_raw_text_runs_to_the_end() {
        let source = "<wxs>if (a<b) { x = 1 }";
        let tokens = tokenize(source);
        let raw = tokens
            .iter()
            .find(|token| token.kind() == TokenKind::RawText)
            .expect("raw text token");
        assert_eq!(token_text(source, raw), "if (a<b) { x = 1 }");
        assert_eq!(
            tokens.last().copied().map(Token::kind),
            Some(TokenKind::Eof)
        );
        assert!(
            !tokens
                .iter()
                .any(|token| token.kind() == TokenKind::TagClose)
        );
    }

    #[test]
    fn wxs_closing_tag_allows_whitespace_and_slash() {
        for source in ["<wxs>x</wxs >", "<wxs>x</wxs/>", "<wxs>x</wxs />"] {
            let tokens = tokenize(source);
            assert!(
                tokens
                    .iter()
                    .any(|token| token.kind() == TokenKind::RawText),
                "source {source:?}"
            );
            assert!(
                tokens.iter().any(|token| matches!(
                    token.kind(),
                    TokenKind::TagEnd | TokenKind::TagSelfClose
                )),
                "source {source:?}"
            );
            assert_eq!(
                tokens.last().copied().map(Token::kind),
                Some(TokenKind::Eof)
            );
        }
    }
}
