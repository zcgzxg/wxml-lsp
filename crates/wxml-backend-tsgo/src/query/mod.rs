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
