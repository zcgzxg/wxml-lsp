//! Conservative, fidelity-preserving WXML formatting.
//!
//! The formatter never reprints the syntax tree. It only rewrites ASCII
//! whitespace *inside a start tag*: the gaps between the tag name, its
//! attributes and the closing `>`/`/>`. Text nodes, comments, entities,
//! mustaches, WXS raw bodies, attribute values, quotes, attribute order and
//! duplicate attributes are byte-for-byte preserved.
//!
//! Layout rules:
//!
//! * A start tag that stays on one line uses one space between attributes and
//!   no space before `>`/`/>`.
//! * A start tag that already spans multiple lines keeps one attribute per
//!   line, indented one unit more than the line that opens the tag. The
//!   closing delimiter stays attached to the last attribute.
//! * Multiline tags that start in the middle of a line are left alone: their
//!   continuation indentation cannot be derived safely.
//!
//! A start tag is only touched when it is complete (`>` or `/>` is present),
//! every attribute range is known and every gap is ASCII whitespace. Damaged
//! or unclosed regions therefore stay byte-for-byte unchanged while complete
//! siblings are still formatted. Range formatting only edits start tags whose
//! whole source range lies inside the requested range.

use serde_json::{Value, json};
use wxml_syntax::parser::{NodeKind, SyntaxTree};
use wxml_text::{SourceRange, TextRange};

use crate::jsonrpc::{self, Outgoing};
use crate::server::{Server, log};

/// Largest number of edits one formatting request may produce.
///
/// A request that would exceed this budget is answered with no edits instead
/// of a partial rewrite.
pub(crate) const MAX_FORMAT_EDITS: usize = 4_096;

/// Formatting options that affect attribute indentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FormatOptions {
    tab_size: usize,
    insert_spaces: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            tab_size: 2,
            insert_spaces: true,
        }
    }
}

impl FormatOptions {
    /// Creates options, clamping the tab size to a useful range.
    #[must_use]
    pub(crate) fn new(tab_size: usize, insert_spaces: bool) -> Self {
        Self {
            tab_size: tab_size.clamp(1, 16),
            insert_spaces,
        }
    }

    /// Returns the indentation unit for one nesting level.
    fn unit(self) -> String {
        if self.insert_spaces {
            " ".repeat(self.tab_size)
        } else {
            "\t".to_owned()
        }
    }
}

/// One minimal text edit replacing an intra-tag whitespace gap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FormatEdit {
    /// Byte range of the whitespace gap.
    pub(crate) range: TextRange,
    /// Replacement whitespace (possibly empty).
    pub(crate) text: String,
}

/// Formats the whole document.
pub(crate) fn format_document(text: &str, options: FormatOptions) -> Vec<FormatEdit> {
    format_scope(text, None, options)
}

/// Formats only start tags fully contained in `range`.
pub(crate) fn format_range(
    text: &str,
    range: TextRange,
    options: FormatOptions,
) -> Vec<FormatEdit> {
    format_scope(text, Some(range), options)
}

/// Shared formatting pass over an optional requested range.
fn format_scope(text: &str, scope: Option<TextRange>, options: FormatOptions) -> Vec<FormatEdit> {
    let parsed = wxml_syntax::parse(text);
    if parsed.is_truncated() {
        log(format_args!(
            "formatting skipped: parse was truncated by a resource budget"
        ));
        return Vec::new();
    }
    let tree = &parsed.tree;
    let mut edits = Vec::new();
    for id in tree.descendants(tree.root()) {
        if tree.kind(id) != Some(NodeKind::StartTag) {
            continue;
        }
        let Some(tag) = tree.text_range(id) else {
            continue;
        };
        if let Some(scope) = scope {
            if tag.start() < scope.start() || tag.end() > scope.end() {
                continue;
            }
        }
        let Some(tag_edits) = start_tag_edits(text, tree, id, options) else {
            continue;
        };
        if edits.len().saturating_add(tag_edits.len()) > MAX_FORMAT_EDITS {
            log(format_args!(
                "formatting skipped: more than {MAX_FORMAT_EDITS} edits would be required"
            ));
            return Vec::new();
        }
        edits.extend(tag_edits);
    }
    edits.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
    edits
}

/// Builds the gap edits for one complete start tag.
///
/// Returns `None` when the tag cannot be formatted safely.
fn start_tag_edits(
    text: &str,
    tree: &SyntaxTree,
    id: wxml_syntax::NodeId,
    options: FormatOptions,
) -> Option<Vec<FormatEdit>> {
    let tag = tree.text_range(id)?;
    let name = tree.name_range(id)?;
    let raw = text.get(tag.start() as usize..tag.end() as usize)?;
    if raw.contains('\r') && !raw.contains('\n') {
        // Lone-CR line endings are not part of the supported WXML layout;
        // normalizing them could change what the user sees.
        return None;
    }
    let delimiter_len = if raw.ends_with("/>") {
        2
    } else if raw.ends_with('>') {
        1
    } else {
        // An unterminated start tag is a damaged region: leave it untouched.
        return None;
    };
    let delimiter_start = tag.end().checked_sub(delimiter_len)?;
    if name.end() > delimiter_start {
        return None;
    }

    let mut attributes = Vec::new();
    for child in tree.children(id) {
        if tree.kind(*child) != Some(NodeKind::Attribute) {
            return None;
        }
        attributes.push(tree.text_range(*child)?);
    }
    let mut previous_end = name.end();
    for attribute in &attributes {
        if attribute.start() < previous_end || attribute.end() > delimiter_start {
            return None;
        }
        previous_end = attribute.end();
    }

    // Gaps between the tag name, each attribute and the closing delimiter.
    let mut gaps = Vec::with_capacity(attributes.len().saturating_add(1));
    let mut cursor = name.end();
    for attribute in &attributes {
        gaps.push(TextRange::new(cursor, attribute.start()).ok()?);
        cursor = attribute.end();
    }
    gaps.push(TextRange::new(cursor, delimiter_start).ok()?);

    for gap in &gaps {
        let gap_text = text.get(gap.start() as usize..gap.end() as usize)?;
        if !gap_text.bytes().all(|byte| byte.is_ascii_whitespace()) {
            return None;
        }
    }

    let multiline = raw.contains('\n');
    let line_start = text
        .get(..tag.start() as usize)?
        .rfind('\n')
        .map_or(0, |newline| newline.saturating_add(1));
    let line_prefix = text.get(line_start..tag.start() as usize)?;
    let inline = !line_prefix.bytes().all(|byte| byte.is_ascii_whitespace());
    if multiline && inline {
        // Continuation indentation of an inline multiline tag cannot be
        // derived without guessing the surrounding layout.
        return None;
    }
    let base = if inline { "" } else { line_prefix };
    let attribute_indent = if multiline {
        let unit = options.unit();
        format!("{base}{unit}")
    } else {
        String::new()
    };
    let newline = if raw.contains("\r\n") { "\r\n" } else { "\n" };

    let last = gaps.len().saturating_sub(1);
    let mut edits = Vec::new();
    for (index, gap) in gaps.iter().enumerate() {
        let replacement = if index == last || !multiline {
            if index == last {
                String::new()
            } else {
                " ".to_owned()
            }
        } else {
            format!("{newline}{attribute_indent}")
        };
        let current = text.get(gap.start() as usize..gap.end() as usize)?;
        if current != replacement {
            edits.push(FormatEdit {
                range: *gap,
                text: replacement,
            });
        }
    }
    Some(edits)
}

/// Reads formatting options from the request, falling back to the language
/// defaults for absent or malformed values.
fn request_options(params: &Value) -> FormatOptions {
    let Some(options) = params.get("options").and_then(Value::as_object) else {
        return FormatOptions::default();
    };
    let tab_size = options
        .get("tabSize")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(2);
    let insert_spaces = options
        .get("insertSpaces")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    FormatOptions::new(tab_size, insert_spaces)
}

/// Reads a request range and converts it to byte offsets.
///
/// A missing, malformed or out-of-bounds range returns `None`; the caller
/// answers with no edits instead of clamping.
fn request_range(params: &Value, document: &crate::workspace::Document) -> Option<TextRange> {
    let range = params.get("range")?.as_object()?;
    let start = crate::handlers::position(range.get("start")?)?;
    let end = crate::handlers::position(range.get("end")?)?;
    let start = document.offset_at(start)?;
    let end = document.offset_at(end)?;
    TextRange::new(start, end).ok()
}

/// Serializes edits for one document.
fn edits_value(document: &crate::workspace::Document, edits: &[FormatEdit]) -> Vec<Value> {
    edits
        .iter()
        .filter_map(|edit| {
            let range =
                crate::handlers::source_range_value(document, SourceRange::new(edit.range))?;
            Some(json!({"range": range, "newText": edit.text}))
        })
        .collect()
}

impl Server {
    /// Handles `textDocument/formatting`.
    pub(crate) fn formatting(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        self.format_request(id, params, false)
    }

    /// Handles `textDocument/rangeFormatting`.
    pub(crate) fn range_formatting(&self, id: Value, params: Option<Value>) -> Vec<Outgoing> {
        self.format_request(id, params, true)
    }

    /// Shared implementation of document and range formatting.
    fn format_request(&self, id: Value, params: Option<Value>, range_only: bool) -> Vec<Outgoing> {
        let Some(params) = params else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "missing parameters",
            )];
        };
        let Some(document_object) = crate::handlers::text_document(&params) else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument",
            )];
        };
        let Some(uri) = crate::handlers::string(document_object, "uri") else {
            return vec![Outgoing::failure(
                id,
                jsonrpc::error_codes::INVALID_PARAMS,
                "invalid textDocument URI",
            )];
        };
        // Formatting is a template feature; scripts and JSON are routed to
        // other handlers and must not be rewritten as WXML.
        if !crate::handlers::template_document(self, uri) {
            return vec![Outgoing::success(id, json!([]))];
        }
        let Some(document) = self.workspace.get(uri) else {
            return vec![Outgoing::success(id, json!([]))];
        };
        let options = request_options(&params);
        let edits = if range_only {
            let Some(range) = request_range(&params, document) else {
                return vec![Outgoing::success(id, json!([]))];
            };
            format_range(document.text(), range, options)
        } else {
            format_document(document.text(), options)
        };
        let values = edits_value(document, &edits);
        vec![Outgoing::success(id, json!(values))]
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn format(text: &str) -> String {
        let edits = format_document(text, FormatOptions::default());
        apply(text, &edits)
    }

    fn apply(text: &str, edits: &[FormatEdit]) -> String {
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0_usize;
        for edit in edits {
            let start = edit.range.start() as usize;
            let end = edit.range.end() as usize;
            assert!(start >= cursor, "overlapping edits: {edits:?}");
            assert!(start <= end && end <= text.len(), "edit out of bounds");
            output.push_str(&text[cursor..start]);
            output.push_str(&edit.text);
            cursor = end;
        }
        output.push_str(&text[cursor..]);
        output
    }

    #[test]
    fn collapses_single_line_tag_whitespace() {
        let text = "<view   class=\"a\"\t id='b'  >x</view>";
        assert_eq!(format(text), "<view class=\"a\" id='b'>x</view>");
    }

    #[test]
    fn normalizes_multiline_attribute_layout() {
        let text = "<view\n     class=\"a\"\n          id=\"b\"\n>中文</view>";
        assert_eq!(format(text), "<view\n  class=\"a\"\n  id=\"b\">中文</view>");
    }

    #[test]
    fn respects_the_enclosing_line_indent() {
        let text = "<block>\n    <view\n       class=\"a\"\n    >x</view>\n</block>";
        assert_eq!(
            format(text),
            "<block>\n    <view\n      class=\"a\">x</view>\n</block>"
        );
    }

    #[test]
    fn keeps_crlf_and_never_touches_text_values_or_raw_text() {
        let text = "<view\r\n   class=\"中文😀\"\r\n   id='a'\r\n>\r\n  <text>a &amp; {{ user.name }}</text>\r\n  <wxs module=\"m\">var x = 1 < 2;</wxs>\r\n</view>";
        let formatted = format(text);
        assert_eq!(
            formatted,
            "<view\r\n  class=\"中文😀\"\r\n  id='a'>\r\n  <text>a &amp; {{ user.name }}</text>\r\n  <wxs module=\"m\">var x = 1 < 2;</wxs>\r\n</view>"
        );
        assert!(formatted.contains("中文😀"));
        assert!(formatted.contains("&amp;"));
        assert!(formatted.contains("{{ user.name }}"));
        assert!(formatted.contains("var x = 1 < 2;"));
    }

    #[test]
    fn preserves_bom_and_attribute_order_and_quotes() {
        let text = "\u{feff}<view   id=\"x\"   id=\"x\"   class='y'  ></view>";
        let formatted = format(text);
        assert_eq!(
            formatted,
            "\u{feff}<view id=\"x\" id=\"x\" class='y'></view>"
        );
    }

    #[test]
    fn leaves_an_unterminated_tag_and_its_siblings_alone() {
        let text = "<view   class=\"a\"   ><text>ok</text><view class=\"x\"";
        assert_eq!(
            format(text),
            "<view class=\"a\"><text>ok</text><view class=\"x\""
        );
    }

    #[test]
    fn skips_inline_multiline_tags() {
        let text = "<view><text\n   class=\"a\">x</text></view>";
        assert_eq!(format(text), text);
    }

    #[test]
    fn empty_or_whitespace_only_tags_are_stable() {
        for (text, expected) in [
            ("<view/>", "<view/>"),
            ("<view />", "<view/>"),
            ("<view\n/>", "<view/>"),
            ("<view >", "<view>"),
            ("<view\n>", "<view>"),
        ] {
            let first = format(text);
            assert_eq!(first, expected, "{text:?}");
            assert_eq!(format(&first), first, "not idempotent for {text:?}");
        }
    }

    #[test]
    fn range_formatting_requires_full_tag_containment() {
        let text = "<view   a=\"1\"   >\n  <text   b=\"2\"   >x</text>\n</view>";
        let line = u32::try_from(text.find("  <text").expect("text tag")).expect("line");
        let end = (line as usize) + "  <text   b=\"2\"   >x</text>".len();
        let range = TextRange::new(line, u32::try_from(end).expect("end")).expect("range");
        let edits = format_range(text, range, FormatOptions::default());
        assert!(!edits.is_empty(), "{edits:?}");
        assert!(edits.iter().all(|edit| {
            edit.range.start() >= range.start() && edit.range.end() <= range.end()
        }));
        let output = apply(text, &edits);
        assert!(output.starts_with("<view   a=\"1\"   >"), "{output}");
        assert!(output.contains("<text b=\"2\">x</text>"), "{output}");
    }

    #[test]
    fn range_formatting_rejects_a_range_inside_a_tag() {
        let text = "<view   a=\"1\"   >x</view>";
        for (start, end) in [(0, 8), (7, 14), (0, 10)] {
            let range = TextRange::new(start, u32::try_from(end).expect("end")).expect("range");
            assert!(
                format_range(text, range, FormatOptions::default()).is_empty(),
                "range {start}..{end} must not edit a partially covered tag"
            );
        }
    }

    /// Serializes semantic facts that formatting must never change.
    fn fingerprint(text: &str) -> String {
        use std::fmt::Write as _;
        use wxml_syntax::AstNode;

        fn push_node(node: AstNode<'_>, output: &mut String) {
            match node {
                AstNode::Text(text) => {
                    write!(output, "T{:?};", text.text()).expect("write");
                }
                AstNode::Comment(comment) => {
                    write!(output, "C{:?};", comment.text()).expect("write");
                }
                AstNode::Mustache(mustache) => {
                    write!(
                        output,
                        "M{:?}{};",
                        mustache.expr_source(),
                        mustache.is_closed()
                    )
                    .expect("write");
                }
                AstNode::Element(element) => {
                    write!(
                        output,
                        "E{:?}{}{}[",
                        element.name(),
                        element.is_closed(),
                        element.is_self_closing()
                    )
                    .expect("write");
                    for attribute in element.attributes() {
                        write!(
                            output,
                            "A{:?}{:?};",
                            attribute.name(),
                            attribute.value().map(wxml_syntax::AttributeValue::raw)
                        )
                        .expect("write");
                    }
                    for child in element.children() {
                        push_node(child, output);
                    }
                    output.push(']');
                }
                AstNode::Document(_) | AstNode::Attribute(_) | AstNode::AttributeValue(_) => {}
            }
        }

        let parsed = wxml_syntax::parse(text);
        let mut output = String::new();
        for child in parsed.tree.document().children() {
            push_node(child, &mut output);
        }
        write!(output, "errors={}", parsed.errors.len()).expect("write");
        output
    }

    #[test]
    fn formatting_is_idempotent_and_semantics_preserving() {
        let cases = [
            "<view   class=\"中文😀\"   id='a'   >你好 &amp; {{ user.name }}</view>",
            "<view\r\n   class=\"a\"\r\n   id=\"b\"\r\n>🙂</view>",
            "<?xml?><view   a=\"1\"   ><!-- 注释 --><text   b=\"2\"   >x</text></view>",
            "<block><view\n   wx:if=\"{{ ok }}\"\n>x</view></block>",
            "<wxs   module=\"m\"   >var x = 1 < 2 && \"中文\";</wxs>",
            "<view   class=\"x\"   ><text>ok</text><view class=\"unclosed\"",
            "纯文本 {{ user.name }} <!-- c -->",
        ];
        for text in cases {
            let first = format(text);
            assert_eq!(format(&first), first, "not idempotent for {text:?}");
            assert_eq!(
                fingerprint(text),
                fingerprint(&first),
                "semantics changed for {text:?} -> {first:?}"
            );
            let edits = format_document(text, FormatOptions::default());
            for edit in &edits {
                assert!(
                    edit.text.bytes().all(|byte| byte.is_ascii_whitespace()),
                    "non-whitespace replacement in {edit:?}"
                );
                assert!(edit.range.end() as usize <= text.len());
            }
        }
    }

    #[test]
    fn randomized_layouts_stay_idempotent_and_semantics_preserving() {
        fn next(state: &mut u64) -> u64 {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            *state
        }

        let gaps = [" ", "   ", "\t", "\n  ", "\n      ", "\r\n    "];
        let mut state = 0x5eed_u64;
        let mut checked = 0_usize;
        for index in 0..300_u64 {
            let gap = gaps[usize::try_from(next(&mut state) % 6).expect("gap")];
            let tab = (next(&mut state) % 2) == 0;
            let mut text = String::new();
            if index % 3 == 0 {
                text.push_str("<block>");
            }
            text.push_str("<view");
            text.push_str(gap);
            text.push_str("class=\"中文😀\"");
            text.push_str(gap);
            text.push_str("id='a'");
            if tab {
                text.push('\n');
                text.push_str("  ");
            }
            text.push('>');
            text.push_str("你好 &amp; {{ user.name }}");
            text.push_str("<!-- 注释 -->");
            text.push_str("<text>ok</text>");
            text.push_str("</view>");
            if index % 3 == 0 {
                text.push_str("</block>");
            }
            if index % 7 == 0 {
                text.push_str("<view class=\"unclosed\"");
            }
            let first = format(&text);
            assert_eq!(format(&first), first, "not idempotent for {text:?}");
            assert_eq!(
                fingerprint(&text),
                fingerprint(&first),
                "semantics changed for {text:?} -> {first:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 300);
    }

    #[test]
    fn honors_tab_indentation_options() {
        let text = "<view\n class=\"a\"\n>";
        let edits = format_document(text, FormatOptions::new(4, false));
        assert_eq!(apply(text, &edits), "<view\n\tclass=\"a\">");
    }
}
