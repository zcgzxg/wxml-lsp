//! Shared inputs, packaging and assertions for the F3 real-release harness.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use wxml_delivery::assets_lock::{Asset, AssetsLock};
use wxml_delivery::index_v2::read_index_v2;
use wxml_delivery::manifest::{FileEntry, parse_sha256sums, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_BINARY_PATH, ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_FILE_COUNT,
    ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE,
    ENGINE_NAME, ENGINE_PROBE_PREFIX, ENGINE_VERSION, ExpectedManifestV2,
    MINIPROGRAM_TYPINGS_ASSET, MINIPROGRAM_TYPINGS_FILE_COUNT, NATIVE_PACKAGE_PLATFORM,
    STANDARD_LIB_ASSET, STANDARD_LIB_FILE_COUNT, read_manifest_v2,
};
use wxml_delivery::provision::{
    CacheEntry, ProvisionOptions, ProvisionReport, SourceKind, plan, provision,
};
use wxml_delivery::type_assets::forbidden_runtime_path;
use wxml_delivery::verify_v2::{collect_tree, tree_fingerprint, verify_tree_v2};

pub use crate::real_asset_support::{
    TestDir, Tripwires, assert_journal_absent_or_empty, assert_no_node_payload, assert_success,
    assert_supported_host, create_private_install_root, installer_command, mode_of, run_installer,
    run_native_engine, run_with_timeout, stderr, stdout,
};

/// Package version of the installed N1 release.
pub const VERSION_N1: &str = "0.1.0";
/// Package version of the upgraded N2 release.
pub const VERSION_N2: &str = "0.2.0";
/// Fixed build timestamp used by both deterministic builds.
pub const BUILT_AT: &str = "2026-11-19T00:00:00Z";
/// Native RPC protocol shared by both releases.
pub const PROTOCOL: u64 = 2;
/// Frozen SHA-256 of the canonical repository lock bytes.
pub const LOCK_SHA256: &str = "ce33fa960aa3eaf433419e2baeb6709587783ec11450642b6b1188c597725456";

pub const ENGINE_TARBALL_ENV: &str = "TSGO_NATIVE_TARBALL";
pub const TYPINGS_TARBALL_ENV: &str = "TSGO_TYPINGS_TARBALL";
pub const LSP_BIN_ENV: &str = "WXML_F3_LSP_BIN";
pub const INSTALL_BIN_ENV: &str = "WXML_F3_INSTALL_BIN";
pub const ZED_WASM_ENV: &str = "WXML_F3_ZED_WASM";
pub const REQUIRE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";

const ENGINE_TARBALL_NAME: &str = "typescript-linux-x64-7.1.0-beta.tgz";
const TYPINGS_TARBALL_NAME: &str = "miniprogram-api-typings-5.3.1.tgz";
const FALLBACK_ENGINE_TARBALL: &str =
    "/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz";
const FALLBACK_TYPINGS_TARBALL: &str =
    "/tmp/opencode/tsgo-typings-provenance/miniprogram-api-typings-5.3.1.tgz";

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate below the repository root")
        .to_path_buf()
}

pub fn repository_lock() -> PathBuf {
    repository_root().join("assets.lock.json")
}

/// Only `1`/`true` enable required mode; other values fail closed.
pub fn required_mode() -> bool {
    match std::env::var(REQUIRE_ENV) {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value.is_empty() => false,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
        Ok(value) => panic!("{REQUIRE_ENV} must be unset, empty, `1` or `true`; got `{value}`"),
        Err(std::env::VarError::NotUnicode(_)) => panic!("{REQUIRE_ENV} must be valid UTF-8"),
    }
}

fn canonical_file(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path)
        .unwrap_or_else(|error| panic!("cannot resolve {}: {error}", path.display()));
    assert!(
        canonical.is_file(),
        "{} is not a regular file",
        canonical.display()
    );
    canonical
}

fn explicit_or_skip(
    env_name: &str,
    label: &str,
    optional_fallback: Option<&str>,
    test: &str,
) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(env_name) {
        if !value.is_empty() {
            return Some(canonical_file(Path::new(&value)));
        }
    }
    assert!(
        !required_mode(),
        "{env_name} is required when {REQUIRE_ENV}=1 ({label} for {test})"
    );
    if let Some(fallback) = optional_fallback {
        let path = Path::new(fallback);
        if path.is_file() {
            eprintln!(
                "{test}: {label} using optional fallback {} (previous run wrote it)",
                path.display()
            );
            return Some(canonical_file(path));
        }
    }
    None
}

/// Resolved pinned inputs for one real release build.
#[derive(Debug)]
pub struct RealInputs {
    pub engine_tarball: PathBuf,
    pub typings_tarball: PathBuf,
    pub lsp_bin: PathBuf,
    pub install_bin: PathBuf,
    pub zed_wasm: PathBuf,
}

pub fn resolve_real_inputs(test: &str) -> Option<RealInputs> {
    let engine_tarball = explicit_or_skip(
        ENGINE_TARBALL_ENV,
        "pinned engine tarball",
        Some(FALLBACK_ENGINE_TARBALL),
        test,
    )?;
    let Some(typings_tarball) = explicit_or_skip(
        TYPINGS_TARBALL_ENV,
        "pinned typings tarball",
        Some(FALLBACK_TYPINGS_TARBALL),
        test,
    ) else {
        assert!(
            !required_mode(),
            "{TYPINGS_TARBALL_ENV} is required when {REQUIRE_ENV}=1 ({test})"
        );
        eprintln!(
            "{test} skipped: set {TYPINGS_TARBALL_ENV} to the pinned 5.3.1 \
             miniprogram-api-typings tarball"
        );
        return None;
    };
    let lsp_bin = resolve_lsp_bin(test)?;
    let install_bin = resolve_install_bin(test)?;
    let zed_wasm = resolve_zed_wasm(test)?;
    Some(RealInputs {
        engine_tarball,
        typings_tarball,
        lsp_bin,
        install_bin,
        zed_wasm,
    })
}

fn resolve_lsp_bin(test: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(LSP_BIN_ENV) {
        if !value.is_empty() {
            return Some(canonical_file(Path::new(&value)));
        }
    }
    assert!(
        !required_mode(),
        "{LSP_BIN_ENV} is required when {REQUIRE_ENV}=1 ({test}); build \
         `cargo build --release -p wxml-lsp`"
    );
    let target = repository_root().join("target");
    for candidate in [
        target.join("release/wxml-lsp"),
        target.join("debug/wxml-lsp"),
    ] {
        if candidate.is_file() {
            eprintln!(
                "{test}: {LSP_BIN_ENV} not set; using {} (not necessarily a release build)",
                candidate.display()
            );
            return Some(canonical_file(&candidate));
        }
    }
    eprintln!(
        "{test} skipped: set {LSP_BIN_ENV} to a production `wxml-lsp` binary or run \
         `cargo build --release -p wxml-lsp`"
    );
    None
}

fn resolve_install_bin(test: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(INSTALL_BIN_ENV) {
        if !value.is_empty() {
            return Some(canonical_file(Path::new(&value)));
        }
    }
    let built = PathBuf::from(env!("CARGO_BIN_EXE_wxml-lsp-install"));
    if built.is_file() {
        eprintln!(
            "{test}: {INSTALL_BIN_ENV} not set; using the test-built installer {}",
            built.display()
        );
        return Some(canonical_file(&built));
    }
    assert!(
        !required_mode(),
        "{INSTALL_BIN_ENV} is required when {REQUIRE_ENV}=1 ({test})"
    );
    eprintln!("{test} skipped: set {INSTALL_BIN_ENV} to a production `wxml-lsp-install`");
    None
}

fn resolve_zed_wasm(test: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(ZED_WASM_ENV) {
        if !value.is_empty() {
            let path = canonical_file(Path::new(&value));
            assert!(
                fs::read(&path).is_ok_and(|bytes| bytes.starts_with(b"\0asm")),
                "{} is not a WebAssembly module",
                path.display()
            );
            return Some(path);
        }
    }
    let target = repository_root().join("target");
    for candidate in [
        target.join("wasm32-wasip2/release/zed_wxml.wasm"),
        repository_root().join("editors/zed/extension.wasm"),
        PathBuf::from(
            "/tmp/opencode/f3-real-release/zed-target/wasm32-wasip2/release/zed_wxml.wasm",
        ),
    ] {
        if candidate.is_file() {
            eprintln!(
                "{test}: {ZED_WASM_ENV} not set; using {}",
                candidate.display()
            );
            return Some(canonical_file(&candidate));
        }
    }
    assert!(
        !required_mode(),
        "{ZED_WASM_ENV} is required when {REQUIRE_ENV}=1 ({test}); build \
         `CARGO_TARGET_DIR=<outside> cargo build --manifest-path editors/zed/Cargo.toml \
         --release --target wasm32-wasip2`"
    );
    eprintln!("{test} skipped: set {ZED_WASM_ENV} to a wasm32-wasip2 build of editors/zed");
    None
}

/// A provisioned cache plus the verified lock facts used to build releases.
#[derive(Debug)]
pub struct Provisioned {
    pub lock_path: PathBuf,
    pub lock: AssetsLock,
    pub lock_sha256: String,
    pub cache: PathBuf,
    pub entries: Vec<CacheEntry>,
}

impl Provisioned {
    pub fn entry(&self, name: &str) -> (&Asset, &CacheEntry) {
        let asset = self
            .lock
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .unwrap_or_else(|| panic!("lock is missing `{name}`"));
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("cache report is missing `{name}`"));
        (asset, entry)
    }

    pub fn payload_path(&self, name: &str, relative: &str) -> PathBuf {
        let (_, entry) = self.entry(name);
        entry.directory.join("payload").join(relative)
    }
}

pub fn provision_real_assets(base: &TestDir, inputs: &RealInputs) -> Provisioned {
    assert_supported_host();
    let lock_path = canonical_file(&repository_lock());
    let lock_bytes = fs::read(&lock_path).expect("read repository assets.lock.json");
    let lock = AssetsLock::parse(&lock_bytes).expect("parse repository lock");
    lock.validate().expect("validate repository lock");
    let lock_sha256 = lock.lock_sha256().expect("lock sha256");
    assert_eq!(
        lock_sha256, LOCK_SHA256,
        "the committed assets.lock.json identity changed"
    );
    assert_eq!(
        sha256_file(&lock_path).expect("hash lock file"),
        LOCK_SHA256,
        "raw lock bytes and canonical lock identity disagree"
    );

    let from_dir = base.path().join("from-dir");
    fs::create_dir(&from_dir).expect("create from-dir");
    set_mode(&from_dir, 0o700).expect("private from-dir");
    copy_input(&inputs.engine_tarball, &from_dir.join(ENGINE_TARBALL_NAME));
    copy_input(
        &inputs.typings_tarball,
        &from_dir.join(TYPINGS_TARBALL_NAME),
    );

    let cache = base.path().join("cache");
    let options = ProvisionOptions {
        lock: lock_path.clone(),
        cache: cache.clone(),
        from_dir: Some(from_dir),
        offline: true,
        print_plan: false,
        allow_download: false,
    };
    let report: ProvisionReport = provision(&options).expect("provision pinned real assets");
    assert_eq!(report.lock_sha256, lock_sha256);
    assert_eq!(report.entries.len(), lock.assets.len());
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.source == SourceKind::FromDir)
    );
    assert_lock_matches_cache(&lock, &report);

    let replay = ProvisionOptions {
        from_dir: None,
        ..options
    };
    let cached = provision(&replay).expect("second provisioning run must hit the cache");
    assert!(
        cached
            .entries
            .iter()
            .all(|entry| entry.source == SourceKind::Cache),
        "a second provisioning run re-extracted instead of hitting the cache"
    );
    let dry_plan = plan(&replay).expect("plan cached assets");
    assert!(dry_plan.assets.iter().all(|asset| asset.cache_hit));

    println!(
        "PROVISION: lock={} sha256={} assets={} cache={}",
        lock_path.display(),
        lock_sha256,
        report.entries.len(),
        cache.display()
    );
    Provisioned {
        lock_path,
        lock,
        lock_sha256,
        cache,
        entries: report.entries,
    }
}

fn assert_lock_matches_cache(lock: &AssetsLock, report: &ProvisionReport) {
    for asset in &lock.assets {
        let entry = report
            .entries
            .iter()
            .find(|entry| entry.name == asset.name)
            .unwrap_or_else(|| panic!("cache report is missing `{}`", asset.name));
        assert_eq!(entry.payload_tree_digest, asset.payload.tree_digest);
        assert_eq!(entry.file_count, asset.payload.file_count);
        assert_eq!(entry.total_bytes, asset.payload.total_bytes);
        let expected_key = format!(
            "{}.{}",
            asset.payload.tree_digest,
            &report.lock_sha256[..16]
        );
        assert_eq!(entry.cache_key, expected_key);
        println!(
            "PROVISION-ASSET: {} version={} key={} files={} bytes={} tree={} source={}",
            asset.name,
            asset.version,
            entry.cache_key,
            entry.file_count,
            entry.total_bytes,
            entry.payload_tree_digest,
            entry.source.label()
        );
    }
}

fn copy_input(source: &Path, destination: &Path) {
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    set_mode(destination, 0o600).expect("private input copy");
}

/// Builds the production-shaped package input tree. `engine/` and `typings/`
/// are deliberately absent; they come only from the verified cache.
pub fn write_input_tree(base: &TestDir, inputs: &RealInputs, provisioned: &Provisioned) -> PathBuf {
    let root = base.path().join("input-tree");
    fs::create_dir(&root).expect("create input tree");
    set_mode(&root, 0o755).expect("input tree mode");

    copy_tree_file(&inputs.lsp_bin, &root.join("bin/wxml-lsp"), 0o755);
    copy_tree_file(
        &inputs.install_bin,
        &root.join("bin/wxml-lsp-install"),
        0o755,
    );
    copy_tree_file(
        &inputs.zed_wasm,
        &root.join("zed-extension/extension.wasm"),
        0o644,
    );
    let zed_source = repository_root().join("editors/zed");
    copy_tree_file(
        &zed_source.join("extension.toml"),
        &root.join("zed-extension/extension.toml"),
        0o644,
    );
    copy_directory(
        &zed_source.join("languages"),
        &root.join("zed-extension/languages"),
    );
    let licenses = aggregate_licenses(provisioned);
    fs::write(root.join("THIRD_PARTY_LICENSES.md"), licenses).expect("write third-party licenses");
    set_mode(&root.join("THIRD_PARTY_LICENSES.md"), 0o644).expect("license mode");
    println!(
        "INPUT-TREE: {} lsp={} install={} zed-wasm={}",
        root.display(),
        inputs.lsp_bin.display(),
        inputs.install_bin.display(),
        inputs.zed_wasm.display()
    );
    root
}

fn copy_tree_file(source: &Path, destination: &Path, mode: u32) {
    fs::create_dir_all(destination.parent().expect("file parent")).expect("create file parent");
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    set_mode(destination, mode).expect("payload mode");
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied directory");
    set_mode(destination, 0o755).expect("copied directory mode");
    let mut entries: Vec<_> = fs::read_dir(source)
        .expect("read source directory")
        .map(|entry| entry.expect("source entry").path())
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("file name"));
        let metadata = fs::symlink_metadata(&entry).expect("source metadata");
        if metadata.file_type().is_dir() {
            copy_directory(&entry, &target);
        } else {
            assert!(
                metadata.file_type().is_file(),
                "unexpected non-file {} in staged input",
                entry.display()
            );
            copy_tree_file(&entry, &target, 0o644);
        }
    }
}

fn aggregate_licenses(provisioned: &Provisioned) -> Vec<u8> {
    let mut text = String::from(
        "# Third-party licenses\n\n\
         Generated by `crates/wxml-delivery/tests/f3_real_release.rs` from the verified\n\
         provisioning cache of the committed `assets.lock.json`. This is acceptance-harness\n\
         evidence, not a legal review.\n\n",
    );
    for asset in &provisioned.lock.assets {
        let _ = write!(
            text,
            "## {} {}\n\nLicense: `{}`\nPackage root: `{}`\nTarball SHA-256: `{}`\n\n",
            asset.npm_package,
            asset.version,
            asset.license,
            asset.package_root,
            asset.tarball.sha256
        );
        for license_file in &asset.license_files {
            let path = provisioned.payload_path(&asset.name, license_file);
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("read bundled license {}: {error}", path.display()));
            let _ = write!(text, "### `{license_file}`\n\n```text\n");
            text.push_str(&String::from_utf8_lossy(&bytes));
            text.push_str("\n```\n\n");
        }
    }
    text.push_str(
        "## Rust dependencies\n\n\
         The Rust binaries are built from this workspace and its `Cargo.lock`. Their\n\
         transitive dependency licenses are limited by `deny.toml` to permissive SPDX\n\
         identifiers (MIT, Apache-2.0, BSD-3-Clause, ISC, Unicode-3.0, Zlib, ...).\n\
         Run `cargo deny --offline check` for the authoritative dependency graph.\n",
    );
    text.into_bytes()
}

/// Copies a production binary and rewrites its embedded release version.
///
/// The N1→N2 upgrade probe requires `bin/* --version` to equal the package
/// version. A second production release does not exist at this commit, so the
/// N2 fixture is the N1 release binary with the single same-length version
/// constant replaced. Every other byte, and the whole engine/typings payload,
/// stays real; the evidence document states this limitation explicitly.
pub fn retarget_binary_version(source: &Path, destination: &Path, from: &str, to: &str) {
    assert_eq!(
        from.len(),
        to.len(),
        "version constants must have equal length"
    );
    let mut bytes = fs::read(source).unwrap_or_else(|error| {
        panic!("read {} for version retargeting: {error}", source.display())
    });
    let needle = from.as_bytes();
    let occurrences = bytes
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count();
    assert_eq!(
        occurrences,
        1,
        "{} contains {occurrences} `{from}` constants; expected exactly one",
        source.display()
    );
    let offset = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("version constant position");
    bytes[offset..offset + needle.len()].copy_from_slice(to.as_bytes());
    fs::write(destination, &bytes).expect("write version-retargeted binary");
    set_mode(destination, 0o755).expect("version-retargeted binary mode");
}

/// Opt-in stable install root for the external Zed smoke.
///
/// `WXML_F3_KEEP_ROOT=1` moves the root below `WXML_TEST_TMPDIR` so the shell
/// harness can point Zed at an installed package after the test process exits.
pub fn keep_root_enabled() -> bool {
    std::env::var("WXML_F3_KEEP_ROOT")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub fn install_root(base: &TestDir, name: &str) -> PathBuf {
    if !keep_root_enabled() {
        return create_private_install_root(base, name);
    }
    let parent = std::env::var_os("WXML_TEST_TMPDIR")
        .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
    let stable = parent.join("f3-real-release-keep");
    fs::create_dir_all(&stable).expect("create stable keep root");
    set_mode(&stable, 0o700).expect("private keep root");
    let root = stable.join(name);
    if root.exists() {
        fs::remove_dir_all(&root).expect("remove previous stable root");
    }
    fs::create_dir(&root).expect("create stable install root");
    set_mode(&root, 0o700).expect("private stable install root");
    println!("KEEP-ROOT: {}", root.display());
    root
}

/// One deterministic package build produced by the production packager CLI.
#[derive(Debug)]
pub struct BuiltRelease {
    pub output: PathBuf,
    pub release_dir: PathBuf,
    pub archive: PathBuf,
    pub sidecar: PathBuf,
    pub index: PathBuf,
    pub sums: PathBuf,
    pub archive_sha256: String,
    pub archive_integrity: String,
    pub archive_size: u64,
    pub manifest_sha256: String,
}

pub fn build_release(
    output: &Path,
    input: &Path,
    provisioned: &Provisioned,
    version: &str,
    force: bool,
) -> BuiltRelease {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command
        .arg("native")
        .arg("--input")
        .arg(input)
        .arg("--output")
        .arg(output)
        .arg("--assets-lock")
        .arg(&provisioned.lock_path)
        .arg("--cache")
        .arg(&provisioned.cache)
        .arg("--release-version")
        .arg(version)
        .arg("--built-at")
        .arg(BUILT_AT)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    if force {
        command.arg("--force");
    }
    let output_result = run_with_timeout(
        &mut command,
        std::time::Duration::from_secs(300),
        "packager",
    );
    assert!(
        output_result.status.success(),
        "native package build failed: status={:?}\nstdout={}\nstderr={}",
        output_result.status,
        stdout(&output_result),
        String::from_utf8_lossy(&output_result.stderr)
    );
    let text = stdout(&output_result);
    assert!(
        text.lines().any(|line| line
            == format!("native package: wxml-lsp {version} ({NATIVE_PACKAGE_PLATFORM})")),
        "{text}"
    );

    let release_name = format!("wxml-lsp-{version}-{NATIVE_PACKAGE_PLATFORM}");
    let archive_name = format!("{release_name}.tar.gz");
    let release_dir = output.join(&release_name);
    let archive = output.join(&archive_name);
    let sidecar = output.join(format!("{archive_name}.sha256"));
    let index = output.join("index.json");
    let sums = output.join("SHA256SUMS");
    let manifest_path = release_dir.join("manifest.json");
    let manifest =
        read_manifest_v2(&release_dir, expected(version)).expect("read release manifest");
    assert_eq!(manifest.package_version, version);
    let archive_bytes = fs::read(&archive).expect("read release archive");
    let archive_sha256 = sha256_bytes(&archive_bytes);
    let index = read_index_v2(&index).expect("read release index");
    let record = index
        .find(version, NATIVE_PACKAGE_PLATFORM)
        .unwrap_or_else(|_| panic!("release index has no {version} record"));
    record
        .verify_archive(&archive)
        .expect("archive must match the release index");
    assert_eq!(record.sha256, archive_sha256);
    assert_eq!(
        record.size,
        u64::try_from(archive_bytes.len()).expect("archive size")
    );
    println!(
        "BUILD: version={version} archive={} sha256={} sri={} size={} manifest={}",
        archive.display(),
        archive_sha256,
        record.archive_integrity,
        record.size,
        sha256_file(&manifest_path).expect("manifest sha256")
    );
    preserve_release(output, release_dir.file_name().expect("release name"));
    BuiltRelease {
        output: output.to_path_buf(),
        release_dir,
        archive,
        sidecar,
        index: output.join("index.json"),
        sums,
        archive_sha256,
        archive_integrity: record.archive_integrity.clone(),
        archive_size: record.size,
        manifest_sha256: sha256_file(&manifest_path).expect("manifest sha256"),
    }
}

fn preserve_release(output: &Path, release_name: &std::ffi::OsStr) {
    if !keep_root_enabled() {
        return;
    }
    let Some(parent) = std::env::var_os("WXML_TEST_TMPDIR") else {
        return;
    };
    let artifacts = PathBuf::from(parent)
        .join("f3-real-release-keep")
        .join("artifacts");
    if let Err(error) = fs::create_dir_all(&artifacts) {
        eprintln!(
            "cannot create artifact directory {}: {error}",
            artifacts.display()
        );
        return;
    }
    let _ = set_mode(&artifacts, 0o700);
    let destination = artifacts.join(release_name);
    if destination.exists() {
        if let Err(error) = fs::remove_dir_all(&destination) {
            eprintln!(
                "cannot refresh artifact directory {}: {error}",
                destination.display()
            );
            return;
        }
    }
    copy_directory(output, &destination);
    println!("ARTIFACT: {}", destination.display());
}

pub const fn expected(version: &str) -> ExpectedManifestV2<'_> {
    ExpectedManifestV2 {
        version: Some(version),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        protocol_version: Some(PROTOCOL),
    }
}

/// Verifies layout, license and negative-scan acceptance on one built release.
pub fn assert_release_contents(release: &BuiltRelease, provisioned: &Provisioned) {
    let tree = &release.release_dir;
    let manifest =
        read_manifest_v2(tree, expected(&manifest_version(release))).expect("read manifest");
    let report = verify_tree_v2(tree, expected(&manifest.package_version))
        .expect("release tree must verify");
    assert_eq!(manifest.engine.name, ENGINE_NAME);
    assert_eq!(manifest.engine.version, ENGINE_VERSION);
    assert_eq!(
        manifest.engine.distribution_sha256,
        ENGINE_DISTRIBUTION_SHA256
    );
    assert_eq!(manifest.engine.distribution, ENGINE_DISTRIBUTION);
    assert_eq!(
        manifest.engine.distribution_integrity,
        ENGINE_DISTRIBUTION_INTEGRITY
    );
    assert_eq!(manifest.engine.distribution_size, ENGINE_DISTRIBUTION_SIZE);
    assert_eq!(
        manifest.engine.distribution_file_count,
        ENGINE_DISTRIBUTION_FILE_COUNT
    );
    assert_eq!(manifest.type_assets.len(), 2);
    let standard = manifest
        .type_assets
        .iter()
        .find(|asset| asset.name == STANDARD_LIB_ASSET)
        .expect("standard lib asset");
    assert_eq!(standard.file_count, STANDARD_LIB_FILE_COUNT);
    let typings = manifest
        .type_assets
        .iter()
        .find(|asset| asset.name == MINIPROGRAM_TYPINGS_ASSET)
        .expect("typings asset");
    assert_eq!(typings.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);
    let engine_entry = manifest
        .files
        .iter()
        .find(|file| file.path == ENGINE_BINARY_PATH)
        .expect("engine binary entry");
    assert_eq!(engine_entry.mode, "0755");
    assert_eq!(
        manifest
            .binaries
            .iter()
            .find(|binary| binary.path == "bin/wxml-lsp")
            .expect("lsp artifact")
            .sha256,
        sha256_file(&tree.join("bin/wxml-lsp")).expect("lsp binary sha256")
    );
    assert_eq!(report.file_count, manifest.files.len());

    for file in &manifest.files {
        assert!(
            forbidden_runtime_path(&file.path).is_none(),
            "release contains a Node runtime file: {}",
            file.path
        );
    }
    let contents = collect_tree(tree).expect("collect release tree");
    assert_no_node_payload(&contents.files);
    for required in [
        "engine/LICENSE",
        "engine/NOTICE.txt",
        "engine/package.json",
        "engine/lib/tsc",
        "engine/lib/tsc.sig",
        "typings/miniprogram-api-typings/LICENSE",
        "typings/miniprogram-api-typings/index.d.ts",
        "bin/wxml-lsp",
        "bin/wxml-lsp-install",
        "zed-extension/extension.wasm",
        "zed-extension/extension.toml",
        "THIRD_PARTY_LICENSES.md",
        "provenance.json",
        "manifest.json",
        "SHA256SUMS",
    ] {
        assert!(
            contents.files.contains(required),
            "release is missing `{required}`"
        );
    }

    assert_provenance(tree, provisioned);
    assert_checksums(tree, &manifest);
    println!(
        "RELEASE: version={} files={} bytes={} engine={} {} tree={}",
        manifest.package_version,
        report.file_count,
        report.total_bytes,
        manifest.engine.name,
        manifest.engine.version,
        tree_fingerprint(tree).expect("release fingerprint")
    );
}

fn manifest_version(release: &BuiltRelease) -> String {
    release
        .release_dir
        .file_name()
        .expect("release dir name")
        .to_string_lossy()
        .strip_prefix("wxml-lsp-")
        .and_then(|rest| rest.strip_suffix(&format!("-{NATIVE_PACKAGE_PLATFORM}")))
        .expect("release directory carries the package version")
        .to_owned()
}

fn assert_provenance(tree: &Path, provisioned: &Provisioned) {
    let value: Value =
        serde_json::from_slice(&fs::read(tree.join("provenance.json")).expect("read provenance"))
            .expect("parse provenance");
    assert_eq!(
        value["assetsLockSha256"].as_str(),
        Some(provisioned.lock_sha256.as_str())
    );
    let assets = value["assets"].as_array().expect("provenance assets");
    assert_eq!(assets.len(), provisioned.lock.assets.len());
    for asset in &provisioned.lock.assets {
        let entry = assets
            .iter()
            .find(|entry| entry["name"].as_str() == Some(asset.name.as_str()))
            .unwrap_or_else(|| panic!("provenance is missing `{}`", asset.name));
        assert_eq!(
            entry["payloadTreeDigest"].as_str(),
            Some(asset.payload.tree_digest.as_str())
        );
        assert_eq!(entry["fileCount"].as_u64(), Some(asset.payload.file_count));
        assert_eq!(
            entry["totalBytes"].as_u64(),
            Some(asset.payload.total_bytes)
        );
    }
    assert_eq!(value["engine"]["version"].as_str(), Some(ENGINE_VERSION));
    let engine_binary = &value["engine"]["binary"];
    assert_eq!(
        engine_binary["sha256"].as_str(),
        Some(
            sha256_file(&tree.join(ENGINE_BINARY_PATH))
                .unwrap()
                .as_str()
        )
    );
    assert_eq!(
        engine_binary["size"].as_u64(),
        Some(fs::metadata(tree.join(ENGINE_BINARY_PATH)).unwrap().len())
    );
    assert_eq!(engine_binary["mode"].as_str(), Some("0755"));
}

fn assert_checksums(tree: &Path, manifest: &wxml_delivery::manifest_v2::ManifestV2) {
    let sums = parse_sha256sums(&fs::read(tree.join("SHA256SUMS")).expect("read SHA256SUMS"))
        .expect("parse SHA256SUMS");
    let mut expected: std::collections::BTreeMap<String, String> = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect();
    expected.insert(
        "manifest.json".to_owned(),
        sha256_file(&tree.join("manifest.json")).expect("manifest sha256"),
    );
    assert_eq!(sums, expected);
}

/// Compare two builds byte for byte and print the evidence digests.
pub fn assert_byte_identical(
    first: &BuiltRelease,
    second: &BuiltRelease,
) -> (String, String, String, String) {
    assert_eq!(
        fs::read(&first.archive).expect("read first archive"),
        fs::read(&second.archive).expect("read second archive"),
        "archives differ between two builds"
    );
    assert_eq!(first.archive_sha256, second.archive_sha256);
    assert_eq!(first.archive_integrity, second.archive_integrity);
    assert_eq!(first.archive_size, second.archive_size);
    assert_eq!(first.manifest_sha256, second.manifest_sha256);
    assert_eq!(
        fs::read(first.release_dir.join("manifest.json")).unwrap(),
        fs::read(second.release_dir.join("manifest.json")).unwrap()
    );
    for name in ["SHA256SUMS", "provenance.json"] {
        assert_eq!(
            fs::read(first.release_dir.join(name)).unwrap(),
            fs::read(second.release_dir.join(name)).unwrap(),
            "{name} differs between two builds"
        );
    }
    for name in ["index.json", "SHA256SUMS"] {
        assert_eq!(
            fs::read(first.output.join(name)).unwrap(),
            fs::read(second.output.join(name)).unwrap(),
            "output {name} differs between two builds"
        );
    }
    assert_eq!(
        fs::read(&first.sidecar).unwrap(),
        fs::read(&second.sidecar).unwrap()
    );
    assert_eq!(
        tree_fingerprint(&first.release_dir).expect("first fingerprint"),
        tree_fingerprint(&second.release_dir).expect("second fingerprint")
    );
    let manifest_digest = first.manifest_sha256.clone();
    let index_digest = sha256_file(&first.index).expect("index sha256");
    let sums_digest = sha256_file(&first.sums).expect("sums sha256");
    println!(
        "DETERMINISM: archive={} manifest={} index={} sums={}",
        first.archive_sha256, manifest_digest, index_digest, sums_digest
    );
    (
        first.archive_sha256.clone(),
        manifest_digest,
        index_digest,
        sums_digest,
    )
}

/// The exact set of files and modes below an installed version directory.
pub fn installed_fingerprint(root: &Path, version: &str) -> String {
    let directory = root.join("versions").join(version);
    tree_fingerprint(&directory).expect("installed tree fingerprint")
}

pub fn installed_tree(root: &Path, version: &str) -> PathBuf {
    root.join("versions").join(version)
}

/// Runs a production binary with a PATH that contains only Node tripwire shims.
pub fn clean_command(program: &Path, tripwires: &Tripwires, work: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", tripwires.path())
        .env("HOME", work)
        .env("XDG_DATA_HOME", work)
        .env("TMPDIR", work)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .env("WXML_NODE", tripwires.path().join("node"))
        .env("NODE", tripwires.path().join("node"))
        .env("WXML_TYPE_BACKEND", "/nonexistent/poison-server.js")
        .env("WXML_BACKEND_DIR", "/nonexistent")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .current_dir(work)
        .stdin(Stdio::null());
    command
}

pub fn run_clean(
    program: &Path,
    args: &[&str],
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> Output {
    let mut command = clean_command(program, tripwires, work);
    command.args(args);
    run_with_timeout(&mut command, std::time::Duration::from_secs(120), label)
}

/// Proves the child environment has no Node on PATH and no injected Node vars.
pub fn assert_clean_environment_probe(tripwires: &Tripwires, work: &Path) {
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "printf 'PATH=%s\\nPWD=%s\\n' \"$PATH\" \"$PWD\"; \
             /usr/bin/readlink /proc/$$/exe; \
             /usr/bin/env | /usr/bin/sort",
        ])
        .env_clear()
        .env("PATH", tripwires.path())
        .env("HOME", work)
        .env("TMPDIR", work)
        .env("LANG", "C")
        .current_dir(work)
        .stdin(Stdio::null());
    let output = run_with_timeout(
        &mut command,
        std::time::Duration::from_secs(5),
        "clean-env probe",
    );
    assert!(
        output.status.success(),
        "clean-env probe failed: {:?}",
        output.status
    );
    let text = stdout(&output);
    assert!(
        text.lines()
            .any(|line| line == format!("PATH={}", tripwires.path().display())),
        "child PATH changed: {text}"
    );
    assert!(
        text.lines()
            .any(|line| line == format!("PWD={}", work.display())),
        "child cwd changed: {text}"
    );
    for forbidden in [
        "NODE_PATH=",
        "NPM_CONFIG_PREFIX=",
        "WXML_TYPE_BACKEND=/nonexistent",
    ] {
        assert!(
            !text.lines().any(|line| line.starts_with(forbidden)),
            "clean-env probe unexpectedly contains `{forbidden}`: {text}"
        );
    }
    println!(
        "CLEAN-ENV: PATH={} cwd={} /proc/self/exe={}",
        tripwires.path().display(),
        work.display(),
        text.lines().nth(2).unwrap_or("<missing>")
    );
}

/// Verifies an installed tree against its manifest and frozen engine identity.
pub fn assert_installed_version(root: &Path, version: &str, tripwires: &Tripwires) {
    let directory = installed_tree(root, version);
    let report = verify_tree_v2(&directory, expected(version)).expect("installed tree must verify");
    assert_eq!(report.manifest.package_version, version);
    assert_eq!(report.manifest.engine.version, ENGINE_VERSION);
    let contents = collect_tree(&directory).expect("collect installed tree");
    assert_no_node_payload(&contents.files);
    assert_eq!(
        sha256_file(&directory.join(ENGINE_BINARY_PATH)).expect("installed engine sha256"),
        report
            .manifest
            .files
            .iter()
            .find(|file| file.path == ENGINE_BINARY_PATH)
            .expect("engine file entry")
            .sha256
    );
    let engine = run_native_engine(&directory.join(ENGINE_BINARY_PATH));
    assert_eq!(
        engine,
        format!("{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\n").as_bytes()
    );
    tripwires.assert_not_executed();
}
