//! F0-07/F0-08 framing: `Content-Length` streaming parser, writer and JSON
//! budgets.
//!
//! The wire format is exactly:
//!
//! ```text
//! Content-Length: <N>\r\n
//! \r\n
//! <N bytes of UTF-8 JSON-RPC 2.0>
//! ```
//!
//! [`FrameReader`] parses frames incrementally from any [`Read`] source. It
//! keeps one fixed-size read buffer, parses the header one byte at a time and
//! checks the declared body length against the response budget **before** it
//! reserves memory, so a malicious child cannot grow the client without bound.
//! Header bytes, JSON depth, string length and container entries are all
//! capped by [`FrameLimits`].
//!
//! [`FrameWriter`] is the only supported way to produce an outbound frame. It
//! validates the role-specific budget before writing and always emits exactly
//! one `Content-Length` header with CRLF separators and the body with no
//! trailing newline.
//!
//! Parsing accepts both CRLF and bare LF line endings because the pipe is a
//! local, single-reader stream; writing always uses CRLF.

use std::fmt;
use std::io::{self, Read, Write};

use thiserror::Error;

use crate::config::{
    DEFAULT_REQUEST_DEADLINE_MS, MAX_AST_NODES, MAX_AST_STRINGS, MAX_CALLBACK_BODY_BYTES,
    MAX_FRAME_HEADER_BYTES, MAX_IN_FLIGHT_CALLBACKS, MAX_IN_FLIGHT_REQUESTS, MAX_JSON_DEPTH,
    MAX_MESSAGE_ENTRIES, MAX_PENDING_QUEUE, MAX_REQUEST_BODY_BYTES, MAX_RESPONSE_BODY_BYTES,
    MAX_STRING_BYTES, SNAPSHOT_DEADLINE_MS,
};

/// Bytes pulled from the child per `read` call while parsing a frame.
///
/// The buffer is the only memory the parser may hold outside the body itself;
/// bytes belonging to the next frame stay in this buffer between calls.
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// F0-08 budgets for one native connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    /// Maximum bytes for the complete header block, including the terminating
    /// empty line (the CRLF sequences are counted).
    pub header_bytes: usize,
    /// Maximum outbound request body.
    pub request_bytes: usize,
    /// Maximum accepted response body.
    pub response_bytes: usize,
    /// Maximum accepted callback request or response body.
    pub callback_bytes: usize,
    /// Maximum accepted JSON nesting depth.
    pub max_json_depth: usize,
    /// Maximum array entries or object members in one message.
    pub message_entries: usize,
    /// Maximum length of one JSON string in bytes.
    pub string_bytes: usize,
    /// Maximum AST node count for one source file.
    pub ast_nodes: usize,
    /// Maximum AST string-pool entry count for one source file.
    pub ast_strings: usize,
    /// Maximum concurrent in-flight client requests.
    pub in_flight_requests: usize,
    /// Maximum queued client requests while all in-flight slots are busy.
    pub pending_queue: usize,
    /// Maximum concurrent in-flight engine callbacks.
    pub in_flight_callbacks: usize,
    /// Default deadline for one native request, in milliseconds.
    pub request_deadline_ms: u64,
    /// Deadline for snapshot creation or update, in milliseconds.
    pub snapshot_deadline_ms: u64,
}

/// Budgets frozen by F0-08 (contract section 3.4).
pub const DEFAULT_FRAME_LIMITS: FrameLimits = FrameLimits {
    header_bytes: MAX_FRAME_HEADER_BYTES,
    request_bytes: MAX_REQUEST_BODY_BYTES,
    response_bytes: MAX_RESPONSE_BODY_BYTES,
    callback_bytes: MAX_CALLBACK_BODY_BYTES,
    max_json_depth: MAX_JSON_DEPTH,
    message_entries: MAX_MESSAGE_ENTRIES,
    string_bytes: MAX_STRING_BYTES,
    ast_nodes: MAX_AST_NODES,
    ast_strings: MAX_AST_STRINGS,
    in_flight_requests: MAX_IN_FLIGHT_REQUESTS,
    pending_queue: MAX_PENDING_QUEUE,
    in_flight_callbacks: MAX_IN_FLIGHT_CALLBACKS,
    request_deadline_ms: DEFAULT_REQUEST_DEADLINE_MS,
    snapshot_deadline_ms: SNAPSHOT_DEADLINE_MS,
};

impl FrameLimits {
    /// Returns the body budget that applies to `role`.
    #[must_use]
    pub const fn body_limit(self, role: FrameRole) -> usize {
        match role {
            FrameRole::Request => self.request_bytes,
            FrameRole::Response => self.response_bytes,
            FrameRole::CallbackRequest | FrameRole::CallbackResponse => self.callback_bytes,
        }
    }
}

/// Logical frame direction; both directions share one duplex stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRole {
    /// Client request written to the engine.
    Request,
    /// Engine response read from the engine.
    Response,
    /// Engine-to-client filesystem callback request.
    CallbackRequest,
    /// Client callback response written to the engine.
    CallbackResponse,
}

impl FrameRole {
    /// Returns whether the client writes frames with this role.
    ///
    /// Inbound roles are [`Self::Response`] and [`Self::CallbackRequest`];
    /// [`FrameWriter`] rejects them so a reader-side role cannot be written by
    /// mistake.
    #[must_use]
    pub const fn is_outbound(self) -> bool {
        matches!(self, Self::Request | Self::CallbackResponse)
    }
}

/// One complete frame body read from the engine.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    body: Vec<u8>,
}

impl Frame {
    /// Wraps a body that was already validated against a frame budget.
    ///
    /// Construction is private: only [`FrameReader`] may produce a frame, so
    /// callers cannot fabricate one that bypassed the header and body checks.
    const fn new(body: Vec<u8>) -> Self {
        Self { body }
    }

    /// Returns the frame body exactly as it appeared on the wire.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Consumes the frame and returns its body.
    #[must_use]
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }

    /// Returns the body length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.body.len()
    }

    /// Returns whether the body is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Framing failure.
///
/// All variants are terminal for the connection: the client must stop reading,
/// fail pending work and terminate/reap the child (contract section 3.3).
#[derive(Debug, Error)]
pub enum FrameError {
    /// The underlying pipe failed.
    #[error("frame I/O failed: {0}")]
    Io(#[from] io::Error),
    /// EOF arrived after a frame started but before it was complete.
    #[error("stream ended inside a frame")]
    UnexpectedEof,
    /// The header block exceeded the configured budget.
    #[error("frame header exceeds the {limit}-byte budget")]
    HeaderTooLarge {
        /// Configured header budget.
        limit: usize,
    },
    /// A header line violated the framing grammar.
    #[error("frame header is malformed: {reason}")]
    MalformedHeader {
        /// Static description of the failed invariant.
        reason: &'static str,
    },
    /// The header block did not declare a body length.
    #[error("frame header has no Content-Length")]
    MissingContentLength,
    /// More than one `Content-Length` header was present.
    #[error("frame header has more than one Content-Length")]
    DuplicateContentLength,
    /// `Content-Length` was not a bare unsigned decimal integer.
    #[error("frame Content-Length is not an unsigned decimal integer")]
    InvalidContentLength,
    /// The declared body cannot be accepted under the role budget.
    #[error("frame body of {len} bytes exceeds the {limit}-byte budget")]
    BodyTooLarge {
        /// Declared body length.
        len: usize,
        /// Budget that applies to the frame.
        limit: usize,
    },
    /// The client tried to write an inbound-only frame role.
    #[error("frame role {role:?} is not written by the client")]
    InvalidDirection {
        /// Role supplied to the writer.
        role: FrameRole,
    },
    /// The body could not be reserved without aborting the process.
    #[error("could not reserve memory for a frame body")]
    AllocationFailed,
}

/// JSON budget violation inside one already framed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum JsonBudgetError {
    /// The message was not valid UTF-8.
    #[error("JSON message is not valid UTF-8")]
    InvalidUtf8,
    /// Nesting exceeded the configured depth.
    #[error("JSON message nesting exceeds {limit} levels")]
    DepthExceeded {
        /// Configured depth budget.
        limit: usize,
    },
    /// One container held more entries than the budget allows.
    #[error("JSON container exceeds {limit} entries")]
    EntriesExceeded {
        /// Configured entry budget.
        limit: usize,
    },
    /// One string was longer than the budget allows.
    #[error("JSON string exceeds {limit} bytes")]
    StringTooLarge {
        /// Configured string budget.
        limit: usize,
    },
}

/// Streaming `Content-Length` frame reader.
///
/// The reader owns one fixed 8 KiB internal buffer and returns a
/// [`Frame`] only after its body has been fully read. It never allocates more
/// than the declared body length and never consumes bytes that belong to the
/// next frame.
///
/// Every inbound frame is first capped by [`FrameLimits::response_bytes`]
/// (64 MiB). Engine callbacks share the stream and are additionally capped by
/// [`FrameLimits::callback_bytes`] (32 MiB) in
/// [`crate::native::rpc::decode_inbound`]; the reader cannot know the role
/// before the body is decoded.
#[derive(Debug)]
pub struct FrameReader<R> {
    reader: R,
    limits: FrameLimits,
    buffer: Box<[u8]>,
    start: usize,
    end: usize,
    eof: bool,
}

impl<R: Read> FrameReader<R> {
    /// Creates a reader with the frozen budgets.
    #[must_use]
    pub fn new(reader: R, limits: FrameLimits) -> Self {
        Self {
            reader,
            limits,
            buffer: vec![0; READ_CHUNK_BYTES].into_boxed_slice(),
            start: 0,
            end: 0,
            eof: false,
        }
    }

    /// Returns the budgets this reader enforces.
    #[must_use]
    pub const fn limits(&self) -> FrameLimits {
        self.limits
    }

    /// Returns the next frame, or `None` at a clean EOF before any frame byte.
    ///
    /// # Errors
    ///
    /// Returns a [`FrameError`] for malformed headers, budget overruns, EOF in
    /// the middle of a frame, or a failing pipe. Every error is terminal.
    pub fn read_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        let Some(length) = self.read_header()? else {
            return Ok(None);
        };

        let limit = self.limits.response_bytes;
        if length > limit {
            return Err(FrameError::BodyTooLarge { len: length, limit });
        }

        let body = self.read_body(length)?;
        Ok(Some(Frame::new(body)))
    }

    /// Reads until the empty header line and returns the declared length.
    fn read_header(&mut self) -> Result<Option<usize>, FrameError> {
        let mut consumed = 0usize;
        let mut content_length = None;
        let mut line = Vec::new();

        loop {
            line.clear();

            loop {
                let Some(byte) = self.next_byte()? else {
                    return if consumed == 0 {
                        Ok(None)
                    } else {
                        Err(FrameError::UnexpectedEof)
                    };
                };

                consumed = consumed.saturating_add(1);
                if consumed > self.limits.header_bytes {
                    return Err(FrameError::HeaderTooLarge {
                        limit: self.limits.header_bytes,
                    });
                }

                if byte == b'\n' {
                    break;
                }
                line.push(byte);
            }

            if line.last() == Some(&b'\r') {
                line.pop();
            }

            if line.is_empty() {
                return content_length.map_or(Err(FrameError::MissingContentLength), |length| {
                    Ok(Some(length))
                });
            }

            let text = std::str::from_utf8(&line).map_err(|_| FrameError::MalformedHeader {
                reason: "header line is not UTF-8",
            })?;
            parse_header_line(text, &mut content_length)?;
        }
    }

    /// Reads exactly `length` bytes into a freshly reserved body.
    fn read_body(&mut self, length: usize) -> Result<Vec<u8>, FrameError> {
        let mut body = Vec::new();
        body.try_reserve_exact(length)
            .map_err(|_| FrameError::AllocationFailed)?;

        while body.len() < length {
            if self.start == self.end && !self.fill()? {
                return Err(FrameError::UnexpectedEof);
            }

            let available = self.end - self.start;
            let wanted = length - body.len();
            let take = available.min(wanted);
            let Some(chunk) = self.buffer.get(self.start..self.start + take) else {
                return Err(FrameError::UnexpectedEof);
            };
            body.extend_from_slice(chunk);
            self.start += take;
        }

        Ok(body)
    }

    /// Ensures the read buffer holds at least one byte.
    fn fill(&mut self) -> Result<bool, FrameError> {
        if self.start < self.end {
            return Ok(true);
        }
        if self.eof {
            return Ok(false);
        }

        loop {
            match self.reader.read(&mut self.buffer) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(read) => {
                    self.start = 0;
                    self.end = read;
                    return Ok(true);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(FrameError::Io(error)),
            }
        }
    }

    /// Returns the next buffered byte, filling the buffer on demand.
    fn next_byte(&mut self) -> Result<Option<u8>, FrameError> {
        if self.start == self.end && !self.fill()? {
            return Ok(None);
        }

        let byte = self.buffer.get(self.start).copied();
        if byte.is_some() {
            self.start += 1;
        }
        Ok(byte)
    }
}

/// Parses one header line, recording exactly one `Content-Length`.
fn parse_header_line(line: &str, content_length: &mut Option<usize>) -> Result<(), FrameError> {
    let Some((name, value)) = line.split_once(':') else {
        return Err(FrameError::MalformedHeader {
            reason: "header line has no colon",
        });
    };

    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(FrameError::MalformedHeader {
            reason: "header name is not an ASCII token",
        });
    }

    let value = value.trim_matches(|character| character == ' ' || character == '\t');
    if name.eq_ignore_ascii_case("content-length") {
        if content_length.is_some() {
            return Err(FrameError::DuplicateContentLength);
        }
        *content_length = Some(parse_content_length(value)?);
    }

    Ok(())
}

/// Parses `Content-Length` without accepting signs, hex or whitespace.
fn parse_content_length(value: &str) -> Result<usize, FrameError> {
    if value.is_empty() {
        return Err(FrameError::InvalidContentLength);
    }

    let mut total: usize = 0;
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return Err(FrameError::InvalidContentLength);
        }
        let digit = usize::from(byte.saturating_sub(b'0'));
        total = total
            .checked_mul(10)
            .and_then(|current| current.checked_add(digit))
            .ok_or(FrameError::InvalidContentLength)?;
    }

    Ok(total)
}

/// Writes one `Content-Length`-framed message.
#[derive(Debug)]
pub struct FrameWriter<W> {
    inner: W,
    limits: FrameLimits,
}

impl<W: Write> FrameWriter<W> {
    /// Wraps a writable pipe with the frozen budgets.
    #[must_use]
    pub const fn new(inner: W, limits: FrameLimits) -> Self {
        Self { inner, limits }
    }

    /// Returns the budgets this writer enforces.
    #[must_use]
    pub const fn limits(&self) -> FrameLimits {
        self.limits
    }

    /// Writes a single frame with exact CRLF framing.
    ///
    /// # Errors
    ///
    /// Rejects inbound-only roles and bodies above the role budget before any
    /// byte is written. Pipe failures are surfaced as [`FrameError::Io`].
    pub fn write_frame(&mut self, role: FrameRole, body: &[u8]) -> Result<(), FrameError> {
        if !role.is_outbound() {
            return Err(FrameError::InvalidDirection { role });
        }

        let limit = self.limits.body_limit(role);
        if body.len() > limit {
            return Err(FrameError::BodyTooLarge {
                len: body.len(),
                limit,
            });
        }

        write!(self.inner, "Content-Length: {}\r\n\r\n", body.len())?;
        self.inner.write_all(body)?;
        self.inner.flush()?;
        Ok(())
    }

    /// Returns a shared reference to the underlying writer.
    #[must_use]
    pub const fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Returns a mutable reference to the underlying writer.
    #[must_use]
    pub const fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consumes the frame writer and returns the underlying writer.
    #[must_use]
    pub fn into_inner(self) -> W {
        self.inner
    }
}

/// Validates UTF-8 plus the JSON depth, string and entry budgets.
///
/// The scan is iterative and allocation-bounded by the configured depth, so a
/// malformed message cannot overflow the stack. Syntax is still validated by
/// `serde_json` in the router; this function only enforces budgets.
///
/// # Errors
///
/// Returns [`JsonBudgetError`] when the message is not UTF-8 or exceeds a
/// configured budget.
pub fn validate_json_budget(message: &[u8], limits: FrameLimits) -> Result<&str, JsonBudgetError> {
    let text = std::str::from_utf8(message).map_err(|_| JsonBudgetError::InvalidUtf8)?;
    scan_json(message, limits)?;
    Ok(text)
}

/// Iterative structural scan that enforces depth, string and entry budgets.
fn scan_json(message: &[u8], limits: FrameLimits) -> Result<(), JsonBudgetError> {
    let mut depth = 0usize;
    let mut containers: Vec<usize> = Vec::new();
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut string_start = 0usize;

    while let Some(&byte) = message.get(index) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                let length = index.saturating_sub(string_start);
                if length > limits.string_bytes {
                    return Err(JsonBudgetError::StringTooLarge {
                        limit: limits.string_bytes,
                    });
                }
            }
        } else {
            match byte {
                b'"' => {
                    in_string = true;
                    string_start = index.saturating_add(1);
                }
                b'{' | b'[' => {
                    depth = depth.saturating_add(1);
                    if depth > limits.max_json_depth {
                        return Err(JsonBudgetError::DepthExceeded {
                            limit: limits.max_json_depth,
                        });
                    }
                    containers.push(0);
                }
                b'}' | b']' => {
                    depth = depth.saturating_sub(1);
                    containers.pop();
                }
                b',' => {
                    if let Some(count) = containers.last_mut() {
                        *count = count.saturating_add(1);
                        if *count >= limits.message_entries {
                            return Err(JsonBudgetError::EntriesExceeded {
                                limit: limits.message_entries,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        index = index.saturating_add(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;

    fn frame_bytes(body: &[u8]) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n", body.len())
            .into_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect()
    }

    fn small_limits() -> FrameLimits {
        FrameLimits {
            header_bytes: 128,
            request_bytes: 16,
            response_bytes: 32,
            callback_bytes: 8,
            max_json_depth: 4,
            message_entries: 4,
            string_bytes: 8,
            ..DEFAULT_FRAME_LIMITS
        }
    }

    /// Yields the payload in fixed-size chunks to exercise split reads.
    struct ChunkedReader {
        data: Vec<u8>,
        offset: usize,
        chunk: usize,
    }

    impl ChunkedReader {
        fn new(data: Vec<u8>, chunk: usize) -> Self {
            Self {
                data,
                offset: 0,
                chunk,
            }
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let remaining = self.data.len().saturating_sub(self.offset);
            let take = remaining.min(self.chunk).min(buffer.len());
            let Some(chunk) = self.data.get(self.offset..self.offset + take) else {
                return Ok(0);
            };
            let Some(target) = buffer.get_mut(..take) else {
                return Ok(0);
            };
            target.copy_from_slice(chunk);
            self.offset += take;
            Ok(take)
        }
    }

    #[test]
    fn f0_budget_table_matches_contract_section_3_4() {
        let limits = DEFAULT_FRAME_LIMITS;

        assert_eq!(limits.header_bytes, 8 * 1024);
        assert_eq!(limits.request_bytes, 32 * 1024 * 1024);
        assert_eq!(limits.response_bytes, 64 * 1024 * 1024);
        assert_eq!(limits.callback_bytes, 32 * 1024 * 1024);
        assert_eq!(limits.max_json_depth, 128);
        assert_eq!(limits.message_entries, 100_000);
        assert_eq!(limits.string_bytes, 32 * 1024 * 1024);
        assert_eq!(limits.ast_nodes, 1_000_000);
        assert_eq!(limits.ast_strings, 1_000_000);
        assert_eq!(limits.in_flight_requests, 8);
        assert_eq!(limits.pending_queue, 64);
        assert_eq!(limits.in_flight_callbacks, 16);
        assert_eq!(limits.request_deadline_ms, 15_000);
        assert_eq!(limits.snapshot_deadline_ms, 30_000);
    }

    #[test]
    fn parses_frames_split_across_every_read_boundary() {
        let mut payload = frame_bytes(BODY);
        payload.extend_from_slice(&frame_bytes(b"{}"));

        let mut reader = FrameReader::new(ChunkedReader::new(payload, 1), DEFAULT_FRAME_LIMITS);

        let first = reader.read_frame().expect("first frame").expect("frame");
        assert_eq!(first.body(), BODY);
        let second = reader.read_frame().expect("second frame").expect("frame");
        assert_eq!(second.body(), b"{}");
        assert!(reader.read_frame().expect("clean eof").is_none());
    }

    #[test]
    fn keeps_next_frame_bytes_in_the_read_buffer() {
        let mut payload = frame_bytes(BODY);
        payload.extend_from_slice(&frame_bytes(BODY));

        let mut reader = FrameReader::new(ChunkedReader::new(payload, 4096), DEFAULT_FRAME_LIMITS);
        assert_eq!(
            reader.read_frame().expect("first").expect("frame").body(),
            BODY
        );
        assert_eq!(
            reader.read_frame().expect("second").expect("frame").body(),
            BODY
        );
    }

    #[test]
    fn accepts_case_insensitive_header_names_and_unknown_headers() {
        let body = b"{}";
        let mut payload =
            format!("X-Trace: ignored\r\ncontent-length: {}\r\n\r\n", body.len()).into_bytes();
        payload.extend_from_slice(body);

        let mut reader = FrameReader::new(io::Cursor::new(payload), DEFAULT_FRAME_LIMITS);
        let frame = reader.read_frame().expect("frame").expect("body");
        assert_eq!(frame.body(), body);
    }

    #[test]
    fn rejects_duplicate_missing_and_invalid_content_length() {
        for (payload, expected) in [
            (
                b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
                "more than one",
            ),
            (b"X-Test: 2\r\n\r\n{}".to_vec(), "no Content-Length"),
            (b"Content-Length: -1\r\n\r\n".to_vec(), "not an unsigned"),
            (b"Content-Length: +1\r\n\r\n{}".to_vec(), "not an unsigned"),
            (b"Content-Length: 0x2\r\n\r\n{}".to_vec(), "not an unsigned"),
            (b"Content-Length: \r\n\r\n".to_vec(), "not an unsigned"),
            (b"Content-Length 2\r\n\r\n{}".to_vec(), "malformed"),
        ] {
            let mut reader = FrameReader::new(io::Cursor::new(payload), DEFAULT_FRAME_LIMITS);
            let error = reader.read_frame().expect_err("must be rejected");
            let text = error.to_string();
            assert!(text.contains(expected), "unexpected error: {text}");
        }
    }

    #[test]
    fn rejects_oversized_header_before_the_terminating_line() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"X-Fill: ");
        payload.extend(std::iter::repeat_n(b'a', 200));
        payload.extend_from_slice(b"\r\n\r\n");

        let mut reader = FrameReader::new(io::Cursor::new(payload), small_limits());
        assert!(matches!(
            reader.read_frame(),
            Err(FrameError::HeaderTooLarge { .. })
        ));
    }

    #[test]
    fn header_budget_boundary_is_inclusive() {
        let body = b"{}";
        let mut payload = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        payload.extend_from_slice(body);
        let header_len = payload.len() - body.len();

        let mut at_limit = DEFAULT_FRAME_LIMITS;
        at_limit.header_bytes = header_len;
        let mut reader = FrameReader::new(io::Cursor::new(payload.clone()), at_limit);
        assert_eq!(
            reader
                .read_frame()
                .expect("at limit")
                .expect("frame")
                .body(),
            body
        );

        let mut over_limit = DEFAULT_FRAME_LIMITS;
        over_limit.header_bytes = header_len - 1;
        let mut reader = FrameReader::new(io::Cursor::new(payload), over_limit);
        assert!(matches!(
            reader.read_frame(),
            Err(FrameError::HeaderTooLarge { .. })
        ));
    }

    #[test]
    fn rejects_short_body_at_eof() {
        let mut payload = b"Content-Length: 10\r\n\r\n".to_vec();
        payload.extend_from_slice(b"abc");

        let mut reader = FrameReader::new(io::Cursor::new(payload), DEFAULT_FRAME_LIMITS);
        assert!(matches!(
            reader.read_frame(),
            Err(FrameError::UnexpectedEof)
        ));
    }

    #[test]
    fn rejects_response_body_above_the_default_budget_before_reading() {
        let mut payload = format!(
            "Content-Length: {}\r\n\r\n",
            DEFAULT_FRAME_LIMITS.response_bytes + 1
        )
        .into_bytes();
        payload.extend_from_slice(b"{}");

        let mut reader = FrameReader::new(io::Cursor::new(payload), DEFAULT_FRAME_LIMITS);
        assert!(matches!(
            reader.read_frame(),
            Err(FrameError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn reports_clean_eof_only_before_a_frame_starts() {
        let mut reader = FrameReader::new(io::Cursor::new(Vec::new()), DEFAULT_FRAME_LIMITS);
        assert!(reader.read_frame().expect("clean eof").is_none());
    }

    #[test]
    fn writer_emits_exact_crlf_framing() {
        let mut sink = Vec::new();
        let mut writer = FrameWriter::new(&mut sink, DEFAULT_FRAME_LIMITS);
        writer
            .write_frame(FrameRole::Request, BODY)
            .expect("write frame");

        let expected = format!("Content-Length: {}\r\n\r\n", BODY.len()).into_bytes();
        assert_eq!(sink, [expected, BODY.to_vec()].concat());
    }

    #[test]
    fn writer_enforces_request_and_callback_budgets_before_writing() {
        let limits = small_limits();

        for role in [FrameRole::Request, FrameRole::CallbackResponse] {
            let mut sink = Vec::new();
            let mut writer = FrameWriter::new(&mut sink, limits);
            let body = vec![b'x'; limits.body_limit(role) + 1];
            let error = writer.write_frame(role, &body).expect_err("over budget");
            assert!(matches!(error, FrameError::BodyTooLarge { .. }));
            assert!(sink.is_empty(), "nothing may be written past a budget");
        }
    }

    #[test]
    fn writer_rejects_inbound_roles() {
        let mut sink = Vec::new();
        let mut writer = FrameWriter::new(&mut sink, DEFAULT_FRAME_LIMITS);
        for role in [FrameRole::Response, FrameRole::CallbackRequest] {
            assert!(matches!(
                writer.write_frame(role, b"{}"),
                Err(FrameError::InvalidDirection { .. })
            ));
        }
        assert_eq!(sink, [] as [u8; 0]);
    }

    #[test]
    fn json_budget_accepts_a_message_within_every_limit() {
        let text = validate_json_budget(BODY, DEFAULT_FRAME_LIMITS).expect("valid message");
        assert!(text.starts_with('{'));
    }

    #[test]
    fn json_budget_rejects_invalid_utf8_and_deep_nesting() {
        assert_eq!(
            validate_json_budget(b"\xff", DEFAULT_FRAME_LIMITS).expect_err("utf8"),
            JsonBudgetError::InvalidUtf8
        );

        let deep = format!("{}{}", "[".repeat(129), "]".repeat(129));
        assert!(matches!(
            validate_json_budget(deep.as_bytes(), DEFAULT_FRAME_LIMITS),
            Err(JsonBudgetError::DepthExceeded { .. })
        ));

        let shallow = format!("{}{}", "[".repeat(4), "]".repeat(4));
        assert!(validate_json_budget(shallow.as_bytes(), small_limits()).is_ok());
    }

    #[test]
    fn json_budget_rejects_long_strings_and_crowded_containers() {
        let limit = small_limits().string_bytes;
        let exact = format!("\"{}\"", "a".repeat(limit));
        assert!(
            validate_json_budget(exact.as_bytes(), small_limits()).is_ok(),
            "a string exactly at the budget must pass"
        );

        let over = format!("\"{}\"", "a".repeat(limit + 1));
        assert!(matches!(
            validate_json_budget(over.as_bytes(), small_limits()),
            Err(JsonBudgetError::StringTooLarge { .. })
        ));

        let crowded = "[1,2,3,4,5]";
        assert!(matches!(
            validate_json_budget(crowded.as_bytes(), small_limits()),
            Err(JsonBudgetError::EntriesExceeded { .. })
        ));

        // Escapes do not confuse the string scanner: `"a\"b"` is one string.
        assert!(validate_json_budget(br#""a\"b""#, small_limits()).is_ok());
    }
}
