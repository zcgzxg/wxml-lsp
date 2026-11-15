//! Minimal JSON-RPC 2.0 framing for the stdio transport.
//!
//! The protocol is hand-written on top of [`serde_json`] so the crate does not
//! depend on `lsp-types` or `lsp-server`. Everything the client sends is
//! treated as untrusted input:
//!
//! - a frame body is limited to [`MAX_MESSAGE_BYTES`];
//! - header lines and header counts are bounded;
//! - the JSON parser keeps its own recursion limit, so deeply nested values
//!   are rejected instead of overflowing the stack;
//! - malformed frames and bodies produce typed errors, never panics.

use std::fmt;
use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

/// Largest accepted JSON-RPC body (8 MiB).
pub(crate) const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// Largest accepted header line, including the line terminator.
const MAX_HEADER_BYTES: usize = 8 * 1024;

/// Largest accepted number of header lines per frame.
const MAX_HEADERS: usize = 16;

/// Largest accepted request id string. Ids are echoed and kept in the
/// in-flight map, so an unbounded id is a memory amplification vector.
const MAX_REQUEST_ID_BYTES: usize = 256;

/// JSON-RPC error codes used by the server.
pub(crate) mod error_codes {
    /// The body was not valid JSON.
    pub(crate) const PARSE_ERROR: i64 = -32_700;
    /// The JSON value was not a valid JSON-RPC message.
    pub(crate) const INVALID_REQUEST: i64 = -32_600;
    /// The method is unknown.
    pub(crate) const METHOD_NOT_FOUND: i64 = -32_601;
    /// Parameters do not match the method.
    pub(crate) const INVALID_PARAMS: i64 = -32_602;
    /// The server failed while handling the request.
    pub(crate) const INTERNAL_ERROR: i64 = -32_603;
    /// The client cancelled the request.
    pub(crate) const REQUEST_CANCELLED: i64 = -32_800;
    /// A request arrived before `initialize`.
    pub(crate) const SERVER_NOT_INITIALIZED: i64 = -32_002;
}

/// Why a frame could not be read.
#[derive(Debug)]
pub(crate) enum FrameError {
    /// The underlying stream failed.
    Io(io::Error),
    /// The stream ended in the middle of a frame.
    UnexpectedEof,
    /// A header line was not `Name: value`.
    InvalidHeader,
    /// A header line exceeded [`MAX_HEADER_BYTES`].
    HeaderLineTooLong,
    /// A frame carried too many headers.
    TooManyHeaders,
    /// No `Content-Length` header was present.
    MissingContentLength,
    /// More than one `Content-Length` header was present.
    DuplicateContentLength,
    /// `Content-Length` was not a decimal byte count.
    InvalidContentLength,
    /// The declared body is larger than [`MAX_MESSAGE_BYTES`].
    MessageTooLarge {
        /// Declared body length.
        length: usize,
        /// Accepted maximum.
        limit: usize,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "transport error: {error}"),
            Self::UnexpectedEof => formatter.write_str("unexpected end of stream inside a frame"),
            Self::InvalidHeader => formatter.write_str("malformed header line"),
            Self::HeaderLineTooLong => formatter.write_str("header line is too long"),
            Self::TooManyHeaders => formatter.write_str("too many headers"),
            Self::MissingContentLength => formatter.write_str("missing Content-Length"),
            Self::DuplicateContentLength => formatter.write_str("duplicate Content-Length"),
            Self::InvalidContentLength => formatter.write_str("invalid Content-Length"),
            Self::MessageTooLarge { length, limit } => {
                write!(
                    formatter,
                    "message of {length} bytes exceeds the {limit} byte limit"
                )
            }
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Reads one `Content-Length` framed body.
///
/// Returns `Ok(None)` at a clean end of stream. The body bytes are returned
/// unparsed so JSON errors can be answered with a protocol-level error.
pub(crate) fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, FrameError> {
    let mut line = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut header_count = 0_usize;

    loop {
        line.clear();
        if read_line_bounded(reader, &mut line, MAX_HEADER_BYTES)? == 0 {
            return if header_count == 0 && content_length.is_none() {
                Ok(None)
            } else {
                Err(FrameError::UnexpectedEof)
            };
        }
        header_count += 1;
        if header_count > MAX_HEADERS {
            return Err(FrameError::TooManyHeaders);
        }
        trim_line_ending(&mut line);
        if line.is_empty() {
            break;
        }
        let (name, value) = parse_header(&line)?;
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(FrameError::DuplicateContentLength);
            }
            content_length = Some(parse_content_length(value)?);
        }
    }

    let length = content_length.ok_or(FrameError::MissingContentLength)?;
    if length > MAX_MESSAGE_BYTES {
        return Err(FrameError::MessageTooLarge {
            length,
            limit: MAX_MESSAGE_BYTES,
        });
    }
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => FrameError::UnexpectedEof,
            _ => FrameError::Io(error),
        })?;
    Ok(Some(body))
}

/// Reads one line, enforcing a hard byte cap so a malformed peer cannot grow
/// the buffer without bound.
fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    limit: usize,
) -> Result<usize, FrameError> {
    let mut total = 0_usize;
    loop {
        let available = reader.fill_buf().map_err(FrameError::Io)?;
        if available.is_empty() {
            return Ok(total);
        }
        let newline = available.iter().position(|&byte| byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        total += take;
        if total > limit {
            return Err(FrameError::HeaderLineTooLong);
        }
        if newline.is_some() {
            return Ok(total);
        }
    }
}

/// Strips a trailing `\n` and `\r\n` from a header line.
fn trim_line_ending(line: &mut Vec<u8>) {
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
}

/// Splits a header line into a name and a value.
fn parse_header(line: &[u8]) -> Result<(&str, &str), FrameError> {
    let text = std::str::from_utf8(line).map_err(|_| FrameError::InvalidHeader)?;
    let (name, value) = text.split_once(':').ok_or(FrameError::InvalidHeader)?;
    let name = name.trim();
    if name.is_empty() {
        return Err(FrameError::InvalidHeader);
    }
    Ok((name, value.trim()))
}

/// Parses a decimal `Content-Length` value without overflowing.
fn parse_content_length(value: &str) -> Result<usize, FrameError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FrameError::InvalidContentLength);
    }
    value
        .parse::<usize>()
        .map_err(|_| FrameError::InvalidContentLength)
}

/// A parsed incoming JSON-RPC message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Incoming {
    /// A request, which must be answered with exactly one response.
    Request {
        /// The client-provided id, echoed back verbatim.
        id: Value,
        /// Method name.
        method: String,
        /// Raw parameters, when present and not `null`.
        params: Option<Value>,
    },
    /// A notification, which must not be answered.
    Notification {
        /// Method name.
        method: String,
        /// Raw parameters, when present and not `null`.
        params: Option<Value>,
    },
    /// A response. This server does not send requests, so responses are only
    /// logged by the caller.
    Response {
        /// The id of the request this answers.
        id: Value,
    },
}

/// Why a body could not be turned into an [`Incoming`] message.
#[derive(Debug)]
pub(crate) enum ParseError {
    /// The body was not valid UTF-8.
    Utf8,
    /// The body was not valid JSON, including when it exceeded the JSON
    /// recursion limit.
    Json(serde_json::Error),
    /// The JSON value was not an object.
    NotAnObject,
    /// The value was an object but not a valid JSON-RPC message.
    Invalid(&'static str),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8 => formatter.write_str("message is not valid UTF-8"),
            Self::Json(error) => write!(formatter, "invalid JSON: {error}"),
            Self::NotAnObject => formatter.write_str("message must be a JSON object"),
            Self::Invalid(reason) => formatter.write_str(reason),
        }
    }
}

impl ParseError {
    /// JSON-RPC code to answer with.
    pub(crate) const fn code(&self) -> i64 {
        match self {
            Self::Utf8 | Self::Json(_) => error_codes::PARSE_ERROR,
            Self::NotAnObject | Self::Invalid(_) => error_codes::INVALID_REQUEST,
        }
    }

    /// Client-facing message; never contains parser internals.
    pub(crate) const fn response_message(&self) -> &'static str {
        match self {
            Self::Utf8 | Self::Json(_) => "message is not valid JSON",
            Self::NotAnObject | Self::Invalid(_) => "message is not a valid JSON-RPC request",
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

/// Parses a frame body into an incoming message.
pub(crate) fn parse(body: &[u8]) -> Result<Incoming, ParseError> {
    let text = std::str::from_utf8(body).map_err(|_| ParseError::Utf8)?;
    let value: Value = serde_json::from_str(text).map_err(ParseError::Json)?;
    let object = value.as_object().ok_or(ParseError::NotAnObject)?;

    if let Some(version) = object.get("jsonrpc") {
        if version.as_str() != Some("2.0") {
            return Err(ParseError::Invalid("jsonrpc must be \"2.0\""));
        }
    }

    let method = object.get("method").map(|method| {
        method
            .as_str()
            .ok_or(ParseError::Invalid("method must be a string"))
    });
    let id = valid_id(object.get("id"))?;
    let params = object
        .get("params")
        .filter(|params| !params.is_null())
        .cloned();

    match (method, id) {
        (Some(method), Some(id)) => Ok(Incoming::Request {
            id,
            method: method?.to_owned(),
            params,
        }),
        (Some(method), None) => Ok(Incoming::Notification {
            method: method?.to_owned(),
            params,
        }),
        (None, Some(id)) => {
            if object.contains_key("result") || object.contains_key("error") {
                Ok(Incoming::Response { id })
            } else {
                Err(ParseError::Invalid(
                    "message has neither a method nor a response payload",
                ))
            }
        }
        (None, None) => Err(ParseError::Invalid(
            "message has neither a method nor an id",
        )),
    }
}

/// Accepts string and number ids, treats `null` as absent and rejects the rest.
fn valid_id(id: Option<&Value>) -> Result<Option<Value>, ParseError> {
    match id {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.len() <= MAX_REQUEST_ID_BYTES => Ok(id.cloned()),
        Some(Value::String(_)) => Err(ParseError::Invalid("request id is too long")),
        Some(Value::Number(_)) => Ok(id.cloned()),
        Some(_) => Err(ParseError::Invalid("id must be a string or a number")),
    }
}

/// A hashable key derived from a request id.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RequestKey(String);

impl RequestKey {
    /// Canonical key for a string or number id.
    pub(crate) fn from_id(id: &Value) -> Option<Self> {
        match id {
            Value::String(_) | Value::Number(_) => serde_json::to_string(id).ok().map(Self),
            _ => None,
        }
    }

    /// Returns the serialized id.
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// One message the server wants to send.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outgoing {
    /// A response to a request.
    Response {
        /// Id echoed from the request.
        id: Value,
        /// Success payload or failure description.
        outcome: ResponseOutcome,
    },
    /// A notification.
    Notification {
        /// Method name.
        method: String,
        /// JSON parameters.
        params: Value,
    },
}

/// The result or error of a response.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ResponseOutcome {
    /// Successful result; `null` is a valid result.
    Success(Value),
    /// Failure description.
    Failure(ResponseError),
}

/// A JSON-RPC error object.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResponseError {
    /// JSON-RPC error code.
    pub(crate) code: i64,
    /// Short human-readable message. Must not contain internal paths.
    pub(crate) message: String,
}

impl Outgoing {
    /// Builds a successful response.
    pub(crate) const fn success(id: Value, result: Value) -> Self {
        Self::Response {
            id,
            outcome: ResponseOutcome::Success(result),
        }
    }

    /// Builds an error response.
    pub(crate) fn failure(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self::Response {
            id,
            outcome: ResponseOutcome::Failure(ResponseError {
                code,
                message: message.into(),
            }),
        }
    }

    /// Builds a notification.
    pub(crate) fn notification(method: impl Into<String>, params: Value) -> Self {
        Self::Notification {
            method: method.into(),
            params,
        }
    }

    /// Converts the message into its JSON representation.
    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Response {
                id,
                outcome: ResponseOutcome::Success(result),
            } => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Self::Response {
                id,
                outcome: ResponseOutcome::Failure(error),
            } => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": error.code, "message": error.message},
            }),
            Self::Notification { method, params } => json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
        }
    }
}

/// Writes one framed JSON message and flushes the writer.
pub(crate) fn write_frame(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn frame(headers: &str, body: &[u8]) -> Cursor<Vec<u8>> {
        let mut bytes = headers.as_bytes().to_vec();
        bytes.extend_from_slice(body);
        Cursor::new(bytes)
    }

    #[test]
    fn reads_a_content_length_framed_message() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let mut reader = frame(&format!("Content-Length: {}\r\n\r\n", body.len()), body);
        assert_eq!(read_frame(&mut reader).expect("frame"), Some(body.to_vec()));
    }

    #[test]
    fn accepts_other_headers_and_case_insensitive_names() {
        let body = br#"{"jsonrpc":"2.0","method":"initialized"}"#;
        let mut reader = frame(
            &format!(
                "content-type: application/vscode-jsonrpc; charset=utf-8\r\nconTent-LenGth: {}\r\n\r\n",
                body.len()
            ),
            body,
        );
        assert_eq!(read_frame(&mut reader).expect("frame"), Some(body.to_vec()));
    }

    #[test]
    fn accepts_bare_lf_line_endings() {
        let body = b"{}";
        let mut reader = frame(&format!("Content-Length: {}\n\n", body.len()), body);
        assert_eq!(read_frame(&mut reader).expect("frame"), Some(body.to_vec()));
    }

    #[test]
    fn returns_none_on_a_clean_end_of_stream() {
        let mut reader = Cursor::new(Vec::new());
        assert_eq!(read_frame(&mut reader).expect("eof"), None);
    }

    #[test]
    fn rejects_a_truncated_body() {
        let mut reader = frame("Content-Length: 10\r\n\r\n", b"{}");
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_a_missing_content_length() {
        let mut reader = frame("Content-Type: text/plain\r\n\r\n", b"");
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::MissingContentLength)
        ));
    }

    #[test]
    fn rejects_duplicate_content_length() {
        let mut reader = frame("Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}", b"");
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::DuplicateContentLength)
        ));
    }

    #[test]
    fn rejects_a_non_numeric_content_length() {
        for value in ["", "-1", "2.5", "0x10", "2 2", "２"] {
            let mut reader = frame(&format!("Content-Length: {value}\r\n\r\n"), b"");
            assert!(
                matches!(
                    read_frame(&mut reader),
                    Err(FrameError::InvalidContentLength)
                ),
                "value {value:?}"
            );
        }
    }

    #[test]
    fn rejects_a_malformed_header() {
        let mut reader = frame("Content-Length 2\r\n\r\n{}", b"");
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::InvalidHeader)
        ));
    }

    #[test]
    fn rejects_an_oversized_message_before_reading_the_body() {
        let mut reader = frame(
            &format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1),
            b"",
        );
        let error = read_frame(&mut reader).expect_err("too large");
        assert!(matches!(
            error,
            FrameError::MessageTooLarge {
                length,
                limit: MAX_MESSAGE_BYTES
            } if length == MAX_MESSAGE_BYTES + 1
        ));
    }

    #[test]
    fn rejects_an_oversized_header_line() {
        let mut reader = frame(&format!("X-Long: {}\r\n\r\n", "a".repeat(9_000)), b"");
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::HeaderLineTooLong)
        ));
    }

    #[test]
    fn rejects_too_many_headers() {
        let headers = "X-Test: 1\r\n".repeat(MAX_HEADERS + 1);
        let mut reader = frame(&format!("{headers}\r\n"), b"");
        assert!(matches!(
            read_frame(&mut reader),
            Err(FrameError::TooManyHeaders)
        ));
    }

    #[test]
    fn parses_requests_notifications_and_responses() {
        let request = parse(
            br#"{"jsonrpc":"2.0","id":"abc","method":"textDocument/hover","params":{"x":1}}"#,
        )
        .expect("request");
        assert_eq!(
            request,
            Incoming::Request {
                id: Value::String("abc".to_owned()),
                method: "textDocument/hover".to_owned(),
                params: Some(json!({"x": 1})),
            }
        );

        let notification =
            parse(br#"{"jsonrpc":"2.0","method":"initialized"}"#).expect("notification");
        assert_eq!(
            notification,
            Incoming::Notification {
                method: "initialized".to_owned(),
                params: None,
            }
        );

        let response = parse(br#"{"jsonrpc":"2.0","id":7,"result":null}"#).expect("response");
        assert_eq!(
            response,
            Incoming::Response {
                id: Value::Number(7.into()),
            }
        );
    }

    #[test]
    fn treats_a_null_id_as_absent() {
        let message = parse(br#"{"jsonrpc":"2.0","id":null,"method":"exit"}"#).expect("message");
        assert_eq!(
            message,
            Incoming::Notification {
                method: "exit".to_owned(),
                params: None,
            }
        );
    }

    #[test]
    fn rejects_wrong_jsonrpc_versions_and_ids() {
        for body in [
            &br#"{"jsonrpc":"1.0","id":1,"method":"x"}"#[..],
            &br#"{"jsonrpc":2,"id":1,"method":"x"}"#[..],
            &br#"{"jsonrpc":"2.0","id":true,"method":"x"}"#[..],
            &br#"{"jsonrpc":"2.0","id":{},"method":"x"}"#[..],
            &br#"{"jsonrpc":"2.0","id":1,"method":7}"#[..],
            &br#"{"jsonrpc":"2.0","id":1}"#[..],
            &br#"{"jsonrpc":"2.0"}"#[..],
            &br"[]"[..],
            &br"null"[..],
        ] {
            assert!(
                parse(body).is_err(),
                "body {:?}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn bounds_request_id_length() {
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":"{}","method":"x"}}"#,
            "a".repeat(MAX_REQUEST_ID_BYTES)
        );
        assert!(parse(body.as_bytes()).is_ok());
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":"{}","method":"x"}}"#,
            "a".repeat(MAX_REQUEST_ID_BYTES + 1)
        );
        assert!(parse(body.as_bytes()).is_err());
    }

    #[test]
    fn rejects_invalid_utf8_and_malformed_json() {
        assert!(matches!(parse(&[0xff, 0xfe]), Err(ParseError::Utf8)));
        assert!(matches!(parse(b"{oops"), Err(ParseError::Json(_))));
        assert_eq!(
            parse(b"{oops").expect_err("syntax").code(),
            error_codes::PARSE_ERROR
        );
        assert_eq!(
            parse(b"[]").expect_err("not an object").code(),
            error_codes::INVALID_REQUEST
        );
    }

    #[test]
    fn rejects_deeply_nested_json_without_overflowing() {
        let depth = 100_000;
        let mut body = "[".repeat(depth);
        body.push_str(&"]".repeat(depth));
        assert!(matches!(parse(body.as_bytes()), Err(ParseError::Json(_))));
    }

    #[test]
    fn request_keys_are_stable_and_typed() {
        let numeric = RequestKey::from_id(&json!(1)).expect("numeric id");
        let string = RequestKey::from_id(&json!("1")).expect("string id");
        assert_ne!(numeric, string);
        assert_eq!(numeric.as_str(), "1");
        assert_eq!(string.as_str(), "\"1\"");
        assert!(RequestKey::from_id(&Value::Null).is_none());
        assert!(RequestKey::from_id(&json!(true)).is_none());
        assert!(RequestKey::from_id(&json!({"id": 1})).is_none());
    }

    #[test]
    fn writes_frames_with_a_byte_length_and_flushes() {
        let message = json!({"jsonrpc": "2.0", "id": 1, "result": "中文"});
        let expected_body = serde_json::to_vec(&message).expect("serialize");
        let mut output = Vec::new();
        write_frame(&mut output, &message).expect("write");
        let expected_header = format!("Content-Length: {}\r\n\r\n", expected_body.len());
        assert!(output.starts_with(expected_header.as_bytes()));
        assert_eq!(&output[expected_header.len()..], expected_body.as_slice());
    }

    #[test]
    fn serializes_responses_and_notifications() {
        let success = Outgoing::success(json!(3), json!(null)).into_value();
        assert_eq!(success, json!({"jsonrpc": "2.0", "id": 3, "result": null}));

        let failure =
            Outgoing::failure(json!("x"), error_codes::METHOD_NOT_FOUND, "no such method")
                .into_value();
        assert_eq!(
            failure,
            json!({
                "jsonrpc": "2.0",
                "id": "x",
                "error": {"code": -32601, "message": "no such method"},
            })
        );

        let notification =
            Outgoing::notification("textDocument/publishDiagnostics", json!({"uri": "x"}))
                .into_value();
        assert_eq!(
            notification,
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"uri": "x"},
            })
        );
    }
}
