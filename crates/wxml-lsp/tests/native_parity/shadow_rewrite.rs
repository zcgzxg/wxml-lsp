//! `shadow_rewrite` native parity partition.
//!
//! Status: **blocked skeleton (P3-T)**. `MIGRATED_CASES` is intentionally
//! empty; no ledger case in this partition has been implemented or executed.
//! P3-S shadow unit tests in `wxml-backend-tsgo` are separate backend tests and
//! do not migrate these parity cases. Future parity tests must go through
//! `ParityRunner`; there is no skip path.

use crate::harness::Partition;

/// Ledger target for this partition (19 planned cases).
pub(crate) const PARTITION: Partition = Partition::new("shadow_rewrite", 19);

/// Cases with a real Rust assertion. Empty until the native backend lands.
pub(crate) const MIGRATED_CASES: &[&str] = &[];
