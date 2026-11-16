//! Maps catalog entries and discovered components to LSP completion items.

use std::collections::BTreeSet;

use wxml_project::ComponentTargetKind;
use wxml_text::{SourceRange, TextRange};
use wxml_types::{CompletionItem, CompletionItemKind, InsertTextFormat, MAX_COMPLETION_ITEMS};

use super::catalog;
use super::context::{AttributeOccurrence, Context, Site};
use super::rules;
use crate::project_host::ViewComponent;

/// Builds the static item list for one context.
///
/// Returns the items and whether the list had to be capped.
pub(crate) fn items(
    context: &Context,
    components: &[ViewComponent],
) -> (Vec<CompletionItem>, bool) {
    let mut items = match &context.site {
        Site::Tag { closing, .. } => tag_items(context, components, *closing),
        Site::AttributeName { tag, .. } => attribute_items(context, tag, components),
        Site::AttributeValue { tag, attribute, .. } => value_items(context, tag, attribute),
        Site::Mustache => Vec::new(),
    };
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
fn matches(candidate: &str, prefix: &str) -> bool {
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

/// Describes a custom component tag, including honest degradation for
/// unresolved local, plugin and package components.
fn component_detail(component: &ViewComponent) -> String {
    match component.kind {
        ComponentTargetKind::Local => {
            let mut detail = component.wxml.as_ref().map_or_else(
                || {
                    component.component.as_ref().map_or_else(
                        || format!("Custom component ({})", component.specifier),
                        |base| format!("Custom component {} (target not found)", base.as_str()),
                    )
                },
                |wxml| format!("Custom component {}", wxml.as_str()),
            );
            if !component.property_facts.complete {
                detail.push_str(" (property list incomplete)");
            }
            detail
        }
        ComponentTargetKind::Plugin => "Plugin component (local metadata unavailable)".to_owned(),
        ComponentTargetKind::Package => component.wxml.as_ref().map_or_else(
            || "Package component (local metadata unavailable)".to_owned(),
            |wxml| {
                let mut detail = format!("Package component {}", wxml.as_str());
                if !component.property_facts.complete {
                    detail.push_str(" (property list incomplete)");
                }
                detail
            },
        ),
        _ => format!("Custom component ({})", component.specifier),
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
        items.push(item(
            &component.name,
            CompletionItemKind::Tag,
            1,
            Some(component_detail(component)),
            component.name.clone(),
            replace,
        ));
    }
    items
}

/// Returns whether a completion range overlaps an existing attribute token.
const fn overlaps(range: TextRange, replace: TextRange) -> bool {
    range.start() < replace.end() && replace.start() < range.end()
}

/// Returns whether the attribute under the cursor is the one being edited.
///
/// A zero-width replacement at the token start is treated as editing it, so
/// moving the cursor to the beginning of an existing attribute still offers
/// that attribute.
const fn is_current(occurrence: &AttributeOccurrence, replace: TextRange) -> bool {
    overlaps(occurrence.range, replace)
        || (replace.is_empty() && occurrence.range.start() == replace.start())
}

/// Returns whether the cursor is on an attribute-name token.
fn is_editing_attribute(context: &Context) -> bool {
    context
        .attributes
        .iter()
        .any(|occurrence| is_current(occurrence, context.replace))
}

/// Returns whether a unique attribute is already present elsewhere in the tag.
fn already_declared(context: &Context, name: &str) -> bool {
    let mut current = false;
    let mut other = false;
    for occurrence in &context.attributes {
        if !occurrence.name.eq_ignore_ascii_case(name) {
            continue;
        }
        if is_current(occurrence, context.replace) {
            current = true;
        } else {
            other = true;
        }
    }
    other && !current
}

/// Adds one catalog attribute unless it is filtered out.
#[allow(clippy::too_many_arguments)]
fn push_attribute(
    items: &mut Vec<CompletionItem>,
    seen: &mut BTreeSet<String>,
    context: &Context,
    tag: &str,
    replace: SourceRange,
    name: &str,
    detail: &str,
    category: u8,
) {
    if !matches(name, context.prefix.as_str())
        || seen.contains(name)
        || already_declared(context, name)
    {
        return;
    }
    let hidden = rules::is_hidden(tag, name, &context.attributes);
    if hidden && !is_editing_attribute(context) {
        return;
    }
    seen.insert(name.to_owned());
    let detail = if hidden {
        format!("{detail} (unavailable in the current mode)")
    } else {
        detail.to_owned()
    };
    items.push(item(
        name,
        CompletionItemKind::Attribute,
        category,
        Some(detail),
        name.to_owned(),
        replace,
    ));
}

/// Adds one custom component's properties to an attribute completion list.
fn push_component_properties(
    context: &Context,
    tag: &str,
    component: &ViewComponent,
    items: &mut Vec<CompletionItem>,
    seen: &mut BTreeSet<String>,
    replace: SourceRange,
) {
    for property in &component.property_facts.properties {
        if !matches(&property.name, context.prefix.as_str())
            || seen.contains(&property.name)
            || already_declared(context, &property.name)
        {
            continue;
        }
        let hidden = rules::is_hidden(tag, &property.name, &context.attributes);
        if hidden && !is_editing_attribute(context) {
            continue;
        }
        seen.insert(property.name.clone());
        let mut candidate = item(
            &property.name,
            CompletionItemKind::Property,
            2,
            Some(if hidden {
                format!("{} (unavailable in the current mode)", property.type_text)
            } else {
                property.type_text.clone()
            }),
            property.name.clone(),
            replace,
        );
        candidate.documentation = property
            .documentation
            .as_ref()
            .map(|documentation| crate::handlers::markdown_escape(documentation));
        items.push(candidate);
    }
}

/// Attribute-name suggestions for one tag.
fn attribute_items(
    context: &Context,
    tag: &str,
    components: &[ViewComponent],
) -> Vec<CompletionItem> {
    let replace = source_range(context.replace);
    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(spec) = catalog::tag(tag) {
        for attribute in spec.attributes {
            push_attribute(
                &mut items,
                &mut seen,
                context,
                tag,
                replace,
                attribute.name,
                attribute.detail,
                2,
            );
        }
    }
    if let Some(component) = components.iter().find(|component| component.name == tag) {
        push_component_properties(context, tag, component, &mut items, &mut seen, replace);
    }
    for attribute in catalog::GLOBAL_ATTRIBUTES {
        push_attribute(
            &mut items,
            &mut seen,
            context,
            tag,
            replace,
            attribute.name,
            attribute.detail,
            3,
        );
    }
    for directive in catalog::DIRECTIVE_ATTRIBUTES {
        push_attribute(
            &mut items,
            &mut seen,
            context,
            tag,
            replace,
            directive.name,
            directive.detail,
            3,
        );
    }
    for event in catalog::EVENT_ATTRIBUTES {
        push_attribute(
            &mut items,
            &mut seen,
            context,
            tag,
            replace,
            event.name,
            event.detail,
            4,
        );
    }
    for name in catalog::EVENT_NAMES {
        // Only expose generated event names when the user is already typing an
        // event prefix; otherwise the list is dominated by noise.
        if catalog::is_event_prefix(prefix) {
            let attribute = format!("bind{name}");
            let detail = format!("Bind `{name}`");
            push_attribute(
                &mut items, &mut seen, context, tag, replace, &attribute, &detail, 4,
            );
            let attribute = format!("catch{name}");
            let detail = format!("Bind `{name}` and stop bubbling");
            push_attribute(
                &mut items, &mut seen, context, tag, replace, &attribute, &detail, 4,
            );
            let attribute = format!("bind:{name}");
            let detail = format!("Bind `{name}` (alternate form)");
            push_attribute(
                &mut items, &mut seen, context, tag, replace, &attribute, &detail, 4,
            );
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
            attributes: Vec::new(),
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
            attributes: Vec::new(),
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
            kind: ComponentTargetKind::Local,
            component: None,
            wxml: None,
            json: None,
            property_facts: wxml_project::ComponentPropertyFacts {
                properties: Vec::new(),
                complete: true,
                reason: None,
            },
            origin: crate::project_host::ComponentOrigin::App {
                file: wxml_project::ProjectPath::new("app.json").expect("path"),
            },
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

    fn picker_context(
        prefix: &str,
        start: u32,
        end: u32,
        attributes: Vec<AttributeOccurrence>,
    ) -> Context {
        let mut context = context(
            Site::AttributeName {
                tag: "picker".to_owned(),
                name: prefix.to_owned(),
            },
            prefix,
            start,
            end,
        );
        context.attributes = attributes;
        context
    }

    #[test]
    fn picker_mode_conditions_are_data_driven() {
        let date = picker_context(
            "",
            30,
            30,
            vec![AttributeOccurrence {
                name: "mode".to_owned(),
                value: Some("date".to_owned()),
                range: TextRange::new(8, 12).expect("range"),
            }],
        );
        let (date_items, _) = items(&date, &[]);
        let labels: Vec<_> = date_items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"start"), "{labels:?}");
        assert!(labels.contains(&"end"), "{labels:?}");
        assert!(labels.contains(&"fields"), "{labels:?}");
        assert!(!labels.contains(&"range"), "{labels:?}");
        assert!(!labels.contains(&"range-key"), "{labels:?}");
        assert!(!labels.contains(&"bindcolumnchange"), "{labels:?}");

        let selector = picker_context(
            "",
            30,
            30,
            vec![AttributeOccurrence {
                name: "mode".to_owned(),
                value: Some("selector".to_owned()),
                range: TextRange::new(8, 12).expect("range"),
            }],
        );
        let (selector_items, _) = items(&selector, &[]);
        let labels: Vec<_> = selector_items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert!(labels.contains(&"range"));
        assert!(!labels.contains(&"start"));
        assert!(!labels.contains(&"end"));

        let dynamic = picker_context(
            "",
            30,
            30,
            vec![AttributeOccurrence {
                name: "mode".to_owned(),
                value: Some("{{ pickerMode }}".to_owned()),
                range: TextRange::new(8, 12).expect("range"),
            }],
        );
        let (dynamic_items, _) = items(&dynamic, &[]);
        let labels: Vec<_> = dynamic_items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert!(labels.contains(&"range"));
        assert!(labels.contains(&"start"));
    }

    #[test]
    fn already_declared_attributes_are_hidden_but_the_current_one_is_not() {
        let context = picker_context(
            "ra",
            30,
            32,
            vec![
                AttributeOccurrence {
                    name: "mode".to_owned(),
                    value: Some("selector".to_owned()),
                    range: TextRange::new(8, 12).expect("range"),
                },
                AttributeOccurrence {
                    name: "range".to_owned(),
                    value: Some("a".to_owned()),
                    range: TextRange::new(20, 25).expect("range"),
                },
                AttributeOccurrence {
                    name: "ra".to_owned(),
                    value: None,
                    range: TextRange::new(30, 32).expect("range"),
                },
            ],
        );
        let (declared_items, _) = items(&context, &[]);
        let labels: Vec<_> = declared_items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert!(labels.contains(&"range-key"), "{labels:?}");
        assert!(!labels.contains(&"range"), "{labels:?}");

        let editing = picker_context(
            "mode",
            10,
            14,
            vec![AttributeOccurrence {
                name: "mode".to_owned(),
                value: Some("selector".to_owned()),
                range: TextRange::new(10, 14).expect("range"),
            }],
        );
        let (editing_items, _) = items(&editing, &[]);
        let labels: Vec<_> = editing_items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        assert!(labels.contains(&"mode"), "{labels:?}");
    }

    #[test]
    fn the_attribute_under_the_cursor_survives_a_mode_hidden_rule() {
        let context = picker_context(
            "star",
            24,
            28,
            vec![
                AttributeOccurrence {
                    name: "mode".to_owned(),
                    value: Some("selector".to_owned()),
                    range: TextRange::new(8, 12).expect("range"),
                },
                AttributeOccurrence {
                    name: "star".to_owned(),
                    value: None,
                    range: TextRange::new(24, 28).expect("range"),
                },
            ],
        );
        let (items, _) = items(&context, &[]);
        let start = items
            .iter()
            .find(|item| item.label == "start")
            .expect("editing the hidden attribute must keep it");
        assert!(
            start
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("unavailable")),
            "{start:?}"
        );
    }

    #[test]
    fn input_confirm_type_follows_the_declared_type() {
        let context = context(
            Site::AttributeName {
                tag: "input".to_owned(),
                name: String::new(),
            },
            "",
            6,
            6,
        );
        let (default_items, _) = items(&context, &[]);
        assert!(
            default_items
                .iter()
                .any(|item| item.label == "confirm-type"),
            "default text input keeps confirm-type"
        );

        let mut digit = context;
        digit.attributes = vec![AttributeOccurrence {
            name: "type".to_owned(),
            value: Some("digit".to_owned()),
            range: TextRange::new(7, 11).expect("range"),
        }];
        let (digit_items, _) = items(&digit, &[]);
        assert!(
            !digit_items.iter().any(|item| item.label == "confirm-type"),
            "non-text input types hide confirm-type"
        );
    }

    #[test]
    fn component_properties_complete_with_type_and_documentation() {
        let components = vec![ViewComponent {
            name: "my-card".to_owned(),
            specifier: "../../components/my-card/index".to_owned(),
            kind: ComponentTargetKind::Local,
            component: Some(
                wxml_project::ProjectPath::new("components/my-card/index").expect("path"),
            ),
            wxml: Some(
                wxml_project::ProjectPath::new("components/my-card/index.wxml").expect("path"),
            ),
            json: None,
            property_facts: wxml_project::ComponentPropertyFacts {
                properties: vec![wxml_project::ComponentProperty {
                    name: "title".to_owned(),
                    type_text: "string | number".to_owned(),
                    default_text: None,
                    documentation: Some("标题".to_owned()),
                    script: wxml_project::ProjectPath::new("components/my-card/index.ts")
                        .expect("path"),
                    range: TextRange::new(10, 15).expect("range"),
                }],
                complete: true,
                reason: None,
            },
            origin: crate::project_host::ComponentOrigin::Local { file: None },
        }];
        let context = context(
            Site::AttributeName {
                tag: "my-card".to_owned(),
                name: String::new(),
            },
            "",
            10,
            10,
        );
        let (items_with_property, _) = items(&context, &components);
        let title = items_with_property
            .iter()
            .find(|item| item.label == "title")
            .expect("component property");
        assert_eq!(title.detail.as_deref(), Some("string | number"));
        assert_eq!(title.documentation.as_deref(), Some("标题"));

        let mut with_title = context;
        with_title.attributes = vec![AttributeOccurrence {
            name: "title".to_owned(),
            value: Some("x".to_owned()),
            range: TextRange::new(11, 16).expect("range"),
        }];
        let (items_with_title, _) = items(&with_title, &components);
        assert!(!items_with_title.iter().any(|item| item.label == "title"));
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
