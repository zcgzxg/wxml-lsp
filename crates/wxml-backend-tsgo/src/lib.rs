//! Native `tsgo` backend foundations for the WXML language server.
//!
//! The crate freezes the module boundaries, pinned engine identity,
//! input/provider seam and query DTO placeholders that `P3-N`, `P3-S` and `P4`
//! fill in. It has no Node dependency and no fallback path to Node. The
//! `P3-N` supervisor can verify a pinned executable's SHA-256, spawn it with
//! piped stdio and manage its lifecycle, and the `P4-N` session layer owns
//! initialize/snapshot/update/release plus a read-only virtual filesystem.
//! The backend is still inert because nothing constructs a native session from
//! `wxml-lsp`, no engine artifact is downloaded, and no server-facing file
//! service is implemented.
//!
//! `wxml-lsp` only declares an inert adapter module for this crate; the
//! default server configuration still selects the existing Node backend.
//! Nothing in this crate implements or replaces `wxml_types::Backend`, and
//! there is no fallback path to Node.
//!
//! Layering: `wxml-lsp -> wxml-backend-tsgo -> wxml-types`. This crate does
//! not depend on `wxml-lsp` or any LSP transport crate. It reuses the shared
//! `CapabilityStatus` and completion cap from `wxml-types`, but still does not
//! implement `wxml_types::Backend`; the future adapter owns the conversion to
//! C0 request/result DTOs.

pub mod asset_loader;
pub mod backend;
pub mod config;
pub mod input;
pub mod native;
pub mod query;
pub mod script;
pub mod shadow;

pub use backend::{BackendError, BackendState, ResultCache, TsgoBackend};
pub use config::{
    AST_HEADER_BYTES, AST_PROTOCOL_VERSION, ENGINE_VERSION, LaunchError, PinnedEngine,
};
pub use input::{
    AnalysisInput, DocumentRevision, InputError, InputProvider, ProjectKey, SnapshotRevision,
    SourceFile,
};
