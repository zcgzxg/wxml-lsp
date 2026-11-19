//! Qualified real-engine checks for the F3-06/F3-07 delivery slice.
//!
//! This harness is delivery-path evidence, not an F3 closure. It drives the
//! production `wxml-lsp-install` binary with a real native 7.1.0-beta engine:
//!
//! 1. the pinned tarball is verified by SRI, SHA-256 and size *before*
//!    extraction; the extracted `lib/tsc` is audited and executed with an empty
//!    environment to prove the exact `Version 7.1.0-beta` line;
//! 2. two schema-v2 package trees share that engine but carry distinct package
//!    versions (N1 = [`support::PACKAGE_VERSION`], N2 = `0.2.0`);
//! 3. `install` N1, `upgrade` N2 and `rollback --to N1 --offline` run over
//!    local-only sources with a PATH that contains only Node tripwire shims;
//! 4. a tampered N2 tree whose archive still matches its index is rejected
//!    before activation, and a corrupt installed N1 is refused as a rollback
//!    target without executing it.
//!
//! All asset-dependent tests are `#[ignore]`d, so a default run reports
//! `ignored`, never green evidence. The required invocation is:
//!
//! ```text
//! TSGO_NATIVE_TARBALL=<pinned 7.1.0-beta tgz> WXML_REQUIRE_REAL_ENGINE=1 \
//!   cargo test -p wxml-delivery --test native_real_upgrade -- --ignored --nocapture
//! ```
//!
//! In required mode `TSGO_NATIVE_TARBALL` is mandatory before any fallback; the
//! `/tmp/opencode/...` fallback is optional-mode only. Only `1`/`true` enable
//! required mode; other non-empty values fail closed.
//!
//! BLOCKED (F3-06/F3-07 remain open):
//! - Post-switch failure/interruption (`AfterSwitch`/`BeforeStateWrite`/
//!   `AfterStateWrite` and crash recovery) is covered only by injected-hook
//!   synthetic tests in `native_delivery.rs` / `native_rollback.rs`, not here.
//! - No network-namespace or syscall/execve audit is performed; `--offline` and
//!   dead-proxy environment variables are the only network evidence.
//! - No F3-13 ABI check, no real release package, and no production Rust
//!   binaries: `bin/`, `typings/`, `zed-extension/` and the activation probes
//!   are test fixtures (the Rust entry points are shell scripts).
//! - Rollback re-verification is evidenced only by the corrupt-N1 negative.
//!
//! No F3 stage ID is closed by this harness.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty
)]

#[path = "real_asset/support.rs"]
#[allow(dead_code)]
mod support;

use std::env::VarError;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};
use support::{
    TestDir, Tripwires, assert_supported_host, audit_upstream_package, build_v2_tree,
    create_private_install_root, installer_command, mode_of, run_installer, run_native_engine,
    run_with_timeout, shell_quote, stage_verified_asset, stdout,
};
use wxml_delivery::archive::create_tar_gz;
use wxml_delivery::index_v2::{IndexV2, ReleaseV2, read_index_v2};
use wxml_delivery::install::InstallRoot;
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_BINARY_PATH, ENGINE_DISTRIBUTION,
    ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_DISTRIBUTION_SHA256,
    ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE, ENGINE_NAME, ENGINE_PROBE_PREFIX,
    ENGINE_SOURCE_COMMIT, ENGINE_VERSION, ExpectedManifestV2, ManifestV2, NATIVE_PACKAGE_PLATFORM,
    NativeArtifactKind, read_manifest_v2,
};
use wxml_delivery::semver::compare_versions;
use wxml_delivery::state::{InstallState, Journal, TransactionOp, state_path};
use wxml_delivery::verify_v2::{checksums_for_tree_v2, tree_fingerprint, verify_tree_v2};

/// N1 version. `support::build_v2_tree` writes exactly this version.
const N1: &str = support::PACKAGE_VERSION;
/// N2 version; stable and newer than N1 so `upgrade` selects it.
const N2: &str = "0.2.0";
/// Native RPC protocol shared by both fixtures.
const PROTOCOL: u64 = 2;
const TARBALL_ENV: &str = "TSGO_NATIVE_TARBALL";
const REQUIRE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";
/// Optional-mode fallback; never consulted in required mode.
const FALLBACK_TARBALL: &str =
    "/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz";
const EXPECTED_INTEGRITY_FAILURE: &str =
    "error: package integrity check failed: payload `engine/lib/tsc` does not match its SHA-256";

/// Positive control only: it is not asset evidence, it proves the shims work.
#[test]
fn node_tripwire_positive_control_is_exercised() {
    let base = TestDir::new("real-upgrade-tripwire");
    let tripwires = Tripwires::install(base.path());
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "node --version && npm --version"])
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
        "tripwire node did not exit 99: status={:?} stdout={}",
        output.status,
        stdout(&output)
    );
    assert!(tripwires.marker().is_file());
    println!("node tripwire positive control: exit=99 (not asset evidence)");
    tripwires.clear_marker();
    tripwires.assert_not_executed();
}

#[test]
#[ignore = "requires TSGO_NATIVE_TARBALL"]
fn real_engine_n1_install_n2_upgrade_then_offline_rollback() {
    let prepared = prepare_real_release("upgrade-rollback");
    let root_path = create_private_install_root(&prepared.base, "root");
    let tripwires = Tripwires::install(prepared.base.path());
    let root = open_root(&root_path);

    install_n1(&prepared, &root_path, &tripwires);
    let n1_fingerprint = assert_after_install(&root, &prepared);
    upgrade_n2(&prepared, &prepared.release, &root_path, &tripwires, true);
    let (n1_fingerprint, n2_fingerprint) = assert_after_upgrade(&root, &prepared, n1_fingerprint);
    rollback_n1(&prepared, &root_path, &tripwires, true);
    assert_after_rollback(&root, &prepared, &n1_fingerprint, &n2_fingerprint);

    tripwires.assert_not_executed();
    assert_eq!(mode_of(&root_path), 0o700, "install root mode changed");
    println!(
        "qualified real-engine transition: {N1} -> {N2} -> {N1}, state.previous={N2}, \
         engine={ENGINE_NAME} {ENGINE_VERSION}, engineSha256={}, \
         tarballSha256={ENGINE_DISTRIBUTION_SHA256}, no PATH Node",
        prepared.engine_sha256
    );
}

#[test]
#[ignore = "requires TSGO_NATIVE_TARBALL"]
fn tampered_real_n2_tree_is_rejected_before_activation() {
    let prepared = prepare_real_release("upgrade-failure");
    let root_path = create_private_install_root(&prepared.base, "root");
    let tripwires = Tripwires::install(prepared.base.path());
    let root = open_root(&root_path);

    install_n1(&prepared, &root_path, &tripwires);
    let n1_fingerprint = assert_after_install(&root, &prepared);
    let before = N1Snapshot::capture(&root);
    let tampered_release = build_tampered_release(&prepared);
    let output = upgrade_n2(&prepared, &tampered_release, &root_path, &tripwires, false);

    assert_integrity_failure(&output, &format!("upgraded to {N2}"));
    assert_n1_unchanged(&root, &prepared, &before, &n1_fingerprint);
    tripwires.assert_not_executed();
    println!("tampered N2 rejected before switch; N1 current/state/tree unchanged");
}

#[test]
#[ignore = "requires TSGO_NATIVE_TARBALL"]
fn corrupt_installed_n1_is_not_executed_or_activated_by_rollback() {
    let prepared = prepare_real_release("rollback-negative");
    let root_path = create_private_install_root(&prepared.base, "root");
    let tripwires = Tripwires::install(prepared.base.path());
    let root = open_root(&root_path);

    install_n1(&prepared, &root_path, &tripwires);
    upgrade_n2(&prepared, &prepared.release, &root_path, &tripwires, true);
    let n2_fingerprint = assert_exact_tree(&root, N2, &prepared.engine_sha256, N1);
    let before = N1Snapshot::capture(&root);
    let trap = N1Trap::install(&prepared, &root);
    let output = rollback_n1(&prepared, &root_path, &tripwires, false);
    assert_corrupt_n1_rejected(&root, &prepared, &trap, &before, &n2_fingerprint, &output);

    fs::remove_dir_all(root.versions_dir().join(N1)).expect("remove corrupt N1");
    copy_tree(&prepared.tree_n1, &root.versions_dir().join(N1));
    rollback_n1(&prepared, &root_path, &tripwires, true);
    assert_state(&root, N1, Some(N2), TransactionOp::Rollback);
    assert_current(&root, N1);
    assert_eq!(
        assert_exact_tree(&root, N1, &prepared.engine_sha256, N2),
        prepared.n1_fingerprint
    );
    assert_eq!(
        assert_exact_tree(&root, N2, &prepared.engine_sha256, N1),
        n2_fingerprint
    );
    assert_clean_state(&root);
    println!("corrupt N1 refused without execution; repaired N1 then rolled back");
}

struct N1Trap {
    engine: PathBuf,
    marker: PathBuf,
    sha256: String,
}

impl N1Trap {
    fn install(prepared: &RealRelease, root: &InstallRoot) -> Self {
        let engine = root.versions_dir().join(N1).join(ENGINE_BINARY_PATH);
        let marker = prepared.base.path().join("N1_WAS_EXECUTED");
        install_execution_trap(&engine, &marker);
        let sha256 = sha256_file(&engine).expect("hash N1 trap");
        Self {
            engine,
            marker,
            sha256,
        }
    }
}

fn assert_corrupt_n1_rejected(
    root: &InstallRoot,
    prepared: &RealRelease,
    trap: &N1Trap,
    before: &N1Snapshot,
    n2_fingerprint: &str,
    output: &Output,
) {
    assert_integrity_failure(output, &format!("rolled back to {N1}"));
    assert!(
        !trap.marker.exists(),
        "rollback executed the tampered N1 engine: {}",
        trap.marker.display()
    );
    assert_eq!(
        sha256_file(&trap.engine).expect("rehash N1 trap"),
        trap.sha256,
        "tampered N1 changed after the rejected rollback"
    );
    assert_eq!(
        root.current_version().expect("current").as_deref(),
        Some(N2)
    );
    assert_eq!(
        fs::canonicalize(root.path().join("current")).expect("canonical current"),
        before.current
    );
    assert_eq!(
        fs::read(state_path(root)).expect("read state"),
        before.state
    );
    let state = read_state(root);
    assert_eq!(
        (state.active.as_str(), state.previous.as_deref()),
        (N2, Some(N1))
    );
    assert_eq!(
        assert_exact_tree(root, N2, &prepared.engine_sha256, N1),
        n2_fingerprint
    );
    assert!(root.versions_dir().join(N1).is_dir());
    assert_clean_state(root);
}

/// N1 bytes captured before a rejected transaction.
struct N1Snapshot {
    state: Vec<u8>,
    current: PathBuf,
    engine: Vec<u8>,
}

impl N1Snapshot {
    fn capture(root: &InstallRoot) -> Self {
        Self {
            state: fs::read(state_path(root)).expect("read N1 state"),
            current: fs::canonicalize(root.path().join("current")).expect("canonical current"),
            engine: run_native_engine(&root.path().join("current").join(ENGINE_BINARY_PATH)),
        }
    }
}

fn build_tampered_release(prepared: &RealRelease) -> PathBuf {
    let tampered_tree = prepared.base.path().join("tree-n2-tampered");
    copy_tree(&prepared.tree_n2, &tampered_tree);
    flip_one_byte(&tampered_tree.join(ENGINE_BINARY_PATH));
    let tampered_release = prepared.base.path().join("release-tampered");
    write_release_set(&tampered_release, &[(N2, tampered_tree.as_path())]);
    assert_archive_matches_its_own_index(&tampered_release);
    tampered_release
}

fn assert_integrity_failure(output: &Output, forbidden_action: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit 1: stdout={} stderr={}",
        stdout(output),
        stderr_text(output)
    );
    let stderr = stderr_text(output);
    assert!(
        stderr
            .lines()
            .any(|line| line == EXPECTED_INTEGRITY_FAILURE),
        "missing exact integrity failure: stderr={stderr}"
    );
    let entry = stdout(output);
    assert!(
        !entry.contains(forbidden_action),
        "rejected transaction reported success: {entry}"
    );
    println!("expected integrity failure: {EXPECTED_INTEGRITY_FAILURE}");
}

fn assert_n1_unchanged(
    root: &InstallRoot,
    prepared: &RealRelease,
    before: &N1Snapshot,
    n1_fingerprint: &str,
) {
    assert_eq!(
        root.current_version().expect("current").as_deref(),
        Some(N1)
    );
    assert_eq!(
        fs::canonicalize(root.path().join("current")).expect("canonical current"),
        before.current,
        "current link changed on a pre-switch failure"
    );
    assert_eq!(
        fs::read(state_path(root)).expect("read state"),
        before.state,
        "state.json changed on a pre-switch failure"
    );
    let state = read_state(root);
    assert_eq!(
        (state.active.as_str(), state.previous.as_deref()),
        (N1, None)
    );
    assert_eq!(
        assert_exact_tree(root, N1, &prepared.engine_sha256, N2),
        n1_fingerprint,
        "N1 tree changed on a pre-switch failure"
    );
    assert!(
        !root.versions_dir().join(N2).exists(),
        "tampered N2 was renamed into versions/"
    );
    assert_clean_state(root);
    assert_eq!(
        run_native_engine(&root.path().join("current").join(ENGINE_BINARY_PATH)),
        before.engine,
        "N1 engine output changed"
    );
}

/// A fully prepared real-engine release channel plus both package trees.
struct RealRelease {
    base: TestDir,
    release: PathBuf,
    tree_n1: PathBuf,
    tree_n2: PathBuf,
    engine_sha256: String,
    n1_fingerprint: String,
}

fn prepare_real_release(label: &str) -> RealRelease {
    assert!(
        compare_versions(N2, N1).is_gt(),
        "N2 must sort after N1: N1={N1} N2={N2}"
    );
    let tarball = resolve_pinned_tarball(label);
    assert_supported_host();
    let base = TestDir::new(label);
    let (package, engine_sha256) = verify_extract_engine(&base, &tarball);

    let tree_n1 = base.path().join("tree-n1");
    let fixture = build_v2_tree(&tree_n1, &package);
    assert_eq!(fixture.manifest.package_version, N1);
    assert_eq!(manifest_engine_sha256(&fixture.manifest), engine_sha256);
    let n1_fingerprint = tree_fingerprint(&tree_n1).expect("N1 fixture fingerprint");

    let tree_n2 = base.path().join("tree-n2");
    copy_tree(&tree_n1, &tree_n2);
    retarget_tree(&tree_n2, N2);

    let release = base.path().join("release");
    write_release_set(
        &release,
        &[(N1, tree_n1.as_path()), (N2, tree_n2.as_path())],
    );
    RealRelease {
        base,
        release,
        tree_n1,
        tree_n2,
        engine_sha256,
        n1_fingerprint,
    }
}

/// Strictly parses `WXML_REQUIRE_REAL_ENGINE`: only `1`/`true` enable it.
fn asset_mode() -> AssetMode {
    match std::env::var(REQUIRE_ENV) {
        Err(VarError::NotPresent) => AssetMode::Optional,
        Ok(value) if value.is_empty() => AssetMode::Optional,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => AssetMode::Required,
        Ok(value) => panic!("{REQUIRE_ENV} must be unset, empty, `1` or `true`; got `{value}`"),
        Err(VarError::NotUnicode(_)) => panic!("{REQUIRE_ENV} must be valid UTF-8"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssetMode {
    Optional,
    Required,
}

/// Resolves the pinned tarball; required mode never consults the fallback.
fn resolve_pinned_tarball(label: &str) -> PathBuf {
    let mode = asset_mode();
    if let Some(value) = std::env::var_os(TARBALL_ENV) {
        if !value.is_empty() {
            let path = canonical_regular_file(Path::new(&value));
            println!("{label}: explicit pinned tarball {}", path.display());
            return path;
        }
    }
    assert!(
        mode == AssetMode::Optional,
        "{REQUIRE_ENV} requires explicit {TARBALL_ENV}; fallback {FALLBACK_TARBALL} is disabled \
         in required mode (pinned sha256 {ENGINE_DISTRIBUTION_SHA256}, \
         {ENGINE_DISTRIBUTION_SIZE} bytes)"
    );
    let fallback = Path::new(FALLBACK_TARBALL);
    assert!(
        fallback.is_file(),
        "no pinned asset for {label}: set {TARBALL_ENV} (fallback {FALLBACK_TARBALL} is absent). \
         Asset tests are ignored by default; run --ignored only when the pinned tarball exists"
    );
    let path = canonical_regular_file(fallback);
    eprintln!(
        "{label}: optional mode using fallback pinned tarball {}",
        path.display()
    );
    path
}

fn canonical_regular_file(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap_or_else(|error| {
        panic!("cannot resolve pinned tarball {}: {error}", path.display())
    });
    assert!(
        canonical.is_file(),
        "pinned tarball is not a regular file: {}",
        canonical.display()
    );
    canonical
}

/// Verifies the pinned tarball before extraction, audits the extracted package
/// and proves the real engine prints the exact frozen version line.
fn verify_extract_engine(base: &TestDir, tarball: &Path) -> (PathBuf, String) {
    let package = verify_and_extract(base, tarball);
    let audit = audit_upstream_package(&package);
    assert_eq!(
        run_native_engine(&package.join("lib/tsc")),
        format!("{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\n").as_bytes(),
        "upstream engine does not print the exact frozen version"
    );
    (package, audit.engine_sha256)
}

fn verify_and_extract(base: &TestDir, tarball: &Path) -> PathBuf {
    let private = base.path().join("pinned-engine.tgz");
    fs::copy(tarball, &private).expect("copy pinned tarball into the private test root");
    set_mode(&private, 0o600).expect("private tarball mode");

    let verified = wxml_delivery::sri::verify_asset_file(
        &private,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect("pinned identity must be verified before extraction");
    assert_eq!(verified.size, ENGINE_DISTRIBUTION_SIZE);
    assert_eq!(verified.sha256, ENGINE_DISTRIBUTION_SHA256);
    assert_eq!(
        verified.sha512.to_sri(),
        ENGINE_DISTRIBUTION_INTEGRITY,
        "SRI changed"
    );
    println!(
        "pinned asset verified: path={} sha256={} size={}",
        private.display(),
        verified.sha256,
        verified.size
    );

    let upstream = base.path().join("upstream");
    let extracted = stage_verified_asset(
        &private,
        &upstream,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect("pinned tarball extraction");
    assert_eq!(
        u64::try_from(extracted.files.len()).expect("file count"),
        ENGINE_DISTRIBUTION_FILE_COUNT
    );
    assert_eq!(
        extracted.total_bytes, ENGINE_DISTRIBUTION_UNPACKED_SIZE,
        "unpacked size changed"
    );
    upstream.join("package")
}

fn install_n1(prepared: &RealRelease, root: &Path, tripwires: &Tripwires) {
    let output = run_from_dir(
        prepared,
        &prepared.release,
        root,
        tripwires,
        &["install", "--version", N1],
    );
    support::assert_success(&output, "install N1");
    assert_action_line(&output, &format!("installed {N1} (native)"));
}

fn upgrade_n2(
    prepared: &RealRelease,
    release: &Path,
    root: &Path,
    tripwires: &Tripwires,
    expect_success: bool,
) -> Output {
    let output = run_from_dir(prepared, release, root, tripwires, &["upgrade"]);
    if expect_success {
        support::assert_success(&output, "upgrade N2");
        assert_action_line(&output, &format!("upgraded to {N2} (native)"));
    }
    output
}

fn rollback_n1(
    prepared: &RealRelease,
    root: &Path,
    tripwires: &Tripwires,
    expect_success: bool,
) -> Output {
    let mut command = installer_command(tripwires, prepared.base.path());
    command.args([
        "rollback",
        "--root",
        root.to_str().expect("UTF-8 root"),
        "--to",
        N1,
        "--offline",
    ]);
    let output = run_installer(&mut command, tripwires);
    if expect_success {
        support::assert_success(&output, "rollback N1");
        assert_action_line(&output, &format!("rolled back to {N1}"));
    }
    output
}

fn run_from_dir(
    prepared: &RealRelease,
    release: &Path,
    root: &Path,
    tripwires: &Tripwires,
    command_args: &[&str],
) -> Output {
    let mut command = installer_command(tripwires, prepared.base.path());
    command.args(command_args);
    command.args([
        "--root",
        root.to_str().expect("UTF-8 root"),
        "--from-dir",
        release.to_str().expect("UTF-8 release"),
        "--offline",
    ]);
    run_installer(&mut command, tripwires)
}

fn open_root(path: &Path) -> InstallRoot {
    InstallRoot::open(path)
        .expect("open install root")
        .expect("install root exists")
}

fn assert_after_install(root: &InstallRoot, prepared: &RealRelease) -> String {
    assert_state(root, N1, None, TransactionOp::Install);
    assert_current(root, N1);
    assert_engine_version(root);
    let fingerprint = assert_exact_tree(root, N1, &prepared.engine_sha256, N2);
    assert_clean_state(root);
    assert_versions(root, &[N1]);
    println!("N1 installed: current={N1} state.previous=null");
    fingerprint
}

fn assert_after_upgrade(
    root: &InstallRoot,
    prepared: &RealRelease,
    n1_fingerprint: String,
) -> (String, String) {
    assert_state(root, N2, Some(N1), TransactionOp::Upgrade);
    assert_current(root, N2);
    assert_engine_version(root);
    assert_eq!(
        assert_exact_tree(root, N1, &prepared.engine_sha256, N2),
        n1_fingerprint,
        "N1 tree changed during upgrade"
    );
    let n2_fingerprint = assert_exact_tree(root, N2, &prepared.engine_sha256, N1);
    assert_clean_state(root);
    assert_versions(root, &[N1, N2]);
    println!("N2 upgraded: current={N2} state.previous={N1}, both trees intact");
    (n1_fingerprint, n2_fingerprint)
}

fn assert_after_rollback(
    root: &InstallRoot,
    prepared: &RealRelease,
    n1_fingerprint: &str,
    n2_fingerprint: &str,
) {
    assert_state(root, N1, Some(N2), TransactionOp::Rollback);
    assert_current(root, N1);
    assert_engine_version(root);
    assert_eq!(
        assert_exact_tree(root, N1, &prepared.engine_sha256, N2),
        n1_fingerprint
    );
    assert_eq!(
        assert_exact_tree(root, N2, &prepared.engine_sha256, N1),
        n2_fingerprint
    );
    assert_clean_state(root);
    assert_versions(root, &[N1, N2]);
    println!("offline rollback: current={N1} state.previous={N2}, both trees intact");
}

fn assert_state(root: &InstallRoot, active: &str, previous: Option<&str>, op: TransactionOp) {
    let state = read_state(root);
    assert_eq!(state.active, active);
    assert_eq!(state.previous.as_deref(), previous);
    assert_eq!(state.engine.name, ENGINE_NAME);
    assert_eq!(state.engine.version, ENGINE_VERSION);
    let last = state.last_transaction.as_ref().expect("last transaction");
    assert_eq!(last.op, op);
    assert_eq!(last.target, active);
    assert_eq!(last.previous.as_deref(), previous);
    state.validate_against(root).expect("state matches disk");
}

fn read_state(root: &InstallRoot) -> InstallState {
    InstallState::read(root)
        .expect("read state.json")
        .expect("state.json exists")
}

fn assert_current(root: &InstallRoot, version: &str) {
    assert_eq!(
        root.current_version().expect("current version").as_deref(),
        Some(version)
    );
    let current = fs::canonicalize(root.path().join("current")).expect("canonicalize current");
    let target = fs::canonicalize(root.versions_dir().join(version)).expect("canonicalize version");
    assert_eq!(current, target, "current does not point at {version}");
    assert!(root.current_binary().expect("current binary").is_file());
}

/// Re-verifies one tree and proves it holds this version's bytes and the real
/// pinned engine, not a mix with the other version.
fn assert_exact_tree(
    root: &InstallRoot,
    version: &str,
    engine_sha256: &str,
    other: &str,
) -> String {
    let directory = root.versions_dir().join(version);
    let report = verify_tree_v2(&directory, expected(version)).expect("installed tree verifies");
    assert_eq!(report.manifest.package_version, version);
    assert_eq!(
        sha256_file(&directory.join(ENGINE_BINARY_PATH)).expect("hash installed engine"),
        engine_sha256
    );
    let provenance: Value = serde_json::from_slice(
        &fs::read(directory.join("provenance.json")).expect("read provenance"),
    )
    .expect("parse provenance");
    assert_eq!(provenance["packageVersion"].as_str(), Some(version));
    let entry_point = fs::read_to_string(directory.join("bin/wxml-lsp")).expect("read entry point");
    assert_eq!(
        entry_point,
        format!("#!/bin/sh\nprintf 'wxml-lsp {version}\\n'\n")
    );
    assert!(
        !entry_point.contains(other),
        "tree {version} contains {other} bytes"
    );
    tree_fingerprint(&directory).expect("tree fingerprint")
}

fn assert_engine_version(root: &InstallRoot) {
    let engine = root.path().join("current").join(ENGINE_BINARY_PATH);
    assert_eq!(
        run_native_engine(&engine),
        format!("{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\n").as_bytes()
    );
}

fn assert_clean_state(root: &InstallRoot) {
    assert!(
        Journal::list(root).expect("list journals").is_empty(),
        "transaction journals were left behind"
    );
    support::assert_journal_absent_or_empty(root);
    if root.tmp_dir().exists() {
        assert!(
            fs::read_dir(root.tmp_dir())
                .expect("read tmp")
                .next()
                .is_none(),
            "staging files were left in tmp/"
        );
    }
}

fn assert_versions(root: &InstallRoot, expected_versions: &[&str]) {
    let mut versions: Vec<String> = fs::read_dir(root.versions_dir())
        .expect("read versions")
        .map(|entry| {
            entry
                .expect("version entry")
                .file_name()
                .to_str()
                .expect("UTF-8 version")
                .to_owned()
        })
        .collect();
    versions.sort_by(|left, right| compare_versions(left, right));
    let expected_versions: Vec<String> =
        expected_versions.iter().map(ToString::to_string).collect();
    assert_eq!(versions, expected_versions);
}

fn assert_action_line(output: &Output, action: &str) {
    let text = stdout(output);
    assert!(text.lines().any(|line| line == action), "{text}");
    assert!(
        text.lines()
            .any(|line| line == format!("engine: {ENGINE_NAME} {ENGINE_VERSION}")),
        "{text}"
    );
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const fn expected(version: &str) -> ExpectedManifestV2<'_> {
    ExpectedManifestV2 {
        version: Some(version),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        protocol_version: Some(PROTOCOL),
    }
}

fn manifest_engine_sha256(manifest: &ManifestV2) -> String {
    manifest
        .binaries
        .iter()
        .find(|artifact| artifact.kind == NativeArtifactKind::NativeEngine)
        .expect("native engine artifact")
        .sha256
        .clone()
}

/// Rewrites the version-bearing fixture files and re-seals the tree.
fn retarget_tree(tree: &Path, version: &str) {
    rewrite_entry_points(tree, version);
    rewrite_provenance(tree, version);
    let manifest = rewrite_manifest(tree, version);
    atomic_write(
        &tree.join("manifest.json"),
        &manifest.to_json().expect("serialize manifest"),
        0o644,
    )
    .expect("write manifest");
    let checksums = checksums_for_tree_v2(tree, &manifest).expect("tree checksums");
    atomic_write(&tree.join("SHA256SUMS"), &checksums, 0o644).expect("write checksums");
    verify_tree_v2(tree, expected(version)).expect("retargeted tree verifies");
}

fn rewrite_entry_points(tree: &Path, version: &str) {
    for name in ["wxml-lsp", "wxml-lsp-install"] {
        atomic_write(
            &tree.join("bin").join(name),
            format!("#!/bin/sh\nprintf '{name} {version}\\n'\n").as_bytes(),
            0o755,
        )
        .expect("rewrite entry point");
    }
}

fn rewrite_provenance(tree: &Path, version: &str) {
    let provenance_path = tree.join("provenance.json");
    let mut provenance: Value =
        serde_json::from_slice(&fs::read(&provenance_path).expect("read provenance"))
            .expect("parse provenance");
    provenance["packageVersion"] = json!(version);
    provenance["note"] = json!(format!(
        "Derived schema v2 fixture ({version}) generated by \
         crates/wxml-delivery/tests/native_real_upgrade.rs from the real-engine N1 tree; only \
         engine/ comes from the pinned upstream tarball, and this is not a production release."
    ));
    let mut provenance_bytes =
        serde_json::to_vec_pretty(&provenance).expect("serialize provenance");
    provenance_bytes.push(b'\n');
    atomic_write(&provenance_path, &provenance_bytes, 0o644).expect("write provenance");
}

/// Moves the manifest to `version` and re-hashes the rewritten payload files.
fn rewrite_manifest(tree: &Path, version: &str) -> ManifestV2 {
    let manifest_path = tree.join("manifest.json");
    let mut manifest: ManifestV2 =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read manifest"))
            .expect("parse manifest");
    version.clone_into(&mut manifest.package_version);
    for artifact in &mut manifest.binaries {
        if artifact.kind == NativeArtifactKind::Rust {
            version.clone_into(&mut artifact.version);
        }
    }
    for path in ["bin/wxml-lsp", "bin/wxml-lsp-install", "provenance.json"] {
        let file = manifest
            .files
            .iter_mut()
            .find(|entry| entry.path == path)
            .expect("version-bearing file entry");
        let bytes = fs::read(tree.join(path)).expect("read rewritten file");
        file.sha256 = sha256_bytes(&bytes);
        file.size = u64::try_from(bytes.len()).expect("file size");
    }
    for artifact in &mut manifest.binaries {
        if artifact.kind == NativeArtifactKind::Rust {
            artifact.sha256 = manifest
                .files
                .iter()
                .find(|entry| entry.path == artifact.path)
                .expect("artifact payload entry")
                .sha256
                .clone();
        }
    }
    manifest
        .validate(expected(version))
        .expect("N2 manifest validates");
    manifest
}

/// Copies a verified package tree while preserving file contents and modes.
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("create copied tree root");
    set_mode(destination, 0o755).expect("copied tree root mode");
    let mut entries: Vec<_> = fs::read_dir(source)
        .expect("read source tree")
        .map(|entry| entry.expect("source entry").path())
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("file name"));
        let metadata = fs::symlink_metadata(&entry).expect("source metadata");
        if metadata.file_type().is_dir() {
            copy_tree(&entry, &target);
        } else {
            assert!(
                metadata.file_type().is_file(),
                "unexpected entry {} in package tree",
                entry.display()
            );
            fs::copy(&entry, &target).expect("copy package file");
            set_mode(&target, metadata.permissions().mode() & 0o777).expect("copied file mode");
        }
    }
}

fn flip_one_byte(path: &Path) {
    let mut bytes = fs::read(path).expect("read payload for tampering");
    assert!(!bytes.is_empty());
    *bytes.last_mut().expect("non-empty payload") ^= 0xff;
    fs::write(path, &bytes).expect("write tampered payload");
    set_mode(path, 0o755).expect("restore tampered payload mode");
}

/// Same-size replacement that creates `marker` and prints the exact version if
/// it is ever executed. Verification must reject it before the probe runs.
fn install_execution_trap(engine: &Path, marker: &Path) {
    let size = fs::metadata(engine).expect("engine metadata").len();
    let script = format!(
        "#!/bin/sh\n: > {}\nprintf '{}%s\\n' '{}'\nexit 0\n",
        shell_quote(marker),
        ENGINE_PROBE_PREFIX,
        ENGINE_VERSION
    );
    let mut bytes = script.into_bytes();
    assert!(
        u64::try_from(bytes.len()).expect("script length") < size,
        "execution trap does not fit in the engine file"
    );
    bytes.resize(usize::try_from(size).expect("engine size fits usize"), b'#');
    atomic_write(engine, &bytes, 0o755).expect("write same-size execution trap");
    assert_eq!(fs::metadata(engine).expect("trap metadata").len(), size);
}

fn write_release_set(release: &Path, trees: &[(&str, &Path)]) {
    fs::create_dir(release).expect("create release directory");
    let mut index = IndexV2::empty();
    for (version, tree) in trees {
        let archive_name = format!("wxml-lsp-{version}-{NATIVE_PACKAGE_PLATFORM}.tar.gz");
        let manifest = read_manifest_v2(tree, expected(version)).expect("read release manifest");
        let mut entries = manifest.files;
        for name in ["manifest.json", "SHA256SUMS"] {
            let path = tree.join(name);
            entries.push(FileEntry {
                path: name.to_owned(),
                sha256: sha256_file(&path).expect("hash control file"),
                size: fs::metadata(&path).expect("control file metadata").len(),
                mode: "0644".to_owned(),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let archive = release.join(&archive_name);
        create_tar_gz(tree, &archive, &entries, 0).expect("create release archive");
        let bytes = fs::read(&archive).expect("read release archive");
        index.upsert(release_record(version, &archive_name, &bytes));
    }
    atomic_write(
        &release.join("index.json"),
        &index.to_json().expect("serialize release index"),
        0o644,
    )
    .expect("write release index");
}

fn release_record(version: &str, archive_name: &str, bytes: &[u8]) -> ReleaseV2 {
    ReleaseV2 {
        version: version.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: PROTOCOL,
        engine_version: ENGINE_VERSION.to_owned(),
        engine_source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        engine_protocol_version: PROTOCOL,
        engine_api_schema_version: ENGINE_API_SCHEMA_VERSION,
        engine_ast_header_bytes: ENGINE_AST_HEADER_BYTES,
        engine_distribution: ENGINE_DISTRIBUTION.to_owned(),
        engine_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        min_installer_schema: 2,
        archive: archive_name.to_owned(),
        archive_integrity: wxml_delivery::sri::sha512_sri_bytes(bytes).to_sri(),
        sha256: sha256_bytes(bytes),
        size: u64::try_from(bytes.len()).expect("archive size"),
    }
}

/// The tampered archive must pass its own index integrity; only the extracted
/// tree is inconsistent with `manifest.json`.
fn assert_archive_matches_its_own_index(release: &Path) {
    let index = read_index_v2(&release.join("index.json")).expect("read tampered index");
    let record = index.find(N2, NATIVE_PACKAGE_PLATFORM).expect("N2 record");
    record
        .verify_archive(&release.join(&record.archive))
        .expect("tampered archive matches its own index");
}
