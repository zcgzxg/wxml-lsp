//! Tree-fingerprint tests, including an on-disk comparison with `verify_v2`.

use std::fs;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use wxml_delivery::assets_lock::canonical_tree_digest;
use wxml_delivery::manifest::FileEntry;
use wxml_delivery::verify_v2;

use crate::support::{
    ENGINE_ASSET, ENGINE_TREE_DIGEST, TYPINGS_ASSET, TYPINGS_PAYLOAD_TREE_DIGEST, TYPINGS_ROOT,
    TYPINGS_UPSTREAM_TREE_DIGEST, TempDir, assert_rejected, asset_mut, refresh_payload,
    refresh_upstream, sha256_hex, tree_digest_entries, valid_value, validated,
};

#[test]
fn golden_tree_digests_are_frozen() {
    let lock = validated(&valid_value());
    let engine = lock.asset(ENGINE_ASSET).expect("engine");
    assert_eq!(engine.upstream.tree_digest, ENGINE_TREE_DIGEST);
    assert_eq!(engine.payload.tree_digest, ENGINE_TREE_DIGEST);
    let typings = lock.asset(TYPINGS_ASSET).expect("typings");
    assert_eq!(typings.upstream.tree_digest, TYPINGS_UPSTREAM_TREE_DIGEST);
    assert_eq!(typings.payload.tree_digest, TYPINGS_PAYLOAD_TREE_DIGEST);
}

#[test]
fn canonical_tree_digest_matches_independent_implementation() {
    let lock = validated(&valid_value());
    for asset in &lock.assets {
        let upstream_expected = tree_digest_entries(&asset.upstream.files, "");
        assert_eq!(asset.upstream.tree_digest, upstream_expected);
        assert_eq!(
            canonical_tree_digest(&asset.upstream.files, "").expect("digest"),
            upstream_expected
        );
        let payload_expected = tree_digest_entries(&asset.payload.files, &asset.package_root);
        assert_eq!(asset.payload.tree_digest, payload_expected);
        assert_eq!(
            canonical_tree_digest(&asset.payload.files, &asset.package_root).expect("digest"),
            payload_expected
        );
    }
}

#[test]
fn tree_digest_covers_directories_modes_and_order() {
    let entries = vec![
        FileEntry {
            path: "z/c/d.txt".to_owned(),
            sha256: "d".repeat(64),
            size: 1,
            mode: "0644".to_owned(),
        },
        FileEntry {
            path: "a.txt".to_owned(),
            sha256: "a".repeat(64),
            size: 1,
            mode: "0755".to_owned(),
        },
        FileEntry {
            path: "z/b.txt".to_owned(),
            sha256: "b".repeat(64),
            size: 1,
            mode: "0644".to_owned(),
        },
    ];
    let expected_lines = format!(
        "D 755 z\nD 755 z/c\nF 755 {} a.txt\nF 644 {} z/b.txt\nF 644 {} z/c/d.txt\n",
        "a".repeat(64),
        "b".repeat(64),
        "d".repeat(64)
    );
    let expected = sha256_hex(expected_lines.as_bytes());
    assert_eq!(
        canonical_tree_digest(&entries, "").expect("digest"),
        expected
    );

    let prefixed: Vec<FileEntry> = entries
        .into_iter()
        .map(|mut entry| {
            entry.path = format!("pkg/{}", entry.path);
            entry
        })
        .collect();
    assert_eq!(
        canonical_tree_digest(&prefixed, "pkg").expect("prefixed digest"),
        expected
    );
}

#[cfg(unix)]
fn on_disk_entries() -> Vec<FileEntry> {
    vec![
        FileEntry {
            path: "LICENSE".to_owned(),
            sha256: sha256_hex(b"license\n"),
            size: 8,
            mode: "0644".to_owned(),
        },
        FileEntry {
            path: "lib/tsc".to_owned(),
            sha256: sha256_hex(b"engine\n"),
            size: 7,
            mode: "0755".to_owned(),
        },
        FileEntry {
            path: "lib/nested/lib.d.ts".to_owned(),
            sha256: sha256_hex(b"types\n"),
            size: 6,
            mode: "0644".to_owned(),
        },
    ]
}

#[cfg(unix)]
#[test]
fn tree_fingerprint_matches_verify_v2_on_disk() {
    let temp = TempDir::new("digest-tree");
    let root = temp.path();
    fs::create_dir_all(root.join("lib/nested")).expect("create dirs");
    fs::write(root.join("LICENSE"), b"license\n").expect("write license");
    fs::write(root.join("lib/tsc"), b"engine\n").expect("write tsc");
    fs::write(root.join("lib/nested/lib.d.ts"), b"types\n").expect("write lib.d.ts");
    for directory in [
        root.to_path_buf(),
        root.join("lib"),
        root.join("lib/nested"),
    ] {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).expect("chmod dir");
    }
    fs::set_permissions(root.join("LICENSE"), fs::Permissions::from_mode(0o644))
        .expect("chmod license");
    fs::set_permissions(root.join("lib/tsc"), fs::Permissions::from_mode(0o755))
        .expect("chmod tsc");
    fs::set_permissions(
        root.join("lib/nested/lib.d.ts"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("chmod d.ts");
    let entries = on_disk_entries();
    let expected = tree_digest_entries(&entries, "");
    assert_eq!(
        verify_v2::tree_fingerprint(root).expect("fingerprint"),
        expected
    );
    assert_eq!(
        canonical_tree_digest(&entries, "").expect("lock digest"),
        expected
    );
}

#[test]
fn tampered_tree_digests_are_rejected() {
    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["payload"]["treeDigest"] = json!("a".repeat(64));
    assert_rejected(&value, "payload tree digest");

    let mut value = valid_value();
    asset_mut(&mut value, ENGINE_ASSET)["upstream"]["treeDigest"] = json!("a".repeat(64));
    assert_rejected(&value, "upstream tree digest");
}

#[test]
fn payload_must_match_upstream_bytes() {
    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    asset["payload"]["files"]
        .as_array_mut()
        .expect("payload files")
        .iter_mut()
        .find(|file| file["path"] == json!(format!("{TYPINGS_ROOT}/LICENSE")))
        .expect("typings LICENSE")["sha256"] = json!("b".repeat(64));
    refresh_payload(asset);
    assert_rejected(&value, "does not match its upstream bytes");

    let mut value = valid_value();
    let asset = asset_mut(&mut value, TYPINGS_ASSET);
    asset["upstream"]["files"]
        .as_array_mut()
        .expect("upstream files")
        .iter_mut()
        .find(|file| file["path"] == json!("LICENSE"))
        .expect("upstream LICENSE")["sha256"] = json!("b".repeat(64));
    refresh_upstream(asset);
    assert_rejected(&value, "does not match its upstream bytes");
}
