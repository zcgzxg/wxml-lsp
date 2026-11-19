//! Reproducible local packaging and a verified installer for WXML LSP.
//!
//! The crate has two binaries:
//!
//! - `wxml-lsp-package` turns a staged build tree into a deterministic
//!   versioned directory, `.tar.gz` archive, `SHA256SUMS`, sidecar checksum and
//!   `index.json`.
//! - `wxml-lsp-install` downloads or copies a release, verifies its hash,
//!   manifest, version, protocol, file digests and archive entry safety, then
//!   atomically installs and activates it.
//!
//! Neither binary uploads anything, creates remote tags or contacts a service
//! other than the explicitly supplied release source.

// The HTTP/TLS stack pulls in multiple versions of a few transitive crates
// (`getrandom`, `windows-sys`, ...). They are outside this crate's source and
// cannot be unified without pinning unrelated dependencies.
#![allow(clippy::multiple_crate_versions)]

pub mod acquire;
pub mod acquire_v2;
pub mod archive;
pub mod assets_lock;
pub mod cache;
pub mod error;
pub mod index_v2;
pub mod install;
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
