//! Mapping between UTF-8 byte offsets and UTF-16 line/column positions.
//!
//! The Language Server Protocol reports positions as a zero-based line plus a
//! column measured in UTF-16 code units, while Rust strings are indexed by
//! UTF-8 bytes. [`LineIndex`] owns one immutable text snapshot and bridges the
//! two, so an index can never be applied to a different text.
//!
//! Line endings: `\n` separates lines. The `\r` of a `\r\n` pair belongs to
//! the line ending and is not counted in columns; a lone `\r` is an ordinary
//! character. Offsets inside a line terminator report the position just after
//! the line content instead of a column that includes `\r`.

use std::sync::Arc;

/// A zero-based line and UTF-16 column, mirroring the LSP `Position`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based column, measured in UTF-16 code units.
    pub character: u32,
}

impl Position {
    /// Creates a position from a line and a UTF-16 column.
    #[must_use]
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// An immutable text snapshot with the byte offsets of every line start.
///
/// Build one index per document revision and query it without passing the
/// text again. Cloning is cheap: the text is shared through [`Arc`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineIndex {
    text: Arc<str>,
    line_starts: Vec<usize>,
}

impl LineIndex {
    /// Builds an index that owns `text`.
    #[must_use]
    pub fn new(text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        Self { text, line_starts }
    }

    /// Returns the indexed text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the number of lines.
    ///
    /// A trailing newline starts a final, empty line.
    #[must_use]
    pub const fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Converts a byte offset into a [`Position`].
    ///
    /// Returns `None` when `offset` is out of bounds or not on a UTF-8
    /// character boundary. Offsets inside `\r\n` or `\n` map to the end of the
    /// line content; an offset at a line start maps to column `0` of that line.
    #[must_use]
    pub fn position(&self, offset: usize) -> Option<Position> {
        if offset > self.text.len() || !self.text.is_char_boundary(offset) {
            return None;
        }
        let line = self.line_index(offset);
        let line_start = *self.line_starts.get(line)?;
        let line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        let measured_end = offset.min(self.content_end(line_start, line_end));
        let line = u32::try_from(line).ok()?;
        let character = u32::try_from(
            self.text
                .get(line_start..measured_end)?
                .encode_utf16()
                .count(),
        )
        .ok()?;
        Some(Position { line, character })
    }

    /// Converts a [`Position`] into a byte offset.
    ///
    /// Returns `None` when the line or column does not exist, including a
    /// column past the line content or one that would split a surrogate pair.
    /// A column equal to the content length maps to the `\r` of `\r\n` or to
    /// the line end.
    #[must_use]
    pub fn offset(&self, position: Position) -> Option<usize> {
        let line = usize::try_from(position.line).ok()?;
        let line_start = *self.line_starts.get(line)?;
        let line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        let content_end = self.content_end(line_start, line_end);
        let content = self.text.get(line_start..content_end)?;
        let mut column = 0_u32;
        for (relative, character) in content.char_indices() {
            if column == position.character {
                return Some(line_start + relative);
            }
            column += u32::try_from(character.len_utf16()).ok()?;
            if column > position.character {
                return None;
            }
        }
        (column == position.character).then_some(content_end)
    }

    /// Returns the index of the line containing `offset`.
    fn line_index(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// Returns the end of the line content, excluding `\r\n` or `\n`.
    fn content_end(&self, line_start: usize, line_end: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut end = line_end;
        if end > line_start && bytes.get(end - 1) == Some(&b'\n') {
            end -= 1;
            if end > line_start && bytes.get(end - 1) == Some(&b'\r') {
                end -= 1;
            }
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn pos(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    fn arbitrary_text() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![
                prop::collection::vec(prop::char::any(), 0..=8)
                    .prop_map(|chars| chars.into_iter().collect::<String>()),
                Just("\r".to_owned()),
                Just("\r\n".to_owned()),
                Just("\n".to_owned()),
            ],
            0..=20,
        )
        .prop_map(|parts| parts.concat())
    }

    #[test]
    fn indexes_empty_text_as_a_single_line() {
        let index = LineIndex::new("");
        assert_eq!(index.text(), "");
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.position(0), Some(pos(0, 0)));
        assert_eq!(index.offset(pos(0, 0)), Some(0));
        assert_eq!(index.offset(pos(0, 1)), None);
    }

    #[test]
    fn converts_ascii_offsets() {
        let text = "hello\nworld\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.position(0), Some(pos(0, 0)));
        assert_eq!(index.position(5), Some(pos(0, 5)));
        assert_eq!(index.position(6), Some(pos(1, 0)));
        assert_eq!(index.position(11), Some(pos(1, 5)));
        assert_eq!(index.position(12), Some(pos(2, 0)));
        assert_eq!(index.offset(pos(1, 3)), Some(9));
        assert_eq!(index.offset(pos(2, 0)), Some(12));
    }

    #[test]
    fn counts_utf16_units_for_cjk_and_emoji() {
        let text = "中文😀a\n";
        let index = LineIndex::new(text);
        // "中文" = 2 units, "😀" = 2 units, "a" = 1 unit.
        assert_eq!(index.position("中文".len()), Some(pos(0, 2)));
        assert_eq!(index.position("中文😀".len()), Some(pos(0, 4)));
        assert_eq!(index.position(text.len() - 1), Some(pos(0, 5)));
        assert_eq!(index.offset(pos(0, 2)), Some(6));
        assert_eq!(index.offset(pos(0, 4)), Some(10));
        assert_eq!(index.offset(pos(0, 5)), Some(11));
    }

    #[test]
    fn rejects_positions_inside_a_surrogate_pair() {
        let text = "😀";
        let index = LineIndex::new(text);
        assert_eq!(index.offset(pos(0, 1)), None);
        assert_eq!(index.offset(pos(0, 2)), Some(4));
    }

    #[test]
    fn rejects_out_of_bounds_input() {
        let text = "a\nb";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text.len() + 1), None);
        assert_eq!(index.position(2), Some(pos(1, 0)));
        assert_eq!(index.offset(pos(2, 0)), None);
        assert_eq!(index.offset(pos(0, 2)), None);
        assert_eq!(index.offset(pos(u32::MAX, 0)), None);
    }

    #[test]
    fn excludes_crlf_line_endings_from_columns() {
        let text = "ab\r\ncd\r\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        // Inside the terminator: report the end of the line content.
        assert_eq!(index.position(2), Some(pos(0, 2)));
        assert_eq!(index.position(3), Some(pos(0, 2)));
        assert_eq!(index.position(4), Some(pos(1, 0)));
        assert_eq!(index.position(6), Some(pos(1, 2)));
        assert_eq!(index.position(7), Some(pos(1, 2)));
        assert_eq!(index.position(8), Some(pos(2, 0)));
        assert_eq!(index.offset(pos(0, 2)), Some(2));
        assert_eq!(index.offset(pos(0, 3)), None);
        assert_eq!(index.offset(pos(1, 2)), Some(6));
        assert_eq!(index.offset(pos(2, 0)), Some(8));
    }

    #[test]
    fn treats_a_lone_cr_as_content() {
        let text = "ab\rcd";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.position(2), Some(pos(0, 2)));
        assert_eq!(index.position(3), Some(pos(0, 3)));
        assert_eq!(index.offset(pos(0, 2)), Some(2));
        assert_eq!(index.offset(pos(0, 3)), Some(3));
        assert_eq!(index.offset(pos(0, 5)), Some(5));
    }

    #[test]
    fn maps_every_offset_of_a_crlf_line_to_a_column() {
        let text = "ab\r\ncd";
        let index = LineIndex::new(text);
        let expected = [
            (0, pos(0, 0)),
            (1, pos(0, 1)),
            (2, pos(0, 2)),
            (3, pos(0, 2)),
            (4, pos(1, 0)),
            (5, pos(1, 1)),
            (6, pos(1, 2)),
        ];
        for (offset, position) in expected {
            assert_eq!(index.position(offset), Some(position), "offset {offset}");
        }
        assert_eq!(index.position(7), None);
    }

    #[test]
    fn handles_a_trailing_newline() {
        let text = "a\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 2);
        assert_eq!(index.position(2), Some(pos(1, 0)));
        assert_eq!(index.offset(pos(1, 0)), Some(2));
        assert_eq!(index.offset(pos(1, 1)), None);
    }

    #[test]
    fn shares_the_snapshot_on_clone() {
        let index = LineIndex::new(String::from("a\nb"));
        let clone = index.clone();
        drop(index);
        assert_eq!(clone.text(), "a\nb");
        assert_eq!(clone.position(2), Some(pos(1, 0)));
    }

    proptest! {
        #[test]
        fn positions_round_trip_for_every_char_boundary(text in arbitrary_text()) {
            let index = LineIndex::new(text.as_str());
            prop_assert_eq!(index.text(), text.as_str());
            for offset in 0..=text.len() {
                if !text.is_char_boundary(offset) {
                    continue;
                }
                let position = index.position(offset).expect("char boundary is in bounds");
                let back = index.offset(position).expect("position is representable");
                prop_assert_eq!(index.position(back), Some(position), "offset {}", offset);
            }
        }

        #[test]
        fn positions_round_trip_through_offsets(
            text in arbitrary_text(),
            line in 0u32..20,
            character in 0u32..40,
        ) {
            let index = LineIndex::new(text.as_str());
            let position = Position::new(line, character);
            if let Some(offset) = index.offset(position) {
                prop_assert_eq!(index.position(offset), Some(position));
            }
        }
    }
}
