//! Editor-independent text primitives shared by every WXML crate.
//!
//! The Language Server Protocol addresses text with zero-based lines and
//! columns measured in UTF-16 code units, while Rust strings are UTF-8. This
//! crate is the single place where those coordinate systems meet, so parsing,
//! analysis and protocol code never reimplement the conversion.
//!
//! [`LineIndex`] owns its text snapshot; [`TextRange`] is the raw byte-range
//! primitive; [`SourceRange`] and [`FragmentRange`] keep original-file ranges
//! and generated-fragment ranges from being confused.

pub mod line_index;
pub mod range;
pub mod source_range;

pub use line_index::{LineIndex, Position};
pub use range::{RangeError, TextRange};
pub use source_range::{FragmentRange, SourceRange};
