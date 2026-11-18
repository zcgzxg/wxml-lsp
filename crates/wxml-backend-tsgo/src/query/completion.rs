//! Completion query DTO placeholders.
//!
//! Native completion details and auto-imports are not part of the frozen v1
//! allow-list. `P4` must degrade explicitly; this module only declares the
//! bounded internal result shape.

use wxml_types::{CapabilityStatus, MAX_COMPLETION_ITEMS};

use crate::input::ProjectKey;
use crate::native::strings::Utf16Position;
use crate::shadow::mapping::SourceRange;

/// Returns the shared C0 completion cap (`wxml_types::MAX_COMPLETION_ITEMS`).
///
/// The new backend must not define a second completion budget.
#[must_use]
pub const fn completion_limit() -> usize {
    MAX_COMPLETION_ITEMS
}

/// Completion request in native coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
    /// Native UTF-16 position.
    pub position: Utf16Position,
}

/// One completion entry placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEntry {
    /// Display label.
    pub label: String,
    /// Optional type/detail text.
    pub detail: Option<String>,
    /// Replacement range when it maps to original text.
    pub replacement: Option<SourceRange>,
}

/// Completion result placeholder.
///
/// Empty entries mean "checked, nothing to suggest" only when [`Self::status`]
/// is [`CapabilityStatus::Supported`]. [`Self::truncated`] must be `true` when
/// [`MAX_COMPLETION_ITEMS`] cut the candidate list short, and the status must
/// then be `Partial` so callers cannot treat the list as complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    /// Entries in native order.
    pub entries: Vec<CompletionEntry>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
    /// Whether the shared C0 cap cut the candidate list short.
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_cap_uses_the_shared_c0_limit() {
        assert_eq!(completion_limit(), 256);
        assert_eq!(completion_limit(), MAX_COMPLETION_ITEMS);
    }
}
