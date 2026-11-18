//! Diagnostics query DTO placeholders.
//!
//! `P4` maps native semantic and syntactic diagnostics into C0 results. Exact
//! code, severity and range evidence must survive that conversion.

use wxml_types::CapabilityStatus;

use crate::input::ProjectKey;
use crate::native::strings::Utf16Position;

/// Diagnostics request for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
}

/// Diagnostic category mirrored from the native category field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    /// Native error.
    Error,
    /// Native warning.
    Warning,
    /// Native suggestion.
    Suggestion,
    /// Informational message.
    Message,
}

/// One native diagnostic with exact code and range evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDiagnostic {
    /// Native diagnostic code.
    pub code: u32,
    /// Sanitized message text.
    pub message: String,
    /// Severity category.
    pub severity: DiagnosticSeverity,
    /// Native exclusive end position.
    pub end: Utf16Position,
    /// Native inclusive start position.
    pub start: Utf16Position,
}

/// Diagnostics result placeholder.
///
/// Empty diagnostics mean "clean" only when [`Self::status`] is
/// [`CapabilityStatus::Supported`]. Empty plus any degraded status means the
/// check could not answer and must not be shown as a clean file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsResult {
    /// Collected diagnostics.
    pub diagnostics: Vec<NativeDiagnostic>,
    /// Completeness and availability of the check.
    pub status: CapabilityStatus,
}
