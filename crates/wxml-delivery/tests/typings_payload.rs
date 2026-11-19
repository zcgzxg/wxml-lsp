//! Opt-in consistency check for the frozen 23-file typings payload.
//!
//! This test reads an already unpacked `miniprogram-api-typings@5.3.1` tree
//! supplied through `WXML_TYPINGS_FIXTURE_DIR`. It is a local regression
//! fixture, not upstream provenance: no tarball is created, downloaded or
//! executed, and the expected values only hold for the unmodified 5.3.1 tree.
//!
//! Run explicitly:
//!
//! ```text
//! WXML_TYPINGS_FIXTURE_DIR=/path/to/miniprogram-api-typings \
//!   cargo test -p wxml-delivery --test typings_payload -- --ignored --nocapture
//! ```
//!
//! `WXML_REQUIRE_TYPINGS_FIXTURE=1` (or `=true`) turns a missing fixture into a
//! hard failure instead of a skip. F3-03 remains BLOCKED by the real tarball
//! SRI/SHA-256, provisioning/lock, and clean-env/ABI evidence; this harness
//! closes no acceptance ID.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use wxml_delivery::manifest::{FileEntry, sha256_bytes};
use wxml_delivery::manifest_v2::{
    MINIPROGRAM_TYPINGS_BUNDLE_DIGEST, MINIPROGRAM_TYPINGS_FILE_COUNT,
    MINIPROGRAM_TYPINGS_PAYLOAD_PATHS, MINIPROGRAM_TYPINGS_ROOT, MINIPROGRAM_TYPINGS_TOTAL_BYTES,
    MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT, MINIPROGRAM_TYPINGS_UPSTREAM_TOTAL_BYTES,
    TypeAssetBundle, compute_type_asset_bundle, forbidden_runtime_path,
};

const FIXTURE_ENV: &str = "WXML_TYPINGS_FIXTURE_DIR";
const REQUIRE_ENV: &str = "WXML_REQUIRE_TYPINGS_FIXTURE";

/// Files present upstream but excluded from the delivery payload.
const EXCLUDED: [&str; 3] = ["package.json", "README-en.md", "typings.json"];

const UPSTREAM_FILES: u64 = 26;
const UPSTREAM_BYTES: u64 = 2_552_052;
const UPSTREAM_DIGEST: &str = "2141aac47206441f0434a7b519e49da1e8756d1bcdb6c17809a7f5e188a34c42";

const PAYLOAD_FILES: u64 = 23;
const PAYLOAD_BYTES: u64 = 2_548_075;
const PAYLOAD_DIGEST: &str = "ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b";

const RUNTIME_FILES: u64 = 20;
const RUNTIME_BYTES: u64 = 2_522_752;
const RUNTIME_DIGEST: &str = "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9";

fn require_fixture() -> bool {
    std::env::var(REQUIRE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn fixture_dir_or_skip(test: &str) -> Option<PathBuf> {
    let Some(value) = std::env::var_os(FIXTURE_ENV) else {
        assert!(
            !require_fixture(),
            "{FIXTURE_ENV} is required when {REQUIRE_ENV}=1; point it at an unpacked \
             miniprogram-api-typings 5.3.1 package directory"
        );
        eprintln!(
            "{test} skipped: set {FIXTURE_ENV} to an unpacked miniprogram-api-typings 5.3.1 \
             package directory"
        );
        return None;
    };
    let path = PathBuf::from(value);
    let path = fs::canonicalize(&path).expect("canonicalize typings fixture dir");
    assert!(path.is_dir(), "{} is not a directory", path.display());
    Some(path)
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<FileEntry>) {
    let mut children: Vec<PathBuf> = fs::read_dir(directory)
        .expect("read typings fixture directory")
        .map(|entry| entry.expect("typings fixture entry").path())
        .collect();
    children.sort();
    for path in children {
        let metadata = fs::symlink_metadata(&path).expect("read typings fixture metadata");
        assert!(
            !metadata.file_type().is_symlink(),
            "typings fixture contains a symlink at {}",
            path.display()
        );
        if metadata.is_dir() {
            collect_files(root, &path, files);
            continue;
        }
        assert!(
            metadata.is_file(),
            "typings fixture contains a non-regular entry at {}",
            path.display()
        );
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o644,
            "typings fixture file is not 0644: {}",
            path.display()
        );
        let relative = path
            .strip_prefix(root)
            .expect("typings fixture path escapes its root")
            .to_str()
            .expect("typings fixture path is UTF-8");
        let package_path = format!("{MINIPROGRAM_TYPINGS_ROOT}/{relative}");
        files.push(FileEntry {
            sha256: sha256_bytes(&fs::read(&path).expect("read typings fixture file")),
            size: metadata.len(),
            path: package_path,
            mode: "0644".to_owned(),
        });
    }
}

fn bundle(files: &[FileEntry], include: &[&str]) -> TypeAssetBundle {
    let include: Vec<String> = include
        .iter()
        .map(|pattern| (*pattern).to_owned())
        .collect();
    compute_type_asset_bundle(files, MINIPROGRAM_TYPINGS_ROOT, &include)
        .expect("compute typings bundle")
}

fn total_bytes(files: &[FileEntry]) -> u64 {
    files.iter().map(|file| file.size).sum()
}

fn is_excluded(path: &str) -> bool {
    EXCLUDED
        .iter()
        .any(|name| path == format!("{MINIPROGRAM_TYPINGS_ROOT}/{name}"))
}

#[test]
#[ignore = "requires WXML_TYPINGS_FIXTURE_DIR with an unpacked miniprogram-api-typings 5.3.1 tree"]
fn payload_and_upstream_typings_digests_are_stable_and_isolated() {
    let Some(root) = fixture_dir_or_skip("typings payload fixture") else {
        return;
    };
    let mut upstream_files = Vec::new();
    collect_files(&root, &root, &mut upstream_files);
    assert_eq!(upstream_files.len() as u64, UPSTREAM_FILES);
    assert_eq!(total_bytes(&upstream_files), UPSTREAM_BYTES);

    for name in EXCLUDED {
        assert!(
            upstream_files
                .iter()
                .any(|file| file.path == format!("{MINIPROGRAM_TYPINGS_ROOT}/{name}")),
            "fixture is missing upstream file `{name}`"
        );
    }

    let mut payload_files: Vec<FileEntry> = upstream_files
        .iter()
        .filter(|file| !is_excluded(&file.path))
        .cloned()
        .collect();
    let runtime_files: Vec<FileEntry> = payload_files
        .iter()
        .filter(|file| file.path.ends_with(".d.ts"))
        .cloned()
        .collect();

    let payload_paths: std::collections::BTreeSet<&str> = payload_files
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(&format!("{MINIPROGRAM_TYPINGS_ROOT}/"))
                .expect("payload path must be below the typings root")
        })
        .collect();
    let expected_paths: std::collections::BTreeSet<&str> =
        MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.iter().copied().collect();
    assert_eq!(payload_paths, expected_paths);

    let upstream = bundle(&upstream_files, &["**"]);
    let payload = bundle(&payload_files, &["**"]);
    let runtime = bundle(&runtime_files, &["**"]);
    assert_eq!(payload.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);
    assert_eq!(payload.file_count, PAYLOAD_FILES);
    assert_eq!(payload.total_bytes, MINIPROGRAM_TYPINGS_TOTAL_BYTES);
    assert_eq!(total_bytes(&payload_files), PAYLOAD_BYTES);
    assert_eq!(payload.sha256, MINIPROGRAM_TYPINGS_BUNDLE_DIGEST);
    assert_eq!(payload.sha256, PAYLOAD_DIGEST);
    assert_eq!(
        MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT,
        MINIPROGRAM_TYPINGS_FILE_COUNT + u64::try_from(EXCLUDED.len()).unwrap()
    );
    assert_eq!(
        total_bytes(&upstream_files),
        MINIPROGRAM_TYPINGS_UPSTREAM_TOTAL_BYTES
    );

    assert_eq!(upstream.file_count, UPSTREAM_FILES);
    assert_eq!(upstream.sha256, UPSTREAM_DIGEST);

    // The 20-file runtime closure is what the native VFS may mount; it is not
    // the delivery payload digest and must never substitute for it.
    assert_eq!(runtime.file_count, RUNTIME_FILES);
    assert_eq!(total_bytes(&runtime_files), RUNTIME_BYTES);
    assert_eq!(runtime.sha256, RUNTIME_DIGEST);
    assert_ne!(payload.sha256, upstream.sha256);
    assert_ne!(payload.sha256, runtime.sha256);

    // Byte-order sorting is insertion-order independent.
    payload_files.reverse();
    assert_eq!(bundle(&payload_files, &["**"]), payload);

    // None of the 20 runtime declarations reference the excluded metadata.
    for file in &runtime_files {
        let relative = file
            .path
            .strip_prefix(&format!("{MINIPROGRAM_TYPINGS_ROOT}/"))
            .expect("runtime file must be below the typings root");
        let text = fs::read_to_string(root.join(relative)).expect("read runtime declaration");
        for name in EXCLUDED {
            assert!(
                !text.contains(name),
                "runtime declaration `{relative}` references excluded file `{name}`"
            );
        }
    }

    // `package.json` is rejected lexically; the other two are excluded by the
    // packager allowlist, not by the Node-runtime scan.
    assert!(forbidden_runtime_path(&format!("{MINIPROGRAM_TYPINGS_ROOT}/package.json")).is_some());
    assert!(forbidden_runtime_path(&format!("{MINIPROGRAM_TYPINGS_ROOT}/README-en.md")).is_none());
    assert!(forbidden_runtime_path(&format!("{MINIPROGRAM_TYPINGS_ROOT}/typings.json")).is_none());

    println!(
        "typings fixture: upstream={} files/{} B/{}; payload={} files/{} B/{}; \
         runtime={} files/{} B/{}",
        upstream.file_count,
        total_bytes(&upstream_files),
        upstream.sha256,
        payload.file_count,
        total_bytes(&payload_files),
        payload.sha256,
        runtime.file_count,
        total_bytes(&runtime_files),
        runtime.sha256
    );
}
