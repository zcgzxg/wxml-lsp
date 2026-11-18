//! F0-08 framing and session budgets plus frame roles.
//!
//! `P3-N` implements the incremental parser and the router. This module only
//! freezes the budget table so the future parser, router and tests share one
//! source of truth.

use crate::config::{
    DEFAULT_REQUEST_DEADLINE_MS, MAX_AST_NODES, MAX_AST_STRINGS, MAX_CALLBACK_BODY_BYTES,
    MAX_FRAME_HEADER_BYTES, MAX_IN_FLIGHT_CALLBACKS, MAX_IN_FLIGHT_REQUESTS, MAX_JSON_DEPTH,
    MAX_MESSAGE_ENTRIES, MAX_PENDING_QUEUE, MAX_REQUEST_BODY_BYTES, MAX_RESPONSE_BODY_BYTES,
    MAX_STRING_BYTES, SNAPSHOT_DEADLINE_MS,
};

/// F0-08 budgets for one native connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    /// Maximum bytes before the terminating empty header line.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
