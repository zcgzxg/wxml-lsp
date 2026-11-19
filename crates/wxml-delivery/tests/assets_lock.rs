//! Structural and frozen-constant tests for `assets.lock.json` (F3-03 L1).
//!
//! The engine file table in `support` is test data read from the local
//! `7.1.0-beta` extract during development; it is not upstream provenance and
//! it is never downloaded or executed. The typings payload is read from the
//! checked-in `miniprogram-api-typings-5.3.1` fixture. Every expected digest is
//! computed by an independent implementation in this test crate.
//!
//! F3-03 remains BLOCKED: this slice only proves parsing, normalization and
//! constant reconciliation. It does not implement provisioning, does not read a
//! tarball and does not create the repository `assets.lock.json`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names,
    clippy::redundant_pub_crate
)]

#[path = "assets_lock/fixtures.rs"]
mod fixtures;

#[path = "assets_lock/engine_table.rs"]
mod engine_table;

#[path = "assets_lock/digests.rs"]
mod digests;

#[path = "assets_lock/support.rs"]
mod support;

#[path = "assets_lock/parse.rs"]
mod parse;

#[path = "assets_lock/paths.rs"]
mod paths;

#[path = "assets_lock/constants.rs"]
mod constants;

#[path = "assets_lock/digest.rs"]
mod digest;
