//! Conservative WXSS/SCSS selector scanner.
//!
//! The scanner records only literal class selectors. It tracks comments,
//! strings, nested blocks and `&` concatenations that can be expanded from a
//! single-class parent. Anything that could generate classes dynamically
//! (interpolation, mixins, `@extend`, `@include`, control directives) sets the
//! incomplete flag instead of being guessed.

use std::sync::Arc;

use wxml_text::SourceRange;

use super::{
    ClassToken, MAX_CLASS_NAME_BYTES, StyleImport, StyleImportKind, StyleIssue, StyleIssueKind,
    StyleSheet,
};
use crate::files::ProjectPath;

/// Maximum expanded selectors produced for one nested selector.
const MAX_SELECTOR_EXPANSIONS: usize = 8;

/// Parses one style sheet.
#[must_use]
pub fn parse_style_sheet(path: &ProjectPath, text: &str) -> StyleSheet {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        path,
        sheet: StyleSheet::new(path),
        frames: Vec::new(),
        prelude_start: 0,
        pending_comment: None,
        paren_depth: 0,
    };
    parser.run();
    parser.sheet.bytes = text.len();
    parser.sheet.estimated_bytes = super::estimate_sheet_bytes(&parser.sheet);
    parser.sheet
}

/// Returns whether `byte` may appear in a class name.
pub(super) const fn is_class_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte >= 0x80
}

/// One nesting frame.
#[derive(Debug)]
struct Frame {
    /// Expanded selectors for a rule; `None` inherits the parent selectors for
    /// grouping at-rules such as `@media`.
    selectors: Option<Vec<String>>,
    /// Contents are not selector context (`@keyframes`, mixin bodies).
    skip: bool,
}

/// Parser state.
struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    path: &'a ProjectPath,
    sheet: StyleSheet,
    frames: Vec<Frame>,
    prelude_start: usize,
    pending_comment: Option<String>,
    paren_depth: usize,
}

impl Parser<'_> {
    /// Returns whether the extension enables SCSS line comments.
    fn is_scss(&self) -> bool {
        self.path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("scss"))
    }

    /// Returns the nearest rule's expanded selectors.
    fn current_selectors(&self) -> &[String] {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.selectors.as_deref())
            .unwrap_or(&[])
    }

    /// Returns whether any enclosing frame suppresses selector recording.
    fn in_skip_frame(&self) -> bool {
        self.frames.iter().any(|frame| frame.skip)
    }

    /// Marks the sheet incomplete once.
    fn mark_incomplete(&mut self, reason: impl Into<String>) {
        if self.sheet.complete {
            self.sheet.complete = false;
            self.sheet.reason = Some(reason.into());
        }
    }

    /// Records one issue.
    fn issue(&mut self, kind: StyleIssueKind, message: impl Into<String>) {
        self.sheet
            .issues
            .push(StyleIssue::new(self.path, kind, message));
    }

    /// Scans the whole sheet.
    fn run(&mut self) {
        let len = self.text.len();
        let mut index = 0_usize;
        while index < len {
            let byte = self.bytes[index];
            if starts_url_function(self.bytes, index) {
                if let Some(end) = skip_url_function(self.bytes, index, len) {
                    index = end;
                    continue;
                }
                self.mark_incomplete("unterminated url()");
                self.issue(
                    StyleIssueKind::Malformed,
                    "unterminated url(); the rest of the style file was not indexed",
                );
                break;
            }
            match byte {
                b'/' if self.bytes.get(index + 1) == Some(&b'*') => {
                    let end = self.block_comment_end(index);
                    self.handle_comment(index, end);
                    index = end;
                }
                b'/' if self.bytes.get(index + 1) == Some(&b'/')
                    && self.is_scss()
                    && self.paren_depth == 0 =>
                {
                    index = self.text[index..]
                        .find('\n')
                        .map_or(len, |offset| index + offset + 1);
                }
                b'\'' | b'"' => {
                    if let Some(next) = skip_string(self.bytes, index, len) {
                        index = next;
                    } else {
                        self.mark_incomplete("unterminated style string");
                        self.issue(
                            StyleIssueKind::Malformed,
                            "unterminated style string; the remainder of the line was skipped",
                        );
                        index = self.text[index..]
                            .find('\n')
                            .map_or(len, |offset| index + offset + 1);
                    }
                }
                b'#' if self.bytes.get(index + 1) == Some(&b'{') => {
                    self.mark_incomplete("dynamic SCSS interpolation is not expanded");
                    self.issue(
                        StyleIssueKind::DynamicConstruct,
                        "dynamic SCSS interpolation is not expanded",
                    );
                    index = skip_interpolation(self.bytes, index, len).unwrap_or(len);
                }
                b'(' => {
                    self.paren_depth = self.paren_depth.saturating_add(1);
                    index += 1;
                }
                b')' => {
                    self.paren_depth = self.paren_depth.saturating_sub(1);
                    index += 1;
                }
                b'{' if self.paren_depth == 0 => {
                    self.handle_block(index);
                    self.prelude_start = index + 1;
                    index += 1;
                }
                b'}' if self.paren_depth == 0 => {
                    if self.frames.pop().is_none() {
                        // Unbalanced input: ignore the stray brace.
                    }
                    self.pending_comment = None;
                    self.prelude_start = index + 1;
                    index += 1;
                }
                b';' => {
                    if self.paren_depth > 0 {
                        // Malformed parentheses: recover at the statement end.
                        self.paren_depth = 0;
                    }
                    self.handle_statement(index);
                    self.prelude_start = index + 1;
                    index += 1;
                }
                _ => index += 1,
            }
        }
    }

    /// Returns the end offset of a block comment starting at `start`.
    fn block_comment_end(&self, start: usize) -> usize {
        self.text[start + 2..]
            .find("*/")
            .map_or(self.text.len(), |offset| start + 2 + offset + 2)
    }

    /// Records a block comment that starts at a statement boundary.
    fn handle_comment(&mut self, start: usize, end: usize) {
        let boundary = self
            .text
            .get(self.prelude_start..start)
            .is_some_and(|text| text.trim().is_empty());
        if !boundary {
            return;
        }
        if self.text.len() >= start + 2 {
            let content_end = if self.text.get(end.saturating_sub(2)..end) == Some("*/") {
                end - 2
            } else {
                end
            };
            let comment = self
                .text
                .get(start + 2..content_end)
                .map_or_else(String::new, clean_comment);
            if !comment.is_empty() {
                self.pending_comment = Some(comment);
            }
        }
        self.prelude_start = end;
    }

    /// Handles a `;`-terminated statement.
    fn handle_statement(&mut self, end: usize) {
        let Some(prelude) = self.text.get(self.prelude_start..end) else {
            self.pending_comment = None;
            return;
        };
        let trimmed = prelude.trim();
        if trimmed.starts_with('@') {
            let lead = prelude.len().saturating_sub(prelude.trim_start().len());
            self.handle_at_statement(trimmed, self.prelude_start + lead);
        }
        self.pending_comment = None;
    }

    /// Parses an at-rule statement (`@import`, `@use`, `@extend`, ...).
    fn handle_at_statement(&mut self, statement: &str, offset: usize) {
        let name = at_rule_name(statement).to_ascii_lowercase();
        match name.as_str() {
            "import" => self.parse_imports(statement, offset, false),
            "use" | "forward" => self.parse_imports(statement, offset, true),
            "extend" => {
                self.mark_incomplete("SCSS @extend is not expanded");
                self.issue(
                    StyleIssueKind::DynamicConstruct,
                    "SCSS @extend is not expanded",
                );
            }
            "include" => {
                self.mark_incomplete("SCSS @include is not expanded");
                self.issue(
                    StyleIssueKind::DynamicConstruct,
                    "SCSS @include is not expanded",
                );
            }
            _ => {}
        }
    }

    /// Extracts quoted or `url()` specifiers from an import statement.
    fn parse_imports(&mut self, statement: &str, offset: usize, single: bool) {
        let bytes = statement.as_bytes();
        let mut index = 0_usize;
        let mut found = false;
        while index < bytes.len() {
            match bytes[index] {
                b'\'' | b'"' => {
                    let quote = bytes[index];
                    let start = index + 1;
                    let mut end = start;
                    while end < bytes.len() {
                        if bytes[end] == b'\\' {
                            end = end.saturating_add(2);
                            continue;
                        }
                        if bytes[end] == quote {
                            break;
                        }
                        end += 1;
                    }
                    if end >= bytes.len() {
                        break;
                    }
                    let specifier = &statement[start..end];
                    self.push_import(specifier, offset + start, offset + end);
                    found = true;
                    if single {
                        return;
                    }
                    index = end + 1;
                }
                b'u' | b'U'
                    if bytes
                        .get(index..index + 4)
                        .is_some_and(|slice| slice.eq_ignore_ascii_case(b"url(")) =>
                {
                    let start = index + 4;
                    let end = statement[start..]
                        .find(')')
                        .map_or(bytes.len(), |position| start + position);
                    let inner = statement[start..end].trim();
                    let inner = inner.trim_matches(['\'', '"']);
                    if !inner.is_empty() {
                        let inner_offset = statement[start..end]
                            .find(inner)
                            .map_or(start, |position| start + position);
                        self.push_import(
                            inner,
                            offset + inner_offset,
                            offset + inner_offset + inner.len(),
                        );
                        found = true;
                    }
                    index = end.saturating_add(1);
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    index = statement[index + 2..]
                        .find("*/")
                        .map_or(bytes.len(), |position| index + 2 + position + 2);
                }
                _ => index += 1,
            }
        }
        if !found {
            self.mark_incomplete("style import could not be parsed");
        }
    }

    /// Records one import and classifies remote URLs.
    fn push_import(&mut self, specifier: &str, start: usize, end: usize) {
        let trimmed = specifier.trim();
        if trimmed.is_empty() {
            return;
        }
        let (Some(start), Some(end)) = (u32::try_from(start).ok(), u32::try_from(end).ok()) else {
            return;
        };
        let Ok(range) = SourceRange::from_bounds(start, end) else {
            return;
        };
        let kind = if trimmed
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sass:"))
        {
            StyleImportKind::Builtin
        } else if crate::resources::is_remote_specifier(trimmed) {
            StyleImportKind::Remote
        } else {
            StyleImportKind::Local
        };
        self.sheet.imports.push(StyleImport {
            specifier: trimmed.to_owned(),
            range,
            kind,
        });
    }

    /// Handles a `{` block.
    fn handle_block(&mut self, open: usize) {
        let Some(prelude) = self.text.get(self.prelude_start..open) else {
            self.pending_comment = None;
            self.frames.push(Frame {
                selectors: Some(Vec::new()),
                skip: self.in_skip_frame(),
            });
            return;
        };
        let trimmed = prelude.trim();
        let lead = prelude.len().saturating_sub(prelude.trim_start().len());
        let offset = self.prelude_start + lead;
        if trimmed.is_empty() {
            self.pending_comment = None;
            self.frames.push(Frame {
                selectors: None,
                skip: self.in_skip_frame(),
            });
            return;
        }
        if trimmed.starts_with('@') {
            self.handle_at_block(trimmed, offset);
            self.pending_comment = None;
            return;
        }
        if self.in_skip_frame() {
            self.pending_comment = None;
            self.frames.push(Frame {
                selectors: Some(Vec::new()),
                skip: true,
            });
            return;
        }

        let base = self.current_selectors().to_vec();
        let mut expanded = Vec::new();
        let mut tokens = Vec::new();
        for (selector, relative) in split_selector_list(trimmed) {
            let (mut selectors, mut items) =
                expand_selector(selector, offset + relative, &base, &mut self.sheet);
            expanded.append(&mut selectors);
            tokens.append(&mut items);
        }
        if let Some(comment) = self.pending_comment.take() {
            for token in &mut tokens {
                if token.comment.is_none() {
                    token.comment = Some(comment.clone());
                }
            }
        }
        for token in tokens {
            self.push_class(token);
        }
        self.frames.push(Frame {
            selectors: Some(expanded),
            skip: false,
        });
    }

    /// Handles a block-opening at-rule.
    fn handle_at_block(&mut self, prelude: &str, offset: usize) {
        let name = at_rule_name(prelude).to_ascii_lowercase();
        match name.as_str() {
            "media" | "supports" | "container" | "layer" | "scope" | "starting-style"
            | "document" => {
                self.frames.push(Frame {
                    selectors: None,
                    skip: self.in_skip_frame(),
                });
            }
            "each" | "for" | "while" | "if" | "else" => {
                self.mark_incomplete("SCSS control directives are not expanded");
                self.issue(
                    StyleIssueKind::DynamicConstruct,
                    "SCSS control directives are not expanded",
                );
                self.frames.push(Frame {
                    selectors: None,
                    skip: self.in_skip_frame(),
                });
            }
            "at-root" => {
                if prelude.split_whitespace().count() <= 1 {
                    self.frames.push(Frame {
                        selectors: Some(Vec::new()),
                        skip: false,
                    });
                } else {
                    self.mark_incomplete("SCSS @at-root selectors are not expanded");
                    self.issue(
                        StyleIssueKind::DynamicConstruct,
                        "SCSS @at-root selectors are not expanded",
                    );
                    self.frames.push(Frame {
                        selectors: Some(Vec::new()),
                        skip: true,
                    });
                }
            }
            "mixin" | "function" => {
                self.mark_incomplete("SCSS mixins are not expanded");
                self.issue(
                    StyleIssueKind::DynamicConstruct,
                    "SCSS mixins are not expanded",
                );
                self.frames.push(Frame {
                    selectors: Some(Vec::new()),
                    skip: true,
                });
            }
            "keyframes" | "-webkit-keyframes" | "font-face" | "page" | "counter-style"
            | "property" | "viewport" => {
                self.frames.push(Frame {
                    selectors: Some(Vec::new()),
                    skip: true,
                });
            }
            _ => {
                let _ = offset;
                self.mark_incomplete("unsupported style at-rule is not expanded");
                self.issue(
                    StyleIssueKind::DynamicConstruct,
                    format!("unsupported at-rule `@{name}` is not expanded"),
                );
                self.frames.push(Frame {
                    selectors: Some(Vec::new()),
                    skip: true,
                });
            }
        }
    }

    /// Appends one class token.
    fn push_class(&mut self, token: ClassToken) {
        if token.name.is_empty() || token.name.len() > MAX_CLASS_NAME_BYTES {
            self.mark_incomplete("class name exceeds the index limit");
            return;
        }
        self.sheet.classes.push(Arc::new(token));
    }
}

/// Expands one selector item and extracts its class tokens.
fn expand_selector(
    selector: &str,
    offset: usize,
    base: &[String],
    sheet: &mut StyleSheet,
) -> (Vec<String>, Vec<ClassToken>) {
    let selector = selector.trim();
    if selector.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let selectors = if base.is_empty() {
        if contains_top_level_ampersand(selector) {
            if sheet.complete {
                sheet.complete = false;
                sheet.reason = Some("`&` has no parent selector".to_owned());
            }
            return (Vec::new(), Vec::new());
        }
        vec![normalize_selector(selector)]
    } else if contains_top_level_ampersand(selector) {
        let (expanded, truncated) = expand_parent_references(selector, base);
        if truncated {
            if sheet.complete {
                sheet.complete = false;
                sheet.reason = Some("selector expansion budget reached".to_owned());
            }
            sheet.issues.push(StyleIssue::new(
                &sheet.path,
                StyleIssueKind::Truncated,
                "selector expansion budget reached",
            ));
        }
        expanded
    } else {
        base.iter()
            .map(|parent| normalize_selector(&format!("{parent} {selector}")))
            .collect()
    };
    let display = selectors.join(", ");
    let mut tokens = Vec::new();
    scan_selector_classes(selector, offset, base, &display, &mut tokens, sheet);
    (selectors, tokens)
}

/// Expands `&` occurrences against parent selectors with a bounded product.
///
/// Only top-level `&` characters are replaced; ampersands inside strings,
/// comments, attribute selectors or pseudo arguments stay literal.
fn expand_parent_references(selector: &str, base: &[String]) -> (Vec<String>, bool) {
    let positions = top_level_ampersands(selector);
    if positions.is_empty() {
        return (vec![normalize_selector(selector)], false);
    }
    let mut results = vec![String::new()];
    let mut last = 0_usize;
    let mut truncated = false;
    for position in positions {
        let Some(text) = selector.get(last..position) else {
            return (Vec::new(), true);
        };
        let mut next = Vec::new();
        'expand: for prefix in &results {
            for parent in base {
                if next.len() >= MAX_SELECTOR_EXPANSIONS {
                    truncated = true;
                    break 'expand;
                }
                next.push(format!("{prefix}{text}{parent}"));
            }
        }
        results = next;
        last = position + 1;
    }
    if let Some(tail) = selector.get(last..) {
        for result in &mut results {
            result.push_str(tail);
        }
    }
    (
        results
            .into_iter()
            .map(|item| normalize_selector(&item))
            .collect(),
        truncated,
    )
}

/// Returns byte positions of `&` outside strings, comments, brackets and
/// parentheses.
fn top_level_ampersands(selector: &str) -> Vec<usize> {
    let bytes = selector.as_bytes();
    let mut positions = Vec::new();
    let mut index = 0_usize;
    let mut paren_depth = 0_usize;
    let mut bracket_depth = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                index = skip_string(bytes, index, bytes.len()).unwrap_or(bytes.len());
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = selector[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |position| index + 2 + position + 2);
            }
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'&' if paren_depth == 0 && bracket_depth == 0 => positions.push(index),
            _ => {}
        }
        index += 1;
    }
    positions
}

/// Scans one selector item for literal class tokens.
#[allow(clippy::too_many_lines)] // one linear selector scanner
fn scan_selector_classes(
    selector: &str,
    offset: usize,
    base: &[String],
    display: &str,
    tokens: &mut Vec<ClassToken>,
    sheet: &mut StyleSheet,
) {
    let bytes = selector.as_bytes();
    let mut index = 0_usize;
    let mut paren_depth = 0_usize;
    let mut bracket_depth = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = selector[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |position| index + 2 + position + 2);
            }
            b'\'' | b'"' => {
                if let Some(next) = skip_string(bytes, index, bytes.len()) {
                    index = next;
                } else {
                    if sheet.complete {
                        sheet.complete = false;
                        sheet.reason = Some("unterminated style string".to_owned());
                    }
                    return;
                }
            }
            b'\\' => {
                // Escaped identifiers are not normalized; skip the escape.
                index = index.saturating_add(2);
            }
            b'(' => {
                paren_depth = paren_depth.saturating_add(1);
                index += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                index += 1;
            }
            b'[' => {
                bracket_depth = bracket_depth.saturating_add(1);
                index += 1;
            }
            b']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                index += 1;
            }
            b'#' if bytes.get(index + 1) == Some(&b'{') => {
                index = skip_interpolation(bytes, index, bytes.len()).unwrap_or(bytes.len());
            }
            b'.' if paren_depth == 0 && bracket_depth == 0 => {
                let start = index + 1;
                let mut end = start;
                while end < bytes.len() && is_class_name_byte(bytes[end]) {
                    end += 1;
                }
                if end == start {
                    index += 1;
                    continue;
                }
                let dynamic = bytes.get(end) == Some(&b'#') && bytes.get(end + 1) == Some(&b'{');
                if dynamic {
                    if sheet.complete {
                        sheet.complete = false;
                        sheet.reason =
                            Some("dynamic class interpolation is not expanded".to_owned());
                    }
                    index = end;
                    continue;
                }
                if bytes.get(end) == Some(&b'\\') {
                    // Escaped identifiers are not normalized; claiming the
                    // prefix before the escape would be wrong.
                    index = end;
                    continue;
                }
                let name = &selector[start..end];
                let (Some(range_start), Some(range_end)) = (
                    u32::try_from(offset + start).ok(),
                    u32::try_from(offset + end).ok(),
                ) else {
                    index = end;
                    continue;
                };
                let Ok(range) = SourceRange::from_bounds(range_start, range_end) else {
                    index = end;
                    continue;
                };
                tokens.push(ClassToken {
                    name: name.to_owned(),
                    range,
                    selector: display.to_owned(),
                    comment: None,
                    rename_safe: true,
                });
                index = end;
            }
            b'&' if paren_depth == 0 && bracket_depth == 0 => {
                let at_compound_start = index == 0
                    || matches!(
                        bytes[index - 1],
                        b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'+' | b'~' | b','
                    );
                let suffix_start = index + 1;
                let mut end = suffix_start;
                while end < bytes.len() && is_class_name_byte(bytes[end]) {
                    end += 1;
                }
                if at_compound_start && end > suffix_start {
                    let suffix = &selector[suffix_start..end];
                    if let Some(names) = composite_names(base, suffix) {
                        let mut range_start = suffix_start;
                        while range_start < end && matches!(bytes[range_start], b'-' | b'_') {
                            range_start += 1;
                        }
                        let (Some(range_start), Some(range_end)) = (
                            u32::try_from(offset + range_start).ok(),
                            u32::try_from(offset + end).ok(),
                        ) else {
                            index = end;
                            continue;
                        };
                        if let Ok(range) = SourceRange::from_bounds(range_start, range_end) {
                            for name in names {
                                tokens.push(ClassToken {
                                    name,
                                    range,
                                    selector: display.to_owned(),
                                    comment: None,
                                    rename_safe: false,
                                });
                            }
                        }
                    } else if sheet.complete {
                        sheet.complete = false;
                        sheet.reason = Some("`&` concatenation could not be expanded".to_owned());
                    }
                }
                index = end;
            }
            _ => index += 1,
        }
    }
}

/// Builds `parent + suffix` class names for simple single-class parents.
fn composite_names(base: &[String], suffix: &str) -> Option<Vec<String>> {
    if suffix.contains('#') || suffix.contains('{') || suffix.contains('}') {
        return None;
    }
    if base.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for parent in base {
        let parent = parent.trim();
        let name = parent.strip_prefix('.')?;
        if name.is_empty() || !name.bytes().all(is_class_name_byte) {
            return None;
        }
        names.push(format!("{name}{suffix}"));
    }
    Some(names)
}

/// Returns whether a selector contains `&` outside strings, comments,
/// parentheses and attribute selectors.
fn contains_top_level_ampersand(selector: &str) -> bool {
    !top_level_ampersands(selector).is_empty()
}

/// Splits a selector list at top-level commas.
fn split_selector_list(text: &str) -> Vec<(&str, usize)> {
    let bytes = text.as_bytes();
    let mut items = Vec::new();
    let mut start = 0_usize;
    let mut index = 0_usize;
    let mut paren_depth = 0_usize;
    let mut bracket_depth = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' => {
                index = skip_string(bytes, index, bytes.len()).unwrap_or(bytes.len());
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = text[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |position| index + 2 + position + 2);
            }
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b',' if paren_depth == 0 && bracket_depth == 0 => {
                if let Some(item) = selector_item(text, start, index) {
                    items.push(item);
                }
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if let Some(item) = selector_item(text, start, text.len()) {
        items.push(item);
    }
    items
}

/// Trims one selector item and returns it with its relative offset.
fn selector_item(text: &str, start: usize, end: usize) -> Option<(&str, usize)> {
    let item = text.get(start..end)?;
    let trimmed = item.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lead = item.len().saturating_sub(item.trim_start().len());
    Some((trimmed, start + lead))
}

/// Returns the at-rule name without `@`.
fn at_rule_name(prelude: &str) -> &str {
    prelude
        .strip_prefix('@')
        .unwrap_or(prelude)
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '(' | '{' | ';' | ',')
        })
        .next()
        .unwrap_or("")
}

/// Returns whether `index` starts a case-insensitive `url(` function.
fn starts_url_function(bytes: &[u8], index: usize) -> bool {
    if index > 0 && is_identifier_continue(bytes[index - 1]) {
        return false;
    }
    bytes
        .get(index..index + 4)
        .is_some_and(|slice| slice.eq_ignore_ascii_case(b"url("))
}

/// Skips one `url(...)` function, including quoted strings and escapes.
fn skip_url_function(bytes: &[u8], index: usize, limit: usize) -> Option<usize> {
    let mut cursor = index + 4;
    let mut depth = 1_usize;
    while cursor < limit {
        match bytes[cursor] {
            b'\\' => cursor = cursor.saturating_add(2),
            b'\'' | b'"' => cursor = skip_string(bytes, cursor, limit)?,
            b'(' => {
                depth = depth.saturating_add(1);
                cursor += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor = bytes[cursor + 2..]
                    .windows(2)
                    .position(|window| window == b"*/")
                    .map_or(limit, |position| cursor + 2 + position + 2);
            }
            _ => cursor += 1,
        }
    }
    None
}

/// Returns whether a byte may continue an identifier.
const fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'-')
}

/// Skips a quoted string and returns the offset after it.
fn skip_string(bytes: &[u8], start: usize, limit: usize) -> Option<usize> {
    let quote = *bytes.get(start)?;
    let mut index = start + 1;
    while index < limit {
        match bytes[index] {
            b'\\' => index = index.saturating_add(2),
            byte if byte == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Skips `#{...}` and returns the offset after the closing brace.
fn skip_interpolation(bytes: &[u8], start: usize, limit: usize) -> Option<usize> {
    let mut index = start + 2;
    let mut depth = 1_usize;
    while index < limit {
        match bytes[index] {
            b'\'' | b'"' => index = skip_string(bytes, index, limit)?,
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

/// Collapses whitespace in a selector.
fn normalize_selector(selector: &str) -> String {
    selector.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cleans a block comment for class documentation.
fn clean_comment(comment: &str) -> String {
    let mut lines = Vec::new();
    for line in comment.lines() {
        let line = line.trim();
        let line = line.strip_prefix('*').unwrap_or(line).trim_start();
        if line.is_empty() && lines.is_empty() {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    fn parse(text: &str) -> StyleSheet {
        parse_style_sheet(&path("styles/test.scss"), text)
    }

    fn names(sheet: &StyleSheet) -> Vec<&str> {
        sheet
            .classes
            .iter()
            .map(|class| class.name.as_str())
            .collect()
    }

    #[test]
    fn parses_comma_and_compound_selectors() {
        let sheet =
            parse("/* card doc */\n.card, .panel .title { color: red; }\n.card.active { }\n");
        assert_eq!(
            names(&sheet),
            vec!["card", "panel", "title", "card", "active"]
        );
        assert_eq!(sheet.classes[0].comment.as_deref(), Some("card doc"));
        assert_eq!(sheet.classes[0].selector, ".card");
        assert_eq!(sheet.classes[2].selector, ".panel .title");
        assert!(sheet.complete);
    }

    #[test]
    fn expands_nested_selectors_and_ampersands() {
        let sheet = parse(
            r"
.card {
  &.active { color: red; }
  &-header { color: blue; }
  .title { color: green; }
  @media screen { &--wide { color: black; } }
}
",
        );
        assert_eq!(
            names(&sheet),
            vec!["card", "active", "card-header", "title", "card--wide"]
        );
        assert_eq!(sheet.classes[1].selector, ".card.active");
        assert_eq!(sheet.classes[2].selector, ".card-header");
        assert_eq!(sheet.classes[3].selector, ".card .title");
        assert_eq!(sheet.classes[4].selector, ".card--wide");
    }

    #[test]
    fn ignores_comments_strings_pseudo_arguments_and_keyframes() {
        let sheet = parse(
            r#"
/* .fake-comment */
.real[data-x=".fake-string"]:not(.fake-pseudo), .other { }
@keyframes spin { from { } to { } .fake-keyframe { } }
@font-face { font-family: ".fake-font"; }
"#,
        );
        assert_eq!(names(&sheet), vec!["real", "other"]);
    }

    #[test]
    fn marks_dynamic_scss_incomplete_without_claiming_classes() {
        let sheet = parse(
            r"
.#{$name} { color: red; }
.foo-#{$x} { color: red; }
@mixin card { .inside-mixin { } }
@include card;
@extend .base;
",
        );
        assert!(names(&sheet).is_empty(), "{:?}", names(&sheet));
        assert!(!sheet.complete);
        assert!(sheet.reason.is_some());
    }

    #[test]
    fn parses_static_imports_and_remote_urls() {
        let sheet = parse(
            r#"
@import "shared";
@use "./theme" as t;
@import url("https://example.com/a.css");
@forward 'tokens';
"#,
        );
        let imports: Vec<_> = sheet
            .imports
            .iter()
            .map(|import| (import.specifier.as_str(), import.kind))
            .collect();
        assert_eq!(
            imports,
            vec![
                ("shared", StyleImportKind::Local),
                ("./theme", StyleImportKind::Local),
                ("https://example.com/a.css", StyleImportKind::Remote),
                ("tokens", StyleImportKind::Local),
            ]
        );
    }

    #[test]
    fn records_utf8_and_crlf_ranges_exactly() {
        let text = "/* 中文 */\r\n.卡片,\r\n.other {\r\n}\r\n";
        let sheet = parse(text);
        assert_eq!(names(&sheet), vec!["卡片", "other"]);
        let card = &sheet.classes[0];
        let start = u32::try_from(text.find("卡片").expect("class")).expect("fits u32");
        assert_eq!(card.range.start(), start);
        assert_eq!(
            card.range.end(),
            start + u32::try_from("卡片".len()).expect("fits u32")
        );
    }

    #[test]
    fn skips_escaped_identifiers_and_placeholder_selectors() {
        let sheet = parse(r".foo\:bar { } %placeholder { } #identifier { }");
        assert!(names(&sheet).is_empty(), "{:?}", names(&sheet));
    }

    #[test]
    fn long_selector_lists_keep_every_class() {
        use std::fmt::Write as _;
        let mut text = String::new();
        for index in 0..300 {
            write!(text, ".c{index}, ").expect("selector");
        }
        text.push_str(".last { }");
        let sheet = parse(&text);
        assert!(sheet.complete, "{:?}", sheet.issues);
        assert_eq!(sheet.classes.len(), 301);
        assert!(names(&sheet).contains(&"c299"));
        assert!(names(&sheet).contains(&"last"));
    }

    #[test]
    fn selector_expansion_truncation_marks_the_sheet_incomplete() {
        let mut text = String::new();
        for index in 0..MAX_SELECTOR_EXPANSIONS {
            use std::fmt::Write as _;
            write!(text, ".c{index}, ").expect("selector");
        }
        text.push_str(".last { &-x { } }");
        let sheet = parse(&text);
        assert!(!sheet.complete);
        assert!(
            sheet
                .issues
                .iter()
                .any(|issue| issue.kind == StyleIssueKind::Truncated)
        );
    }

    #[test]
    fn selector_expansion_at_the_limit_stays_complete() {
        let mut text = String::new();
        for index in 0..(MAX_SELECTOR_EXPANSIONS - 1) {
            use std::fmt::Write as _;
            write!(text, ".c{index}, ").expect("selector");
        }
        text.push_str(".last { &-x { } }");
        let sheet = parse(&text);
        assert!(sheet.complete, "{:?}", sheet.reason);
    }

    #[test]
    fn twenty_thousand_unique_classes_parse() {
        use std::fmt::Write as _;
        let mut text = String::new();
        for index in 0..20_000 {
            writeln!(text, ".c{index} {{ color: red; }}").expect("class");
        }
        let sheet = parse_style_sheet(&path("large.scss"), &text);
        assert_eq!(sheet.classes.len(), 20_000);
        assert!(sheet.complete);
        assert_eq!(sheet.classes[0].name, "c0");
        assert_eq!(sheet.classes[19_999].name, "c19999");
    }

    #[test]
    fn malformed_comments_and_non_ascii_at_rules_do_not_panic() {
        let _ = parse("/* unterminated 中文");
        let sheet = parse("@中文 { .inside { } }\n.after { }\n");
        assert!(names(&sheet).contains(&"after"), "{:?}", names(&sheet));
        assert!(!sheet.complete);
    }

    #[test]
    fn use_with_only_imports_the_module() {
        let sheet = parse(
            r#"
@use "theme" with ($primary: "body");
@use "sass:math";
@forward "tokens" show $spacing;
"#,
        );
        let imports: Vec<_> = sheet
            .imports
            .iter()
            .map(|import| (import.specifier.as_str(), import.kind))
            .collect();
        assert_eq!(
            imports,
            vec![
                ("theme", StyleImportKind::Local),
                ("sass:math", StyleImportKind::Builtin),
                ("tokens", StyleImportKind::Local),
            ]
        );
        assert!(sheet.complete);
    }

    #[test]
    fn ampersand_composite_ranges_cover_only_the_suffix() {
        let text = ".card { &-header { } &:hover { } &[title=\"a&b\"] { } }";
        let sheet = parse(text);
        let header = sheet
            .classes
            .iter()
            .find(|class| class.name == "card-header")
            .expect("composite class");
        assert!(!header.rename_safe);
        assert_eq!(
            &text[header.range.start() as usize..header.range.end() as usize],
            "header"
        );
        assert!(
            sheet
                .classes
                .iter()
                .all(|class| !class.selector.contains("a.cardb")),
            "{:?}",
            sheet.classes
        );
    }

    #[test]
    fn unterminated_strings_mark_incomplete_and_recover_at_newline() {
        let sheet = parse(".a { content: \"oops\n } .b { }\n.c { }");
        assert!(!sheet.complete);
        assert!(
            sheet
                .issues
                .iter()
                .any(|issue| issue.kind == StyleIssueKind::Malformed)
        );
        assert!(names(&sheet).contains(&"c"), "{:?}", names(&sheet));
    }

    #[test]
    fn trailing_comment_does_not_document_the_next_rule() {
        let sheet = parse(".a { color: red; /* note */ }\n.b { }");
        assert_eq!(sheet.classes[0].comment, None);
        assert_eq!(sheet.classes[1].comment, None);
    }

    #[test]
    fn keeps_long_import_specifiers() {
        let long = "x".repeat(2_000);
        let sheet = parse(&format!("@import \"{long}\";\n.a {{ }}\n"));
        assert_eq!(sheet.imports.len(), 1);
        assert_eq!(sheet.imports[0].specifier, long);
        assert!(sheet.complete, "{:?}", sheet.issues);
        assert!(names(&sheet).contains(&"a"));
    }

    #[test]
    fn keeps_every_import() {
        use std::fmt::Write as _;
        let mut text = String::new();
        for index in 0..600 {
            writeln!(text, "@import \"m{index}\";").expect("import");
        }
        let sheet = parse(&text);
        assert_eq!(sheet.imports.len(), 600);
        assert!(sheet.complete, "{:?}", sheet.issues);
        assert_eq!(sheet.imports[599].specifier, "m599");
    }

    #[test]
    fn urls_do_not_start_scss_line_comments_or_rule_boundaries() {
        let sheet = parse(".a { background: url(https://cdn/x.png); }\n.b { color: red; }\n");
        assert_eq!(names(&sheet), vec!["a", "b"]);
        assert!(sheet.complete, "{:?}", sheet.issues);

        let sheet = parse(".a { background: url(//cdn/x.png); }\n.b { }");
        assert_eq!(names(&sheet), vec!["a", "b"]);

        let sheet = parse(".a { background: url(data:image/svg+xml;utf8,<svg>{}</svg>); }\n.b { }");
        assert_eq!(names(&sheet), vec!["a", "b"]);
        assert!(sheet.complete, "{:?}", sheet.issues);
    }

    #[test]
    fn remote_url_imports_are_recorded() {
        let sheet = parse("@import url(https://cdn/x.css);\n.a { }\n");
        assert_eq!(sheet.imports.len(), 1);
        assert_eq!(sheet.imports[0].kind, StyleImportKind::Remote);
        assert!(names(&sheet).contains(&"a"));
    }

    #[test]
    fn unterminated_url_is_malformed_and_incomplete() {
        let sheet = parse(".a { background: url(./bg.png; }\n.b { }\n");
        assert!(!sheet.complete);
        assert!(
            sheet
                .issues
                .iter()
                .any(|issue| issue.kind == StyleIssueKind::Malformed)
        );
    }

    #[test]
    fn strings_with_braces_do_not_confuse_nesting() {
        let sheet = parse(
            r#"
.a::before { content: "}"; }
.b { background: url("data:image/svg+xml;utf8,<svg>{}</svg>"); }
.c { }
"#,
        );
        assert_eq!(names(&sheet), vec!["a", "b", "c"]);
        assert!(sheet.complete);
    }
}
