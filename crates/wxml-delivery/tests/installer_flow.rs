//! End-to-end installer flows against a loopback fixture server.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty
)]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::{
    FixtureServer, TempDir, assert_failure, assert_success, installer, pack, pack_broken_binary,
    pack_with_extra, run, stderr, stdout,
};

fn install(root: &Path, source: &str, version: &str, extra: &[&str]) -> Output {
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().expect("utf-8 root"),
        "--source",
        source,
        "--enable-download",
        "--version",
        version,
    ]);
    command.args(extra);
    run(&mut command)
}

fn current(root: &Path) -> Output {
    let mut command = installer();
    command.args(["current", "--root", root.to_str().expect("utf-8 root")]);
    run(&mut command)
}

fn active_version(root: &Path) -> String {
    let link = fs::read_link(root.join("current")).expect("current link");
    link.components()
        .nth(1)
        .and_then(|component| component.as_os_str().to_str())
        .expect("version")
        .to_owned()
}

fn cache_archives(root: &Path) -> Vec<PathBuf> {
    let mut archives = Vec::new();
    let cache = root.join("cache");
    if let Ok(entries) = fs::read_dir(&cache) {
        for entry in entries.flatten() {
            let archive = entry.path().join("archive.tar.gz");
            if archive.is_file() {
                archives.push(archive);
            }
        }
    }
    archives
}

#[test]
fn installs_verifies_and_reports_a_path_with_spaces() {
    let base = TempDir::new("flow-success");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("install root with spaces");

    let result = install(&root, &server.url(), "0.1.0", &[]);
    assert_success(&result);
    assert!(
        stdout(&result).contains("installed 0.1.0"),
        "{}",
        stdout(&result)
    );
    assert!(root.join("current").exists());
    assert!(
        stdout(&result).contains("current/bin/wxml-lsp"),
        "install must print the stable current path: {}",
        stdout(&result)
    );
    assert!(
        !stdout(&result).contains("versions/0.1.0/bin/wxml-lsp"),
        "install must not pin the versioned path: {}",
        stdout(&result)
    );
    assert_eq!(active_version(&root), "0.1.0");
    assert!(!cache_archives(&root).is_empty());
    assert!(server.hits().iter().any(|hit| hit.ends_with("index.json")));
    assert!(server.hits().iter().any(|hit| hit.ends_with(".tar.gz")));

    let binary = root.join("current/bin/wxml-lsp");
    let version = Command::new(&binary)
        .arg("--version")
        .output()
        .expect("run installed binary");
    assert_success(&version);
    assert_eq!(stdout(&version).trim(), "wxml-lsp 0.1.0");

    let current_result = current(&root);
    assert_success(&current_result);
    assert!(stdout(&current_result).contains("version: 0.1.0"));
    assert!(stdout(&current_result).contains("binary:"));

    let mut verify = installer();
    verify.args(["verify", "--root", root.to_str().expect("root")]);
    let verify_result = run(&mut verify);
    assert_success(&verify_result);
    assert!(stdout(&verify_result).contains("verified 0.1.0"));

    let mut env = installer();
    env.args(["env", "--root", root.to_str().expect("root")]);
    let env_result = run(&mut env);
    assert_success(&env_result);
    assert!(stdout(&env_result).contains("WXML_LSP_BIN="));
    assert!(stdout(&env_result).contains("install root with spaces"));
}

#[test]
fn upgrades_and_rolls_back_to_a_verified_version() {
    let base = TempDir::new("flow-upgrade");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    pack(&output, "0.2.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");

    assert_success(&install(&root, &server.url(), "0.1.0", &[]));
    assert_eq!(active_version(&root), "0.1.0");

    let mut versions = installer();
    versions.args([
        "versions",
        "--source",
        &server.url(),
        "--enable-download",
        "--root",
        root.to_str().expect("root"),
    ]);
    let versions_result = run(&mut versions);
    assert_success(&versions_result);
    assert!(stdout(&versions_result).contains("0.1.0"));
    assert!(stdout(&versions_result).contains("0.2.0"));

    let mut upgrade = installer();
    upgrade.args([
        "upgrade",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
    ]);
    let upgrade_result = run(&mut upgrade);
    assert_success(&upgrade_result);
    assert_eq!(active_version(&root), "0.2.0");
    let upgraded = current(&root);
    assert_success(&upgraded);
    assert!(stdout(&upgraded).contains("version: 0.2.0"));
    assert!(stdout(&upgraded).contains("current/bin/wxml-lsp"));

    let mut rollback = installer();
    rollback.args([
        "rollback",
        "--root",
        root.to_str().expect("root"),
        "--to",
        "0.1.0",
    ]);
    let rollback_result = run(&mut rollback);
    assert_success(&rollback_result);
    assert_eq!(active_version(&root), "0.1.0");
    assert!(stdout(&rollback_result).contains("reported version: 0.1.0"));
    assert!(stdout(&rollback_result).contains("current/bin/wxml-lsp"));
    let rolled_back = current(&root);
    assert_success(&rolled_back);
    assert!(stdout(&rolled_back).contains("version: 0.1.0"));
}

#[test]
fn offline_rollback_can_restore_a_verified_cached_version() {
    let base = TempDir::new("flow-offline-rollback");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    pack(&output, "0.2.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    assert_success(&install(&root, &server.url(), "0.1.0", &[]));
    let mut upgrade = installer();
    upgrade.args([
        "upgrade",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
    ]);
    assert_success(&run(&mut upgrade));
    assert_eq!(active_version(&root), "0.2.0");
    drop(server);
    fs::remove_dir_all(root.join("versions/0.1.0")).unwrap();
    let mut rollback = installer();
    rollback.args([
        "rollback",
        "--root",
        root.to_str().expect("root"),
        "--to",
        "0.1.0",
        "--offline",
    ]);
    let result = run(&mut rollback);
    assert_success(&result);
    assert_eq!(active_version(&root), "0.1.0");
    assert!(stdout(&result).contains("rolled back to 0.1.0"));
}

#[test]
fn offline_install_uses_only_verified_cache_and_refetches_damage() {
    let base = TempDir::new("flow-offline");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");

    assert_success(&install(&root, &server.url(), "0.1.0", &[]));
    drop(server);
    fs::remove_dir_all(root.join("versions/0.1.0")).unwrap();

    let mut offline = installer();
    offline.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--version",
        "0.1.0",
        "--offline",
    ]);
    let offline_result = run(&mut offline);
    assert_success(&offline_result);
    assert!(
        stdout(&offline_result).contains("cache"),
        "{}",
        stdout(&offline_result)
    );
    assert_eq!(active_version(&root), "0.1.0");

    fs::remove_dir_all(root.join("versions/0.1.0")).unwrap();
    let archive = cache_archives(&root).pop().expect("cached archive");
    fs::write(&archive, b"damaged").unwrap();

    let offline_result = run(&mut offline);
    assert_failure(&offline_result, "cache");

    let server = FixtureServer::directory(&output);
    let online_result = install(&root, &server.url(), "0.1.0", &[]);
    assert_success(&online_result);
    assert_eq!(active_version(&root), "0.1.0");
    assert!(
        cache_archives(&root)
            .iter()
            .any(|path| { fs::read(path).is_ok_and(|bytes| bytes != b"damaged") })
    );
}

#[test]
fn concurrent_installs_are_serialized_and_consistent() {
    let base = TempDir::new("flow-concurrent");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");

    let mut first = installer();
    first.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let mut second = installer();
    second.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let first = first.spawn().expect("spawn first");
    let second = second.spawn().expect("spawn second");
    let first = first.wait_with_output().expect("wait first");
    let second = second.wait_with_output().expect("wait second");
    assert_success(&first);
    assert_success(&second);
    assert_eq!(active_version(&root), "0.1.0");
    assert_success(&current(&root));
}

#[test]
fn explicit_binary_wins_and_invalid_values_never_download() {
    let base = TempDir::new("flow-env");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");

    let explicit = base.path().join("custom-server");
    fs::write(
        &explicit,
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'wxml-lsp 9.9.9\\n'; fi\n",
    )
    .unwrap();
    wxml_delivery::manifest::set_mode(&explicit, 0o755).unwrap();

    let mut command = installer();
    command.env("WXML_LSP_BIN", &explicit).args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert!(stdout(&result).contains("takes precedence"));
    assert!(
        server.hits().is_empty(),
        "explicit binary must not download"
    );
    assert!(!root.exists());

    let missing = base.path().join("missing-server");
    let mut command = installer();
    command.env("WXML_LSP_BIN", &missing).args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "WXML_LSP_BIN");
    assert!(
        server.hits().is_empty(),
        "invalid explicit binary must not download"
    );
    assert!(!root.exists());
}

#[test]
fn relative_xdg_data_home_is_rejected_without_creating_a_root() {
    let base = TempDir::new("flow-relative-xdg");
    let mut command = installer();
    command.env("XDG_DATA_HOME", "relative-data").arg("list");
    let result = run(&mut command);
    assert_failure(&result, "absolute");
    assert!(!base.path().join("wxml-lsp").exists());
}

#[test]
fn read_only_commands_do_not_create_or_chmod_the_root() {
    let base = TempDir::new("flow-readonly");
    let root = base.path().join("private");
    fs::create_dir(&root).unwrap();
    wxml_delivery::manifest::set_mode(&root, 0o700).unwrap();
    let before = fs::read_dir(&root).unwrap().count();
    let mut list = installer();
    list.args(["list", "--root", root.to_str().expect("root")]);
    let result = run(&mut list);
    assert_success(&result);
    assert_eq!(fs::read_dir(&root).unwrap().count(), before);
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["current", "env", "verify"] {
        let mut command = installer();
        command.args([name, "--root", root.to_str().expect("root")]);
        let result = run(&mut command);
        assert!(!result.status.success());
        assert_eq!(fs::read_dir(&root).unwrap().count(), before);
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert!(!root.join("versions").exists());
    assert!(!root.join("tmp").exists());
    assert!(!root.join("cache").exists());
    assert!(!root.join(".lock").exists());
}

#[test]
fn verify_does_not_execute_a_tampered_binary() {
    let base = TempDir::new("flow-verify-order");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    assert_success(&install(&root, &server.url(), "0.1.0", &[]));
    let marker = base.path().join("EXECUTED");
    let binary = root.join("current/bin/wxml-lsp");
    fs::write(
        &binary,
        format!(
            "#!/bin/sh\ntouch {}\nprintf 'wxml-lsp 0.1.0\\n'\n",
            marker.display()
        ),
    )
    .unwrap();
    wxml_delivery::manifest::set_mode(&binary, 0o755).unwrap();
    let mut verify = installer();
    verify.args(["verify", "--root", root.to_str().expect("root")]);
    let result = run(&mut verify);
    assert!(!result.status.success());
    assert!(!marker.exists(), "verify executed a tampered binary");
}

#[test]
fn failed_runtime_check_keeps_the_previous_active_version() {
    let base = TempDir::new("flow-runtime-rollback");
    let good = base.path().join("good");
    let bad = base.path().join("bad");
    pack(&good, "0.1.0");
    pack_broken_binary(&bad, "0.3.0", "0.2.0-broken");
    let server = FixtureServer::directory(&good);
    let root = base.path().join("root");
    assert_success(&install(&root, &server.url(), "0.1.0", &[]));
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--from-dir",
        bad.to_str().expect("bad"),
        "--version",
        "0.3.0",
        "--offline",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "reports");
    assert_eq!(active_version(&root), "0.1.0");
    assert!(!root.join("versions/0.3.0").exists());
    assert_success(&current(&root));
}

#[test]
fn upgrade_refuses_downgrades_and_same_version_rebuilds() {
    let base = TempDir::new("flow-upgrade-policy");
    let high = base.path().join("high");
    let low = base.path().join("low");
    let rebuilt = base.path().join("rebuilt");
    pack(&high, "0.2.0");
    pack(&low, "0.1.0");
    pack_with_extra(&rebuilt, "0.2.0", "bin/extra.txt", b"new bytes");
    let server = FixtureServer::directory(&high);
    let root = base.path().join("root");
    assert_success(&install(&root, &server.url(), "0.2.0", &[]));
    let mut upgrade = installer();
    upgrade.args([
        "upgrade",
        "--root",
        root.to_str().expect("root"),
        "--from-dir",
        low.to_str().expect("low"),
        "--offline",
    ]);
    let result = run(&mut upgrade);
    assert_failure(&result, "downgrade");
    assert_eq!(active_version(&root), "0.2.0");

    let mut install = installer();
    install.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--from-dir",
        rebuilt.to_str().expect("rebuilt"),
        "--version",
        "0.2.0",
        "--offline",
    ]);
    let result = run(&mut install);
    assert_failure(&result, "different contents");
    assert_eq!(active_version(&root), "0.2.0");
    assert!(!root.join("current/bin/extra.txt").exists());
}

#[test]
fn reports_node_missing_old_and_broken_runtime() {
    let base = TempDir::new("flow-node");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    assert_success(&install(&root, &server.url(), "0.1.0", &[]));

    let missing = base.path().join("no-such-node");
    let mut verify = installer();
    verify
        .env("WXML_NODE", &missing)
        .args(["verify", "--root", root.to_str().expect("root")]);
    let result = run(&mut verify);
    assert_failure(&result, "Node.js");

    let old = base.path().join("old-node");
    fs::write(&old, b"#!/bin/sh\necho v20.0.0\n").unwrap();
    wxml_delivery::manifest::set_mode(&old, 0o755).unwrap();
    let mut verify = installer();
    verify
        .env("WXML_NODE", &old)
        .args(["verify", "--root", root.to_str().expect("root")]);
    let result = run(&mut verify);
    assert_failure(&result, "older");

    let typescript = root.join("current/bin/wxml-type-backend/node_modules/typescript");
    fs::remove_dir_all(&typescript).unwrap();
    let mut verify = installer();
    verify.args(["verify", "--root", root.to_str().expect("root")]);
    let result = run(&mut verify);
    assert_failure(&result, "backend");

    let binary = root.join("current/bin/wxml-lsp");
    wxml_delivery::manifest::set_mode(&binary, 0o644).unwrap();
    let mut verify = installer();
    verify.args(["verify", "--root", root.to_str().expect("root")]);
    let result = run(&mut verify);
    assert_failure(&result, "mode");
}

#[test]
fn installs_from_a_local_release_directory() {
    let base = TempDir::new("flow-local");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
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
        "--offline",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert_eq!(active_version(&root), "0.1.0");
}

#[test]
fn project_configuration_cannot_select_a_download_source() {
    let base = TempDir::new("flow-project");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let empty = base.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let bad = FixtureServer::directory(&empty);
    let project = base.path().join("project");
    fs::create_dir_all(project.join(".zed")).unwrap();
    fs::write(
        project.join(".wxml-lsp.json"),
        format!(
            "{{\"installSource\":\"{}\",\"enableDownload\":true}}",
            bad.url()
        ),
    )
    .unwrap();
    fs::write(
        project.join(".zed/settings.json"),
        format!(
            "{{\"lsp\":{{\"wxml-lsp\":{{\"source\":\"{}\"}}}}}}",
            bad.url()
        ),
    )
    .unwrap();
    let root = base.path().join("root");
    let mut command = installer();
    command.current_dir(&project).args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_success(&result);
    assert!(!server.hits().is_empty());
    assert!(bad.hits().is_empty(), "project config must never be used");
    assert_eq!(active_version(&root), "0.1.0");
}

#[test]
fn rollback_rejects_unverified_or_unknown_versions() {
    let base = TempDir::new("flow-rollback");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    assert_success(&install(&root, &server.url(), "0.1.0", &[]));
    let mut rollback = installer();
    rollback.args([
        "rollback",
        "--root",
        root.to_str().expect("root"),
        "--to",
        "9.9.9",
        "--offline",
    ]);
    let result = run(&mut rollback);
    assert_failure(&result, "cache");
    assert_eq!(active_version(&root), "0.1.0");
}

#[test]
fn download_requires_explicit_enablement() {
    let base = TempDir::new("flow-disabled");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        &server.url(),
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "enable-download");
    assert!(server.hits().is_empty());
    assert!(!root.join("versions").exists());
}

#[test]
fn rejects_untrusted_plain_http_sources() {
    let base = TempDir::new("flow-http");
    let root = base.path().join("root");
    let mut command = installer();
    command.args([
        "install",
        "--root",
        root.to_str().expect("root"),
        "--source",
        "http://example.com/releases/",
        "--enable-download",
        "--version",
        "0.1.0",
    ]);
    let result = run(&mut command);
    assert_failure(&result, "untrusted");
    assert!(!root.join("versions").exists());
}

#[test]
fn help_and_version_are_available() {
    let mut command = installer();
    command.arg("--help");
    let result = run(&mut command);
    assert_success(&result);
    assert!(stdout(&result).contains("install"));

    let mut command = installer();
    command.arg("--version");
    let result = run(&mut command);
    assert_success(&result);
    assert!(stdout(&result).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn stderr_is_not_used_for_success_output() {
    let base = TempDir::new("flow-stderr");
    let output = base.path().join("release");
    pack(&output, "0.1.0");
    let server = FixtureServer::directory(&output);
    let root = base.path().join("root");
    let result = install(&root, &server.url(), "0.1.0", &[]);
    assert_success(&result);
    assert_eq!(stderr(&result), "");
}
