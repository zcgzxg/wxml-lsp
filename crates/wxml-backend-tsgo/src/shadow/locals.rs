//! Local binding and loop-scope placeholders.
//!
//! `P3-S` decides which `wx:for` item/index locals and nested scopes enter the
//! shadow. This module only declares the budget and slot records.

/// Hard budget for locals materialized in one shadow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalBudget {
    /// Maximum local bindings.
    pub max_bindings: usize,
    /// Maximum nested scope depth.
    pub max_depth: u16,
}

/// Initial budget frozen for `P3-S` tests.
pub const DEFAULT_LOCAL_BUDGET: LocalBudget = LocalBudget {
    max_bindings: 512,
    max_depth: 32,
};

/// One local name occurrence selected for the shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSlot {
    /// Original identifier text.
    pub name: String,
    /// Zero-based lexical depth.
    pub depth: u16,
}
