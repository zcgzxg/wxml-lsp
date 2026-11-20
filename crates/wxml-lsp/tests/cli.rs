//! End-to-end tests for the `wxml-lsp` binary.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

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
    assert!(stdout.contains("--backend native"), "{stdout}");
    assert!(!stdout.contains("native|node"), "{stdout}");
    assert!(
        !stdout.contains("deprecated transitional Node backend"),
        "{stdout}"
    );
    assert!(!stdout.contains("--backend node"), "{stdout}");
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
        !stderr.contains("static WXML features only"),
        "explicit `--backend native` must not degrade to HostOnly: {stderr}"
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
#[allow(clippy::too_many_lines)]
fn default_without_install_serves_host_only() {
    let project = repo_fixture("events-basic");
    let root_uri = format!("file://{}", project.display());
    let uri = format!("{root_uri}/pages/index/index.wxml");
    let cwd = TempTree::new("host-only-cwd");
    let mut client = SessionClient::start(
        Path::new(BIN),
        &["--stdio"],
        &cwd.path,
        &[
            ("WXML_NODE", "/nonexistent/node"),
            ("WXML_TYPE_BACKEND", "/nonexistent/server.js"),
            ("WXML_BACKEND_DIR", "/nonexistent/backend"),
            ("WXML_LSP_BIN", "/nonexistent/bin/wxml-lsp"),
        ],
    );
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"capabilities": {}, "rootUri": root_uri},
    }));
    let initialize = client.response(1);
    assert!(
        initialize.get("result").is_some(),
        "HostOnly initialize failed: {initialize}"
    );
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {},
    }));
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": "<view>",
            },
        },
    }));
    let syntax = client.diagnostics(1);
    assert!(
        syntax["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["severity"] == serde_json::json!(1)),
        "HostOnly must keep template syntax diagnostics: {syntax}"
    );

    let template = "<view>{{ label }}</view>";
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"text": template}],
        },
    }));
    let type_status = client.diagnostics(2);
    assert!(
        type_status["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["message"].as_str().is_some_and(
                |message| message == "type checking is unavailable: type backend unavailable"
            )),
        "HostOnly must publish an unavailable type status: {type_status}"
    );

    let hover_position = lsp_position(template, template.find("label").expect("label in template"));
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/hover",
        "params": {"textDocument": {"uri": uri}, "position": hover_position},
    }));
    let hover = client.response(2);
    assert_eq!(
        hover["result"],
        serde_json::Value::Null,
        "HostOnly hover must be an honest null, not a fabricated type: {hover}"
    );
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "shutdown",
    }));
    let shutdown = client.response(3);
    assert_eq!(shutdown["result"], serde_json::Value::Null, "{shutdown}");
    let (status, stderr) = client.finish();
    assert!(
        status.success(),
        "default HostOnly session failed: {stderr}"
    );
    assert!(
        stderr.contains("native engine is not installed"),
        "HostOnly startup must name the missing engine: {stderr}"
    );
    assert!(
        stderr.contains("hover failed: type backend unavailable: native engine is not installed"),
        "HostOnly hover must fail with the stable unavailable reason: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("server.js"),
        "HostOnly must not probe the Node backend: {stderr}"
    );
}

#[test]
fn rejects_the_removed_node_backend() {
    let output = Command::new(BIN)
        .args(["--stdio", "--backend", "node"])
        .output()
        .expect("spawn wxml-lsp");
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "a rejected backend must not write to the LSP stream: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8(output.stderr).expect("utf-8 output");
    assert!(
        stderr.contains("the `node` backend was removed"),
        "explicit `--backend node` must be rejected: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("server.js"),
        "the removed Node backend must not be probed: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Damaged installed package: fail closed before the LSP handshake
// ---------------------------------------------------------------------------

fn build_package_skeleton(tag: &str) -> TempTree {
    let tree = TempTree::new(tag);
    let package = tree.path.join("version");
    fs::create_dir_all(package.join("bin")).expect("bin");
    fs::create_dir_all(package.join("engine/lib")).expect("engine lib");
    fs::create_dir_all(package.join("typings/miniprogram-api-typings")).expect("typings root");
    copy_file(Path::new(BIN), &package.join("bin/wxml-lsp"));
    tree
}

fn assert_default_fails_closed(package: &Path, expected: &str) {
    let unrelated = TempTree::new("damaged-cwd");
    let output = Command::new(package.join("bin/wxml-lsp"))
        .args(["--stdio"])
        .current_dir(&unrelated.path)
        .env_clear()
        .env("PATH", "/nonexistent")
        .stdin(Stdio::null())
        .output()
        .expect("spawn installed wxml-lsp");
    assert!(
        !output.status.success(),
        "a damaged install must fail closed for {expected}"
    );
    assert!(
        output.stdout.is_empty(),
        "failed default startup wrote to the LSP stream: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(expected), "{expected}: {stderr}");
    assert!(
        !stderr.contains("static WXML features only"),
        "only `NotInstalled` may degrade to HostOnly: {stderr}"
    );
}

#[test]
fn default_fails_closed_on_a_missing_typings_root() {
    let tree = build_package_skeleton("missing-typings");
    let package = tree.path.join("version");
    fs::remove_dir_all(package.join("typings")).expect("remove typings");
    assert_default_fails_closed(&package, "the installed package layout is unsafe");
}

#[test]
fn default_fails_closed_on_a_missing_engine() {
    let tree = build_package_skeleton("missing-engine");
    let package = tree.path.join("version");
    assert_default_fails_closed(&package, "the pinned native engine is missing");
}

#[test]
fn default_fails_closed_on_an_engine_artifact_mismatch() {
    let tree = build_package_skeleton("engine-mismatch");
    let package = tree.path.join("version");
    let engine = package.join("engine/lib/tsc");
    fs::write(&engine, b"").expect("empty engine");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).expect("engine mode");
    }
    assert_default_fails_closed(
        &package,
        "the pinned native engine does not match the pinned artifact",
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

fn install_manifest_value(package: &Path) -> serde_json::Value {
    let pin = PinnedEngine::pinned();
    let prefix = pin
        .version_line()
        .strip_suffix(pin.version())
        .unwrap_or("Version ");
    let files = manifest_file_entries(package);
    serde_json::json!({
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
    })
}

fn write_manifest_value(package: &Path, manifest: &serde_json::Value) {
    let path = package.join("manifest.json");
    let mut bytes = serde_json::to_vec_pretty(manifest).expect("manifest json");
    bytes.push(b'\n');
    fs::write(&path, bytes).expect("manifest write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("manifest mode");
    }
}

fn write_install_manifest(package: &Path) {
    write_manifest_value(package, &install_manifest_value(package));
}

/// Builds a temporary install tree shaped like a real native package.
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

fn lsp_position(text: &str, byte: usize) -> serde_json::Value {
    assert!(text.is_char_boundary(byte), "offset {byte} is a boundary");
    let prefix = &text[..byte];
    let line = prefix.matches('\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = prefix[line_start..].encode_utf16().count();
    serde_json::json!({"line": line, "character": character})
}

fn repo_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures")
        .join(name)
        .canonicalize()
        .expect("repository fixture root")
}

fn standard_payloads(root_uri: Option<&str>) -> Vec<serde_json::Value> {
    let initialize = root_uri.map_or_else(
        || serde_json::json!({"capabilities": {}}),
        |root| serde_json::json!({"capabilities": {}, "rootUri": root}),
    );
    vec![
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": initialize,
        }),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {},
        }),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
    ]
}

/// Spawns one framed LSP session with a clean environment and a Node tripwire.
fn run_framed_session(
    executable: &Path,
    args: &[&str],
    cwd: &Path,
    extra_env: &[(&str, &str)],
    payloads: &[serde_json::Value],
) -> std::process::Output {
    let (_tripwire, path, marker) = tripwire_directory();
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .env("PATH", &path)
        .env("WXML_TRIPWIRE_MARKER", &marker)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("spawn wxml-lsp");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for payload in payloads {
            stdin.write_all(&frame(payload)).expect("write frame");
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("session output");
    assert!(
        !marker.exists(),
        "a Node tripwire executed: {:?}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
    output
}

const POISON_ENV: &[(&str, &str)] = &[
    ("WXML_NODE", "/nonexistent/node"),
    ("WXML_TYPE_BACKEND", "/nonexistent/server.js"),
    ("WXML_BACKEND_DIR", "/nonexistent/backend"),
];

fn read_message(reader: &mut impl BufRead) -> Option<serde_json::Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0_u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// A framed client that waits for individual responses and notifications.
struct SessionClient {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<std::process::ChildStdout>,
    _tripwire: TempTree,
    marker: PathBuf,
}

impl SessionClient {
    fn start(executable: &Path, args: &[&str], cwd: &Path, extra_env: &[(&str, &str)]) -> Self {
        let (tripwire, path, marker) = tripwire_directory();
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_clear()
            .env("PATH", &path)
            .env("WXML_TRIPWIRE_MARKER", &marker)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in extra_env {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("spawn wxml-lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self {
            child,
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
            _tripwire: tripwire,
            marker,
        }
    }

    fn send(&mut self, payload: &serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("stdin");
        stdin.write_all(&frame(payload)).expect("write frame");
        stdin.flush().expect("flush frame");
    }

    fn response(&mut self, id: i64) -> serde_json::Value {
        loop {
            let message = read_message(&mut self.reader).expect("server closed stdout");
            if message.get("id").and_then(serde_json::Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
        }
    }

    fn diagnostics(&mut self, version: i64) -> serde_json::Value {
        loop {
            let message = read_message(&mut self.reader).expect("server closed stdout");
            if message.get("method").and_then(serde_json::Value::as_str)
                == Some("textDocument/publishDiagnostics")
                && message["params"]["version"].as_i64() == Some(version)
            {
                return message;
            }
        }
    }

    fn finish(mut self) -> (std::process::ExitStatus, String) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit",
            "params": null,
        }));
        drop(self.stdin.take());
        let status = self.child.wait().expect("child exit");
        let mut stderr = String::new();
        if let Some(mut child_stderr) = self.child.stderr.take() {
            child_stderr.read_to_string(&mut stderr).expect("stderr");
        }
        assert!(
            !self.marker.exists(),
            "a Node tripwire executed: {:?}",
            fs::read_to_string(&self.marker).unwrap_or_default()
        );
        (status, stderr)
    }
}

fn run_native_session(package: &Path, cwd: &Path) -> std::process::Output {
    let executable = package.join("bin/wxml-lsp");
    run_framed_session(
        &executable,
        &["--stdio", "--backend", "native"],
        cwd,
        POISON_ENV,
        &standard_payloads(None),
    )
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
fn optional_real_install_tree_serves_native_stdio_by_default() {
    let Some((engine, typings)) = real_inputs() else {
        return;
    };
    let tree = build_install_tree("serve-default", &engine, &typings);
    let package = tree.path.join("version");
    let unrelated = tree.path.join("unrelated-cwd");
    fs::create_dir_all(&unrelated).expect("unrelated cwd");
    let project = repo_fixture("events-basic");
    assert!(
        project.join("app.json").is_file(),
        "fixture root is wrong: {}",
        project.display()
    );
    let root_uri = format!("file://{}", project.display());
    let uri = format!("{root_uri}/pages/index/index.wxml");
    let template = "<view>{{ label }}</view>";

    let executable = package.join("bin/wxml-lsp");
    let mut client = SessionClient::start(&executable, &["--stdio"], &unrelated, POISON_ENV);
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"capabilities": {}, "rootUri": root_uri},
    }));
    let initialize = client.response(1);
    assert!(
        initialize.get("result").is_some(),
        "default native initialize failed: {initialize}"
    );
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {},
    }));
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "wxml",
                "version": 1,
                "text": template,
            },
        },
    }));
    let hover_position = lsp_position(template, template.find("label").expect("label in template"));
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/hover",
        "params": {"textDocument": {"uri": uri}, "position": hover_position},
    }));
    let hover = client.response(2);
    client.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "shutdown",
    }));
    assert_eq!(client.response(3)["result"], serde_json::Value::Null);
    let (status, stderr) = client.finish();
    let hover_text = hover["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "default native hover returned no type for {root_uri}: {hover}; stderr: {stderr}"
            )
        });
    assert!(
        hover_text.contains("string"),
        "default native hover lost the script type: {hover}; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("native engine is not installed"),
        "default `--stdio` degraded to HostOnly despite an installed package: {stderr}"
    );
    assert!(
        !stderr.contains("native engine discovery failed")
            && !stderr.contains("type assets are unavailable"),
        "installed default native session failed before the handshake: {stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("server.js"),
        "default native session probed the Node backend: {stderr}"
    );
    assert!(
        status.success(),
        "default native session did not exit cleanly: {stderr}"
    );
}

#[test]
fn optional_real_install_tree_default_fails_closed_without_typings() {
    let Some((engine, typings)) = real_inputs() else {
        return;
    };
    let tree = build_install_tree("default-missing-assets", &engine, &typings);
    let package = tree.path.join("version");
    fs::remove_dir_all(package.join("typings/miniprogram-api-typings")).expect("remove typings");
    let output = Command::new(package.join("bin/wxml-lsp"))
        .args(["--stdio"])
        .stdin(Stdio::null())
        .env_clear()
        .env("PATH", "/nonexistent")
        .output()
        .expect("spawn installed wxml-lsp");
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "failed default startup wrote to the LSP stream: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("native type assets are unavailable"),
        "missing assets must fail closed: {stderr}"
    );
    assert!(
        !stderr.contains("static WXML features only"),
        "an installed package must never degrade to HostOnly: {stderr}"
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
