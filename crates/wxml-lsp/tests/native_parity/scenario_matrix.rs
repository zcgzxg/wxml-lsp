//! `scenario_matrix` native parity partition.
//!
//! Status: **blocked skeleton (P3-T)**. `MIGRATED_CASES` is intentionally
//! empty; no ledger case in this partition has been implemented or executed.
//! The 43 planned cases are the in-place-shadow/matrix cases; the 35 retired
//! Candidate B/symbol-bridge targets that also point here are excluded.
//! Future tests must go through `ParityRunner`; there is no skip path.

use crate::harness::Partition;

/// Ledger target for this partition (43 planned cases).
pub(crate) const PARTITION: Partition = Partition::new("scenario_matrix", 43);

/// Cases with a real Rust assertion. Empty until the native backend lands.
pub(crate) const MIGRATED_CASES: &[&str] = &[];
