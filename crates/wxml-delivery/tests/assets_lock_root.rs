//! Frozen repository-root `assets.lock.json` acceptance test.
//!
//! This test reads the committed lock through the real parser and validator and
//! cross-checks every frozen identity against the values already pinned by
//! `manifest_v2`. Tree digests and the typings runtime closure are recomputed by
//! an independent implementation in this file.
//!
//! The test is offline: it never downloads a tarball, never reads the local
//! extraction under `/tmp` and never starts Node/npm.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use wxml_delivery::assets_lock::{Asset, AssetsLock};
use wxml_delivery::manifest::{FileEntry, sha256_bytes};

/// Canonical digest of the complete engine tarball tree (`stripPrefix=package/`).
const ENGINE_UPSTREAM_TREE_DIGEST: &str =
    "5ee874f4f3310cc5c80c4ec4296dafbbb0fd067bc0cb16751f757f5d40fd196d";
/// Canonical digest of the 122-file engine payload rooted at `engine`.
const ENGINE_PAYLOAD_TREE_DIGEST: &str =
    "5ee874f4f3310cc5c80c4ec4296dafbbb0fd067bc0cb16751f757f5d40fd196d";
/// Canonical digest of the complete typings tarball tree.
const TYPINGS_UPSTREAM_TREE_DIGEST: &str =
    "b89a8bc2d00e14d9b03719328093435dc8153ada5f79f2e7fb9f720b160caaed";
/// Canonical digest of the 23-file typings payload.
const TYPINGS_PAYLOAD_TREE_DIGEST: &str =
    "12a224f865880487f63b78d35f5e81835b1542c2bdb3a1e43bad0bc0eda7facb";
/// SHA-256 of the canonical lock bytes (never stored inside the lock itself).
const LOCK_SHA256: &str = "ce33fa960aa3eaf433419e2baeb6709587783ec11450642b6b1188c597725456";

fn repository_lock_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets.lock.json")
}

fn asset<'a>(lock: &'a AssetsLock, name: &str) -> &'a Asset {
    lock.asset(name)
        .unwrap_or_else(|| panic!("lock must contain asset `{name}`"))
}

fn tree_digest(files: &[FileEntry], root: &str) -> String {
    let prefix = if root.is_empty() {
        String::new()
    } else {
        format!("{root}/")
    };
    let mut directories = BTreeSet::new();
    let mut rows = Vec::with_capacity(files.len());
    for file in files {
        let relative = file
            .path
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("`{}` must be below `{root}`", file.path));
        assert_ne!(relative, "");
        for (index, _) in relative.match_indices('/') {
            directories.insert(relative[..index].to_owned());
        }
        rows.push((relative.to_owned(), file.mode.clone(), file.sha256.clone()));
    }
    rows.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut preimage = String::new();
    for directory in &directories {
        let _ = writeln!(preimage, "D 755 {directory}");
    }
    for (relative, mode, digest) in &rows {
        let octal = match mode.as_str() {
            "0755" => "755",
            "0644" => "644",
            other => panic!("unexpected mode `{other}`"),
        };
        let _ = writeln!(preimage, "F {octal} {digest} {relative}");
    }
    sha256_bytes(preimage.as_bytes())
}

fn runtime_closure(files: &[FileEntry], root: &str) -> (u64, u64, String) {
    let prefix = format!("{root}/");
    let mut matched = Vec::new();
    let mut total = 0_u64;
    for file in files {
        let Some(relative) = file.path.strip_prefix(&prefix) else {
            continue;
        };
        if relative == "index.d.ts"
            || (relative.starts_with("types/") && relative.ends_with(".d.ts"))
        {
            total += file.size;
            matched.push((relative.to_owned(), file.sha256.clone()));
        }
    }
    matched.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut preimage = String::new();
    for (relative, digest) in &matched {
        let _ = writeln!(preimage, "{digest}  {relative}");
    }
    (
        u64::try_from(matched.len()).unwrap(),
        total,
        sha256_bytes(preimage.as_bytes()),
    )
}

fn assert_safe_path(path: &str) {
    assert!(!path.starts_with('/'), "absolute path `{path}`");
    assert!(!path.contains('\\'), "backslash path `{path}`");
    assert!(
        !path.chars().any(char::is_control),
        "control char in `{path}`"
    );
    assert_ne!(path, "");
    for component in path.split('/') {
        assert!(
            !matches!(component, "" | "." | ".."),
            "unsafe component in `{path}`"
        );
    }
}

#[test]
fn repository_root_lock_is_frozen_and_canonical() {
    let path = repository_lock_path();
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!("read {}: {error}", path.display());
    });
    let lock = AssetsLock::parse(&bytes).expect("repository lock must parse");
    lock.validate().expect("repository lock must validate");

    assert_eq!(lock.schema_version, 1);
    assert_eq!(lock.assets.len(), 2);
    assert_eq!(
        lock.to_canonical_json().expect("canonical serialization"),
        bytes,
        "committed lock must already be canonical (2-space, LF, sorted arrays)"
    );
    assert_eq!(lock.lock_sha256().expect("lock sha256"), LOCK_SHA256);

    assert_engine_asset(&lock);
    assert_typings_asset(&lock);
}

fn assert_engine_asset(lock: &AssetsLock) {
    let engine = asset(lock, "typescript-native-engine");
    assert_eq!(engine.version, "7.1.0-beta");
    assert_eq!(
        engine.source_commit.as_deref(),
        Some("3e9236361fa97a9e6c685f4833ac457ae2d8c50a")
    );
    assert_eq!(engine.npm_package, "@typescript/typescript-linux-x64");
    assert_eq!(engine.license, "Apache-2.0");
    assert_eq!(engine.package_root, "engine");
    assert_eq!(
        engine.tarball.url,
        "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz"
    );
    assert_eq!(engine.tarball.size, 10_175_135);
    assert_eq!(
        engine.tarball.integrity,
        "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ=="
    );
    assert_eq!(
        engine.tarball.sha256,
        "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855"
    );
    assert_eq!(engine.upstream.file_count, 122);
    assert_eq!(engine.upstream.unpacked_size, 28_450_796);
    assert_eq!(engine.upstream.files.len(), 122);
    assert_eq!(engine.upstream.strip_prefix, "package/");
    assert_eq!(engine.upstream.tree_digest, ENGINE_UPSTREAM_TREE_DIGEST);
    assert_eq!(engine.payload.file_count, 122);
    assert_eq!(engine.payload.total_bytes, 28_450_796);
    assert_eq!(engine.payload.files.len(), 122);
    assert_eq!(engine.payload.tree_digest, ENGINE_PAYLOAD_TREE_DIGEST);
    assert_eq!(engine.extract.exclude.len(), 0);

    let engine_binary = engine
        .payload
        .files
        .iter()
        .find(|file| file.path == "engine/lib/tsc")
        .expect("engine payload must contain `engine/lib/tsc`");
    assert_eq!(
        engine_binary.sha256,
        "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70"
    );
    assert_eq!(engine_binary.size, 24_522_912);
    assert_eq!(engine_binary.mode, "0755");

    assert_eq!(engine.payload.bundles.len(), 1);
    let stdlib = &engine.payload.bundles[0];
    assert_eq!(stdlib.name, "typescript-standard-lib");
    assert_eq!(stdlib.root, "engine/lib");
    assert_eq!(stdlib.include, vec!["lib*.d.ts".to_owned()]);
    assert_eq!(stdlib.file_count, 116);
    assert_eq!(stdlib.total_bytes, 3_870_196);
    assert_eq!(
        stdlib.sha256,
        "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e"
    );
}

fn assert_typings_asset(lock: &AssetsLock) {
    let typings = asset(lock, "miniprogram-api-typings");
    assert_eq!(typings.version, "5.3.1");
    assert_eq!(typings.source_commit, None);
    assert_eq!(typings.npm_package, "miniprogram-api-typings");
    assert_eq!(typings.license, "MIT");
    assert_eq!(typings.package_root, "typings/miniprogram-api-typings");
    assert_eq!(
        typings.tarball.url,
        "https://registry.npmjs.org/miniprogram-api-typings/-/miniprogram-api-typings-5.3.1.tgz"
    );
    assert_eq!(typings.tarball.size, 449_429);
    assert_eq!(
        typings.tarball.integrity,
        "sha512-2CirO+C1z5RmKXDqTUSNbuyouHf5d0/KJTt+Qqi1EsiQYs8jlYhq7yWjp6EdTcJpvZzjuefZQCcDixYbrgIqow=="
    );
    assert_eq!(
        typings.tarball.sha256,
        "e5f8b50fe25b8979432060b915eb1cdfd1c91852e7e3842f3bfca932bdbd4b71"
    );
    assert_eq!(typings.upstream.file_count, 26);
    assert_eq!(typings.upstream.unpacked_size, 2_552_052);
    assert_eq!(typings.upstream.files.len(), 26);
    assert_eq!(typings.upstream.tree_digest, TYPINGS_UPSTREAM_TREE_DIGEST);
    assert_eq!(typings.payload.file_count, 23);
    assert_eq!(typings.payload.total_bytes, 2_548_075);
    assert_eq!(typings.payload.files.len(), 23);
    assert_eq!(typings.payload.tree_digest, TYPINGS_PAYLOAD_TREE_DIGEST);

    assert_eq!(typings.extract.exclude.len(), 3);
    let excluded: Vec<&str> = typings
        .extract
        .exclude
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(
        excluded,
        vec!["README-en.md", "package.json", "typings.json"]
    );
    for entry in &typings.extract.exclude {
        assert!(
            !entry.reason.trim().is_empty(),
            "exclusion `{}` needs a reason",
            entry.path
        );
        assert!(entry.reason.len() <= 256);
        assert!(!entry.reason.chars().any(char::is_control));
    }

    assert_eq!(typings.payload.bundles.len(), 1);
    let typings_bundle = &typings.payload.bundles[0];
    assert_eq!(typings_bundle.name, "miniprogram-api-typings");
    assert_eq!(typings_bundle.root, "typings/miniprogram-api-typings");
    assert_eq!(typings_bundle.include, vec!["**".to_owned()]);
    assert_eq!(typings_bundle.file_count, 23);
    assert_eq!(typings_bundle.total_bytes, 2_548_075);
    assert_eq!(
        typings_bundle.sha256,
        "ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b"
    );

    let (runtime_count, runtime_bytes, runtime_digest) =
        runtime_closure(&typings.payload.files, &typings.package_root);
    assert_eq!(runtime_count, 20);
    assert_eq!(runtime_bytes, 2_522_752);
    assert_eq!(
        runtime_digest,
        "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9"
    );
}

#[test]
fn repository_root_lock_uses_only_safe_relative_paths() {
    let bytes = fs::read(repository_lock_path()).expect("read repository lock");
    let lock = AssetsLock::parse(&bytes).expect("repository lock must parse");
    lock.validate().expect("repository lock must validate");

    for asset in &lock.assets {
        assert_safe_path(&asset.package_root);
        for license in &asset.license_files {
            assert_safe_path(license);
        }
        for file in asset.upstream.files.iter().chain(&asset.payload.files) {
            assert_safe_path(&file.path);
        }
        for exclusion in &asset.extract.exclude {
            assert_safe_path(&exclusion.path);
        }
        assert_eq!(
            tree_digest(&asset.upstream.files, ""),
            asset.upstream.tree_digest
        );
        assert_eq!(
            tree_digest(&asset.payload.files, &asset.package_root),
            asset.payload.tree_digest
        );
        assert_eq!(
            asset.payload.file_count,
            u64::try_from(asset.payload.files.len()).unwrap()
        );
        assert_eq!(
            asset.payload.total_bytes,
            asset
                .payload
                .files
                .iter()
                .map(|file| file.size)
                .sum::<u64>()
        );
        assert_eq!(
            asset.upstream.file_count,
            u64::try_from(asset.upstream.files.len()).unwrap()
        );
        assert_eq!(
            asset.upstream.unpacked_size,
            asset
                .upstream
                .files
                .iter()
                .map(|file| file.size)
                .sum::<u64>()
        );
    }

    let text = String::from_utf8(bytes).expect("lock is UTF-8");
    assert!(!text.contains('\\'), "lock contains a backslash");
    assert!(!text.contains('\r'), "lock must use LF line endings");
    assert!(
        !text
            .chars()
            .any(|character| character.is_control() && character != '\n'),
        "lock contains a control character"
    );
    assert!(!text.contains("\"../"));
    assert!(!text.contains("/../"));
}
