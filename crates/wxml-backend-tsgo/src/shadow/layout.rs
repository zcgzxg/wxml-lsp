//! Original-context-preserving shadow layout.
//!
//! Each WXML fragment is mounted either in place (keeping `this`, module and
//! closure context) or as a synthetic fragment with an explicit mapping.
//! `P3-S` fills the layout; this module only declares the mount record.

use crate::native::strings::Utf8Offset;

/// How a WXML fragment reaches the shadow program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountKind {
    /// The fragment keeps its original statement and lexical context.
    InPlace,
    /// A synthetic fragment is added and mapped back explicitly.
    Synthetic,
}

/// One mount point in the generated shadow text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowMount {
    /// Inclusive UTF-8 start in the shadow text.
    pub shadow_start: Utf8Offset,
    /// Exclusive UTF-8 end in the shadow text.
    pub shadow_end: Utf8Offset,
    /// Mount strategy.
    pub kind: MountKind,
}

/// Ordered mounts for one shadow document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShadowLayout {
    mounts: Vec<ShadowMount>,
}

impl ShadowLayout {
    /// Creates a layout from ordered mounts.
    #[must_use]
    pub const fn new(mounts: Vec<ShadowMount>) -> Self {
        Self { mounts }
    }

    /// Returns the mounts in shadow order.
    #[must_use]
    pub fn mounts(&self) -> &[ShadowMount] {
        &self.mounts
    }
}
