//! F3-03 package-level acceptance: real lock, real tarballs, two builds.

use std::fs;
use std::process::{Command, Stdio};

use crate::real_asset_support::{TestDir, run_with_timeout, stdout};
use crate::support::{
    VERSION_N1, assert_byte_identical, assert_release_contents, build_release,
    provision_real_assets, resolve_real_inputs, write_input_tree,
};

fn packager_command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-package"));
    command
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    command
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn pinned_real_assets_provision_from_the_frozen_lock() {
    let Some(inputs) = resolve_real_inputs("real_assets_provision") else {
        return;
    };
    let base = TestDir::new("f3-provision");
    let provisioned = provision_real_assets(&base, &inputs);
    assert_eq!(provisioned.entries.len(), 2);
    assert_eq!(provisioned.lock.assets.len(), 2);
    assert_eq!(provisioned.lock_sha256.len(), 64);

    let mut plan = packager_command(&[
        "provision",
        "--lock",
        provisioned.lock_path.to_str().expect("UTF-8 lock path"),
        "--cache",
        provisioned.cache.to_str().expect("UTF-8 cache path"),
        "--print-plan",
    ]);
    let output = run_with_timeout(
        &mut plan,
        std::time::Duration::from_secs(120),
        "provision --print-plan",
    );
    assert!(
        output.status.success(),
        "provision --print-plan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = stdout(&output);
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("cache hit"))
            .count(),
        2,
        "{text}"
    );
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("source cache"))
            .count(),
        2,
        "{text}"
    );
    assert!(
        !text.contains(provisioned.cache.to_str().expect("UTF-8 cache path")),
        "CLI reports must not leak absolute cache paths: {text}"
    );

    let mut cached = packager_command(&[
        "provision",
        "--lock",
        provisioned.lock_path.to_str().expect("UTF-8 lock path"),
        "--cache",
        provisioned.cache.to_str().expect("UTF-8 cache path"),
        "--offline",
    ]);
    let output = run_with_timeout(
        &mut cached,
        std::time::Duration::from_secs(120),
        "provision cache-only",
    );
    assert!(
        output.status.success(),
        "cache-only provisioning failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = stdout(&output);
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("source cache"))
            .count(),
        2,
        "{text}"
    );
    println!(
        "PROVISION-CLI: lock sha256 {} cache hits 2, no download path",
        provisioned.lock_sha256
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn two_real_release_builds_are_byte_identical() {
    let Some(inputs) = resolve_real_inputs("real_release_deterministic") else {
        return;
    };
    let base = TestDir::new("f3-deterministic");
    let provisioned = provision_real_assets(&base, &inputs);
    let tree = write_input_tree(&base, &inputs, &provisioned);

    let first = build_release(
        &base.path().join("out-one"),
        &tree,
        &provisioned,
        VERSION_N1,
        true,
    );
    let second = build_release(
        &base.path().join("out-two"),
        &tree,
        &provisioned,
        VERSION_N1,
        true,
    );
    assert_release_contents(&first, &provisioned);
    assert_release_contents(&second, &provisioned);
    assert!(
        first.archive_size > 1_000_000,
        "real release archive is unexpectedly small: {} bytes",
        first.archive_size
    );
    assert!(first.archive_integrity.starts_with("sha512-"));
    let digests = assert_byte_identical(&first, &second);
    assert_eq!(digests.0, first.archive_sha256);
    assert_eq!(digests.1, first.manifest_sha256);
    assert!(
        fs::metadata(&first.archive)
            .expect("archive metadata")
            .len()
            > 0,
        "archive is empty"
    );
    println!(
        "F3-03: two independent real-asset builds are byte-identical \
         (archive/manifest/index/SHA256SUMS)"
    );
}
