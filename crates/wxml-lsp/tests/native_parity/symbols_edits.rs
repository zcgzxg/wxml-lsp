//! `symbols_edits` native parity partition.
//!
//! Status: **blocked skeleton (P3-T)**. `MIGRATED_CASES` is intentionally
//! empty; no ledger case in this partition has been implemented or executed.
//! The three mandatory native edit gates below are registered as blocked, not
//! as passing tests. They replace the two `semantic_tools` silent returns and
//! the optional `tsc` branch recorded in `docs/plans/tsgo-test-ledger.json`.

use crate::harness::{BlockedGate, Partition};

/// Ledger target for this partition (21 planned cases).
pub(crate) const PARTITION: Partition = Partition::new("symbols_edits", 21);

/// Cases with a real Rust assertion. Empty until the native backend lands.
pub(crate) const MIGRATED_CASES: &[&str] = &[];

/// Future tests that must run before the old silent edit paths can be removed.
pub(crate) const BLOCKED_NATIVE_GATES: &[BlockedGate] = &[
    BlockedGate::new(
        "native_component_property_edit_applies_and_reparses",
        "G-P5-E2E-22: a real native component property edit must apply to an \
         isolated snapshot and reparse; missing engine is a hard failure",
    ),
    BlockedGate::new(
        "assert_native_edit_reparses_and_typechecks",
        "G-P5-E2E-22: every applied WorkspaceEdit must reparse/typecheck natively; \
         no tsc or Node fallback",
    ),
    BlockedGate::new(
        "native_rejects_cross_namespace_and_dynamic_edits",
        "G-P5-E2E-23: cross-namespace/dynamic rename must be rejected with -32803, \
         no edit, and unchanged sources",
    ),
];
