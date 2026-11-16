//! Completion snippets with strict insertion-safety gating.
//!
//! Snippets are only produced when the client advertised
//! `textDocument.completion.completionItem.snippetSupport`. A tag snippet is
//! offered only when the tag name is the last text in the document, because
//! the snippet body supplies its own `>` and closing tag. An attribute
//! snippet is offered only when the current attribute token has no `=value`
//! and no trailing name characters after the cursor; the remainder of the tag
//! is inspected before an edit is produced. When those conditions do not
//! hold, the server returns only the plain completion item, never a snippet
//! that could leave `</view>>`, stray attributes or a truncated tag name.
//!
//! Built-in snippets are merged with the workspace catalog from
//! [`super::user_snippets`]. A user entry overrides a built-in with the same
//! `id` or label; `wx:for`, `wx:if` and `bindtap` are suppressed when the
//! attribute already exists, and `wx:for` omits `wx:key` when that attribute
//! is already present.

// Snippet bodies intentionally contain LSP placeholders such as `${1:items}`;
// they are not `format!` arguments.
#![allow(clippy::literal_string_with_formatting_args)]

use wxml_text::SourceRange;
use wxml_types::{CompletionItem, CompletionItemKind, InsertTextFormat};

use super::context::{Context, Site, is_attribute_name, is_tag_name};
use super::static_items::matches;
use super::user_snippets::{Catalog, Scope};

/// Largest line-prefix indentation copied into a snippet.
const MAX_INDENT_CHARS: usize = 64;

/// Bytes scanned on each side when detecting the document newline style.
const NEWLINE_SCAN_BYTES: usize = 2_048;

/// Largest tail after a tag-name prefix considered for a tag snippet.
const MAX_TAG_SNIPPET_TAIL: usize = 4_096;

/// Indentation context used to lay out an inserted tag snippet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnippetEnv {
    /// Whitespace before the tag on its line.
    closing_indent: String,
    /// Indentation for nested content and attributes.
    content_indent: String,
    /// Line ending of the current document.
    newline: String,
}

impl SnippetEnv {
    /// Derives the indentation context at `offset` in `text`.
    #[must_use]
    pub(crate) fn new(text: &str, offset: u32) -> Self {
        let offset = usize::try_from(offset).ok().unwrap_or(0).min(text.len());
        let line_start = text
            .get(..offset)
            .and_then(|head| head.rfind('\n'))
            .map_or(0, |newline| newline.saturating_add(1));
        let line_prefix = text.get(line_start..offset).unwrap_or_default();
        let closing_indent: String = line_prefix
            .chars()
            .take(MAX_INDENT_CHARS)
            .take_while(|character| *character == ' ' || *character == '\t')
            .collect();
        let unit = if line_prefix.contains('\t') || closing_indent.contains('\t') {
            "\t"
        } else {
            "  "
        };
        Self {
            content_indent: format!("{closing_indent}{unit}"),
            closing_indent,
            newline: newline_near(text, offset).to_owned(),
        }
    }

    /// Returns the document line ending.
    pub(crate) fn line(&self) -> &str {
        &self.newline
    }

    /// Normalizes a user snippet body to this document's line ending.
    pub(crate) fn normalize_newlines(&self, body: &str) -> String {
        let normalized = body.replace("\r\n", "\n");
        if self.newline == "\r\n" {
            normalized.replace('\n', "\r\n")
        } else {
            normalized
        }
    }
}

/// Detects the newline style around `offset` without scanning the whole file.
fn newline_near(text: &str, offset: usize) -> &'static str {
    let bytes = text.as_bytes();
    let start = offset.saturating_sub(NEWLINE_SCAN_BYTES);
    let end = offset.saturating_add(NEWLINE_SCAN_BYTES).min(bytes.len());
    if bytes[start..end].windows(2).any(|window| window == b"\r\n") {
        return "\r\n";
    }
    if bytes[start..end].contains(&b'\n') {
        return "\n";
    }
    let head_end = NEWLINE_SCAN_BYTES.min(bytes.len());
    if bytes[..head_end].windows(2).any(|window| window == b"\r\n") {
        return "\r\n";
    }
    "\n"
}

/// One built-in tag snippet.
struct TagSnippet {
    /// Stable identity for user overrides.
    id: &'static str,
    /// Candidate label, starting with the tag name for prefix filtering.
    label: &'static str,
    /// Tag name used for matching and detail text.
    name: &'static str,
    /// Body inserted after the `<` already typed by the user.
    body: fn(&SnippetEnv) -> String,
}

/// Built-in block snippets.
const TAG_SNIPPETS: &[TagSnippet] = &[
    TagSnippet {
        id: "tag.view",
        label: "view (snippet)",
        name: "view",
        body: |env| {
            format!(
                "view>{line}{content}$0{line}{closing}</view>",
                line = env.line(),
                content = env.content_indent,
                closing = env.closing_indent,
            )
        },
    },
    TagSnippet {
        id: "tag.text",
        label: "text (snippet)",
        name: "text",
        body: |_| "text>$0</text>".to_owned(),
    },
    TagSnippet {
        id: "tag.image",
        label: "image (snippet)",
        name: "image",
        body: |_| "image src=\"${1:/images/icon.png}\"$0/>".to_owned(),
    },
    TagSnippet {
        id: "tag.button",
        label: "button (snippet)",
        name: "button",
        body: |_| "button type=\"${1:default}\">$0</button>".to_owned(),
    },
    TagSnippet {
        id: "tag.block",
        label: "block (snippet)",
        name: "block",
        body: |_| "block>$0</block>".to_owned(),
    },
    TagSnippet {
        id: "tag.template",
        label: "template (snippet)",
        name: "template",
        body: |_| "template name=\"${1:name}\">$0</template>".to_owned(),
    },
    TagSnippet {
        id: "tag.include",
        label: "include (snippet)",
        name: "include",
        body: |_| "include src=\"${1:path/to/file.wxml}\"$0/>".to_owned(),
    },
    TagSnippet {
        id: "tag.navigator",
        label: "navigator (snippet)",
        name: "navigator",
        body: |_| "navigator url=\"${1:/pages/index/index}\">$0</navigator>".to_owned(),
    },
];

/// Built-in attribute categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttributeKind {
    /// `wx:for` with an optional `wx:key`.
    For,
    /// `wx:if`.
    If,
    /// `bindtap`.
    Tap,
}

/// One built-in attribute snippet.
struct AttributeSnippet {
    /// Stable identity for user overrides.
    id: &'static str,
    /// Candidate label, starting with the attribute prefix.
    label: &'static str,
    /// Short explanation shown next to the label.
    detail: &'static str,
    /// Attribute this snippet inserts.
    kind: AttributeKind,
}

/// Built-in directive and event snippets.
const ATTRIBUTE_SNIPPETS: &[AttributeSnippet] = &[
    AttributeSnippet {
        id: "attr.wx-for",
        label: "wx:for (snippet)",
        detail: "Loop over a list and key each item",
        kind: AttributeKind::For,
    },
    AttributeSnippet {
        id: "attr.wx-if",
        label: "wx:if (snippet)",
        detail: "Render the element when the condition is true",
        kind: AttributeKind::If,
    },
    AttributeSnippet {
        id: "attr.bindtap",
        label: "bindtap (snippet)",
        detail: "Handle a tap event",
        kind: AttributeKind::Tap,
    },
];

/// Snippet insertion context shared by built-in and user entries.
pub(crate) struct SnippetContext<'a> {
    /// Indentation/newline environment.
    pub(crate) env: &'a SnippetEnv,
    /// Effective user catalog and switches.
    pub(crate) catalog: &'a Catalog,
}

/// A parsed start tag around the completion position.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TagAnalysis {
    /// Tag name.
    name: String,
    /// Attribute names with their byte starts.
    attributes: Vec<(String, usize)>,
    /// Byte offset of `>`/`/>`, or the end of the document for an open tag.
    tag_end: usize,
}

/// Builds snippet items for a detected completion context.
pub(crate) fn items(
    context: &Context,
    text: &str,
    snippet: &SnippetContext<'_>,
) -> Vec<CompletionItem> {
    if !snippet.catalog.enabled {
        return Vec::new();
    }
    match &context.site {
        Site::Tag {
            closing: false,
            name,
        } => tag_items(context, text, snippet, name),
        Site::AttributeName { tag, .. } => attribute_items(context, text, snippet, tag),
        Site::Tag { closing: true, .. } | Site::AttributeValue { .. } | Site::Mustache => {
            Vec::new()
        }
    }
}

/// Tag snippets offered at an end-of-document tag name.
fn tag_items(
    context: &Context,
    text: &str,
    snippet: &SnippetContext<'_>,
    typed_name: &str,
) -> Vec<CompletionItem> {
    let replace = SourceRange::new(context.replace);
    let end = usize::try_from(context.replace.end()).unwrap_or(usize::MAX);
    // The body supplies `>` and `</tag>`. Anything after the typed name would
    // survive the name-only replacement and produce a second `>` or orphaned
    // attributes, so snippets are limited to the end-of-document state.
    let Some(tail) = text.get(end..) else {
        return Vec::new();
    };
    if tail.len() > MAX_TAG_SNIPPET_TAIL || !tail.trim().is_empty() {
        return Vec::new();
    }
    let Some(analysis) = analyze_tag(text, end) else {
        return Vec::new();
    };
    if !analysis.name.starts_with(typed_name) {
        return Vec::new();
    }

    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let mut seen = Vec::new();
    if snippet.catalog.builtin {
        for entry in TAG_SNIPPETS {
            if snippet.catalog.overrides(entry.id, entry.label) {
                continue;
            }
            if !matches(entry.label, prefix) && !matches(entry.name, prefix) {
                continue;
            }
            seen.push(entry.label.to_owned());
            items.push(CompletionItem {
                label: entry.label.to_owned(),
                kind: CompletionItemKind::Tag,
                detail: Some(format!("Insert <{}>…</{}>", entry.name, entry.name)),
                documentation: None,
                insert_text: (entry.body)(snippet.env),
                insert_text_format: InsertTextFormat::Snippet,
                replace,
                sort_text: Some(format!("5_{}", entry.label)),
                filter_text: Some(entry.name.to_owned()),
            });
        }
    }
    for entry in snippet
        .catalog
        .snippets
        .iter()
        .filter(|entry| entry.scope == Scope::Tag)
    {
        if seen.iter().any(|label| label == &entry.label)
            || (!matches(&entry.label, prefix) && !matches(&entry.prefix, prefix))
        {
            continue;
        }
        items.push(CompletionItem {
            label: entry.label.clone(),
            kind: CompletionItemKind::Tag,
            detail: Some(entry.description.clone()),
            documentation: None,
            insert_text: snippet.env.normalize_newlines(&entry.body),
            insert_text_format: InsertTextFormat::Snippet,
            replace,
            sort_text: Some(format!("4_{}", entry.label)),
            filter_text: Some(entry.prefix.clone()),
        });
    }
    items
}

/// Attribute snippets offered while typing an attribute name.
fn attribute_items(
    context: &Context,
    text: &str,
    snippet: &SnippetContext<'_>,
    typed_tag: &str,
) -> Vec<CompletionItem> {
    let offset = usize::try_from(context.replace.end()).unwrap_or(usize::MAX);
    let Some(analysis) = analyze_tag(text, offset) else {
        return Vec::new();
    };
    if analysis.name != typed_tag {
        return Vec::new();
    }
    if matches!(analysis.name.as_str(), "wxs" | "import" | "include") {
        return Vec::new();
    }
    let replace_start = usize::try_from(context.replace.start()).unwrap_or(usize::MAX);
    let Ok(rest) = text.get(offset..analysis.tag_end).ok_or(()) else {
        return Vec::new();
    };
    if !safe_attribute_rest(rest) {
        return Vec::new();
    }
    let existing: Vec<&str> = analysis
        .attributes
        .iter()
        .filter(|(_, start)| *start != replace_start)
        .map(|(name, _)| name.as_str())
        .collect();
    let separator = attribute_separator(text, replace_start);
    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let mut seen = Vec::new();
    if snippet.catalog.builtin {
        for entry in ATTRIBUTE_SNIPPETS {
            if snippet.catalog.overrides(entry.id, entry.label) {
                continue;
            }
            if !matches(entry.label, prefix) {
                continue;
            }
            let Some(body) = builtin_attribute_body(entry.kind, &existing) else {
                continue;
            };
            seen.push(entry.label.to_owned());
            items.push(CompletionItem {
                label: entry.label.to_owned(),
                kind: CompletionItemKind::Attribute,
                detail: Some(entry.detail.to_owned()),
                documentation: None,
                insert_text: format!("{separator}{body}"),
                insert_text_format: InsertTextFormat::Snippet,
                replace: SourceRange::new(context.replace),
                sort_text: Some(format!("5_{}", entry.label)),
                filter_text: Some(attribute_label_prefix(entry.label).to_owned()),
            });
        }
    }
    for entry in snippet
        .catalog
        .snippets
        .iter()
        .filter(|entry| entry.scope == Scope::Attribute)
    {
        if seen.iter().any(|label| label == &entry.label)
            || (!matches(&entry.label, prefix) && !matches(&entry.prefix, prefix))
            || existing.iter().any(|name| *name == entry.prefix)
        {
            continue;
        }
        items.push(CompletionItem {
            label: entry.label.clone(),
            kind: CompletionItemKind::Attribute,
            detail: Some(entry.description.clone()),
            documentation: None,
            insert_text: format!("{separator}{}", snippet.env.normalize_newlines(&entry.body)),
            insert_text_format: InsertTextFormat::Snippet,
            replace: SourceRange::new(context.replace),
            sort_text: Some(format!("4_{}", entry.label)),
            filter_text: Some(entry.prefix.clone()),
        });
    }
    items
}

/// Returns the separator needed when the replacement range is empty.
fn attribute_separator(text: &str, replace_start: usize) -> &'static str {
    let previous = text
        .get(..replace_start)
        .and_then(|head| head.chars().next_back());
    match previous {
        Some(character) if !character.is_whitespace() && character != '<' => " ",
        _ => "",
    }
}

/// Checks the remainder of an attribute token after the cursor.
fn safe_attribute_rest(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'=') {
        return false;
    }
    if bytes.first().is_some_and(|byte| is_attribute_name(*byte)) {
        // The cursor is inside an attribute name; completing would leave a
        // suffix such as `or="x"` behind.
        return false;
    }
    let trimmed = rest.trim_start_matches(|character: char| character.is_ascii_whitespace());
    if trimmed.starts_with('=') {
        return false;
    }
    if trimmed.starts_with('/') && !trimmed.starts_with("/>") {
        return false;
    }
    true
}

/// Builds one built-in attribute body, skipping existing attributes.
fn builtin_attribute_body(kind: AttributeKind, existing: &[&str]) -> Option<String> {
    match kind {
        AttributeKind::For => {
            if existing.contains(&"wx:for") {
                return None;
            }
            let mut body = "wx:for=\"{{${1:items}}}\"".to_owned();
            if !existing.contains(&"wx:key") {
                body.push_str(" wx:key=\"${2:index}\"");
            }
            Some(body)
        }
        AttributeKind::If => {
            if existing.contains(&"wx:if") {
                None
            } else {
                Some("wx:if=\"{{${1:condition}}}\"".to_owned())
            }
        }
        AttributeKind::Tap => {
            if existing.contains(&"bindtap") || existing.contains(&"bind:tap") {
                None
            } else {
                Some("bindtap=\"${1:onTap}\"".to_owned())
            }
        }
    }
}

/// Returns the label prefix used as `filterText`.
fn attribute_label_prefix(label: &str) -> &str {
    label.split_once(" (").map_or(label, |(head, _)| head)
}

/// Parses the start tag enclosing `offset`.
///
/// The scan is bounded by the tag itself. A malformed tag or a `<` that does
/// not actually open the enclosing start tag returns `None`, which suppresses
/// snippets instead of risking a bad edit.
fn analyze_tag(text: &str, offset: usize) -> Option<TagAnalysis> {
    let tag_start = text.get(..offset)?.rfind('<')?;
    let bytes = text.as_bytes();
    let mut index = tag_start.checked_add(1)?;
    if bytes.get(index) == Some(&b'/') || bytes.get(index) == Some(&b'!') {
        return None;
    }
    let name_start = index;
    while index < bytes.len() && is_tag_name(bytes[index]) {
        index = index.checked_add(1)?;
    }
    if index == name_start {
        return None;
    }
    let name = text.get(name_start..index)?.to_owned();
    let mut attributes = Vec::new();
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index = index.checked_add(1)?;
        }
        if index >= bytes.len() {
            return Some(TagAnalysis {
                name,
                attributes,
                tag_end: text.len(),
            });
        }
        match bytes[index] {
            b'>' => {
                return Some(TagAnalysis {
                    name,
                    attributes,
                    tag_end: index.saturating_add(1),
                });
            }
            b'/' if bytes.get(index.saturating_add(1)) == Some(&b'>') => {
                return Some(TagAnalysis {
                    name,
                    attributes,
                    tag_end: index.saturating_add(2),
                });
            }
            _ => {
                let attribute_start = index;
                while index < bytes.len() && is_attribute_name(bytes[index]) {
                    index = index.checked_add(1)?;
                }
                if index == attribute_start {
                    return None;
                }
                attributes.push((
                    text.get(attribute_start..index)?.to_owned(),
                    attribute_start,
                ));
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index = index.checked_add(1)?;
                }
                if bytes.get(index) != Some(&b'=') {
                    continue;
                }
                index = index.checked_add(1)?;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index = index.checked_add(1)?;
                }
                match bytes.get(index) {
                    Some(&quote) if quote == b'"' || quote == b'\'' => {
                        index = index.checked_add(1)?;
                        while index < bytes.len() && bytes[index] != quote {
                            index = index.checked_add(1)?;
                        }
                        if index >= bytes.len() {
                            return None;
                        }
                        index = index.checked_add(1)?;
                    }
                    Some(_) => {
                        while index < bytes.len()
                            && !bytes[index].is_ascii_whitespace()
                            && bytes[index] != b'>'
                        {
                            index = index.checked_add(1)?;
                        }
                    }
                    None => return None,
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::assert_is_empty, clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use wxml_text::TextRange;
    use wxml_types::CompletionItem;

    use super::*;
    use crate::completion::context::detect;
    use crate::completion::user_snippets::UserSnippet;

    fn catalog_with(entries: Vec<UserSnippet>) -> Catalog {
        Catalog {
            enabled: true,
            builtin: true,
            snippets: entries,
        }
    }

    fn items_at(text: &str, offset: usize, catalog: &Catalog) -> (Context, Vec<CompletionItem>) {
        let detected = detect(text, u32::try_from(offset).expect("offset")).expect("site");
        let env = SnippetEnv::new(text, detected.replace.start());
        let snippet = SnippetContext { env: &env, catalog };
        let items = items(&detected, text, &snippet);
        (detected, items)
    }

    fn expand(body: &str) -> String {
        let mut output = String::new();
        let mut chars = body.chars().peekable();
        while let Some(character) = chars.next() {
            if character != '$' {
                output.push(character);
                continue;
            }
            match chars.peek() {
                Some('{') => {
                    chars.next();
                    let mut inner = String::new();
                    for next in chars.by_ref() {
                        if next == '}' {
                            break;
                        }
                        inner.push(next);
                    }
                    let default = inner.split_once(':').map_or("", |(_, value)| value);
                    output.push_str(default);
                }
                Some(next) if next.is_ascii_digit() => {
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                }
                _ => output.push('$'),
            }
        }
        output
    }

    fn apply(text: &str, item: &CompletionItem) -> String {
        let start = usize::try_from(item.replace.start()).expect("start");
        let end = usize::try_from(item.replace.end()).expect("end");
        let mut output = String::new();
        output.push_str(&text[..start]);
        output.push_str(&expand(&item.insert_text));
        output.push_str(&text[end..]);
        output
    }

    fn apply_and_parse(text: &str, item: &CompletionItem) -> String {
        let output = apply(text, item);
        let parsed = wxml_syntax::parse(&output);
        assert!(
            parsed.is_clean(),
            "snippet produced invalid WXML: {output:?} ({:?})",
            parsed.errors
        );
        output
    }

    fn builtin_catalog() -> Arc<Catalog> {
        Arc::new(Catalog::default())
    }

    #[test]
    fn tag_snippet_applies_cleanly_at_the_end_of_the_document() {
        let text = "<vi";
        let (_, items) = items_at(text, text.len(), &builtin_catalog());
        let item = items
            .iter()
            .find(|item| item.label == "view (snippet)")
            .expect("view snippet");
        assert_eq!(apply_and_parse(text, item), "<view>\n  \n</view>");
        assert_eq!(item.insert_text_format, InsertTextFormat::Snippet);
    }

    #[test]
    fn tag_snippets_are_suppressed_when_the_tag_already_closed_or_has_content() {
        for (text, offset) in [
            ("<vi>", 3),
            ("<vi/>", 3),
            ("<view class=\"a\"", 5),
            ("<vi>text</vi>", 3),
            ("<vi\n  class=\"a\"", 3),
        ] {
            let (_, items) = items_at(text, offset, &builtin_catalog());
            assert!(
                items
                    .iter()
                    .all(|item| item.insert_text_format != InsertTextFormat::Snippet),
                "{text:?} produced {items:?}"
            );
        }
    }

    #[test]
    fn mid_name_tag_completion_does_not_offer_a_snippet() {
        let text = "<view>text</view>";
        let offset = text.find('i').expect("i") + 1;
        let (_, items) = items_at(text, offset, &builtin_catalog());
        assert!(
            items
                .iter()
                .all(|item| item.insert_text_format != InsertTextFormat::Snippet)
        );
    }

    #[test]
    fn attribute_snippet_at_end_of_name_is_safe() {
        let text = "<view wx:f></view>";
        let offset = text.find('>').expect(">");
        let (_, items) = items_at(text, offset, &builtin_catalog());
        let item = items
            .iter()
            .find(|item| item.label == "wx:for (snippet)")
            .expect("wx:for snippet");
        assert_eq!(
            apply_and_parse(text, item),
            "<view wx:for=\"{{items}}\" wx:key=\"index\"></view>"
        );
    }

    #[test]
    fn attribute_snippet_at_the_end_of_an_open_tag_keeps_it_open() {
        let text = "<view wx:f";
        let (_, items) = items_at(text, text.len(), &builtin_catalog());
        let item = items
            .iter()
            .find(|item| item.label == "wx:for (snippet)")
            .expect("wx:for snippet");
        assert_eq!(
            apply(text, item),
            "<view wx:for=\"{{items}}\" wx:key=\"index\""
        );
    }

    #[test]
    fn attribute_snippet_before_a_closing_delimiter_is_safe() {
        for (text, offset, expected) in [
            (
                "<view >x</view>",
                6,
                "<view wx:for=\"{{items}}\" wx:key=\"index\">x</view>",
            ),
            (
                "<view wx:f>x</view>",
                10,
                "<view wx:for=\"{{items}}\" wx:key=\"index\">x</view>",
            ),
            (
                "<view wx:f/>",
                10,
                "<view wx:for=\"{{items}}\" wx:key=\"index\"/>",
            ),
        ] {
            let (_, items) = items_at(text, offset, &builtin_catalog());
            let item = items
                .iter()
                .find(|item| item.label == "wx:for (snippet)")
                .expect("wx:for snippet");
            assert_eq!(apply_and_parse(text, item), expected);
        }
    }

    #[test]
    fn attribute_snippet_after_a_closed_value_inserts_a_separator() {
        let text = "<view class=\"a\" >x</view>";
        let offset = text.find('>').expect(">");
        let (_, items) = items_at(text, offset, &builtin_catalog());
        let item = items
            .iter()
            .find(|item| item.label == "wx:for (snippet)")
            .expect("wx:for snippet");
        assert_eq!(
            apply_and_parse(text, item),
            "<view class=\"a\" wx:for=\"{{items}}\" wx:key=\"index\">x</view>"
        );
    }

    #[test]
    fn attribute_snippets_are_suppressed_with_an_existing_value_or_mid_name() {
        for (text, offset) in [
            ("<view wx:f=\"\" >", 10),
            ("<view wx:f=1 >", 10),
            ("<view wx:foo >", 10),
        ] {
            let (_, items) = items_at(text, offset, &builtin_catalog());
            assert!(
                items
                    .iter()
                    .all(|item| item.insert_text_format != InsertTextFormat::Snippet),
                "{text:?} produced {items:?}"
            );
        }
    }

    #[test]
    fn value_sites_and_wxs_tags_do_not_offer_snippets() {
        let text = "<view wx:f=\"\"";
        let offset = text.find('"').expect("quote") + 1;
        let (_, items) = items_at(text, offset, &builtin_catalog());
        assert!(items.is_empty(), "{items:?}");

        let text = "<wxs >";
        let (_, items) = items_at(text, 5, &builtin_catalog());
        assert!(items.is_empty(), "{items:?}");
    }

    #[test]
    fn existing_attributes_suppress_duplicates_and_omit_wx_key() {
        let text = "<view bindtap=\"x\" wx:f>x</view>";
        let offset = text.rfind("wx:f").expect("attr") + "wx:f".len();
        let (_, items) = items_at(text, offset, &builtin_catalog());
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(!labels.contains(&"bindtap (snippet)"), "{labels:?}");
        let item = items
            .iter()
            .find(|item| item.label == "wx:for (snippet)")
            .expect("wx:for snippet");
        assert_eq!(
            apply_and_parse(text, item),
            "<view bindtap=\"x\" wx:for=\"{{items}}\" wx:key=\"index\">x</view>"
        );

        let text = "<view wx:for-item=\"x\" wx:key=\"id\" wx:f>x</view>";
        let offset = text.rfind("wx:f").expect("attr") + "wx:f".len();
        let (_, items) = items_at(text, offset, &builtin_catalog());
        let item = items
            .iter()
            .find(|item| item.label == "wx:for (snippet)")
            .expect("wx:for snippet");
        let output = apply_and_parse(text, item);
        assert_eq!(
            output,
            "<view wx:for-item=\"x\" wx:key=\"id\" wx:for=\"{{items}}\">x</view>"
        );
    }

    #[test]
    fn existing_wx_for_suppresses_the_loop_snippet() {
        let text = "<view wx:for=\"{{x}}\" wx:key=\"id\" wx:f>";
        let offset = text.rfind("wx:f").expect("attr") + "wx:f".len();
        let (_, items) = items_at(text, offset, &builtin_catalog());
        assert!(
            items.iter().all(|item| item.label != "wx:for (snippet)"),
            "{items:?}"
        );
    }

    #[test]
    fn crlf_and_indentation_are_preserved_in_snippets() {
        let text = "<!--x-->\r\n  <vi";
        let (_, items) = items_at(text, text.len(), &builtin_catalog());
        let item = items
            .iter()
            .find(|item| item.label == "view (snippet)")
            .expect("view snippet");
        let output = apply_and_parse(text, item);
        assert_eq!(output, "<!--x-->\r\n  <view>\r\n    \r\n  </view>");
        assert!(item.insert_text.contains("\r\n"));
    }

    #[test]
    fn builtin_template_and_include_snippets_are_available() {
        for (text, label) in [("<te", "template (snippet)"), ("<in", "include (snippet)")] {
            let (_, items) = items_at(text, text.len(), &builtin_catalog());
            assert!(
                items.iter().any(|item| item.label == label),
                "{text:?} did not offer {label}: {items:?}"
            );
        }
    }

    #[test]
    fn user_attribute_snippets_skip_existing_same_name_attributes() {
        let catalog = catalog_with(vec![UserSnippet {
            id: "attr.bindtap".to_owned(),
            label: "bindtap (custom)".to_owned(),
            prefix: "bindtap".to_owned(),
            scope: Scope::Attribute,
            description: "Custom tap".to_owned(),
            body: "bindtap=\"${1:onTap}\"".to_owned(),
        }]);
        let text = "<view bindtap=\"x\" ></view>";
        let offset = text.find('>').expect(">");
        let (_, items) = items_at(text, offset, &catalog);
        assert!(
            items.iter().all(|item| item.label != "bindtap (custom)"),
            "{items:?}"
        );
    }

    #[test]
    fn builtin_switch_disables_only_builtins() {
        let catalog = Catalog {
            enabled: true,
            builtin: false,
            snippets: vec![UserSnippet {
                id: "tag.custom".to_owned(),
                label: "custom (snippet)".to_owned(),
                prefix: "custom".to_owned(),
                scope: Scope::Tag,
                description: "Custom".to_owned(),
                body: "view>$0</view>".to_owned(),
            }],
        };
        let text = "<cu";
        let (_, items) = items_at(text, text.len(), &catalog);
        assert!(items.iter().any(|item| item.label == "custom (snippet)"));
        assert!(items.iter().all(|item| item.label != "view (snippet)"));

        let text = "<vi";
        let (_, items) = items_at(text, text.len(), &catalog);
        assert!(
            items
                .iter()
                .all(|item| item.insert_text_format != InsertTextFormat::Snippet)
        );
    }

    #[test]
    fn user_snippets_override_by_id_and_respect_switch() {
        let catalog = catalog_with(vec![UserSnippet {
            id: "tag.view".to_owned(),
            label: "view (custom)".to_owned(),
            prefix: "view".to_owned(),
            scope: Scope::Tag,
            description: "Custom".to_owned(),
            body: "view class=\"card\">$0</view>".to_owned(),
        }]);
        let text = "<vi";
        let (_, items) = items_at(text, text.len(), &catalog);
        assert!(
            items.iter().all(|item| item.label != "view (snippet)"),
            "{items:?}"
        );
        let item = items
            .iter()
            .find(|item| item.label == "view (custom)")
            .expect("custom snippet");
        assert_eq!(apply_and_parse(text, item), "<view class=\"card\"></view>");

        let mut disabled = catalog;
        disabled.enabled = false;
        let (_, items) = items_at(text, text.len(), &disabled);
        assert!(items.is_empty(), "{items:?}");
    }

    #[test]
    fn user_snippets_load_through_the_store() {
        let base = std::env::var_os("TMPDIR").map_or_else(
            || std::path::PathBuf::from("/tmp/opencode"),
            std::path::PathBuf::from,
        );
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = base.join(format!("wxml-snippet-completion-{stamp}.json"));
        std::fs::write(
            &path,
            r#"{"snippets": [{"label": "card (custom)", "prefix": "card",
                 "body": "view class=\"card\">$0</view>"}]}"#,
        )
        .expect("write");
        let store = crate::completion::user_snippets::Store::default();
        store.configure(Some(path.clone()), true);
        let catalog = store.catalog();
        let text = "<ca";
        let (_, items) = items_at(text, text.len(), &catalog);
        let item = items
            .iter()
            .find(|item| item.label == "card (custom)")
            .expect("custom snippet");
        assert_eq!(apply_and_parse(text, item), "<view class=\"card\"></view>");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn replacement_ranges_are_never_empty_for_nonempty_prefixes() {
        let text = "<vi";
        let (context, _) = items_at(text, text.len(), &builtin_catalog());
        assert_eq!(context.replace, TextRange::new(1, 3).expect("range"));
    }
}
