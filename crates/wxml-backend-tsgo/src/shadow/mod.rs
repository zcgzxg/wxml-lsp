//! In-place shadow construction, rewriting and mapping.
//!
//! # What this module implements
//!
//! `P3-S` builds the first real Rust vertical slice of the production
//! in-place-shadow layout:
//!
//! - script scanning for `Page`/`Component`/`Behavior` registrations with an
//!   object-literal argument ([`layout`]),
//! - deterministic, non-overlapping UTF-16 edit plans plus an assembler that
//!   preserves the original text verbatim outside edits ([`rewrite`]),
//! - expression-root rewriting for template fragments, template locals and
//!   object shorthand ([`expression`], [`rewrite`]),
//! - typed `wx:for` locals and `wx:if` guard rendering ([`locals`], [`guards`]),
//! - guard-wrapper source segments folded into the owning fragment mount, so a
//!   diagnostic on the generated `if (...)` copy maps to the original
//!   `wx:if`/`wx:elif` condition bytes ([`guards`], [`layout`]),
//! - original ⇄ generated script offset mapping and mounted-expression spans
//!   back to WXML bytes ([`mapping`]).
//!
//! Everything here is pure Rust text processing. No Node, no native engine and
//! no TypeScript checker is involved. Diagnostics and hover typing are
//! therefore *not* produced by this slice.
//!
//! # Not migrated yet (`BLOCKED`)
//!
//! The following TypeScript-oracle cases cannot be honestly migrated without a
//! real TypeScript checker (native `tsgo` session) and are deliberately absent:
//!
//! - `TS-LAYOUTS-0062`: the excess-property diagnostic (2353) that proves a
//!   probe directly in `Component` options is rejected. This slice instead
//!   always mounts inside `methods`, which is the other half of the same rule.
//! - `TS-LAYOUTS-0084`, `TS-LAYOUTS-0101`, `TS-LAYOUTS-0117`,
//!   `TS-LAYOUTS-0147`: hover text, documentation and `optionalTypes`
//!   visibility on `this.data` require the checker.
//! - `TS-REWRITE-0197`, `TS-REWRITE-0349`, `TS-REWRITE-0368`,
//!   `TS-REWRITE-0413`: published diagnostics, cross-gap diagnostic cropping
//!   and "no fabricated error" assertions require the checker.
//! - `TS-REWRITE-0378`: the 1 MiB / 4 MiB emission performance gate belongs to
//!   the P5 performance budget, not this unit slice.
//! - `TS-MAPPING-0188`, `TS-MAPPING-0208`, `TS-MAPPING-0306`: mapped
//!   diagnostics and diagnostic de-duplication require the checker.
//! - `TS-LOCALS-0261`, `TS-LOCALS-0288`, `TS-LOCALS-0312`: end-to-end hover
//!   types need the checker. The pure rendering cases (`0048`, `0063`,
//!   `0089`, `0124`, `0130`, `0138`, `0144`, `0334`, `0343`) are ported.
//! - `TS-GUARDS-0097`, `TS-GUARDS-0128`, `TS-GUARDS-0152`, `TS-GUARDS-0174`,
//!   `TS-GUARDS-0218`: control-flow narrowing needs the checker. The pure
//!   rendering cases (`0047`, `0057`, `0066`, `0075`, `0087`, `0201`) and the
//!   rendered if/elif/else structure for `0097`-family inputs are ported.
//! - `TS-GUARDS-0302`, `TS-GUARDS-0321`, `TS-GUARDS-0329`: diagnostic and
//!   `JSDoc` observations need the checker. `TS-GUARDS-0312` is ported at the
//!   derived-text level (`@ts-check` injection).
//! - `TS-GUARDS-0261`: the full nested-loop guard narrowing needs the checker.
//!   Only rendered structure is asserted.
//! - Behavior graph resolution (imports, spreads, nested behaviors) is owned by
//!   [`crate::script::behavior`]. The in-place shadow consumes its merged
//!   properties only through [`layout::build_in_place_shadow_with_behavior`];
//!   without an overlay a non-empty behavior list stays `Partial`. Behavior
//!   `data` is still read from the mini-program typings via `this.data` and is
//!   not enumerated here.
//!
//! # Syntax and resource limits
//!
//! The expression/script scanner is a token-level checker, not a full
//! JavaScript parser. It rejects malformed delimiters, statement wrappers,
//! unsupported shapes and adjacent bare values (`a b`), but the native checker
//! remains the authority for complete syntax. Script scans have hard budgets
//! (`MAX_SCRIPT_TOKENS`, `MAX_SCRIPT_DEPTH`) and return
//! [`ShadowError::BudgetExceeded`] at the cap.
//!
//! # Slice test coverage (not an oracle-case count)
//!
//! This slice adds 75 module unit tests and 98 integration tests across
//! `tests/shadow_layout.rs` (33), `tests/shadow_locals_guards.rs` (21),
//! `tests/shadow_mapping.rs` (18), `tests/shadow_rewrite.rs` (22) and
//! `tests/shadow_guard_ranges.rs` (8). The package run is 598 tests total
//! because it also executes the pre-existing crate tests. These are
//! test-function counts, not TypeScript oracle cases.
//! Tests named `case_<file>_<line>_...` assert only the text/mapping/status
//! layer named in the test; structural guard tests are named without the
//! `case_` prefix and are listed as BLOCKED for narrowing.
//!
//! # Deferred work (not a parity claim)
//!
//! `DerivedView` and `ShadowMap` currently own clones of the script text. The
//! text-copy/file-size optimization is a follow-up, not silently claimed as
//! solved by this slice.
//!
//! # Deliberate differences from the TypeScript oracle
//!
//! - Numeric offsets are typed. Fractional/negative offsets
//!   (`TS-MAPPING-0110`) are unrepresentable instead of being accepted and
//!   rejected at runtime; out-of-range or non-character-boundary spans return
//!   typed errors rather than an `unattributable` boolean.
//! - Edit plans reject two edits at the same offset. The TypeScript assembler
//!   sorts them and lets insertion order decide; rejecting is stricter and
//!   guarantees a single mapping.
//! - Duplicate input file paths are rejected (TypeScript's request validator
//!   does the same before analysis).

pub mod expression;
pub mod guards;
pub mod layout;
pub mod locals;
pub mod mapping;
pub mod rewrite;

use thiserror::Error;

/// Shadow construction, rewriting or mapping failure.
#[derive(Debug, Error)]
pub enum ShadowError {
    /// A fragments/locals/depth/token budget would be exceeded.
    ///
    /// `budget` is stable, for example `locals`, `script.tokens` or
    /// `expression.tokens`.
    #[error("shadow budget exceeded: {budget}")]
    BudgetExceeded {
        /// Budget identifier, for example `locals`.
        budget: &'static str,
    },
    /// The construct is valid TypeScript but not supported by this slice.
    #[error("unsupported shadow construct ({feature}): {reason}")]
    Unsupported {
        /// Feature marker, for example `component.methods`.
        feature: &'static str,
        /// Human-readable reason; must not contain project text beyond a
        /// bounded expression description.
        reason: String,
    },
    /// The supplied input is malformed for this slice.
    #[error("invalid shadow input ({feature}): {reason}")]
    Invalid {
        /// Feature marker, for example `fragment.range`.
        feature: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// A WXML expression cannot be parsed or contains an unsupported shape.
    #[error("bad WXML expression: {detail}")]
    BadExpression {
        /// Bounded description with the rejection reason.
        detail: String,
    },
    /// Two edits share an offset or overlap.
    #[error("shadow edits overlap: `{first}` and `{second}`")]
    EditOverlap {
        /// First edit identifier.
        first: String,
        /// Second edit identifier.
        second: String,
    },
    /// Two edits share an identifier.
    #[error("duplicate shadow edit id: {id}")]
    DuplicateEditId {
        /// Duplicated identifier.
        id: String,
    },
    /// An edit lies outside the original text or splits a code point.
    #[error("shadow edit `{id}` is out of range at {at} (length {length}, text {text_length})")]
    EditOutOfRange {
        /// Edit identifier.
        id: String,
        /// Requested UTF-16 start.
        at: u32,
        /// Requested UTF-16 replacement length.
        length: u32,
        /// Original text length in UTF-16 code units.
        text_length: u32,
    },
    /// A UTF-16 or UTF-8 position is outside the text.
    #[error("position {position} is outside text of length {length} ({unit})")]
    PositionOutOfRange {
        /// Observed position.
        position: u32,
        /// Text length in the same unit.
        length: u32,
        /// `utf-16` or `utf-8`.
        unit: &'static str,
    },
    /// A position falls inside a surrogate pair or a UTF-8 code point.
    #[error("position {position} is not a character boundary")]
    NotCharBoundary {
        /// Rejected position.
        position: u32,
    },
    /// A derived position lies inside inserted shadow text.
    #[error("derived position {position} is inside generated text")]
    MappingInsertedText {
        /// Rejected derived UTF-16 position.
        position: u32,
    },
    /// An original position lies inside text removed by an edit.
    #[error("original position {position} was removed from the derived view")]
    MappingRemovedText {
        /// Rejected original UTF-16 position.
        position: u32,
    },
    /// An original position is exactly at an insertion point.
    #[error("original position {position} maps ambiguously across an insertion")]
    MappingAmbiguous {
        /// Rejected original UTF-16 position.
        position: u32,
    },
    /// A mapped span crosses a generated edit.
    #[error("source range crosses a generated edit")]
    MappingCrossesEdit,
    /// A segment produced more text than its recorded source range.
    #[error("segment needs {needed} source bytes but only {available} are recorded")]
    MappingSourceMismatch {
        /// Bytes required by the segment text.
        needed: u32,
        /// Bytes available in the source range.
        available: u32,
    },
    /// The mapping layer is not implemented for this value.
    #[error("mapping layer is not supported here: {0:?}")]
    MappingUnsupportedLayer(mapping::MappingLayer),
}
