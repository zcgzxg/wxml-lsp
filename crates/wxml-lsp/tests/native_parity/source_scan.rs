//! Minimal TypeScript source scanner for parity ledger verification.
//!
//! This is not a TypeScript parser. It skips comments, quoted strings,
//! template literals, and regex literals just enough to match delimiters in the
//! old test corpus. The ledger gate proves the extracted `test(...)` ranges
//! reproduce all 339 recorded `body_sha256` values.

use std::ops::Range;

/// Returns the byte range of a 1-based line, excluding the line terminator.
pub(crate) const fn line_range(source: &str, line: usize) -> Option<Range<usize>> {
    if line == 0 {
        return None;
    }
    let bytes = source.as_bytes();
    let mut current = 1_usize;
    let mut start = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            if current == line {
                let end = if index > start && bytes[index - 1] == b'\r' {
                    index - 1
                } else {
                    index
                };
                return Some(start..end);
            }
            current += 1;
            start = index + 1;
        }
        index += 1;
    }
    if current == line {
        Some(start..bytes.len())
    } else {
        None
    }
}

/// Returns the byte offset of the first non-whitespace byte on a line.
pub(crate) fn first_non_whitespace(source: &str, line: usize) -> Option<usize> {
    let range = line_range(source, line)?;
    let bytes = source.as_bytes();
    let mut index = range.start;
    while index < range.end && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    Some(index)
}

/// Returns true for a `test(...)` / `it(...)` declaration line.
///
/// This is the token check behind the ledger line assertion; a line that merely
/// contains `test(` inside a string or assertion body does not qualify.
pub(crate) fn is_test_declaration(line: &str) -> bool {
    let line = line.trim_start();
    let Some(rest) = line
        .strip_prefix("test")
        .or_else(|| line.strip_prefix("it"))
    else {
        return false;
    };
    let rest = if let Some(rest) = rest.strip_prefix('.') {
        let end = rest
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return false;
        }
        &rest[end..]
    } else {
        rest
    };
    rest.starts_with('(')
}

/// Extracts a function name from a declaration line.
pub(crate) fn parse_fn_name(line: &str) -> Option<String> {
    let index = line.find("fn ")?;
    let after = line.get(index + 3..)?;
    let name = after
        .split(|character: char| character == '(' || character == '<' || character.is_whitespace())
        .next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

/// Returns the exact `test(...)` / `it(...)` call range on a 1-based line.
///
/// The range starts at the `test` token and ends at the matching `)`, mirroring
/// the original TypeScript AST's `node.getStart()` / `node.getEnd()` call
/// expression range. A trailing statement semicolon is not included.
pub(crate) fn test_call_range(source: &str, line: usize) -> Option<Range<usize>> {
    let start = first_non_whitespace(source, line)?;
    let rest = source.get(start..)?;
    if !rest.starts_with("test") && !rest.starts_with("it") {
        return None;
    }
    let open = rest.find('(')? + start;
    let prefix = source.get(start..open)?;
    if prefix != "test" && prefix != "it" {
        return None;
    }
    let close = matching_delimiter(source, open, b'(', b')')?;
    Some(start..close + 1)
}

/// Returns the inner body of a function starting on a 1-based line.
pub(crate) fn function_body(source: &str, line: usize) -> Option<&str> {
    let start = first_non_whitespace(source, line)?;
    let function_keyword = source.get(start..)?.find("fn ")? + start;
    let open_paren = source.get(function_keyword..)?.find('(')? + function_keyword;
    let close_paren = matching_delimiter(source, open_paren, b'(', b')')?;
    let open_brace = source.get(close_paren + 1..)?.find('{')? + close_paren + 1;
    let close_brace = matching_delimiter(source, open_brace, b'{', b'}')?;
    Some(&source[open_brace + 1..close_brace])
}

/// Returns true when the full attribute/doc block above `function_index`
/// contains `needle`.
pub(crate) fn attribute_block_contains<S: AsRef<str>>(
    lines: &[S],
    function_index: usize,
    needle: &str,
) -> bool {
    let mut start = function_index;
    while start > 0 {
        let previous = lines[start - 1].as_ref().trim();
        if is_attribute_or_doc(previous) {
            start -= 1;
        } else {
            break;
        }
    }
    lines[start..function_index]
        .iter()
        .any(|line| line.as_ref().trim_start().starts_with(needle))
}

fn is_attribute_or_doc(line: &str) -> bool {
    line.is_empty()
        || line.starts_with("#[")
        || line.starts_with("///")
        || line.starts_with("//!")
        || line.starts_with("/**")
        || line.starts_with('*')
}

/// Matches a delimiter while skipping comments, strings, templates, and regexes.
pub(crate) fn matching_delimiter(
    source: &str,
    open_index: usize,
    open: u8,
    close: u8,
) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_index).copied()? != open {
        return None;
    }
    let mut depth = 0_usize;
    let mut index = open_index;
    let mut can_start_regex = true;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' {
            if let Some(next) = bytes.get(index + 1).copied() {
                if next == b'/' {
                    index = skip_line_comment(bytes, index);
                    continue;
                }
                if next == b'*' {
                    index = skip_block_comment(bytes, index)?;
                    continue;
                }
            }
            if can_start_regex {
                index = skip_regex(bytes, index)?;
                can_start_regex = false;
                continue;
            }
            can_start_regex = true;
            index += 1;
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            index = skip_quoted(bytes, index, byte)?;
            can_start_regex = false;
            continue;
        }
        if byte == b'`' {
            index = skip_template(bytes, index)?;
            can_start_regex = false;
            continue;
        }
        if byte == open {
            depth += 1;
            index += 1;
            can_start_regex = true;
            continue;
        }
        if byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
            index += 1;
            can_start_regex = false;
            continue;
        }
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'$')
            {
                end += 1;
            }
            can_start_regex = is_regex_prefix_word(source.get(index..end)?);
            index = end;
            continue;
        }
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        can_start_regex = !matches!(byte, b')' | b']' | b'}' | b'.');
        index += 1;
    }
    None
}

fn is_regex_prefix_word(word: &str) -> bool {
    matches!(
        word,
        "return"
            | "case"
            | "throw"
            | "else"
            | "typeof"
            | "instanceof"
            | "in"
            | "of"
            | "new"
            | "delete"
            | "void"
            | "yield"
            | "await"
    )
}

const fn skip_line_comment(bytes: &[u8], index: usize) -> usize {
    let mut cursor = index + 2;
    while cursor < bytes.len() && bytes[cursor] != b'\n' {
        cursor += 1;
    }
    cursor
}

const fn skip_block_comment(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index + 2;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
            return Some(cursor + 2);
        }
        cursor += 1;
    }
    None
}

const fn skip_quoted(bytes: &[u8], index: usize, quote: u8) -> Option<usize> {
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            byte if byte == quote => return Some(cursor + 1),
            _ => cursor += 1,
        }
    }
    None
}

const fn skip_template(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'`' => return Some(cursor + 1),
            _ => cursor += 1,
        }
    }
    None
}

const fn skip_regex(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index + 1;
    let mut in_class = false;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'[' => {
                in_class = true;
                cursor += 1;
            }
            b']' => {
                in_class = false;
                cursor += 1;
            }
            b'/' if !in_class => {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
                    cursor += 1;
                }
                return Some(cursor);
            }
            b'\n' => return None,
            _ => cursor += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_tokens_are_anchored() {
        assert!(is_test_declaration("test(\"a\", () => {});"));
        assert!(is_test_declaration("  it.skip(\"a\", () => {});"));
        assert!(is_test_declaration("test.only(\"a\", () => {});"));
        assert!(!is_test_declaration("testing(\"a\");"));
        assert!(!is_test_declaration("assert.equal(value, test(x));"));
    }

    #[test]
    fn delimiter_matching_skips_literals_and_regexes() {
        let source = r#"outer(")", /\)/, `template ) ${value}`, [1, 2]) + 1"#;
        let open = source.find('(').expect("open");
        let close = matching_delimiter(source, open, b'(', b')').expect("close");
        assert_eq!(
            &source[open..=close],
            r#"(")", /\)/, `template ) ${value}`, [1, 2])"#
        );
    }

    #[test]
    fn test_call_range_excludes_the_semicolon() {
        let source = "test(\"a\", () => {\n  assert.ok(true);\n});\n";
        let range = test_call_range(source, 1).expect("range");
        assert_eq!(
            &source[range],
            "test(\"a\", () => {\n  assert.ok(true);\n})"
        );
    }
}
