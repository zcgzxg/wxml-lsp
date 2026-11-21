//! Static Markdown documentation for tags, attributes and values.

use std::fmt::Write as _;

use wxml_project::ComponentTargetKind;
use wxml_project::typing::component_name_eq;
use wxml_project::typing_contract::expected_signature;

use super::component;
use crate::completion::catalog;
use crate::project_host::ViewComponent;

/// Builds documentation for a tag name.
pub(super) fn tag_document(components: &[ViewComponent], name: &str) -> Option<String> {
    if let Some(spec) = catalog::tag(name) {
        return Some(format!(
            "### {}\n\n{}\n\n[官方文档]({})",
            escape(name),
            escape(spec.detail),
            catalog::docs_url(name)
        ));
    }
    let component = component(components, name)?;
    let mut document = format!("### {}\n\n", escape(name));
    match component.kind {
        ComponentTargetKind::Local => {
            document.push_str("自定义组件。\n\n");
            if let Some(file) = component.origin.file() {
                let _ = write!(document, "声明位置：`{}`。\n\n", escape(file.as_str()));
            }
            if let Some(wxml) = component.wxml.as_ref() {
                let _ = write!(document, "定义：`{}`。\n\n", escape(wxml.as_str()));
            } else if let Some(base) = component.component.as_ref() {
                let _ = write!(document, "未能解析组件路径 `{}`。", escape(base.as_str()));
                return Some(document.trim_end().to_owned());
            } else {
                document.push_str("未能解析组件路径。");
                return Some(document.trim_end().to_owned());
            }
        }
        ComponentTargetKind::Plugin => {
            let _ = write!(
                document,
                "插件组件（`{}`）。\n\n",
                escape(&component.specifier)
            );
        }
        ComponentTargetKind::Package => {
            let _ = write!(
                document,
                "包组件（`{}`）。\n\n",
                escape(&component.specifier)
            );
            if let Some(wxml) = component.wxml.as_ref() {
                let _ = write!(document, "本地构建：`{}`。\n\n", escape(wxml.as_str()));
            }
        }
        _ => {
            let _ = write!(
                document,
                "自定义组件（`{}`）。\n\n",
                escape(&component.specifier)
            );
        }
    }
    if !component.property_facts.properties.is_empty() {
        if component.property_facts.complete {
            document.push_str("属性：\n\n");
        } else {
            document.push_str("属性（静态提取可能不完整）：\n\n");
        }
        for property in &component.property_facts.properties {
            let _ = write!(
                document,
                "- `{}`: `{}`",
                escape(&property.name),
                escape(&property.type_text)
            );
            if let Some(documentation) = property.documentation.as_ref() {
                let _ = write!(document, " - {}", escape(documentation));
            }
            document.push('\n');
        }
    } else if component.wxml.is_some() && component.property_facts.complete {
        document.push_str("未找到静态声明的属性。");
    }
    Some(document.trim_end().to_owned())
}

/// Builds documentation for an attribute name.
pub(super) fn attribute_document(
    components: &[ViewComponent],
    tag: &str,
    name: &str,
) -> Option<String> {
    if catalog::tag(tag).is_some() {
        return shared_attribute_document(tag, name);
    }
    if let Some(component) = component(components, tag) {
        if let Some(property) = component
            .property_facts
            .properties
            .iter()
            .find(|property| component_name_eq(&property.name, name))
        {
            return Some(property_document(tag, property));
        }
        if let Some(document) = declared_event_document(component, name) {
            return Some(document);
        }
        if let Some(document) = shared_attribute_document(tag, name) {
            return Some(document);
        }
        // A resolved local component may have properties the static extractor
        // cannot see (Behavior, spreads, wrappers such as `VantComponent`).
        // With complete facts, leave the unknown name to the backend instead
        // of claiming it does not exist. With incomplete facts, answer with a
        // short honest hint: returning nothing would leave the user with no
        // hover at all (for example on `van-button`'s `round`).
        if component.wxml.is_some() && component.property_facts.complete {
            return None;
        }
        return Some(format!("### {}\n\n暂无属性说明。", escape(name)));
    }
    shared_attribute_document(tag, name)
}

/// Builds documentation for catalog attributes, directives and events.
fn shared_attribute_document(tag: &str, name: &str) -> Option<String> {
    if let Some(spec) = catalog::directive(name) {
        return Some(format!(
            "### {}\n\n{}\n\nWXML 指令。",
            escape(name),
            escape(spec.detail)
        ));
    }
    if let Some(spec) = catalog::event_attribute(name) {
        return Some(event_document(name, spec.detail));
    }
    if catalog::is_event_attribute(name) {
        let event = event_name(name).unwrap_or("event");
        return Some(event_document(name, &format!("绑定 `{event}` 事件。")));
    }
    if let Some(spec) = catalog::tag_attribute(tag, name) {
        let mut document = format!(
            "### {}\n\n{}\n\n`{}` 属性。",
            escape(name),
            escape(spec.detail),
            escape(tag)
        );
        let values = catalog::values_for(tag, name);
        if !values.is_empty() {
            document.push_str("\n\n可选值：\n\n");
            for value in values {
                let _ = writeln!(
                    document,
                    "- `{}`：{}",
                    escape(value.value),
                    escape(value.detail)
                );
            }
        }
        let _ = write!(document, "\n[官方文档]({})", catalog::docs_url(tag));
        return Some(document);
    }
    if let Some(spec) = catalog::global_attribute(name) {
        return Some(format!(
            "### {}\n\n{}\n\n全局属性。",
            escape(name),
            escape(spec.detail)
        ));
    }
    None
}

/// Builds documentation for an event attribute.
fn event_document(name: &str, detail: &str) -> String {
    format!("### {}\n\n{}", escape(name), escape(detail))
}

/// Builds documentation for a declared component event attribute.
///
/// Only a declared event produces a signature. The declaration is found through
/// [`TypingComponent::event_named`](wxml_project::typing::TypingComponent::event_named),
/// so `bind:some-event` and `bindsomeEvent` both reach a declared `someEvent`
/// while the hover keeps the attribute as written. The detail text is the
/// declaration's own text (never a guessed type); an unproven detail keeps it
/// and says so explicitly. A declared `JSDoc` description, when present, is
/// appended as its own paragraph.
fn declared_event_document(component: &ViewComponent, name: &str) -> Option<String> {
    let (_, event) = wxml_backend_tsgo::script::events::parse_event_attribute(name)?;
    let typing = component.typing.as_deref()?;
    let declared = typing.event_named(&event)?;
    let signature = expected_signature(declared.detail_text.as_deref());
    // The signature is shown verbatim inside a fenced block: escaping it would
    // render `&lt;` instead of the declared generic arguments. Backticks and
    // line breaks are removed so the fence cannot be closed early.
    let signature = signature.replace(['\r', '\n'], " ").replace('`', "'");
    let mut document = format!(
        "### {}\n\n```ts\n{}\n```\n\n组件 `{}` 声明的事件处理函数签名。",
        escape(name),
        signature,
        escape(&component.name)
    );
    if !declared.proven {
        document.push_str("\n\n（detail 类型未证明）");
    }
    if let Some(documentation) = declared.documentation.as_ref() {
        let _ = write!(document, "\n\n{}", escape(documentation));
    }
    Some(document)
}

/// Builds documentation for one component property.
fn property_document(tag: &str, property: &wxml_project::ComponentProperty) -> String {
    let mut document = format!(
        "### {}\n\n`{}`，`{}` 的组件属性。\n\n声明位置：`{}`。",
        escape(&property.name),
        escape(&property.type_text),
        escape(tag),
        escape(property.script.as_str())
    );
    if let Some(default) = property.default_text.as_ref() {
        let _ = write!(document, "\n\n默认值：`{}`。", escape(default));
    }
    if let Some(documentation) = property.documentation.as_ref() {
        let _ = write!(document, "\n\n{}", escape(documentation));
    }
    document
}

/// Builds documentation for a literal attribute value.
pub(super) fn value_document(
    components: &[ViewComponent],
    tag: &str,
    attribute: &str,
    value: &str,
) -> Option<String> {
    if catalog::tag(tag).is_none() {
        if let Some(component) = component(components, tag) {
            if let Some(property) = component
                .property_facts
                .properties
                .iter()
                .find(|property| component_name_eq(&property.name, attribute))
            {
                let mut document = format!("### {}\n\n", escape(value));
                if let Some(documentation) = property.documentation.as_ref() {
                    let _ = write!(document, "{}\n\n", escape(documentation));
                }
                let _ = write!(
                    document,
                    "`{}`，`{}` 的 `{}` 取值。",
                    escape(&property.type_text),
                    escape(tag),
                    escape(attribute)
                );
                return Some(document);
            }
            return None;
        }
    }
    if catalog::is_event_attribute(attribute) {
        return None;
    }
    let value_spec = catalog::values_for(tag, attribute)
        .iter()
        .find(|candidate| candidate.value == value)?;
    Some(format!(
        "### {}\n\n{}\n\n`{}` 的 `{}` 取值。",
        escape(value),
        escape(value_spec.detail),
        escape(tag),
        escape(attribute)
    ))
}

/// Extracts the event name from a binding attribute.
fn event_name(attribute: &str) -> Option<&str> {
    const PREFIXES: [&str; 5] = ["capture-bind", "capture-catch", "mut-bind", "bind", "catch"];
    for prefix in PREFIXES {
        if let Some(rest) = attribute.strip_prefix(prefix) {
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

/// Escapes untrusted documentation and collapses line breaks.
///
/// Paths and names keep their hyphens, dots and slashes so project-relative
/// targets stay readable; Markdown link/emphasis/HTML punctuation is escaped
/// and control or bidirectional format characters are dropped.
fn escape(text: &str) -> String {
    let text = text.replace(['\r', '\n'], " ");
    let mut escaped = String::new();
    for character in text.chars().take(1_024) {
        if is_unsafe(character) {
            continue;
        }
        match character {
            '\\' | '`' | '[' | ']' | '(' | ')' | '!' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Returns `true` for control and bidirectional formatting characters.
fn is_unsafe(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{feff}'
        )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Arc;

    use super::*;
    use crate::metadata::{HoverAnswer, hover, target_at};
    use wxml_project::ProjectPath;
    use wxml_text::SourceRange;

    /// Builds one declared event.
    fn event(name: &str, detail: Option<&str>, proven: bool) -> wxml_project::typing::TypingEvent {
        wxml_project::typing::TypingEvent {
            name: name.to_owned(),
            documentation: None,
            detail_text: detail.map(str::to_owned),
            detail: detail.map(wxml_project::typing::TypeText::parse),
            proven,
        }
    }

    /// Builds one declared event with a `JSDoc` description.
    fn documented_event(
        name: &str,
        detail: Option<&str>,
        documentation: &str,
    ) -> wxml_project::typing::TypingEvent {
        let mut event = event(name, detail, true);
        event.documentation = Some(documentation.to_owned());
        event
    }

    /// Builds one component with declared events.
    fn component(events: Vec<wxml_project::typing::TypingEvent>) -> ViewComponent {
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
            typing: Some(Arc::new(wxml_project::typing::TypingComponent {
                props: Vec::new(),
                events,
                complete: true,
                source: ProjectPath::new("components.d.ts").expect("path"),
            })),
            origin: crate::project_host::ComponentOrigin::Local { file: None },
        }
    }

    /// Returns the documented markdown and range for `needle` inside `text`.
    fn hover_at(text: &str, components: &[ViewComponent], needle: &str) -> (String, SourceRange) {
        let offset = u32::try_from(text.find(needle).expect("needle")).expect("offset");
        let target = target_at(text, offset).expect("target");
        match hover(components, &target) {
            HoverAnswer::Document { markdown, range } => (markdown, range),
            HoverAnswer::NotStatic => panic!("expected a document hover, got NotStatic"),
        }
    }

    /// Builds one component property.
    fn property(name: &str, type_text: &str) -> wxml_project::ComponentProperty {
        wxml_project::ComponentProperty {
            name: name.to_owned(),
            type_text: type_text.to_owned(),
            default_text: None,
            documentation: None,
            script: ProjectPath::new("components/field.js").expect("path"),
            range: wxml_text::TextRange::new(0, 0).expect("range"),
        }
    }

    /// Builds one component with declared properties and no events.
    fn component_with_properties(
        properties: Vec<wxml_project::ComponentProperty>,
    ) -> ViewComponent {
        let mut component = component(Vec::new());
        component.wxml = Some(ProjectPath::new("components/field.wxml").expect("path"));
        component.property_facts = wxml_project::ComponentPropertyFacts {
            properties,
            complete: true,
            reason: None,
            source: wxml_project::PropertySource::Script,
        };
        component
    }

    #[test]
    fn a_declared_event_attribute_hovers_the_expected_signature() {
        let text = "<van-field bind:change=\"onChange\" />";
        let components = vec![component(vec![event(
            "change",
            Some("{ index: number }"),
            true,
        )])];
        let (markdown, range) = hover_at(text, &components, "bind:change");
        assert!(
            markdown.contains("(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void"),
            "{markdown}"
        );
        let start = u32::try_from(text.find("bind:change").expect("attribute")).expect("start");
        let end = start + u32::try_from("bind:change".len()).expect("len");
        assert_eq!((range.start(), range.end()), (start, end));
        assert_eq!(&text[start as usize..end as usize], "bind:change");
    }

    #[test]
    fn a_declared_event_description_is_shown_after_the_signature() {
        let text = "<van-field bind:change=\"onChange\" />";
        let components = vec![component(vec![documented_event(
            "change",
            Some("{ index: number }"),
            "输入框内容变化时触发",
        )])];
        let (markdown, _) = hover_at(text, &components, "bind:change");
        assert_eq!(
            markdown,
            "### bind:change\n\n```ts\n(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void\n```\n\n组件 `van-field` 声明的事件处理函数签名。\n\n输入框内容变化时触发"
        );
    }

    #[test]
    fn an_event_without_a_description_keeps_the_exact_document() {
        let text = "<van-field bind:change=\"onChange\" />";
        let components = vec![component(vec![event(
            "change",
            Some("{ index: number }"),
            true,
        )])];
        let (markdown, _) = hover_at(text, &components, "bind:change");
        assert_eq!(
            markdown,
            "### bind:change\n\n```ts\n(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void\n```\n\n组件 `van-field` 声明的事件处理函数签名。"
        );
    }

    #[test]
    fn an_unproven_detail_is_marked_instead_of_guessed() {
        let text = "<van-field bind:change=\"onChange\" />";
        let components = vec![component(vec![event(
            "change",
            Some("SomeLib.Options"),
            false,
        )])];
        let (markdown, _) = hover_at(text, &components, "bind:change");
        assert!(markdown.contains("SomeLib.Options"), "{markdown}");
        assert!(markdown.contains("detail 类型未证明"), "{markdown}");
    }

    #[test]
    fn an_undeclared_event_keeps_the_generic_document() {
        let text = "<van-field bind:chaneg=\"onChange\" />";
        let components = vec![component(vec![event("change", None, true)])];
        let (markdown, _) = hover_at(text, &components, "bind:chaneg");
        assert!(markdown.contains("chaneg"), "{markdown}");
        assert!(!markdown.contains("CustomEvent"), "{markdown}");
    }

    #[test]
    fn a_component_without_declarations_keeps_the_generic_document() {
        let text = "<van-field bind:change=\"onChange\" />";
        let mut undeclared = component(vec![event("change", None, true)]);
        undeclared.typing = None;
        let (markdown, _) = hover_at(text, &[undeclared], "bind:change");
        assert!(markdown.contains("bind:change"), "{markdown}");
        assert!(!markdown.contains("CustomEvent"), "{markdown}");
    }

    #[test]
    fn a_kebab_event_spelling_hovers_the_camel_declaration() {
        let components = vec![component(vec![event(
            "someEvent",
            Some("{ index: number }"),
            true,
        )])];
        for attribute in ["bind:some-event", "bindsome-event"] {
            let text = format!("<van-field {attribute}=\"onSome\" />");
            let (markdown, range) = hover_at(&text, &components, attribute);
            assert!(
                markdown.contains("(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void"),
                "{attribute}: {markdown}"
            );
            let start = u32::try_from(text.find(attribute).expect("attribute")).expect("start");
            let end = start + u32::try_from(attribute.len()).expect("len");
            assert_eq!((range.start(), range.end()), (start, end), "{attribute}");
        }
    }

    #[test]
    fn a_camel_event_spelling_hovers_the_kebab_declaration() {
        let text = "<van-field bind:someEvent=\"onSome\" />";
        let components = vec![component(vec![event(
            "some-event",
            Some("{ index: number }"),
            true,
        )])];
        let (markdown, _) = hover_at(text, &components, "bind:someEvent");
        assert!(
            markdown.contains("CustomEvent<{ index: number }>"),
            "{markdown}"
        );
    }

    #[test]
    fn a_compact_camel_event_spelling_hovers_the_camel_declaration() {
        let text = "<van-field bindsomeEvent=\"onSome\" />";
        let components = vec![component(vec![event("someEvent", None, true)])];
        let (markdown, range) = hover_at(text, &components, "bindsomeEvent");
        assert!(
            markdown.contains("(e: WechatMiniprogram.CustomEvent<void>) => void"),
            "{markdown}"
        );
        let start = u32::try_from(text.find("bindsomeEvent").expect("attribute")).expect("start");
        let end = start + u32::try_from("bindsomeEvent".len()).expect("len");
        assert_eq!((range.start(), range.end()), (start, end));
    }

    #[test]
    fn a_kebab_property_spelling_hovers_the_camel_declaration() {
        let text = "<van-field some-prop=\"x\" />";
        let components = vec![component_with_properties(vec![property(
            "someProp", "string",
        )])];
        let (markdown, range) = hover_at(text, &components, "some-prop");
        assert!(markdown.contains("### someProp"), "{markdown}");
        assert!(markdown.contains("`string`"), "{markdown}");
        let start = u32::try_from(text.find("some-prop").expect("attribute")).expect("start");
        let end = start + u32::try_from("some-prop".len()).expect("len");
        assert_eq!((range.start(), range.end()), (start, end));
    }

    #[test]
    fn a_documented_property_shows_its_description() {
        let text = "<van-field autosize=\"x\" />";
        let mut autosize = property("autosize", "any");
        autosize.documentation = Some("是否自适应内容高度".to_owned());
        let components = vec![component_with_properties(vec![autosize])];
        let (markdown, _) = hover_at(text, &components, "autosize");
        assert!(markdown.contains("`any`"), "{markdown}");
        assert!(markdown.contains("是否自适应内容高度"), "{markdown}");
    }

    #[test]
    fn an_unknown_property_stays_undocumented() {
        let text = "<van-field other-prop=\"x\" />";
        let components = vec![component_with_properties(vec![property(
            "someProp", "string",
        )])];
        let offset = u32::try_from(text.find("other-prop").expect("attribute")).expect("offset");
        let target = target_at(text, offset).expect("target");
        assert_eq!(hover(&components, &target), HoverAnswer::NotStatic);
    }

    #[test]
    fn an_unknown_event_stays_generic() {
        let text = "<van-field bind:someevnt=\"onSome\" />";
        let components = vec![component(vec![event("someEvent", None, true)])];
        let (markdown, _) = hover_at(text, &components, "bind:someevnt");
        assert!(!markdown.contains("CustomEvent"), "{markdown}");
    }
}
