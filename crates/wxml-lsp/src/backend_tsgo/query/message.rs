//! User-visible diagnostic message sanitization.
//!
//! TypeScript diagnostics embed bridge helper names and typings internals
//! (`__wxml_root`, `FilterUnknownType<...>`, `WechatMiniprogram.*`). None of
//! that is meaningful to a WXML author, so the published text is a cleaned,
//! bounded variant; the raw engine message is kept for internal logs only.
//!
//! This is a pure port of the TypeScript `sanitizeMessage` oracle. It never
//! echoes precomputed output: every transformation below decides the result,
//! and the five native-parity cases call this function with the same raw
//! strings as the oracle. The display budget here is separate from
//! `wxml-lsp`'s `MAX_DISPLAY_MESSAGE_CHARS`; this module owns the 200-unit C1
//! budget.

/// Maximum UTF-16 code units in a sanitized user-visible message.
pub(crate) const MAX_MESSAGE_UNITS: usize = 200;

/// Maximum input characters processed before the rewrite pipeline.
///
/// The output is always bounded by [`MAX_MESSAGE_UNITS`]; this cap only keeps
/// a malformed or hostile engine message from making the intermediate passes
/// unbounded. Truncating the input at a character boundary cannot change the
/// published prefix for any message the 200-unit budget keeps.
pub(crate) const MAX_INPUT_CHARS: usize = 8_192;

/// Generic bridge wrappers whose single type argument is user-visible.
const WRAPPERS: [&str; 9] = [
    "FilterUnknownType",
    "PropertyOptionToData",
    "FullPropertyToData",
    "MixinProperties",
    "MixinMethods",
    "MixinData",
    "ExtractProperties",
    "ExtractMethods",
    "ExtractBehaviorType",
];

/// Cleans one engine message for display.
#[must_use]
pub(crate) fn sanitize_message(raw: &str) -> String {
    let mut text = bounded_input(raw).to_owned();
    for wrapper in WRAPPERS {
        text = unwrap_generic(&text, wrapper);
    }
    text = strip_root_prefixes(&text);
    text = replace_prefixed(&text, "__wxml_", "helper", is_helper_byte);
    text = replace_prefixed(&text, "__WxmlProps_", "declared properties", |byte| {
        byte.is_ascii_digit()
    });
    text = replace_prefixed(
        &text,
        "WechatMiniprogram.",
        "miniprogram instance",
        is_typings_byte,
    );
    text = collapse_duplicated_intersections(&text);
    text = dedupe_repeated_literals(&text);
    text = remove_empty_intersections(&text);
    text = remove_intersection_before_quote(&text);
    text = remove_intersection_after_quote(&text);
    text = normalize_helper_dots(&text);
    text = collapse_whitespace(&text);
    text = collapse_ampersand_pairs(&text);
    truncate_message(&text)
}

/// Borrows at most [`MAX_INPUT_CHARS`] characters from `raw`.
fn bounded_input(raw: &str) -> &str {
    match raw.char_indices().nth(MAX_INPUT_CHARS) {
        Some((end, _)) => &raw[..end],
        None => raw,
    }
}

/// Replaces `Wrapper<X>` with `X` for one single-argument wrapper.
fn unwrap_generic(text: &str, name: &str) -> String {
    let marker = format!("{name}<");
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(relative) = text[index..].find(&marker) else {
            output.push_str(&text[index..]);
            break;
        };
        let start = index + relative;
        output.push_str(&text[index..start]);

        let open = start + name.len();
        let mut depth = 0_i32;
        let mut close = None;
        for (offset, byte) in text.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            // An unterminated wrapper is malformed input. Keep the remaining
            // text rather than slicing through a UTF-8 boundary; the final
            // budget still bounds the published result.
            output.push_str(&text[open + 1..]);
            return output;
        };
        output.push_str(&text[open + 1..close]);
        index = close + 1;
    }
    output
}

/// Removes generated root/props prefixes such as `__wxml_root.` and
/// `__wxml_props_0.`.
fn strip_root_prefixes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if let Some(end) = root_prefix_end(text, index) {
            index = end;
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Returns the end of a root/props prefix starting at `index`.
fn root_prefix_end(text: &str, index: usize) -> Option<usize> {
    let rest = text.get(index..)?;
    let after = if let Some(after) = rest.strip_prefix("__wxml_root") {
        after.strip_prefix('_').unwrap_or(after)
    } else {
        rest.strip_prefix("__wxml_props_")?
    };
    let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    after[digits..]
        .starts_with('.')
        .then_some(index + rest.len() - after.len() + digits + 1)
}

/// Whether `byte` belongs to the `[A-Za-z0-9_]` helper-name class.
const fn is_helper_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `byte` belongs to the `[A-Za-z0-9_.$]` typings-name class.
const fn is_typings_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$')
}

/// Replaces `prefix` plus every following byte accepted by `allowed`.
fn replace_prefixed(
    text: &str,
    prefix: &str,
    replacement: &str,
    allowed: impl Fn(u8) -> bool,
) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(relative) = text[index..].find(prefix) else {
            output.push_str(&text[index..]);
            break;
        };
        let start = index + relative;
        output.push_str(&text[index..start]);
        let mut end = start + prefix.len();
        while end < text.len() && allowed(text.as_bytes()[end]) {
            end += 1;
        }
        output.push_str(replacement);
        index = end;
    }
    output
}

/// Collapses `A & A` when the two intersection members are identical.
fn collapse_duplicated_intersections(text: &str) -> String {
    let parts = split_intersections(text);
    if parts.len() < 2 {
        return text.to_owned();
    }
    let mut unique: Vec<&str> = Vec::with_capacity(parts.len());
    for part in parts {
        if !unique.contains(&part) {
            unique.push(part);
        }
    }
    unique.join(" & ")
}

/// Splits on `\s+&\s+`, returning the parts without the delimiter.
fn split_intersections(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut segment_start = 0;
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '&' {
            let segment = &text[segment_start..index];
            let trimmed = segment.trim_end_matches(|c: char| c.is_whitespace());
            let after_ampersand = index + 1;
            let rest = &text[after_ampersand..];
            let after_space = rest.trim_start_matches(|c: char| c.is_whitespace());
            let next_start = after_ampersand + rest.len() - after_space.len();
            if trimmed.len() < segment.len() && next_start > after_ampersand {
                parts.push(&text[segment_start..segment_start + trimmed.len()]);
                segment_start = next_start;
                index = next_start;
                continue;
            }
        }
        index += character.len_utf8();
    }
    parts.push(&text[segment_start..]);
    parts
}

/// Removes a repeated flat `{ ... }` literal from the first duplicated pair.
fn dedupe_repeated_literals(text: &str) -> String {
    let literals = brace_literals(text);
    let mut output = text.to_owned();
    for literal in literals {
        let Some(first) = output.find(literal) else {
            continue;
        };
        let search_from = first + literal.len();
        let Some(relative) = output[search_from..].find(literal) else {
            continue;
        };
        let second = search_from + relative;
        output.replace_range(second..second + literal.len(), "");
        output = collapse_double_ampersands(&output);
    }
    output
}

/// Collects flat brace literals: `{` with no nested brace before `}`.
fn brace_literals(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'{' {
            let mut cursor = index + 1;
            let mut close = None;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'{' => break,
                    b'}' => {
                        close = Some(cursor);
                        break;
                    }
                    _ => cursor += 1,
                }
            }
            if let Some(close) = close {
                literals.push(&text[index..=close]);
                index = close + 1;
                continue;
            }
        }
        index += 1;
    }
    literals
}

/// Collapses `\s*&\s*&\s*` to ` & `.
fn collapse_double_ampersands(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '&' {
            let after_first = index + 1;
            let after_skip = skip_whitespace(text, after_first);
            if after_skip < text.len() && text.as_bytes()[after_skip] == b'&' {
                let trimmed = output.trim_end_matches(|c: char| c.is_whitespace()).len();
                output.truncate(trimmed);
                output.push_str(" & ");
                index = skip_whitespace(text, after_skip + 1);
                continue;
            }
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Removes intersections with `...` or an empty `{}` literal.
fn remove_empty_intersections(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '&' {
            let after_ampersand = index + 1;
            let token = skip_whitespace(text, after_ampersand);
            let token_len = empty_intersection_len(text, token);
            if token_len > 0 {
                let trimmed = output.trim_end_matches(|c: char| c.is_whitespace()).len();
                output.truncate(trimmed);
                index = token + token_len;
                continue;
            }
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Length of `...` or `{ }` at `index`, or zero when neither matches.
fn empty_intersection_len(text: &str, index: usize) -> usize {
    if text[index..].starts_with("...") {
        return 3;
    }
    if !text[index..].starts_with('{') {
        return 0;
    }
    let close = skip_whitespace(text, index + 1);
    if text[close..].starts_with('}') {
        close - index + 1
    } else {
        0
    }
}

/// Removes `\s*&\s*` before a single or double quote.
fn remove_intersection_before_quote(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '&' {
            let next = skip_whitespace(text, index + 1);
            if next < text.len() && matches!(text.as_bytes()[next], b'\'' | b'"') {
                let trimmed = output.trim_end_matches(|c: char| c.is_whitespace()).len();
                output.truncate(trimmed);
                index = next;
                continue;
            }
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Removes `\s*&\s*` after a single or double quote.
fn remove_intersection_after_quote(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if matches!(output.as_bytes().last(), Some(b'\'' | b'"')) {
            let ampersand = skip_whitespace(text, index);
            if ampersand < text.len() && text.as_bytes()[ampersand] == b'&' {
                index = skip_whitespace(text, ampersand + 1);
                continue;
            }
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Collapses whitespace before a dot after the word `helper`.
fn normalize_helper_dots(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if text[index..].starts_with("helper") {
            let boundary = index == 0 || !is_helper_byte(text.as_bytes()[index - 1]);
            let dot = skip_whitespace(text, index + "helper".len());
            if boundary && text[dot..].starts_with('.') {
                output.push_str("helper.");
                index = dot + 1;
                continue;
            }
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Collapses all whitespace runs to one space and trims both ends.
fn collapse_whitespace(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            pending_space = !output.is_empty();
        } else {
            if pending_space {
                output.push(' ');
                pending_space = false;
            }
            output.push(character);
        }
    }
    output
}

/// Collapses `&\s*&` to `&`.
fn collapse_ampersand_pairs(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character == '&' {
            let pair = skip_whitespace(text, index + 1);
            if pair < text.len() && text.as_bytes()[pair] == b'&' {
                output.push('&');
                index = pair + 1;
                continue;
            }
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

/// Returns the first non-whitespace byte index at or after `index`.
fn skip_whitespace(text: &str, index: usize) -> usize {
    let rest = &text[index..];
    index + rest.len() - rest.trim_start_matches(|c: char| c.is_whitespace()).len()
}

/// Truncates to [`MAX_MESSAGE_UNITS`] UTF-16 units with a trailing ellipsis.
fn truncate_message(text: &str) -> String {
    if text.encode_utf16().count() <= MAX_MESSAGE_UNITS {
        return text.to_owned();
    }
    let budget = MAX_MESSAGE_UNITS - 1;
    let mut used = 0;
    let mut end = 0;
    for (offset, character) in text.char_indices() {
        let width = character.len_utf16();
        if used + width > budget {
            end = offset;
            break;
        }
        used += width;
        end = offset + character.len_utf8();
    }
    let prefix = text[..end].trim_end();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_messages_0005_helper_names_are_removed() {
        assert_eq!(
            sanitize_message("'__wxml_root.maybeUser' is possibly 'null'."),
            "'maybeUser' is possibly 'null'."
        );
        assert_eq!(
            sanitize_message("Property '__wxml_check_0' does not exist on type 'X'."),
            "Property 'helper' does not exist on type 'X'."
        );
    }

    #[test]
    fn case_messages_0016_typings_internals_are_unwrapped() {
        let raw = "Property 'subtitle' does not exist on type 'FilterUnknownType<{ state: string; }> & FilterUnknownType<{}> & PropertyOptionToData<FilterUnknownType<{ title: { type: StringConstructor; }; }>>'.";
        let clean = sanitize_message(raw);
        assert!(!clean.contains("FilterUnknownType"), "{clean}");
        assert!(!clean.contains("PropertyOptionToData"), "{clean}");
        assert!(clean.contains("{ state: string; }"), "{clean}");
        assert_eq!(
            clean,
            "Property 'subtitle' does not exist on type '{ state: string; } & { title: { type: StringConstructor; }; }'."
        );
    }

    #[test]
    fn case_messages_0025_generated_overlay_type_names_are_replaced() {
        assert_eq!(
            sanitize_message(
                "Property 'subtitle' does not exist on type '{ state: string; } & __WxmlProps_0'."
            ),
            "Property 'subtitle' does not exist on type '{ state: string; } & declared properties'."
        );
    }

    #[test]
    fn case_messages_0032_duplicated_intersection_literals_collapse() {
        assert_eq!(
            sanitize_message(
                "Property 'nmae' does not exist on type '{ name: string; } & { name: string; }'."
            ),
            "Property 'nmae' does not exist on type '{ name: string; }'."
        );
    }

    #[test]
    fn case_messages_0039_long_messages_are_truncated() {
        let members = (0..60)
            .map(|index| format!("field{index}: string"))
            .collect::<Vec<_>>()
            .join("; ");
        let clean = sanitize_message(&format!(
            "Property 'x' does not exist on type '{{ {members} }}'."
        ));
        assert!(clean.encode_utf16().count() <= MAX_MESSAGE_UNITS);
        assert!(clean.ends_with('…'), "{clean}");
        assert!(clean.starts_with("Property 'x' does not exist"), "{clean}");
        // The 199th budget unit is the space before the closing brace;
        // `trimEnd` removes it, so the final message is 198 units + `…`.
        assert_eq!(clean.encode_utf16().count(), 199);
    }

    #[test]
    fn removes_numbered_root_props_and_typings_prefixes() {
        assert_eq!(
            sanitize_message("__wxml_root_2.title is not assignable to __wxml_props_4.title"),
            "title is not assignable to title"
        );
        assert_eq!(
            sanitize_message("Type 'WechatMiniprogram.IAnyObject' is not assignable."),
            "Type 'miniprogram instance' is not assignable."
        );
    }

    #[test]
    fn collapses_remaining_ampersand_runs_and_helper_spacing() {
        assert_eq!(sanitize_message("A & & B"), "A & B");
        assert_eq!(sanitize_message("helper .x"), "helper.x");
    }

    #[test]
    fn input_is_bounded_at_a_character_boundary() {
        let raw = "é".repeat(MAX_INPUT_CHARS + 10);
        let clean = sanitize_message(&raw);
        assert!(clean.encode_utf16().count() <= MAX_MESSAGE_UNITS);
        assert!(clean.ends_with('…'));
    }
}
