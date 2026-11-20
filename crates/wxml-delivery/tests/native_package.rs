//! Fixture-level integration tests for the deterministic schema v2 packager.
//!
//! Structural and negative tests use a synthetic engine plus the checked-in
//! 23-file typings payload fixture. Positive builds require an extracted pinned
//! engine supplied through `TSGO_NATIVE_ENGINE_DIR`; the packager rejects any
//! other `engine/lib/tsc` bytes. Nothing is downloaded and no Node-family
//! process is executed.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::too_many_lines,
    clippy::indexing_slicing
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use support::TempDir;
use wxml_delivery::archive::extract_tar_gz_allowlisted;
use wxml_delivery::manifest::{
    MAX_FILE_BYTES, parse_sha256sums, set_mode, sha256_bytes, sha256_file,
};
use wxml_delivery::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_DISTRIBUTION, ENGINE_SOURCE_COMMIT,
    ENGINE_VERSION, ExpectedManifestV2, MINIPROGRAM_TYPINGS_FILE_COUNT, NATIVE_PACKAGE_PLATFORM,
    STANDARD_LIB_FILE_COUNT, forbidden_runtime_path, read_index_v2, read_manifest_v2,
};
use wxml_delivery::package::{BuildOptionsV2, build_native_package};
use wxml_delivery::sri::sha512_sri_file;
use wxml_delivery::verify_v2::{tree_fingerprint, verify_tree_v2};

const REQUIRE_ENGINE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";
const PINNED_ENGINE_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";
const PINNED_ENGINE_SIZE: u64 = 24_522_912;

fn require_real_engine() -> bool {
    std::env::var(REQUIRE_ENGINE_ENV)
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// A prepared package input tree with the exact schema v2 input shape.
///
/// Engine and typings bytes are intentionally absent: the packager consumes
/// them only from the provisioned asset cache.
struct NativeTree {
    root: PathBuf,
}

impl NativeTree {
    fn synthetic(base: &Path, version: &str) -> Self {
        let root = base.join(format!("native-tree-{version}"));
        fs::create_dir_all(&root).unwrap();
        let tree = Self { root };
        tree.write_input(version);
        tree
    }

    fn write_input(&self, version: &str) {
        self.write(
            "bin/wxml-lsp",
            format!("#!/bin/sh\nprintf 'wxml-lsp {version}\\n'\n").as_bytes(),
            0o755,
        );
        self.write(
            "bin/wxml-lsp-install",
            format!("#!/bin/sh\nprintf 'wxml-lsp-install {version}\\n'\n").as_bytes(),
            0o755,
        );
        self.write("zed-extension/extension.wasm", b"\0asm-fixture", 0o644);
        self.write(
            "zed-extension/extension.toml",
            b"id = \"wxml-lsp\"\nname = \"WXML LSP\"\nversion = \"0.0.1\"\n",
            0o644,
        );
        self.write(
            "zed-extension/languages/wxml/config.toml",
            b"name = \"WXML\"\n",
            0o644,
        );
        self.write("THIRD_PARTY_LICENSES.md", b"# licenses\n", 0o644);
    }

    fn write(&self, path: &str, bytes: &[u8], mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, bytes).unwrap();
        set_mode(&destination, mode).unwrap();
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

/// Repository lock plus a provisioned cache for the real pinned assets.
struct RealAssets {
    lock: PathBuf,
    cache: PathBuf,
}

struct RealFixture {
    base: TempDir,
    tree: NativeTree,
    assets: RealAssets,
}

impl RealFixture {
    fn build(&self, output: &Path, version: &str) -> BuildOptionsV2 {
        self.build_input(self.tree.path(), output, version)
    }

    fn build_input(&self, input: &Path, output: &Path, version: &str) -> BuildOptionsV2 {
        let mut options = options(input, output, version);
        options.assets_lock = Some(self.assets.lock.clone());
        options.assets_cache = Some(self.assets.cache.clone());
        options
    }
}

fn repo_lock() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("assets.lock.json")
}

fn real_assets(base: &Path, test: &str) -> Option<RealAssets> {
    let lock = repo_lock();
    if !lock.is_file() {
        assert!(
            !require_real_engine(),
            "repository {} is required for {test}",
            lock.display()
        );
        eprintln!("{test} skipped: {} is missing", lock.display());
        return None;
    }
    if let Some(cache) = std::env::var_os("TSGO_NATIVE_CACHE") {
        let cache = fs::canonicalize(PathBuf::from(cache)).unwrap();
        return Some(RealAssets { lock, cache });
    }
    let (Some(engine), Some(typings)) = (
        std::env::var_os("TSGO_NATIVE_TARBALL"),
        std::env::var_os("TSGO_TYPINGS_TARBALL"),
    ) else {
        assert!(
            !require_real_engine(),
            "{test} requires TSGO_NATIVE_CACHE or TSGO_NATIVE_TARBALL + TSGO_TYPINGS_TARBALL \
             when {REQUIRE_ENGINE_ENV}=1"
        );
        eprintln!(
            "{test} skipped: set TSGO_NATIVE_CACHE to a provisioned cache, or set \
             TSGO_NATIVE_TARBALL and TSGO_TYPINGS_TARBALL"
        );
        return None;
    };
    let from_dir = base.join("from-dir");
    fs::create_dir_all(&from_dir).unwrap();
    fs::copy(engine, from_dir.join("typescript-linux-x64-7.1.0-beta.tgz")).unwrap();
    fs::copy(typings, from_dir.join("miniprogram-api-typings-5.3.1.tgz")).unwrap();
    let cache = base.join("cache");
    let report = wxml_delivery::provision::provision(&wxml_delivery::provision::ProvisionOptions {
        lock: lock.clone(),
        cache: cache.clone(),
        from_dir: Some(from_dir),
        offline: true,
        print_plan: false,
        allow_download: false,
    })
    .unwrap();
    assert_eq!(report.entries.len(), 2);
    Some(RealAssets { lock, cache })
}

fn real_fixture(name: &str) -> Option<RealFixture> {
    let base = TempDir::new(name);
    let assets = real_assets(base.path(), name)?;
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    Some(RealFixture { base, tree, assets })
}

fn copy_tree(source: &Path, destination: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from).unwrap();
        if metadata.file_type().is_dir() {
            copy_tree(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
            set_mode(&to, metadata.permissions().mode() & 0o777).unwrap();
        }
    }
}

fn set_mtime(path: &Path, seconds: u64) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(
        fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
    )
    .unwrap();
}

fn touch_tree(root: &Path, seconds: u64) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            touch_tree(&path, seconds);
        } else {
            set_mtime(&path, seconds);
        }
    }
}

fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(bytes.len()).unwrap());
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *bytes).unwrap();
    }
    builder.finish().unwrap();
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap();
}

fn write_raw_archive(path: &Path, name: &[u8], bytes: &[u8]) {
    assert!(name.len() < 100);
    let file = fs::File::create(path).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(bytes.len()).unwrap());
    header.set_mode(0o644);
    header.as_mut_bytes()[..name.len()].copy_from_slice(name);
    header.set_cksum();
    builder.append(&header, bytes).unwrap();
    builder.finish().unwrap();
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap();
}

fn gzip_header(archive: &[u8]) -> [u8; 10] {
    archive[..10].try_into().unwrap()
}

fn options(input: &Path, output: &Path, version: &str) -> BuildOptionsV2 {
    BuildOptionsV2 {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        assets_lock: None,
        assets_cache: None,
        release_version: version.to_owned(),
        protocol_version: 2,
        built_at: "2026-01-01T00:00:00Z".to_owned(),
        source_commit: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
        source_date_epoch: Some(1_763_683_200),
        force: true,
        dry_run: false,
    }
}

const fn expected(version: &str) -> ExpectedManifestV2<'_> {
    ExpectedManifestV2 {
        version: Some(version),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        protocol_version: Some(2),
    }
}

struct ArchiveHeader {
    path: String,
    mtime: u64,
    uid: u64,
    gid: u64,
    mode: u32,
    uname: Option<String>,
    gname: Option<String>,
}

fn archive_headers(path: &Path) -> Vec<ArchiveHeader> {
    let file = fs::File::open(path).unwrap();
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        entries.push(ArchiveHeader {
            path: entry.path().unwrap().to_string_lossy().into_owned(),
            mtime: header.mtime().unwrap(),
            uid: header.uid().unwrap(),
            gid: header.gid().unwrap(),
            mode: header.mode().unwrap(),
            uname: header.username().unwrap().map(ToOwned::to_owned),
            gname: header.groupname().unwrap().map(ToOwned::to_owned),
        });
    }
    entries
}

#[test]
fn two_builds_are_byte_identical_and_manifest_index_match_archive() {
    let Some(fixture) = real_fixture("native-package-deterministic") else {
        return;
    };
    let base = &fixture.base;
    let first =
        build_native_package(&fixture.build(&base.path().join("out-one"), "0.1.0")).unwrap();
    let second =
        build_native_package(&fixture.build(&base.path().join("out-two"), "0.1.0")).unwrap();

    assert_eq!(first.sha256, second.sha256);
    assert_eq!(first.size, second.size);
    assert_eq!(first.archive_integrity, second.archive_integrity);
    assert_eq!(
        fs::read(first.archive.as_ref().unwrap()).unwrap(),
        fs::read(second.archive.as_ref().unwrap()).unwrap()
    );

    let release = first.release_dir.as_ref().unwrap();
    let manifest = read_manifest_v2(release, expected("0.1.0")).unwrap();
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(manifest.package_version, "0.1.0");
    assert_eq!(manifest.platform, NATIVE_PACKAGE_PLATFORM);
    assert_eq!(manifest.protocol_version, 2);
    assert_eq!(manifest.engine.version, ENGINE_VERSION);
    assert_eq!(manifest.engine.source_commit, ENGINE_SOURCE_COMMIT);
    assert_eq!(manifest.engine.distribution, ENGINE_DISTRIBUTION);
    assert_eq!(
        manifest.engine.api_schema_version,
        ENGINE_API_SCHEMA_VERSION
    );
    assert_eq!(manifest.engine.ast_header_bytes, ENGINE_AST_HEADER_BYTES);
    assert_eq!(manifest.type_assets.len(), 2);
    assert_eq!(manifest.binaries.len(), 4);
    assert!(
        manifest
            .files
            .iter()
            .all(|file| forbidden_runtime_path(&file.path).is_none())
    );
    assert!(
        manifest
            .files
            .iter()
            .all(|file| !file.path.contains(base.path().to_string_lossy().as_ref()))
    );

    // The generated checksum file is the canonical `files[] + manifest.json` set.
    let release_sums = parse_sha256sums(&fs::read(release.join("SHA256SUMS")).unwrap()).unwrap();
    let mut expected_sums: BTreeMap<String, String> = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect();
    expected_sums.insert(
        "manifest.json".to_owned(),
        sha256_file(&release.join("manifest.json")).unwrap(),
    );
    assert_eq!(release_sums, expected_sums);
    assert_eq!(
        fs::read(release.join("manifest.json")).unwrap(),
        fs::read(Path::new(second.release_dir.as_ref().unwrap()).join("manifest.json")).unwrap()
    );

    // Index v2 agrees with the archive identity.
    let output = release.parent().unwrap();
    let index = read_index_v2(&output.join("index.json")).unwrap();
    let release_entry = index.find("0.1.0", NATIVE_PACKAGE_PLATFORM).unwrap();
    assert!(release_entry.matches_manifest(&manifest));
    assert_eq!(release_entry.sha256, first.sha256);
    assert_eq!(release_entry.size, first.size);
    assert_eq!(
        release_entry.archive_integrity,
        sha512_sri_file(first.archive.as_ref().unwrap())
            .unwrap()
            .to_sri()
    );
    assert_eq!(
        release_entry.archive,
        first
            .archive
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
    );
    let index_text = fs::read_to_string(output.join("index.json")).unwrap();
    assert!(!index_text.contains(base.path().to_string_lossy().as_ref()));

    // Output-level sums list the archive exactly once.
    let output_sums = parse_sha256sums(&fs::read(output.join("SHA256SUMS")).unwrap()).unwrap();
    assert_eq!(output_sums.get(&release_entry.archive), Some(&first.sha256));

    // Archive headers are normalized and sorted.
    let headers = archive_headers(first.archive.as_ref().unwrap());
    assert_eq!(headers.len(), manifest.files.len() + 2);
    let paths: Vec<String> = headers.iter().map(|header| header.path.clone()).collect();
    assert_eq!(paths.len(), manifest.files.len() + 2);
    assert!(paths.contains(&"manifest.json".to_owned()));
    assert!(paths.contains(&"SHA256SUMS".to_owned()));
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
    for header in &headers {
        let path = &header.path;
        assert_eq!(header.mtime, 0, "mtime leaked into `{path}`");
        assert_eq!(header.uid, 0, "uid leaked into `{path}`");
        assert_eq!(header.gid, 0, "gid leaked into `{path}`");
        assert!(
            header.uname.as_deref().unwrap_or_default().is_empty(),
            "uname leaked into `{path}`"
        );
        assert!(
            header.gname.as_deref().unwrap_or_default().is_empty(),
            "gname leaked into `{path}`"
        );
        assert!(
            matches!(header.mode, 0o644 | 0o755),
            "mode {:o} for `{path}`",
            header.mode
        );
    }

    // Extracted archive verifies against the manifest and matches the tree.
    let extracted = base.path().join("extracted");
    let mut allowed: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    allowed.insert("manifest.json".to_owned());
    allowed.insert("SHA256SUMS".to_owned());
    extract_tar_gz_allowlisted(first.archive.as_ref().unwrap(), &extracted, &allowed).unwrap();
    verify_tree_v2(&extracted, expected("0.1.0")).unwrap();
    assert_eq!(
        tree_fingerprint(release).unwrap(),
        tree_fingerprint(&extracted).unwrap()
    );
    assert_eq!(
        first.manifest_sha256.as_deref(),
        Some(
            sha256_file(&release.join("manifest.json"))
                .unwrap()
                .as_str()
        )
    );
    let standard = manifest
        .type_assets
        .iter()
        .find(|asset| asset.name == "typescript-standard-lib")
        .unwrap();
    assert_eq!(standard.file_count, STANDARD_LIB_FILE_COUNT);
    let typings = manifest
        .type_assets
        .iter()
        .find(|asset| asset.name == "miniprogram-api-typings")
        .unwrap();
    assert_eq!(typings.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);

    // Engine/typings manifest entries come from the lock payload table, not
    // from any input path.
    let lock: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture.assets.lock).unwrap()).unwrap();
    for asset in lock["assets"].as_array().unwrap() {
        for locked in asset["payload"]["files"].as_array().unwrap() {
            let path = locked["path"].as_str().unwrap();
            let entry = manifest.file(path).unwrap_or_else(|| {
                panic!("manifest is missing lock payload `{path}`");
            });
            assert_eq!(entry.sha256, locked["sha256"].as_str().unwrap());
            assert_eq!(entry.size, locked["size"].as_u64().unwrap());
            assert_eq!(entry.mode, locked["mode"].as_str().unwrap());
        }
    }
    let provenance: serde_json::Value =
        serde_json::from_slice(&fs::read(release.join("provenance.json")).unwrap()).unwrap();
    let parsed_lock =
        wxml_delivery::assets_lock::AssetsLock::parse(&fs::read(&fixture.assets.lock).unwrap())
            .unwrap();
    let lock_sha256 = parsed_lock.lock_sha256().unwrap();
    assert_eq!(
        provenance["assetsLockSha256"].as_str(),
        Some(lock_sha256.as_str())
    );
    assert_eq!(provenance["assets"].as_array().unwrap().len(), 2);
    assert_eq!(
        provenance["sourceCommit"].as_str(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(provenance["sourceDateEpoch"].as_u64(), Some(1_763_683_200));
    let mut preimage = String::new();
    for file in manifest
        .files
        .iter()
        .filter(|file| file.path != "provenance.json")
    {
        preimage.push_str(&file.path);
        preimage.push('\0');
        preimage.push_str(&file.sha256);
        preimage.push('\n');
    }
    assert_eq!(
        provenance["archivePayloadDigest"].as_str(),
        Some(sha256_bytes(preimage.as_bytes()).as_str())
    );
}

fn build_error(tree: &NativeTree, base: &Path, needle: &str) {
    let error =
        build_native_package(&options(tree.path(), &base.join("out-error"), "0.1.0")).unwrap_err();
    let message = error.to_string().to_lowercase();
    assert!(
        message.contains(&needle.to_lowercase()),
        "expected `{needle}` in `{error}`"
    );
}

#[test]
fn rejects_node_runtime_and_unexpected_payloads() {
    let cases: &[(&str, &[u8], &str)] = &[
        (
            "node_modules/typescript/package.json",
            b"{}\n",
            "node_modules",
        ),
        (
            "Node_Modules/typescript/package.json",
            b"{}\n",
            "node_modules",
        ),
        ("a/b/node_modules/x", b"{}\n", "node_modules"),
        (
            "bin/wxml-type-backend/server.js",
            b"console.log('backend');\n",
            "wxml-type-backend",
        ),
        ("bin/helper.js", b"module.exports = 1;\n", "javascript"),
        ("bin/helper.JS", b"module.exports = 1;\n", "javascript"),
        ("Package.JSON", b"{}\n", "package.json"),
        (
            "typings/miniprogram-api-typings/package.json",
            b"{}\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/miniprogram-api-typings/PACKAGE.JSON",
            b"{}\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/miniprogram-api-typings/README-en.md",
            b"English readme\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/miniprogram-api-typings/README-EN.MD",
            b"English readme\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/miniprogram-api-typings/typings.json",
            b"{\"main\":\"index.d.ts\"}\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/miniprogram-api-typings/Typings.JSON",
            b"{\"main\":\"index.d.ts\"}\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/README-en.md",
            b"English readme\n",
            "must come from the verified asset cache",
        ),
        (
            "typings/typings.json",
            b"{\"main\":\"index.d.ts\"}\n",
            "must come from the verified asset cache",
        ),
        (
            "extra.txt",
            b"extra\n",
            "not part of the schema v2 package layout",
        ),
        (
            "engine/lib/lib.d.ts.map",
            b"{}\n",
            "must come from the verified asset cache",
        ),
        (
            "engine/package.json.bak",
            b"{}\n",
            "must come from the verified asset cache",
        ),
    ];
    for (index, (path, bytes, needle)) in cases.iter().enumerate() {
        let base = TempDir::new(&format!("native-package-reject-{index}"));
        let tree = NativeTree::synthetic(base.path(), "0.1.0");
        tree.write(path, bytes, 0o644);
        build_error(&tree, base.path(), needle);
    }
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_and_unsafe_modes() {
    let base = TempDir::new("native-package-symlink");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    std::os::unix::fs::symlink("/etc/passwd", tree.path().join("zed-extension/README.md")).unwrap();
    let error = build_native_package(&options(tree.path(), &base.path().join("out-a"), "0.1.0"))
        .unwrap_err();
    assert!(
        matches!(error, wxml_delivery::error::DeliveryError::UnsafeArchive(_)),
        "{error:?}"
    );

    let base = TempDir::new("native-package-writable");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("zed-extension/extension.wasm", b"world-writable\n", 0o666);
    build_error(&tree, base.path(), "group/other-write");
}

#[cfg(unix)]
#[test]
fn rejects_non_allowlisted_executable_and_non_executable_binaries() {
    let base = TempDir::new("native-package-exec-allowlist");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("zed-extension/extension.wasm", b"readme\n", 0o755);
    build_error(&tree, base.path(), "executable but not in the allowlist");

    let base = TempDir::new("native-package-bin-mode");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("bin/wxml-lsp", b"#!/bin/sh\nexit 0\n", 0o644);
    build_error(&tree, base.path(), "must have mode 0755");
}

#[test]
fn rejects_input_supplied_engine_or_typings() {
    for path in [
        "engine/lib/lib.es2020.d.ts",
        "engine/package.json",
        "typings/miniprogram-api-typings/index.d.ts",
    ] {
        let base = TempDir::new("native-package-input-assets");
        let tree = NativeTree::synthetic(base.path(), "0.1.0");
        tree.write(path, b"// supplied by the input tree\n", 0o644);
        build_error(
            &tree,
            base.path(),
            "must come from the verified asset cache",
        );
    }
}

#[test]
fn rejects_missing_required_input_payload() {
    let base = TempDir::new("native-package-missing");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    fs::remove_file(tree.path().join("bin/wxml-lsp-install")).unwrap();
    build_error(&tree, base.path(), "missing `bin/wxml-lsp-install`");
}

#[test]
fn requires_assets_lock_and_cache_before_creating_output() {
    let base = TempDir::new("native-package-no-assets");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    let output = base.path().join("release");
    let error = build_native_package(&options(tree.path(), &output, "0.1.0")).unwrap_err();
    assert!(error.to_string().contains("--assets-lock"), "{error}");
    assert!(!output.exists());

    let mut with_lock = options(tree.path(), &output, "0.1.0");
    with_lock.assets_lock = Some(repo_lock());
    let error = build_native_package(&with_lock).unwrap_err();
    assert!(error.to_string().contains("--cache"), "{error}");
    assert!(!output.exists());
}

#[test]
fn dry_run_writes_nothing() {
    let Some(fixture) = real_fixture("native-package-dry") else {
        return;
    };
    let output = fixture.base.path().join("release");
    let mut options = fixture.build(&output, "0.1.0");
    options.dry_run = true;
    let report = build_native_package(&options).unwrap();
    assert!(report.index.is_none());
    assert!(report.release_dir.is_none());
    assert!(report.archive.is_none());
    assert!(report.manifest_sha256.is_none());
    assert!(!output.exists());
}

#[test]
fn native_cli_prints_usage() {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args(["native", "--help"]);
    let result = support::run(&mut command);
    support::assert_success(&result);
    let text = support::stdout(&result);
    assert!(text.contains("--release-version"), "{text}");
    assert!(text.contains("--built-at"), "{text}");
}

#[test]
fn native_cli_builds_without_node_or_network() {
    let Some(fixture) = real_fixture("native-package-cli") else {
        return;
    };
    let base = &fixture.base;
    let tree = &fixture.tree;
    let output = base.path().join("release");
    let shim = base.path().join("shims");
    fs::create_dir_all(&shim).unwrap();
    let marker = base.path().join("node-executed.txt");
    for name in ["node", "npm", "npx", "pnpm", "yarn"] {
        let path = shim.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$0\" >> '{}'\nexit 97\n",
                marker.display()
            ),
        )
        .unwrap();
        set_mode(&path, 0o755).unwrap();
    }
    let empty = base.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    let server = support::FixtureServer::directory(&empty);

    let mut path = shim.clone().into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args([
        "native",
        "--input",
        tree.path().to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--assets-lock",
        fixture.assets.lock.to_str().unwrap(),
        "--cache",
        fixture.assets.cache.to_str().unwrap(),
        "--release-version",
        "0.1.0",
        "--built-at",
        "2026-01-01T00:00:00Z",
    ]);
    command.env("PATH", path.clone());
    command.env("WXML_NODE", base.path().join("shim-node"));
    command.env("WXML_TYPE_BACKEND", base.path().join("shim-server.js"));
    command.env("WXML_BACKEND_DIR", &shim);
    command.env("HTTP_PROXY", server.url());
    command.env("HTTPS_PROXY", server.url());
    let result = support::run(&mut command);
    support::assert_success(&result);
    assert!(
        !marker.exists(),
        "packager executed a Node-family shim: {:?}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
    assert!(
        server.hits().is_empty(),
        "packager contacted the network: {:?}",
        server.hits()
    );

    let positive_node = std::process::Command::new("node")
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(positive_node.status.code(), Some(97));
    assert!(
        marker.is_file(),
        "node tripwire positive control did not run"
    );
    fs::remove_file(&marker).unwrap();

    let address = server
        .url()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let mut stream = std::net::TcpStream::connect(&address).unwrap();
    stream
        .write_all(b"GET /positive-control HTTP/1.1\r\nHost: fixture\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
    assert!(
        !server.hits().is_empty(),
        "network tripwire positive control did not record a request"
    );
    assert!(output.join("index.json").is_file());
    let archive = output.join(format!("wxml-lsp-0.1.0-{NATIVE_PACKAGE_PLATFORM}.tar.gz"));
    assert!(archive.is_file());
    let release = output.join(format!("wxml-lsp-0.1.0-{NATIVE_PACKAGE_PLATFORM}"));
    let manifest = read_manifest_v2(&release, expected("0.1.0")).unwrap();
    assert_eq!(manifest.engine.version, ENGINE_VERSION);
    assert!(support::stdout(&result).contains("native package"));
}

#[test]
fn packages_optional_engine_readme_and_wxml_grammar() {
    let Some(fixture) = real_fixture("native-package-optional") else {
        return;
    };
    let base = &fixture.base;
    let tree = &fixture.tree;
    tree.write("zed-extension/grammars/wxml.wasm", b"\0asm-grammar", 0o644);
    let report =
        build_native_package(&fixture.build(&base.path().join("release"), "0.1.0")).unwrap();
    let manifest =
        read_manifest_v2(report.release_dir.as_ref().unwrap(), expected("0.1.0")).unwrap();
    assert!(manifest.file("engine/README.md").is_some());
    assert!(manifest.file("zed-extension/grammars/wxml.wasm").is_some());
}

#[test]
fn refuses_overwrite_without_force_and_replaces_an_invalid_index_with_force() {
    let Some(fixture) = real_fixture("native-package-force") else {
        return;
    };
    let base = &fixture.base;
    let output = base.path().join("release");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("index.json"), b"{}\n").unwrap();

    let report = build_native_package(&fixture.build(&output, "0.1.0")).unwrap();
    assert!(report.index.is_some());
    let index = read_index_v2(&output.join("index.json")).unwrap();
    index.find("0.1.0", NATIVE_PACKAGE_PLATFORM).unwrap();

    let mut second = fixture.build(&output, "0.1.0");
    second.force = false;
    let error = build_native_package(&second).unwrap_err();
    assert!(error.to_string().contains("--force"), "{error}");
}

#[test]
fn rejects_unsupported_native_protocol() {
    let base = TempDir::new("native-package-protocol");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    let mut options = options(tree.path(), &base.path().join("release"), "0.1.0");
    options.protocol_version = 3;
    let error = build_native_package(&options).unwrap_err();
    assert!(error.to_string().contains("protocol"), "{error}");
}

#[test]
fn rejects_missing_top_level_zed_extension_version() {
    let base = TempDir::new("native-package-zed-version");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\n[grammars.wxml]\nversion = \"9.9.9\"\n",
        0o644,
    );
    build_error(&tree, base.path(), "missing top-level `version`");
}

#[test]
fn native_cli_rejects_a_non_pinned_platform() {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args([
        "native",
        "--input",
        "missing",
        "--output",
        "out",
        "--platform",
        "linux-aarch64",
    ]);
    let result = support::run(&mut command);
    support::assert_failure(&result, "linux-x86_64");
}

#[test]
fn rejects_input_supplied_provenance() {
    let base = TempDir::new("native-package-input-provenance");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("provenance.json", b"{\"claim\":\"unverified\"}\n", 0o644);
    build_error(&tree, base.path(), "must not supply");
}

#[cfg(unix)]
#[test]
fn rejects_hardlinks_non_utf8_backslash_empty_and_oversized_inputs() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let base = TempDir::new("native-package-non-utf8");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    let name = OsString::from_vec(vec![b'x', 0xff]);
    fs::write(tree.path().join(name), b"x").unwrap();
    build_error(&tree, base.path(), "non-utf-8");

    let base = TempDir::new("native-package-backslash");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("bad\\name", b"x", 0o644);
    build_error(&tree, base.path(), "backslash");

    let base = TempDir::new("native-package-empty");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("THIRD_PARTY_LICENSES.md", b"", 0o644);
    build_error(&tree, base.path(), "is empty");

    let base = TempDir::new("native-package-oversized");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    let oversized = tree.path().join("THIRD_PARTY_LICENSES.md");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(file);
    build_error(&tree, base.path(), "per-file limit");

    let base = TempDir::new("native-package-hardlink");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    fs::hard_link(
        tree.path().join("THIRD_PARTY_LICENSES.md"),
        tree.path().join("zed-extension/languages/wxml/hard.wasm"),
    )
    .unwrap();
    build_error(&tree, base.path(), "hard link");
}

#[test]
fn archives_ignore_input_mtimes_and_mode_metadata() {
    let Some(fixture) = real_fixture("native-package-metadata") else {
        return;
    };
    let tree_a = fixture.tree.path();
    let tree_b = fixture.base.path().join("native-tree-copy");
    copy_tree(tree_a, &tree_b);
    touch_tree(tree_a, 1_000_000);
    touch_tree(&tree_b, 2_000_000);
    set_mode(&tree_b.join("zed-extension/extension.wasm"), 0o600).unwrap();

    let first = build_native_package(&fixture.build_input(
        tree_a,
        &fixture.base.path().join("out-a"),
        "0.1.0",
    ))
    .unwrap();
    let second = build_native_package(&fixture.build_input(
        &tree_b,
        &fixture.base.path().join("out-b"),
        "0.1.0",
    ))
    .unwrap();
    let first_bytes = fs::read(first.archive.as_ref().unwrap()).unwrap();
    let second_bytes = fs::read(second.archive.as_ref().unwrap()).unwrap();
    assert_eq!(first_bytes, second_bytes);
    let first_header = gzip_header(&first_bytes);
    let second_header = gzip_header(&second_bytes);
    assert_eq!(&first_header[4..8], &[0, 0, 0, 0], "gzip mtime is not zero");
    assert_eq!(
        &second_header[4..8],
        &[0, 0, 0, 0],
        "gzip mtime is not zero"
    );
    assert_eq!(
        first_header[9], second_header[9],
        "gzip OS byte is unstable"
    );
    assert_eq!(
        fs::read(first.release_dir.as_ref().unwrap().join("manifest.json")).unwrap(),
        fs::read(second.release_dir.as_ref().unwrap().join("manifest.json")).unwrap()
    );
    let first_output = first.archive.as_ref().unwrap().parent().unwrap();
    let second_output = second.archive.as_ref().unwrap().parent().unwrap();
    assert_eq!(
        fs::read(first_output.join("index.json")).unwrap(),
        fs::read(second_output.join("index.json")).unwrap()
    );
}

#[test]
fn generated_provenance_binds_measured_and_pinned_facts() {
    let Some(fixture) = real_fixture("native-package-provenance") else {
        return;
    };
    let report =
        build_native_package(&fixture.build(&fixture.base.path().join("release"), "0.1.0"))
            .unwrap();
    let release = report.release_dir.as_ref().unwrap();
    let manifest = read_manifest_v2(release, expected("0.1.0")).unwrap();
    let provenance_file = manifest.file("provenance.json").unwrap();
    let provenance_path = release.join("provenance.json");
    assert_eq!(
        provenance_file.sha256,
        sha256_file(&provenance_path).unwrap()
    );
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&provenance_path).unwrap()).unwrap();
    assert_eq!(value["engine"]["binary"]["measurement"], "measured");
    assert_eq!(value["engine"]["binary"]["sha256"], PINNED_ENGINE_SHA256);
    assert_eq!(value["engine"]["binary"]["size"], PINNED_ENGINE_SIZE);
    assert_eq!(
        value["engine"]["distribution"]["measurement"],
        "pinned-expected"
    );
    assert_eq!(
        value["engine"]["distribution"]["sha256"],
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_SHA256
    );
    let parsed_lock =
        wxml_delivery::assets_lock::AssetsLock::parse(&fs::read(&fixture.assets.lock).unwrap())
            .unwrap();
    assert_eq!(
        value["assetsLockSha256"].as_str(),
        Some(parsed_lock.lock_sha256().unwrap().as_str())
    );
    let assets = value["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    for entry in assets {
        let name = entry["name"].as_str().unwrap();
        let root = entry["packageRoot"].as_str().unwrap();
        let digest = entry["payloadTreeDigest"].as_str().unwrap();
        let files: Vec<wxml_delivery::manifest::FileEntry> = manifest
            .files
            .iter()
            .filter(|file| file.path.starts_with(&format!("{root}/")))
            .cloned()
            .collect();
        assert_eq!(
            digest,
            wxml_delivery::assets_lock::canonical_tree_digest(&files, root).unwrap(),
            "provenance tree digest for `{name}`"
        );
    }
}

fn assert_no_stage_dirs(output: &Path) {
    for entry in fs::read_dir(output).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        assert!(
            !name.to_string_lossy().starts_with(".native-stage-"),
            "staging directory was not cleaned: {}",
            entry.path().display()
        );
    }
}

#[test]
fn failed_forced_rebuild_keeps_previous_output_byte_identical() {
    let Some(fixture) = real_fixture("native-package-force-failure") else {
        return;
    };
    let output = fixture.base.path().join("release");
    let first = build_native_package(&fixture.build(&output, "0.1.0")).unwrap();
    let release = first.release_dir.as_ref().unwrap();
    let fingerprint = tree_fingerprint(release).unwrap();
    let archive = first.archive.as_ref().unwrap();
    let archive_bytes = fs::read(archive).unwrap();
    let index_bytes = fs::read(output.join("index.json")).unwrap();
    let sums_bytes = fs::read(output.join("SHA256SUMS")).unwrap();
    let provenance_bytes = fs::read(release.join("provenance.json")).unwrap();

    fs::remove_file(fixture.tree.path().join("THIRD_PARTY_LICENSES.md")).unwrap();
    let error = build_native_package(&fixture.build(&output, "0.1.0")).unwrap_err();
    assert!(
        error.to_string().contains("THIRD_PARTY_LICENSES.md"),
        "{error}"
    );
    assert_eq!(tree_fingerprint(release).unwrap(), fingerprint);
    assert_eq!(fs::read(archive).unwrap(), archive_bytes);
    assert_eq!(fs::read(output.join("index.json")).unwrap(), index_bytes);
    assert_eq!(fs::read(output.join("SHA256SUMS")).unwrap(), sums_bytes);
    assert_eq!(
        fs::read(release.join("provenance.json")).unwrap(),
        provenance_bytes
    );
    assert_no_stage_dirs(&output);
}

#[test]
fn archive_reader_rejects_duplicate_and_case_folded_entries() {
    let base = TempDir::new("native-package-archive-duplicate");
    let duplicate = base.path().join("duplicate.tar.gz");
    write_archive(&duplicate, &[("a.txt", b"x"), ("a.txt", b"y")]);
    let destination = base.path().join("duplicate-extract");
    let allowed = BTreeSet::from(["a.txt".to_owned()]);
    let error = extract_tar_gz_allowlisted(&duplicate, &destination, &allowed).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("duplicate") || message.contains("case folding"),
        "{error}"
    );
    assert!(!destination.exists());

    let folded = base.path().join("folded.tar.gz");
    write_archive(&folded, &[("a.txt", b"x"), ("A.TXT", b"y")]);
    let destination = base.path().join("folded-extract");
    let allowed = BTreeSet::from(["a.txt".to_owned(), "A.TXT".to_owned()]);
    let error = extract_tar_gz_allowlisted(&folded, &destination, &allowed).unwrap_err();
    assert!(error.to_string().contains("case folding"), "{error}");
    assert!(!destination.exists());

    for (name, label) in [
        (b"../escape".as_slice(), "traversal"),
        (b"/absolute".as_slice(), "absolute"),
    ] {
        let archive = base.path().join(format!("{label}.tar.gz"));
        write_raw_archive(&archive, name, b"x");
        let destination = base.path().join(format!("{label}-extract"));
        let error =
            extract_tar_gz_allowlisted(&archive, &destination, &BTreeSet::new()).unwrap_err();
        assert!(
            matches!(error, wxml_delivery::error::DeliveryError::UnsafeArchive(_)),
            "{label}: {error:?}"
        );
        assert!(!destination.exists());
    }
}

#[test]
fn corrupted_archive_is_rejected_on_reextract() {
    let Some(fixture) = real_fixture("native-package-tamper") else {
        return;
    };
    let report =
        build_native_package(&fixture.build(&fixture.base.path().join("release"), "0.1.0"))
            .unwrap();
    let release = report.release_dir.as_ref().unwrap();
    let manifest = read_manifest_v2(release, expected("0.1.0")).unwrap();
    let mut allowed: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    allowed.insert("manifest.json".to_owned());
    allowed.insert("SHA256SUMS".to_owned());
    let mut bytes = fs::read(report.archive.as_ref().unwrap()).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    let tampered = fixture.base.path().join("tampered.tar.gz");
    fs::write(&tampered, bytes).unwrap();
    let destination = fixture.base.path().join("tampered-extract");
    let outcome = extract_tar_gz_allowlisted(&tampered, &destination, &allowed)
        .and_then(|_| verify_tree_v2(&destination, expected("0.1.0")).map(|_| ()));
    assert!(outcome.is_err(), "tampered archive was accepted");
    assert!(!destination.exists() || verify_tree_v2(&destination, expected("0.1.0")).is_err());
}

#[test]
fn native_cli_requires_built_at_or_source_date_epoch() {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args(["native", "--input", "missing", "--output", "out"]);
    command.env_remove("SOURCE_DATE_EPOCH");
    let result = support::run(&mut command);
    support::assert_failure(&result, "built-at");

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args(["native", "--input", "missing", "--output", "out"]);
    command.env("SOURCE_DATE_EPOCH", "not-a-number");
    let result = support::run(&mut command);
    support::assert_failure(&result, "source_date_epoch");
}

#[test]
fn native_cli_accepts_source_date_epoch() {
    let Some(fixture) = real_fixture("native-package-source-date-epoch") else {
        return;
    };
    let output = fixture.base.path().join("release");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args([
        "native",
        "--input",
        fixture.tree.path().to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--assets-lock",
        fixture.assets.lock.to_str().unwrap(),
        "--cache",
        fixture.assets.cache.to_str().unwrap(),
        "--release-version",
        "0.1.0",
    ]);
    command.env("SOURCE_DATE_EPOCH", "1700000000");
    let result = support::run(&mut command);
    support::assert_success(&result);
    let release = output.join(format!("wxml-lsp-0.1.0-{NATIVE_PACKAGE_PLATFORM}"));
    let manifest = read_manifest_v2(&release, expected("0.1.0")).unwrap();
    assert_eq!(manifest.built_at, "2023-11-14T22:13:20Z");
}

#[test]
fn native_cli_rejects_illegal_version_and_missing_values() {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args([
        "native",
        "--input",
        "missing",
        "--output",
        "out",
        "--release-version",
        "01.0.0",
        "--built-at",
        "2026-01-01T00:00:00Z",
    ]);
    let result = support::run(&mut command);
    support::assert_failure(&result, "semantic version");

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args(["native", "--input"]);
    let result = support::run(&mut command);
    support::assert_failure(&result, "requires a value");

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command.args([
        "native",
        "--output",
        "out",
        "--built-at",
        "2026-01-01T00:00:00Z",
    ]);
    let result = support::run(&mut command);
    support::assert_failure(&result, "--input is required");
}
