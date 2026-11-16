//! Bounded static summaries of WXS modules.
//!
//! WXS is a small JavaScript-like language, but treating it as a full JS/TS
//! module would mean executing or fully parsing untrusted source. This module
//! deliberately implements a **static subset**:
//!
//! - top-level `function name(...) { ... }`;
//! - `const`/`let`/`var` declarations with literal or function initializers;
//! - `module.exports = { ... }` object literals (a callable
//!   `module.exports = function` is reported as dynamic);
//! - `exports.name = ...` assignments;
//! - `require("./other.wxs")` bindings.
//!
//! Every summary is conservative: unknown initializers become `any`, function
//! return types are the union of statically visible literal returns, and
//! `JSDoc` `@param`/`@returns` annotations override the inferred text. Dynamic
//! forms (computed exports, re-assigned `module.exports` from a non-literal,
//! `eval`, `new Function`, unknown require specifiers) are reported through
//! [`WxsSummary::dynamic`] instead of being guessed. Source is never executed.
//!
//! The scanner is bounded in bytes, tokens, members and nesting and never
//! panics on malformed input; [`WxsSummary::truncated`] marks a partial result.

use wxml_text::{SourceRange, TextRange};

/// Maximum bytes scanned for one WXS module.
pub const MAX_WXS_BYTES: usize = 1024 * 1024;
/// Maximum members collected for one WXS module.
pub const MAX_WXS_MEMBERS: usize = 512;
/// Maximum lexer tokens kept for one WXS module.
pub const MAX_WXS_TOKENS: usize = 200_000;
/// Maximum nesting accepted while skipping statements.
pub const MAX_WXS_DEPTH: usize = 128;

/// The declaration shape of one WXS member.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WxsMemberKind {
    /// A function declaration, function expression or arrow function.
    Function,
    /// A value produced by a literal, identifier alias or unknown initializer.
    Value,
    /// A `const x = require("./x.wxs")` binding.
    Module,
}

impl WxsMemberKind {
    /// Stable lower-case label used by hover and completion details.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Value => "value",
            Self::Module => "module",
        }
    }
}

/// One statically declared WXS member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WxsMember {
    /// Member name as written.
    pub name: String,
    /// Declaration shape.
    pub kind: WxsMemberKind,
    /// Conservative type text, for example `string`, `number[]`, `any`.
    pub type_text: String,
    /// Function signature such as `(a: any, b: number) => string`; `None` for
    /// non-function members.
    pub signature: Option<String>,
    /// Leading `JSDoc` or line-comment text, already trimmed.
    pub doc: String,
    /// `true` when the member is reachable through `module.exports`.
    pub exported: bool,
    /// Byte range of the declared name in the WXS text.
    pub name_range: SourceRange,
    /// Byte range of the whole declaration (or exported property) in the WXS
    /// text. Definition queries return this range.
    pub decl_range: SourceRange,
    /// Specifier of a `require(...)` binding, for [`WxsMemberKind::Module`].
    pub module_specifier: Option<String>,
    /// Local declaration this exported alias points at, when the export is a
    /// shorthand/alias. Definition queries may use it, but types always come
    /// from the exported member itself.
    pub alias_of: Option<SourceRange>,
}

/// One `require("./x.wxs")` binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WxsRequire {
    /// Local binding name, empty for an unbound `require(...)` expression.
    pub binding: String,
    /// Specifier exactly as written (without quotes).
    pub specifier: String,
    /// Byte range of the binding identifier, or of the call for an unbound
    /// require.
    pub range: SourceRange,
    /// Byte range of the specifier without quotes.
    pub specifier_range: SourceRange,
}

/// Static summary of one WXS module.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WxsSummary {
    /// Members in source order. An exported alias appears once, with
    /// `exported = true`.
    pub members: Vec<WxsMember>,
    /// `require(...)` bindings in source order.
    pub requires: Vec<WxsRequire>,
    /// Set when the module contains an export form that cannot be summarized
    /// statically. The summary is then incomplete, never wrong.
    pub dynamic: Option<String>,
    /// `true` when a bound stopped the scan; `members` is incomplete.
    pub truncated: bool,
}

impl WxsSummary {
    /// Returns the member with `name`, preferring an exported declaration.
    #[must_use]
    pub fn member(&self, name: &str) -> Option<&WxsMember> {
        self.members
            .iter()
            .find(|member| member.exported && member.name == name)
            .or_else(|| self.members.iter().find(|member| member.name == name))
    }

    /// Returns exported members in source order.
    pub fn exported(&self) -> impl Iterator<Item = &WxsMember> {
        self.members.iter().filter(|member| member.exported)
    }

    /// Returns the hover text of one member without documentation.
    #[must_use]
    pub fn describe(&self, member: &WxsMember) -> String {
        match member.kind {
            WxsMemberKind::Function => member
                .signature
                .clone()
                .unwrap_or_else(|| format!("(...args: any) => {}", member.type_text)),
            WxsMemberKind::Module => format!(
                "module {}",
                member.module_specifier.as_deref().unwrap_or("<unknown>")
            ),
            WxsMemberKind::Value => member.type_text.clone(),
        }
    }
}

/// Summarizes a WXS module from its source text.
///
/// The scan is read-only and bounded. Malformed input produces a summary of
/// what could be proven plus [`WxsSummary::truncated`] or
/// [`WxsSummary::dynamic`]; it never throws.
#[must_use]
pub fn summarize_wxs(text: &str) -> WxsSummary {
    let limit = MAX_WXS_BYTES.min(text.len());
    let end = (0..=limit)
        .rev()
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(0);
    let sliced = text.get(..end).unwrap_or_default();
    let (tokens, token_truncated) = lex(sliced);
    // The lexer stops before adding a token past the cap; an input that ends
    // exactly at the cap is complete.
    let truncated = end < text.len() || token_truncated;
    let mut parser = Parser {
        tokens: &tokens,
        summary: WxsSummary::default(),
        last_doc: String::new(),
        local_by_name: Vec::new(),
    };
    parser.run();
    if truncated {
        parser.summary.truncated = true;
    }
    parser.summary
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenKind {
    Identifier,
    Number,
    String,
    Punct(u8),
    Comment,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Token<'a> {
    kind: TokenKind,
    text: &'a str,
    start: usize,
    end: usize,
    /// First non-whitespace token on its line.
    starts_line: bool,
    /// Block comment that starts with `/**`.
    doc_comment: bool,
}

fn lex(text: &str) -> (Vec<Token<'_>>, bool) {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0_usize;
    let mut at_line_start = true;
    let mut truncated = false;
    while index < bytes.len() {
        if tokens.len() >= MAX_WXS_TOKENS {
            truncated = true;
            break;
        }
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            if byte == b'\n' {
                at_line_start = true;
            }
            index += 1;
            continue;
        }
        let start = index;
        let kind;
        let mut doc_comment = false;
        match byte {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                kind = TokenKind::Comment;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let is_doc =
                    bytes.get(index + 2) == Some(&b'*') && bytes.get(index + 3) != Some(&b'/');
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
                kind = TokenKind::Comment;
                doc_comment = is_doc;
            }
            b'\'' | b'"' | b'`' => {
                let quote = byte;
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index = index.saturating_add(2).min(bytes.len()),
                        quoted if quoted == quote => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                kind = TokenKind::String;
            }
            _ if is_ident_start(byte) => {
                index += 1;
                while index < bytes.len() && is_ident_continue(bytes[index]) {
                    index += 1;
                }
                kind = TokenKind::Identifier;
            }
            _ if byte.is_ascii_digit() => {
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric()
                        || matches!(bytes[index], b'.' | b'_' | b'x' | b'X'))
                {
                    index += 1;
                }
                kind = TokenKind::Number;
            }
            _ if is_punct(byte) => {
                index += 1;
                kind = TokenKind::Punct(byte);
            }
            _ => {
                // Advance one UTF-8 character, not one byte.
                let width = text[start..].chars().next().map_or(1, char::len_utf8);
                index = (start + width).min(bytes.len());
                kind = TokenKind::Other;
            }
        }
        let end = index.min(bytes.len());
        if start >= end {
            index = end.max(start + 1);
            continue;
        }
        tokens.push(Token {
            kind,
            text: text.get(start..end).unwrap_or_default(),
            start,
            end,
            starts_line: at_line_start,
            doc_comment,
        });
        at_line_start = false;
    }
    (tokens, truncated)
}

const fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

const fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

const fn is_punct(byte: u8) -> bool {
    matches!(
        byte,
        b'{' | b'}'
            | b'('
            | b')'
            | b'['
            | b']'
            | b','
            | b';'
            | b':'
            | b'.'
            | b'='
            | b'>'
            | b'<'
            | b'!'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'?'
            | b'&'
            | b'|'
            | b'^'
            | b'~'
    )
}

type TokenIndex = usize;

struct Parser<'a> {
    tokens: &'a [Token<'a>],
    summary: WxsSummary,
    last_doc: String,
    /// Local declarations by name, for export aliases.
    local_by_name: Vec<(String, WxsMemberKind, String, Option<String>)>,
}

impl<'a> Parser<'a> {
    fn run(&mut self) {
        let mut index = 0_usize;
        let mut depth = 0_i32;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                self.remember_doc(token);
                index += 1;
                continue;
            }
            if depth > 0 {
                index = self.skip_to_statement_end(index, &mut depth);
                continue;
            }
            if token.kind == TokenKind::Identifier {
                match token.text {
                    "function" => {
                        index = self.parse_function(index);
                        continue;
                    }
                    "const" | "let" | "var" => {
                        index = self.parse_declaration(index);
                        continue;
                    }
                    "module" => {
                        if let Some(next) = self.parse_module_exports(index) {
                            index = next;
                            continue;
                        }
                    }
                    "exports" => {
                        if let Some(next) = self.parse_exports_member(index) {
                            index = next;
                            continue;
                        }
                    }
                    "eval" => {
                        self.mark_dynamic("eval() is not analyzed");
                    }
                    "new" if self.token_text(index + 1) == Some("Function") => {
                        self.mark_dynamic("`new Function` is not analyzed");
                    }
                    _ => {}
                }
            }
            index += 1;
        }
    }

    fn remember_doc(&mut self, token: Token<'_>) {
        self.last_doc = comment_text(token.text, token.doc_comment);
    }

    fn take_doc(&mut self) -> String {
        std::mem::take(&mut self.last_doc)
    }

    fn mark_dynamic(&mut self, reason: &str) {
        if self.summary.dynamic.is_none() {
            self.summary.dynamic = Some(reason.to_owned());
        }
    }

    fn token_text(&self, index: TokenIndex) -> Option<&'a str> {
        self.tokens
            .get(index)
            .filter(|token| token.kind != TokenKind::Comment)
            .map(|token| token.text)
    }

    fn is_punct(&self, index: TokenIndex, punct: u8) -> bool {
        self.tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Punct(punct))
    }

    fn skip_comments(&self, mut index: TokenIndex) -> TokenIndex {
        while self
            .tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Comment)
        {
            index += 1;
        }
        index
    }

    fn range(start: usize, end: usize) -> SourceRange {
        SourceRange::new(valid_range(start, end))
    }

    fn token_range(&self, index: TokenIndex) -> SourceRange {
        self.tokens.get(index).map_or_else(
            || Self::range(0, 0),
            |token| Self::range(token.start, token.end),
        )
    }

    fn push_member(&mut self, member: WxsMember) {
        if self.summary.members.len() >= MAX_WXS_MEMBERS {
            self.summary.truncated = true;
            return;
        }
        self.local_by_name.push((
            member.name.clone(),
            member.kind,
            member.type_text.clone(),
            member.signature.clone(),
        ));
        self.summary.members.push(member);
    }

    /// Skips an expression or statement until `;`/new statement at depth zero.
    fn skip_to_statement_end(&self, start: TokenIndex, depth: &mut i32) -> TokenIndex {
        let mut index = start;
        let mut local_depth = *depth;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                index += 1;
                continue;
            }
            match token.kind {
                TokenKind::Punct(b'{' | b'(' | b'[') => {
                    local_depth += 1;
                    if local_depth.unsigned_abs() as usize > MAX_WXS_DEPTH {
                        *depth = 0;
                        return index;
                    }
                }
                TokenKind::Punct(b'}' | b')' | b']') => {
                    local_depth -= 1;
                }
                TokenKind::Punct(b';') if local_depth <= 0 => {
                    *depth = 0;
                    return index + 1;
                }
                _ => {}
            }
            let next_index = self.skip_comments(index + 1);
            if local_depth <= 0
                && token.starts_line
                && index != start
                && self.tokens.get(next_index).is_some_and(|next| {
                    next.kind == TokenKind::Identifier
                        && matches!(
                            next.text,
                            "function" | "const" | "let" | "var" | "module" | "exports"
                        )
                        && next.starts_line
                })
                && token.kind == TokenKind::Punct(b';')
            {
                *depth = 0;
                return index + 1;
            }
            if local_depth <= 0 && token.starts_line && index != start && starts_statement(token) {
                *depth = 0;
                return index;
            }
            index += 1;
            if local_depth < 0 {
                *depth = 0;
                return index;
            }
        }
        *depth = 0;
        index
    }

    /// Parses a `function name(...) { ... }` declaration.
    fn parse_function(&mut self, start: TokenIndex) -> TokenIndex {
        let mut index = self.skip_comments(start + 1);
        let mut name = None;
        if self
            .tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Identifier)
        {
            let token = self.tokens[index];
            name = Some((token.text.to_owned(), self.token_range(index)));
            index = self.skip_comments(index + 1);
        }
        let params = self.parse_params(index);
        let Some((_signature, return_type, body_end)) = self.parse_block_returns(params.1) else {
            self.mark_dynamic("function body could not be summarized");
            return self.skip_to_statement_end(start, &mut 0);
        };
        let doc = self.take_doc();
        let (param_types, return_override) = jsdoc_annotations(&doc);
        let return_type = return_override.unwrap_or(return_type);
        let params_rendered = apply_param_types(&params.0, &param_types);
        if let Some((name_text, name_range)) = name {
            let decl_end = self
                .tokens
                .get(body_end.saturating_sub(1))
                .map_or_else(|| name_range.end() as usize, |token| token.end);
            let signature_text = format!("({params_rendered}) => {return_type}");
            let member = WxsMember {
                name: name_text,
                kind: WxsMemberKind::Function,
                type_text: return_type,
                signature: Some(signature_text),
                doc,
                exported: false,
                name_range,
                decl_range: Self::range(
                    self.tokens
                        .get(start)
                        .map_or_else(|| name_range.start() as usize, |token| token.start),
                    decl_end,
                ),
                module_specifier: None,
                alias_of: None,
            };
            self.push_member(member);
        }
        body_end
    }

    /// Parses `(a, b = 1, ...rest)` and returns rendered parameter text plus
    /// the index after the closing parenthesis.
    fn parse_params(&self, start: TokenIndex) -> (String, TokenIndex) {
        let open = self.skip_comments(start);
        if !self.is_punct(open, b'(') {
            return (String::new(), open);
        }
        let mut parts = Vec::new();
        let mut current: Vec<Token<'a>> = Vec::new();
        let mut depth = 0_i32;
        let mut index = open + 1;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                index += 1;
                continue;
            }
            match token.kind {
                TokenKind::Punct(b'(' | b'[' | b'{') => {
                    depth += 1;
                    current.push(token);
                }
                TokenKind::Punct(b')') if depth == 0 => {
                    if !current.is_empty() {
                        parts.push(current);
                    }
                    return (render_params(&parts), index + 1);
                }
                TokenKind::Punct(b')' | b']' | b'}') => {
                    depth -= 1;
                    current.push(token);
                }
                TokenKind::Punct(b',') if depth == 0 => {
                    if !current.is_empty() {
                        parts.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(token),
            }
            index += 1;
        }
        (render_params(&parts), index)
    }

    /// Finds the body block and infers its return type.
    fn parse_body(&self, start: TokenIndex) -> Option<(String, String, TokenIndex)> {
        let mut index = self.skip_comments(start);
        // Arrow function with an expression body.
        if self.is_punct(index, b'=') && self.is_punct(index + 1, b'>') {
            index = self.skip_comments(index + 2);
            if self.is_punct(index, b'{') {
                return self.parse_block_returns(index);
            }
            let end = self.expression_end(index);
            let return_type = infer_tokens(&self.tokens[index..end], &self.local_by_name);
            return Some((String::new(), return_type, end));
        }
        if self.token_text(index) == Some("function") {
            index = self.skip_comments(index + 1);
            if self
                .tokens
                .get(index)
                .is_some_and(|token| token.kind == TokenKind::Identifier)
            {
                index = self.skip_comments(index + 1);
            }
            let (params, after) = self.parse_params(index);
            return self
                .parse_block_returns(after)
                .map(|(_, returns, end)| (params, returns, end));
        }
        None
    }

    fn parse_block_returns(&self, open: TokenIndex) -> Option<(String, String, TokenIndex)> {
        if !self.is_punct(open, b'{') {
            return None;
        }
        let mut depth = 0_i32;
        let mut index = open;
        let mut returns = Vec::new();
        let mut nested_function = 0_i32;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                index += 1;
                continue;
            }
            match token.kind {
                TokenKind::Punct(b'{' | b'(' | b'[') => {
                    depth += 1;
                }
                TokenKind::Punct(b'}' | b')' | b']') => {
                    depth -= 1;
                    if depth == 0 && token.kind == TokenKind::Punct(b'}') {
                        let return_type = if returns.is_empty() {
                            "void".to_owned()
                        } else {
                            returns.join(" | ")
                        };
                        return Some((String::new(), return_type, index + 1));
                    }
                }
                TokenKind::Identifier if token.text == "function" => {
                    nested_function += 1;
                }
                TokenKind::Identifier if token.text == "return" && nested_function == 0 => {
                    let start = self.skip_comments(index + 1);
                    let end = self.expression_end(start);
                    if end > start {
                        let inferred = infer_tokens(&self.tokens[start..end], &self.local_by_name);
                        if !returns.contains(&inferred) {
                            returns.push(inferred);
                        }
                    } else if returns.is_empty() {
                        returns.push("undefined".to_owned());
                    }
                    index = end;
                    continue;
                }
                _ => {}
            }
            if token.kind == TokenKind::Punct(b'}') && depth < nested_function {
                nested_function = nested_function.saturating_sub(1);
            }
            index += 1;
        }
        None
    }

    /// Returns the token index one past the expression starting at `start`
    /// (stops at `;`, a closing bracket at depth 0, or a new statement).
    fn expression_end(&self, start: TokenIndex) -> TokenIndex {
        let mut depth = 0_i32;
        let mut index = start;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                index += 1;
                continue;
            }
            match token.kind {
                TokenKind::Punct(b'{' | b'(' | b'[') => {
                    depth += 1;
                }
                TokenKind::Punct(b'}' | b')' | b']') => {
                    if depth == 0 {
                        return index;
                    }
                    depth -= 1;
                }
                TokenKind::Punct(b';' | b',') if depth == 0 => return index,
                _ => {}
            }
            index += 1;
        }
        index
    }

    fn parse_declaration(&mut self, start: TokenIndex) -> TokenIndex {
        let mut index = self.skip_comments(start + 1);
        let mut first = true;
        loop {
            if !self
                .tokens
                .get(index)
                .is_some_and(|token| token.kind == TokenKind::Identifier)
            {
                return self.skip_to_statement_end(index, &mut 0);
            }
            let name_token = self.tokens[index];
            let name = name_token.text.to_owned();
            let name_range = self.token_range(index);
            index = self.skip_comments(index + 1);
            if !self.is_punct(index, b'=') {
                let doc = if first {
                    self.take_doc()
                } else {
                    String::new()
                };
                self.push_member(WxsMember {
                    name,
                    kind: WxsMemberKind::Value,
                    type_text: "any".to_owned(),
                    signature: None,
                    doc,
                    exported: false,
                    name_range,
                    decl_range: Self::range(name_token.start, name_token.end),
                    module_specifier: None,
                    alias_of: None,
                });
                return self.skip_to_statement_end(index, &mut 0);
            }
            index = self.skip_comments(index + 1);
            let end = self.expression_end(index);
            let doc = if first {
                self.take_doc()
            } else {
                String::new()
            };
            let (kind, type_text, signature, module_specifier) =
                self.classify_initializer(index, end, &name);
            if kind == WxsMemberKind::Module {
                if let Some((specifier, specifier_range)) = self.require_specifier(index) {
                    self.push_require(name.clone(), specifier, name_range, specifier_range);
                }
            }
            // A mutable local keeps its declared/initial type for hover;
            // reassignment is not a reason to discard the whole module summary.
            let decl_end = self
                .tokens
                .get(end.saturating_sub(1))
                .map_or(name_token.end, |token| token.end);
            self.push_member(WxsMember {
                name,
                kind,
                type_text,
                signature,
                doc,
                exported: false,
                name_range,
                decl_range: Self::range(name_token.start, decl_end),
                module_specifier,
                alias_of: None,
            });
            first = false;
            if self.is_punct(end, b',') {
                index = self.skip_comments(end + 1);
                continue;
            }
            return end;
        }
    }

    fn classify_initializer(
        &mut self,
        start: TokenIndex,
        end: TokenIndex,
        _name: &str,
    ) -> (WxsMemberKind, String, Option<String>, Option<String>) {
        let start = self.skip_comments(start);
        if start >= end {
            return (WxsMemberKind::Value, "any".to_owned(), None, None);
        }
        let first = self.tokens[start];
        if first.kind == TokenKind::Identifier {
            match first.text {
                "function" => {
                    let (params, after) = self.parse_params(self.skip_comments(start + 1));
                    if let Some((_, returns, _)) = self.parse_block_returns(after) {
                        return (
                            WxsMemberKind::Function,
                            returns.clone(),
                            Some(format!("({params}) => {returns}")),
                            None,
                        );
                    }
                    return (WxsMemberKind::Function, "any".to_owned(), None, None);
                }
                "require" => {
                    if let Some((specifier, _specifier_range)) = self.require_specifier(start) {
                        return (
                            WxsMemberKind::Module,
                            format!("module {specifier}"),
                            None,
                            Some(specifier),
                        );
                    }
                    self.mark_dynamic("computed require() is not analyzed");
                    return (WxsMemberKind::Value, "any".to_owned(), None, None);
                }
                _ => {}
            }
        }
        // Arrow function: identifier/param list followed by `=>`.
        if let Some((params, returns, _)) = self.arrow_function(start, end) {
            return (
                WxsMemberKind::Function,
                returns.clone(),
                Some(format!("({params}) => {returns}")),
                None,
            );
        }
        (
            WxsMemberKind::Value,
            infer_tokens(&self.tokens[start..end], &self.local_by_name),
            None,
            None,
        )
    }

    fn arrow_function(
        &self,
        start: TokenIndex,
        end: TokenIndex,
    ) -> Option<(String, String, TokenIndex)> {
        let mut index = start;
        let first = self.tokens[index];
        if first.kind == TokenKind::Identifier {
            // Single-parameter arrow `x => ...`.
            if self.is_punct(index + 1, b'=') && self.is_punct(index + 2, b'>') {
                let body = self.skip_comments(index + 3);
                if body >= end {
                    return None;
                }
                let (_, returns, _) = self.parse_body(index + 1)?;
                return Some((format!("{}: any", first.text), returns, body));
            }
            if first.text == "async" {
                index += 1;
            }
        }
        if self.is_punct(index, b'(') {
            let (params, after) = self.parse_params(index);
            let arrow = self.skip_comments(after);
            if self.is_punct(arrow, b'=') && self.is_punct(arrow + 1, b'>') {
                let (_, returns, body_end) = self.parse_body(arrow)?;
                return Some((params, returns, body_end));
            }
        }
        None
    }

    fn require_specifier(&self, start: TokenIndex) -> Option<(String, SourceRange)> {
        let open = self.skip_comments(start + 1);
        if !self.is_punct(open, b'(') {
            return None;
        }
        let token = *self
            .tokens
            .get(open + 1)
            .filter(|token| token.kind == TokenKind::String)?;
        let close = self.skip_comments(open + 2);
        if !self.is_punct(close, b')') {
            return None;
        }
        let raw = token.text;
        let quote = raw.chars().next()?;
        if raw.len() < 2 || !raw.ends_with(quote) {
            return None;
        }
        let specifier = raw.get(1..raw.len() - 1)?.to_owned();
        let range = Self::range(token.start + quote.len_utf8(), token.end.saturating_sub(1));
        Some((specifier, range))
    }

    fn push_require(
        &mut self,
        binding: String,
        specifier: String,
        range: SourceRange,
        specifier_range: SourceRange,
    ) {
        self.summary.requires.push(WxsRequire {
            binding,
            specifier,
            range,
            specifier_range,
        });
    }

    /// Parses `module.exports` forms. Returns the next token index when the
    /// statement was recognized.
    fn parse_module_exports(&mut self, start: TokenIndex) -> Option<TokenIndex> {
        let mut index = self.skip_comments(start + 1);
        if !self.is_punct(index, b'.') {
            return None;
        }
        index = self.skip_comments(index + 1);
        if self.token_text(index) != Some("exports") {
            return None;
        }
        index = self.skip_comments(index + 1);
        if self.is_punct(index, b'[') {
            self.mark_dynamic("computed `module.exports[...]` members are not analyzed");
            return Some(self.skip_to_statement_end(index, &mut 0));
        }
        if self.is_punct(index, b'.') {
            let name_index = self.skip_comments(index + 1);
            let name = self
                .tokens
                .get(name_index)
                .filter(|token| token.kind == TokenKind::Identifier)?
                .text
                .to_owned();
            index = self.skip_comments(name_index + 1);
            if !self.is_punct(index, b'=') {
                return Some(index);
            }
            let value_start = self.skip_comments(index + 1);
            let end = self.expression_end(value_start);
            let doc = self.take_doc();
            self.export_value(name, self.token_range(name_index), value_start, end, doc);
            return Some(end);
        }
        if !self.is_punct(index, b'=') {
            return None;
        }
        let value_start = self.skip_comments(index + 1);
        let doc = self.take_doc();
        if self.is_punct(value_start, b'{') {
            let end = self.parse_export_object(value_start, doc);
            return Some(end);
        }
        if self.token_text(value_start) == Some("function") {
            self.mark_dynamic(
                "`module.exports` is a callable function; its members are not statically enumerable",
            );
            return Some(self.skip_to_statement_end(value_start, &mut 0));
        }
        // `module.exports = identifier` aliases an existing local only when the
        // identifier resolves to a statically known object literal.
        if self
            .tokens
            .get(value_start)
            .is_some_and(|token| token.kind == TokenKind::Identifier)
        {
            self.mark_dynamic("`module.exports` is assigned from a dynamic value");
            return Some(self.skip_to_statement_end(value_start, &mut 0));
        }
        self.mark_dynamic("`module.exports` is assigned from a dynamic value");
        Some(self.skip_to_statement_end(value_start, &mut 0))
    }

    #[allow(clippy::too_many_lines)]
    fn parse_export_object(&mut self, open: TokenIndex, doc: String) -> TokenIndex {
        let mut index = open + 1;
        let mut depth = 0_i32;
        let mut pending_doc = doc;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                if token.doc_comment || token.text.starts_with("//") {
                    pending_doc = comment_text(token.text, token.doc_comment);
                }
                index += 1;
                continue;
            }
            if depth == 0 && token.kind == TokenKind::Punct(b'}') {
                return index + 1;
            }
            if depth == 0 && token.kind == TokenKind::Punct(b',') {
                index += 1;
                continue;
            }
            if depth == 0 && token.kind == TokenKind::Punct(b'.') {
                // Spread of another module/object is dynamic.
                self.mark_dynamic("export spread members are not analyzed");
                index = self.skip_to_statement_end(index, &mut depth);
                continue;
            }
            if depth == 0 && matches!(token.kind, TokenKind::String | TokenKind::Number) {
                let (name, name_range) = key_literal(token, Self::range(token.start, token.end));
                let next = self.skip_comments(index + 1);
                if self.is_punct(next, b':') {
                    let value_start = self.skip_comments(next + 1);
                    let end = self.member_value_end(value_start, open);
                    let (kind, type_text, signature, module_specifier) =
                        self.classify_initializer(value_start, end, &name);
                    let alias_of = self.local_alias_range(value_start);
                    self.push_exported(
                        name,
                        kind,
                        type_text,
                        signature,
                        pending_doc.clone(),
                        name_range,
                        end,
                        module_specifier,
                        alias_of,
                    );
                    index = end;
                    continue;
                }
                self.mark_dynamic("string/number export keys must have a static value");
                index += 1;
                continue;
            }
            if depth == 0 && token.kind == TokenKind::Punct(b'[') {
                self.mark_dynamic("computed export keys are not analyzed");
                let after_key = self.skip_computed_key(index);
                let next = self.skip_comments(after_key);
                if self.is_punct(next, b':') {
                    let value_start = self.skip_comments(next + 1);
                    index = self.member_value_end(value_start, open);
                } else {
                    // Computed method key: skip its parameter list and body.
                    index = self.member_value_end(after_key, open);
                }
                continue;
            }
            if depth == 0
                && token.kind == TokenKind::Identifier
                && matches!(token.text, "get" | "set" | "async")
            {
                let after = self.skip_comments(index + 1);
                if self.tokens.get(after).is_some_and(|next| {
                    next.kind == TokenKind::Identifier || next.kind == TokenKind::Punct(b'[')
                }) {
                    self.mark_dynamic("accessor/async export members are not analyzed statically");
                    index = self.member_value_end(after, open);
                    continue;
                }
            }
            if depth == 0 && token.kind == TokenKind::Identifier {
                let name = token.text.to_owned();
                let name_range = self.token_range(index);
                let mut next = self.skip_comments(index + 1);
                if self.is_punct(next, b':') {
                    let value_start = self.skip_comments(next + 1);
                    let end = self.member_value_end(value_start, open);
                    let (kind, type_text, signature, module_specifier) =
                        self.classify_initializer(value_start, end, &name);
                    let alias_of = self.local_alias_range(value_start);
                    self.push_exported(
                        name,
                        kind,
                        type_text,
                        signature,
                        pending_doc.clone(),
                        name_range,
                        end,
                        module_specifier,
                        alias_of,
                    );
                    index = end;
                    continue;
                }
                if self.is_punct(next, b'(') {
                    // Method shorthand `foo(a) { ... }`.
                    let (params, after) = self.parse_params(next);
                    if let Some((_, returns, end)) = self.parse_block_returns(after) {
                        self.push_exported(
                            name,
                            WxsMemberKind::Function,
                            returns.clone(),
                            Some(format!("({params}) => {returns}")),
                            pending_doc.clone(),
                            name_range,
                            end,
                            None,
                            None,
                        );
                        index = end;
                        continue;
                    }
                }
                // Shorthand `{ foo }` or `{ foo = 1 }` (invalid) — alias local.
                let alias = self
                    .local_by_name
                    .iter()
                    .find(|entry| entry.0 == name)
                    .cloned();
                if let Some((local_name, kind, type_text, signature)) = alias {
                    let local_range = self
                        .summary
                        .members
                        .iter()
                        .rev()
                        .find(|member| member.name == local_name && !member.exported)
                        .map(|member| member.name_range);
                    self.push_exported(
                        name.clone(),
                        kind,
                        type_text,
                        signature,
                        pending_doc.clone(),
                        name_range,
                        index + 1,
                        None,
                        local_range,
                    );
                } else {
                    self.push_exported(
                        name,
                        WxsMemberKind::Value,
                        "any".to_owned(),
                        None,
                        pending_doc.clone(),
                        name_range,
                        index + 1,
                        None,
                        None,
                    );
                }
                if self.is_punct(next, b'=') {
                    next = self.skip_comments(next + 1);
                    let end = self.member_value_end(next, open);
                    index = end;
                } else {
                    index += 1;
                }
                continue;
            }
            // Nested value/brace: skip it as one member value.
            if matches!(token.kind, TokenKind::Punct(b'(' | b'[' | b'{')) {
                depth += 1;
            } else if matches!(token.kind, TokenKind::Punct(b')' | b']' | b'}')) {
                depth -= 1;
            }
            index += 1;
        }
        self.summary.truncated = true;
        index
    }

    fn member_value_end(&self, start: TokenIndex, object_open: TokenIndex) -> TokenIndex {
        let mut depth = 0_i32;
        let mut index = start;
        while index < self.tokens.len() {
            let token = self.tokens[index];
            if token.kind == TokenKind::Comment {
                index += 1;
                continue;
            }
            match token.kind {
                TokenKind::Punct(b'{' | b'(' | b'[') => {
                    depth += 1;
                }
                TokenKind::Punct(b'}' | b',') if depth == 0 => return index,
                TokenKind::Punct(b')' | b']' | b'}') => {
                    depth -= 1;
                }
                _ => {}
            }
            if index <= object_open {
                return index + 1;
            }
            index += 1;
        }
        index
    }

    /// Skips a `[computed]` key, balancing nested brackets.
    #[allow(clippy::missing_const_for_fn)]
    fn skip_computed_key(&self, open: TokenIndex) -> TokenIndex {
        let mut depth = 0_i32;
        let mut index = open;
        while index < self.tokens.len() {
            match self.tokens[index].kind {
                TokenKind::Punct(b'[') => depth += 1,
                TokenKind::Punct(b']') => {
                    depth -= 1;
                    if depth == 0 {
                        return index + 1;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        index
    }

    /// Local declaration range when the export value is a bare identifier.
    fn local_alias_range(&self, value_start: TokenIndex) -> Option<SourceRange> {
        let token = self.tokens.get(value_start)?;
        if token.kind != TokenKind::Identifier {
            return None;
        }
        self.summary
            .members
            .iter()
            .rev()
            .find(|member| member.name == token.text && !member.exported)
            .map(|member| member.name_range)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_exported(
        &mut self,
        name: String,
        kind: WxsMemberKind,
        mut type_text: String,
        signature: Option<String>,
        doc: String,
        name_range: SourceRange,
        end: TokenIndex,
        module_specifier: Option<String>,
        alias_of: Option<SourceRange>,
    ) {
        let decl_end = self.tokens.get(end.saturating_sub(1)).map_or_else(
            || name_range.end(),
            |token| u32::try_from(token.end).unwrap_or(0),
        );
        let local = self
            .summary
            .members
            .iter()
            .rev()
            .find(|member| member.name == name && !member.exported);
        let module_specifier =
            module_specifier.or_else(|| local.and_then(|member| member.module_specifier.clone()));
        let doc = if doc.is_empty() {
            local.map_or_else(String::new, |member| member.doc.clone())
        } else {
            doc
        };
        let signature = signature.map(|signature| apply_signature_jsdoc(&signature, &doc));
        if kind == WxsMemberKind::Function {
            if let Some(returned) = signature.as_deref().and_then(|signature| {
                signature
                    .rsplit_once("=>")
                    .map(|(_, returned)| returned.trim())
            }) {
                returned.clone_into(&mut type_text);
            }
        }
        self.push_member(WxsMember {
            name,
            kind,
            type_text,
            signature,
            doc,
            exported: true,
            name_range,
            decl_range: Self::range(name_range.start() as usize, decl_end as usize),
            module_specifier,
            alias_of,
        });
    }

    fn export_value(
        &mut self,
        name: String,
        name_range: SourceRange,
        value_start: TokenIndex,
        end: TokenIndex,
        doc: String,
    ) {
        if self.is_punct(value_start, b'.') || self.is_punct(value_start, b'{') {
            self.mark_dynamic("`module.exports` was replaced after an export");
        }
        let (kind, type_text, signature, module_specifier) =
            self.classify_initializer(value_start, end, &name);
        let alias_of = self.local_alias_range(value_start);
        self.push_exported(
            name,
            kind,
            type_text,
            signature,
            doc,
            name_range,
            end,
            module_specifier,
            alias_of,
        );
    }

    /// Parses `exports.name = ...`.
    fn parse_exports_member(&mut self, start: TokenIndex) -> Option<TokenIndex> {
        let mut index = self.skip_comments(start + 1);
        if self.is_punct(index, b'[') {
            self.mark_dynamic("computed `exports[...]` members are not analyzed");
            return Some(index + 1);
        }
        if !self.is_punct(index, b'.') {
            return None;
        }
        index = self.skip_comments(index + 1);
        let name_token = self
            .tokens
            .get(index)
            .filter(|token| token.kind == TokenKind::Identifier)?;
        let name = name_token.text.to_owned();
        let name_range = self.token_range(index);
        index = self.skip_comments(index + 1);
        if !self.is_punct(index, b'=') {
            return Some(index);
        }
        let value_start = self.skip_comments(index + 1);
        let end = self.expression_end(value_start);
        let doc = self.take_doc();
        if self.token_text(value_start) == Some("function") {
            let (params, after) = self.parse_params(self.skip_comments(value_start + 1));
            if let Some((_, returns, body_end)) = self.parse_block_returns(after) {
                let (param_types, return_override) = jsdoc_annotations(&doc);
                let returns = return_override.unwrap_or(returns);
                self.push_exported(
                    name,
                    WxsMemberKind::Function,
                    returns.clone(),
                    Some(format!(
                        "({}) => {returns}",
                        apply_param_types(&params, &param_types)
                    )),
                    doc,
                    name_range,
                    body_end,
                    None,
                    None,
                );
                return Some(body_end);
            }
        }
        self.export_value(name, name_range, value_start, end, doc);
        Some(end)
    }
}

/// Builds a byte range from scanner offsets.
///
/// Scanner offsets are ordered by construction; the fallback only satisfies
/// the range invariant without panicking on hostile input.
fn valid_range(start: usize, end: usize) -> TextRange {
    let start = u32::try_from(start).unwrap_or(u32::MAX);
    let end = u32::try_from(end).unwrap_or(start);
    TextRange::new(start, end).unwrap_or_else(|_| {
        TextRange::new(0, 0).unwrap_or_else(|_| unreachable!("an empty text range is valid"))
    })
}

/// Decodes the small escape subset meaningful in a static export key.
fn unescape_key(raw: &str) -> String {
    let mut output = String::new();
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            match chars.next() {
                Some('n') => output.push('\n'),
                Some('r') => output.push('\r'),
                Some('t') => output.push('\t'),
                Some('b') => output.push('\u{8}'),
                Some('f') => output.push('\u{c}'),
                Some(quote @ ('\'' | '"' | '\\' | '`')) => output.push(quote),
                Some(other) => {
                    output.push('\\');
                    output.push(other);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// Extracts a static string/number export key and its name range.
fn key_literal(token: Token<'_>, token_range: SourceRange) -> (String, SourceRange) {
    if token.kind == TokenKind::String {
        let raw = token.text;
        let quote = raw.chars().next().unwrap_or('"');
        let name = unescape_key(
            raw.strip_prefix(quote)
                .and_then(|raw| raw.strip_suffix(quote))
                .unwrap_or(raw),
        );
        let start = token_range.start() + u32::try_from(quote.len_utf8()).unwrap_or(0);
        let end = token_range.end().saturating_sub(1);
        (
            name,
            SourceRange::from_bounds(start, end).unwrap_or(token_range),
        )
    } else {
        (token.text.to_owned(), token_range)
    }
}

fn comment_text(raw: &str, doc_block: bool) -> String {
    let body = if doc_block {
        raw.strip_prefix("/**")
            .and_then(|rest| rest.strip_suffix("*/"))
            .unwrap_or(raw)
    } else if let Some(rest) = raw.strip_prefix("//") {
        rest
    } else if let Some(rest) = raw
        .strip_prefix("/*")
        .and_then(|rest| rest.strip_suffix("*/"))
    {
        rest
    } else {
        raw
    };
    let mut lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim().trim_start_matches('*').trim();
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
    }
    lines.join("\n").trim().to_owned()
}

/// Minimal `JSDoc` annotations for WXS functions: `@param {type} name` and
/// `@returns {type}`. Unknown tags are ignored; the comment stays documentation.
fn jsdoc_annotations(doc: &str) -> (Vec<(String, String)>, Option<String>) {
    let mut params = Vec::new();
    let mut returns = None;
    let normalized = doc.replace(" @", "\n@");
    for line in normalized.lines() {
        let line = line.trim().trim_start_matches('*').trim();
        let Some(rest) = line.strip_prefix('@') else {
            continue;
        };
        let tag_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let tag = &rest[..tag_end];
        let after = rest[tag_end..].trim_start();
        let Some(open) = after.find('{') else {
            continue;
        };
        let Some(close) = after[open + 1..].find('}') else {
            continue;
        };
        let type_text = after[open + 1..open + 1 + close].trim();
        if type_text.is_empty() {
            continue;
        }
        let tail = after[open + 1 + close + 1..].trim();
        match tag {
            "param" | "arg" | "argument" => {
                let name = tail
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('[')
                    .trim_end_matches(']');
                if !name.is_empty() {
                    params.push((name.to_owned(), type_text.to_owned()));
                }
            }
            "returns" | "return" => returns = Some(type_text.to_owned()),
            _ => {}
        }
    }
    (params, returns)
}

fn apply_signature_jsdoc(signature: &str, doc: &str) -> String {
    let (param_types, return_type) = jsdoc_annotations(doc);
    if param_types.is_empty() && return_type.is_none() {
        return signature.to_owned();
    }
    let Some(rest) = signature.strip_prefix('(') else {
        return signature.to_owned();
    };
    let Some((params, after)) = rest.split_once(") =>") else {
        return signature.to_owned();
    };
    let params = apply_param_types(params, &param_types);
    let returns = return_type.unwrap_or_else(|| after.trim().to_owned());
    format!("({params}) => {returns}")
}

fn apply_param_types(rendered: &str, annotations: &[(String, String)]) -> String {
    if annotations.is_empty() {
        return rendered.to_owned();
    }
    rendered
        .split(", ")
        .map(|param| {
            let Some((name, _)) = param.split_once(": ") else {
                return param.to_owned();
            };
            match annotations.iter().find(|(annotated, _)| annotated == name) {
                Some((_, type_text)) => format!("{name}: {type_text}"),
                None => param.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_params(parts: &[Vec<Token<'_>>]) -> String {
    let mut rendered = Vec::new();
    for part in parts {
        let mut rest = false;
        let mut name = None;
        let mut default_type = None;
        let mut index = 0;
        while index < part.len() {
            let token = part[index];
            if token.kind == TokenKind::Punct(b'.') && index + 2 < part.len() {
                rest = true;
                index += 3;
                continue;
            }
            if token.kind == TokenKind::Identifier && name.is_none() {
                name = Some(token.text.to_owned());
                index += 1;
                continue;
            }
            if token.kind == TokenKind::Punct(b'=') {
                default_type = Some(infer_tokens(&part[index + 1..], &[]));
                break;
            }
            index += 1;
        }
        let Some(name) = name else {
            rendered.push("arg: any".to_owned());
            continue;
        };
        let type_text = if rest {
            "any[]".to_owned()
        } else {
            default_type.unwrap_or_else(|| "any".to_owned())
        };
        rendered.push(format!("{name}: {type_text}"));
    }
    rendered.join(", ")
}

/// Conservative type of a token slice.
fn infer_tokens(
    tokens: &[Token<'_>],
    locals: &[(String, WxsMemberKind, String, Option<String>)],
) -> String {
    let mut iter = tokens
        .iter()
        .filter(|token| token.kind != TokenKind::Comment);
    let Some(first) = iter.next() else {
        return "any".to_owned();
    };
    match first.kind {
        TokenKind::String => "string".to_owned(),
        TokenKind::Number => "number".to_owned(),
        TokenKind::Identifier => match first.text {
            "true" | "false" => "boolean".to_owned(),
            "null" | "undefined" => "any".to_owned(),
            "function" => "(...args: any) => any".to_owned(),
            "async" => "(...args: any) => Promise<any>".to_owned(),
            name => locals
                .iter()
                .find(|entry| entry.0 == name)
                .map_or_else(|| "any".to_owned(), |entry| entry.2.clone()),
        },
        TokenKind::Punct(b'[') => array_type(tokens),
        TokenKind::Punct(b'(') => {
            if tokens
                .iter()
                .any(|token| token.kind == TokenKind::Punct(b'=') && token.start > first.start)
            {
                "(...args: any) => any".to_owned()
            } else {
                "object".to_owned()
            }
        }
        TokenKind::Punct(b'{') => "object".to_owned(),
        _ => "any".to_owned(),
    }
}

fn array_type(tokens: &[Token<'_>]) -> String {
    let mut types: Vec<&str> = Vec::new();
    let mut depth = 0_i32;
    for token in tokens {
        match token.kind {
            TokenKind::Punct(b'[' | b'{' | b'(') => depth += 1,
            TokenKind::Punct(b']' | b'}' | b')') => depth -= 1,
            TokenKind::String if depth == 1 => push_unique(&mut types, "string"),
            TokenKind::Number if depth == 1 => push_unique(&mut types, "number"),
            TokenKind::Identifier if depth == 1 && matches!(token.text, "true" | "false") => {
                push_unique(&mut types, "boolean");
            }
            _ => {}
        }
        if depth == 0 && token.start > 0 {
            break;
        }
    }
    match types.as_slice() {
        [] => "any[]".to_owned(),
        [single] => format!("{single}[]"),
        many => format!("({})[]", many.join(" | ")),
    }
}

fn push_unique<'a>(values: &mut Vec<&'a str>, value: &'a str) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn starts_statement(token: Token<'_>) -> bool {
    token.kind == TokenKind::Punct(b'}')
        || (token.kind == TokenKind::Identifier
            && matches!(token.text, "function" | "const" | "let" | "var" | "class"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn member<'a>(summary: &'a WxsSummary, name: &str) -> &'a WxsMember {
        summary
            .member(name)
            .unwrap_or_else(|| panic!("missing member {name}: {:?}", summary.members))
    }

    fn slice(text: &str, range: SourceRange) -> &str {
        &text[range.start() as usize..range.end() as usize]
    }

    #[test]
    fn summarizes_functions_and_literals_with_docs() {
        let text = r#"
/** Formats a value. */
function format(value, width = 2) {
  return "x";
}
const label = "hello";
let count = 0;
const flags = [true, false];
"#;
        let summary = summarize_wxs(text);
        assert!(summary.dynamic.is_none(), "{summary:?}");
        let format = member(&summary, "format");
        assert_eq!(format.kind, WxsMemberKind::Function);
        assert_eq!(format.type_text, "string");
        assert_eq!(
            format.signature.as_deref(),
            Some("(value: any, width: number) => string")
        );
        assert_eq!(format.doc, "Formats a value.");
        assert_eq!(slice(text, format.name_range), "format");
        assert_eq!(member(&summary, "label").type_text, "string");
        assert_eq!(member(&summary, "count").type_text, "number");
        assert_eq!(member(&summary, "flags").type_text, "boolean[]");
        assert!(!member(&summary, "format").exported);
    }

    #[test]
    fn summarizes_module_exports_object_and_exports_assignments() {
        let text = r#"
const suffix = "!";
function upper(value) {
  return value;
}
module.exports = {
  upper: upper,
  suffix,
  add: function (a, b) { return a + b; },
  version: 1,
};
exports.extra = true;
"#;
        let summary = summarize_wxs(text);
        assert!(summary.dynamic.is_none(), "{summary:?}");
        assert!(member(&summary, "upper").exported);
        assert!(member(&summary, "suffix").exported);
        assert!(member(&summary, "add").exported);
        assert_eq!(member(&summary, "version").type_text, "number");
        assert_eq!(member(&summary, "extra").type_text, "boolean");
        assert!(summary.exported().any(|member| member.name == "add"));
        assert_eq!(
            member(&summary, "add").signature.as_deref(),
            Some("(a: any, b: any) => any")
        );
    }

    #[test]
    fn summarizes_requires_and_arrow_functions() {
        let text = r#"
const utils = require("./utils.wxs");
const double = (value) => value * 2;
module.exports = { double, utils };
"#;
        let summary = summarize_wxs(text);
        assert_eq!(summary.requires.len(), 1);
        assert_eq!(summary.requires[0].binding, "utils");
        assert_eq!(summary.requires[0].specifier, "./utils.wxs");
        assert_eq!(
            slice(text, summary.requires[0].specifier_range),
            "./utils.wxs"
        );
        assert_eq!(member(&summary, "utils").kind, WxsMemberKind::Module);
        assert_eq!(
            member(&summary, "utils").module_specifier.as_deref(),
            Some("./utils.wxs")
        );
        assert!(member(&summary, "double").exported);
        assert_eq!(member(&summary, "double").kind, WxsMemberKind::Function);
    }

    #[test]
    fn jsdoc_annotations_override_inferred_signatures() {
        let text = "
/**
 * Formats a value.
 * @param {string} value source value
 * @returns {number}
 */
function format(value) { return value; }
module.exports = { format };
";
        let summary = summarize_wxs(text);
        let format = member(&summary, "format");
        assert_eq!(
            format.signature.as_deref(),
            Some("(value: string) => number"),
            "{format:?}"
        );
        assert_eq!(format.type_text, "number");
        assert!(format.doc.contains("Formats a value."));
    }

    #[test]
    fn exported_function_properties_get_jsdoc_types() {
        let text = "
module.exports = {
  /** @param {string} value @returns {number} */
  score: function (value) { return value; },
};
";
        let summary = summarize_wxs(text);
        assert_eq!(
            member(&summary, "score").signature.as_deref(),
            Some("(value: string) => number")
        );
    }

    #[test]
    fn string_export_keys_are_collected_and_computed_keys_are_dynamic() {
        let text = r#"
module.exports = { "a-b": 1, 2: "two", [name]: 3, "bad" };
module.exports[computed] = 4;
"#;
        let summary = summarize_wxs(text);
        assert_eq!(member(&summary, "a-b").type_text, "number");
        assert_eq!(member(&summary, "2").type_text, "string");
        assert!(summary.dynamic.is_some(), "{summary:?}");
    }

    #[test]
    fn escaped_string_export_keys_are_decoded() {
        let summary = summarize_wxs(r#"module.exports = { "a\"b": 1 };"#);
        assert_eq!(member(&summary, "a\"b").type_text, "number", "{summary:?}");
    }

    #[test]
    fn computed_keys_skip_their_values_and_accessors_are_not_members() {
        let text = r#"
const k = "x";
const foo = 1;
module.exports = {
  [k]: foo,
  [k]: foo(),
  [k]() { return 1; },
  get label() { return "x"; },
  set label(value) {},
  async run() { return 1; },
  normal: 2,
};
"#;
        let summary = summarize_wxs(text);
        for name in ["foo", "get", "set", "async", "label", "run"] {
            assert!(
                summary.exported().all(|member| member.name != name),
                "name {name} in {summary:?}"
            );
        }
        assert_eq!(member(&summary, "normal").type_text, "number");
        assert!(summary.dynamic.is_some());
    }

    #[test]
    fn callable_module_exports_are_reported_not_faked_as_members() {
        let summary = summarize_wxs("module.exports = function format(value) { return value; };");
        assert!(summary.dynamic.is_some(), "{summary:?}");
        assert!(summary.exported().next().is_none());
    }

    #[test]
    fn computed_exports_and_eval_are_reported_not_guessed() {
        for text in [
            "const name = 'x'; exports[name] = 1;",
            "module.exports = makeExports();",
            "eval('x');",
            "new Function('return 1');",
        ] {
            let summary = summarize_wxs(text);
            assert!(summary.dynamic.is_some(), "text {text:?}: {summary:?}");
        }
    }

    #[test]
    fn malformed_input_stays_bounded() {
        let text = "function broken(a, b { return ";
        let summary = summarize_wxs(text);
        assert!(!summary.members.is_empty() || summary.dynamic.is_some() || summary.truncated);
        let deep = format!("const x = {}1;", "[".repeat(200));
        let summary = summarize_wxs(&deep);
        assert!(summary.truncated || summary.members.len() <= MAX_WXS_MEMBERS);
    }

    #[test]
    fn doc_comments_attach_to_the_following_declaration_only() {
        let text = "/** docs */\nconst a = 1;\n// other\nconst b = 2;";
        let summary = summarize_wxs(text);
        assert_eq!(member(&summary, "a").doc, "docs");
        assert_eq!(member(&summary, "b").doc, "other");
    }

    #[test]
    fn byte_and_member_budgets_are_visible() {
        let mut text = "a".repeat(MAX_WXS_BYTES - 1);
        text.push('😀');
        text.push_str("module.exports = { label: 1 };");
        let summary = summarize_wxs(&text);
        assert!(summary.truncated, "a non-UTF-8-boundary cut must truncate");

        let mut many = String::new();
        for index in 0..600 {
            let _ = writeln!(many, "exports.m{index} = {index};");
        }
        let summary = summarize_wxs(&many);
        assert!(summary.truncated, "member budget must set truncated");
        assert!(summary.members.len() <= MAX_WXS_MEMBERS);
    }

    #[test]
    fn multiple_declarators_are_all_collected() {
        let text = "const a = 1, b = \"x\", c = [true];";
        let summary = summarize_wxs(text);
        assert_eq!(member(&summary, "a").type_text, "number");
        assert_eq!(member(&summary, "b").type_text, "string");
        assert_eq!(member(&summary, "c").type_text, "boolean[]");
    }

    #[test]
    fn value_alias_uses_the_local_type() {
        let text = "const base = 1;\nconst alias = base;\nmodule.exports = { alias };";
        let summary = summarize_wxs(text);
        assert_eq!(member(&summary, "alias").type_text, "number");
        assert!(member(&summary, "alias").exported);
    }
}
