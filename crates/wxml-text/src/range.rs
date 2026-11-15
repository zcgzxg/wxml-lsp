//! A typed half-open range over UTF-8 byte offsets.

use std::fmt;

/// A half-open range `[start, end)` of UTF-8 byte offsets.
///
/// This is the low-level primitive for byte ranges in source text and syntax
/// trees. It is limited to 4 GiB (`u32`) and does not validate UTF-8 character
/// boundaries or the text length; boundary checks happen in
/// `SourceSnapshot::source_range` in `wxml-types` or in syntax code that owns the
/// text. Use [`crate::SourceRange`] for ranges that may be published to an
/// editor and [`crate::FragmentRange`] for generated check fragments.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextRange {
    start: u32,
    end: u32,
}

impl TextRange {
    /// Creates a range covering `[start, end)`.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError`] when `start > end`.
    pub const fn new(start: u32, end: u32) -> Result<Self, RangeError> {
        if start > end {
            return Err(RangeError { start, end });
        }
        Ok(Self { start, end })
    }

    /// Returns the inclusive start offset.
    #[must_use]
    pub const fn start(&self) -> u32 {
        self.start
    }

    /// Returns the exclusive end offset.
    #[must_use]
    pub const fn end(&self) -> u32 {
        self.end
    }

    /// Returns the length in bytes.
    ///
    /// This cannot overflow because `end >= start` is an invariant.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.end - self.start
    }

    /// Returns `true` when the range covers no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns `true` when `offset` lies in `[start, end)`.
    #[must_use]
    pub const fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

/// The error returned when a [`TextRange`] would be inverted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeError {
    start: u32,
    end: u32,
}

impl RangeError {
    /// Returns the rejected start offset.
    #[must_use]
    pub const fn start(&self) -> u32 {
        self.start
    }

    /// Returns the rejected end offset.
    #[must_use]
    pub const fn end(&self) -> u32 {
        self.end
    }
}

impl fmt::Display for RangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid text range: start {} is greater than end {}",
            self.start, self.end
        )
    }
}

impl std::error::Error for RangeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn keeps_start_and_end() {
        let range = TextRange::new(2, 7).expect("valid range");
        assert_eq!(range.start(), 2);
        assert_eq!(range.end(), 7);
        assert_eq!(range.len(), 5);
        assert!(!range.is_empty());
    }

    #[test]
    fn allows_the_empty_range() {
        let range = TextRange::new(4, 4).expect("valid empty range");
        assert_eq!(range.len(), 0);
        assert!(range.is_empty());
        assert!(!range.contains(4));
    }

    #[test]
    fn rejects_an_inverted_range() {
        let error = TextRange::new(9, 3).expect_err("start > end must fail");
        assert_eq!(error.start(), 9);
        assert_eq!(error.end(), 3);
        assert_eq!(
            error.to_string(),
            "invalid text range: start 9 is greater than end 3"
        );
    }

    #[test]
    fn contains_the_start_but_not_the_end() {
        let range = TextRange::new(10, 20).expect("valid range");
        assert!(!range.contains(9));
        assert!(range.contains(10));
        assert!(range.contains(19));
        assert!(!range.contains(20));
        assert!(!range.contains(21));
    }

    #[test]
    fn contains_nothing_when_empty() {
        let range = TextRange::new(10, 10).expect("valid empty range");
        assert!(!range.contains(9));
        assert!(!range.contains(10));
    }

    #[test]
    fn accepts_the_largest_representable_offset() {
        let range = TextRange::new(0, u32::MAX).expect("valid range");
        assert_eq!(range.len(), u32::MAX);
        assert!(range.contains(u32::MAX - 1));
    }

    #[test]
    fn accepts_an_empty_range_at_the_largest_offset() {
        let range = TextRange::new(u32::MAX, u32::MAX).expect("valid empty range");
        assert_eq!(range.start(), u32::MAX);
        assert_eq!(range.end(), u32::MAX);
        assert!(range.is_empty());
        assert!(!range.contains(u32::MAX));
    }

    proptest! {
        #[test]
        fn round_trips_ordered_bounds(start in 0u32..u32::MAX, length in 0u32..1024) {
            let Some(end) = start.checked_add(length) else {
                return Ok(());
            };
            let range = TextRange::new(start, end).expect("ordered bounds are valid");
            prop_assert_eq!(range.start(), start);
            prop_assert_eq!(range.end(), end);
            prop_assert_eq!(range.len(), length);
            prop_assert_eq!(range.is_empty(), length == 0);
            prop_assert_eq!(range.contains(start), length > 0);
            prop_assert!(!range.contains(end));
        }

        #[test]
        fn rejects_every_inverted_pair(start in 1u32..u32::MAX, delta in 1u32..1024) {
            let Some(end) = start.checked_sub(delta) else {
                return Ok(());
            };
            prop_assert!(TextRange::new(start, end).is_err());
        }
    }
}
