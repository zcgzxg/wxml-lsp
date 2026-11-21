//! Tolerant completion-context detection.
//!
//! The WXML parser is error tolerant, but completion is requested while the
//! document is often incomplete (`<vi`, `{{ user.`, an unclosed quote), so this
//! scanner works on the raw text instead of requiring a syntax tree. It tracks
//! just enough state to classify the cursor and never panics on invalid UTF-8
//! boundaries: offsets are validated by the workspace before they get here.

use wxml_text::TextRange;

/// What the cursor is inside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Site {
    /// Inside a tag name: `<vi` (`closing == false`) or `</vi`.
    Tag {
        /// True after `</`; only opened elements are suggested.
        closing: bool,
        /// Name typed so far.
        name: String,
    },
    /// Inside an attribute name: `<view cla`.
    AttributeName {
        /// Enclosing tag name.
        tag: String,
        /// Name typed so far.
        name: String,
    },
    /// Inside an attribute value.
    AttributeValue {
        /// Enclosing tag name.
        tag: String,
        /// Attribute name.
        attribute: String,
        /// True when the cursor is between quotes.
        quoted: bool,
    },
    /// Inside a `{{ ... }}` expression.
    Mustache,
}

/// One attribute occurrence inside the current start tag.
///
/// Completion uses the name and range to avoid suggesting a unique attribute
/// that is already present, while still allowing the attribute under the
/// cursor to be completed again. `value` keeps the raw literal text (including
/// any quotes-free content) so conditional rules can read a controlling
/// attribute such as `picker mode`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttributeOccurrence {
    /// Attribute name.
    pub(crate) name: String,
    /// Raw value text, or `None` when the attribute has no `=`.
    pub(crate) value: Option<String>,
    /// Byte range of the attribute name.
    pub(crate) range: TextRange,
}

/// One detected completion site plus the exact text it replaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Context {
    /// Cursor classification.
    pub(crate) site: Site,
    /// Byte range replaced by an inserted item.
    pub(crate) replace: TextRange,
    /// Identifier/value prefix before the cursor.
    pub(crate) prefix: String,
    /// Open element names, outermost first.
    pub(crate) open_tags: Vec<String>,
    /// Attributes of the current start tag, in source order.
    pub(crate) attributes: Vec<AttributeOccurrence>,
}

/// Characters allowed in a tag name.
/// Bytes that may begin a tag name. Anything else after `<` is prose.
const fn is_tag_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':' | b'.' | b'-')
}

/// Characters allowed in a tag name.
pub(super) const fn is_tag_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

/// Characters allowed in an attribute name.
pub(super) const fn is_attribute_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.' | b'@' | b'*')
}

/// Characters allowed in a mustache identifier.
const fn is_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

/// Scans backwards while `accept` matches and returns the start offset.
fn token_start(bytes: &[u8], end: usize, accept: fn(u8) -> bool) -> usize {
    let mut start = end;
    while start > 0 && accept(bytes[start - 1]) {
        start -= 1;
    }
    start
}

/// Returns the byte slice as text when it is a valid boundary.
fn text_slice(text: &str, start: usize, end: usize) -> Option<&str> {
    text.get(start..end)
}

/// Skips a quoted TypeScript-like string inside a mustache.
///
/// `None` means the string never closes before `limit`.
#[allow(clippy::missing_const_for_fn)]
fn skip_string(bytes: &[u8], mut index: usize, quote: u8, limit: usize) -> Option<usize> {
    index += 1;
    while index < limit {
        match bytes[index] {
            b'\\' => index = index.saturating_add(2),
            byte if byte == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Skips a mustache starting at `index` (which points at `{`).
fn skip_mustache(bytes: &[u8], mut index: usize, offset: usize) -> Option<usize> {
    index += 2;
    while index < offset {
        match bytes[index] {
            b'\'' | b'"' => {
                index = skip_string(bytes, index, bytes[index], offset)?;
            }
            b'}' if bytes.get(index + 1) == Some(&b'}') => return Some(index + 2),
            _ => index += 1,
        }
    }
    None
}

/// Detects the completion site at `offset`.
///
/// `offset` must be a UTF-8 boundary of `text`; callers obtain it from
/// `Document::offset_at`, which guarantees that.
pub(crate) fn detect(text: &str, offset: u32) -> Option<Context> {
    let offset = usize::try_from(offset).ok()?;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut open_tags: Vec<String> = Vec::new();
    let mut index = 0_usize;

    // Text mode.
    loop {
        if index >= offset {
            return None;
        }
        if bytes[index] == b'<' {
            if bytes.get(index..index + 4) == Some(b"<!--") {
                match text[index + 4..].find("-->") {
                    Some(end) if index + 4 + end + 3 <= offset => {
                        index = index + 4 + end + 3;
                        continue;
                    }
                    Some(_) | None => return None,
                }
            }
            if bytes.get(index + 1) == Some(&b'/') {
                match scan_tag(text, bytes, index + 2, offset, true, index, &mut open_tags) {
                    TagScan::Found(context) => return Some(context),
                    TagScan::Continue(next) => {
                        index = next;
                        continue;
                    }
                }
            }
            if bytes.get(index + 1) == Some(&b'!') {
                // Doctype or declaration: skip to `>`.
                match text[index..].find('>') {
                    Some(end) if index + end < offset => {
                        index += end + 1;
                        continue;
                    }
                    _ => return None,
                }
            }
            if bytes.get(index + 1) == Some(&b'{') {
                index += 1;
                continue;
            }
            // `<` in prose (`a < b`) is text, not a tag. Only a name-start byte
            // opens a tag; a cursor directly after `<` still gets tag items.
            if index + 1 < offset {
                match bytes.get(index + 1).copied() {
                    Some(next) if !is_tag_start(next) => {
                        index += 1;
                        continue;
                    }
                    Some(_) => {}
                    None => return None,
                }
            }
            match scan_tag(text, bytes, index + 1, offset, false, index, &mut open_tags) {
                TagScan::Found(context) => return Some(context),
                TagScan::Continue(next) => {
                    index = next;
                    continue;
                }
            }
        }
        if bytes.get(index..index + 2) == Some(b"{{") {
            if let Some(site) = scan_mustache(text, bytes, index, offset) {
                return Some(site);
            }
            // A closed mustache: continue after it.
            if let Some(next) = skip_mustache(bytes, index, offset) {
                index = next;
                continue;
            }
            return None;
        }
        // WXS element bodies are JavaScript, not WXML.
        if matches!(open_tags.last().map(String::as_str), Some("wxs")) {
            if let Some(end) = text[index..].find("</wxs")
                && index + end < offset
            {
                index += end;
                continue;
            }
            return None;
        }
        index += 1;
    }
}

/// Result of scanning a tag: either a completion site or where to continue.
enum TagScan {
    Found(Context),
    Continue(usize),
}

/// Builds a `Found` result, or continues the scan when a range is not
/// representable (only possible above 4 GiB or outside `u32`).
fn found(
    site: Site,
    start: usize,
    offset: usize,
    prefix: String,
    open_tags: &[String],
    attributes: &[AttributeOccurrence],
) -> TagScan {
    let (Some(start), Some(end)) = (u32::try_from(start).ok(), u32::try_from(offset).ok()) else {
        return TagScan::Continue(offset);
    };
    let Ok(replace) = TextRange::new(start.min(end), end) else {
        return TagScan::Continue(offset);
    };
    TagScan::Found(Context {
        site,
        replace,
        prefix,
        open_tags: open_tags.to_vec(),
        attributes: attributes.to_vec(),
    })
}

/// Scans a start or end tag starting at the first name byte.
#[allow(clippy::too_many_lines)]
fn scan_tag(
    text: &str,
    bytes: &[u8],
    mut index: usize,
    offset: usize,
    closing: bool,
    tag_start: usize,
    open_tags: &mut Vec<String>,
) -> TagScan {
    if closing {
        while index < offset && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
    }
    let attributes = if closing {
        Vec::new()
    } else {
        scan_tag_attributes(text, tag_start)
    };
    let name_start = index;
    while index < offset && is_tag_name(bytes[index]) {
        index += 1;
    }
    let name = text_slice(text, name_start, index)
        .unwrap_or_default()
        .to_owned();

    if index >= offset {
        let start = name_start;
        return found(
            Site::Tag {
                closing,
                name: name.clone(),
            },
            start,
            offset,
            name,
            open_tags,
            &attributes,
        );
    }

    if closing {
        // `</name ... >`: consume to `>` and pop the matching element.
        if let Some(end) = text[index..offset].find('>') {
            if let Some(position) = open_tags.iter().rposition(|tag| tag == &name) {
                open_tags.truncate(position);
            }
            // There is no completion after a completed closing tag.
            return TagScan::Continue(index + end + 1);
        }
        // LSP requires the edit range to contain the cursor. This branch is
        // only reached when the cursor is past the name (trailing whitespace
        // or junk), so a name-only edit would end before the cursor. Stay
        // silent instead; typing inside the name is handled above.
        return TagScan::Continue(index);
    }

    // Attributes.
    loop {
        while index < offset && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= offset {
            // `<view `: attribute-name completion with an empty prefix.
            return found(
                Site::AttributeName {
                    tag: name,
                    name: String::new(),
                },
                offset,
                offset,
                String::new(),
                open_tags,
                &attributes,
            );
        }
        if bytes[index] == b'>' {
            if !name.is_empty() {
                open_tags.push(name);
            }
            return TagScan::Continue(index + 1);
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'>') {
            return TagScan::Continue(index + 2);
        }
        if bytes[index] == b'/' {
            index += 1;
            continue;
        }
        // Attribute name.
        let attr_start = index;
        while index < offset && is_attribute_name(bytes[index]) {
            index += 1;
        }
        let attribute = text_slice(text, attr_start, index)
            .unwrap_or_default()
            .to_owned();
        if attribute.is_empty() {
            // Unknown byte inside a start tag (most often a new `<` after an
            // unterminated tag). Stop this tag so the text scanner can rescan
            // from here; this also guarantees the attribute loop progresses.
            return TagScan::Continue(index);
        }
        if index >= offset {
            let start = attr_start;
            return found(
                Site::AttributeName {
                    tag: name,
                    name: attribute.clone(),
                },
                start,
                offset,
                attribute,
                open_tags,
                &attributes,
            );
        }
        while index < offset && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= offset || bytes[index] != b'=' {
            // The attribute has no value yet; continue with the next attribute.
            continue;
        }
        index += 1;
        while index < offset && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= offset {
            return found(
                Site::AttributeValue {
                    tag: name,
                    attribute,
                    quoted: false,
                },
                offset,
                offset,
                String::new(),
                open_tags,
                &attributes,
            );
        }
        let quote = bytes[index];
        if quote == b'"' || quote == b'\'' {
            let value_start = index + 1;
            index = value_start;
            let mut closed = false;
            while index < offset {
                if bytes[index] == quote {
                    index += 1;
                    closed = true;
                    break;
                }
                if bytes.get(index..index + 2) == Some(b"{{") {
                    if let Some(site) = scan_mustache(text, bytes, index, offset) {
                        return TagScan::Found(site);
                    }
                    if let Some(next) = skip_mustache(bytes, index, offset) {
                        index = next;
                        continue;
                    }
                    return TagScan::Continue(offset);
                }
                index += 1;
            }
            if closed {
                // The value ended before the cursor; keep scanning attributes.
                if index >= offset {
                    return TagScan::Continue(index);
                }
                continue;
            }
            let start = value_start;
            let prefix = text_slice(text, start, offset)
                .unwrap_or_default()
                .to_owned();
            return found(
                Site::AttributeValue {
                    tag: name,
                    attribute,
                    quoted: true,
                },
                start,
                offset,
                prefix,
                open_tags,
                &attributes,
            );
        }
        // Unquoted value. A mustache is legal here (`wx:if={{user.id}}`) and
        // must route to expression completion like a quoted one.
        let value_start = index;
        while index < offset && !bytes[index].is_ascii_whitespace() && bytes[index] != b'>' {
            if bytes.get(index..index + 2) == Some(b"{{") {
                if let Some(site) = scan_mustache(text, bytes, index, offset) {
                    return TagScan::Found(site);
                }
                if let Some(next) = skip_mustache(bytes, index, offset) {
                    index = next;
                    continue;
                }
                return TagScan::Continue(offset);
            }
            index += 1;
        }
        let prefix = text_slice(text, value_start, index)
            .unwrap_or_default()
            .to_owned();
        if index >= offset {
            // The cursor sits inside the token: replace the whole unquoted
            // token (quotes are added by the insert), otherwise the tail would
            // survive as a stray attribute (`type=pri|mary` -> `"primary"mary`).
            let mut token_end = index;
            while token_end < bytes.len()
                && !bytes[token_end].is_ascii_whitespace()
                && bytes[token_end] != b'>'
            {
                token_end += 1;
            }
            return found(
                Site::AttributeValue {
                    tag: name,
                    attribute,
                    quoted: false,
                },
                value_start,
                token_end,
                prefix,
                open_tags,
                &attributes,
            );
        }
        // Continue with the next attribute.
    }
}

/// Scans attributes of one start tag without requiring a syntax tree.
///
/// The scanner is deliberately tolerant: an unterminated tag ends at the next
/// `<`, and unparsable bytes are skipped one at a time. It never loops without
/// advancing.
fn scan_tag_attributes(text: &str, tag_start: usize) -> Vec<AttributeOccurrence> {
    let bytes = text.as_bytes();
    let mut index = tag_start.saturating_add(1);
    while index < bytes.len() && is_tag_name(bytes[index]) {
        index += 1;
    }
    let mut attributes = Vec::new();
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        match bytes[index] {
            b'>' | b'<' => break,
            b'/' if bytes.get(index + 1) == Some(&b'>') => break,
            _ => {}
        }
        let name_start = index;
        while index < bytes.len() && is_attribute_name(bytes[index]) {
            index += 1;
        }
        if index == name_start {
            index += 1;
            continue;
        }
        let name_end = index;
        let Some(name) = text.get(name_start..name_end).map(str::to_owned) else {
            index = name_end;
            continue;
        };
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
                    while index < bytes.len() {
                        if bytes[index] == quote {
                            break;
                        }
                        if bytes.get(index..index + 2) == Some(b"{{") {
                            if let Some(next) = skip_mustache(bytes, index, bytes.len()) {
                                index = next;
                                continue;
                            }
                            index = bytes.len();
                            break;
                        }
                        index += 1;
                    }
                    value = text.get(value_start..index).map(str::to_owned);
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
                            if let Some(next) = skip_mustache(bytes, index, bytes.len()) {
                                index = next;
                                continue;
                            }
                            index = bytes.len();
                            break;
                        }
                        index += 1;
                    }
                    value = text.get(value_start..index).map(str::to_owned);
                }
            }
        } else {
            index = after_name;
        }
        if let Some(range) = text_range(name_start, name_end) {
            attributes.push(AttributeOccurrence { name, value, range });
        }
    }
    attributes
}

/// Converts validated byte offsets into a range.
fn text_range(start: usize, end: usize) -> Option<TextRange> {
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(end).ok()?;
    TextRange::new(start, end).ok()
}

/// Scans a mustache beginning at `index`; returns a site only when the cursor
/// is inside it.
fn scan_mustache(text: &str, bytes: &[u8], index: usize, offset: usize) -> Option<Context> {
    let expr_start = index + 2;
    let mut cursor = expr_start;
    while cursor < offset {
        match bytes[cursor] {
            b'\'' | b'"' => {
                let quote = bytes[cursor];
                // An unterminated literal contains the cursor.
                cursor = skip_string(bytes, cursor, quote, offset)?;
            }
            b'}' if bytes.get(cursor + 1) == Some(&b'}') => return None,
            _ => cursor += 1,
        }
    }
    let start = token_start(bytes, offset, is_identifier);
    let prefix = text_slice(text, start, offset)?.to_owned();
    let start = u32::try_from(start).ok()?;
    let end = u32::try_from(offset).ok()?;
    Some(Context {
        site: Site::Mustache,
        replace: TextRange::new(start.min(end), end).ok()?,
        prefix,
        open_tags: Vec::new(),
        attributes: Vec::new(),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn detect_at(text: &str, offset: usize) -> Option<Context> {
        detect(text, u32::try_from(offset).expect("offset"))
    }

    #[test]
    fn detects_tag_prefix_and_closing_tags() {
        let text = "<vi";
        let context = detect_at(text, 3).expect("tag");
        assert_eq!(
            context.site,
            Site::Tag {
                closing: false,
                name: "vi".to_owned()
            }
        );
        assert_eq!(context.replace.len(), 2);

        let text = "<view></vi";
        let context = detect_at(text, 10).expect("closing");
        assert_eq!(
            context.site,
            Site::Tag {
                closing: true,
                name: "vi".to_owned()
            }
        );
        assert_eq!(context.open_tags, vec!["view".to_owned()]);
    }

    #[test]
    fn detects_attribute_name_and_value() {
        let text = "<view cla";
        let context = detect_at(text, 9).expect("attr");
        assert_eq!(
            context.site,
            Site::AttributeName {
                tag: "view".to_owned(),
                name: "cla".to_owned()
            }
        );
        assert_eq!(context.replace.start(), 6);

        let text = "<view class=\"a b";
        let context = detect_at(text, 16).expect("value");
        assert_eq!(
            context.site,
            Site::AttributeValue {
                tag: "view".to_owned(),
                attribute: "class".to_owned(),
                quoted: true,
            }
        );
        assert_eq!(context.replace.start(), 13);
        assert_eq!(context.prefix, "a b");
    }

    #[test]
    fn collects_current_tag_attributes_for_duplicate_filtering() {
        let text = "<picker mode=\"date\" range=\"a\" start=\"{{ value }}\" va";
        let context = detect_at(text, text.len()).expect("attribute site");
        assert!(matches!(
            context.site,
            Site::AttributeName { ref tag, ref name } if tag == "picker" && name == "va"
        ));
        assert_eq!(context.attributes.len(), 4);
        assert_eq!(context.attributes[0].name, "mode");
        assert_eq!(context.attributes[0].value.as_deref(), Some("date"));
        assert_eq!(context.attributes[1].name, "range");
        assert_eq!(context.attributes[2].value.as_deref(), Some("{{ value }}"));
        assert_eq!(context.attributes[3].name, "va");
        assert!(context.attributes[3].range.start() as usize <= text.len() - "va".len());
    }

    #[test]
    fn detects_mustache_members_and_ignores_strings() {
        let text = "<view>{{ user.na }}</view>";
        let context = detect_at(text, 16).expect("member");
        assert_eq!(context.site, Site::Mustache);
        assert_eq!(context.prefix, "na");
        assert_eq!(context.replace.start(), 14);

        let text = "<view>{{ user['na";
        assert!(detect_at(text, text.len()).is_none());
    }

    #[test]
    fn ignores_text_comments_and_wxs_bodies() {
        assert!(detect_at("<view>plain text", 17).is_none());
        assert!(detect_at("<view><!-- com", 14).is_none());
        assert!(detect_at("<wxs module=\"m\">var x = ", 24).is_none());
        assert!(detect_at("<wxs module=\"m\">var x</wxs>", 29).is_none());
    }

    #[test]
    fn handles_unicode_before_the_cursor() {
        let text = "<view>{{ 用户.na";
        let context = detect_at(text, text.len()).expect("member");
        assert_eq!(context.prefix, "na");
        assert_eq!(context.replace.start() as usize, text.len() - 2);
    }

    #[test]
    fn unterminated_start_tag_does_not_hang_on_the_next_tag() {
        let text = "<input type=\"\"\n<view bind:tap=\"\"";
        let offset = u32::try_from(text.find("bind:t").expect("attribute") + "bind:t".len())
            .expect("offset");
        let context = detect(text, offset).expect("second tag attribute context");
        assert!(matches!(
            context.site,
            Site::AttributeName { ref tag, ref name }
                if tag == "view" && name == "bind:t"
        ));
        assert_eq!(context.prefix, "bind:t");
    }

    #[test]
    fn poison_bytes_terminate_without_a_site() {
        let inputs = [
            "<view \"",
            "<view <",
            "<text>a<b</text>",
            "<text>折扣 5<10 元</text>",
            "<text>{{a}} < {{b}}</text>",
        ];
        for text in inputs {
            let offset = u32::try_from(text.len()).expect("offset");
            // Run on a worker so a regression fails the test instead of
            // blocking the test harness forever.
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = detect(text, offset);
                let _ = sender.send(());
            });
            receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("scanner did not terminate for {text:?}"));
        }
    }

    #[test]
    fn incomplete_closing_tag_only_completes_inside_the_name() {
        let text = "</view cla";
        let offset = u32::try_from(text.len()).expect("offset");
        // The cursor is past the scanned name, so no edit may be offered:
        // an LSP textEdit range must contain the cursor.
        assert!(detect(text, offset).is_none());

        let text = "</vi";
        let offset = u32::try_from(text.len()).expect("offset");
        let context = detect(text, offset).expect("closing tag");
        assert!(matches!(
            context.site,
            Site::Tag {
                closing: true,
                ref name,
            } if name == "vi"
        ));
        assert_eq!(context.replace.start(), 2);
        assert_eq!(context.replace.end(), offset);
        assert_eq!(context.prefix, "vi");
    }

    #[test]
    fn unquoted_attribute_mustache_is_a_member_site() {
        let text = "<view wx:if={{user.na";
        let offset = u32::try_from(text.len()).expect("offset");
        let context = detect(text, offset).expect("unquoted mustache");
        assert!(matches!(context.site, Site::Mustache));
        // The replacement range is the trailing identifier, not the whole
        // expression: accepting `name` must not delete `user.`.
        assert_eq!(context.prefix, "na");
        assert_eq!(context.replace.end(), offset);
    }

    /// Deterministic pseudo-random sequence (no new test dependency).
    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    #[test]
    fn random_legal_prefixes_keep_exact_replacement_ranges() {
        let mut state = 0x5eed_u64;
        let unicode = ["中", "🙂", "é", "ß"];
        let mut per_shape = [0_usize; 5];
        for _ in 0..500 {
            let noise_len = usize::try_from(lcg(&mut state) % 3).expect("len");
            let noise: String = (0..noise_len)
                .map(|_| unicode[usize::try_from(lcg(&mut state) % 4).expect("index")])
                .collect();
            let suffix =
                ["", "x", "wx:", "my-card"][usize::try_from(lcg(&mut state) % 4).expect("index")];
            let forms = [
                format!("{noise}<vi{suffix}"),
                format!("{noise}<view cl{suffix}"),
                format!("{noise}<view class=\"{suffix}"),
                format!("{noise}<view>{{{{ user.{suffix}"),
                format!("{noise}<view bindtap=\"on{suffix}"),
            ];
            let shape = usize::try_from(lcg(&mut state) % 5).expect("index");
            let text = forms[shape].clone();
            let offset = text.len();
            let Some(context) = detect_at(&text, offset) else {
                continue;
            };
            per_shape[shape] += 1;
            assert!(context.replace.start() <= context.replace.end());
            let start = context.replace.start() as usize;
            let end = context.replace.end() as usize;
            assert!(end >= offset, "range must include the cursor: {text:?}");
            assert!(
                text.is_char_boundary(start) && text.is_char_boundary(end),
                "range splits a code point: {text:?}"
            );
            assert_eq!(
                &text[start..offset],
                context.prefix,
                "prefix is not the typed text: {text:?}"
            );
            assert!(!context.prefix.contains('<'));
            assert!(!context.prefix.contains('>'));
        }
        for (shape, count) in per_shape.iter().enumerate() {
            assert!(*count >= 50, "shape {shape} had too few contexts: {count}");
        }
    }

    #[test]
    fn less_than_in_prose_is_not_a_tag() {
        for text in ["<view>a < b", "<view>1 < 2", "<text>折扣 5<10 元"] {
            assert!(detect_at(text, text.len()).is_none(), "{text:?}");
        }
        let context = detect_at("<", 1).expect("lone '<' is a tag site");
        assert!(matches!(
            context.site,
            Site::Tag {
                closing: false,
                ref name
            } if name.is_empty()
        ));
    }

    #[test]
    fn unquoted_value_replaces_the_whole_token() {
        let text = "<button type=primary";
        let context = detect_at(text, 16).expect("value site");
        assert_eq!(context.replace.start(), 13);
        assert_eq!(context.replace.end(), 20);
        assert_eq!(context.prefix, "pri");
    }

    #[test]
    fn closing_tag_skips_whitespace_before_the_name() {
        let text = "</ view";
        let context = detect_at(text, text.len()).expect("closing site");
        assert!(matches!(
            context.site,
            Site::Tag {
                closing: true,
                ref name
            } if name == "view"
        ));
        assert_eq!(context.replace.start(), 3);
        assert_eq!(context.replace.end(), 7);
    }

    #[test]
    fn tolerates_incomplete_tags_and_attributes() {
        assert!(matches!(
            detect_at("<", 1).map(|context| context.site),
            Some(Site::Tag { closing: false, .. })
        ));
        assert!(matches!(
            detect_at("</", 2).map(|context| context.site),
            Some(Site::Tag { closing: true, .. })
        ));
        assert!(matches!(
            detect_at("<view ", 6).map(|context| context.site),
            Some(Site::AttributeName { .. })
        ));
    }
}
