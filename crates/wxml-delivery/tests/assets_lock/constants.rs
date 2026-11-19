//! Frozen constant reconciliation and negative budget tests.

use serde_json::{Value, json};

use wxml_delivery::assets_lock as lock;
use wxml_delivery::assets_lock::ExcludedPath;

use crate::support::{
    ENGINE_ASSET, ENGINE_COMMIT, ENGINE_FILE_COUNT, ENGINE_SRI, ENGINE_STDLIB_BYTES,
    ENGINE_STDLIB_COUNT, ENGINE_STDLIB_DIGEST, ENGINE_TARBALL_SIZE, ENGINE_TSC_SHA256,
    ENGINE_TSC_SIZE, ENGINE_UNPACKED_SIZE, ENGINE_URL, ENGINE_VERSION, TYPINGS_ASSET,
    TYPINGS_PAYLOAD_BYTES, TYPINGS_PAYLOAD_COUNT, TYPINGS_PAYLOAD_DIGEST,
    TYPINGS_PAYLOAD_TREE_DIGEST, TYPINGS_RUNTIME_BYTES, TYPINGS_RUNTIME_COUNT,
    TYPINGS_RUNTIME_DIGEST, TYPINGS_UPSTREAM_26_DIGEST, TYPINGS_UPSTREAM_BYTES,
    TYPINGS_UPSTREAM_COUNT, TYPINGS_UPSTREAM_TREE_DIGEST, TYPINGS_VERSION, assert_rejected,
    asset_mut, refresh_payload, refresh_upstream, valid_value, validated,
};

#[test]
fn invalid_digest_size_and_mode_are_rejected() {
    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["files"][0]["sha256"] = json!("g".repeat(64));
    assert_rejected(&value, "SHA-256");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["files"][0]["sha256"] = json!("0".repeat(64));
    assert_rejected(&value, "placeholder");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["files"][0]["sha256"] = json!("deadbeef");
    assert_rejected(&value, "SHA-256");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["files"][0]["size"] = json!(0);
    assert_rejected(&value, "size 0");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["files"][0]["size"] =
        json!(128 * 1024 * 1024 + 1);
    assert_rejected(&value, "exceeds");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["files"][0]["mode"] = json!("0777");
    assert_rejected(&value, "unsupported mode");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["tarball"]["sha256"] = json!("0".repeat(64));
    assert_rejected(&value, "placeholder");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["tarball"]["size"] = json!(512 * 1024 * 1024 + 1);
    assert_rejected(&value, "asset archive exceeds");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["treeDigest"] = json!("0".repeat(64));
    assert_rejected(&value, "placeholder");
}

#[test]
fn engine_identity_drift_is_rejected() {
    let reject = |mutate: &dyn Fn(&mut Value), needle: &str| {
        let mut value = valid_value();
        mutate(asset_mut(&mut value, ENGINE_ASSET));
        assert_rejected(&value, needle);
    };

    reject(
        &|asset| {
            asset["version"] = json!("9.9.9");
            asset["tarball"]["url"] = json!(
                "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-9.9.9.tgz"
            );
        },
        "frozen engine version",
    );
    reject(
        &|asset| asset["sourceCommit"] = json!("0".repeat(40)),
        "frozen engine sourceCommit",
    );
    reject(
        &|asset| {
            asset["npmPackage"] = json!("@typescript/typescript-linux-arm64");
            asset["tarball"]["url"] = json!(
                "https://registry.npmjs.org/@typescript/typescript-linux-arm64/-/typescript-linux-arm64-7.1.0-beta.tgz"
            );
        },
        "frozen engine npmPackage",
    );
    reject(
        &|asset| asset["license"] = json!("MIT"),
        "frozen engine license",
    );
    reject(&|asset| asset["os"] = json!(["darwin"]), "frozen engine os");
    reject(
        &|asset| asset["cpu"] = json!(["arm64"]),
        "frozen engine cpu",
    );
}

#[test]
fn engine_tarball_drift_is_rejected() {
    let reject = |mutate: &dyn Fn(&mut Value), needle: &str| {
        let mut value = valid_value();
        mutate(asset_mut(&mut value, ENGINE_ASSET));
        assert_rejected(&value, needle);
    };

    reject(
        &|asset| asset["tarball"]["size"] = json!(ENGINE_TARBALL_SIZE + 1),
        "frozen engine tarball size",
    );
    reject(
        &|asset| asset["tarball"]["integrity"] = json!(crate::support::TYPINGS_SRI),
        "frozen engine tarball integrity",
    );
    reject(
        &|asset| asset["tarball"]["sha256"] = json!("b".repeat(64)),
        "frozen engine tarball SHA-256",
    );
    reject(
        &|asset| {
            asset["tarball"]["url"] = json!(
                "https://registry.npmjs.org.evil/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz"
            );
        },
        "tarball URL",
    );
    reject(
        &|asset| asset["upstream"]["unpackedSize"] = json!(ENGINE_UNPACKED_SIZE + 1),
        "total bytes",
    );
    reject(
        &|asset| asset["payload"]["fileCount"] = json!(ENGINE_FILE_COUNT - 1),
        "fileCount",
    );
}

#[test]
fn engine_extract_drift_is_rejected() {
    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["extract"]["include"]
        .as_array_mut()
        .expect("include")
        .push(json!("*.md"));
    assert_rejected(&value, "frozen engine extract include");

    let mut value = valid_value();
    let asset = asset_mut(&mut value, ENGINE_ASSET);
    asset["extract"]["executables"] = json!(["lib/tsc", "lib/tsc.sig"]);
    for (table, path) in [
        ("upstream", "lib/tsc.sig"),
        ("payload", "engine/lib/tsc.sig"),
    ] {
        asset[table]["files"]
            .as_array_mut()
            .expect("files")
            .iter_mut()
            .find(|file| file["path"] == json!(path))
            .expect("tsc.sig entry")["mode"] = json!("0755");
    }
    refresh_upstream(asset);
    refresh_payload(asset);
    assert_rejected(&value, "frozen engine extract executables");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["bundles"][0]["name"] = json!("other-lib");
    assert_rejected(&value, "frozen standard library bundle identity");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["bundles"][0]["sha256"] = json!("b".repeat(64));
    assert_rejected(&value, "type asset bundle");
}

#[test]
fn engine_binary_pin_is_enforced() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, ENGINE_ASSET);
    for (table, path) in [("upstream", "lib/tsc"), ("payload", "engine/lib/tsc")] {
        asset[table]["files"]
            .as_array_mut()
            .expect("files")
            .iter_mut()
            .find(|file| file["path"] == json!(path))
            .expect("tsc entry")["sha256"] = json!("b".repeat(64));
    }
    refresh_upstream(asset);
    refresh_payload(asset);
    assert_rejected(&value, "frozen payload file");
}

#[test]
fn typings_identity_drift_is_rejected() {
    let reject = |mutate: &dyn Fn(&mut Value), needle: &str| {
        let mut value = valid_value();
        mutate(asset_mut(&mut value, TYPINGS_ASSET));
        assert_rejected(&value, needle);
    };

    reject(
        &|asset| {
            asset["version"] = json!("5.3.2");
            asset["tarball"]["url"] = json!(
                "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.2.tgz"
            );
        },
        "frozen typings version",
    );
    reject(
        &|asset| asset["license"] = json!("Apache-2.0"),
        "frozen typings license",
    );
    reject(&|asset| asset["os"] = json!(["linux"]), "frozen typings os");
    reject(
        &|asset| asset["packageRoot"] = json!("typings/other"),
        "outside",
    );
}

#[test]
fn typings_payload_drift_is_rejected() {
    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["bundles"][0]["sha256"] = json!("b".repeat(64));
    assert_rejected(&value, "type asset bundle");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["payload"]["bundles"][0]["name"] = json!("other-typings");
    assert_rejected(&value, "frozen miniprogram typings bundle identity");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["include"]
        .as_array_mut()
        .expect("include")
        .push(json!("index.*"));
    assert_rejected(&value, "frozen typings extract include");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["exclude"]
        .as_array_mut()
        .expect("exclude")
        .pop();
    assert_rejected(&value, "does not classify");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["licenseFiles"] =
        json!([format!("{}/LICENSE-missing", crate::support::TYPINGS_ROOT)]);
    assert_rejected(&value, "is not in the payload");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["upstream"]["fileCount"] =
        json!(TYPINGS_UPSTREAM_COUNT - 1);
    assert_rejected(&value, "fileCount");
}

#[test]
fn typings_payload_23_is_frozen() {
    let lock = validated(&valid_value());
    let typings = lock.asset(TYPINGS_ASSET).expect("typings");
    assert_eq!(typings.upstream.file_count, 26);
    assert_eq!(typings.payload.file_count, 23);
    assert_eq!(typings.payload.total_bytes, 2_548_075);
    assert_eq!(typings.payload.tree_digest, TYPINGS_PAYLOAD_TREE_DIGEST);
    assert_eq!(typings.upstream.tree_digest, TYPINGS_UPSTREAM_TREE_DIGEST);
    assert_eq!(typings.payload.bundles[0].sha256, TYPINGS_PAYLOAD_DIGEST);
    let mut paths: Vec<&str> = typings
        .extract
        .exclude
        .iter()
        .map(|entry: &ExcludedPath| entry.path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["README-en.md", "package.json", "typings.json"]);
}

#[test]
fn typings_payload_26_is_rejected() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    let upstream = asset["upstream"]["files"]
        .as_array()
        .expect("upstream")
        .clone();
    let payload: Vec<Value> = upstream
        .iter()
        .map(|file| {
            json!({
                "path": format!("{}/{}", crate::support::TYPINGS_ROOT, file["path"].as_str().expect("path")),
                "sha256": file["sha256"],
                "size": file["size"],
                "mode": "0644",
            })
        })
        .collect();
    asset["payload"]["files"] = Value::Array(payload);
    asset["extract"]["include"] = json!(["**"]);
    asset["extract"]["exclude"] = json!([]);
    asset["payload"]["bundles"] = json!([{
        "name": TYPINGS_ASSET,
        "root": crate::support::TYPINGS_ROOT,
        "include": ["**"],
        "fileCount": 26,
        "totalBytes": TYPINGS_UPSTREAM_BYTES,
        "sha256": TYPINGS_UPSTREAM_26_DIGEST,
    }]);
    refresh_payload(asset);
    assert_rejected(&value, "frozen typings payload fileCount");
}

#[test]
fn typings_payload_20_is_rejected() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    asset["payload"]["files"]
        .as_array_mut()
        .expect("payload")
        .retain(|file| {
            let path = file["path"].as_str().expect("path");
            !path.ends_with("/LICENSE")
                && !path.ends_with("/README.md")
                && !path.ends_with("/CHANGELOG.md")
        });
    asset["extract"]["include"] = json!(["index.d.ts", "types/**"]);
    asset["extract"]["exclude"] = json!([
        {"path": "package.json", "reason": "npm metadata"},
        {"path": "README-en.md", "reason": "not in payload"},
        {"path": "typings.json", "reason": "legacy metadata"},
        {"path": "LICENSE", "reason": "license evidence is reviewed separately"},
        {"path": "README.md", "reason": "not runtime"},
        {"path": "CHANGELOG.md", "reason": "not runtime"},
    ]);
    asset["licenseFiles"] = json!([format!("{}/index.d.ts", crate::support::TYPINGS_ROOT)]);
    refresh_payload(asset);
    assert_rejected(&value, "frozen typings payload fileCount");
}

#[test]
fn typings_upstream_26_is_frozen() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    asset["upstream"]["files"]
        .as_array_mut()
        .expect("upstream")
        .retain(|file| {
            let path = file["path"].as_str().expect("path");
            !crate::support::TYPINGS_EXCLUDED.contains(&path)
        });
    asset["extract"]["exclude"] = json!([]);
    refresh_upstream(asset);
    assert_rejected(&value, "frozen typings upstream fileCount");
}

#[test]
fn lock_constants_are_pinned() {
    assert_eq!(lock::ASSETS_LOCK_SCHEMA_VERSION, 1);
    assert_eq!(lock::MAX_ASSETS_LOCK_BYTES, 1024 * 1024);
    assert_eq!(lock::ENGINE_ASSET_NAME, ENGINE_ASSET);
    assert_eq!(lock::TYPINGS_ASSET_NAME, TYPINGS_ASSET);
    assert_eq!(lock::TYPINGS_NPM_PACKAGE, TYPINGS_ASSET);
    assert_eq!(lock::STANDARD_LIB_TOTAL_BYTES, ENGINE_STDLIB_BYTES);
    assert_eq!(lock::STANDARD_LIB_BUNDLE_DIGEST, ENGINE_STDLIB_DIGEST);
    assert_eq!(
        lock::MINIPROGRAM_TYPINGS_RUNTIME_FILE_COUNT,
        TYPINGS_RUNTIME_COUNT
    );
    assert_eq!(
        lock::MINIPROGRAM_TYPINGS_RUNTIME_TOTAL_BYTES,
        TYPINGS_RUNTIME_BYTES
    );
    assert_eq!(
        lock::MINIPROGRAM_TYPINGS_RUNTIME_BUNDLE_DIGEST,
        TYPINGS_RUNTIME_DIGEST
    );
    assert_eq!(lock::ENGINE_EXECUTABLES, ["lib/tsc"]);
}

#[test]
fn engine_manifest_constants_are_pinned() {
    assert_eq!(wxml_delivery::manifest_v2::ENGINE_VERSION, ENGINE_VERSION);
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_SOURCE_COMMIT,
        ENGINE_COMMIT
    );
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_SIZE,
        ENGINE_TARBALL_SIZE
    );
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_INTEGRITY,
        ENGINE_SRI
    );
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_SHA256,
        crate::support::ENGINE_SHA256
    );
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_FILE_COUNT,
        ENGINE_FILE_COUNT
    );
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_UNPACKED_SIZE,
        ENGINE_UNPACKED_SIZE
    );
    assert_eq!(
        wxml_delivery::manifest_v2::ENGINE_DISTRIBUTION_URL,
        ENGINE_URL
    );
    assert_eq!(
        wxml_delivery::package::PINNED_ENGINE_BINARY_SHA256,
        ENGINE_TSC_SHA256
    );
    assert_eq!(
        wxml_delivery::package::PINNED_ENGINE_BINARY_SIZE,
        ENGINE_TSC_SIZE
    );
    assert_eq!(
        wxml_delivery::manifest_v2::STANDARD_LIB_FILE_COUNT,
        ENGINE_STDLIB_COUNT
    );
}

#[test]
fn typings_manifest_constants_are_pinned() {
    assert_eq!(
        wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT,
        26
    );
    assert_eq!(
        wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_UPSTREAM_TOTAL_BYTES,
        TYPINGS_UPSTREAM_BYTES
    );
    assert_eq!(
        wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_FILE_COUNT,
        TYPINGS_PAYLOAD_COUNT
    );
    assert_eq!(
        wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_TOTAL_BYTES,
        TYPINGS_PAYLOAD_BYTES
    );
    assert_eq!(
        wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_BUNDLE_DIGEST,
        TYPINGS_PAYLOAD_DIGEST
    );
    assert_eq!(
        wxml_delivery::manifest_v2::MINIPROGRAM_TYPINGS_VERSION,
        TYPINGS_VERSION
    );
}
