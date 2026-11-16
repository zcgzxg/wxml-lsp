//! Lightweight checks for the release script's local-only contract.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/build-release.sh")
}

#[test]
fn dry_run_does_not_write_and_publish_is_rejected() {
    let base = std::env::var_os("WXML_TEST_TMPDIR")
        .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
    let output = base.join(format!(
        "wxml-script-dry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let mut dry = Command::new("bash");
    dry.arg(script())
        .arg("--dry-run")
        .arg("--output")
        .arg(&output);
    let result = dry.output().expect("run build script");
    assert!(
        result.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("dry-run"));
    assert!(!output.exists(), "dry-run wrote output");

    let mut publish = Command::new("bash");
    publish
        .arg(script())
        .arg("--output")
        .arg(&output)
        .arg("--publish");
    let result = publish.output().expect("run build script");
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("publish"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.exists(), "rejected publish wrote output");
}
