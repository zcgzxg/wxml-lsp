//! Test-only native package provisioning for the first-wave native gates.
//!
//! This module deliberately does **not** add a production package lookup path.
//! A native gate either uses an explicit `WXML_TEST_NATIVE_PACKAGE` or builds a
//! disposable package below `/tmp/opencode` from `TSGO_BIN`/`TSGO_ASSET_ROOT`.
//! The package layout is exactly the one `wxml-lsp --stdio --backend native`
//! discovers from `current_exe`: `<package>/bin/wxml-lsp`, `manifest.json` and
//! `engine/lib/tsc`.
//!
//! Gate semantics:
//!
//! - `WXML_TEST_NATIVE=1` is the explicit request. Running an ignored gate
//!   without it is a failure, never a skip.
//! - `WXML_REQUIRE_REAL_ENGINE=1` is required mode. A missing package or engine
//!   panics with the remediation, including the exact environment variable.
//! - Without required mode a missing package is an **explicit** skip: the
//!   reason is printed, then the gate returns `None`. No silent `return`.
//! - A present but wrong engine (size or SHA-256) is always fatal; a different
//!   engine must never be accepted to make a gate pass.
//!
//! The disposable package builder lives in `native_package.rs`.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use wxml_backend_tsgo::config::PinnedEngine;

use crate::ledger;

/// Set to `1` to request an otherwise `#[ignore]` native gate.
pub(crate) const NATIVE_GATE_ENV: &str = "WXML_TEST_NATIVE";
/// Absolute package root. Test-only; production discovery never reads it.
pub(crate) const NATIVE_PACKAGE_ENV: &str = "WXML_TEST_NATIVE_PACKAGE";
/// Existing convention that turns a missing engine into a hard failure.
pub(crate) const REQUIRE_REAL_ENGINE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";
/// Pinned engine executable for the disposable fixture.
pub(crate) const TSGO_BIN_ENV: &str = "TSGO_BIN";
/// Extracted upstream package root (contains `lib/tsc` and `lib/lib*.d.ts`).
pub(crate) const TSGO_ASSET_ROOT_ENV: &str = "TSGO_ASSET_ROOT";
/// Optional mini-program typings root copied into the fixture.
pub(crate) const TSGO_TYPINGS_ROOT_ENV: &str = "TSGO_TYPINGS_ROOT";
/// Test-only temporary base, defaulting to `/tmp/opencode`.
pub(crate) const TEST_TMPDIR_ENV: &str = "WXML_TEST_TMPDIR";
/// Pinned size of the 7.1.0-beta `lib/tsc` artifact.
pub(crate) const PINNED_ENGINE_SIZE: u64 = 24_522_912;
/// Private base outside the repository.
pub(crate) const DEFAULT_TMP_BASE: &str = "/tmp/opencode";
/// Install-relative package binary.
pub(crate) const PACKAGE_BIN: &str = "bin/wxml-lsp";
/// Install-relative engine executable.
pub(crate) const ENGINE_BINARY: &str = "engine/lib/tsc";
/// Install-relative manifest.
pub(crate) const MANIFEST: &str = "manifest.json";
/// Fixed provenance URL recorded by the manifest projection.
pub(crate) const DISTRIBUTION_URL: &str = "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz";
/// Fixed packed tarball size recorded by the manifest projection.
pub(crate) const DISTRIBUTION_SIZE: u64 = 10_175_135;

/// One read-only projection of the gate-related environment.
///
/// Tests construct this directly so the resolution rules can be exercised
/// without mutating the process environment (which is shared and racy).
#[derive(Debug, Clone, Default)]
pub(crate) struct GateEnv {
    /// `WXML_TEST_NATIVE`.
    pub(crate) test_native: Option<OsString>,
    /// `WXML_TEST_NATIVE_PACKAGE`.
    pub(crate) package: Option<OsString>,
    /// `WXML_REQUIRE_REAL_ENGINE`.
    pub(crate) require_real_engine: Option<OsString>,
    /// `TSGO_BIN`.
    pub(crate) engine_bin: Option<OsString>,
    /// `TSGO_ASSET_ROOT`.
    pub(crate) asset_root: Option<OsString>,
    /// `TSGO_TYPINGS_ROOT`.
    pub(crate) typings_root: Option<OsString>,
    /// `WXML_TEST_TMPDIR`.
    pub(crate) tmp_base: Option<OsString>,
}

impl GateEnv {
    /// Reads the process environment once.
    pub(crate) fn from_process() -> Self {
        Self {
            test_native: std::env::var_os(NATIVE_GATE_ENV),
            package: std::env::var_os(NATIVE_PACKAGE_ENV),
            require_real_engine: std::env::var_os(REQUIRE_REAL_ENGINE_ENV),
            engine_bin: std::env::var_os(TSGO_BIN_ENV),
            asset_root: std::env::var_os(TSGO_ASSET_ROOT_ENV),
            typings_root: std::env::var_os(TSGO_TYPINGS_ROOT_ENV),
            tmp_base: std::env::var_os(TEST_TMPDIR_ENV),
        }
    }

    /// Returns true only for the existing `1`/`true` convention.
    fn flag(value: Option<&OsString>) -> bool {
        value
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    }

    /// True when the native gate was explicitly requested.
    pub(crate) fn requested(&self) -> bool {
        Self::flag(self.test_native.as_ref())
    }

    /// True when a missing package/engine must be fatal.
    pub(crate) fn required(&self) -> bool {
        Self::flag(self.require_real_engine.as_ref())
    }

    /// Returns the temporary base, defaulting to `/tmp/opencode`.
    pub(crate) fn tmp_base(&self) -> PathBuf {
        self.tmp_base
            .as_ref()
            .map_or_else(|| PathBuf::from(DEFAULT_TMP_BASE), PathBuf::from)
    }
}

/// How the package for one gate is obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackageRequest {
    /// An explicitly provisioned package that must already be valid.
    Existing(PathBuf),
    /// A disposable package built from a pinned engine and optional assets.
    Fixture {
        /// Pinned engine executable.
        engine_bin: PathBuf,
        /// Upstream package root that supplies `lib/lib*.d.ts`.
        asset_root: Option<PathBuf>,
        /// Mini-program typings root, when supplied.
        typings_root: Option<PathBuf>,
    },
}

/// Why a native gate cannot be provisioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateError {
    /// The ignored gate was executed without `WXML_TEST_NATIVE=1`.
    NotRequested {
        /// Gate identifier used in the message.
        gate_id: String,
    },
    /// No package/engine exists. `required=false` is the explicit skip.
    MissingPackage {
        /// Gate identifier used in the message.
        gate_id: String,
        /// True when `WXML_REQUIRE_REAL_ENGINE=1`.
        required: bool,
        /// What was missing.
        reason: String,
    },
    /// A package/engine exists but is not the pinned, safe artifact.
    InvalidPackage {
        /// Gate identifier used in the message.
        gate_id: String,
        /// Stable description of the failed invariant.
        reason: String,
    },
}

impl fmt::Display for GateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRequested { gate_id } => write!(
                formatter,
                "native gate {gate_id} was invoked but {NATIVE_GATE_ENV}=1 is not set; \
                 the native gate job must request it explicitly"
            ),
            Self::MissingPackage {
                gate_id, reason, ..
            } => write!(
                formatter,
                "native package for {gate_id} is missing: {reason}; {remediation}",
                remediation = remediation()
            ),
            Self::InvalidPackage { gate_id, reason } => {
                write!(
                    formatter,
                    "native package for {gate_id} is invalid: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for GateError {}

/// Exact remediation printed for a missing package and for the skip path.
fn remediation() -> String {
    format!(
        "provision the pinned package with {NATIVE_PACKAGE_ENV}=/absolute/package/root, \
         or set {TSGO_BIN_ENV} to the pinned 7.1.0-beta engine (sha256 {sha}), or set \
         {TSGO_ASSET_ROOT_ENV} to an extracted upstream package root; \
         {REQUIRE_REAL_ENGINE_ENV}=1 makes a missing native package fatal",
        sha = PinnedEngine::pinned().executable_sha256()
    )
}

fn missing(gate_id: &str, required: bool, reason: String) -> GateError {
    GateError::MissingPackage {
        gate_id: gate_id.to_owned(),
        required,
        reason,
    }
}

pub(crate) fn invalid(gate_id: &str, reason: impl Into<String>) -> GateError {
    GateError::InvalidPackage {
        gate_id: gate_id.to_owned(),
        reason: reason.into(),
    }
}

fn non_empty(value: Option<&OsString>) -> Option<&OsString> {
    value.filter(|value| !value.is_empty())
}

/// Resolves the package request without building anything.
///
/// This is the fail-closed decision boundary: request, required mode, explicit
/// path validation and engine-source discovery. It never hashes a 24 MiB
/// engine, so it is cheap to unit-test.
pub(crate) fn resolve_package_request(
    env: &GateEnv,
    gate_id: &str,
) -> Result<PackageRequest, GateError> {
    if !env.requested() {
        return Err(GateError::NotRequested {
            gate_id: gate_id.to_owned(),
        });
    }
    let required = env.required();

    if let Some(package) = non_empty(env.package.as_ref()) {
        let package = PathBuf::from(package);
        if !package.is_absolute() {
            return Err(invalid(
                gate_id,
                format!(
                    "{NATIVE_PACKAGE_ENV} must be an absolute package root: {}",
                    package.display()
                ),
            ));
        }
        let metadata = match fs::symlink_metadata(&package) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(missing(
                    gate_id,
                    required,
                    format!(
                        "{NATIVE_PACKAGE_ENV} points at {} which does not exist",
                        package.display()
                    ),
                ));
            }
            Err(error) => {
                return Err(invalid(
                    gate_id,
                    format!(
                        "{NATIVE_PACKAGE_ENV} cannot be inspected at {}: {error}",
                        package.display()
                    ),
                ));
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(invalid(
                gate_id,
                format!(
                    "{NATIVE_PACKAGE_ENV} must not be a symlink: {}",
                    package.display()
                ),
            ));
        }
        if !metadata.is_dir() {
            return Err(invalid(
                gate_id,
                format!(
                    "{NATIVE_PACKAGE_ENV} must be a directory: {}",
                    package.display()
                ),
            ));
        }
        let canonical = fs::canonicalize(&package).map_err(|error| {
            invalid(
                gate_id,
                format!(
                    "{NATIVE_PACKAGE_ENV} cannot be canonicalized at {}: {error}",
                    package.display()
                ),
            )
        })?;
        let repository = ledger::workspace_root();
        if canonical.starts_with(&repository) {
            return Err(invalid(
                gate_id,
                format!(
                    "{NATIVE_PACKAGE_ENV} must be outside the repository ({}): {}",
                    repository.display(),
                    canonical.display()
                ),
            ));
        }
        for required_path in [PACKAGE_BIN, MANIFEST, ENGINE_BINARY] {
            let path = canonical.join(required_path);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(invalid(
                        gate_id,
                        format!("package entry {required_path} must not be a symlink"),
                    ));
                }
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(invalid(
                        gate_id,
                        format!("package entry {required_path} must be a regular file"),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(missing(
                        gate_id,
                        required,
                        format!(
                            "explicit native package {} does not contain {required_path}",
                            canonical.display()
                        ),
                    ));
                }
                Err(error) => {
                    return Err(invalid(
                        gate_id,
                        format!("package entry {required_path} cannot be inspected: {error}"),
                    ));
                }
            }
        }
        return Ok(PackageRequest::Existing(canonical));
    }

    let asset_root = match non_empty(env.asset_root.as_ref()) {
        Some(root) => Some(resolve_existing_directory(
            gate_id,
            &PathBuf::from(root),
            TSGO_ASSET_ROOT_ENV,
        )?),
        None => None,
    };

    let engine_bin = if let Some(bin) = non_empty(env.engine_bin.as_ref()) {
        Some(resolve_existing_file(
            gate_id,
            required,
            &PathBuf::from(bin),
            TSGO_BIN_ENV,
        )?)
    } else if let Some(root) = asset_root.as_ref() {
        let candidate = root.join("lib/tsc");
        Some(resolve_existing_file(
            gate_id,
            required,
            &candidate,
            TSGO_ASSET_ROOT_ENV,
        )?)
    } else {
        None
    };

    let Some(engine_bin) = engine_bin else {
        return Err(missing(
            gate_id,
            required,
            "neither a package nor a pinned engine was provided".to_owned(),
        ));
    };

    let typings_root = match non_empty(env.typings_root.as_ref()) {
        Some(root) => {
            let root = PathBuf::from(root);
            let metadata = fs::symlink_metadata(&root).map_err(|error| {
                invalid(
                    gate_id,
                    format!(
                        "{TSGO_TYPINGS_ROOT_ENV} cannot be inspected at {}: {error}",
                        root.display()
                    ),
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(invalid(
                    gate_id,
                    format!(
                        "{TSGO_TYPINGS_ROOT_ENV} must be a real directory: {}",
                        root.display()
                    ),
                ));
            }
            Some(root)
        }
        None => None,
    };

    Ok(PackageRequest::Fixture {
        engine_bin,
        asset_root,
        typings_root,
    })
}

/// Resolves one optional asset directory, rejecting symlinks and non-dirs.
fn resolve_existing_directory(
    gate_id: &str,
    path: &Path,
    variable: &str,
) -> Result<PathBuf, GateError> {
    if !path.is_absolute() {
        return Err(invalid(
            gate_id,
            format!("{variable} must be an absolute path: {}", path.display()),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        invalid(
            gate_id,
            format!(
                "{variable} cannot be inspected at {}: {error}",
                path.display()
            ),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(
            gate_id,
            format!("{variable} must be a real directory: {}", path.display()),
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        invalid(
            gate_id,
            format!(
                "{variable} cannot be canonicalized at {}: {error}",
                path.display()
            ),
        )
    })
}

/// Resolves one required input file, distinguishing missing from unsafe.
fn resolve_existing_file(
    gate_id: &str,
    required: bool,
    path: &Path,
    variable: &str,
) -> Result<PathBuf, GateError> {
    if !path.is_absolute() {
        return Err(invalid(
            gate_id,
            format!("{variable} must be an absolute path: {}", path.display()),
        ));
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(missing(
                gate_id,
                required,
                format!(
                    "{variable} points at {} which does not exist",
                    path.display()
                ),
            ));
        }
        Err(error) => {
            return Err(invalid(
                gate_id,
                format!(
                    "{variable} cannot be inspected at {}: {error}",
                    path.display()
                ),
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(invalid(
            gate_id,
            format!("{variable} must not be a symlink: {}", path.display()),
        ));
    }
    if !metadata.is_file() {
        return Err(invalid(
            gate_id,
            format!("{variable} must be a regular file: {}", path.display()),
        ));
    }
    fs::canonicalize(path).map_err(|error| {
        invalid(
            gate_id,
            format!(
                "{variable} cannot be canonicalized at {}: {error}",
                path.display()
            ),
        )
    })
}
