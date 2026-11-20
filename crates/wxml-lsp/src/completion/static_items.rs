//! Maps catalog entries and discovered components to LSP completion items.

use std::collections::BTreeSet;

use wxml_project::ComponentTargetKind;
use wxml_text::{SourceRange, TextRange};
use wxml_types::{CompletionItem, CompletionItemKind, InsertTextFormat, MAX_COMPLETION_ITEMS};

use super::catalog;
use super::context::{AttributeOccurrence, Context, Site};
use super::rules;
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
        Site::AttributeName { tag, .. } => attribute_items(context, tag, components),
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

/// Describes a custom component tag, including honest degradation for
/// unresolved local, plugin and package components.
fn component_detail(component: &ViewComponent) -> String {
    match component.kind {
        ComponentTargetKind::Local => {
            let mut detail = component.wxml.as_ref().map_or_else(
                || {
                    component.component.as_ref().map_or_else(
                        || format!("自定义组件（{}）", component.specifier),
                        |base| format!("自定义组件 {}（未找到目标）", base.as_str()),
                    )
                },
                |wxml| format!("自定义组件 {}", wxml.as_str()),
            );
            if !component.property_facts.complete {
                detail.push_str("（属性列表可能不完整）");
            }
            detail
        }
        ComponentTargetKind::Plugin => "插件组件（本地元数据不可用）".to_owned(),
        ComponentTargetKind::Package => component.wxml.as_ref().map_or_else(
            || "包组件（本地元数据不可用）".to_owned(),
            |wxml| {
                let mut detail = format!("包组件 {}", wxml.as_str());
                if !component.property_facts.complete {
                    detail.push_str("（属性列表可能不完整）");
                }
                detail
            },
        ),
        _ => format!("自定义组件（{}）", component.specifier),
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
                Some("未闭合的元素".to_owned()),
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
    is_declared_elsewhere(context, |written| written.eq_ignore_ascii_case(name))
}

/// Returns whether a declared component property is already present in another
/// spelling of the same name.
///
/// Declared camelCase and written kebab-case names refer to the same member in
/// both directions; see [`wxml_project::component_name_eq`].
fn component_attribute_declared(context: &Context, declared: &str) -> bool {
    is_declared_elsewhere(context, |written| {
        wxml_project::component_name_eq(declared, written)
    })
}

/// Returns whether one matching attribute is declared outside the edited token.
fn is_declared_elsewhere(context: &Context, same: impl Fn(&str) -> bool) -> bool {
    let mut current = false;
    let mut other = false;
    for occurrence in &context.attributes {
        if !same(&occurrence.name) {
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

/// Returns whether a declared component property continues the typed prefix.
///
/// Both sides are canonicalized first, so `card-` matches a declared
/// `cardTitle` and `cardT` matches a declared `card-title`.
fn component_property_matches(declared: &str, prefix: &str) -> bool {
    matches(declared, prefix)
        || matches(
            &wxml_project::canonical_component_name(declared),
            &wxml_project::canonical_component_name(prefix),
        )
}

/// Returns the spelling to insert for one declared component property.
///
/// A typed prefix that already contains `-` keeps the kebab style; any other
/// prefix uses the declaration's camelCase style.
fn component_property_insert(declared: &str, prefix: &str) -> String {
    let camel = wxml_project::canonical_component_name(declared);
    if prefix.contains('-') {
        crate::semantics::target::kebab_case(&camel)
    } else {
        camel
    }
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
        let prefix = context.prefix.as_str();
        let insert = component_property_insert(&property.name, prefix);
        if !component_property_matches(&property.name, prefix)
            || seen.contains(&insert)
            || component_attribute_declared(context, &property.name)
        {
            continue;
        }
        let hidden = rules::is_hidden(tag, &property.name, &context.attributes);
        if hidden && !is_editing_attribute(context) {
            continue;
        }
        seen.insert(insert.clone());
        let mut candidate = item(
            &property.name,
            CompletionItemKind::Property,
            2,
            Some(if hidden {
                format!("{} (unavailable in the current mode)", property.type_text)
            } else {
                property.type_text.clone()
            }),
            insert.clone(),
            replace,
        );
        if insert != property.name {
            // Keeps client-side filtering aligned with the spelling we insert.
            candidate.filter_text = Some(insert);
        }
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
    let declared_events = components
        .iter()
        .find(|component| component.name == tag)
        .and_then(|component| {
            crate::backend_tsgo::event_completion::declared_event_items(
                component,
                context.prefix.as_str(),
                replace.start(),
            )
        });
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
    match declared_events {
        // A declared component's event list replaces the generic generated
        // names: the declaration is the authoritative source for this tag.
        Some(events) => items.extend(events),
        None => {
            for name in catalog::EVENT_NAMES {
                // Only expose generated event names when the user is already typing an
                // event prefix; otherwise the list is dominated by noise.
                if catalog::is_event_prefix(prefix) {
                    let attribute = format!("bind{name}");
                    let detail = format!("绑定 `{name}` 事件");
                    push_attribute(
                        &mut items, &mut seen, context, tag, replace, &attribute, &detail, 4,
                    );
                    let attribute = format!("catch{name}");
                    let detail = format!("绑定 `{name}` 事件并阻止冒泡");
                    push_attribute(
                        &mut items, &mut seen, context, tag, replace, &attribute, &detail, 4,
                    );
                    let attribute = format!("bind:{name}");
                    let detail = format!("绑定 `{name}` 事件（另一种写法）");
                    push_attribute(
                        &mut items, &mut seen, context, tag, replace, &attribute, &detail, 4,
                    );
                }
            }
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
                source: wxml_project::PropertySource::Script,
            },
            typing: None,
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

    /// Builds one declared component used by the event-name tests.
    fn declared_component(
        events: Vec<wxml_project::typing::TypingEvent>,
        complete: bool,
    ) -> ViewComponent {
        ViewComponent {
            name: "van-field".to_owned(),
            specifier: "/components/field".to_owned(),
            kind: ComponentTargetKind::Local,
            component: None,
            wxml: None,
            json: None,
            property_facts: wxml_project::ComponentPropertyFacts {
                properties: Vec::new(),
                complete: true,
                reason: None,
                source: wxml_project::PropertySource::Script,
            },
            typing: Some(std::sync::Arc::new(wxml_project::typing::TypingComponent {
                props: Vec::new(),
                events,
                complete,
                source: wxml_project::ProjectPath::new("components.d.ts").expect("path"),
            })),
            origin: crate::project_host::ComponentOrigin::Local { file: None },
        }
    }

    /// Builds one declared event.
    fn declared_event(name: &str, detail: Option<&str>) -> wxml_project::typing::TypingEvent {
        wxml_project::typing::TypingEvent {
            name: name.to_owned(),
            detail_text: detail.map(str::to_owned),
            detail: detail.map(wxml_project::typing::TypeText::parse),
            proven: true,
        }
    }

    #[test]
    fn declared_events_are_suggested_and_replace_only_the_typed_fragment() {
        let components = vec![declared_component(
            vec![
                declared_event("change", Some("{ index: number }")),
                declared_event("input", None),
            ],
            true,
        )];
        // `<van-field bind:cha|>`: the name starts at 11, `bind:` is 5 bytes.
        let (items, _) = items(
            &context(
                Site::AttributeName {
                    tag: "van-field".to_owned(),
                    name: "bind:cha".to_owned(),
                },
                "bind:cha",
                11,
                19,
            ),
            &components,
        );
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["change"], "{labels:?}");
        let change = items
            .iter()
            .find(|item| item.label == "change")
            .expect("change item");
        assert_eq!(change.insert_text, "change");
        assert_eq!(change.kind, CompletionItemKind::Event);
        assert_eq!((change.replace.start(), change.replace.end()), (16, 19));
        assert_eq!(
            change.detail.as_deref(),
            Some("(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void")
        );
        // The generic generated full-name candidates are replaced by the
        // declaration, so no duplicate `bind:change` item appears.
        assert!(!labels.contains(&"bind:change"), "{labels:?}");
    }

    #[test]
    fn an_empty_event_name_fragment_yields_a_zero_width_replacement() {
        let components = vec![declared_component(
            vec![declared_event("input", None)],
            true,
        )];
        let (items, _) = items(
            &context(
                Site::AttributeName {
                    tag: "van-field".to_owned(),
                    name: "bind:".to_owned(),
                },
                "bind:",
                7,
                12,
            ),
            &components,
        );
        let item = items
            .iter()
            .find(|item| item.label == "input")
            .expect("declared event item");
        assert_eq!((item.replace.start(), item.replace.end()), (12, 12));
    }

    #[test]
    fn undeclared_and_incomplete_components_keep_the_generic_candidates() {
        let incomplete = vec![declared_component(
            vec![declared_event("change", None)],
            false,
        )];
        for components in [Vec::new(), incomplete] {
            let (items, _) = items(
                &context(
                    Site::AttributeName {
                        tag: "van-field".to_owned(),
                        name: "bind:".to_owned(),
                    },
                    "bind:",
                    7,
                    12,
                ),
                &components,
            );
            let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
            assert!(labels.contains(&"bind:change"), "{labels:?}");
            assert!(!labels.contains(&"change"), "{labels:?}");
        }
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
                source: wxml_project::PropertySource::Script,
            },
            typing: None,
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

    fn property_component(properties: &[&str]) -> ViewComponent {
        ViewComponent {
            name: "item".to_owned(),
            specifier: "../../components/item/item".to_owned(),
            kind: ComponentTargetKind::Local,
            component: None,
            wxml: None,
            json: None,
            property_facts: wxml_project::ComponentPropertyFacts {
                properties: properties
                    .iter()
                    .map(|name| wxml_project::ComponentProperty {
                        name: (*name).to_owned(),
                        type_text: "string".to_owned(),
                        default_text: None,
                        documentation: None,
                        script: wxml_project::ProjectPath::new("components/item/item.ts")
                            .expect("path"),
                        range: TextRange::new(0, 0).expect("range"),
                    })
                    .collect(),
                complete: true,
                reason: None,
                source: wxml_project::PropertySource::Script,
            },
            typing: None,
            origin: crate::project_host::ComponentOrigin::Local { file: None },
        }
    }

    fn property_context(
        prefix: &str,
        start: u32,
        end: u32,
        attributes: Vec<AttributeOccurrence>,
    ) -> Context {
        Context {
            site: Site::AttributeName {
                tag: "item".to_owned(),
                name: prefix.to_owned(),
            },
            replace: TextRange::new(start, end).expect("range"),
            prefix: prefix.to_owned(),
            open_tags: Vec::new(),
            attributes,
        }
    }

    fn property_item<'a>(items: &'a [CompletionItem], label: &str) -> Option<&'a CompletionItem> {
        items.iter().find(|item| item.label == label)
    }

    #[test]
    fn component_property_completion_follows_the_typed_spelling() {
        let camel = [property_component(&["cardTitle"])];
        let kebab_prefix = property_context("card-", 0, 5, Vec::new());
        let (listed, _) = items(&kebab_prefix, &camel);
        let item = property_item(&listed, "cardTitle").expect("kebab prefix item");
        assert_eq!(item.insert_text, "card-title");
        assert_eq!(item.filter_text.as_deref(), Some("card-title"));

        let camel_prefix = property_context("cardT", 0, 5, Vec::new());
        let (listed, _) = items(&camel_prefix, &camel);
        let item = property_item(&listed, "cardTitle").expect("camel prefix item");
        assert_eq!(item.insert_text, "cardTitle");
        assert_eq!(item.filter_text, None);

        let kebab = [property_component(&["card-title"])];
        let camel_prefix = property_context("cardT", 0, 5, Vec::new());
        let (listed, _) = items(&camel_prefix, &kebab);
        let item = property_item(&listed, "card-title").expect("camel item");
        assert_eq!(item.insert_text, "cardTitle");

        let kebab_prefix = property_context("card-", 0, 5, Vec::new());
        let (listed, _) = items(&kebab_prefix, &kebab);
        let item = property_item(&listed, "card-title").expect("kebab item");
        assert_eq!(item.insert_text, "card-title");
    }

    #[test]
    fn component_property_completion_hides_any_declared_spelling() {
        let camel = [property_component(&["cardTitle"])];
        let context = property_context(
            "cardT",
            20,
            25,
            vec![
                AttributeOccurrence {
                    name: "card-title".to_owned(),
                    value: None,
                    range: TextRange::new(0, 10).expect("range"),
                },
                AttributeOccurrence {
                    name: "cardT".to_owned(),
                    value: None,
                    range: TextRange::new(20, 25).expect("range"),
                },
            ],
        );
        let (listed, _) = items(&context, &camel);
        assert!(property_item(&listed, "cardTitle").is_none());

        let editing = property_context(
            "card-",
            20,
            25,
            vec![AttributeOccurrence {
                name: "card-".to_owned(),
                value: None,
                range: TextRange::new(20, 25).expect("range"),
            }],
        );
        let (listed, _) = items(&editing, &camel);
        let item = property_item(&listed, "cardTitle").expect("the edited token stays");
        assert_eq!(item.insert_text, "card-title");
    }

    #[test]
    fn component_property_completion_keeps_non_equivalent_spellings_distinct() {
        let components = [property_component(&["someProp"])];
        let snake = property_context("some_", 0, 5, Vec::new());
        let (listed, _) = items(&snake, &components);
        assert!(property_item(&listed, "someProp").is_none());
    }
}
