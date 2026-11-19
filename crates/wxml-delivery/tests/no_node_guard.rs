//! F3-02 no-Node guard for the installer CLI.
//!
//! This test exercises the real `wxml-lsp-install` binary against a schema v2
//! fixture release while `PATH` contains only Node tripwire shims. It covers
//! `install`, `verify`, `current` and `env`.
//!
//! Scope limits:
//!
//! - A PATH shim cannot rule out an absolute-path Node invocation. It is
//!   auxiliary evidence, not the `strace -f -e trace=execve,execveat` audit
//!   required by F3-02. This test therefore does not close F3-02.
//! - A native stdio LSP session is not asserted: the test derives the
//!   `NodeBackend::new` line from `crates/wxml-lsp/src/main.rs` and prints
//!   `LSP_NATIVE: BLOCKED` while it is present. Without it, the status is
//!   `UNVERIFIED`, never PASS.
//! - Two-version real-engine `upgrade`/`rollback` is not exercised here; the
//!   CLI flow test uses a shell fixture engine and the real-asset harness covers
//!   only one install version.
//! - `WXML_REQUIRE_REAL_ENGINE=1|true` makes a missing `TSGO_ASSET`/`TSGO_BIN`
//!   (or the `TSGO_NATIVE_TARBALL` alias) a hard failure; otherwise the
//!   real-engine identity check skips loudly. Full real-asset installation
//!   remains in `real_asset.rs`.
//! - `WXML_KEEP_TEST_ROOT=1` intentionally leaks the fixture root for manual
//!   script runs. It must not be set in CI; the test rejects `CI` combined with
//!   it. Without `--nocapture`, successful-test skip/status lines are hidden by
//!   libtest.
//!
//! Run explicitly:
//!
//! ```text
//! cargo test -p wxml-delivery --test no_node_guard -- --nocapture
//! ```

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::too_many_lines
)]

#[path = "support/typings_payload.rs"]
mod typings_payload;

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use wxml_delivery::archive::{create_tar_gz, extract_tar_gz};
use wxml_delivery::index_v2::{IndexV2, ReleaseV2};
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY,
    ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE,
    ENGINE_DISTRIBUTION_URL, ENGINE_LICENSE, ENGINE_PROBE_PREFIX, ENGINE_SOURCE_COMMIT,
    ENGINE_VERSION, EngineInfo, EngineProbe, MANIFEST_SCHEMA_V2, MINIPROGRAM_TYPINGS_ASSET,
    MINIPROGRAM_TYPINGS_FILE_COUNT, MINIPROGRAM_TYPINGS_LICENSE, MINIPROGRAM_TYPINGS_ROOT,
    MINIPROGRAM_TYPINGS_VERSION, ManifestV2, NATIVE_PACKAGE_PLATFORM, NativeArtifactKind,
    NativeBinaryArtifact, STANDARD_LIB_ASSET, STANDARD_LIB_FILE_COUNT, STANDARD_LIB_INCLUDE,
    TypeAsset, compute_type_asset_bundle,
};
use wxml_delivery::sri::{sha512_sri_bytes, verify_asset_file};
use wxml_delivery::verify_v2::{checksums_for_tree_v2, verify_tree_v2};

const FIXTURE_VERSION: &str = "0.1.0";
/// SHA-256 of the pinned 7.1.0-beta `lib/tsc`, audited with the tarball.
const PINNED_ENGINE_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";
/// Size of the pinned 7.1.0-beta `lib/tsc`.
const PINNED_ENGINE_SIZE: u64 = 24_522_912;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const ENGINE_TIMEOUT: Duration = Duration::from_secs(10);
const READER_GRACE: Duration = Duration::from_secs(1);

static ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate below the repository root")
        .to_path_buf()
}

fn assert_safe_temp_path(path: &Path) {
    let text = path.to_string_lossy();
    assert!(!text.is_empty(), "temporary path must not be empty");
    assert!(
        !text.contains('\''),
        "temporary path must not contain a single quote: {text}"
    );
    assert!(
        !text.chars().any(char::is_control),
        "temporary path must not contain control characters: {text:?}"
    );
}

/// A private `0700` test root below `/tmp/opencode` or `WXML_TEST_TMPDIR`.
///
/// The base is canonicalized and checked against the repository before a child
/// directory is created atomically with mode `0700`.
struct PrivateRoot {
    path: PathBuf,
}

impl PrivateRoot {
    fn new(label: &str) -> Self {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        assert_safe_temp_path(&base);
        let repository = repository_root();
        assert!(
            !base.starts_with(&repository),
            "WXML_TEST_TMPDIR must be outside the repository ({})",
            repository.display()
        );
        fs::create_dir_all(&base).expect("create test base");
        let canonical_base = fs::canonicalize(&base).expect("canonicalize test base");
        assert!(
            canonical_base.starts_with("/tmp"),
            "test base must resolve under /tmp, got {}",
            canonical_base.display()
        );
        let canonical_repo = fs::canonicalize(repository_root()).expect("canonicalize repository");
        assert!(
            !canonical_base.starts_with(&canonical_repo),
            "test base must be outside the repository ({})",
            canonical_repo.display()
        );
        let nonce = ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = canonical_base.join(format!("wxml-f3-{label}-{}-{nonce}", std::process::id()));
        assert_safe_temp_path(&path);
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&path).expect("create private test root");
        let canonical = fs::canonicalize(&path).expect("canonicalize private test root");
        assert!(
            canonical.starts_with("/tmp"),
            "private test root must resolve under /tmp, got {}",
            canonical.display()
        );
        assert!(
            !canonical.starts_with(&canonical_repo),
            "private test root resolves inside the repository: {}",
            canonical.display()
        );
        Self { path: canonical }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    assert!(
        !text.contains('\''),
        "test paths must not contain single quotes: {text}"
    );
    format!("'{text}'")
}

/// Node/PATH tripwires. Any lookup or direct execution logs and exits 99.
///
/// The marker and log live in a dedicated `tripwire/` directory, separate from
/// `HOME` and `TMPDIR`, so ordinary subject cleanup cannot erase them.
struct NodeShims {
    dir: PathBuf,
    marker: PathBuf,
    log: PathBuf,
}

impl NodeShims {
    fn install(base: &Path) -> Self {
        let dir = base.join("tripwire");
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&dir).expect("create tripwire dir");
        let marker = dir.join("NODE_WAS_EXECUTED");
        let log = dir.join("node-exec-attempts.log");
        assert_safe_temp_path(&marker);
        assert_safe_temp_path(&log);
        for name in ["node", "nodejs", "npm", "npx", "pnpm", "yarn", "bun"] {
            let shim = dir.join(name);
            let script = format!(
                "#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> {}\n: > {}\nexit 99\n",
                shell_quote(&log),
                shell_quote(&marker)
            );
            fs::write(&shim, script).expect("write shim");
            set_mode(&shim, 0o755).expect("make shim executable");
        }
        Self { dir, marker, log }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn clear(&self) -> io::Result<()> {
        for path in [&self.marker, &self.log] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn assert_absent(&self) {
        assert!(
            !self.marker.exists(),
            "a Node shim was executed (marker exists at {})",
            self.marker.display()
        );
        assert!(
            !self.log.exists(),
            "a Node shim was executed (log exists at {})",
            self.log.display()
        );
    }

    fn assert_positive_control(&self) {
        assert!(
            self.marker.is_file(),
            "tripwire positive control did not create {}",
            self.marker.display()
        );
        let attempts = fs::read_to_string(&self.log).expect("read tripwire attempt log");
        assert!(
            attempts.contains("node --version"),
            "tripwire log did not record the control: {attempts}"
        );
    }
}

/// Bounded child execution with a process group killed on timeout or after a
/// reader grace period. Every child is started as its own process-group leader.
struct RunResult {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl RunResult {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

fn spawn_reader(mut reader: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        output
    })
}

fn kill_group(pid: u32) -> io::Result<()> {
    let group = format!("-{pid}");
    let mut last_error = None;
    for candidate in ["/bin/kill", "/usr/bin/kill"] {
        if !Path::new(candidate).is_file() {
            continue;
        }
        let status = Command::new(candidate)
            .args(["-KILL", "--", &group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            return Ok(());
        }
        last_error = Some(format!("{candidate} exited with {status}"));
    }
    let status = Command::new("kill")
        .args(["-KILL", "--", &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            last_error.unwrap_or_else(|| format!("kill exited with {status}")),
        ))
    }
}

fn wait_for_readers(pid: u32, readers: &[&JoinHandle<Vec<u8>>]) {
    let deadline = Instant::now() + READER_GRACE;
    while !readers.iter().all(|reader| reader.is_finished()) {
        if Instant::now() >= deadline {
            kill_group(pid).unwrap_or_else(|error| {
                panic!("could not kill process group {pid} while draining output: {error}")
            });
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn join_reader(reader: Option<JoinHandle<Vec<u8>>>, label: &str) -> Vec<u8> {
    reader
        .map(|reader| {
            reader
                .join()
                .unwrap_or_else(|_| panic!("{label}: output reader panicked"))
        })
        .unwrap_or_default()
}

fn run_bounded(command: &mut Command, timeout: Duration, label: &str) -> RunResult {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    command.process_group(0);
    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {label}: {error}"));
    let pid = child.id();
    let stdout_reader = child.stdout.take().map(spawn_reader);
    let stderr_reader = child.stderr.take().map(spawn_reader);
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let readers: Vec<&JoinHandle<Vec<u8>>> =
                    [stdout_reader.as_ref(), stderr_reader.as_ref()]
                        .into_iter()
                        .flatten()
                        .collect();
                wait_for_readers(pid, &readers);
                let status = child
                    .wait()
                    .unwrap_or_else(|error| panic!("wait for {label}: {error}"));
                let stdout = join_reader(stdout_reader, label);
                let stderr = join_reader(stderr_reader, label);
                return RunResult {
                    status,
                    stdout,
                    stderr,
                };
            }
            Ok(None) => {}
            Err(error) => {
                let kill_error = kill_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                let stderr = join_reader(stderr_reader, label);
                panic!(
                    "wait for {label}: {error}; group-kill={kill_error:?}; stderr={}",
                    String::from_utf8_lossy(&stderr)
                );
            }
        }
        if Instant::now() >= deadline {
            kill_group(pid)
                .unwrap_or_else(|error| panic!("{label} timed out; group-kill failed: {error}"));
            let child_kill = child.kill();
            let wait_error = child.wait();
            let stderr = join_reader(stderr_reader, label);
            panic!(
                "{label} exceeded {timeout:?}; group-kill=ok; \
                 child-kill={child_kill:?}; wait={wait_error:?}; stderr={}",
                String::from_utf8_lossy(&stderr)
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn assert_success(result: &RunResult, label: &str) -> String {
    assert!(
        result.status.success(),
        "{label} failed: status={:?}\nstdout={}\nstderr={}",
        result.status,
        result.stdout(),
        result.stderr()
    );
    result.stdout()
}

fn installer_command(shims: &NodeShims, base: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wxml-lsp-install"));
    command
        .env_clear()
        .env("PATH", shims.path())
        .env("HOME", base.join("home"))
        .env("XDG_DATA_HOME", base.join("home"))
        .env("TMPDIR", base.join("tmp"))
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("NO_COLOR", "1")
        .env("WXML_NODE", shims.path().join("node"))
        .env("NODE", shims.path().join("node"))
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9");
    command
}

/// Positive control shared by the dedicated test and the installer test. It
/// exercises PATH resolution, asserts exit 99 and marker/log creation, then
/// clears both so the installer checks start from a clean tripwire.
fn assert_tripwire_positive_control(shims: &NodeShims) {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "node --version"])
        .env_clear()
        .env("PATH", shims.path());
    let result = run_bounded(&mut command, CONTROL_TIMEOUT, "node tripwire control");
    assert_eq!(
        result.status.code(),
        Some(99),
        "tripwire node did not exit 99: stdout={} stderr={}",
        result.stdout(),
        result.stderr()
    );
    shims.assert_positive_control();
    shims
        .clear()
        .expect("clear tripwire after positive control");
    shims.assert_absent();
    println!("NO_NODE: positive control exercised (shim exit 99, marker created)");
}

fn environment_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn assert_engine_identity(engine: &Path) {
    let metadata = fs::metadata(engine)
        .unwrap_or_else(|error| panic!("real engine metadata {}: {error}", engine.display()));
    assert_eq!(
        metadata.len(),
        PINNED_ENGINE_SIZE,
        "real engine size changed for {}",
        engine.display()
    );
    assert_eq!(
        sha256_file(engine).expect("hash real engine"),
        PINNED_ENGINE_SHA256,
        "real engine SHA-256 changed for {}",
        engine.display()
    );
}

struct LspStatus {
    status: &'static str,
    detail: String,
}

fn lsp_native_status() -> LspStatus {
    let path = repository_root().join("crates/wxml-lsp/src/main.rs");
    let line = fs::read_to_string(&path).ok().and_then(|text| {
        text.lines()
            .position(|line| line.contains("NodeBackend::new"))
            .map(|index| index + 1)
    });
    line.map_or_else(
        || LspStatus {
            status: "UNVERIFIED",
            detail: format!("{} has no NodeBackend::new", path.display()),
        },
        |line| LspStatus {
            status: "BLOCKED",
            detail: format!("{}:{line} still wires NodeBackend", path.display()),
        },
    )
}

struct FileBuilder {
    root: PathBuf,
    files: Vec<FileEntry>,
}

impl FileBuilder {
    fn new(root: &Path) -> Self {
        fs::create_dir_all(root).expect("create fixture root");
        Self {
            root: root.to_path_buf(),
            files: Vec::new(),
        }
    }

    fn file(&mut self, path: &str, bytes: &[u8], mode: u32) {
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().expect("file parent")).expect("create parent");
        fs::write(&destination, bytes).expect("write fixture file");
        set_mode(&destination, mode).expect("set fixture mode");
        self.files.push(FileEntry {
            path: path.to_owned(),
            sha256: sha256_bytes(bytes),
            size: u64::try_from(bytes.len()).expect("fixture file size"),
            mode: format!("{mode:04o}"),
        });
    }

    fn hash(&self, path: &str) -> String {
        self.files
            .iter()
            .find(|file| file.path == path)
            .expect("fixture file")
            .sha256
            .clone()
    }
}

/// Builds a schema v2 fixture tree. The engine is a shell script that prints the
/// frozen version line; it never invokes Node.
fn build_native_tree(root: &Path, version: &str) -> ManifestV2 {
    let mut builder = FileBuilder::new(root);
    builder.file(
        "bin/wxml-lsp",
        format!("#!/bin/sh\nprintf 'wxml-lsp {version}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file(
        "bin/wxml-lsp-install",
        format!("#!/bin/sh\nprintf 'wxml-lsp-install {version}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file(
        "engine/lib/tsc",
        format!("#!/bin/sh\nprintf '{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\\n'\n").as_bytes(),
        0o755,
    );
    builder.file("engine/lib/tsc.sig", b"signature-placeholder\n", 0o644);
    builder.file("engine/LICENSE", b"Apache License fixture\n", 0o644);
    builder.file("engine/NOTICE.txt", b"NOTICE fixture\n", 0o644);
    builder.file("engine/package.json", b"{\"name\":\"fixture\"}\n", 0o644);
    builder.file("engine/lib/lib.d.ts", b"// lib.d.ts\n", 0o644);
    builder.file("engine/lib/lib.es5.d.ts", b"// lib.es5.d.ts\n", 0o644);
    builder.file("engine/lib/lib.es2020.d.ts", b"// lib.es2020.d.ts\n", 0o644);
    for index in 0..113 {
        builder.file(
            &format!("engine/lib/lib.f{index:03}.d.ts"),
            format!("// filler {index}\n").as_bytes(),
            0o644,
        );
    }
    typings_payload::write_payload(|path, bytes| builder.file(path, bytes, 0o644));
    builder.file("zed-extension/extension.wasm", b"\0asm-fixture", 0o644);
    builder.file(
        "zed-extension/extension.toml",
        b"id = \"wxml-lsp\"\n",
        0o644,
    );
    builder.file("provenance.json", b"{\"fixture\":true}\n", 0o644);
    builder.file("THIRD_PARTY_LICENSES.md", b"# licenses\n", 0o644);

    let standard = compute_type_asset_bundle(
        &builder.files,
        "engine/lib",
        &[STANDARD_LIB_INCLUDE.to_owned()],
    )
    .expect("standard lib bundle");
    assert_eq!(standard.file_count, STANDARD_LIB_FILE_COUNT);
    let typings =
        compute_type_asset_bundle(&builder.files, MINIPROGRAM_TYPINGS_ROOT, &["**".to_owned()])
            .expect("typings bundle");
    assert_eq!(typings.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);

    let manifest = ManifestV2 {
        schema_version: MANIFEST_SCHEMA_V2,
        package: "wxml-lsp".to_owned(),
        package_version: version.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: 2,
        built_at: "2026-11-17T00:00:00Z".to_owned(),
        engine: EngineInfo {
            name: "typescript".to_owned(),
            version: ENGINE_VERSION.to_owned(),
            source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
            distribution: ENGINE_DISTRIBUTION.to_owned(),
            distribution_url: ENGINE_DISTRIBUTION_URL.to_owned(),
            distribution_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
            distribution_sha256: ENGINE_DISTRIBUTION_SHA256.to_owned(),
            distribution_size: ENGINE_DISTRIBUTION_SIZE,
            distribution_file_count: ENGINE_DISTRIBUTION_FILE_COUNT,
            distribution_unpacked_size: ENGINE_DISTRIBUTION_UNPACKED_SIZE,
            api_schema_version: 9,
            ast_header_bytes: 64,
            license: ENGINE_LICENSE.to_owned(),
            license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
            binary: "engine/lib/tsc".to_owned(),
            probe: EngineProbe {
                args: vec!["--version".to_owned()],
                expect_prefix: ENGINE_PROBE_PREFIX.to_owned(),
            },
        },
        type_assets: vec![
            TypeAsset {
                name: STANDARD_LIB_ASSET.to_owned(),
                version: ENGINE_VERSION.to_owned(),
                license: ENGINE_LICENSE.to_owned(),
                root: "engine/lib".to_owned(),
                include: vec![STANDARD_LIB_INCLUDE.to_owned()],
                file_count: standard.file_count,
                sha256: standard.sha256,
                license_files: vec!["engine/LICENSE".to_owned(), "engine/NOTICE.txt".to_owned()],
            },
            TypeAsset {
                name: MINIPROGRAM_TYPINGS_ASSET.to_owned(),
                version: MINIPROGRAM_TYPINGS_VERSION.to_owned(),
                license: MINIPROGRAM_TYPINGS_LICENSE.to_owned(),
                root: MINIPROGRAM_TYPINGS_ROOT.to_owned(),
                include: vec!["**".to_owned()],
                file_count: typings.file_count,
                sha256: typings.sha256,
                license_files: vec![format!("{MINIPROGRAM_TYPINGS_ROOT}/LICENSE")],
            },
        ],
        binaries: vec![
            NativeBinaryArtifact {
                kind: NativeArtifactKind::Rust,
                name: "wxml-lsp".to_owned(),
                path: "bin/wxml-lsp".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: builder.hash("bin/wxml-lsp"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::Rust,
                name: "wxml-lsp-install".to_owned(),
                path: "bin/wxml-lsp-install".to_owned(),
                version: version.to_owned(),
                protocol_version: None,
                sha256: builder.hash("bin/wxml-lsp-install"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::NativeEngine,
                name: "tsgo".to_owned(),
                path: "engine/lib/tsc".to_owned(),
                version: ENGINE_VERSION.to_owned(),
                protocol_version: Some(2),
                sha256: builder.hash("engine/lib/tsc"),
            },
            NativeBinaryArtifact {
                kind: NativeArtifactKind::ZedExtension,
                name: "zed-extension".to_owned(),
                path: "zed-extension/extension.wasm".to_owned(),
                version: "0.0.1".to_owned(),
                protocol_version: None,
                sha256: builder.hash("zed-extension/extension.wasm"),
            },
        ],
        files: builder.files,
    };
    manifest
        .validate(wxml_delivery::manifest_v2::ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            protocol_version: Some(2),
        })
        .expect("fixture manifest validates");
    atomic_write(
        &root.join("manifest.json"),
        &manifest.to_json().expect("serialize manifest"),
        0o644,
    )
    .expect("write manifest");
    let checksums = checksums_for_tree_v2(root, &manifest).expect("fixture checksums");
    atomic_write(&root.join("SHA256SUMS"), &checksums, 0o644).expect("write checksums");
    verify_tree_v2(
        root,
        wxml_delivery::manifest_v2::ExpectedManifestV2 {
            version: Some(version),
            platform: Some(NATIVE_PACKAGE_PLATFORM),
            protocol_version: Some(2),
        },
    )
    .expect("fixture tree verifies");
    manifest
}

fn pack_native_release(base: &Path) -> PathBuf {
    let release = base.join("native-release");
    fs::create_dir_all(&release).expect("create release dir");
    let tree = base.join("tree");
    let manifest = build_native_tree(&tree, FIXTURE_VERSION);
    let archive_name = format!("wxml-lsp-{FIXTURE_VERSION}-{NATIVE_PACKAGE_PLATFORM}.tar.gz");
    let archive = release.join(&archive_name);
    let mut entries = manifest.files;
    for name in ["manifest.json", "SHA256SUMS"] {
        let path = tree.join(name);
        entries.push(FileEntry {
            path: name.to_owned(),
            sha256: sha256_file(&path).expect("hash fixture metadata"),
            size: fs::metadata(&path).expect("fixture metadata size").len(),
            mode: "0644".to_owned(),
        });
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    create_tar_gz(&tree, &archive, &entries, 0).expect("create release archive");
    let bytes = fs::read(&archive).expect("read release archive");
    let mut index = IndexV2::empty();
    index.upsert(ReleaseV2 {
        version: FIXTURE_VERSION.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: 2,
        engine_version: ENGINE_VERSION.to_owned(),
        engine_source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        engine_protocol_version: 2,
        engine_api_schema_version: 9,
        engine_ast_header_bytes: 64,
        engine_distribution: ENGINE_DISTRIBUTION.to_owned(),
        engine_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        min_installer_schema: 2,
        archive: archive_name,
        archive_integrity: sha512_sri_bytes(&bytes).to_sri(),
        sha256: sha256_bytes(&bytes),
        size: u64::try_from(bytes.len()).expect("archive size"),
    });
    atomic_write(
        &release.join("index.json"),
        &index.to_json().expect("serialize index"),
        0o644,
    )
    .expect("write index");
    release
}

#[test]
fn node_tripwire_positive_control_creates_marker_and_exits_99() {
    let base = PrivateRoot::new("f3-tripwire-control");
    let shims = NodeShims::install(base.path());
    assert_tripwire_positive_control(&shims);
    shims.assert_absent();
}

#[test]
fn installer_cli_never_executes_path_node() {
    let base = PrivateRoot::new("f3-no-node");
    let shims = NodeShims::install(base.path());
    // Run the positive control inside this test too: filtering to this test must
    // not make the no-execution assertion vacuous.
    assert_tripwire_positive_control(&shims);
    fs::create_dir_all(base.path().join("home")).expect("create HOME");
    fs::create_dir_all(base.path().join("tmp")).expect("create TMPDIR");

    let release = pack_native_release(base.path());
    let root = base.path().join("root");
    fs::create_dir(&root).expect("create install root");
    set_mode(&root, 0o700).expect("make install root private");

    let mut install = installer_command(&shims, base.path());
    install.args([
        "install",
        "--root",
        root.to_str().expect("UTF-8 root"),
        "--from-dir",
        release.to_str().expect("UTF-8 release"),
        "--version",
        FIXTURE_VERSION,
        "--offline",
    ]);
    let output = assert_success(
        &run_bounded(&mut install, OPERATION_TIMEOUT, "install"),
        "install",
    );
    assert!(
        output.contains(&format!("installed {FIXTURE_VERSION} (native)")),
        "{output}"
    );
    assert!(
        output.contains(&format!("engine: typescript {ENGINE_VERSION}")),
        "{output}"
    );
    shims.assert_absent();

    let mut verify = installer_command(&shims, base.path());
    verify.args(["verify", "--root", root.to_str().expect("UTF-8 root")]);
    let output = assert_success(
        &run_bounded(&mut verify, OPERATION_TIMEOUT, "verify"),
        "verify",
    );
    assert!(
        output.contains(&format!("verified {FIXTURE_VERSION} (native)")),
        "{output}"
    );
    assert!(
        output.contains(&format!("engine version: {ENGINE_VERSION}")),
        "{output}"
    );
    shims.assert_absent();

    let mut current = installer_command(&shims, base.path());
    current.args(["current", "--root", root.to_str().expect("UTF-8 root")]);
    let output = assert_success(
        &run_bounded(&mut current, OPERATION_TIMEOUT, "current"),
        "current",
    );
    assert!(
        output.contains(&format!("version: {FIXTURE_VERSION}")),
        "{output}"
    );
    assert!(
        output.contains(&format!("engine: typescript {ENGINE_VERSION}")),
        "{output}"
    );
    shims.assert_absent();

    let mut env = installer_command(&shims, base.path());
    env.args(["env", "--root", root.to_str().expect("UTF-8 root")]);
    let output = assert_success(&run_bounded(&mut env, OPERATION_TIMEOUT, "env"), "env");
    assert!(output.contains("export WXML_LSP_BIN="), "{output}");
    assert!(
        !output.contains("WXML_TYPE_BACKEND"),
        "native env must not export a Node backend: {output}"
    );
    shims.assert_absent();

    println!("NO_NODE: PASS (installer scope: install/verify/current/env)");
    let lsp = lsp_native_status();
    println!("LSP_NATIVE: {} ({})", lsp.status, lsp.detail);
    if environment_flag("WXML_REQUIRE_LSP_NATIVE") {
        assert_eq!(
            lsp.status, "PASS",
            "WXML_REQUIRE_LSP_NATIVE=1 but LSP native is {}",
            lsp.status
        );
    }
    println!(
        "F3-02: BLOCKED (no execve audit, no native stdio LSP session, \
         no two-version real-engine upgrade/rollback)"
    );

    if std::env::var_os("WXML_KEEP_TEST_ROOT").is_some() {
        assert!(
            std::env::var_os("CI").is_none(),
            "WXML_KEEP_TEST_ROOT leaks the fixture root and must not be set in CI"
        );
        eprintln!(
            "WARNING: WXML_KEEP_TEST_ROOT keeps {} (manual script runs only)",
            base.path().display()
        );
        println!("KEEP_INSTALL_ROOT={}", root.display());
        std::mem::forget(base);
    }
}

#[test]
fn optional_real_engine_identity_under_clean_path() {
    let asset = std::env::var_os("TSGO_ASSET")
        .or_else(|| std::env::var_os("TSGO_NATIVE_TARBALL"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let explicit_bin = std::env::var_os("TSGO_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if asset.is_none() && explicit_bin.is_none() {
        assert!(
            !environment_flag("WXML_REQUIRE_REAL_ENGINE"),
            "REAL_ENGINE: BLOCKED; WXML_REQUIRE_REAL_ENGINE=1|true requires TSGO_ASSET or TSGO_BIN"
        );
        eprintln!(
            "REAL_ENGINE: SKIP (set TSGO_ASSET/TSGO_NATIVE_TARBALL or TSGO_BIN; \
             WXML_REQUIRE_REAL_ENGINE=1|true fails closed)"
        );
        return;
    }

    let base = PrivateRoot::new("f3-real-engine");
    let shims = NodeShims::install(base.path());
    let engine = explicit_bin.unwrap_or_else(|| {
        let tarball = asset.expect("asset present when no explicit engine binary");
        verify_asset_file(
            &tarball,
            ENGINE_DISTRIBUTION_INTEGRITY,
            ENGINE_DISTRIBUTION_SHA256,
            ENGINE_DISTRIBUTION_SIZE,
        )
        .expect("pinned real engine asset identity");
        let extracted = base.path().join("extract");
        extract_tar_gz(&tarball, &extracted).expect("extract pinned real engine asset");
        extracted.join("package/lib/tsc")
    });
    assert_engine_identity(&engine);
    let mut command = Command::new(&engine);
    command
        .arg("--version")
        .current_dir(engine.parent().expect("engine parent"))
        .env_clear()
        .env("PATH", shims.path())
        .env("LANG", "C")
        .env("LC_ALL", "C");
    let result = run_bounded(&mut command, ENGINE_TIMEOUT, "real engine --version");
    let output = assert_success(&result, "real engine --version");
    assert_eq!(
        output,
        format!("{ENGINE_PROBE_PREFIX}{ENGINE_VERSION}\n"),
        "real engine version line changed"
    );
    shims.assert_absent();
    println!(
        "REAL_ENGINE: PASS (sha256={PINNED_ENGINE_SHA256}, \
         Version {ENGINE_VERSION}, no PATH Node)"
    );
    println!("REAL_ENGINE: install scope covered by real_asset.rs; this check is identity only");
}
