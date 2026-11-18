//! Coordinate and string-pool units crossing the native boundary.
//!
//! The units are deliberately distinct newtypes: native API positions and AST
//! `pos`/`end` are UTF-16 code units, the string pool uses UTF-8/WTF-8 bytes
//! and Rust source ranges are UTF-8 bytes. Conversions happen only against a
//! text snapshot; there are no implicit `From<u32>` impls.

use thiserror::Error;

/// UTF-8 byte offset into Rust source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Utf8Offset(u32);

impl Utf8Offset {
    /// Creates a UTF-8 offset.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw byte offset.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// UTF-16 code-unit offset in native API or AST coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Utf16Offset(u32);

impl Utf16Offset {
    /// Creates a UTF-16 offset.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw code-unit offset.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// UTF-16 line/character position in native coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Utf16Position {
    /// Zero-based line.
    pub line: u32,
    /// Zero-based UTF-16 code-unit column.
    pub character: u32,
}

/// Index into the native AST string pool.
///
/// Wire indices are `uint32` word indices and are always even; the ordinal is
/// `index / 2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringPoolIndex(u32);

impl StringPoolIndex {
    /// Creates a pool index when `index` is a valid even word index.
    #[must_use]
    pub const fn new(index: u32) -> Option<Self> {
        if index % 2 == 0 {
            Some(Self(index))
        } else {
            None
        }
    }

    /// Returns the raw word index.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Returns the ordinal used to address the string table.
    #[must_use]
    pub const fn ordinal(self) -> u32 {
        self.0 / 2
    }
}

/// Failure to convert between UTF-16 and UTF-8 units for one text snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TextConversionError {
    /// A line number does not exist in the text.
    #[error("line {line} does not exist")]
    LineOutOfRange {
        /// Requested zero-based line.
        line: u32,
    },
    /// A character column is past the end of its line.
    #[error("character {character} is past the end of line {line}")]
    CharacterOutOfRange {
        /// Zero-based line.
        line: u32,
        /// Requested UTF-16 column.
        character: u32,
    },
    /// The offset is past the end of the text.
    #[error("offset {offset} is past the end of the text")]
    OffsetOutOfRange {
        /// Requested UTF-16 offset.
        offset: u32,
    },
    /// The offset points between the two units of a surrogate pair.
    #[error("offset {offset} splits a surrogate pair")]
    MidSurrogate {
        /// Requested UTF-16 offset.
        offset: u32,
    },
    /// The offset points inside a `\r\n` pair.
    #[error("offset {offset} is inside a CRLF pair")]
    MidNewline {
        /// Requested UTF-16 offset.
        offset: u32,
    },
    /// A UTF-8 offset is not on a character boundary.
    #[error("UTF-8 offset {offset} is not a character boundary")]
    NotCharBoundary {
        /// Requested UTF-8 byte offset.
        offset: u32,
    },
}

/// Returns the length of `text` in UTF-16 code units.
#[must_use]
pub fn utf16_len(text: &str) -> u32 {
    text.chars()
        .map(|character| u32::try_from(character.len_utf16()).unwrap_or(0))
        .fold(0u32, u32::saturating_add)
}

/// Converts a zero-based line/UTF-16 column to a UTF-16 offset.
///
/// `\n` separates lines. A trailing `\r` before `\n` belongs to the line
/// ending and is not addressable as a column, matching the C0 line index; a
/// lone `\r` stays a normal character.
///
/// # Errors
///
/// Returns [`TextConversionError`] when the line does not exist, the column is
/// past the line content, or the column splits a surrogate pair.
pub fn position_to_offset(
    text: &str,
    position: Utf16Position,
) -> Result<Utf16Offset, TextConversionError> {
    if text.is_empty() {
        return if position.line == 0 && position.character == 0 {
            Ok(Utf16Offset::new(0))
        } else if position.line == 0 {
            Err(TextConversionError::CharacterOutOfRange {
                line: 0,
                character: position.character,
            })
        } else {
            Err(TextConversionError::LineOutOfRange {
                line: position.line,
            })
        };
    }

    let mut offset = 0u32;
    let mut line = 0u32;

    for segment in text.split_inclusive('\n') {
        let line_start = offset;
        if line == position.line {
            let content = segment.strip_suffix('\n').unwrap_or(segment);
            let content = content.strip_suffix('\r').unwrap_or(content);
            let mut column = 0u32;
            for character in content.chars() {
                if column == position.character {
                    return Ok(Utf16Offset::new(offset));
                }
                let width = u32::try_from(character.len_utf16()).unwrap_or(0);
                if position.character < column.saturating_add(width) {
                    return Err(TextConversionError::MidSurrogate {
                        offset: line_start.saturating_add(position.character),
                    });
                }
                column = column.saturating_add(width);
                offset = offset.saturating_add(width);
            }
            if column == position.character {
                return Ok(Utf16Offset::new(offset));
            }
            return Err(TextConversionError::CharacterOutOfRange {
                line,
                character: position.character,
            });
        }

        offset = offset.saturating_add(utf16_len(segment));
        line = line.saturating_add(1);
    }

    // Only a text ending in `\n` has an extra empty final line.
    if text.ends_with('\n') && line == position.line && position.character == 0 {
        return Ok(Utf16Offset::new(offset));
    }

    Err(TextConversionError::LineOutOfRange {
        line: position.line,
    })
}

/// Converts a UTF-16 offset to a zero-based line/UTF-16 column.
///
/// The offset must be on a UTF-16 code-unit boundary and within the text. A
/// `\r` before `\n` advances the offset but not the column; `\n` starts a new
/// line.
///
/// # Errors
///
/// Returns [`TextConversionError`] when the offset is past the end of the text
/// or splits a surrogate pair.
pub fn offset_to_position(
    text: &str,
    offset: Utf16Offset,
) -> Result<Utf16Position, TextConversionError> {
    let target = offset.get();
    let mut consumed = 0u32;
    let mut line = 0u32;
    let mut character = 0u32;
    let mut pending_cr = false;

    let mut characters = text.chars().peekable();
    while let Some(character_value) = characters.next() {
        if consumed == target {
            if pending_cr && character_value == '\n' {
                return Err(TextConversionError::MidNewline { offset: target });
            }
            return Ok(Utf16Position { line, character });
        }
        let width = u32::try_from(character_value.len_utf16()).unwrap_or(0);
        let next = consumed.saturating_add(width);
        if target < next {
            return Err(TextConversionError::MidSurrogate { offset: target });
        }
        consumed = next;

        match character_value {
            '\n' => {
                line = line.saturating_add(1);
                character = 0;
                pending_cr = false;
            }
            '\r' if characters.peek() == Some(&'\n') => {
                pending_cr = true;
            }
            _ => {
                character = character.saturating_add(width);
                pending_cr = false;
            }
        }
    }

    if consumed == target {
        return Ok(Utf16Position { line, character });
    }

    Err(TextConversionError::OffsetOutOfRange { offset: target })
}

/// Converts a UTF-16 offset to a UTF-8 byte offset.
///
/// # Errors
///
/// Returns [`TextConversionError`] for an out-of-range offset or one that
/// splits a surrogate pair.
pub fn utf16_to_utf8(text: &str, offset: Utf16Offset) -> Result<Utf8Offset, TextConversionError> {
    let target = offset.get();
    let mut consumed = 0u32;
    let mut bytes = 0u32;

    for character in text.chars() {
        if consumed == target {
            return Ok(Utf8Offset::new(bytes));
        }
        let width = u32::try_from(character.len_utf16()).unwrap_or(0);
        let next = consumed.saturating_add(width);
        if target < next {
            return Err(TextConversionError::MidSurrogate { offset: target });
        }
        consumed = next;
        bytes = bytes.saturating_add(u32::try_from(character.len_utf8()).unwrap_or(0));
    }

    if consumed == target {
        return Ok(Utf8Offset::new(bytes));
    }

    Err(TextConversionError::OffsetOutOfRange { offset: target })
}

/// Converts a UTF-8 byte offset to a UTF-16 offset.
///
/// # Errors
///
/// Returns [`TextConversionError`] when the byte offset is past the text or is
/// not on a UTF-8 character boundary.
pub fn utf8_to_utf16(text: &str, offset: Utf8Offset) -> Result<Utf16Offset, TextConversionError> {
    let target = offset.get();
    let mut utf8 = 0u32;
    let mut utf16 = 0u32;

    for character in text.chars() {
        if utf8 == target {
            return Ok(Utf16Offset::new(utf16));
        }
        let next = utf8.saturating_add(u32::try_from(character.len_utf8()).unwrap_or(0));
        if target < next {
            return Err(TextConversionError::NotCharBoundary { offset: target });
        }
        utf8 = next;
        utf16 = utf16.saturating_add(u32::try_from(character.len_utf16()).unwrap_or(0));
    }

    if utf8 == target {
        return Ok(Utf16Offset::new(utf16));
    }

    Err(TextConversionError::OffsetOutOfRange { offset: target })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(line: u32, character: u32) -> Utf16Position {
        Utf16Position { line, character }
    }

    #[test]
    fn converts_ascii_and_cjk_positions() {
        let text = "const 名字 = 1;\n值;\n";
        assert_eq!(
            position_to_offset(text, position(0, 6)).expect("cjk start"),
            Utf16Offset::new(6)
        );
        assert_eq!(
            utf16_to_utf8(text, Utf16Offset::new(8)).expect("cjk end"),
            Utf8Offset::new(12)
        );
        assert_eq!(
            utf8_to_utf16(text, Utf8Offset::new(12)).expect("cjk end"),
            Utf16Offset::new(8)
        );
        assert_eq!(
            offset_to_position(text, Utf16Offset::new(14)).expect("line 1"),
            position(1, 0)
        );
    }

    #[test]
    fn converts_non_bmp_and_rejects_mid_surrogate() {
        let text = "😀x";
        assert_eq!(utf16_len(text), 3);
        assert_eq!(
            position_to_offset(text, position(0, 2)).expect("after emoji"),
            Utf16Offset::new(2)
        );
        assert_eq!(
            position_to_offset(text, position(0, 1)).expect_err("mid emoji"),
            TextConversionError::MidSurrogate { offset: 1 }
        );
        assert_eq!(
            utf16_to_utf8(text, Utf16Offset::new(1)).expect_err("mid"),
            TextConversionError::MidSurrogate { offset: 1 }
        );
        assert_eq!(
            utf8_to_utf16(text, Utf8Offset::new(1)).expect_err("mid utf8"),
            TextConversionError::NotCharBoundary { offset: 1 }
        );
        assert_eq!(
            offset_to_position(text, Utf16Offset::new(1)).expect_err("mid"),
            TextConversionError::MidSurrogate { offset: 1 }
        );
    }

    #[test]
    fn handles_crlf_and_lone_cr() {
        let text = "ab\r\ncd\rx";
        assert_eq!(
            position_to_offset(text, position(0, 2)).expect("before crlf"),
            Utf16Offset::new(2)
        );
        assert_eq!(
            offset_to_position(text, Utf16Offset::new(2)).expect("at cr"),
            position(0, 2)
        );
        assert_eq!(
            offset_to_position(text, Utf16Offset::new(3)).expect_err("inside crlf"),
            TextConversionError::MidNewline { offset: 3 }
        );
        assert_eq!(
            offset_to_position(text, Utf16Offset::new(4)).expect("after crlf"),
            position(1, 0)
        );
        // A lone `\r` is an ordinary character and advances the column.
        let lone = "ab\rcd";
        assert_eq!(
            offset_to_position(lone, Utf16Offset::new(3)).expect("lone cr"),
            position(0, 3)
        );
        // Only a trailing newline creates an extra empty line.
        assert_eq!(
            position_to_offset("a\n", position(1, 0)).expect("empty final line"),
            Utf16Offset::new(2)
        );
        assert!(matches!(
            position_to_offset("a\nb", position(2, 0)),
            Err(TextConversionError::LineOutOfRange { .. })
        ));
    }

    #[test]
    fn rejects_out_of_range_inputs() {
        let text = "a\nb";
        assert!(matches!(
            position_to_offset(text, position(2, 0)),
            Err(TextConversionError::LineOutOfRange { .. })
        ));
        assert!(matches!(
            position_to_offset(text, position(0, 2)),
            Err(TextConversionError::CharacterOutOfRange { .. })
        ));
        assert!(matches!(
            offset_to_position(text, Utf16Offset::new(4)),
            Err(TextConversionError::OffsetOutOfRange { .. })
        ));
        assert!(matches!(
            utf16_to_utf8(text, Utf16Offset::new(4)),
            Err(TextConversionError::OffsetOutOfRange { .. })
        ));
        assert!(matches!(
            utf8_to_utf16(text, Utf8Offset::new(4)),
            Err(TextConversionError::OffsetOutOfRange { .. })
        ));
    }
}
