//! Shared real-package fixtures, controlled mutations and evidence lines for
//! the F3 acceptance matrix.
//!
//! Every package used here is built by the production `wxml-lsp-package native`
//! path from the committed `assets.lock.json` and the pinned tarballs. Negative
//! cases are *real-package-derived controlled mutations*: the archive is
//! extracted from the production release, one thing is changed, and the
//! result is repacked with a matching release index. They are never described
//! as a published defective release. Pure v1 Node packages and test-injected
//! journal/failpoint states are printed as `FIXTURE` evidence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use wxml_delivery::assets_lock::{ENGINE_ASSET_NAME, ENGINE_ASSET_ROOT, canonical_tree_digest};
use wxml_delivery::index_v2::read_index_v2;
use wxml_delivery::install::InstallRoot;
use wxml_delivery::manifest::{FileEntry, atomic_write, set_mode, sha256_bytes, sha256_file};
use wxml_delivery::manifest_v2::{
    ENGINE_BINARY_PATH, MINIPROGRAM_TYPINGS_ROOT, ManifestSchema, NATIVE_PACKAGE_PLATFORM,
    NativeArtifactKind, read_manifest_schema, read_manifest_v2,
};
use wxml_delivery::native_install::{CommandEngineProbe, FailPoint, InstallHooks, install_native};
use wxml_delivery::sri::sha512_sri_bytes;
use wxml_delivery::state::{InstallState, Journal, TransactionOp};
use wxml_delivery::verify_v2::{collect_tree, verify_tree_v2};

pub use crate::real_asset_support::{TestDir, Tripwires, mode_of};
pub use crate::release::{
    BUILT_AT, BuiltRelease, PROTOCOL, VERSION_N1, VERSION_N2, build_release, clean_command,
    expected, installed_fingerprint, provision_real_assets, retarget_binary_version,
    write_input_tree,
};

const INSTALL_BIN_ENV: &str = "WXML_F3_INSTALL_BIN";
/// Opt-in path to a channel produced by two real `scripts/build-release.sh`
/// builds (0.1.0 and 0.2.0). When set, the suite uses those packages verbatim
/// instead of retargeting the N1 binary to N2.
pub const REAL_CHANNEL_ENV: &str = "WXML_F3_REAL_CHANNEL";
/// Hard gate: refuse the retargeted-N2 fallback.
pub const REQUIRE_REAL_N2_ENV: &str = "WXML_REQUIRE_REAL_N2";

/// Legacy package version used by the v1 migration fixtures.
pub const LEGACY_VERSION: &str = "0.0.9";
/// Maximum time for one production installer process in this harness.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Prints a machine-greppable evidence line.
pub fn evidence(id: &str, verdict: &str, detail: impl std::fmt::Display) {
    println!("F3-EVIDENCE: {id} {verdict} :: {detail}");
}

/// Explicitly records a skipped real-asset case.
pub fn skip(id: &str, detail: impl std::fmt::Display) {
    println!("F3-EVIDENCE: {id} SKIP :: {detail}");
}

/// Labels evidence that only came from a fixture, never from a real release.
pub fn fixture(id: &str, detail: impl std::fmt::Display) {
    println!("F3-EVIDENCE: {id} FIXTURE :: {detail}");
}

/// Returns true only for `1`/`true`; other values fail closed.
fn strict_flag(name: &str) -> bool {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value.is_empty() => false,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
        Ok(value) => panic!("{name} must be unset, empty, `1` or `true`; got `{value}`"),
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} must be valid UTF-8"),
    }
}

/// Shared, process-wide real package channel.
///
/// All real tests read the same immutable channel; every install uses a
/// separate private root. A channel built inside the suite intentionally
/// outlives its `TestDir`; an imported real channel needs no such anchor.
pub struct SharedChannel {
    /// Keeps an internally built channel alive for the process lifetime.
    _base: Option<&'static TestDir>,
    /// Release channel containing both N1 and N2.
    pub channel: PathBuf,
    /// Real N1 release.
    pub n1: BuiltRelease,
    /// Real N2 release when [`REAL_CHANNEL_ENV`] is set; otherwise the N1
    /// payload with only the two Rust version constants retargeted.
    pub n2: BuiltRelease,
    /// True only when N1 and N2 came from independent real builds.
    pub n2_is_real: bool,
}

static CHANNEL: OnceLock<Option<SharedChannel>> = OnceLock::new();

/// Imports one release from a channel produced by `scripts/build-release.sh`.
fn import_real_release(channel: &Path, version: &str) -> BuiltRelease {
    let release_name = format!("wxml-lsp-{version}-{NATIVE_PACKAGE_PLATFORM}");
    let release_dir = channel.join(&release_name);
    let archive = channel.join(format!("{release_name}.tar.gz"));
    let sidecar = channel.join(format!("{release_name}.tar.gz.sha256"));
    let index = channel.join("index.json");
    let sums = channel.join("SHA256SUMS");
    let manifest = read_manifest_v2(&release_dir, expected(version))
        .unwrap_or_else(|error| panic!("import {version} manifest: {error}"));
    assert_eq!(manifest.package_version, version);
    let parsed = read_index_v2(&index).unwrap_or_else(|error| panic!("import index: {error}"));
    let record = parsed
        .find(version, NATIVE_PACKAGE_PLATFORM)
        .unwrap_or_else(|error| panic!("import index has no {version} release: {error}"));
    record
        .verify_archive(&archive)
        .unwrap_or_else(|error| panic!("import {version} archive: {error}"));
    let archive_sha256 = sha256_file(&archive).expect("imported archive sha256");
    assert_eq!(record.sha256, archive_sha256);
    assert_eq!(
        record.size,
        fs::metadata(&archive).expect("imported archive size").len()
    );
    BuiltRelease {
        output: channel.to_path_buf(),
        release_dir: release_dir.clone(),
        archive,
        sidecar,
        index,
        sums,
        archive_sha256,
        archive_integrity: record.archive_integrity.clone(),
        archive_size: record.size,
        manifest_sha256: sha256_file(&release_dir.join("manifest.json"))
            .expect("imported manifest sha256"),
    }
}

/// Resolves the opt-in real inputs and builds one shared N1/N2 channel.
///
/// With [`REAL_CHANNEL_ENV`] set, the channel is imported verbatim and both
/// releases must be independently built (different archive and `bin/wxml-lsp`
/// SHA-256). Otherwise a channel is built in-process and N2 is the retargeted
/// N1 binary; [`REQUIRE_REAL_N2_ENV`] fails closed instead of using it.
///
/// Returns `None` when the pinned inputs are absent and the hard gate is off.
/// With `WXML_REQUIRE_REAL_ENGINE=1` a missing input panics inside
/// [`crate::release::resolve_real_inputs`] instead of skipping.
pub fn shared_channel(test: &str) -> Option<&'static SharedChannel> {
    CHANNEL
        .get_or_init(|| {
            if let Some(value) = std::env::var_os(REAL_CHANNEL_ENV) {
                assert!(
                    !value.is_empty(),
                    "{REAL_CHANNEL_ENV} must not be empty when set"
                );
                let channel = fs::canonicalize(Path::new(&value)).unwrap_or_else(|error| {
                    panic!(
                        "{REAL_CHANNEL_ENV}={} is invalid: {error}",
                        Path::new(&value).display()
                    )
                });
                assert!(channel.is_dir(), "{} is not a directory", channel.display());
                let n1 = import_real_release(&channel, VERSION_N1);
                let n2 = import_real_release(&channel, VERSION_N2);
                let n1_lsp =
                    sha256_file(&n1.release_dir.join("bin/wxml-lsp")).expect("N1 lsp sha256");
                let n2_lsp =
                    sha256_file(&n2.release_dir.join("bin/wxml-lsp")).expect("N2 lsp sha256");
                let n1_install = sha256_file(&n1.release_dir.join("bin/wxml-lsp-install"))
                    .expect("N1 installer sha256");
                let n2_install = sha256_file(&n2.release_dir.join("bin/wxml-lsp-install"))
                    .expect("N2 installer sha256");
                assert_ne!(
                    n1.archive_sha256, n2.archive_sha256,
                    "real N1/N2 archives must differ"
                );
                assert_ne!(
                    n1_lsp, n2_lsp,
                    "real N1/N2 bin/wxml-lsp must differ; N2 must not be a patched N1"
                );
                assert_ne!(
                    n1_install, n2_install,
                    "real N1/N2 bin/wxml-lsp-install must differ"
                );
                assert_ne!(
                    n1.manifest_sha256, n2.manifest_sha256,
                    "real N1/N2 manifests must differ"
                );
                println!(
                    "REAL-CHANNEL: channel={} n1_archive={} n2_archive={} n1_lsp={} n2_lsp={} \
                     n1_install={} n2_install={}",
                    channel.display(),
                    n1.archive_sha256,
                    n2.archive_sha256,
                    n1_lsp,
                    n2_lsp,
                    n1_install,
                    n2_install
                );
                return Some(SharedChannel {
                    _base: None,
                    channel,
                    n1,
                    n2,
                    n2_is_real: true,
                });
            }
            assert!(
                !strict_flag(REQUIRE_REAL_N2_ENV),
                "{REQUIRE_REAL_N2_ENV}=1 requires {REAL_CHANNEL_ENV} to point at two real \
                 release builds; the in-process N2 is a retargeted N1 binary"
            );
            let inputs = crate::release::resolve_real_inputs(test)?;
            let base: &'static TestDir = Box::leak(Box::new(TestDir::new("f3-acceptance-shared")));
            let provisioned = provision_real_assets(base, &inputs);
            let input_tree = write_input_tree(base, &inputs, &provisioned);
            let channel = base.path().join("channel");
            let n1 = build_release(&channel, &input_tree, &provisioned, VERSION_N1, true);
            let n2_tree = base.path().join("input-tree-n2");
            copy_tree(&input_tree, &n2_tree);
            for name in ["wxml-lsp", "wxml-lsp-install"] {
                retarget_binary_version(
                    &input_tree.join("bin").join(name),
                    &n2_tree.join("bin").join(name),
                    VERSION_N1,
                    VERSION_N2,
                );
            }
            let n2 = build_release(&channel, &n2_tree, &provisioned, VERSION_N2, true);
            println!(
                "SHARED-CHANNEL: root={} n1={} n2={} install={} (N2 retargeted fixture)",
                base.path().display(),
                n1.archive_sha256,
                n2.archive_sha256,
                inputs.install_bin.display()
            );
            Some(SharedChannel {
                _base: Some(base),
                channel,
                n1,
                n2,
                n2_is_real: false,
            })
        })
        .as_ref()
}

/// Returns the production installer path resolved by the real-input probe.
pub fn installer_bin() -> PathBuf {
    match std::env::var_os(INSTALL_BIN_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => {
            let built = PathBuf::from(env!("CARGO_BIN_EXE_wxml-lsp-install"));
            assert!(built.is_file(), "test-built installer is missing");
            built
        }
    }
}

/// Creates the isolated `work`, `home` and `tmp` directories used per test.
pub fn workspace(base: &TestDir) -> (PathBuf, PathBuf, PathBuf) {
    let work = base.path().join("work");
    let home = base.path().join("home");
    let tmp = base.path().join("tmp");
    for directory in [&work, &home, &tmp] {
        fs::create_dir_all(directory).expect("create isolated directory");
        set_mode(directory, 0o700).expect("private isolated directory");
    }
    (work, home, tmp)
}

/// Creates a fresh private install root below the test workspace.
pub fn private_root(base: &TestDir, name: &str) -> PathBuf {
    let root = base.path().join(name);
    fs::create_dir(&root).expect("create install root");
    set_mode(&root, 0o700).expect("private install root");
    root
}

/// Runs one installer with the dead-proxy / Node-tripwire environment.
pub fn run_installer(
    install_bin: &Path,
    args: &[&str],
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> Output {
    run_installer_env(install_bin, args, &[], tripwires, work, label)
}

/// Runs one installer with additional explicit environment variables.
///
/// Used by the F3-15 precedence cases; every other variable still comes from
/// the dead-proxy / Node-tripwire baseline.
pub fn run_installer_env(
    install_bin: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> Output {
    let mut command = clean_command(install_bin, tripwires, work);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = run_with_deadline(&mut command, INSTALL_TIMEOUT, label);
    tripwires.assert_not_executed();
    output
}

/// Spawns a child and waits with a hard deadline, killing it on timeout.
pub fn run_with_deadline(command: &mut Command, timeout: Duration, label: &str) -> Output {
    use std::process::Stdio;
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {label}: {error}"));
    wait_child(child, timeout, label)
}

fn wait_child(mut child: Child, timeout: Duration, label: &str) -> Output {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .unwrap_or_else(|error| panic!("wait for {label}: {error}"));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("wait for {label}: {error}");
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("wait for timed-out {label}: {error}"));
            panic!(
                "{label} timed out after {timeout:?}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Spawns two installers that must serialize on the same install root.
pub fn run_two_installers(
    first: &mut Command,
    second: &mut Command,
    timeout: Duration,
    tripwires: &Tripwires,
) -> (Output, Output) {
    use std::process::Stdio;
    for command in [&mut *first, &mut *second] {
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
    }
    let first_child = first.spawn().expect("spawn first installer");
    let second_child = second.spawn().expect("spawn second installer");
    let (first, second) = std::thread::scope(|scope| {
        let second = scope.spawn(|| wait_child(second_child, timeout, "second installer"));
        let first = wait_child(first_child, timeout, "first installer");
        (first, second.join().expect("second installer thread"))
    });
    tripwires.assert_not_executed();
    (first, second)
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn combined(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

pub fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        stdout(output),
        stderr(output)
    );
}

pub fn assert_failure(output: &Output, label: &str) {
    assert!(
        !output.status.success(),
        "{label} unexpectedly succeeded: stdout={} stderr={}",
        stdout(output),
        stderr(output)
    );
}

pub fn assert_contains(output: &Output, needles: &[&str]) {
    let text = combined(output).to_ascii_lowercase();
    assert!(
        needles
            .iter()
            .any(|needle| text.contains(&needle.to_ascii_lowercase())),
        "expected one of {needles:?}: {text}"
    );
}

/// Opens an existing install root.
pub fn open_root(path: &Path) -> InstallRoot {
    InstallRoot::open(path)
        .expect("open install root")
        .expect("install root exists")
}

/// Reads `state.json`, if present.
pub fn state_bytes(path: &Path) -> Option<Vec<u8>> {
    let path = path.join("state.json");
    match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("read {}: {error}", path.display()),
    }
}

pub fn active_version(root: &InstallRoot) -> Option<String> {
    root.current_version().expect("read current")
}

pub fn installed_versions(root: &InstallRoot) -> Vec<String> {
    root.versions().expect("list installed versions")
}

pub fn assert_journal_empty(root: &InstallRoot) {
    let journals = Journal::list(root).expect("list journals");
    assert!(journals.is_empty(), "journals remain: {journals:?}");
}

pub fn assert_tmp_empty(root: &InstallRoot) {
    if root.tmp_dir().exists() {
        let mut entries = fs::read_dir(root.tmp_dir()).expect("read tmp");
        assert!(
            entries.next().is_none(),
            "staging files were left in {}",
            root.tmp_dir().display()
        );
    }
}

/// Asserts that a failed first install left no activation state at all.
pub fn assert_no_activation(root: &Path) {
    assert!(!root.join("current").exists(), "current must not exist");
    assert!(
        !root.join("state.json").exists(),
        "state.json must not exist"
    );
    if root.join("versions").exists() {
        let mut entries = fs::read_dir(root.join("versions")).expect("read versions");
        assert!(
            entries.next().is_none(),
            "a version directory was created before activation"
        );
    }
    if root.join("tmp").exists() {
        let mut entries = fs::read_dir(root.join("tmp")).expect("read tmp");
        assert!(entries.next().is_none(), "staging files were left behind");
    }
    assert_eq!(mode_of(root), 0o700, "install root mode changed");
}

/// Asserts that `<root>/current` resolves to `versions/<version>` and that the
/// resolved tree still verifies against its own manifest.
///
/// This is the explicit "current is always valid" sampling probe: callers name
/// each critical point and print the result instead of claiming continuous
/// monitoring. Returns the verified file count.
pub fn assert_current_verified(root_path: &Path, version: &str) -> usize {
    let root = open_root(root_path);
    assert_eq!(
        active_version(&root).as_deref(),
        Some(version),
        "current does not name {version}"
    );
    let link = fs::read_link(root_path.join("current")).expect("read current link");
    assert_eq!(
        link,
        Path::new("versions").join(version),
        "current must be a relative versions/<version> link"
    );
    let versions = fs::canonicalize(root.versions_dir()).expect("canonical versions directory");
    let resolved = fs::canonicalize(root_path.join("current")).expect("resolve current link");
    assert!(
        resolved.starts_with(&versions),
        "current escapes versions/: {}",
        resolved.display()
    );
    assert_eq!(
        resolved,
        fs::canonicalize(root.versions_dir().join(version)).expect("canonical version directory"),
        "current does not resolve to versions/{version}"
    );
    let report = verify_tree_v2(&resolved, expected(version))
        .expect("the active current tree must verify against its manifest");
    assert_eq!(report.manifest.package_version, version);
    report.file_count
}

/// Returns the installer-written archive cache entry for `version`, if any.
///
/// The entry directory name contains the release SHA-256 prefix, so callers
/// must not construct it; the `entry.json` identity is authoritative.
pub fn cached_entry(root: &InstallRoot, version: &str) -> Option<(PathBuf, String)> {
    let directory = wxml_delivery::acquire_v2::native_cache_dir(root);
    let entries = fs::read_dir(&directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(bytes) = fs::read(path.join("entry.json")) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if value["version"].as_str() == Some(version) {
            let sha256 = value["sha256"].as_str().unwrap_or_default().to_owned();
            return Some((path, sha256));
        }
    }
    None
}

/// Asserts the previous current version and its tree are untouched.
pub fn assert_previous_current(
    root_path: &Path,
    version: &str,
    fingerprint: &str,
    state_before: Option<&[u8]>,
) {
    let root = open_root(root_path);
    assert_eq!(active_version(&root).as_deref(), Some(version));
    assert_eq!(installed_versions(&root), vec![version.to_owned()]);
    assert_eq!(
        installed_fingerprint(root_path, version),
        fingerprint,
        "{version} tree changed"
    );
    assert_eq!(state_bytes(root_path).as_deref(), state_before);
    assert_tmp_empty(&root);
    assert_journal_empty(&root);
}

/// Copies a directory tree without following symlinks.
pub fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("create copied root");
    set_mode(destination, 0o755).expect("copied root mode");
    let mut entries: Vec<_> = fs::read_dir(source)
        .expect("read source tree")
        .map(|entry| entry.expect("source entry").path())
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("file name"));
        let metadata = fs::symlink_metadata(&entry).expect("source metadata");
        if metadata.file_type().is_dir() {
            copy_tree(&entry, &target);
        } else {
            assert!(
                metadata.file_type().is_file(),
                "unexpected non-file {} in copied tree",
                entry.display()
            );
            fs::copy(&entry, &target).expect("copy file");
            set_mode(&target, mode_of(&entry)).expect("copied file mode");
        }
    }
}

/// Extracts a production release archive into a fresh tree and verifies it.
pub fn extract_release_tree(release: &BuiltRelease, destination: &Path) {
    wxml_delivery::archive::extract_tar_gz(&release.archive, destination)
        .expect("extract production release");
    verify_tree_v2(destination, expected(&release_version(release)))
        .expect("extracted production tree must verify");
}

/// Derives the package version from a built release directory name.
pub fn release_version(release: &BuiltRelease) -> String {
    release
        .release_dir
        .file_name()
        .expect("release directory name")
        .to_string_lossy()
        .strip_prefix("wxml-lsp-")
        .and_then(|rest| rest.strip_suffix(&format!("-{NATIVE_PACKAGE_PLATFORM}")))
        .expect("release directory carries the version")
        .to_owned()
}

/// A repacked real-package-derived variant plus its from-dir release.
#[derive(Debug)]
pub struct Variant {
    /// Directory accepted by `install --from-dir`.
    pub release_dir: PathBuf,
    /// Repacked archive.
    pub archive: PathBuf,
    /// Archive SHA-256 (also written into the release index).
    pub sha256: String,
}

/// Extracts a production archive, applies one controlled mutation, repacks it
/// with a matching release index, and returns the from-dir channel.
///
/// The payload bytes outside the mutation are the production packager output.
/// The resulting archive is therefore *not* a published release; callers label
/// it as a real-package-derived controlled mutation in the evidence line.
pub fn mutate_release(
    base: &Path,
    release: &BuiltRelease,
    name: &str,
    mutate: impl FnOnce(&Path),
) -> Variant {
    let release_dir = base.join(name);
    fs::create_dir(&release_dir).expect("create variant release dir");
    set_mode(&release_dir, 0o755).expect("variant release dir mode");
    let tree = release_dir.join("tree");
    wxml_delivery::archive::extract_tar_gz(&release.archive, &tree)
        .expect("extract production archive for mutation");
    mutate(&tree);

    let contents = collect_tree(&tree).expect("collect mutated tree");
    let mut entries = Vec::with_capacity(contents.files.len());
    for path in &contents.files {
        let file = tree.join(path);
        let metadata = fs::symlink_metadata(&file).expect("mutated file metadata");
        entries.push(FileEntry {
            path: path.clone(),
            sha256: sha256_file(&file).expect("hash mutated file"),
            size: metadata.len(),
            mode: format!("{:04o}", mode_of(&file)),
        });
    }
    let archive_name = release
        .archive
        .file_name()
        .expect("release archive name")
        .to_owned();
    let archive = release_dir.join(&archive_name);
    wxml_delivery::archive::create_tar_gz(&tree, &archive, &entries, 0)
        .expect("repack mutated release");
    let bytes = fs::read(&archive).expect("read repacked archive");
    let sha256 = sha256_bytes(&bytes);
    let integrity = sha512_sri_bytes(&bytes).to_sri();
    let mut index = read_index_v2(&release.output.join("index.json")).expect("read release index");
    let version = release_version(release);
    let entry = index
        .releases
        .iter_mut()
        .find(|entry| entry.version == version && entry.platform == NATIVE_PACKAGE_PLATFORM)
        .expect("release index has the package version");
    entry.sha256.clone_from(&sha256);
    entry.archive_integrity = integrity;
    entry.size = u64::try_from(bytes.len()).expect("variant archive size");
    atomic_write(
        &release_dir.join("index.json"),
        &index.to_json().expect("serialize variant index"),
        0o644,
    )
    .expect("write variant index");
    Variant {
        release_dir,
        archive,
        sha256,
    }
}

/// Publishes arbitrary archive bytes under the release's canonical archive
/// name with a matching, valid schema-v2 index.
pub fn publish_archive_bytes(
    base: &Path,
    release: &BuiltRelease,
    name: &str,
    bytes: &[u8],
) -> Variant {
    let release_dir = base.join(name);
    fs::create_dir(&release_dir).expect("create malicious release dir");
    set_mode(&release_dir, 0o755).expect("malicious release dir mode");
    let archive = release_dir.join(
        release
            .archive
            .file_name()
            .expect("release archive file name"),
    );
    fs::write(&archive, bytes).expect("write malicious archive");
    set_mode(&archive, 0o644).expect("malicious archive mode");
    let sha256 = sha256_bytes(bytes);
    let integrity = sha512_sri_bytes(bytes).to_sri();
    let mut index = read_index_v2(&release.output.join("index.json")).expect("read release index");
    let version = release_version(release);
    let entry = index
        .releases
        .iter_mut()
        .find(|entry| entry.version == version && entry.platform == NATIVE_PACKAGE_PLATFORM)
        .expect("release index has the package version");
    entry.sha256.clone_from(&sha256);
    entry.archive_integrity = integrity;
    entry.size = u64::try_from(bytes.len()).expect("malicious archive size");
    atomic_write(
        &release_dir.join("index.json"),
        &index.to_json().expect("serialize malicious index"),
        0o644,
    )
    .expect("write malicious index");
    Variant {
        release_dir,
        archive,
        sha256,
    }
}

/// Rewrites `engine/lib/tsc` into a well-formed package whose engine probe
/// reports the wrong version. Manifest, provenance and checksums are updated so
/// the tree passes `verify_tree_v2`; the probe is the only failing gate.
pub fn make_probe_failure(tree: &Path, version: &str) {
    let script = b"#!/bin/sh\nprintf 'Version 9.9.9\\n'\n";
    let engine = tree.join(ENGINE_BINARY_PATH);
    fs::write(&engine, script).expect("write probe-failure engine");
    set_mode(&engine, 0o755).expect("probe-failure engine mode");
    let engine_size = u64::try_from(script.len()).expect("engine size");
    let engine_sha256 = sha256_bytes(script);

    let mut manifest = read_manifest_v2(tree, expected(version)).expect("read variant manifest");
    let file = manifest
        .files
        .iter_mut()
        .find(|file| file.path == ENGINE_BINARY_PATH)
        .expect("manifest lists the engine");
    file.sha256.clone_from(&engine_sha256);
    file.size = engine_size;
    let artifact = manifest
        .binaries
        .iter_mut()
        .find(|artifact| artifact.kind == NativeArtifactKind::NativeEngine)
        .expect("manifest lists the native engine artifact");
    artifact.sha256.clone_from(&engine_sha256);

    let provenance_path = tree.join("provenance.json");
    let mut provenance: Value = serde_json::from_slice(
        &fs::read(&provenance_path).expect("read provenance for probe variant"),
    )
    .expect("parse provenance");
    provenance["engine"]["binary"]["sha256"] = Value::String(engine_sha256.clone());
    provenance["engine"]["binary"]["size"] = Value::from(engine_size);
    if let Some(engine_files) = provenance["engineFiles"].as_array_mut() {
        for entry in engine_files {
            if entry["path"].as_str() == Some(ENGINE_BINARY_PATH) {
                entry["sha256"] = Value::String(engine_sha256.clone());
                entry["size"] = Value::from(engine_size);
            }
        }
    }
    let engine_entries: Vec<FileEntry> = manifest
        .files
        .iter()
        .filter(|file| file.path.starts_with(&format!("{ENGINE_ASSET_ROOT}/")))
        .cloned()
        .collect();
    let engine_digest =
        canonical_tree_digest(&engine_entries, ENGINE_ASSET_ROOT).expect("engine tree digest");
    let engine_count = u64::try_from(engine_entries.len()).expect("engine file count");
    let engine_bytes = engine_entries
        .iter()
        .try_fold(0_u64, |sum, file| sum.checked_add(file.size))
        .expect("engine bytes");
    let assets = provenance["assets"]
        .as_array_mut()
        .expect("provenance assets");
    let engine_asset = assets
        .iter_mut()
        .find(|entry| entry["name"].as_str() == Some(ENGINE_ASSET_NAME))
        .expect("provenance engine asset");
    engine_asset["payloadTreeDigest"] = Value::String(engine_digest);
    engine_asset["fileCount"] = Value::from(engine_count);
    engine_asset["totalBytes"] = Value::from(engine_bytes);
    let mut provenance_bytes =
        serde_json::to_vec_pretty(&provenance).expect("serialize probe provenance");
    provenance_bytes.push(b'\n');
    fs::write(&provenance_path, &provenance_bytes).expect("write probe provenance");
    set_mode(&provenance_path, 0o644).expect("probe provenance mode");
    let provenance_entry = manifest
        .files
        .iter_mut()
        .find(|file| file.path == "provenance.json")
        .expect("manifest lists provenance");
    provenance_entry.sha256 = sha256_bytes(&provenance_bytes);
    provenance_entry.size = u64::try_from(provenance_bytes.len()).expect("provenance size");

    atomic_write(
        &tree.join("manifest.json"),
        &manifest.to_json().expect("serialize probe manifest"),
        0o644,
    )
    .expect("write probe manifest");
    let checksums =
        wxml_delivery::verify_v2::checksums_for_tree_v2(tree, &manifest).expect("probe checksums");
    atomic_write(&tree.join("SHA256SUMS"), &checksums, 0o644).expect("write probe checksums");
    verify_tree_v2(tree, expected(version))
        .expect("probe-failure variant must pass tree verification");
}

/// Removes one standard-library file from a real package tree.
pub fn remove_standard_lib(tree: &Path) {
    let victim = tree.join("engine/lib/lib.es2020.d.ts");
    assert!(victim.is_file(), "standard-library fixture file is missing");
    fs::remove_file(victim).expect("remove standard-library file");
}

/// Removes the typings entry point from a real package tree.
pub fn remove_typings(tree: &Path) {
    let victim = tree.join(MINIPROGRAM_TYPINGS_ROOT).join("index.d.ts");
    assert!(victim.is_file(), "typings fixture file is missing");
    fs::remove_file(victim).expect("remove typings file");
}

/// Flips one byte while keeping the payload length unchanged.
pub fn flip_payload_byte(tree: &Path, relative: &str) {
    let path = tree.join(relative);
    let mut bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("read {} for tampering: {error}", path.display()));
    assert!(!bytes.is_empty(), "cannot tamper an empty file");
    let index = bytes.len() / 2;
    bytes[index] ^= 0x01;
    fs::write(&path, &bytes).expect("write tampered payload");
}

/// Adds a file that no manifest or checksum lists.
pub fn add_extra_file(tree: &Path) {
    let path = tree.join("engine/lib/extra-acceptance.d.ts");
    fs::write(&path, b"// unexpected extra payload\n").expect("write extra file");
    set_mode(&path, 0o644).expect("extra file mode");
}

/// Removes a file that the manifest and checksums still require.
pub fn remove_required_file(tree: &Path, relative: &str) {
    let path = tree.join(relative);
    assert!(path.is_file(), "required file {relative} is missing");
    fs::remove_file(path).expect("remove required file");
}

/// Rewrites only `manifest.json`, leaving `SHA256SUMS` stale.
pub fn tamper_manifest(tree: &Path) {
    let path = tree.join("manifest.json");
    let mut value: Value =
        serde_json::from_slice(&fs::read(&path).expect("read manifest")).expect("parse manifest");
    let built_at = value["builtAt"]
        .as_str()
        .expect("manifest builtAt")
        .to_owned();
    let replacement = if built_at == "2026-11-19T00:00:00Z" {
        "2026-11-20T00:00:00Z"
    } else {
        "2026-11-19T00:00:00Z"
    };
    value["builtAt"] = Value::String(replacement.to_owned());
    let mut bytes = serde_json::to_vec_pretty(&value).expect("serialize tampered manifest");
    bytes.push(b'\n');
    fs::write(&path, &bytes).expect("write tampered manifest");
    set_mode(&path, 0o644).expect("tampered manifest mode");
}

/// Installs a real N1 into a fresh root through the production CLI.
pub fn install_n1(
    install_bin: &Path,
    channel: &SharedChannel,
    root: &Path,
    tripwires: &Tripwires,
    work: &Path,
) -> Output {
    run_installer(
        install_bin,
        &[
            "install",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--from-dir",
            channel.channel.to_str().expect("UTF-8 channel"),
            "--version",
            VERSION_N1,
            "--offline",
        ],
        tripwires,
        work,
        "install real N1",
    )
}

/// Upgrades an N1 root to the real N2 through the production CLI.
pub fn upgrade_n2(
    install_bin: &Path,
    channel: &SharedChannel,
    root: &Path,
    tripwires: &Tripwires,
    work: &Path,
) -> Output {
    run_installer(
        install_bin,
        &[
            "upgrade",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--from-dir",
            channel.channel.to_str().expect("UTF-8 channel"),
            "--offline",
        ],
        tripwires,
        work,
        "upgrade real N2",
    )
}

/// Attempts to install one variant channel into a root.
pub fn install_variant(
    install_bin: &Path,
    variant: &Variant,
    version: &str,
    root: &Path,
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> Output {
    run_installer(
        install_bin,
        &[
            "install",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--from-dir",
            variant.release_dir.to_str().expect("UTF-8 variant release"),
            "--version",
            version,
            "--offline",
        ],
        tripwires,
        work,
        label,
    )
}

/// Stages a production archive directly under `<root>/tmp` for the library
/// failpoint API. The returned path can be renamed by the real transaction.
pub fn stage_release_tree(root: &InstallRoot, release: &BuiltRelease) -> PathBuf {
    let staged = root
        .new_temp_dir("accept-stage")
        .expect("create staging dir");
    fs::remove_dir(&staged).expect("clear staging placeholder");
    wxml_delivery::archive::extract_tar_gz(&release.archive, &staged)
        .expect("extract production release into staging");
    verify_tree_v2(&staged, expected(&release_version(release)))
        .expect("staged production tree must verify");
    staged
}

#[derive(Debug)]
pub struct FailAt {
    pub point: FailPoint,
}

impl InstallHooks for FailAt {
    fn checkpoint(&self, point: FailPoint) -> wxml_delivery::Result<()> {
        if point == self.point {
            return Err(wxml_delivery::error::DeliveryError::Runtime(format!(
                "injected acceptance failure at {point:?}"
            )));
        }
        Ok(())
    }
}

/// Runs the real library transaction with the production engine probe and an
/// injected stop at one journal checkpoint.
pub fn install_with_failpoint(
    root: &InstallRoot,
    staged: &Path,
    version: &str,
    point: FailPoint,
) -> wxml_delivery::Result<wxml_delivery::native_install::NativeInstallOutcome> {
    install_native(
        root,
        staged,
        expected(version),
        &CommandEngineProbe::default(),
        &FailAt { point },
    )
}

/// Runs the real native rollback with an injected stop.
pub fn rollback_with_failpoint(
    root: &InstallRoot,
    to: &str,
    point: FailPoint,
) -> wxml_delivery::Result<wxml_delivery::native_install::NativeInstallOutcome> {
    wxml_delivery::native_install::rollback_native(
        root,
        to,
        &CommandEngineProbe::default(),
        &FailAt { point },
    )
}

/// Builds a legacy schema v1 Node-package fixture through the production v1
/// packager. The `wxml-lsp` entry point is a tripwire that records execution.
pub fn build_legacy_release(base: &TestDir, marker: &Path) -> PathBuf {
    let input = base.path().join("legacy-input");
    fs::create_dir_all(input.join("bin/wxml-type-backend/node_modules/typescript"))
        .expect("create legacy input");
    fs::create_dir_all(input.join("bin/wxml-type-backend/node_modules/miniprogram-api-typings"))
        .expect("create legacy input");
    fs::create_dir_all(input.join("zed-extension")).expect("create legacy input");
    let script = format!(
        "#!/bin/sh\n: > {}\nif [ \"$1\" = \"--version\" ]; then printf 'wxml-lsp {LEGACY_VERSION}\\n'; exit 0; fi\nexit 0\n",
        shell_quote(marker)
    );
    write_file(&input.join("bin/wxml-lsp"), script.as_bytes(), 0o755);
    write_file(
        &input.join("bin/wxml-lsp-install"),
        format!("#!/bin/sh\nprintf 'wxml-lsp-install {LEGACY_VERSION}\\n'\n").as_bytes(),
        0o755,
    );
    write_file(
        &input.join("bin/wxml-type-backend/server.js"),
        b"console.log('legacy fixture');\n",
        0o644,
    );
    write_file(
        &input.join("bin/wxml-type-backend/node_modules/typescript/package.json"),
        b"{}\n",
        0o644,
    );
    write_file(
        &input.join("bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json"),
        b"{}\n",
        0o644,
    );
    write_file(
        &input.join("zed-extension/extension.wasm"),
        b"\0asm-legacy-fixture",
        0o644,
    );
    write_file(
        &input.join("zed-extension/extension.toml"),
        b"id = \"wxml-lsp\"\nversion = \"0.0.1\"\n",
        0o644,
    );
    write_file(
        &input.join("THIRD_PARTY_LICENSES.md"),
        b"# licenses\n",
        0o644,
    );
    let output = base.path().join("legacy-release");
    wxml_delivery::package::build_package(&wxml_delivery::package::BuildOptions {
        input,
        output: output.clone(),
        release_version: LEGACY_VERSION.to_owned(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: 2,
        node_minimum: "24.0.0".to_owned(),
        backend_version: "0.1.0".to_owned(),
        zed_version: Some("0.0.1".to_owned()),
        built_at: BUILT_AT.to_owned(),
        mtime: 0,
        force: true,
        dry_run: false,
    })
    .expect("build legacy v1 fixture package");
    output
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create legacy file parent");
    }
    fs::write(path, bytes).expect("write legacy fixture file");
    set_mode(path, mode).expect("legacy fixture file mode");
}

/// Installs the legacy fixture with the production schema-v1 flow.
pub fn install_legacy(
    install_bin: &Path,
    legacy_release: &Path,
    root: &Path,
    tripwires: &Tripwires,
    work: &Path,
) -> Output {
    run_installer(
        install_bin,
        &[
            "install",
            "--root",
            root.to_str().expect("UTF-8 root"),
            "--from-dir",
            legacy_release.to_str().expect("UTF-8 legacy release"),
            "--version",
            LEGACY_VERSION,
            "--offline",
        ],
        tripwires,
        work,
        "install legacy v1 fixture",
    )
}

/// Clears the legacy tripwire marker after the install's own `--version` check.
pub fn clear_marker(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("clear marker {}: {error}", path.display()),
    }
}

pub fn assert_marker_absent(path: &Path) {
    assert!(
        !path.exists(),
        "legacy entry point was executed after install: {}",
        path.display()
    );
}

/// Returns the legacy tree fingerprint.
pub fn legacy_fingerprint(root: &Path, version: &str) -> String {
    wxml_delivery::verify_v2::tree_fingerprint(&root.join("versions").join(version))
        .expect("legacy tree fingerprint")
}

pub fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Asserts the active state matches `active`/`previous`.
pub fn assert_state(root_path: &Path, active: &str, previous: Option<&str>) {
    let root = open_root(root_path);
    let state = InstallState::read(&root)
        .expect("read state.json")
        .expect("state.json exists");
    assert_eq!(state.active, active);
    assert_eq!(state.previous.as_deref(), previous);
    state.validate_against(&root).expect("state matches disk");
}

/// Writes a valid journal directly for crash-window acceptance.
pub fn write_journal(
    root: &InstallRoot,
    op: TransactionOp,
    target: &str,
    previous: Option<&str>,
    state: wxml_delivery::state::TransactionState,
) -> Journal {
    let now = "2026-11-19T00:00:00Z";
    let mut journal = Journal::new(op, target, previous, now).expect("new journal");
    if state != wxml_delivery::state::TransactionState::Staged {
        journal.transition(state, now).expect("journal transition");
    }
    journal.write(root).expect("write journal");
    journal
}

/// Confirms the legacy schema is readable and the tree is a real directory.
pub fn assert_legacy_current(root_path: &Path) {
    let root = open_root(root_path);
    let version = active_version(&root).expect("legacy current");
    assert_eq!(version, LEGACY_VERSION);
    assert_eq!(
        read_manifest_schema(&root.versions_dir().join(&version)).expect("legacy manifest schema"),
        ManifestSchema::V1
    );
}
