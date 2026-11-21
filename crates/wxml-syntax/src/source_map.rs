//! Segmented mapping between original WXML ranges and generated check
//! fragments.
//!
//! The template layer copies selected source ranges into a generated fragment
//! (for example, a mustache expression into a TypeScript check file) and adds
//! synthetic text around them. Each copied range is recorded here as a
//! one-to-one segment: the fragment segment has the same byte length as the
//! source segment, so a range or offset inside it maps by a constant delta.
//!
//! Synthetic text has no segment and therefore never maps back to a source
//! range. Queries that cross segment boundaries return `None` instead of
//! guessing; callers must not publish an approximate location.
//!
//! [`FragmentBuilder`] is a small helper that appends synthetic text and
//! verbatim source copies while maintaining the map.

use std::fmt;

use wxml_text::{FragmentRange, TextRange};

use crate::error::span;

/// Why a value in a segment correlates with the generated fragment.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SegmentKind {
    /// A `{{ ... }}` expression copied into a check fragment.
    MustacheExpression,
    /// An attribute value copied into a check fragment.
    AttributeValue,
    /// A text run copied into a check fragment.
    Text,
    /// Any other one-to-one copy.
    Other,
}

/// One one-to-one mapping between a source range and a fragment range.
///
/// Both ranges have the same byte length. Segments in a [`SourceMap`] are
/// ordered and non-overlapping in both coordinate spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceMapSegment {
    /// Range in the original WXML source.
    pub source: TextRange,
    /// Range in the generated check fragment.
    pub fragment: FragmentRange,
    /// Why the text was copied.
    pub kind: SegmentKind,
}

/// Errors produced while building a [`SourceMap`] or a [`FragmentBuilder`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceMapError {
    /// A segment must cover at least one byte in both coordinate spaces.
    EmptySegment,
    /// Source and fragment segments must have the same length.
    LengthMismatch {
        /// The source range that was pushed.
        source: TextRange,
        /// The fragment range that was pushed.
        fragment: FragmentRange,
    },
    /// The source range overlaps or precedes the previous segment.
    OverlappingSource {
        /// Previous accepted source range.
        previous: TextRange,
        /// Rejected source range.
        next: TextRange,
    },
    /// The fragment range overlaps or precedes the previous segment.
    OverlappingFragment {
        /// Previous accepted fragment range.
        previous: FragmentRange,
        /// Rejected fragment range.
        next: FragmentRange,
    },
    /// Verbatim text length does not match the declared source range.
    VerbatimLengthMismatch {
        /// The source range that was declared.
        source: TextRange,
        /// The actual byte length of the supplied text.
        text_len: u32,
    },
    /// The generated fragment grew beyond the 4 GiB range limit.
    FragmentTooLarge,
}

impl fmt::Display for SourceMapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySegment => write!(formatter, "source map segment is empty"),
            Self::LengthMismatch { source, fragment } => write!(
                formatter,
                "source map segment lengths differ: source {}..{} is {} bytes, fragment {}..{} is {} bytes",
                source.start(),
                source.end(),
                source.len(),
                fragment.start(),
                fragment.end(),
                fragment.len()
            ),
            Self::OverlappingSource { previous, next } => write!(
                formatter,
                "source map segment {}..{} overlaps previous source range {}..{}",
                next.start(),
                next.end(),
                previous.start(),
                previous.end()
            ),
            Self::OverlappingFragment { previous, next } => write!(
                formatter,
                "source map segment {}..{} overlaps previous fragment range {}..{}",
                next.start(),
                next.end(),
                previous.start(),
                previous.end()
            ),
            Self::VerbatimLengthMismatch { source, text_len } => write!(
                formatter,
                "verbatim text is {text_len} bytes but source range {}..{} is {} bytes",
                source.start(),
                source.end(),
                source.len()
            ),
            Self::FragmentTooLarge => write!(formatter, "generated fragment exceeds 4 GiB"),
        }
    }
}

impl std::error::Error for SourceMapError {}

/// An ordered, non-overlapping set of one-to-one source/fragment segments.
///
/// Segments must be pushed in increasing order. Text that is not covered by a
/// segment (synthetic text, wrappers, separators) has no mapping.
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    segments: Vec<SourceMapSegment>,
}

impl SourceMap {
    /// Creates an empty map.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Creates an empty map with room for `capacity` segments.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            segments: Vec::with_capacity(capacity),
        }
    }

    /// Returns the accepted segments in source order.
    #[must_use]
    pub fn segments(&self) -> &[SourceMapSegment] {
        &self.segments
    }

    /// Returns the number of segments.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.segments.len()
    }

    /// Returns `true` when no segments were added.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Adds a segment.
    ///
    /// Source and fragment ranges must be non-empty, have equal length, and
    /// start at or after the end of the previous segment in both coordinate
    /// spaces.
    ///
    /// # Errors
    ///
    /// Returns [`SourceMapError`] when the segment is empty, has different
    /// source/fragment lengths, or overlaps a previous segment.
    pub fn push(
        &mut self,
        source: TextRange,
        fragment: FragmentRange,
        kind: SegmentKind,
    ) -> Result<(), SourceMapError> {
        if source.is_empty() || fragment.is_empty() {
            return Err(SourceMapError::EmptySegment);
        }
        if source.len() != fragment.len() {
            return Err(SourceMapError::LengthMismatch { source, fragment });
        }
        if let Some(previous) = self.segments.last() {
            if source.start() < previous.source.end() {
                return Err(SourceMapError::OverlappingSource {
                    previous: previous.source,
                    next: source,
                });
            }
            if fragment.start() < previous.fragment.end() {
                return Err(SourceMapError::OverlappingFragment {
                    previous: previous.fragment,
                    next: fragment,
                });
            }
        }
        self.segments.push(SourceMapSegment {
            source,
            fragment,
            kind,
        });
        Ok(())
    }

    /// Returns the segment containing a source offset.
    #[must_use]
    pub fn segment_at_source(&self, offset: u32) -> Option<&SourceMapSegment> {
        let index = self
            .segments
            .partition_point(|segment| segment.source.end() <= offset);
        self.segments
            .get(index)
            .filter(|segment| segment.source.contains(offset))
    }

    /// Returns the segment containing a fragment offset.
    #[must_use]
    pub fn segment_at_fragment(&self, offset: u32) -> Option<&SourceMapSegment> {
        let index = self
            .segments
            .partition_point(|segment| segment.fragment.end() <= offset);
        self.segments
            .get(index)
            .filter(|segment| segment.fragment.contains(offset))
    }

    /// Maps a non-empty source range to its fragment range.
    ///
    /// Returns `None` when the range is empty, crosses a segment boundary, or
    /// is not covered by a segment.
    #[must_use]
    pub fn source_to_fragment(&self, source: TextRange) -> Option<FragmentRange> {
        if source.is_empty() {
            return None;
        }
        let segment = self.segment_at_source(source.start())?;
        if source.end() > segment.source.end() {
            return None;
        }
        let delta = source.start() - segment.source.start();
        let start = segment.fragment.start().checked_add(delta)?;
        let end = start.checked_add(source.len())?;
        Some(FragmentRange::new(span(start, end)))
    }

    /// Maps a non-empty fragment range back to the original source range.
    ///
    /// Returns `None` when the range is empty, crosses a segment boundary, or
    /// lies in synthetic text.
    #[must_use]
    pub fn fragment_to_source(&self, fragment: FragmentRange) -> Option<TextRange> {
        if fragment.is_empty() {
            return None;
        }
        let segment = self.segment_at_fragment(fragment.start())?;
        if fragment.end() > segment.fragment.end() {
            return None;
        }
        let delta = fragment.start() - segment.fragment.start();
        let start = segment.source.start().checked_add(delta)?;
        let end = start.checked_add(fragment.len())?;
        Some(span(start, end))
    }

    /// Maps a single source offset to a fragment offset.
    #[must_use]
    pub fn source_offset_to_fragment(&self, offset: u32) -> Option<u32> {
        let segment = self.segment_at_source(offset)?;
        let delta = offset - segment.source.start();
        segment.fragment.start().checked_add(delta)
    }

    /// Maps a single fragment offset back to a source offset.
    #[must_use]
    pub fn fragment_offset_to_source(&self, offset: u32) -> Option<u32> {
        let segment = self.segment_at_fragment(offset)?;
        let delta = offset - segment.fragment.start();
        segment.source.start().checked_add(delta)
    }
}

/// Builds a generated fragment and its [`SourceMap`] together.
///
/// Synthetic text can be inserted freely; only [`FragmentBuilder::push_verbatim`]
/// adds a mapping. The builder keeps the map consistent when a segment is
/// rejected: the text is not appended unless the mapping succeeded.
#[derive(Debug, Default)]
pub struct FragmentBuilder {
    text: String,
    map: SourceMap,
}

impl FragmentBuilder {
    /// Creates an empty builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            map: SourceMap::new(),
        }
    }

    /// Creates an empty builder with preallocated text capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity),
            map: SourceMap::new(),
        }
    }

    /// Appends text that has no source mapping.
    pub fn push_synthetic(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Appends verbatim source text and records its one-to-one mapping.
    ///
    /// `source_text` must be exactly the text covered by `source`; otherwise
    /// the mapping would be wrong. On error nothing is appended.
    ///
    /// # Errors
    ///
    /// Returns [`SourceMapError`] when the text length differs from the range
    /// length, the segment would overlap a previous one, or the fragment
    /// exceeds the 4 GiB range limit.
    pub fn push_verbatim(
        &mut self,
        source_text: &str,
        source: TextRange,
        kind: SegmentKind,
    ) -> Result<FragmentRange, SourceMapError> {
        let text_len =
            u32::try_from(source_text.len()).map_err(|_| SourceMapError::FragmentTooLarge)?;
        if text_len != source.len() {
            return Err(SourceMapError::VerbatimLengthMismatch { source, text_len });
        }
        let start = u32::try_from(self.text.len()).map_err(|_| SourceMapError::FragmentTooLarge)?;
        let end = start
            .checked_add(text_len)
            .ok_or(SourceMapError::FragmentTooLarge)?;
        let fragment = FragmentRange::new(span(start, end));
        self.map.push(source, fragment, kind)?;
        self.text.push_str(source_text);
        Ok(fragment)
    }

    /// Returns the fragment text built so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the mapping built so far.
    #[must_use]
    pub const fn map(&self) -> &SourceMap {
        &self.map
    }

    /// Consumes the builder and returns the fragment text with its map.
    #[must_use]
    pub fn finish(self) -> (String, SourceMap) {
        (self.text, self.map)
    }
}

#[cfg(test)]
mod tests {
    use super::{FragmentBuilder, SegmentKind, SourceMap, SourceMapError, SourceMapSegment};
    use wxml_text::{FragmentRange, TextRange};

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(start, end).expect("valid range")
    }

    fn fragment(start: u32, end: u32) -> FragmentRange {
        FragmentRange::from_bounds(start, end).expect("valid fragment range")
    }

    type SegmentSpec = ((u32, u32), (u32, u32), SegmentKind);

    fn map_with(segments: &[SegmentSpec]) -> SourceMap {
        let mut map = SourceMap::new();
        for ((source_start, source_end), (fragment_start, fragment_end), kind) in segments {
            map.push(
                range(*source_start, *source_end),
                fragment(*fragment_start, *fragment_end),
                *kind,
            )
            .expect("segment accepted");
        }
        map
    }

    #[test]
    fn empty_map_maps_nothing() {
        let map = SourceMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert!(
            map.segments().is_empty(),
            "unexpected segments: {:?}",
            map.segments()
        );
        assert!(map.segment_at_source(0).is_none());
        assert!(map.segment_at_fragment(0).is_none());
        assert_eq!(map.source_to_fragment(range(0, 1)), None);
        assert_eq!(map.fragment_to_source(fragment(0, 1)), None);
    }

    #[test]
    fn maps_ranges_and_offsets_in_both_directions() {
        let map = map_with(&[((10, 16), (100, 106), SegmentKind::MustacheExpression)]);
        let segment = map.segment_at_source(10).expect("segment at source");
        assert_eq!(segment.source, range(10, 16));
        assert_eq!(segment.fragment, fragment(100, 106));
        assert_eq!(segment.kind, SegmentKind::MustacheExpression);

        assert_eq!(
            map.source_to_fragment(range(10, 16)),
            Some(fragment(100, 106))
        );
        assert_eq!(
            map.source_to_fragment(range(12, 14)),
            Some(fragment(102, 104))
        );
        assert_eq!(
            map.fragment_to_source(fragment(102, 104)),
            Some(range(12, 14))
        );
        assert_eq!(map.source_offset_to_fragment(12), Some(102));
        assert_eq!(map.fragment_offset_to_source(102), Some(12));
        assert_eq!(map.source_offset_to_fragment(16), None);
        assert_eq!(map.fragment_offset_to_source(106), None);
    }

    #[test]
    fn round_trips_every_segment() {
        let map = map_with(&[
            ((0, 4), (20, 24), SegmentKind::Text),
            ((10, 18), (40, 48), SegmentKind::MustacheExpression),
            ((30, 35), (60, 65), SegmentKind::AttributeValue),
        ]);
        for segment in map.segments() {
            let mapped = map
                .source_to_fragment(segment.source)
                .expect("source maps to a fragment");
            assert_eq!(mapped, segment.fragment);
            assert_eq!(map.fragment_to_source(mapped), Some(segment.source));
        }
    }

    #[test]
    fn rejects_ranges_spanning_segments() {
        let map = map_with(&[
            ((0, 2), (10, 12), SegmentKind::Text),
            ((4, 6), (14, 16), SegmentKind::Text),
        ]);
        assert_eq!(map.source_to_fragment(range(0, 6)), None);
        assert_eq!(map.fragment_to_source(fragment(10, 16)), None);
        assert_eq!(map.source_to_fragment(range(2, 4)), None);
        assert_eq!(map.source_to_fragment(range(0, 0)), None);
    }

    #[test]
    fn rejects_empty_segments() {
        let mut map = SourceMap::new();
        let error = map
            .push(range(3, 3), fragment(7, 7), SegmentKind::Text)
            .expect_err("empty segments are rejected");
        assert!(matches!(error, SourceMapError::EmptySegment));
        assert!(map.is_empty());
    }

    #[test]
    fn rejects_segments_with_different_lengths() {
        let mut map = SourceMap::new();
        let error = map
            .push(range(0, 4), fragment(0, 5), SegmentKind::Text)
            .expect_err("length mismatch is rejected");
        assert!(matches!(
            error,
            SourceMapError::LengthMismatch { source, fragment }
                if source == range(0, 4) && fragment == FragmentRange::from_bounds(0, 5).expect("range")
        ));
        assert!(map.is_empty());
    }

    #[test]
    fn rejects_overlapping_source_ranges() {
        let mut map = SourceMap::new();
        map.push(range(0, 4), fragment(0, 4), SegmentKind::Text)
            .expect("first segment");
        let error = map
            .push(range(3, 6), fragment(10, 13), SegmentKind::Text)
            .expect_err("overlap is rejected");
        assert!(matches!(
            error,
            SourceMapError::OverlappingSource { previous, next }
                if previous.start() == 0 && next.start() == 3
        ));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn rejects_overlapping_fragment_ranges() {
        let mut map = SourceMap::new();
        map.push(range(0, 4), fragment(0, 4), SegmentKind::Text)
            .expect("first segment");
        let error = map
            .push(range(10, 13), fragment(3, 6), SegmentKind::Text)
            .expect_err("fragment overlap is rejected");
        assert!(matches!(
            error,
            SourceMapError::OverlappingFragment { previous, next }
                if previous.start() == 0 && next.start() == 3
        ));
    }

    #[test]
    fn rejects_out_of_order_segments() {
        let mut map = SourceMap::new();
        map.push(range(10, 14), fragment(0, 4), SegmentKind::Text)
            .expect("first segment");
        let error = map
            .push(range(0, 4), fragment(10, 14), SegmentKind::Text)
            .expect_err("order is enforced");
        assert!(matches!(error, SourceMapError::OverlappingSource { .. }));
    }

    #[test]
    fn adjacent_segments_are_allowed() {
        let map = map_with(&[
            ((0, 2), (0, 2), SegmentKind::Text),
            ((2, 4), (2, 4), SegmentKind::AttributeValue),
        ]);
        assert_eq!(map.len(), 2);
        assert_eq!(map.source_offset_to_fragment(1), Some(1));
        assert_eq!(map.source_offset_to_fragment(2), Some(2));
    }

    #[test]
    fn builder_copies_verbatim_segments_and_inserts_synthetic_text() {
        let mut builder = FragmentBuilder::new();
        builder.push_synthetic("const __wxml_root = ");
        let source = "user.name";
        let copied = builder
            .push_verbatim(source, range(10, 19), SegmentKind::MustacheExpression)
            .expect("verbatim copy");
        builder.push_synthetic(";");
        let (text, map) = builder.finish();
        assert_eq!(text, "const __wxml_root = user.name;");
        assert_eq!(copied, fragment(20, 29));
        assert_eq!(map.len(), 1);
        assert_eq!(map.fragment_to_source(copied), Some(range(10, 19)));
        assert_eq!(&text[20..29], source);
    }

    #[test]
    fn builder_rejects_verbatim_text_of_the_wrong_length() {
        let mut builder = FragmentBuilder::new();
        let error = builder
            .push_verbatim("abc", range(0, 2), SegmentKind::Text)
            .expect_err("length mismatch is rejected");
        assert!(matches!(
            error,
            SourceMapError::VerbatimLengthMismatch { .. }
        ));
        assert_eq!(builder.text(), "");
        assert!(builder.map().is_empty(), "map should stay empty");
    }

    #[test]
    fn builder_text_and_map_are_visible_before_finish() {
        let mut builder = FragmentBuilder::with_capacity(16);
        builder.push_synthetic("x");
        assert_eq!(builder.text(), "x");
        assert!(builder.map().is_empty(), "map should stay empty");
        builder
            .push_verbatim("y", range(1, 2), SegmentKind::Text)
            .expect("verbatim copy");
        assert_eq!(builder.text(), "xy");
        assert_eq!(builder.map().len(), 1);
    }

    #[test]
    fn builder_default_is_empty() {
        let builder = FragmentBuilder::default();
        assert_eq!(builder.text(), "");
        assert!(builder.map().is_empty(), "map should stay empty");
    }

    #[test]
    fn segment_kinds_are_comparable() {
        assert_eq!(
            SegmentKind::MustacheExpression,
            SegmentKind::MustacheExpression
        );
        assert_ne!(SegmentKind::AttributeValue, SegmentKind::Text);
        let segment = SourceMapSegment {
            source: range(0, 1),
            fragment: fragment(0, 1),
            kind: SegmentKind::Other,
        };
        assert_eq!(segment.source, range(0, 1));
        assert_eq!(segment.kind, SegmentKind::Other);
    }
}
