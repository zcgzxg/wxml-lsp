//! Test-only fixture and JSON-RPC client for the native `documentSymbol` gate.
//!
//! The fixture builds a disposable typings-only package below
//! `WXML_TEST_TMPDIR` (default `/tmp/opencode`) from the pinned typings that the
//! production startup path requires. The type engine runs in-process, so there
//! is no engine executable, engine tarball or `engine/lib` standard library. It
//! never writes inside the repository and never selects the Node backend.
//!
//! Gate semantics:
//!
//! - `TSGO_TYPINGS_ROOT` is optional. Only a *missing* input
//!   ([`ProvisionError::Missing`]) becomes an explicit `SKIP` print plus
//!   `None` in optional mode; there is no silent pass.
//! - A present but invalid input is always fatal, even in optional mode:
//!   a relative path, an unusable `TSGO_TYPINGS_ROOT`, a package without the
//!   required declarations, or a temporary base inside the repository.
//! - `WXML_REQUIRE_REAL_ENGINE=1|true` turns every missing input into a panic.
//!   The variable name is legacy; the engine itself is in-process.
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
pub(crate) const TSGO_TYPINGS_ROOT_ENV: &str = "TSGO_TYPINGS_ROOT";
pub(crate) const TEST_TMPDIR_ENV: &str = "WXML_TEST_TMPDIR";
pub(crate) const DEFAULT_TMP_BASE: &str = "/tmp/opencode";

pub(crate) const PACKAGE_BIN: &str = "bin/wxml-lsp";
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
pub(crate) const EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const STDERR_LIMIT: usize = 256 * 1024;
/// Upper bound for one Content-Length frame accepted by the test client.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// One read-only projection of the gate environment.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProvisionEnv {
    pub(crate) require_real_engine: Option<OsString>,
    pub(crate) typings_root: Option<OsString>,
    pub(crate) tmp_base: Option<OsString>,
}

impl ProvisionEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            require_real_engine: std::env::var_os(REQUIRE_ENV),
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
/// The in-process engine needs only the typings package. There is no committed
/// fallback: an unset typings input is a missing input and skips in optional
/// mode.
pub(crate) fn provisioning_hint() -> String {
    format!(
        "set {TSGO_TYPINGS_ROOT_ENV} to the directory containing miniprogram-api-typings \
         (the package directory itself is also accepted); the TypeScript standard library \
         is embedded in the in-process engine; {REQUIRE_ENV}=1 makes every missing input fatal"
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
pub(crate) struct TypingsInputs {
    pub(crate) typings_package: PathBuf,
    pub(crate) tmp_base: PathBuf,
}

/// Resolves and verifies every fixture input without writing a package.
///
/// `Missing` is skippable in optional mode; `Invalid` is always fatal.
pub(crate) fn resolve(env: &ProvisionEnv) -> Result<TypingsInputs, ProvisionError> {
    let tmp_base = resolve_tmp_base(env)?;
    let typings_package = resolve_typings(env)?;
    Ok(TypingsInputs {
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

/// Resolves the pinned `miniprogram-api-typings` package.
///
/// Only an explicit `TSGO_TYPINGS_ROOT` is accepted. When it is unset the input
/// is absent and optional mode skips it.
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
        return require_typings_package(package);
    }
    let reason = format!(
        "the pinned miniprogram-api-typings package is unavailable: set \
         {TSGO_TYPINGS_ROOT_ENV} to the directory containing miniprogram-api-typings"
    );
    Err(ProvisionError::Missing { reason })
}

/// Requires both declarations the pinned typings loader needs.
fn require_typings_package(package: PathBuf) -> Result<PathBuf, ProvisionError> {
    if package.join("index.d.ts").is_file() && package.join("types/index.d.ts").is_file() {
        return fs::canonicalize(&package).map_err(|error| ProvisionError::Invalid {
            reason: format!("cannot canonicalize typings {}: {error}", package.display()),
        });
    }
    let reason = format!(
        "the pinned miniprogram-api-typings package is unavailable: {}",
        package.display()
    );
    Err(ProvisionError::Invalid { reason })
}

/// A disposable private directory below the test temp base.
#[derive(Debug)]
pub(crate) struct TestRoot {
    dir: tempfile::TempDir,
}

impl TestRoot {
    pub(crate) fn new(base: &Path, label: &str) -> Self {
        let prefix = format!("wxml-native-ds-{label}-");
        let dir = tempfile::Builder::new()
            .prefix(prefix.as_str())
            .tempdir_in(base)
            .unwrap_or_else(|error| panic!("create test root in {}: {error}", base.display()));
        set_mode(dir.path(), 0o700).expect("private test root");
        Self { dir }
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
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
