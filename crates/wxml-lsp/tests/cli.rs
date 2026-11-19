//! End-to-end tests for the `wxml-lsp` binary.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_wxml-lsp");

#[test]
fn prints_package_version() {
    let output = Command::new(BIN)
        .arg("--version")
        .output()
        .expect("spawn wxml-lsp");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 output");
    assert_eq!(
        stdout.trim(),
        format!("wxml-lsp {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn rejects_unknown_arguments() {
    let output = Command::new(BIN)
        .arg("--nope")
        .output()
        .expect("spawn wxml-lsp");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(stderr.contains("unexpected argument"));
}

#[test]
fn prints_help() {
    let output = Command::new(BIN)
        .arg("--help")
        .output()
        .expect("spawn wxml-lsp");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 output");
    assert!(stdout.contains("Usage: wxml-lsp"), "{stdout}");
    assert!(stdout.contains("--stdio"), "{stdout}");
}

#[test]
fn requires_a_mode() {
    let output = Command::new(BIN).output().expect("spawn wxml-lsp");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(stderr.contains("a mode is required"), "{stderr}");
}

#[test]
fn rejects_extra_arguments() {
    let output = Command::new(BIN)
        .args(["--stdio", "--nope"])
        .output()
        .expect("spawn wxml-lsp");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}

#[test]
fn native_without_install_fails_closed() {
    let output = Command::new(BIN)
        .args(["--stdio", "--backend", "native"])
        .output()
        .expect("spawn wxml-lsp");
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "native startup must not write to the LSP stream: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(
        stderr.contains("native engine discovery failed"),
        "{stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("server.js"),
        "native startup must not probe the Node backend: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("wxml_node"),
        "native startup must not read Node configuration: {stderr}"
    );
}

#[test]
fn rejects_unknown_backend_value() {
    let output = Command::new(BIN)
        .args(["--stdio", "--backend", "tsgo"])
        .output()
        .expect("spawn wxml-lsp");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(stderr.contains("unsupported backend `tsgo`"), "{stderr}");
}

#[test]
fn backend_requires_a_value() {
    let output = Command::new(BIN)
        .args(["--stdio", "--backend"])
        .output()
        .expect("spawn wxml-lsp");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(stderr.contains("--backend` requires a value"), "{stderr}");
}
