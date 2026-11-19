//! Shared fixtures and helpers for the assets-lock tests.

pub(crate) use crate::digests::{
    bundle_digest, sha256_hex, table_bytes, table_count, tree_digest, tree_digest_entries,
};
pub(crate) use crate::engine_table::ENGINE_FILES;
pub(crate) use crate::fixtures::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

use wxml_delivery::assets_lock::AssetsLock;
use wxml_delivery::error::DeliveryError;

/// Temporary directory removed on drop.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "wxml-assets-lock-{label}-{}-{nanos}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn provenance() -> Value {
    json!({
        "registry": "https://registry.npmjs.org",
        "distSha1": "0123456789abcdef0123456789abcdef01234567",
        "publishedAt": "2025-01-01T00:00:00Z",
    })
}

fn file_values(files: &[(&str, &str, u64, &str)], prefix: &str) -> Vec<Value> {
    files
        .iter()
        .map(|entry| {
            let (path, sha256, size, mode) = *entry;
            json!({
                "path": format!("{prefix}{path}"),
                "sha256": sha256,
                "size": size,
                "mode": mode,
            })
        })
        .collect()
}

fn collect_fixture_files(root: &Path, directory: &Path, output: &mut Vec<(String, String, u64)>) {
    let entries = fs::read_dir(directory).expect("read typings fixture directory");
    for entry in entries {
        let entry = entry.expect("typings fixture entry");
        let path = entry.path();
        if entry
            .file_type()
            .expect("typings fixture file type")
            .is_dir()
        {
            collect_fixture_files(root, &path, output);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("fixture path below root")
                .to_string_lossy()
                .into_owned();
            let bytes = fs::read(&path).expect("read typings fixture file");
            let size = u64::try_from(bytes.len()).expect("fixture size fits u64");
            output.push((relative, sha256_hex(&bytes), size));
        }
    }
}

pub(crate) fn typings_files() -> Vec<(String, String, u64)> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/miniprogram-api-typings-5.3.1");
    let mut files = Vec::new();
    collect_fixture_files(&root, &root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        files.len(),
        23,
        "checked-in typings fixture must have 23 files"
    );
    files
}

fn typings_payload_values() -> Vec<Value> {
    typings_files()
        .iter()
        .map(|(relative, sha256, size)| {
            json!({
                "path": format!("{TYPINGS_ROOT}/{relative}"),
                "sha256": sha256,
                "size": size,
                "mode": "0644",
            })
        })
        .collect()
}

fn typings_upstream_values() -> Vec<Value> {
    let mut values: Vec<Value> = typings_files()
        .iter()
        .map(|(relative, sha256, size)| {
            json!({
                "path": relative,
                "sha256": sha256,
                "size": size,
                "mode": "0644",
            })
        })
        .collect();
    for (path, sha256, size) in TYPINGS_EXCLUDED_FILES {
        values.push(json!({
            "path": path,
            "sha256": sha256,
            "size": size,
            "mode": "0644",
        }));
    }
    values.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    values
}

pub(crate) fn valid_value() -> Value {
    json!({
        "schemaVersion": 1,
        "assets": [engine_asset(), typings_asset()],
    })
}

fn engine_asset() -> Value {
    let upstream = file_values(&ENGINE_FILES, "");
    let payload = file_values(&ENGINE_FILES, "engine/");
    let count = table_count(&upstream);
    let bytes = table_bytes(&upstream);
    assert_eq!(count, ENGINE_FILE_COUNT);
    assert_eq!(bytes, ENGINE_UNPACKED_SIZE);
    let mut asset = json!({
        "name": ENGINE_ASSET,
        "kind": "npm-platform-tarball",
        "npmPackage": "@typescript/typescript-linux-x64",
        "version": ENGINE_VERSION,
        "sourceCommit": ENGINE_COMMIT,
        "os": ["linux"],
        "cpu": ["x64"],
        "provenance": provenance(),
        "license": "Apache-2.0",
        "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
        "packageRoot": ENGINE_ROOT,
        "tarball": {
            "url": ENGINE_URL,
            "size": ENGINE_TARBALL_SIZE,
            "integrity": ENGINE_SRI,
            "sha256": ENGINE_SHA256,
        },
        "upstream": {
            "fileCount": count,
            "unpackedSize": bytes,
            "stripPrefix": "package/",
            "treeDigest": tree_digest(&upstream, ""),
            "files": upstream,
        },
        "extract": {
            "include": ENGINE_INCLUDE,
            "exclude": [],
            "executables": ["lib/tsc"],
        },
    });
    asset["payload"] = engine_payload_value(&payload, count, bytes);
    asset
}

fn engine_payload_value(payload: &[Value], count: u64, bytes: u64) -> Value {
    json!({
        "fileCount": count,
        "totalBytes": bytes,
        "treeDigest": tree_digest(payload, ENGINE_ROOT),
        "files": payload,
        "bundles": [{
            "name": "typescript-standard-lib",
            "root": "engine/lib",
            "include": ["lib*.d.ts"],
            "fileCount": ENGINE_STDLIB_COUNT,
            "totalBytes": ENGINE_STDLIB_BYTES,
            "sha256": ENGINE_STDLIB_DIGEST,
        }],
    })
}

fn typings_asset() -> Value {
    let upstream = typings_upstream_values();
    let payload = typings_payload_values();
    let upstream_count = table_count(&upstream);
    let upstream_bytes = table_bytes(&upstream);
    assert_eq!(upstream_count, TYPINGS_UPSTREAM_COUNT);
    assert_eq!(upstream_bytes, TYPINGS_UPSTREAM_BYTES);
    let mut asset = json!({
        "name": TYPINGS_ASSET,
        "kind": "npm-tarball",
        "npmPackage": TYPINGS_ASSET,
        "version": TYPINGS_VERSION,
        "os": ["any"],
        "cpu": ["any"],
        "provenance": provenance(),
        "license": "MIT",
        "licenseFiles": [format!("{TYPINGS_ROOT}/LICENSE")],
        "packageRoot": TYPINGS_ROOT,
        "tarball": {
            "url": TYPINGS_URL,
            "size": 449_429,
            "integrity": TYPINGS_SRI,
            "sha256": "c".repeat(64),
        },
        "upstream": {
            "fileCount": upstream_count,
            "unpackedSize": upstream_bytes,
            "stripPrefix": "package/",
            "treeDigest": tree_digest(&upstream, ""),
            "files": upstream,
        },
        "extract": {
            "include": TYPINGS_INCLUDE,
            "exclude": [
                {"path": "package.json", "reason": "npm metadata is forbidden at runtime"},
                {"path": "README-en.md", "reason": "not part of the frozen payload"},
                {"path": "typings.json", "reason": "legacy typings metadata"},
            ],
            "executables": [],
        },
    });
    asset["payload"] = typings_payload_value(&payload);
    asset
}

fn typings_payload_value(payload: &[Value]) -> Value {
    let count = table_count(payload);
    let bytes = table_bytes(payload);
    assert_eq!(count, TYPINGS_PAYLOAD_COUNT);
    assert_eq!(bytes, TYPINGS_PAYLOAD_BYTES);
    json!({
        "fileCount": count,
        "totalBytes": bytes,
        "treeDigest": tree_digest(payload, TYPINGS_ROOT),
        "files": payload,
        "bundles": [{
            "name": TYPINGS_ASSET,
            "root": TYPINGS_ROOT,
            "include": ["**"],
            "fileCount": TYPINGS_PAYLOAD_COUNT,
            "totalBytes": TYPINGS_PAYLOAD_BYTES,
            "sha256": TYPINGS_PAYLOAD_DIGEST,
        }],
    })
}

pub(crate) fn parse_validate(value: &Value) -> Result<AssetsLock, DeliveryError> {
    let bytes = serde_json::to_vec(value).expect("serialize lock fixture");
    let lock = AssetsLock::parse(&bytes)?;
    lock.validate()?;
    Ok(lock)
}

pub(crate) fn validated(value: &Value) -> AssetsLock {
    parse_validate(value).expect("lock fixture must be valid")
}

pub(crate) fn assert_rejected(value: &Value, needle: &str) {
    let Err(error) = parse_validate(value) else {
        panic!("lock should be rejected: expected `{needle}`");
    };
    assert!(
        error.to_string().contains(needle),
        "expected `{needle}` in `{error}`"
    );
}

pub(crate) fn asset_mut<'a>(value: &'a mut Value, name: &str) -> &'a mut Value {
    value["assets"]
        .as_array_mut()
        .expect("assets array")
        .iter_mut()
        .find(|asset| asset["name"] == name)
        .expect("asset by name")
}

pub(crate) fn refresh_upstream(asset: &mut Value) {
    let files = asset["upstream"]["files"]
        .as_array()
        .expect("upstream files")
        .clone();
    asset["upstream"]["fileCount"] = json!(table_count(&files));
    asset["upstream"]["unpackedSize"] = json!(table_bytes(&files));
    asset["upstream"]["treeDigest"] = json!(tree_digest(&files, ""));
}

pub(crate) fn refresh_payload(asset: &mut Value) {
    let files = asset["payload"]["files"]
        .as_array()
        .expect("payload files")
        .clone();
    let root = asset["packageRoot"]
        .as_str()
        .expect("package root")
        .to_owned();
    asset["payload"]["fileCount"] = json!(table_count(&files));
    asset["payload"]["totalBytes"] = json!(table_bytes(&files));
    asset["payload"]["treeDigest"] = json!(tree_digest(&files, &root));
    let bundles = asset["payload"]["bundles"]
        .as_array()
        .expect("bundles")
        .clone();
    let mut refreshed = Vec::new();
    for mut bundle in bundles {
        let bundle_root = bundle["root"].as_str().expect("bundle root").to_owned();
        let include: Vec<String> = bundle["include"]
            .as_array()
            .expect("bundle include")
            .iter()
            .map(|value| value.as_str().expect("include pattern").to_owned())
            .collect();
        let (count, bytes, digest) = bundle_digest(&files, &bundle_root, &include);
        bundle["fileCount"] = json!(count);
        bundle["totalBytes"] = json!(bytes);
        bundle["sha256"] = json!(digest);
        refreshed.push(bundle);
    }
    asset["payload"]["bundles"] = Value::Array(refreshed);
}

pub(crate) fn reverse_arrays(value: &mut Value) {
    let assets = value["assets"].as_array_mut().expect("assets");
    assets.reverse();
    for asset in value["assets"].as_array_mut().expect("assets") {
        for key in ["os", "cpu", "licenseFiles"] {
            asset[key].as_array_mut().expect(key).reverse();
        }
        asset["extract"]["include"]
            .as_array_mut()
            .expect("include")
            .reverse();
        asset["extract"]["executables"]
            .as_array_mut()
            .expect("executables")
            .reverse();
        asset["extract"]["exclude"]
            .as_array_mut()
            .expect("exclude")
            .reverse();
        asset["upstream"]["files"]
            .as_array_mut()
            .expect("upstream files")
            .reverse();
        asset["payload"]["files"]
            .as_array_mut()
            .expect("payload files")
            .reverse();
        let bundles = asset["payload"]["bundles"].as_array_mut().expect("bundles");
        bundles.reverse();
        for bundle in bundles {
            bundle["include"]
                .as_array_mut()
                .expect("bundle include")
                .reverse();
        }
    }
}
