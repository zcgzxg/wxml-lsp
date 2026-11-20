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

use wxml_project::{ComponentTargetKind, ProjectPath, component_name_eq};
use wxml_text::{SourceRange, TextRange};

use crate::completion::catalog;
use crate::project_host::ViewComponent;

mod docs;

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

/// Returns whether a tag name is a built-in tag.
pub(crate) fn is_builtin_tag(tag: &str) -> bool {
    catalog::tag(tag).is_some()
}

/// Returns whether static metadata for `target` needs the component table.
pub(crate) fn needs_components(target: &Target) -> bool {
    match target {
        Target::Tag { name, .. } => !is_builtin_tag(name),
        Target::Attribute { tag, .. } | Target::Value { tag, .. } => !is_builtin_tag(tag),
    }
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
            if value.contains_mustache || catalog::is_event_attribute(&attribute.name) {
                // Mustaches and documented event values stay on the backend path.
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
        Target::Tag { name, range, .. } => (docs::tag_document(components, name), *range),
        Target::Attribute { tag, name, range } => {
            (docs::attribute_document(components, tag, name), *range)
        }
        Target::Value {
            tag,
            attribute,
            value,
            range,
        } => (
            docs::value_document(components, tag, attribute, value),
            *range,
        ),
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
            if catalog::tag(name).is_some() {
                return DefinitionAnswer::NotStatic;
            }
            let Some(component) = component(components, name) else {
                return DefinitionAnswer::NotStatic;
            };
            let path = component.wxml.clone().or_else(|| component.json.clone());
            path.map_or(DefinitionAnswer::None, |path| {
                start_of_file(path).map_or(DefinitionAnswer::None, DefinitionAnswer::Location)
            })
        }
        Target::Attribute { tag, name, .. } => {
            if catalog::tag(tag).is_some() {
                return DefinitionAnswer::NotStatic;
            }
            let Some(component) = component(components, tag) else {
                return DefinitionAnswer::NotStatic;
            };
            if let Some(property) = component
                .property_facts
                .properties
                .iter()
                .find(|property| component_name_eq(&property.name, name))
            {
                return DefinitionAnswer::Location(StaticLocation {
                    path: property.script.clone(),
                    range: property.range,
                });
            }
            if catalog::is_event_prefix(name) {
                return DefinitionAnswer::NotStatic;
            }
            // Metadata that could not be resolved or is known incomplete must
            // not become an authoritative "no definition".
            if component.kind != ComponentTargetKind::Local
                || component.wxml.is_none()
                || !component.property_facts.complete
            {
                return DefinitionAnswer::NotStatic;
            }
            DefinitionAnswer::None
        }
        Target::Value { .. } => DefinitionAnswer::NotStatic,
    }
}

/// Creates a zero-length location at the start of `path`.
fn start_of_file(path: ProjectPath) -> Option<StaticLocation> {
    let range = TextRange::new(0, 0).ok()?;
    Some(StaticLocation { path, range })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod spelling_tests {
    use wxml_project::{ComponentProperty, ComponentPropertyFacts, ProjectPath, PropertySource};
    use wxml_text::{SourceRange, TextRange};

    use super::*;

    fn property(name: &str) -> ComponentProperty {
        ComponentProperty {
            name: name.to_owned(),
            type_text: "string".to_owned(),
            default_text: None,
            documentation: None,
            script: ProjectPath::new("components/item/item.ts").expect("path"),
            range: TextRange::new(40, 55).expect("range"),
        }
    }

    fn component(properties: Vec<ComponentProperty>, complete: bool) -> ViewComponent {
        ViewComponent {
            name: "item".to_owned(),
            specifier: "../../components/item/item".to_owned(),
            kind: ComponentTargetKind::Local,
            component: Some(ProjectPath::new("components/item/item").expect("path")),
            wxml: Some(ProjectPath::new("components/item/item.wxml").expect("path")),
            json: Some(ProjectPath::new("components/item/item.json").expect("path")),
            property_facts: ComponentPropertyFacts {
                properties,
                complete,
                reason: None,
                source: PropertySource::Typing,
            },
            typing: None,
            origin: crate::project_host::ComponentOrigin::Local { file: None },
        }
    }

    fn attribute(name: &str) -> Target {
        Target::Attribute {
            tag: "item".to_owned(),
            name: name.to_owned(),
            range: SourceRange::new(TextRange::new(0, 3).expect("range")),
        }
    }

    fn location() -> DefinitionAnswer {
        DefinitionAnswer::Location(StaticLocation {
            path: ProjectPath::new("components/item/item.ts").expect("path"),
            range: TextRange::new(40, 55).expect("range"),
        })
    }

    #[test]
    fn property_definition_accepts_camel_and_kebab_in_both_directions() {
        let camel = component(vec![property("cardTitle")], true);
        assert_eq!(
            definition(std::slice::from_ref(&camel), &attribute("card-title")),
            location()
        );
        assert_eq!(definition(&[camel], &attribute("cardTitle")), location());

        let kebab = component(vec![property("card-title")], true);
        assert_eq!(
            definition(std::slice::from_ref(&kebab), &attribute("cardTitle")),
            location()
        );
        assert_eq!(definition(&[kebab], &attribute("card-title")), location());
    }

    #[test]
    fn property_definition_keeps_case_and_underscore_variants_unresolved() {
        let components = [component(vec![property("someProp")], true)];
        for written in ["someprop", "some_prop", "other"] {
            assert_eq!(
                definition(&components, &attribute(written)),
                DefinitionAnswer::None,
                "{written}"
            );
        }
    }

    #[test]
    fn event_attributes_stay_non_static() {
        let components = [component(vec![property("someProp")], true)];
        for written in ["bindtap", "bind:tap", "catch:tap"] {
            assert_eq!(
                definition(&components, &attribute(written)),
                DefinitionAnswer::NotStatic,
                "{written}"
            );
        }
    }

    #[test]
    fn missing_or_incomplete_declarations_keep_the_existing_answer() {
        assert_eq!(
            definition(&[], &attribute("card-title")),
            DefinitionAnswer::NotStatic
        );
        let incomplete = [component(Vec::new(), false)];
        assert_eq!(
            definition(&incomplete, &attribute("card-title")),
            DefinitionAnswer::NotStatic
        );
        let complete = [component(Vec::new(), true)];
        assert_eq!(
            definition(&complete, &attribute("card-title")),
            DefinitionAnswer::None
        );
    }
}
