//! Shared acceptance-fixture entry points.
//!
//! The fixtures under `tests/fixtures/` are the single source of truth for
//! AC-001..AC-013. This crate exposes their location; the structural
//! validation lives in `tests/manifest.rs` and is run with
//! `cargo test -p wxml-fixtures`.

use std::path::{Path, PathBuf};

/// Returns the `tests/fixtures` directory of this repository.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Returns the fixture manifest path.
#[must_use]
pub fn manifest_path() -> PathBuf {
    fixtures_dir().join("manifest.json")
}
