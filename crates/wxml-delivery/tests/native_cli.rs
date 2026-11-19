//! End-to-end schema v2 installer CLI tests.
//!
//! These tests use local `--from-dir` releases only. They never download and
//! never execute Node: the fixture `engine/lib/tsc` and Rust entry points are
//! tiny shell scripts, and `PATH` contains only tripwire shims for the Node
//! command names.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::too_many_lines
)]

mod support;

#[path = "support/typings_payload.rs"]
mod typings_payload;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::json;
use support::{TempDir, assert_failure, assert_success, installer, run, stdout};
use wxml_delivery::archive::create_tar_gz;
use wxml_delivery::index_v2::{IndexV2, ReleaseV2};
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY,
    ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE,
    ENGINE_LICENSE, ENGINE_PROBE_PREFIX, ENGINE_SOURCE_COMMIT, ENGINE_VERSION, EngineInfo,
    EngineProbe, MANIFEST_SCHEMA_V2, MINIPROGRAM_TYPINGS_FILE_COUNT, MINIPROGRAM_TYPINGS_LICENSE,
    MINIPROGRAM_TYPINGS_ROOT, MINIPROGRAM_TYPINGS_VERSION, ManifestV2, NATIVE_PACKAGE_PLATFORM,
    NativeArtifactKind, NativeBinaryArtifact, STANDARD_LIB_ASSET, STANDARD_LIB_FILE_COUNT,
    STANDARD_LIB_INCLUDE, TypeAsset, compute_type_asset_bundle,
};
use wxml_delivery::sri::sha512_sri_bytes;
use wxml_delivery::state::{InstallState, Journal, TransactionOp, TransactionState};
use wxml_delivery::verify_v2::{checksums_for_tree_v2, tree_fingerprint, verify_tree_v2};

const V1: &str = "0.1.0";
const V2: &str = "0.2.0";

#[derive(Clone, Copy)]
struct Scripts<'a> {
    engine: &'a str,
    lsp: Option<&'a str>,
    installer: Option<&'a str>,
}

impl Default for Scripts<'_> {
    fn default() -> Self {
        Self {
            engine: "printf 'Version 7.1.0-beta\\n'\n",
            lsp: None,
            installer: None,
        }
    }
}

struct FileBuilder {
    root: PathBuf,
    files: Vec<FileEntry>,
}

impl FileBuilder {
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

fn build_native_tree(root: &Path, version: &str, scripts: Scripts<'_>) -> ManifestV2 {
    let mut builder = FileBuilder::new(root);
    let lsp = scripts.lsp.map_or_else(
        || format!("#!/bin/sh\nprintf 'wxml-lsp {version}\\n'\n"),
        ToOwned::to_owned,
    );
    let installer = scripts.installer.map_or_else(
        || format!("#!/bin/sh\nprintf 'wxml-lsp-install {version}\\n'\n"),
        ToOwned::to_owned,
    );
    builder.file("bin/wxml-lsp", lsp.as_bytes(), 0o755);
    builder.file("bin/wxml-lsp-install", installer.as_bytes(), 0o755);
    builder.file(
        "engine/lib/tsc",
        format!("#!/bin/sh\n{}", scripts.engine).as_bytes(),
        0o755,
    );
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
    typings_payload::write_payload(|path, bytes| builder.file(path, bytes, 0o644));
    builder.file("zed-extension/extension.wasm", b"\0asm-fixture", 0o644);
    builder.file(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\n",
        0o644,
    );
    builder.file("provenance.json", b"{\"fixture\":true}\n", 0o644);
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
    manifest
        .validate(wxml_delivery::manifest_v2::ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            protocol_version: Some(2),
        })
        .unwrap();
    atomic_write(
        &root.join("manifest.json"),
        &manifest.to_json().unwrap(),
        0o644,
    )
    .unwrap();
    let checksums = checksums_for_tree_v2(root, &manifest).unwrap();
    atomic_write(&root.join("SHA256SUMS"), &checksums, 0o644).unwrap();
    verify_tree_v2(
        root,
        wxml_delivery::manifest_v2::ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            protocol_version: Some(2),
        },
    )
    .unwrap();
    manifest
}

fn pack_native_release(base: &Path, versions: &[(&str, Scripts<'_>)]) -> PathBuf {
    let release = base.join("native-release");
    fs::create_dir_all(&release).unwrap();
    let mut index = IndexV2::empty();
    for (version, scripts) in versions {
        let tree = base.join(format!("tree-{version}"));
        let manifest = build_native_tree(&tree, version, *scripts);
        let archive_name = format!("wxml-lsp-{version}-{NATIVE_PACKAGE_PLATFORM}.tar.gz");
        let archive = release.join(&archive_name);
        let mut entries = manifest.files;
        for name in ["manifest.json", "SHA256SUMS"] {
            let path = tree.join(name);
            entries.push(FileEntry {
                path: name.to_owned(),
                sha256: sha256_file(&path).unwrap(),
                size: fs::metadata(&path).unwrap().len(),
                mode: "0644".to_owned(),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        create_tar_gz(&tree, &archive, &entries, 0).unwrap();
        let bytes = fs::read(&archive).unwrap();
        index.upsert(ReleaseV2 {
            version: (*version).to_owned(),
            platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
            protocol_version: 2,
            engine_version: ENGINE_VERSION.to_owned(),
            engine_source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
            engine_protocol_version: 2,
            engine_api_schema_version: 9,
            engine_ast_header_bytes: 64,
            engine_distribution: ENGINE_DISTRIBUTION.to_owned(),
            engine_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
            min_installer_schema: 2,
            archive: archive_name,
            archive_integrity: sha512_sri_bytes(&bytes).to_sri(),
            sha256: sha256_bytes(&bytes),
            size: u64::try_from(bytes.len()).unwrap(),
        });
    }
    atomic_write(
        &release.join("index.json"),
        &index.to_json().unwrap(),
        0o644,
    )
    .unwrap();
    release
}

fn native_install(root: &Path, release: &Path, version: &str) -> Output {
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        release.to_str().unwrap(),
        "--version",
        version,
        "--offline",
    ]);
    run(&mut command)
}

fn native_upgrade(root: &Path, release: &Path, path: &Path) -> Output {
    let mut command = installer();
    command.env("PATH", path).args([
        "upgrade",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        release.to_str().unwrap(),
        "--offline",
    ]);
    run(&mut command)
}

fn command_for(root: &Path, name: &str) -> Command {
    let mut command = installer();
    command.args([name, "--root", root.to_str().unwrap()]);
    command
}

fn active_version(root: &Path) -> String {
    let link = fs::read_link(root.join("current")).unwrap();
    link.components()
        .nth(1)
        .and_then(|component| component.as_os_str().to_str())
        .unwrap()
        .to_owned()
}

fn no_node_path(base: &Path) -> PathBuf {
    let shims = base.join("no-node-path");
    fs::create_dir_all(&shims).unwrap();
    for name in ["node", "npm", "npx", "pnpm", "yarn"] {
        let path = shims.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\ntouch '{}'\nexit 99\n",
                base.join("NODE_WAS_EXECUTED").display()
            ),
        )
        .unwrap();
        set_mode(&path, 0o755).unwrap();
    }
    shims
}

fn assert_no_node(base: &Path) {
    assert!(
        !base.join("NODE_WAS_EXECUTED").exists(),
        "a Node command was executed"
    );
}

fn assert_tmp_empty(root: &Path) {
    let tmp = root.join("tmp");
    if tmp.exists() {
        assert!(fs::read_dir(&tmp).unwrap().next().is_none());
    }
}

#[test]
fn installs_native_verify_upgrade_rollback_env_without_node() {
    let base = TempDir::new("native-cli-flow");
    let release = pack_native_release(
        base.path(),
        &[(V1, Scripts::default()), (V2, Scripts::default())],
    );
    let root = base.path().join("root");
    let shims = no_node_path(base.path());

    let mut install = installer();
    install.env("PATH", &shims).args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        release.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut install);
    assert_success(&result);
    assert!(
        stdout(&result).contains("installed 0.1.0 (native)"),
        "{}",
        stdout(&result)
    );
    assert!(stdout(&result).contains("engine: typescript 7.1.0-beta"));
    assert!(stdout(&result).contains("export WXML_LSP_BIN="));
    assert_eq!(active_version(&root), V1);
    assert!(
        InstallState::read(&wxml_delivery::install::InstallRoot::create(&root).unwrap())
            .unwrap()
            .is_some()
    );
    assert_tmp_empty(&root);
    assert_no_node(base.path());

    let mut current = command_for(&root, "current");
    current.env("PATH", &shims);
    let result = run(&mut current);
    assert_success(&result);
    assert!(stdout(&result).contains("version: 0.1.0"));
    assert!(stdout(&result).contains("engine: typescript 7.1.0-beta"));

    let mut verify = command_for(&root, "verify");
    verify.env("PATH", &shims);
    let result = run(&mut verify);
    assert_success(&result);
    assert!(stdout(&result).contains("verified 0.1.0 (native)"));
    assert!(stdout(&result).contains("engine version: 7.1.0-beta"));

    let mut env = command_for(&root, "env");
    env.env("PATH", &shims);
    let result = run(&mut env);
    assert_success(&result);
    assert!(stdout(&result).contains("export WXML_LSP_BIN="));
    assert!(
        !stdout(&result).contains("WXML_TYPE_BACKEND"),
        "native env must not export a Node backend: {}",
        stdout(&result)
    );
    assert_no_node(base.path());

    let result = native_upgrade(&root, &release, &shims);
    assert_success(&result);
    assert!(stdout(&result).contains("upgraded to 0.2.0 (native)"));
    assert_eq!(active_version(&root), V2);
    let state = InstallState::read(&wxml_delivery::install::InstallRoot::create(&root).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(state.active, V2);
    assert_eq!(state.previous.as_deref(), Some(V1));

    let mut rollback = installer();
    rollback.env("PATH", &shims).args([
        "rollback",
        "--root",
        root.to_str().unwrap(),
        "--to",
        V1,
        "--offline",
    ]);
    let result = run(&mut rollback);
    assert_success(&result);
    assert!(stdout(&result).contains("rolled back to 0.1.0"));
    assert_eq!(active_version(&root), V1);
    let state = InstallState::read(&wxml_delivery::install::InstallRoot::create(&root).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(state.previous.as_deref(), Some(V2));

    let mut versions = installer();
    versions.args([
        "versions",
        "--from-dir",
        release.to_str().unwrap(),
        "--offline",
    ]);
    let result = run(&mut versions);
    assert_success(&result);
    assert!(stdout(&result).contains("0.1.0  protocol 2  engine 7.1.0-beta"));
    assert!(stdout(&result).contains("0.2.0  protocol 2  engine 7.1.0-beta"));

    let mut verify_package = installer();
    verify_package.args([
        "verify-package",
        "--from-dir",
        release.to_str().unwrap(),
        "--version",
        V2,
        "--offline",
    ]);
    let result = run(&mut verify_package);
    assert_success(&result);
    assert!(stdout(&result).contains("verified native package wxml-lsp 0.2.0"));

    let mut list = command_for(&root, "list");
    let result = run(&mut list);
    assert_success(&result);
    assert!(stdout(&result).contains("* 0.1.0"));
    assert!(!stdout(&result).contains("legacy"));
    assert_no_node(base.path());
}

#[test]
fn exact_engine_and_rust_probes_reject_before_activation() {
    for (name, scripts) in [
        (
            "engine",
            Scripts {
                engine: "printf 'Version 7.1.0-beta-extra\\n'\n",
                ..Scripts::default()
            },
        ),
        (
            "engine-minor",
            Scripts {
                engine: "printf 'Version 7.0.2\\n'\n",
                ..Scripts::default()
            },
        ),
        (
            "lsp",
            Scripts {
                lsp: Some("#!/bin/sh\nprintf 'wxml-lsp 9.9.9\\n'\n"),
                ..Scripts::default()
            },
        ),
        (
            "installer",
            Scripts {
                installer: Some("#!/bin/sh\nprintf 'wxml-lsp-install 9.9.9\\n'\n"),
                ..Scripts::default()
            },
        ),
    ] {
        let base = TempDir::new(&format!("native-cli-probe-{name}"));
        let release = pack_native_release(base.path(), &[(V1, scripts)]);
        let root = base.path().join("root");
        let result = native_install(&root, &release, V1);
        assert_failure(&result, "runtime check failed");
        assert!(
            fs::read_link(root.join("current")).is_err(),
            "{name}: current must not exist"
        );
        assert!(!root.join("versions").exists(), "{name}");
        assert_tmp_empty(&root);
    }
}

#[test]
fn verify_detects_tamper_without_executing_it() {
    let base = TempDir::new("native-cli-tamper");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let root = base.path().join("root");
    assert_success(&native_install(&root, &release, V1));

    let marker = base.path().join("EXECUTED");
    let binary = root.join("versions").join(V1).join("bin/wxml-lsp");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\ntouch '{}'\nprintf 'wxml-lsp 0.1.0\\n'\n",
            marker.display()
        ),
    )
    .unwrap();
    set_mode(&binary, 0o755).unwrap();

    let mut verify = command_for(&root, "verify");
    let result = run(&mut verify);
    assert!(!result.status.success());
    assert!(!marker.exists(), "verify executed a tampered binary");
    assert_eq!(active_version(&root), V1);
}

#[test]
fn wxml_lsp_bin_takes_precedence_for_native_mutations() {
    let base = TempDir::new("native-cli-env");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let root = base.path().join("root");

    let explicit = base.path().join("explicit-lsp");
    fs::write(
        &explicit,
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'wxml-lsp 9.9.9\\n'; fi\n",
    )
    .unwrap();
    set_mode(&explicit, 0o755).unwrap();

    for args in [
        vec!["install", "--version", V1],
        vec!["upgrade"],
        vec!["rollback", "--to", V1],
    ] {
        let mut command = installer();
        command.env("WXML_LSP_BIN", &explicit).args(args);
        command.args(["--root", root.to_str().unwrap()]);
        let result = run(&mut command);
        assert_success(&result);
        assert!(stdout(&result).contains("takes precedence"));
        assert!(!root.exists(), "explicit binary must not create a root");
    }

    let mut current = command_for(&root, "current");
    current.env("WXML_LSP_BIN", &explicit);
    let result = run(&mut current);
    assert_success(&result);
    assert!(stdout(&result).contains("version: 9.9.9"));
    assert!(stdout(&result).contains("source: WXML_LSP_BIN"));

    let missing = base.path().join("missing-lsp");
    let mut install = installer();
    install.env("WXML_LSP_BIN", &missing).args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        release.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut install);
    assert_failure(&result, "WXML_LSP_BIN");
    assert!(!root.exists());
}

#[test]
fn recover_keeps_a_committed_target_inactive_and_cleans_staged_state() {
    let base = TempDir::new("native-cli-recover");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let root = base.path().join("root");
    assert_success(&native_install(&root, &release, V1));
    let install_root = wxml_delivery::install::InstallRoot::create(&root).unwrap();

    // Simulate a crash after `rename` but before the switch: the target tree is
    // present and the journal is committed, while `current` still points at V1.
    build_native_tree(
        &install_root.versions_dir().join(V2),
        V2,
        Scripts::default(),
    );
    let mut journal =
        Journal::new(TransactionOp::Upgrade, V2, Some(V1), "2026-11-17T00:00:00Z").unwrap();
    journal
        .transition(TransactionState::Committed, "2026-11-17T00:00:05Z")
        .unwrap();
    journal.write(&install_root).unwrap();

    let mut recover = command_for(&root, "recover");
    let result = run(&mut recover);
    assert_success(&result);
    assert!(stdout(&result).contains("kept target; not auto-activated"));
    assert_eq!(active_version(&root), V1);
    let journals = Journal::list(&install_root).unwrap();
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0].state, TransactionState::Committed);

    // A staged journal without a target tree is disposable.
    let staged_dir = install_root.new_temp_dir("staged").unwrap();
    fs::write(staged_dir.join("partial"), b"partial").unwrap();
    let staged = Journal::new(
        TransactionOp::Install,
        "0.3.0",
        None,
        "2026-11-17T00:00:00Z",
    )
    .unwrap();
    staged.write(&install_root).unwrap();
    let mut recover = command_for(&root, "recover");
    let result = run(&mut recover);
    assert_success(&result);
    assert!(stdout(&result).contains("removed temporary state"));
    assert_eq!(active_version(&root), V1);
    assert!(
        Journal::list(&install_root)
            .unwrap()
            .iter()
            .all(|journal| journal.target != "0.3.0")
    );
    assert_tmp_empty(&root);
}

#[test]
fn native_offline_without_from_dir_fails_closed() {
    let base = TempDir::new("native-cli-offline");
    let release = pack_native_release(
        base.path(),
        &[(V1, Scripts::default()), (V2, Scripts::default())],
    );
    let root = base.path().join("root");
    assert_success(&native_install(&root, &release, V1));

    let mut install = installer();
    install.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--version",
        V2,
        "--offline",
    ]);
    let result = run(&mut install);
    assert_failure(&result, "--from-dir");
    assert_eq!(active_version(&root), V1);

    let mut remote = installer();
    remote.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--source",
        "http://127.0.0.1:9/",
        "--version",
        V2,
        "--offline",
    ]);
    let result = run(&mut remote);
    assert_failure(&result, "--from-dir");
    assert_eq!(active_version(&root), V1);
}

#[test]
fn legacy_v1_is_read_only_and_migrates_without_node() {
    let base = TempDir::new("native-cli-legacy");
    let v1_release = base.path().join("v1-release");
    support::pack(&v1_release, V1);
    let v2_release = pack_native_release(base.path(), &[(V2, Scripts::default())]);
    let root = base.path().join("root");
    let shims = no_node_path(base.path());
    let poison = base.path().join("poison");
    fs::create_dir_all(&poison).unwrap();

    // Installing a legacy package must not execute Node either, even with the
    // legacy variables poisoned and PATH containing only tripwire shims.
    let mut legacy_install = installer();
    legacy_install
        .env("PATH", &shims)
        .env("WXML_NODE", poison.join("node"))
        .env("WXML_TYPE_BACKEND", poison.join("server.js"))
        .env("WXML_BACKEND_DIR", &poison)
        .args([
            "install",
            "--root",
            root.to_str().unwrap(),
            "--from-dir",
            v1_release.to_str().unwrap(),
            "--version",
            V1,
            "--offline",
        ]);
    let result = run(&mut legacy_install);
    assert_success(&result);
    assert!(stdout(&result).contains(&format!("installed {V1}")));
    assert!(!stdout(&result).contains("node:"), "{}", stdout(&result));
    assert_no_node(base.path());
    assert_eq!(active_version(&root), V1);
    let legacy_dir = root.join("versions").join(V1);
    let legacy_fingerprint = tree_fingerprint(&legacy_dir).unwrap();

    // `current`, `verify` and `env` inspect a schema v1 package read-only:
    // no legacy binary, no Node runtime, and only WXML_LSP_BIN is exported.
    let mut legacy_current = command_for(&root, "current");
    legacy_current.env("PATH", &shims);
    let result = run(&mut legacy_current);
    assert_success(&result);
    assert!(stdout(&result).contains(&format!("version: {V1}")));
    assert!(stdout(&result).contains("schema: 1 (legacy, read-only"));

    let mut legacy_verify = command_for(&root, "verify");
    legacy_verify.env("PATH", &shims);
    let result = run(&mut legacy_verify);
    assert_success(&result);
    assert!(
        stdout(&result).contains(&format!("verified {V1} (legacy, read-only)")),
        "{}",
        stdout(&result)
    );

    let mut legacy_env = command_for(&root, "env");
    legacy_env.env("PATH", &shims);
    let result = run(&mut legacy_env);
    assert_success(&result);
    assert!(stdout(&result).contains("WXML_LSP_BIN="));
    for variable in ["WXML_TYPE_BACKEND", "WXML_NODE", "WXML_BACKEND_DIR"] {
        assert!(
            !stdout(&result).contains(variable),
            "legacy env must not export {variable}: {}",
            stdout(&result)
        );
    }
    assert_no_node(base.path());
    assert_eq!(tree_fingerprint(&legacy_dir).unwrap(), legacy_fingerprint);

    let mut migrate = installer();
    migrate.env("PATH", &shims).args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        v2_release.to_str().unwrap(),
        "--version",
        V2,
        "--offline",
    ]);
    let result = run(&mut migrate);
    assert_success(&result);
    assert!(stdout(&result).contains("migrated to 0.2.0 (native)"));
    assert_eq!(active_version(&root), V2);
    assert_no_node(base.path());
    assert_eq!(tree_fingerprint(&legacy_dir).unwrap(), legacy_fingerprint);
    let state = InstallState::read(&wxml_delivery::install::InstallRoot::create(&root).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(state.active, V2);
    assert_eq!(state.previous, None, "legacy must never become previous");

    // After migration the read-only commands report the native installation
    // and never fall back to the legacy package or Node.
    let mut native_env = command_for(&root, "env");
    native_env.env("PATH", &shims);
    let result = run(&mut native_env);
    assert_success(&result);
    assert!(stdout(&result).contains("export WXML_LSP_BIN="));
    assert!(!stdout(&result).contains("WXML_TYPE_BACKEND"));
    let mut native_current = command_for(&root, "current");
    native_current.env("PATH", &shims);
    let result = run(&mut native_current);
    assert_success(&result);
    assert!(stdout(&result).contains(&format!("version: {V2}")));
    assert_no_node(base.path());

    let mut list = command_for(&root, "list");
    let result = run(&mut list);
    assert_success(&result);
    assert!(stdout(&result).contains("0.1.0 (legacy, read-only)"));
    assert!(stdout(&result).contains("* 0.2.0"));

    let mut rollback = installer();
    rollback.args(["rollback", "--root", root.to_str().unwrap(), "--to", V1]);
    let result = run(&mut rollback);
    assert_failure(&result, "legacy Node package");
    assert_eq!(active_version(&root), V2);
    assert_eq!(tree_fingerprint(&legacy_dir).unwrap(), legacy_fingerprint);

    // A schema v1 source cannot be installed over an active native root.
    let mut downgrade = installer();
    downgrade.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        v1_release.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut downgrade);
    assert_failure(&result, "refusing to install a legacy");
    assert_eq!(active_version(&root), V2);
    assert_eq!(tree_fingerprint(&legacy_dir).unwrap(), legacy_fingerprint);
}

#[test]
fn native_manifest_v1_and_v2_are_never_reinterpreted() {
    let base = TempDir::new("native-cli-schema");
    let v1_release = base.path().join("v1-release");
    support::pack(&v1_release, V1);
    let root = base.path().join("root");
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        v1_release.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert!(!stdout(&result).contains("(native)"));

    let manifest = root.join("versions").join(V1).join("manifest.json");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    assert!(value.get("engine").is_none());
    assert_eq!(
        value["node"],
        json!({"minimumVersion": "24.0.0", "backendVersion": "0.1.0"})
    );
    assert!(value["unknownLegacyField"].is_null());
}

fn rewrite_index(release: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let path = release.join("index.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    mutate(&mut value);
    atomic_write(&path, &serde_json::to_vec_pretty(&value).unwrap(), 0o644).unwrap();
}

#[test]
fn v2_archive_integrity_failures_stop_before_extraction() {
    for case in ["sri", "sha256", "size", "zero-size", "missing"] {
        let base = TempDir::new(&format!("native-cli-integrity-{case}"));
        let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
        let index_path = release.join("index.json");
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
        let archive_name = index["releases"][0]["archive"].as_str().unwrap().to_owned();
        let archive = release.join(&archive_name);
        // Deliberately invalid gzip/tar bytes: a verification failure must be
        // reported before extraction, not as an archive parse error.
        let bytes = b"not a gzip tar archive".to_vec();
        fs::write(&archive, &bytes).unwrap();
        let good_sri = sha512_sri_bytes(&bytes).to_sri();
        let good_sha = sha256_bytes(&bytes);
        let good_size = u64::try_from(bytes.len()).unwrap();
        match case {
            "sri" => rewrite_index(&release, |value| {
                value["releases"][0]["archiveIntegrity"] =
                    json!(sha512_sri_bytes(b"different archive").to_sri());
                value["releases"][0]["sha256"] = json!(good_sha);
                value["releases"][0]["size"] = json!(good_size);
            }),
            "sha256" => rewrite_index(&release, |value| {
                value["releases"][0]["archiveIntegrity"] = json!(good_sri);
                value["releases"][0]["sha256"] = json!("0".repeat(64));
                value["releases"][0]["size"] = json!(good_size);
            }),
            "size" => rewrite_index(&release, |value| {
                value["releases"][0]["archiveIntegrity"] = json!(good_sri);
                value["releases"][0]["sha256"] = json!(good_sha);
                value["releases"][0]["size"] = json!(good_size + 1);
            }),
            "zero-size" => rewrite_index(&release, |value| {
                value["releases"][0]["size"] = json!(0);
            }),
            "missing" => {
                fs::remove_file(&archive).unwrap();
            }
            _ => unreachable!(),
        }

        let root = base.path().join("root");
        let result = native_install(&root, &release, V1);
        let needle = match case {
            "sri" => "sri",
            "sha256" => "sha-256",
            "size" | "zero-size" => "size",
            "missing" => "i/o",
            _ => unreachable!(),
        };
        assert_failure(&result, needle);
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
        .to_lowercase();
        assert!(
            !combined.contains("unsafe archive"),
            "{case}: verification must fail before extraction: {combined}"
        );
        assert!(
            fs::read_link(root.join("current")).is_err(),
            "{case}: current must not be created"
        );
        assert!(
            !root.join("versions").exists(),
            "{case}: versions must not be created"
        );
        assert_tmp_empty(&root);
    }
}

#[test]
fn offline_remote_source_does_not_fall_back_to_legacy_cache() {
    let base = TempDir::new("native-cli-offline-remote");
    let v1_release = base.path().join("v1-release");
    support::pack(&v1_release, V1);
    let root = base.path().join("root");

    let mut install = installer();
    install.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        v1_release.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    assert_success(&run(&mut install));
    fs::remove_file(root.join("current")).unwrap();
    fs::remove_dir_all(root.join("versions")).unwrap();

    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--source",
        "http://127.0.0.1:9/",
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "offline");
    assert!(
        fs::read_link(root.join("current")).is_err(),
        "a cached legacy package must not be activated for an offline remote source"
    );
    assert!(
        !root.join("versions").exists(),
        "a cached legacy package must not be reinstalled"
    );
}

#[test]
fn concurrent_native_installs_are_serialized_without_losing_staging() {
    let base = TempDir::new("native-cli-concurrent");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let root = base.path().join("root");
    let shims = no_node_path(base.path());
    let mut children = Vec::new();
    for _ in 0..4 {
        let mut command = installer();
        command.env("PATH", &shims).args([
            "install",
            "--root",
            root.to_str().unwrap(),
            "--from-dir",
            release.to_str().unwrap(),
            "--version",
            V1,
            "--offline",
        ]);
        children.push(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    for child in children {
        let result = child.wait_with_output().unwrap();
        assert_success(&result);
    }

    assert_eq!(active_version(&root), V1);
    assert_tmp_empty(&root);
    let versions: Vec<_> = fs::read_dir(root.join("versions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(versions, vec![std::ffi::OsString::from(V1)]);
    let state = InstallState::read(&wxml_delivery::install::InstallRoot::create(&root).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(state.active, V1);
    assert_no_node(base.path());
}

#[test]
fn concurrent_legacy_and_native_install_cannot_wedge_state() {
    let base = TempDir::new("native-cli-race");
    let v1_release = base.path().join("v1-release");
    support::pack(&v1_release, V1);
    let v2_release = pack_native_release(base.path(), &[(V2, Scripts::default())]);
    let root = base.path().join("root");
    let install_root = wxml_delivery::install::InstallRoot::create(&root).unwrap();
    let lock = install_root.lock().unwrap();

    let mut legacy = installer();
    legacy.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        v1_release.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let mut native = installer();
    native.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        v2_release.to_str().unwrap(),
        "--version",
        V2,
        "--offline",
    ]);
    let legacy_child = legacy
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let native_child = native
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(lock);

    let legacy_output = legacy_child.wait_with_output().unwrap();
    let native_output = native_child.wait_with_output().unwrap();
    assert_success(&native_output);
    if !legacy_output.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&legacy_output.stdout),
            String::from_utf8_lossy(&legacy_output.stderr)
        )
        .to_lowercase();
        assert!(
            combined.contains("refusing to install a legacy"),
            "legacy install failed for an unexpected reason: {combined}"
        );
    }

    assert_eq!(active_version(&root), V2);
    let state = InstallState::read(&wxml_delivery::install::InstallRoot::create(&root).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(state.active, V2);
    assert_eq!(state.previous, None);
    assert_tmp_empty(&root);
    let mut verify = command_for(&root, "verify");
    assert_success(&run(&mut verify));
}

fn private_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

/// Returns `(version, entry directory)` for every native archive cache entry.
fn native_cache_entries(root: &Path) -> Vec<(String, PathBuf)> {
    let directory = root.join("cache/v2");
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(bytes) = fs::read(path.join("entry.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(version) = value["version"].as_str() {
            found.push((version.to_owned(), path));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

fn cached_archive(root: &Path, version: &str) -> PathBuf {
    native_cache_entries(root)
        .into_iter()
        .find(|(cached, _)| cached == version)
        .unwrap_or_else(|| panic!("no native archive cache entry for {version}"))
        .1
        .join("archive.tar.gz")
}

/// Copies only `index.json`; every archive is expected to come from the cache.
fn index_only_source(release: &Path, destination: &Path) -> PathBuf {
    fs::create_dir_all(destination).unwrap();
    fs::copy(release.join("index.json"), destination.join("index.json")).unwrap();
    destination.to_path_buf()
}

fn rollback_to(root: &Path, version: &str) -> Output {
    let mut command = installer();
    command.args([
        "rollback",
        "--root",
        root.to_str().unwrap(),
        "--to",
        version,
        "--offline",
    ]);
    run(&mut command)
}

fn offline_upgrade(root: &Path, shims: &Path, proxy: Option<&str>) -> Output {
    let mut command = installer();
    command
        .env("PATH", shims)
        .args(["upgrade", "--root", root.to_str().unwrap(), "--offline"]);
    if let Some(proxy) = proxy {
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(name, proxy);
        }
    }
    run(&mut command)
}

#[test]
fn offline_cli_reuses_the_verified_native_archive_cache_without_network() {
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    let base = TempDir::new("native-cli-cache-reuse");
    let release = pack_native_release(
        base.path(),
        &[(V1, Scripts::default()), (V2, Scripts::default())],
    );
    let root = base.path().join("root");
    let shims = no_node_path(base.path());

    assert_success(&native_install(&root, &release, V1));
    assert_success(&native_upgrade(&root, &release, &shims));
    assert_success(&rollback_to(&root, V1));
    assert_eq!(active_version(&root), V1);

    let entries = native_cache_entries(&root);
    assert_eq!(entries.len(), 2, "{entries:?}");
    for (version, directory) in &entries {
        assert_eq!(
            private_mode(directory),
            0o700,
            "{version} cache entry must be private"
        );
        assert_eq!(private_mode(&directory.join("archive.tar.gz")), 0o600);
        assert_eq!(private_mode(&directory.join("entry.json")), 0o644);
    }
    assert_eq!(private_mode(&root.join("cache/v2")), 0o700);
    let cached_before: Vec<(String, Vec<u8>)> = entries
        .iter()
        .map(|(version, directory)| {
            (
                version.clone(),
                fs::read(directory.join("archive.tar.gz")).unwrap(),
            )
        })
        .collect();

    // A proxy sink: only a TCP connection would increment this counter.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let connections = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let sink = {
        let connections = Arc::clone(&connections);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok(_) => {
                        connections.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        })
    };

    // The whole source directory is gone: the index and both archives come
    // from the private cache.
    fs::remove_dir_all(&release).unwrap();

    let upgrade = offline_upgrade(&root, &shims, Some(&proxy));
    assert_success(&upgrade);
    assert!(
        stdout(&upgrade).contains("upgraded to 0.2.0 (native)"),
        "{}",
        stdout(&upgrade)
    );
    assert_eq!(active_version(&root), V2);
    let rollback = rollback_to(&root, V1);
    assert_success(&rollback);
    assert_eq!(active_version(&root), V1);

    // `--offline` with a remote source is rejected before any client exists.
    let mut remote = installer();
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        remote.env(name, &proxy);
    }
    remote.args([
        "upgrade",
        "--root",
        root.to_str().unwrap(),
        "--source",
        "http://127.0.0.1:9/",
        "--offline",
    ]);
    let remote = run(&mut remote);
    assert_failure(&remote, "offline");

    stop.store(true, Ordering::Relaxed);
    sink.join().unwrap();
    assert_eq!(
        connections.load(Ordering::Relaxed),
        0,
        "offline cache installs must never open a network connection"
    );

    // Cached archives are reused byte for byte and never rewritten.
    for (version, bytes) in cached_before {
        assert_eq!(
            fs::read(cached_archive(&root, &version)).unwrap(),
            bytes,
            "{version} cache archive changed"
        );
    }
    let combined = format!("{}{}", stdout(&upgrade), stdout(&rollback));
    assert!(
        !combined.contains(root.join("cache").to_string_lossy().as_ref()),
        "CLI output must not print absolute cache paths: {combined}"
    );
    assert_no_node(base.path());
}

#[test]
fn offline_cli_cold_archive_cache_fails_closed() {
    let base = TempDir::new("native-cli-cache-cold");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let index_only = index_only_source(&release, &base.path().join("index-only"));
    let root = base.path().join("root");

    let mut install = installer();
    install.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        index_only.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut install);
    assert_failure(&result, "verified cache");
    assert_failure(&result, "--from-dir");
    assert!(!root.join("current").exists());
    assert_eq!(native_cache_entries(&root).len(), 0);
    if root.join("versions").exists() {
        assert_eq!(fs::read_dir(root.join("versions")).unwrap().count(), 0);
    }

    // With no source at all the cached index may exist, but the archive miss is
    // still fail-closed and current is never created.
    let mut install = installer();
    install.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut install);
    assert_failure(&result, "--from-dir");
    assert!(!root.join("current").exists());
    assert_eq!(native_cache_entries(&root).len(), 0);
}

#[test]
fn offline_cli_rejects_tampered_native_archive_cache_without_changing_current() {
    let base = TempDir::new("native-cli-cache-tamper");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let index_only = index_only_source(&release, &base.path().join("index-only"));
    let root = base.path().join("root");
    assert_success(&native_install(&root, &release, V1));
    let version_dir = root.join("versions").join(V1);
    let fingerprint = tree_fingerprint(&version_dir).unwrap();

    let archive = cached_archive(&root, V1);
    let original = fs::read(&archive).unwrap();

    let mut tampered = original.clone();
    tampered[0] ^= 0xff;
    fs::write(&archive, &tampered).unwrap();
    let mut install = installer();
    install.args([
        "install",
        "--root",
        root.to_str().unwrap(),
        "--from-dir",
        index_only.to_str().unwrap(),
        "--version",
        V1,
        "--offline",
    ]);
    let result = run(&mut install);
    assert_failure(&result, "sri");
    assert_eq!(active_version(&root), V1);
    assert_eq!(tree_fingerprint(&version_dir).unwrap(), fingerprint);
    assert_eq!(fs::read(&archive).unwrap(), tampered);

    fs::write(&archive, &original).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&archive).unwrap();
    file.set_len(u64::try_from(original.len()).unwrap() - 1)
        .unwrap();
    drop(file);
    let result = run(&mut install);
    assert_failure(&result, "size");
    assert_eq!(active_version(&root), V1);
    assert_eq!(tree_fingerprint(&version_dir).unwrap(), fingerprint);

    fs::write(&archive, &original).unwrap();
    let entry = archive.parent().unwrap().join("entry.json");
    let entry_original = fs::read(&entry).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&entry_original).unwrap();
    value["sha256"] = serde_json::json!("0".repeat(64));
    fs::write(&entry, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let result = run(&mut install);
    assert_failure(&result, "does not match");
    assert_eq!(active_version(&root), V1);
    assert_eq!(tree_fingerprint(&version_dir).unwrap(), fingerprint);

    fs::write(&entry, &entry_original).unwrap();
    fs::remove_file(&archive).unwrap();
    let result = run(&mut install);
    assert_failure(&result, "regular archive");
    assert_eq!(active_version(&root), V1);
    assert_eq!(tree_fingerprint(&version_dir).unwrap(), fingerprint);
    assert_eq!(fs::read_dir(root.join("versions")).unwrap().count(), 1);
    assert_tmp_empty(&root);
}

#[test]
fn offline_cli_rejects_a_tampered_cached_index_and_insecure_cache() {
    use std::os::unix::fs::PermissionsExt;

    let base = TempDir::new("native-cli-cache-index");
    let release = pack_native_release(base.path(), &[(V1, Scripts::default())]);
    let root = base.path().join("root");
    assert_success(&native_install(&root, &release, V1));
    let version_dir = root.join("versions").join(V1);
    let fingerprint = tree_fingerprint(&version_dir).unwrap();
    fs::remove_dir_all(&release).unwrap();

    let index = root.join("cache/v2/index.json");
    let original = fs::read(&index).unwrap();
    fs::write(&index, b"{}\n").unwrap();
    let mut upgrade = installer();
    upgrade.args(["upgrade", "--root", root.to_str().unwrap(), "--offline"]);
    let result = run(&mut upgrade);
    assert_failure(&result, "index");
    assert_eq!(active_version(&root), V1);
    assert_eq!(tree_fingerprint(&version_dir).unwrap(), fingerprint);
    fs::write(&index, &original).unwrap();

    let cache = root.join("cache/v2");
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();
    let result = run(&mut upgrade);
    assert_failure(&result, "private");
    assert_eq!(active_version(&root), V1);
    assert_eq!(tree_fingerprint(&version_dir).unwrap(), fingerprint);
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();

    // Restoring the mode makes the same command succeed again.
    let result = run(&mut upgrade);
    assert_success(&result);
    assert_eq!(active_version(&root), V1);
}
