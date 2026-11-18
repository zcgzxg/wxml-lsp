//! Definition query DTO placeholders.
//!
//! Definition has no direct native API; `P4` combines symbol declarations,
//! references and same-snapshot AST ranges. This module declares the internal
//! result shape and keeps incompleteness explicit through the status.

use wxml_types::CapabilityStatus;

use crate::input::ProjectKey;
use crate::native::strings::Utf16Position;
use crate::shadow::mapping::SourceRange;

/// Definition request in native coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
    /// Native UTF-16 position.
    pub position: Utf16Position,
}

/// One definition target in original coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionTarget {
    /// Normalized target path.
    pub path: String,
    /// Original UTF-8 range of the identifier token.
    pub range: SourceRange,
}

/// Definition result placeholder.
///
/// Empty targets mean "checked, no definition" only when [`Self::status`] is
/// [`CapabilityStatus::Supported`]. Empty plus any degraded status means the
/// backend could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionResult {
    /// Targets that mapped back to real source text.
    pub targets: Vec<DefinitionTarget>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
}
