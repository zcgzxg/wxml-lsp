//! Query presentation seams and pure decision helpers for hover, diagnostics,
//! definition, completion, binding facts, message cleaning and probe health.
//!
//! These DTOs are internal to the `wxml-lsp` native adapter, which converts
//! them to C0 results. Every result carries the shared
//! [`wxml_types::CapabilityStatus`] so no-symbol, partial, unsupported,
//! unavailable and failure states stay distinct. Nothing here calls a native
//! method or publishes a result.

pub(crate) mod bindings;
pub(crate) mod message;
pub(crate) mod probe_health;

// These modules are also composed by the native session in `wxml-backend-tsgo`;
// keep one definition there and re-export it here instead of carrying a copy.
pub(crate) use wxml_backend_tsgo::query::{
    completion, definition, diagnostics, document_symbol, documentation, hover, type_info,
};

use thiserror::Error;

/// Query failure shared by the presentation modules.
#[derive(Debug, Error)]
pub(crate) enum QueryError {
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
    Native(#[from] wxml_backend_tsgo::native::NativeError),
}
