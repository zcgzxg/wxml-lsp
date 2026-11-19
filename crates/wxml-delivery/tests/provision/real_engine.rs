//! Frozen typings fixture and opt-in real-engine provisioning tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use wxml_delivery::assets_lock::AssetsLock;
use wxml_delivery::error::DeliveryError;
use wxml_delivery::provision::{SourceKind, plan, provision, provision_asset};

use crate::support::{
    Spec, TempDir, mode_of, npm_tar_gz, provision_options, sha256_hex, sha512_sri,
    tree_digest_pairs, valid_value, write_repo_lock_value,
};

const ENGINE_TARBALL_NAME: &str = "typescript-linux-x64-7.1.0-beta.tgz";
const TYPINGS_TARBALL_NAME: &str = "miniprogram-api-typings-5.3.1.tgz";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/miniprogram-api-typings-5.3.1")
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<(String, Vec<u8>)>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            collect_files(root, &path, output);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            output.push((relative, fs::read(&path).unwrap()));
        }
    }
}

fn typings_payload() -> Vec<(String, Vec<u8>)> {
    let root = fixture_root();
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        files.len(),
        23,
        "checked-in typings payload must stay 23 files"
    );
    files
}

/// The three excluded upstream files are synthetic but keep the frozen sizes.
fn typings_excluded() -> Vec<(String, Vec<u8>)> {
    vec![
        ("README-en.md".to_owned(), vec![b'a'; 2_839]),
        ("package.json".to_owned(), vec![b'{'; 1_077]),
        ("typings.json".to_owned(), vec![b't'; 61]),
    ]
}

fn build_typings_tarball(payload: &[(String, Vec<u8>)], excluded: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut specs = Vec::new();
    for (path, bytes) in payload.iter().chain(excluded) {
        specs.push(Spec::File(path, 0o644, bytes));
    }
    npm_tar_gz(&specs)
}

fn entry_value(path: &str, bytes: &[u8]) -> Value {
    json!({
        "path": path,
        "sha256": sha256_hex(bytes),
        "size": bytes.len(),
        "mode": "0644",
    })
}

/// Builds a lock whose typings tarball is synthetic but whose payload equals
/// the checked-in 23-file fixture and the frozen typings constants.
fn provision_lock() -> (Value, Vec<u8>) {
    let payload = typings_payload();
    let excluded = typings_excluded();
    let tarball = build_typings_tarball(&payload, &excluded);
    let mut upstream: Vec<Value> = payload
        .iter()
        .chain(&excluded)
        .map(|(path, bytes)| entry_value(path, bytes))
        .collect();
    upstream.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let unpacked: u64 = upstream
        .iter()
        .map(|file| file["size"].as_u64().unwrap())
        .sum();
    assert_eq!(unpacked, crate::fixtures::TYPINGS_UPSTREAM_BYTES);
    let rows: Vec<(String, String, String)> = upstream
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["mode"].as_str().unwrap().to_owned(),
                file["sha256"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let tree = tree_digest_pairs(&rows, "");

    let mut value = valid_value();
    let typings = value["assets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|asset| asset["name"] == crate::fixtures::TYPINGS_ASSET)
        .unwrap();
    typings["tarball"]["size"] = json!(tarball.len());
    typings["tarball"]["integrity"] = json!(sha512_sri(&tarball));
    typings["tarball"]["sha256"] = json!(sha256_hex(&tarball));
    typings["upstream"] = json!({
        "fileCount": upstream.len(),
        "unpackedSize": unpacked,
        "stripPrefix": "package/",
        "treeDigest": tree,
        "files": upstream,
    });
    (value, tarball)
}

fn write_lock(root: &Path, value: &Value) -> PathBuf {
    write_repo_lock_value(root, value)
}

fn validate_lock(value: &Value) -> (Vec<u8>, AssetsLock) {
    let bytes = serde_json::to_vec_pretty(value).unwrap();
    let lock = AssetsLock::parse(&bytes).unwrap();
    lock.validate().unwrap();
    (bytes, lock)
}

#[test]
fn frozen_typings_asset_provisions_and_hits_the_cache() {
    let (value, tarball) = provision_lock();
    let (lock_bytes, lock) = validate_lock(&value);
    let typings = lock.asset(crate::fixtures::TYPINGS_ASSET).unwrap().clone();
    assert_eq!(
        typings.payload.file_count,
        crate::fixtures::TYPINGS_PAYLOAD_COUNT
    );
    assert_eq!(
        typings.payload.tree_digest,
        crate::fixtures::TYPINGS_PAYLOAD_TREE_DIGEST
    );
    assert_eq!(
        typings.payload.bundles[0].sha256,
        crate::fixtures::TYPINGS_PAYLOAD_DIGEST
    );
    assert_eq!(
        u64::try_from(tarball.len()).unwrap(),
        typings.tarball.size,
        "the synthetic tarball identity is the lock identity"
    );

    let temp = TempDir::new("typings-only");
    let source = temp.path().join("from-dir");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join(TYPINGS_TARBALL_NAME), &tarball).unwrap();
    let cache = temp.path().join("cache");
    let lock_sha = lock.lock_sha256().unwrap();
    let entry = provision_asset(&typings, &cache, Some(&source), &lock_sha, &lock_bytes).unwrap();
    assert_eq!(entry.source, SourceKind::FromDir);
    assert_eq!(entry.file_count, crate::fixtures::TYPINGS_PAYLOAD_COUNT);
    assert_eq!(entry.total_bytes, crate::fixtures::TYPINGS_PAYLOAD_BYTES);
    assert_eq!(entry.payload_tree_digest, typings.payload.tree_digest);
    assert!(
        entry
            .directory
            .join("payload/typings/miniprogram-api-typings/LICENSE")
            .is_file()
    );

    let hit = provision_asset(&typings, &cache, None, &lock_sha, &lock_bytes).unwrap();
    assert_eq!(hit.source, SourceKind::Cache);
    assert_eq!(hit.directory, entry.directory);
}

#[test]
fn synthetic_typings_tarball_does_not_match_the_frozen_payload_tree() {
    let (value, tarball) = provision_lock();
    let (lock_bytes, lock) = validate_lock(&value);
    let typings = lock.asset(crate::fixtures::TYPINGS_ASSET).unwrap().clone();
    let temp = TempDir::new("typings-drift");
    let source = temp.path().join("from-dir");
    fs::create_dir_all(&source).unwrap();
    let mut tampered = tarball;
    tampered[10] ^= 0xff;
    fs::write(source.join(TYPINGS_TARBALL_NAME), &tampered).unwrap();
    let error = provision_asset(
        &typings,
        &temp.path().join("cache"),
        Some(&source),
        &lock.lock_sha256().unwrap(),
        &lock_bytes,
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    let assets = temp.path().join("cache/assets");
    assert_eq!(fs::read_dir(&assets).map_or(0, Iterator::count), 0);
}

fn real_engine_tarball() -> Option<PathBuf> {
    let path = std::env::var_os("TSGO_NATIVE_TARBALL").map_or_else(
        || PathBuf::from("/tmp/opencode/tsgo-spike/download/typescript-linux-x64-7.1.0-beta.tgz"),
        PathBuf::from,
    );
    path.is_file().then_some(path)
}

fn require_or_skip() -> Option<PathBuf> {
    if let Some(path) = real_engine_tarball() {
        return Some(path);
    }
    let required = std::env::var("WXML_REQUIRE_REAL_ENGINE")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    assert!(
        !required,
        "WXML_REQUIRE_REAL_ENGINE is set but no engine tarball was found; set TSGO_NATIVE_TARBALL"
    );
    eprintln!(
        "skipping real-engine provisioning test: set TSGO_NATIVE_TARBALL or place {ENGINE_TARBALL_NAME} in the spike download directory"
    );
    None
}

#[test]
fn real_engine_provisions_and_full_run_uses_cache() {
    let Some(engine_tarball) = require_or_skip() else {
        return;
    };
    let (value, typings_tarball) = provision_lock();
    let (_, lock) = validate_lock(&value);
    let temp = TempDir::new("real-engine");
    let lock_path = write_lock(temp.path(), &value);
    let from_dir = temp.path().join("from-dir");
    fs::create_dir_all(&from_dir).unwrap();
    fs::copy(&engine_tarball, from_dir.join(ENGINE_TARBALL_NAME)).unwrap();
    fs::write(from_dir.join(TYPINGS_TARBALL_NAME), &typings_tarball).unwrap();

    let cache = temp.path().join("cache");
    let mut options = provision_options(&lock_path, &cache);
    options.from_dir = Some(from_dir.clone());
    options.offline = true;
    let report = provision(&options).unwrap();
    assert_eq!(report.lock_sha256, lock.lock_sha256().unwrap());
    assert_eq!(report.entries.len(), 2);
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.source == SourceKind::FromDir)
    );
    let engine = report
        .entries
        .iter()
        .find(|entry| entry.name == crate::fixtures::ENGINE_ASSET)
        .unwrap();
    assert_eq!(engine.file_count, crate::fixtures::ENGINE_FILE_COUNT);
    assert_eq!(engine.total_bytes, crate::fixtures::ENGINE_UNPACKED_SIZE);
    let engine_value = value["assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|asset| asset["name"] == crate::fixtures::ENGINE_ASSET)
        .unwrap();
    assert_eq!(
        engine.payload_tree_digest,
        engine_value["payload"]["treeDigest"].as_str().unwrap()
    );

    let cache_plan = plan(&options).unwrap();
    assert!(cache_plan.assets.iter().all(|asset| asset.cache_hit));
    assert!(
        cache_plan
            .assets
            .iter()
            .all(|asset| asset.source == Some(SourceKind::Cache))
    );

    let mut missing_source = options.clone();
    missing_source.from_dir = Some(temp.path().join("deleted-from-dir"));
    let hit_plan = plan(&missing_source).unwrap();
    assert!(hit_plan.assets.iter().all(|asset| asset.cache_hit));
    let all_hit = provision(&missing_source).unwrap();
    assert!(
        all_hit
            .entries
            .iter()
            .all(|entry| entry.source == SourceKind::Cache)
    );

    let mut cached_only = options;
    cached_only.from_dir = None;
    let cached = provision(&cached_only).unwrap();
    assert_eq!(cached.entries.len(), 2);
    assert!(
        cached
            .entries
            .iter()
            .all(|entry| entry.source == SourceKind::Cache)
    );
    assert_eq!(cached.lock_sha256, report.lock_sha256);
    let entries = cache.join("assets");
    assert_eq!(fs::read_dir(entries).unwrap().count(), 2);

    assert_cli_reports(&lock_path, &cache);
    archive_cache_rebuilds_payload(
        &lock_path,
        &cache,
        &from_dir,
        engine_value["payload"]["treeDigest"].as_str().unwrap(),
    );
    assert_eq!(lock.assets.len(), 2);
}

/// P8a: the verified archive cache alone must rebuild both payloads after the
/// payload cache and the whole source directory are deleted.
fn archive_cache_rebuilds_payload(
    lock_path: &Path,
    cache: &Path,
    from_dir: &Path,
    engine_tree_digest: &str,
) {
    assert_eq!(mode_of(&cache.join("archives")), 0o700);
    let engine_archive = cache
        .join("archives")
        .join(crate::fixtures::ENGINE_ASSET)
        .join(format!(
            "{}-{}",
            crate::fixtures::ENGINE_SHA256,
            crate::fixtures::ENGINE_TARBALL_SIZE
        ));
    assert_eq!(mode_of(&engine_archive), 0o700);
    assert_eq!(mode_of(&engine_archive.join("archive.tgz")), 0o600);
    fs::remove_dir_all(cache.join("assets")).unwrap();
    fs::remove_dir_all(from_dir).unwrap();

    let mut archive_only = provision_options(lock_path, cache);
    archive_only.from_dir = None;
    let archive_plan = plan(&archive_only).unwrap();
    assert!(archive_plan.assets.iter().all(|asset| !asset.cache_hit));
    assert!(
        archive_plan
            .assets
            .iter()
            .all(|asset| asset.source == Some(SourceKind::ArchiveCache))
    );
    let rebuilt = provision(&archive_only).unwrap();
    assert_eq!(rebuilt.entries.len(), 2);
    assert!(
        rebuilt
            .entries
            .iter()
            .all(|entry| entry.source == SourceKind::ArchiveCache)
    );
    let rebuilt_engine = rebuilt
        .entries
        .iter()
        .find(|entry| entry.name == crate::fixtures::ENGINE_ASSET)
        .unwrap();
    assert_eq!(
        rebuilt_engine.file_count,
        crate::fixtures::ENGINE_FILE_COUNT
    );
    assert_eq!(
        rebuilt_engine.total_bytes,
        crate::fixtures::ENGINE_UNPACKED_SIZE
    );
    assert_eq!(rebuilt_engine.payload_tree_digest, engine_tree_digest);
    assert!(cache.join("assets").is_dir());
}

fn assert_cli_reports(lock_path: &Path, cache: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
        .arg("provision")
        .arg("--lock")
        .arg(lock_path)
        .arg("--cache")
        .arg(cache)
        .arg("--print-plan")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cache hit"), "{stdout}");
    assert!(stdout.contains("source cache"), "{stdout}");

    let output = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
        .arg("provision")
        .arg("--lock")
        .arg(lock_path)
        .arg("--cache")
        .arg(cache)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("source cache"), "{stdout}");
    let cache_text = cache.to_string_lossy().into_owned();
    assert!(
        !stdout.contains(&cache_text),
        "successful reports must not print absolute cache paths: {stdout}"
    );
}
