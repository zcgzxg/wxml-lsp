//! Checked-in fixture for the frozen 23-file `miniprogram-api-typings@5.3.1`
//! delivery payload.
//!
//! The files under `tests/fixtures/miniprogram-api-typings-5.3.1` are a
//! test-only copy of the Docker-free payload allowlist. They are not upstream
//! provenance and carry no tarball SRI evidence; F3-03 remains BLOCKED. The
//! fixture exists so packager/installer tests can recompute the real payload
//! count, bytes and bundle digest hermetically instead of trusting synthetic
//! file hashes.

#![allow(dead_code, unreachable_pub)]

use std::path::{Path, PathBuf};

use wxml_delivery::manifest_v2::{MINIPROGRAM_TYPINGS_PAYLOAD_PATHS, MINIPROGRAM_TYPINGS_ROOT};

/// Package-relative typings root.
pub const ROOT: &str = MINIPROGRAM_TYPINGS_ROOT;

/// Directory containing the checked-in fixture payload.
#[must_use]
pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/miniprogram-api-typings-5.3.1")
}

/// Returns every fixture payload file as `(root-relative path, bytes)`.
#[must_use]
pub fn payload_files() -> Vec<(&'static str, Vec<u8>)> {
    let root = fixture_root();
    MINIPROGRAM_TYPINGS_PAYLOAD_PATHS
        .iter()
        .map(|relative| {
            let relative = *relative;
            let path = root.join(relative);
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!(
                    "read checked-in typings fixture {}: {error}; the fixture must ship with the crate",
                    path.display()
                )
            });
            (relative, bytes)
        })
        .collect()
}

/// Writes the fixture payload below `root` using `write(root_relative, bytes)`.
pub fn write_payload(mut write: impl FnMut(&str, &[u8])) {
    for (relative, bytes) in payload_files() {
        let path = format!("{ROOT}/{relative}");
        write(&path, &bytes);
    }
}
