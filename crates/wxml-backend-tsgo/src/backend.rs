//! Error seam for the native backend.
//!
//! The crate no longer ships a backend shell: the LSP adapter in `wxml-lsp`
//! owns `wxml_types::Backend` and every request path. This module only keeps
//! the error type that the input seam can surface.

use thiserror::Error;

use crate::input::InputError;

/// Errors exposed by the native backend seams.
#[derive(Debug, Error)]
pub enum BackendError {
    /// The requested capability is scaffolding only.
    #[error("tsgo backend capability is not implemented: {feature}")]
    Unimplemented {
        /// Capability marker supplied by the caller.
        feature: &'static str,
    },
    /// Input collection failed before any native work.
    #[error(transparent)]
    Input(#[from] InputError),
}
