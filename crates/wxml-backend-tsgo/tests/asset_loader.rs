//! Trusted native type-asset loader tests.
//!
//! The synthetic trees are hermetic: each test writes a complete 116-file
//! standard-library root plus a 20-file `miniprogram-api-typings` package into a
//! scratch directory and computes the expected pin from the intended content.
//!
//! The three real package tests are `#[ignore]`d and only run with
//! `-- --ignored`. They read roots from `TSGO_ASSET_ROOT` (engine `lib` or
//! `engine/lib` plus `typings/`) or `TSGO_BIN` plus `TSGO_TYPINGS_ROOT`, falling
//! back to the repository typings fixture. `WXML_REQUIRE_REAL_ENGINE=1` or
//! `WXML_REQUIRE_REAL_ENGINE=true` (case-insensitive) turns a missing real root
//! into a hard failure instead of a skip.
//!
//! [`repository_typings_runtime_closure_matches_821c93_stdlib_is_real_gate_only`]
//! is also `#[ignore]`d because it requires the untracked repository
//! `backends/typescript/node_modules` fixture (run `npm ci` first). When run
//! explicitly with `--ignored`, it verifies the 20-file/2,522,752 B/`821c93…`
//! typings closure with an independent walk and digest implementation without
//! calling the loader; a missing fixture is a hard failure there. The default
//! suite verifies neither that fixture nor the standard-library digest
//! `5e126e…`, which is checked only by the opt-in real gate.
//!
//! The loader is a disk-to-memory building block only. It does not verify an
//! install or the delivery manifest; that remains BLOCKED under P6-3-4/F3-03.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use wxml_backend_tsgo::asset_loader::MAX_ASSET_ENTRIES;
use wxml_backend_tsgo::input::{
    MAX_ASSET_PATH_COMPONENTS, MAX_TYPE_ASSET_BYTES, MAX_TYPE_ASSET_FILES, MINIPROGRAM_TYPINGS,
    PINNED_STANDARD_LIB_BYTES, PINNED_STANDARD_LIB_FILE_COUNT, PINNED_TYPINGS_BYTES,
    PINNED_TYPINGS_FILE_COUNT, PINNED_TYPINGS_VERSION, TypeAssetError, TypeAssetFile, TypeAssets,
    TypeAssetsPin,
};

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn sha(text: &[u8]) -> String {
    hex(&Sha256::digest(text))
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join("opencode/wxml-tsgo-asset-loader-tests");
    fs::create_dir_all(&base).expect("scratch base");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = base.join(format!("{tag}-{}-{count}", std::process::id()));
    fs::create_dir_all(&path).expect("scratch dir");
    path
}

/// Short scratch directory for Unix-domain sockets (path-length limited).
#[cfg(unix)]
fn scratch_short(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join("opencode");
    fs::create_dir_all(&base).expect("scratch base");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = base.join(format!("wl-{tag}-{}-{count}", std::process::id()));
    fs::create_dir_all(&path).expect("scratch dir");
    path
}

fn stdlib_name(index: u32) -> String {
    match index {
        0 => "lib.d.ts".to_owned(),
        1 => "lib.es5.d.ts".to_owned(),
        _ => format!("lib.extra{index}.d.ts"),
    }
}

fn stdlib_text(index: u32) -> String {
    format!("declare const stdlib{index}: number;\n")
}

fn typings_name(index: u32) -> String {
    match index {
        0 => "index.d.ts".to_owned(),
        1 => "types/index.d.ts".to_owned(),
        2 => "types/wx/index.d.ts".to_owned(),
        _ => format!("types/extra{index}.d.ts"),
    }
}

fn typings_text(index: u32) -> String {
    format!("declare const typings{index}: number;\n")
}

/// Computes the manifest bundle digest over intended (not on-disk) content.
fn bundle_digest(root: &str, files: &[(String, String)]) -> String {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|(path, text)| {
            let relative = path
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(path)
                .to_owned();
            (relative, sha(text.as_bytes()))
        })
        .collect();
    entries.sort();
    let mut hasher = Sha256::new();
    for (relative, digest) in entries {
        hasher.update(digest.as_bytes());
        hasher.update(b"  ");
        hasher.update(relative.as_bytes());
        hasher.update(b"\n");
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Returns the pin implied by the intended fixture content.
fn fixture_pin(engine_root: &str, typings_root: &str) -> TypeAssetsPin {
    let mut stdlib = Vec::new();
    for index in 0..PINNED_STANDARD_LIB_FILE_COUNT {
        stdlib.push((
            format!("{engine_root}/{}", stdlib_name(index)),
            stdlib_text(index),
        ));
    }
    let package = format!("{typings_root}/{MINIPROGRAM_TYPINGS}");
    let mut typings = Vec::new();
    for index in 0..PINNED_TYPINGS_FILE_COUNT {
        typings.push((
            format!("{package}/{}", typings_name(index)),
            typings_text(index),
        ));
    }
    let stdlib_bytes = stdlib
        .iter()
        .map(|(_, text)| u64::try_from(text.len()).expect("size"))
        .sum();
    let typings_bytes = typings
        .iter()
        .map(|(_, text)| u64::try_from(text.len()).expect("size"))
        .sum();
    TypeAssetsPin::new(
        PINNED_TYPINGS_VERSION,
        PINNED_STANDARD_LIB_FILE_COUNT,
        stdlib_bytes,
        bundle_digest(engine_root, &stdlib),
        PINNED_TYPINGS_FILE_COUNT,
        typings_bytes,
        bundle_digest(&package, &typings),
    )
    .expect("synthetic pin")
}

fn write_fixture(engine_root: &Path, typings_root: &Path) -> TypeAssetsPin {
    let package = typings_root.join(MINIPROGRAM_TYPINGS);
    fs::create_dir_all(engine_root).expect("engine root");
    fs::create_dir_all(typings_root).expect("typings root");
    fs::create_dir_all(package.join("types/wx")).expect("typings package");
    for index in 0..PINNED_STANDARD_LIB_FILE_COUNT {
        fs::write(engine_root.join(stdlib_name(index)), stdlib_text(index)).expect("stdlib file");
    }
    for index in 0..PINNED_TYPINGS_FILE_COUNT {
        fs::write(package.join(typings_name(index)), typings_text(index)).expect("typings file");
    }
    // Non-declaration files ship in the pinned package but are never mounted.
    fs::write(engine_root.join("tsc"), b"#!/bin/sh\n").expect("engine shim");
    fs::write(engine_root.join("tsc.sig"), b"signature").expect("engine signature");
    fs::write(
        package.join("package.json"),
        b"{\"name\":\"miniprogram-api-typings\"}",
    )
    .expect("package manifest");
    fs::write(package.join("README.md"), b"# typings\n").expect("readme");

    let engine = engine_root.to_str().expect("utf-8 engine root");
    let typings = typings_root.to_str().expect("utf-8 typings root");
    fixture_pin(engine, typings)
}

#[test]
fn loads_a_complete_synthetic_tree_and_matches_the_pin() {
    let base = scratch("happy");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    let assets = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect("a complete pinned tree loads");

    assert_eq!(
        assets.file_count(),
        PINNED_STANDARD_LIB_FILE_COUNT + PINNED_TYPINGS_FILE_COUNT
    );
    assert_eq!(assets.files().len(), 136);
    assert_eq!(assets.pin(), &pin);
    assert_eq!(
        assets.standard_lib_root().as_str(),
        engine_root.to_str().unwrap()
    );
    assert_eq!(
        assets.typings_root().as_str(),
        typings_root.to_str().unwrap()
    );
    assert_eq!(
        assets.total_bytes(),
        assets.files().iter().map(TypeAssetFile::size).sum::<u64>()
    );

    let engine = engine_root.to_str().unwrap();
    let package = format!("{}/{}", typings_root.to_str().unwrap(), MINIPROGRAM_TYPINGS);
    assert_eq!(
        assets
            .file(&format!("{engine}/lib.d.ts"))
            .expect("lib.d.ts")
            .text(),
        stdlib_text(0)
    );
    assert_eq!(
        assets
            .file(&format!("{package}/types/index.d.ts"))
            .expect("types/index.d.ts")
            .text(),
        typings_text(1)
    );
    assert!(assets.file(&format!("{engine}/tsc")).is_none());
    assert!(assets.file(&format!("{package}/package.json")).is_none());
    let roots = [engine, package.as_str()];
    assert!(assets.files().iter().all(|file| {
        roots
            .iter()
            .any(|root| file.path().as_str().starts_with(root))
    }));
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for file in assets.files() {
        assert!(
            paths.insert(file.path().as_str()),
            "path order must be unique"
        );
    }
}

#[test]
fn load_pinned_uses_the_builtin_pin_not_caller_supplied_values() {
    let base = scratch("load-pinned");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    let error = TypeAssets::load_pinned(&engine_root, &typings_root)
        .expect_err("the synthetic tree must not match the built-in pinned bytes/digests");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "standardLibBytes",
                ..
            }
        ),
        "expected a built-in-pin byte mismatch, got {error}"
    );

    // The same tree still verifies against its explicit test pin through the
    // lower-level entry point; only `load_pinned` ignores caller values.
    TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect("the explicit test pin still loads through load_trusted");
}

#[test]
fn real_pin_constants_match_the_pinned_fixture_values() {
    // Guards the pinned constants used by the opt-in real gate below.
    assert_eq!(PINNED_STANDARD_LIB_FILE_COUNT, 116);
    assert_eq!(PINNED_TYPINGS_FILE_COUNT, 20);
    assert_eq!(PINNED_STANDARD_LIB_BYTES, 3_870_196);
    assert_eq!(PINNED_TYPINGS_BYTES, 2_522_752);
}

#[test]
fn rejects_a_missing_root() {
    let base = scratch("missing");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);
    let missing = base.join("absent-engine");

    let error = TypeAssets::load_trusted(&missing, &typings_root, &pin)
        .expect_err("a missing engine root must fail closed");
    assert!(
        matches!(error, TypeAssetError::MissingRoot { .. }),
        "expected MissingRoot, got {error}"
    );

    let not_a_dir = engine_root.join("lib.d.ts");
    let error = TypeAssets::load_trusted(&not_a_dir, &typings_root, &pin)
        .expect_err("a file used as a root must fail closed");
    assert!(
        matches!(error, TypeAssetError::RootNotDirectory { .. }),
        "expected RootNotDirectory, got {error}"
    );

    let missing_package = base.join("typings-without-package");
    fs::create_dir_all(&missing_package).expect("empty typings root");
    let error = TypeAssets::load_trusted(&engine_root, &missing_package, &pin)
        .expect_err("a missing typings package must fail closed");
    assert!(
        matches!(error, TypeAssetError::MissingRoot { .. }),
        "expected MissingRoot for the package, got {error}"
    );
}

#[test]
fn rejects_non_normalized_roots() {
    let base = scratch("roots");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    for root in [
        PathBuf::from("relative/engine/lib"),
        PathBuf::from("/tmp/../etc"),
        PathBuf::from("/"),
    ] {
        let error = TypeAssets::load_trusted(&root, &typings_root, &pin)
            .expect_err("a non-normalized root must fail closed");
        assert!(
            matches!(error, TypeAssetError::InvalidRoot { .. }),
            "expected InvalidRoot for {}, got {error}",
            root.display()
        );
    }
}

fn substitute_same_size(path: &Path) {
    let text = fs::read_to_string(path).expect("declaration text");
    let changed = text.replacen("declare", "DECLARE", 1);
    assert_ne!(text, changed, "substitution must change content");
    assert_eq!(text.len(), changed.len(), "substitution must keep the size");
    fs::write(path, changed).expect("tampered declaration");
}

#[test]
fn rejects_substituted_sets_with_the_same_count_and_bytes() {
    for root in ["engine", "typings"] {
        let base = scratch("substituted");
        let _cleanup = Cleanup(base.clone());
        let engine_root = base.join("engine/lib");
        let typings_root = base.join("typings");
        let package = typings_root.join(MINIPROGRAM_TYPINGS);
        let pin = write_fixture(&engine_root, &typings_root);

        if root == "engine" {
            for index in 0..PINNED_STANDARD_LIB_FILE_COUNT {
                substitute_same_size(&engine_root.join(stdlib_name(index)));
            }
        } else {
            for index in 0..PINNED_TYPINGS_FILE_COUNT {
                substitute_same_size(&package.join(typings_name(index)));
            }
        }

        let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
            .expect_err("a same-size substitution must fail closed");
        let expected_field = if root == "engine" {
            "standardLibBundleDigest"
        } else {
            "typingsBundleDigest"
        };
        assert!(
            matches!(
                &error,
                TypeAssetError::PinMismatch { field, .. } if *field == expected_field
            ),
            "expected a {expected_field} mismatch for {root}, got {error}"
        );
    }
}

#[test]
fn rejects_missing_and_extra_declarations() {
    let base = scratch("set");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let package = typings_root.join(MINIPROGRAM_TYPINGS);
    let pin = write_fixture(&engine_root, &typings_root);

    fs::write(engine_root.join("lib.extra-overflow.d.ts"), "export {};\n")
        .expect("extra declaration");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("an extra declaration must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "standardLibFileCount",
                ..
            }
        ),
        "expected a standard-library count mismatch, got {error}"
    );
    fs::remove_file(engine_root.join("lib.extra-overflow.d.ts")).expect("restore engine root");

    fs::write(package.join("types/extra-overflow.d.ts"), "export {};\n")
        .expect("extra typings declaration");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("an extra typings declaration must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "typingsFileCount",
                ..
            }
        ),
        "expected a typings count mismatch, got {error}"
    );
    fs::remove_file(package.join("types/extra-overflow.d.ts")).expect("restore typings root");

    fs::remove_file(engine_root.join("lib.es5.d.ts")).expect("remove declaration");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a missing declaration must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "standardLibFileCount",
                ..
            }
        ),
        "expected a count mismatch, got {error}"
    );
    fs::write(engine_root.join("lib.es5.d.ts"), stdlib_text(1)).expect("restore");

    fs::remove_file(engine_root.join("lib.d.ts")).expect("remove required declaration");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a missing required declaration must fail closed");
    assert!(
        matches!(error, TypeAssetError::MissingRequired { .. }),
        "expected MissingRequired, got {error}"
    );
}

#[test]
fn ignores_files_outside_the_mounted_roots() {
    let base = scratch("confined");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    fs::write(base.join("outside.d.ts"), "declare const escape: true;\n").expect("outside file");
    fs::write(
        typings_root.join("sibling.d.ts"),
        "declare const sibling: true;\n",
    )
    .expect("sibling declaration");
    fs::write(engine_root.join("notes.md"), "# not mounted\n").expect("ignored engine file");

    let assets = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect("only the pinned roots are read");
    assert_eq!(assets.file_count(), 136);
    assert!(
        assets
            .files()
            .iter()
            .all(|file| !file.path().as_str().contains("sibling.d.ts")
                && !file.path().as_str().contains("outside.d.ts"))
    );
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}

#[cfg(unix)]
#[test]
fn rejects_symbolic_links_inside_and_at_the_roots() {
    let base = scratch("symlink");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let package = typings_root.join(MINIPROGRAM_TYPINGS);
    let pin = write_fixture(&engine_root, &typings_root);

    let outside = base.join("outside.d.ts");
    fs::write(&outside, stdlib_text(1)).expect("outside declaration");
    let declaration = engine_root.join("lib.es5.d.ts");
    fs::remove_file(&declaration).expect("remove declaration");
    symlink(&outside, &declaration);
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a symlinked declaration must fail closed");
    assert!(
        matches!(error, TypeAssetError::Symlink { .. }),
        "expected Symlink, got {error}"
    );
    fs::remove_file(&declaration).expect("remove link");
    fs::write(&declaration, stdlib_text(1)).expect("restore declaration");

    let wx = package.join("types/wx");
    let moved = base.join("moved-wx");
    fs::rename(&wx, &moved).expect("move directory");
    symlink(&moved, &wx);
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a symlinked package directory must fail closed");
    assert!(
        matches!(error, TypeAssetError::Symlink { .. }),
        "expected Symlink for a directory, got {error}"
    );
    fs::remove_file(&wx).expect("remove directory link");
    fs::rename(&moved, &wx).expect("restore directory");

    let root_link = base.join("engine-link");
    symlink(&engine_root, &root_link);
    let error = TypeAssets::load_trusted(&root_link, &typings_root, &pin)
        .expect_err("a symlinked engine root must fail closed");
    assert!(
        matches!(error, TypeAssetError::Symlink { .. }),
        "expected Symlink for the root, got {error}"
    );

    let typings_link = base.join("typings-link");
    symlink(&typings_root, &typings_link);
    let error = TypeAssets::load_trusted(&engine_root, &typings_link, &pin)
        .expect_err("a symlinked typings root must fail closed");
    assert!(
        matches!(error, TypeAssetError::Symlink { .. }),
        "expected Symlink for the typings root, got {error}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_hard_linked_declarations() {
    let base = scratch("hardlink");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    let outside = base.join("hardlink-source.d.ts");
    fs::write(&outside, stdlib_text(1)).expect("outside declaration");
    let declaration = engine_root.join("lib.es5.d.ts");
    fs::remove_file(&declaration).expect("remove declaration");
    fs::hard_link(&outside, &declaration).expect("hard link");

    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a hard-linked declaration must fail closed");
    assert!(
        matches!(error, TypeAssetError::HardLink { .. }),
        "expected HardLink, got {error}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_non_regular_files() {
    let base = scratch_short("socket");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    let socket = engine_root.join("lib.socket.d.ts");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a socket in a flat root must fail closed");
    assert!(
        matches!(error, TypeAssetError::NonRegularFile { .. }),
        "expected NonRegularFile, got {error}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_non_utf8_entry_names() {
    use std::os::unix::ffi::OsStringExt;

    let base = scratch("non-utf8-name");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    let name = std::ffi::OsString::from_vec(b"lib.\xff.d.ts".to_vec());
    fs::write(engine_root.join(name), "declare const x: 1;\n").expect("non-utf8 name");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a non-UTF-8 name must fail closed");
    assert!(
        matches!(error, TypeAssetError::NonUtf8Path { .. }),
        "expected NonUtf8Path, got {error}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_unsafe_entry_names_and_flat_directories() {
    let base = scratch("unsafe-name");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    fs::write(engine_root.join("lib\\escape.d.ts"), "export {};\n").expect("backslash name");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a backslash in a name must fail closed");
    assert!(
        matches!(error, TypeAssetError::UnsafeName { .. }),
        "expected UnsafeName, got {error}"
    );
    fs::remove_file(engine_root.join("lib\\escape.d.ts")).expect("remove backslash name");

    fs::write(engine_root.join("lib\nnewline.d.ts"), "export {};\n").expect("control name");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a control character in a name must fail closed");
    assert!(
        matches!(error, TypeAssetError::UnsafeName { .. }),
        "expected UnsafeName, got {error}"
    );
    fs::remove_file(engine_root.join("lib\nnewline.d.ts")).expect("remove control name");

    fs::create_dir(engine_root.join("nested")).expect("nested directory");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a nested directory in the flat engine root must fail closed");
    assert!(
        matches!(error, TypeAssetError::UnexpectedEntry { .. }),
        "expected UnexpectedEntry, got {error}"
    );
}

#[test]
fn rejects_non_utf8_declaration_content() {
    let base = scratch("non-utf8-content");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    fs::write(engine_root.join("lib.extra7.d.ts"), [0xff, 0xfe, 0x00]).expect("invalid text");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("non-UTF-8 declaration text must fail closed");
    assert!(
        matches!(error, TypeAssetError::NonUtf8Content { .. }),
        "expected NonUtf8Content, got {error}"
    );
}

#[test]
fn rejects_case_colliding_names() {
    let base = scratch("case");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    let probe = base.join("CaseProbe");
    fs::write(&probe, b"probe").expect("probe");
    if base.join("caseprobe").exists() {
        eprintln!("case-collision test skipped: file system is case-insensitive");
        return;
    }
    fs::write(engine_root.join("LIB.EXTRA5.D.TS"), "declare const x: 1;\n")
        .expect("case-colliding name");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a case-colliding name must fail closed");
    assert!(
        matches!(error, TypeAssetError::CaseCollision { .. }),
        "expected CaseCollision, got {error}"
    );
}

#[test]
fn rejects_an_over_budget_tree() {
    let base = scratch("budget");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    for index in 0..=MAX_TYPE_ASSET_FILES {
        fs::write(
            engine_root.join(format!("lib.overflow{index}.d.ts")),
            "export {};\n",
        )
        .expect("overflow declaration");
    }
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("an over-count tree must fail closed");
    assert!(
        matches!(error, TypeAssetError::TooManyFiles { .. }),
        "expected TooManyFiles, got {error}"
    );

    let base = scratch("oversize");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);
    fs::write(
        engine_root.join("lib.extra7.d.ts"),
        vec![b'x'; MAX_TYPE_ASSET_BYTES + 1],
    )
    .expect("oversized declaration");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("an over-size tree must fail closed");
    assert!(
        matches!(error, TypeAssetError::TooLarge { .. }),
        "expected TooLarge, got {error}"
    );
}

#[test]
fn rejects_excessive_typings_nesting() {
    let base = scratch("nested");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let package = typings_root.join(MINIPROGRAM_TYPINGS);
    let pin = write_fixture(&engine_root, &typings_root);

    let mut nested = package.join("types");
    for index in 0..=MAX_ASSET_PATH_COMPONENTS {
        nested = nested.join(format!("level{index}"));
    }
    fs::create_dir_all(&nested).expect("nested directories");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("excessive nesting must fail closed");
    let path = match error {
        TypeAssetError::NestingTooDeep { path } => path,
        other => panic!("expected NestingTooDeep, got {other}"),
    };
    assert!(
        path.starts_with(package.to_str().unwrap()),
        "nesting error must name the actual virtual directory: {path}"
    );
    assert!(
        path.contains("/types/level"),
        "nesting error must include the traversed subdirectories: {path}"
    );
}

#[test]
fn rejects_a_tree_with_too_many_directory_entries() {
    let base = scratch("entries");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let pin = write_fixture(&engine_root, &typings_root);

    // Hidden, non-declaration entries still count toward the walk budget.
    for index in 0..MAX_ASSET_ENTRIES {
        fs::write(engine_root.join(format!(".hidden{index}")), b"x").expect("hidden entry");
    }
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("an over-entry tree must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::TooManyEntries {
                count: 4097,
                max: 4096
            }
        ),
        "expected a typed entry limit, got {error}"
    );
}

#[cfg(unix)]
#[test]
fn rejects_non_regular_files_in_the_typings_package() {
    let base = scratch_short("typings-socket");
    let _cleanup = Cleanup(base.clone());
    let engine_root = base.join("engine/lib");
    let typings_root = base.join("typings");
    let package = typings_root.join(MINIPROGRAM_TYPINGS);
    let pin = write_fixture(&engine_root, &typings_root);

    let socket = package.join("types/socket.d.ts");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
    let error = TypeAssets::load_trusted(&engine_root, &typings_root, &pin)
        .expect_err("a socket in the typings package must fail closed");
    assert!(
        matches!(error, TypeAssetError::NonRegularFile { .. }),
        "expected NonRegularFile, got {error}"
    );
}

// ---------------------------------------------------------------------------
// Independent repository typings digest check (no loader call).
// ---------------------------------------------------------------------------

fn repository_typings_package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backends/typescript/node_modules/miniprogram-api-typings")
}

fn collect_runtime_declarations(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String, u64)>,
) {
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .expect("repository typings directory")
        .map(|entry| entry.expect("repository typings entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let metadata = fs::symlink_metadata(&path).expect("repository typings metadata");
        assert!(
            !metadata.file_type().is_symlink(),
            "repository typings fixture contains a symlink: {}",
            path.display()
        );
        if metadata.is_dir() {
            collect_runtime_declarations(root, &path, files);
            continue;
        }
        assert!(
            metadata.is_file(),
            "repository typings fixture contains a non-regular file: {}",
            path.display()
        );
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("repository typings name");
        if !name.ends_with(".d.ts") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("repository typings path stays below its root")
            .to_str()
            .expect("repository typings path is UTF-8")
            .to_owned();
        let bytes = fs::read(&path).expect("repository typings bytes");
        let size = u64::try_from(bytes.len()).expect("repository typings size");
        files.push((relative, sha(&bytes), size));
    }
}

#[ignore = "requires repository node_modules fixture; run with --ignored"]
#[test]
fn repository_typings_runtime_closure_matches_821c93_stdlib_is_real_gate_only() {
    let package = repository_typings_package();
    assert!(
        package.is_dir(),
        "repository typings fixture is missing at {}; run `npm ci --prefix backends/typescript` \
         (the standard-library digest 5e126e… is verified only by the opt-in real gate)",
        package.display()
    );
    let mut files = Vec::new();
    collect_runtime_declarations(&package, &package, &mut files);
    assert_eq!(files.len(), 20, "runtime typings closure file count");
    let total: u64 = files.iter().map(|(_, _, size)| *size).sum();
    assert_eq!(total, 2_522_752, "runtime typings closure bytes");

    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, digest, _) in &files {
        hasher.update(digest.as_bytes());
        hasher.update(b"  ");
        hasher.update(relative.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hex(&hasher.finalize());
    assert_eq!(
        digest, "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9",
        "runtime typings bundle digest must match the pinned value"
    );
}

// ---------------------------------------------------------------------------
// Opt-in real pinned package gates.
// ---------------------------------------------------------------------------

struct RealRoots {
    engine: PathBuf,
    typings: PathBuf,
}

fn normalize_typings_root(path: &Path) -> Option<PathBuf> {
    let root = if path.file_name().and_then(|name| name.to_str()) == Some(MINIPROGRAM_TYPINGS) {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    root.join(MINIPROGRAM_TYPINGS)
        .join("index.d.ts")
        .is_file()
        .then_some(root)
}

fn repository_typings_root() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let node_modules = manifest
        .parent()?
        .parent()?
        .join("backends/typescript/node_modules");
    normalize_typings_root(&node_modules)
}

fn resolve_real_roots() -> Option<RealRoots> {
    let packaged = std::env::var_os("TSGO_ASSET_ROOT").map(PathBuf::from);
    let engine = packaged
        .as_ref()
        .and_then(|root| {
            [root.join("engine/lib"), root.join("lib")]
                .into_iter()
                .find(|candidate| candidate.join("lib.d.ts").is_file())
        })
        .or_else(|| {
            let executable = PathBuf::from(std::env::var_os("TSGO_BIN")?);
            let parent = executable.parent()?;
            parent
                .join("lib.d.ts")
                .is_file()
                .then(|| parent.to_path_buf())
        })?;
    let typings = std::env::var_os("TSGO_TYPINGS_ROOT")
        .map(PathBuf::from)
        .and_then(|path| normalize_typings_root(&path))
        .or_else(|| {
            packaged
                .as_ref()
                .and_then(|root| normalize_typings_root(&root.join("typings")))
        })
        .or_else(repository_typings_root)?;
    Some(RealRoots { engine, typings })
}

/// Returns whether the real gate must fail instead of skipping.
fn require_real_gate() -> bool {
    std::env::var("WXML_REQUIRE_REAL_ENGINE")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Returns the real pinned roots, or skips explicitly unless the gate is required.
fn real_roots() -> Option<RealRoots> {
    let required = require_real_gate();
    let roots = resolve_real_roots();
    if roots.is_none() {
        assert!(
            !required,
            "real type-asset roots are required when WXML_REQUIRE_REAL_ENGINE is 1 or true; \
             set TSGO_ASSET_ROOT or TSGO_BIN and TSGO_TYPINGS_ROOT"
        );
        eprintln!(
            "real type-asset loader gate skipped: set TSGO_ASSET_ROOT or TSGO_BIN \
             plus TSGO_TYPINGS_ROOT (or WXML_REQUIRE_REAL_ENGINE=1/true for a hard gate)"
        );
    }
    roots
}

fn copy_declarations(source: &Path, destination: &Path, recursive: bool) {
    fs::create_dir_all(destination).expect("destination");
    for entry in fs::read_dir(source).expect("source directory") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_str().expect("utf-8 fixture name");
        if path.is_dir() {
            if recursive {
                copy_declarations(&path, &destination.join(name), recursive);
            }
            continue;
        }
        if name.ends_with(".d.ts") {
            fs::copy(&path, destination.join(name)).expect("copy declaration");
        }
    }
}

fn tamper_same_size(path: &Path) {
    let mut bytes = fs::read(path).expect("declaration bytes");
    let position = bytes
        .iter()
        .position(u8::is_ascii_alphabetic)
        .expect("declaration has a letter");
    bytes[position] = if bytes[position].is_ascii_lowercase() {
        bytes[position].to_ascii_uppercase()
    } else {
        bytes[position].to_ascii_lowercase()
    };
    fs::write(path, bytes).expect("tampered declaration");
}

fn declaration_paths(root: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).expect("declaration root") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                paths.extend(declaration_paths(&path, recursive));
            }
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".d.ts"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

#[ignore = "requires TSGO_BIN/TSGO_ASSET_ROOT"]
#[test]
fn real_pinned_roots_load_pinned_and_match_stdlib_and_typings_digests() {
    let Some(roots) = real_roots() else {
        return;
    };
    let assets = TypeAssets::load_pinned(&roots.engine, &roots.typings).expect("pinned tree");
    println!(
        "real type assets: files={} bytes={} engine={} typings={}",
        assets.file_count(),
        assets.total_bytes(),
        roots.engine.display(),
        roots.typings.display()
    );
    assert_eq!(assets.file_count(), 136);
    assert_eq!(
        assets.total_bytes(),
        PINNED_STANDARD_LIB_BYTES + PINNED_TYPINGS_BYTES
    );
    let pin = TypeAssetsPin::pinned_runtime();
    assert_eq!(assets.pin(), &pin);
    assert_eq!(pin.standard_lib_file_count(), 116);
    assert_eq!(pin.standard_lib_bytes(), PINNED_STANDARD_LIB_BYTES);
    assert_eq!(
        pin.standard_lib_bundle_digest(),
        "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e"
    );
    assert_eq!(pin.typings_file_count(), 20);
    assert_eq!(pin.typings_bytes(), PINNED_TYPINGS_BYTES);
    assert_eq!(
        pin.typings_bundle_digest(),
        "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9"
    );
    assert_eq!(
        assets.standard_lib_root().as_str(),
        roots.engine.to_str().unwrap()
    );
    assert_eq!(
        assets.typings_root().as_str(),
        roots.typings.to_str().unwrap()
    );
    assert!(
        assets
            .file(&format!("{}/lib.es5.d.ts", roots.engine.display()))
            .is_some()
    );
}

#[ignore = "requires TSGO_BIN/TSGO_ASSET_ROOT"]
#[test]
fn real_pinned_roots_reject_same_size_substitutions() {
    let Some(roots) = real_roots() else {
        return;
    };
    let base = scratch("real-substitution");
    let _cleanup = Cleanup(base.clone());

    let engine_copy = base.join("engine/lib");
    copy_declarations(&roots.engine, &engine_copy, false);
    for path in declaration_paths(&engine_copy, false) {
        tamper_same_size(&path);
    }
    let error = TypeAssets::load_pinned(&engine_copy, &roots.typings)
        .expect_err("a same-size standard-library substitution must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "standardLibBundleDigest",
                ..
            }
        ),
        "expected a standard-library digest mismatch, got {error}"
    );

    let typings_copy = base.join("typings");
    copy_declarations(
        &roots.typings.join(MINIPROGRAM_TYPINGS),
        &typings_copy.join(MINIPROGRAM_TYPINGS),
        true,
    );
    let declarations = declaration_paths(&typings_copy.join(MINIPROGRAM_TYPINGS), true);
    assert_eq!(declarations.len(), 20, "runtime typings closure");
    for path in &declarations {
        tamper_same_size(path);
    }
    let error = TypeAssets::load_pinned(&roots.engine, &typings_copy)
        .expect_err("a same-size typings substitution must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "typingsBundleDigest",
                ..
            }
        ),
        "expected a typings digest mismatch, got {error}"
    );
}

#[ignore = "requires TSGO_BIN/TSGO_ASSET_ROOT"]
#[test]
fn real_pinned_roots_reject_missing_extra_and_symlinked_typings() {
    let Some(roots) = real_roots() else {
        return;
    };
    let base = scratch("real-set");
    let _cleanup = Cleanup(base.clone());

    let typings_copy = base.join("typings");
    let package = typings_copy.join(MINIPROGRAM_TYPINGS);
    copy_declarations(&roots.typings.join(MINIPROGRAM_TYPINGS), &package, true);

    fs::remove_file(package.join("types/index.d.ts")).expect("remove declaration");
    let error = TypeAssets::load_pinned(&roots.engine, &typings_copy)
        .expect_err("a missing required declaration must fail closed");
    assert!(
        matches!(error, TypeAssetError::MissingRequired { .. }),
        "expected MissingRequired, got {error}"
    );

    let base = scratch("real-extra");
    let _cleanup = Cleanup(base.clone());
    let typings_copy = base.join("typings");
    let package = typings_copy.join(MINIPROGRAM_TYPINGS);
    copy_declarations(&roots.typings.join(MINIPROGRAM_TYPINGS), &package, true);
    fs::write(package.join("types/extra-unlisted.d.ts"), "export {};\n")
        .expect("extra declaration");
    let error = TypeAssets::load_pinned(&roots.engine, &typings_copy)
        .expect_err("an extra declaration must fail closed");
    assert!(
        matches!(
            error,
            TypeAssetError::PinMismatch {
                field: "typingsFileCount",
                ..
            }
        ),
        "expected a typings count mismatch, got {error}"
    );

    let base = scratch("real-symlink");
    let _cleanup = Cleanup(base.clone());
    let typings_copy = base.join("typings");
    let package = typings_copy.join(MINIPROGRAM_TYPINGS);
    copy_declarations(&roots.typings.join(MINIPROGRAM_TYPINGS), &package, true);
    let index = package.join("index.d.ts");
    fs::remove_file(&index).expect("remove declaration");
    #[cfg(unix)]
    std::os::unix::fs::symlink(package.join("types/index.d.ts"), &index).expect("symlink");
    #[cfg(unix)]
    {
        let error = TypeAssets::load_pinned(&roots.engine, &typings_copy)
            .expect_err("a symlinked declaration must fail closed");
        assert!(
            matches!(error, TypeAssetError::Symlink { .. }),
            "expected Symlink, got {error}"
        );
    }
}
