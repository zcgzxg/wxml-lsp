//! Coordinate and string-pool units crossing the native boundary.
//!
//! The units are deliberately distinct newtypes: native API positions and AST
//! `pos`/`end` are UTF-16 code units, the string pool uses UTF-8/WTF-8 bytes
//! and Rust source ranges are UTF-8 bytes. Conversions happen only against a
//! text snapshot in `P3-N`/`P3-S`; there are no implicit `From<u32>` impls.

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
