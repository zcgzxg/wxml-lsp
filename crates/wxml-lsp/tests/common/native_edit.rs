//! Test-only fail-closed guard for the superseded Node edit tests.
//!
//! The two `semantic_tools` edit tests still drive the old Node backend by
//! default. When a native edit gate is requested, they must not silently
//! return green without a native package/engine. This module validates the
//! test-only `WXML_TEST_NATIVE_PACKAGE` / `TSGO_*` provisioning and panics with
//! the exact remediation when it is missing.
//!
//! Production discovery never reads these variables; only integration tests do.

#![allow(clippy::redundant_pub_crate)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::native::supervisor::verify_engine_executable;

/// Set to request a native edit gate.
const NATIVE_GATE_ENV: &str = "WXML_TEST_NATIVE";
/// Absolute package root. Test-only; production discovery never reads it.
const NATIVE_PACKAGE_ENV: &str = "WXML_TEST_NATIVE_PACKAGE";
/// Existing convention that turns a missing engine into a hard failure.
const REQUIRE_REAL_ENGINE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";
/// Pinned engine executable for the source-only provisioning path.
const TSGO_BIN_ENV: &str = "TSGO_BIN";
/// Extracted upstream package root that supplies `lib/tsc`.
const TSGO_ASSET_ROOT_ENV: &str = "TSGO_ASSET_ROOT";
/// Install-relative package binary.
const PACKAGE_BIN: &str = "bin/wxml-lsp";
/// Install-relative engine executable.
const ENGINE_BINARY: &str = "engine/lib/tsc";
/// Install-relative manifest.
const MANIFEST: &str = "manifest.json";
/// Install-relative typings payload directory.
const TYPINGS_DIR: &str = "typings/miniprogram-api-typings";

/// Returns true when either native gate request flag is set.
///
/// `WXML_TEST_NATIVE=1` alone is enough: a requested native edit gate must
/// never return green without the package/engine.
pub(crate) fn requested() -> bool {
    [NATIVE_GATE_ENV, REQUIRE_REAL_ENGINE_ENV]
        .iter()
        .any(|name| flag(std::env::var_os(name).as_ref()))
}

fn flag(value: Option<&OsString>) -> bool {
    value
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Fails closed when a requested native edit gate is not provisioned.
///
/// Checks the explicit test-only package first, then the `TSGO_ASSET_ROOT` /
/// `TSGO_BIN` fixture sources. A present but unsafe or unpinned engine is
/// always fatal; a missing one is fatal because the gate was requested.
pub(crate) fn require(gate_id: &str) {
    if let Some(package) = non_empty(std::env::var_os(NATIVE_PACKAGE_ENV)) {
        require_package(gate_id, &PathBuf::from(package));
        return;
    }
    if let Some(root) = non_empty(std::env::var_os(TSGO_ASSET_ROOT_ENV)) {
        let root = require_directory(gate_id, &PathBuf::from(root), TSGO_ASSET_ROOT_ENV);
        require_engine(gate_id, &root.join("lib/tsc"));
        return;
    }
    if let Some(engine) = non_empty(std::env::var_os(TSGO_BIN_ENV)) {
        require_engine(gate_id, &PathBuf::from(engine));
        return;
    }
    panic!(
        "native edit gate {gate_id} was requested but no native package/engine is \
         available; set {NATIVE_PACKAGE_ENV}=/absolute/package/root, or \
         {TSGO_ASSET_ROOT_ENV} to an extracted upstream package root, or \
         {TSGO_BIN_ENV} to the pinned 7.1.0-beta engine (sha256 {})",
        PinnedEngine::pinned().executable_sha256()
    );
}

fn non_empty(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

fn require_package(gate_id: &str, package: &Path) {
    assert!(
        package.is_absolute(),
        "native edit gate {gate_id}: {NATIVE_PACKAGE_ENV} must be an absolute \
         package root: {}",
        package.display()
    );
    let metadata = fs::symlink_metadata(package).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: {NATIVE_PACKAGE_ENV} points at {} which \
             cannot be inspected: {error}; provision the pinned package",
            package.display()
        )
    });
    assert!(
        !metadata.file_type().is_symlink() && metadata.is_dir(),
        "native edit gate {gate_id}: {NATIVE_PACKAGE_ENV} must be a real \
         directory: {}",
        package.display()
    );
    let canonical = fs::canonicalize(package).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: {NATIVE_PACKAGE_ENV} cannot be \
             canonicalized at {}: {error}",
            package.display()
        )
    });
    let repository = repository_root();
    assert!(
        !canonical.starts_with(&repository),
        "native edit gate {gate_id}: {NATIVE_PACKAGE_ENV} must be outside the \
         repository ({}): {}",
        repository.display(),
        canonical.display()
    );
    for (relative, directory) in [
        (PACKAGE_BIN, false),
        (MANIFEST, false),
        (ENGINE_BINARY, false),
        (TYPINGS_DIR, true),
    ] {
        require_entry(gate_id, &canonical.join(relative), relative, directory);
    }
    require_engine(gate_id, &canonical.join(ENGINE_BINARY));
}

fn require_entry(gate_id: &str, path: &Path, relative: &str, directory: bool) {
    let metadata = fs::symlink_metadata(path).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: the native package is missing {relative} \
             at {}: {error}; provision the pinned package",
            path.display()
        )
    });
    assert!(
        !metadata.file_type().is_symlink(),
        "native edit gate {gate_id}: package entry {relative} must not be a \
         symlink: {}",
        path.display()
    );
    assert!(
        !directory || metadata.is_dir(),
        "native edit gate {gate_id}: package entry {relative} must be a \
         directory: {}",
        path.display()
    );
    assert!(
        directory || metadata.is_file(),
        "native edit gate {gate_id}: package entry {relative} must be a \
         regular file: {}",
        path.display()
    );
}

fn require_directory(gate_id: &str, path: &Path, variable: &str) -> PathBuf {
    assert!(
        path.is_absolute(),
        "native edit gate {gate_id}: {variable} must be an absolute path: {}",
        path.display()
    );
    let metadata = fs::symlink_metadata(path).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: {variable} points at {} which cannot be \
             inspected: {error}",
            path.display()
        )
    });
    assert!(
        !metadata.file_type().is_symlink() && metadata.is_dir(),
        "native edit gate {gate_id}: {variable} must be a real directory: {}",
        path.display()
    );
    fs::canonicalize(path).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: {variable} cannot be canonicalized at \
             {}: {error}",
            path.display()
        )
    })
}

fn require_engine(gate_id: &str, path: &Path) {
    let metadata = fs::symlink_metadata(path).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: the pinned engine is missing at {}: \
             {error}; set {NATIVE_PACKAGE_ENV}, {TSGO_ASSET_ROOT_ENV} or \
             {TSGO_BIN_ENV}",
            path.display()
        )
    });
    assert!(
        !metadata.file_type().is_symlink() && metadata.is_file(),
        "native edit gate {gate_id}: the engine must be a regular file: {}",
        path.display()
    );
    let pin = PinnedEngine::pinned();
    verify_engine_executable(path, pin.executable_sha256()).unwrap_or_else(|error| {
        panic!(
            "native edit gate {gate_id}: engine {} failed the pinned 7.1.0-beta \
             SHA-256 check (expected {}): {error}",
            path.display(),
            pin.executable_sha256()
        )
    });
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}
