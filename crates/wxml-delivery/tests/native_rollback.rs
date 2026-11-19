//! F3-09 legacy -> native rollback/reconciliation tests.
//!
//! Every package tree here is synthetic. The native engine is injected through
//! [`FakeProbe`], so no engine, Node-family process or network source is ever
//! started. The only executed payloads are the tiny shell scripts generated
//! below, which the activation-binary checks run with an empty environment.
//! Process-level no-Node proof lives in `no_node_guard.rs`; this module does
//! not claim an inert PATH tripwire as evidence.
//!
//! The fault points are deterministic: they are installed through the public
//! [`InstallHooks`] seam rather than by sleeping or racing a real process.
//! The `SwitchOutcome::SwitchedButNotDurable` fsync path has no injection seam
//! and is deliberately not claimed as covered here.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::too_many_lines,
    clippy::indexing_slicing
)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use support::TempDir;
use wxml_delivery::error::DeliveryError;
use wxml_delivery::install::InstallRoot;
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes};
use wxml_delivery::manifest_v2::{
    ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY,
    ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE,
    ENGINE_LICENSE, ENGINE_PROBE_PREFIX, ENGINE_SOURCE_COMMIT, ENGINE_VERSION, EngineInfo,
    EngineProbe, ExpectedManifestV2, MANIFEST_SCHEMA_V2, MINIPROGRAM_TYPINGS_FILE_COUNT,
    MINIPROGRAM_TYPINGS_LICENSE, MINIPROGRAM_TYPINGS_ROOT, MINIPROGRAM_TYPINGS_VERSION, ManifestV2,
    NATIVE_PACKAGE_PLATFORM, NativeArtifactKind, NativeBinaryArtifact, STANDARD_LIB_ASSET,
    STANDARD_LIB_FILE_COUNT, STANDARD_LIB_INCLUDE, TypeAsset, compute_type_asset_bundle,
};
use wxml_delivery::native_install::{
    FailPoint, InstallHooks, NativeEngineProbe, NativeInstallOutcome, NoHooks, install_native,
    recover_native, rollback_native, verify_native,
};
use wxml_delivery::state::{InstallState, Journal, TransactionState};
use wxml_delivery::verify_v2::{checksums_for_tree_v2, tree_fingerprint, verify_tree_v2};

const VERSION_1: &str = "0.1.0";
const VERSION_2: &str = "0.2.0";
const LEGACY_VERSION: &str = "0.0.9";

#[derive(Debug, Default)]
struct FakeProbe {
    calls: AtomicUsize,
    fail: bool,
}

impl FakeProbe {
    const fn failing() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail: true,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl NativeEngineProbe for FakeProbe {
    fn probe(&self, _tree: &Path, _manifest: &ManifestV2) -> wxml_delivery::Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return Err(DeliveryError::Runtime("injected probe failure".to_owned()));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct FailAt {
    point: FailPoint,
}

impl InstallHooks for FailAt {
    fn checkpoint(&self, point: FailPoint) -> wxml_delivery::Result<()> {
        if point == self.point {
            return Err(DeliveryError::Runtime(format!(
                "injected failure at {point:?}"
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct FailThenRemove {
    fail: FailPoint,
    remove: FailPoint,
    path: PathBuf,
}

impl InstallHooks for FailThenRemove {
    fn checkpoint(&self, point: FailPoint) -> wxml_delivery::Result<()> {
        if point == self.fail {
            return Err(DeliveryError::Runtime(format!(
                "injected failure at {point:?}"
            )));
        }
        if point == self.remove {
            fs::remove_dir_all(&self.path).unwrap();
        }
        Ok(())
    }
}

#[derive(Debug)]
struct TamperAt {
    point: FailPoint,
    path: PathBuf,
    bytes: &'static [u8],
}

impl InstallHooks for TamperAt {
    fn checkpoint(&self, point: FailPoint) -> wxml_delivery::Result<()> {
        if point == self.point {
            fs::write(&self.path, self.bytes).unwrap();
            return Err(DeliveryError::Runtime(format!(
                "injected failure at {point:?}"
            )));
        }
        Ok(())
    }
}

const fn expected(version: &str) -> ExpectedManifestV2<'_> {
    ExpectedManifestV2 {
        version: Some(version),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        protocol_version: Some(2),
    }
}

struct FixtureBuilder {
    root: PathBuf,
    files: Vec<FileEntry>,
}

impl FixtureBuilder {
    fn new(root: &Path) -> Self {
        fs::create_dir_all(root).unwrap();
        Self {
            root: root.to_path_buf(),
            files: Vec::new(),
        }
    }

    fn file(&mut self, path: &str, bytes: &[u8], mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, bytes).unwrap();
        set_mode(&destination, mode).unwrap();
        self.files.push(FileEntry {
            path: path.to_owned(),
            sha256: sha256_bytes(bytes),
            size: u64::try_from(bytes.len()).unwrap(),
            mode: format!("{mode:04o}"),
        });
    }

    fn hash(&self, path: &str) -> String {
        self.files
            .iter()
            .find(|file| file.path == path)
            .unwrap()
            .sha256
            .clone()
    }
}

fn stage_fixture(root: &InstallRoot, version: &str) -> (PathBuf, ManifestV2) {
    let staged = root.new_temp_dir("staged").unwrap();
    let manifest = build_fixture(&staged, version);
    (staged, manifest)
}

fn build_fixture(root: &Path, version: &str) -> ManifestV2 {
    let lsp = format!("#!/bin/sh\nprintf 'wxml-lsp {version}\\n'\n");
    let installer = format!("#!/bin/sh\nprintf 'wxml-lsp-install {version}\\n'\n");
    let mut builder = FixtureBuilder::new(root);
    builder.file("bin/wxml-lsp", lsp.as_bytes(), 0o755);
    builder.file("bin/wxml-lsp-install", installer.as_bytes(), 0o755);
    builder.file("engine/lib/tsc", b"\x7fELF-fixture-native-engine", 0o755);
    builder.file("engine/lib/tsc.sig", b"signature-placeholder\n", 0o644);
    builder.file("engine/LICENSE", b"Apache License fixture\n", 0o644);
    builder.file("engine/NOTICE.txt", b"NOTICE fixture\n", 0o644);
    builder.file("engine/package.json", b"{\"name\":\"fixture\"}\n", 0o644);
    builder.file("engine/lib/lib.d.ts", b"// lib.d.ts\n", 0o644);
    builder.file("engine/lib/lib.es5.d.ts", b"// lib.es5.d.ts\n", 0o644);
    builder.file("engine/lib/lib.es2020.d.ts", b"// lib.es2020.d.ts\n", 0o644);
    for index in 0..113 {
        builder.file(
            &format!("engine/lib/lib.f{index:03}.d.ts"),
            format!("// filler {index}\n").as_bytes(),
            0o644,
        );
    }
    builder.file(
        "typings/miniprogram-api-typings/index.d.ts",
        b"// miniprogram index\n",
        0o644,
    );
    builder.file(
        "typings/miniprogram-api-typings/LICENSE",
        b"MIT fixture\n",
        0o644,
    );
    for index in 0..24 {
        builder.file(
            &format!("typings/miniprogram-api-typings/types/t{index:02}.d.ts"),
            format!("// typings {index}\n").as_bytes(),
            0o644,
        );
    }
    builder.file("zed-extension/extension.wasm", b"\0asm-fixture", 0o644);
    builder.file(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\n",
        0o644,
    );
    builder.file(
        "provenance.json",
        format!("{{\"fixture\":true,\"version\":\"{version}\"}}\n").as_bytes(),
        0o644,
    );
    builder.file("THIRD_PARTY_LICENSES.md", b"# licenses\n", 0o644);

    let standard = compute_type_asset_bundle(
        &builder.files,
        "engine/lib",
        &[STANDARD_LIB_INCLUDE.to_owned()],
    )
    .unwrap();
    assert_eq!(standard.file_count, STANDARD_LIB_FILE_COUNT);
    let typings =
        compute_type_asset_bundle(&builder.files, MINIPROGRAM_TYPINGS_ROOT, &["**".to_owned()])
            .unwrap();
    assert_eq!(typings.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);

    let manifest = ManifestV2 {
        schema_version: MANIFEST_SCHEMA_V2,
        package: "wxml-lsp".to_owned(),
        package_version: version.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: 2,
        built_at: "2026-11-17T00:00:00Z".to_owned(),
        engine: EngineInfo {
            name: "typescript".to_owned(),
            version: ENGINE_VERSION.to_owned(),
            source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
            distribution: ENGINE_DISTRIBUTION.to_owned(),
            distribution_url: "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz".to_owned(),
            distribution_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
            distribution_sha256: ENGINE_DISTRIBUTION_SHA256.to_owned(),
            distribution_size: ENGINE_DISTRIBUTION_SIZE,
            distribution_file_count: ENGINE_DISTRIBUTION_FILE_COUNT,
            distribution_unpacked_size: ENGINE_DISTRIBUTION_UNPACKED_SIZE,
            api_schema_version: 9,
            ast_header_bytes: 64,
            license: ENGINE_LICENSE.to_owned(),
            license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
            binary: "engine/lib/tsc".to_owned(),
            probe: EngineProbe {
                args: vec!["--version".to_owned()],
                expect_prefix: ENGINE_PROBE_PREFIX.to_owned(),
            },
        },
        type_assets: vec![
            TypeAsset {
                name: STANDARD_LIB_ASSET.to_owned(),
                version: ENGINE_VERSION.to_owned(),
                license: ENGINE_LICENSE.to_owned(),
                root: "engine/lib".to_owned(),
                include: vec![STANDARD_LIB_INCLUDE.to_owned()],
                file_count: standard.file_count,
                sha256: standard.sha256,
                license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
            },
            TypeAsset {
                name: wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_ASSET.to_owned(),
                version: MINIPROGRAM_TYPINGS_VERSION.to_owned(),
                license: MINIPROGRAM_TYPINGS_LICENSE.to_owned(),
                root: MINIPROGRAM_TYPINGS_ROOT.to_owned(),
                include: vec!["**".to_owned()],
                file_count: typings.file_count,
                sha256: typings.sha256,
                license_files: vec![format!("{MINIPROGRAM_TYPINGS_ROOT}/LICENSE")],
            },
        ],
        binaries: vec![
            NativeBinaryArtifact {
                kind: NativeArtifactKind::Rust,
                name: "wxml-lsp".to_owned(),
                path: "bin/wxml-lsp".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: builder.hash("bin/wxml-lsp"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::Rust,
                name: "wxml-lsp-install".to_owned(),
                path: "bin/wxml-lsp-install".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: builder.hash("bin/wxml-lsp-install"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::NativeEngine,
                name: "tsgo".to_owned(),
                path: "engine/lib/tsc".to_owned(),
                version: ENGINE_VERSION.to_owned(),
                protocol_version: Some(2),
                sha256: builder.hash("engine/lib/tsc"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::ZedExtension,
                name: "zed-extension".to_owned(),
                path: "zed-extension/extension.wasm".to_owned(),
                version: "0.0.1".to_owned(),
                protocol_version: None,
                sha256: builder.hash("zed-extension/extension.wasm"),
            },
        ],
        files: builder.files,
    };
    manifest.validate(expected(version)).unwrap();
    atomic_write(
        &root.join("manifest.json"),
        &manifest.to_json().unwrap(),
        0o644,
    )
    .unwrap();
    let checksums = checksums_for_tree_v2(root, &manifest).unwrap();
    atomic_write(&root.join("SHA256SUMS"), &checksums, 0o644).unwrap();
    verify_tree_v2(root, expected(version)).unwrap();
    manifest
}

fn write_legacy_version(root: &InstallRoot, version: &str) -> String {
    let directory = root.versions_dir().join(version);
    fs::create_dir_all(&directory).unwrap();
    let manifest = json!({
        "schemaVersion": 1,
        "package": "wxml-lsp",
        "packageVersion": version,
        "platform": NATIVE_PACKAGE_PLATFORM,
        "protocolVersion": 2,
        "node": {"minimumVersion": "24.0.0", "backendVersion": "0.1.0"},
        "builtAt": "2026-01-01T00:00:00Z",
        "binaries": [{
            "kind": "node-backend",
            "name": "node-backend",
            "path": "bin/wxml-type-backend/server.js",
            "version": "0.1.0",
            "sha256": "0".repeat(64)
        }],
        "files": [],
        "unknownLegacyField": true
    });
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    set_mode(&directory.join("manifest.json"), 0o644).unwrap();
    root.switch(version).unwrap();
    tree_fingerprint(&directory).unwrap()
}

fn active_version(root: &InstallRoot) -> String {
    root.current_version().unwrap().unwrap()
}

fn current_link_target(root: &InstallRoot) -> Option<PathBuf> {
    fs::read_link(root.path().join("current")).ok()
}

fn state_bytes(root: &InstallRoot) -> Vec<u8> {
    fs::read(root.path().join("state.json")).unwrap()
}

fn assert_tmp_empty(root: &InstallRoot) {
    if let Ok(mut entries) = fs::read_dir(root.tmp_dir()) {
        assert!(entries.next().is_none(), "staging files were left in tmp/");
    }
}

fn assert_legacy_tree_unchanged(root: &InstallRoot, version: &str, fingerprint: &str) {
    assert_eq!(
        tree_fingerprint(&root.versions_dir().join(version)).unwrap(),
        fingerprint
    );
    assert!(
        fs::read_dir(root.versions_dir().join(version))
            .unwrap()
            .any(|entry| entry.unwrap().file_name() == "manifest.json")
    );
}

fn assert_legacy_unchanged(root: &InstallRoot, version: &str, fingerprint: &str) {
    assert_eq!(active_version(root), version);
    assert_legacy_tree_unchanged(root, version, fingerprint);
}

fn write_committed_journal(
    root: &InstallRoot,
    op: wxml_delivery::state::TransactionOp,
    target: &str,
    previous: Option<&str>,
) {
    let now = "2026-11-17T00:00:00Z";
    let mut journal = Journal::new(op, target, previous, now).unwrap();
    journal
        .transition(TransactionState::Committed, now)
        .unwrap();
    journal.write(root).unwrap();
}

#[test]
fn staging_failure_keeps_legacy_current_and_tree_byte_identical() {
    let base = TempDir::new("rollback-staging");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);
    let current_before = current_link_target(&root);

    let (staged, _) = stage_fixture(&root, VERSION_1);
    // A staging/extraction defect discovered by verification, before any
    // journal or rename.
    fs::remove_file(staged.join("engine/lib/lib.d.ts")).unwrap();
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_1),
        &FakeProbe::default(),
        &NoHooks,
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");

    assert_eq!(current_link_target(&root), current_before);
    assert_legacy_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
    assert!(InstallState::read(&root).unwrap().is_none());
    assert!(!root.versions_dir().join(VERSION_1).exists());
    assert!(Journal::list(&root).unwrap().is_empty());
    assert_tmp_empty(&root);

    // Recovery is idempotent and never activates or rewrites the legacy tree.
    for _ in 0..2 {
        let report = recover_native(&root, &FakeProbe::default()).unwrap();
        assert_eq!(report.current.as_deref(), Some(LEGACY_VERSION));
        assert_eq!(current_link_target(&root), current_before);
        assert_legacy_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
        assert!(InstallState::read(&root).unwrap().is_none());
        assert!(Journal::list(&root).unwrap().is_empty());
    }
}

#[test]
fn probe_failure_keeps_legacy_current_and_tree_byte_identical() {
    let base = TempDir::new("rollback-probe");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);
    let current_before = current_link_target(&root);

    let probe = FakeProbe::failing();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    let error = install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap_err();
    assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");
    assert_eq!(probe.calls(), 1);

    assert_eq!(current_link_target(&root), current_before);
    assert_legacy_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
    assert!(InstallState::read(&root).unwrap().is_none());
    assert!(!root.versions_dir().join(VERSION_1).exists());
    assert!(Journal::list(&root).unwrap().is_empty());
    assert_tmp_empty(&root);
}

#[test]
fn failure_after_target_rename_keeps_legacy_current_until_explicit_recovery() {
    let base = TempDir::new("rollback-rename");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);
    let current_before = current_link_target(&root);

    let (staged, _) = stage_fixture(&root, VERSION_1);
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_1),
        &FakeProbe::default(),
        &FailAt {
            point: FailPoint::AfterRename,
        },
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");

    // The version directory is committed, but `current` is still the legacy
    // Node package and no native state exists yet.
    assert_eq!(current_link_target(&root), current_before);
    assert_legacy_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
    assert!(root.versions_dir().join(VERSION_1).is_dir());
    assert!(InstallState::read(&root).unwrap().is_none());
    let journals = Journal::list(&root).unwrap();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0].state, TransactionState::Committed);
    assert_eq!(journals[0].previous, None, "legacy must never be previous");

    // Committed recovery must not auto-activate either; two runs are identical.
    for _ in 0..2 {
        let report = recover_native(&root, &FakeProbe::default()).unwrap();
        assert_eq!(report.current.as_deref(), Some(LEGACY_VERSION));
        assert!(
            report
                .actions
                .iter()
                .any(|action| action.contains("not auto-activated"))
        );
        assert_eq!(current_link_target(&root), current_before);
        assert_legacy_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
        assert!(InstallState::read(&root).unwrap().is_none());
        assert_eq!(Journal::list(&root).unwrap().len(), 1);
    }
}

#[test]
fn post_switch_failures_roll_forward_legacy_migration() {
    let points = [
        FailPoint::AfterSwitch,
        FailPoint::BeforeStateWrite,
        FailPoint::AfterStateWrite,
    ];
    for point in points {
        let base = TempDir::new(&format!("rollback-{point:?}"));
        let root = InstallRoot::create(&base.path().join("root")).unwrap();
        let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);

        let probe = FakeProbe::default();
        let (staged, _) = stage_fixture(&root, VERSION_1);
        let error = install_native(
            &root,
            &staged,
            expected(VERSION_1),
            &probe,
            &FailAt { point },
        )
        .unwrap_err();
        let DeliveryError::SwitchedButNotDurable {
            version,
            previous,
            state_reconciled,
            recovery,
            reason,
        } = error
        else {
            panic!("expected SwitchedButNotDurable for {point:?}, got {error:?}");
        };
        assert_eq!(version, VERSION_1);
        assert_eq!(previous, None, "legacy is never a native previous");
        assert!(
            state_reconciled,
            "state must be reconciled for {point:?}: {reason}"
        );
        assert!(
            recovery.contains("wxml-lsp-install verify"),
            "explicit recovery action missing for {point:?}: {recovery}"
        );
        assert!(reason.contains("injected failure"), "{reason}");

        // The switch is real and the verified native target is now current.
        assert_eq!(active_version(&root), VERSION_1);
        let state = InstallState::read(&root).unwrap().unwrap();
        assert_eq!(state.active, VERSION_1);
        assert_eq!(state.previous, None);
        let last = state.last_transaction.unwrap();
        assert_eq!(last.op, wxml_delivery::state::TransactionOp::Install);
        assert_eq!(last.target, VERSION_1);
        assert_eq!(last.previous, None);
        assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
        assert!(Journal::list(&root).unwrap().is_empty());
        assert_tmp_empty(&root);

        // Recovery is idempotent after the roll-forward.
        for _ in 0..2 {
            let report = recover_native(&root, &probe).unwrap();
            assert_eq!(report.current.as_deref(), Some(VERSION_1));
            assert_eq!(active_version(&root), VERSION_1);
            let state = InstallState::read(&root).unwrap().unwrap();
            assert_eq!(state.active, VERSION_1);
            assert_eq!(state.previous, None);
            assert!(Journal::list(&root).unwrap().is_empty());
            assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
        }

        // Rollback must refuse the legacy package and leave the native current.
        let error = rollback_native(&root, LEGACY_VERSION, &probe, &NoHooks).unwrap_err();
        assert!(error.to_string().contains("legacy Node package"), "{error}");
        assert_eq!(active_version(&root), VERSION_1);
        assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
    }
}

#[test]
fn recovery_rolls_forward_a_committed_legacy_target_idempotently() {
    let base = TempDir::new("rollback-recover");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);

    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(
        &root,
        &staged,
        expected(VERSION_1),
        &FakeProbe::default(),
        &FailAt {
            point: FailPoint::AfterRename,
        },
    )
    .unwrap_err();
    assert_eq!(active_version(&root), LEGACY_VERSION);
    // Simulate a stop after the symlink rename but before `state.json`.
    root.switch(VERSION_1).unwrap();
    assert!(InstallState::read(&root).unwrap().is_none());

    for run in 0..2 {
        let report = recover_native(&root, &FakeProbe::default()).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_1));
        assert_eq!(active_version(&root), VERSION_1);
        let state = InstallState::read(&root).unwrap().unwrap();
        assert_eq!(state.active, VERSION_1);
        assert_eq!(state.previous, None);
        assert!(state.last_transaction.is_some());
        assert!(Journal::list(&root).unwrap().is_empty());
        assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
        if run == 0 {
            assert!(
                report
                    .actions
                    .iter()
                    .any(|action| action.contains("verified and state written")),
                "{:?}",
                report.actions
            );
        }
    }
}

#[test]
fn successful_migration_and_upgrade_record_only_verified_native_previous() {
    let base = TempDir::new("rollback-previous");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);
    let probe = FakeProbe::default();

    let (staged, _) = stage_fixture(&root, VERSION_1);
    let outcome = install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    assert_eq!(outcome, NativeInstallOutcome::Migrated);
    let state = InstallState::read(&root).unwrap().unwrap();
    assert_eq!(state.active, VERSION_1);
    assert_eq!(state.previous, None, "legacy must never become previous");
    assert_eq!(
        state.last_transaction.unwrap().previous,
        None,
        "lastTransaction must not record the legacy package"
    );
    assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);

    let (staged, _) = stage_fixture(&root, VERSION_2);
    let outcome = install_native(&root, &staged, expected(VERSION_2), &probe, &NoHooks).unwrap();
    assert_eq!(outcome, NativeInstallOutcome::Upgraded);
    let state = InstallState::read(&root).unwrap().unwrap();
    assert_eq!(state.active, VERSION_2);
    assert_eq!(state.previous.as_deref(), Some(VERSION_1));
    let last = state.last_transaction.unwrap();
    assert_eq!(last.op, wxml_delivery::state::TransactionOp::Upgrade);
    assert_eq!(last.target, VERSION_2);
    assert_eq!(last.previous.as_deref(), Some(VERSION_1));
    assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);

    let error = rollback_native(&root, LEGACY_VERSION, &probe, &NoHooks).unwrap_err();
    assert!(error.to_string().contains("legacy Node package"), "{error}");
    assert_eq!(active_version(&root), VERSION_2);
    assert_legacy_tree_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
}

#[test]
fn upgrade_refuses_to_record_a_tampered_native_previous() {
    let base = TempDir::new("rollback-tampered-previous");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let state_before = state_bytes(&root);

    let n1 = root.versions_dir().join(VERSION_1);
    fs::write(n1.join("engine/lib/lib.d.ts"), b"// tampered\n").unwrap();
    let tampered_fingerprint = tree_fingerprint(&n1).unwrap();

    let (staged, _) = stage_fixture(&root, VERSION_2);
    let error = install_native(&root, &staged, expected(VERSION_2), &probe, &NoHooks).unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert_eq!(active_version(&root), VERSION_1);
    assert_eq!(state_bytes(&root), state_before);
    let state = InstallState::read(&root).unwrap().unwrap();
    assert_eq!(state.previous, None);
    assert!(!root.versions_dir().join(VERSION_2).exists());
    assert!(Journal::list(&root).unwrap().is_empty());
    assert_eq!(tree_fingerprint(&n1).unwrap(), tampered_fingerprint);
    assert_tmp_empty(&root);
}

#[test]
fn rollback_refuses_legacy_target_and_tampered_native_target() {
    let base = TempDir::new("rollback-refuse");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let (staged, _) = stage_fixture(&root, VERSION_2);
    install_native(&root, &staged, expected(VERSION_2), &probe, &NoHooks).unwrap();
    let state_before = state_bytes(&root);

    let legacy = root.versions_dir().join(LEGACY_VERSION);
    fs::create_dir_all(&legacy).unwrap();
    fs::write(
        legacy.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "package": "wxml-lsp",
            "packageVersion": LEGACY_VERSION,
            "platform": NATIVE_PACKAGE_PLATFORM
        }))
        .unwrap(),
    )
    .unwrap();
    let legacy_fingerprint = tree_fingerprint(&legacy).unwrap();

    let error = rollback_native(&root, LEGACY_VERSION, &probe, &NoHooks).unwrap_err();
    assert!(error.to_string().contains("legacy Node package"), "{error}");
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(state_bytes(&root), state_before);
    assert_eq!(tree_fingerprint(&legacy).unwrap(), legacy_fingerprint);
    assert!(Journal::list(&root).unwrap().is_empty());

    let n1 = root.versions_dir().join(VERSION_1);
    fs::write(n1.join("engine/lib/lib.es2020.d.ts"), b"// tampered\n").unwrap();
    let tampered_fingerprint = tree_fingerprint(&n1).unwrap();
    let error = rollback_native(&root, VERSION_1, &probe, &NoHooks).unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(state_bytes(&root), state_before);
    assert_eq!(tree_fingerprint(&n1).unwrap(), tampered_fingerprint);
}

#[test]
fn rollback_refuses_a_legacy_current_without_touching_native_targets() {
    let base = TempDir::new("rollback-legacy-current");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let legacy_fingerprint = write_legacy_version(&root, LEGACY_VERSION);
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();

    // Return to the legacy current while the verified native directory remains
    // installed, then remove the native state to model a pre-migration root.
    root.switch(LEGACY_VERSION).unwrap();
    fs::remove_file(root.path().join("state.json")).unwrap();
    let native_fingerprint = tree_fingerprint(&root.versions_dir().join(VERSION_1)).unwrap();

    let error = rollback_native(&root, VERSION_1, &probe, &NoHooks).unwrap_err();
    assert!(error.to_string().contains("legacy Node package"), "{error}");
    assert_eq!(active_version(&root), LEGACY_VERSION);
    assert_legacy_unchanged(&root, LEGACY_VERSION, &legacy_fingerprint);
    assert_eq!(
        tree_fingerprint(&root.versions_dir().join(VERSION_1)).unwrap(),
        native_fingerprint
    );
    assert!(InstallState::read(&root).unwrap().is_none());
    assert!(Journal::list(&root).unwrap().is_empty());
}

#[test]
fn native_after_state_write_rolls_forward_without_switching_back() {
    let base = TempDir::new("rollback-native-state-write");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let n1 = root.versions_dir().join(VERSION_1);
    let n1_fingerprint = tree_fingerprint(&n1).unwrap();

    let (staged, _) = stage_fixture(&root, VERSION_2);
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_2),
        &probe,
        &FailAt {
            point: FailPoint::AfterStateWrite,
        },
    )
    .unwrap_err();
    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        recovery,
        ..
    } = error
    else {
        panic!("expected typed post-switch state, got {error:?}");
    };
    assert_eq!(version, VERSION_2);
    assert_eq!(previous.as_deref(), Some(VERSION_1));
    assert!(state_reconciled);
    assert!(recovery.contains("wxml-lsp-install verify"), "{recovery}");

    // The persisted state already named N2: never switch back to N1.
    assert_eq!(active_version(&root), VERSION_2);
    let state = InstallState::read(&root).unwrap().unwrap();
    assert_eq!(state.active, VERSION_2);
    assert_eq!(state.previous.as_deref(), Some(VERSION_1));
    assert!(Journal::list(&root).unwrap().is_empty());
    assert_eq!(tree_fingerprint(&n1).unwrap(), n1_fingerprint);
    assert_tmp_empty(&root);

    for _ in 0..2 {
        let report = recover_native(&root, &probe).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_2));
        assert_eq!(active_version(&root), VERSION_2);
        assert_eq!(
            InstallState::read(&root).unwrap().unwrap().active,
            VERSION_2
        );
        assert!(Journal::list(&root).unwrap().is_empty());
    }
}

#[test]
fn recovery_repairs_current_previous_state_target_mismatch_idempotently() {
    let base = TempDir::new("rollback-mismatch-repair");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let (staged, _) = stage_fixture(&root, VERSION_2);
    install_native(&root, &staged, expected(VERSION_2), &probe, &NoHooks).unwrap();
    let n1_fingerprint = tree_fingerprint(&root.versions_dir().join(VERSION_1)).unwrap();
    let n2_fingerprint = tree_fingerprint(&root.versions_dir().join(VERSION_2)).unwrap();

    // Model the old restore_previous crash window: current was switched back
    // to N1, state.json still names N2, and the committed journal targets N2.
    write_committed_journal(
        &root,
        wxml_delivery::state::TransactionOp::Upgrade,
        VERSION_2,
        Some(VERSION_1),
    );
    root.switch(VERSION_1).unwrap();

    for run in 0..2 {
        let report = recover_native(&root, &probe).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_2));
        assert_eq!(active_version(&root), VERSION_2);
        let state = InstallState::read(&root).unwrap().unwrap();
        assert_eq!(state.active, VERSION_2);
        assert_eq!(state.previous.as_deref(), Some(VERSION_1));
        assert!(Journal::list(&root).unwrap().is_empty());
        assert_eq!(
            tree_fingerprint(&root.versions_dir().join(VERSION_1)).unwrap(),
            n1_fingerprint
        );
        assert_eq!(
            tree_fingerprint(&root.versions_dir().join(VERSION_2)).unwrap(),
            n2_fingerprint
        );
        if run == 0 {
            assert!(
                report
                    .actions
                    .iter()
                    .any(|action| action.contains("rolled forward")),
                "{:?}",
                report.actions
            );
        }
    }
}

#[test]
fn restore_previous_verify_failure_is_typed_and_fails_closed() {
    let base = TempDir::new("rollback-restore-verify");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let n1 = root.versions_dir().join(VERSION_1);
    let n1_lib = n1.join("engine/lib/lib.d.ts");

    let (staged, _) = stage_fixture(&root, VERSION_2);
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_2),
        &probe,
        &TamperAt {
            point: FailPoint::AfterSwitch,
            path: n1_lib.clone(),
            bytes: b"// tampered\n",
        },
    )
    .unwrap_err();
    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        ..
    } = error
    else {
        panic!("expected typed restore failure, got {error:?}");
    };
    assert_eq!(version, VERSION_2, "current stays on the verified target");
    assert_eq!(previous.as_deref(), Some(VERSION_1));
    assert!(!state_reconciled, "state still names the previous version");
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(
        InstallState::read(&root).unwrap().unwrap().active,
        VERSION_1
    );
    assert_eq!(
        Journal::list(&root).unwrap()[0].state,
        TransactionState::Committed
    );

    // Recovery fails closed while the recorded previous tree is tampered.
    let error = recover_native(&root, &probe).unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(
        InstallState::read(&root).unwrap().unwrap().active,
        VERSION_1
    );

    // Repairing the previous tree lets idempotent recovery finish.
    fs::write(&n1_lib, b"// lib.d.ts\n").unwrap();
    for _ in 0..2 {
        let report = recover_native(&root, &probe).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_2));
        let state = InstallState::read(&root).unwrap().unwrap();
        assert_eq!(state.active, VERSION_2);
        assert_eq!(state.previous.as_deref(), Some(VERSION_1));
        assert!(Journal::list(&root).unwrap().is_empty());
    }
}

#[test]
fn recovery_rejects_a_journal_recording_a_legacy_previous() {
    let base = TempDir::new("rollback-legacy-previous");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();

    let legacy = root.versions_dir().join(LEGACY_VERSION);
    fs::create_dir_all(&legacy).unwrap();
    fs::write(
        legacy.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "schemaVersion": 1,
            "package": "wxml-lsp",
            "packageVersion": LEGACY_VERSION,
            "platform": NATIVE_PACKAGE_PLATFORM
        }))
        .unwrap(),
    )
    .unwrap();
    fs::remove_file(root.path().join("state.json")).unwrap();
    write_committed_journal(
        &root,
        wxml_delivery::state::TransactionOp::Install,
        VERSION_1,
        Some(LEGACY_VERSION),
    );

    let error = recover_native(&root, &probe).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("legacy Node package as previous"),
        "{error}"
    );
    assert_eq!(active_version(&root), VERSION_1);
    assert!(InstallState::read(&root).unwrap().is_none());
    assert_eq!(
        Journal::list(&root).unwrap()[0].state,
        TransactionState::Committed
    );
}

#[test]
fn rollback_after_state_write_rolls_forward() {
    let base = TempDir::new("rollback-rollback-state-write");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let (staged, _) = stage_fixture(&root, VERSION_2);
    install_native(&root, &staged, expected(VERSION_2), &probe, &NoHooks).unwrap();
    let n2_fingerprint = tree_fingerprint(&root.versions_dir().join(VERSION_2)).unwrap();

    let error = rollback_native(
        &root,
        VERSION_1,
        &probe,
        &FailAt {
            point: FailPoint::AfterStateWrite,
        },
    )
    .unwrap_err();
    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        ..
    } = error
    else {
        panic!("expected typed rollback state, got {error:?}");
    };
    assert_eq!(version, VERSION_1);
    assert_eq!(previous.as_deref(), Some(VERSION_2));
    assert!(state_reconciled);
    assert_eq!(active_version(&root), VERSION_1);
    let state = InstallState::read(&root).unwrap().unwrap();
    assert_eq!(state.active, VERSION_1);
    assert_eq!(state.previous.as_deref(), Some(VERSION_2));
    assert!(Journal::list(&root).unwrap().is_empty());
    assert_eq!(
        tree_fingerprint(&root.versions_dir().join(VERSION_2)).unwrap(),
        n2_fingerprint
    );

    for _ in 0..2 {
        let report = recover_native(&root, &probe).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_1));
        assert_eq!(active_version(&root), VERSION_1);
        assert!(Journal::list(&root).unwrap().is_empty());
    }
}

#[test]
fn rollback_with_corrupt_active_and_good_target_succeeds() {
    let base = TempDir::new("rollback-corrupt-active");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let (staged, _) = stage_fixture(&root, VERSION_2);
    install_native(&root, &staged, expected(VERSION_2), &probe, &NoHooks).unwrap();
    let n1_fingerprint = tree_fingerprint(&root.versions_dir().join(VERSION_1)).unwrap();

    // The contract verifies the rollback target, not the outgoing active tree.
    fs::write(
        root.versions_dir()
            .join(VERSION_2)
            .join("engine/lib/lib.d.ts"),
        b"// corrupt active\n",
    )
    .unwrap();

    let outcome = rollback_native(&root, VERSION_1, &probe, &NoHooks).unwrap();
    assert_eq!(outcome, NativeInstallOutcome::RolledBack);
    assert_eq!(active_version(&root), VERSION_1);
    let state = InstallState::read(&root).unwrap().unwrap();
    assert_eq!(state.active, VERSION_1);
    assert_eq!(state.previous.as_deref(), Some(VERSION_2));
    assert_eq!(
        tree_fingerprint(&root.versions_dir().join(VERSION_1)).unwrap(),
        n1_fingerprint
    );
    assert_eq!(verify_native(&root, &probe).unwrap().version, VERSION_1);
}

#[test]
fn restore_previous_switch_failure_is_typed() {
    let base = TempDir::new("rollback-restore-switch");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();
    let n1 = root.versions_dir().join(VERSION_1);

    let (staged, _) = stage_fixture(&root, VERSION_2);
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_2),
        &probe,
        &FailThenRemove {
            fail: FailPoint::AfterSwitch,
            remove: FailPoint::BeforeRestoreSwitch,
            path: n1,
        },
    )
    .unwrap_err();
    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        recovery,
        ..
    } = error
    else {
        panic!("expected typed switch-back failure, got {error:?}");
    };
    assert_eq!(version, VERSION_2);
    assert_eq!(previous.as_deref(), Some(VERSION_1));
    assert!(!state_reconciled);
    assert!(recovery.contains("wxml-lsp-install recover"), "{recovery}");
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(
        InstallState::read(&root).unwrap().unwrap().active,
        VERSION_1
    );
    assert_eq!(
        Journal::list(&root).unwrap()[0].state,
        TransactionState::Committed
    );

    // Recovery cannot silently drop the previous native version.
    assert!(recover_native(&root, &probe).is_err());
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(
        InstallState::read(&root).unwrap().unwrap().active,
        VERSION_1
    );
}

#[derive(Debug)]
struct ChmodAt {
    point: FailPoint,
    path: PathBuf,
    mode: u32,
}

impl InstallHooks for ChmodAt {
    fn checkpoint(&self, point: FailPoint) -> wxml_delivery::Result<()> {
        if point == self.point {
            set_mode(&self.path, self.mode).unwrap();
        }
        Ok(())
    }
}

struct RestoreMode(PathBuf);

impl Drop for RestoreMode {
    fn drop(&mut self) {
        let _ = set_mode(&self.0, 0o755);
    }
}

#[test]
fn state_write_failure_is_typed_and_recoverable() {
    let base = TempDir::new("rollback-state-write-io");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let (staged, _) = stage_fixture(&root, VERSION_1);
    install_native(&root, &staged, expected(VERSION_1), &probe, &NoHooks).unwrap();

    let root_path = root.path().to_path_buf();
    let restore_mode = RestoreMode(root_path.clone());
    let (staged, _) = stage_fixture(&root, VERSION_2);
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_2),
        &probe,
        &ChmodAt {
            point: FailPoint::BeforeStateWrite,
            path: root_path,
            mode: 0o555,
        },
    )
    .unwrap_err();
    drop(restore_mode);

    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        reason,
        recovery,
    } = error
    else {
        panic!("state-write failure must be typed, got {error:?}");
    };
    assert_eq!(version, VERSION_2);
    assert_eq!(previous.as_deref(), Some(VERSION_1));
    assert!(!state_reconciled);
    assert!(reason.contains("I/O error"), "{reason}");
    assert!(recovery.contains("wxml-lsp-install recover"), "{recovery}");
    assert_eq!(active_version(&root), VERSION_2);
    assert_eq!(
        InstallState::read(&root).unwrap().unwrap().active,
        VERSION_1
    );
    assert_eq!(
        Journal::list(&root).unwrap()[0].state,
        TransactionState::Committed
    );

    for _ in 0..2 {
        let report = recover_native(&root, &probe).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_2));
        let state = InstallState::read(&root).unwrap().unwrap();
        assert_eq!(state.active, VERSION_2);
        assert_eq!(state.previous.as_deref(), Some(VERSION_1));
        assert!(Journal::list(&root).unwrap().is_empty());
    }
    assert_tmp_empty(&root);
}

#[test]
fn fresh_state_write_failure_is_typed_and_recoverable() {
    let base = TempDir::new("rollback-fresh-state-io");
    let root = InstallRoot::create(&base.path().join("root")).unwrap();
    let probe = FakeProbe::default();
    let root_path = root.path().to_path_buf();
    let restore_mode = RestoreMode(root_path.clone());
    let (staged, _) = stage_fixture(&root, VERSION_1);
    let error = install_native(
        &root,
        &staged,
        expected(VERSION_1),
        &probe,
        &ChmodAt {
            point: FailPoint::BeforeStateWrite,
            path: root_path,
            mode: 0o555,
        },
    )
    .unwrap_err();
    drop(restore_mode);

    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        reason,
        recovery,
    } = error
    else {
        panic!("fresh state-write failure must be typed, got {error:?}");
    };
    assert_eq!(version, VERSION_1);
    assert_eq!(previous, None);
    assert!(!state_reconciled);
    assert!(reason.contains("I/O error"), "{reason}");
    assert!(recovery.contains("wxml-lsp-install recover"), "{recovery}");
    assert_eq!(active_version(&root), VERSION_1);
    assert!(InstallState::read(&root).unwrap().is_none());
    assert_eq!(
        Journal::list(&root).unwrap()[0].state,
        TransactionState::Committed
    );

    for _ in 0..2 {
        let report = recover_native(&root, &probe).unwrap();
        assert_eq!(report.current.as_deref(), Some(VERSION_1));
        let state = InstallState::read(&root).unwrap().unwrap();
        assert_eq!(state.active, VERSION_1);
        assert_eq!(state.previous, None);
        assert!(Journal::list(&root).unwrap().is_empty());
    }
}
