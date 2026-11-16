//! Static tag/attribute documentation and component navigation targets.
//!
//! The scanner is deliberately separate from the tolerant completion scanner:
//! hover needs the full token around the cursor and the attribute value, while
//! completion only needs the text being replaced. Both work on raw text so a
//! half-typed template still answers.
//!
//! Nothing here reads the filesystem. Component facts come from the project
//! host; definition targets stay project-relative until the LSP layer maps them
//! through the host.

use std::fmt::Write as _;

use wxml_project::{ComponentTargetKind, ProjectPath};
use wxml_text::{SourceRange, TextRange};

use crate::completion::catalog;
use crate::project_host::ViewComponent;

/// The token under the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Target {
    /// A start or closing tag name.
    Tag {
        /// Tag name.
        name: String,
        /// True inside `</name`.
        closing: bool,
        /// Range of the tag name.
        range: SourceRange,
    },
    /// An attribute name inside a start tag.
    Attribute {
        /// Enclosing tag name.
        tag: String,
        /// Attribute name.
        name: String,
        /// Range of the attribute name.
        range: SourceRange,
    },
    /// A literal attribute value without a mustache.
    Value {
        /// Enclosing tag name.
        tag: String,
        /// Attribute name.
        attribute: String,
        /// Value text.
        value: String,
        /// Range of the value token (without quotes).
        range: SourceRange,
    },
}

/// A static hover result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HoverAnswer {
    /// The token is not documented statically; the backend may answer.
    NotStatic,
    /// Static Markdown plus the exact token range.
    Document {
        /// Markdown contents.
        markdown: String,
        /// Range of the token under the cursor.
        range: SourceRange,
    },
}

/// One project-relative definition target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StaticLocation {
    /// Target file.
    pub(crate) path: ProjectPath,
    /// Byte range in that file.
    pub(crate) range: TextRange,
}

/// A static definition result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DefinitionAnswer {
    /// The token is not answered statically; the backend may answer.
    NotStatic,
    /// The token is static, but no provable target exists.
    None,
    /// A project-relative target.
    Location(StaticLocation),
}

/// Finds the token under a UTF-8 byte offset.
///
/// Returns `None` inside text, comments, WXS bodies and mustaches; those are
/// left to the type backend or intentionally have no static answer.
pub(crate) fn target_at(text: &str, offset: u32) -> Option<Target> {
    let offset = usize::try_from(offset).ok()?;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes.get(index..index + 4) == Some(b"<!--") {
            let end = text.get(index + 4..)?.find("-->")?;
            index = index.saturating_add(4 + end + 3);
            continue;
        }
        if bytes.get(index..index + 2) == Some(b"{{") {
            index = skip_mustache(bytes, index)?;
            continue;
        }
        if bytes[index] == b'<' {
            if let Some(tag) = scan_tag(text, index) {
                if offset < tag.start {
                    return None;
                }
                if offset <= tag.end {
                    return token_in_tag(&tag, offset);
                }
                index = tag.end.max(index + 1);
                if tag.name == "wxs" && !tag.closing && !tag.self_closing {
                    let close = text.get(tag.end..)?.find("</wxs")?;
                    let close = tag.end.saturating_add(close);
                    if offset < close {
                        return None;
                    }
                    index = close;
                }
                continue;
            }
        }
        index += 1;
    }
    None
}

/// One parsed value token.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RawValue {
    text: String,
    range: TextRange,
    contains_mustache: bool,
}

/// One parsed attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RawAttribute {
    name: String,
    name_range: TextRange,
    value: Option<RawValue>,
}

/// One parsed start or closing tag.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RawTag {
    start: usize,
    end: usize,
    closing: bool,
    self_closing: bool,
    name: String,
    name_range: TextRange,
    attributes: Vec<RawAttribute>,
}

/// Parses one tag beginning at `<`.
///
/// Returns `None` for prose (`a < b`), comments and malformed declarations.
#[allow(clippy::too_many_lines)] // one linear tag scanner
fn scan_tag(text: &str, start: usize) -> Option<RawTag> {
    let bytes = text.as_bytes();
    let mut index = start.checked_add(1)?;
    let closing = bytes.get(index) == Some(&b'/');
    if closing {
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
    }
    let name_start = index;
    if !bytes.get(index).is_some_and(|byte| is_tag_start(*byte)) {
        return None;
    }
    while index < bytes.len() && is_tag_name(bytes[index]) {
        index += 1;
    }
    let name = text.get(name_start..index)?.to_owned();
    let name_range = text_range(name_start, index)?;
    if closing {
        let mut end = index;
        while end < bytes.len() && bytes[end] != b'>' && bytes[end] != b'<' {
            end += 1;
        }
        if bytes.get(end) == Some(&b'>') {
            end += 1;
        }
        return Some(RawTag {
            start,
            end,
            closing: true,
            self_closing: false,
            name,
            name_range,
            attributes: Vec::new(),
        });
    }

    let mut attributes = Vec::new();
    let mut self_closing = false;
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        match bytes[index] {
            b'>' => {
                index += 1;
                break;
            }
            b'/' if bytes.get(index + 1) == Some(&b'>') => {
                index += 2;
                self_closing = true;
                break;
            }
            b'<' => break,
            _ => {}
        }
        let attribute_start = index;
        while index < bytes.len() && is_attribute_name(bytes[index]) {
            index += 1;
        }
        if index == attribute_start {
            index += 1;
            continue;
        }
        let attribute_name = text.get(attribute_start..index)?.to_owned();
        let attribute_range = text_range(attribute_start, index)?;
        let after_name = index;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut value = None;
        if bytes.get(index) == Some(&b'=') {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() {
                let quote = bytes[index];
                if quote == b'"' || quote == b'\'' {
                    index += 1;
                    let value_start = index;
                    while index < bytes.len() && bytes[index] != quote {
                        if bytes.get(index..index + 2) == Some(b"{{") {
                            index = skip_mustache(bytes, index).unwrap_or(bytes.len());
                            continue;
                        }
                        index += 1;
                    }
                    let value_end = index;
                    let raw = text.get(value_start..value_end).unwrap_or_default();
                    if let Some(range) = text_range(value_start, value_end) {
                        value = Some(RawValue {
                            text: raw.to_owned(),
                            range,
                            contains_mustache: raw.contains("{{"),
                        });
                    }
                    if index < bytes.len() {
                        index += 1;
                    }
                } else {
                    let value_start = index;
                    while index < bytes.len()
                        && !bytes[index].is_ascii_whitespace()
                        && bytes[index] != b'>'
                    {
                        if bytes.get(index..index + 2) == Some(b"{{") {
                            index = skip_mustache(bytes, index).unwrap_or(bytes.len());
                            continue;
                        }
                        index += 1;
                    }
                    let value_end = index;
                    let raw = text.get(value_start..value_end).unwrap_or_default();
                    if let Some(range) = text_range(value_start, value_end) {
                        value = Some(RawValue {
                            text: raw.to_owned(),
                            range,
                            contains_mustache: raw.contains("{{"),
                        });
                    }
                }
            }
        } else {
            index = after_name;
        }
        attributes.push(RawAttribute {
            name: attribute_name,
            name_range: attribute_range,
            value,
        });
    }
    Some(RawTag {
        start,
        end: index,
        closing: false,
        self_closing,
        name,
        name_range,
        attributes,
    })
}

/// Maps the cursor inside one parsed tag to a token.
fn token_in_tag(tag: &RawTag, offset: usize) -> Option<Target> {
    if contains(tag.name_range, offset) {
        return Some(Target::Tag {
            name: tag.name.clone(),
            closing: tag.closing,
            range: SourceRange::new(tag.name_range),
        });
    }
    if tag.closing {
        return None;
    }
    for attribute in &tag.attributes {
        if contains(attribute.name_range, offset) {
            return Some(Target::Attribute {
                tag: tag.name.clone(),
                name: attribute.name.clone(),
                range: SourceRange::new(attribute.name_range),
            });
        }
        let Some(value) = attribute.value.as_ref() else {
            continue;
        };
        if contains(value.range, offset) {
            if value.contains_mustache || catalog::is_event_prefix(&attribute.name) {
                // Event values and mustaches stay on the backend path.
                return None;
            }
            return Some(Target::Value {
                tag: tag.name.clone(),
                attribute: attribute.name.clone(),
                value: value.text.clone(),
                range: SourceRange::new(value.range),
            });
        }
    }
    None
}

/// Returns whether `offset` is on or immediately after `range`.
const fn contains(range: TextRange, offset: usize) -> bool {
    let start = range.start() as usize;
    let end = range.end() as usize;
    offset >= start && offset <= end
}

const fn is_tag_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':' | b'.' | b'-')
}

const fn is_tag_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

const fn is_attribute_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.' | b'@' | b'*')
}

/// Skips a mustache beginning at `index`.
fn skip_mustache(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index.checked_add(2)?;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' => {
                let quote = bytes[cursor];
                cursor += 1;
                while cursor < bytes.len() {
                    match bytes[cursor] {
                        b'\\' => cursor = cursor.saturating_add(2),
                        byte if byte == quote => {
                            cursor += 1;
                            break;
                        }
                        _ => cursor += 1,
                    }
                }
            }
            b'}' if bytes.get(cursor + 1) == Some(&b'}') => return Some(cursor + 2),
            _ => cursor += 1,
        }
    }
    None
}

/// Converts validated byte offsets into a range.
fn text_range(start: usize, end: usize) -> Option<TextRange> {
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?;
    TextRange::new(start, end).ok()
}

/// Looks up one registered component.
fn component<'a>(components: &'a [ViewComponent], name: &str) -> Option<&'a ViewComponent> {
    components.iter().find(|component| component.name == name)
}

/// Builds a static hover for one token.
pub(crate) fn hover(components: &[ViewComponent], target: &Target) -> HoverAnswer {
    let (document, range) = match target {
        Target::Tag { name, range, .. } => (tag_document(components, name), *range),
        Target::Attribute { tag, name, range } => {
            (attribute_document(components, tag, name), *range)
        }
        Target::Value {
            tag,
            attribute,
            value,
            range,
        } => (value_document(components, tag, attribute, value), *range),
    };
    document.map_or(HoverAnswer::NotStatic, |markdown| HoverAnswer::Document {
        markdown,
        range,
    })
}

/// Builds a static definition answer for one token.
pub(crate) fn definition(components: &[ViewComponent], target: &Target) -> DefinitionAnswer {
    match target {
        Target::Tag { name, .. } => {
            let Some(component) = component(components, name) else {
                return DefinitionAnswer::NotStatic;
            };
            let path = component.wxml.clone().or_else(|| component.json.clone());
            path.map_or(DefinitionAnswer::None, |path| {
                start_of_file(path).map_or(DefinitionAnswer::None, DefinitionAnswer::Location)
            })
        }
        Target::Attribute { tag, name, .. } => {
            if catalog::is_event_prefix(name) {
                return DefinitionAnswer::NotStatic;
            }
            let Some(component) = component(components, tag) else {
                return DefinitionAnswer::NotStatic;
            };
            let Some(property) = component
                .properties
                .iter()
                .find(|property| property.name.eq_ignore_ascii_case(name))
            else {
                // A resolved local component may declare the property through
                // a Behavior or a spread; let the backend try instead of
                // reporting a definitive "no definition".
                if component.kind == ComponentTargetKind::Local && component.wxml.is_some() {
                    return DefinitionAnswer::NotStatic;
                }
                return DefinitionAnswer::None;
            };
            DefinitionAnswer::Location(StaticLocation {
                path: property.script.clone(),
                range: property.range,
            })
        }
        Target::Value { .. } => DefinitionAnswer::NotStatic,
    }
}

/// Creates a zero-length location at the start of `path`.
fn start_of_file(path: ProjectPath) -> Option<StaticLocation> {
    let range = TextRange::new(0, 0).ok()?;
    Some(StaticLocation { path, range })
}

/// Builds documentation for a tag name.
fn tag_document(components: &[ViewComponent], name: &str) -> Option<String> {
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
                "Plugin component (`{}`).\n\nLocal metadata is unavailable; no component properties are known.\n\n",
                escape(&component.specifier)
            );
        }
        ComponentTargetKind::Package => {
            let _ = write!(
                document,
                "Package component (`{}`).\n\nLocal metadata is unavailable; no component properties are known.\n\n",
                escape(&component.specifier)
            );
        }
        _ => {
            let _ = write!(
                document,
                "Custom component (`{}`).\n\n",
                escape(&component.specifier)
            );
        }
    }
    if !component.properties.is_empty() {
        document.push_str("Properties:\n\n");
        for property in &component.properties {
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
    } else if component.kind == ComponentTargetKind::Local && component.wxml.is_some() {
        document.push_str("No statically declared properties were found.\n");
    }
    Some(document.trim_end().to_owned())
}

/// Builds documentation for an attribute name.
fn attribute_document(components: &[ViewComponent], tag: &str, name: &str) -> Option<String> {
    if let Some(component) = component(components, tag) {
        if let Some(property) = component
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
        if component.kind == ComponentTargetKind::Local && component.wxml.is_some() {
            return None;
        }
        return Some(format!(
            "### {}\n\nNo statically declared property `{}` on component `{}`.",
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
    if catalog::is_event_prefix(name) {
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
fn value_document(
    components: &[ViewComponent],
    tag: &str,
    attribute: &str,
    value: &str,
) -> Option<String> {
    if let Some(component) = component(components, tag) {
        if let Some(property) = component
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
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '(' | ')' | '#' | '+' | '!' => {
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
    use super::*;

    fn target(text: &str, offset: usize) -> Option<Target> {
        let offset = u32::try_from(offset).expect("offset");
        target_at(text, offset)
    }

    #[test]
    fn finds_tag_names_attributes_and_values() {
        let text = "<my-card title=\"hello\"></my-card>";
        match target(text, 4).expect("tag") {
            Target::Tag {
                name,
                closing: false,
                ..
            } => assert_eq!(name, "my-card"),
            other => panic!("unexpected target: {other:?}"),
        }
        match target(text, 14).expect("attribute") {
            Target::Attribute { tag, name, .. } => {
                assert_eq!(tag, "my-card");
                assert_eq!(name, "title");
            }
            other => panic!("unexpected target: {other:?}"),
        }
        match target(text, 18).expect("value") {
            Target::Value {
                attribute, value, ..
            } => {
                assert_eq!(attribute, "title");
                assert_eq!(value, "hello");
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn mustaches_comments_and_wxs_bodies_have_no_static_target() {
        for (text, offset) in [
            ("<view>{{ user.name }}</view>", 12),
            ("<view><!-- count --></view>", 12),
            ("<wxs module=\"m\">var a = 1;</wxs>", 24),
            ("<view>plain text</view>", 8),
            ("<view title=\"{{ name }}\"></view>", 18),
            ("<view bindtap=\"onTap\"></view>", 18),
        ] {
            assert!(
                target(text, offset).is_none(),
                "{text:?} at {offset} produced a target"
            );
        }
    }

    #[test]
    fn prose_less_than_is_not_a_tag() {
        assert!(target("<view>a < b</view>", 10).is_none());
    }

    #[test]
    fn unicode_offsets_stay_on_character_boundaries() {
        let text = "<view>😀</view><my-card title=\"中\" />";
        let offset = text.find("my-card").expect("tag") + 3;
        assert!(matches!(target(text, offset), Some(Target::Tag { .. })));
        let value = text.find("中").expect("value");
        assert!(matches!(target(text, value), Some(Target::Value { .. })));
    }

    #[test]
    fn closing_tags_have_tag_targets() {
        let text = "<my-card></my-card>";
        match target(text, 13).expect("closing tag") {
            Target::Tag {
                name,
                closing: true,
                ..
            } => assert_eq!(name, "my-card"),
            other => panic!("unexpected target: {other:?}"),
        }
    }

    fn component_fixture() -> ViewComponent {
        ViewComponent {
            name: "my-card".to_owned(),
            specifier: "../../components/my-card/index".to_owned(),
            kind: ComponentTargetKind::Local,
            component: Some(ProjectPath::new("components/my-card/index").expect("path")),
            wxml: Some(ProjectPath::new("components/my-card/index.wxml").expect("path")),
            json: Some(ProjectPath::new("components/my-card/index.json").expect("path")),
            properties: vec![wxml_project::ComponentProperty {
                name: "title".to_owned(),
                type_text: "string | number".to_owned(),
                default_text: Some("''".to_owned()),
                documentation: Some("标题".to_owned()),
                script: ProjectPath::new("components/my-card/index.ts").expect("path"),
                range: TextRange::new(20, 25).expect("range"),
            }],
            origin: crate::project_host::ComponentOrigin::Local {
                file: Some(ProjectPath::new("pages/index/index.json").expect("path")),
            },
        }
    }

    #[test]
    fn builtin_tags_attributes_and_values_have_static_docs() {
        let text = "<button type=\"primary\" wx:if=\"ok\">";
        let components: Vec<ViewComponent> = Vec::new();
        let tag = target(text, 2).expect("tag");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
            panic!("tag docs");
        };
        assert!(markdown.contains("Button"), "{markdown}");

        let attribute = target(text, 10).expect("attribute");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
            panic!("attribute docs");
        };
        assert!(markdown.contains("primary"), "{markdown}");
        assert!(markdown.contains("default"), "{markdown}");

        let offset = text.find("primary").expect("value") + 2;
        let value = target(text, offset).expect("value");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &value) else {
            panic!("value docs");
        };
        assert!(markdown.contains("Primary button"), "{markdown}");

        let offset = text.find("wx:if").expect("directive") + 2;
        let directive = target(text, offset).expect("directive");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &directive) else {
            panic!("directive docs");
        };
        assert!(markdown.contains("WXML directive"), "{markdown}");
    }

    #[test]
    fn component_tag_and_property_have_static_docs_and_definitions() {
        let components = vec![component_fixture()];
        let text = "<my-card title=\"x\" />";

        let tag = target(text, 3).expect("tag");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
            panic!("component docs");
        };
        assert!(markdown.contains("Custom component"), "{markdown}");
        assert!(
            markdown.contains("components/my-card/index.wxml"),
            "{markdown}"
        );
        assert!(markdown.contains("pages/index/index.json"), "{markdown}");
        assert!(markdown.contains("title"), "{markdown}");
        assert!(markdown.contains("string | number"), "{markdown}");
        assert_eq!(
            definition(&components, &tag),
            DefinitionAnswer::Location(StaticLocation {
                path: ProjectPath::new("components/my-card/index.wxml").expect("path"),
                range: TextRange::new(0, 0).expect("range"),
            })
        );

        let attribute =
            target(text, text.find("title").expect("attribute") + 2).expect("attribute");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
            panic!("property docs");
        };
        assert!(markdown.contains("标题"), "{markdown}");
        assert_eq!(
            definition(&components, &attribute),
            DefinitionAnswer::Location(StaticLocation {
                path: ProjectPath::new("components/my-card/index.ts").expect("path"),
                range: TextRange::new(20, 25).expect("range"),
            })
        );
    }

    #[test]
    fn unresolved_components_and_unknown_properties_do_not_fabricate_targets() {
        let mut plugin = component_fixture();
        plugin.kind = ComponentTargetKind::Plugin;
        plugin.component = None;
        plugin.wxml = None;
        plugin.json = None;
        plugin.properties.clear();
        plugin.specifier = "plugin://demo/card".to_owned();
        let components = vec![plugin];
        let text = "<my-card title=\"x\" />";
        let tag = target(text, 3).expect("tag");
        let HoverAnswer::Document { markdown, .. } = hover(&components, &tag) else {
            panic!("plugin docs");
        };
        assert!(markdown.contains("Plugin component"), "{markdown}");
        assert!(markdown.contains("unavailable"), "{markdown}");
        assert_eq!(definition(&components, &tag), DefinitionAnswer::None);

        let attribute =
            target(text, text.find("title").expect("attribute") + 2).expect("attribute");
        assert_eq!(definition(&components, &attribute), DefinitionAnswer::None);
        let HoverAnswer::Document { markdown, .. } = hover(&components, &attribute) else {
            panic!("unknown property docs");
        };
        assert!(
            markdown.contains("No statically declared property"),
            "{markdown}"
        );
    }
}
