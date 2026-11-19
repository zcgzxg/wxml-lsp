//! Frozen `assets.lock.json` model, canonicalization and constant reconciliation.
//!
//! This module is deliberately byte-free with respect to the filesystem: it
//! parses a submitted lock, validates its internal file tables and compares the
//! declared identities with the compile-time constants that the packager and
//! installer already enforce. It never reads a tarball, a cache or an
//! environment variable and it never starts a process.
//!
//! The lock separates two facts that the v2 manifest cannot separate:
//!
//! - `upstream.files[]` is the complete npm tarball tree after
//!   `stripPrefix=package/`, including files that the delivery payload drops.
//! - `payload.files[]` is the exact tree that will be packaged, addressed by
//!   its final package-relative paths below `packageRoot`.
//!
//! `upstream.fileCount=26` and `payload.fileCount=23` are therefore visible at
//! the same time instead of being hidden in different allowlists.
//!
//! # Tree digests
//!
//! `upstream.treeDigest` and `payload.treeDigest` use the same canonical
//! preimage as [`crate::verify_v2::tree_fingerprint`]: sorted
//! `D <octal mode> <directory>\n` rows followed by sorted
//! `F <octal mode> <sha256> <file>\n` rows, then SHA-256. The lock has no
//! `directories[]` field: directories are the ancestor closure of `files[]`
//! (excluding the root) and are always `0755`. Explicit empty directories are
//! therefore not representable; the L2/L3 extractor must reject or ignore them.
//!
//! The upstream root is the empty string; the payload root is `packageRoot`.
//! Paths in the digest are relative to that root.
//!
//! # Stability
//!
//! This is the F3-03 L1 surface only. The types, constants and helper names are
//! **not frozen** until L5/L7 wire the lock into provisioning and packaging;
//! callers inside this repository may use them, but they are not a public
//! compatibility promise.

mod digest;
mod model;
mod validate;

pub use digest::canonical_tree_digest;
pub use model::{
    Asset, AssetBundle, AssetExtract, AssetKind, AssetPayload, AssetProvenance, AssetTarball,
    AssetsLock, ExcludedPath, UpstreamTree,
};

use crate::manifest::MAX_INDEX_BYTES;
use crate::manifest_v2::{MINIPROGRAM_TYPINGS_ASSET, MINIPROGRAM_TYPINGS_EXCLUDED_PATHS};

/// Lock schema version accepted by this build.
pub const ASSETS_LOCK_SCHEMA_VERSION: u32 = 1;
/// Conventional repository file name.
pub const ASSETS_LOCK_FILE_NAME: &str = "assets.lock.json";
/// Maximum accepted lock size (reuses the 1 MiB index budget).
pub const MAX_ASSETS_LOCK_BYTES: u64 = MAX_INDEX_BYTES;

/// Frozen name of the native engine asset.
pub const ENGINE_ASSET_NAME: &str = "typescript-native-engine";
/// Final package-relative root of the native engine asset.
pub const ENGINE_ASSET_ROOT: &str = "engine";
/// Frozen name of the miniprogram typings asset.
pub const TYPINGS_ASSET_NAME: &str = MINIPROGRAM_TYPINGS_ASSET;
/// npm package name of the miniprogram typings asset.
pub const TYPINGS_NPM_PACKAGE: &str = "miniprogram-api-typings";
/// Only accepted provenance registry.
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org";
/// Only accepted npm tarball prefix.
pub const NPM_STRIP_PREFIX: &str = "package/";
/// Standard-library bundle root inside the final package.
pub const STANDARD_LIB_ROOT: &str = "engine/lib";
/// Frozen standard-library bundle byte count.
pub const STANDARD_LIB_TOTAL_BYTES: u64 = 3_870_196;
/// Frozen standard-library bundle digest.
///
/// Algorithm: sorted `<sha256>  <root-relative path>\n` lines, then SHA-256.
pub const STANDARD_LIB_BUNDLE_DIGEST: &str =
    "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e";
/// Frozen runtime `.d.ts` closure file count.
pub const MINIPROGRAM_TYPINGS_RUNTIME_FILE_COUNT: u64 = 20;
/// Frozen runtime `.d.ts` closure byte count.
pub const MINIPROGRAM_TYPINGS_RUNTIME_TOTAL_BYTES: u64 = 2_522_752;
/// Frozen runtime `.d.ts` closure digest.
///
/// This is the 20-file `index.d.ts` + `types/**/*.d.ts` closure pinned by the
/// native backend, not the 23-file delivery payload.
pub const MINIPROGRAM_TYPINGS_RUNTIME_BUNDLE_DIGEST: &str =
    "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9";
/// Frozen engine extract include patterns.
pub const ENGINE_EXTRACT_INCLUDE: &[&str] = &[
    "lib/**",
    "LICENSE",
    "NOTICE.txt",
    "package.json",
    "README.md",
];
/// Frozen engine executables (upstream-relative).
pub const ENGINE_EXECUTABLES: &[&str] = &["lib/tsc"];
/// Frozen typings extract include patterns.
pub const MINIPROGRAM_TYPINGS_EXTRACT_INCLUDE: &[&str] = &[
    "index.d.ts",
    "types/**",
    "LICENSE",
    "README.md",
    "CHANGELOG.md",
];
/// Frozen typings extract exclusions.
pub const MINIPROGRAM_TYPINGS_EXTRACT_EXCLUDE: &[&str] = MINIPROGRAM_TYPINGS_EXCLUDED_PATHS;
/// Maximum `exclude[].reason` bytes.
pub const MAX_EXCLUDE_REASON_BYTES: usize = 256;
/// Maximum accepted tarball URL bytes.
pub const MAX_TARBALL_URL_BYTES: usize = 2048;
/// Maximum accepted asset name bytes.
pub const MAX_ASSET_NAME_BYTES: usize = 64;
