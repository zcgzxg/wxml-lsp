//! Parsing, canonicalization and top-level identity tests.

use serde_json::json;

use wxml_delivery::assets_lock as lock;
use wxml_delivery::assets_lock::{AssetKind, AssetsLock};
use wxml_delivery::error::DeliveryError;

use crate::support::{
    ENGINE_ASSET, ENGINE_TSC_SIZE, MINIMAL_CANONICAL_BYTES, MINIMAL_LOCK_SHA256, TYPINGS_ASSET,
    TYPINGS_PAYLOAD_BYTES, TYPINGS_PAYLOAD_COUNT, TYPINGS_PAYLOAD_DIGEST, TYPINGS_UPSTREAM_COUNT,
    VALID_LOCK_SHA256, assert_rejected, asset_mut, reverse_arrays, valid_value, validated,
};

#[test]
fn valid_lock_parses_and_validates() {
    let lock = validated(&valid_value());
    assert_eq!(lock.schema_version, lock::ASSETS_LOCK_SCHEMA_VERSION);
    assert_eq!(lock.assets.len(), 2);
    let engine = lock.asset(ENGINE_ASSET).expect("engine asset");
    assert_eq!(engine.kind, AssetKind::NpmPlatformTarball);
    assert_eq!(engine.payload.file_count, 122);
    let typings = lock.asset(TYPINGS_ASSET).expect("typings asset");
    assert_eq!(typings.kind, AssetKind::NpmTarball);
    assert_eq!(typings.upstream.file_count, TYPINGS_UPSTREAM_COUNT);
    assert_eq!(typings.payload.file_count, TYPINGS_PAYLOAD_COUNT);
    assert_eq!(typings.payload.total_bytes, TYPINGS_PAYLOAD_BYTES);
    assert_eq!(typings.payload.bundles[0].sha256, TYPINGS_PAYLOAD_DIGEST);
}

#[test]
fn asset_lookup_is_exact() {
    let lock = validated(&valid_value());
    assert!(lock.asset(ENGINE_ASSET).is_some());
    assert!(lock.asset(TYPINGS_ASSET).is_some());
    assert!(lock.asset("TypeScript-Native-Engine").is_none());
    assert!(lock.asset("").is_none());
}

#[test]
fn canonical_form_is_order_independent() {
    let first = validated(&valid_value());
    let mut reordered = valid_value();
    reverse_arrays(&mut reordered);
    let second = validated(&reordered);
    let first_bytes = first.to_canonical_json().expect("canonical bytes");
    let second_bytes = second.to_canonical_json().expect("canonical bytes");
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(
        first.lock_sha256().expect("lock sha"),
        second.lock_sha256().expect("lock sha")
    );
    assert!(first_bytes.ends_with(b"\n"));
    let text = String::from_utf8(first_bytes.clone()).expect("utf8");
    assert!(text.contains("\n  \"assets\": ["));
    assert_eq!(first.lock_sha256().expect("sha").len(), 64);
    let round_trip = AssetsLock::parse(&first_bytes).expect("parse canonical bytes");
    assert_eq!(
        round_trip.to_canonical_json().expect("canonical"),
        first_bytes
    );

    let minimal = json!({"assets": [], "schemaVersion": 1});
    let minimal_bytes = serde_json::to_vec(&minimal).expect("serialize minimal");
    let minimal_lock = AssetsLock::parse(&minimal_bytes).expect("parse minimal");
    assert_eq!(
        minimal_lock.to_canonical_json().expect("canonical"),
        MINIMAL_CANONICAL_BYTES.as_bytes()
    );
    assert_eq!(
        minimal_lock.lock_sha256().expect("minimal sha"),
        MINIMAL_LOCK_SHA256
    );
}

#[test]
fn canonical_lock_sha256_is_frozen() {
    let lock = validated(&valid_value());
    assert_eq!(lock.lock_sha256().expect("lock sha"), VALID_LOCK_SHA256);
}

#[test]
fn unknown_fields_are_rejected() {
    let mut value = valid_value();
    value["unexpected"] = json!(true);
    assert_rejected(&value, "malformed");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["unexpected"] = json!(1);
    assert_rejected(&value, "malformed");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["tarball"]["unexpected"] = json!(1);
    assert_rejected(&value, "malformed");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["upstream"]["unexpected"] = json!(1);
    assert_rejected(&value, "malformed");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["extract"]["exclude"][0]["unexpected"] = json!(1);
    assert_rejected(&value, "malformed");
}

#[test]
fn schema_and_identity_policy_are_enforced() {
    let mut value = valid_value();
    value["schemaVersion"] = json!(2);
    assert_rejected(&value, "unsupported assets lock schema");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["kind"] = json!("local-dir");
    assert_rejected(&value, "malformed");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["kind"] = json!("npm-platform-tarball");
    assert_rejected(&value, "frozen typings kind");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["license"] = json!("BSD-3-Clause");
    assert_rejected(&value, "unsupported license");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["provenance"]["registry"] = json!("https://example.com");
    assert_rejected(&value, "registry");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["os"] = json!([]);
    assert_rejected(&value, "1-8 entries");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)
        .as_object_mut()
        .expect("engine asset")
        .remove("sourceCommit");
    assert_rejected(&value, "frozen engine sourceCommit");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["tarball"]["size"] = json!(0);
    assert_rejected(&value, "zero placeholder");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["files"][0]["size"] = json!(ENGINE_TSC_SIZE);
    assert_rejected(&value, "total bytes");
}

#[test]
fn tarball_url_policy_is_enforced() {
    for (url, needle) in [
        (
            "http://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz",
            "must use https",
        ),
        (
            "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz?x=1",
            "forbidden characters",
        ),
        (
            "https://user@registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz",
            "registry.npmjs.org",
        ),
        (
            "https://registry.npmjs.org/miniprogram-api-typings/../miniprogram-api-typings-5.3.1.tgz",
            "unsafe path",
        ),
    ] {
        let mut value = valid_value();
        asset_mut(&mut value, TYPINGS_ASSET)["tarball"]["url"] = json!(url);
        assert_rejected(&value, needle);
    }
}

#[test]
fn oversized_input_is_rejected() {
    let oversized = vec![b' '; 1024 * 1024 + 1];
    assert!(matches!(
        AssetsLock::parse(&oversized),
        Err(DeliveryError::SizeLimit(_))
    ));
    assert_eq!(lock::MAX_ASSETS_LOCK_BYTES, 1024 * 1024);
}

#[test]
fn empty_and_unknown_asset_sets_are_rejected() {
    let mut value = valid_value();
    value["assets"] = json!([]);
    assert_rejected(&value, "exactly the two frozen assets");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["name"] = json!("unknown-asset");
    assert_rejected(&value, "missing the miniprogram typings");

    let mut value = valid_value();
    asset_mut(&mut value, TYPINGS_ASSET)["name"] = json!(ENGINE_ASSET);
    assert_rejected(&value, "duplicate asset");
}

#[test]
fn parse_errors_do_not_echo_input() {
    let secret = "s".repeat(512);
    let payload = format!("{{\"schemaVersion\":1,\"assets\":[],\"{secret}\":1}}");
    let error = AssetsLock::parse(payload.as_bytes()).expect_err("unknown field");
    let text = error.to_string();
    assert!(text.contains("malformed"));
    assert!(!text.contains(&secret));
}
