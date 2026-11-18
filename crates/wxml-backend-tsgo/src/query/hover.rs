//! Hover query DTO placeholders.
//!
//! Hover has no direct native API; `P4` composes type, symbol and documentation
//! calls. This module only declares the internal request and result shapes.

use wxml_types::CapabilityStatus;

use crate::input::ProjectKey;
use crate::native::strings::Utf16Position;
use crate::shadow::mapping::SourceRange;

/// Hover request in native coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
    /// Native UTF-16 position.
    pub position: Utf16Position,
}

/// One resolved hover symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverSymbol {
    /// Rendered type/documentation text.
    pub display: String,
    /// Original source range when it can be mapped.
    pub range: Option<SourceRange>,
}

/// Hover result placeholder.
///
/// The pair carries four distinct meanings:
///
/// - `symbol: None` plus [`CapabilityStatus::Supported`] means "checked, no
///   symbol";
/// - `symbol: Some(..)` plus `Supported` is a complete answer;
/// - `symbol: Some(..)` plus `Partial` is an answer with missing evidence;
/// - `symbol: None` plus `Unsupported`/`Disabled`/`Unavailable`/`Unknown`
///   means the backend could not answer.
///
/// The adapter must not synthesize a type for either `None` case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverResult {
    /// Resolved symbol, if one exists.
    pub symbol: Option<HoverSymbol>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
}
