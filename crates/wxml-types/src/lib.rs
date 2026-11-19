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
//! - [`document_symbols`]: per-document outline DTOs, separate from script
//!   symbol facts.
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
pub mod document_symbols;
pub mod mock;
pub mod status;
pub mod symbols;
pub mod template;
pub mod template_semantics;
pub mod wxs;

pub use backend::{
    Backend, BackendCapabilities, BackendError, CompletionItem, CompletionItemKind, CompletionKind,
    CompletionRequest, CompletionResult, DefinitionRequest, DefinitionResult, Diagnostic,
    DiagnosticSeverity, DiagnosticsRequest, DiagnosticsResult, HoverRequest, HoverResult,
    InsertTextFormat, MAX_COMPLETION_ITEMS, MarkupKind, SourceLocation, TypeQuality,
};
pub use check::{
    AttributeValueKind, CheckFragment, ComponentAttribute, ComponentUsage, EventBindingKind,
    EventHandler, GuardBranch, GuardChain, GuardCondition, LocalBinding, LocalRole, RootKind,
    TemplateAnalysis, UnsupportedFeature, UnsupportedKind,
};
pub use context::{ContextError, RequestContext, SourceSnapshot};
pub use contract::{
    BackendEpoch, BackendId, ClientVersion, FileId, FileIdError, ProjectId, ProjectRevision,
    SnapshotId, SourceRevision, ViewRevision,
};
pub use document_symbols::{
    DocumentSymbol, DocumentSymbolGap, DocumentSymbolKind, DocumentSymbolsRequest,
    DocumentSymbolsResult, MAX_DOCUMENT_SYMBOL_DEPTH, MAX_DOCUMENT_SYMBOL_GAPS,
    MAX_DOCUMENT_SYMBOLS,
};
pub use mock::{BackendCall, MockBackend};
pub use status::CapabilityStatus;
pub use symbols::{
    HandlerInsertion, MAX_SYMBOL_ENTRIES, MAX_SYMBOL_NAME_BYTES, MAX_SYMBOL_PATH, SymbolError,
    SymbolField, SymbolOccurrence, SymbolOccurrenceKind, SymbolQuery, SymbolReference,
    SymbolReferenceRole, SymbolRequest, SymbolResult, SymbolToken,
};
pub use template::{
    MAX_BINDINGS, MAX_COMPONENTS, MAX_FRAGMENTS, MAX_GUARD_CHAINS, MAX_GUARD_CONDITIONS,
    TemplateAnalyzer, analyze_template,
};
pub use template_semantics::{
    StaticImport, StaticImportKind, StaticIssue, StaticIssueKind, TemplateCall, TemplateData,
    TemplateDataKey, TemplateDataUse, TemplateDefinition, TemplateSemantics, WxsBlock,
    analyze_template_semantics, resolve_static_specifier, summarize_wxs_block,
};
pub use wxml_text::SourceRange;
pub use wxs::{WxsMember, WxsMemberKind, WxsRequire, WxsSummary, summarize_wxs};
