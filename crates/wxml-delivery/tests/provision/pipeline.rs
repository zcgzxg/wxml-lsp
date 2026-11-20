//! Provisioning pipeline, cache re-verification and concurrency tests.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use wxml_delivery::assets_lock::Asset;
use wxml_delivery::error::DeliveryError;
use wxml_delivery::manifest::sha256_file;
use wxml_delivery::provision::{CacheEntry, SourceKind, provision_asset};

use crate::support::{
    EXCLUDED_FILES, GOLDEN_BUNDLE, GOLDEN_PAYLOAD_TREE, GOLDEN_UPSTREAM_TREE, PAYLOAD_FILES,
    TEST_CACHE_KEY, TEST_LOCK_SHA, TempDir, mode_of, mode_of_full, sha256_hex, synthetic_asset,
    synthetic_asset_value, synthetic_tarball, write_file,
};

const SUBMITTED_LOCK: &[u8] = b"submitted lock bytes\n";

fn private_temp(label: &str) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new(label);
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

struct Fixture {
    source: TempDir,
    asset: Asset,
    tarball: Vec<u8>,
}

fn fixture() -> Fixture {
    let tarball = synthetic_tarball();
    let asset = synthetic_asset(&tarball);
    let source = private_temp("pipeline-source");
    fs::write(source.path().join("synthetic-asset-1.0.0.tgz"), &tarball).unwrap();
    Fixture {
        source,
        asset,
        tarball,
    }
}

fn provision(fixture: &Fixture, cache: &Path) -> CacheEntry {
    provision_asset(
        &fixture.asset,
        cache,
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap()
}

#[test]
fn from_dir_builds_the_golden_payload_and_provenance() {
    let fixture = fixture();
    let cache = private_temp("pipeline-create");
    let entry = provision(&fixture, cache.path());
    assert_eq!(entry.source, SourceKind::FromDir);
    assert_eq!(entry.cache_key, TEST_CACHE_KEY);
    assert_eq!(entry.payload_tree_digest, GOLDEN_PAYLOAD_TREE);
    assert_eq!(entry.file_count, 6);
    assert_eq!(entry.total_bytes, 159);
    assert!(entry.directory.ends_with(TEST_CACHE_KEY));
    assert_eq!(
        entry
            .directory
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "synthetic-asset"
    );

    let payload = entry.directory.join("payload/assets/synthetic");
    assert_eq!(mode_of(&payload), 0o755);
    assert_eq!(mode_of(&payload.join("bin")), 0o755);
    assert_eq!(mode_of(&payload.join("bin/tool")), 0o755);
    assert_eq!(mode_of(&payload.join("index.d.ts")), 0o644);
    assert_eq!(
        fs::read(payload.join("index.d.ts")).unwrap(),
        PAYLOAD_FILES[0].1
    );
    assert!(!payload.join("package.json").exists());
    assert!(!payload.join("README-en.md").exists());
    assert!(!payload.join("typings.json").exists());

    let provenance: Value =
        serde_json::from_slice(&fs::read(entry.directory.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(provenance["schemaVersion"], json!(1));
    assert_eq!(provenance["lockSha256"], json!(TEST_LOCK_SHA));
    assert_eq!(provenance["assetName"], json!("synthetic-asset"));
    assert_eq!(
        provenance["tarball"]["url"],
        json!("https://registry.npmjs.org/synthetic-asset/-/synthetic-asset-1.0.0.tgz")
    );
    assert_eq!(provenance["tarball"]["size"], json!(fixture.tarball.len()));
    assert_eq!(
        provenance["tarball"]["sha256"],
        json!(sha256_hex(&fixture.tarball))
    );
    assert_eq!(provenance["payloadTreeDigest"], json!(GOLDEN_PAYLOAD_TREE));
    assert_eq!(provenance["fileCount"], json!(6));
    assert_eq!(provenance["totalBytes"], json!(159));
    assert_eq!(provenance["sourceKind"], json!("from-dir"));
    assert!(provenance.get("downloadedAt").is_none());
    assert!(
        provenance["toolVersion"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
    );
    assert_eq!(mode_of(&cache.path().join("assets")), 0o700);
    assert_eq!(mode_of(&cache.path().join("tmp")), 0o700);
    assert_eq!(mode_of(&entry.directory), 0o700);

    assert_eq!(
        fs::read(entry.directory.join("lock.json")).unwrap(),
        b"submitted lock bytes\n"
    );
    let mut expected = BTreeMap::new();
    for (path, bytes) in PAYLOAD_FILES {
        expected.insert(format!("assets/synthetic/{path}"), sha256_hex(bytes));
    }
    let mut expected_text = String::new();
    for (path, digest) in &expected {
        let _ = writeln!(expected_text, "{digest}  {path}");
    }
    assert_eq!(
        String::from_utf8(fs::read(entry.directory.join("SHA256SUMS")).unwrap()).unwrap(),
        expected_text
    );

    let parsed: Value = serde_json::from_value(synthetic_asset_value(&fixture.tarball)).unwrap();
    assert_eq!(parsed["payload"]["treeDigest"], json!(GOLDEN_PAYLOAD_TREE));
    assert_eq!(
        parsed["upstream"]["treeDigest"],
        json!(GOLDEN_UPSTREAM_TREE)
    );
    assert_eq!(
        parsed["payload"]["bundles"][0]["sha256"],
        json!(GOLDEN_BUNDLE)
    );
    assert_eq!(EXCLUDED_FILES.len(), 3);
}

#[test]
fn a_cache_hit_is_reverified_and_never_rewritten() {
    let fixture = fixture();
    let cache = private_temp("pipeline-hit");
    let first = provision(&fixture, cache.path());
    let provenance_before = fs::read(first.directory.join("provenance.json")).unwrap();
    let sums_before = fs::read(first.directory.join("SHA256SUMS")).unwrap();

    let hit = provision_asset(
        &fixture.asset,
        cache.path(),
        Some(&fixture.source.path().join("missing-from-dir")),
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap();
    assert_eq!(hit.source, SourceKind::Cache);
    assert_eq!(hit.directory, first.directory);
    assert_eq!(hit.payload_tree_digest, GOLDEN_PAYLOAD_TREE);
    assert_eq!(
        fs::read(first.directory.join("provenance.json")).unwrap(),
        provenance_before
    );
    assert_eq!(
        fs::read(first.directory.join("SHA256SUMS")).unwrap(),
        sums_before
    );

    let error = provision_asset(
        &fixture.asset,
        cache.path(),
        None,
        TEST_LOCK_SHA,
        b"different submitted bytes are rejected on a hit\n",
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert!(error.to_string().contains("lock copy"), "{error}");
    assert_eq!(
        fs::read(first.directory.join("provenance.json")).unwrap(),
        provenance_before
    );

    let empty_source = private_temp("pipeline-empty-source");
    let hit = provision_asset(
        &fixture.asset,
        cache.path(),
        Some(empty_source.path()),
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap();
    assert_eq!(hit.source, SourceKind::Cache);
}

fn expect_cache_failure(needle: &str, mutate: impl FnOnce(&Path)) -> (TempDir, PathBuf) {
    let fixture = fixture();
    let cache = private_temp("pipeline-tamper");
    let entry = provision(&fixture, cache.path());
    mutate(&entry.directory);
    let error = provision_asset(
        &fixture.asset,
        cache.path(),
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains(needle),
        "expected `{needle}` in `{error}`"
    );
    let assets = cache.path().join("assets/synthetic-asset");
    assert_eq!(
        fs::read_dir(&assets).unwrap().count(),
        1,
        "a failed re-verification must not create a second entry"
    );
    (cache, entry.directory)
}

#[test]
fn cache_tampering_fails_closed_without_repair() {
    let (_cache, entry) = expect_cache_failure("does not match", |entry| {
        let path = entry.join("payload/assets/synthetic/index.d.ts");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
    });
    let tampered = fs::read(entry.join("payload/assets/synthetic/index.d.ts")).unwrap();
    assert_ne!(tampered, PAYLOAD_FILES[0].1);

    let (_cache, entry) = expect_cache_failure("file set", |entry| {
        fs::remove_file(entry.join("payload/assets/synthetic/README.md")).unwrap();
    });
    assert!(!entry.join("payload/assets/synthetic/README.md").exists());

    let (_cache, entry) = expect_cache_failure("directory set", |entry| {
        write_file(
            &entry.join("payload/assets/synthetic/extra/extra.d.ts"),
            b"extra",
        );
    });
    assert!(
        entry
            .join("payload/assets/synthetic/extra/extra.d.ts")
            .exists()
    );

    let (_cache, entry) = expect_cache_failure("does not match", |entry| {
        use std::os::unix::fs::PermissionsExt;
        let path = entry.join("payload/assets/synthetic/index.d.ts");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    });
    assert_eq!(
        mode_of(&entry.join("payload/assets/synthetic/index.d.ts")),
        0o755
    );

    let (_cache, entry) = expect_cache_failure("setuid", |entry| {
        use std::os::unix::fs::PermissionsExt;
        let path = entry.join("payload/assets/synthetic/index.d.ts");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o4755)).unwrap();
    });
    assert_eq!(
        mode_of_full(&entry.join("payload/assets/synthetic/index.d.ts")),
        0o4755
    );

    let (_cache, entry) = expect_cache_failure("setuid", |entry| {
        use std::os::unix::fs::PermissionsExt;
        let path = entry.join("payload/assets/synthetic/types");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o2755)).unwrap();
    });
    assert_eq!(
        mode_of_full(&entry.join("payload/assets/synthetic/types")),
        0o2755
    );

    let (_cache, entry) = expect_cache_failure("world-writable", |entry| {
        use std::os::unix::fs::PermissionsExt;
        let path = entry.join("payload/assets/synthetic/index.d.ts");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    });
    assert_eq!(
        mode_of(&entry.join("payload/assets/synthetic/index.d.ts")),
        0o666
    );

    let (_cache, entry) = expect_cache_failure("symlink", |entry| {
        let path = entry.join("payload/assets/synthetic/index.d.ts");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("LICENSE", &path).unwrap();
    });
    assert!(
        fs::symlink_metadata(entry.join("payload/assets/synthetic/index.d.ts"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn cache_metadata_tampering_and_foreign_locks_fail_closed() {
    let (_cache, entry) = expect_cache_failure("different lock", |entry| {
        rewrite_json(&entry.join("provenance.json"), |value| {
            value["lockSha256"] = json!("b".repeat(64));
        });
    });
    let value: Value =
        serde_json::from_slice(&fs::read(entry.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(value["lockSha256"], json!("b".repeat(64)));

    let (_cache, entry) = expect_cache_failure("does not match", |entry| {
        rewrite_json(&entry.join("provenance.json"), |value| {
            value["tarball"]["sha256"] = json!("0".repeat(64));
        });
    });
    let value: Value =
        serde_json::from_slice(&fs::read(entry.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(value["tarball"]["sha256"], json!("0".repeat(64)));

    let (_cache, entry) = expect_cache_failure("incomplete or unsafe", |entry| {
        fs::remove_file(entry.join("provenance.json")).unwrap();
    });
    assert!(!entry.join("provenance.json").exists());
    assert!(entry.join("payload/assets/synthetic/index.d.ts").exists());

    let (_cache, entry) = expect_cache_failure("SHA256SUMS", |entry| {
        fs::write(entry.join("SHA256SUMS"), b"not a checksum file\n").unwrap();
    });
    assert_eq!(
        fs::read(entry.join("SHA256SUMS")).unwrap(),
        b"not a checksum file\n"
    );

    let (_cache, entry) = expect_cache_failure("lock copy", |entry| {
        fs::remove_file(entry.join("lock.json")).unwrap();
    });
    assert!(!entry.join("lock.json").exists());

    let (_cache, entry) = expect_cache_failure("lock copy", |entry| {
        fs::write(entry.join("lock.json"), b"rewritten lock bytes\n").unwrap();
    });
    assert_eq!(
        fs::read(entry.join("lock.json")).unwrap(),
        b"rewritten lock bytes\n"
    );
}

fn rewrite_json(path: &Path, mutate: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(&mut value);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn concurrent_provisioning_of_the_same_payload_creates_one_entry() {
    let fixture = fixture();
    let cache = private_temp("pipeline-concurrent");
    let mut handles = Vec::new();
    for _ in 0..4 {
        let asset = fixture.asset.clone();
        let from_dir = fixture.source.path().to_path_buf();
        let cache_root = cache.path().to_path_buf();
        handles.push(std::thread::spawn(move || {
            provision_asset(&asset, &cache_root, Some(&from_dir), TEST_LOCK_SHA, b"x")
        }));
    }
    let mut entries = Vec::new();
    for handle in handles {
        entries.push(handle.join().unwrap().unwrap());
    }
    let first = &entries[0];
    for entry in &entries {
        assert_eq!(entry.directory, first.directory);
        assert_eq!(entry.payload_tree_digest, GOLDEN_PAYLOAD_TREE);
    }
    let assets = cache.path().join("assets/synthetic-asset");
    assert_eq!(fs::read_dir(&assets).unwrap().count(), 1);
    let entries = fs::read_dir(&assets)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries[0].file_name().to_str().unwrap(), TEST_CACHE_KEY);
    assert_eq!(
        sha256_file(&first.directory.join("SHA256SUMS"))
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn wrong_declared_size_sri_and_sha_are_rejected_before_extraction() {
    let fixture = fixture();
    let cache = private_temp("pipeline-before-extract");

    let mut asset = fixture.asset.clone();
    asset.tarball.size += 1;
    let error = provision_asset(
        &asset,
        cache.path(),
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert!(error.to_string().contains("size"), "{error}");

    let mut asset = fixture.asset.clone();
    asset.tarball.sha256 = "0".repeat(64);
    let error = provision_asset(
        &asset,
        cache.path(),
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert!(error.to_string().contains("SHA-256"), "{error}");

    let mut asset = fixture.asset.clone();
    asset.tarball.integrity = crate::support::sha512_sri(b"another asset");
    let error = provision_asset(
        &asset,
        cache.path(),
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    assert!(error.to_string().contains("SHA-512"), "{error}");

    assert!(!cache.path().join("assets/synthetic-asset").exists());
    assert_eq!(
        fs::read_dir(cache.path().join("tmp")).unwrap().count(),
        0,
        "tarball verification failure must not leave an extracted tree"
    );
}

#[test]
fn payload_tree_digest_and_bundle_mismatches_fail_before_publish() {
    let tarball = synthetic_tarball();
    let source = private_temp("pipeline-tables-source");
    fs::write(source.path().join("synthetic-asset-1.0.0.tgz"), &tarball).unwrap();

    let mut value = synthetic_asset_value(&tarball);
    value["payload"]["treeDigest"] = json!("f".repeat(64));
    let asset: Asset = serde_json::from_value(value).unwrap();
    let cache = private_temp("pipeline-tree-cache");
    let error = provision_asset(
        &asset,
        cache.path(),
        Some(source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap_err();
    assert!(error.to_string().contains("tree digest"), "{error}");
    assert!(!cache.path().join("assets/synthetic-asset").exists());

    let mut value = synthetic_asset_value(&tarball);
    value["payload"]["bundles"][0]["sha256"] = json!("f".repeat(64));
    let asset: Asset = serde_json::from_value(value).unwrap();
    let error = provision_asset(
        &asset,
        cache.path(),
        Some(source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap_err();
    assert!(error.to_string().contains("bundle"), "{error}");
    assert!(!cache.path().join("assets/synthetic-asset").exists());
}

#[test]
fn provenance_payload_digest_tampering_fails_closed() {
    let (_cache, entry) = expect_cache_failure("does not match", |entry| {
        rewrite_json(&entry.join("provenance.json"), |value| {
            value["payloadTreeDigest"] = json!("f".repeat(64));
        });
    });
    let value: Value =
        serde_json::from_slice(&fs::read(entry.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(value["payloadTreeDigest"], json!("f".repeat(64)));
}

#[test]
fn cache_keys_bind_both_payload_and_lock_identity() {
    let fixture = fixture();
    let cache = private_temp("pipeline-keys");
    let first = provision_asset(
        &fixture.asset,
        cache.path(),
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap();
    let other_lock = "b".repeat(64);
    let second = provision_asset(
        &fixture.asset,
        cache.path(),
        Some(fixture.source.path()),
        &other_lock,
        b"y",
    )
    .unwrap();
    assert_ne!(first.cache_key, second.cache_key);
    assert_eq!(first.payload_tree_digest, second.payload_tree_digest);
    assert_eq!(
        first.cache_key,
        format!("{GOLDEN_PAYLOAD_TREE}.aaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        second.cache_key,
        format!("{GOLDEN_PAYLOAD_TREE}.bbbbbbbbbbbbbbbb")
    );
    assert_eq!(
        fs::read_dir(cache.path().join("assets/synthetic-asset"))
            .unwrap()
            .count(),
        2
    );
    let displaced = cache
        .path()
        .join("assets/synthetic-asset")
        .join(format!("{GOLDEN_PAYLOAD_TREE}.cccccccccccccccc"));
    fs::rename(&second.directory, &displaced).unwrap();
    copy_tree(&first.directory, &second.directory);
    let error = provision_asset(&fixture.asset, cache.path(), None, &other_lock, b"z").unwrap_err();
    assert!(matches!(error, DeliveryError::InstallState(_)), "{error:?}");
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn unclassified_or_mismatched_payload_tables_fail_before_publish() {
    let (source, tarball) = (private_temp("pipeline-classify"), synthetic_tarball());
    let source_dir = source.path().to_path_buf();
    fs::write(source_dir.join("synthetic-asset-1.0.0.tgz"), &tarball).unwrap();

    let mut value = synthetic_asset_value(&tarball);
    value["extract"]["include"] = json!([
        "index.d.ts",
        "types/**",
        "LICENSE",
        "README.md",
        "CHANGELOG.md"
    ]);
    let asset: Asset = serde_json::from_value(value).unwrap();
    let cache = private_temp("pipeline-classify-cache");
    let error =
        provision_asset(&asset, cache.path(), Some(&source_dir), TEST_LOCK_SHA, b"x").unwrap_err();
    assert!(error.to_string().contains("does not classify"), "{error}");
    assert!(!cache.path().join("assets/synthetic-asset").exists());

    let mut value = synthetic_asset_value(&tarball);
    value["extract"]["exclude"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path": "index.d.ts", "reason": "conflict"}));
    let asset: Asset = serde_json::from_value(value).unwrap();
    let error =
        provision_asset(&asset, cache.path(), Some(&source_dir), TEST_LOCK_SHA, b"x").unwrap_err();
    assert!(
        error.to_string().contains("both included and excluded"),
        "{error}"
    );

    let mut value = synthetic_asset_value(&tarball);
    value["payload"]["files"].as_array_mut().unwrap().pop();
    let asset: Asset = serde_json::from_value(value).unwrap();
    let error =
        provision_asset(&asset, cache.path(), Some(&source_dir), TEST_LOCK_SHA, b"x").unwrap_err();
    assert!(error.to_string().contains("payload file set"), "{error}");
    assert!(!cache.path().join("assets/synthetic-asset").exists());
}

#[test]
fn payload_paths_cannot_escape_the_cache() {
    let (source, tarball) = (private_temp("pipeline-escape"), synthetic_tarball());
    let source_dir = source.path().to_path_buf();
    fs::write(source_dir.join("synthetic-asset-1.0.0.tgz"), &tarball).unwrap();
    let mut value = synthetic_asset_value(&tarball);
    let files = value["payload"]["files"].as_array_mut().unwrap();
    let mut escaped = files[0].clone();
    escaped["path"] = json!("assets/synthetic/../../escape.d.ts");
    files[0] = escaped;
    let asset: Asset = serde_json::from_value(value).unwrap();
    let cache = private_temp("pipeline-escape-cache");
    let error =
        provision_asset(&asset, cache.path(), Some(&source_dir), TEST_LOCK_SHA, b"x").unwrap_err();
    assert!(
        matches!(error, DeliveryError::UnsafeArchive(_)),
        "{error:?}"
    );
    assert!(!cache.path().join("escape.d.ts").exists());
    assert!(!cache.path().join("assets").join("escape.d.ts").exists());
}

#[test]
fn wrong_tarball_bytes_leave_no_cache_entry() {
    let fixture = fixture();
    let cache = private_temp("pipeline-bad-bytes");
    let bad_source = private_temp("pipeline-bad-source");
    let mut bad = fixture.tarball.clone();
    bad[0] ^= 0xff;
    fs::write(bad_source.path().join("synthetic-asset-1.0.0.tgz"), &bad).unwrap();
    let error = provision_asset(
        &fixture.asset,
        cache.path(),
        Some(bad_source.path()),
        TEST_LOCK_SHA,
        b"x",
    )
    .unwrap_err();
    assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    let assets = cache.path().join("assets");
    assert_eq!(fs::read_dir(&assets).unwrap().count(), 0);
    assert_eq!(
        fs::read_dir(cache.path().join("tmp")).unwrap().count(),
        0,
        "failed provisioning must clean the temporary workspace"
    );
}
