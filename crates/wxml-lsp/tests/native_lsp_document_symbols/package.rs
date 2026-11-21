//! Disposable package, project and fail-closed package builders.
//!
//! The generated package contains only `<package>/bin/wxml-lsp` and
//! `<package>/typings/miniprogram-api-typings`; discovery and startup are
//! typings-only because the type engine runs in-process and embeds the
//! TypeScript standard library. No `engine/` tree or manifest is produced or
//! read.

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

use super::{
    PACKAGE_BIN, ProvisionEnv, ProvisionError, REQUIRE_ENV, TestRoot, Tripwire, TypingsInputs,
    provisioning_hint, resolve, resolve_tmp_base, set_mode,
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
// A second project script that is not an owning page script. Its deliberate
// type error (TS2322) is the native script-diagnostics signal; `index.js` stays
// clean so the document-symbol gate is unaffected.
pub(crate) const PROJECT_TS: &str = "const count: number = 'not a number';\n";

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
             missing input fatal.",
            error.message(),
            provisioning_hint()
        ),
    }
}

/// Builds the disposable package, project and private TMPDIR.
fn build_fixture(inputs: &TypingsInputs, label: &str) -> Fixture {
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

/// Creates the package directories and copies the verified typings.
///
/// The production startup path verifies `miniprogram-api-typings` before any
/// session exists, so the fixture copies the package instead of leaving
/// `typings/` empty. The running server still performs its own pin checks; the
/// fixture never bypasses them.
fn create_package(package: &Path, inputs: &TypingsInputs) {
    for directory in [
        package.to_path_buf(),
        package.join("bin"),
        package.join("typings"),
    ] {
        fs::create_dir_all(&directory)
            .unwrap_or_else(|error| panic!("create {}: {error}", directory.display()));
        set_mode(&directory, 0o755).expect("package directory mode");
    }
    let test_binary = Path::new(env!("CARGO_BIN_EXE_wxml-lsp"));
    copy_executable(test_binary, &package.join(PACKAGE_BIN));
    copy_tree(
        &inputs.typings_package,
        &package.join("typings/miniprogram-api-typings"),
    );
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
        ("pages/index/typed.ts", PROJECT_TS),
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
    MissingTypings,
    IncompleteTypings,
}

/// Builds a package that must fail discovery or startup before `initialize`.
pub(crate) fn broken_package(label: &str, defect: PackageDefect) -> BrokenPackage {
    let env = ProvisionEnv::from_process();
    let base = resolve_tmp_base(&env).unwrap_or_else(|error| panic!("{}", error.message()));
    let root = TestRoot::new(&base, label);
    let package = root.path().join("package");
    for directory in [
        package.clone(),
        package.join("bin"),
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
    match defect {
        PackageDefect::MissingTypings => {
            fs::remove_dir_all(package.join("typings")).expect("remove typings");
        }
        PackageDefect::IncompleteTypings => {
            fs::create_dir_all(package.join("typings/miniprogram-api-typings"))
                .expect("incomplete typings");
        }
    }
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
