//! LSP facade for the WXML language server.
//!
//! Owns transport, document state and request routing. Parsing, project
//! discovery and type analysis live in the sibling crates; this crate only
//! adapts their results to the Language Server Protocol.
//!
//! The stdio server loop and request handlers land in phase 2 of the plan.

/// Returns the version reported by `wxml-lsp --version`.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
