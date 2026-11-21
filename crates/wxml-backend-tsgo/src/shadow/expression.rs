//! WXML expression tokens and value-identifier collection.
//!
//! WXML expressions are not TypeScript files; this module lexes the small
//! expression grammar the shadow needs and proves its shape before any text is
//! emitted. It replaces `ts.createSourceFile` for the Rust slice, but only for
//! the constructs the slice can prove:
//!
//! - member names (`.name`, `?.name`), object keys, strings, template text,
//!   comments, regular expressions and numeric literals are not free roots;
//! - object shorthand `{ title }` is reported so the rewriter can expand it;
//! - function/class/arrow/`new`/`as`/`satisfies`/assignment/update/type-shape
//!   constructs fail closed with [`ShadowError::BadExpression`].
//!
//! Template substitutions are lexed as expressions, so `` `title ${title}` ``
//! keeps the template text and rewrites only the substitution.
//!
//! # Limits
//!
//! [`MAX_EXPRESSION_DEPTH`] and [`MAX_EXPRESSION_NODES`] bound untrusted WXML
//! input. Exceeding them is a typed error, never a panic or a partial rewrite.

use crate::shadow::ShadowError;

/// Maximum nested delimiter/template depth of one expression.
pub const MAX_EXPRESSION_DEPTH: usize = 256;

/// Maximum token count of one expression.
pub const MAX_EXPRESSION_NODES: usize = 10_000;

/// Hard token budget for a whole script scan.
pub const MAX_SCRIPT_TOKENS: usize = 1_000_000;

/// Hard delimiter/template nesting budget for a whole script scan.
pub const MAX_SCRIPT_DEPTH: usize = 1_024;

/// Why an expression is mounted into the shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressionKind {
    /// `{{ ... }}` interpolation.
    Mustache,
    /// Directive attribute expression such as `wx:if`.
    Directive,
    /// Plain attribute binding.
    Attribute,
    /// Event handler attribute.
    EventHandler,
}

/// Half-open UTF-8 range of one expression in the original WXML text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpressionSlot {
    /// Inclusive UTF-8 start.
    pub start: u32,
    /// Exclusive UTF-8 end.
    pub end: u32,
    /// Expression role.
    pub kind: ExpressionKind,
}

/// Lexical token kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// ECMAScript identifier (keywords included).
    Identifier,
    /// Single ASCII punctuator.
    Punctuator(char),
    /// Quoted string literal, kept opaque.
    StringLiteral,
    /// Numeric literal, kept opaque.
    Number,
    /// Regular-expression literal, kept opaque.
    Regex,
    /// Template literal text (including the enclosing backticks).
    TemplateChunk,
    /// Start of a template substitution (`${`).
    TemplateExpressionStart,
    /// End of a template substitution (`}`).
    TemplateExpressionEnd,
}

/// One token with half-open UTF-8 byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    /// Token kind.
    pub kind: TokenKind,
    /// Inclusive UTF-8 byte start.
    pub start: usize,
    /// Exclusive UTF-8 byte end.
    pub end: usize,
}

impl Token {
    /// Returns the exact token text.
    #[must_use]
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}

/// One identifier in value position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueIdentifier {
    /// UTF-8 byte start of the identifier.
    pub start: usize,
    /// Exclusive UTF-8 byte end of the identifier.
    pub end: usize,
    /// True for `{ title }`, where the same token is key and value.
    pub shorthand: bool,
}

/// Value identifiers of one expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectedIdentifiers {
    /// Every identifier in value position, including member names and globals.
    pub values: Vec<ValueIdentifier>,
    /// Every identifier token, including object keys; used for explicit targets
    /// and hover candidates (mirrors `scanIdentifiers` in the oracle).
    pub all: Vec<ValueIdentifier>,
    /// Identifiers that are free roots (member names and object keys excluded).
    pub roots: Vec<ValueIdentifier>,
}

/// Lexes one expression, including template substitutions.
///
/// Comments are skipped. The result is not yet validated for shape; call
/// [`collect_value_identifiers`] for rewriter-grade validation.
pub fn lex_expression(text: &str) -> Result<Vec<Token>, ShadowError> {
    Lexer::new(text, MAX_EXPRESSION_NODES, "expression.tokens", false).run()
}

/// Lexes a script for structural scanning.
///
/// This is the same lexer as [`lex_expression`]; registration scanning is
/// responsible for interpreting braces. It never panics on malformed input.
pub fn lex_script(text: &str) -> Result<Vec<Token>, ShadowError> {
    Lexer::new(text, MAX_SCRIPT_TOKENS, "script.tokens", true).run()
}

/// Collects value-position identifiers from one WXML expression.
///
/// Returns [`ShadowError::BadExpression`] for malformed text, unbalanced
/// delimiters, statement wrappers and unsupported expression shapes.
pub fn collect_value_identifiers(text: &str) -> Result<CollectedIdentifiers, ShadowError> {
    let tokens = lex_expression(text).map_err(|error| annotate(error, text))?;
    let pairs = validate_expression(text, &tokens).map_err(|error| annotate(error, text))?;
    let mut walker = Walker {
        source: text,
        tokens: &tokens,
        pairs: &pairs,
        collected: CollectedIdentifiers::default(),
    };
    walker.walk(0, tokens.len())?;
    Ok(walker.collected)
}

/// Returns `true` when `name` is an ECMAScript identifier (simple form).
#[must_use]
pub fn is_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|character| character == '_' || character == '$' || character.is_alphanumeric())
}

/// Returns `true` when `name` can be declared as a template-local binding.
///
/// Keywords, strict-mode reserved words, global value names and the bridge's
/// `__wxml_*`/`__WxmlProps_*` namespaces are rejected.
#[must_use]
pub fn is_binding_identifier(name: &str) -> bool {
    if !is_identifier_name(name) {
        return false;
    }
    if RESERVED_WORDS.binary_search(&name).is_ok() {
        return false;
    }
    !name.starts_with("__wxml_") && !name.starts_with("__WxmlProps_")
}

/// Bounded, UTF-16-aware expression description for error messages.
///
/// Long input is cut at 120 UTF-16 code units; the message only ever contains
/// that prefix plus the total length.
#[must_use]
pub fn describe_expression(expr: &str) -> String {
    const LIMIT: usize = 120;
    let total = expr.encode_utf16().count();
    if total <= LIMIT {
        return expr.to_owned();
    }
    let mut units = 0;
    let mut end = 0;
    for (index, character) in expr.char_indices() {
        let width = character.len_utf16();
        if units + width > LIMIT {
            break;
        }
        units += width;
        end = index + character.len_utf8();
    }
    format!("{}… (total {total} UTF-16 chars)", &expr[..end])
}

/// ECMAScript keywords/literals that are never value identifiers.
fn is_non_value_keyword(name: &str) -> bool {
    matches!(
        name,
        "true" | "false" | "null" | "this" | "typeof" | "void" | "in" | "instanceof" | "of"
    )
}

/// Identifiers that make an expression unsupported in this slice.
fn is_unsupported_keyword(name: &str) -> bool {
    matches!(
        name,
        "new"
            | "delete"
            | "await"
            | "yield"
            | "function"
            | "class"
            | "import"
            | "export"
            | "super"
            | "throw"
            | "var"
            | "let"
            | "const"
            | "if"
            | "else"
            | "for"
            | "while"
            | "do"
            | "switch"
            | "case"
            | "default"
            | "try"
            | "catch"
            | "finally"
            | "return"
            | "break"
            | "continue"
            | "debugger"
            | "with"
            | "enum"
            | "implements"
            | "interface"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "static"
    )
}

/// Sorted reserved words rejected for template-local bindings.
const RESERVED_WORDS: &[&str] = &[
    "Infinity",
    "NaN",
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn bad(detail: impl Into<String>) -> ShadowError {
    ShadowError::BadExpression {
        detail: detail.into(),
    }
}

fn annotate(error: ShadowError, text: &str) -> ShadowError {
    match error {
        ShadowError::BadExpression { detail } if !detail.contains("… (total ") => {
            ShadowError::BadExpression {
                detail: format!("{detail}: {}", describe_expression(text)),
            }
        }
        other => other,
    }
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    index: usize,
    tokens: Vec<Token>,
    depth: usize,
    max_tokens: usize,
    token_budget: &'static str,
    script_mode: bool,
}

impl<'a> Lexer<'a> {
    fn new(
        source: &'a str,
        max_tokens: usize,
        token_budget: &'static str,
        script_mode: bool,
    ) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            index: if source.starts_with('\u{FEFF}') { 3 } else { 0 },
            tokens: Vec::new(),
            depth: 0,
            max_tokens,
            token_budget,
            script_mode,
        }
    }

    fn run(mut self) -> Result<Vec<Token>, ShadowError> {
        if self.script_mode && self.source[self.index..].starts_with("#!") {
            self.index = self.source[self.index..]
                .find('\n')
                .map_or(self.source.len(), |offset| self.index + offset + 1);
        }
        self.scan_code(false)?;
        Ok(self.tokens)
    }

    fn push(&mut self, kind: TokenKind, start: usize) -> Result<(), ShadowError> {
        let end = self.index;
        self.push_token_at(kind, start, end)
    }

    fn push_token_at(
        &mut self,
        kind: TokenKind,
        start: usize,
        end: usize,
    ) -> Result<(), ShadowError> {
        if self.tokens.len() >= self.max_tokens {
            return Err(ShadowError::BudgetExceeded {
                budget: self.token_budget,
            });
        }
        self.tokens.push(Token { kind, start, end });
        Ok(())
    }

    fn scan_code(&mut self, stop_at_substitution_end: bool) -> Result<(), ShadowError> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(bad("expression exceeds traversal depth limits"));
        }
        let mut brace_depth = 0_usize;
        while self.index < self.bytes.len() {
            let byte = self.bytes[self.index];
            if byte.is_ascii_whitespace() {
                self.index += 1;
                continue;
            }
            if byte == b'/' && self.peek(1) == Some(b'/') {
                self.skip_line_comment();
                continue;
            }
            if byte == b'/' && self.peek(1) == Some(b'*') {
                self.skip_block_comment()?;
                continue;
            }
            match byte {
                b'}' => {
                    if stop_at_substitution_end && brace_depth == 0 {
                        self.index += 1;
                        self.push(TokenKind::TemplateExpressionEnd, self.index - 1)?;
                        self.depth -= 1;
                        return Ok(());
                    }
                    if stop_at_substitution_end {
                        brace_depth -= 1;
                    }
                    self.index += 1;
                    self.push(TokenKind::Punctuator('}'), self.index - 1)?;
                }
                b'{' => {
                    if stop_at_substitution_end {
                        brace_depth += 1;
                    }
                    self.index += 1;
                    self.push(TokenKind::Punctuator('{'), self.index - 1)?;
                }
                b'\'' | b'"' => self.scan_string(byte)?,
                b'`' => self.scan_template()?,
                b'/' if self.regex_allowed() => self.scan_regex()?,
                b'0'..=b'9' => self.scan_number()?,
                b'.' if self.peek(1).is_some_and(|next| next.is_ascii_digit()) => {
                    self.scan_number()?;
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => self.scan_identifier()?,
                byte if byte.is_ascii() => {
                    self.index += 1;
                    self.push(TokenKind::Punctuator(char::from(byte)), self.index - 1)?;
                }
                _ => self.scan_identifier()?,
            }
        }
        if stop_at_substitution_end {
            return Err(bad("unterminated template substitution"));
        }
        self.depth -= 1;
        Ok(())
    }

    fn scan_identifier(&mut self) -> Result<(), ShadowError> {
        let start = self.index;
        let mut chars = self.source[start..].chars();
        let Some(first) = chars.next() else {
            return Err(bad("unexpected end of expression"));
        };
        if first == '\\' {
            return Err(bad("escaped identifiers are not supported"));
        }
        if !(first == '_' || first == '$' || first.is_alphabetic()) {
            return Err(bad(format!(
                "unsupported character `{}` in expression",
                first.escape_default()
            )));
        }
        self.index += first.len_utf8();
        for character in self.source[self.index..].chars() {
            let allowed = character == '_'
                || character == '$'
                || character.is_alphanumeric()
                || character == '\u{200C}'
                || character == '\u{200D}';
            if !allowed {
                break;
            }
            self.index += character.len_utf8();
        }
        self.push(TokenKind::Identifier, start)
    }

    fn scan_string(&mut self, quote: u8) -> Result<(), ShadowError> {
        let start = self.index;
        self.index += 1;
        while self.index < self.bytes.len() {
            let byte = self.bytes[self.index];
            if byte == b'\\' {
                self.skip_escape(true)?;
                continue;
            }
            if byte == b'\n' || byte == b'\r' {
                return Err(bad("unterminated string literal"));
            }
            self.index += 1;
            if byte == quote {
                self.push(TokenKind::StringLiteral, start)?;
                return Ok(());
            }
        }
        Err(bad("unterminated string literal"))
    }

    fn scan_template(&mut self) -> Result<(), ShadowError> {
        let start = self.index;
        self.index += 1;
        let mut chunk_start = start;
        loop {
            if self.index >= self.bytes.len() {
                return Err(bad("unterminated template literal"));
            }
            match self.bytes[self.index] {
                b'\\' => self.skip_escape(true)?,
                b'`' => {
                    self.index += 1;
                    self.push(TokenKind::TemplateChunk, chunk_start)?;
                    return Ok(());
                }
                b'$' if self.peek(1) == Some(b'{') => {
                    let chunk_end = self.index;
                    if chunk_end > chunk_start {
                        self.push_token_at(TokenKind::TemplateChunk, chunk_start, chunk_end)?;
                    }
                    self.index += 2;
                    self.push_token_at(TokenKind::TemplateExpressionStart, chunk_end, self.index)?;
                    self.scan_code(true)?;
                    chunk_start = self.index;
                }
                _ => {
                    let character = self.source[self.index..]
                        .chars()
                        .next()
                        .ok_or_else(|| bad("unterminated template literal"))?;
                    self.index += character.len_utf8();
                }
            }
        }
    }

    fn scan_number(&mut self) -> Result<(), ShadowError> {
        let start = self.index;
        let bytes = self.bytes;
        if bytes[self.index] == b'0'
            && let Some(radix) = self.peek(1)
        {
            let lower = radix.to_ascii_lowercase();
            if lower == b'x' || lower == b'b' || lower == b'o' {
                self.index += 2;
                while self.index < bytes.len()
                    && (bytes[self.index].is_ascii_hexdigit() || bytes[self.index] == b'_')
                {
                    self.index += 1;
                }
                if self.index < bytes.len() && bytes[self.index] == b'n' {
                    self.index += 1;
                }
                self.push(TokenKind::Number, start)?;
                return Ok(());
            }
        }
        while self.index < bytes.len()
            && (bytes[self.index].is_ascii_digit() || bytes[self.index] == b'_')
        {
            self.index += 1;
        }
        if self.index < bytes.len() && bytes[self.index] == b'.' {
            self.index += 1;
            while self.index < bytes.len()
                && (bytes[self.index].is_ascii_digit() || bytes[self.index] == b'_')
            {
                self.index += 1;
            }
        }
        if self.index < bytes.len() && (bytes[self.index] == b'e' || bytes[self.index] == b'E') {
            let exponent_start = self.index;
            self.index += 1;
            if self.index < bytes.len() && (bytes[self.index] == b'+' || bytes[self.index] == b'-')
            {
                self.index += 1;
            }
            if self.index < bytes.len() && bytes[self.index].is_ascii_digit() {
                while self.index < bytes.len()
                    && (bytes[self.index].is_ascii_digit() || bytes[self.index] == b'_')
                {
                    self.index += 1;
                }
            } else {
                self.index = exponent_start;
            }
        }
        if self.index < bytes.len() && bytes[self.index] == b'n' {
            self.index += 1;
        }
        self.push(TokenKind::Number, start)
    }

    fn scan_regex(&mut self) -> Result<(), ShadowError> {
        let start = self.index;
        self.index += 1;
        let mut in_class = false;
        while self.index < self.bytes.len() {
            let byte = self.bytes[self.index];
            if byte == b'\\' {
                self.skip_escape(false)?;
                continue;
            }
            if byte == b'\n' || byte == b'\r' {
                return Err(bad("unterminated regular expression literal"));
            }
            self.index += 1;
            match byte {
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => {
                    while self.index < self.bytes.len()
                        && self.bytes[self.index].is_ascii_alphanumeric()
                    {
                        self.index += 1;
                    }
                    self.push(TokenKind::Regex, start)?;
                    return Ok(());
                }
                _ => {}
            }
        }
        Err(bad("unterminated regular expression literal"))
    }

    fn skip_escape(&mut self, allow_line_continuation: bool) -> Result<(), ShadowError> {
        self.index += 1; // backslash
        if self.index >= self.bytes.len() {
            return Err(bad("unterminated escape sequence"));
        }
        match self.bytes[self.index] {
            b'\r' if allow_line_continuation => {
                self.index += 1;
                if self.peek(0) == Some(b'\n') {
                    self.index += 1;
                }
                Ok(())
            }
            b'\n' if allow_line_continuation => {
                self.index += 1;
                Ok(())
            }
            b'\n' | b'\r' => Err(bad("unterminated escape sequence")),
            _ => {
                let character = self.source[self.index..]
                    .chars()
                    .next()
                    .ok_or_else(|| bad("unterminated escape sequence"))?;
                self.index += character.len_utf8();
                Ok(())
            }
        }
    }

    const fn skip_line_comment(&mut self) {
        while self.index < self.bytes.len() && self.bytes[self.index] != b'\n' {
            self.index += 1;
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), ShadowError> {
        self.index += 2;
        while self.index + 1 < self.bytes.len() {
            if self.bytes[self.index] == b'*' && self.bytes[self.index + 1] == b'/' {
                self.index += 2;
                return Ok(());
            }
            self.index += 1;
        }
        Err(bad("unterminated block comment"))
    }

    fn regex_allowed(&self) -> bool {
        let Some(previous) = self.tokens.last() else {
            return true;
        };
        match previous.kind {
            TokenKind::Identifier => matches!(
                previous.text(self.source),
                "return"
                    | "typeof"
                    | "instanceof"
                    | "in"
                    | "of"
                    | "new"
                    | "delete"
                    | "void"
                    | "case"
                    | "do"
                    | "else"
                    | "yield"
                    | "await"
            ),
            TokenKind::Punctuator(character) => matches!(
                character,
                '(' | '['
                    | '{'
                    | ','
                    | ';'
                    | ':'
                    | '='
                    | '!'
                    | '?'
                    | '&'
                    | '|'
                    | '+'
                    | '-'
                    | '*'
                    | '%'
                    | '~'
                    | '<'
                    | '>'
                    | '^'
            ),
            TokenKind::TemplateExpressionStart => true,
            TokenKind::StringLiteral
            | TokenKind::Number
            | TokenKind::Regex
            | TokenKind::TemplateChunk
            | TokenKind::TemplateExpressionEnd => false,
        }
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.index + offset).copied()
    }
}

/// Matching close token index for every open delimiter.
/// Matches every open delimiter/template-substitution with its closer.
///
/// Returns a table where both directions carry the paired token index. The
/// result is used by expression walking and by the script scanner.
pub fn matching_pairs(tokens: &[Token]) -> Result<Vec<Option<usize>>, ShadowError> {
    matching_pairs_with_limits(
        tokens,
        MAX_EXPRESSION_NODES,
        MAX_EXPRESSION_DEPTH,
        "expression.tokens",
    )
}

/// Matches delimiters in a whole script.
///
/// Scripts are much larger than WXML expressions, so the expression node budget
/// does not apply. The depth bound still prevents pathological nesting.
pub fn matching_pairs_for_script(tokens: &[Token]) -> Result<Vec<Option<usize>>, ShadowError> {
    matching_pairs_with_limits(tokens, MAX_SCRIPT_TOKENS, MAX_SCRIPT_DEPTH, "script.tokens")
}

fn matching_pairs_with_limits(
    tokens: &[Token],
    max_nodes: usize,
    max_depth: usize,
    token_budget: &'static str,
) -> Result<Vec<Option<usize>>, ShadowError> {
    if tokens.len() > max_nodes {
        return Err(ShadowError::BudgetExceeded {
            budget: token_budget,
        });
    }
    let mut pairs = vec![None; tokens.len()];
    let mut stack: Vec<(usize, TokenKind)> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Punctuator('(' | '[' | '{') | TokenKind::TemplateExpressionStart => {
                stack.push((index, token.kind));
                if stack.len() > max_depth {
                    return Err(bad("expression exceeds traversal depth limits"));
                }
            }
            TokenKind::Punctuator(')' | ']' | '}') | TokenKind::TemplateExpressionEnd => {
                let expected = match token.kind {
                    TokenKind::Punctuator(')') => TokenKind::Punctuator('('),
                    TokenKind::Punctuator(']') => TokenKind::Punctuator('['),
                    TokenKind::Punctuator('}') => TokenKind::Punctuator('{'),
                    _ => TokenKind::TemplateExpressionStart,
                };
                let Some((open_index, open_kind)) = stack.pop() else {
                    return Err(bad("unbalanced closing delimiter"));
                };
                if open_kind != expected {
                    return Err(bad("mismatched delimiter"));
                }
                pairs[open_index] = Some(index);
                pairs[index] = Some(open_index);
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(bad("unbalanced opening delimiter"));
    }
    Ok(pairs)
}

fn validate_expression(source: &str, tokens: &[Token]) -> Result<Vec<Option<usize>>, ShadowError> {
    if tokens.is_empty() {
        return Err(bad("empty expression"));
    }
    if is_invalid_start(tokens[0].kind) {
        return Err(bad("expression starts with an operator"));
    }
    if is_invalid_end(tokens[tokens.len() - 1].kind) {
        return Err(bad("expression ends with an operator"));
    }
    let pairs = matching_pairs(tokens)?;
    for index in 0..tokens.len() {
        validate_token_shape(source, tokens, index)?;
    }
    validate_adjacent_values(source, tokens)?;
    Ok(pairs)
}

const fn is_invalid_start(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Punctuator(character) => {
            !matches!(character, '(' | '[' | '{' | '!' | '-' | '+' | '~')
        }
        TokenKind::TemplateExpressionStart => true,
        TokenKind::TemplateExpressionEnd
        | TokenKind::Identifier
        | TokenKind::StringLiteral
        | TokenKind::Number
        | TokenKind::Regex
        | TokenKind::TemplateChunk => false,
    }
}

const fn is_invalid_end(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Punctuator(character) => !matches!(character, ')' | ']' | '}'),
        TokenKind::TemplateExpressionStart => true,
        TokenKind::TemplateExpressionEnd
        | TokenKind::Identifier
        | TokenKind::StringLiteral
        | TokenKind::Number
        | TokenKind::Regex
        | TokenKind::TemplateChunk => false,
    }
}

#[allow(clippy::too_many_lines)] // one token grammar; splitting obscures the operator rules
fn validate_token_shape(source: &str, tokens: &[Token], index: usize) -> Result<(), ShadowError> {
    let token = tokens[index];
    match token.kind {
        TokenKind::Punctuator(';') => Err(bad("statement terminators are not expressions")),
        TokenKind::Punctuator('=') => {
            let next = tokens.get(index + 1).copied();
            let previous = index.checked_sub(1).map(|previous| tokens[previous]);
            let adjacent_next = next.is_some_and(|next| next.start == token.end);
            let adjacent_previous = previous.is_some_and(|previous| previous.end == token.start);
            let previous_previous = index.checked_sub(2).map(|previous| tokens[previous]);
            let shift_assignment = adjacent_previous
                && previous.is_some_and(|previous| {
                    matches!(previous.kind, TokenKind::Punctuator('<' | '>'))
                })
                && previous_previous.is_some_and(|previous| {
                    previous.end == tokens[index - 1].start
                        && matches!(previous.kind, TokenKind::Punctuator('<' | '>'))
                });
            if shift_assignment {
                return Err(bad("assignments are not supported"));
            }
            let next_is = |character| {
                adjacent_next
                    && next.is_some_and(|next| next.kind == TokenKind::Punctuator(character))
            };
            if next_is('>') {
                return Err(bad("arrow functions are not supported"));
            }
            if next_is('=') {
                return Ok(());
            }
            let comparison = adjacent_previous
                && previous.is_some_and(|previous| {
                    matches!(previous.kind, TokenKind::Punctuator('=' | '!' | '<' | '>'))
                });
            if comparison {
                return Ok(());
            }
            Err(bad("assignments are not supported"))
        }
        TokenKind::Punctuator('+') => {
            let next = tokens.get(index + 1).copied();
            if next.is_some_and(|next| {
                next.start == token.end && next.kind == TokenKind::Punctuator('+')
            }) {
                return Err(bad("update expressions are not supported"));
            }
            Ok(())
        }
        TokenKind::Punctuator('-') => {
            let next = tokens.get(index + 1).copied();
            if next.is_some_and(|next| {
                next.start == token.end && next.kind == TokenKind::Punctuator('-')
            }) {
                return Err(bad("update expressions are not supported"));
            }
            Ok(())
        }
        TokenKind::Punctuator('<') => {
            if looks_like_type_syntax(source, tokens, index) {
                return Err(bad("type syntax is not supported in WXML expressions"));
            }
            Ok(())
        }
        TokenKind::Punctuator('!') => {
            let previous = index.checked_sub(1).map(|previous| tokens[previous]);
            let next = tokens.get(index + 1).copied();
            let adjacent_previous = previous.is_some_and(|previous| previous.end == token.start);
            let postfix = adjacent_previous
                && previous.is_some_and(|previous| {
                    matches!(
                        previous.kind,
                        TokenKind::Identifier
                            | TokenKind::Number
                            | TokenKind::StringLiteral
                            | TokenKind::Regex
                            | TokenKind::TemplateChunk
                            | TokenKind::TemplateExpressionEnd
                            | TokenKind::Punctuator(')' | ']' | '}')
                    )
                });
            let comparison = next.is_some_and(|next| {
                next.start == token.end && next.kind == TokenKind::Punctuator('=')
            });
            if postfix && !comparison {
                return Err(bad(
                    "non-null assertions are not supported in WXML expressions",
                ));
            }
            Ok(())
        }
        TokenKind::Identifier => {
            let name = token.text(source);
            let member_or_key = is_member_or_object_key(tokens, index);
            if matches!(name, "as" | "satisfies") {
                if !member_or_key && is_type_operator_position(source, tokens, index) {
                    return Err(bad(format!(
                        "type operator `{name}` is not supported in WXML expressions"
                    )));
                }
                return Ok(());
            }
            if !member_or_key && is_unsupported_keyword(name) {
                return Err(bad(format!("expression shape is not supported (`{name}`)")));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn is_member_or_object_key(tokens: &[Token], index: usize) -> bool {
    let member = index
        .checked_sub(1)
        .is_some_and(|previous| tokens[previous].kind == TokenKind::Punctuator('.'));
    let key = tokens
        .get(index + 1)
        .is_some_and(|next| next.kind == TokenKind::Punctuator(':'));
    member || key
}

fn is_type_operator_position(source: &str, tokens: &[Token], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).map(|previous| tokens[previous]) else {
        return false;
    };
    if previous.kind == TokenKind::Punctuator('.') {
        return false;
    }
    can_end_expression(source, previous)
}

fn can_end_expression(source: &str, token: Token) -> bool {
    match token.kind {
        TokenKind::Identifier => {
            let name = token.text(source);
            !matches!(
                name,
                "typeof"
                    | "void"
                    | "new"
                    | "delete"
                    | "await"
                    | "yield"
                    | "in"
                    | "instanceof"
                    | "of"
                    | "as"
                    | "satisfies"
                    | "return"
                    | "case"
                    | "do"
                    | "else"
            )
        }
        TokenKind::Number
        | TokenKind::StringLiteral
        | TokenKind::Regex
        | TokenKind::TemplateChunk
        | TokenKind::TemplateExpressionEnd
        | TokenKind::Punctuator(')' | ']' | '}') => true,
        _ => false,
    }
}

fn validate_adjacent_values(source: &str, tokens: &[Token]) -> Result<(), ShadowError> {
    for window in tokens.windows(2) {
        if can_end_expression(source, window[0]) && starts_bare_value(window[1], source) {
            return Err(bad(format!(
                "adjacent values are not an expression: {}",
                describe_expression(source)
            )));
        }
    }
    Ok(())
}

fn starts_bare_value(token: Token, source: &str) -> bool {
    match token.kind {
        TokenKind::Identifier => {
            let name = token.text(source);
            !matches!(
                name,
                "in" | "instanceof"
                    | "of"
                    | "as"
                    | "satisfies"
                    | "typeof"
                    | "void"
                    | "new"
                    | "delete"
                    | "await"
                    | "yield"
                    | "function"
                    | "class"
            )
        }
        TokenKind::Number
        | TokenKind::StringLiteral
        | TokenKind::Regex
        | TokenKind::Punctuator('{') => true,
        _ => false,
    }
}

/// Conservative detection of `<T>expr` assertions and `foo<T>()` calls.
///
/// A plain comparison such as `a < b` is only rejected when the `<` directly
/// follows an identifier and a matching `>` is followed by a call, or when the
/// `<` starts an operand (the type-assertion position).
#[allow(clippy::match_same_arms)] // keep type-syntax arms separate for readability
fn looks_like_type_syntax(source: &str, tokens: &[Token], index: usize) -> bool {
    let next = tokens.get(index + 1);
    if next.is_none_or(|next| next.kind != TokenKind::Identifier) {
        return false;
    }
    let previous = index.checked_sub(1).map(|previous| tokens[previous]);
    let attached_to_identifier = previous.is_some_and(|previous| {
        previous.kind == TokenKind::Identifier && previous.end == tokens[index].start
    });
    let operand_position = previous.is_none_or(|previous| match previous.kind {
        TokenKind::Punctuator(character) => matches!(
            character,
            '(' | '['
                | '{'
                | ','
                | ';'
                | ':'
                | '='
                | '!'
                | '?'
                | '&'
                | '|'
                | '+'
                | '-'
                | '*'
                | '/'
                | '%'
                | '^'
                | '~'
                | '<'
                | '>'
        ),
        TokenKind::Identifier => matches!(
            previous.text(source),
            "return" | "typeof" | "void" | "in" | "of" | "instanceof" | "case" | "yield" | "await"
        ),
        _ => false,
    });
    if !attached_to_identifier && !operand_position {
        return false;
    }
    let mut angle_depth = 0_usize;
    let mut cursor = index + 1;
    let stop = (index + 16).min(tokens.len());
    while cursor < stop {
        match tokens[cursor].kind {
            TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number => {}
            TokenKind::Punctuator('<') => angle_depth += 1,
            TokenKind::Punctuator('>') => {
                if angle_depth == 0 {
                    if attached_to_identifier {
                        return tokens
                            .get(cursor + 1)
                            .is_some_and(|token| token.kind == TokenKind::Punctuator('('));
                    }
                    return operand_position;
                }
                angle_depth -= 1;
            }
            TokenKind::Punctuator('.' | ',' | '[' | ']' | '{' | '}' | ':' | '?') => {}
            TokenKind::Punctuator('&' | '|') => {
                let next = tokens.get(cursor + 1).copied();
                if next.is_some_and(|next| {
                    next.start == tokens[cursor].end && next.kind == tokens[cursor].kind
                }) {
                    return false;
                }
            }
            TokenKind::Punctuator('(' | ')' | ';') if angle_depth == 0 => return false,
            _ => return false,
        }
        cursor += 1;
    }
    false
}

struct Walker<'a> {
    source: &'a str,
    tokens: &'a [Token],
    pairs: &'a [Option<usize>],
    collected: CollectedIdentifiers,
}

impl Walker<'_> {
    fn walk(&mut self, start: usize, end: usize) -> Result<(), ShadowError> {
        let mut index = start;
        while index < end {
            match self.tokens[index].kind {
                TokenKind::TemplateExpressionStart | TokenKind::Punctuator('(' | '[') => {
                    let close = self.close_of(index)?;
                    self.walk(index + 1, close)?;
                    index = close + 1;
                }
                TokenKind::Punctuator('{') => {
                    let close = self.close_of(index)?;
                    self.walk_object(index + 1, close)?;
                    index = close + 1;
                }
                TokenKind::Punctuator('.')
                    if index + 2 < end
                        && self.tokens[index + 1].kind == TokenKind::Punctuator('.')
                        && self.tokens[index + 2].kind == TokenKind::Punctuator('.') =>
                {
                    index += 3;
                }
                TokenKind::Punctuator('.') => {
                    if index + 1 < end && self.tokens[index + 1].kind == TokenKind::Identifier {
                        self.push_value(index + 1, false);
                        self.push_all(index + 1);
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                TokenKind::Identifier => {
                    if !is_non_value_keyword(self.tokens[index].text(self.source)) {
                        self.push_value(index, false);
                        self.push_all(index);
                        self.collected.roots.push(ValueIdentifier {
                            start: self.tokens[index].start,
                            end: self.tokens[index].end,
                            shorthand: false,
                        });
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        Ok(())
    }

    fn walk_object(&mut self, start: usize, end: usize) -> Result<(), ShadowError> {
        let mut index = start;
        while index < end {
            match self.tokens[index].kind {
                TokenKind::Punctuator(',') => {
                    index += 1;
                }
                TokenKind::Punctuator('.')
                    if index + 2 < end
                        && self.tokens[index + 1].kind == TokenKind::Punctuator('.')
                        && self.tokens[index + 2].kind == TokenKind::Punctuator('.') =>
                {
                    let (value_start, value_end) = self.value_range(index + 3, end);
                    self.walk(value_start, value_end)?;
                    index = value_end.max(index + 3);
                }
                TokenKind::Punctuator('[') => {
                    let close = self.close_of(index)?;
                    self.walk(index + 1, close)?;
                    index = close + 1;
                    if index < end && self.tokens[index].kind == TokenKind::Punctuator(':') {
                        let (value_start, value_end) = self.value_range(index + 1, end);
                        self.walk(value_start, value_end)?;
                        index = value_end.max(index + 1);
                    }
                }
                TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number => {
                    let key = index;
                    if self.tokens[key].kind == TokenKind::Identifier
                        && !is_non_value_keyword(self.tokens[key].text(self.source))
                    {
                        self.push_all(key);
                    }
                    let next = index + 1;
                    if next < end && self.tokens[next].kind == TokenKind::Punctuator(':') {
                        let (value_start, value_end) = self.value_range(next + 1, end);
                        self.walk(value_start, value_end)?;
                        index = value_end.max(next + 1);
                        continue;
                    }
                    if next < end && self.tokens[next].kind == TokenKind::Punctuator('(') {
                        return Err(bad("object methods are not supported in WXML expressions"));
                    }
                    if self.tokens[key].kind == TokenKind::Identifier
                        && matches!(self.tokens[key].text(self.source), "get" | "set")
                        && next < end
                        && self.tokens[next].kind == TokenKind::Identifier
                        && next + 1 < end
                        && self.tokens[next + 1].kind == TokenKind::Punctuator('(')
                    {
                        return Err(bad(
                            "object accessors are not supported in WXML expressions",
                        ));
                    }
                    if self.tokens[key].kind == TokenKind::Identifier
                        && !is_non_value_keyword(self.tokens[key].text(self.source))
                    {
                        self.push_value(key, true);
                        self.push_all(key);
                        self.collected.roots.push(ValueIdentifier {
                            start: self.tokens[key].start,
                            end: self.tokens[key].end,
                            shorthand: true,
                        });
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        Ok(())
    }

    const fn value_range(&self, start: usize, end: usize) -> (usize, usize) {
        let mut index = start;
        let mut nesting = 0_usize;
        while index < end {
            match self.tokens[index].kind {
                TokenKind::TemplateExpressionStart | TokenKind::Punctuator('(' | '[' | '{') => {
                    nesting += 1;
                }
                TokenKind::TemplateExpressionEnd | TokenKind::Punctuator(')' | ']' | '}') => {
                    if nesting == 0 {
                        break;
                    }
                    nesting -= 1;
                }
                TokenKind::Punctuator(',') if nesting == 0 => break,
                _ => {}
            }
            index += 1;
        }
        (start, index)
    }

    fn push_value(&mut self, index: usize, shorthand: bool) {
        let token = self.tokens[index];
        self.collected.values.push(ValueIdentifier {
            start: token.start,
            end: token.end,
            shorthand,
        });
    }

    fn push_all(&mut self, index: usize) {
        let token = self.tokens[index];
        self.collected.all.push(ValueIdentifier {
            start: token.start,
            end: token.end,
            shorthand: false,
        });
    }

    fn close_of(&self, index: usize) -> Result<usize, ShadowError> {
        self.pairs[index].ok_or_else(|| bad("unbalanced delimiter"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::rewrite::{RewriteOptions, rewrite_expression};

    fn roots(expr: &str) -> Vec<&str> {
        let collected = collect_value_identifiers(expr).expect("valid expression");
        collected
            .roots
            .iter()
            .map(|identifier| &expr[identifier.start..identifier.end])
            .collect()
    }

    #[test]
    fn member_names_keys_strings_and_globals_are_not_roots() {
        assert_eq!(roots("user.name"), ["user"]);
        assert_eq!(roots("{ title: user.name, user }"), ["user", "user"]);
        assert_eq!(roots("Math.max(title)"), ["Math", "title"]);
        assert_eq!(roots("user?.name"), ["user"]);
        assert_eq!(roots("format(user.name)"), ["format", "user"]);
        assert_eq!(roots("'\"name\"'"), Vec::<&str>::new());
    }

    #[test]
    fn shorthand_is_reported() {
        let collected = collect_value_identifiers("{ title }").expect("valid");
        assert_eq!(collected.roots.len(), 1);
        assert!(collected.roots[0].shorthand);
        assert_eq!(collected.values.len(), 1);
    }

    #[test]
    fn template_substitutions_are_roots_but_template_text_is_not() {
        assert_eq!(roots("`title ${title}`"), ["title"]);
        assert_eq!(roots("`a ${user.name} b`"), ["user"]);
    }

    #[test]
    fn unsupported_shapes_fail_closed() {
        for expr in [
            "new Foo()",
            "title satisfies string",
            "foo<Bar>()",
            "title as string",
            "title); (user",
            "title) + (user",
            "title);",
            "a = b",
            "a += b",
            "a!.b",
            "a!",
            "a++",
            "--a",
            "x => x",
            "function () {}",
            "class {}",
            "{ m() {} }",
        ] {
            assert!(
                collect_value_identifiers(expr).is_err(),
                "{expr:?} must fail closed"
            );
        }
        assert!(collect_value_identifiers("title").is_ok());
        assert!(collect_value_identifiers("1 + 2").is_ok());
    }

    #[test]
    fn deep_expressions_are_bounded_not_panicking() {
        let at_limit = format!("{}title{}", "(".repeat(256), ")".repeat(256));
        assert!(collect_value_identifiers(&at_limit).is_ok());
        let over_limit = format!("{}title{}", "(".repeat(257), ")".repeat(257));
        assert!(matches!(
            collect_value_identifiers(&over_limit),
            Err(ShadowError::BadExpression { .. })
        ));
    }

    #[test]
    fn huge_expression_descriptions_are_truncated() {
        let expr = "(".repeat(5000);
        let error = collect_value_identifiers(&expr).expect_err("must fail");
        let message = error.to_string();
        assert!(message.len() < 300, "message was {} chars", message.len());
        assert!(message.contains("total 5000 UTF-16 chars"));
    }

    #[test]
    fn template_escapes_never_split_utf8_boundaries() {
        for expression in [r"`\你`", r"`\é`", r"`C:\用户\${name}`", r"`a\😀b`"] {
            assert!(
                lex_expression(expression).is_ok(),
                "escaped template must lex: {expression:?}"
            );
            let _ = collect_value_identifiers(expression);
        }
        assert!(lex_expression(r"`abc\").is_err());
        assert!(lex_expression(r"'abc\").is_err());
    }

    #[test]
    fn template_text_fuzz_never_panics() {
        const ALPHABET: [char; 14] = [
            'a', '$', '{', '}', '`', '\\', '你', 'é', '😀', '\n', '\'', '"', '/', '*',
        ];
        let mut state = 0x1234_5678_9abc_def0_u64;
        for _ in 0..2_000 {
            let mut body = String::new();
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let length = usize::try_from((state >> 33) % 24).unwrap_or(0);
            for _ in 0..length {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let index = usize::try_from((state >> 33) % 14).unwrap_or(0);
                body.push(ALPHABET[index]);
            }
            let expression = format!("`{body}`");
            let _ = lex_expression(&expression);
            let _ = lex_script(&expression);
            let _ = collect_value_identifiers(&expression);
        }
    }

    #[test]
    fn keyword_like_names_are_allowed_in_value_key_and_member_positions() {
        assert_eq!(rewrite("item.type"), "__wxml_root.item.type");
        assert_eq!(rewrite("item.default"), "__wxml_root.item.default");
        assert_eq!(rewrite("type"), "__wxml_root.type");
        assert_eq!(rewrite("{ type: value }"), "{ type: __wxml_root.value }");
        assert_eq!(
            rewrite("{ default: value }"),
            "{ default: __wxml_root.value }"
        );
        assert!(collect_value_identifiers("{ type: 'x' }").is_ok());
        // No free root is still the oracle's separate `bad-fragment` rejection.
        assert!(rewrite_expression("{ type: 'x' }", None, &RewriteOptions::default()).is_err());
        assert!(rewrite_expression("title as string", None, &RewriteOptions::default()).is_err());
    }

    #[test]
    fn adjacent_bare_values_are_rejected() {
        for expression in ["a b", "1 2", "\"a\" \"b\"", "title type"] {
            assert!(
                rewrite_expression(expression, None, &RewriteOptions::default()).is_err(),
                "{expression:?} must be rejected"
            );
        }
        assert_eq!(rewrite("a in b"), "__wxml_root.a in __wxml_root.b");
        assert_eq!(
            rewrite("tag`x`"),
            "__wxml_root.tag`x`",
            "tagged templates stay valid"
        );
    }

    #[test]
    fn shift_assignments_are_rejected_but_shifts_are_allowed() {
        for expression in ["a <<= b", "a >>= b", "a >>>= b"] {
            assert!(
                rewrite_expression(expression, None, &RewriteOptions::default()).is_err(),
                "{expression:?} must be rejected"
            );
        }
        assert_eq!(rewrite("a << b"), "__wxml_root.a << __wxml_root.b");
    }

    #[test]
    fn comparison_chains_are_not_misread_as_type_arguments() {
        assert!(rewrite_expression("a < b && c > (d)", None, &RewriteOptions::default()).is_ok());
        assert!(rewrite_expression("a<b && c>(d)", None, &RewriteOptions::default()).is_ok());
        assert!(rewrite_expression("foo<Bar>()", None, &RewriteOptions::default()).is_err());
    }

    #[test]
    fn script_hashbang_is_skipped_and_script_tokens_are_bounded() {
        let source = "#!/usr/bin/env node\nPage({ data: {} });";
        let tokens = lex_script(source).expect("valid script");
        assert!(
            !tokens
                .iter()
                .any(|token| token.kind == TokenKind::Punctuator('#'))
        );
        assert!(tokens.iter().any(|token| token.text(source) == "Page"));

        let too_many = ",".repeat(MAX_SCRIPT_TOKENS + 1);
        assert!(matches!(
            lex_script(&too_many),
            Err(ShadowError::BudgetExceeded {
                budget: "script.tokens"
            })
        ));
    }

    fn rewrite(expr: &str) -> String {
        rewrite_expression(expr, None, &RewriteOptions::default())
            .expect("valid expression")
            .text
    }

    #[test]
    fn binding_names_reject_keywords_and_bridge_namespaces() {
        assert!(is_binding_identifier("item"));
        assert!(is_binding_identifier("type"));
        assert!(is_binding_identifier("中文"));
        assert!(!is_binding_identifier("class"));
        assert!(!is_binding_identifier("__wxml_item"));
        assert!(!is_binding_identifier("1item"));
    }
}
