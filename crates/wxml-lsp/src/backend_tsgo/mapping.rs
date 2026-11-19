//! Coordinate mapping between WXML bytes, the derived shadow script and native
//! UTF-16 positions.
//!
//! A template expression is mounted as one [`MountRecord`] whose source
//! segments point back into the original WXML bytes. A cursor offset inside a
//! source segment becomes a UTF-16 offset in the derived script; native ranges
//! use the reverse path. Ranges in generated probe code have no WXML owner and
//! are reported as [`MappingError`] gaps instead of being clamped.
//!
//! The [`ShadowMap`] remains the authority for derived-script ranges that are
//! outside a template mount. This module uses it for those ranges so a native
//! result in original script text is distinguishable from a generated range.

use thiserror::Error;
use wxml_backend_tsgo::native::strings::{TextConversionError, Utf16Offset, utf16_len};
use wxml_backend_tsgo::shadow::ShadowError;
use wxml_backend_tsgo::shadow::mapping::{
    MappedSpan, MappingLayer, MountRecord, ShadowMap, ShadowMapper,
    SourceRange as ShadowSourceRange, map_span_to_source, utf16_to_byte,
};

/// A byte range in the original template.
pub(crate) type TemplateRange = wxml_text::SourceRange;

/// One fragment and the mount that carries it in the derived script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FragmentMap {
    /// Original WXML expression range.
    pub(crate) wxml_range: TemplateRange,
    /// Generated mount in derived UTF-16 coordinates.
    pub(crate) mount: MountRecord,
}

/// Original template plus the derived script and its fragment mounts.
#[derive(Debug, Clone)]
pub(crate) struct ShadowLayout {
    original_wxml: String,
    derived: String,
    script_map: ShadowMap,
    fragments: Vec<FragmentMap>,
}

impl ShadowLayout {
    /// Creates a layout from the original template, derived text and mounts.
    #[must_use]
    pub(crate) fn new(
        original_wxml: impl Into<String>,
        derived: impl Into<String>,
        script_map: ShadowMap,
        fragments: Vec<FragmentMap>,
    ) -> Self {
        Self {
            original_wxml: original_wxml.into(),
            derived: derived.into(),
            script_map,
            fragments,
        }
    }

    /// Returns the derived script text.
    #[must_use]
    pub(crate) fn derived(&self) -> &str {
        &self.derived
    }

    /// Returns the original owning-script text before fragment insertion.
    ///
    /// Native ranges in the derived text map back to this text; a range that
    /// cannot be attributed here is dropped instead of clamped.
    #[must_use]
    pub(crate) fn original_script(&self) -> &str {
        self.script_map.original()
    }

    /// Returns the number of mounted template fragments.
    #[must_use]
    pub(crate) fn fragment_count(&self) -> usize {
        self.fragments.len()
    }

    /// Maps a WXML byte offset at a hover/query position into derived UTF-16.
    ///
    /// `allow_end` permits a cursor at the end of an expression, which is the
    /// normal completion position after a token or `.`.
    pub(crate) fn derived_offset(&self, offset: u32, allow_end: bool) -> Result<u32, MappingError> {
        self.validate_source_offset(offset)?;
        for fragment in &self.fragments {
            let in_range = if allow_end {
                fragment.wxml_range.start() <= offset && offset <= fragment.wxml_range.end()
            } else {
                fragment.wxml_range.start() <= offset && offset < fragment.wxml_range.end()
            };
            if in_range {
                return self.fragment_derived_offset(fragment, offset, allow_end);
            }
        }
        Err(MappingError::NoFragment)
    }

    /// Returns the identifier token around `offset`, clipped to its source
    /// segment.
    ///
    /// A cursor directly after an identifier selects that identifier; a cursor
    /// in template text without an identifier returns `None`.
    pub(crate) fn token_range(&self, offset: u32) -> Result<Option<TemplateRange>, MappingError> {
        let hit = self.source_segment_at(offset, true)?;
        let segment = hit.segment_source();
        let start = usize::try_from(segment.start.get()).map_err(|_| MappingError::InvalidRange)?;
        let end = usize::try_from(segment.end.get()).map_err(|_| MappingError::InvalidRange)?;
        let cursor = usize::try_from(offset).map_err(|_| MappingError::InvalidRange)?;
        let Some((token_start, token_end)) =
            identifier_bounds(&self.original_wxml, cursor, start, end)
        else {
            return Ok(None);
        };
        let start = u32::try_from(token_start).map_err(|_| MappingError::InvalidRange)?;
        let end = u32::try_from(token_end).map_err(|_| MappingError::InvalidRange)?;
        self.source_range(start, end).map(Some)
    }

    /// Returns the replacement range for completion at `offset`.
    ///
    /// The range starts at the beginning of the identifier prefix and ends at
    /// the cursor. An empty prefix produces an empty range at the cursor rather
    /// than guessing a token.
    pub(crate) fn completion_replace_range(
        &self,
        offset: u32,
    ) -> Result<TemplateRange, MappingError> {
        let hit = self.source_segment_at(offset, true)?;
        let segment = hit.segment_source();
        let start = usize::try_from(segment.start.get()).map_err(|_| MappingError::InvalidRange)?;
        let cursor = usize::try_from(offset).map_err(|_| MappingError::InvalidRange)?;
        let mut token_start = cursor;
        while token_start > start {
            let Some((previous, character)) =
                self.original_wxml[..token_start].char_indices().next_back()
            else {
                break;
            };
            if previous < start || !is_identifier_character(character) {
                break;
            }
            token_start = previous;
        }
        let start = u32::try_from(token_start).map_err(|_| MappingError::InvalidRange)?;
        self.source_range(start, offset)
    }

    /// Maps a native derived-script range back to WXML or original script.
    pub(crate) fn map_native_range(
        &self,
        start: u32,
        end: u32,
    ) -> Result<NativeRange, MappingError> {
        if end < start {
            return Err(MappingError::InvalidRange);
        }
        let length = utf16_len(&self.derived);
        if end > length {
            return Err(MappingError::OffsetOutOfRange { offset: end });
        }
        // Validate both bounds against the same derived text before mapping.
        utf16_to_byte(&self.derived, start).map_err(MappingError::from)?;
        utf16_to_byte(&self.derived, end).map_err(MappingError::from)?;

        if let Some(mount) = self.mount_at(start) {
            let mount_end = mount.end_utf16()?;
            if end > mount_end {
                return Err(MappingError::MappingGap);
            }
            if start == end {
                let Some(point) = source_point_in_mount(mount, start) else {
                    return Err(MappingError::MappingGap);
                };
                let range = self.source_range(point, point)?;
                return Ok(NativeRange::Template {
                    range,
                    approximate: false,
                });
            }
            let mapped = map_span_to_source(mount, Utf16Offset::new(start), end - start)?;
            return match mapped {
                MappedSpan {
                    range: Some(range),
                    trimmed_synthetic_prefix,
                    unattributable: false,
                } => {
                    let range = self.source_range(range.start.get(), range.end.get())?;
                    Ok(NativeRange::Template {
                        range,
                        approximate: trimmed_synthetic_prefix,
                    })
                }
                _ => Err(MappingError::MappingGap),
            };
        }

        if self.mount_at(end).is_some() {
            return Err(MappingError::MappingGap);
        }
        let start_byte = utf16_to_byte(&self.derived, start).map_err(MappingError::from)?;
        let end_byte = utf16_to_byte(&self.derived, end).map_err(MappingError::from)?;
        let start = u32::try_from(start_byte).map_err(|_| MappingError::InvalidRange)?;
        let end = u32::try_from(end_byte).map_err(|_| MappingError::InvalidRange)?;
        let mapped = self
            .script_map
            .to_original(MappingLayer::Shadow, ShadowSourceRange::new(start, end))
            .map_err(|_| MappingError::MappingGap)?;
        let range = TemplateRange::from_bounds(mapped.start.get(), mapped.end.get())
            .map_err(|_| MappingError::InvalidRange)?;
        Ok(NativeRange::Script { range })
    }

    fn validate_source_offset(&self, offset: u32) -> Result<(), MappingError> {
        let length = u32::try_from(self.original_wxml.len()).unwrap_or(u32::MAX);
        if offset > length {
            return Err(MappingError::OffsetOutOfRange { offset });
        }
        let byte = usize::try_from(offset).map_err(|_| MappingError::InvalidRange)?;
        if !self.original_wxml.is_char_boundary(byte) {
            return Err(MappingError::NotCharBoundary { offset });
        }
        Ok(())
    }

    fn fragment_derived_offset(
        &self,
        fragment: &FragmentMap,
        offset: u32,
        allow_end: bool,
    ) -> Result<u32, MappingError> {
        let mount = &fragment.mount;
        let mut candidates = mount.segments.iter().filter_map(|segment| {
            segment.source.and_then(|source| {
                let inside = source.start.get() <= offset
                    && (offset < source.end.get() || (allow_end && offset == source.end.get()));
                inside.then_some((segment, source))
            })
        });
        let Some((segment, source)) = candidates.next() else {
            return Err(MappingError::MappingGap);
        };
        let relative_start = segment
            .start_utf16
            .checked_sub(mount.start_utf16)
            .ok_or(MappingError::MappingGap)?;
        let segment_start_byte =
            utf16_to_byte(&mount.text, relative_start).map_err(MappingError::from)?;
        let segment_end_byte = utf16_to_byte(
            &mount.text,
            relative_start
                .checked_add(segment.length_utf16)
                .ok_or(MappingError::MappingGap)?,
        )
        .map_err(MappingError::from)?;
        let segment_text = mount
            .text
            .get(segment_start_byte..segment_end_byte)
            .ok_or(MappingError::MappingGap)?;
        let source_text = self
            .original_wxml
            .get(
                usize::try_from(source.start.get()).map_err(|_| MappingError::InvalidRange)?
                    ..usize::try_from(source.end.get()).map_err(|_| MappingError::InvalidRange)?,
            )
            .ok_or(MappingError::MappingGap)?;
        if segment_text != source_text {
            return Err(MappingError::MappingGap);
        }
        let delta = offset
            .checked_sub(source.start.get())
            .ok_or(MappingError::MappingGap)?;
        let delta = usize::try_from(delta).map_err(|_| MappingError::InvalidRange)?;
        let prefix = segment_text.get(..delta).ok_or(MappingError::MappingGap)?;
        let units = utf16_len(prefix);
        segment
            .start_utf16
            .checked_add(units)
            .ok_or(MappingError::MappingGap)
    }

    fn source_segment_at(&self, offset: u32, allow_end: bool) -> Result<SegmentHit, MappingError> {
        self.validate_source_offset(offset)?;
        for fragment in &self.fragments {
            for segment in &fragment.mount.segments {
                let Some(source) = segment.source else {
                    continue;
                };
                let inside = source.start.get() <= offset
                    && (offset < source.end.get() || (allow_end && offset == source.end.get()));
                if inside {
                    return Ok(SegmentHit { source });
                }
            }
        }
        if self
            .fragments
            .iter()
            .any(|fragment| fragment.wxml_range.contains(offset))
        {
            Err(MappingError::MappingGap)
        } else {
            Err(MappingError::NoFragment)
        }
    }

    fn mount_at(&self, offset: u32) -> Option<&MountRecord> {
        self.fragments
            .iter()
            .map(|fragment| &fragment.mount)
            .find(|mount| {
                mount
                    .start_utf16
                    .checked_add(utf16_len(&mount.text))
                    .is_some_and(|end| mount.start_utf16 <= offset && offset <= end)
            })
    }

    fn source_range(&self, start: u32, end: u32) -> Result<TemplateRange, MappingError> {
        if end < start {
            return Err(MappingError::InvalidRange);
        }
        let length = u32::try_from(self.original_wxml.len()).unwrap_or(u32::MAX);
        if end > length {
            return Err(MappingError::InvalidRange);
        }
        let start_byte = usize::try_from(start).map_err(|_| MappingError::InvalidRange)?;
        let end_byte = usize::try_from(end).map_err(|_| MappingError::InvalidRange)?;
        if !self.original_wxml.is_char_boundary(start_byte)
            || !self.original_wxml.is_char_boundary(end_byte)
        {
            return Err(MappingError::InvalidRange);
        }
        TemplateRange::from_bounds(start, end).map_err(|_| MappingError::InvalidRange)
    }
}

/// Where a native derived range belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeRange {
    /// The range maps into a mounted template expression.
    Template {
        /// Mapped WXML byte range.
        range: TemplateRange,
        /// The range was cropped at a synthetic shadow prefix.
        approximate: bool,
    },
    /// The range belongs to the original script, not the template.
    Script {
        /// Mapped byte range in the original script.
        range: TemplateRange,
    },
}

#[derive(Debug, Clone, Copy)]
struct SegmentHit {
    source: ShadowSourceRange,
}

impl SegmentHit {
    const fn segment_source(self) -> ShadowSourceRange {
        self.source
    }
}

fn source_point_in_mount(mount: &MountRecord, offset: u32) -> Option<u32> {
    mount.segments.iter().find_map(|segment| {
        let source = segment.source?;
        let relative_start = segment.start_utf16.checked_sub(mount.start_utf16)?;
        let start = utf16_to_byte(&mount.text, relative_start).ok()?;
        let end = utf16_to_byte(
            &mount.text,
            relative_start.checked_add(segment.length_utf16)?,
        )
        .ok()?;
        if start > end {
            return None;
        }
        let segment_text = mount.text.get(start..end)?;
        let relative = offset.checked_sub(segment.start_utf16)?;
        let relative = usize::try_from(relative).ok()?;
        let byte = utf16_to_byte(segment_text, u32::try_from(relative).ok()?).ok()?;
        let source_byte = source.start.get().checked_add(u32::try_from(byte).ok()?)?;
        (source_byte <= source.end.get()).then_some(source_byte)
    })
}

fn identifier_bounds(text: &str, cursor: usize, min: usize, max: usize) -> Option<(usize, usize)> {
    if cursor < min || cursor > max || cursor > text.len() {
        return None;
    }
    let mut start = cursor;
    let mut end = cursor;
    if cursor < max {
        if let Some(character) = text.get(cursor..)?.chars().next() {
            if is_identifier_character(character) {
                end = cursor.checked_add(character.len_utf8())?;
            }
        }
    }
    if start == end {
        let (previous, character) = text.get(..cursor)?.char_indices().next_back()?;
        if previous < min || !is_identifier_character(character) {
            return None;
        }
        start = previous;
    }
    while start > min {
        let Some((previous, character)) = text[..start].char_indices().next_back() else {
            break;
        };
        if previous < min || !is_identifier_character(character) {
            break;
        }
        start = previous;
    }
    while end < max {
        let Some(character) = text.get(end..)?.chars().next() else {
            break;
        };
        if !is_identifier_character(character) {
            break;
        }
        end = end.checked_add(character.len_utf8())?;
    }
    (start < end).then_some((start, end))
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// Typed mapping failure.
#[derive(Debug, Error)]
pub(crate) enum MappingError {
    /// A source offset is past the template.
    #[error("offset {offset} is outside the template")]
    OffsetOutOfRange {
        /// Rejected byte offset.
        offset: u32,
    },
    /// A source offset splits a UTF-8 code point.
    #[error("offset {offset} is not on a UTF-8 character boundary")]
    NotCharBoundary {
        /// Rejected byte offset.
        offset: u32,
    },
    /// The cursor is not inside a mounted template expression.
    #[error("cursor is not inside a mounted template expression")]
    NoFragment,
    /// The range crosses generated text, a synthetic prefix or a mount gap.
    #[error("range cannot be attributed to original template text")]
    MappingGap,
    /// The final range is not valid original-template bytes.
    #[error("mapped range is not valid template text")]
    InvalidRange,
    /// The shadow layer rejected the text conversion.
    #[error(transparent)]
    Shadow(#[from] ShadowError),
    /// The native text conversion rejected a position.
    #[error(transparent)]
    Text(#[from] TextConversionError),
}
