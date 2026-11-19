//! F3 remaining acceptance matrix: F3-05/06/08/09/10/11/15/16.
//!
//! This suite is deliberately separate from `f3_real_release.rs` so the
//! already-committed harness is not modified. It reuses only that harness's
//! package-building support: the committed `assets.lock.json`, the pinned
//! engine/typings tarballs, the production `wxml-lsp-package native` output and
//! the production Rust/Zed binaries.
//!
//! Evidence labels:
//!
//! - `PASS` — observed on the current host with the pinned assets;
//! - `FIXTURE` — the v1 Node package or an injected journal/failpoint state is
//!   a test fixture, never a published artifact;
//! - `BLOCKED` — the exact production seam does not exist (process-level kill
//!   windows) or the criterion needs a tracer/UI not present on this host.
//!
//! Real tests are opt-in. A missing pinned input prints an explicit `SKIP`
//! unless `WXML_REQUIRE_REAL_ENGINE=1|true`, which fails closed.
//!
//! Run explicitly:
//!
//! ```text
//! TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
//! TSGO_TYPINGS_TARBALL=/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz \
//! WXML_F3_LSP_BIN=<release>/wxml-lsp WXML_F3_INSTALL_BIN=<release>/wxml-lsp-install \
//! WXML_F3_ZED_WASM=<release>/zed_wxml.wasm WXML_REQUIRE_REAL_ENGINE=1 \
//!   cargo test -p wxml-delivery --test f3_acceptance -- --ignored --nocapture --test-threads=1
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    dead_code,
    unused_imports,
    unreachable_pub
)]

#[path = "real_asset/support.rs"]
mod real_asset_support;

#[path = "f3_real_release/support.rs"]
mod release;

#[path = "f3_acceptance/support.rs"]
mod support;

#[path = "f3_acceptance/package_guards.rs"]
mod package_guards;

#[path = "f3_acceptance/transactions.rs"]
mod transactions;

#[path = "f3_acceptance/legacy.rs"]
mod legacy;
