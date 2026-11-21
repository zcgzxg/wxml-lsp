//! Typings-only discovery from the installed package tree.
//!
//! Discovery starts at [`std::env::current_exe`], resolves the real path and
//! requires the native-only layout `<root>/bin/wxml-lsp` and `<root>/typings`.
//! The package root plus the `bin` and `typings` directories must be real
//! directories with safe modes. The type engine runs in-process and embeds the
//! TypeScript standard library, so there is no `engine/` directory, no
//! executable file, no file mode check, no SHA-256 check and no version probe.
//! An old `engine/` tree left by a previous install is ignored.
//!
//! Discovery never reads `PATH`, the process working directory, backend
//! environment variables or a package outside the running executable's own
//! tree, and it never falls back to another backend.
//!
//! Type assets are not read here; the adapter loads and verifies
//! `miniprogram-api-typings` below the returned `typings` root before it starts
//! a session.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// A package tree that passed typings-only discovery.
///
/// This is **not** full install verification. Discovery checks the native-only
/// layout and the `typings` directory. The runtime independently verifies the
/// typings bytes against the crate pin in
/// [`TsrsBackendAdapter::with_install`](super::TsrsBackendAdapter::with_install).
/// `typings_root` is not read during discovery.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredInstall {
    /// Canonical package root (`<root>`).
    pub(crate) package_root: PathBuf,
    /// `<root>/typings`; verified against the built-in pin by the adapter.
    pub(crate) typings_root: PathBuf,
}

impl DiscoveredInstall {
    /// Returns the canonical package root.
    pub(crate) fn package_root(&self) -> &Path {
        &self.package_root
    }

    /// Returns the directory that must contain `miniprogram-api-typings`.
    pub(crate) fn typings_root(&self) -> &Path {
        &self.typings_root
    }
}

/// Why the running executable's package tree cannot supply the type assets.
///
/// Messages deliberately contain no absolute paths or engine output; callers
/// can present them to users.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EngineDiscoveryError {
    /// The running executable could not be canonicalized.
    #[error("the running executable cannot be resolved to a real path")]
    UnresolvableExecutable,
    /// The executable is not laid out as `<root>/bin/wxml-lsp`.
    #[error("the running executable is not installed as <root>/bin/wxml-lsp")]
    NotInstalled,
    /// A required package directory is missing, a symlink or unsafe.
    #[error("the installed package layout is unsafe: {reason}")]
    PackageUnsafe {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// The installed `typings` directory is absent.
    #[error("the installed typings directory is missing")]
    TypingsMissing,
}

/// Discovers the typings layout from the running executable.
///
/// # Errors
///
/// Returns a typed discovery error and no fallback backend when the executable
/// is not installed, the layout is unsafe, or the `typings` directory is
/// missing.
pub(crate) fn discover_from_current_exe() -> Result<DiscoveredInstall, EngineDiscoveryError> {
    let executable =
        std::env::current_exe().map_err(|_| EngineDiscoveryError::UnresolvableExecutable)?;
    discover(&executable)
}

/// Test and benchmark discovery at an explicit executable path.
///
/// Production startup always uses [`discover_from_current_exe`]; this seam
/// exists so `#[cfg(test)]` fixtures and the `bench-support` benchmarks can
/// point discovery at an installed package without replacing the process
/// executable.
#[cfg(any(test, feature = "bench-support"))]
pub(crate) fn discover_at(executable: &Path) -> Result<DiscoveredInstall, EngineDiscoveryError> {
    discover(executable)
}

fn discover(executable: &Path) -> Result<DiscoveredInstall, EngineDiscoveryError> {
    let executable =
        canonicalize(executable).map_err(|_| EngineDiscoveryError::UnresolvableExecutable)?;

    if executable.file_name().and_then(|name| name.to_str()) != Some("wxml-lsp") {
        return Err(EngineDiscoveryError::NotInstalled);
    }
    let bin = executable
        .parent()
        .ok_or(EngineDiscoveryError::NotInstalled)?;
    if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return Err(EngineDiscoveryError::NotInstalled);
    }
    let package_root = bin.parent().ok_or(EngineDiscoveryError::NotInstalled)?;
    if !package_root.is_absolute() || package_root == Path::new("/") {
        return Err(EngineDiscoveryError::PackageUnsafe {
            reason: "package root must be an absolute directory below /",
        });
    }

    validate_package_directory(package_root)?;
    validate_package_directory(&package_root.join("bin"))?;
    let typings_root = package_root.join("typings");
    match fs::symlink_metadata(&typings_root) {
        Ok(_) => validate_package_directory(&typings_root)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(EngineDiscoveryError::TypingsMissing);
        }
        Err(_) => {
            return Err(EngineDiscoveryError::PackageUnsafe {
                reason: "typings directory is unreadable",
            });
        }
    }

    Ok(DiscoveredInstall {
        package_root: package_root.to_path_buf(),
        typings_root,
    })
}

fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path)
}

fn validate_package_directory(path: &Path) -> Result<(), EngineDiscoveryError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        let reason = if error.kind() == io::ErrorKind::NotFound {
            "package directory is missing"
        } else {
            "package directory is unreadable"
        };
        EngineDiscoveryError::PackageUnsafe { reason }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(EngineDiscoveryError::PackageUnsafe {
            reason: "package directories must not be symlinks",
        });
    }
    if !metadata.is_dir() {
        return Err(EngineDiscoveryError::PackageUnsafe {
            reason: "package path must be a directory",
        });
    }
    let mode = file_mode(&metadata);
    if mode & 0o022 != 0 {
        return Err(EngineDiscoveryError::PackageUnsafe {
            reason: "package directory must not be group or world writable",
        });
    }
    if mode & 0o7000 != 0 {
        return Err(EngineDiscoveryError::PackageUnsafe {
            reason: "package directory must not carry setuid, setgid or sticky bits",
        });
    }
    Ok(())
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use super::*;

    fn temp_base() -> PathBuf {
        std::env::var_os("WXML_TEST_TMPDIR")
            .or_else(|| std::env::var_os("TMPDIR"))
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from)
    }

    fn test_root(label: &str) -> tempfile::TempDir {
        let base = temp_base();
        assert!(base.is_absolute(), "test temp base must be absolute");
        fs::create_dir_all(&base).expect("test base");
        let prefix = format!("wxml-lsp-engine-{label}-");
        let dir = tempfile::Builder::new()
            .prefix(prefix.as_str())
            .tempdir_in(&base)
            .expect("test root");
        set_mode(dir.path(), 0o700);
        dir
    }

    struct FakeInstall {
        root: tempfile::TempDir,
        package: PathBuf,
        executable: PathBuf,
    }

    impl FakeInstall {
        fn new() -> Self {
            let root = test_root("typings");
            let package = root.path().join("package");
            for directory in [
                package.clone(),
                package.join("bin"),
                package.join("typings"),
                package.join("typings/miniprogram-api-typings"),
            ] {
                fs::create_dir_all(&directory).expect("package directory");
                set_mode(&directory, 0o755);
            }

            let executable = package.join("bin/wxml-lsp");
            fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("executable");
            set_mode(&executable, 0o755);

            Self {
                root,
                package,
                executable,
            }
        }

        fn discover(&self) -> Result<DiscoveredInstall, EngineDiscoveryError> {
            discover_at(&self.executable)
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("permissions");
    }

    #[test]
    fn discovers_fake_package_from_bin_executable() {
        let fake = FakeInstall::new();
        let install = fake.discover().expect("discovered install");

        assert_eq!(
            install.package_root(),
            fs::canonicalize(&fake.package).unwrap()
        );
        assert_eq!(install.typings_root(), fake.package.join("typings"));
    }

    #[test]
    fn discovery_ignores_a_legacy_engine_tree() {
        let fake = FakeInstall::new();
        fs::create_dir_all(fake.package.join("engine/lib")).unwrap();
        fs::write(fake.package.join("engine/lib/tsc"), b"stale engine\n").unwrap();
        // An unsafe legacy tree must not affect discovery: it is never read.
        let outside = fake.root.path().join("outside-lib");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, fake.package.join("engine/other")).unwrap();

        let install = fake
            .discover()
            .expect("the legacy engine layout must be ignored");
        assert_eq!(install.typings_root(), fake.package.join("typings"));
    }

    #[test]
    fn discovery_checks_the_typings_layout_root_but_not_asset_bytes() {
        let fake = FakeInstall::new();
        fs::remove_dir_all(fake.package.join("typings/miniprogram-api-typings")).unwrap();
        fs::write(
            fake.package.join("typings/unpinned.txt"),
            b"not a pinned asset\n",
        )
        .unwrap();

        let install = fake
            .discover()
            .expect("discovery checks the layout root only and must not claim assets");
        assert_eq!(install.typings_root(), fake.package.join("typings"));
        assert!(install.typings_root().is_dir());
    }

    #[test]
    fn with_install_rejects_a_package_without_pinned_assets() {
        let fake = FakeInstall::new();
        let install = fake.discover().expect("discovered install");
        let host: Arc<dyn crate::project_host::ProjectHost> =
            Arc::new(crate::project_host::ProjectState::new());
        let error = super::super::TsrsBackendAdapter::with_install(host, &install)
            .expect_err("a package without pinned assets must fail before a session");
        assert!(matches!(
            error,
            super::super::NativeStartupError::Environment(_)
        ));
        let message = error.to_string();
        assert!(!message.contains("server.js"), "{message}");
        assert!(!message.to_ascii_lowercase().contains("node"), "{message}");
    }

    #[test]
    fn with_install_rejects_a_missing_typings_root() {
        let fake = FakeInstall::new();
        let install = fake.discover().expect("discovered install");
        fs::remove_dir_all(fake.package.join("typings")).unwrap();
        let host: Arc<dyn crate::project_host::ProjectHost> =
            Arc::new(crate::project_host::ProjectState::new());
        let error = super::super::TsrsBackendAdapter::with_install(host, &install)
            .expect_err("missing typings must fail closed");
        assert!(matches!(
            error,
            super::super::NativeStartupError::Environment(_)
        ));
    }

    #[test]
    fn discovery_reports_the_missing_typings_directory() {
        let fake = FakeInstall::new();
        fs::remove_dir_all(fake.package.join("typings")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::TypingsMissing)
        ));
    }

    #[test]
    fn discovery_does_not_search_parent_directories_for_layout_roots() {
        let fake = FakeInstall::new();
        fs::remove_dir_all(fake.package.join("typings")).unwrap();
        // A typings directory above the package root must never be used.
        fs::create_dir_all(fake.root.path().join("typings")).unwrap();
        set_mode(&fake.root.path().join("typings"), 0o755);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::TypingsMissing)
        ));
    }

    #[test]
    fn discovery_requires_the_bin_and_typings_layout_roots() {
        let fake = FakeInstall::new();
        fs::remove_dir_all(fake.package.join("typings")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::TypingsMissing)
        ));

        let fake = FakeInstall::new();
        fs::remove_dir_all(fake.package.join("bin")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::UnresolvableExecutable)
        ));
    }

    #[test]
    fn rejects_a_symlinked_bin_executable() {
        let fake = FakeInstall::new();
        let outside = fake.root.path().join("outside-wxml-lsp");
        fs::copy(&fake.executable, &outside).unwrap();
        set_mode(&outside, 0o755);
        fs::remove_file(&fake.executable).unwrap();
        std::os::unix::fs::symlink(&outside, &fake.executable).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::NotInstalled)
        ));
    }

    #[test]
    fn rejects_executable_outside_bin() {
        let fake = FakeInstall::new();
        let loose = fake.root.path().join("wxml-lsp");
        fs::copy(&fake.executable, &loose).unwrap();
        set_mode(&loose, 0o755);
        assert!(matches!(
            discover_at(&loose),
            Err(EngineDiscoveryError::NotInstalled)
        ));

        let other = fake.root.path().join("other/wxml-lsp");
        fs::create_dir_all(fake.root.path().join("other")).unwrap();
        fs::copy(&fake.executable, &other).unwrap();
        set_mode(&other, 0o755);
        assert!(matches!(
            discover_at(&other),
            Err(EngineDiscoveryError::NotInstalled)
        ));
    }

    #[test]
    fn rejects_missing_executable() {
        let fake = FakeInstall::new();
        let missing = fake.root.path().join("missing/bin/wxml-lsp");
        assert!(matches!(
            discover_at(&missing),
            Err(EngineDiscoveryError::UnresolvableExecutable)
        ));
    }

    #[test]
    fn current_test_binary_is_not_an_install() {
        assert!(matches!(
            discover_from_current_exe(),
            Err(EngineDiscoveryError::NotInstalled | EngineDiscoveryError::UnresolvableExecutable)
        ));
    }

    #[test]
    fn rejects_group_writable_package_root() {
        let fake = FakeInstall::new();
        set_mode(&fake.package, 0o775);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::PackageUnsafe { .. })
        ));
    }
}
