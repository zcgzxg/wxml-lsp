//! Download, integrity and package-metadata failure paths.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty
)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;
use support::{
    FixtureServer, TempDir, assert_failure, assert_success, host_platform, installer, pack, run,
    stdout,
};

fn mutate_index(output: &Path, mutate: impl FnOnce(&mut Value)) {
    let path = output.join("index.json");
    let mut value: Value =
        serde_json::from_slice(&fs::read(&path).expect("read index")).expect("parse index");
    mutate(&mut value);
    fs::write(&path, serde_json::to_vec_pretty(&value).expect("serialize")).unwrap();
}

fn install_from(root: &Path, source: &str, version: &str) -> Output {
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        source,
        "--enable-download",
        "--version",
        version,
    ]);
    run(&mut command)
}

fn assert_no_install(root: &Path) {
    assert!(
        !root.join("current").exists(),
        "current link must not exist"
    );
    assert!(
        !root.join("versions").exists()
            || fs::read_dir(root.join("versions"))
                .unwrap()
                .next()
                .is_none(),
        "no half-installed version may remain"
    );
}

fn release_archive(output: &Path) -> PathBuf {
    let index: Value =
        serde_json::from_slice(&fs::read(output.join("index.json")).unwrap()).unwrap();
    output.join(index["releases"][0]["archive"].as_str().unwrap())
}

#[test]
fn rejects_a_bad_archive_hash() {
    let base = TempDir::new("sec-hash");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    mutate_index(&output, |index| {
        index["releases"][0]["sha256"] = Value::String("0".repeat(64));
    });
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert_failure(&result, "integrity");
    assert_no_install(&root);
    assert!(server.hits().iter().any(|hit| hit.ends_with(".tar.gz")));
}

#[test]
fn rejects_false_inline_boolean_flags_without_network() {
    let base = TempDir::new("sec-bool");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let mut command = installer();
    command.args([
        "versions",
        "--source",
        &server.url(),
        "--enable-download=false",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "download is disabled");
    assert!(server.hits().is_empty());
}

#[test]
fn rejects_an_index_with_an_unsupported_protocol_before_download() {
    let base = TempDir::new("sec-protocol");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    mutate_index(&output, |index| {
        index["releases"][0]["protocolVersion"] = Value::from(99);
    });
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert_failure(&result, "protocol");
    assert_no_install(&root);
    assert!(
        !server.hits().iter().any(|hit| hit.ends_with(".tar.gz")),
        "protocol validation must happen before archive download"
    );
}

#[test]
fn rejects_a_missing_requested_version() {
    let base = TempDir::new("sec-version");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.2.0");
    assert_failure(&result, "no release");
    assert_no_install(&root);
}

#[test]
fn rejects_an_oversized_archive_declaration_before_download() {
    let base = TempDir::new("sec-size");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    mutate_index(&output, |index| {
        index["releases"][0]["size"] = Value::from(wxml_delivery::manifest::MAX_ARCHIVE_BYTES + 1);
    });
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert_failure(&result, "size");
    assert_no_install(&root);
    assert!(!server.hits().iter().any(|hit| hit.ends_with(".tar.gz")));
}

#[test]
fn rejects_a_truncated_download() {
    let base = TempDir::new("sec-truncated");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::truncated_archive(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert!(
        !result.status.success(),
        "truncated download unexpectedly succeeded: {}",
        stdout(&result)
    );
    let combined = format!("{}{}", stdout(&result), support::stderr(&result));
    assert!(
        combined.to_lowercase().contains("download")
            || combined.to_lowercase().contains("error")
            || combined.to_lowercase().contains("byte"),
        "{combined}"
    );
    assert_no_install(&root);
}

#[test]
fn rejects_an_archive_redirect_in_the_index() {
    let base = TempDir::new("sec-redirect");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    mutate_index(&output, |index| {
        index["releases"][0]["archive"] =
            Value::String("http://example.com/evil.tar.gz".to_owned());
    });
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert_failure(&result, "archive");
    assert_no_install(&root);
    assert!(!server.hits().iter().any(|hit| hit.contains("evil")));
}

#[test]
fn rejects_a_manifest_whose_version_differs_from_the_index() {
    let base = TempDir::new("sec-manifest-version");
    let output = base.path().join("release");
    pack(&output, "0.2.0");
    let archive = release_archive(&output)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    mutate_index(&output, |index| {
        index["releases"][0]["version"] = Value::String("0.1.0".to_owned());
        index["releases"][0]["archive"] = Value::String(archive);
    });
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert_failure(&result, "manifest version");
    assert_no_install(&root);
}

#[test]
fn rejects_manifest_node_metadata_that_differs_from_the_index() {
    let base = TempDir::new("sec-manifest-node");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    mutate_index(&output, |index| {
        index["releases"][0]["nodeMinimumVersion"] = Value::String("99.0.0".to_owned());
    });
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install_from(&root, &server.url(), "0.1.0");
    assert_failure(&result, "manifest metadata");
    assert_no_install(&root);
}

#[test]
fn rejects_a_local_archive_symlink() {
    let base = TempDir::new("sec-local-link");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let archive = release_archive(&output);
    fs::remove_file(&archive).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &archive).unwrap();
    let root = base.path().join("root");
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--from-dir",
        output.to_str().expect("release"),
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "regular file");
    assert_no_install(&root);
}

#[test]
fn verify_package_accepts_a_valid_release() {
    let base = TempDir::new("sec-verify-package");
    let output = base.path().join("release");
    let report = pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let mut command = installer();
    command.args([
        "verify-package",
        "--source",
        &server.url(),
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert!(stdout(&result).contains(&report.sha256));
    assert_eq!(host_platform(), report.manifest.platform);
}
