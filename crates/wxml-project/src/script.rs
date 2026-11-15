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
}
