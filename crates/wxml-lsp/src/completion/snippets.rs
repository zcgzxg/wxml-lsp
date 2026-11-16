//! Built-in WXML completion snippets.
//!
//! Snippets are only produced when the client advertised
//! `textDocument.completion.completionItem.snippetSupport`. Each item carries
//! [`InsertTextFormat::Snippet`], LSP placeholders and an indentation that
//! matches the line that opens the tag. Plain tag/attribute candidates remain
//! available next to the snippets, so users never lose the non-snippet path.

// Snippet bodies intentionally contain LSP placeholders such as `${1:items}`;
// they are not `format!` arguments.
#![allow(clippy::literal_string_with_formatting_args)]

use wxml_text::SourceRange;
use wxml_types::{CompletionItem, CompletionItemKind, InsertTextFormat};

use super::context::{Context, Site};
use super::static_items::matches;

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
            newline: if text.contains("\r\n") {
                "\r\n".to_owned()
            } else {
                "\n".to_owned()
            },
        }
    }

    /// Inserts the document line ending into a snippet body.
    fn line(&self) -> &str {
        &self.newline
    }
}

/// One built-in tag snippet.
struct TagSnippet {
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
        label: "text (snippet)",
        name: "text",
        body: |_| "text>$0</text>".to_owned(),
    },
    TagSnippet {
        label: "image (snippet)",
        name: "image",
        body: |_| "image src=\"${1:/images/icon.png}\"$0/>".to_owned(),
    },
    TagSnippet {
        label: "button (snippet)",
        name: "button",
        body: |_| "button type=\"${1:default}\">$0</button>".to_owned(),
    },
    TagSnippet {
        label: "block (snippet)",
        name: "block",
        body: |_| "block>$0</block>".to_owned(),
    },
];

/// One built-in attribute snippet.
struct AttributeSnippet {
    /// Candidate label, starting with the attribute prefix.
    label: &'static str,
    /// Inserted attribute text including values.
    body: &'static str,
    /// Short explanation shown next to the label.
    detail: &'static str,
}

/// Built-in directive and event snippets.
const ATTRIBUTE_SNIPPETS: &[AttributeSnippet] = &[
    AttributeSnippet {
        label: "wx:for (snippet)",
        body: "wx:for=\"{{${1:items}}}\" wx:key=\"${2:index}\"",
        detail: "Loop over a list and key each item",
    },
    AttributeSnippet {
        label: "wx:if (snippet)",
        body: "wx:if=\"{{${1:condition}}}\"",
        detail: "Render the element when the condition is true",
    },
    AttributeSnippet {
        label: "bindtap (snippet)",
        body: "bindtap=\"${1:onTap}\"",
        detail: "Handle a tap event",
    },
];

/// Builds snippet items for a detected completion context.
pub(crate) fn items(context: &Context, env: &SnippetEnv) -> Vec<CompletionItem> {
    match &context.site {
        Site::Tag { closing: false, .. } => tag_items(context, env),
        Site::AttributeName { .. } => attribute_items(context),
        Site::Tag { closing: true, .. } | Site::AttributeValue { .. } | Site::Mustache => {
            Vec::new()
        }
    }
}

/// Tag snippets offered at a start-tag name.
fn tag_items(context: &Context, env: &SnippetEnv) -> Vec<CompletionItem> {
    let prefix = context.prefix.as_str();
    let replace = SourceRange::new(context.replace);
    TAG_SNIPPETS
        .iter()
        .filter(|snippet| matches(snippet.label, prefix) || matches(snippet.name, prefix))
        .map(|snippet| CompletionItem {
            label: snippet.label.to_owned(),
            kind: CompletionItemKind::Tag,
            detail: Some(format!("Insert <{}>…</{}>", snippet.name, snippet.name)),
            documentation: None,
            insert_text: (snippet.body)(env),
            insert_text_format: InsertTextFormat::Snippet,
            replace,
            sort_text: Some(format!("5_{}", snippet.label)),
            filter_text: Some(snippet.name.to_owned()),
        })
        .collect()
}

/// Attribute snippets offered while typing an attribute name.
fn attribute_items(context: &Context) -> Vec<CompletionItem> {
    let prefix = context.prefix.as_str();
    let replace = SourceRange::new(context.replace);
    ATTRIBUTE_SNIPPETS
        .iter()
        .filter(|snippet| matches(snippet.label, prefix))
        .map(|snippet| CompletionItem {
            label: snippet.label.to_owned(),
            kind: CompletionItemKind::Attribute,
            detail: Some(snippet.detail.to_owned()),
            documentation: None,
            insert_text: snippet.body.to_owned(),
            insert_text_format: InsertTextFormat::Snippet,
            replace,
            sort_text: Some(format!("5_{}", snippet.label)),
            filter_text: Some(
                snippet
                    .label
                    .split_once(" (")
                    .map_or(snippet.label, |(name, _)| name)
                    .to_owned(),
            ),
        })
        .collect()
}

/// Replaces the replacement range of a context with `text`; used by tests.
#[cfg(test)]
fn apply(context: &Context, text: &str) -> wxml_text::TextRange {
    let start = context.replace.start();
    let end = start.saturating_add(u32::try_from(text.len()).unwrap_or(u32::MAX));
    wxml_text::TextRange::new(start, end).expect("range")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::assert_is_empty)]
mod tests {
    use wxml_text::TextRange;

    use super::*;

    fn context(site: Site, prefix: &str, start: u32, end: u32) -> Context {
        Context {
            site,
            replace: TextRange::new(start, end).expect("range"),
            prefix: prefix.to_owned(),
            open_tags: Vec::new(),
        }
    }

    #[test]
    fn tag_snippets_keep_the_already_typed_angle_bracket() {
        let context = context(
            Site::Tag {
                closing: false,
                name: "vi".to_owned(),
            },
            "vi",
            1,
            3,
        );
        let env = SnippetEnv::new("  <vi", 4);
        let items = items(&context, &env);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.insert_text_format, InsertTextFormat::Snippet);
        assert_eq!(
            item.insert_text, "view>\n    $0\n  </view>",
            "closing indent must match the tag line"
        );
        assert_eq!(item.replace.start(), 1);
        assert_eq!(item.replace.end(), 3);
    }

    #[test]
    fn crlf_documents_get_crlf_snippets() {
        let context = context(
            Site::Tag {
                closing: false,
                name: "view".to_owned(),
            },
            "view",
            1,
            5,
        );
        let env = SnippetEnv::new("<view\r\n  <view", 14);
        let items = items(&context, &env);
        assert_eq!(items[0].insert_text, "view>\r\n    $0\r\n  </view>");
    }

    #[test]
    fn attribute_snippets_carry_placeholders() {
        let context = context(
            Site::AttributeName {
                tag: "view".to_owned(),
                name: "wx:f".to_owned(),
            },
            "wx:f",
            6,
            10,
        );
        let env = SnippetEnv::new("<view wx:f", 10);
        let items = items(&context, &env);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "wx:for (snippet)");
        assert_eq!(
            items[0].insert_text,
            "wx:for=\"{{${1:items}}}\" wx:key=\"${2:index}\""
        );
        assert_eq!(items[0].insert_text_format, InsertTextFormat::Snippet);
    }

    #[test]
    fn closing_and_value_sites_never_offer_snippets() {
        let env = SnippetEnv::new("", 0);
        for site in [
            Site::Tag {
                closing: true,
                name: String::new(),
            },
            Site::AttributeValue {
                tag: "button".to_owned(),
                attribute: "type".to_owned(),
                quoted: true,
            },
            Site::Mustache,
        ] {
            let context = context(site, "", 0, 0);
            assert!(items(&context, &env).is_empty());
        }
    }

    #[test]
    fn snippet_filtering_uses_the_typed_prefix() {
        let env = SnippetEnv::new("", 0);
        let matching = context(
            Site::Tag {
                closing: false,
                name: "te".to_owned(),
            },
            "te",
            1,
            3,
        );
        assert_eq!(items(&matching, &env).len(), 1);
        let none = context(
            Site::Tag {
                closing: false,
                name: "zz".to_owned(),
            },
            "zz",
            1,
            3,
        );
        assert!(items(&none, &env).is_empty());
    }

    #[test]
    fn replacement_ranges_stay_inside_the_original_text() {
        let context = context(
            Site::Tag {
                closing: false,
                name: "view".to_owned(),
            },
            "view",
            1,
            5,
        );
        let range = apply(&context, "view>…</view>");
        assert!(range.contains(1));
        assert!(range.contains(range.end() - 1));
    }
}
