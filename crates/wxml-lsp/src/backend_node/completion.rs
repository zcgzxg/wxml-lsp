//! Parses protocol v2 `complete` responses into C0 completion DTOs.

use std::collections::BTreeSet;

use serde::Deserialize;
use wxml_text::SourceRange;
use wxml_types::{
    CapabilityStatus, CompletionItem, CompletionItemKind, CompletionResult, InsertTextFormat,
    MAX_COMPLETION_ITEMS,
};

use super::protocol::{PROTOCOL_VERSION, ProtocolError};

/// One completion item on the wire.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemWire {
    #[serde(default)]
    label: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    documentation: Option<String>,
    #[serde(default)]
    insert_text: String,
    #[serde(default)]
    replace: Option<RangeWire>,
    #[serde(default)]
    sort_text: Option<String>,
}

/// WXML byte range on the wire.
#[derive(Clone, Copy, Debug, Deserialize)]
struct RangeWire {
    start: u32,
    end: u32,
}

/// Capability status on the wire (`{"status":"supported"}` etc.).
#[derive(Clone, Debug, Deserialize)]
struct StateWire {
    #[serde(default)]
    status: String,
    #[serde(default)]
    reason: Option<String>,
}

/// One `complete` response.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteWire {
    #[serde(default)]
    protocol_version: u32,
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    state: Option<StateWire>,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    items: Vec<ItemWire>,
}

/// Maps a wire status to the shared capability status.
fn status(state: Option<&StateWire>) -> CapabilityStatus {
    let Some(state) = state else {
        return CapabilityStatus::Unknown { reason: None };
    };
    let reason = || state.reason.clone().unwrap_or_default();
    match state.status.as_str() {
        "supported" => CapabilityStatus::Supported,
        "partial" => CapabilityStatus::Partial { reason: reason() },
        "unsupported" => CapabilityStatus::Unsupported { reason: reason() },
        "disabled" => CapabilityStatus::Disabled { reason: reason() },
        "unavailable" => CapabilityStatus::Unavailable { reason: reason() },
        other => CapabilityStatus::Unknown {
            reason: Some(format!("unrecognized completion status `{other}`")),
        },
    }
}

/// Maps a wire item kind to the C0 kind.
fn item_kind(kind: &str) -> CompletionItemKind {
    match kind {
        "tag" => CompletionItemKind::Tag,
        "attribute" => CompletionItemKind::Attribute,
        "value" => CompletionItemKind::Value,
        "method" => CompletionItemKind::Method,
        "event" => CompletionItemKind::Event,
        "keyword" => CompletionItemKind::Keyword,
        _ => CompletionItemKind::Property,
    }
}

/// Parses and validates one `complete` result value.
///
/// Invalid ranges and empty labels are dropped; a wrong protocol version or a
/// mismatched request id rejects the whole response.
pub(crate) fn parse(
    value: serde_json::Value,
    expected_request: &str,
    text_len: u32,
) -> Result<CompletionResult, ProtocolError> {
    let wire: CompleteWire = serde_json::from_value(value).map_err(|_| ProtocolError::Json)?;
    if wire.protocol_version != u32::try_from(PROTOCOL_VERSION).unwrap_or(2) {
        return Err(ProtocolError::Version);
    }
    if wire.request_id != expected_request {
        return Err(ProtocolError::IdMismatch);
    }
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut truncated = wire.truncated;
    for item in wire.items {
        if item.label.is_empty() {
            continue;
        }
        let Some(range) = item.replace else {
            continue;
        };
        if range.end > text_len || range.start > range.end {
            continue;
        }
        let Ok(replace) = SourceRange::from_bounds(range.start, range.end) else {
            continue;
        };
        if !seen.insert(item.label.clone()) {
            continue;
        }
        if items.len() >= MAX_COMPLETION_ITEMS {
            truncated = true;
            break;
        }
        let insert_text = if item.insert_text.is_empty() {
            item.label.clone()
        } else {
            item.insert_text
        };
        items.push(CompletionItem {
            label: item.label,
            kind: item_kind(&item.kind),
            detail: item.detail,
            documentation: item.documentation,
            insert_text,
            insert_text_format: InsertTextFormat::PlainText,
            replace,
            sort_text: item.sort_text,
            filter_text: None,
        });
    }
    Ok(CompletionResult {
        items,
        status: status(wire.state.as_ref()),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn response(items: &serde_json::Value) -> serde_json::Value {
        json!({
            "protocolVersion": 2,
            "requestId": "req-1",
            "state": {"status": "supported"},
            "truncated": false,
            "items": items.clone(),
        })
    }

    #[test]
    fn maps_items_and_rejects_out_of_bounds_ranges() {
        let value = response(&json!([
            {"label": "name", "kind": "property", "insertText": "name", "replace": {"start": 10, "end": 12}},
            {"label": "bad", "kind": "property", "insertText": "bad", "replace": {"start": 900, "end": 901}},
        ]));
        let result = parse(value, "req-1", 100).expect("response");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].label, "name");
        assert_eq!(result.items[0].kind, CompletionItemKind::Property);
        assert_eq!(result.items[0].replace.start(), 10);
        assert!(result.status.is_supported());
    }

    #[test]
    fn rejects_a_wrong_request_id_or_version() {
        let mut value = response(&json!([]));
        value["requestId"] = json!("other");
        assert_eq!(parse(value, "req-1", 100), Err(ProtocolError::IdMismatch));

        let mut value = response(&json!([]));
        value["protocolVersion"] = json!(1);
        assert_eq!(parse(value, "req-1", 100), Err(ProtocolError::Version));
    }

    #[test]
    fn dedupes_labels_and_maps_degraded_status() {
        let value = json!({
            "protocolVersion": 2,
            "requestId": "req-1",
            "state": {"status": "unavailable", "reason": "node exited"},
            "truncated": false,
            "items": [
                {"label": "save", "kind": "method", "insertText": "save", "replace": {"start": 0, "end": 0}},
                {"label": "save", "kind": "method", "insertText": "save", "replace": {"start": 0, "end": 0}},
            ],
        });
        let result = parse(value, "req-1", 100).expect("response");
        assert_eq!(result.items.len(), 1);
        assert!(matches!(
            result.status,
            CapabilityStatus::Unavailable { .. }
        ));
    }
}
