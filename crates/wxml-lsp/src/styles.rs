//! WXML attribute scanning for classes and local resources.
//!
//! These helpers work on the raw template text. Hover, definition and document
//! links parse the document once; class completion reuses the tolerant
//! completion scanner so it also works while the attribute is being typed.

use std::collections::BTreeSet;
use std::ops::Range;

use wxml_syntax::AttributeValue;
use wxml_text::TextRange;

use crate::completion::context::{Context as Detected, Site};

/// One literal class token under the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClassSite {
    /// Byte range of the class name in the template.
    pub(crate) token: TextRange,
    /// Class name.
    pub(crate) name: String,
}

/// One class attribute value being completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClassCompletionSite {
    /// Full attribute value (without quotes).
    pub(crate) value: TextRange,
    /// Token under the cursor that a selected class replaces.
    pub(crate) token: TextRange,
    /// Text before the cursor in the current token.
    pub(crate) prefix: String,
    /// Classes already present in static parts of the value.
    pub(crate) existing: BTreeSet<String>,
    /// Whether the value is quoted.
    pub(crate) quoted: bool,
}

/// One literal resource attribute value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResourceSite {
    /// Enclosing tag name.
    pub(crate) tag: String,
    /// Attribute name.
    pub(crate) attribute: String,
    /// Trimmed value range (without quotes and surrounding whitespace).
    pub(crate) value: TextRange,
    /// Trimmed value text.
    pub(crate) text: String,
}

/// Returns the range inside an attribute value's quotes.
fn inner_value_range(value: AttributeValue<'_>) -> Option<TextRange> {
    let raw = value.raw();
    let range = value.text_range();
    if raw.is_empty() {
        return Some(range);
    }
    let bytes = raw.as_bytes();
    let quote = match bytes.first() {
        Some(b'"') => Some(b'"'),
        Some(b'\'') => Some(b'\''),
        _ => None,
    };
    let Some(quote) = quote else {
        return Some(range);
    };
    let start = range.start().checked_add(1)?;
    let end = if raw.len() >= 2 && bytes.last() == Some(&quote) {
        range.end().checked_sub(1)?
    } else {
        range.end()
    };
    TextRange::new(start, end).ok()
}

/// Returns the trimmed range and text of a value.
fn trimmed_value(text: &str, range: TextRange) -> Option<(TextRange, String)> {
    let raw = text.get(usize::try_from(range.start()).ok()?..usize::try_from(range.end()).ok()?)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lead = raw.len().saturating_sub(raw.trim_start().len());
    let start = range.start().checked_add(u32::try_from(lead).ok()?)?;
    let end = start.checked_add(u32::try_from(trimmed.len()).ok()?)?;
    Some((TextRange::new(start, end).ok()?, trimmed.to_owned()))
}

/// Returns whether a byte may appear in a class name.
const fn is_class_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte >= 0x80
}

/// Scans the class token at `offset` inside a literal attribute value.
fn class_token_at(text: &str, range: TextRange, offset: u32) -> Option<(TextRange, &str)> {
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    let mut probe = usize::try_from(offset).ok()?;
    if probe >= end {
        probe = end.checked_sub(1)?;
    }
    while probe > start && !text.is_char_boundary(probe) {
        probe -= 1;
    }
    let bytes = text.as_bytes();
    if probe >= end {
        return None;
    }
    let probe = if is_class_byte(bytes[probe]) {
        probe
    } else if probe > start && is_class_byte(bytes[probe - 1]) {
        probe - 1
    } else {
        return None;
    };
    let mut token_start = probe;
    while token_start > start && is_class_byte(bytes[token_start - 1]) {
        token_start -= 1;
    }
    let mut token_end = probe + 1;
    while token_end < end && is_class_byte(bytes[token_end]) {
        token_end += 1;
    }
    let token = TextRange::new(
        u32::try_from(token_start).ok()?,
        u32::try_from(token_end).ok()?,
    )
    .ok()?;
    Some((token, text.get(token_start..token_end)?))
}

/// Finds the class token at `offset` in a class attribute's static parts.
///
/// A mixed value such as `class="card {{extra}}"` still exposes `card`; the
/// cursor inside the mustache is left to expression hover instead of being
/// treated as a class.
pub(crate) fn class_site_at(text: &str, offset: u32) -> Option<ClassSite> {
    let parsed = wxml_syntax::parse(text);
    for element in parsed.tree.document().elements() {
        for attribute in element.attributes() {
            if attribute.name() != Some("class") {
                continue;
            }
            let Some(value) = attribute.value() else {
                continue;
            };
            let Some(inner) = inner_value_range(value) else {
                continue;
            };
            if offset < inner.start() || offset > inner.end() {
                continue;
            }
            let start = usize::try_from(inner.start()).ok()?;
            let end = usize::try_from(inner.end()).ok()?;
            let cursor = usize::try_from(offset).ok()?;
            let run = static_runs(text, start, end)
                .into_iter()
                .find(|run| run.start <= cursor && cursor <= run.end)?;
            let run = TextRange::new(u32::try_from(run.start).ok()?, u32::try_from(run.end).ok()?)
                .ok()?;
            let Some((token, name)) = class_token_at(text, run, offset) else {
                return Some(ClassSite {
                    token: TextRange::new(offset, offset).ok()?,
                    name: String::new(),
                });
            };
            return Some(ClassSite {
                token,
                name: name.to_owned(),
            });
        }
    }
    None
}

/// Returns the attribute value bounds and quote for a detected value site.
pub(crate) fn value_bounds(
    text: &str,
    offset: u32,
    detected: &Detected,
) -> Option<(TextRange, Option<u8>)> {
    let Site::AttributeValue { .. } = detected.site else {
        return None;
    };
    let value_start = usize::try_from(detected.replace.start()).ok()?;
    if value_start > text.len() || !text.is_char_boundary(value_start) {
        return None;
    }
    let bytes = text.as_bytes();
    let quote = value_start
        .checked_sub(1)
        .and_then(|index| bytes.get(index).copied())
        .filter(|byte| matches!(byte, b'"' | b'\''));
    let value_end = attribute_value_end(text, value_start, offset, quote)?;
    Some((
        TextRange::new(
            u32::try_from(value_start).ok()?,
            u32::try_from(value_end).ok()?,
        )
        .ok()?,
        quote,
    ))
}

/// Finds the class attribute value being completed at `offset`.
pub(crate) fn class_completion_site(
    text: &str,
    offset: u32,
    detected: &Detected,
) -> Option<ClassCompletionSite> {
    let Site::AttributeValue { attribute, .. } = &detected.site else {
        return None;
    };
    if attribute != "class" {
        return None;
    }
    let (value, quote) = value_bounds(text, offset, detected)?;
    let value_start = usize::try_from(value.start()).ok()?;
    let value_end = usize::try_from(value.end()).ok()?;
    let bytes = text.as_bytes();
    let runs = static_runs(text, value_start, value_end);
    let cursor = usize::try_from(offset).ok()?;
    let run = runs
        .iter()
        .find(|run| run.start <= cursor && cursor <= run.end)?;
    let mut token_start = cursor;
    while token_start > run.start && is_class_byte(bytes[token_start - 1]) {
        token_start -= 1;
    }
    let mut token_end = cursor;
    while token_end < run.end && is_class_byte(bytes[token_end]) {
        token_end += 1;
    }
    let token = TextRange::new(
        u32::try_from(token_start).ok()?,
        u32::try_from(token_end).ok()?,
    )
    .ok()?;
    let prefix = text.get(token_start..cursor)?.to_owned();
    let mut existing = BTreeSet::new();
    for run in &runs {
        for (start, end) in class_tokens_in_run(text, run) {
            if start == token_start && end == token_end {
                continue;
            }
            if let Some(name) = text.get(start..end) {
                existing.insert(name.to_owned());
            }
        }
    }
    Some(ClassCompletionSite {
        value,
        token,
        prefix,
        existing,
        quoted: quote.is_some(),
    })
}

/// Splits one static value run into whitespace-separated class tokens.
fn class_tokens_in_run(text: &str, run: &Range<usize>) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = run.start;
    while index < run.end {
        while index < run.end && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= run.end {
            break;
        }
        let start = index;
        while index < run.end && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        tokens.push((start, index));
    }
    tokens
}

/// Splits a class value into runs outside `{{ ... }}` expressions.
fn static_runs(text: &str, start: usize, end: usize) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut runs = Vec::new();
    let mut run_start = start;
    let mut index = start;
    while index < end {
        if bytes.get(index) == Some(&b'{') && bytes.get(index + 1) == Some(&b'{') {
            if run_start < index {
                runs.push(run_start..index);
            }
            let Some(close) = text
                .get(index + 2..end)
                .and_then(|rest| rest.find("}}"))
                .map(|position| index + 2 + position)
            else {
                return runs;
            };
            index = close + 2;
            run_start = index;
            continue;
        }
        index += 1;
    }
    if run_start < end {
        runs.push(run_start..end);
    }
    runs
}

/// Returns the end of an attribute value starting at `value_start`.
fn attribute_value_end(
    text: &str,
    value_start: usize,
    offset: u32,
    quote: Option<u8>,
) -> Option<usize> {
    let bytes = text.as_bytes();
    if let Some(quote) = quote {
        let mut index = usize::try_from(offset).ok()?;
        if index < value_start {
            index = value_start;
        }
        while index < text.len() {
            match bytes[index] {
                b'\\' => index = index.saturating_add(2),
                b'{' if bytes.get(index + 1) == Some(&b'{') => {
                    let close = text
                        .get(index + 2..)
                        .and_then(|rest| rest.find("}}"))
                        .map(|position| index + 2 + position)?;
                    index = close + 2;
                }
                b'<' => return Some(index),
                byte if byte == quote => return Some(index),
                _ => index += 1,
            }
        }
        Some(text.len())
    } else {
        let mut index = value_start;
        while index < text.len() && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>' {
            index += 1;
        }
        Some(index)
    }
}

/// Returns whether an attribute is a local resource path by this index.
///
/// Only values that resolve to existing files become links, so a custom
/// component's `src`/`url` is as safe as an `image` tag's.
pub(crate) fn is_resource_attribute(_tag: &str, attribute: &str) -> bool {
    matches!(attribute, "src" | "poster" | "background" | "url" | "image")
}

/// Returns every literal resource value in a document.
pub(crate) fn resource_sites(text: &str) -> Vec<ResourceSite> {
    let parsed = wxml_syntax::parse(text);
    let mut sites = Vec::new();
    for element in parsed.tree.document().elements() {
        let Some(tag) = element.name() else {
            continue;
        };
        for attribute in element.attributes() {
            let Some(name) = attribute.name() else {
                continue;
            };
            if !is_resource_attribute(tag, name) {
                continue;
            }
            let Some(value) = attribute.value() else {
                continue;
            };
            if value.literal_text().is_none() {
                continue;
            }
            let Some(inner) = inner_value_range(value) else {
                continue;
            };
            let Some((range, trimmed)) = trimmed_value(text, inner) else {
                continue;
            };
            sites.push(ResourceSite {
                tag: tag.to_owned(),
                attribute: name.to_owned(),
                value: range,
                text: trimmed,
            });
        }
    }
    sites
}

/// Finds the literal resource value at `offset`.
pub(crate) fn resource_site_at(text: &str, offset: u32) -> Option<ResourceSite> {
    resource_sites(text)
        .into_iter()
        .find(|site| offset >= site.value.start() && offset <= site.value.end())
}

/// Returns the inner value range for tests and range checks.
#[cfg(test)]
pub(crate) fn value_range_bounds(value: AttributeValue<'_>) -> Option<(u32, u32)> {
    inner_value_range(value).map(|range| (range.start(), range.end()))
}

#[cfg(test)]
mod tests {
    use wxml_text::Position;

    use super::*;
    use crate::completion::context::detect;

    fn completion(text: &str, byte_offset: usize) -> Option<ClassCompletionSite> {
        let offset = u32::try_from(byte_offset).expect("offset");
        let detected = detect(text, offset)?;
        class_completion_site(text, offset, &detected)
    }

    #[test]
    fn class_hover_uses_the_token_not_the_whole_value() {
        let text = "<view class=\"card active\"></view>";
        let offset = u32::try_from(text.find("act").expect("active") + 1).expect("offset");
        let site = class_site_at(text, offset).expect("class site");
        assert_eq!(site.name, "active");
        assert_eq!(
            &text[site.token.start() as usize..site.token.end() as usize],
            "active"
        );

        let site = class_site_at(
            text,
            u32::try_from(text.find("card").expect("card") + 4).unwrap(),
        )
        .expect("end of token");
        assert_eq!(site.name, "card");
    }

    #[test]
    fn class_hover_handles_static_parts_next_to_mustaches() {
        let text = "<view class=\"card {{extra}}\"></view>";
        let offset = u32::try_from(text.find("card").expect("card") + 1).expect("offset");
        let site = class_site_at(text, offset).expect("static class");
        assert_eq!(site.name, "card");

        let offset = u32::try_from(text.find("extra").expect("extra") + 1).expect("offset");
        assert!(class_site_at(text, offset).is_none());
    }

    #[test]
    fn class_completion_replaces_only_the_current_token() {
        let text = "<view class=\"card act\">";
        let offset = text.find("act").expect("act") + 3;
        let site = completion(text, offset).expect("completion");
        assert_eq!(site.prefix, "act");
        assert!(site.quoted);
        assert!(site.existing.contains("card"));
        assert_eq!(
            &text[site.token.start() as usize..site.token.end() as usize],
            "act"
        );

        let text = "<view class=\"card act extra\">";
        let offset = text.find("act").expect("act") + 3;
        let site = completion(text, offset).expect("completion");
        assert!(site.existing.contains("card"));
        assert!(site.existing.contains("extra"));
    }

    #[test]
    fn class_completion_handles_static_fragments_around_mustaches() {
        let text = "<view class=\"card {{extra}} ba\">";
        let offset = text.find("ba\"").expect("ba") + 2;
        let site = completion(text, offset).expect("completion");
        assert_eq!(site.prefix, "ba");
        assert!(site.existing.contains("card"));
        assert!(!site.existing.contains("ba"));
        assert_eq!(
            &text[site.value.start() as usize..site.value.end() as usize],
            "card {{extra}} ba"
        );
    }

    #[test]
    fn class_completion_does_not_offer_inside_a_mustache() {
        let text = "<view class=\"card {{extra}}\">";
        let offset = text.find("extra").expect("extra") + 2;
        assert!(completion(text, offset).is_none());
    }

    #[test]
    fn resource_sites_are_literal_and_trimmed() {
        let text = "<image src=\" images/logo.png \" /><video poster='p.png' src='v.mp4'/><view url=\"a\"/>";
        let sites = resource_sites(text);
        assert_eq!(sites.len(), 4);
        assert_eq!(sites[0].text, "images/logo.png");
        assert_eq!(
            &text[sites[0].value.start() as usize..sites[0].value.end() as usize],
            "images/logo.png"
        );
        assert_eq!(sites[1].text, "p.png");
        assert_eq!(sites[2].text, "v.mp4");
        assert_eq!(sites[3].text, "a");
    }

    #[test]
    fn dynamic_resource_values_are_not_sites() {
        let text = "<image src=\"{{url}}\"/><image src=\"a/{{name}}.png\"/>";
        assert_eq!(resource_sites(text), Vec::new());
    }

    #[test]
    fn class_tokens_use_utf16_ranges_through_utf8_bytes() {
        let text = "<view class=\"中文 card\"></view>";
        let offset = text.find("card").expect("card") + 1;
        let site = completion(text, offset).expect("completion");
        assert_eq!(site.prefix, "c");
        let start = text.find("card").expect("card");
        assert_eq!(site.token.start() as usize, start);
        // The LSP layer converts these byte offsets through the document's
        // line index; verify the UTF-16 positions of the token.
        let index = wxml_text::LineIndex::new(text);
        let expected = u32::try_from(text[..start].encode_utf16().count()).expect("column");
        assert_eq!(
            index.position(site.token.start() as usize),
            Some(Position::new(0, expected))
        );
    }

    #[test]
    fn quoted_and_unquoted_value_bounds_are_exact() {
        let text = "<image src=\"a.png\"/>";
        let parsed = wxml_syntax::parse(text);
        let value = parsed
            .tree
            .document()
            .elements()
            .next()
            .expect("element")
            .attribute("src")
            .expect("src")
            .value()
            .expect("value");
        assert_eq!(value_range_bounds(value), Some((12, 17)));
        assert_eq!(&text[12..17], "a.png", "range excludes the quotes");
    }

    #[test]
    fn comments_and_other_attributes_are_not_resource_sites() {
        let text = "<view class=\"a.png\" title=\"b.png\"><!-- <image src=\"c.png\"/> --></view>";
        assert_eq!(resource_sites(text), Vec::new());
    }
}
