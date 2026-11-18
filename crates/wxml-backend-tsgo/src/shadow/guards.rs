//! `wx:if`/`wx:elif`/`wx:else` guard-chain placeholders.
//!
//! `P3-S` owns condition narrowing and loop interleaving. This module only
//! declares the branch vocabulary used by the future shadow builder.

/// Guard attribute kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    /// Leading `wx:if` branch.
    If,
    /// Subsequent `wx:elif` branch.
    Elif,
    /// Final `wx:else` branch.
    Else,
}

/// One guard branch placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardBranch {
    /// Branch kind.
    pub kind: GuardKind,
    /// Nesting depth of the chain.
    pub depth: u16,
}
