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
