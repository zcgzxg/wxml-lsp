//! NDJSON JSON-RPC framing for the Node type backend.
//!
//! The Node server accepts one JSON-RPC object per line and writes one object
//! per line. Requests always use string ids; responses must echo the id and may
//! carry a structured C1 error code in `error.data.code`.

use serde_json::{Value, json};
use thiserror::Error;

/// C1 protocol version.
///
/// Version 2 adds optional `bindings`, `guards`, `components` and `events`
/// inputs to `analyze` and reserves a `complete` method. Phase 0 only freezes
/// the shapes; the server ignores unknown optional fields.
pub const PROTOCOL_VERSION: u64 = 2;

/// Why a Node response could not be accepted.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ProtocolError {
    /// The response line was not valid JSON.
    #[error("node response is not valid JSON")]
    Json,
    /// The response was not a JSON-RPC object with a result or error.
    #[error("node response is not a JSON-RPC response")]
    Shape,
    /// The response id did not match the request id.
    #[error("node response id does not match the request")]
    IdMismatch,
    /// The Node backend returned a structured error.
    #[error("node error {code}: {message}")]
    Node {
        /// JSON-RPC error code.
        code: i64,
        /// Node message (log only).
        message: String,
        /// C1 `data.code` value, when present.
        data_code: Option<String>,
    },
    /// The ping response reported an unsupported protocol version.
    #[error("node protocol version is not supported")]
    Version,
    /// The response view revision did not match the request.
    #[error("node view revision does not match the request")]
    RevisionMismatch,
    /// The response exceeds one of the C1 entry-count limits.
    #[error("node response exceeds configured limits")]
    Limit,
    /// One NDJSON line exceeded the configured cap.
    #[error("node response line is too large")]
    LineTooLarge,
}

impl ProtocolError {
    /// Returns the C1 error code, when Node supplied one.
    #[must_use]
    pub fn data_code(&self) -> Option<&str> {
        match self {
            Self::Node { data_code, .. } => data_code.as_deref(),
            _ => None,
        }
    }
}

/// Builds one NDJSON request line.
pub fn request_line(
    id: &str,
    method: &str,
    params: Option<&Value>,
) -> Result<String, ProtocolError> {
    let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if let Some(params) = params {
        if let Some(object) = message.as_object_mut() {
            object.insert("params".to_owned(), params.clone());
        }
    }
    serde_json::to_string(&message).map_err(|_| ProtocolError::Json)
}

/// Parses one response line and checks the JSON-RPC id.
pub fn parse_response(line: &str, expected_id: &str) -> Result<Value, ProtocolError> {
    let value: Value = serde_json::from_str(line).map_err(|_| ProtocolError::Json)?;
    let Some(object) = value.as_object() else {
        return Err(ProtocolError::Shape);
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(ProtocolError::Shape);
    }
    if object.get("id").and_then(Value::as_str) != Some(expected_id) {
        return Err(ProtocolError::IdMismatch);
    }
    if let Some(error) = object.get("error") {
        let Some(error) = error.as_object() else {
            return Err(ProtocolError::Shape);
        };
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32_603);
        let message = crate::handlers::log_safe_text(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("node error"),
            512,
        );
        let data_code = error
            .get("data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Err(ProtocolError::Node {
            code,
            message,
            data_code,
        });
    }
    object.get("result").cloned().ok_or(ProtocolError::Shape)
}

/// Validates the startup ping response.
pub fn validate_ping(result: &Value) -> Result<(), ProtocolError> {
    let Some(object) = result.as_object() else {
        return Err(ProtocolError::Shape);
    };
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(ProtocolError::Shape);
    }
    if object.get("protocolVersion").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(ProtocolError::Version);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_ndjson_requests_with_string_ids() {
        let line = request_line("r1", "ping", None).expect("request");
        assert!(!line.contains('\n'));
        let value: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], "r1");
        assert_eq!(value["method"], "ping");
        assert!(value.get("params").is_none());
    }

    #[test]
    fn parses_results_and_rejects_mismatched_ids() {
        let value = parse_response(r#"{"jsonrpc":"2.0","id":"r1","result":{"ok":true}}"#, "r1")
            .expect("response");
        assert_eq!(value["ok"], true);
        assert_eq!(
            parse_response(r#"{"jsonrpc":"2.0","id":"r2","result":{}}"#, "r1"),
            Err(ProtocolError::IdMismatch)
        );
        assert_eq!(
            parse_response(r#"{"id":"r1","result":{}}"#, "r1"),
            Err(ProtocolError::Shape)
        );
        assert_eq!(parse_response("{bad", "r1"), Err(ProtocolError::Json));
    }

    #[test]
    fn exposes_structured_node_errors() {
        let error = parse_response(
            r#"{"jsonrpc":"2.0","id":"r1","error":{"code":-32602,"message":"bad","data":{"code":"bad-limit"}}}"#,
            "r1",
        )
        .expect_err("node error");
        assert_eq!(error.data_code(), Some("bad-limit"));
        assert!(matches!(error, ProtocolError::Node { code: -32602, .. }));
    }

    #[test]
    fn node_error_messages_are_bounded() {
        let long = "x".repeat(5_000);
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":"r1","error":{{"code":-32603,"message":"{long}"}}}}"#
        );
        let error = parse_response(&line, "r1").expect_err("node error");
        assert!(error.to_string().len() < 1_000, "{error}");
    }

    #[test]
    fn validates_ping_version() {
        let ok = serde_json::json!({"ok": true, "protocolVersion": 2});
        assert!(validate_ping(&ok).is_ok());
        let bad = serde_json::json!({"ok": true, "protocolVersion": 1});
        assert_eq!(validate_ping(&bad), Err(ProtocolError::Version));
    }
}
