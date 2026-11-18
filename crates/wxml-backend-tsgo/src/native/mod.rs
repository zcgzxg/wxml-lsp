//! Native transport and supervision substrate as defined by F0/F1.
//!
//! `P3-N`/`P4-N` implement the native client substrate:
//!
//! - [`framing`]: incremental `Content-Length` parser/writer and the frozen
//!   frame/JSON budgets;
//! - [`rpc`]: JSON-RPC 2.0 envelopes, request-id allocation, callback-id
//!   preservation and separate routing tables;
//! - [`protocol`]: the typed method allow-list and handshake/snapshot DTOs;
//! - [`supervisor`]: exact argv construction, piped process spawn, bounded
//!   `stderr`, deadlines and kill-on-drop/explicit shutdown;
//! - [`session`]: real initialize/schema handshake, virtual snapshots,
//!   callback dispatch and the query operations;
//! - [`vfs`]: a read-only in-memory filesystem that never falls back to the
//!   host;
//! - [`handles`], [`ast`], [`strings`] and [`wire`]: epoch-bound node handles,
//!   v9 AST header validation, UTF-16/UTF-8 conversion and wire DTOs.
//!
//! No native engine artifact is downloaded by this crate's tests; the
//! real-engine gate runs when `TSGO_BIN` points at the pinned executable, and
//! `WXML_REQUIRE_REAL_ENGINE=1` makes a missing engine a hard failure.
//! The backend is not wired into `wxml-lsp` and no virtual file service is
//! exposed to a server yet.

pub mod ast;
pub mod framing;
pub mod handles;
pub mod protocol;
pub mod rpc;
pub mod session;
pub mod strings;
pub mod supervisor;
pub mod vfs;
pub mod wire;

use thiserror::Error;

use crate::native::framing::FrameError;
use crate::native::protocol::ShapeError;
use crate::native::rpc::RpcError;

/// Why the native engine is unavailable.
///
/// The categories are intentionally distinct so startup failures can be
/// audited without leaking engine output or collapsing them into one error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableReason {
    /// The executable or a pinned asset is missing or mismatched.
    ArtifactMismatch,
    /// A required API method is missing from the running engine.
    ApiMismatch,
    /// The initialize handshake returned an incompatible shape.
    HandshakeFailed,
    /// The child process exited or could not be spawned.
    ProcessExited,
    /// The session is poisoned after a timeout and must be restarted.
    SessionPoisoned,
    /// A required standard-library or typings asset is missing.
    MissingAsset,
    /// The startup virtual-filesystem canary self-check failed.
    VfsConfinement,
}

impl UnavailableReason {
    /// Returns a stable, user-safe description.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArtifactMismatch => "engine artifact mismatch",
            Self::ApiMismatch => "engine API mismatch",
            Self::HandshakeFailed => "engine handshake failed",
            Self::ProcessExited => "engine process exited",
            Self::SessionPoisoned => "native session is poisoned",
            Self::MissingAsset => "engine asset missing",
            Self::VfsConfinement => "virtual filesystem confinement check failed",
        }
    }
}

impl std::fmt::Display for UnavailableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Failures at the native boundary as defined by F0/F1.
#[derive(Debug, Error)]
pub enum NativeError {
    /// The engine is missing, mismatched or not currently running.
    #[error("native engine is unavailable: {reason}")]
    Unavailable {
        /// Category that separates artifact, API, handshake, process and
        /// asset failures.
        reason: UnavailableReason,
    },
    /// A framing, JSON-RPC or AST invariant was violated.
    #[error("native protocol violation: {0}")]
    Protocol(String),
    /// A request deadline elapsed; the response must be discarded.
    #[error("native request timed out after {0} ms")]
    Timeout(u64),
    /// A handle or lease no longer belongs to the current epoch.
    #[error("stale native handle or lease")]
    StaleHandle,
    /// A result was produced for a snapshot that is no longer current.
    #[error("stale native snapshot response")]
    StaleSnapshot,
    /// No native snapshot is open.
    #[error("no native snapshot is open")]
    NoSnapshot,
    /// A node handle is malformed or outside the current project.
    #[error("invalid native node handle: {reason}")]
    InvalidHandle {
        /// Static rejection reason.
        reason: &'static str,
    },
    /// A request position, path or revision is invalid.
    #[error("invalid native request: {reason}")]
    InvalidParams {
        /// Static rejection reason.
        reason: &'static str,
    },
    /// A serialized snapshot request would exceed the frozen request budget.
    #[error("native input is too large: {bytes} bytes exceeds the {limit}-byte request budget")]
    InputTooLarge {
        /// Conservative upper bound of the serialized request body.
        bytes: u64,
        /// Frozen request budget.
        limit: usize,
    },
    /// The method is not available in the running engine or this slice.
    #[error("native method is not available in this session: {method}")]
    Unsupported {
        /// Static method or capability name.
        method: &'static str,
    },
    /// The method is not on the typed allow-list.
    #[error("native method is not allowlisted: {0}")]
    MethodNotAllowed(String),
    /// The engine answered with a JSON-RPC error.
    ///
    /// Only the numeric code is retained; the engine message may contain a Go
    /// panic stack or absolute paths and is never exposed through this error.
    #[error("native engine error (code {code})")]
    EngineError {
        /// JSON-RPC error code, for example `-32603`.
        code: i64,
    },
    /// Capability reserved for `P3-N`/`P4`.
    #[error("native capability is not implemented: {feature}")]
    Unimplemented {
        /// Capability marker supplied by the caller.
        feature: &'static str,
    },
}

impl From<FrameError> for NativeError {
    fn from(error: FrameError) -> Self {
        Self::Protocol(error.to_string())
    }
}

impl From<RpcError> for NativeError {
    fn from(error: RpcError) -> Self {
        Self::Protocol(error.to_string())
    }
}

impl From<ShapeError> for NativeError {
    fn from(error: ShapeError) -> Self {
        error.into_native_error()
    }
}
