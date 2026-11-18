//! WXML expression slots selected for the shadow.
//!
//! Selection and copying are `P3-S` work; this module only declares the
//! placement vocabulary and the original UTF-8 range shape.

use crate::native::strings::Utf8Offset;

/// Why an expression is mounted into the shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressionKind {
    /// `{{ ... }}` interpolation.
    Mustache,
    /// Directive attribute expression such as `wx:if`.
    Directive,
    /// Plain attribute binding.
    Attribute,
    /// Event handler attribute.
    EventHandler,
}

/// Half-open UTF-8 range of one expression in the original WXML text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpressionSlot {
    /// Inclusive UTF-8 start.
    pub start: Utf8Offset,
    /// Exclusive UTF-8 end.
    pub end: Utf8Offset,
    /// Expression role.
    pub kind: ExpressionKind,
}
