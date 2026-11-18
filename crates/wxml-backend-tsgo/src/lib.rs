//! Native `tsgo` backend scaffold for the WXML language server.
//!
//! This crate is deliberately non-functional. It freezes the module
//! boundaries, pinned engine identity, input/provider seam and query DTO
//! placeholders that `P3-N`, `P3-S` and `P4` will fill in. It has no Node
//! dependency, never starts a process, never opens a socket and never reads
//! the file system.
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

pub mod backend;
pub mod config;
pub mod input;
pub mod native;
pub mod query;
pub mod shadow;

pub use backend::{BackendError, BackendState, ResultCache, TsgoBackend};
pub use config::{
    AST_HEADER_BYTES, AST_PROTOCOL_VERSION, ENGINE_VERSION, LaunchError, PinnedEngine,
};
pub use input::{
    AnalysisInput, DocumentRevision, InputError, InputProvider, ProjectKey, SnapshotRevision,
    SourceFile,
};
