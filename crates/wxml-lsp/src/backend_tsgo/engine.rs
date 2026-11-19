//! Pinned native engine discovery from the installed package tree.
//!
//! Discovery starts at [`std::env::current_exe`], resolves the real path and
//! requires the layout `<package>/bin/wxml-lsp`, `<package>/manifest.json` and
//! `<package>/engine/lib/tsc`. It never reads `PATH`, the process working
//! directory, backend environment variables or a package outside the running
//! executable's own tree, and it never falls back to the Node backend.
//!
//! This is the P6-4a scaffold plus the P6-4b asset handoff. It validates the
//! manifest identity that `wxml-lsp` can check without the shared manifest
//! crate. Type assets are not read here; the adapter loads and verifies the
//! returned roots before it creates a sandbox or session.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::native::supervisor::{SpawnError, verify_engine_executable};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Delivery package name recorded by the native manifest.
///
/// These manifest-schema constants mirror `wxml-delivery` until the shared
/// manifest crate planned as P6-4 4-1 exists. P6-4a must not add the full
/// delivery crate to `wxml-lsp`: it would pull in network and archive
/// dependencies the server does not need.
const PACKAGE_NAME: &str = "wxml-lsp";
/// The only native platform covered by the frozen delivery contract.
const PLATFORM: &str = "linux-x86_64";
/// Native manifest schema version.
const MANIFEST_SCHEMA_V2: u32 = 2;
/// Rust-to-engine protocol version accepted by this build.
const NATIVE_PROTOCOL_VERSION: u64 = 2;
/// Maximum accepted `manifest.json` size.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// Exact POSIX mode required for `manifest.json`.
const MANIFEST_MODE: u32 = 0o644;
/// Exact POSIX mode required for the pinned engine executable.
const ENGINE_MODE: u32 = 0o755;

/// A package tree that passed P6-4a engine discovery.
///
/// This is **not** full install verification. The strict manifest projection
/// checks schema, package, version, platform, protocol and the pinned engine
/// identity, but it parses without validating `typeAssets`, `binaries`,
/// `builtAt`, `engine.distributionUrl` and `engine.distributionSize`.
/// `standard_lib_root` and `typings_root` are not read here;
/// [`TsgoBackendAdapter::with_install`](super::TsgoBackendAdapter::with_install)
/// loads and verifies them before any sandbox or session exists. Cross-checking
/// the manifest `typeAssets` fields remains P6-3-4/F3-03.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredInstall {
    /// Canonical package root (`<install>/versions/<version>`).
    pub(crate) package_root: PathBuf,
    /// Canonical `<package>/engine/lib/tsc`.
    pub(crate) engine: PathBuf,
    /// `<package>/engine/lib`; verified against the built-in pin by the adapter.
    pub(crate) standard_lib_root: PathBuf,
    /// `<package>/typings`; verified against the built-in pin by the adapter.
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
    /// The executable is not laid out as `<package>/bin/wxml-lsp`.
    #[error("the running executable is not installed as <package>/bin/wxml-lsp")]
    NotInstalled,
    /// A required package directory is missing, a symlink or unsafe.
    #[error("the installed package layout is unsafe: {reason}")]
    PackageUnsafe {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// `package/manifest.json` is missing, unreadable or unsafe.
    #[error("the installed package manifest is unavailable: {reason}")]
    ManifestUnavailable {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// The manifest parses but does not match this build's pinned identity.
    #[error("the installed package manifest does not match this build: {field}")]
    ManifestMismatch {
        /// Manifest field or shape that failed.
        field: &'static str,
    },
    /// The manifest's engine file entry is absent.
    #[error("the pinned native engine is missing")]
    EngineMissing,
    /// The engine is a symlink, a non-file or has an unsafe mode.
    #[error("the pinned native engine is unsafe: {reason}")]
    EngineUnsafe {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// The engine bytes, size or digest differ from the manifest.
    #[error("the pinned native engine does not match its manifest: {reason}")]
    EngineArtifactMismatch {
        /// Stable invariant that failed.
        reason: &'static str,
    },
    /// The engine path is absolute, contains `..` or resolves outside the package.
    #[error("the pinned native engine escapes the package root")]
    EngineEscapesPackage,
}

/// Discovers and verifies the pinned engine from the running executable.
///
/// # Errors
///
/// Returns a typed discovery error and no fallback backend when the executable
/// is not installed, the manifest is missing or mismatched, or the engine is
/// missing, unsafe or not byte-identical to the pinned artifact.
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
/// exercising every manifest, layout and digest check.
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
    for relative in ["bin", "engine", "engine/lib"] {
        validate_package_directory(&package_root.join(relative))?;
    }

    let manifest = read_manifest(package_root)?;
    validate_manifest(&manifest, expected_engine_sha256)?;

    let pin = PinnedEngine::pinned();
    let engine_entry = manifest
        .files
        .iter()
        .find(|file| file.path == pin.executable())
        .ok_or(EngineDiscoveryError::EngineMissing)?;
    let engine = verify_engine_file(
        package_root,
        pin.executable(),
        engine_entry.size,
        expected_engine_sha256,
    )?;

    Ok(DiscoveredInstall {
        package_root: package_root.to_path_buf(),
        engine,
        standard_lib_root: package_root.join("engine/lib"),
        typings_root: package_root.join("typings"),
    })
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

fn read_manifest(package_root: &Path) -> Result<ManifestProjection, EngineDiscoveryError> {
    let path = package_root.join("manifest.json");
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        let reason = if error.kind() == io::ErrorKind::NotFound {
            "manifest.json is missing"
        } else {
            "manifest.json is unreadable"
        };
        EngineDiscoveryError::ManifestUnavailable { reason }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json must not be a symlink",
        });
    }
    if !metadata.is_file() {
        return Err(EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json must be a regular file",
        });
    }
    if file_mode(&metadata) != MANIFEST_MODE {
        return Err(EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json mode must be 0644",
        });
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json exceeds the 4 MiB limit",
        });
    }

    let file = File::open(&path).map_err(|_| EngineDiscoveryError::ManifestUnavailable {
        reason: "manifest.json is unreadable",
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json is unreadable",
        })?;
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length > MAX_MANIFEST_BYTES {
        return Err(EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json exceeds the 4 MiB limit",
        });
    }

    parse_manifest(&bytes)
}

fn parse_manifest(bytes: &[u8]) -> Result<ManifestProjection, EngineDiscoveryError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| EngineDiscoveryError::ManifestUnavailable {
            reason: "manifest.json is not valid JSON",
        })?;
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(MANIFEST_SCHEMA_V2))
    {
        return Err(EngineDiscoveryError::ManifestMismatch {
            field: "schemaVersion",
        });
    }
    serde_json::from_value(value).map_err(|_| EngineDiscoveryError::ManifestMismatch {
        field: "manifest shape",
    })
}

fn validate_manifest(
    manifest: &ManifestProjection,
    expected_engine_sha256: &str,
) -> Result<(), EngineDiscoveryError> {
    let pin = PinnedEngine::pinned();

    require(manifest.package == PACKAGE_NAME, "package")?;
    require(
        manifest.package_version == crate::version(),
        "packageVersion",
    )?;
    require(manifest.platform == PLATFORM, "platform")?;
    require(
        manifest.protocol_version == NATIVE_PROTOCOL_VERSION,
        "protocolVersion",
    )?;

    let engine = &manifest.engine;
    require(engine.name == "typescript", "engine.name")?;
    require(engine.version == pin.version(), "engine.version")?;
    require(
        engine.source_commit == pin.git_head(),
        "engine.sourceCommit",
    )?;
    require(engine.distribution == pin.package(), "engine.distribution")?;
    require(
        engine.distribution_integrity == pin.sri(),
        "engine.distributionIntegrity",
    )?;
    require(
        engine.distribution_sha256 == pin.tarball_sha256(),
        "engine.distributionSha256",
    )?;
    require(
        engine.distribution_unpacked_size == pin.unpacked_bytes(),
        "engine.distributionUnpackedSize",
    )?;
    require(
        engine.distribution_file_count == u64::from(pin.file_count()),
        "engine.distributionFileCount",
    )?;
    require(
        engine.api_schema_version == u64::from(pin.ast_protocol_version()),
        "engine.apiSchemaVersion",
    )?;
    require(
        engine.ast_header_bytes == u64::try_from(pin.ast_header_bytes()).unwrap_or(u64::MAX),
        "engine.astHeaderBytes",
    )?;
    require(engine.license == "Apache-2.0", "engine.license")?;

    if engine.binary != pin.executable() {
        return if engine_path_escapes(&engine.binary) {
            Err(EngineDiscoveryError::EngineEscapesPackage)
        } else {
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "engine.binary",
            })
        };
    }

    let only_version_argument = engine.probe.args.len() == 1
        && engine
            .probe
            .args
            .first()
            .is_some_and(|arg| arg == "--version");
    require(only_version_argument, "engine.probe.args")?;
    let expected_prefix = pin.version_line().strip_suffix(pin.version()).unwrap_or("");
    require(
        engine.probe.expect_prefix == expected_prefix,
        "engine.probe.expectPrefix",
    )?;

    let mut engine_entry = None;
    for file in &manifest.files {
        if file.path == pin.executable() && engine_entry.replace(file).is_some() {
            return Err(EngineDiscoveryError::ManifestMismatch { field: "files" });
        }
    }
    let engine_entry = engine_entry.ok_or(EngineDiscoveryError::EngineMissing)?;
    require(engine_entry.mode == "0755", "files[engine].mode")?;
    require(
        engine_entry.sha256 == expected_engine_sha256,
        "files[engine].sha256",
    )?;
    require(engine_entry.size > 0, "files[engine].size")?;

    Ok(())
}

fn engine_path_escapes(path: &str) -> bool {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return true;
    }
    Path::new(path).components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
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
            reason: "engine size differs from the manifest",
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
            reason: "engine SHA-256 differs from the manifest",
        },
        _ => EngineDiscoveryError::EngineArtifactMismatch {
            reason: "engine executable could not be verified",
        },
    }
}

const fn require(condition: bool, field: &'static str) -> Result<(), EngineDiscoveryError> {
    if condition {
        Ok(())
    } else {
        Err(EngineDiscoveryError::ManifestMismatch { field })
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestProjection {
    schema_version: u32,
    package: String,
    package_version: String,
    platform: String,
    protocol_version: u64,
    built_at: String,
    engine: EngineProjection,
    type_assets: Vec<TypeAssetProjection>,
    binaries: Vec<BinaryProjection>,
    files: Vec<FileProjection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EngineProjection {
    name: String,
    version: String,
    source_commit: String,
    distribution: String,
    distribution_url: String,
    distribution_integrity: String,
    distribution_sha256: String,
    distribution_size: u64,
    distribution_file_count: u64,
    distribution_unpacked_size: u64,
    api_schema_version: u64,
    ast_header_bytes: u64,
    license: String,
    license_files: Vec<String>,
    binary: String,
    probe: ProbeProjection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProbeProjection {
    args: Vec<String>,
    expect_prefix: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypeAssetProjection {
    name: String,
    version: String,
    license: String,
    root: String,
    include: Vec<String>,
    file_count: u64,
    sha256: String,
    license_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BinaryProjection {
    kind: String,
    name: String,
    path: String,
    version: String,
    protocol_version: Option<u64>,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileProjection {
    path: String,
    sha256: String,
    size: u64,
    mode: String,
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(label: &str) -> Self {
            let base =
                std::env::var_os("WXML_TEST_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
            assert!(base.is_absolute(), "WXML_TEST_TMPDIR must be absolute");
            fs::create_dir_all(&base).expect("test base");
            let path = base.join(format!(
                "wxml-lsp-p64a-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("test root");
            set_mode(&path, 0o700);
            Self { path }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct FakeInstall {
        root: TestRoot,
        package: PathBuf,
        executable: PathBuf,
        engine: PathBuf,
        engine_sha: String,
        engine_size: u64,
    }

    impl FakeInstall {
        fn new() -> Self {
            let root = TestRoot::new("engine");
            let package = root.path.join("package");
            for directory in [
                package.clone(),
                package.join("bin"),
                package.join("engine"),
                package.join("engine/lib"),
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

            let fake = Self {
                root,
                package,
                executable,
                engine,
                engine_sha: sha256_hex(engine_bytes),
                engine_size: u64::try_from(engine_bytes.len()).expect("engine size"),
            };
            fake.write_manifest_value(&fake.manifest_value());
            fake
        }

        fn manifest_value(&self) -> serde_json::Value {
            manifest_value(&self.engine_sha, self.engine_size)
        }

        fn write_manifest_value(&self, value: &serde_json::Value) {
            let path = self.package.join("manifest.json");
            let bytes = serde_json::to_vec_pretty(value).expect("manifest json");
            fs::write(&path, bytes).expect("manifest");
            set_mode(&path, 0o644);
        }

        fn discover(&self) -> Result<DiscoveredInstall, EngineDiscoveryError> {
            discover_at_with_engine_sha256(&self.executable, &self.engine_sha)
        }
    }

    fn manifest_value(engine_sha: &str, engine_size: u64) -> serde_json::Value {
        let pin = PinnedEngine::pinned();
        json!({
            "schemaVersion": MANIFEST_SCHEMA_V2,
            "package": PACKAGE_NAME,
            "packageVersion": env!("CARGO_PKG_VERSION"),
            "platform": PLATFORM,
            "protocolVersion": NATIVE_PROTOCOL_VERSION,
            "builtAt": "2026-11-18T00:00:00Z",
            "engine": {
                "name": "typescript",
                "version": pin.version(),
                "sourceCommit": pin.git_head(),
                "distribution": pin.package(),
                "distributionUrl": "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz",
                "distributionIntegrity": pin.sri(),
                "distributionSha256": pin.tarball_sha256(),
                "distributionSize": 10_175_135,
                "distributionFileCount": pin.file_count(),
                "distributionUnpackedSize": pin.unpacked_bytes(),
                "apiSchemaVersion": pin.ast_protocol_version(),
                "astHeaderBytes": pin.ast_header_bytes(),
                "license": "Apache-2.0",
                "licenseFiles": ["engine/LICENSE", "engine/NOTICE.txt"],
                "binary": pin.executable(),
                "probe": {
                    "args": ["--version"],
                    "expectPrefix": "Version "
                }
            },
            "typeAssets": [],
            "binaries": [],
            "files": [
                {
                    "path": pin.executable(),
                    "sha256": engine_sha,
                    "size": engine_size,
                    "mode": "0755"
                },
                {
                    "path": "bin/wxml-lsp",
                    "sha256": "0".repeat(64),
                    "size": 1,
                    "mode": "0755"
                }
            ]
        })
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
    fn discovery_is_engine_only_and_does_not_claim_asset_verification() {
        let fake = FakeInstall::new();
        fs::remove_dir_all(fake.package.join("typings")).unwrap();
        let mut value = fake.manifest_value();
        value["builtAt"] = json!("not-a-date");
        value["engine"]["distributionUrl"] = json!("https://invalid.example/engine.tgz");
        value["engine"]["distributionSize"] = json!(1);
        value["typeAssets"] = json!([{
            "name": "not-a-pinned-asset",
            "version": "0.0.0",
            "license": "GPL-3.0",
            "root": "types",
            "include": ["**"],
            "fileCount": 1,
            "sha256": "0".repeat(64),
            "licenseFiles": []
        }]);
        value["binaries"] = json!([{
            "kind": "node-backend",
            "name": "legacy",
            "path": "node/server.js",
            "version": "0.0.0",
            "sha256": "0".repeat(64)
        }]);
        fake.write_manifest_value(&value);

        let install = fake
            .discover()
            .expect("P6-4a discovery is engine-only and must not overclaim assets");
        assert_eq!(install.typings_root, fake.package.join("typings"));
        assert!(!install.typings_root.exists());
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
    }

    #[test]
    fn rejects_missing_manifest() {
        let fake = FakeInstall::new();
        fs::remove_file(fake.package.join("manifest.json")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestUnavailable { .. })
        ));
    }

    #[test]
    fn rejects_manifest_with_an_unsafe_mode() {
        let fake = FakeInstall::new();
        set_mode(&fake.package.join("manifest.json"), 0o600);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestUnavailable { .. })
        ));
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
    fn rejects_missing_engine_manifest_entry() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["files"] = json!([]);
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::EngineMissing)
        ));
    }

    #[test]
    fn rejects_manifest_engine_entry_mode() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["files"][0]["mode"] = json!("0644");
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "files[engine].mode"
            })
        ));
    }

    #[test]
    fn rejects_a_symlinked_bin_executable() {
        let fake = FakeInstall::new();
        let outside = fake.root.path.join("outside-wxml-lsp");
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
    fn rejects_manifest_symlink() {
        let fake = FakeInstall::new();
        let outside = fake.root.path.join("outside-manifest.json");
        fs::copy(fake.package.join("manifest.json"), &outside).unwrap();
        fs::remove_file(fake.package.join("manifest.json")).unwrap();
        std::os::unix::fs::symlink(&outside, fake.package.join("manifest.json")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestUnavailable { .. })
        ));
    }

    #[test]
    fn rejects_v1_manifest() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["schemaVersion"] = json!(1);
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "schemaVersion"
            })
        ));
    }

    #[test]
    fn rejects_unknown_manifest_field() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["futureField"] = json!(true);
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "manifest shape"
            })
        ));
    }

    #[test]
    fn rejects_wrong_package_version() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["packageVersion"] = json!("9.9.9");
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "packageVersion"
            })
        ));
    }

    #[test]
    fn rejects_wrong_platform_and_protocol() {
        let fake = FakeInstall::new();
        let mut platform = fake.manifest_value();
        platform["platform"] = json!("darwin-x86_64");
        fake.write_manifest_value(&platform);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch { field: "platform" })
        ));

        let mut protocol = fake.manifest_value();
        protocol["protocolVersion"] = json!(1);
        fake.write_manifest_value(&protocol);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "protocolVersion"
            })
        ));
    }

    #[test]
    fn rejects_wrong_engine_version_and_ast_header() {
        let fake = FakeInstall::new();
        let mut version = fake.manifest_value();
        version["engine"]["version"] = json!("7.0.2");
        fake.write_manifest_value(&version);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "engine.version"
            })
        ));

        let mut ast = fake.manifest_value();
        ast["engine"]["astHeaderBytes"] = json!(60);
        fake.write_manifest_value(&ast);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "engine.astHeaderBytes"
            })
        ));
    }

    #[test]
    fn rejects_manifest_engine_digest_mismatch() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["files"][0]["sha256"] = json!("0".repeat(64));
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestMismatch {
                field: "files[engine].sha256"
            })
        ));
    }

    #[test]
    fn rejects_engine_artifact_digest_mismatch() {
        let fake = FakeInstall::new();
        let zeros = "0".repeat(64);
        let mut value = fake.manifest_value();
        value["files"][0]["sha256"] = json!(zeros);
        fake.write_manifest_value(&value);
        assert!(matches!(
            discover_at_with_engine_sha256(&fake.executable, &zeros),
            Err(EngineDiscoveryError::EngineArtifactMismatch { .. })
        ));
    }

    #[test]
    fn rejects_engine_size_mismatch() {
        let fake = FakeInstall::new();
        let mut value = fake.manifest_value();
        value["files"][0]["size"] = json!(fake.engine_size + 1);
        fake.write_manifest_value(&value);
        assert!(matches!(
            fake.discover(),
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
        let outside = fake.root.path.join("outside-tsc");
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
        let outside = fake.root.path.join("outside-lib");
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
    fn rejects_engine_paths_that_escape_the_package() {
        let fake = FakeInstall::new();
        for escaped in ["/tmp/tsc", "../tsc", "engine/../tsc"] {
            let mut value = fake.manifest_value();
            value["engine"]["binary"] = json!(escaped);
            fake.write_manifest_value(&value);
            assert!(
                matches!(
                    fake.discover(),
                    Err(EngineDiscoveryError::EngineEscapesPackage)
                ),
                "expected EngineEscapesPackage for {escaped}"
            );
        }
    }

    #[test]
    fn rejects_executable_outside_bin() {
        let fake = FakeInstall::new();
        let loose = fake.root.path.join("wxml-lsp");
        fs::copy(&fake.executable, &loose).unwrap();
        set_mode(&loose, 0o755);
        assert!(matches!(
            discover_at_with_engine_sha256(&loose, &fake.engine_sha),
            Err(EngineDiscoveryError::NotInstalled)
        ));

        let other = fake.root.path.join("other/wxml-lsp");
        fs::create_dir_all(fake.root.path.join("other")).unwrap();
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
        let missing = fake.root.path.join("missing/bin/wxml-lsp");
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
    fn does_not_search_parent_directories_for_the_manifest() {
        let fake = FakeInstall::new();
        fs::copy(
            fake.package.join("manifest.json"),
            fake.root.path.join("manifest.json"),
        )
        .unwrap();
        fs::remove_file(fake.package.join("manifest.json")).unwrap();
        assert!(matches!(
            fake.discover(),
            Err(EngineDiscoveryError::ManifestUnavailable { .. })
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
        let size = fs::metadata(&fake.engine).unwrap().len();
        let pinned_sha = PinnedEngine::pinned().executable_sha256();
        fake.write_manifest_value(&manifest_value(pinned_sha, size));
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
