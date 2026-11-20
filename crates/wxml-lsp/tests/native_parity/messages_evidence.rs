//! `messages_evidence` native parity partition.
//!
//! Status: **blocked skeleton (P3-T)**. `MIGRATED_CASES` is intentionally
//! empty; no ledger case in this partition has been implemented or executed.
//! Future tests must go through `ParityRunner`; there is no skip path.

use crate::harness::Partition;

/// Ledger target for this partition (8 planned cases).
pub(crate) const PARTITION: Partition = Partition::new("messages_evidence", 8);

/// Cases with a real Rust assertion. Empty until the native backend lands.
pub(crate) const MIGRATED_CASES: &[&str] = &[];
