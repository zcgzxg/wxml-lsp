//! Command-line coverage for the deterministic packager.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty
)]

mod support;

use std::process::Command;

use support::{TempDir, assert_failure, assert_success, fake_input, host_platform, run, stdout};

fn packager() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"))
}

#[test]
fn prints_version_platform_and_help() {
    let mut command = packager();
    command.arg("--print-version");
    let version = run(&mut command);
    assert_success(&version);
    assert_eq!(stdout(&version).trim(), env!("CARGO_PKG_VERSION"));

    let mut command = packager();
    command.arg("--print-platform");
    let platform = run(&mut command);
    assert_success(&platform);
    assert_eq!(stdout(&platform).trim(), host_platform());

    let mut command = packager();
    command.arg("--help");
    let help = run(&mut command);
    assert_success(&help);
    assert!(stdout(&help).contains("--input"));
    assert!(stdout(&help).contains("--output"));
}

#[test]
fn builds_a_package_and_rejects_bad_arguments() {
    let base = TempDir::new("packager-cli");
    let input = fake_input(base.path(), "0.1.0");
    let output = base.path().join("release");
    let platform = host_platform();
    let mut command = packager();
    command.args([
        "--input",
        input.to_str().expect("input"),
        "--output",
        output.to_str().expect("output"),
        "--release-version",
        "0.1.0",
        "--platform",
        platform.as_str(),
        "--protocol-version",
        "2",
        "--node-minimum",
        "24.0.0",
        "--built-at",
        "2026-01-01T00:00:00Z",
        "--mtime",
        "0",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert!(output.join("index.json").is_file());
    assert!(
        output
            .join(format!("wxml-lsp-0.1.0-{platform}.tar.gz"))
            .is_file()
    );

    let mut command = packager();
    command.args(["--input", "missing", "--output", "out"]);
    let result = run(&mut command);
    assert_failure(&result, "node-minimum");

    let mut command = packager();
    command.arg("--unknown");
    let result = run(&mut command);
    assert_failure(&result, "unexpected argument");
}

#[test]
fn dry_run_writes_no_output() {
    let base = TempDir::new("packager-dry");
    let input = fake_input(base.path(), "0.1.0");
    let output = base.path().join("release");
    let mut command = packager();
    command.args([
        "--input",
        input.to_str().expect("input"),
        "--output",
        output.to_str().expect("output"),
        "--node-minimum",
        "24.0.0",
        "--dry-run",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert!(stdout(&result).contains("dry-run"));
    assert!(!output.exists());
}
