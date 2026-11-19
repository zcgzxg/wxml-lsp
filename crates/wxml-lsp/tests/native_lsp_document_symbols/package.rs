//! Disposable package, project and fail-closed package builders.
//!
//! The generated `manifest.json` is a **discovery-only** `schemaVersion: 2`
//! projection for `backend_tsgo::engine::discover_from_current_exe`. It is not
//! a release package: `typeAssets` and `binaries` are empty, `files` lists only
//! the engine executable and the test binary, and `engine.licenseFiles` name
//! files that are not copied. The pinned standard-library and typings trees
//! copied next to it are intentionally not enumerated. F3-03 manifest/asset
//! cross-checking is not wired, so this fixture must not be presented as a
//! complete v2 delivery package.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::missing_const_for_fn,
    clippy::redundant_pub_crate,
    missing_docs
)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_BINARY, ENGINE_DISTRIBUTION,
    ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_DISTRIBUTION_SHA256,
    ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE, ENGINE_DISTRIBUTION_URL,
    ENGINE_SOURCE_COMMIT, ENGINE_VERSION, EngineInputs, MANIFEST, PACKAGE_BIN,
    PINNED_ENGINE_SHA256, PINNED_ENGINE_SIZE, ProvisionEnv, ProvisionError, REQUIRE_ENV, TestRoot,
    Tripwire, provisioning_hint, resolve, resolve_tmp_base, set_mode,
};

pub(crate) const PROJECT_APP_JSON: &str = "{\"pages\":[\"pages/index/index\"]}\n";
pub(crate) const PROJECT_TSCONFIG: &str = "{\"compilerOptions\":{\"allowJs\":true}}\n";
pub(crate) const PROJECT_WXML: &str = concat!(
    "<view class=\"root\">{{ user.name }}</view>\n",
    "<button bindtap=\"onTap\">tap</button>\n",
);
// `allowJs` is required: without it the native program does not contain the
// JavaScript file and `getSourceFile` cannot return an AST. The explicit
// `data`/`onTap` property assignments keep the production shadow supported.
pub(crate) const PROJECT_JS: &str = concat!(
    "const user = { name: 'Ada' };\n",
    "function onTap() {}\n",
    "class PageHelpers {\n",
    "  formatName() { return user.name; }\n",
    "}\n",
    "Page({ data: { user: user }, onTap: onTap });\n",
);

/// One provisioned native package, project and PATH tripwire.
#[derive(Debug)]
pub(crate) struct Fixture {
    _root: TestRoot,
    package: PathBuf,
    project: PathBuf,
    private_tmp: PathBuf,
    tripwire: Tripwire,
}

impl Fixture {
    pub(crate) fn bin(&self) -> PathBuf {
        self.package.join(PACKAGE_BIN)
    }

    pub(crate) fn project(&self) -> &Path {
        &self.project
    }

    pub(crate) fn private_tmp(&self) -> &Path {
        &self.private_tmp
    }

    pub(crate) const fn tripwire(&self) -> &Tripwire {
        &self.tripwire
    }

    pub(crate) fn project_uri(&self) -> String {
        format!("file://{}", self.project.display())
    }

    pub(crate) fn uri(&self, relative: &str) -> String {
        format!("file://{}", self.project.join(relative).display())
    }
}

/// Provisions the gate from the process environment.
pub(crate) fn provision(label: &str) -> Option<Fixture> {
    provision_with_env(&ProvisionEnv::from_process(), label)
}

/// Provisions the gate from an explicit environment projection.
///
/// Only [`ProvisionError::Missing`] is skippable in optional mode. A present
/// but invalid input always panics, and required mode panics on every error.
pub(crate) fn provision_with_env(env: &ProvisionEnv, label: &str) -> Option<Fixture> {
    match resolve(env) {
        Ok(inputs) => Some(build_fixture(&inputs, label)),
        Err(error @ ProvisionError::Missing { .. }) if !env.required() => {
            eprintln!("SKIP(native document symbols {label}): {}", error.message());
            eprintln!(
                "  explicit optional-mode skip (only missing inputs are skippable): {}",
                provisioning_hint()
            );
            None
        }
        Err(error) => panic!(
            "native document symbol fixture is unavailable: {}\n{}\n\
             A present-but-invalid input is always fatal; {REQUIRE_ENV}=1 makes every \
             missing input fatal. Pinned engine SHA-256: {PINNED_ENGINE_SHA256}.",
            error.message(),
            provisioning_hint()
        ),
    }
}

/// Builds the disposable package, project and private TMPDIR.
fn build_fixture(inputs: &EngineInputs, label: &str) -> Fixture {
    let root = TestRoot::new(&inputs.tmp_base, label);
    let package = root.path().join("package");
    let project = root.path().join("project");
    let private_tmp = root.path().join("tmp");
    create_package(&package, inputs);
    write_project(&project);
    fs::create_dir(&private_tmp).expect("create private TMPDIR");
    set_mode(&private_tmp, 0o700).expect("private TMPDIR mode");
    let tripwire = Tripwire::install(&root.path().join("tripwire"));
    Fixture {
        _root: root,
        package,
        project,
        private_tmp,
        tripwire,
    }
}

/// Creates the package directories and copies every verified input.
///
/// The production startup path verifies the pinned standard library and
/// `miniprogram-api-typings` before any session exists, so the fixture copies
/// both roots instead of leaving `typings/` empty. The running server still
/// performs its own pin checks; the fixture never bypasses them.
fn create_package(package: &Path, inputs: &EngineInputs) {
    for directory in [
        package.to_path_buf(),
        package.join("bin"),
        package.join("engine"),
        package.join("engine/lib"),
        package.join("typings"),
    ] {
        fs::create_dir_all(&directory)
            .unwrap_or_else(|error| panic!("create {}: {error}", directory.display()));
        set_mode(&directory, 0o755).expect("package directory mode");
    }
    let test_binary = Path::new(env!("CARGO_BIN_EXE_wxml-lsp"));
    copy_executable(test_binary, &package.join(PACKAGE_BIN));
    copy_executable(&inputs.engine_bin, &package.join(ENGINE_BINARY));
    copy_standard_library(&inputs.asset_lib, &package.join("engine/lib"));
    copy_tree(
        &inputs.typings_package,
        &package.join("typings/miniprogram-api-typings"),
    );
    write_discovery_manifest(package, PINNED_ENGINE_SHA256);
}

/// Copies one executable and forces mode 0755.
fn copy_executable(source: &Path, destination: &Path) {
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    set_mode(destination, 0o755).expect("executable mode");
}

/// Copies every `lib*.d.ts` from the standard-library root.
fn copy_standard_library(source: &Path, destination: &Path) {
    let entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("read standard library {}: {error}", source.display()));
    let mut copied = 0_usize;
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("standard library entry: {error}"));
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("lib") || !name.ends_with(".d.ts") {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).unwrap_or_else(|error| {
            panic!("standard library metadata {}: {error}", path.display())
        });
        assert!(
            !metadata.file_type().is_symlink() && metadata.is_file(),
            "standard library entry is not a regular file: {}",
            path.display()
        );
        let target = destination.join(entry.file_name());
        fs::copy(&path, &target).unwrap_or_else(|error| {
            panic!("copy {} to {}: {error}", path.display(), target.display());
        });
        set_mode(&target, 0o644).expect("standard library mode");
        copied += 1;
    }
    assert!(
        copied > 0,
        "{} contains no lib*.d.ts files; the pinned startup path would reject it",
        source.display()
    );
}

/// Recursively copies a typings tree with safe, deterministic modes.
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("typings directory");
    set_mode(destination, 0o755).expect("typings directory mode");
    let entries =
        fs::read_dir(source).unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("typings entry: {error}"));
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("typings file type: {error}"));
        let target = destination.join(entry.file_name());
        assert!(
            !file_type.is_symlink(),
            "typings entries must not be symlinks: {}",
            entry.path().display()
        );
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).unwrap_or_else(|error| {
                panic!("copy typings {}: {error}", entry.path().display());
            });
            set_mode(&target, 0o644).expect("typings file mode");
        }
    }
}

/// Writes a **discovery-only** schema-v2 manifest with the actual test binary
/// digest.
///
/// This is not a release package (see the module header): `typeAssets` and
/// `binaries` stay empty, `files` contains only the engine and test binary, and
/// the copied license/standard-library/typings trees are not listed. The
/// running server's `discover_from_current_exe` projection only needs this
/// subset; F3-03 cross-checking is not applicable until the delivery gate
/// lands.
fn write_discovery_manifest(package: &Path, engine_sha256: &str) {
    let binary = package.join(PACKAGE_BIN);
    let binary_size = fs::metadata(&binary)
        .unwrap_or_else(|error| panic!("binary metadata: {error}"))
        .len();
    let binary_sha = sha256_file(&binary).unwrap_or_else(|reason| panic!("{reason}"));
    let manifest = json!({
        "schemaVersion": 2,
        "package": "wxml-lsp",
        "packageVersion": env!("CARGO_PKG_VERSION"),
        "platform": "linux-x86_64",
        "protocolVersion": 2,
        "builtAt": "2026-11-18T00:00:00Z",
        "engine": {
            "name": "typescript",
            "version": ENGINE_VERSION,
            "sourceCommit": ENGINE_SOURCE_COMMIT,
            "distribution": ENGINE_DISTRIBUTION,
            "distributionUrl": ENGINE_DISTRIBUTION_URL,
            "distributionIntegrity": ENGINE_DISTRIBUTION_INTEGRITY,
            "distributionSha256": ENGINE_DISTRIBUTION_SHA256,
            "distributionSize": ENGINE_DISTRIBUTION_SIZE,
            "distributionFileCount": ENGINE_DISTRIBUTION_FILE_COUNT,
            "distributionUnpackedSize": ENGINE_DISTRIBUTION_UNPACKED_SIZE,
            "apiSchemaVersion": ENGINE_API_SCHEMA_VERSION,
            "astHeaderBytes": ENGINE_AST_HEADER_BYTES,
            "license": "Apache-2.0",
            "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
            "binary": ENGINE_BINARY,
            "probe": {"args": ["--version"], "expectPrefix": "Version "}
        },
        "typeAssets": [],
        "binaries": [],
        "files": [
            {
                "path": ENGINE_BINARY,
                "sha256": engine_sha256,
                "size": PINNED_ENGINE_SIZE,
                "mode": "0755"
            },
            {
                "path": PACKAGE_BIN,
                "sha256": binary_sha,
                "size": binary_size,
                "mode": "0755"
            }
        ]
    });
    let path = package.join(MANIFEST);
    fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    set_mode(&path, 0o644).expect("manifest mode");
}

/// Writes the mini-program project used by the end-to-end test.
fn write_project(project: &Path) {
    fs::create_dir_all(project.join("pages/index"))
        .unwrap_or_else(|error| panic!("create project: {error}"));
    set_mode(project, 0o755).expect("project mode");
    for (relative, text) in [
        ("app.json", PROJECT_APP_JSON),
        ("tsconfig.json", PROJECT_TSCONFIG),
        ("pages/index/index.wxml", PROJECT_WXML),
        ("pages/index/index.js", PROJECT_JS),
    ] {
        let path = project.join(relative);
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
}

/// One deliberately broken package used by the fail-closed tests.
#[derive(Debug)]
pub(crate) struct BrokenPackage {
    _root: TestRoot,
    package: PathBuf,
    project: PathBuf,
    private_tmp: PathBuf,
    tripwire: Tripwire,
}

impl BrokenPackage {
    pub(crate) fn package(&self) -> &Path {
        &self.package
    }

    pub(crate) fn project(&self) -> &Path {
        &self.project
    }

    pub(crate) fn private_tmp(&self) -> &Path {
        &self.private_tmp
    }

    pub(crate) const fn tripwire(&self) -> &Tripwire {
        &self.tripwire
    }
}

/// Which discovery invariant the broken package violates.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PackageDefect {
    MissingEngine,
    TamperedManifest,
}

/// Builds a package that must fail discovery before `initialize`.
pub(crate) fn broken_package(label: &str, defect: PackageDefect) -> BrokenPackage {
    let env = ProvisionEnv::from_process();
    let base = resolve_tmp_base(&env).unwrap_or_else(|error| panic!("{}", error.message()));
    let root = TestRoot::new(&base, label);
    let package = root.path().join("package");
    for directory in [
        package.clone(),
        package.join("bin"),
        package.join("engine"),
        package.join("engine/lib"),
        package.join("typings"),
    ] {
        fs::create_dir_all(&directory)
            .unwrap_or_else(|error| panic!("create {}: {error}", directory.display()));
        set_mode(&directory, 0o755).expect("package directory mode");
    }
    copy_executable(
        Path::new(env!("CARGO_BIN_EXE_wxml-lsp")),
        &package.join(PACKAGE_BIN),
    );
    let engine_sha = match defect {
        PackageDefect::MissingEngine => PINNED_ENGINE_SHA256,
        PackageDefect::TamperedManifest => {
            "0000000000000000000000000000000000000000000000000000000000000000"
        }
    };
    write_discovery_manifest(&package, engine_sha);
    let project = root.path().join("project");
    fs::create_dir(&project).expect("broken project");
    set_mode(&project, 0o755).expect("broken project mode");
    let private_tmp = root.path().join("tmp");
    fs::create_dir(&private_tmp).expect("broken private TMPDIR");
    set_mode(&private_tmp, 0o700).expect("broken private TMPDIR mode");
    let tripwire = Tripwire::install(&root.path().join("tripwire"));
    BrokenPackage {
        _root: root,
        package,
        project,
        private_tmp,
        tripwire,
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("hex encoding");
    }
    Ok(encoded)
}
