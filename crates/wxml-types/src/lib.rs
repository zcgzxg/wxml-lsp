//! Template type analysis with a pluggable backend.
//!
//! Converts parsed templates plus project context into an in-memory check
//! view, asks a type backend for hovers, diagnostics and definitions, and maps
//! the results back to WXML byte ranges. The backend trait keeps the Node +
//! TypeScript Language Service implementation replaceable.
//!
//! C0 contracts frozen in this crate:
//!
//! - [`contract`]: project, backend and file identities plus revision counters.
//! - [`context`]: immutable source snapshots and the versioned request context.
//! - [`status`]: six-state capability reporting.
//! - [`backend`]: requests, results and the synchronous backend boundary.
//! - [`mock`]: an in-process backend for parallel work and tests.
//! - [`check`]: C1.1 template check DTOs.
//! - [`template`]: `wx:for`/event/mustache analysis that produces check
//!   fragments, bounded by [`MAX_FRAGMENTS`].
//!
//! The real Node backend lands in a later phase.

pub mod backend;
pub mod check;
pub mod context;
pub mod contract;
pub mod mock;
pub mod status;
pub mod template;

pub use backend::{
    Backend, BackendCapabilities, BackendError, DefinitionRequest, DefinitionResult, Diagnostic,
    DiagnosticSeverity, DiagnosticsRequest, DiagnosticsResult, HoverRequest, HoverResult,
    MarkupKind, SourceLocation, TypeQuality,
};
pub use check::{
    CheckFragment, EventBindingKind, EventHandler, RootKind, TemplateAnalysis, UnsupportedFeature,
    UnsupportedKind,
};
pub use context::{ContextError, RequestContext, SourceSnapshot};
pub use contract::{
    BackendEpoch, BackendId, ClientVersion, FileId, FileIdError, ProjectId, ProjectRevision,
    SnapshotId, SourceRevision, ViewRevision,
};
pub use mock::{BackendCall, MockBackend};
pub use status::CapabilityStatus;
pub use template::{MAX_FRAGMENTS, TemplateAnalyzer, analyze_template};
