//! F3 real-release acceptance harness: real `assets.lock.json` + real tarballs.
//!
//! Every test is opt-in. Without the pinned inputs the test prints a skip line
//! and returns; `WXML_REQUIRE_REAL_ENGINE=1|true` turns a missing input into a
//! hard failure. Nothing is downloaded and no Node/npm process is executed.
//!
//! What is real in this harness:
//!
//! - the committed repository `assets.lock.json` and both pinned npm tarballs
//!   are provisioned through the production `wxml-lsp-package provision` path;
//! - the packaged `engine/` and `typings/` bytes come only from that verified
//!   cache, and the package is assembled by `wxml-lsp-package native`;
//! - `bin/wxml-lsp` and `bin/wxml-lsp-install` are production Rust binaries and
//!   `zed-extension/extension.wasm` is a production `wasm32-wasip2` build.
//!
//! What is *not* here (kept BLOCKED in the evidence document):
//!
//! - no `strace -f -e trace=execve,execveat` audit (the host has no tracer), so
//!   the PATH tripwire and `/proc` process observation stay auxiliar evidence;
//! - no signed/published release channel and no Zed UI interaction; the Zed
//!   smoke is a separate best-effort script section and F3-14 stays BLOCKED
//!   while `wxml-lsp --stdio` defaults to the Node backend.
//!
//! Run explicitly:
//!
//! ```text
//! TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
//! TSGO_TYPINGS_TARBALL=/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz \
//! WXML_F3_LSP_BIN=<release>/wxml-lsp WXML_F3_INSTALL_BIN=<release>/wxml-lsp-install \
//! WXML_F3_ZED_WASM=<release>/zed_wxml.wasm WXML_REQUIRE_REAL_ENGINE=1 \
//!   cargo test -p wxml-delivery --test f3_real_release -- --nocapture --test-threads=1
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
mod support;

#[path = "f3_real_release/lsp.rs"]
mod lsp;

#[path = "f3_real_release/package.rs"]
mod package;

#[path = "f3_real_release/runtime.rs"]
mod runtime;

#[path = "f3_real_release/abi.rs"]
mod abi;
