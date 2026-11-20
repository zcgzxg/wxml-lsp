//! Opt-in real-asset acceptance harness for the schema v2 native installer.
//!
//! The test consumes the pinned `@typescript/typescript-linux-x64` 7.1.0-beta
//! tarball supplied through `TSGO_NATIVE_TARBALL`, verifies its fixed SRI,
//! SHA-256 and size, extracts it with the delivery archive reader, builds a
//! schema v2 package around the real engine payload, and installs that package
//! with the `wxml-lsp-install` binary.
//!
//! Run explicitly:
//!
//! ```text
//! TSGO_NATIVE_TARBALL=/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz \
//!   cargo test -p wxml-delivery --test real_asset -- --nocapture
//! ```
//!
//! `WXML_REQUIRE_REAL_ENGINE=1` (or `=true`) turns a missing asset into a hard
//! failure instead of a skip. Nothing is downloaded; a missing or malformed
//! asset fails closed.
//!
//! The installer runs with a `PATH` that contains only Node tripwire shims, so
//! this proves no **PATH-resolved** Node was used. It does not prove F3-02: no
//! `strace`/execve audit, network-namespace audit, ABI evidence, production
//! release package, or Zed UI smoke is performed here. This harness closes no
//! F3 acceptance ID; only `engine/` comes from the upstream tarball and the rest
//! of the package is a test fixture.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty
)]

#[path = "real_asset/support.rs"]
mod support;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use wxml_delivery::install::InstallRoot;
use wxml_delivery::manifest::atomic_write;
use wxml_delivery::manifest_v2::{
    ENGINE_BINARY_PATH, ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_INTEGRITY,
    ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE, ENGINE_VERSION,
};
use wxml_delivery::native_install::{CurrentInstall, inspect_current};

/// Positive control for the tripwire: a real `node --version` through the shim
/// `PATH` must exit 99 and create the marker. Without this, the no-execution
/// assertion in the install test would be vacuous.
#[test]
fn node_tripwire_positive_control_is_exercised() {
    let base = TestDir::new("tripwire-positive");
    let tripwires = Tripwires::install(base.path());
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "node --version"])
        .env_clear()
        .env("PATH", tripwires.path());
    let output = run_with_timeout(
        &mut command,
        Duration::from_secs(5),
        "node tripwire positive control",
    );
    assert_eq!(
        output.status.code(),
        Some(99),
        "tripwire node did not exit 99: status={:?} stdout={} stderr={}",
        output.status,
        stdout(&output),
        stderr(&output)
    );
    assert!(
        tripwires.marker().is_file(),
        "tripwire node did not create {}",
        tripwires.marker().display()
    );
    println!(
        "node tripwire positive control: exit=99 marker={}",
        tripwires.marker().display()
    );
    tripwires.clear_marker();
    tripwires.assert_not_executed();
}

#[test]
fn pinned_tarball_identity_is_checked_before_extraction() {
    let Some(tarball) = pinned_tarball_or_skip("pinned_tarball_identity") else {
        return;
    };
    let base = TestDir::new("identity");
    let destination = base.path().join("extract");

    wxml_delivery::sri::verify_asset_file(
        &tarball,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect("the pinned tarball must pass the positive identity check");
    println!("positive identity: pinned tarball matches SRI/SHA-256/size");
    assert_identity_rejections(&base, &tarball, &destination);
}

fn assert_identity_rejections(base: &TestDir, tarball: &Path, destination: &Path) {
    assert_missing_and_truncated_rejected(base, tarball, destination);
    assert_wrong_digest_rejected(base, tarball, destination);
}

fn assert_missing_and_truncated_rejected(base: &TestDir, tarball: &Path, destination: &Path) {
    let missing = base.path().join("missing.tgz");
    let error = stage_verified_asset(
        &missing,
        destination,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect_err("a missing tarball must fail");
    assert!(
        matches!(&error, wxml_delivery::error::DeliveryError::Io { .. }),
        "expected an I/O error, got {error}"
    );
    assert!(
        !destination.exists(),
        "verification failure must not create the extraction destination"
    );

    let truncated = base.path().join("truncated.tgz");
    let prefix = fs::read(tarball).expect("read tarball for truncation");
    fs::write(&truncated, &prefix[..4096]).expect("write truncated tarball");
    let error = stage_verified_asset(
        &truncated,
        destination,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect_err("a wrong-size tarball must fail");
    assert!(error.to_string().contains("size"), "{error}");
    assert!(!destination.exists());
}

fn assert_wrong_digest_rejected(base: &TestDir, tarball: &Path, destination: &Path) {
    let wrong_sri = wxml_delivery::sri::sha512_sri_bytes(b"not the pinned tarball").to_sri();
    let error = stage_verified_asset(
        tarball,
        destination,
        &wrong_sri,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect_err("a wrong SRI must fail");
    assert!(error.to_string().contains("SRI"), "{error}");
    assert!(!destination.exists());

    let corrupted = base.path().join("corrupted.tgz");
    let corrupted_bytes = flip_last_byte_copy(tarball, &corrupted);
    let corrupted_sri = wxml_delivery::sri::sha512_sri_bytes(&corrupted_bytes).to_sri();
    let error = stage_verified_asset(
        &corrupted,
        destination,
        &corrupted_sri,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect_err("a same-size wrong tarball must fail the pinned SHA-256");
    assert!(error.to_string().contains("SHA-256"), "{error}");
    assert!(!destination.exists());

    println!(
        "pinned tarball identity rejected missing/wrong-size/wrong-SRI/same-size-wrong-SHA-256 \
         inputs before creating the extraction destination"
    );
}

#[test]
fn installs_pinned_real_engine_without_path_node() {
    let Some(tarball) = pinned_tarball_or_skip("real_asset_install") else {
        return;
    };
    assert_supported_host();
    let base = TestDir::new("install");
    assert_eq!(
        mode_of(base.path()),
        0o700,
        "TestDir mode was not preserved as 0700; this checks preservation, not fresh root creation"
    );
    let prepared = prepare_release(&base, &tarball);
    print_prepared(&prepared);

    let root_path = create_private_install_root(&base, "root");

    let tripwires = Tripwires::install(base.path());
    let output = install_from_release(&prepared, &tripwires, base.path(), &root_path);
    assert_install_output(&output);
    let root = InstallRoot::open(&root_path)
        .expect("open install root")
        .expect("install root exists");
    assert_install_state(&root);
    assert_install_layout(&root, &root_path);
    assert_current_identity(&root);
    let installed = root.versions_dir().join(PACKAGE_VERSION);
    let report = assert_installed_tree(&installed, &prepared);
    let current_tsc = root.path().join("current").join(ENGINE_BINARY_PATH);
    assert_eq!(
        fs::canonicalize(&current_tsc).expect("canonicalize current tsc"),
        fs::canonicalize(installed.join(ENGINE_BINARY_PATH)).expect("canonicalize installed tsc")
    );
    assert_installed_engine(&current_tsc);
    assert_installed_provenance(&installed, &prepared);
    assert_cli_views(&root_path, &tripwires, &report);
    tripwires.assert_not_executed();
    println!(
        "installed real engine without PATH-resolved Node: version={ENGINE_VERSION} files={} \
         engineSha256={} installRootMode={:o}",
        report.file_count,
        prepared.audit.engine_sha256,
        mode_of(&root_path)
    );
}

fn print_prepared(prepared: &PreparedRelease) {
    println!(
        "real asset verified: sha256={} sri={} size={} extractedFiles={} extractedBytes={}",
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SIZE,
        prepared.extracted_files,
        prepared.extracted_bytes
    );
    println!(
        "upstream package: files={} unpacked={} engineSha256={} engineSize={}",
        prepared.audit.file_count,
        prepared.audit.unpacked_size,
        prepared.audit.engine_sha256,
        prepared.audit.engine_size
    );
}

fn assert_current_identity(root: &InstallRoot) {
    match inspect_current(root).expect("inspect current") {
        CurrentInstall::Native { version, manifest } => {
            assert_eq!(version, PACKAGE_VERSION);
            assert_eq!(manifest.engine.version, ENGINE_VERSION);
            assert_eq!(manifest.engine.distribution, ENGINE_DISTRIBUTION);
            assert_eq!(
                manifest.engine.distribution_integrity,
                ENGINE_DISTRIBUTION_INTEGRITY
            );
        }
        other => panic!("expected a native current, got {other:?}"),
    }
}

struct TamperCase {
    label: &'static str,
    needle: &'static str,
    mutate: fn(&mut Value),
}

#[test]
fn tampered_release_archive_fails_before_activation() {
    let Some(tarball) = pinned_tarball_or_skip("release_tamper") else {
        return;
    };
    assert_supported_host();
    let base = TestDir::new("release-tamper");
    let prepared = prepare_release(&base, &tarball);
    println!(
        "tamper fixture built from pinned engine sha256={} files={}",
        prepared.audit.engine_sha256, prepared.audit.file_count
    );

    let index_path = prepared.release.join("index.json");
    let original_index = fs::read(&index_path).expect("read pristine index");
    let tripwires = Tripwires::install(base.path());
    let cases = tamper_cases();
    for (index, case) in cases.iter().enumerate() {
        assert_tampered_case(
            &base,
            &prepared,
            &tripwires,
            &index_path,
            &original_index,
            index,
            case,
        );
    }
    tripwires.assert_not_executed();
}

fn tamper_cases() -> [TamperCase; 3] {
    [
        TamperCase {
            label: "wrong-sri",
            needle: "SRI",
            mutate: |value| {
                value["releases"][0]["archiveIntegrity"] =
                    json!(wxml_delivery::sri::sha512_sri_bytes(b"tampered release").to_sri());
            },
        },
        TamperCase {
            label: "wrong-sha256",
            needle: "SHA-256",
            mutate: |value| {
                value["releases"][0]["sha256"] = json!("0".repeat(64));
            },
        },
        TamperCase {
            label: "wrong-size",
            needle: "size",
            mutate: |value| {
                let size = value["releases"][0]["size"]
                    .as_u64()
                    .expect("index archive size");
                value["releases"][0]["size"] = json!(size + 1);
            },
        },
    ]
}

fn write_tampered_index(index_path: &Path, original_index: &[u8], case: &TamperCase) {
    let mut value: Value = serde_json::from_slice(original_index).expect("parse pristine index");
    (case.mutate)(&mut value);
    atomic_write(
        index_path,
        &serde_json::to_vec_pretty(&value).expect("serialize tampered index"),
        0o644,
    )
    .expect("write tampered index");
}

fn assert_tampered_case(
    base: &TestDir,
    prepared: &PreparedRelease,
    tripwires: &Tripwires,
    index_path: &Path,
    original_index: &[u8],
    index: usize,
    case: &TamperCase,
) {
    write_tampered_index(index_path, original_index, case);

    let root_path = base.path().join(format!("root-{index}"));
    fs::create_dir(&root_path).expect("create failure root");
    wxml_delivery::manifest::set_mode(&root_path, 0o700).expect("make failure root private");
    let mut install = installer_command(tripwires, base.path());
    install.args([
        "install",
        "--root",
        root_path.to_str().expect("UTF-8 root"),
        "--from-dir",
        prepared.release.to_str().expect("UTF-8 release"),
        "--version",
        PACKAGE_VERSION,
        "--offline",
    ]);
    let output = run_installer(&mut install, tripwires);
    assert!(
        !output.status.success(),
        "{} unexpectedly installed: stdout={} stderr={}",
        case.label,
        stdout(&output),
        stderr(&output)
    );
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains(case.needle),
        "{}: expected `{}` in output: {combined}",
        case.label,
        case.needle
    );
    assert_no_failure_artifacts(&root_path, case.label);
    println!(
        "{} rejected before activation: {}",
        case.label,
        combined.trim()
    );
}

fn assert_no_failure_artifacts(root_path: &Path, label: &str) {
    for name in ["current", "versions", "state.json"] {
        assert!(
            !root_path.join(name).exists(),
            "{label}: {name} must not exist"
        );
    }
    let tmp = root_path.join("tmp");
    if tmp.exists() {
        assert!(
            fs::read_dir(&tmp)
                .expect("read failure tmp")
                .next()
                .is_none(),
            "{label}: staging files were left in tmp/"
        );
    }
    assert_eq!(
        mode_of(root_path),
        0o700,
        "{label}: root mode was not preserved"
    );
}
