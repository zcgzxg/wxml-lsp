//! Native `tsgo` backend foundations for the WXML language server.
//!
//! The crate owns the pinned engine identity, the verified native process
//! supervisor, the input/provider seam, the script overlay and the shadow
//! mapping. It has no Node dependency and no fallback path to Node.
//!
//! The supervisor verifies a pinned executable's SHA-256, spawns it with piped
//! stdio and manages its lifecycle; the session layer owns
//! initialize/snapshot/update/release plus a read-only virtual filesystem.
//! The `wxml-lsp` adapter (`backend_tsgo`) constructs these native sessions
//! and owns the `wxml_types::Backend` implementation, the query presentation
//! helpers and every server-facing conversion. Nothing in this crate
//! implements `wxml_types::Backend` and nothing here serves LSP requests.
//!
//! Layering: `wxml-lsp -> wxml-backend-tsgo -> wxml-types`. This crate does
//! not depend on `wxml-lsp` or any LSP transport crate. It reuses shared value
//! types (`CapabilityStatus`, completion and document-symbol kinds) from
//! `wxml-types`, while request/result DTO conversion stays in the adapter.

pub mod asset_loader;
pub mod backend;
pub mod config;
pub mod input;
pub mod native;
pub mod query;
pub mod script;
pub mod shadow;

pub use backend::BackendError;
pub use config::{
    AST_HEADER_BYTES, AST_PROTOCOL_VERSION, ENGINE_VERSION, LaunchError, PinnedEngine,
};
pub use input::{
    AnalysisInput, DocumentRevision, InputError, InputProvider, ProjectKey, SnapshotRevision,
    SourceFile,
};
