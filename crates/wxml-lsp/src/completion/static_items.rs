//! Maps catalog entries and discovered components to LSP completion items.

use std::collections::BTreeSet;

use wxml_text::{SourceRange, TextRange};
use wxml_types::{CompletionItem, CompletionItemKind, InsertTextFormat, MAX_COMPLETION_ITEMS};

use super::catalog;
use super::context::{Context, Site};
use super::snippets::{self, SnippetContext};
use crate::project_host::ViewComponent;

/// Builds the static item list for one context.
///
/// Returns the items and whether the list had to be capped.
///
/// This wrapper keeps the historical test surface snippet-free; production
/// completion calls [`items_with_snippets`] so snippet support can be gated on
/// the client capability.
#[cfg(test)]
pub(crate) fn items(
    context: &Context,
    components: &[ViewComponent],
) -> (Vec<CompletionItem>, bool) {
    items_with_snippets(context, components, None, "")
}

/// Builds the static item list, optionally adding built-in and user snippets.
///
/// `snippet` is `Some` only when the client advertised snippet support and the
/// workspace catalog is enabled; otherwise snippet items would be rendered as
/// literal placeholder text. `text` is the current document used to verify
/// that a snippet insertion is safe.
pub(crate) fn items_with_snippets(
    context: &Context,
    components: &[ViewComponent],
    snippet: Option<&SnippetContext<'_>>,
    text: &str,
) -> (Vec<CompletionItem>, bool) {
    let mut items = match &context.site {
        Site::Tag { closing, .. } => tag_items(context, components, *closing),
        Site::AttributeName { tag, .. } => attribute_items(context, tag),
        Site::AttributeValue { tag, attribute, .. } => value_items(context, tag, attribute),
        Site::Mustache => Vec::new(),
    };
    if let Some(snippet) = snippet {
        items.extend(snippets::items(context, text, snippet));
    }
    items.sort_by(|left, right| {
        left.sort_text
            .as_deref()
            .unwrap_or(&left.label)
            .cmp(right.sort_text.as_deref().unwrap_or(&right.label))
            .then_with(|| left.label.cmp(&right.label))
    });
    let truncated = items.len() > MAX_COMPLETION_ITEMS;
    items.truncate(MAX_COMPLETION_ITEMS);
    (items, truncated)
}

/// Converts a text range into the source range used by the DTO.
const fn source_range(range: TextRange) -> SourceRange {
    SourceRange::new(range)
}

/// True when `candidate` continues `prefix` (ASCII case-insensitive).
pub(super) fn matches(candidate: &str, prefix: &str) -> bool {
    candidate
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// One completion item with a category sort key.
fn item(
    label: &str,
    kind: CompletionItemKind,
    category: u8,
    detail: Option<String>,
    insert: String,
    replace: SourceRange,
) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind,
        detail,
        documentation: None,
        insert_text: insert,
        insert_text_format: InsertTextFormat::PlainText,
        replace,
        sort_text: Some(format!("{category}_{label}")),
        filter_text: None,
    }
}

/// Tag suggestions: builtin tags plus the current view's custom components.
fn tag_items(
    context: &Context,
    components: &[ViewComponent],
    closing: bool,
) -> Vec<CompletionItem> {
    let replace = source_range(context.replace);
    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    if closing {
        for name in context.open_tags.iter().rev() {
            if !matches(name, prefix) || !seen.insert(name.clone()) {
                continue;
            }
            items.push(item(
                name,
                CompletionItemKind::Tag,
                1,
                Some("Open element".to_owned()),
                name.clone(),
                replace,
            ));
        }
        return items;
    }
    for spec in catalog::all_tags() {
        if !matches(spec.name, prefix) || !seen.insert(spec.name.to_owned()) {
            continue;
        }
        items.push(item(
            spec.name,
            CompletionItemKind::Tag,
            1,
            Some(spec.detail.to_owned()),
            spec.name.to_owned(),
            replace,
        ));
    }
    for component in components {
        if !matches(&component.name, prefix) || !seen.insert(component.name.clone()) {
            continue;
        }
        let detail = component.component.as_ref().map_or_else(
            || format!("Custom component ({})", component.specifier),
            |path| format!("Custom component {}", path.as_str()),
        );
        items.push(item(
            &component.name,
            CompletionItemKind::Tag,
            1,
            Some(detail),
            component.name.clone(),
            replace,
        ));
    }
    items
}

/// Attribute-name suggestions for one tag.
fn attribute_items(context: &Context, tag: &str) -> Vec<CompletionItem> {
    let replace = source_range(context.replace);
    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |name: &str, detail: &str, category: u8| {
        if !matches(name, prefix) || !seen.insert(name.to_owned()) {
            return;
        }
        items.push(item(
            name,
            CompletionItemKind::Attribute,
            category,
            Some(detail.to_owned()),
            name.to_owned(),
            replace,
        ));
    };
    if let Some(spec) = catalog::tag(tag) {
        for attribute in spec.attributes {
            push(attribute.name, attribute.detail, 2);
        }
    }
    for attribute in catalog::GLOBAL_ATTRIBUTES {
        push(attribute.name, attribute.detail, 3);
    }
    for directive in catalog::DIRECTIVE_ATTRIBUTES {
        push(directive.name, directive.detail, 3);
    }
    for event in catalog::EVENT_ATTRIBUTES {
        push(event.name, event.detail, 4);
    }
    for name in catalog::EVENT_NAMES {
        // Only expose generated event names when the user is already typing an
        // event prefix; otherwise the list is dominated by noise.
        if catalog::is_event_prefix(prefix) {
            let attribute = format!("bind{name}");
            push(&attribute, &format!("Bind `{name}`"), 4);
            let attribute = format!("catch{name}");
            push(&attribute, &format!("Bind `{name}` and stop bubbling"), 4);
            let attribute = format!("bind:{name}");
            push(&attribute, &format!("Bind `{name}` (alternate form)"), 4);
        }
    }
    items
}

/// Enumerated attribute-value suggestions.
fn value_items(context: &Context, tag: &str, attribute: &str) -> Vec<CompletionItem> {
    if catalog::is_event_attribute(attribute) {
        return Vec::new();
    }
    let quoted = matches!(context.site, Site::AttributeValue { quoted: true, .. });
    let replace = source_range(context.replace);
    let mut items = Vec::new();
    for value in catalog::values_for(tag, attribute) {
        if !matches(value.value, &context.prefix) {
            continue;
        }
        let insert = if quoted {
            value.value.to_owned()
        } else {
            format!("\"{}\"", value.value)
        };
        items.push(item(
            value.value,
            CompletionItemKind::Value,
            1,
            Some(value.detail.to_owned()),
            insert,
            replace,
        ));
    }
    items
}

#[cfg(test)]
mod tests {
    use wxml_text::TextRange;

    use super::*;
    use crate::completion::context::Context;

    fn context(site: Site, prefix: &str, start: u32, end: u32) -> Context {
        Context {
            site,
            replace: TextRange::new(start, end).expect("range"),
            prefix: prefix.to_owned(),
            open_tags: Vec::new(),
        }
    }

    #[test]
    fn value_items_filter_by_the_typed_prefix() {
        let quoted = |prefix: &str| Context {
            site: Site::AttributeValue {
                tag: "button".to_owned(),
                attribute: "type".to_owned(),
                quoted: true,
            },
            replace: TextRange::new(0, 0).expect("range"),
            prefix: prefix.to_owned(),
            open_tags: Vec::new(),
        };
        let all = value_items(&quoted(""), "button", "type");
        assert_eq!(all.len(), 3);
        let primary = value_items(&quoted("pr"), "button", "type");
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].label, "primary");
        assert_eq!(value_items(&quoted("zzz"), "button", "type").len(), 0);
    }

    #[test]
    fn tag_items_filter_by_prefix_and_include_components() {
        let components = vec![ViewComponent {
            name: "my-card".to_owned(),
            specifier: "/components/card".to_owned(),
            component: None,
        }];
        let (items, truncated) = items(
            &context(
                Site::Tag {
                    closing: false,
                    name: "my".to_owned(),
                },
                "my",
                1,
                3,
            ),
            &components,
        );
        assert!(!truncated);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "my-card");
    }

    #[test]
    fn closing_tags_only_suggest_open_elements() {
        let mut context = context(
            Site::Tag {
                closing: true,
                name: "vi".to_owned(),
            },
            "vi",
            10,
            12,
        );
        context.open_tags = vec!["view".to_owned(), "text".to_owned()];
        let (items, _) = items(&context, &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "view");
    }

    #[test]
    fn attribute_items_include_tag_global_directive_and_events() {
        let (items, _) = items(
            &context(
                Site::AttributeName {
                    tag: "button".to_owned(),
                    name: String::new(),
                },
                "",
                6,
                6,
            ),
            &[],
        );
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"type"));
        assert!(labels.contains(&"id"));
        assert!(labels.contains(&"bindtap"));
    }

    #[test]
    fn generated_event_names_only_appear_after_an_event_prefix() {
        let empty = context(
            Site::AttributeName {
                tag: "view".to_owned(),
                name: String::new(),
            },
            "",
            6,
            6,
        );
        let (empty_items, _) = items(&empty, &[]);
        let labels: Vec<_> = empty_items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"bindtap"));
        assert!(!labels.contains(&"bindlongpress"), "{labels:?}");

        let event = context(
            Site::AttributeName {
                tag: "view".to_owned(),
                name: "bindl".to_owned(),
            },
            "bindl",
            6,
            11,
        );
        let (items, _) = items(&event, &[]);
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"bindlongpress"), "{labels:?}");
    }

    #[test]
    fn matches_accepts_a_prefix_inside_a_non_ascii_name() {
        assert!(matches("my卡片", "my卡"));
        assert!(matches("my卡片", "MY卡"));
        assert!(!matches("my卡片", "xx"));
    }

    #[test]
    fn enum_values_are_isolated_by_tag_and_attribute() {
        let (items, _) = items(
            &context(
                Site::AttributeValue {
                    tag: "picker".to_owned(),
                    attribute: "mode".to_owned(),
                    quoted: true,
                },
                "",
                15,
                15,
            ),
            &[],
        );
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["date", "multiSelector", "region", "selector", "time"]
        );
        assert!(labels.iter().all(|label| *label != "primary"));
    }

    #[test]
    fn unquoted_values_insert_quotes() {
        let (items, _) = items(
            &context(
                Site::AttributeValue {
                    tag: "button".to_owned(),
                    attribute: "type".to_owned(),
                    quoted: false,
                },
                "",
                10,
                10,
            ),
            &[],
        );
        assert_eq!(items[0].insert_text, "\"default\"");
    }
}
