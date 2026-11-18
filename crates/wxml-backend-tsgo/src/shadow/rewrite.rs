//! Identifier rewrite plans for shadow copies.
//!
//! Rewrites are exact-text, ordered and non-overlapping. `P3-S` implements the
//! rules that keep strings, comments and object keys untouched; this module
//! only declares the edit container.

use crate::native::strings::Utf8Offset;

/// One exact substitution in a copied expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteEdit {
    /// Inclusive UTF-8 start in the copy.
    pub start: Utf8Offset,
    /// Exclusive UTF-8 end in the copy.
    pub end: Utf8Offset,
    /// Replacement text.
    pub replacement: String,
}

/// Ordered edits for one expression copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RewritePlan {
    edits: Vec<RewriteEdit>,
}

impl RewritePlan {
    /// Creates a plan from ordered, non-overlapping edits.
    #[must_use]
    pub const fn new(edits: Vec<RewriteEdit>) -> Self {
        Self { edits }
    }

    /// Returns the edits in application order.
    #[must_use]
    pub fn edits(&self) -> &[RewriteEdit] {
        &self.edits
    }

    /// Returns the number of edits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.edits.len()
    }

    /// Returns whether the plan is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}
