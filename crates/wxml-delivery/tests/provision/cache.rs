//! Offline archive-cache tests (F3-12 provisioning slice).
//!
//! These fixtures are synthetic npm tarballs. They exercise the content
//! addressed `archives/<asset>/<sha256>-<size>` cache: seeding from
//! `--from-dir`, payload reassembly with the source deleted, fail-closed
//! tampering, concurrent publication and CLI path redaction. No test downloads.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wxml_delivery::assets_lock::Asset;
use wxml_delivery::error::DeliveryError;
use wxml_delivery::provision::{SourceKind, provision_asset};

use crate::support::{
    GOLDEN_PAYLOAD_TREE, PAYLOAD_FILES, TempDir, mode_of, sha256_hex, synthetic_asset,
    synthetic_tarball, write_repo_lock,
};

const SUBMITTED_LOCK: &[u8] = b"submitted lock bytes\n";

struct Fixture {
    source: TempDir,
    asset: Asset,
    tarball: Vec<u8>,
}

fn private_temp(label: &str) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new(label);
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

fn fixture(label: &str) -> Fixture {
    let tarball = synthetic_tarball();
    let asset = synthetic_asset(&tarball);
    let source = private_temp(label);
    fs::write(source.path().join("synthetic-asset-1.0.0.tgz"), &tarball).unwrap();
    Fixture {
        source,
        asset,
        tarball,
    }
}

fn seed(fixture: &Fixture, cache: &Path) {
    let entry = provision_asset(
        &fixture.asset,
        cache,
        Some(fixture.source.path()),
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap();
    assert_eq!(entry.source, SourceKind::FromDir);
}

const TEST_LOCK_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn archive_entry(cache: &Path, fixture: &Fixture) -> PathBuf {
    cache.join("archives/synthetic-asset").join(format!(
        "{}-{}",
        sha256_hex(&fixture.tarball),
        fixture.tarball.len()
    ))
}

fn reload(fixture: &Fixture, cache: &Path) -> wxml_delivery::provision::CacheEntry {
    provision_asset(&fixture.asset, cache, None, TEST_LOCK_SHA, SUBMITTED_LOCK).unwrap()
}

#[test]
fn cold_archive_cache_without_from_dir_fails_closed() {
    let fixture = fixture("cache-cold");
    let cache = private_temp("cache-cold-store");
    let error = provision_asset(
        &fixture.asset,
        cache.path(),
        None,
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidArgument(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("--from-dir"), "{error}");
    assert!(error.to_string().contains("archive cache"), "{error}");
    assert_eq!(
        fs::read_dir(cache.path().join("assets")).unwrap().count(),
        0
    );
    assert!(!cache.path().join("archives").exists());
    assert_eq!(fs::read_dir(cache.path().join("tmp")).unwrap().count(), 0);
}

#[test]
fn verified_tarballs_enter_the_content_addressed_archive_cache() {
    let fixture = fixture("cache-seed");
    let cache = private_temp("cache-seed-store");
    seed(&fixture, cache.path());

    assert_eq!(mode_of(&cache.path().join("archives")), 0o700);
    let entry = archive_entry(cache.path(), &fixture);
    assert_eq!(mode_of(&entry), 0o700);
    assert_eq!(mode_of(&entry.join("archive.tgz")), 0o600);
    assert_eq!(
        fs::read(entry.join("archive.tgz")).unwrap(),
        fixture.tarball
    );

    let metadata: Value =
        serde_json::from_slice(&fs::read(entry.join("archive.json")).unwrap()).unwrap();
    assert_eq!(metadata["schemaVersion"], 1);
    assert_eq!(metadata["assetName"], "synthetic-asset");
    assert_eq!(metadata["assetVersion"], "1.0.0");
    assert_eq!(
        metadata["url"],
        "https://registry.npmjs.org/synthetic-asset/-/synthetic-asset-1.0.0.tgz"
    );
    assert_eq!(metadata["sha256"], sha256_hex(&fixture.tarball));
    assert_eq!(metadata["size"], fixture.tarball.len());
    assert_eq!(
        metadata["integrity"],
        crate::support::sha512_sri(&fixture.tarball)
    );
    assert_eq!(metadata["archive"], "archive.tgz");

    // The payload entry still exists next to the archive cache.
    assert!(cache.path().join("assets/synthetic-asset").is_dir());
}

#[test]
fn archive_cache_reassembles_payload_after_source_removal() {
    let fixture = fixture("cache-rebuild");
    let cache = private_temp("cache-rebuild-store");
    seed(&fixture, cache.path());
    fs::remove_dir_all(cache.path().join("assets")).unwrap();
    fs::remove_dir_all(fixture.source.path()).unwrap();

    let entry = reload(&fixture, cache.path());
    assert_eq!(entry.source, SourceKind::ArchiveCache);
    assert_eq!(entry.payload_tree_digest, GOLDEN_PAYLOAD_TREE);
    assert_eq!(entry.file_count, 6);
    let payload = entry.directory.join("payload/assets/synthetic");
    assert_eq!(mode_of(&payload.join("bin/tool")), 0o755);
    assert_eq!(
        fs::read(payload.join("index.d.ts")).unwrap(),
        PAYLOAD_FILES[0].1
    );
    let provenance: Value =
        serde_json::from_slice(&fs::read(entry.directory.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(provenance["sourceKind"], "archive-cache");

    // The rebuilt payload entry is a normal cache hit from now on.
    let hit = reload(&fixture, cache.path());
    assert_eq!(hit.source, SourceKind::Cache);
    assert_eq!(hit.directory, entry.directory);
}

fn expect_archive_failure(needle: &str, mutate: impl FnOnce(&Path)) {
    let fixture = fixture("cache-tamper");
    let cache = private_temp("cache-tamper-store");
    seed(&fixture, cache.path());
    fs::remove_dir_all(cache.path().join("assets")).unwrap();
    let entry = archive_entry(cache.path(), &fixture);
    mutate(&entry);
    let error = provision_asset(
        &fixture.asset,
        cache.path(),
        None,
        TEST_LOCK_SHA,
        SUBMITTED_LOCK,
    )
    .unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains(needle),
        "expected `{needle}` in `{error}`"
    );
    assert!(
        !cache.path().join("assets/synthetic-asset").exists(),
        "a failed archive cache hit must not create a payload entry"
    );
}

#[test]
fn tampered_archive_cache_bytes_fail_closed() {
    expect_archive_failure("sri", |entry| {
        let path = entry.join("archive.tgz");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&path, &bytes).unwrap();
        // A damaged cache entry is never repaired in place.
        assert_eq!(fs::read(&path).unwrap(), bytes);
    });
}

#[test]
fn truncated_archive_cache_size_fails_closed() {
    expect_archive_failure("size", |entry| {
        let path = entry.join("archive.tgz");
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        let length = file.metadata().unwrap().len();
        file.set_len(length - 1).unwrap();
        assert!(fs::metadata(&path).unwrap().len() > 0);
    });
}

#[test]
fn archive_cache_metadata_tampering_and_unsafe_entries_fail_closed() {
    expect_archive_failure("does not match", |entry| {
        let path = entry.join("archive.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["sha256"] = Value::String("0".repeat(64));
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    });

    expect_archive_failure("does not match", |entry| {
        let path = entry.join("archive.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["size"] = Value::from(1);
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    });

    expect_archive_failure("regular file", |entry| {
        let path = entry.join("archive.tgz");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("archive.json", &path).unwrap();
    });

    expect_archive_failure("i/o", |entry| {
        fs::remove_file(entry.join("archive.json")).unwrap();
    });
}

#[test]
fn concurrent_archive_cache_writes_publish_one_entry() {
    let fixture = fixture("cache-concurrent");
    let cache = private_temp("cache-concurrent-store");
    let mut handles = Vec::new();
    for _ in 0..4 {
        let asset = fixture.asset.clone();
        let from_dir = fixture.source.path().to_path_buf();
        let cache_root = cache.path().to_path_buf();
        handles.push(std::thread::spawn(move || {
            provision_asset(
                &asset,
                &cache_root,
                Some(&from_dir),
                TEST_LOCK_SHA,
                SUBMITTED_LOCK,
            )
        }));
    }
    let mut entries = Vec::new();
    for handle in handles {
        entries.push(handle.join().unwrap().unwrap());
    }
    for entry in &entries {
        assert_eq!(entry.payload_tree_digest, GOLDEN_PAYLOAD_TREE);
    }
    assert_eq!(
        fs::read_dir(cache.path().join("archives/synthetic-asset"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(cache.path().join("assets/synthetic-asset"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(fs::read_dir(cache.path().join("tmp")).unwrap().count(), 0);

    // The single published archive entry can rebuild the payload after both the
    // source directory and the payload cache are gone.
    fs::remove_dir_all(cache.path().join("assets")).unwrap();
    fs::remove_dir_all(fixture.source.path()).unwrap();
    let entry = reload(&fixture, cache.path());
    assert_eq!(entry.source, SourceKind::ArchiveCache);
    assert_eq!(entry.payload_tree_digest, GOLDEN_PAYLOAD_TREE);
}

#[test]
fn provision_cli_never_prints_the_absolute_cache_path() {
    let temp = TempDir::new("cache-cli");
    let lock = write_repo_lock(temp.path());
    let cache = temp.path().join("distinctive-cache-directory");

    let plan = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
        .arg("provision")
        .arg("--lock")
        .arg(&lock)
        .arg("--cache")
        .arg(&cache)
        .arg("--print-plan")
        .arg("--offline")
        .output()
        .unwrap();
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan_out = String::from_utf8_lossy(&plan.stdout);
    assert!(plan_out.contains("cache miss"), "{plan_out}");
    assert!(
        !plan_out.contains(cache.to_string_lossy().as_ref()),
        "{plan_out}"
    );
    assert!(!temp.path().join("distinctive-cache-directory").exists());

    let cold = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
        .arg("provision")
        .arg("--lock")
        .arg(&lock)
        .arg("--cache")
        .arg(&cache)
        .arg("--offline")
        .output()
        .unwrap();
    assert!(!cold.status.success());
    let stdout = String::from_utf8_lossy(&cold.stdout);
    let stderr = String::from_utf8_lossy(&cold.stderr);
    assert!(stderr.contains("--from-dir"), "{stderr}");
    assert!(stderr.contains("archive cache"), "{stderr}");
    for output in [stdout.as_ref(), stderr.as_ref()] {
        assert!(
            !output.contains(cache.to_string_lossy().as_ref()),
            "CLI output leaked the absolute cache path: {output}"
        );
    }
}
