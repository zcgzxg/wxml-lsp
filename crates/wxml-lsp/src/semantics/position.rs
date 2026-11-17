//! Linear UTF-16 offset mapping for semantic-tool result encoders.
//!
//! `LineIndex::position` rescans from the line start on every call. A single
//! very long line therefore makes token/hint/location encoding quadratic. This
//! cursor advances monotonically over the text once per file and enforces a
//! scan budget for hostile inputs.

use wxml_text::Position;

/// Maximum bytes scanned by one cursor.
pub(crate) const MAX_POSITION_SCAN_BYTES: usize = 16 * 1024 * 1024;

/// A monotonic byte-offset to UTF-16 position mapper.
#[derive(Debug)]
pub(crate) struct Utf16Cursor<'a> {
    text: &'a str,
    pos: usize,
    line: u32,
    column: u32,
    scanned: usize,
    limit: usize,
}

impl<'a> Utf16Cursor<'a> {
    /// Creates a cursor at the start of `text`.
    pub(crate) const fn new(text: &'a str) -> Self {
        Self::with_limit(text, MAX_POSITION_SCAN_BYTES)
    }

    /// Creates a cursor with an explicit scan budget.
    pub(crate) const fn with_limit(text: &'a str, limit: usize) -> Self {
        Self {
            text,
            pos: 0,
            line: 0,
            column: 0,
            scanned: 0,
            limit,
        }
    }

    /// Maps a non-decreasing byte offset to a zero-based UTF-16 position.
    ///
    /// Returns `None` when the offset is outside the text, is not on a character
    /// boundary, moves backwards, or exceeds the scan budget.
    pub(crate) fn position(&mut self, offset: u32) -> Option<Position> {
        let offset = usize::try_from(offset).ok()?;
        if offset < self.pos || offset > self.text.len() || !self.text.is_char_boundary(offset) {
            return None;
        }
        self.scanned = self.scanned.checked_add(offset - self.pos)?;
        if self.scanned > self.limit {
            return None;
        }
        let mut consumed = 0_usize;
        for (index, character) in self.text[self.pos..offset].char_indices() {
            if character == '\n' {
                self.line = self.line.saturating_add(1);
                self.column = 0;
            } else {
                self.column = self
                    .column
                    .saturating_add(u32::try_from(character.len_utf16()).unwrap_or(1));
            }
            consumed = index;
        }
        if offset > self.pos {
            // `consumed` is the last index before `offset`; the cursor is now at
            // `offset` regardless of whether the loop body ran.
            self.pos = offset;
        }
        let _ = consumed;
        Some(Position::new(self.line, self.column))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn maps_utf16_columns_and_crlf_lines() {
        let text = "标题😀\r\nnext😀";
        let mut cursor = Utf16Cursor::new(text);
        assert_eq!(cursor.position(0), Some(Position::new(0, 0)));
        let first_line_bytes = "标题😀".len();
        assert_eq!(
            cursor.position(u32::try_from(first_line_bytes).expect("offset")),
            Some(Position::new(0, 4))
        );
        let after_newline = u32::try_from(first_line_bytes + 2).expect("offset");
        assert_eq!(cursor.position(after_newline), Some(Position::new(1, 0)));
        assert_eq!(cursor.position(0), None);
    }

    #[test]
    fn long_single_line_scan_is_linear() {
        let text = "a".repeat(1024 * 1024);
        let mut cursor = Utf16Cursor::new(&text);
        let started = Instant::now();
        for index in 0..20_000_u32 {
            let offset = index.saturating_mul(50).min(1_000_000);
            assert!(cursor.position(offset).is_some());
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "linear cursor took {:?}",
            started.elapsed()
        );
    }
}
