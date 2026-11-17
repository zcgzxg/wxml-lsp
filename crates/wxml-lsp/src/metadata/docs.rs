//! Static Markdown documentation for tags, attributes and values.

use std::fmt::Write as _;

use wxml_project::ComponentTargetKind;

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
            .find(|property| property.name.eq_ignore_ascii_case(name))
        {
            return Some(property_document(tag, property));
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
                .find(|property| property.name.eq_ignore_ascii_case(attribute))
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
