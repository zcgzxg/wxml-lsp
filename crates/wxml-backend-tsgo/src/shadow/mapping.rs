//! Two-way mapping between shadow script, expression copy and WXML bytes.
//!
//! The mapper composes three coordinate systems:
//!
//! - original/derived **script** offsets, expressed in UTF-16 code units;
//! - mounted-expression segments, expressed in derived UTF-16 plus original
//!   WXML **UTF-8 bytes**;
//! - Rust source ranges, which are UTF-8 bytes.
//!
//! Conversions are explicit. A position inside a surrogate pair, outside the
//! text or inside generated/removed text returns a typed [`ShadowError`]; it is
//! never clamped and never silently dropped. CRLF is two independent code
//! units (`\r` and `\n`) and a BOM is one code unit; no normalization happens
//! here.
//!
//! The six-tuple native handle boundary lives in `crate::native::handles` and
//! is not touched by this module.

use crate::native::strings::{Utf8Offset, Utf16Offset};
use crate::shadow::ShadowError;
use crate::shadow::rewrite::{Assembled, MountedSegment};

/// Half-open UTF-8 byte range in one original text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRange {
    /// Inclusive UTF-8 start.
    pub start: Utf8Offset,
    /// Exclusive UTF-8 end.
    pub end: Utf8Offset,
}

impl SourceRange {
    /// Creates a range without validating it against any text.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            start: Utf8Offset::new(start),
            end: Utf8Offset::new(end),
        }
    }

    /// Returns the byte length, saturating on malformed inverted ranges.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.get().saturating_sub(self.start.get())
    }

    /// Returns whether the range is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start.get() >= self.end.get()
    }

    /// Returns whether `offset` falls inside the half-open range.
    #[must_use]
    pub const fn contains(self, offset: u32) -> bool {
        offset >= self.start.get() && offset < self.end.get()
    }
}

/// Text layer a range belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingLayer {
    /// Original WXML attribute value.
    Template,
    /// Rewritten expression copy.
    ExpressionCopy,
    /// Generated shadow document.
    Shadow,
    /// Original `.js`/`.ts` script text.
    OriginalScript,
}

/// Mapping seam implemented by [`ShadowMap`].
pub trait ShadowMapper {
    /// Maps a range from `layer` back to the original script or template.
    fn to_original(
        &self,
        layer: MappingLayer,
        range: SourceRange,
    ) -> Result<SourceRange, ShadowError>;

    /// Maps an original range into the generated script.
    ///
    /// When `layer` is [`MappingLayer::Shadow`], the range is already in shadow
    /// coordinates and must be returned unchanged.
    fn to_shadow(
        &self,
        layer: MappingLayer,
        range: SourceRange,
    ) -> Result<SourceRange, ShadowError>;
}

/// Returns the length of `text` in UTF-16 code units.
#[must_use]
pub fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// Converts a UTF-16 code-unit offset into a UTF-8 byte offset.
///
/// A position beyond the text returns [`ShadowError::PositionOutOfRange`]; a
/// position between the halves of a surrogate pair returns
/// [`ShadowError::NotCharBoundary`].
pub fn utf16_to_byte(text: &str, units: u32) -> Result<usize, ShadowError> {
    let length = utf16_len(text);
    if units > length {
        return Err(ShadowError::PositionOutOfRange {
            position: units,
            length,
            unit: "utf-16",
        });
    }
    let mut remaining = units;
    let mut byte = 0;
    for character in text.chars() {
        if remaining == 0 {
            return Ok(byte);
        }
        let width = u32::try_from(character.len_utf16()).unwrap_or(1);
        if remaining < width {
            return Err(ShadowError::NotCharBoundary { position: units });
        }
        remaining -= width;
        byte += character.len_utf8();
    }
    if remaining == 0 {
        Ok(byte)
    } else {
        Err(ShadowError::PositionOutOfRange {
            position: units,
            length,
            unit: "utf-16",
        })
    }
}

/// Converts a UTF-8 byte offset into a UTF-16 code-unit offset.
///
/// A non-boundary byte (inside a multi-byte code point) returns
/// [`ShadowError::NotCharBoundary`].
pub fn byte_to_utf16(text: &str, byte: usize) -> Result<u32, ShadowError> {
    if byte > text.len() {
        return Err(ShadowError::PositionOutOfRange {
            position: u32::try_from(byte).unwrap_or(u32::MAX),
            length: u32::try_from(text.len()).unwrap_or(u32::MAX),
            unit: "utf-8",
        });
    }
    if !text.is_char_boundary(byte) {
        return Err(ShadowError::NotCharBoundary {
            position: u32::try_from(byte).unwrap_or(u32::MAX),
        });
    }
    u32::try_from(text[..byte].encode_utf16().count()).map_err(|_| {
        ShadowError::PositionOutOfRange {
            position: u32::try_from(byte).unwrap_or(u32::MAX),
            length: u32::MAX,
            unit: "utf-8",
        }
    })
}

/// One mounted expression with absolute derived coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountRecord {
    /// Stable mount identifier (`fragment#0`).
    pub mount_id: String,
    /// Fragment identifier.
    pub fragment_id: String,
    /// Insertion that contains this mount.
    pub insertion_id: String,
    /// Mounted expression text.
    pub text: String,
    /// Inclusive UTF-16 start in the derived script.
    pub start_utf16: u32,
    /// Target identifier start, relative to the derived script.
    pub target_start_utf16: u32,
    /// Target identifier length in UTF-16 code units.
    pub target_length_utf16: u32,
    /// Segments of [`Self::text`], absolute in the derived script.
    pub segments: Vec<MountedSegment>,
}

impl MountRecord {
    /// Returns the exclusive end of the mount in the derived script.
    ///
    /// The mount occupies `[start_utf16, end)`. A range that starts exactly at
    /// this offset is outside the mount.
    ///
    /// Overflow is a typed error so a malformed mount can never wrap around and
    /// publish an in-range-looking mapping.
    pub fn end_utf16(&self) -> Result<u32, ShadowError> {
        self.start_utf16
            .checked_add(utf16_len(&self.text))
            .ok_or(ShadowError::PositionOutOfRange {
                position: self.start_utf16,
                length: u32::MAX,
                unit: "utf-16",
            })
    }
}

/// Result of mapping one derived span back to WXML bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappedSpan {
    /// Mapped WXML byte range, when attributable.
    pub range: Option<SourceRange>,
    /// The span started inside synthetic text and was clamped to source text.
    pub trimmed_synthetic_prefix: bool,
    /// The span cannot be attributed to original WXML text.
    pub unattributable: bool,
}

/// Maps one span of a mounted expression back to WXML bytes.
///
/// Rules (ported from the TypeScript oracle):
///
/// - a span entirely inside synthetic text is unattributable;
/// - a span that starts in synthetic text is clamped to the following source
///   segment;
/// - a span crossing a synthetic gap is cropped to the target identifier when
///   the target is fully inside the span, otherwise to the largest-overlap
///   source segment;
/// - out-of-bounds spans and boundaries that split a surrogate pair are typed
///   errors instead of a silent clamp.
#[allow(clippy::too_many_lines)] // one linear mapping algorithm; helpers live in map_within
pub fn map_span_to_source(
    mount: &MountRecord,
    start: Utf16Offset,
    length: u32,
) -> Result<MappedSpan, ShadowError> {
    let mount_end = mount.end_utf16()?;
    for segment in &mount.segments {
        let segment_end = segment
            .start_utf16
            .checked_add(segment.length_utf16)
            .ok_or(ShadowError::PositionOutOfRange {
                position: segment.start_utf16,
                length: mount_end,
                unit: "utf-16",
            })?;
        if segment.start_utf16 < mount.start_utf16 || segment_end > mount_end {
            return Err(ShadowError::PositionOutOfRange {
                position: segment.start_utf16,
                length: mount_end,
                unit: "utf-16",
            });
        }
    }
    if mount.target_length_utf16 > 0 {
        let target_end = mount
            .target_start_utf16
            .checked_add(mount.target_length_utf16)
            .ok_or(ShadowError::PositionOutOfRange {
                position: mount.target_start_utf16,
                length: mount_end,
                unit: "utf-16",
            })?;
        if mount.target_start_utf16 < mount.start_utf16 || target_end > mount_end {
            return Err(ShadowError::PositionOutOfRange {
                position: mount.target_start_utf16,
                length: mount_end,
                unit: "utf-16",
            });
        }
    }
    let source_segments: Vec<&MountedSegment> = mount
        .segments
        .iter()
        .filter(|segment| segment.source.is_some())
        .collect();
    if source_segments.is_empty() {
        return Ok(MappedSpan {
            range: None,
            trimmed_synthetic_prefix: false,
            unattributable: true,
        });
    }
    let start_units = start.get();
    let end = start_units
        .checked_add(length)
        .ok_or(ShadowError::PositionOutOfRange {
            position: start_units,
            length: mount_end,
            unit: "utf-16",
        })?;
    if start_units < mount.start_utf16 || end > mount_end {
        return Err(ShadowError::PositionOutOfRange {
            position: start_units,
            length: mount_end,
            unit: "utf-16",
        });
    }

    let mut effective_start = start_units;
    let synthetic = mount.segments.iter().find(|segment| {
        segment.source.is_none()
            && effective_start >= segment.start_utf16
            && effective_start < segment.start_utf16 + segment.length_utf16
    });
    let trimmed = synthetic.is_some();
    if let Some(synthetic) = synthetic {
        effective_start = synthetic.start_utf16 + synthetic.length_utf16;
    }
    if end <= effective_start {
        return Ok(MappedSpan {
            range: None,
            trimmed_synthetic_prefix: trimmed,
            unattributable: true,
        });
    }

    for segment in &source_segments {
        let segment_end = segment.start_utf16 + segment.length_utf16;
        if effective_start >= segment.start_utf16 && end <= segment_end {
            return map_within(mount, segment, effective_start, end, trimmed, mount_end);
        }
    }

    if mount.target_length_utf16 > 0 {
        let target_start = mount.target_start_utf16;
        let target_end = target_start.checked_add(mount.target_length_utf16).ok_or(
            ShadowError::PositionOutOfRange {
                position: target_start,
                length: mount_end,
                unit: "utf-16",
            },
        )?;
        if target_start < mount.start_utf16 || target_end > mount_end {
            return Err(ShadowError::PositionOutOfRange {
                position: target_start,
                length: mount_end,
                unit: "utf-16",
            });
        }
        if target_start >= start_units && target_end <= end {
            if let Some(segment) = source_segments.iter().find(|segment| {
                target_start >= segment.start_utf16
                    && target_end <= segment.start_utf16 + segment.length_utf16
            }) {
                let mapped = map_within(mount, segment, target_start, target_end, true, mount_end)?;
                if mapped.range.is_some() {
                    return Ok(mapped);
                }
            }
        }
    }

    let mut overlaps: Vec<(&MountedSegment, u32, u32, u32)> = source_segments
        .iter()
        .map(|segment| {
            let segment_end = segment.start_utf16 + segment.length_utf16;
            let from = effective_start.max(segment.start_utf16);
            let to = end.min(segment_end);
            (*segment, from, to, to.saturating_sub(from))
        })
        .filter(|(_, _, _, overlap)| *overlap > 0)
        .collect();
    overlaps.sort_by_key(|(_, _, _, overlap)| std::cmp::Reverse(*overlap));
    for (segment, from, to, _) in overlaps {
        let mapped = map_within(mount, segment, from, to, true, mount_end)?;
        if mapped.range.is_some() {
            return Ok(mapped);
        }
    }

    Ok(MappedSpan {
        range: None,
        trimmed_synthetic_prefix: trimmed,
        unattributable: true,
    })
}

fn map_within(
    mount: &MountRecord,
    segment: &MountedSegment,
    from: u32,
    to: u32,
    trimmed: bool,
    mount_end: u32,
) -> Result<MappedSpan, ShadowError> {
    let Some(source) = segment.source else {
        return Ok(MappedSpan {
            range: None,
            trimmed_synthetic_prefix: trimmed,
            unattributable: true,
        });
    };
    if to <= from {
        return Ok(MappedSpan {
            range: None,
            trimmed_synthetic_prefix: trimmed,
            unattributable: true,
        });
    }
    let segment_start_units = segment.start_utf16.checked_sub(mount.start_utf16).ok_or(
        ShadowError::PositionOutOfRange {
            position: segment.start_utf16,
            length: mount_end,
            unit: "utf-16",
        },
    )?;
    let from_relative =
        from.checked_sub(mount.start_utf16)
            .ok_or(ShadowError::PositionOutOfRange {
                position: from,
                length: mount_end,
                unit: "utf-16",
            })?;
    let to_relative = to
        .checked_sub(mount.start_utf16)
        .ok_or(ShadowError::PositionOutOfRange {
            position: to,
            length: mount_end,
            unit: "utf-16",
        })?;
    let segment_start_byte = utf16_to_byte(&mount.text, segment_start_units)?;
    let from_byte = utf16_to_byte(&mount.text, from_relative)?;
    let to_byte = utf16_to_byte(&mount.text, to_relative)?;
    let start_offset = from_byte.saturating_sub(segment_start_byte);
    let end_offset = to_byte.saturating_sub(segment_start_byte);
    let needed = u32::try_from(end_offset.saturating_sub(start_offset)).unwrap_or(u32::MAX);
    let available = source.len();
    if available < needed {
        return Err(ShadowError::MappingSourceMismatch { needed, available });
    }
    let start = source
        .start
        .get()
        .saturating_add(u32::try_from(start_offset).unwrap_or(u32::MAX));
    let end = source
        .start
        .get()
        .saturating_add(u32::try_from(end_offset).unwrap_or(u32::MAX));
    Ok(MappedSpan {
        range: Some(SourceRange::new(start, end)),
        trimmed_synthetic_prefix: trimmed,
        unattributable: false,
    })
}

/// Original ⇄ derived script map built from an assembled shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowMap {
    original: String,
    derived: String,
    edits: Vec<crate::shadow::rewrite::AppliedEdit>,
}

impl ShadowMap {
    /// Creates a map from the original text and the assembler result.
    #[must_use]
    pub fn new(original: impl Into<String>, assembled: &Assembled) -> Self {
        Self {
            original: original.into(),
            derived: assembled.text.clone(),
            edits: assembled.edits.clone(),
        }
    }

    /// Returns the original text.
    #[must_use]
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Returns the derived text.
    #[must_use]
    pub fn derived(&self) -> &str {
        &self.derived
    }

    /// Returns the applied edits in original order.
    #[must_use]
    pub fn edits(&self) -> &[crate::shadow::rewrite::AppliedEdit] {
        &self.edits
    }

    /// Maps a derived UTF-16 offset back to the original script.
    pub fn to_original_offset(&self, derived: Utf16Offset) -> Result<Utf16Offset, ShadowError> {
        let derived_byte = utf16_to_byte(&self.derived, derived.get())?;
        let original_byte = self.to_original_byte(derived_byte, derived.get())?;
        let units = byte_to_utf16(&self.original, original_byte)?;
        Ok(Utf16Offset::new(units))
    }

    /// Maps an original UTF-16 offset into the derived script.
    ///
    /// An offset exactly at an insertion point, or inside removed text, is
    /// ambiguous and rejected.
    pub fn to_derived_offset(&self, original: Utf16Offset) -> Result<Utf16Offset, ShadowError> {
        let original_byte = utf16_to_byte(&self.original, original.get())?;
        let derived_byte = self.to_derived_byte(original_byte, original.get())?;
        let units = byte_to_utf16(&self.derived, derived_byte)?;
        Ok(Utf16Offset::new(units))
    }

    fn to_original_byte(
        &self,
        derived_byte: usize,
        derived_units: u32,
    ) -> Result<usize, ShadowError> {
        if derived_byte > self.derived.len() {
            return Err(ShadowError::PositionOutOfRange {
                position: derived_units,
                length: utf16_len(&self.derived),
                unit: "utf-16",
            });
        }
        let mut original_cursor = 0;
        let mut derived_cursor = 0;
        for edit in &self.edits {
            let unchanged = edit.at_byte.saturating_sub(original_cursor);
            if derived_byte < derived_cursor + unchanged {
                return Ok(original_cursor + (derived_byte - derived_cursor));
            }
            derived_cursor += unchanged;
            if derived_byte < derived_cursor + edit.inserted_length_bytes {
                return Err(ShadowError::MappingInsertedText {
                    position: derived_units,
                });
            }
            derived_cursor += edit.inserted_length_bytes;
            original_cursor = edit.at_byte + edit.replace_length_bytes;
        }
        Ok(original_cursor + derived_byte.saturating_sub(derived_cursor))
    }

    fn to_derived_byte(
        &self,
        original_byte: usize,
        original_units: u32,
    ) -> Result<usize, ShadowError> {
        if original_byte > self.original.len() {
            return Err(ShadowError::PositionOutOfRange {
                position: original_units,
                length: utf16_len(&self.original),
                unit: "utf-16",
            });
        }
        for edit in &self.edits {
            if edit.at_byte == original_byte {
                return Err(ShadowError::MappingAmbiguous {
                    position: original_units,
                });
            }
        }
        self.to_derived_byte_at(original_byte, original_units, false)
    }

    fn to_derived_byte_at(
        &self,
        original_byte: usize,
        original_units: u32,
        prefer_before_insertion: bool,
    ) -> Result<usize, ShadowError> {
        let mut original_cursor = 0;
        let mut derived_cursor = 0;
        for edit in &self.edits {
            if original_byte < edit.at_byte {
                return Ok(derived_cursor + (original_byte - original_cursor));
            }
            if original_byte == edit.at_byte {
                if edit.replace_length_bytes > 0 {
                    return Err(ShadowError::MappingAmbiguous {
                        position: original_units,
                    });
                }
                let before = derived_cursor + (edit.at_byte - original_cursor);
                return Ok(if prefer_before_insertion {
                    before
                } else {
                    before + edit.inserted_length_bytes
                });
            }
            if original_byte < edit.at_byte + edit.replace_length_bytes {
                return Err(ShadowError::MappingRemovedText {
                    position: original_units,
                });
            }
            derived_cursor += (edit.at_byte - original_cursor) + edit.inserted_length_bytes;
            original_cursor = edit.at_byte + edit.replace_length_bytes;
        }
        Ok(derived_cursor + (original_byte - original_cursor))
    }

    fn derived_range_to_original(&self, range: SourceRange) -> Result<SourceRange, ShadowError> {
        let start = usize::try_from(range.start.get()).unwrap_or(usize::MAX);
        let end = usize::try_from(range.end.get()).unwrap_or(usize::MAX);
        if end < start || end > self.derived.len() {
            return Err(ShadowError::PositionOutOfRange {
                position: range.end.get(),
                length: u32::try_from(self.derived.len()).unwrap_or(u32::MAX),
                unit: "utf-8",
            });
        }
        for edit in &self.edits {
            let inserted_start = edit.inserted_start_byte;
            let inserted_end = inserted_start + edit.inserted_length_bytes;
            if start < inserted_end && end > inserted_start {
                return Err(ShadowError::MappingCrossesEdit);
            }
        }
        let start_units = byte_to_utf16(
            &self.derived,
            usize::try_from(range.start.get()).unwrap_or(usize::MAX),
        )?;
        let end_units = byte_to_utf16(
            &self.derived,
            usize::try_from(range.end.get()).unwrap_or(usize::MAX),
        )?;
        let original_start = self.to_original_byte(start, start_units)?;
        let original_end = self.to_original_byte(end, end_units)?;
        let start = u32::try_from(original_start).map_err(|_| ShadowError::PositionOutOfRange {
            position: u32::MAX,
            length: u32::MAX,
            unit: "utf-8",
        })?;
        let end = u32::try_from(original_end).map_err(|_| ShadowError::PositionOutOfRange {
            position: u32::MAX,
            length: u32::MAX,
            unit: "utf-8",
        })?;
        Ok(SourceRange::new(start, end))
    }

    fn original_range_to_derived(&self, range: SourceRange) -> Result<SourceRange, ShadowError> {
        let start = usize::try_from(range.start.get()).unwrap_or(usize::MAX);
        let end = usize::try_from(range.end.get()).unwrap_or(usize::MAX);
        if end < start || end > self.original.len() {
            return Err(ShadowError::PositionOutOfRange {
                position: range.end.get(),
                length: u32::try_from(self.original.len()).unwrap_or(u32::MAX),
                unit: "utf-8",
            });
        }
        for edit in &self.edits {
            let edit_end = edit.at_byte + edit.replace_length_bytes;
            if start < edit_end && end > edit.at_byte {
                return Err(ShadowError::MappingCrossesEdit);
            }
            if edit.replace_length_bytes == 0
                && edit.at_byte >= start
                && edit.at_byte < end
                && end > start
            {
                return Err(ShadowError::MappingCrossesEdit);
            }
        }
        let start_units = byte_to_utf16(
            &self.original,
            usize::try_from(range.start.get()).unwrap_or(usize::MAX),
        )?;
        let end_units = byte_to_utf16(
            &self.original,
            usize::try_from(range.end.get()).unwrap_or(usize::MAX),
        )?;
        if start == end {
            if self.edits.iter().any(|edit| edit.at_byte == start) {
                return Err(ShadowError::MappingAmbiguous {
                    position: start_units,
                });
            }
            let mapped = self.to_derived_byte_at(start, start_units, false)?;
            let mapped = u32::try_from(mapped).map_err(|_| ShadowError::PositionOutOfRange {
                position: u32::MAX,
                length: u32::MAX,
                unit: "utf-8",
            })?;
            return Ok(SourceRange::new(mapped, mapped));
        }
        let derived_start = self.to_derived_byte_at(start, start_units, false)?;
        let derived_end = self.to_derived_byte_at(end, end_units, true)?;
        let start = u32::try_from(derived_start).map_err(|_| ShadowError::PositionOutOfRange {
            position: u32::MAX,
            length: u32::MAX,
            unit: "utf-8",
        })?;
        let end = u32::try_from(derived_end).map_err(|_| ShadowError::PositionOutOfRange {
            position: u32::MAX,
            length: u32::MAX,
            unit: "utf-8",
        })?;
        Ok(SourceRange::new(start, end))
    }
}

impl ShadowMapper for ShadowMap {
    fn to_original(
        &self,
        layer: MappingLayer,
        range: SourceRange,
    ) -> Result<SourceRange, ShadowError> {
        match layer {
            MappingLayer::Shadow => self.derived_range_to_original(range),
            MappingLayer::OriginalScript => Ok(range),
            MappingLayer::Template | MappingLayer::ExpressionCopy => {
                Err(ShadowError::MappingUnsupportedLayer(layer))
            }
        }
    }

    fn to_shadow(
        &self,
        layer: MappingLayer,
        range: SourceRange,
    ) -> Result<SourceRange, ShadowError> {
        match layer {
            MappingLayer::Shadow => Ok(range),
            MappingLayer::OriginalScript => self.original_range_to_derived(range),
            MappingLayer::Template | MappingLayer::ExpressionCopy => {
                Err(ShadowError::MappingUnsupportedLayer(layer))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::rewrite::{AppliedEdit, EditKind, MountedSegment, ShadowEdit, apply_edits};

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::new(start, end)
    }

    fn segment(start: u32, length: u32, source: Option<SourceRange>) -> MountedSegment {
        MountedSegment {
            start_utf16: start,
            length_utf16: length,
            source,
        }
    }

    fn mount(
        start: u32,
        text: &str,
        segments: Vec<MountedSegment>,
        target: Option<(u32, u32)>,
    ) -> MountRecord {
        MountRecord {
            mount_id: "f#0".to_owned(),
            fragment_id: "f".to_owned(),
            insertion_id: "probe".to_owned(),
            text: text.to_owned(),
            start_utf16: start,
            target_start_utf16: target.map_or(0, |(start, _)| start),
            target_length_utf16: target.map_or(0, |(_, length)| length),
            segments,
        }
    }

    #[test]
    fn utf16_and_byte_offsets_agree_around_astral_characters() {
        let text = "😀中文\nabc";
        assert_eq!(utf16_to_byte(text, 0).expect("boundary"), 0);
        assert_eq!(utf16_to_byte(text, 2).expect("boundary"), 4);
        assert_eq!(utf16_to_byte(text, 3).expect("boundary"), 7);
        assert!(matches!(
            utf16_to_byte(text, 1),
            Err(ShadowError::NotCharBoundary { .. })
        ));
        assert_eq!(byte_to_utf16(text, 4).expect("boundary"), 2);
        assert!(matches!(
            byte_to_utf16(text, 3),
            Err(ShadowError::NotCharBoundary { .. })
        ));
    }

    #[test]
    fn crlf_and_bom_are_preserved_as_code_units() {
        let text = "\u{FEFF}a\r\n😀";
        assert_eq!(utf16_len(text), 1 + 1 + 2 + 2);
        assert_eq!(utf16_to_byte(text, 2).expect("CR ends"), 4);
        assert_eq!(utf16_to_byte(text, 4).expect("BOM+CRLF"), 4 + 2);
        assert_eq!(byte_to_utf16(text, 4).expect("after CR"), 2);
        assert_eq!(utf16_to_byte(text, 1).expect("after BOM"), 3);
    }

    #[test]
    fn mapped_span_trims_a_synthetic_prefix_and_keeps_the_source_part() {
        let mount = mount(
            100,
            "__wxml_root.maybeUser",
            vec![segment(100, 12, None), segment(112, 9, Some(range(10, 19)))],
            None,
        );
        let mapped = map_span_to_source(&mount, Utf16Offset::new(100), 21).expect("mapped");
        assert_eq!(mapped.range, Some(range(10, 19)));
        assert!(mapped.trimmed_synthetic_prefix);
    }

    #[test]
    fn a_span_entirely_inside_synthetic_text_is_unattributable() {
        let mount = mount(
            0,
            "__wxml_root.title",
            vec![segment(0, 12, None), segment(12, 5, Some(range(3, 8)))],
            None,
        );
        let first = map_span_to_source(&mount, Utf16Offset::new(0), 12).expect("valid");
        assert!(first.unattributable);
        let second = map_span_to_source(&mount, Utf16Offset::new(8), 2).expect("valid");
        assert!(second.unattributable);
        let inside = map_span_to_source(&mount, Utf16Offset::new(14), 2).expect("valid");
        assert_eq!(inside.range, Some(range(5, 7)));
    }

    #[test]
    fn out_of_bounds_and_non_boundary_spans_are_typed_errors() {
        let mount = mount(
            100,
            "__wxml_root.😀",
            vec![segment(100, 12, None), segment(112, 2, Some(range(10, 14)))],
            Some((112, 2)),
        );
        assert!(matches!(
            map_span_to_source(&mount, Utf16Offset::new(100), 15),
            Err(ShadowError::PositionOutOfRange { .. })
        ));
        assert!(matches!(
            map_span_to_source(&mount, Utf16Offset::new(99), 2),
            Err(ShadowError::PositionOutOfRange { .. })
        ));
        assert!(matches!(
            map_span_to_source(&mount, Utf16Offset::new(113), 1),
            Err(ShadowError::NotCharBoundary { .. })
        ));
        assert!(matches!(
            map_span_to_source(&mount, Utf16Offset::new(112), 1),
            Err(ShadowError::NotCharBoundary { .. })
        ));
        let mapped = map_span_to_source(&mount, Utf16Offset::new(112), 2).expect("valid");
        assert_eq!(mapped.range, Some(range(10, 14)));
    }

    #[test]
    fn a_segment_cannot_publish_more_bytes_than_its_source() {
        let mount = mount(
            0,
            "__wxml_root.title",
            vec![segment(0, 12, None), segment(12, 5, Some(range(2, 4)))],
            None,
        );
        assert!(matches!(
            map_span_to_source(&mount, Utf16Offset::new(12), 5),
            Err(ShadowError::MappingSourceMismatch { .. })
        ));
        let mapped = map_span_to_source(&mount, Utf16Offset::new(12), 2).expect("valid");
        assert_eq!(mapped.range, Some(range(2, 4)));
    }

    #[test]
    fn cross_gap_target_is_used_only_when_fully_inside_the_span() {
        let mount = mount(
            0,
            "__wxml_root.a + __wxml_root.b",
            vec![
                segment(0, 12, None),
                segment(12, 1, Some(range(0, 1))),
                segment(13, 3, Some(range(1, 4))),
                segment(16, 12, None),
                segment(28, 1, Some(range(4, 5))),
            ],
            Some((28, 1)),
        );
        let mapped = map_span_to_source(&mount, Utf16Offset::new(10), 8).expect("valid");
        assert!(mapped.trimmed_synthetic_prefix);
        assert_eq!(mapped.range, Some(range(1, 4)));
    }

    #[test]
    fn shadow_offsets_map_through_insertions_and_inserted_text() {
        let original = "abc";
        let edits = vec![ShadowEdit::insert("probe", EditKind::Probe, 0, "x ")];
        let assembled = apply_edits(original, &edits).expect("valid");
        let map = ShadowMap::new(original, &assembled);
        assert!(matches!(
            map.to_original_offset(Utf16Offset::new(1)),
            Err(ShadowError::MappingInsertedText { .. })
        ));
        assert_eq!(
            map.to_original_offset(Utf16Offset::new(2))
                .expect("after insertion"),
            Utf16Offset::new(0)
        );
        assert_eq!(
            map.to_original_offset(Utf16Offset::new(3))
                .expect("shifted"),
            Utf16Offset::new(1)
        );
        assert!(matches!(
            map.to_derived_offset(Utf16Offset::new(0)),
            Err(ShadowError::MappingAmbiguous { .. })
        ));
    }

    #[test]
    fn bom_removal_is_visible_in_both_directions() {
        let original = "\u{FEFF}#!x\nBody({});";
        let edits = vec![ShadowEdit::replace("bom", EditKind::Bom, 0, 1, "")];
        let assembled = apply_edits(original, &edits).expect("valid");
        assert!(assembled.text.starts_with("#!"));
        let map = ShadowMap::new(original, &assembled);
        assert!(matches!(
            map.to_derived_offset(Utf16Offset::new(0)),
            Err(ShadowError::MappingAmbiguous { .. } | ShadowError::MappingRemovedText { .. })
        ));
        assert_eq!(
            map.to_derived_offset(Utf16Offset::new(1))
                .expect("after BOM"),
            Utf16Offset::new(0)
        );
        assert_eq!(
            map.to_original_offset(Utf16Offset::new(0))
                .expect("BOM removed"),
            Utf16Offset::new(1)
        );
    }

    #[test]
    fn zero_length_range_at_an_edit_point_is_ambiguous() {
        let original = "abc";
        let edits = vec![ShadowEdit::insert("probe", EditKind::Probe, 1, "X")];
        let assembled = apply_edits(original, &edits).expect("valid");
        let map = ShadowMap::new(original, &assembled);
        assert!(map.to_original_offset(Utf16Offset::new(1)).is_err());
        assert!(map.to_original(MappingLayer::Shadow, range(1, 1)).is_err());
    }

    #[test]
    fn zero_length_original_range_at_an_insertion_is_ambiguous() {
        let original = "abc";
        let edits = vec![ShadowEdit::insert("probe", EditKind::Probe, 1, "XYZ")];
        let assembled = apply_edits(original, &edits).expect("valid");
        let map = ShadowMap::new(original, &assembled);
        assert!(matches!(
            map.to_shadow(MappingLayer::OriginalScript, range(1, 1)),
            Err(ShadowError::MappingAmbiguous { .. })
        ));
    }

    #[test]
    fn shadow_layer_ranges_crossing_edits_are_rejected() {
        let original = "abc";
        let edits = vec![ShadowEdit::insert("probe", EditKind::Probe, 1, "XYZ")];
        let assembled = apply_edits(original, &edits).expect("valid");
        let map = ShadowMap::new(original, &assembled);
        assert!(matches!(
            map.to_shadow(MappingLayer::OriginalScript, range(0, 3)),
            Err(ShadowError::MappingCrossesEdit)
        ));
        assert_eq!(
            map.to_shadow(MappingLayer::OriginalScript, range(0, 1))
                .expect("left of edit"),
            range(0, 1)
        );
        assert!(matches!(
            map.to_original(MappingLayer::Template, range(0, 1)),
            Err(ShadowError::MappingUnsupportedLayer(_))
        ));
    }

    #[test]
    fn applied_edit_keeps_byte_and_utf16_coordinates() {
        let original = "a😀b";
        let edits = vec![ShadowEdit::insert("probe", EditKind::Probe, 1, "x")];
        let assembled = apply_edits(original, &edits).expect("valid");
        let edit: &AppliedEdit = &assembled.edits[0];
        assert_eq!(edit.at_byte, 1);
        assert_eq!(edit.inserted_start_utf16, 1);
        assert_eq!(assembled.text, "ax😀b");
    }
}
