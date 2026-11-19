//! End-to-end tests for the `wxml-lsp` binary.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};
use wxml_backend_tsgo::config::PinnedEngine;

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

// ---------------------------------------------------------------------------
// Opt-in real install layout
// ---------------------------------------------------------------------------

fn required_engine() -> bool {
    matches!(
        std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
        Ok("1" | "true")
    )
}

fn skip_or_fail(message: &str, required: bool) {
    assert!(!required, "{message}");
    eprintln!("optional real install layout test skipped: {message}");
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn real_engine_dir() -> Option<PathBuf> {
    let bin = env_path("TSGO_BIN")?;
    let bin = fs::canonicalize(bin).ok()?;
    let directory = bin.parent()?.to_path_buf();
    directory.join("lib.d.ts").is_file().then_some(directory)
}

fn real_typings_package() -> Option<PathBuf> {
    let root = env_path("TSGO_TYPINGS_ROOT")
        .or_else(|| env_path("TSGO_ASSET_ROOT").map(|asset| asset.join("typings")))?;
    let package =
        if root.file_name().and_then(|name| name.to_str()) == Some("miniprogram-api-typings") {
            root
        } else {
            root.join("miniprogram-api-typings")
        };
    package.join("index.d.ts").is_file().then_some(package)
}

fn real_inputs() -> Option<(PathBuf, PathBuf)> {
    let required = required_engine();
    let Some(engine) = real_engine_dir() else {
        skip_or_fail("TSGO_BIN must point at the pinned engine lib/tsc", required);
        return None;
    };
    let Some(typings) = real_typings_package() else {
        skip_or_fail(
            "TSGO_TYPINGS_ROOT (or TSGO_ASSET_ROOT/typings) must provide miniprogram-api-typings",
            required,
        );
        return None;
    };
    Some((engine, typings))
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new(tag: &str) -> Self {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        fs::create_dir_all(&base).expect("test base");
        let path = base.join(format!(
            "wxml-lsp-cli-install-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir(&path).expect("temp tree");
        Self { path }
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn copy_file(source: &Path, destination: &Path) {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).expect("destination parent");
    }
    fs::copy(source, destination).expect("copy fixture file");
}

fn copy_flat_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("flat destination");
    for entry in fs::read_dir(source).expect("source directory") {
        let entry = entry.expect("directory entry");
        let metadata = entry.metadata().expect("entry metadata");
        if metadata.is_file() {
            copy_file(&entry.path(), &destination.join(entry.file_name()));
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("tree destination");
    for entry in fs::read_dir(source).expect("source tree") {
        let entry = entry.expect("tree entry");
        let target = destination.join(entry.file_name());
        let metadata = entry.metadata().expect("tree metadata");
        if metadata.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if metadata.is_file() {
            copy_file(&entry.path(), &target);
        }
    }
}

fn sha256_file(path: &Path) -> String {
    use std::fmt::Write as _;
    let bytes = fs::read(path).expect("hash file");
    let digest = Sha256::digest(&bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("hex");
    }
    encoded
}

#[cfg(unix)]
fn mode_of(path: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .expect("mode metadata")
        .permissions()
        .mode()
        & 0o777;
    format!("{mode:04o}")
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> String {
    "0644".to_owned()
}

fn manifest_file_entries(package: &Path) -> Vec<serde_json::Value> {
    let mut files = Vec::new();
    let mut stack = vec![package.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("package directory") {
            let entry = entry.expect("package entry");
            let path = entry.path();
            let metadata = entry.metadata().expect("package metadata");
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(package)
                .expect("package relative path")
                .to_string_lossy()
                .replace('\\', "/");
            files.push(serde_json::json!({
                "path": relative,
                "sha256": sha256_file(&path),
                "size": metadata.len(),
                "mode": mode_of(&path),
            }));
        }
    }
    files.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["path"].as_str().unwrap_or_default())
    });
    files
}

fn write_install_manifest(package: &Path) {
    let pin = PinnedEngine::pinned();
    let prefix = pin
        .version_line()
        .strip_suffix(pin.version())
        .unwrap_or("Version ");
    let files = manifest_file_entries(package);
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "package": "wxml-lsp",
        "packageVersion": env!("CARGO_PKG_VERSION"),
        "platform": "linux-x86_64",
        "protocolVersion": 2,
        "builtAt": "2026-11-19T00:00:00Z",
        "engine": {
            "name": "typescript",
            "version": pin.version(),
            "sourceCommit": pin.git_head(),
            "distribution": pin.package(),
            "distributionUrl": "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz",
            "distributionIntegrity": pin.sri(),
            "distributionSha256": pin.tarball_sha256(),
            "distributionSize": 10_175_135,
            "distributionFileCount": pin.file_count(),
            "distributionUnpackedSize": pin.unpacked_bytes(),
            "apiSchemaVersion": pin.ast_protocol_version(),
            "astHeaderBytes": pin.ast_header_bytes(),
            "license": "Apache-2.0",
            "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
            "binary": pin.executable(),
            "probe": {"args": ["--version"], "expectPrefix": prefix},
        },
        "typeAssets": [],
        "binaries": [],
        "files": files,
    });
    let path = package.join("manifest.json");
    let mut bytes = serde_json::to_vec_pretty(&manifest).expect("manifest json");
    bytes.push(b'\n');
    fs::write(&path, bytes).expect("manifest write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("manifest mode");
    }
}

/// Builds a temporary install tree shaped like a real v2 package.
fn build_install_tree(tag: &str, engine: &Path, typings: &Path) -> TempTree {
    let tree = TempTree::new(tag);
    let package = tree.path.join("version");
    fs::create_dir_all(package.join("bin")).expect("bin");
    fs::create_dir_all(package.join("engine/lib")).expect("engine lib");
    fs::create_dir_all(package.join("typings/miniprogram-api-typings")).expect("typings root");
    copy_file(Path::new(BIN), &package.join("bin/wxml-lsp"));
    copy_flat_directory(engine, &package.join("engine/lib"));
    copy_tree(typings, &package.join("typings/miniprogram-api-typings"));
    write_install_manifest(&package);
    tree
}

fn tripwire_directory() -> (TempTree, PathBuf, PathBuf) {
    let base = TempTree::new("tripwire");
    let directory = base.path.join("bin");
    fs::create_dir_all(&directory).expect("tripwire bin");
    let marker = base.path.join("executed");
    for name in ["node", "npm", "npx"] {
        let script = directory.join(name);
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$0\" >> \"$WXML_TRIPWIRE_MARKER\"\n",
        )
        .expect("tripwire script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("tripwire mode");
        }
    }
    (base, directory, marker)
}

fn frame(payload: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(payload).expect("frame json");
    let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    framed.extend_from_slice(&body);
    framed
}

fn run_native_session(package: &Path, cwd: &Path) -> std::process::Output {
    let (_tripwire, path, marker) = tripwire_directory();
    let mut child = Command::new(package.join("bin/wxml-lsp"))
        .args(["--stdio", "--backend", "native"])
        .env_clear()
        .env("PATH", &path)
        .env("WXML_TRIPWIRE_MARKER", &marker)
        .env("WXML_NODE", "/nonexistent/node")
        .env("WXML_TYPE_BACKEND", "/nonexistent/server.js")
        .env("WXML_BACKEND_DIR", "/nonexistent/backend")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn installed wxml-lsp");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for payload in [
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"capabilities": {}},
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {},
            }),
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
            serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
        ] {
            stdin.write_all(&frame(&payload)).expect("write frame");
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("native session output");
    assert!(
        !marker.exists(),
        "a Node tripwire executed: {:?}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
    output
}

#[test]
fn optional_real_install_tree_serves_native_stdio_without_path_node() {
    let Some((engine, typings)) = real_inputs() else {
        return;
    };
    let tree = build_install_tree("serve", &engine, &typings);
    let package = tree.path.join("version");
    let unrelated = tree.path.join("unrelated-cwd");
    fs::create_dir_all(&unrelated).expect("unrelated cwd");

    let output = run_native_session(&package, &unrelated);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("native engine discovery failed")
            && !stderr.contains("type assets are unavailable"),
        "installed native session failed before the handshake: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("server.js"),
        "native session probed the Node backend: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"id\":1") && stdout.contains("\"capabilities\""),
        "initialize response missing: {stdout}"
    );
    assert!(
        output.status.success(),
        "native session did not exit cleanly: {stderr}"
    );
}

#[test]
fn optional_real_install_tree_without_typings_fails_closed() {
    let Some((engine, typings)) = real_inputs() else {
        return;
    };
    let tree = build_install_tree("missing-assets", &engine, &typings);
    let package = tree.path.join("version");
    fs::remove_dir_all(package.join("typings/miniprogram-api-typings")).expect("remove typings");
    let output = Command::new(package.join("bin/wxml-lsp"))
        .args(["--stdio", "--backend", "native"])
        .stdin(Stdio::null())
        .env_clear()
        .env("PATH", "/nonexistent")
        .output()
        .expect("spawn installed wxml-lsp");
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "failed native startup wrote to the LSP stream: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("native type assets are unavailable"),
        "missing assets must fail closed: {stderr}"
    );
}
