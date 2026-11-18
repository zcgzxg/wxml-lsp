//! In-place shadow construction and mapping seams.
//!
//! `P3-S` owns the real implementation. This scaffold declares the vocabulary
//! for expression slots, rewrites, mounts, locals, guards and two-way mapping;
//! no WXML text is copied or rewritten here.

pub mod expression;
pub mod guards;
pub mod layout;
pub mod locals;
pub mod mapping;
pub mod rewrite;

use thiserror::Error;

/// Shadow construction or mapping failure.
#[derive(Debug, Error)]
pub enum ShadowError {
    /// A fragments/locals/depth budget would be exceeded.
    #[error("shadow budget exceeded: {budget}")]
    BudgetExceeded {
        /// Budget identifier, for example `locals`.
        budget: &'static str,
    },
    /// The requested range cannot be mapped to a real source span.
    #[error("shadow range cannot be mapped: {0}")]
    Unmapped(&'static str),
    /// Capability reserved for `P3-S`/`P4`.
    #[error("shadow capability is not implemented: {feature}")]
    Unimplemented {
        /// Capability marker supplied by the caller.
        feature: &'static str,
    },
}
