//! Typed ranges for source files and generated check fragments.
//!
//! [`TextRange`] is deliberately untyped: the same byte pair can address an
//! original WXML file or a generated TypeScript fragment, and mixing them
//! produces ranges that look valid but point at the wrong text. [`SourceRange`]
//! and [`FragmentRange`] keep the layers apart at the type level.

use crate::range::{RangeError, TextRange};

/// A byte range in an original source file.
///
/// This is the only range type that may be published as
/// `Diagnostic::range`, `HoverResult::range` or `SourceLocation::range`. A
/// value is produced by `SourceSnapshot::source_range` in `wxml-types`, which checks
/// the bounds and UTF-8 character boundaries against the snapshot text; the
/// constructor itself does not.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceRange(TextRange);

impl SourceRange {
    /// Wraps an already validated byte range.
    #[must_use]
    pub const fn new(range: TextRange) -> Self {
        Self(range)
    }

    /// Creates a source range from raw bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError`] when `start > end`.
    pub const fn from_bounds(start: u32, end: u32) -> Result<Self, RangeError> {
        match TextRange::new(start, end) {
            Ok(range) => Ok(Self(range)),
            Err(error) => Err(error),
        }
    }

    /// Returns the underlying untyped range.
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.0
    }

    /// Returns the inclusive start offset.
    #[must_use]
    pub const fn start(self) -> u32 {
        self.0.start()
    }

    /// Returns the exclusive end offset.
    #[must_use]
    pub const fn end(self) -> u32 {
        self.0.end()
    }

    /// Returns the length in bytes.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.len()
    }

    /// Returns `true` when the range covers no bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Returns `true` when `offset` lies in `[start, end)`.
    #[must_use]
    pub const fn contains(self, offset: u32) -> bool {
        self.0.contains(offset)
    }
}

/// A byte range in a generated check fragment.
///
/// Fragment ranges live in TypeScript text produced by the template layer, not
/// in the user's WXML. They must be mapped through the fragment source map
/// before becoming a [`SourceRange`]; never publish one directly as a
/// diagnostic or hover range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FragmentRange(TextRange);

impl FragmentRange {
    /// Wraps an already validated byte range.
    #[must_use]
    pub const fn new(range: TextRange) -> Self {
        Self(range)
    }

    /// Creates a fragment range from raw bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError`] when `start > end`.
    pub const fn from_bounds(start: u32, end: u32) -> Result<Self, RangeError> {
        match TextRange::new(start, end) {
            Ok(range) => Ok(Self(range)),
            Err(error) => Err(error),
        }
    }

    /// Returns the underlying untyped range.
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.0
    }

    /// Returns the inclusive start offset.
    #[must_use]
    pub const fn start(self) -> u32 {
        self.0.start()
    }

    /// Returns the exclusive end offset.
    #[must_use]
    pub const fn end(self) -> u32 {
        self.0.end()
    }

    /// Returns the length in bytes.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.len()
    }

    /// Returns `true` when the range covers no bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Returns `true` when `offset` lies in `[start, end)`.
    #[must_use]
    pub const fn contains(self, offset: u32) -> bool {
        self.0.contains(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_range_delegates_to_text_range() {
        let text = TextRange::new(3, 9).expect("valid range");
        let range = SourceRange::new(text);
        assert_eq!(range.range(), text);
        assert_eq!(range.start(), 3);
        assert_eq!(range.end(), 9);
        assert_eq!(range.len(), 6);
        assert!(!range.is_empty());
        assert!(range.contains(3));
        assert!(!range.contains(9));
    }

    #[test]
    fn source_range_from_bounds_checks_order() {
        let range = SourceRange::from_bounds(2, 5).expect("valid range");
        assert_eq!(range.range(), TextRange::new(2, 5).expect("valid range"));
        assert!(SourceRange::from_bounds(6, 5).is_err());
    }

    #[test]
    fn fragment_range_delegates_to_text_range() {
        let text = TextRange::new(10, 10).expect("valid empty range");
        let range = FragmentRange::new(text);
        assert_eq!(range.range(), text);
        assert!(range.is_empty());
        assert_eq!(range.len(), 0);
        assert!(!range.contains(10));
    }

    #[test]
    fn fragment_range_from_bounds_checks_order() {
        let range = FragmentRange::from_bounds(0, 4).expect("valid range");
        assert_eq!(range.start(), 0);
        assert_eq!(range.end(), 4);
        assert!(FragmentRange::from_bounds(5, 4).is_err());
    }

    #[test]
    fn ranges_are_ordered_copyable_values() {
        let first = SourceRange::from_bounds(0, 2).expect("valid range");
        let second = SourceRange::from_bounds(0, 3).expect("valid range");
        let copy = first;
        assert_eq!(copy, first);
        assert!(first < second);
        assert!(
            FragmentRange::from_bounds(1, 2).expect("valid range")
                > FragmentRange::from_bounds(0, 2).expect("valid range")
        );
    }
}
