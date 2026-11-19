//! F3-09 acceptance against a *real* legacy schema v1 release package.
//!
//! The package is not a fixture: it is built by the production schema v1
//! `wxml-lsp-package` from the real release binaries, the real
//! `backends/typescript` build output and the real `editors/zed` wasm. The
//! opt-in script `scripts/f3-legacy-real.sh` produces it and points this test
//! at it through `WXML_F3_LEGACY_PACKAGE`; the native target is the shared
//! pinned real-asset channel used by the rest of the F3 acceptance matrix.
//!
//! Required mode (`WXML_REQUIRE_LEGACY_PACKAGE=1|true`) fails closed when the
//! real package is missing or malformed. Without the variable the test prints
//! an explicit `SKIP` and returns, so the default CI never needs Node, npm or
//! the pinned tarballs.
//!
//! Nothing here changes production code, Cargo metadata, the default backend
//! or the Node tree. The test is `#[ignore]`d and only runs when named.

#![allow(
    dead_code,
    unused_imports,
    unreachable_pub,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::missing_const_for_fn,
    clippy::similar_names,
    clippy::uninlined_format_args,
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps,
    clippy::redundant_closure_for_method_calls,
    clippy::map_unwrap_or,
    clippy::if_not_else,
    clippy::manual_let_else,
    clippy::option_if_let_else,
    clippy::items_after_statements,
    clippy::doc_markdown,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::redundant_clone
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wxml_delivery::error::DeliveryError;
use wxml_delivery::manifest::{
    ExpectedManifest, Manifest, PACKAGE_NAME, read_index, read_manifest, sha256_file,
};
use wxml_delivery::manifest_v2::{ManifestSchema, read_legacy_manifest, read_manifest_schema};
use wxml_delivery::native_install::FailPoint;
use wxml_delivery::paths::host_platform;
use wxml_delivery::state::Journal;
use wxml_delivery::verify_v2::{collect_tree, tree_fingerprint};

#[path = "real_asset/support.rs"]
mod real_asset_support;

#[path = "f3_real_release/support.rs"]
mod release;

#[path = "f3_acceptance/support.rs"]
mod support;

use release::VERSION_N2;
use support::{
    TestDir, Tripwires, active_version, assert_contains, assert_failure, assert_journal_empty,
    assert_state, assert_success, assert_tmp_empty, combined, evidence, install_with_failpoint,
    installed_versions, installer_bin, mutate_release, open_root, private_root, remove_typings,
    run_installer, shared_channel, skip, stage_release_tree, state_bytes, stdout, workspace,
};

const PACKAGE_ENV: &str = "WXML_F3_LEGACY_PACKAGE";
const REQUIRE_ENV: &str = "WXML_REQUIRE_LEGACY_PACKAGE";
const REAL_NODE_ENV: &str = "WXML_F3_REAL_NODE";

/// Opt-in legacy package identity, validated by the schema v1 reader and the
/// production installer before it is used.
struct LegacyPackage {
    output: PathBuf,
    release_dir: PathBuf,
    archive: PathBuf,
    version: String,
    platform: String,
    protocol: u64,
    node_minimum: String,
    backend_version: String,
    built_at: String,
    file_count: usize,
    archive_sha256: String,
    archive_size: u64,
    binary_sha256: String,
    backend_sha256: String,
}

impl LegacyPackage {
    fn resolve() -> Option<Self> {
        let raw = match std::env::var_os(PACKAGE_ENV) {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => {
                assert!(
                    !required_mode(),
                    "{REQUIRE_ENV} is set but {PACKAGE_ENV} is missing; build the real v1 \
                     package with scripts/f3-legacy-real.sh"
                );
                skip(
                    "F3-09",
                    format!(
                        "real legacy schema v1 package is absent; set {PACKAGE_ENV} to the \
                         output of scripts/f3-legacy-real.sh"
                    ),
                );
                return None;
            }
        };
        let output = fs::canonicalize(&raw).unwrap_or_else(|error| {
            panic!("cannot resolve {PACKAGE_ENV}={}: {error}", raw.display())
        });
        assert!(
            output.is_dir(),
            "{PACKAGE_ENV} must be a release output directory, got {}",
            output.display()
        );
        let index_path = output.join("index.json");
        assert!(
            index_path.is_file(),
            "{PACKAGE_ENV} has no index.json: {}",
            output.display()
        );
        let index = read_index(&index_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", index_path.display()));
        let platform = host_platform().expect("host platform");
        let release = index
            .releases
            .iter()
            .find(|release| release.platform == platform)
            .unwrap_or_else(|| {
                panic!(
                    "legacy index has no release for {platform}: {:?}",
                    index.releases
                )
            });
        let release_dir = output.join(format!("wxml-lsp-{}-{}", release.version, release.platform));
        let archive = output.join(&release.archive);
        assert!(
            release_dir.is_dir(),
            "legacy release directory is missing: {}",
            release_dir.display()
        );
        assert!(
            archive.is_file(),
            "legacy release archive is missing: {}",
            archive.display()
        );
        assert_eq!(
            read_manifest_schema(&release_dir).expect("legacy manifest schema"),
            ManifestSchema::V1,
            "the package must be schema v1"
        );
        let projection =
            read_legacy_manifest(&release_dir).expect("legacy manifest identity projection");
        assert_eq!(projection.package, PACKAGE_NAME);
        assert_eq!(projection.package_version, release.version);
        assert_eq!(projection.platform, platform);
        let manifest = read_manifest(
            &release_dir,
            ExpectedManifest {
                version: Some(&release.version),
                platform: Some(&release.platform),
                protocol_version: Some(release.protocol_version),
            },
        )
        .expect("validate legacy manifest");
        let actual_sha = sha256_file(&archive).expect("hash legacy archive");
        assert_eq!(
            actual_sha, release.sha256,
            "legacy archive does not match index.json"
        );
        let actual_size = fs::metadata(&archive)
            .expect("legacy archive metadata")
            .len();
        assert_eq!(
            actual_size, release.size,
            "legacy archive size does not match index.json"
        );

        // A real package, not a shell-script fixture: the Rust entry is a
        // native ELF executable and the Node payload is the real TypeScript
        // backend with real dependency manifests.
        let binary = release_dir.join("bin/wxml-lsp");
        let binary_bytes = fs::read(&binary).expect("read legacy Rust binary");
        assert!(
            binary_bytes.starts_with(b"\x7fELF"),
            "legacy bin/wxml-lsp is not an ELF binary"
        );
        assert!(
            binary_bytes.len() > 512 * 1024,
            "legacy bin/wxml-lsp is too small for a real release binary: {} bytes",
            binary_bytes.len()
        );
        let backend = release_dir.join("bin/wxml-type-backend/server.js");
        let backend_bytes = fs::read(&backend).expect("read legacy Node backend");
        assert!(
            backend_bytes.windows(6).any(|window| window == b"import"),
            "legacy server.js does not look like the real ESM backend"
        );
        assert!(
            backend_bytes.len() > 4 * 1024,
            "legacy server.js is too small for the real backend: {} bytes",
            backend_bytes.len()
        );
        let typescript =
            release_dir.join("bin/wxml-type-backend/node_modules/typescript/package.json");
        let typescript_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&typescript).expect("read typescript package.json"))
                .expect("parse typescript package.json");
        assert_eq!(
            typescript_json.get("name").and_then(|value| value.as_str()),
            Some("typescript")
        );
        let typings = release_dir
            .join("bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json");
        let typings_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&typings).expect("read typings package.json"))
                .expect("parse typings package.json");
        assert_eq!(
            typings_json.get("name").and_then(|value| value.as_str()),
            Some("miniprogram-api-typings")
        );
        let binary_sha = manifest
            .binaries
            .iter()
            .find(|artifact| artifact.path == "bin/wxml-lsp")
            .map(|artifact| artifact.sha256.clone())
            .expect("legacy manifest names bin/wxml-lsp");
        assert_eq!(
            binary_sha,
            sha256_file(&binary).expect("hash legacy binary")
        );
        let backend_sha = manifest
            .binaries
            .iter()
            .find(|artifact| artifact.path == "bin/wxml-type-backend/server.js")
            .map(|artifact| artifact.sha256.clone())
            .expect("legacy manifest names the Node backend");
        assert_eq!(
            backend_sha,
            sha256_file(&backend).expect("hash legacy Node backend")
        );
        Some(Self {
            output: output.clone(),
            release_dir,
            archive,
            version: release.version.clone(),
            platform: release.platform.clone(),
            protocol: release.protocol_version,
            node_minimum: release.node_minimum_version.clone(),
            backend_version: manifest.node.backend_version.clone(),
            built_at: manifest.built_at.clone(),
            file_count: manifest.files.len(),
            archive_sha256: actual_sha,
            archive_size: actual_size,
            binary_sha256: binary_sha,
            backend_sha256: backend_sha,
        })
    }

    fn identity(&self) -> String {
        format!(
            "version={} platform={} protocol={} node>={} backend={} builtAt={} files={} \
             archiveSha256={} archiveSize={} binarySha256={} backendSha256={}",
            self.version,
            self.platform,
            self.protocol,
            self.node_minimum,
            self.backend_version,
            self.built_at,
            self.file_count,
            self.archive_sha256,
            self.archive_size,
            self.binary_sha256,
            self.backend_sha256
        )
    }

    fn assert_tree_unchanged(
        &self,
        root: &Path,
        expected_fingerprint: &str,
        expected_current: &str,
        label: &str,
    ) {
        let version_dir = root.join("versions").join(&self.version);
        assert_eq!(
            tree_fingerprint(&version_dir).expect("legacy tree fingerprint"),
            expected_fingerprint,
            "{label}: legacy tree bytes changed"
        );
        assert_eq!(
            sha256_file(&version_dir.join("bin/wxml-lsp")).expect("hash installed binary"),
            self.binary_sha256,
            "{label}: legacy Rust binary changed"
        );
        assert_eq!(
            sha256_file(&version_dir.join("bin/wxml-type-backend/server.js"))
                .expect("hash installed backend"),
            self.backend_sha256,
            "{label}: legacy Node backend changed"
        );
        assert_eq!(
            read_manifest_schema(&version_dir).expect("installed legacy schema"),
            ManifestSchema::V1,
            "{label}: installed legacy manifest stopped being schema v1"
        );
        let link = fs::read_link(root.join("current")).expect("read current link");
        assert_eq!(
            link,
            PathBuf::from("versions").join(expected_current),
            "{label}: current link changed"
        );
    }
}

fn hard_gate(name: &str) -> bool {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value.is_empty() => false,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
        Ok(value) => panic!("{name} must be unset, empty, `1` or `true`; got `{value}`"),
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} must be valid UTF-8"),
    }
}

fn required_mode() -> bool {
    hard_gate(REQUIRE_ENV) || hard_gate("WXML_REQUIRE_REAL_ENGINE")
}

fn install_legacy_real(
    package: &LegacyPackage,
    install_bin: &Path,
    root: &Path,
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> std::process::Output {
    run_installer(
        install_bin,
        &[
            "install",
            "--root",
            root.to_str().expect("UTF-8 legacy root"),
            "--from-dir",
            package.output.to_str().expect("UTF-8 legacy output"),
            "--version",
            &package.version,
            "--offline",
        ],
        tripwires,
        work,
        label,
    )
}

fn assert_no_state(root: &Path, label: &str) {
    assert!(
        state_bytes(root).is_none(),
        "{label}: state.json must not exist for a legacy root"
    );
}

fn assert_no_legacy_process(root: &Path) {
    #[cfg(target_os = "linux")]
    {
        let root_text = root.to_string_lossy();
        let mut found = Vec::new();
        let Ok(entries) = fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            if bytes.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes).replace('\0', " ");
            if (text.contains("wxml-type-backend") || text.contains("server.js"))
                && (text.contains(root_text.as_ref()) || text.contains("wxml-lsp"))
            {
                found.push(format!("{}: {text}", name.to_string_lossy()));
            }
        }
        assert!(
            found.is_empty(),
            "legacy Node/server.js process observed after the command: {found:?}"
        );
    }
}

fn real_node() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(REAL_NODE_ENV) {
        if !value.is_empty() {
            let path = PathBuf::from(value);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join("node");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn lsp_frame(value: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).expect("serialize LSP frame");
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    frame
}

/// Runs the installed legacy `wxml-lsp --stdio --backend node` entry against
/// the packaged `wxml-type-backend/server.js` with a real Node runtime.
fn run_legacy_node_smoke(binary: &Path, node: &Path, work: &Path) -> Result<String, String> {
    let payload_path = work.join("legacy-lsp-smoke.bin");
    let mut payload = Vec::new();
    for message in [
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"processId": serde_json::Value::Null, "rootUri": serde_json::Value::Null, "capabilities": {}}
        }),
        serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": serde_json::Value::Null}),
        serde_json::json!({"jsonrpc": "2.0", "method": "exit", "params": serde_json::Value::Null}),
    ] {
        payload.extend_from_slice(&lsp_frame(&message));
    }
    fs::write(&payload_path, &payload).expect("write LSP smoke input");
    let input = fs::File::open(&payload_path).expect("open LSP smoke input");
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let node_dir = node.parent().expect("node parent");
    let mut path_value = std::ffi::OsString::from(node_dir);
    for directory in std::env::split_paths(&path_env) {
        path_value.push(":");
        path_value.push(directory);
    }
    let mut command = Command::new(binary);
    command
        .args(["--stdio", "--backend", "node"])
        .env_clear()
        .env("PATH", &path_value)
        .env("HOME", work)
        .env("TMPDIR", work)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(work);
    let child = command
        .spawn()
        .map_err(|error| format!("spawn legacy Node smoke: {error}"))?;
    let output = wait_with_timeout(child, Duration::from_secs(120), "legacy Node smoke")?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        return Err(format!(
            "legacy Node smoke exited {:?}; stdout={} stderr={}",
            output.status,
            text,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !text.contains("\"id\":1") && !text.contains("\"id\": 1") {
        return Err(format!(
            "legacy Node smoke produced no initialize response: {text}"
        ));
    }
    if !text.contains("wxml-lsp") {
        return Err(format!(
            "legacy Node smoke response did not name wxml-lsp: {text}"
        ));
    }
    Ok(text)
}

fn wait_with_timeout(
    mut child: Child,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|error| format!("wait for {label}: {error}"));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait for {label}: {error}"));
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|error| format!("wait for timed-out {label}: {error}"))?;
            return Err(format!(
                "{label} timed out after {timeout:?}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_no_legacy_in_current(root: &Path, label: &str) {
    let contents = collect_tree(&root.join("current")).expect("collect native current tree");
    for path in &contents.files {
        let lower = path.to_ascii_lowercase();
        assert!(
            !lower.contains("wxml-type-backend"),
            "{label}: native current contains `{path}`"
        );
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
        assert!(
            !matches!(extension, Some("js" | "cjs" | "mjs")),
            "{label}: native current contains JavaScript `{path}`"
        );
    }
}

fn install_and_assert_legacy(
    package: &LegacyPackage,
    base: &TestDir,
    name: &str,
    install_bin: &Path,
    tripwires: &Tripwires,
    work: &Path,
) -> (PathBuf, String) {
    let root = private_root(base, name);
    let install = install_legacy_real(package, install_bin, &root, tripwires, work, name);
    assert_success(&install, name);
    assert_contains(&install, &["installed"]);
    assert_no_state(&root, name);
    assert_journal_empty(&open_root(&root));
    assert_tmp_empty(&open_root(&root));
    let handle = open_root(&root);
    assert_eq!(
        active_version(&handle).as_deref(),
        Some(package.version.as_str())
    );
    assert_eq!(
        read_manifest_schema(&root.join("versions").join(&package.version))
            .expect("installed legacy schema"),
        ManifestSchema::V1
    );
    let fingerprint = tree_fingerprint(&root.join("versions").join(&package.version))
        .expect("legacy baseline fingerprint");
    (root, fingerprint)
}

#[test]
#[ignore = "requires a real schema v1 package (WXML_F3_LEGACY_PACKAGE) and pinned native assets"]
#[allow(clippy::too_many_lines)]
fn f3_09_real_legacy_package_migrates_once_and_stays_immutable() {
    let Some(package) = LegacyPackage::resolve() else {
        return;
    };
    let Some(channel) = shared_channel("f3_09_real_legacy") else {
        skip(
            "F3-09",
            "pinned native assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-09-real-legacy");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();

    evidence(
        "F3-09",
        "REAL",
        format!("legacy schema v1 package identity: {}", package.identity()),
    );
    evidence(
        "F3-09",
        "LIMIT",
        "the v1 package is a full-fidelity build by the production v1 packager from the real Rust \
         release binaries, real backends/typescript dist and real Zed wasm; it is not a historical \
         published artifact. The native target is the pinned real-asset channel (N2 0.2.0, built \
         by retargeting the two Rust version constants for the second-version harness).",
    );

    // The production installer must accept the real archive before anything
    // is installed.
    let verify_package = run_installer(
        &install_bin,
        &[
            "verify-package",
            "--from-dir",
            package.output.to_str().expect("UTF-8 legacy output"),
            "--version",
            &package.version,
            "--offline",
        ],
        &tripwires,
        &work,
        "verify real legacy package",
    );
    assert_success(&verify_package, "verify real legacy package");
    evidence(
        "F3-09",
        "PASS",
        format!(
            "production installer verified the real schema v1 archive: {}",
            combined(&verify_package).trim().replace('\n', " | ")
        ),
    );

    let (root, legacy_fingerprint) = install_and_assert_legacy(
        &package,
        &base,
        "legacy-root",
        &install_bin,
        &tripwires,
        &work,
    );
    package.assert_tree_unchanged(
        &root,
        &legacy_fingerprint,
        &package.version,
        "after real v1 install",
    );
    let legacy_files = collect_tree(&root.join("versions").join(&package.version))
        .expect("collect legacy tree")
        .files
        .len();
    evidence(
        "F3-09",
        "PASS",
        format!(
            "production schema v1 installer activated the real legacy package: current={}; \
             files={legacy_files}; treeFingerprint={legacy_fingerprint}; no state.json",
            package.version
        ),
    );

    // The legacy entry point is runnable before migration: `--version` and a
    // real stdio initialize through the packaged Node backend.
    let mut version_command =
        release::clean_command(&root.join("current/bin/wxml-lsp"), &tripwires, &work);
    version_command.arg("--version");
    let version = support::run_with_deadline(
        &mut version_command,
        Duration::from_secs(30),
        "real legacy --version",
    );
    assert_success(&version, "real legacy --version");
    assert!(
        stdout(&version).contains(&format!("wxml-lsp {}", package.version)),
        "{}",
        stdout(&version)
    );
    match real_node() {
        Some(node) => {
            let response = run_legacy_node_smoke(&root.join("current/bin/wxml-lsp"), &node, &work)
                .expect("real legacy Node backend smoke");
            evidence(
                "F3-09",
                "PASS",
                format!(
                    "legacy current/bin/wxml-lsp --version printed `wxml-lsp {}`; \
                     --stdio --backend node completed an LSP initialize through the packaged \
                     wxml-type-backend/server.js with node {}; response bytes={}",
                    package.version,
                    node.display(),
                    response.len()
                ),
            );
        }
        None => {
            skip(
                "F3-09",
                "legacy --version ran, but no real Node runtime was found for the optional \
                 --backend node smoke (set WXML_F3_REAL_NODE)",
            );
        }
    }
    tripwires.assert_not_executed();
    assert_no_legacy_process(&root);

    // Success path: migration to native N2, exactly once, previous=null.
    let migrate = run_installer(
        &install_bin,
        &[
            "install",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--from-dir",
            channel.channel.to_str().expect("UTF-8 channel"),
            "--version",
            VERSION_N2,
            "--offline",
        ],
        &tripwires,
        &work,
        "real legacy to native migration",
    );
    assert_success(&migrate, "real legacy to native migration");
    let migrated_text = format!("migrated to {VERSION_N2} (native)");
    assert_contains(&migrate, &[migrated_text.as_str()]);
    assert_state(&root, VERSION_N2, None);
    assert_eq!(
        active_version(&open_root(&root)).as_deref(),
        Some(VERSION_N2)
    );
    assert_journal_empty(&open_root(&root));
    assert_tmp_empty(&open_root(&root));
    package.assert_tree_unchanged(&root, &legacy_fingerprint, VERSION_N2, "after migration");
    let state = fs::read(root.join("state.json")).expect("native state bytes");
    let state_text = String::from_utf8_lossy(&state);
    assert!(
        !state_text.contains(&package.version),
        "legacy version leaked into native state: {state_text}"
    );
    assert_no_legacy_in_current(&root, "after migration");
    tripwires.assert_not_executed();
    assert_no_legacy_process(&root);
    let current_link = fs::read_link(root.join("current")).expect("native current link");
    assert_eq!(current_link, PathBuf::from("versions").join(VERSION_N2));
    assert!(
        !root.join("current/bin/wxml-type-backend").exists(),
        "native current still exposes the legacy Node backend"
    );
    evidence(
        "F3-09",
        "PASS",
        format!(
            "real legacy {} migrated once to native {VERSION_N2}; current={VERSION_N2}; \
             state=active:{VERSION_N2}/previous:null; no journal/tmp residue; legacy tree \
             fingerprint={legacy_fingerprint} and binary/backend hashes unchanged; no Node shim \
             executed; no Node/server.js process observed",
            package.version
        ),
    );

    // Recover must not resurrect the legacy package or rewrite the native
    // state.
    let recover = run_installer(
        &install_bin,
        &["recover", "--root", root.to_str().expect("UTF-8 root")],
        &tripwires,
        &work,
        "recover after real legacy migration",
    );
    assert_success(&recover, "recover after real legacy migration");
    assert_contains(
        &recover,
        &[
            "no recovery actions were needed",
            "valid state already active",
        ],
    );
    assert_eq!(
        active_version(&open_root(&root)).as_deref(),
        Some(VERSION_N2)
    );
    assert_eq!(
        fs::read(root.join("state.json")).expect("state bytes after recover"),
        state,
        "recover rewrote the native state"
    );
    package.assert_tree_unchanged(&root, &legacy_fingerprint, VERSION_N2, "after recover");
    tripwires.assert_not_executed();
    assert_no_legacy_process(&root);
    evidence(
        "F3-09",
        "PASS",
        "recover kept the native state byte-identical and never restored legacy; no Node shim was executed",
    );

    // A legacy schema v1 directory is never a valid native rollback target.
    let rollback = run_installer(
        &install_bin,
        &[
            "rollback",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--to",
            &package.version,
            "--offline",
        ],
        &tripwires,
        &work,
        "rollback to real legacy package",
    );
    assert_failure(&rollback, "rollback to real legacy package");
    assert_contains(&rollback, &["legacy node package"]);
    assert_eq!(
        active_version(&open_root(&root)).as_deref(),
        Some(VERSION_N2)
    );
    assert_eq!(
        fs::read(root.join("state.json")).expect("state bytes after rollback refusal"),
        state
    );
    package.assert_tree_unchanged(
        &root,
        &legacy_fingerprint,
        VERSION_N2,
        "after rollback refusal",
    );
    tripwires.assert_not_executed();
    assert_no_legacy_process(&root);
    evidence(
        "F3-09",
        "PASS",
        format!(
            "native rollback --to {} rejected the real legacy package without touching current, \
             state or the legacy tree",
            package.version
        ),
    );

    // The install is idempotent after migration.
    let repeat = run_installer(
        &install_bin,
        &[
            "install",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--from-dir",
            channel.channel.to_str().expect("UTF-8 channel"),
            "--version",
            VERSION_N2,
            "--offline",
        ],
        &tripwires,
        &work,
        "repeat install after migration",
    );
    assert_success(&repeat, "repeat install after migration");
    assert_contains(&repeat, &["already installed"]);
    assert_eq!(
        fs::read(root.join("state.json")).expect("state bytes after repeat install"),
        state
    );
    package.assert_tree_unchanged(
        &root,
        &legacy_fingerprint,
        VERSION_N2,
        "after repeat install",
    );
    evidence(
        "F3-09",
        "PASS",
        "repeating install after migration reported already installed and left state and the \
         legacy tree unchanged",
    );
}

#[test]
#[ignore = "requires a real schema v1 package (WXML_F3_LEGACY_PACKAGE) and pinned native assets"]
#[allow(clippy::too_many_lines)]
fn f3_09_real_legacy_package_pre_switch_failure_and_post_switch_recovery() {
    let Some(package) = LegacyPackage::resolve() else {
        return;
    };
    let Some(channel) = shared_channel("f3_09_real_legacy_failures") else {
        skip(
            "F3-09",
            "pinned native assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-09-real-legacy-failures");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();

    // Pre-switch failure: a real-package-derived N2 variant missing one
    // typings file fails before the switch, leaving the real legacy current,
    // tree and state untouched.
    let (failure_root, failure_fingerprint) = install_and_assert_legacy(
        &package,
        &base,
        "legacy-failure-root",
        &install_bin,
        &tripwires,
        &work,
    );
    let missing_typings = mutate_release(
        base.path(),
        &channel.n2,
        "n2-missing-typings-for-real-legacy",
        remove_typings,
    );
    let failed = run_installer(
        &install_bin,
        &[
            "install",
            "--root",
            failure_root.to_str().expect("UTF-8 root"),
            "--from-dir",
            missing_typings.release_dir.to_str().expect("UTF-8 variant"),
            "--version",
            VERSION_N2,
            "--offline",
        ],
        &tripwires,
        &work,
        "pre-switch failure from real legacy",
    );
    assert_failure(&failed, "pre-switch failure from real legacy");
    let failure_handle = open_root(&failure_root);
    assert_eq!(
        active_version(&failure_handle).as_deref(),
        Some(package.version.as_str())
    );
    assert_no_state(&failure_root, "after pre-switch failure");
    assert_eq!(
        support::installed_versions(&failure_handle),
        vec![package.version.clone()]
    );
    assert_journal_empty(&failure_handle);
    assert_tmp_empty(&failure_handle);
    package.assert_tree_unchanged(
        &failure_root,
        &failure_fingerprint,
        &package.version,
        "after pre-switch failure",
    );
    tripwires.assert_not_executed();
    assert_no_legacy_process(&failure_root);
    evidence(
        "F3-09",
        "PASS",
        format!(
            "a real-package-derived native {VERSION_N2} archive missing one typings file was \
             rejected before activation; real legacy current/tree/state unchanged; no native \
             state or journal; Node tripwire not triggered; no legacy process observed"
        ),
    );

    // Post-switch interruption at the production `AfterSwitch` checkpoint:
    // the verified native target rolls forward and legacy is never previous.
    let (switched_root, switched_fingerprint) = install_and_assert_legacy(
        &package,
        &base,
        "legacy-switched-root",
        &install_bin,
        &tripwires,
        &work,
    );
    let switched_handle = open_root(&switched_root);
    let staged = stage_release_tree(&switched_handle, &channel.n2);
    let error = install_with_failpoint(
        &switched_handle,
        &staged,
        VERSION_N2,
        FailPoint::AfterSwitch,
    )
    .expect_err("AfterSwitch must interrupt the migration");
    let DeliveryError::SwitchedButNotDurable {
        version,
        previous,
        state_reconciled,
        ..
    } = error
    else {
        panic!("expected typed post-switch state, got {error:?}");
    };
    assert_eq!(version, VERSION_N2);
    assert_eq!(previous, None, "legacy must never become previous");
    assert!(
        state_reconciled,
        "the verified native target must roll forward"
    );
    assert_eq!(
        active_version(&open_root(&switched_root)).as_deref(),
        Some(VERSION_N2)
    );
    assert_state(&switched_root, VERSION_N2, None);
    let journals = Journal::list(&open_root(&switched_root)).expect("list journals");
    assert!(
        journals.is_empty(),
        "post-switch recovery left journals: {journals:?}"
    );
    package.assert_tree_unchanged(
        &switched_root,
        &switched_fingerprint,
        VERSION_N2,
        "after post-switch interruption",
    );
    let recovered_state = fs::read(switched_root.join("state.json")).expect("native state");
    assert!(
        !String::from_utf8_lossy(&recovered_state).contains(&package.version),
        "legacy version was recorded in native state"
    );
    assert_no_legacy_in_current(&switched_root, "after post-switch interruption");
    tripwires.assert_not_executed();
    assert_no_legacy_process(&switched_root);
    evidence(
        "F3-09",
        "PASS",
        format!(
            "AfterSwitch interruption rolled the verified native {VERSION_N2} target forward: \
             state=active:{VERSION_N2}/previous:null, no journal, legacy tree unchanged, legacy \
             never recorded as previous, no Node/server.js process observed"
        ),
    );

    let recover = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            switched_root.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover after post-switch interruption",
    );
    assert_success(&recover, "recover after post-switch interruption");
    assert_contains(
        &recover,
        &[
            "no recovery actions were needed",
            "valid state already active",
        ],
    );
    assert_eq!(
        fs::read(switched_root.join("state.json")).expect("state bytes after recover"),
        recovered_state
    );
    assert_eq!(
        active_version(&open_root(&switched_root)).as_deref(),
        Some(VERSION_N2)
    );
    package.assert_tree_unchanged(
        &switched_root,
        &switched_fingerprint,
        VERSION_N2,
        "after post-switch recover",
    );
    evidence(
        "F3-09",
        "PASS",
        "recover after post-switch interruption kept native state byte-identical and did not \
         restore the legacy package; no Node shim was executed",
    );

    // Recovery on a still-legacy root must not invent native state.
    let recover_legacy = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            failure_root.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover on real legacy root",
    );
    assert_success(&recover_legacy, "recover on real legacy root");
    assert_eq!(
        active_version(&open_root(&failure_root)).as_deref(),
        Some(package.version.as_str())
    );
    assert_no_state(&failure_root, "recover on real legacy root");
    package.assert_tree_unchanged(
        &failure_root,
        &failure_fingerprint,
        &package.version,
        "after recover on legacy root",
    );
    tripwires.assert_not_executed();
    assert_no_legacy_process(&failure_root);
    evidence(
        "F3-09",
        "PASS",
        "recover on a real legacy root left it read-only with no state; the Node tripwire was not \
         triggered and no legacy process was observed",
    );
}
