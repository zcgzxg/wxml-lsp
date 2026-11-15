//! Backend request/response boundary shared by every type backend.
//!
//! Requests carry a [`RequestContext`] plus a UTF-8 byte offset. Ranges in
//! results are [`SourceRange`] values in the original file; the LSP layer is
//! responsible for converting to UTF-16 positions at the protocol edge.
//!
//! Status and failure are different channels:
//!
//! - [`CapabilityStatus`] says how complete or available a check is.
//! - [`BackendError`] says the request itself failed.
//!
//! An empty [`DiagnosticsResult::diagnostics`] only means "clean" when
//! [`CapabilityStatus::Supported`] is reported. An empty
//! [`DefinitionResult::locations`] only means "checked, no definition" when
//! the status is `Supported`; `Unsupported`/`Unavailable` means the backend
//! could not answer.

use thiserror::Error;
use wxml_text::SourceRange;

use crate::context::RequestContext;
use crate::contract::{BackendEpoch, FileId};
use crate::status::CapabilityStatus;

/// A failure reported by a type backend.
///
/// This describes the request, not the quality of a successful check. A
/// backend that cannot run reports `Unavailable`; a malformed private-protocol
/// message reports `Protocol`. `Protocol` and `Internal` messages are meant for
/// logs and diagnostics, not for direct display to end users.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BackendError {
    /// The backend is not running or not ready yet.
    #[error("type backend unavailable: {0}")]
    Unavailable(String),
    /// The backend violated the private protocol.
    #[error("type backend protocol error: {0}")]
    Protocol(String),
    /// The request exceeded its deadline.
    #[error("type backend request timed out")]
    Timeout,
    /// The request was cancelled before it completed.
    #[error("type backend request cancelled")]
    Cancelled,
    /// The backend failed for another reason.
    #[error("type backend internal error: {0}")]
    Internal(String),
}

/// The markup language of hover contents.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MarkupKind {
    /// Markdown, the default for LSP hover text.
    #[default]
    Markdown,
    /// Plain text with no markup.
    PlainText,
}

/// How much trust the hover type deserves.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TypeQuality {
    /// A concrete, declared type.
    Known,
    /// An explicit `any`; members are not checked.
    Any,
    /// The type could not be established (`unknown`, unanalysed, missing root).
    Unknown,
    /// The property or symbol does not exist.
    Missing,
}

/// A hover query for one byte offset in a template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverRequest {
    /// Versions and source snapshot the query depends on.
    pub context: RequestContext,
    /// UTF-8 byte offset; it must be on a character boundary.
    ///
    /// Backends can validate with
    /// `context.source().line_index().position(offset)`.
    pub offset: u32,
}

/// Hover text plus the status and quality of the underlying check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverResult {
    /// Markdown or plain text shown by the editor.
    pub contents: String,
    /// Interpretation of `contents`.
    pub markup: MarkupKind,
    /// Range in the original template, when the backend narrowed it.
    pub range: Option<SourceRange>,
    /// Completeness and availability of the check.
    pub status: CapabilityStatus,
    /// Trustworthiness of the displayed type.
    pub quality: TypeQuality,
}

/// A diagnostics query for one template snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsRequest {
    /// Versions and source snapshot the query depends on.
    pub context: RequestContext,
}

/// One diagnostic in original-template coordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// Exact byte range to mark in the template.
    pub range: SourceRange,
    /// Severity level to publish.
    pub severity: DiagnosticSeverity,
    /// Human-readable explanation.
    pub message: String,
}

/// Severity of a [`Diagnostic`], mirroring the four LSP levels.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiagnosticSeverity {
    /// A definite type error.
    Error,
    /// A suspicious construct that is not necessarily wrong.
    Warning,
    /// Extra information attached to a location.
    Information,
    /// Low-priority hint.
    Hint,
}

/// Diagnostics plus the status of the check that produced them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsResult {
    /// Diagnostics mapped back to the template.
    ///
    /// Empty means "clean" only when [`Self::status`] is
    /// [`CapabilityStatus::Supported`]; otherwise callers must show the
    /// degraded status instead of treating the file as checked.
    pub diagnostics: Vec<Diagnostic>,
    /// Completeness and availability of the check.
    pub status: CapabilityStatus,
}

/// A definition query for one byte offset in a template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionRequest {
    /// Versions and source snapshot the query depends on.
    pub context: RequestContext,
    /// UTF-8 byte offset; it must be on a character boundary.
    pub offset: u32,
}

/// One definition target in an original source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    /// Target file.
    pub file: FileId,
    /// Exact target range in that file.
    pub range: SourceRange,
}

/// Definition locations plus the status of the query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionResult {
    /// Definition targets.
    ///
    /// Empty plus [`CapabilityStatus::Supported`] means "checked, and there is
    /// no definition". Empty plus `Unsupported`/`Disabled`/`Unavailable`/
    /// `Unknown` means the backend could not answer; callers must not show it
    /// as "no definition".
    pub locations: Vec<SourceLocation>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
}

/// Feature flags and per-feature support advertised by a backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendCapabilities {
    /// Support for [`Backend::hover`].
    pub hover: CapabilityStatus,
    /// Support for [`Backend::diagnostics`].
    pub diagnostics: CapabilityStatus,
    /// Support for [`Backend::definition`].
    pub definition: CapabilityStatus,
    /// Completion support; the field exists before the method, which lands in
    /// C1.
    pub completion: CapabilityStatus,
}

impl BackendCapabilities {
    /// Returns capabilities with every feature reported as not implemented.
    #[must_use]
    pub fn none() -> Self {
        let unsupported = || CapabilityStatus::Unsupported {
            reason: "not implemented".to_owned(),
        };
        Self {
            hover: unsupported(),
            diagnostics: unsupported(),
            definition: unsupported(),
            completion: unsupported(),
        }
    }
}

impl Default for BackendCapabilities {
    fn default() -> Self {
        Self::none()
    }
}

/// A replaceable source of template type information.
///
/// This is a synchronous, blocking boundary. Callers must run requests on a
/// dedicated thread or through `spawn_blocking`, and must enforce a read
/// timeout; implementations must not block indefinitely. Queries are issued
/// from more than one task, so every backend must be `Send + Sync` and do its
/// own synchronization.
pub trait Backend: Send + Sync {
    /// Returns the backend generation that will answer requests.
    fn epoch(&self) -> BackendEpoch;

    /// Returns hover information for `request`.
    fn hover(&self, request: HoverRequest) -> Result<HoverResult, BackendError>;

    /// Returns the diagnostics for `request`.
    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError>;

    /// Returns definition targets for `request`.
    ///
    /// The default implementation reports the feature as unsupported, which is
    /// different from an empty supported result.
    fn definition(&self, _request: DefinitionRequest) -> Result<DefinitionResult, BackendError> {
        Ok(DefinitionResult {
            locations: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: "definition is not implemented".to_owned(),
            },
        })
    }

    /// Reports which features this backend implements and how completely.
    ///
    /// The default claims nothing, so a new backend must opt in explicitly.
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use wxml_text::{Position, TextRange};

    use crate::context::SourceSnapshot;
    use crate::contract::{
        BackendId, FileId, ProjectId, ProjectRevision, SnapshotId, SourceRevision, ViewRevision,
    };

    fn file() -> FileId {
        FileId::new(ProjectId::new("example"), "pages/index/index.wxml").expect("valid path")
    }

    fn context() -> RequestContext {
        RequestContext::new(
            ProjectRevision::new(ProjectId::new("example"), 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("mock"), 1),
            Arc::new(SourceSnapshot::new(
                file(),
                SourceRevision::new(1),
                SnapshotId::new(1),
                "{{ user.name }}",
            )),
        )
        .expect("matching projects")
    }

    struct BareBackend;

    impl Backend for BareBackend {
        fn epoch(&self) -> BackendEpoch {
            BackendEpoch::new(BackendId::new("bare"), 1)
        }

        fn hover(&self, _request: HoverRequest) -> Result<HoverResult, BackendError> {
            Err(BackendError::Timeout)
        }

        fn diagnostics(
            &self,
            _request: DiagnosticsRequest,
        ) -> Result<DiagnosticsResult, BackendError> {
            Ok(DiagnosticsResult {
                diagnostics: Vec::new(),
                status: CapabilityStatus::Supported,
            })
        }
    }

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::from_bounds(start, end).expect("valid range")
    }

    #[test]
    fn hover_request_carries_context_and_offset() {
        let request = HoverRequest {
            context: context(),
            offset: 3,
        };
        assert_eq!(request.offset, 3);
        assert_eq!(request.context.file(), &file());
        assert_eq!(
            request
                .context
                .source()
                .line_index()
                .position(usize::try_from(request.offset).expect("offset fits")),
            Some(Position::new(0, 3))
        );
    }

    #[test]
    fn hover_result_describes_markup_status_and_quality() {
        let result = HoverResult {
            contents: "**string**".to_owned(),
            markup: MarkupKind::Markdown,
            range: Some(range(3, 12)),
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Known,
        };
        assert_eq!(result.markup, MarkupKind::Markdown);
        assert!(result.status.is_supported());
        assert_eq!(result.quality, TypeQuality::Known);
        assert_eq!(result.range.expect("range").start(), 3);
    }

    #[test]
    fn hover_result_allows_a_missing_range() {
        let result = HoverResult {
            contents: "string".to_owned(),
            markup: MarkupKind::PlainText,
            range: None,
            status: CapabilityStatus::Supported,
            quality: TypeQuality::Known,
        };
        assert_eq!(result.range, None);
        assert_eq!(result.markup, MarkupKind::PlainText);
    }

    #[test]
    fn markup_defaults_to_markdown() {
        assert_eq!(MarkupKind::default(), MarkupKind::Markdown);
        assert_ne!(MarkupKind::default(), MarkupKind::PlainText);
    }

    #[test]
    fn diagnostic_keeps_its_exact_source_range() {
        let diagnostic = Diagnostic {
            range: range(10, 14),
            severity: DiagnosticSeverity::Error,
            message: "Property 'nmae' does not exist".to_owned(),
        };
        assert_eq!(diagnostic.range.start(), 10);
        assert_eq!(diagnostic.range.end(), 14);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    }

    #[test]
    fn severity_covers_the_four_lsp_levels() {
        let levels = [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Information,
            DiagnosticSeverity::Hint,
        ];
        assert_eq!(levels.len(), 4);
        assert_ne!(DiagnosticSeverity::Error, DiagnosticSeverity::Warning);
    }

    #[test]
    fn diagnostics_result_distinguishes_clean_from_degraded() {
        let clean = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        assert!(clean.status.is_supported());

        let degraded = DiagnosticsResult {
            diagnostics: Vec::new(),
            status: CapabilityStatus::Partial {
                reason: "wxs is not analysed".to_owned(),
            },
        };
        assert!(!degraded.status.is_supported());
        assert_eq!(degraded.status.reason(), Some("wxs is not analysed"));
    }

    #[test]
    fn definition_result_distinguishes_missing_from_unavailable() {
        let checked = DefinitionResult {
            locations: Vec::new(),
            status: CapabilityStatus::Supported,
        };
        assert_eq!(checked.locations, Vec::new());
        assert!(checked.status.is_supported());

        let unavailable = DefinitionResult {
            locations: Vec::new(),
            status: CapabilityStatus::Unavailable {
                reason: "backend exited".to_owned(),
            },
        };
        assert!(!unavailable.status.is_supported());
    }

    #[test]
    fn source_location_keeps_file_and_range() {
        let location = SourceLocation {
            file: FileId::new(ProjectId::new("example"), "pages/index/index.ts")
                .expect("valid path"),
            range: range(40, 48),
        };
        assert_eq!(location.file.path(), "pages/index/index.ts");
        assert_eq!(location.range.len(), 8);
    }

    #[test]
    fn backend_errors_explain_the_failure() {
        assert_eq!(
            BackendError::Unavailable("no node".to_owned()).to_string(),
            "type backend unavailable: no node"
        );
        assert_eq!(
            BackendError::Protocol("bad frame".to_owned()).to_string(),
            "type backend protocol error: bad frame"
        );
        assert_eq!(
            BackendError::Timeout.to_string(),
            "type backend request timed out"
        );
        assert_eq!(
            BackendError::Cancelled.to_string(),
            "type backend request cancelled"
        );
        assert_eq!(
            BackendError::Internal("bug".to_owned()).to_string(),
            "type backend internal error: bug"
        );
    }

    #[test]
    fn backend_errors_are_cloneable() {
        let error = BackendError::Protocol("bad frame".to_owned());
        assert_eq!(error.clone(), error);
    }

    #[test]
    fn capabilities_default_to_nothing_enabled() {
        let capabilities = BareBackend.capabilities();
        assert_eq!(capabilities, BackendCapabilities::none());
        assert_eq!(capabilities, BackendCapabilities::default());
        for (name, status) in [
            ("hover", &capabilities.hover),
            ("diagnostics", &capabilities.diagnostics),
            ("definition", &capabilities.definition),
            ("completion", &capabilities.completion),
        ] {
            assert!(!status.is_supported(), "{name}");
            assert_eq!(status.reason(), Some("not implemented"), "{name}");
        }
    }

    #[test]
    fn definition_defaults_to_unsupported_rather_than_empty_success() {
        let result = BareBackend
            .definition(DefinitionRequest {
                context: context(),
                offset: 0,
            })
            .expect("default implementation succeeds");
        assert_eq!(result.locations, Vec::new());
        assert!(!result.status.is_supported());
    }

    #[test]
    fn backend_is_usable_as_a_trait_object() {
        let backend: Box<dyn Backend> = Box::new(BareBackend);
        assert_eq!(backend.epoch().backend().as_str(), "bare");
        assert!(
            backend
                .hover(HoverRequest {
                    context: context(),
                    offset: 0,
                })
                .is_err()
        );
        let diagnostics = backend
            .diagnostics(DiagnosticsRequest { context: context() })
            .expect("diagnostics succeed");
        assert_eq!(diagnostics.diagnostics, Vec::new());
    }

    #[test]
    fn source_range_is_the_only_response_range_type() {
        // Compile-time witness: `Diagnostic` and `HoverResult` accept
        // `SourceRange`, and `TextRange` has to be converted explicitly.
        let text = TextRange::new(0, 4).expect("valid range");
        let source = SourceRange::new(text);
        let diagnostic = Diagnostic {
            range: source,
            severity: DiagnosticSeverity::Warning,
            message: "demo".to_owned(),
        };
        assert_eq!(diagnostic.range.range(), text);
    }
}
