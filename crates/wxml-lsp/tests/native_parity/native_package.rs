//! Disposable native package builder for the first-wave gates.
//!
//! This module owns the test-only package tree, PATH tripwire and gate
//! workspace. It never adds a production lookup path; the running binary must
//! be `<package>/bin/wxml-lsp` for engine discovery to succeed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::native::supervisor::verify_engine_executable;

use crate::ledger;
use crate::native_gate_env::{
    DEFAULT_TMP_BASE, DISTRIBUTION_SIZE, DISTRIBUTION_URL, ENGINE_BINARY, GateEnv, GateError,
    MANIFEST, PACKAGE_BIN, PINNED_ENGINE_SIZE, PackageRequest, REQUIRE_REAL_ENGINE_ENV,
    TEST_TMPDIR_ENV, invalid, resolve_package_request,
};

static ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::native_gate_env::*;

    fn requested() -> GateEnv {
        GateEnv {
            test_native: Some(OsString::from("1")),
            ..GateEnv::default()
        }
    }

    fn required() -> GateEnv {
        GateEnv {
            require_real_engine: Some(OsString::from("1")),
            ..requested()
        }
    }

    #[test]
    fn running_a_gate_without_the_request_flag_is_a_failure() {
        let error = resolve_package_request(&GateEnv::default(), "G-P5-E2E-09")
            .expect_err("an unrequested gate must not resolve");
        assert!(matches!(error, GateError::NotRequested { .. }), "{error:?}");
        let text = error.to_string();
        assert!(text.contains("WXML_TEST_NATIVE=1"), "{text}");
    }

    #[test]
    fn required_mode_missing_everything_panics_with_remediation() {
        let error = resolve_package_request(&required(), "G-P5-E2E-09")
            .expect_err("required mode must not skip");
        assert!(
            matches!(error, GateError::MissingPackage { required: true, .. }),
            "{error:?}"
        );
        let text = error.to_string();
        assert!(text.contains("native package"), "{text}");
        assert!(text.contains("WXML_TEST_NATIVE_PACKAGE"), "{text}");
        assert!(text.contains("TSGO_BIN"), "{text}");
        assert!(text.contains("TSGO_ASSET_ROOT"), "{text}");
        assert!(text.contains("WXML_REQUIRE_REAL_ENGINE=1"), "{text}");
    }

    #[test]
    fn optional_mode_missing_everything_is_an_explicit_skip() {
        let error = resolve_package_request(&requested(), "G-P5-E2E-12")
            .expect_err("missing package must be reported");
        assert!(
            matches!(
                error,
                GateError::MissingPackage {
                    required: false,
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("WXML_TEST_NATIVE_PACKAGE"));
    }

    #[test]
    fn required_mode_missing_explicit_package_names_the_path() {
        let path = format!(
            "{DEFAULT_TMP_BASE}/wxml-native-gate-does-not-exist-{}",
            std::process::id()
        );
        let env = GateEnv {
            package: Some(OsString::from(&path)),
            ..required()
        };
        let error = resolve_package_request(&env, "G-P5-E2E-14")
            .expect_err("a nonexistent explicit package must fail");
        assert!(
            matches!(error, GateError::MissingPackage { required: true, .. }),
            "{error:?}"
        );
        let text = error.to_string();
        assert!(text.contains(&path), "{text}");
        assert!(text.contains("WXML_TEST_NATIVE_PACKAGE"), "{text}");
    }

    #[test]
    fn a_relative_explicit_package_is_always_invalid() {
        let env = GateEnv {
            package: Some(OsString::from("relative/package")),
            ..requested()
        };
        let error = resolve_package_request(&env, "G-P5-E2E-09")
            .expect_err("a relative package must be rejected");
        assert!(
            matches!(error, GateError::InvalidPackage { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("absolute"), "{error}");
    }

    #[test]
    fn an_explicit_package_inside_the_repository_is_rejected() {
        let env = GateEnv {
            package: Some(ledger::workspace_root().into_os_string()),
            ..requested()
        };
        let error = resolve_package_request(&env, "G-P5-E2E-09")
            .expect_err("the repository must never be a native package");
        assert!(
            matches!(error, GateError::InvalidPackage { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("outside the repository"));
    }

    #[test]
    fn a_missing_engine_binary_is_required_fail_or_optional_skip() {
        let path = format!(
            "{DEFAULT_TMP_BASE}/wxml-native-engine-does-not-exist-{}",
            std::process::id()
        );
        let optional = GateEnv {
            engine_bin: Some(OsString::from(&path)),
            ..requested()
        };
        let error = resolve_package_request(&optional, "G-P5-E2E-12")
            .expect_err("a missing engine must be reported");
        assert!(matches!(
            error,
            GateError::MissingPackage {
                required: false,
                ..
            }
        ));

        let required = GateEnv {
            engine_bin: Some(OsString::from(&path)),
            ..required()
        };
        let error = resolve_package_request(&required, "G-P5-E2E-12")
            .expect_err("required mode must not skip a missing engine");
        assert!(matches!(
            error,
            GateError::MissingPackage { required: true, .. }
        ));
        assert!(error.to_string().contains("TSGO_BIN"), "{error}");
    }

    #[test]
    fn required_mode_accepts_the_true_spelling() {
        let env = GateEnv {
            require_real_engine: Some(OsString::from("TRUE")),
            ..requested()
        };
        assert!(env.required());
        assert!(matches!(
            resolve_package_request(&env, "G-P5-E2E-09"),
            Err(GateError::MissingPackage { required: true, .. })
        ));
    }

    #[test]
    fn an_asset_root_without_lib_tsc_is_reported_as_missing() {
        let root = TestRoot::new(&requested(), "asset-missing");
        let env = GateEnv {
            asset_root: Some(root.path().as_os_str().to_owned()),
            ..required()
        };
        let error = resolve_package_request(&env, "G-P5-E2E-14")
            .expect_err("the asset root has no lib/tsc");
        assert!(
            matches!(error, GateError::MissingPackage { required: true, .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("TSGO_ASSET_ROOT"), "{error}");
    }
}

/// A disposable private directory below the test temp base.
#[derive(Debug)]
pub(crate) struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    /// Creates one `0700` directory outside the repository.
    pub(crate) fn new(env: &GateEnv, label: &str) -> Self {
        let base = env.tmp_base();
        assert!(
            base.is_absolute(),
            "{TEST_TMPDIR_ENV} must be an absolute path: {}",
            base.display()
        );
        fs::create_dir_all(&base)
            .unwrap_or_else(|error| panic!("create test base {}: {error}", base.display()));
        let canonical = fs::canonicalize(&base)
            .unwrap_or_else(|error| panic!("canonicalize base {}: {error}", base.display()));
        let repository = ledger::workspace_root();
        assert!(
            !canonical.starts_with(&repository),
            "{TEST_TMPDIR_ENV} must be outside the repository ({}): {}",
            repository.display(),
            canonical.display()
        );
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = canonical.join(format!(
            "wxml-native-gate-{label}-{}-{nanos}-{}",
            std::process::id(),
            ROOT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)
            .unwrap_or_else(|error| panic!("create test root {}: {error}", path.display()));
        set_mode(&path, 0o700).expect("private test root");
        Self { path }
    }

    /// Returns the private root.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn file_mode(path: &Path) -> io::Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::symlink_metadata(path)?.permissions().mode() & 0o7777)
}

/// PATH tripwire that records any `node`/`npm`/`npx` execution.
#[derive(Debug)]
pub(crate) struct Tripwire {
    dir: PathBuf,
    marker: PathBuf,
}

impl Tripwire {
    fn install(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        set_mode(dir, 0o700)?;
        let marker = dir.join("marker");
        let script = "#!/bin/sh\nprintf '%s\\n' \"$0\" >> \"$WXML_TRIPWIRE_MARKER\"\nexit 99\n";
        for name in ["node", "nodejs", "npm", "npx"] {
            let path = dir.join(name);
            fs::write(&path, script)?;
            set_mode(&path, 0o755)?;
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            marker,
        })
    }

    /// Directory used as the child's complete `PATH`.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Marker written by any shim that ran.
    pub(crate) fn marker(&self) -> &Path {
        &self.marker
    }

    /// Fails when a Node shim was executed.
    pub(crate) fn assert_absent(&self) {
        assert!(
            !self.marker.exists(),
            "the native server executed a Node shim through PATH; marker: {}",
            self.marker.display()
        );
    }
}

/// One provisioned first-wave native gate.
#[derive(Debug)]
pub(crate) struct NativeGate {
    package_root: PathBuf,
    _work: TestRoot,
    tripwire: Tripwire,
    project: PathBuf,
    private_tmp: PathBuf,
}

impl NativeGate {
    /// Provisions the gate, or returns `None` for an explicit optional skip.
    ///
    /// # Panics
    ///
    /// Panics when the gate was invoked without `WXML_TEST_NATIVE=1`, when
    /// required mode is on and the package/engine is missing, and when a
    /// present package/engine is not the pinned, safe artifact.
    pub(crate) fn require(gate_id: &str) -> Option<Self> {
        let env = GateEnv::from_process();
        let request = match resolve_package_request(&env, gate_id) {
            Ok(request) => request,
            Err(GateError::MissingPackage {
                required: false,
                reason,
                ..
            }) => {
                eprintln!("SKIP(native gate {gate_id}): native package is unavailable: {reason}");
                eprintln!(
                    "  explicit optional-mode skip; set {REQUIRE_REAL_ENGINE_ENV}=1 to make it fatal"
                );
                return None;
            }
            Err(error) => panic!("{error}"),
        };

        let work = TestRoot::new(&env, gate_id);
        let package_root = match &request {
            PackageRequest::Existing(path) => path.clone(),
            PackageRequest::Fixture { .. } => {
                build_fixture(&work, gate_id, &request).unwrap_or_else(|error| panic!("{error}"))
            }
        };

        let project = work.path().join("project");
        fs::create_dir_all(&project).expect("create gate project");
        set_mode(&project, 0o755).expect("gate project mode");
        let private_tmp = work.path().join("tmp");
        fs::create_dir(&private_tmp).expect("create private TMPDIR");
        set_mode(&private_tmp, 0o700).expect("private TMPDIR mode");
        let tripwire = Tripwire::install(&work.path().join("tripwire"))
            .unwrap_or_else(|error| panic!("install PATH tripwire: {error}"));

        Some(Self {
            package_root,
            _work: work,
            tripwire,
            project,
            private_tmp,
        })
    }

    /// Install-relative package root.
    pub(crate) fn package_root(&self) -> &Path {
        &self.package_root
    }

    /// `<package>/bin/wxml-lsp`.
    pub(crate) fn bin(&self) -> PathBuf {
        self.package_root.join(PACKAGE_BIN)
    }

    /// Project root the server runs in.
    pub(crate) fn project_root(&self) -> &Path {
        &self.project
    }

    /// Private `TMPDIR` used to observe native sandbox creation.
    pub(crate) fn private_tmp(&self) -> &Path {
        &self.private_tmp
    }

    /// PATH tripwire installed for the child.
    pub(crate) const fn tripwire(&self) -> &Tripwire {
        &self.tripwire
    }

    /// Absolute path of one project file.
    pub(crate) fn project_file(&self, relative: &str) -> PathBuf {
        self.project.join(relative)
    }

    /// Writes one project file, creating parent directories.
    pub(crate) fn write(&self, relative: &str, text: &str) {
        let path = self.project_file(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
        }
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    /// `file://` URI of the project root.
    pub(crate) fn project_uri(&self) -> String {
        format!("file://{}", self.project.display())
    }

    /// `file://` URI of one project file.
    pub(crate) fn uri(&self, relative: &str) -> String {
        format!("file://{}", self.project_file(relative).display())
    }

    /// Fails when the server kept a native session/sandbox alive.
    pub(crate) fn assert_no_native_sandbox(&self) {
        let entries = fs::read_dir(&self.private_tmp).unwrap_or_else(|error| {
            panic!(
                "read private TMPDIR {}: {error}",
                self.private_tmp.display()
            )
        });
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| panic!("private TMPDIR entry: {error}"));
            let name = entry.file_name();
            assert!(
                !name.to_string_lossy().starts_with("wxml-lsp-"),
                "the server started a native engine session (sandbox {}); \
                 this gate must not call the engine",
                entry.path().display()
            );
        }
    }
}

/// Builds the disposable package from the resolved fixture request.
fn build_fixture(
    work: &TestRoot,
    gate_id: &str,
    request: &PackageRequest,
) -> Result<PathBuf, GateError> {
    let PackageRequest::Fixture {
        engine_bin,
        asset_root,
        typings_root,
    } = request
    else {
        return Err(invalid(
            gate_id,
            "internal: build_fixture needs a fixture request",
        ));
    };

    let package = work.path().join("package");
    for directory in [
        package.clone(),
        package.join("bin"),
        package.join("engine"),
        package.join("engine/lib"),
        package.join("typings"),
        package.join("typings/miniprogram-api-typings"),
    ] {
        fs::create_dir_all(&directory).map_err(|error| {
            invalid(
                gate_id,
                format!(
                    "cannot create package directory {}: {error}",
                    directory.display()
                ),
            )
        })?;
        set_mode(&directory, 0o755).map_err(|error| {
            invalid(
                gate_id,
                format!(
                    "cannot set package directory mode on {}: {error}",
                    directory.display()
                ),
            )
        })?;
    }

    let engine_size = verify_engine(gate_id, engine_bin)?;
    let installed_engine = package.join(ENGINE_BINARY);
    fs::copy(engine_bin, &installed_engine).map_err(|error| {
        invalid(
            gate_id,
            format!(
                "cannot copy engine {} to {}: {error}",
                engine_bin.display(),
                installed_engine.display()
            ),
        )
    })?;
    set_mode(&installed_engine, 0o755)
        .map_err(|error| invalid(gate_id, format!("cannot set engine mode: {error}")))?;
    verify_engine(gate_id, &installed_engine)?;

    if let Some(root) = asset_root {
        copy_standard_library(gate_id, root, &package)?;
    }
    if let Some(root) = typings_root {
        let destination = package.join("typings/miniprogram-api-typings");
        copy_tree(gate_id, root, &destination)?;
    }

    let binary = package.join(PACKAGE_BIN);
    fs::copy(env!("CARGO_BIN_EXE_wxml-lsp"), &binary).map_err(|error| {
        invalid(
            gate_id,
            format!(
                "cannot copy {} to {}: {error}",
                env!("CARGO_BIN_EXE_wxml-lsp"),
                binary.display()
            ),
        )
    })?;
    set_mode(&binary, 0o755)
        .map_err(|error| invalid(gate_id, format!("cannot set binary mode: {error}")))?;

    let manifest = manifest_value(gate_id, engine_size, &binary)?;
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| invalid(gate_id, format!("manifest JSON: {error}")))?;
    let manifest_path = package.join(MANIFEST);
    fs::write(&manifest_path, bytes).map_err(|error| {
        invalid(
            gate_id,
            format!("cannot write {}: {error}", manifest_path.display()),
        )
    })?;
    set_mode(&manifest_path, 0o644)
        .map_err(|error| invalid(gate_id, format!("cannot set manifest mode: {error}")))?;

    Ok(package)
}

/// Verifies size, regular-file shape and the pinned SHA-256.
fn verify_engine(gate_id: &str, path: &Path) -> Result<u64, GateError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        invalid(
            gate_id,
            format!("engine {} cannot be inspected: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(invalid(
            gate_id,
            format!("engine must not be a symlink: {}", path.display()),
        ));
    }
    if !metadata.is_file() {
        return Err(invalid(
            gate_id,
            format!("engine must be a regular file: {}", path.display()),
        ));
    }
    if metadata.len() != PINNED_ENGINE_SIZE {
        return Err(invalid(
            gate_id,
            format!(
                "engine {} is {} bytes, expected {PINNED_ENGINE_SIZE} bytes for the pinned 7.1.0-beta artifact",
                path.display(),
                metadata.len()
            ),
        ));
    }
    verify_engine_executable(path, PinnedEngine::pinned().executable_sha256()).map_err(
        |error| {
            invalid(
                gate_id,
                format!(
                    "engine {} failed pinned SHA-256 verification (expected {}): {error}",
                    path.display(),
                    PinnedEngine::pinned().executable_sha256()
                ),
            )
        },
    )?;
    #[cfg(unix)]
    if file_mode(path).map_err(|error| invalid(gate_id, format!("engine mode: {error}")))? != 0o755
    {
        return Err(invalid(
            gate_id,
            format!("engine mode must be 0755: {}", path.display()),
        ));
    }
    Ok(metadata.len())
}

fn copy_standard_library(
    gate_id: &str,
    asset_root: &Path,
    package: &Path,
) -> Result<(), GateError> {
    let lib = asset_root.join("lib");
    let entries = fs::read_dir(&lib).map_err(|error| {
        invalid(
            gate_id,
            format!("cannot read standard library {}: {error}", lib.display()),
        )
    })?;
    let mut copied = 0_usize;
    for entry in entries {
        let entry =
            entry.map_err(|error| invalid(gate_id, format!("standard library: {error}")))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("lib") || !name.ends_with(".d.ts") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| invalid(gate_id, format!("standard library entry: {error}")))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(invalid(
                gate_id,
                format!(
                    "standard library entry must be a regular file: {}",
                    path.display()
                ),
            ));
        }
        let destination = package.join("engine/lib").join(entry.file_name());
        fs::copy(&path, &destination).map_err(|error| {
            invalid(
                gate_id,
                format!(
                    "cannot copy standard library {} to {}: {error}",
                    path.display(),
                    destination.display()
                ),
            )
        })?;
        set_mode(&destination, 0o644)
            .map_err(|error| invalid(gate_id, format!("standard library mode: {error}")))?;
        copied += 1;
    }
    assert!(
        copied > 0,
        "{} has no lib*.d.ts files; refusing to build a package that cannot contain the standard library",
        lib.display()
    );
    Ok(())
}

fn copy_tree(gate_id: &str, source: &Path, destination: &Path) -> Result<(), GateError> {
    for entry in fs::read_dir(source).map_err(|error| {
        invalid(
            gate_id,
            format!("cannot read {}: {error}", source.display()),
        )
    })? {
        let entry = entry.map_err(|error| invalid(gate_id, format!("typings entry: {error}")))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| invalid(gate_id, format!("typings entry: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid(
                gate_id,
                format!("typings entries must not be symlinks: {}", path.display()),
            ));
        }
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir_all(&target).map_err(|error| {
                invalid(
                    gate_id,
                    format!(
                        "cannot create typings directory {}: {error}",
                        target.display()
                    ),
                )
            })?;
            set_mode(&target, 0o755)
                .map_err(|error| invalid(gate_id, format!("typings directory mode: {error}")))?;
            copy_tree(gate_id, &path, &target)?;
        } else if metadata.is_file() {
            fs::copy(&path, &target).map_err(|error| {
                invalid(
                    gate_id,
                    format!(
                        "cannot copy typings {} to {}: {error}",
                        path.display(),
                        target.display()
                    ),
                )
            })?;
            set_mode(&target, 0o644)
                .map_err(|error| invalid(gate_id, format!("typings file mode: {error}")))?;
        }
    }
    Ok(())
}

fn manifest_value(gate_id: &str, engine_size: u64, binary: &Path) -> Result<Value, GateError> {
    let pin = PinnedEngine::pinned();
    let probe_prefix = pin
        .version_line()
        .strip_suffix(pin.version())
        .ok_or_else(|| invalid(gate_id, "pinned version line does not end with the version"))?;
    let binary_size = fs::metadata(binary)
        .map_err(|error| invalid(gate_id, format!("binary metadata: {error}")))?
        .len();
    Ok(json!({
        "schemaVersion": 2,
        "package": "wxml-lsp",
        "packageVersion": env!("CARGO_PKG_VERSION"),
        "platform": "linux-x86_64",
        "protocolVersion": 2,
        "builtAt": "2026-11-18T00:00:00Z",
        "engine": {
            "name": "typescript",
            "version": pin.version(),
            "sourceCommit": pin.git_head(),
            "distribution": pin.package(),
            "distributionUrl": DISTRIBUTION_URL,
            "distributionIntegrity": pin.sri(),
            "distributionSha256": pin.tarball_sha256(),
            "distributionSize": DISTRIBUTION_SIZE,
            "distributionFileCount": pin.file_count(),
            "distributionUnpackedSize": pin.unpacked_bytes(),
            "apiSchemaVersion": pin.ast_protocol_version(),
            "astHeaderBytes": pin.ast_header_bytes(),
            "license": "Apache-2.0",
            "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
            "binary": pin.executable(),
            "probe": {"args": ["--version"], "expectPrefix": probe_prefix}
        },
        "typeAssets": [],
        "binaries": [],
        "files": [
            {
                "path": pin.executable(),
                "sha256": pin.executable_sha256(),
                "size": engine_size,
                "mode": "0755"
            },
            {
                "path": PACKAGE_BIN,
                "sha256": sha256_file(binary).map_err(|error| invalid(gate_id, error))?,
                "size": binary_size,
                "mode": "0755"
            }
        ]
    }))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    Ok(hex_lower(&digest))
}

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        if let (Some(&high), Some(&low)) = (
            DIGITS.get(usize::from(byte >> 4)),
            DIGITS.get(usize::from(byte & 0x0f)),
        ) {
            encoded.push(char::from(high));
            encoded.push(char::from(low));
        }
    }
    encoded
}
