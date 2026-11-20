//! Reproducible native packaging and a verified installer for WXML LSP.
//!
//! The crate has two binaries:
//!
//! - `wxml-lsp-package native` turns a prepared, Node-free tree plus a
//!   provisioned asset cache into a deterministic schema v2 package with a
//!   versioned directory, `.tar.gz` archive, `SHA256SUMS`, sidecar checksum and
//!   `index.json`.
//! - `wxml-lsp-package licenses` turns saved `cargo metadata` documents into a
//!   deterministic third-party license inventory with crate names, versions,
//!   license expressions and deduplicated local license texts.
//! - `wxml-lsp-install` downloads or copies a native release, verifies its
//!   hash, manifest, version, protocol, file digests and archive entry safety,
//!   then atomically installs and activates it.
//!
//! Schema v1 (legacy Node) packages are read-only: their identity and tree may
//! be inspected, but they are never built, installed or activated.
//!
//! Neither binary uploads anything, creates remote tags or contacts a service
//! other than the explicitly supplied release source.

// The HTTP/TLS stack pulls in multiple versions of a few transitive crates
// (`getrandom`, `windows-sys`, ...). They are outside this crate's source and
// cannot be unified without pinning unrelated dependencies.
#![allow(clippy::multiple_crate_versions)]

pub mod acquire_v2;
pub mod archive;
pub mod assets_lock;
pub mod error;
pub mod index_v2;
pub mod install;
pub mod licenses;
pub mod manifest;
pub mod manifest_v2;
pub mod native_install;
pub mod package;
pub mod paths;
pub mod probe;
pub mod provision;
pub mod semver;
pub mod source;
pub mod sri;
pub mod state;
pub mod type_assets;
pub mod verify;
pub mod verify_v2;

pub use error::{DeliveryError, Result};
