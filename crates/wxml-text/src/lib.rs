//! Editor-independent text primitives shared by every WXML crate.
//!
//! The Language Server Protocol addresses text with zero-based lines and
//! columns measured in UTF-16 code units, while Rust strings are UTF-8. This
//! crate is the single place where those coordinate systems meet, so parsing,
//! analysis and protocol code never reimplement the conversion.

pub mod line_index;

pub use line_index::{LineIndex, Position};
