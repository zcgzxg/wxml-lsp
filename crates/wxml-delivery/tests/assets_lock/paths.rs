//! Path safety, include/exclude coverage and payload-shape tests.

use serde_json::json;

use crate::support::{
    ENGINE_ASSET, ENGINE_FILE_COUNT, ENGINE_UNPACKED_SIZE, TYPINGS_ASSET, TYPINGS_PAYLOAD_BYTES,
    TYPINGS_PAYLOAD_COUNT, TYPINGS_ROOT, assert_rejected, asset_mut, bundle_digest,
    refresh_payload, valid_value,
};

#[test]
fn duplicate_paths_and_case_collisions_are_rejected() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, ENGINE_ASSET);
    let files = asset["upstream"]["files"].as_array_mut().expect("files");
    let duplicate = files[0].clone();
    files.push(duplicate);
    assert_rejected(&value, "duplicate path");

    let mut value = valid_value();
    let asset = asset_mut(&mut value, ENGINE_ASSET);
    asset["upstream"]["files"]
        .as_array_mut()
        .expect("files")
        .push(json!({
            "path": "license",
            "sha256": "d".repeat(64),
            "size": 1,
            "mode": "0644",
        }));
    assert_rejected(&value, "collides");

    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    asset["payload"]["files"]
        .as_array_mut()
        .expect("files")
        .push(json!({
            "path": format!("{TYPINGS_ROOT}/license"),
            "sha256": "d".repeat(64),
            "size": 1,
            "mode": "0644",
        }));
    assert_rejected(&value, "collides");
}

#[test]
fn ancestor_file_paths_are_rejected() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, ENGINE_ASSET);
    asset["upstream"]["files"]
        .as_array_mut()
        .expect("files")
        .extend([
            json!({
                "path": "a",
                "sha256": "d".repeat(64),
                "size": 1,
                "mode": "0644",
            }),
            json!({
                "path": "a/b",
                "sha256": "e".repeat(64),
                "size": 1,
                "mode": "0644",
            }),
        ]);
    assert_rejected(&value, "nested below file path");

    let mut value = valid_value();
    let asset = asset_mut(&mut value, ENGINE_ASSET);
    asset["upstream"]["files"]
        .as_array_mut()
        .expect("files")
        .extend([
            json!({
                "path": "A",
                "sha256": "d".repeat(64),
                "size": 1,
                "mode": "0644",
            }),
            json!({
                "path": "a/b",
                "sha256": "e".repeat(64),
                "size": 1,
                "mode": "0644",
            }),
        ]);
    assert_rejected(&value, "collides with ancestor");
}

#[test]
fn unsafe_paths_are_rejected() {
    for (path, needle) in [
        ("/etc/passwd", "absolute path"),
        ("../escape", "`..` path component"),
        ("a\\b", "backslash separators"),
        ("a\nb", "control character"),
        ("a//b", "empty path component"),
        ("a/./b", "`.` path component"),
        ("", "empty path"),
        ("a/../../b", "`..` path component"),
    ] {
        let mut value = valid_value();
        asset_mut(&mut value, ENGINE_ASSET)["upstream"]["files"][0]["path"] = json!(path);
        assert_rejected(&value, needle);
    }

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["files"][0]["path"] = json!("/absolute");
    assert_rejected(&value, "absolute path");
}

#[test]
fn include_and_exclude_must_classify_every_upstream_file() {
    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["exclude"]
        .as_array_mut()
        .expect("exclude")
        .pop();
    assert_rejected(&value, "does not classify");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["include"]
        .as_array_mut()
        .expect("include")
        .push(json!("package.json"));
    assert_rejected(&value, "both included and excluded");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["exclude"]
        .as_array_mut()
        .expect("exclude")
        .push(json!({"path": "missing.txt", "reason": "not upstream"}));
    assert_rejected(&value, "not an upstream file");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["include"]
        .as_array_mut()
        .expect("include")
        .push(json!("missing/**"));
    assert_rejected(&value, "matches no upstream file");
}

#[test]
fn license_files_must_be_payload_files() {
    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["licenseFiles"] =
        json!([format!("{TYPINGS_ROOT}/LICENSE-missing")]);
    assert_rejected(&value, "is not in the payload");
}

#[test]
fn payload_must_equal_included_upstream_set() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    asset["payload"]["files"]
        .as_array_mut()
        .expect("payload files")
        .retain(|file| file["path"] != json!(format!("{TYPINGS_ROOT}/CHANGELOG.md")));
    refresh_payload(asset);
    assert_rejected(&value, "payload file set");
}

#[test]
fn bundle_identity_and_escape_are_rejected() {
    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["bundles"][0]["root"] = json!("engine");
    assert_rejected(&value, "type asset bundle");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["bundles"][0]["root"] = json!("../escape");
    assert_rejected(&value, "`..` path component");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["bundles"][0]["include"] = json!(["../**"]);
    assert_rejected(&value, "not a safe relative glob");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["bundles"][0]["fileCount"] = json!(22);
    assert_rejected(&value, "type asset bundle");

    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    let files = asset["payload"]["files"].as_array().expect("files").clone();
    let (count, bytes, digest) = bundle_digest(&files, TYPINGS_ROOT, &["**".to_owned()]);
    asset["payload"]["bundles"]
        .as_array_mut()
        .expect("bundles")
        .push(json!({
            "name": "other",
            "root": TYPINGS_ROOT,
            "include": ["**"],
            "fileCount": count,
            "totalBytes": bytes,
            "sha256": digest,
        }));
    assert_rejected(&value, "must declare exactly one bundle");
}

#[test]
fn declared_counts_must_match_tables() {
    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["fileCount"] = json!(ENGINE_FILE_COUNT + 1);
    assert_rejected(&value, "fileCount");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["unpackedSize"] =
        json!(ENGINE_UNPACKED_SIZE + 1);
    assert_rejected(&value, "total bytes");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["fileCount"] = json!(ENGINE_FILE_COUNT + 1);
    assert_rejected(&value, "fileCount");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["totalBytes"] = json!(ENGINE_UNPACKED_SIZE + 1);
    assert_rejected(&value, "total bytes");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["fileCount"] = json!(TYPINGS_PAYLOAD_COUNT + 1);
    assert_rejected(&value, "fileCount");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["totalBytes"] =
        json!(TYPINGS_PAYLOAD_BYTES + 1);
    assert_rejected(&value, "total bytes");
}
