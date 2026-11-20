//! Query presentation seams for hover, diagnostics, definition and completion.
//!
//! These DTOs are internal to the `wxml-lsp` native adapter, which converts
//! them to C0 results. Every result carries the shared
//! [`wxml_types::CapabilityStatus`] so no-symbol, partial, unsupported,
//! unavailable and failure states stay distinct. Nothing here calls a native
//! method or publishes a result.
//!
//! Message sanitization lives at the protocol edge
//! (`handlers::sanitized_message`); probe-health decisions were removed with
//! the unwired contract and are recoverable from git history.

// `hover`, `documentation`, `type_info` and `completion` options are also
// composed by the native session in `wxml-backend-tsgo`; keep one definition
// there and re-export it here instead of carrying a copy.
pub(crate) use wxml_backend_tsgo::query::{
    completion, definition, diagnostics, document_symbol, hover, type_info,
};
