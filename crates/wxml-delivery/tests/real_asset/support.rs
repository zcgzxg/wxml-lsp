//! Shared fixtures and process helpers for the real-asset acceptance harness.
//!
//! This module intentionally contains no `#[test]` functions. It is compiled as
//! a submodule of `tests/real_asset.rs`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    unreachable_pub
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wxml_delivery::archive::{ExtractReport, create_tar_gz, extract_tar_gz};
use wxml_delivery::index_v2::{IndexV2, ReleaseV2};
use wxml_delivery::install::InstallRoot;
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_BINARY_PATH, ENGINE_DISTRIBUTION,
    ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_DISTRIBUTION_SHA256,
    ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE, ENGINE_DISTRIBUTION_URL,
    ENGINE_LICENSE, ENGINE_NAME, ENGINE_PROBE_PREFIX, ENGINE_SOURCE_COMMIT, ENGINE_VERSION,
    EngineInfo, EngineProbe, ExpectedManifestV2, MANIFEST_SCHEMA_V2, MINIPROGRAM_TYPINGS_ASSET,
    MINIPROGRAM_TYPINGS_FILE_COUNT, MINIPROGRAM_TYPINGS_LICENSE, MINIPROGRAM_TYPINGS_ROOT,
    MINIPROGRAM_TYPINGS_VERSION, ManifestV2, NATIVE_PACKAGE_PLATFORM, NativeArtifactKind,
    NativeBinaryArtifact, STANDARD_LIB_ASSET, STANDARD_LIB_FILE_COUNT, STANDARD_LIB_INCLUDE,
    TypeAsset, compute_type_asset_bundle,
};
use wxml_delivery::state::{InstallState, journal_dir, state_path};
use wxml_delivery::verify_v2::{TreeContents, checksums_for_tree_v2, collect_tree, verify_tree_v2};

pub const TARBALL_ENV: &str = "TSGO_NATIVE_TARBALL";
pub const PACKAGE_VERSION: &str = "0.1.0";
pub const PROTOCOL_VERSION: u64 = 2;
/// Upper bound for one installer process. A hang fails the harness instead of
/// blocking CI forever.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Upper bound for the native engine `--version` probe.
pub const ENGINE_TIMEOUT: Duration = Duration::from_secs(10);

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A private test directory below `/tmp/opencode` (never the project tree).
#[derive(Debug)]
pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub fn new(label: &str) -> Self {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        assert!(
            base.is_absolute(),
            "WXML_TEST_TMPDIR must be an absolute path, got {}",
            base.display()
        );
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repository = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("crate is below the repository root");
        assert!(
            !base.starts_with(repository),
            "WXML_TEST_TMPDIR must be outside the repository ({})",
            repository.display()
        );
        fs::create_dir_all(&base)
            .unwrap_or_else(|error| panic!("create test base {}: {error}", base.display()));
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = base.join(format!(
            "wxml-real-asset-{label}-{}-{}-{nonce}",
            std::process::id(),
            DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)
            .unwrap_or_else(|error| panic!("create test directory {}: {error}", path.display()));
        set_mode(&path, 0o700).expect("make test directory private");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
pub fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("metadata for {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

#[cfg(not(unix))]
pub fn mode_of(_path: &Path) -> u32 {
    0o700
}

pub fn required_mode() -> bool {
    std::env::var("WXML_REQUIRE_REAL_ENGINE")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Returns the pinned tarball, or skips loudly unless the hard gate is on.
pub fn pinned_tarball_or_skip(test: &str) -> Option<PathBuf> {
    match std::env::var_os(TARBALL_ENV) {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(value);
            let canonical = fs::canonicalize(&path).unwrap_or_else(|error| {
                panic!(
                    "{TARBALL_ENV} cannot be resolved ({}): {error}",
                    path.display()
                )
            });
            let metadata = fs::metadata(&canonical).unwrap_or_else(|error| {
                panic!(
                    "{TARBALL_ENV} cannot be inspected ({}): {error}",
                    canonical.display()
                )
            });
            assert!(
                metadata.is_file(),
                "{TARBALL_ENV} does not resolve to a regular file: {}",
                canonical.display()
            );
            Some(canonical)
        }
        _ => {
            assert!(
                !required_mode(),
                "{TARBALL_ENV} is required when WXML_REQUIRE_REAL_ENGINE=1; point it at the \
                 pinned 7.1.0-beta @typescript/typescript-linux-x64 tarball \
                 (sha256 {ENGINE_DISTRIBUTION_SHA256}, {ENGINE_DISTRIBUTION_SIZE} bytes)"
            );
            eprintln!(
                "{test} skipped: set {TARBALL_ENV} to the pinned 7.1.0-beta \
                 @typescript/typescript-linux-x64 tarball \
                 (sha256 {ENGINE_DISTRIBUTION_SHA256}, {ENGINE_DISTRIBUTION_SIZE} bytes); \
                 WXML_REQUIRE_REAL_ENGINE=1 makes a missing asset a hard failure"
            );
            None
        }
    }
}

pub fn assert_supported_host() {
    let host = wxml_delivery::paths::host_platform().expect("host platform");
    assert_eq!(
        host, NATIVE_PACKAGE_PLATFORM,
        "the pinned tsgo asset and the v2 installer only support {NATIVE_PACKAGE_PLATFORM}"
    );
}

/// Verifies the asset before creating the extraction destination.
pub fn stage_verified_asset(
    tarball: &Path,
    destination: &Path,
    integrity: &str,
    sha256: &str,
    size: u64,
) -> wxml_delivery::Result<ExtractReport> {
    wxml_delivery::sri::verify_asset_file(tarball, integrity, sha256, size)?;
    extract_tar_gz(tarball, destination)
}

/// Facts observed in the extracted upstream package.
#[derive(Debug)]
pub struct UpstreamAudit {
    pub file_count: u64,
    pub unpacked_size: u64,
    pub engine_sha256: String,
    pub engine_size: u64,
}

/// Audits the extracted tarball against the frozen manifest facts.
pub fn audit_upstream_package(package: &Path) -> UpstreamAudit {
    let contents = collect_tree(package).expect("collect upstream package tree");
    let unpacked_size = audited_unpacked_size(package, &contents);
    audit_file_set(&contents);
    audit_required_entries(&contents);
    audit_package_json(package);
    let (engine_sha256, engine_size) = audit_engine(package);
    UpstreamAudit {
        file_count: u64::try_from(contents.files.len()).expect("upstream file count"),
        unpacked_size,
        engine_sha256,
        engine_size,
    }
}

fn audited_unpacked_size(package: &Path, contents: &TreeContents) -> u64 {
    let mut unpacked_size = 0_u64;
    for relative in &contents.files {
        let path = package.join(relative);
        let metadata = fs::symlink_metadata(&path)
            .unwrap_or_else(|error| panic!("metadata for {}: {error}", path.display()));
        assert!(
            metadata.file_type().is_file(),
            "pinned tarball entry `{relative}` is not a regular file"
        );
        unpacked_size = unpacked_size
            .checked_add(metadata.len())
            .expect("upstream size overflow");
    }
    assert_eq!(
        unpacked_size, ENGINE_DISTRIBUTION_UNPACKED_SIZE,
        "pinned tarball unpacked size changed"
    );
    unpacked_size
}

fn audit_file_set(contents: &TreeContents) {
    let file_count = u64::try_from(contents.files.len()).expect("upstream file count");
    assert_eq!(
        file_count, ENGINE_DISTRIBUTION_FILE_COUNT,
        "pinned tarball file count changed"
    );
    assert_eq!(
        contents.directories,
        BTreeSet::from(["lib".to_owned()]),
        "pinned tarball has unexpected directories"
    );
}

fn audit_required_entries(contents: &TreeContents) {
    for relative in [
        "LICENSE",
        "NOTICE.txt",
        "README.md",
        "package.json",
        "lib/tsc",
        "lib/tsc.sig",
    ] {
        assert!(
            contents.files.contains(relative),
            "pinned tarball is missing `{relative}`"
        );
    }
    let standard_lib_count = contents
        .files
        .iter()
        .filter(|relative| relative.starts_with("lib/") && relative.ends_with(".d.ts"))
        .count();
    assert_eq!(
        u64::try_from(standard_lib_count).expect("standard lib count"),
        STANDARD_LIB_FILE_COUNT,
        "pinned tarball standard-library file count changed"
    );
}

fn audit_package_json(package: &Path) {
    let package_json: Value = serde_json::from_slice(
        &fs::read(package.join("package.json")).expect("read upstream package.json"),
    )
    .expect("parse upstream package.json");
    assert_eq!(package_json["name"].as_str(), Some(ENGINE_DISTRIBUTION));
    assert_eq!(package_json["version"].as_str(), Some(ENGINE_VERSION));
    assert_eq!(package_json["license"].as_str(), Some(ENGINE_LICENSE));
    assert_eq!(package_json["gitHead"].as_str(), Some(ENGINE_SOURCE_COMMIT));
}

fn audit_engine(package: &Path) -> (String, u64) {
    let engine_path = package.join("lib/tsc");
    assert_ne!(
        mode_of(&engine_path) & 0o111,
        0,
        "pinned lib/tsc is not executable"
    );
    let engine_size = fs::metadata(&engine_path)
        .expect("pinned lib/tsc metadata")
        .len();
    let engine_sha256 = sha256_file(&engine_path).expect("hash pinned lib/tsc");
    (engine_sha256, engine_size)
}

struct TreeBuilder {
    root: PathBuf,
    files: Vec<FileEntry>,
}

impl TreeBuilder {
    pub fn new(root: &Path) -> Self {
        fs::create_dir_all(root).expect("create package tree");
        Self {
            root: root.to_path_buf(),
            files: Vec::new(),
        }
    }

    pub fn file(&mut self, path: &str, bytes: &[u8], mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().expect("file parent")).expect("create file parent");
        fs::write(&destination, bytes).expect("write payload file");
        set_mode(&destination, mode).expect("set payload mode");
        self.files.push(FileEntry {
            path: path.to_owned(),
            sha256: sha256_bytes(bytes),
            size: u64::try_from(bytes.len()).expect("payload size"),
            mode: format!("{mode:04o}"),
        });
    }

    pub fn copy_file(&mut self, source: &Path, path: &str, mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().expect("file parent")).expect("create file parent");
        fs::copy(source, &destination).unwrap_or_else(|error| {
            panic!(
                "copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        });
        set_mode(&destination, mode).expect("set payload mode");
        self.files.push(FileEntry {
            path: path.to_owned(),
            sha256: sha256_file(&destination).expect("hash payload file"),
            size: fs::metadata(&destination).expect("payload metadata").len(),
            mode: format!("{mode:04o}"),
        });
    }

    pub fn hash(&self, path: &str) -> String {
        self.files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("payload `{path}` is not registered"))
            .sha256
            .clone()
    }

    pub fn size(&self, path: &str) -> u64 {
        self.files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("payload `{path}` is not registered"))
            .size
    }
}

/// The generated package tree and the exact provenance bytes inside it.
#[derive(Debug)]
pub struct Fixture {
    pub manifest: ManifestV2,
    pub provenance: Vec<u8>,
}

const fn expected(version: &str) -> ExpectedManifestV2<'_> {
    ExpectedManifestV2 {
        version: Some(version),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        protocol_version: Some(PROTOCOL_VERSION),
    }
}

/// Builds a complete schema v2 package around the real upstream engine.
///
/// `bin/`, `typings/` and `zed-extension/` are test fixtures; only `engine/`
/// comes from the pinned tarball. The provenance file records that fact.
pub fn build_v2_tree(tree: &Path, upstream: &Path) -> Fixture {
    let mut builder = TreeBuilder::new(tree);
    copy_engine_payload(&mut builder, upstream);
    let provenance = provenance_bytes(&builder);
    builder.file("provenance.json", &provenance, 0o644);
    write_fixture_entry_points(&mut builder);
    write_synthetic_typings(&mut builder);
    write_zed_and_licenses(&mut builder);
    let manifest = manifest_for(&builder);
    write_manifest_and_checksums(tree, &manifest);
    Fixture {
        manifest,
        provenance,
    }
}

fn copy_engine_payload(builder: &mut TreeBuilder, upstream: &Path) {
    let mut lib_entries: Vec<_> = fs::read_dir(upstream.join("lib"))
        .expect("read upstream lib")
        .map(|entry| entry.expect("upstream lib entry").file_name())
        .collect();
    lib_entries.sort();
    for name in &lib_entries {
        let name = name.to_str().expect("upstream lib name is UTF-8");
        let mode = if name == "tsc" { 0o755 } else { 0o644 };
        builder.copy_file(
            &upstream.join("lib").join(name),
            &format!("engine/lib/{name}"),
            mode,
        );
    }
    for name in ["LICENSE", "NOTICE.txt", "package.json", "README.md"] {
        builder.copy_file(&upstream.join(name), &format!("engine/{name}"), 0o644);
    }
    let copied = builder
        .files
        .iter()
        .filter(|file| file.path.starts_with("engine/"))
        .count();
    assert_eq!(
        u64::try_from(copied).expect("engine file count"),
        ENGINE_DISTRIBUTION_FILE_COUNT,
        "not every upstream file was copied into engine/"
    );
}

fn engine_identity_json(builder: &TreeBuilder) -> Value {
    json!({
        "name": ENGINE_NAME,
        "version": ENGINE_VERSION,
        "sourceCommit": ENGINE_SOURCE_COMMIT,
        "distribution": ENGINE_DISTRIBUTION,
        "distributionUrl": ENGINE_DISTRIBUTION_URL,
        "distributionIntegrity": ENGINE_DISTRIBUTION_INTEGRITY,
        "distributionSha256": ENGINE_DISTRIBUTION_SHA256,
        "distributionSize": ENGINE_DISTRIBUTION_SIZE,
        "distributionFileCount": ENGINE_DISTRIBUTION_FILE_COUNT,
        "distributionUnpackedSize": ENGINE_DISTRIBUTION_UNPACKED_SIZE,
        "license": ENGINE_LICENSE,
        "binary": ENGINE_BINARY_PATH,
        "binarySha256": builder.hash(ENGINE_BINARY_PATH),
        "binarySize": builder.size(ENGINE_BINARY_PATH),
        "probe": format!("{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}"),
    })
}

fn provenance_bytes(builder: &TreeBuilder) -> Vec<u8> {
    let engine_files: Vec<Value> = builder
        .files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "sha256": file.sha256,
                "size": file.size,
                "mode": file.mode,
            })
        })
        .collect();
    let provenance = json!({
        "package": "wxml-lsp",
        "packageVersion": PACKAGE_VERSION,
        "kind": "test-harness-real-engine",
        "note": "Synthetic schema v2 package built by crates/wxml-delivery/tests/real_asset.rs; \
                 only engine/ comes from the pinned upstream tarball, bin/ and typings/ are test \
                 fixtures, and this is not a production release.",
        "engine": engine_identity_json(builder),
        "engineFiles": engine_files,
    });
    let mut bytes = serde_json::to_vec_pretty(&provenance).expect("serialize provenance.json");
    bytes.push(b'\n');
    bytes
}

fn write_fixture_entry_points(builder: &mut TreeBuilder) {
    // Test-only Rust entry points. The real wxml-lsp release binaries are not
    // part of this harness, so these print the manifest version and do nothing.
    builder.file(
        "bin/wxml-lsp",
        format!("#!/bin/sh\nprintf 'wxml-lsp {PACKAGE_VERSION}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file(
        "bin/wxml-lsp-install",
        format!("#!/bin/sh\nprintf 'wxml-lsp-install {PACKAGE_VERSION}\\n'\n").as_bytes(),
        0o755,
    );
}

fn write_synthetic_typings(builder: &mut TreeBuilder) {
    builder.file(
        "typings/miniprogram-api-typings/index.d.ts",
        b"// synthetic miniprogram-api-typings fixture\n",
        0o644,
    );
    builder.file(
        "typings/miniprogram-api-typings/LICENSE",
        b"MIT synthetic fixture\n",
        0o644,
    );
    for index in 0..24 {
        builder.file(
            &format!("typings/miniprogram-api-typings/types/t{index:02}.d.ts"),
            format!("// synthetic typings fixture {index}\n").as_bytes(),
            0o644,
        );
    }
}

fn write_zed_and_licenses(builder: &mut TreeBuilder) {
    builder.file("zed-extension/extension.wasm", b"\0asm-synthetic", 0o644);
    builder.file(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\nversion = \"0.0.1\"\n",
        0o644,
    );
    builder.file(
        "THIRD_PARTY_LICENSES.md",
        b"# Third-party licenses (test fixture)\n\n\
          - TypeScript native engine: Apache-2.0; see `engine/LICENSE` and `engine/NOTICE.txt`.\n\
          - miniprogram-api-typings: MIT; the files under `typings/miniprogram-api-typings/`\n\
            are synthetic fixtures, not the published package.\n",
        0o644,
    );
}

fn engine_info() -> EngineInfo {
    EngineInfo {
        name: ENGINE_NAME.to_owned(),
        version: ENGINE_VERSION.to_owned(),
        source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        distribution: ENGINE_DISTRIBUTION.to_owned(),
        distribution_url: ENGINE_DISTRIBUTION_URL.to_owned(),
        distribution_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        distribution_sha256: ENGINE_DISTRIBUTION_SHA256.to_owned(),
        distribution_size: ENGINE_DISTRIBUTION_SIZE,
        distribution_file_count: ENGINE_DISTRIBUTION_FILE_COUNT,
        distribution_unpacked_size: ENGINE_DISTRIBUTION_UNPACKED_SIZE,
        api_schema_version: ENGINE_API_SCHEMA_VERSION,
        ast_header_bytes: ENGINE_AST_HEADER_BYTES,
        license: ENGINE_LICENSE.to_owned(),
        license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
        binary: ENGINE_BINARY_PATH.to_owned(),
        probe: EngineProbe {
            args: vec!["--version".to_owned()],
            expect_prefix: ENGINE_PROBE_PREFIX.to_owned(),
        },
    }
}

fn type_assets(builder: &TreeBuilder) -> Vec<TypeAsset> {
    let standard = compute_type_asset_bundle(
        &builder.files,
        "engine/lib",
        &[STANDARD_LIB_INCLUDE.to_owned()],
    )
    .expect("compute standard-lib bundle");
    assert_eq!(standard.file_count, STANDARD_LIB_FILE_COUNT);
    let typings =
        compute_type_asset_bundle(&builder.files, MINIPROGRAM_TYPINGS_ROOT, &["**".to_owned()])
            .expect("compute typings bundle");
    assert_eq!(typings.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);
    vec![
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
            name: MINIPROGRAM_TYPINGS_ASSET.to_owned(),
            version: MINIPROGRAM_TYPINGS_VERSION.to_owned(),
            license: MINIPROGRAM_TYPINGS_LICENSE.to_owned(),
            root: MINIPROGRAM_TYPINGS_ROOT.to_owned(),
            include: vec!["**".to_owned()],
            file_count: typings.file_count,
            sha256: typings.sha256,
            license_files: vec![format!("{MINIPROGRAM_TYPINGS_ROOT}/LICENSE")],
        },
    ]
}

fn binaries(builder: &TreeBuilder) -> Vec<NativeBinaryArtifact> {
    vec![
        NativeBinaryArtifact {
            kind: NativeArtifactKind::Rust,
            name: "wxml-lsp".to_owned(),
            path: "bin/wxml-lsp".to_owned(),
            version: PACKAGE_VERSION.to_owned(),
            protocol_version: None,
            sha256: builder.hash("bin/wxml-lsp"),
        },
        NativeBinaryArtifact {
            kind: NativeArtifactKind::Rust,
            name: "wxml-lsp-install".to_owned(),
            path: "bin/wxml-lsp-install".to_owned(),
            version: PACKAGE_VERSION.to_owned(),
            protocol_version: None,
            sha256: builder.hash("bin/wxml-lsp-install"),
        },
        NativeBinaryArtifact {
            kind: NativeArtifactKind::NativeEngine,
            name: "tsgo".to_owned(),
            path: ENGINE_BINARY_PATH.to_owned(),
            version: ENGINE_VERSION.to_owned(),
            protocol_version: Some(PROTOCOL_VERSION),
            sha256: builder.hash(ENGINE_BINARY_PATH),
        },
        NativeBinaryArtifact {
            kind: NativeArtifactKind::ZedExtension,
            name: "zed-extension".to_owned(),
            path: "zed-extension/extension.wasm".to_owned(),
            version: "0.0.1".to_owned(),
            protocol_version: None,
            sha256: builder.hash("zed-extension/extension.wasm"),
        },
    ]
}

fn manifest_for(builder: &TreeBuilder) -> ManifestV2 {
    ManifestV2 {
        schema_version: MANIFEST_SCHEMA_V2,
        package: "wxml-lsp".to_owned(),
        package_version: PACKAGE_VERSION.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        built_at: "2026-11-19T00:00:00Z".to_owned(),
        engine: engine_info(),
        type_assets: type_assets(builder),
        binaries: binaries(builder),
        files: builder.files.clone(),
    }
}

fn write_manifest_and_checksums(tree: &Path, manifest: &ManifestV2) {
    manifest
        .validate(expected(PACKAGE_VERSION))
        .expect("fixture manifest must validate");
    atomic_write(
        &tree.join("manifest.json"),
        &manifest.to_json().expect("serialize manifest"),
        0o644,
    )
    .expect("write manifest");
    let checksums = checksums_for_tree_v2(tree, manifest).expect("compute tree checksums");
    atomic_write(&tree.join("SHA256SUMS"), &checksums, 0o644).expect("write checksums");
    verify_tree_v2(tree, expected(PACKAGE_VERSION)).expect("fixture tree must verify");
}

/// Packs the already-verified tree into a local `--from-dir` release.
pub fn pack_release(base: &TestDir, tree: &Path, manifest: &ManifestV2) -> PathBuf {
    let release = base.path().join("release");
    fs::create_dir_all(&release).expect("create release dir");
    let archive_name = format!("wxml-lsp-{PACKAGE_VERSION}-{NATIVE_PACKAGE_PLATFORM}.tar.gz");
    let archive = release.join(&archive_name);

    let mut entries = manifest.files.clone();
    for name in ["manifest.json", "SHA256SUMS"] {
        let path = tree.join(name);
        entries.push(FileEntry {
            path: name.to_owned(),
            sha256: sha256_file(&path).expect("hash archive control file"),
            size: fs::metadata(&path)
                .expect("archive control file metadata")
                .len(),
            mode: "0644".to_owned(),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    create_tar_gz(tree, &archive, &entries, 0).expect("create release archive");
    write_release_index(
        &release,
        &archive_name,
        &fs::read(&archive).expect("read archive"),
    );
    release
}

fn write_release_index(release: &Path, archive_name: &str, bytes: &[u8]) {
    let mut index = IndexV2::empty();
    index.upsert(ReleaseV2 {
        version: PACKAGE_VERSION.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        engine_version: ENGINE_VERSION.to_owned(),
        engine_source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        engine_protocol_version: PROTOCOL_VERSION,
        engine_api_schema_version: ENGINE_API_SCHEMA_VERSION,
        engine_ast_header_bytes: ENGINE_AST_HEADER_BYTES,
        engine_distribution: ENGINE_DISTRIBUTION.to_owned(),
        engine_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        min_installer_schema: 2,
        archive: archive_name.to_owned(),
        archive_integrity: wxml_delivery::sri::sha512_sri_bytes(bytes).to_sri(),
        sha256: sha256_bytes(bytes),
        size: u64::try_from(bytes.len()).expect("release archive size"),
    });
    atomic_write(
        &release.join("index.json"),
        &index.to_json().expect("serialize index"),
        0o644,
    )
    .expect("write index");
}

/// A verified real-engine release plus the facts observed from the tarball.
#[derive(Debug)]
pub struct PreparedRelease {
    pub fixture: Fixture,
    pub release: PathBuf,
    pub audit: UpstreamAudit,
    pub extracted_files: u64,
    pub extracted_bytes: u64,
}

pub fn prepare_release(base: &TestDir, tarball: &Path) -> PreparedRelease {
    // TOCTOU hardening: the install path verifies and extracts one private copy,
    // never the caller-provided path. Provenance still records the pinned bytes.
    let private_tarball = base.path().join("pinned-engine.tgz");
    fs::copy(tarball, &private_tarball).unwrap_or_else(|error| {
        panic!(
            "copy pinned tarball {} to {}: {error}",
            tarball.display(),
            private_tarball.display()
        )
    });
    set_mode(&private_tarball, 0o600).expect("make private tarball copy private");
    assert_eq!(
        sha256_file(&private_tarball).expect("hash private tarball copy"),
        ENGINE_DISTRIBUTION_SHA256,
        "private tarball copy does not match the pinned SHA-256"
    );
    let upstream = base.path().join("upstream");
    let report = stage_verified_asset(
        &private_tarball,
        &upstream,
        ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_DISTRIBUTION_SHA256,
        ENGINE_DISTRIBUTION_SIZE,
    )
    .expect("pinned tarball must verify before extraction");
    let extracted_files = u64::try_from(report.files.len()).expect("extracted file count");
    let extracted_bytes = report.total_bytes;
    assert_eq!(extracted_files, ENGINE_DISTRIBUTION_FILE_COUNT);
    assert_eq!(extracted_bytes, ENGINE_DISTRIBUTION_UNPACKED_SIZE);

    let audit = audit_upstream_package(&upstream.join("package"));
    let tree = base.path().join("package-tree");
    let fixture = build_v2_tree(&tree, &upstream.join("package"));
    let release = pack_release(base, &tree, &fixture.manifest);
    PreparedRelease {
        fixture,
        release,
        audit,
        extracted_files,
        extracted_bytes,
    }
}

pub fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Tripwire executables for Node-style commands. They use only shell builtins.
#[derive(Debug)]
pub struct Tripwires {
    path: PathBuf,
    marker: PathBuf,
}

impl Tripwires {
    pub fn install(base: &Path) -> Self {
        let path = base.join("node-shims");
        fs::create_dir_all(&path).expect("create shim dir");
        set_mode(&path, 0o700).expect("make shim dir private");
        let marker = base.join("NODE_WAS_EXECUTED");
        for name in ["node", "nodejs", "npm", "npx", "pnpm", "yarn", "bun"] {
            let shim = path.join(name);
            fs::write(
                &shim,
                format!("#!/bin/sh\n: > {}\nexit 99\n", shell_quote(&marker)),
            )
            .expect("write shim");
            set_mode(&shim, 0o755).expect("make shim executable");
        }
        Self { path, marker }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn marker(&self) -> &Path {
        &self.marker
    }

    pub fn clear_marker(&self) {
        match fs::remove_file(&self.marker) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove tripwire marker {}: {error}", self.marker.display()),
        }
    }

    pub fn assert_not_executed(&self) {
        assert!(
            !self.marker.exists(),
            "a Node command was executed; tripwire marker: {}",
            self.marker.display()
        );
    }
}

/// Installer command with an environment that cannot find a real Node runtime.
pub fn installer_command(tripwires: &Tripwires, base: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-install"));
    command
        .env_clear()
        .env("PATH", &tripwires.path)
        .env("HOME", base)
        .env("XDG_DATA_HOME", base)
        .env("TMPDIR", base)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .env("WXML_NODE", tripwires.path.join("node"))
        .env("NODE", tripwires.path.join("node"))
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .stdin(Stdio::null());
    command
}

/// Runs one child with a hard deadline. The child is killed on timeout and the
/// failure includes captured stdout/stderr.
pub fn run_with_timeout(command: &mut Command, timeout: Duration, label: &str) -> Output {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {label}: {error}"));
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .unwrap_or_else(|error| panic!("wait for {label}: {error}"));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("wait for {label}: {error}");
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("wait for timed-out {label}: {error}"));
            panic!(
                "{label} timed out after {timeout:?}; stdout={} stderr={}",
                stdout(&output),
                stderr(&output)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn run_installer(command: &mut Command, tripwires: &Tripwires) -> Output {
    let output = run_with_timeout(command, INSTALL_TIMEOUT, "wxml-lsp-install");
    tripwires.assert_not_executed();
    output
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        stdout(output),
        stderr(output)
    );
}

pub fn run_native_engine(binary: &Path) -> Vec<u8> {
    let mut command = Command::new(binary);
    command
        .arg("--version")
        .current_dir(binary.parent().expect("engine parent"))
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C");
    let output = run_with_timeout(&mut command, ENGINE_TIMEOUT, "native engine --version");
    assert!(
        output.status.success(),
        "native engine --version failed: status={:?}, stderr={}",
        output.status,
        stderr(&output)
    );
    output.stdout
}

/// Heuristic payload scan. The installer's manifest allowlist remains the
/// authoritative gate; this catches Node-runtime names that would be surprising
/// in a native package, case-insensitively.
pub fn assert_no_node_payload(files: &BTreeSet<String>) {
    for path in files {
        let lower = path.to_ascii_lowercase();
        assert!(
            !lower.contains("node_modules/"),
            "installed tree contains `{path}`"
        );
        assert!(
            !lower.contains("wxml-type-backend"),
            "installed tree contains `{path}`"
        );
        let file = lower.rsplit('/').next().unwrap_or(lower.as_str());
        let extension = file.rsplit_once('.').map(|(_, extension)| extension);
        assert!(
            !matches!(extension, Some("js" | "cjs" | "mjs" | "node")),
            "installed tree contains a Node/JavaScript payload `{path}`"
        );
        assert!(
            file != "node" && !file.starts_with("node."),
            "installed tree contains a Node executable payload `{path}`"
        );
        assert!(
            !file.ends_with("package-lock.json")
                && !file.ends_with("yarn.lock")
                && !file.ends_with("pnpm-lock.yaml"),
            "installed tree contains Node lockfile `{path}`"
        );
    }
}

/// Asserts that the journal directory is absent or empty without panicking when
/// a read-only install root did not create it.
pub fn assert_journal_absent_or_empty(root: &InstallRoot) {
    let directory = journal_dir(root);
    if !directory.exists() {
        return;
    }
    let empty = fs::read_dir(&directory).map_or(true, |mut entries| entries.next().is_none());
    assert!(
        empty,
        "transaction journals were left behind in {}",
        directory.display()
    );
}

pub fn flip_last_byte_copy(source: &Path, destination: &Path) -> Vec<u8> {
    let mut bytes = fs::read(source).expect("read tarball for corruption");
    assert_eq!(
        u64::try_from(bytes.len()).expect("tarball length"),
        ENGINE_DISTRIBUTION_SIZE
    );
    *bytes.last_mut().expect("non-empty tarball") ^= 0xff;
    fs::write(destination, &bytes).expect("write corrupted tarball");
    bytes
}

pub fn install_from_release(
    prepared: &PreparedRelease,
    tripwires: &Tripwires,
    base: &Path,
    root_path: &Path,
) -> Output {
    let mut command = installer_command(tripwires, base);
    command.args([
        "install",
        "--root",
        root_path.to_str().expect("UTF-8 root"),
        "--from-dir",
        prepared.release.to_str().expect("UTF-8 release"),
        "--version",
        PACKAGE_VERSION,
        "--offline",
    ]);
    run_installer(&mut command, tripwires)
}

pub fn assert_install_output(output: &Output) {
    assert_success(output, "native install");
    let install_stdout = stdout(output);
    assert!(
        install_stdout
            .lines()
            .any(|line| line == format!("installed {PACKAGE_VERSION} (native)")),
        "{install_stdout}"
    );
    assert!(
        install_stdout
            .lines()
            .any(|line| line == format!("engine: {ENGINE_NAME} {ENGINE_VERSION}")),
        "{install_stdout}"
    );
}

pub fn assert_install_state(root: &InstallRoot) -> InstallState {
    assert_eq!(
        root.current_version().expect("current version"),
        Some(PACKAGE_VERSION.to_owned())
    );
    let state = InstallState::read(root)
        .expect("read state.json")
        .expect("state.json exists");
    assert_eq!(state.active, PACKAGE_VERSION);
    assert_eq!(state.previous, None);
    assert_eq!(state.engine.name, ENGINE_NAME);
    assert_eq!(state.engine.version, ENGINE_VERSION);
    assert_eq!(state.engine.source_commit, ENGINE_SOURCE_COMMIT);
    assert_eq!(state.engine.protocol_version, PROTOCOL_VERSION);
    assert!(state_path(root).is_file(), "state.json is missing");
    state
}

pub fn assert_install_layout(root: &InstallRoot, root_path: &Path) {
    assert_eq!(
        mode_of(root_path),
        0o700,
        "install root mode was not preserved; this asserts 0700 preservation only"
    );
    assert_journal_absent_or_empty(root);
    if root.tmp_dir().exists() {
        assert_eq!(
            mode_of(&root.tmp_dir()),
            0o700,
            "tmp/ mode was not preserved"
        );
        assert!(
            fs::read_dir(root.tmp_dir())
                .expect("read tmp")
                .next()
                .is_none(),
            "staging files were left in tmp/"
        );
    }
    let versions: Vec<String> = fs::read_dir(root.versions_dir())
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
    assert_eq!(versions, vec![PACKAGE_VERSION.to_owned()]);
}

pub fn assert_installed_tree(
    installed: &Path,
    prepared: &PreparedRelease,
) -> wxml_delivery::verify_v2::NativeTreeReport {
    let report =
        verify_tree_v2(installed, expected(PACKAGE_VERSION)).expect("installed tree must verify");
    assert_eq!(report.file_count, prepared.fixture.manifest.files.len());
    assert_eq!(report.manifest.engine.version, ENGINE_VERSION);
    assert_eq!(report.manifest.engine.source_commit, ENGINE_SOURCE_COMMIT);
    assert_eq!(
        report.manifest.engine.distribution_sha256,
        ENGINE_DISTRIBUTION_SHA256
    );
    assert_eq!(
        report.manifest.engine.distribution_file_count,
        ENGINE_DISTRIBUTION_FILE_COUNT
    );
    assert_eq!(
        report.manifest.engine.distribution_unpacked_size,
        ENGINE_DISTRIBUTION_UNPACKED_SIZE
    );
    assert_installed_contents(installed);
    assert_installed_engine_files(installed, prepared);
    report
}

fn assert_installed_contents(installed: &Path) {
    let contents = collect_tree(installed).expect("collect installed tree");
    assert_no_node_payload(&contents.files);
    let engine_files = contents
        .files
        .iter()
        .filter(|path| path.starts_with("engine/"))
        .count();
    assert_eq!(
        u64::try_from(engine_files).expect("installed engine file count"),
        ENGINE_DISTRIBUTION_FILE_COUNT
    );
    for required in [
        "engine/LICENSE",
        "engine/NOTICE.txt",
        "engine/README.md",
        "engine/lib/tsc.sig",
        "engine/package.json",
        "provenance.json",
        "THIRD_PARTY_LICENSES.md",
        "manifest.json",
        "SHA256SUMS",
    ] {
        assert!(
            contents.files.contains(required),
            "installed tree is missing `{required}`"
        );
    }
}

fn assert_installed_engine_files(installed: &Path, prepared: &PreparedRelease) {
    let installed_tsc = installed.join(ENGINE_BINARY_PATH);
    assert_eq!(
        sha256_file(&installed_tsc).expect("hash installed tsc"),
        prepared.audit.engine_sha256
    );
    assert_eq!(
        fs::metadata(&installed_tsc)
            .expect("installed tsc metadata")
            .len(),
        prepared.audit.engine_size
    );
}

pub fn assert_installed_engine(current_tsc: &Path) {
    let version_output = run_native_engine(current_tsc);
    assert_eq!(
        version_output,
        format!("{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\n").as_bytes(),
        "installed native engine must print the exact frozen version line"
    );
    println!(
        "installed engine output: {}",
        String::from_utf8_lossy(&version_output).trim_end()
    );
}

pub fn assert_provenance_identity(provenance: &Value) {
    assert_eq!(
        provenance["engine"]["version"].as_str(),
        Some(ENGINE_VERSION)
    );
    assert_eq!(
        provenance["engine"]["sourceCommit"].as_str(),
        Some(ENGINE_SOURCE_COMMIT)
    );
    assert_eq!(
        provenance["engine"]["distributionIntegrity"].as_str(),
        Some(ENGINE_DISTRIBUTION_INTEGRITY)
    );
    assert_eq!(
        provenance["engine"]["distributionSha256"].as_str(),
        Some(ENGINE_DISTRIBUTION_SHA256)
    );
    assert_eq!(
        provenance["engine"]["distributionSize"].as_u64(),
        Some(ENGINE_DISTRIBUTION_SIZE)
    );
    assert_eq!(
        provenance["engine"]["distributionFileCount"].as_u64(),
        Some(ENGINE_DISTRIBUTION_FILE_COUNT)
    );
    assert_eq!(
        provenance["engine"]["distributionUnpackedSize"].as_u64(),
        Some(ENGINE_DISTRIBUTION_UNPACKED_SIZE)
    );
}

pub fn assert_provenance_files(provenance: &Value, installed: &Path, prepared: &PreparedRelease) {
    let engine_files = provenance["engineFiles"]
        .as_array()
        .expect("engineFiles array");
    assert_eq!(
        u64::try_from(engine_files.len()).expect("provenance engine file count"),
        ENGINE_DISTRIBUTION_FILE_COUNT
    );
    for entry in engine_files {
        let path = entry["path"].as_str().expect("provenance path");
        let digest = entry["sha256"].as_str().expect("provenance sha256");
        let installed_path = installed.join(path);
        assert_eq!(
            sha256_file(&installed_path)
                .unwrap_or_else(|error| panic!("hash installed {path}: {error}")),
            digest,
            "provenance hash mismatch for `{path}`"
        );
    }
    let tsc_entry = engine_files
        .iter()
        .find(|entry| entry["path"].as_str() == Some(ENGINE_BINARY_PATH))
        .expect("provenance entry for engine/lib/tsc");
    assert_eq!(
        tsc_entry["sha256"].as_str(),
        Some(prepared.audit.engine_sha256.as_str())
    );
    assert_eq!(tsc_entry["size"].as_u64(), Some(prepared.audit.engine_size));
}

pub fn assert_installed_provenance(installed: &Path, prepared: &PreparedRelease) {
    let installed_provenance =
        fs::read(installed.join("provenance.json")).expect("read installed provenance.json");
    assert_eq!(
        installed_provenance, prepared.fixture.provenance,
        "installed provenance bytes changed"
    );
    let provenance: Value =
        serde_json::from_slice(&installed_provenance).expect("parse installed provenance.json");
    assert_provenance_identity(&provenance);
    assert_provenance_files(&provenance, installed, prepared);
}

pub fn assert_cli_views(
    root_path: &Path,
    tripwires: &Tripwires,
    report: &wxml_delivery::verify_v2::NativeTreeReport,
) {
    let base = root_path.parent().expect("root parent");
    assert_verify_view(root_path, tripwires, base, report);
    assert_current_view(root_path, tripwires, base);
    assert_env_view(root_path, tripwires, base);
}

fn assert_verify_view(
    root_path: &Path,
    tripwires: &Tripwires,
    base: &Path,
    report: &wxml_delivery::verify_v2::NativeTreeReport,
) {
    let mut verify = installer_command(tripwires, base);
    verify.args([
        "verify",
        "--root",
        root_path.to_str().expect("UTF-8 root"),
        "--version",
        PACKAGE_VERSION,
    ]);
    let output = run_installer(&mut verify, tripwires);
    assert_success(&output, "native verify");
    let text = stdout(&output);
    for expected in [
        format!("verified {PACKAGE_VERSION} (native)"),
        format!("engine version: {ENGINE_VERSION}"),
        format!(
            "files: {}  bytes: {}",
            report.file_count, report.total_bytes
        ),
    ] {
        assert!(text.lines().any(|line| line == expected), "{text}");
    }
}

fn assert_current_view(root_path: &Path, tripwires: &Tripwires, base: &Path) {
    let mut current = installer_command(tripwires, base);
    current.args(["current", "--root", root_path.to_str().expect("UTF-8 root")]);
    let output = run_installer(&mut current, tripwires);
    assert_success(&output, "native current");
    let text = stdout(&output);
    for expected in [
        format!("version: {PACKAGE_VERSION}"),
        format!("platform: {NATIVE_PACKAGE_PLATFORM}"),
        format!("engine: {ENGINE_NAME} {ENGINE_VERSION}"),
    ] {
        assert!(text.lines().any(|line| line == expected), "{text}");
    }
}

fn assert_env_view(root_path: &Path, tripwires: &Tripwires, base: &Path) {
    let mut env = installer_command(tripwires, base);
    env.args(["env", "--root", root_path.to_str().expect("UTF-8 root")]);
    let output = run_installer(&mut env, tripwires);
    assert_success(&output, "native env");
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "env must print exactly one export: {text}");
    assert!(lines[0].starts_with("export WXML_LSP_BIN="), "{text}");
    assert!(lines[0].contains("current/bin/wxml-lsp"), "{text}");
}

pub fn create_private_install_root(base: &TestDir, name: &str) -> PathBuf {
    let root = base.path().join(name);
    fs::create_dir(&root).expect("create install root");
    set_mode(&root, 0o700).expect("make install root private");
    assert_eq!(
        mode_of(&root),
        0o700,
        "install root mode was not preserved as 0700"
    );
    root
}
