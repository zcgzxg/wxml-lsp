//! JSON-RPC 2.0 envelope, identifier and routing state.
//!
//! The engine speaks JSON-RPC 2.0 over the frames parsed in
//! [`crate::native::framing`]. This module owns:
//!
//! - client request identifiers: `u64`, allocated from 1 per connection and
//!   never reused while the connection lives ([`RequestIdAllocator`]);
//! - server callback identifiers: the original JSON scalar is preserved so a
//!   string such as `"api1"` is echoed verbatim ([`CallbackId`]);
//! - strict message classification: a message with `method` plus `id` is a
//!   callback request, a message with `id` and exactly one of `result`/`error`
//!   is a response, and everything else is a protocol violation
//!   ([`decode_inbound`]);
//! - a bounded admission queue (8 in flight, 64 queued) and a bounded
//!   abandoned-id table so a timed-out request can stay discardable without
//!   wedging the session ([`PendingRequests`], [`RpcDispatch`]);
//! - recoverable callback rejections (oversized, duplicate, capacity,
//!   malformed method) that preserve the callback id so the caller can send a
//!   JSON-RPC error and keep the session alive ([`CallbackRejected`]).
//!
//! Error text is bounded: engine error messages are truncated and stripped of
//! control characters before they are stored, structured `data` is not
//! retained at all, and [`JsonRpcError`] converts to
//! [`NativeError::EngineError`] carrying only the numeric code.

use std::collections::{BTreeSet, VecDeque};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Map, Number, Value};
use thiserror::Error;

use crate::native::NativeError;
use crate::native::framing::{FrameLimits, JsonBudgetError, validate_json_budget};
use crate::native::protocol::NativeRequest;

/// Exact JSON-RPC version string required on every envelope.
pub const JSONRPC_VERSION: &str = "2.0";

/// Maximum bytes retained from an engine error message.
const MAX_ERROR_MESSAGE_BYTES: usize = 512;

/// Maximum bytes accepted for a callback identifier string.
const MAX_CALLBACK_ID_BYTES: usize = 256;

/// Maximum bytes accepted for a callback method name.
const MAX_METHOD_BYTES: usize = 256;

/// Monotonic client request identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates an identifier.
    ///
    /// Allocators start at 1 and never produce 0, but parsing an inbound
    /// response may yield any `u64`; the pending table rejects unknown values.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the wire integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Allocates client request identifiers from 1, wrapping after `u64::MAX`.
///
/// Wrapping to 1 after [`u64::MAX`] is documented rather than silently
/// reusing an identifier: a connection that issues 2^64 requests would need to
/// terminate before a still-pending identifier could be reused. The allocator
/// never yields 0, which keeps `0` available as an obviously invalid id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestIdAllocator {
    next: u64,
}

impl RequestIdAllocator {
    /// Starts a fresh connection at id 1.
    #[must_use]
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    /// Starts at `first`, normalizing 0 to 1.
    ///
    /// This exists for tests and for a session that resumes numbering from a
    /// recorded checkpoint; production sessions use [`Self::new`].
    #[must_use]
    pub const fn starting_at(first: u64) -> Self {
        Self {
            next: if first == 0 { 1 } else { first },
        }
    }

    /// Returns the identifier the next call to [`Self::allocate`] will yield.
    #[must_use]
    pub const fn peek(&self) -> RequestId {
        RequestId::new(self.next)
    }

    /// Allocates the next identifier and advances the cursor.
    pub const fn allocate(&mut self) -> RequestId {
        let id = RequestId::new(self.next);
        self.next = if self.next == u64::MAX {
            1
        } else {
            self.next + 1
        };
        id
    }
}

impl Default for RequestIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Server callback identifier with the original JSON scalar preserved.
///
/// The engine currently sends strings such as `"api1"`, but the contract
/// requires echoing the original JSON type, so numbers stay distinct from
/// numeric strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CallbackId {
    /// Numeric callback identifier.
    Number(u64),
    /// String callback identifier.
    String(String),
}

impl CallbackId {
    /// Parses a callback identifier from the wire value.
    ///
    /// Only unsigned integers and non-empty bounded strings are accepted.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::Envelope`] for floats, negative numbers, booleans,
    /// empty strings or strings above 256 bytes. An invalid id is terminal
    /// because the client cannot address an error response to the engine.
    pub fn from_value(value: &Value) -> Result<Self, RpcError> {
        match value {
            Value::Number(number) => number.as_u64().map(Self::Number).ok_or(RpcError::Envelope {
                reason: "callback id number must be an unsigned integer",
            }),
            Value::String(text) => {
                if text.is_empty() || text.len() > MAX_CALLBACK_ID_BYTES {
                    return Err(RpcError::Envelope {
                        reason: "callback id string is empty or too long",
                    });
                }
                Ok(Self::String(text.clone()))
            }
            _ => Err(RpcError::Envelope {
                reason: "callback id must be a number or a string",
            }),
        }
    }

    /// Serializes the identifier with its original JSON type.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Number(number) => Value::Number(Number::from(*number)),
            Self::String(text) => Value::String(text.clone()),
        }
    }
}

/// Why a callback was rejected without invalidating the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackRejectKind {
    /// The callback body exceeded [`FrameLimits::callback_bytes`].
    Oversized,
    /// The callback id is already in flight.
    Duplicate,
    /// The in-flight callback budget is exhausted.
    Capacity,
    /// The callback method was empty, too long or not a string.
    Malformed,
}

impl CallbackRejectKind {
    /// Returns the JSON-RPC error code to send for this rejection.
    #[must_use]
    pub const fn code(self) -> i64 {
        -32600
    }

    /// Returns a bounded, user-safe reason for the error response.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Oversized => "callback body exceeds the callback budget",
            Self::Duplicate => "callback id is already in flight",
            Self::Capacity => "callback capacity is exhausted",
            Self::Malformed => "callback method is malformed",
        }
    }
}

/// Recoverable callback rejection that still carries the id to echo.
///
/// Only JSON syntax and id-shape failures are terminal. Every rejection in
/// this type must be answered with a JSON-RPC error and then ignored; the
/// session and its routing tables stay usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackRejected {
    id: CallbackId,
    kind: CallbackRejectKind,
}

impl CallbackRejected {
    const fn new(id: CallbackId, kind: CallbackRejectKind) -> Self {
        Self { id, kind }
    }

    /// Returns the callback id that must be echoed.
    #[must_use]
    pub const fn id(&self) -> &CallbackId {
        &self.id
    }

    /// Returns the rejection category.
    #[must_use]
    pub const fn kind(&self) -> CallbackRejectKind {
        self.kind
    }

    /// Splits the rejection into its id and category.
    #[must_use]
    pub fn into_parts(self) -> (CallbackId, CallbackRejectKind) {
        (self.id, self.kind)
    }
}

/// Untouched JSON value used for params and results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPayload {
    value: Value,
}

impl RawPayload {
    /// Wraps a JSON value exactly as it will appear on the wire.
    #[must_use]
    pub const fn new(value: Value) -> Self {
        Self { value }
    }

    /// Returns the wrapped JSON value.
    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.value
    }

    /// Consumes the payload and returns the wrapped value.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.value
    }
}

/// One JSON-RPC error object.
///
/// This is internal evidence only. The `message` is truncated and sanitized;
/// the structured `data` member is deliberately not retained because it is
/// unbounded and may contain a Go panic stack with absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcError {
    /// Parses an error object, truncating and sanitizing its message.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::Envelope`] unless `value` is an object with an
    /// integer `code` and a string `message`.
    pub fn from_value(value: &Value) -> Result<Self, RpcError> {
        let Some(object) = value.as_object() else {
            return Err(RpcError::Envelope {
                reason: "error member is not an object",
            });
        };
        let Some(code) = object.get("code").and_then(Value::as_i64) else {
            return Err(RpcError::Envelope {
                reason: "error member has no integer code",
            });
        };
        let Some(message) = object.get("message").and_then(Value::as_str) else {
            return Err(RpcError::Envelope {
                reason: "error member has no string message",
            });
        };

        Ok(Self {
            code,
            message: bounded_text(message),
        })
    }

    /// Returns the JSON-RPC error code.
    #[must_use]
    pub const fn code(&self) -> i64 {
        self.code
    }

    /// Returns the sanitized, length-bounded message for internal diagnostics.
    ///
    /// This text may still contain engine paths and must never be copied into
    /// a user-visible DTO.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns whether the engine reported `-32601` (method not found).
    #[must_use]
    pub const fn is_method_not_found(&self) -> bool {
        self.code == -32601
    }

    /// Returns whether the engine reported `-32603` (internal error).
    ///
    /// The message of an internal error may contain a Go panic stack; callers
    /// must not expose it and must map it to `Internal`/`Unavailable`.
    #[must_use]
    pub const fn is_internal(&self) -> bool {
        self.code == -32603
    }

    /// Maps the error to a bounded [`NativeError`] carrying only the code.
    #[must_use]
    pub const fn to_native_error(&self) -> NativeError {
        NativeError::EngineError { code: self.code }
    }
}

/// Strictly classified inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundMessage {
    /// Successful response to a client request.
    Response {
        /// Client request identifier being answered.
        id: RequestId,
        /// Response result value.
        result: Value,
    },
    /// Error response to a client request.
    ErrorResponse {
        /// Client request identifier being answered.
        id: RequestId,
        /// Structured engine error.
        error: JsonRpcError,
    },
    /// Engine-to-client filesystem callback request.
    CallbackRequest {
        /// Original callback identifier to echo.
        id: CallbackId,
        /// Callback method name, for example `readFile`.
        method: String,
        /// Callback parameters, for example a path string.
        params: Value,
    },
}

/// Result of decoding one inbound frame.
///
/// A [`DecodedInbound::Rejected`] callback is non-terminal: answer it with a
/// JSON-RPC error and continue. Any `Err` from [`decode_inbound`] is terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedInbound {
    /// The message was accepted.
    Message(InboundMessage),
    /// A callback must be answered with an error and then discarded.
    Rejected(CallbackRejected),
}

/// Message accepted by [`RpcDispatch::route`] with identifiers validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutedMessage {
    /// Response whose id matched a pending client request.
    Response {
        /// Client request identifier.
        id: RequestId,
        /// Response result value.
        result: Value,
    },
    /// Error response whose id matched a pending client request.
    ErrorResponse {
        /// Client request identifier.
        id: RequestId,
        /// Structured engine error.
        error: JsonRpcError,
    },
    /// Callback now registered in the separate callback table.
    Callback {
        /// Callback identifier that must be echoed in the response.
        id: CallbackId,
        /// Callback method name.
        method: String,
        /// Callback parameters.
        params: Value,
    },
    /// Late response for an abandoned request; discard it and stay usable.
    LateResponse {
        /// Identifier that was previously abandoned.
        id: RequestId,
    },
    /// Callback rejected without ending the session; answer with an error.
    RejectedCallback(CallbackRejected),
}

/// Admission state of a client request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    /// The request occupies one of the bounded in-flight slots and may be sent.
    InFlight,
    /// The request is queued and must wait for [`RpcDispatch::promote_queued`].
    Queued,
}

/// Identifier and admission state returned by [`RpcDispatch::allocate_request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestAdmission {
    id: RequestId,
    state: RequestState,
}

impl RequestAdmission {
    const fn new(id: RequestId, state: RequestState) -> Self {
        Self { id, state }
    }

    /// Returns the allocated identifier.
    #[must_use]
    pub const fn id(self) -> RequestId {
        self.id
    }

    /// Returns whether the request may be sent immediately.
    #[must_use]
    pub const fn state(self) -> RequestState {
        self.state
    }

    /// Returns whether the request is queued.
    #[must_use]
    pub const fn is_queued(self) -> bool {
        matches!(self.state, RequestState::Queued)
    }
}

/// Outcome of completing a response id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCompletion {
    /// The id belonged to a live in-flight request.
    Completed,
    /// The id was abandoned after a timeout; discard the response.
    Late,
}

/// JSON-RPC envelope or routing failure.
///
/// Every variant is terminal for the connection except the recoverable cases
/// exposed through [`RoutedMessage::RejectedCallback`] and
/// [`DecodedInbound::Rejected`].
#[derive(Debug, Error)]
pub enum RpcError {
    /// The message exceeded a JSON budget before parsing.
    #[error(transparent)]
    Budget(#[from] JsonBudgetError),
    /// The message was valid UTF-8 but not valid JSON.
    #[error("JSON message is syntactically invalid")]
    InvalidJson,
    /// The envelope shape violated the strict classification rules.
    #[error("JSON-RPC envelope is invalid: {reason}")]
    Envelope {
        /// Static description of the failed invariant.
        reason: &'static str,
    },
    /// A response body exceeded the response budget.
    #[error("response body of {len} bytes exceeds the {limit}-byte budget")]
    ResponseTooLarge {
        /// Observed body length.
        len: usize,
        /// Configured response budget.
        limit: usize,
    },
    /// The queued request budget is exhausted.
    #[error("too many queued client requests (limit {limit})")]
    QueueCapacity {
        /// Configured queue budget.
        limit: usize,
    },
    /// The in-flight callback budget is exhausted.
    #[error("too many in-flight callbacks (limit {limit})")]
    CallbackCapacity {
        /// Configured callback budget.
        limit: usize,
    },
    /// The request id is already present in a pending table.
    #[error("request id {id} is already pending")]
    DuplicateRequest {
        /// Duplicated identifier.
        id: u64,
    },
    /// The response id is not in flight and was not abandoned.
    #[error("request id {id} is not pending")]
    UnknownRequest {
        /// Unknown identifier.
        id: u64,
    },
    /// The callback id is already in flight.
    #[error("callback id is already in flight")]
    DuplicateCallback,
    /// The callback id is not in flight.
    #[error("callback id is not in flight")]
    UnknownCallback,
    /// An outbound envelope could not be serialized.
    #[error("could not encode a JSON-RPC message: {0}")]
    Encode(#[source] serde_json::Error),
}

/// Minimal envelope view used before the full [`Value`] tree is built.
///
/// The probe intentionally never stores `params`, `result` or `error`; the
/// boolean members are set from `IgnoredAny` so a present `null` counts as
/// present without allocating a value. This lets an oversized callback be
/// rejected before its params are materialized.
#[derive(Debug, Deserialize)]
struct EnvelopeProbe {
    jsonrpc: Option<Value>,
    method: Option<Value>,
    id: Option<Value>,
    #[serde(default, rename = "result", deserialize_with = "present")]
    result: bool,
    #[serde(default, rename = "error", deserialize_with = "present")]
    error: bool,
}

/// Deserializes any value, including `null`, and reports that it was present.
fn present<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

/// Decodes one inbound frame body into a strictly classified message.
///
/// # Errors
///
/// Returns [`RpcError`] for budget violations, invalid JSON or any envelope
/// shape outside the three accepted forms. Notifications are rejected because
/// the frozen contract observed none and must fail closed.
pub fn decode_inbound(message: &[u8], limits: FrameLimits) -> Result<DecodedInbound, RpcError> {
    let text = validate_json_budget(message, limits)?;
    let probe: EnvelopeProbe = serde_json::from_str(text).map_err(|_| RpcError::InvalidJson)?;

    let version_ok = matches!(
        &probe.jsonrpc,
        Some(Value::String(version)) if version == JSONRPC_VERSION
    );
    if !version_ok {
        return Err(RpcError::Envelope {
            reason: "jsonrpc must be \"2.0\"",
        });
    }

    if probe.method.is_some() {
        decode_callback(&probe, message, limits, text)
    } else {
        decode_response(&probe, message, limits, text)
    }
}

/// Classifies a message that carries a `method` member.
fn decode_callback(
    probe: &EnvelopeProbe,
    message: &[u8],
    limits: FrameLimits,
    text: &str,
) -> Result<DecodedInbound, RpcError> {
    if probe.result || probe.error {
        return Err(RpcError::Envelope {
            reason: "callback request must not carry result or error",
        });
    }

    let Some(raw_id) = probe.id.as_ref() else {
        return Err(RpcError::Envelope {
            reason: "callback request has no id",
        });
    };
    let id = CallbackId::from_value(raw_id)?;

    if message.len() > limits.callback_bytes {
        return Ok(DecodedInbound::Rejected(CallbackRejected::new(
            id,
            CallbackRejectKind::Oversized,
        )));
    }

    let Some(method) = probe.method.as_ref().and_then(Value::as_str) else {
        return Ok(DecodedInbound::Rejected(CallbackRejected::new(
            id,
            CallbackRejectKind::Malformed,
        )));
    };
    if method.is_empty() || method.len() > MAX_METHOD_BYTES {
        return Ok(DecodedInbound::Rejected(CallbackRejected::new(
            id,
            CallbackRejectKind::Malformed,
        )));
    }
    let method = method.to_owned();

    let mut object = parse_object(text)?;
    let params = object.remove("params").unwrap_or(Value::Null);
    Ok(DecodedInbound::Message(InboundMessage::CallbackRequest {
        id,
        method,
        params,
    }))
}

/// Classifies a message without a `method` member as a response.
fn decode_response(
    probe: &EnvelopeProbe,
    message: &[u8],
    limits: FrameLimits,
    text: &str,
) -> Result<DecodedInbound, RpcError> {
    if probe.id.is_none() {
        return Err(RpcError::Envelope {
            reason: "message has neither method nor id",
        });
    }
    if probe.result == probe.error {
        return Err(RpcError::Envelope {
            reason: "response needs exactly one of result or error",
        });
    }
    if message.len() > limits.response_bytes {
        return Err(RpcError::ResponseTooLarge {
            len: message.len(),
            limit: limits.response_bytes,
        });
    }

    let Some(raw_id) = probe.id.as_ref().and_then(Value::as_u64) else {
        return Err(RpcError::Envelope {
            reason: "response id is not an unsigned integer",
        });
    };
    let id = RequestId::new(raw_id);

    let mut object = parse_object(text)?;
    if probe.result {
        let result = object.remove("result").unwrap_or(Value::Null);
        Ok(DecodedInbound::Message(InboundMessage::Response {
            id,
            result,
        }))
    } else {
        let Some(raw_error) = object.get("error") else {
            return Err(RpcError::Envelope {
                reason: "error response has no error member",
            });
        };
        let error = JsonRpcError::from_value(raw_error)?;
        Ok(DecodedInbound::Message(InboundMessage::ErrorResponse {
            id,
            error,
        }))
    }
}

/// Parses the already validated message into an object map.
fn parse_object(text: &str) -> Result<Map<String, Value>, RpcError> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(RpcError::Envelope {
            reason: "message is not a JSON object",
        }),
        Err(_) => Err(RpcError::InvalidJson),
    }
}

/// Encodes one typed client request envelope.
///
/// # Errors
///
/// Serialization of a [`Value`] can only fail for non-finite numbers, which
/// cannot be constructed through this API; the error variant exists so the
/// function never panics.
pub fn encode_request(id: RequestId, request: &NativeRequest) -> Result<Vec<u8>, RpcError> {
    let mut object = Map::new();
    object.insert(
        "jsonrpc".to_owned(),
        Value::String(JSONRPC_VERSION.to_owned()),
    );
    object.insert("id".to_owned(), Value::Number(Number::from(id.get())));
    object.insert(
        "method".to_owned(),
        Value::String(request.method().as_str().to_owned()),
    );
    object.insert("params".to_owned(), request.params().as_value().clone());

    serde_json::to_vec(&Value::Object(object)).map_err(RpcError::Encode)
}

/// Encodes a successful callback response, echoing the original callback id.
///
/// # Errors
///
/// Returns [`RpcError::Encode`] if the envelope cannot be serialized.
pub fn encode_callback_result(id: &CallbackId, result: &Value) -> Result<Vec<u8>, RpcError> {
    let mut object = Map::new();
    object.insert(
        "jsonrpc".to_owned(),
        Value::String(JSONRPC_VERSION.to_owned()),
    );
    object.insert("id".to_owned(), id.to_value());
    object.insert("result".to_owned(), result.clone());

    serde_json::to_vec(&Value::Object(object)).map_err(RpcError::Encode)
}

/// Encodes a callback error response with a bounded message.
///
/// # Errors
///
/// Returns [`RpcError::Encode`] if the envelope cannot be serialized.
pub fn encode_callback_error(
    id: &CallbackId,
    code: i64,
    message: &str,
) -> Result<Vec<u8>, RpcError> {
    let mut error = Map::new();
    error.insert("code".to_owned(), Value::Number(Number::from(code)));
    error.insert("message".to_owned(), Value::String(bounded_text(message)));

    let mut object = Map::new();
    object.insert(
        "jsonrpc".to_owned(),
        Value::String(JSONRPC_VERSION.to_owned()),
    );
    object.insert("id".to_owned(), id.to_value());
    object.insert("error".to_owned(), Value::Object(error));

    serde_json::to_vec(&Value::Object(object)).map_err(RpcError::Encode)
}

/// Encodes the JSON-RPC error that answers a recoverable callback rejection.
///
/// # Errors
///
/// Returns [`RpcError::Encode`] if the envelope cannot be serialized.
pub fn encode_callback_rejection(rejected: &CallbackRejected) -> Result<Vec<u8>, RpcError> {
    encode_callback_error(
        rejected.id(),
        rejected.kind().code(),
        rejected.kind().reason(),
    )
}

/// Truncates text at a character boundary and removes control characters.
fn bounded_text(text: &str) -> String {
    let mut bounded = String::new();
    for character in text.chars() {
        let required = bounded.len().saturating_add(character.len_utf8());
        if required > MAX_ERROR_MESSAGE_BYTES {
            break;
        }
        if character.is_control() && character != '\n' && character != '\t' {
            continue;
        }
        bounded.push(character);
    }
    bounded
}

/// Bounded admission tables for client requests and engine callbacks.
///
/// In-flight and queued identifiers never collide, duplicate registration is
/// rejected, and abandoned ids are remembered in a bounded table so a late
/// response is discarded instead of poisoning the session.
#[derive(Debug)]
pub struct PendingRequests {
    requests: BTreeSet<RequestId>,
    queued: VecDeque<RequestId>,
    abandoned: BTreeSet<RequestId>,
    callbacks: BTreeSet<CallbackId>,
    max_requests: usize,
    max_queued: usize,
    max_abandoned: usize,
    max_callbacks: usize,
}

impl PendingRequests {
    /// Creates tables with explicit budgets.
    ///
    /// The abandoned table is bounded by the queue budget because a healthy
    /// session restarts after timeouts instead of accumulating abandoned ids.
    #[must_use]
    pub const fn new(max_requests: usize, max_queued: usize, max_callbacks: usize) -> Self {
        Self {
            requests: BTreeSet::new(),
            queued: VecDeque::new(),
            abandoned: BTreeSet::new(),
            callbacks: BTreeSet::new(),
            max_requests,
            max_queued,
            max_abandoned: max_queued,
            max_callbacks,
        }
    }

    /// Creates tables from the frozen frame budgets.
    #[must_use]
    pub const fn from_limits(limits: FrameLimits) -> Self {
        Self::new(
            limits.in_flight_requests,
            limits.pending_queue,
            limits.in_flight_callbacks,
        )
    }

    /// Admits a client request before it is written.
    ///
    /// Requests beyond the in-flight budget are queued until
    /// [`Self::promote_queued`] is called; the queue itself is bounded.
    ///
    /// # Errors
    ///
    /// Rejects duplicates and a full queue. An exhausted in-flight budget is
    /// not an error because the request is queued.
    pub fn begin_request(&mut self, id: RequestId) -> Result<RequestState, RpcError> {
        if self.requests.contains(&id)
            || self.queued.iter().any(|queued| *queued == id)
            || self.abandoned.contains(&id)
        {
            return Err(RpcError::DuplicateRequest { id: id.get() });
        }
        if self.requests.len() < self.max_requests {
            self.requests.insert(id);
            return Ok(RequestState::InFlight);
        }
        if self.queued.len() >= self.max_queued {
            return Err(RpcError::QueueCapacity {
                limit: self.max_queued,
            });
        }
        self.queued.push_back(id);
        Ok(RequestState::Queued)
    }

    /// Promotes the oldest queued request when an in-flight slot is free.
    #[must_use]
    pub fn promote_queued(&mut self) -> Option<RequestId> {
        if self.requests.len() >= self.max_requests {
            return None;
        }
        let id = self.queued.pop_front()?;
        self.requests.insert(id);
        Some(id)
    }

    /// Removes a completed client request or recognizes an abandoned one.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::UnknownRequest`] for an id that is neither in
    /// flight nor abandoned. A queued id must be abandoned, not completed.
    pub fn complete_request(&mut self, id: RequestId) -> Result<RequestCompletion, RpcError> {
        if self.abandoned.remove(&id) {
            return Ok(RequestCompletion::Late);
        }
        if self.requests.remove(&id) {
            return Ok(RequestCompletion::Completed);
        }
        Err(RpcError::UnknownRequest { id: id.get() })
    }

    /// Abandons a timed-out request.
    ///
    /// A queued request is simply removed because it was never sent. An
    /// in-flight request is remembered in the bounded abandoned table so its
    /// late response is discarded rather than treated as a protocol violation.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::UnknownRequest`] when the id is not pending.
    pub fn abandon_request(&mut self, id: RequestId) -> Result<(), RpcError> {
        if self.queued.iter().any(|queued| *queued == id) {
            self.queued.retain(|queued| *queued != id);
            return Ok(());
        }
        if !self.requests.remove(&id) {
            return Err(RpcError::UnknownRequest { id: id.get() });
        }
        if self.max_abandoned > 0 {
            while self.abandoned.len() >= self.max_abandoned {
                if self.abandoned.pop_first().is_none() {
                    break;
                }
            }
            self.abandoned.insert(id);
        }
        Ok(())
    }

    /// Registers an engine callback before it is dispatched.
    ///
    /// # Errors
    ///
    /// Rejects duplicates and exhaustion of the callback budget.
    pub fn begin_callback(&mut self, id: CallbackId) -> Result<(), RpcError> {
        if self.callbacks.contains(&id) {
            return Err(RpcError::DuplicateCallback);
        }
        if self.callbacks.len() >= self.max_callbacks {
            return Err(RpcError::CallbackCapacity {
                limit: self.max_callbacks,
            });
        }
        self.callbacks.insert(id);
        Ok(())
    }

    /// Removes a callback after its response has been written.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::UnknownCallback`] for an id that is not in flight.
    pub fn complete_callback(&mut self, id: &CallbackId) -> Result<(), RpcError> {
        if !self.callbacks.remove(id) {
            return Err(RpcError::UnknownCallback);
        }
        Ok(())
    }

    /// Returns the number of in-flight client requests.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.requests.len()
    }

    /// Returns the number of queued client requests.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// Returns the number of abandoned identifiers still tracked.
    #[must_use]
    pub fn abandoned_count(&self) -> usize {
        self.abandoned.len()
    }

    /// Returns the number of in-flight callbacks.
    #[must_use]
    pub fn active_callbacks(&self) -> usize {
        self.callbacks.len()
    }

    /// Returns whether no request or callback remains in flight or queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.queued.is_empty() && self.callbacks.is_empty()
    }

    /// Removes every pending request and returns the ids to fail.
    ///
    /// Queued requests are included because they were admitted but never sent.
    /// Callbacks are left in place so a dying session can still answer them
    /// with an error before it closes.
    pub fn drain_requests(&mut self) -> Vec<RequestId> {
        let mut ids: Vec<RequestId> = self.queued.drain(..).collect();
        ids.extend(self.requests.iter().copied());
        self.requests.clear();
        ids.sort_unstable();
        ids
    }

    /// Removes every in-flight callback and returns the ids to fail.
    pub fn drain_callbacks(&mut self) -> Vec<CallbackId> {
        std::mem::take(&mut self.callbacks).into_iter().collect()
    }

    /// Forgets every abandoned identifier.
    ///
    /// Call this when a new engine session starts: late responses from the old
    /// session can never arrive on the new connection, and tracking them would
    /// consume the bounded table.
    pub fn clear_abandoned(&mut self) {
        self.abandoned.clear();
    }

    /// Clears every table, including abandoned identifiers.
    pub fn clear(&mut self) {
        self.requests.clear();
        self.queued.clear();
        self.abandoned.clear();
        self.callbacks.clear();
    }
}

/// Identifier allocation plus routing tables for one connection.
#[derive(Debug)]
pub struct RpcDispatch {
    ids: RequestIdAllocator,
    pending: PendingRequests,
}

impl RpcDispatch {
    /// Creates dispatch state from the frozen budgets.
    #[must_use]
    pub const fn new(limits: FrameLimits) -> Self {
        Self {
            ids: RequestIdAllocator::new(),
            pending: PendingRequests::from_limits(limits),
        }
    }

    /// Allocates an id and admits the request as in-flight or queued.
    ///
    /// # Errors
    ///
    /// Fails when both the in-flight and queue budgets are exhausted, or when
    /// the allocator produced a duplicate (only possible after wraparound).
    pub fn allocate_request(&mut self) -> Result<RequestAdmission, RpcError> {
        let id = self.ids.allocate();
        let state = self.pending.begin_request(id)?;
        Ok(RequestAdmission::new(id, state))
    }

    /// Promotes the oldest queued request when a slot is free.
    #[must_use]
    pub fn promote_queued(&mut self) -> Option<RequestId> {
        self.pending.promote_queued()
    }

    /// Abandons a timed-out request.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::UnknownRequest`] when the id is not pending.
    pub fn abandon_request(&mut self, id: RequestId) -> Result<(), RpcError> {
        self.pending.abandon_request(id)
    }

    /// Validates a decoded message against the appropriate pending table.
    ///
    /// Callback requests are registered here and must be released with
    /// [`Self::finish_callback`] after the response is written. Recoverable
    /// callback problems are returned as [`RoutedMessage::RejectedCallback`];
    /// they never invalidate the session.
    ///
    /// # Errors
    ///
    /// Unknown or duplicate response ids and malformed envelopes are terminal.
    pub fn route(&mut self, inbound: DecodedInbound) -> Result<RoutedMessage, RpcError> {
        let message = match inbound {
            DecodedInbound::Rejected(rejected) => {
                return Ok(RoutedMessage::RejectedCallback(rejected));
            }
            DecodedInbound::Message(message) => message,
        };

        match message {
            InboundMessage::Response { id, result } => match self.pending.complete_request(id)? {
                RequestCompletion::Completed => Ok(RoutedMessage::Response { id, result }),
                RequestCompletion::Late => Ok(RoutedMessage::LateResponse { id }),
            },
            InboundMessage::ErrorResponse { id, error } => {
                match self.pending.complete_request(id)? {
                    RequestCompletion::Completed => Ok(RoutedMessage::ErrorResponse { id, error }),
                    RequestCompletion::Late => Ok(RoutedMessage::LateResponse { id }),
                }
            }
            InboundMessage::CallbackRequest { id, method, params } => {
                match self.pending.begin_callback(id.clone()) {
                    Ok(()) => Ok(RoutedMessage::Callback { id, method, params }),
                    Err(RpcError::DuplicateCallback) => Ok(RoutedMessage::RejectedCallback(
                        CallbackRejected::new(id, CallbackRejectKind::Duplicate),
                    )),
                    Err(RpcError::CallbackCapacity { .. }) => Ok(RoutedMessage::RejectedCallback(
                        CallbackRejected::new(id, CallbackRejectKind::Capacity),
                    )),
                    Err(other) => Err(other),
                }
            }
        }
    }

    /// Releases a callback after its response has been written.
    ///
    /// # Errors
    ///
    /// Returns [`RpcError::UnknownCallback`] if the id was not in flight.
    pub fn finish_callback(&mut self, id: &CallbackId) -> Result<(), RpcError> {
        self.pending.complete_callback(id)
    }

    /// Forgets every abandoned identifier, for example after a restart.
    pub fn clear_abandoned(&mut self) {
        self.pending.clear_abandoned();
    }

    /// Clears every routing table.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Returns the in-flight request count.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.pending.in_flight()
    }

    /// Returns the queued request count.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.pending.queued()
    }

    /// Returns the abandoned id count.
    #[must_use]
    pub fn abandoned_count(&self) -> usize {
        self.pending.abandoned_count()
    }

    /// Returns the in-flight callback count.
    #[must_use]
    pub fn active_callbacks(&self) -> usize {
        self.pending.active_callbacks()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::framing::DEFAULT_FRAME_LIMITS;
    use crate::native::protocol::{NativeMethod, NativeRequest};
    use serde_json::json;

    fn decode_message(bytes: &[u8], limits: FrameLimits) -> InboundMessage {
        match decode_inbound(bytes, limits).expect("decode") {
            DecodedInbound::Message(message) => message,
            DecodedInbound::Rejected(rejected) => {
                panic!("unexpected callback rejection: {rejected:?}")
            }
        }
    }

    fn message(inbound: InboundMessage) -> DecodedInbound {
        DecodedInbound::Message(inbound)
    }

    #[test]
    fn allocates_monotonic_ids_and_wraps_after_u64_max() {
        let mut allocator = RequestIdAllocator::new();
        assert_eq!(allocator.allocate(), RequestId::new(1));
        assert_eq!(allocator.allocate(), RequestId::new(2));
        assert_eq!(allocator.peek(), RequestId::new(3));

        let mut wrapping = RequestIdAllocator::starting_at(u64::MAX);
        assert_eq!(wrapping.allocate(), RequestId::new(u64::MAX));
        assert_eq!(wrapping.allocate(), RequestId::new(1));
        assert_eq!(wrapping.allocate(), RequestId::new(2));

        assert_eq!(RequestIdAllocator::starting_at(0).peek(), RequestId::new(1));
    }

    #[test]
    fn encodes_the_exact_request_envelope() {
        let request = NativeRequest::new(NativeMethod::Initialize, RawPayload::new(Value::Null));
        let bytes = encode_request(RequestId::new(7), &request).expect("encode");

        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert_eq!(value["method"], "initialize");
        assert!(value["params"].is_null());
        let object = value.as_object().expect("object");
        assert!(!object.contains_key("result"));
        assert!(!object.contains_key("error"));
    }

    #[test]
    fn classifies_callbacks_and_responses_separately() {
        let callback = br#"{"jsonrpc":"2.0","id":"api1","method":"readFile","params":"/x"}"#;
        assert_eq!(
            decode_message(callback, DEFAULT_FRAME_LIMITS),
            InboundMessage::CallbackRequest {
                id: CallbackId::String("api1".to_owned()),
                method: "readFile".to_owned(),
                params: Value::String("/x".to_owned()),
            }
        );

        let response = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        assert_eq!(
            decode_message(response, DEFAULT_FRAME_LIMITS),
            InboundMessage::Response {
                id: RequestId::new(1),
                result: Value::Null,
            }
        );

        let error =
            br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown API method"}}"#;
        match decode_message(error, DEFAULT_FRAME_LIMITS) {
            InboundMessage::ErrorResponse { id, error } => {
                assert_eq!(id, RequestId::new(1));
                assert_eq!(error.code(), -32601);
                assert!(error.is_method_not_found());
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn rejects_notifications_and_mixed_envelopes() {
        for payload in [
            br#"{"jsonrpc":"2.0","method":"log"}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":1,"method":"readFile","result":null}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":-1,"message":"x"}}"#
                .as_slice(),
            br#"{"jsonrpc":"2.0","id":1}"#.as_slice(),
            br#"{"jsonrpc":"2.0","result":null}"#.as_slice(),
            br#"{"id":1,"result":null}"#.as_slice(),
            br#"{"jsonrpc":"1.0","id":1,"result":null}"#.as_slice(),
            br"[1,2,3]".as_slice(),
            br#"{"jsonrpc":"2.0","id":-1,"result":null}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":1,"error":{"message":"no code"}}"#.as_slice(),
        ] {
            assert!(
                decode_inbound(payload, DEFAULT_FRAME_LIMITS).is_err(),
                "payload must be rejected: {}",
                String::from_utf8_lossy(payload)
            );
        }
    }

    #[test]
    fn rejects_invalid_utf8_and_json() {
        assert!(matches!(
            decode_inbound(b"\xff", DEFAULT_FRAME_LIMITS),
            Err(RpcError::Budget(JsonBudgetError::InvalidUtf8))
        ));
        assert!(matches!(
            decode_inbound(b"{not json", DEFAULT_FRAME_LIMITS),
            Err(RpcError::InvalidJson)
        ));
    }

    #[test]
    fn rejects_oversized_callbacks_without_losing_the_id() {
        let mut limits = DEFAULT_FRAME_LIMITS;
        limits.callback_bytes = 16;
        let callback = br#"{"jsonrpc":"2.0","id":"api1","method":"readFile","params":"/x"}"#;

        match decode_inbound(callback, limits).expect("decoded") {
            DecodedInbound::Rejected(rejected) => {
                assert_eq!(rejected.id(), &CallbackId::String("api1".to_owned()));
                assert_eq!(rejected.kind(), CallbackRejectKind::Oversized);
            }
            other @ DecodedInbound::Message(_) => {
                panic!("unexpected outcome: {other:?}")
            }
        }

        // A malformed method is also recoverable and keeps the id.
        let long_method = format!(
            r#"{{"jsonrpc":"2.0","id":"api1","method":"{}"}}"#,
            "m".repeat(MAX_METHOD_BYTES + 1)
        );
        match decode_inbound(long_method.as_bytes(), DEFAULT_FRAME_LIMITS).expect("decoded") {
            DecodedInbound::Rejected(rejected) => {
                assert_eq!(rejected.id(), &CallbackId::String("api1".to_owned()));
                assert_eq!(rejected.kind(), CallbackRejectKind::Malformed);
            }
            other @ DecodedInbound::Message(_) => {
                panic!("unexpected outcome: {other:?}")
            }
        }
    }

    #[test]
    fn invalid_callback_ids_are_terminal() {
        for payload in [
            br#"{"jsonrpc":"2.0","method":"readFile","params":"/x"}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":1.5,"method":"readFile","params":"/x"}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":true,"method":"readFile"}"#.as_slice(),
        ] {
            assert!(
                decode_inbound(payload, DEFAULT_FRAME_LIMITS).is_err(),
                "id-shape failure must be terminal"
            );
        }
    }

    #[test]
    fn preserves_callback_id_types_when_echoing() {
        let numeric = CallbackId::Number(7);
        let value = numeric.to_value();
        assert!(value.is_u64());
        assert_eq!(CallbackId::from_value(&value).expect("numeric id"), numeric);

        let string = CallbackId::String("api1".to_owned());
        assert_eq!(
            CallbackId::from_value(&string.to_value()).expect("string id"),
            string
        );

        for invalid in [json!(1.5), json!(-1), json!(true), json!("")] {
            assert!(CallbackId::from_value(&invalid).is_err());
        }

        let long = "x".repeat(MAX_CALLBACK_ID_BYTES + 1);
        assert!(CallbackId::from_value(&Value::String(long)).is_err());
    }

    #[test]
    fn encodes_callback_rejections_with_the_original_id() {
        let rejected = CallbackRejected::new(
            CallbackId::String("api1".to_owned()),
            CallbackRejectKind::Capacity,
        );
        let bytes = encode_callback_rejection(&rejected).expect("encode");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], "api1");
        assert_eq!(value["error"]["code"], -32600);
        assert!(value["error"]["message"].is_string());
    }

    #[test]
    fn bound_error_text_and_expose_only_the_code() {
        let long = "panic: ".to_owned() + &"a".repeat(10_000) + "\u{0}";
        let error = JsonRpcError::from_value(&json!({
            "code": -32603,
            "message": long,
            "data": {"stack": "ignored"},
        }))
        .expect("error object");

        assert_eq!(error.code(), -32603);
        assert!(error.is_internal());
        assert!(error.message().len() <= MAX_ERROR_MESSAGE_BYTES);
        assert!(!error.message().contains('\u{0}'));

        let native = error.to_native_error();
        assert!(matches!(native, NativeError::EngineError { code: -32603 }));
        assert!(!native.to_string().contains("panic"));
    }

    #[test]
    fn routes_server_callbacks_and_client_ids_separately() {
        let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);

        let client = dispatch.allocate_request().expect("client id");
        assert_eq!(client.id(), RequestId::new(1));
        assert_eq!(client.state(), RequestState::InFlight);

        // A numeric callback with the same value must not satisfy the client
        // request, and must not be confused with it.
        let routed = dispatch
            .route(message(InboundMessage::CallbackRequest {
                id: CallbackId::Number(1),
                method: "readFile".to_owned(),
                params: Value::String("/x".to_owned()),
            }))
            .expect("callback");
        assert!(matches!(routed, RoutedMessage::Callback { .. }));
        assert_eq!(dispatch.in_flight(), 1);
        assert_eq!(dispatch.active_callbacks(), 1);

        dispatch
            .finish_callback(&CallbackId::Number(1))
            .expect("finish callback");
        assert_eq!(dispatch.active_callbacks(), 0);
        assert_eq!(dispatch.in_flight(), 1);

        let routed = dispatch
            .route(message(InboundMessage::Response {
                id: client.id(),
                result: Value::Null,
            }))
            .expect("response");
        assert!(matches!(routed, RoutedMessage::Response { .. }));
        assert_eq!(dispatch.in_flight(), 0);
    }

    #[test]
    fn services_callbacks_while_request_is_pending() {
        let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);
        let request = dispatch.allocate_request().expect("client request");
        let callback = CallbackId::String("api1".to_owned());

        let routed = dispatch
            .route(message(InboundMessage::CallbackRequest {
                id: callback.clone(),
                method: "readFile".to_owned(),
                params: Value::String("/x".to_owned()),
            }))
            .expect("callback must be serviced");
        assert!(matches!(routed, RoutedMessage::Callback { .. }));
        assert_eq!(dispatch.in_flight(), 1);
        assert_eq!(dispatch.active_callbacks(), 1);

        dispatch.finish_callback(&callback).expect("callback done");
        assert_eq!(dispatch.active_callbacks(), 0);
        assert_eq!(dispatch.in_flight(), 1);

        dispatch
            .route(message(InboundMessage::Response {
                id: request.id(),
                result: Value::Null,
            }))
            .expect("request response still accepted");
        assert_eq!(dispatch.in_flight(), 0);
    }

    #[test]
    fn queues_requests_beyond_the_in_flight_budget() {
        let mut limits = DEFAULT_FRAME_LIMITS;
        limits.in_flight_requests = 2;
        limits.pending_queue = 3;
        let mut dispatch = RpcDispatch::new(limits);

        let first = dispatch.allocate_request().expect("first");
        let second = dispatch.allocate_request().expect("second");
        assert_eq!(first.state(), RequestState::InFlight);
        assert_eq!(second.state(), RequestState::InFlight);

        let third = dispatch.allocate_request().expect("queued");
        let fourth = dispatch.allocate_request().expect("queued");
        let fifth = dispatch.allocate_request().expect("queued");
        assert!(third.is_queued() && fourth.is_queued() && fifth.is_queued());
        assert_eq!(dispatch.in_flight(), 2);
        assert_eq!(dispatch.queued(), 3);

        assert!(matches!(
            dispatch.allocate_request(),
            Err(RpcError::QueueCapacity { limit: 3 })
        ));

        dispatch
            .route(message(InboundMessage::Response {
                id: first.id(),
                result: Value::Null,
            }))
            .expect("complete first");
        assert_eq!(
            dispatch.promote_queued(),
            Some(third.id()),
            "FIFO promotion"
        );
        assert_eq!(dispatch.in_flight(), 2);
        assert_eq!(dispatch.queued(), 2);
    }

    #[test]
    fn default_budgets_reject_request_seventy_three() {
        let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);
        for _ in 0..8 {
            assert_eq!(
                dispatch.allocate_request().expect("in flight").state(),
                RequestState::InFlight
            );
        }
        for _ in 0..64 {
            assert!(dispatch.allocate_request().expect("queued").is_queued());
        }
        assert_eq!(dispatch.in_flight(), 8);
        assert_eq!(dispatch.queued(), 64);
        assert!(matches!(
            dispatch.allocate_request(),
            Err(RpcError::QueueCapacity { limit: 64 })
        ));
    }

    #[test]
    fn abandons_requests_and_discards_late_responses() {
        let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);
        let request = dispatch.allocate_request().expect("request");
        assert_eq!(dispatch.in_flight(), 1);

        dispatch.abandon_request(request.id()).expect("abandon");
        assert_eq!(dispatch.in_flight(), 0);
        assert_eq!(dispatch.abandoned_count(), 1);

        let routed = dispatch
            .route(message(InboundMessage::Response {
                id: request.id(),
                result: Value::Null,
            }))
            .expect("late response is not an error");
        assert_eq!(routed, RoutedMessage::LateResponse { id: request.id() });
        assert_eq!(dispatch.abandoned_count(), 0);
        assert_eq!(dispatch.in_flight(), 0);

        // The session remains usable after the late response.
        let next = dispatch.allocate_request().expect("next request");
        assert_eq!(next.state(), RequestState::InFlight);
    }

    #[test]
    fn abandon_is_bounded_and_clearable_on_restart() {
        let mut limits = DEFAULT_FRAME_LIMITS;
        limits.in_flight_requests = 1;
        limits.pending_queue = 2;
        let mut dispatch = RpcDispatch::new(limits);

        for _ in 0..2 {
            let request = dispatch.allocate_request().expect("request");
            dispatch.abandon_request(request.id()).expect("abandon");
        }
        // The cap is 2, so at most two abandoned ids are tracked.
        assert!(dispatch.abandoned_count() <= 2);

        dispatch.clear_abandoned();
        assert_eq!(dispatch.abandoned_count(), 0);
    }

    #[test]
    fn rejects_unknown_and_duplicate_response_ids() {
        let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);

        assert!(matches!(
            dispatch.route(message(InboundMessage::Response {
                id: RequestId::new(9),
                result: Value::Null,
            })),
            Err(RpcError::UnknownRequest { .. })
        ));

        let client = dispatch.allocate_request().expect("client id");
        dispatch
            .route(message(InboundMessage::Response {
                id: client.id(),
                result: Value::Null,
            }))
            .expect("first response");
        assert!(matches!(
            dispatch.route(message(InboundMessage::Response {
                id: client.id(),
                result: Value::Null,
            })),
            Err(RpcError::UnknownRequest { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_and_over_capacity_callbacks_without_ending_the_session() {
        let mut limits = DEFAULT_FRAME_LIMITS;
        limits.in_flight_callbacks = 1;
        let mut dispatch = RpcDispatch::new(limits);

        let first = CallbackId::String("api1".to_owned());
        assert!(matches!(
            dispatch
                .route(message(InboundMessage::CallbackRequest {
                    id: first.clone(),
                    method: "readFile".to_owned(),
                    params: Value::Null,
                }))
                .expect("first callback"),
            RoutedMessage::Callback { .. }
        ));

        let duplicate = dispatch
            .route(message(InboundMessage::CallbackRequest {
                id: first.clone(),
                method: "readFile".to_owned(),
                params: Value::Null,
            }))
            .expect("duplicate is recoverable");
        match duplicate {
            RoutedMessage::RejectedCallback(rejected) => {
                assert_eq!(rejected.id(), &first);
                assert_eq!(rejected.kind(), CallbackRejectKind::Duplicate);
            }
            other => panic!("unexpected route: {other:?}"),
        }

        let second = CallbackId::String("api2".to_owned());
        let capacity = dispatch
            .route(message(InboundMessage::CallbackRequest {
                id: second.clone(),
                method: "readFile".to_owned(),
                params: Value::Null,
            }))
            .expect("capacity is recoverable");
        match capacity {
            RoutedMessage::RejectedCallback(rejected) => {
                assert_eq!(rejected.id(), &second);
                assert_eq!(rejected.kind(), CallbackRejectKind::Capacity);
            }
            other => panic!("unexpected route: {other:?}"),
        }

        assert_eq!(dispatch.active_callbacks(), 1);
        dispatch
            .finish_callback(&first)
            .expect("callback still tracked");
        assert_eq!(dispatch.active_callbacks(), 0);
        assert!(matches!(
            dispatch.finish_callback(&CallbackId::String("missing".to_owned())),
            Err(RpcError::UnknownCallback)
        ));
    }

    #[test]
    fn passes_through_decoder_rejections() {
        let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);
        let rejected = CallbackRejected::new(
            CallbackId::String("api9".to_owned()),
            CallbackRejectKind::Oversized,
        );
        let routed = dispatch
            .route(DecodedInbound::Rejected(rejected.clone()))
            .expect("rejection is not a route error");
        assert_eq!(routed, RoutedMessage::RejectedCallback(rejected));
        assert_eq!(dispatch.active_callbacks(), 0);
    }

    #[test]
    fn drained_requests_and_callbacks_are_failed_together() {
        let mut pending = PendingRequests::new(2, 2, 4);
        pending.begin_request(RequestId::new(3)).expect("request");
        pending.begin_request(RequestId::new(1)).expect("request");
        pending.begin_request(RequestId::new(5)).expect("queued");
        pending
            .begin_callback(CallbackId::String("api1".to_owned()))
            .expect("callback");

        assert_eq!(
            pending.drain_requests(),
            vec![RequestId::new(1), RequestId::new(3), RequestId::new(5)]
        );
        assert_eq!(pending.in_flight(), 0);
        assert_eq!(pending.queued(), 0);
        assert_eq!(
            pending.drain_callbacks(),
            vec![CallbackId::String("api1".to_owned())]
        );
        assert_eq!(pending.active_callbacks(), 0);
        assert!(pending.is_empty());
    }

    #[test]
    fn queued_requests_are_abandoned_without_engine_traffic() {
        let mut limits = DEFAULT_FRAME_LIMITS;
        limits.in_flight_requests = 1;
        limits.pending_queue = 4;
        let mut dispatch = RpcDispatch::new(limits);
        let in_flight = dispatch.allocate_request().expect("in flight");
        let queued = dispatch.allocate_request().expect("queued");
        assert!(queued.is_queued());

        dispatch
            .abandon_request(queued.id())
            .expect("abandon queued");
        assert_eq!(dispatch.queued(), 0);
        assert_eq!(dispatch.abandoned_count(), 0, "never sent, never late");

        // Abandoning the in-flight request still frees its slot.
        dispatch
            .abandon_request(in_flight.id())
            .expect("abandon in flight");
        assert_eq!(dispatch.in_flight(), 0);
        assert_eq!(dispatch.abandoned_count(), 1);
    }
}
