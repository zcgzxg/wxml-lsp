//! `scenario_matrix` native parity partition.
//!
//! The 43 ledger targets stay routed here so the frozen `target_rust_test`
//! paths do not move. The real engine assertions live in the sibling
//! `real_matrix.rs` module and cover:
//!
//! - 21 exact in-place scenarios (19 pure checker + 2 short paths),
//! - 12 characterized gaps (3 DOCS, 5 STATE, 1 PROBE, 1 ROOT, 2 CHECKER),
//! - the 10 top-level cases' in-place halves (4 exact, 6 blocked).
//!
//! There is no skip path: the manifest/comparator tests run by default, and
//! the engine tests are `#[ignore]` required-native jobs.

use crate::harness::Partition;

/// Ledger target for this partition (43 planned cases).
pub(crate) const PARTITION: Partition = Partition::new("scenario_matrix", 43);

/// Tests implemented in `real_matrix.rs` for this partition.
pub(crate) const MIGRATED_CASES: &[&str] = &[
    "real_matrix_manifest_is_frozen_and_oracle_derived",
    "real_matrix_comparator_rejects_missing_shifted_and_perturbed_results",
    "real_matrix_in_place_matches_frozen_oracle",
    "real_matrix_known_gaps_are_characterized",
    "real_matrix_top_level_cases",
];
