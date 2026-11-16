//! Page/component script lookup and registration data-root extraction.

use wxml_text::TextRange;

use crate::files::ProjectPath;

/// Script extensions in preference order: TypeScript before JavaScript.
pub const SCRIPT_EXTENSIONS: [&str; 2] = ["ts", "js"];

/// The script selected for a page or component base path.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScriptResolution {
    /// Chosen script, or `None` when neither candidate exists.
    pub script: Option<ProjectPath>,
    /// `true` when both `.ts` and `.js` exist; `.ts` wins deterministically.
    pub conflict: bool,
}

/// Chooses the page or component script for `base`.
///
/// `.ts` is preferred over `.js`. When both exist the choice is still `.ts`
/// and [`ScriptResolution::conflict`] is set so discovery can report it.
#[must_use]
pub fn resolve_script<F>(base: &ProjectPath, mut exists: F) -> ScriptResolution
where
    F: FnMut(&ProjectPath) -> bool,
{
    let ts = base.with_extension(SCRIPT_EXTENSIONS[0]).ok();
    let js = base.with_extension(SCRIPT_EXTENSIONS[1]).ok();
    let ts_exists = ts.as_ref().is_some_and(&mut exists);
    let js_exists = js.as_ref().is_some_and(&mut exists);
    let script = if ts_exists {
        ts
    } else if js_exists {
        js
    } else {
        None
    };
    ScriptResolution {
        script,
        conflict: ts_exists && js_exists,
    }
}

/// The kind of registration data root found in a script.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DataRootKind {
    /// `Page({ data: ... })`.
    PageData,
    /// `Component({ properties: ... })`.
    ComponentProperties,
    /// `Component({ data: ... })`.
    ComponentData,
}

impl DataRootKind {
    /// Returns the stable lower-case name of the kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PageData => "page-data",
            Self::ComponentProperties => "component-properties",
            Self::ComponentData => "component-data",
        }
    }
}

/// One statically declared component property.
///
/// Only `Component({ properties: { ... } })` object literals are inspected;
/// spreads, computed keys and properties defined in another file keep their
/// root but produce no declaration here. Types are the raw constructor names,
/// not TypeScript-resolved types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentProperty {
    /// Property name as declared in the component script.
    pub name: String,
    /// Display type such as `string`, `string | number` or `any`.
    pub type_text: String,
    /// Raw `value` expression text, when the declaration has one.
    pub default_text: Option<String>,
    /// Leading `JSDoc` or contiguous `//` comments, trimmed.
    pub documentation: Option<String>,
    /// Script that declares the property.
    pub script: ProjectPath,
    /// Byte range of the property key token in `script`.
    pub range: TextRange,
}

/// Maximum component properties kept from one project discovery.
pub const MAX_COMPONENT_PROPERTIES: usize = 512;

/// Maximum characters kept from one property documentation comment.
const MAX_PROPERTY_DOC_CHARS: usize = 512;

/// Maximum characters kept from one property type or default expression.
const MAX_PROPERTY_TYPE_CHARS: usize = 64;

/// Location and raw source of one registration data root.
///
/// The extractor records where the value expression starts and ends; it does
/// not parse or type-check the expression, and does not resolve spreads or
/// identifiers that point at an object defined elsewhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataRoot {
    /// Which registration produced the root.
    pub kind: DataRootKind,
    /// Script that contains the root.
    pub script: ProjectPath,
    /// Byte range of the value expression inside the script.
    pub range: TextRange,
    /// Object key that declared the root (`data` or `properties`).
    pub name: Option<String>,
    /// Raw source slice covered by [`DataRoot::range`].
    pub text: String,
}

/// Extracts `Page({ data })` and `Component({ data, properties })` roots.
///
/// The scanner understands line/block comments, single-, double- and
/// backtick-quoted strings (including `${}` expressions), and balanced
/// brackets. It is bounded by a nesting limit and never panics on malformed
/// input. Regex literals are not recognized; a `}` inside a regex literal can
/// truncate scanning of that script.
#[must_use]
pub fn extract_data_roots(script: &ProjectPath, text: &str) -> Vec<DataRoot> {
    let bytes = text.as_bytes();
    let mut roots = Vec::new();
    let mut brackets = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(next) = skip_lexeme(bytes, index, 0) {
            index = next;
            continue;
        }
        match bytes[index] {
            byte @ (b'(' | b'[' | b'{') => {
                if brackets.len() >= MAX_SCAN_DEPTH {
                    return roots;
                }
                brackets.push(byte);
                index += 1;
            }
            b')' | b']' | b'}' => {
                brackets.pop();
                index += 1;
            }
            byte if is_identifier_start(byte) => {
                let start = index;
                let mut end = index + 1;
                while end < bytes.len() && is_identifier_continue(bytes[end]) {
                    end += 1;
                }
                let Some(identifier) = text.get(start..end) else {
                    index = end;
                    continue;
                };
                if brackets.is_empty()
                    && matches!(identifier, "Page" | "Component")
                    && !is_member_access(bytes, start)
                {
                    let open = skip_trivia(bytes, end);
                    if bytes.get(open) == Some(&b'(') {
                        let registration = if identifier == "Page" {
                            Registration::Page
                        } else {
                            Registration::Component
                        };
                        if let Some((object_start, object_end)) =
                            find_argument_object(bytes, open + 1)
                        {
                            extract_object(
                                script,
                                text,
                                registration,
                                object_start,
                                object_end,
                                &mut roots,
                            );
                        }
                    }
                }
                index = end;
            }
            _ => index += 1,
        }
    }
    roots
}

/// Registration forms that produce data roots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Registration {
    Page,
    Component,
}

/// Windows-like maximum nesting accepted while scanning a script.
const MAX_SCAN_DEPTH: usize = 256;

/// Maximum nested `${ ... }` template literals followed by the scanner.
const MAX_TEMPLATE_DEPTH: usize = 32;

/// Returns `true` for the first byte of an ECMAScript identifier.
const fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

/// Returns `true` for a continuation byte of an ECMAScript identifier.
const fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

/// Skips a comment, string or template literal starting at `index`.
///
/// Returns `None` when the byte does not start a lexeme, so callers can handle
/// ordinary code. Unterminated lexemes consume the rest of the input.
fn skip_lexeme(bytes: &[u8], index: usize, template_depth: usize) -> Option<usize> {
    match *bytes.get(index)? {
        b'/' if bytes.get(index + 1) == Some(&b'/') => {
            let mut cursor = index + 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            Some(cursor)
        }
        b'/' if bytes.get(index + 1) == Some(&b'*') => {
            let mut cursor = index + 2;
            while cursor + 1 < bytes.len() {
                if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                    return Some(cursor + 2);
                }
                cursor += 1;
            }
            Some(bytes.len())
        }
        quote @ (b'\'' | b'"') => {
            let mut cursor = index + 1;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'\\' => cursor += 2,
                    byte if byte == quote => return Some(cursor + 1),
                    _ => cursor += 1,
                }
            }
            Some(bytes.len())
        }
        b'`' => Some(skip_template(bytes, index, template_depth)),
        _ => None,
    }
}

/// Skips a backtick-quoted template literal, including `${ ... }` sections.
fn skip_template(bytes: &[u8], index: usize, depth: usize) -> usize {
    if depth > MAX_TEMPLATE_DEPTH {
        return bytes.len();
    }
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'`' => return cursor + 1,
            b'$' if bytes.get(cursor + 1) == Some(&b'{') => {
                cursor = skip_template_expression(bytes, cursor + 2, depth + 1);
            }
            _ => cursor += 1,
        }
    }
    bytes.len()
}

/// Skips the expression inside `${ ... }`, returning the byte after `}`.
fn skip_template_expression(bytes: &[u8], index: usize, depth: usize) -> usize {
    let mut pending = vec![b'}'];
    let mut cursor = index;
    while cursor < bytes.len() {
        if let Some(next) = skip_lexeme(bytes, cursor, depth) {
            cursor = next;
            continue;
        }
        match bytes[cursor] {
            b'{' => {
                if pending.len() >= MAX_SCAN_DEPTH {
                    return bytes.len();
                }
                pending.push(b'}');
            }
            b'}' => {
                pending.pop();
                if pending.is_empty() {
                    return cursor + 1;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    bytes.len()
}

/// Skips whitespace and comments.
fn skip_trivia(bytes: &[u8], mut index: usize) -> usize {
    loop {
        match bytes.get(index) {
            Some(b' ' | b'\t' | b'\r' | b'\n') => index += 1,
            Some(b'/') => {
                let Some(next) = skip_lexeme(bytes, index, 0) else {
                    return index;
                };
                index = next;
            }
            _ => return index,
        }
    }
}

/// Returns whether the identifier at `start` follows a member-access dot.
fn is_member_access(bytes: &[u8], start: usize) -> bool {
    bytes[..start]
        .iter()
        .rev()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'.')
}

/// Finds the object literal that is the first argument of a registration.
fn find_argument_object(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let open = skip_trivia(bytes, index);
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let end = find_matching(bytes, open, bytes.len())?;
    Some((open, end))
}

/// Finds the end (exclusive) of the bracket opened at `open`.
fn find_matching(bytes: &[u8], open: usize, limit: usize) -> Option<usize> {
    let mut pending = vec![matching_close(*bytes.get(open)?)?];
    let mut cursor = open + 1;
    while cursor < limit {
        if let Some(next) = skip_lexeme(bytes, cursor, 0) {
            cursor = next;
            continue;
        }
        match bytes[cursor] {
            byte @ (b'{' | b'[' | b'(') => {
                if pending.len() >= MAX_SCAN_DEPTH {
                    return None;
                }
                pending.push(matching_close(byte)?);
            }
            b'}' | b']' | b')' => {
                if pending.last() != Some(&bytes[cursor]) {
                    cursor += 1;
                    continue;
                }
                pending.pop();
                if pending.is_empty() {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

/// Maps an opening bracket to its closing byte.
const fn matching_close(open: u8) -> Option<u8> {
    match open {
        b'{' => Some(b'}'),
        b'[' => Some(b']'),
        b'(' => Some(b')'),
        _ => None,
    }
}

/// Extracts roots from the registration object literal.
fn extract_object(
    script: &ProjectPath,
    text: &str,
    registration: Registration,
    object_start: usize,
    object_end: usize,
    roots: &mut Vec<DataRoot>,
) {
    let bytes = text.as_bytes();
    let mut pending = vec![b'}'];
    let mut index = object_start + 1;
    while index + 1 < object_end {
        if let Some(next) = skip_lexeme(bytes, index, 0) {
            index = next;
            continue;
        }
        match bytes[index] {
            byte @ (b'{' | b'[' | b'(') => {
                if pending.len() >= MAX_SCAN_DEPTH {
                    return;
                }
                pending.push(matching_close(byte).unwrap_or(byte));
                index += 1;
            }
            b'}' | b']' | b')' => {
                pending.pop();
                index += 1;
            }
            byte if pending.len() == 1 && is_identifier_start(byte) => {
                let key_start = index;
                let mut key_end = index + 1;
                while key_end < object_end && is_identifier_continue(bytes[key_end]) {
                    key_end += 1;
                }
                let Some(key) = text.get(key_start..key_end) else {
                    index = key_end;
                    continue;
                };
                let colon = skip_trivia(bytes, key_end);
                if bytes.get(colon) != Some(&b':') {
                    index = key_end;
                    continue;
                }
                let value_start = skip_trivia(bytes, colon + 1);
                let Some(value_end) = expression_end(bytes, value_start, object_end) else {
                    index = key_end;
                    continue;
                };
                if let Some(kind) = root_kind(registration, key) {
                    push_root(script, text, kind, value_start, value_end, roots);
                }
                index = value_end;
            }
            _ => index += 1,
        }
    }
}

/// Finds the trimmed end (exclusive) of the value expression at `start`.
fn expression_end(bytes: &[u8], start: usize, limit: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'{' | b'[' | b'(' => find_matching(bytes, start, limit),
        _ => {
            let mut pending: Vec<u8> = Vec::new();
            let mut cursor = start;
            while cursor < limit {
                if let Some(next) = skip_lexeme(bytes, cursor, 0) {
                    cursor = next;
                    continue;
                }
                match bytes[cursor] {
                    byte @ (b'{' | b'[' | b'(') => {
                        if pending.len() >= MAX_SCAN_DEPTH {
                            return None;
                        }
                        pending.push(matching_close(byte).unwrap_or(byte));
                    }
                    b'}' | b']' | b')' => {
                        if pending.pop().is_none() {
                            break;
                        }
                    }
                    b',' if pending.is_empty() => break,
                    _ => {}
                }
                cursor += 1;
            }
            let end = trim_end_whitespace(bytes, start, cursor);
            (end > start).then_some(end)
        }
    }
}

/// Trims ASCII whitespace from the end of `[start, end)`.
const fn trim_end_whitespace(bytes: &[u8], start: usize, mut end: usize) -> usize {
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

/// Maps a registration and member key to a data root kind.
fn root_kind(registration: Registration, key: &str) -> Option<DataRootKind> {
    match (registration, key) {
        (Registration::Page, "data") => Some(DataRootKind::PageData),
        (Registration::Component, "properties") => Some(DataRootKind::ComponentProperties),
        (Registration::Component, "data") => Some(DataRootKind::ComponentData),
        _ => None,
    }
}

/// Converts byte offsets into a range and records the source slice.
fn push_root(
    script: &ProjectPath,
    text: &str,
    kind: DataRootKind,
    start: usize,
    end: usize,
    roots: &mut Vec<DataRoot>,
) {
    let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
        return;
    };
    let Ok(range) = TextRange::new(start, end) else {
        return;
    };
    let Some(snippet) = text.get(start as usize..end as usize) else {
        return;
    };
    roots.push(DataRoot {
        kind,
        script: script.clone(),
        range,
        name: Some(match kind {
            DataRootKind::PageData | DataRootKind::ComponentData => "data".to_owned(),
            DataRootKind::ComponentProperties => "properties".to_owned(),
        }),
        text: snippet.to_owned(),
    });
}

/// Extracts component property declarations from `properties` data roots.
///
/// The returned declarations come from the raw object literal recorded by
/// [`extract_data_roots`]. Non-object `properties` expressions (identifiers,
/// spreads) are silently skipped because their members cannot be proven from
/// this file alone.
#[must_use]
pub fn extract_component_properties(roots: &[DataRoot]) -> Vec<ComponentProperty> {
    let mut properties = Vec::new();
    for root in roots {
        if root.kind != DataRootKind::ComponentProperties {
            continue;
        }
        collect_component_properties(root, &mut properties);
        if properties.len() >= MAX_COMPONENT_PROPERTIES {
            properties.truncate(MAX_COMPONENT_PROPERTIES);
            break;
        }
    }
    properties
}

/// Collects declarations from one `properties` object literal.
fn collect_component_properties(root: &DataRoot, properties: &mut Vec<ComponentProperty>) {
    let text = root.text.as_str();
    let bytes = text.as_bytes();
    let Some(object_end) = find_matching(bytes, 0, bytes.len()) else {
        return;
    };
    let mut index = 1_usize;
    while index + 1 < object_end && properties.len() < MAX_COMPONENT_PROPERTIES {
        index = skip_trivia(bytes, index);
        if index >= object_end {
            break;
        }
        match bytes[index] {
            b',' => {
                index += 1;
                continue;
            }
            b'}' => break,
            b'{' | b'[' | b'(' => {
                index = find_matching(bytes, index, object_end).unwrap_or(object_end);
                continue;
            }
            _ => {}
        }
        let Some((name, key_start, key_end)) = property_key(text, index) else {
            index += 1;
            continue;
        };
        let colon = skip_trivia(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            index = key_end;
            continue;
        }
        let value_start = skip_trivia(bytes, colon + 1);
        let Some(value_end) = expression_end(bytes, value_start, object_end) else {
            index = key_end;
            continue;
        };
        if let Some(property) = component_property(
            root,
            text,
            &name,
            key_start,
            key_end,
            value_start,
            value_end,
        ) {
            properties.push(property);
        }
        index = value_end;
    }
}

/// Parses an object member key at `index`.
fn property_key(text: &str, index: usize) -> Option<(String, usize, usize)> {
    let bytes = text.as_bytes();
    match *bytes.get(index)? {
        b'\'' | b'"' => {
            let end = skip_lexeme(bytes, index, 0)?;
            let name = text.get(index + 1..end.checked_sub(1)?)?.to_owned();
            Some((name, index, end))
        }
        byte if is_identifier_start(byte) => {
            let mut end = index + 1;
            while end < bytes.len() && is_identifier_continue(bytes[end]) {
                end += 1;
            }
            Some((text.get(index..end)?.to_owned(), index, end))
        }
        _ => None,
    }
}

/// Builds one declaration, rejecting names that cannot be attribute names.
fn component_property(
    root: &DataRoot,
    text: &str,
    name: &str,
    key_start: usize,
    key_end: usize,
    value_start: usize,
    value_end: usize,
) -> Option<ComponentProperty> {
    if !is_valid_property_name(name) {
        return None;
    }
    let start = root
        .range
        .start()
        .checked_add(u32::try_from(key_start).ok()?)?;
    let end = root
        .range
        .start()
        .checked_add(u32::try_from(key_end).ok()?)?;
    let range = TextRange::new(start, end).ok()?;
    let value = text.get(value_start..value_end).unwrap_or_default();
    let (type_text, default_text) = property_type(value);
    Some(ComponentProperty {
        name: name.to_owned(),
        type_text,
        default_text,
        documentation: leading_documentation(text, key_start),
        script: root.script.clone(),
        range,
    })
}

/// Returns whether `name` can be shown as a WXML attribute.
fn is_valid_property_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PROPERTY_TYPE_CHARS
        && !name
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && !name
            .chars()
            .any(|character| matches!(character, '<' | '>' | '=' | '"' | '\'' | '\0'))
}

/// Derives the display type and default expression of one property value.
fn property_type(value: &str) -> (String, Option<String>) {
    let trimmed = value.trim();
    if trimmed.starts_with('{') {
        let type_expression = object_member(trimmed, "type");
        let optional = object_member(trimmed, "optionalTypes")
            .map(constructor_list)
            .unwrap_or_default();
        let default = object_member(trimmed, "value")
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| truncate_chars(text, MAX_PROPERTY_TYPE_CHARS));
        let mut types = Vec::new();
        if let Some(expression) = type_expression {
            push_constructor(&mut types, expression);
        }
        for expression in optional {
            push_constructor(&mut types, &expression);
        }
        let type_text = display_union(&types);
        (type_text, default)
    } else {
        let mut types = Vec::new();
        push_constructor(&mut types, trimmed);
        (display_union(&types), None)
    }
}

/// Joins constructor names, collapsing unknown/`any` kinds.
fn display_union(types: &[String]) -> String {
    if types.is_empty() || types.iter().any(|kind| kind == "any") {
        return "any".to_owned();
    }
    types.join(" | ")
}

/// Adds one constructor spelling without duplicates.
fn push_constructor(types: &mut Vec<String>, expression: &str) {
    let name = constructor_name(expression);
    if !types.contains(&name) {
        types.push(name);
    }
}

/// Maps a JavaScript constructor expression to a display name.
fn constructor_name(expression: &str) -> String {
    let trimmed = expression.trim();
    let mapped = match trimmed {
        "String" => "string",
        "Number" => "number",
        "Boolean" => "boolean",
        "Object" => "object",
        "Array" => "array",
        "Function" => "function",
        "null" => "null",
        "" => "any",
        other
            if other.len() <= MAX_PROPERTY_TYPE_CHARS
                && other.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '$' | '.')
                }) =>
        {
            other
        }
        _ => "any",
    };
    mapped.to_owned()
}

/// Finds one top-level object member value.
fn object_member<'a>(object: &'a str, name: &str) -> Option<&'a str> {
    let bytes = object.as_bytes();
    let object_end = find_matching(bytes, 0, bytes.len())?;
    let mut index = 1_usize;
    while index + 1 < object_end {
        index = skip_trivia(bytes, index);
        if index >= object_end {
            break;
        }
        match bytes[index] {
            b',' => {
                index += 1;
                continue;
            }
            b'}' => break,
            _ => {}
        }
        let Some((key, _, key_end)) = property_key(object, index) else {
            index += 1;
            continue;
        };
        let colon = skip_trivia(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            index = key_end;
            continue;
        }
        let value_start = skip_trivia(bytes, colon + 1);
        let Some(value_end) = expression_end(bytes, value_start, object_end) else {
            index = key_end;
            continue;
        };
        if key == name {
            return object.get(value_start..value_end);
        }
        index = value_end;
    }
    None
}

/// Splits an `optionalTypes` array into top-level element expressions.
fn constructor_list(expression: &str) -> Vec<String> {
    let trimmed = expression.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|text| text.strip_suffix(']'))
    else {
        return Vec::new();
    };
    split_top_level(inner)
        .into_iter()
        .map(constructor_name)
        .collect()
}

/// Splits an expression list on top-level commas.
fn split_top_level(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0_usize;
    let mut depth = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        if let Some(next) = skip_lexeme(bytes, index, 0) {
            index = next;
            continue;
        }
        match bytes[index] {
            b'{' | b'[' | b'(' => depth = depth.saturating_add(1),
            b'}' | b']' | b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start <= text.len() {
        parts.push(&text[start..]);
    }
    parts
}

/// Finds a `JSDoc` or contiguous line-comment block immediately before `key_start`.
fn leading_documentation(text: &str, key_start: usize) -> Option<String> {
    let before = text.get(..key_start)?.trim_end();
    if let Some(rest) = before.strip_suffix("*/") {
        let start = rest.rfind("/**")?;
        return clean_block_documentation(rest.get(start + 3..)?);
    }
    let mut lines = Vec::new();
    for line in before.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        let Some(rest) = line.strip_prefix("//") else {
            break;
        };
        lines.push(rest.trim().to_owned());
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(truncate_chars(&lines.join("\n"), MAX_PROPERTY_DOC_CHARS))
}

/// Strips the leading `*` decoration from a block documentation comment.
fn clean_block_documentation(text: &str) -> Option<String> {
    let mut lines: Vec<&str> = text
        .lines()
        .map(|line| {
            let line = line.trim();
            line.strip_prefix('*').unwrap_or(line).trim()
        })
        .collect();
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    Some(truncate_chars(&lines.join("\n"), MAX_PROPERTY_DOC_CHARS))
}

/// Truncates text to `limit` characters.
fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    fn slice<'a>(text: &'a str, root: &DataRoot) -> &'a str {
        &text[root.range.start() as usize..root.range.end() as usize]
    }

    #[test]
    fn resolves_ts_over_js_and_reports_conflict() {
        let base = path("pages/index/index");
        let both = resolve_script(&base, |candidate| {
            candidate.as_str() == "pages/index/index.ts"
                || candidate.as_str() == "pages/index/index.js"
        });
        assert_eq!(both.script, Some(path("pages/index/index.ts")));
        assert!(both.conflict);

        let only_js = resolve_script(&base, |candidate| {
            candidate.as_str() == "pages/index/index.js"
        });
        assert_eq!(only_js.script, Some(path("pages/index/index.js")));
        assert!(!only_js.conflict);

        let none = resolve_script(&base, |_| false);
        assert_eq!(none.script, None);
        assert!(!none.conflict);
    }

    #[test]
    fn extracts_page_data() {
        let script = path("pages/index/index.ts");
        let source = "Page({\n  data: {\n    user: { name: 'Ada' },\n  },\n  onLoad() {}\n});";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].kind, DataRootKind::PageData);
        assert_eq!(roots[0].script, script);
        assert_eq!(roots[0].name.as_deref(), Some("data"));
        assert_eq!(
            slice(source, &roots[0]),
            "{\n    user: { name: 'Ada' },\n  }"
        );
        assert_eq!(roots[0].text, slice(source, &roots[0]));
    }

    #[test]
    fn extracts_component_properties_and_data_in_source_order() {
        let script = path("components/card/card.ts");
        let source = "Component({\n  properties: { title: { type: String } },\n  data: { open: false },\n});";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].kind, DataRootKind::ComponentProperties);
        assert_eq!(slice(source, &roots[0]), "{ title: { type: String } }");
        assert_eq!(roots[1].kind, DataRootKind::ComponentData);
        assert_eq!(slice(source, &roots[1]), "{ open: false }");
    }

    #[test]
    fn ignores_registrations_in_strings_and_comments() {
        let script = path("a.ts");
        let source = r#"
            // Page({ data: { nope: 1 } })
            /* Component({ properties: {} }) */
            const text = "Page({ data: {} })";
            const other = 'Component({ data: {} })';
            Page({ data: { yes: true } });
        "#;
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(slice(source, &roots[0]), "{ yes: true }");
    }

    #[test]
    fn ignores_member_access_and_non_object_arguments() {
        let script = path("a.ts");
        let source = r"
            wx.Page({ data: { nope: 1 } });
            MyPage({ data: { nope: 2 } });
            Page(getConfig());
            Component();
            Page({ data: { yes: 3 } });
        ";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(slice(source, &roots[0]), "{ yes: 3 }");
    }

    #[test]
    fn ignores_nested_data_keys() {
        let script = path("a.ts");
        let source = "Page({ data: { nested: { data: { nope: 1 } } } });";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(
            slice(source, &roots[0]),
            "{ nested: { data: { nope: 1 } } }"
        );
    }

    #[test]
    fn handles_template_literals_and_braces_in_strings() {
        let script = path("a.ts");
        let source = "const banner = `hello ${ { a: 1 }.a } {`;\nPage({ data: { label: '}', other: `x${1}` } });";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(slice(source, &roots[0]), "{ label: '}', other: `x${1}` }");
    }

    #[test]
    fn skips_trivia_around_keys_and_values() {
        let script = path("a.ts");
        let source = "Page({ /*a*/ data /*b*/: /*c*/ { x: 1 } /*d*/, onLoad() {} });";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(slice(source, &roots[0]), "{ x: 1 }");
    }

    #[test]
    fn captures_non_object_values() {
        let script = path("a.ts");
        let source = "Page({\n  data: createData(1, 2),\n  onLoad() {},\n});";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(slice(source, &roots[0]), "createData(1, 2)");
    }

    #[test]
    fn records_ranges_for_crlf_and_unicode_prefixes() {
        let script = path("a.ts");
        let source = "// 中文注释\r\nconst 值 = 1;\r\nPage({ data: { 名: '值' } });";
        let roots = extract_data_roots(&script, source);
        assert_eq!(roots.len(), 1);
        assert_eq!(slice(source, &roots[0]), "{ 名: '值' }");
        assert_eq!(roots[0].text, "{ 名: '值' }");
        assert!(source.is_char_boundary(roots[0].range.start() as usize));
        assert!(source.is_char_boundary(roots[0].range.end() as usize));
    }

    #[test]
    fn malformed_input_does_not_panic() {
        let script = path("a.ts");
        for source in [
            "",
            "Page(",
            "Page({",
            "Page({ data:",
            "Page({ data: {",
            "Page({ data: 'unterminated",
            "`unterminated ${",
            "Page({ data: { a: 1 }",
            "Component({ properties: {",
            "}}}}{{{ Page({ data: {} })",
        ] {
            let roots = extract_data_roots(&script, source);
            for root in roots {
                assert!(source.is_char_boundary(root.range.start() as usize));
                assert!(source.is_char_boundary(root.range.end() as usize));
                assert!(root.range.end() as usize <= source.len());
            }
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let script = path("a.ts");
        let deep = format!("Page({{ data: {}{} }});", "{".repeat(400), "}".repeat(400));
        assert_eq!(extract_data_roots(&script, &deep), Vec::new());

        let unrelated = format!("{}{}", "(".repeat(400), ")".repeat(400));
        assert_eq!(extract_data_roots(&script, &unrelated), Vec::new());
    }

    #[test]
    fn extracts_component_property_metadata() {
        let script = path("components/card/card.ts");
        let source = "Component({\n  properties: {\n    /** 标题，支持字符串或数字 */\n    title: { type: String, optionalTypes: [Number], value: '' },\n    // 是否禁用\n    disabled: { type: Boolean, value: false },\n    plain: String,\n  },\n});";
        let roots = extract_data_roots(&script, source);
        let properties = extract_component_properties(&roots);
        assert_eq!(properties.len(), 3);

        assert_eq!(properties[0].name, "title");
        assert_eq!(properties[0].type_text, "string | number");
        assert_eq!(properties[0].default_text.as_deref(), Some("''"));
        assert_eq!(
            properties[0].documentation.as_deref(),
            Some("标题，支持字符串或数字")
        );
        assert_eq!(properties[0].script, script);
        assert_eq!(
            &source[properties[0].range.start() as usize..properties[0].range.end() as usize],
            "title"
        );

        assert_eq!(properties[1].name, "disabled");
        assert_eq!(properties[1].type_text, "boolean");
        assert_eq!(properties[1].default_text.as_deref(), Some("false"));
        assert_eq!(properties[1].documentation.as_deref(), Some("是否禁用"));

        assert_eq!(properties[2].name, "plain");
        assert_eq!(properties[2].type_text, "string");
        assert_eq!(properties[2].default_text, None);
        assert_eq!(properties[2].documentation, None);
    }

    #[test]
    fn property_types_are_bounded_and_unknown_kinds_degrade() {
        let script = path("components/card/card.ts");
        let source = "Component({ properties: { a: { type: createType() }, b: { type: null, optionalTypes: [String, null] }, c: Object } });";
        let roots = extract_data_roots(&script, source);
        let properties = extract_component_properties(&roots);
        assert_eq!(properties.len(), 3);
        assert_eq!(properties[0].type_text, "any");
        assert_eq!(properties[1].type_text, "null | string");
        assert_eq!(properties[2].type_text, "object");
    }

    #[test]
    fn malformed_component_properties_do_not_panic() {
        let script = path("a.ts");
        let sources = [
            "Component({ properties: ",
            "Component({ properties: {",
            "Component({ properties: { title:",
            "Component({ properties: { \"unterminated: String } });",
            "Component({ properties: { title: { type: String, optionalTypes: [ } });",
        ];
        for source in sources {
            let roots = extract_data_roots(&script, source);
            let properties = extract_component_properties(&roots);
            for property in properties {
                assert!(
                    source.is_char_boundary(property.range.start() as usize),
                    "{source:?}"
                );
                assert!(
                    source.is_char_boundary(property.range.end() as usize),
                    "{source:?}"
                );
                assert!(property.range.end() as usize <= source.len(), "{source:?}");
            }
        }
    }

    #[test]
    fn non_object_properties_yield_no_declarations() {
        let script = path("a.ts");
        let source = "const shared = { title: String };\nComponent({ properties: shared });";
        let roots = extract_data_roots(&script, source);
        assert_eq!(extract_component_properties(&roots), Vec::new());
    }
}
