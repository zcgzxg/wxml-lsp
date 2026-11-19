//! Test-only fixture and JSON-RPC client for the native `documentSymbol` gate.
//!
//! The fixture builds a disposable package below `WXML_TEST_TMPDIR` (default
//! `/tmp/opencode`) from a pinned `TSGO_BIN` and the standard library and
//! typings that the production startup path requires. It never writes inside
//! the repository and never selects the Node backend.
//!
//! Gate semantics:
//!
//! - `TSGO_BIN` is optional. Only a *missing* input
//!   ([`ProvisionError::Missing`]) becomes an explicit `SKIP` print plus
//!   `None` in optional mode; there is no silent pass.
//! - A present but invalid input is always fatal, even in optional mode:
//!   a relative path, a wrong engine size or SHA-256, an unusable
//!   `TSGO_ASSET_ROOT`, an unusable `TSGO_TYPINGS_ROOT`, or a temporary base
//!   inside the repository.
//! - `WXML_REQUIRE_REAL_ENGINE=1|true` turns every missing input into a panic.
//! - A different engine is never accepted to make the gate pass.
//!
//! # Compatibility / commit ordering
//!
//! This gate requires the trusted type-environment handoff. The opt-in
//! `run_stdio_native` path must build the session through
//! `with_install` -> `environment::load_environment` ->
//! `AnalysisInput::with_environment`, which is what makes the project
//! `tsconfig.json` `allowJs` option visible to the native program. With the
//! legacy `AnalysisInput::new` path the `.js` file is not in the program and
//! the query fails; it does not skip. Commit this test after the handoff.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn,
    clippy::redundant_pub_crate,
    clippy::items_after_statements,
    clippy::similar_names,
    missing_docs
)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "lsp.rs"]
mod lsp;
#[path = "package.rs"]
mod package;

pub(crate) use lsp::{Session, run_startup_failure};
pub(crate) use package::{
    PROJECT_JS, PROJECT_TS, PROJECT_WXML, PackageDefect, broken_package, provision,
    provision_with_env,
};

pub(crate) const REQUIRE_ENV: &str = "WXML_REQUIRE_REAL_ENGINE";
pub(crate) const TSGO_BIN_ENV: &str = "TSGO_BIN";
pub(crate) const TSGO_ASSET_ROOT_ENV: &str = "TSGO_ASSET_ROOT";
pub(crate) const TSGO_TYPINGS_ROOT_ENV: &str = "TSGO_TYPINGS_ROOT";
pub(crate) const TEST_TMPDIR_ENV: &str = "WXML_TEST_TMPDIR";
pub(crate) const DEFAULT_TMP_BASE: &str = "/tmp/opencode";
pub(crate) const PINNED_ENGINE_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";
pub(crate) const PINNED_ENGINE_SIZE: u64 = 24_522_912;

pub(crate) const PACKAGE_BIN: &str = "bin/wxml-lsp";
pub(crate) const ENGINE_BINARY: &str = "engine/lib/tsc";
pub(crate) const MANIFEST: &str = "manifest.json";
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub(crate) const EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const STDERR_LIMIT: usize = 256 * 1024;
/// Upper bound for one Content-Length frame accepted by the test client.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub(crate) const ENGINE_VERSION: &str = "7.1.0-beta";
const ENGINE_SOURCE_COMMIT: &str = "3e9236361fa97a9e6c685f4833ac457ae2d8c50a";
const ENGINE_DISTRIBUTION: &str = "@typescript/typescript-linux-x64";
const ENGINE_DISTRIBUTION_URL: &str = "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz";
const ENGINE_DISTRIBUTION_INTEGRITY: &str = "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==";
const ENGINE_DISTRIBUTION_SHA256: &str =
    "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855";
const ENGINE_DISTRIBUTION_SIZE: u64 = 10_175_135;
const ENGINE_DISTRIBUTION_FILE_COUNT: u64 = 122;
const ENGINE_DISTRIBUTION_UNPACKED_SIZE: u64 = 28_450_796;
const ENGINE_API_SCHEMA_VERSION: u64 = 9;
const ENGINE_AST_HEADER_BYTES: u64 = 64;

static ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One read-only projection of the gate environment.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProvisionEnv {
    pub(crate) require_real_engine: Option<OsString>,
    pub(crate) tsgo_bin: Option<OsString>,
    pub(crate) asset_root: Option<OsString>,
    pub(crate) typings_root: Option<OsString>,
    pub(crate) tmp_base: Option<OsString>,
}

impl ProvisionEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            require_real_engine: std::env::var_os(REQUIRE_ENV),
            tsgo_bin: std::env::var_os(TSGO_BIN_ENV),
            asset_root: std::env::var_os(TSGO_ASSET_ROOT_ENV),
            typings_root: std::env::var_os(TSGO_TYPINGS_ROOT_ENV),
            tmp_base: std::env::var_os(TEST_TMPDIR_ENV),
        }
    }

    pub(crate) fn required(&self) -> bool {
        flag(self.require_real_engine.as_ref())
    }
}

/// Returns true only for the existing `1`/`true` convention.
fn flag(value: Option<&OsString>) -> bool {
    value
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub(crate) fn non_empty(value: Option<&OsString>) -> Option<&OsString> {
    value.filter(|value| !value.is_empty())
}

/// Remediation shared by the skip and fatal provisioning messages.
///
/// Resolution order mirrors the production gate: the pinned engine comes from
/// `TSGO_BIN`; the standard library comes from `TSGO_ASSET_ROOT` (an extracted
/// package) or the engine's own directory; typings come from
/// `TSGO_TYPINGS_ROOT`, `TSGO_ASSET_ROOT/typings`, or the repository fixture.
pub(crate) fn provisioning_hint() -> String {
    format!(
        "set {TSGO_BIN_ENV} to the pinned 7.1.0-beta lib/tsc; \
         {TSGO_ASSET_ROOT_ENV} may point at an extracted package whose engine/lib or lib \
         contains lib.d.ts (otherwise the lib*.d.ts next to {TSGO_BIN_ENV} are used); \
         set {TSGO_TYPINGS_ROOT_ENV} to the directory containing miniprogram-api-typings \
         (also accepted: {TSGO_ASSET_ROOT_ENV}/typings); the default typings directory is \
         backends/typescript/node_modules, which is gitignored, so a clean checkout must \
         provide {TSGO_TYPINGS_ROOT_ENV} explicitly; {REQUIRE_ENV}=1 makes every missing \
         input fatal"
    )
}

/// Why a pinned engine fixture cannot be provisioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProvisionError {
    /// An input is absent. This is the only variant that optional mode skips.
    Missing { reason: String },
    /// A present input is not the pinned, safe artifact. Always fatal.
    Invalid { reason: String },
}

impl ProvisionError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Missing { reason } | Self::Invalid { reason } => reason,
        }
    }
}

/// Verified inputs copied into one disposable package.
#[derive(Debug, Clone)]
pub(crate) struct EngineInputs {
    pub(crate) engine_bin: PathBuf,
    pub(crate) asset_lib: PathBuf,
    pub(crate) typings_package: PathBuf,
    pub(crate) tmp_base: PathBuf,
}

/// Resolves and verifies every fixture input without writing a package.
///
/// `Missing` is skippable in optional mode; `Invalid` is always fatal.
pub(crate) fn resolve(env: &ProvisionEnv) -> Result<EngineInputs, ProvisionError> {
    let tmp_base = resolve_tmp_base(env)?;
    let Some(bin) = non_empty(env.tsgo_bin.as_ref()) else {
        return Err(ProvisionError::Missing {
            reason: format!("{TSGO_BIN_ENV} is not set"),
        });
    };
    let engine_bin = resolve_engine_bin(&PathBuf::from(bin))?;
    let asset_lib = resolve_asset_lib(env, &engine_bin)?;
    let typings_package = resolve_typings(env)?;
    Ok(EngineInputs {
        engine_bin,
        asset_lib,
        typings_package,
        tmp_base,
    })
}

/// Resolves the private temporary base.
///
/// The repository check happens before anything is created. A lexical path
/// inside the repository is rejected immediately; an existing or symlinked
/// base is rejected after canonicalization. A missing base is projected from
/// its nearest existing ancestor before creation, and if a race still makes
/// the created directory resolve inside the repository, the newly created
/// directory is removed before the error is returned.
pub(crate) fn resolve_tmp_base(env: &ProvisionEnv) -> Result<PathBuf, ProvisionError> {
    let base = non_empty(env.tmp_base.as_ref())
        .map_or_else(|| PathBuf::from(DEFAULT_TMP_BASE), PathBuf::from);
    if !base.is_absolute() {
        return Err(ProvisionError::Invalid {
            reason: format!("{TEST_TMPDIR_ENV} must be absolute: {}", base.display()),
        });
    }
    let repo = repo_root();
    if base.starts_with(&repo) {
        return Err(tmp_base_outside_repo(&base, &repo));
    }
    if base.exists() {
        let canonical = fs::canonicalize(&base).map_err(|error| ProvisionError::Invalid {
            reason: format!("cannot canonicalize test base {}: {error}", base.display()),
        })?;
        if canonical.starts_with(&repo) {
            return Err(tmp_base_outside_repo(&base, &repo));
        }
        return Ok(canonical);
    }
    if let Some(ancestor) = nearest_existing_ancestor(&base) {
        let canonical = fs::canonicalize(&ancestor).map_err(|error| ProvisionError::Invalid {
            reason: format!(
                "cannot canonicalize test base ancestor {}: {error}",
                ancestor.display()
            ),
        })?;
        let remainder = base
            .strip_prefix(&ancestor)
            .unwrap_or_else(|_| Path::new(""));
        if canonical.join(remainder).starts_with(&repo) {
            return Err(tmp_base_outside_repo(&base, &repo));
        }
    }
    fs::create_dir_all(&base).map_err(|error| ProvisionError::Invalid {
        reason: format!("cannot create test base {}: {error}", base.display()),
    })?;
    let canonical = fs::canonicalize(&base).map_err(|error| ProvisionError::Invalid {
        reason: format!("cannot canonicalize test base {}: {error}", base.display()),
    })?;
    if canonical.starts_with(&repo) {
        let _ = fs::remove_dir_all(&base);
        return Err(tmp_base_outside_repo(&base, &repo));
    }
    Ok(canonical)
}

fn tmp_base_outside_repo(base: &Path, repo: &Path) -> ProvisionError {
    ProvisionError::Invalid {
        reason: format!(
            "{TEST_TMPDIR_ENV} must be outside the repository ({}): {}",
            repo.display(),
            base.display()
        ),
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

/// Returns the canonical repository root from the crate manifest directory.
pub(crate) fn repo_root() -> PathBuf {
    let crate_dir =
        fs::canonicalize(env!("CARGO_MANIFEST_DIR")).expect("canonicalize CARGO_MANIFEST_DIR");
    let root = crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("crate manifest directory has a repository root");
    fs::canonicalize(root).expect("canonicalize repository root")
}

/// Validates the pinned engine size and digest.
fn resolve_engine_bin(path: &Path) -> Result<PathBuf, ProvisionError> {
    if !path.is_absolute() {
        return Err(ProvisionError::Invalid {
            reason: format!("{TSGO_BIN_ENV} must be absolute: {}", path.display()),
        });
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProvisionError::Missing {
                reason: format!(
                    "{TSGO_BIN_ENV} points at {} which does not exist",
                    path.display()
                ),
            });
        }
        Err(error) => {
            return Err(ProvisionError::Invalid {
                reason: format!(
                    "cannot inspect {TSGO_BIN_ENV} at {}: {error}",
                    path.display()
                ),
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ProvisionError::Invalid {
            reason: format!("{TSGO_BIN_ENV} must be a regular file: {}", path.display()),
        });
    }
    if metadata.len() != PINNED_ENGINE_SIZE {
        return Err(ProvisionError::Invalid {
            reason: format!(
                "{TSGO_BIN_ENV} is {} bytes, expected {PINNED_ENGINE_SIZE} bytes for the pinned 7.1.0-beta artifact",
                metadata.len()
            ),
        });
    }
    let digest = package::sha256_file(path).map_err(|reason| ProvisionError::Invalid { reason })?;
    if digest != PINNED_ENGINE_SHA256 {
        return Err(ProvisionError::Invalid {
            reason: format!("{TSGO_BIN_ENV} SHA-256 is {digest}, expected {PINNED_ENGINE_SHA256}"),
        });
    }
    fs::canonicalize(path).map_err(|error| ProvisionError::Invalid {
        reason: format!(
            "cannot canonicalize {TSGO_BIN_ENV} at {}: {error}",
            path.display()
        ),
    })
}

/// Finds the standard-library root next to the engine or in `TSGO_ASSET_ROOT`.
fn resolve_asset_lib(env: &ProvisionEnv, engine_bin: &Path) -> Result<PathBuf, ProvisionError> {
    if let Some(root) = non_empty(env.asset_root.as_ref()) {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err(ProvisionError::Invalid {
                reason: format!("{TSGO_ASSET_ROOT_ENV} must be absolute: {}", root.display()),
            });
        }
        return asset_candidates(&root)
            .into_iter()
            .find(|candidate| candidate.join("lib.d.ts").is_file())
            .and_then(|candidate| fs::canonicalize(candidate).ok())
            .ok_or_else(|| ProvisionError::Invalid {
                reason: format!(
                    "{TSGO_ASSET_ROOT_ENV} has no lib.d.ts under {}/engine/lib or {}/lib",
                    root.display(),
                    root.display()
                ),
            });
    }
    let Some(parent) = engine_bin.parent() else {
        return Err(ProvisionError::Invalid {
            reason: format!("{TSGO_BIN_ENV} has no parent directory"),
        });
    };
    if parent.join("lib.d.ts").is_file() {
        return fs::canonicalize(parent).map_err(|error| ProvisionError::Invalid {
            reason: format!("cannot canonicalize {}: {error}", parent.display()),
        });
    }
    Err(ProvisionError::Missing {
        reason: format!(
            "{TSGO_ASSET_ROOT_ENV} is not set and no lib.d.ts exists next to {TSGO_BIN_ENV}"
        ),
    })
}

/// Candidate standard-library roots in preference order.
fn asset_candidates(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join("engine/lib"),
        root.join("lib"),
        root.to_path_buf(),
    ]
}

/// Resolves the pinned `miniprogram-api-typings` package.
///
/// Explicit `TSGO_TYPINGS_ROOT` wins; an extracted `TSGO_ASSET_ROOT` may carry
/// `typings/miniprogram-api-typings`; the last fallback is the repository
/// fixture below `backends/typescript/node_modules` (gitignored).
fn resolve_typings(env: &ProvisionEnv) -> Result<PathBuf, ProvisionError> {
    if let Some(root) = non_empty(env.typings_root.as_ref()) {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err(ProvisionError::Invalid {
                reason: format!(
                    "{TSGO_TYPINGS_ROOT_ENV} must be absolute: {}",
                    root.display()
                ),
            });
        }
        let package = if root.join("miniprogram-api-typings/index.d.ts").is_file() {
            root.join("miniprogram-api-typings")
        } else if root.join("index.d.ts").is_file() {
            root
        } else {
            return Err(ProvisionError::Invalid {
                reason: format!(
                    "{TSGO_TYPINGS_ROOT_ENV} has no miniprogram-api-typings/index.d.ts: {}",
                    root.display()
                ),
            });
        };
        return require_typings_package(package, true);
    }
    if let Some(root) = non_empty(env.asset_root.as_ref()) {
        let root = PathBuf::from(root);
        for candidate in [
            root.join("typings/miniprogram-api-typings"),
            root.join("typings"),
        ] {
            if candidate.join("index.d.ts").is_file()
                && candidate.join("types/index.d.ts").is_file()
            {
                return require_typings_package(candidate, false);
            }
        }
    }
    let package = repo_root().join("backends/typescript/node_modules/miniprogram-api-typings");
    require_typings_package(package, false)
}

/// Requires both declarations the pinned typings loader needs.
fn require_typings_package(package: PathBuf, explicit: bool) -> Result<PathBuf, ProvisionError> {
    if package.join("index.d.ts").is_file() && package.join("types/index.d.ts").is_file() {
        return fs::canonicalize(&package).map_err(|error| ProvisionError::Invalid {
            reason: format!("cannot canonicalize typings {}: {error}", package.display()),
        });
    }
    let reason = format!(
        "the pinned miniprogram-api-typings package is unavailable: {}",
        package.display()
    );
    if explicit {
        Err(ProvisionError::Invalid { reason })
    } else {
        Err(ProvisionError::Missing { reason })
    }
}

/// A disposable private directory below the test temp base.
#[derive(Debug)]
pub(crate) struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub(crate) fn new(base: &Path, label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = base.join(format!(
            "wxml-native-ds-{label}-{}-{nanos}-{}",
            std::process::id(),
            ROOT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)
            .unwrap_or_else(|error| panic!("create test root {}: {error}", path.display()));
        set_mode(&path, 0o700).expect("private test root");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// PATH tripwire that records any `node`/`npm`/`npx` execution.
#[derive(Debug)]
pub(crate) struct Tripwire {
    dir: PathBuf,
    marker: PathBuf,
}

impl Tripwire {
    pub(crate) fn install(dir: &Path) -> Self {
        fs::create_dir_all(dir).expect("tripwire directory");
        set_mode(dir, 0o700).expect("tripwire mode");
        let marker = dir.join("marker");
        let script = "#!/bin/sh\nprintf '%s\\n' \"$0\" >> \"$WXML_TRIPWIRE_MARKER\"\nexit 99\n";
        for name in ["node", "nodejs", "npm", "npx"] {
            let path = dir.join(name);
            fs::write(&path, script).expect("tripwire shim");
            set_mode(&path, 0o755).expect("tripwire shim mode");
        }
        Self {
            dir: dir.to_path_buf(),
            marker,
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn marker(&self) -> &Path {
        &self.marker
    }

    pub(crate) fn assert_absent(&self) {
        assert!(
            !self.marker.exists(),
            "the native server executed a Node shim through PATH; marker: {}",
            self.marker.display()
        );
    }
}
