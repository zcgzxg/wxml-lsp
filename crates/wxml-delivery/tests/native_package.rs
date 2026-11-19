//! Fixture-level integration tests for the deterministic schema v2 packager.
//!
//! Structural and negative tests use a synthetic tree. Positive builds require
//! an extracted pinned engine supplied through `TSGO_NATIVE_ENGINE_DIR`; the
//! packager rejects any other `engine/lib/tsc` bytes. Nothing is downloaded and
//! no Node-family process is executed.

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
use wxml_delivery::manifest::{MAX_FILE_BYTES, parse_sha256sums, set_mode, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_DISTRIBUTION, ENGINE_SOURCE_COMMIT,
    ENGINE_VERSION, ExpectedManifestV2, MINIPROGRAM_TYPINGS_FILE_COUNT, NATIVE_PACKAGE_PLATFORM,
    STANDARD_LIB_FILE_COUNT, forbidden_runtime_path, read_index_v2, read_manifest_v2,
};
use wxml_delivery::package::{BuildOptionsV2, build_native_package};
use wxml_delivery::sri::sha512_sri_file;
use wxml_delivery::verify_v2::{tree_fingerprint, verify_tree_v2};

const ENGINE_DIR_ENV: &str = "TSGO_NATIVE_ENGINE_DIR";
const REQUIRE_ENGINE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";
const PINNED_ENGINE_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";
const PINNED_ENGINE_SIZE: u64 = 24_522_912;

fn require_real_engine() -> bool {
    std::env::var(REQUIRE_ENGINE_ENV)
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn pinned_engine_dir(test: &str) -> Option<PathBuf> {
    let Some(value) = std::env::var_os(ENGINE_DIR_ENV) else {
        assert!(
            !require_real_engine(),
            "{ENGINE_DIR_ENV} is required when {REQUIRE_ENGINE_ENV}=1; point it at an extracted \
             @typescript/typescript-linux-x64 7.1.0-beta package directory"
        );
        eprintln!(
            "{test} skipped: set {ENGINE_DIR_ENV} to an extracted \
             @typescript/typescript-linux-x64 7.1.0-beta package directory"
        );
        return None;
    };
    let path = PathBuf::from(value);
    let path = fs::canonicalize(&path).unwrap();
    let engine = path.join("lib/tsc");
    let metadata = fs::metadata(&engine).unwrap();
    assert_eq!(metadata.len(), PINNED_ENGINE_SIZE, "pinned engine size");
    assert_eq!(
        sha256_file(&engine).unwrap(),
        PINNED_ENGINE_SHA256,
        "pinned engine SHA-256"
    );
    Some(path)
}

/// A prepared package tree with the exact schema v2 shape.
struct NativeTree {
    root: PathBuf,
}

impl NativeTree {
    /// Synthetic tree whose engine bytes are intentionally not pinned.
    fn synthetic(base: &Path, version: &str) -> Self {
        let root = base.join(format!("native-tree-{version}"));
        fs::create_dir_all(&root).unwrap();
        let tree = Self { root };
        tree.write_common(version);
        tree.write("engine/lib/tsc", b"\x7fELF-fixture-native-engine\n", 0o755);
        tree.write("engine/lib/tsc.sig", b"signature-placeholder\n", 0o644);
        tree.write("engine/LICENSE", b"Apache License fixture\n", 0o644);
        tree.write("engine/NOTICE.txt", b"NOTICE fixture\n", 0o644);
        tree.write(
            "engine/package.json",
            format!(
                "{{\"name\":\"{ENGINE_DISTRIBUTION}\",\"version\":\"{ENGINE_VERSION}\",\
                 \"gitHead\":\"{ENGINE_SOURCE_COMMIT}\"}}\n"
            )
            .as_bytes(),
            0o644,
        );
        tree.write("engine/lib/lib.d.ts", b"// lib.d.ts\n", 0o644);
        tree.write("engine/lib/lib.es5.d.ts", b"// lib.es5.d.ts\n", 0o644);
        tree.write("engine/lib/lib.es2020.d.ts", b"// lib.es2020.d.ts\n", 0o644);
        for index in 0..113 {
            tree.write(
                &format!("engine/lib/lib.f{index:03}.d.ts"),
                format!("// filler {index}\n").as_bytes(),
                0o644,
            );
        }
        tree
    }

    /// Tree carrying the exact pinned engine bytes from `TSGO_NATIVE_ENGINE_DIR`.
    fn pinned(base: &Path, version: &str, test: &str) -> Option<Self> {
        let engine_dir = pinned_engine_dir(test)?;
        let root = base.join(format!("native-tree-{version}"));
        fs::create_dir_all(&root).unwrap();
        let tree = Self { root };
        tree.write_common(version);
        tree.copy_from(&engine_dir.join("lib/tsc"), "engine/lib/tsc", 0o755);
        tree.copy_from(&engine_dir.join("lib/tsc.sig"), "engine/lib/tsc.sig", 0o644);
        tree.copy_from(&engine_dir.join("LICENSE"), "engine/LICENSE", 0o644);
        tree.copy_from(&engine_dir.join("NOTICE.txt"), "engine/NOTICE.txt", 0o644);
        let mut dts_count = 0;
        for entry in fs::read_dir(engine_dir.join("lib")).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("lib") && name.ends_with(".d.ts") {
                tree.copy_from(&entry.path(), &format!("engine/lib/{name}"), 0o644);
                dts_count += 1;
            }
        }
        assert_eq!(dts_count, STANDARD_LIB_FILE_COUNT, "pinned lib*.d.ts count");
        tree.write(
            "engine/package.json",
            format!(
                "{{\"name\":\"{ENGINE_DISTRIBUTION}\",\"version\":\"{ENGINE_VERSION}\",\
                 \"gitHead\":\"{ENGINE_SOURCE_COMMIT}\"}}\n"
            )
            .as_bytes(),
            0o644,
        );
        Some(tree)
    }

    fn write_common(&self, version: &str) {
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
        self.write(
            "typings/miniprogram-api-typings/index.d.ts",
            b"// miniprogram index\n",
            0o644,
        );
        self.write(
            "typings/miniprogram-api-typings/LICENSE",
            b"MIT fixture\n",
            0o644,
        );
        for index in 0..24 {
            self.write(
                &format!("typings/miniprogram-api-typings/types/t{index:02}.d.ts"),
                format!("// typings {index}\n").as_bytes(),
                0o644,
            );
        }
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

    fn copy_from(&self, source: &Path, path: &str, mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(source, &destination).unwrap();
        set_mode(&destination, mode).unwrap();
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

struct PinnedFixture {
    base: TempDir,
    tree: NativeTree,
}

fn pinned_fixture(name: &str) -> Option<PinnedFixture> {
    let base = TempDir::new(name);
    let tree = NativeTree::pinned(base.path(), "0.1.0", name)?;
    Some(PinnedFixture { base, tree })
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
        release_version: version.to_owned(),
        protocol_version: 2,
        built_at: "2026-01-01T00:00:00Z".to_owned(),
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
    let Some(fixture) = pinned_fixture("native-package-deterministic") else {
        return;
    };
    let base = &fixture.base;
    let tree = &fixture.tree;
    let first =
        build_native_package(&options(tree.path(), &base.path().join("out-one"), "0.1.0")).unwrap();
    let second =
        build_native_package(&options(tree.path(), &base.path().join("out-two"), "0.1.0")).unwrap();

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
            "extra.txt",
            b"extra\n",
            "not part of the schema v2 package layout",
        ),
        (
            "engine/lib/lib.d.ts.map",
            b"{}\n",
            "not part of the schema v2 package layout",
        ),
        (
            "engine/package.json.bak",
            b"{}\n",
            "not part of the schema v2 package layout",
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
    std::os::unix::fs::symlink("/etc/passwd", tree.path().join("engine/README.md")).unwrap();
    let error = build_native_package(&options(tree.path(), &base.path().join("out-a"), "0.1.0"))
        .unwrap_err();
    assert!(
        matches!(error, wxml_delivery::error::DeliveryError::UnsafeArchive(_)),
        "{error:?}"
    );

    let base = TempDir::new("native-package-writable");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write("engine/LICENSE", b"world-writable\n", 0o666);
    build_error(&tree, base.path(), "group/other-write");
}

#[cfg(unix)]
#[test]
fn rejects_non_allowlisted_executable_in_a_pinned_tree() {
    let Some(fixture) = pinned_fixture("native-package-exec-allowlist") else {
        return;
    };
    fixture.tree.write("engine/README.md", b"readme\n", 0o755);
    build_error(
        &fixture.tree,
        fixture.base.path(),
        "executable but not in the allowlist",
    );
}

#[cfg(unix)]
#[test]
fn rejects_non_executable_binary_in_a_pinned_tree() {
    let Some(fixture) = pinned_fixture("native-package-bin-mode") else {
        return;
    };
    fixture
        .tree
        .write("bin/wxml-lsp", b"#!/bin/sh\nexit 0\n", 0o644);
    build_error(&fixture.tree, fixture.base.path(), "must have mode 0755");
}

#[test]
fn rejects_missing_or_mismatched_engine_identity() {
    let cases: &[(&[u8], &str)] = &[
        (
            b"{\"name\":\"@typescript/typescript-linux-x64\"}\n",
            "missing `version`",
        ),
        (
            b"{\"name\":\"typescript\",\"version\":\"7.1.0-beta\"}\n",
            "does not match",
        ),
        (
            b"{\"name\":\"@typescript/typescript-linux-x64\",\"version\":\"7.0.2\"}\n",
            "does not match",
        ),
        (
            b"{\"name\":\"@typescript/typescript-linux-x64\",\"version\":\"7.1.0-beta\",\"gitHead\":\"deadbeef\"}\n",
            "githead",
        ),
        (b"not-json\n", "malformed"),
    ];
    for (index, (bytes, needle)) in cases.iter().enumerate() {
        let base = TempDir::new(&format!("native-package-engine-{index}"));
        let tree = NativeTree::synthetic(base.path(), "0.1.0");
        tree.write("engine/package.json", bytes, 0o644);
        build_error(&tree, base.path(), needle);
    }
}

#[test]
fn rejects_missing_required_payload() {
    let Some(fixture) = pinned_fixture("native-package-missing") else {
        return;
    };
    fs::remove_file(fixture.tree.path().join("engine/lib/lib.es2020.d.ts")).unwrap();
    build_error(&fixture.tree, fixture.base.path(), "exactly 116");
}

#[test]
fn dry_run_writes_nothing() {
    let Some(fixture) = pinned_fixture("native-package-dry") else {
        return;
    };
    let output = fixture.base.path().join("release");
    let mut options = options(fixture.tree.path(), &output, "0.1.0");
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
    let Some(fixture) = pinned_fixture("native-package-cli") else {
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
    let Some(fixture) = pinned_fixture("native-package-optional") else {
        return;
    };
    let base = &fixture.base;
    let tree = &fixture.tree;
    tree.write("engine/README.md", b"# engine\n", 0o644);
    tree.write("zed-extension/grammars/wxml.wasm", b"\0asm-grammar", 0o644);
    let report =
        build_native_package(&options(tree.path(), &base.path().join("release"), "0.1.0")).unwrap();
    let manifest =
        read_manifest_v2(report.release_dir.as_ref().unwrap(), expected("0.1.0")).unwrap();
    assert!(manifest.file("engine/README.md").is_some());
    assert!(manifest.file("zed-extension/grammars/wxml.wasm").is_some());
}

#[test]
fn refuses_overwrite_without_force_and_replaces_an_invalid_index_with_force() {
    let Some(fixture) = pinned_fixture("native-package-force") else {
        return;
    };
    let base = &fixture.base;
    let tree = &fixture.tree;
    let output = base.path().join("release");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("index.json"), b"{}\n").unwrap();

    let report = build_native_package(&options(tree.path(), &output, "0.1.0")).unwrap();
    assert!(report.index.is_some());
    let index = read_index_v2(&output.join("index.json")).unwrap();
    index.find("0.1.0", NATIVE_PACKAGE_PLATFORM).unwrap();

    let mut second = options(tree.path(), &output, "0.1.0");
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
    let Some(fixture) = pinned_fixture("native-package-zed-version") else {
        return;
    };
    fixture.tree.write(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\n[grammars.wxml]\nversion = \"9.9.9\"\n",
        0o644,
    );
    build_error(
        &fixture.tree,
        fixture.base.path(),
        "missing top-level `version`",
    );
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
fn rejects_unpinned_engine_binary_bytes() {
    let base = TempDir::new("native-package-engine-bytes");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    build_error(&tree, base.path(), "pinned expected");
}

#[test]
fn rejects_missing_engine_git_head() {
    let base = TempDir::new("native-package-engine-git-head");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    tree.write(
        "engine/package.json",
        b"{\"name\":\"@typescript/typescript-linux-x64\",\"version\":\"7.1.0-beta\"}\n",
        0o644,
    );
    build_error(&tree, base.path(), "githead");
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
    tree.write("engine/README.md", b"", 0o644);
    build_error(&tree, base.path(), "is empty");

    let base = TempDir::new("native-package-oversized");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    let oversized = tree.path().join("engine/README.md");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(file);
    build_error(&tree, base.path(), "per-file limit");

    let base = TempDir::new("native-package-hardlink");
    let tree = NativeTree::synthetic(base.path(), "0.1.0");
    fs::hard_link(
        tree.path().join("engine/lib/lib.d.ts"),
        tree.path().join("engine/lib/lib.hard.d.ts"),
    )
    .unwrap();
    build_error(&tree, base.path(), "hard link");
}

#[test]
fn archives_ignore_input_mtimes_and_mode_metadata() {
    let Some(fixture) = pinned_fixture("native-package-metadata") else {
        return;
    };
    let tree_a = fixture.tree.path();
    let tree_b = fixture.base.path().join("native-tree-copy");
    copy_tree(tree_a, &tree_b);
    touch_tree(tree_a, 1_000_000);
    touch_tree(&tree_b, 2_000_000);
    set_mode(&tree_b.join("engine/LICENSE"), 0o600).unwrap();

    let first = build_native_package(&options(
        tree_a,
        &fixture.base.path().join("out-a"),
        "0.1.0",
    ))
    .unwrap();
    let second = build_native_package(&options(
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
    let Some(fixture) = pinned_fixture("native-package-provenance") else {
        return;
    };
    let report = build_native_package(&options(
        fixture.tree.path(),
        &fixture.base.path().join("release"),
        "0.1.0",
    ))
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
    let Some(fixture) = pinned_fixture("native-package-force-failure") else {
        return;
    };
    let output = fixture.base.path().join("release");
    let first = build_native_package(&options(fixture.tree.path(), &output, "0.1.0")).unwrap();
    let release = first.release_dir.as_ref().unwrap();
    let fingerprint = tree_fingerprint(release).unwrap();
    let archive = first.archive.as_ref().unwrap();
    let archive_bytes = fs::read(archive).unwrap();
    let index_bytes = fs::read(output.join("index.json")).unwrap();
    let sums_bytes = fs::read(output.join("SHA256SUMS")).unwrap();
    let provenance_bytes = fs::read(release.join("provenance.json")).unwrap();

    fixture
        .tree
        .write("engine/lib/tsc", b"tampered engine", 0o755);
    let error = build_native_package(&options(fixture.tree.path(), &output, "0.1.0")).unwrap_err();
    assert!(error.to_string().contains("pinned expected"), "{error}");
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
    let Some(fixture) = pinned_fixture("native-package-tamper") else {
        return;
    };
    let report = build_native_package(&options(
        fixture.tree.path(),
        &fixture.base.path().join("release"),
        "0.1.0",
    ))
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
    let Some(fixture) = pinned_fixture("native-package-source-date-epoch") else {
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
