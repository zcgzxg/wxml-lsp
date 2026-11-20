//! Query presentation seams and pure decision helpers retained by the native
//! session layer.
//!
//! `wxml-lsp` owns the LSP-facing presentation modules. This crate keeps the
//! modules that `native/session` itself composes while executing native
//! requests, so the session does not depend on the adapter crate.
//!
//! Every result carries the shared [`wxml_types::CapabilityStatus`] so
//! no-symbol, partial, unsupported, unavailable and failure states stay
//! distinct. Nothing here calls a native method or publishes a result.

pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod document_symbol;
pub mod documentation;
pub mod hover;
pub mod references;
pub mod symbol;
pub mod type_info;

use thiserror::Error;

/// Query failure shared by the presentation modules.
#[derive(Debug, Error)]
pub enum QueryError {
    /// Evidence was truncated or could not be mapped; callers must not edit.
    #[error("query result is partial: {reason}")]
    Partial {
        /// Machine-readable reason marker.
        reason: &'static str,
    },
    /// Capability reserved for `P3-Q`/`P4`.
    #[error("query capability is not implemented: {feature}")]
    Unimplemented {
        /// Capability marker supplied by the caller.
        feature: &'static str,
    },
    /// Native session failure surfaced unchanged for the adapter to classify.
    #[error(transparent)]
    Native(#[from] crate::native::NativeError),
}
