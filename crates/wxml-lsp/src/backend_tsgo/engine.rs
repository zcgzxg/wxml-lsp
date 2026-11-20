//! Pinned native engine discovery from the installed package tree.
//!
//! Discovery starts at [`std::env::current_exe`], resolves the real path and
//! requires the native-only layout `<root>/bin/wxml-lsp`,
//! `<root>/engine/lib/tsc` and `<root>/typings`. The package root plus the
//! `bin`, `engine`, `engine/lib` and `typings` directories must be real
//! directories with safe modes, and the engine must be a regular `0755` file
//! whose size is read from its metadata and whose SHA-256 matches the built-in
//! pin. It never reads `PATH`, the process working directory, backend
//! environment variables or a package outside the running executable's own
//! tree, and it never falls back to another backend.
//!
//! Type assets are not read here; the adapter loads and verifies the returned
//! `engine/lib` and `typings` roots before it creates a sandbox or session.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::native::supervisor::{SpawnError, verify_engine_executable};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Exact POSIX mode required for the pinned engine executable.
const ENGINE_MODE: u32 = 0o755;

/// A package tree that passed pinned engine discovery.
///
/// This is **not** full install verification. Discovery checks the native-only
/// layout and requires `<root>/engine/lib/tsc` to be a regular `0755` file whose
/// SHA-256 matches the built-in pin. The runtime independently verifies the
/// standard-library and typings bytes against the crate pin in
/// [`TsgoBackendAdapter::with_install`](super::TsgoBackendAdapter::with_install).
/// `standard_lib_root` and `typings_root` are not read during discovery.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredInstall {
    /// Canonical package root (`<root>`).
    pub(crate) package_root: PathBuf,
    /// Canonical `<root>/engine/lib/tsc`.
    pub(crate) engine: PathBuf,
    /// `<root>/engine/lib`; verified against the built-in pin by the adapter.
    pub(crate) standard_lib_root: PathBuf,
    /// `<root>/typings`; verified against the built-in pin by the adapter.
    pub(crate) typings_root: PathBuf,
}

impl DiscoveredInstall {
    /// Returns the engine `lib` directory that must contain the pinned library.
    pub(crate) fn standard_lib_root(&self) -> &Path {
        &self.standard_lib_root
    }

    /// Returns the directory that must contain `miniprogram-api-typings`.
    pub(crate) fn typings_root(&self) -> &Path {
        &self.typings_root
    }
}

/// Why the running executable's package tree cannot supply the pinned engine.
///
/// Messages deliberately contain no absolute paths, engine output or
/// environment values; callers can present them to users.
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
    /// The pinned engine file is absent.
    #[error("the pinned native engine is missing")]
    EngineMissing,
    /// The engine is a symlink, a non-file or has an unsafe mode.
    #[error("the pinned native engine is unsafe: {reason}")]
    EngineUnsafe {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// The engine bytes, size or digest differ from the pinned artifact.
    #[error("the pinned native engine does not match the pinned artifact: {reason}")]
    EngineArtifactMismatch {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// The engine resolves outside the package root.
    #[error("the pinned native engine escapes the package root")]
    EngineEscapesPackage,
}

/// Discovers and verifies the pinned engine from the running executable.
///
/// # Errors
///
/// Returns a typed discovery error and no fallback backend when the executable
/// is not installed, the layout is unsafe, or the engine is missing, unsafe or
/// not byte-identical to the pinned artifact.
pub(crate) fn discover_from_current_exe() -> Result<DiscoveredInstall, EngineDiscoveryError> {
    let executable =
        std::env::current_exe().map_err(|_| EngineDiscoveryError::UnresolvableExecutable)?;
    discover(&executable, PinnedEngine::pinned().executable_sha256())
}

/// Test-only discovery at an explicit executable path with the production pin.
#[cfg(test)]
pub(crate) fn discover_at(executable: &Path) -> Result<DiscoveredInstall, EngineDiscoveryError> {
    discover(executable, PinnedEngine::pinned().executable_sha256())
}

/// Test-only discovery that substitutes the expected engine SHA-256.
///
/// Production callers always use [`discover_from_current_exe`] and therefore
/// the pinned executable digest. The substitution lets unit tests build a
/// small fake package tree without a 24 MiB engine artifact while still
/// exercising every layout and digest check.
#[cfg(test)]
pub(crate) fn discover_at_with_engine_sha256(
    executable: &Path,
    expected_engine_sha256: &str,
) -> Result<DiscoveredInstall, EngineDiscoveryError> {
    discover(executable, expected_engine_sha256)
}

fn discover(
    executable: &Path,
    expected_engine_sha256: &str,
) -> Result<DiscoveredInstall, EngineDiscoveryError> {
    let executable =
        fs::canonicalize(executable).map_err(|_| EngineDiscoveryError::UnresolvableExecutable)?;

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
    for relative in ["bin", "engine", "engine/lib", "typings"] {
        validate_package_directory(&package_root.join(relative))?;
    }

    let pin = PinnedEngine::pinned();
    let engine_relative = pin.executable();
    let engine_size = engine_file_size(package_root, engine_relative)?;
    let engine = verify_engine_file(
        package_root,
        engine_relative,
        engine_size,
        expected_engine_sha256,
    )?;

    Ok(DiscoveredInstall {
        package_root: package_root.to_path_buf(),
        engine,
        standard_lib_root: package_root.join("engine/lib"),
        typings_root: package_root.join("typings"),
    })
}

/// Reads the engine size from the file itself.
fn engine_file_size(package_root: &Path, relative: &str) -> Result<u64, EngineDiscoveryError> {
    match fs::metadata(package_root.join(relative)) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(EngineDiscoveryError::EngineMissing)
        }
        Err(_) => Err(EngineDiscoveryError::EngineUnsafe {
            reason: "engine metadata is unreadable",
        }),
    }
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

fn verify_engine_file(
    package_root: &Path,
    relative: &str,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<PathBuf, EngineDiscoveryError> {
    let path = package_root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            EngineDiscoveryError::EngineMissing
        } else {
            EngineDiscoveryError::EngineUnsafe {
                reason: "engine metadata is unreadable",
            }
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(EngineDiscoveryError::EngineUnsafe {
            reason: "engine must not be a symlink",
        });
    }
    if !metadata.is_file() {
        return Err(EngineDiscoveryError::EngineUnsafe {
            reason: "engine must be a regular file",
        });
    }
    if file_mode(&metadata) != ENGINE_MODE {
        return Err(EngineDiscoveryError::EngineUnsafe {
            reason: "engine mode must be 0755",
        });
    }
    if metadata.len() != expected_size {
        return Err(EngineDiscoveryError::EngineArtifactMismatch {
            reason: "engine size differs from the pinned size",
        });
    }

    let canonical = fs::canonicalize(&path).map_err(|_| EngineDiscoveryError::EngineUnsafe {
        reason: "engine path cannot be canonicalized",
    })?;
    if canonical == package_root || !canonical.starts_with(package_root) {
        return Err(EngineDiscoveryError::EngineEscapesPackage);
    }

    verify_engine_executable(&path, expected_sha256).map_err(|error| map_spawn_error(&error))?;
    Ok(canonical)
}

const fn map_spawn_error(error: &SpawnError) -> EngineDiscoveryError {
    match error {
        SpawnError::ExecutableMissing => EngineDiscoveryError::EngineMissing,
        SpawnError::ExecutableSymlink
        | SpawnError::ExecutableNotFile
        | SpawnError::ExecutableNotExecutable => EngineDiscoveryError::EngineUnsafe {
            reason: "engine executable failed Unix safety checks",
        },
        SpawnError::ExecutableMismatch => EngineDiscoveryError::EngineArtifactMismatch {
            reason: "engine SHA-256 differs from the pinned digest",
        },
        _ => EngineDiscoveryError::EngineArtifactMismatch {
            reason: "engine executable could not be verified",
        },
    }
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

    use sha2::{Digest, Sha256};

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
        engine: PathBuf,
        engine_sha: String,
        engine_size: u64,
    }

    impl FakeInstall {
        fn new() -> Self {
            let root = test_root("engine");
            let package = root.path().join("package");
            for directory in [
                package.clone(),
                package.join("bin"),
                package.join("engine"),
                package.join("engine/lib"),
                package.join("typings"),
                package.join("typings/miniprogram-api-typings"),
            ] {
                fs::create_dir_all(&directory).expect("package directory");
                set_mode(&directory, 0o755);
            }

            let executable = package.join("bin/wxml-lsp");
            fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("executable");
            set_mode(&executable, 0o755);

            let engine_bytes = b"fake pinned engine fixture\n";
            let engine = package.join("engine/lib/tsc");
            fs::write(&engine, engine_bytes).expect("engine");
            set_mode(&engine, 0o755);

            Self {
                root,
                package,
                executable,
                engine,
                engine_sha: sha256_hex(engine_bytes),
                engine_size: u64::try_from(engine_bytes.len()).expect("engine size"),
            }
        }

        fn discover(&self) -> Result<DiscoveredInstall, EngineDiscoveryError> {
            discover_at_with_engine_sha256(&self.executable, &self.engine_sha)
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("permissions");
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut encoded = String::with_capacity(digest.len() * 2);
        for byte in digest {
            for half in [byte >> 4, byte & 0x0f] {
                encoded.push(char::from(b"0123456789abcdef"[usize::from(half)]));
            }
        }
        encoded
    }

    #[test]
    fn discovers_fake_package_from_bin_executable() {
        let fake = FakeInstall::new();
        let install = fake.discover().expect("discovered install");

        assert_eq!(
            install.package_root,
            fs::canonicalize(&fake.package).unwrap()
        );
        assert_eq!(install.engine, fs::canonicalize(&fake.engine).unwrap());
        assert_eq!(
            install.standard_lib_root,
            fs::canonicalize(fake.package.join("engine/lib")).unwrap()
        );
        assert_eq!(install.typings_root, fake.package.join("typings"));
    }

    #[test]
    fn discovery_checks_layout_roots_but_not_asset_bytes() {
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
        assert_eq!(install.typings_root, fake.package.join("typings"));
        assert!(install.typings_root.is_dir());
    }

    #[test]
    fn with_install_rejects_a_package_without_pinned_assets() {
        let fake = FakeInstall::new();
        let install = fake.discover().expect("discovered install");
        let host: Arc<dyn crate::project_host::ProjectHost> =
            Arc::new(crate::project_host::ProjectState::new());
        let error = super::super::TsgoBackendAdapter::with_install(host, &install)
            .expect_err("a package without pinned assets must fail before the sandbox");
        assert!(matches!(
            error,
            super::super::NativeStartupError::Environment(_)
        ));
        let message = error.to_string();
        assert!(!message.contains("server.js"), "{message}");
        assert!(!message.to_ascii_lowercase().contains("node"), "{message}");
    }

    #[test]
    fn with_install_rejects_a_missing_typings_root_before_the_sandbox() {
        let fake = FakeInstall::new();
        let install = fake.discover().expect("discovered install");
        fs::remove_dir_all(fake.package.join("typings")).unwrap();
        let host: Arc<dyn crate::project_host::ProjectHost> =
            Arc::new(crate::project_host::ProjectState::new());
        let error = super::super::TsgoBackendAdapter::with_install(host, &install)
            .expect_err("missing typings must fail closed");
        assert!(matches!(
            error,
            super::super::NativeStartupError::Environment(_)
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
            Err(EngineDiscoveryError::PackageUnsafe { .. })
        ));
    }

    #[test]
    fn discovery_requires_engine_lib_and_typings_layout_roots() {
        for (label, path) in [
            ("engine", "engine"),
            ("engine-lib", "engine/lib"),
            ("typings", "typings"),
        ] {
            let fake = FakeInstall::new();
            fs::remove_dir_all(fake.package.join(path)).unwrap();
            let error = fake.discover().expect_err(label);
            assert!(
                matches!(error, EngineDiscoveryError::PackageUnsafe { .. }),
                "{label}: {error:?}"
            );
        }
    }

    #[test]
    fn rejects_missing_engine_file() {
        let fake = FakeInstall::new();
        fs::remove_file(&fake.engine).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::EngineMissing)
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
    fn rejects_engine_artifact_digest_mismatch() {
        let fake = FakeInstall::new();
        let zeros = "0".repeat(64);
        assert!(matches!(
            discover_at_with_engine_sha256(&fake.executable, &zeros),
            Err(EngineDiscoveryError::EngineArtifactMismatch { .. })
        ));
    }

    #[test]
    fn rejects_engine_size_mismatch() {
        let fake = FakeInstall::new();
        let pin = PinnedEngine::pinned();
        assert!(matches!(
            verify_engine_file(
                &fake.package,
                pin.executable(),
                fake.engine_size + 1,
                &fake.engine_sha,
            ),
            Err(EngineDiscoveryError::EngineArtifactMismatch { .. })
        ));
    }

    #[test]
    fn rejects_engine_mode_mismatch() {
        let fake = FakeInstall::new();
        set_mode(&fake.engine, 0o644);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::EngineUnsafe {
                reason: "engine mode must be 0755"
            })
        ));
    }

    #[test]
    fn rejects_engine_symlink() {
        let fake = FakeInstall::new();
        let outside = fake.root.path().join("outside-tsc");
        fs::write(&outside, b"outside\n").unwrap();
        set_mode(&outside, 0o755);
        fs::remove_file(&fake.engine).unwrap();
        std::os::unix::fs::symlink(&outside, &fake.engine).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::EngineUnsafe {
                reason: "engine must not be a symlink"
            })
        ));
    }

    #[test]
    fn rejects_engine_parent_directory_symlink() {
        let fake = FakeInstall::new();
        let outside = fake.root.path().join("outside-lib");
        fs::create_dir_all(&outside).unwrap();
        set_mode(&outside, 0o755);
        fs::write(outside.join("tsc"), b"outside\n").unwrap();
        set_mode(&outside.join("tsc"), 0o755);
        fs::remove_file(&fake.engine).unwrap();
        fs::remove_dir(fake.package.join("engine/lib")).unwrap();
        std::os::unix::fs::symlink(&outside, fake.package.join("engine/lib")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::PackageUnsafe { .. })
        ));
    }

    #[test]
    fn rejects_executable_outside_bin() {
        let fake = FakeInstall::new();
        let loose = fake.root.path().join("wxml-lsp");
        fs::copy(&fake.executable, &loose).unwrap();
        set_mode(&loose, 0o755);
        assert!(matches!(
            discover_at_with_engine_sha256(&loose, &fake.engine_sha),
            Err(EngineDiscoveryError::NotInstalled)
        ));

        let other = fake.root.path().join("other/wxml-lsp");
        fs::create_dir_all(fake.root.path().join("other")).unwrap();
        fs::copy(&fake.executable, &other).unwrap();
        set_mode(&other, 0o755);
        assert!(matches!(
            discover_at_with_engine_sha256(&other, &fake.engine_sha),
            Err(EngineDiscoveryError::NotInstalled)
        ));
    }

    #[test]
    fn rejects_missing_executable() {
        let fake = FakeInstall::new();
        let missing = fake.root.path().join("missing/bin/wxml-lsp");
        assert!(matches!(
            discover_at_with_engine_sha256(&missing, &fake.engine_sha),
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

    fn require_engine() -> bool {
        matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        )
    }

    fn fake_install_with_real_engine() -> Option<FakeInstall> {
        let Ok(bin) = std::env::var("TSGO_BIN") else {
            assert!(
                !require_engine(),
                "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
            );
            eprintln!(
                "optional real-engine test skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc"
            );
            return None;
        };

        let fake = FakeInstall::new();
        let bin = PathBuf::from(&bin);
        let lib = bin.parent().expect("TSGO_BIN must be <lib>/tsc");
        copy_tree(lib, fake.engine.parent().expect("fake engine parent"));
        set_mode(&fake.engine, 0o755);
        Some(fake)
    }

    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).expect("copy directory");
        for entry in fs::read_dir(from).expect("read source directory") {
            let entry = entry.expect("source entry");
            let file_type = entry.file_type().expect("source entry type");
            let target = to.join(entry.file_name());
            if file_type.is_dir() {
                copy_tree(&entry.path(), &target);
            } else if file_type.is_file() {
                fs::copy(entry.path(), target).expect("copy file");
            }
        }
    }

    /// Resolves the real typings package from `TSGO_TYPINGS_ROOT`.
    ///
    /// Both the containing `node_modules`/`typings` directory and the
    /// `miniprogram-api-typings` package directory itself are accepted.
    fn real_typings_package() -> Option<PathBuf> {
        let root = std::env::var_os("TSGO_TYPINGS_ROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("TSGO_ASSET_ROOT")
                    .filter(|value| !value.is_empty())
                    .map(|asset| PathBuf::from(asset).join("typings"))
            })?;
        let package =
            if root.file_name().and_then(|name| name.to_str()) == Some("miniprogram-api-typings") {
                root
            } else {
                root.join("miniprogram-api-typings")
            };
        package.is_dir().then_some(package)
    }

    fn copy_real_typings(fake: &FakeInstall) -> bool {
        let Some(package) = real_typings_package() else {
            return false;
        };
        copy_tree(
            &package,
            &fake.package.join("typings/miniprogram-api-typings"),
        );
        true
    }

    #[test]
    fn optional_real_pinned_engine_discovers() {
        let Some(fake) = fake_install_with_real_engine() else {
            return;
        };
        let install = discover_at(&fake.executable).expect("discover pinned engine");
        assert_eq!(install.engine, fs::canonicalize(&fake.engine).unwrap());
    }

    #[test]
    fn optional_real_engine_starts_a_native_session_from_a_discovered_install() {
        let Some(fake) = fake_install_with_real_engine() else {
            return;
        };
        if !copy_real_typings(&fake) {
            assert!(
                !require_engine(),
                "TSGO_TYPINGS_ROOT (or TSGO_ASSET_ROOT/typings) must provide the pinned \
                 miniprogram-api-typings when WXML_REQUIRE_REAL_ENGINE=1"
            );
            eprintln!(
                "optional real trusted-asset session test skipped: set TSGO_TYPINGS_ROOT to the \
                 directory that contains miniprogram-api-typings"
            );
            return;
        }
        let install = discover_at(&fake.executable).expect("discover pinned engine");
        let host: Arc<dyn crate::project_host::ProjectHost> =
            Arc::new(crate::project_host::ProjectState::new());
        let adapter = super::super::TsgoBackendAdapter::with_install(host, &install)
            .expect("adapter with managed sandbox and pinned assets");
        adapter
            .start_and_stop_session_for_test()
            .expect("successful native session start and shutdown");
    }
}
