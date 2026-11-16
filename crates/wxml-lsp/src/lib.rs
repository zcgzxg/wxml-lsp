//! LSP facade for the WXML language server.
//!
//! Owns transport, document state and request routing. Parsing, project
//! discovery and type analysis live in the sibling crates; this crate only
//! adapts their results to the Language Server Protocol.
//!
//! The current skeleton serves `initialize`, `initialized`, `shutdown` and
//! `exit`, text document synchronization, `textDocument/hover`, optional
//! `textDocument/definition`, push diagnostics and `$/cancelRequest` over
//! stdio. Backend work runs on one background thread with revision barriers,
//! cancellation and bounded queues. Project discovery and the real Node
//! backend land in later waves; [`run_stdio`] is generic over
//! [`wxml_types::Backend`] so they can be plugged in without changing the
//! protocol layer.
//!
//! Rejected `didChange` notifications keep the previous document revision and
//! send a `window/logMessage` (type Error) asking the client to resynchronize
//! with a full-text change.
//!
//! Request-level backend timeouts are not implemented in this wave: the
//! synchronous [`wxml_types::Backend`] trait cannot be interrupted safely
//! without a thread per call. They are scheduled with the C1 protocol and the
//! real P5 backend; until then a blocked backend stalls hover/diagnostics,
//! while `shutdown` still detaches after two seconds.
//!
//! Backend-supplied capability reasons and diagnostic messages are filtered
//! for control/format characters and length at the protocol edge. Removing
//! absolute paths and other private details is the backend's responsibility
//! (C1/P5); this crate cannot reliably recognise them.
//!
//! `Backend::epoch` is expected to be O(1) and stable unless the backend
//! instance is replaced; the protocol reads it at most once per scheduling or
//! publishing event. See `Server::refresh_epoch`.
//!
//! Memory budgets: the 64 MiB open-document budget counts text plus eight
//! bytes per line start, an estimate rather than an RSS measurement. In-flight
//! jobs pin old snapshots outside that budget; the scheduler queue bounds that
//! extra memory separately.

// The crate has no public module tree: everything below `lib.rs` is internal,
// so `pub(crate)` is the accurate visibility and `pub` would be unreachable.
#![allow(clippy::redundant_pub_crate)]

pub mod backend_node;
pub(crate) mod completion;
pub(crate) mod handlers;
pub(crate) mod jsonrpc;
pub mod project_host;
pub(crate) mod scheduler;
pub(crate) mod server;
pub(crate) mod workspace;

pub use server::{ServerError, ServerOutcome, run_stdio, run_stdio_with_host, run_with_host};

/// Returns the version reported by `wxml-lsp --version`.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
