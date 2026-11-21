//! Semantic tools: references, rename, code actions, tokens, symbols, inlay
//! hints, signature help, code lens and the read-only `wxml.references`
//! command.
//!
//! The package consumes facts produced by P/C/E/N/T and never creates a second
//! URI, WXML parser, component JSON parser or WXS parser.

pub(crate) mod edit_plan;

/// Runtime switches for the semantic handlers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SemanticToolsConfig {
    /// Whether `textDocument/inlayHint` returns hints.
    pub(crate) inlay_hints: bool,
    /// Whether `textDocument/codeLens` returns lenses.
    pub(crate) code_lens: bool,
}

impl Default for SemanticToolsConfig {
    fn default() -> Self {
        Self {
            inlay_hints: true,
            code_lens: true,
        }
    }
}
pub(crate) mod event_definition;
pub(crate) mod event_diagnostics;
pub(crate) mod facts;
pub(crate) mod handlers;
pub(crate) mod hints;
pub(crate) mod position;
pub(crate) mod target;
pub(crate) mod tokens;
pub(crate) mod tsconfig_paths;
