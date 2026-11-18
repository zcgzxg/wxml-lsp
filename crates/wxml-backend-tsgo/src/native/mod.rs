//! Failures at the native boundary as defined by F0/F1.
//!
//! All modules are declarations for `P3-N`/`P4`. No process is started, no
//! byte is written to a pipe and no virtual file is served.

pub mod ast;
pub mod framing;
pub mod handles;
pub mod protocol;
pub mod rpc;
pub mod strings;
pub mod supervisor;
pub mod vfs;

use thiserror::Error;

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
    /// A required standard-library or typings asset is missing.
    MissingAsset,
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
            Self::MissingAsset => "engine asset missing",
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
    /// The method is not on the typed allow-list.
    #[error("native method is not allowlisted: {0}")]
    MethodNotAllowed(String),
    /// Capability reserved for `P3-N`/`P4`.
    #[error("native capability is not implemented: {feature}")]
    Unimplemented {
        /// Capability marker supplied by the caller.
        feature: &'static str,
    },
}
