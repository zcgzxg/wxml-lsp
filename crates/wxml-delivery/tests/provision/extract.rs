//! npm-tarball extraction matrix (P6).

use std::fs;

use wxml_delivery::error::DeliveryError;
use wxml_delivery::provision::extract_npm_tar_gz;

use crate::support::{
    Spec, TempDir, empty_extract, mode_of, npm_tar_gz, pax_record, raw_header, raw_tar_gz,
};

fn destination(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("extracted")
}

fn write_archive(temp: &TempDir, bytes: &[u8]) -> std::path::PathBuf {
    let path = temp.path().join("asset.tgz");
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn strips_package_prefix_and_normalizes_modes() {
    let temp = TempDir::new("extract-normal");
    let archive = write_archive(
        &temp,
        &npm_tar_gz(&[
            Spec::Dir("lib"),
            Spec::File("lib/a.d.ts", 0o664, b"a"),
            Spec::File("lib/b.d.ts", 0o600, b"b"),
            Spec::File("bin/tool", 0o644, b"#!/bin/sh\n"),
        ]),
    );
    let mut extract = empty_extract();
    extract.executables.push("bin/tool".to_owned());
    let files = extract_npm_tar_gz(&archive, &destination(&temp), &extract).unwrap();
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["bin/tool", "lib/a.d.ts", "lib/b.d.ts"]);
    assert_eq!(files[0].mode, "0755");
    assert_eq!(files[1].mode, "0644");
    assert_eq!(files[2].mode, "0644");
    let root = destination(&temp);
    assert_eq!(mode_of(&root.join("lib")), 0o755);
    assert_eq!(mode_of(&root.join("bin/tool")), 0o755);
    assert_eq!(mode_of(&root.join("lib/a.d.ts")), 0o644);
    assert_eq!(fs::read(root.join("lib/a.d.ts")).unwrap(), b"a");
}

#[test]
fn rejects_traversal_absolute_and_non_package_paths() {
    let cases: &[(&str, &[u8])] = &[
        ("absolute", b"/etc/passwd"),
        ("parent", b"package/../escape"),
        ("dot", b"package/./escape"),
        ("empty-component", b"package/a//b"),
        ("outside-prefix", b"other/file"),
        ("top-level-file", b"package"),
        ("backslash", b"package/a\\b"),
        ("control", b"package/a\x01b"),
        ("drive", b"package/C:/windows"),
        ("unc", b"package/\\\\server\\share"),
    ];
    for (name, path) in cases {
        let temp = TempDir::new(&format!("extract-path-{name}"));
        let mut blocks = Vec::new();
        let header = raw_header(path, 1, b'0');
        blocks.extend_from_slice(&header);
        blocks.extend_from_slice(b"x");
        blocks.extend_from_slice(&[0_u8; 511]);
        blocks.extend_from_slice(&[0_u8; 1024]);
        let archive = write_archive(&temp, &raw_tar_gz(&blocks));
        let error =
            extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{name}: {error:?}"
        );
        assert!(!destination(&temp).exists(), "{name}: destination removed");
        assert!(!temp.path().join("escape").exists());
    }
}

#[test]
fn rejects_links_fifos_devices_and_special_mode_bits() {
    let cases: Vec<(&str, Vec<Spec<'static>>)> = vec![
        ("symlink", vec![Spec::Symlink("link", "/etc/passwd")]),
        ("hardlink", vec![Spec::Hardlink("link", "package/target")]),
        ("fifo", vec![Spec::Fifo("pipe")]),
        ("char", vec![Spec::Char("device")]),
        ("setuid", vec![Spec::File("a", 0o4755, b"x")]),
        ("setgid", vec![Spec::File("a", 0o2755, b"x")]),
        ("sticky", vec![Spec::File("a", 0o1755, b"x")]),
    ];
    for (name, specs) in cases {
        let temp = TempDir::new(&format!("extract-kind-{name}"));
        let archive = write_archive(&temp, &npm_tar_gz(&specs));
        let error =
            extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{name}: {error:?}"
        );
        assert!(!destination(&temp).exists(), "{name}: destination removed");
    }
}

#[test]
fn rejects_unsafe_directory_modes() {
    for (name, mode) in [
        ("setgid-dir", 0o2755),
        ("sticky-dir", 0o1755),
        ("world-writable-dir", 0o0777),
    ] {
        let temp = TempDir::new(&format!("extract-dir-mode-{name}"));
        let mut blocks = Vec::new();
        blocks.extend_from_slice(&crate::support::raw_header_mode(
            b"package/d",
            0,
            b'5',
            mode,
        ));
        blocks.extend_from_slice(&[0_u8; 1024]);
        let archive = write_archive(&temp, &raw_tar_gz(&blocks));
        let error =
            extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{name}: {error:?}"
        );
        assert!(!destination(&temp).exists());
    }
}

#[test]
fn pax_path_override_is_applied_and_cannot_escape() {
    let escape = TempDir::new("extract-pax-path-escape");
    let record = pax_record("path", "../escape");
    let mut blocks = Vec::new();
    blocks.extend_from_slice(&raw_header(b"package/pax", record.len() as u64, b'x'));
    blocks.extend_from_slice(record.as_bytes());
    let padding = (512 - record.len() % 512) % 512;
    blocks.extend_from_slice(&vec![0_u8; padding]);
    blocks.extend_from_slice(&raw_header(b"package/safe", 1, b'0'));
    blocks.extend_from_slice(b"x");
    blocks.extend_from_slice(&[0_u8; 511]);
    blocks.extend_from_slice(&[0_u8; 1024]);
    let archive = write_archive(&escape, &raw_tar_gz(&blocks));
    let error = extract_npm_tar_gz(&archive, &destination(&escape), &empty_extract()).unwrap_err();
    assert!(
        matches!(error, DeliveryError::UnsafeArchive(_)),
        "{error:?}"
    );
    assert!(!escape.path().join("escape").exists());
    assert!(!destination(&escape).exists());

    let renamed = TempDir::new("extract-pax-path-renamed");
    let record = pax_record("path", "package/actual.d.ts");
    let mut blocks = Vec::new();
    blocks.extend_from_slice(&raw_header(b"package/pax", record.len() as u64, b'x'));
    blocks.extend_from_slice(record.as_bytes());
    let padding = (512 - record.len() % 512) % 512;
    blocks.extend_from_slice(&vec![0_u8; padding]);
    blocks.extend_from_slice(&raw_header(b"package/ignored.d.ts", 1, b'0'));
    blocks.extend_from_slice(b"x");
    blocks.extend_from_slice(&[0_u8; 511]);
    blocks.extend_from_slice(&[0_u8; 1024]);
    let archive = write_archive(&renamed, &raw_tar_gz(&blocks));
    let files = extract_npm_tar_gz(&archive, &destination(&renamed), &empty_extract()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "actual.d.ts");
    assert!(destination(&renamed).join("actual.d.ts").is_file());
}

#[test]
fn rejects_duplicates_case_folding_and_file_directory_conflicts() {
    let cases: Vec<(&str, Vec<Spec<'static>>)> = vec![
        (
            "duplicate-file",
            vec![Spec::File("a", 0o644, b"x"), Spec::File("a", 0o644, b"y")],
        ),
        ("duplicate-directory", vec![Spec::Dir("d"), Spec::Dir("d")]),
        (
            "case-fold",
            vec![
                Spec::File("Lib.d.ts", 0o644, b"x"),
                Spec::File("lib.d.ts", 0o644, b"y"),
            ],
        ),
        (
            "file-then-dir",
            vec![Spec::File("d", 0o644, b"x"), Spec::Dir("d")],
        ),
        (
            "dir-then-file",
            vec![Spec::Dir("d"), Spec::File("d", 0o644, b"x")],
        ),
        (
            "nested-below-file",
            vec![
                Spec::File("d", 0o644, b"x"),
                Spec::File("d/nested", 0o644, b"x"),
            ],
        ),
        (
            "case-folded-ancestor",
            vec![Spec::File("A/b", 0o644, b"x"), Spec::File("a", 0o644, b"x")],
        ),
    ];
    for (name, specs) in cases {
        let temp = TempDir::new(&format!("extract-dup-{name}"));
        let archive = write_archive(&temp, &npm_tar_gz(&specs));
        let error =
            extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{name}: {error:?}"
        );
        assert!(!destination(&temp).exists());
    }
}

#[test]
fn rejects_paths_above_the_component_budget() {
    let temp = TempDir::new("extract-components");
    let path = format!("package/{}", "a/".repeat(33));
    let mut blocks = Vec::new();
    blocks.extend_from_slice(&raw_header(path.as_bytes(), 1, b'0'));
    blocks.extend_from_slice(b"x");
    blocks.extend_from_slice(&[0_u8; 511]);
    blocks.extend_from_slice(&[0_u8; 1024]);
    let archive = write_archive(&temp, &raw_tar_gz(&blocks));
    let error = extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
    assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
    assert!(!destination(&temp).exists());
}

#[test]
fn rejects_an_oversized_file_header_before_reading_it() {
    let temp = TempDir::new("extract-oversize-file");
    let mut blocks = Vec::new();
    blocks.extend_from_slice(&raw_header(
        b"package/huge.bin",
        wxml_delivery::manifest::MAX_FILE_BYTES + 1,
        b'0',
    ));
    blocks.extend_from_slice(&[0_u8; 1024]);
    let archive = write_archive(&temp, &raw_tar_gz(&blocks));
    let error = extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
    assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
    assert!(!destination(&temp).exists());
}

#[test]
fn rejects_oversized_extension_headers_before_reading_them() {
    let temp = TempDir::new("extract-extension");
    let mut blocks = Vec::new();
    blocks.extend_from_slice(&raw_header(b"package/huge", 1024 * 1024 * 1024, b'L'));
    blocks.extend_from_slice(&[0_u8; 1024]);
    let archive = write_archive(&temp, &raw_tar_gz(&blocks));
    let started = std::time::Instant::now();
    let error = extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
    assert!(matches!(error, DeliveryError::SizeLimit(_)), "{error:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn rejects_pax_size_overrides_and_empty_archives() {
    let temp = TempDir::new("extract-pax");
    let record = pax_record("size", "999999");
    let mut blocks = Vec::new();
    blocks.extend_from_slice(&raw_header(b"package/pax", record.len() as u64, b'x'));
    blocks.extend_from_slice(record.as_bytes());
    let padding = (512 - record.len() % 512) % 512;
    blocks.extend_from_slice(&vec![0_u8; padding]);
    blocks.extend_from_slice(&raw_header(b"package/a", 1, b'0'));
    blocks.extend_from_slice(b"x");
    blocks.extend_from_slice(&[0_u8; 511]);
    blocks.extend_from_slice(&[0_u8; 1024]);
    let archive = write_archive(&temp, &raw_tar_gz(&blocks));
    let error = extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
    assert!(
        matches!(error, DeliveryError::UnsafeArchive(_)),
        "{error:?}"
    );

    let empty = TempDir::new("extract-empty");
    let archive = write_archive(&empty, &npm_tar_gz(&[]));
    let error = extract_npm_tar_gz(&archive, &destination(&empty), &empty_extract()).unwrap_err();
    assert!(
        matches!(error, DeliveryError::UnsafeArchive(_)),
        "{error:?}"
    );
    assert!(!destination(&empty).exists());
}

#[test]
fn refuses_to_extract_over_an_existing_destination() {
    let temp = TempDir::new("extract-existing");
    let archive = write_archive(&temp, &npm_tar_gz(&[Spec::File("a", 0o644, b"a")]));
    fs::create_dir_all(destination(&temp)).unwrap();
    let error = extract_npm_tar_gz(&archive, &destination(&temp), &empty_extract()).unwrap_err();
    assert!(matches!(error, DeliveryError::InstallState(_)), "{error:?}");
}
