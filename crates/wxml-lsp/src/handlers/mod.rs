//! Request handlers and their shared parameter parsing.
//!
//! Parameters are read from `serde_json::Value` because the crate does not
//! depend on `lsp-types`. Every accessor is fallible: malformed parameters
//! become `InvalidParams` responses or logged protocol errors, never panics.

pub(crate) mod diagnostics;
pub(crate) mod hover;
pub(crate) mod lifecycle;
pub(crate) mod mode_diagnostics;
pub(crate) mod styles;

use serde_json::{Map, Value};
use wxml_text::{Position, SourceRange};
use wxml_types::{CapabilityStatus, DiagnosticSeverity, TypeQuality};

use crate::workspace::{ContentChange, Document};

/// Returns `true` when `uri` names a WXML template for this server.
///
/// A project host answers exactly; without one (synthetic documents) the URI
/// extension is the only signal available. Non-template documents must never
/// schedule analysis jobs or publish diagnostics.
pub(crate) fn template_document(server: &crate::server::Server, uri: &str) -> bool {
    match server.host.route(uri) {
        crate::project_host::DocumentRoute::Wxml(_) => true,
        crate::project_host::DocumentRoute::Script(_)
        | crate::project_host::DocumentRoute::Json(_)
        | crate::project_host::DocumentRoute::Style(_) => false,
        crate::project_host::DocumentRoute::Unknown => {
            server.synthetic_templates.contains(uri)
                || uri
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(uri)
                    .to_ascii_lowercase()
                    .ends_with(".wxml")
        }
    }
}

/// Largest accepted number of edits in one `didChange`.
const MAX_CONTENT_CHANGES: usize = 256;

/// Longest backend-supplied reason shown to the client.
const MAX_DISPLAY_REASON_CHARS: usize = 200;

/// Longest backend-supplied diagnostic message shown to the client.
const MAX_DISPLAY_MESSAGE_CHARS: usize = 512;

/// Longest method name written to stderr, in bytes.
const MAX_LOG_METHOD_BYTES: usize = 64;

/// Reads a string field from an object.
pub(crate) fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

/// Reads the `textDocument` object from request parameters.
pub(crate) fn text_document(params: &Value) -> Option<&Map<String, Value>> {
    params.as_object()?.get("textDocument")?.as_object()
}

/// Reads the document URI and UTF-16 position of a point request.
pub(crate) fn hover_target(params: &Value) -> Option<(&str, Position)> {
    let document = text_document(params)?;
    let uri = string(document, "uri")?;
    let position = position(params.as_object()?.get("position")?)?;
    Some((uri, position))
}

/// Reads the document URI and a UTF-16 selection range.
pub(crate) fn range_target(params: &Value) -> Option<(&str, (Position, Position))> {
    let document = text_document(params)?;
    let uri = string(document, "uri")?;
    let range = params.as_object()?.get("range")?.as_object()?;
    let start = position(range.get("start")?)?;
    let end = position(range.get("end")?)?;
    Some((uri, (start, end)))
}

/// Reads a zero-based UTF-16 position.
pub(crate) fn position(value: &Value) -> Option<Position> {
    let object = value.as_object()?;
    let line = u32::try_from(object.get("line")?.as_u64()?).ok()?;
    let character = u32::try_from(object.get("character")?.as_u64()?).ok()?;
    Some(Position::new(line, character))
}

/// Serializes a position back to the wire shape.
pub(crate) fn position_value(position: Position) -> Value {
    serde_json::json!({"line": position.line, "character": position.character})
}

/// Converts a validated source range into an LSP range using the document's
/// own line index. Returns `None` instead of clamping invalid bounds.
pub(crate) fn source_range_value(document: &Document, range: SourceRange) -> Option<Value> {
    line_index_range_value(document.snapshot().line_index(), range)
}

/// Converts a byte range in any indexed text into an LSP range.
pub(crate) fn line_index_range_value(
    line_index: &wxml_text::LineIndex,
    range: SourceRange,
) -> Option<Value> {
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    let start = line_index.position(start)?;
    let end = line_index.position(end)?;
    Some(serde_json::json!({
        "start": position_value(start),
        "end": position_value(end),
    }))
}

/// Parses the `contentChanges` array of a `didChange` notification.
pub(crate) fn content_changes(params: &Value) -> Option<Vec<ContentChange>> {
    let changes = params.as_object()?.get("contentChanges")?.as_array()?;
    if changes.len() > MAX_CONTENT_CHANGES {
        return None;
    }
    let mut parsed = Vec::with_capacity(changes.len());
    for change in changes {
        let change = change.as_object()?;
        let text = string(change, "text")?.to_owned();
        match change.get("range") {
            None | Some(Value::Null) => parsed.push(ContentChange::Full(text)),
            Some(range) => {
                let range = range.as_object()?;
                let start = position(range.get("start")?)?;
                let end = position(range.get("end")?)?;
                parsed.push(ContentChange::Incremental { start, end, text });
            }
        }
    }
    Some(parsed)
}

/// Removes control characters and truncates a backend reason before it is
/// shown to the client.
///
/// The original text, if changed, is written to stderr in escaped form so a
/// reviewer can still diagnose the backend without letting terminal escapes or
/// unbounded text reach the editor.
pub(crate) fn sanitized_reason(reason: &str) -> String {
    let sanitized = sanitize_display(reason, MAX_DISPLAY_REASON_CHARS);
    if sanitized != reason {
        crate::server::log(format_args!(
            "sanitized backend reason for display: {}",
            log_safe_text(reason, MAX_DISPLAY_REASON_CHARS)
        ));
    }
    sanitized
}

/// Cleans a backend diagnostic message before publishing it.
pub(crate) fn sanitized_message(message: &str) -> String {
    let sanitized = sanitize_display(message, MAX_DISPLAY_MESSAGE_CHARS);
    if sanitized != message {
        crate::server::log(format_args!(
            "sanitized backend diagnostic message for display: {}",
            log_safe_text(message, MAX_DISPLAY_MESSAGE_CHARS)
        ));
    }
    sanitized
}

/// Replaces unsafe characters with spaces, collapses whitespace and truncates.
fn sanitize_display(text: &str, limit: usize) -> String {
    let mut sanitized = String::new();
    let mut count = 0_usize;
    let mut previous_space = true;
    for character in text.chars() {
        let character = if is_display_unsafe(character) {
            ' '
        } else {
            character
        };
        if character.is_whitespace() {
            if previous_space {
                continue;
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
        if count == limit {
            sanitized.push('…');
            break;
        }
        sanitized.push(character);
        count += 1;
    }
    while sanitized.ends_with(' ') {
        sanitized.pop();
    }
    sanitized
}

/// Returns `true` for characters that must not reach an editor or a terminal
/// unfiltered: control characters plus bidi/zero-width format characters.
fn is_display_unsafe(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{206a}'..='\u{206f}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{feff}'
        )
}

/// Sanitizes text placed inside a code fence: keeps line breaks, strips
/// control/format characters, and truncates to `limit` characters.
pub(crate) fn code_text(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|character| *character == '\n' || !is_display_unsafe(*character))
        .take(limit)
        .collect()
}

/// Escapes Markdown control characters and unsafe format characters in
/// backend-supplied documentation before it reaches a hover popup.
///
/// Links, images and HTML are disabled by escaping `[]()!<>`; headings,
/// lists, emphasis and tables are escaped as well. The text stays readable but
/// cannot inject commands or fetch remote resources.
pub(crate) fn markdown_escape(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.chars().take(4_096) {
        if is_display_unsafe(character) {
            continue;
        }
        match character {
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '(' | ')' | '#' | '+' | '-' | '|'
            | '!' => {
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

/// Escapes unsafe characters and truncates untrusted text to `limit` bytes
/// for stderr.
pub(crate) fn log_safe_text(text: &str, limit: usize) -> String {
    let mut safe = String::new();
    for character in text.chars() {
        if safe.len() >= limit {
            safe.push('…');
            break;
        }
        if character.is_control() {
            let escaped = character.escape_debug().to_string();
            if safe.len() + escaped.len() > limit {
                safe.push('…');
                break;
            }
            safe.push_str(&escaped);
        } else if is_display_unsafe(character) {
            let escaped = format!("\\u{{{:x}}}", u32::from(character));
            if safe.len() + escaped.len() > limit {
                safe.push('…');
                break;
            }
            safe.push_str(&escaped);
        } else if safe.len() + character.len_utf8() > limit {
            safe.push('…');
            break;
        } else {
            safe.push(character);
        }
    }
    safe
}

/// Makes an untrusted method name safe for a stderr line.
pub(crate) fn log_safe_method(method: &str) -> String {
    log_safe_text(method, MAX_LOG_METHOD_BYTES)
}

/// Maps the backend severity to the LSP numeric level.
pub(crate) const fn severity(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 1,
        DiagnosticSeverity::Warning => 2,
        DiagnosticSeverity::Information => 3,
        DiagnosticSeverity::Hint => 4,
    }
}

/// Stable label for a capability status.
///
/// The wildcard arm is required because [`CapabilityStatus`] is
/// `#[non_exhaustive]`; future variants must not break this crate.
#[allow(clippy::match_same_arms)]
pub(crate) const fn status_label(status: &CapabilityStatus) -> &'static str {
    match status {
        CapabilityStatus::Supported => "supported",
        CapabilityStatus::Partial { .. } => "partial",
        CapabilityStatus::Unsupported { .. } => "unsupported",
        CapabilityStatus::Disabled { .. } => "disabled",
        CapabilityStatus::Unavailable { .. } => "unavailable",
        CapabilityStatus::Unknown { .. } => "unknown",
        _ => "unknown",
    }
}

/// Stable label for a type quality.
///
/// The wildcard arm is required because [`TypeQuality`] is
/// `#[non_exhaustive]`.
#[allow(clippy::match_same_arms)]
pub(crate) const fn quality_label(quality: TypeQuality) -> &'static str {
    match quality {
        TypeQuality::Known => "known",
        TypeQuality::Any => "any",
        TypeQuality::Unknown => "unknown",
        TypeQuality::Missing => "missing",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_positions_strictly() {
        assert_eq!(
            position(&json!({"line": 1, "character": 2})),
            Some(Position::new(1, 2))
        );
        for value in [
            json!({"line": 1}),
            json!({"line": -1, "character": 0}),
            json!({"line": 1.5, "character": 0}),
            json!({"line": 0, "character": "x"}),
            json!({"line": u64::from(u32::MAX) + 1, "character": 0}),
            json!(null),
        ] {
            assert_eq!(position(&value), None, "value {value}");
        }
    }

    #[test]
    fn parses_full_and_incremental_changes() {
        let params = json!({
            "contentChanges": [
                {"text": "all"},
                {
                    "range": {
                        "start": {"line": 0, "character": 1},
                        "end": {"line": 0, "character": 2},
                    },
                    "text": "x",
                },
            ],
        });
        assert_eq!(
            content_changes(&params),
            Some(vec![
                ContentChange::Full("all".to_owned()),
                ContentChange::Incremental {
                    start: Position::new(0, 1),
                    end: Position::new(0, 2),
                    text: "x".to_owned(),
                },
            ])
        );
    }

    #[test]
    fn treats_a_null_range_as_a_full_change() {
        let params = json!({
            "contentChanges": [{"range": null, "text": "new"}],
        });
        assert_eq!(
            content_changes(&params),
            Some(vec![ContentChange::Full("new".to_owned())])
        );
    }

    #[test]
    fn sanitizes_reasons_for_display() {
        assert_eq!(sanitized_reason("plain reason"), "plain reason");
        assert_eq!(sanitized_reason("a\nb"), "a b");
        assert_eq!(sanitized_reason("a\t\tb"), "a b");
        assert_eq!(sanitized_reason("  a  b  "), "a b");
        for unsafe_character in [
            '\u{00ad}',
            '\u{061c}',
            '\u{2028}',
            '\u{2029}',
            '\u{202e}',
            '\u{2060}',
            '\u{206a}',
            '\u{1d173}',
            '\u{feff}',
        ] {
            let dirty = format!("a{unsafe_character}b");
            assert_eq!(sanitized_reason(&dirty), "a b", "{dirty:?}");
        }
        let dirty = format!("safe\u{1b}[31mred\n\u{202e}{}", "x".repeat(300));
        let clean = sanitized_reason(&dirty);
        assert!(!clean.contains('\u{1b}'), "{clean}");
        assert!(!clean.contains('\n'), "{clean}");
        assert!(!clean.contains('\u{202e}'), "{clean}");
        assert!(clean.starts_with("safe [31mred x"), "{clean}");
        assert!(clean.ends_with('…'), "{clean}");
        assert_eq!(clean.chars().count(), 201);
    }

    #[test]
    fn markdown_escape_disables_links_images_and_html() {
        let escaped = markdown_escape("x [a](command:evil) ![i](https://x) <b>");
        assert!(!escaped.contains("](command:"), "{escaped}");
        assert!(!escaped.contains("![i]("), "{escaped}");
        assert!(!escaped.contains("<b>"), "{escaped}");
        assert!(escaped.contains("\\]"), "{escaped}");
    }

    #[test]
    fn sanitizes_backend_diagnostic_messages() {
        assert_eq!(sanitized_message("a\u{1b}b"), "a b");
        let long = "m".repeat(600);
        let clean = sanitized_message(&long);
        assert_eq!(clean.chars().count(), MAX_DISPLAY_MESSAGE_CHARS + 1);
        assert!(clean.ends_with('…'), "{clean}");
    }

    #[test]
    fn shortens_and_escapes_method_names_for_logs() {
        assert_eq!(log_safe_method("textDocument/hover"), "textDocument/hover");
        assert_eq!(log_safe_method("a\nb"), "a\\nb");
        let long = "m".repeat(300);
        let safe = log_safe_method(&long);
        assert_eq!(safe.len(), MAX_LOG_METHOD_BYTES + '…'.len_utf8());
        assert!(safe.ends_with('…'), "{safe}");
        assert_eq!(log_safe_text("a\u{1b}[31mb", 64), "a\\u{1b}[31mb");
        assert_eq!(log_safe_text("a\u{202e}b", 64), "a\\u{202e}b");
    }

    #[test]
    fn rejects_malformed_change_arrays() {
        for params in [
            json!({}),
            json!({"contentChanges": {}}),
            json!({"contentChanges": [{"range": {}}]}),
            json!({"contentChanges": [{"text": 1}]}),
            json!({"contentChanges": [null]}),
        ] {
            assert_eq!(content_changes(&params), None, "params {params}");
        }
        let too_many = json!({
            "contentChanges": (0..=MAX_CONTENT_CHANGES)
                .map(|_| json!({"text": ""}))
                .collect::<Vec<_>>(),
        });
        assert_eq!(content_changes(&too_many), None);
    }

    #[test]
    fn labels_every_status_and_quality() {
        assert_eq!(status_label(&CapabilityStatus::Supported), "supported");
        assert_eq!(
            status_label(&CapabilityStatus::Unknown { reason: None }),
            "unknown"
        );
        assert_eq!(quality_label(TypeQuality::Known), "known");
        assert_eq!(quality_label(TypeQuality::Missing), "missing");
    }
}
