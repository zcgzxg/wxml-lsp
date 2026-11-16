//! Static Markdown documentation for tags, attributes and values.

use std::fmt::Write as _;

use wxml_project::ComponentTargetKind;

use super::component;
use crate::completion::catalog;
use crate::project_host::ViewComponent;

/// Builds documentation for a tag name.
#[allow(clippy::too_many_lines)] // one linear document builder
pub(super) fn tag_document(components: &[ViewComponent], name: &str) -> Option<String> {
    if let Some(spec) = catalog::tag(name) {
        return Some(format!(
            "### {}\n\n{}\n\nBuilt-in component.",
            escape(name),
            escape(spec.detail)
        ));
    }
    let component = component(components, name)?;
    let mut document = format!("### {}\n\n", escape(name));
    match component.kind {
        ComponentTargetKind::Local => {
            document.push_str("Custom component.\n\n");
            if let Some(file) = component.origin.file() {
                let _ = write!(document, "Declared in `{}`.\n\n", escape(file.as_str()));
            }
            if let Some(wxml) = component.wxml.as_ref() {
                let _ = write!(document, "Definition: `{}`.\n\n", escape(wxml.as_str()));
            } else if let Some(base) = component.component.as_ref() {
                let _ = write!(
                    document,
                    "The local target `{}` could not be resolved; properties are unavailable.\n\n",
                    escape(base.as_str())
                );
            } else {
                document.push_str(
                    "The local target could not be resolved; properties are unavailable.\n\n",
                );
            }
        }
        ComponentTargetKind::Plugin => {
            let _ = write!(
                document,
                "Plugin component (`{}`).\n\n",
                escape(&component.specifier)
            );
            if component.property_facts.properties.is_empty() {
                document.push_str(
                    "Local metadata is unavailable; component properties cannot be verified.\n\n",
                );
            }
        }
        ComponentTargetKind::Package => {
            let _ = write!(
                document,
                "Package component (`{}`).\n\n",
                escape(&component.specifier)
            );
            if let Some(wxml) = component.wxml.as_ref() {
                let _ = write!(document, "Local build: `{}`.\n\n", escape(wxml.as_str()));
            } else if component.property_facts.properties.is_empty() {
                document.push_str(
                    "Local metadata is unavailable; component properties cannot be verified.\n\n",
                );
            }
        }
        _ => {
            let _ = write!(
                document,
                "Custom component (`{}`).\n\n",
                escape(&component.specifier)
            );
        }
    }
    if !component.property_facts.properties.is_empty() {
        if component.property_facts.complete {
            document.push_str("Properties:\n\n");
        } else {
            let reason = component
                .property_facts
                .reason
                .unwrap_or("static extraction was incomplete");
            let _ = write!(document, "Properties (incomplete: {}):\n\n", escape(reason));
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
    } else if component.property_facts.complete && component.wxml.is_some() {
        document.push_str("No statically declared properties were found.\n");
    } else if !component.property_facts.complete {
        let reason = component
            .property_facts
            .reason
            .unwrap_or("static extraction was incomplete");
        let _ = writeln!(
            document,
            "The static property list is incomplete: {}.",
            escape(reason)
        );
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
        // cannot see (Behavior, spreads). Leave those to the backend instead
        // of claiming they do not exist.
        if component.wxml.is_some() {
            return None;
        }
        return Some(format!(
            "### {}\n\nLocal metadata is unavailable; cannot verify property `{}` on component `{}`.",
            escape(name),
            escape(name),
            escape(tag)
        ));
    }
    shared_attribute_document(tag, name)
}

/// Builds documentation for catalog attributes, directives and events.
fn shared_attribute_document(tag: &str, name: &str) -> Option<String> {
    if let Some(spec) = catalog::directive(name) {
        return Some(format!(
            "### {}\n\n{}\n\nWXML directive.",
            escape(name),
            escape(spec.detail)
        ));
    }
    if let Some(spec) = catalog::event_attribute(name) {
        return Some(event_document(name, spec.detail));
    }
    if catalog::is_event_attribute(name) {
        let event = event_name(name).unwrap_or("event");
        return Some(event_document(name, &format!("Binds the `{event}` event")));
    }
    if let Some(spec) = catalog::tag_attribute(tag, name) {
        let mut document = format!(
            "### {}\n\n{}\n\n`{}` attribute.",
            escape(name),
            escape(spec.detail),
            escape(tag)
        );
        let values = catalog::values_for(tag, name);
        if !values.is_empty() {
            document.push_str("\n\nValues:");
            for value in values {
                let _ = write!(
                    document,
                    " `{}` ({})",
                    escape(value.value),
                    escape(value.detail)
                );
            }
            document.push('.');
        }
        return Some(document);
    }
    if let Some(spec) = catalog::global_attribute(name) {
        return Some(format!(
            "### {}\n\n{}\n\nGlobal attribute.",
            escape(name),
            escape(spec.detail)
        ));
    }
    None
}

/// Builds documentation for an event attribute.
fn event_document(name: &str, detail: &str) -> String {
    format!(
        "### {}\n\n{}\n\nEvent binding.",
        escape(name),
        escape(detail)
    )
}

/// Builds documentation for one component property.
fn property_document(tag: &str, property: &wxml_project::ComponentProperty) -> String {
    let mut document = format!(
        "### {}\n\n`{}` - component property of `{}`.\n\nDeclared in `{}`.",
        escape(&property.name),
        escape(&property.type_text),
        escape(tag),
        escape(property.script.as_str())
    );
    if let Some(default) = property.default_text.as_ref() {
        let _ = write!(document, "\n\nDefault: `{}`.", escape(default));
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
                return Some(format!(
                    "### {}\n\nValue of `{}` on `{}` (`{}`).\n\n{}",
                    escape(value),
                    escape(attribute),
                    escape(tag),
                    escape(&property.type_text),
                    property
                        .documentation
                        .as_ref()
                        .map_or_else(String::new, |documentation| escape(documentation))
                ));
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
        "### {}\n\n{}\n\nValue of `{}` on `{}`.",
        escape(value),
        escape(value_spec.detail),
        escape(attribute),
        escape(tag)
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
