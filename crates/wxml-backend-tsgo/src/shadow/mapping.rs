//! Two-way mapping between shadow, expression copy and original coordinates.
//!
//! The final mapper must compose WXML UTF-8, expression copy/rewrite, mount and
//! shadow coordinates. `P3-S` implements it; this module declares the layers,
//! range shape and trait so query code can be written against one contract.

use crate::native::strings::Utf8Offset;
use crate::shadow::ShadowError;

/// Half-open UTF-8 range in one source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    /// Inclusive UTF-8 start.
    pub start: Utf8Offset,
    /// Exclusive UTF-8 end.
    pub end: Utf8Offset,
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

/// Mapping seam implemented by `P3-S`.
pub trait ShadowMapper {
    /// Maps a range from `layer` back to the original template or script.
    fn to_original(
        &self,
        layer: MappingLayer,
        range: SourceRange,
    ) -> Result<SourceRange, ShadowError>;

    /// Maps an original range into the shadow text.
    ///
    /// When `layer` is [`MappingLayer::Shadow`], the range is already in shadow
    /// coordinates and must be returned unchanged. For every other layer the
    /// implementation maps through the recorded mount/copy chain.
    fn to_shadow(
        &self,
        layer: MappingLayer,
        range: SourceRange,
    ) -> Result<SourceRange, ShadowError>;
}
