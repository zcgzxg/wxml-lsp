//! Deterministic package construction from a staged build tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::archive::{create_tar_gz, extract_tar_gz_allowlisted};
use crate::assets_lock::{Asset, ENGINE_ASSET_NAME, ENGINE_ASSET_ROOT, TYPINGS_ASSET_NAME};
use crate::error::{DeliveryError, Result};
use crate::index_v2::{IndexV2, ReleaseV2, read_index_v2};
use crate::manifest::{
    ArtifactKind, BinaryArtifact, ExpectedManifest, FileEntry, Index, MAX_ARCHIVE_BYTES,
    MAX_FILE_BYTES, MAX_FILES, MAX_MANIFEST_BYTES, Manifest, NodeInfo, PACKAGE_NAME, Release,
    SCHEMA_VERSION, archive_name, atomic_write, format_sha256sums, read_index, read_limited,
    set_mode, sha256_bytes, sha256_file,
};
use crate::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_BINARY_PATH, ENGINE_DISTRIBUTION,
    ENGINE_DISTRIBUTION_FILE_COUNT, ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_DISTRIBUTION_SHA256,
    ENGINE_DISTRIBUTION_SIZE, ENGINE_DISTRIBUTION_UNPACKED_SIZE, ENGINE_DISTRIBUTION_URL,
    ENGINE_LICENSE, ENGINE_NAME, ENGINE_PROBE_PREFIX, ENGINE_SOURCE_COMMIT, ENGINE_VERSION,
    EngineInfo, EngineProbe, ExpectedManifestV2, MANIFEST_SCHEMA_V2, MAX_INSTALLER_SCHEMA,
    MINIPROGRAM_TYPINGS_ASSET, MINIPROGRAM_TYPINGS_BUNDLE_DIGEST, MINIPROGRAM_TYPINGS_FILE_COUNT,
    MINIPROGRAM_TYPINGS_LICENSE, MINIPROGRAM_TYPINGS_ROOT, MINIPROGRAM_TYPINGS_TOTAL_BYTES,
    MINIPROGRAM_TYPINGS_VERSION, ManifestV2, NATIVE_PACKAGE_PLATFORM, NativeArtifactKind,
    NativeBinaryArtifact, REQUIRED_ENGINE_LICENSE_FILES, STANDARD_LIB_ASSET,
    STANDARD_LIB_FILE_COUNT, STANDARD_LIB_INCLUDE, SUPPORTED_NATIVE_PROTOCOL_VERSIONS, TypeAsset,
    TypeAssetBundle, compute_type_asset_bundle, forbidden_runtime_path,
    is_allowed_miniprogram_typings_path, is_miniprogram_typings_path,
};
use crate::paths::{validate_platform, validate_relative_path, validate_version};
use crate::provision::{CacheEntry, read_assets_lock, verify_cached_asset};
use crate::semver::validate_strict;
use crate::sri::sha512_sri_file;
use crate::verify::{checksums_for_tree, verify_archive, verify_tree};
use crate::verify_v2::{checksums_for_tree_v2, verify_tree_v2};

/// Settings for one local package build.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Staged package root containing `bin/` and `zed-extension/`.
    pub input: PathBuf,
    /// Directory that receives the versioned directory and archive.
    pub output: PathBuf,
    /// Release version.
    pub release_version: String,
    /// Target platform label.
    pub platform: String,
    /// Type-bridge protocol version.
    pub protocol_version: u64,
    /// Minimum Node.js version.
    pub node_minimum: String,
    /// Node backend package version.
    pub backend_version: String,
    /// Zed extension version.
    pub zed_version: Option<String>,
    /// Build timestamp (UTC, RFC 3339).
    pub built_at: String,
    /// Tar entry timestamp; 0 keeps archives byte-identical across builds.
    pub mtime: u64,
    /// Replace an existing release directory/archive.
    pub force: bool,
    /// Validate and describe without writing anything.
    pub dry_run: bool,
}

/// Result of a successful package build.
#[derive(Clone, Debug)]
pub struct BuildReport {
    /// Validated manifest.
    pub manifest: Manifest,
    /// Versioned directory, when not a dry-run.
    pub release_dir: Option<PathBuf>,
    /// Archive path, when not a dry-run.
    pub archive: Option<PathBuf>,
    /// Archive SHA-256.
    pub sha256: String,
    /// Archive size.
    pub size: u64,
}

impl BuildReport {
    /// Serializes the manifest as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn manifest_json(&self) -> Result<Vec<u8>> {
        self.manifest.to_json()
    }
}

/// Builds a versioned directory, archive, checksums and index entry.
///
/// # Errors
///
/// Returns an error for missing required artifacts, unsafe input files,
/// unsupported modes, oversized payloads, existing output without `force`, or
/// a post-build verification failure.
pub fn build_package(options: &BuildOptions) -> Result<BuildReport> {
    validate_build_options(options)?;
    let input = std::fs::canonicalize(&options.input)
        .map_err(|error| DeliveryError::io(&options.input, error))?;
    if !input.is_dir() {
        return Err(DeliveryError::InvalidArgument(
            "input is not a directory".to_owned(),
        ));
    }
    let files = collect_files(&input)?;
    require_files(&files)?;
    let manifest = build_manifest(options, &files)?;

    if options.dry_run {
        return Ok(BuildReport {
            manifest,
            release_dir: None,
            archive: None,
            sha256: String::new(),
            size: 0,
        });
    }

    let (output, release_dir) = prepare_release_dir(&input, options)?;
    write_release_tree(&release_dir, &files, &manifest, options)?;
    let (archive_path, sha256, size) = write_archive(&output, &release_dir, &manifest, options)?;

    let verify_scratch = output.join(format!(
        ".verify-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let _ = std::fs::remove_dir_all(&verify_scratch);
    verify_archive(&archive_path, &verify_scratch, expected_for(options))?;
    record_release(&output, &manifest, &sha256, size, options.force)?;

    Ok(BuildReport {
        manifest,
        release_dir: Some(release_dir),
        archive: Some(archive_path),
        sha256,
        size,
    })
}

fn validate_build_options(options: &BuildOptions) -> Result<()> {
    validate_version(&options.release_version)?;
    validate_platform(&options.platform)?;
    if !crate::manifest::SUPPORTED_PROTOCOL_VERSIONS.contains(&options.protocol_version) {
        return Err(DeliveryError::InvalidArgument(format!(
            "protocol {} is not supported by this packager",
            options.protocol_version
        )));
    }
    crate::semver::Semver::parse(&options.node_minimum)?;
    validate_version(&options.backend_version)?;
    if options.built_at.is_empty() || options.built_at.len() > 64 {
        return Err(DeliveryError::InvalidArgument(
            "builtAt must be 1-64 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn expected_for(options: &BuildOptions) -> ExpectedManifest<'_> {
    ExpectedManifest {
        version: Some(&options.release_version),
        platform: Some(&options.platform),
        protocol_version: Some(options.protocol_version),
    }
}

fn prepare_release_dir(input: &Path, options: &BuildOptions) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(&options.output)
        .map_err(|error| DeliveryError::io(&options.output, error))?;
    let output = std::fs::canonicalize(&options.output)
        .map_err(|error| DeliveryError::io(&options.output, error))?;
    if output.starts_with(input) || input.starts_with(&output) {
        return Err(DeliveryError::InvalidArgument(
            "input and output directories must not contain each other".to_owned(),
        ));
    }
    let release_name = format!(
        "{PACKAGE_NAME}-{}-{}",
        options.release_version, options.platform
    );
    let release_dir = output.join(&release_name);
    if release_dir.exists() {
        if !options.force {
            return Err(DeliveryError::InstallState(format!(
                "release directory `{release_name}` already exists; use --force to replace it"
            )));
        }
        let metadata = std::fs::symlink_metadata(&release_dir)
            .map_err(|error| DeliveryError::io(&release_dir, error))?;
        if !metadata.file_type().is_dir() {
            return Err(DeliveryError::InstallState(
                "existing release output is not a directory".to_owned(),
            ));
        }
        std::fs::remove_dir_all(&release_dir)
            .map_err(|error| DeliveryError::io(&release_dir, error))?;
    }
    std::fs::create_dir(&release_dir).map_err(|error| DeliveryError::io(&release_dir, error))?;
    set_mode(&release_dir, 0o755)?;
    Ok((output, release_dir))
}

fn write_release_tree(
    release_dir: &Path,
    files: &[SourceFile],
    manifest: &Manifest,
    options: &BuildOptions,
) -> Result<()> {
    for file in files {
        let destination = release_dir.join(&file.entry.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
        }
        copy_file(&file.absolute, &destination, file.entry.mode_bits()?)?;
    }
    atomic_write(
        &release_dir.join("manifest.json"),
        &manifest.to_json()?,
        0o644,
    )?;
    let checksums = checksums_for_tree(release_dir, manifest)?;
    atomic_write(&release_dir.join("SHA256SUMS"), &checksums, 0o644)?;
    verify_tree(release_dir, expected_for(options))?;
    Ok(())
}

fn write_archive(
    output: &Path,
    release_dir: &Path,
    manifest: &Manifest,
    options: &BuildOptions,
) -> Result<(PathBuf, String, u64)> {
    let archive_path = output.join(archive_name(&options.release_version, &options.platform)?);
    if archive_path.exists() {
        if !options.force {
            return Err(DeliveryError::InstallState(
                "release archive already exists; use --force to replace it".to_owned(),
            ));
        }
        std::fs::remove_file(&archive_path)
            .map_err(|error| DeliveryError::io(&archive_path, error))?;
    }
    let mut archive_entries = manifest.files.clone();
    for name in ["manifest.json", "SHA256SUMS"] {
        let path = release_dir.join(name);
        let metadata = std::fs::metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        archive_entries.push(FileEntry {
            path: name.to_owned(),
            sha256: sha256_file(&path)?,
            size: metadata.len(),
            mode: "0644".to_owned(),
        });
    }
    archive_entries.sort_by(|left, right| left.path.cmp(&right.path));
    create_tar_gz(release_dir, &archive_path, &archive_entries, options.mtime)?;
    let sha256 = sha256_file(&archive_path)?;
    let size = std::fs::metadata(&archive_path)
        .map_err(|error| DeliveryError::io(&archive_path, error))?
        .len();
    if size > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(
            "generated archive exceeds the size limit".to_owned(),
        ));
    }
    let sidecar = PathBuf::from(format!("{}.sha256", archive_path.display()));
    let name = archive_path.file_name().map_or_else(
        || "archive".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    atomic_write(&sidecar, format!("{sha256}  {name}\n").as_bytes(), 0o644)?;
    Ok((archive_path, sha256, size))
}

fn record_release(
    output: &Path,
    manifest: &Manifest,
    sha256: &str,
    size: u64,
    force: bool,
) -> Result<()> {
    update_index(output, manifest, sha256, size, force)?;
    update_archive_checksums(output)
}

struct SourceFile {
    entry: FileEntry,
    absolute: PathBuf,
}

fn collect_files(root: &Path) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    collect_inner(root, root, &mut files, &mut 0)?;
    files.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));
    Ok(files)
}

fn collect_inner(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SourceFile>,
    directories: &mut usize,
) -> Result<()> {
    *directories += 1;
    if *directories > MAX_FILES {
        return Err(DeliveryError::SizeLimit(
            "input tree contains too many directories".to_owned(),
        ));
    }
    let entries =
        std::fs::read_dir(directory).map_err(|error| DeliveryError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(directory, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(DeliveryError::UnsafeArchive(format!(
                "input `{}` is a symbolic link; packages must be self-contained",
                relative_text(root, &path)?
            )));
        }
        if metadata.file_type().is_dir() {
            collect_inner(root, &path, files, directories)?;
        } else if metadata.file_type().is_file() {
            if files.len() >= MAX_FILES {
                return Err(DeliveryError::SizeLimit(format!(
                    "input tree contains more than {MAX_FILES} files"
                )));
            }
            if metadata.len() > MAX_FILE_BYTES {
                return Err(DeliveryError::SizeLimit(format!(
                    "input `{}` exceeds the per-file limit",
                    relative_text(root, &path)?
                )));
            }
            let relative = relative_text(root, &path)?;
            validate_relative_path(&relative, false)?;
            let mode = normalized_mode(&metadata, &relative)?;
            let sha256 = sha256_file(&path)?;
            files.push(SourceFile {
                entry: FileEntry {
                    path: relative,
                    sha256,
                    size: metadata.len(),
                    mode,
                },
                absolute: path,
            });
        } else {
            return Err(DeliveryError::UnsafeArchive(format!(
                "input `{}` is not a regular file or directory",
                relative_text(root, &path)?
            )));
        }
    }
    Ok(())
}

fn relative_text(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| DeliveryError::InvalidPackage("input path escaped its root".to_owned()))?;
    let text = relative.to_string_lossy();
    if text.contains('\\') {
        return Err(DeliveryError::UnsafeArchive(
            "input path contains a backslash on a Unix filesystem".to_owned(),
        ));
    }
    Ok(text.into_owned())
}

fn normalized_mode(metadata: &std::fs::Metadata, relative: &str) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o7777;
        if mode & 0o7022 != 0 {
            return Err(DeliveryError::UnsafeArchive(format!(
                "input `{relative}` has group/other-write or special permission bits"
            )));
        }
        Ok(if mode & 0o111 != 0 {
            "0755".to_owned()
        } else {
            "0644".to_owned()
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, relative);
        Ok("0644".to_owned())
    }
}

fn require_files(files: &[SourceFile]) -> Result<()> {
    let required = [
        "bin/wxml-lsp",
        "bin/wxml-type-backend/server.js",
        "bin/wxml-type-backend/node_modules/typescript/package.json",
        "bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json",
        "zed-extension/extension.wasm",
        "zed-extension/extension.toml",
    ];
    for path in required {
        if !files.iter().any(|file| file.entry.path == path) {
            return Err(DeliveryError::InvalidPackage(format!(
                "staged package is missing `{path}`"
            )));
        }
    }
    Ok(())
}

fn build_manifest(options: &BuildOptions, files: &[SourceFile]) -> Result<Manifest> {
    let entries: Vec<FileEntry> = files.iter().map(|file| file.entry.clone()).collect();
    let find = |path: &str| {
        entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| DeliveryError::InvalidPackage(format!("missing `{path}`")))
    };
    let mut binaries = vec![BinaryArtifact {
        kind: ArtifactKind::Rust,
        name: "wxml-lsp".to_owned(),
        path: "bin/wxml-lsp".to_owned(),
        version: options.release_version.clone(),
        protocol_version: None,
        sha256: find("bin/wxml-lsp")?.sha256.clone(),
    }];
    if let Ok(installer) = find("bin/wxml-lsp-install") {
        binaries.push(BinaryArtifact {
            kind: ArtifactKind::Rust,
            name: "wxml-lsp-install".to_owned(),
            path: "bin/wxml-lsp-install".to_owned(),
            version: options.release_version.clone(),
            protocol_version: None,
            sha256: installer.sha256.clone(),
        });
    }
    let backend = find("bin/wxml-type-backend/server.js")?;
    binaries.push(BinaryArtifact {
        kind: ArtifactKind::NodeBackend,
        name: "node-backend".to_owned(),
        path: backend.path.clone(),
        version: options.backend_version.clone(),
        protocol_version: Some(options.protocol_version),
        sha256: backend.sha256.clone(),
    });
    let extension = find("zed-extension/extension.wasm")?;
    binaries.push(BinaryArtifact {
        kind: ArtifactKind::ZedExtension,
        name: "zed-extension".to_owned(),
        path: extension.path.clone(),
        version: options
            .zed_version
            .clone()
            .unwrap_or_else(|| options.release_version.clone()),
        protocol_version: None,
        sha256: extension.sha256.clone(),
    });

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        package: PACKAGE_NAME.to_owned(),
        package_version: options.release_version.clone(),
        platform: options.platform.clone(),
        protocol_version: options.protocol_version,
        node: NodeInfo {
            minimum_version: options.node_minimum.clone(),
            backend_version: options.backend_version.clone(),
        },
        built_at: options.built_at.clone(),
        binaries,
        files: entries,
    };
    manifest.validate(ExpectedManifest {
        version: Some(&options.release_version),
        platform: Some(&options.platform),
        protocol_version: Some(options.protocol_version),
    })?;
    Ok(manifest)
}

fn copy_file(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let mut input = File::open(source).map_err(|error| DeliveryError::io(source, error))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| DeliveryError::io(destination, error))?;
    std::io::copy(&mut input, &mut output)
        .map_err(|error| DeliveryError::io(destination, error))?;
    output
        .flush()
        .map_err(|error| DeliveryError::io(destination, error))?;
    drop(output);
    set_mode(destination, mode)
}

fn update_index(
    output: &Path,
    manifest: &Manifest,
    sha256: &str,
    size: u64,
    force: bool,
) -> Result<()> {
    let path = output.join("index.json");
    let mut index = match std::fs::metadata(&path) {
        Ok(_) => match read_index(&path) {
            Ok(index) => index,
            Err(error) if force => {
                eprintln!("warning: replacing invalid index.json: {error}");
                Index::empty()
            }
            Err(error) => return Err(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Index::empty(),
        Err(error) => return Err(DeliveryError::io(&path, error)),
    };
    index.upsert(Release {
        version: manifest.package_version.clone(),
        platform: manifest.platform.clone(),
        protocol_version: manifest.protocol_version,
        node_minimum_version: manifest.node.minimum_version.clone(),
        archive: archive_name(&manifest.package_version, &manifest.platform)?,
        sha256: sha256.to_owned(),
        size,
    });
    index.validate()?;
    atomic_write(&path, &index.to_json()?, 0o644)
}

fn update_archive_checksums(output: &Path) -> Result<()> {
    let entries = std::fs::read_dir(output).map_err(|error| DeliveryError::io(output, error))?;
    let mut sums = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(output, error))?;
        let path = entry.path();
        if !path.is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("gz")
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("{PACKAGE_NAME}-")))
        {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DeliveryError::InvalidPackage("archive name is not UTF-8".to_owned()))?;
        sums.insert(name.to_owned(), sha256_file(&path)?);
    }
    atomic_write(&output.join("SHA256SUMS"), &format_sha256sums(&sums), 0o644)
}

/// Returns a manifest digest for external evidence.
///
/// # Errors
///
/// Returns an error when manifest serialization fails; a digest is never
/// replaced with a placeholder.
pub fn manifest_digest(manifest: &Manifest) -> Result<String> {
    Ok(sha256_bytes(&manifest.to_json()?))
}

/// Pinned SHA-256 of the upstream native engine binary (`engine/lib/tsc`).
///
/// This is the measured P1-A/registry fact for
/// `@typescript/typescript-linux-x64@7.1.0-beta`; a prepared tree must contain
/// these exact bytes before a package can be built.
pub const PINNED_ENGINE_BINARY_SHA256: &str =
    "a183b1956edbe7e7d98f2249858eedf2d8a9790809cc4909db1276e202faaa70";
/// Pinned size in bytes of the upstream native engine binary.
pub const PINNED_ENGINE_BINARY_SIZE: u64 = 24_522_912;

/// Package-relative provenance file generated by the v2 packager.
const GENERATED_PROVENANCE_PATH: &str = "provenance.json";

/// Settings for one deterministic schema v2 native package build.
#[derive(Clone, Debug)]
pub struct BuildOptionsV2 {
    /// Prepared package root containing the exact schema v2 tree, except for
    /// `engine/` and `typings/`, which must come from the verified asset cache.
    pub input: PathBuf,
    /// Directory that receives the versioned directory, archive and index.
    pub output: PathBuf,
    /// Repository `assets.lock.json`.
    pub assets_lock: Option<PathBuf>,
    /// Provisioned, content-addressed asset cache.
    pub assets_cache: Option<PathBuf>,
    /// Release version (strict semantic version).
    pub release_version: String,
    /// Rust-to-engine RPC/frame protocol version.
    pub protocol_version: u64,
    /// Build timestamp (UTC, RFC 3339).
    pub built_at: String,
    /// Replace an existing release directory, archive and index entry.
    pub force: bool,
    /// Validate and describe without writing anything.
    pub dry_run: bool,
}

/// Result of a successful schema v2 native package build.
#[derive(Clone, Debug)]
pub struct BuildReportV2 {
    /// Validated manifest.
    pub manifest: ManifestV2,
    /// Updated native index, when not a dry-run.
    pub index: Option<IndexV2>,
    /// Versioned directory, when not a dry-run.
    pub release_dir: Option<PathBuf>,
    /// Archive path, when not a dry-run.
    pub archive: Option<PathBuf>,
    /// Archive SHA-256.
    pub sha256: String,
    /// Archive size.
    pub size: u64,
    /// Archive SHA-512 SRI.
    pub archive_integrity: String,
    /// SHA-256 of the generated `manifest.json`.
    pub manifest_sha256: Option<String>,
}

impl BuildReportV2 {
    /// Serializes the generated manifest as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn manifest_json(&self) -> Result<Vec<u8>> {
        self.manifest.to_json()
    }
}

/// Builds a deterministic, Node-free schema v2 package from a prepared tree
/// plus a provisioned asset cache.
///
/// The input tree carries `bin/`, `zed-extension/` and the license documents;
/// `engine/` and `typings/` are accepted only from an already provisioned
/// cache that matches the submitted `assets.lock.json`. Every cache payload is
/// re-verified (file set, SHA-256, size, mode, tree digest and bundles) before
/// the manifest is built, and the manifest's `files[]`/`typeAssets[]` entries
/// are taken from the lock's verified payload table rather than the input tree.
/// `engine/lib/tsc` must additionally match the pinned upstream SHA-256/size.
///
/// The validated tree is hashed, copied into a versioned directory, written as
/// `manifest.json`/`SHA256SUMS`, archived with fixed ownership and timestamps,
/// re-extracted for a self-check, and recorded in a schema v2 `index.json`.
///
/// # Errors
///
/// Returns an error for a missing lock/cache, an unsafe input tree, an input
/// file below `engine/` or `typings/`, a missing/corrupt/foreign cache entry,
/// lock/cache drift, a wrong mode or digest, a Node runtime file, a mismatched
/// engine identity, an unsupported platform/protocol, an existing output
/// without `force`, or a post-build verification failure.
pub fn build_native_package(options: &BuildOptionsV2) -> Result<BuildReportV2> {
    validate_native_options(options)?;
    let input = std::fs::canonicalize(&options.input)
        .map_err(|error| DeliveryError::io(&options.input, error))?;
    if !input.is_dir() {
        return Err(DeliveryError::InvalidArgument(
            "input is not a directory".to_owned(),
        ));
    }
    let input_files = collect_native_files(&input)?;
    validate_native_input_files(&input, &input_files)?;
    let lock_path = options.assets_lock.as_deref().ok_or_else(|| {
        DeliveryError::InvalidArgument(
            "native package builds require --assets-lock <repo>/assets.lock.json".to_owned(),
        )
    })?;
    let cache = options.assets_cache.as_deref().ok_or_else(|| {
        DeliveryError::InvalidArgument(
            "native package builds require --cache <provisioned-asset-cache>".to_owned(),
        )
    })?;
    let assets = load_verified_assets(lock_path, cache)?;
    let files = assemble_native_files(input_files, &assets)?;
    validate_engine_identity(&files)?;
    validate_pinned_engine_binary(&files)?;
    let (provenance, provenance_entry) = native_provenance_bytes(options, &files, &assets)?;
    let mut entries: Vec<FileEntry> = files.iter().map(|file| file.entry.clone()).collect();
    entries.push(provenance_entry);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = build_native_manifest(options, &input, entries, &assets)?;

    if options.dry_run {
        return Ok(BuildReportV2 {
            manifest,
            index: None,
            release_dir: None,
            archive: None,
            sha256: String::new(),
            size: 0,
            archive_integrity: String::new(),
            manifest_sha256: None,
        });
    }

    let (output, paths) = prepare_native_output(&input, options)?;
    let mut index = load_native_index(&output, options.force)?;
    let mut stage = NativeStage::create(&output)?;

    let tree = stage.tree();
    std::fs::create_dir(&tree).map_err(|error| DeliveryError::io(&tree, error))?;
    set_mode(&tree, 0o755)?;
    write_native_tree(&tree, &files, &provenance, &manifest)?;
    let archive = write_native_archive(&stage.archive(), &tree, &manifest)?;
    self_verify_native_archive(&stage.verification_dir(), &archive.path, &manifest)?;

    index.upsert(native_release_entry(&manifest, &archive)?);
    index.validate()?;
    atomic_write(&stage.index(), &index.to_json()?, 0o644)?;
    atomic_write(
        &stage.sidecar(),
        format!("{}  {}\n", archive.sha256, paths.archive_name).as_bytes(),
        0o644,
    )?;
    atomic_write(
        &stage.sums(),
        &native_archive_sums(&output, &paths.archive_name, &archive.sha256)?,
        0o644,
    )?;
    let manifest_sha256 = sha256_file(&tree.join("manifest.json"))?;
    let items = native_commit_items(&stage, &paths);
    commit_native_stage(&mut stage, &items, |_| Ok(()))?;

    Ok(BuildReportV2 {
        manifest,
        index: Some(index),
        release_dir: Some(paths.release_dir),
        archive: Some(paths.archive),
        sha256: archive.sha256,
        size: archive.size,
        archive_integrity: archive.integrity,
        manifest_sha256: Some(manifest_sha256),
    })
}

fn validate_native_options(options: &BuildOptionsV2) -> Result<()> {
    validate_strict(&options.release_version)?;
    if !SUPPORTED_NATIVE_PROTOCOL_VERSIONS.contains(&options.protocol_version) {
        return Err(DeliveryError::InvalidArgument(format!(
            "native protocol {} is not supported by this packager",
            options.protocol_version
        )));
    }
    crate::state::validate_rfc3339_utc(&options.built_at)?;
    Ok(())
}

/// One lock asset with its fully re-verified cache payload.
struct VerifiedAsset {
    asset: Asset,
    entry: CacheEntry,
}

/// The two (or more) locked assets consumed by one native build.
struct VerifiedAssets {
    lock_sha256: String,
    assets: Vec<VerifiedAsset>,
}

impl VerifiedAssets {
    fn get(&self, name: &str) -> Result<&VerifiedAsset> {
        self.assets
            .iter()
            .find(|verified| verified.asset.name == name)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!("assets lock is missing the `{name}` asset"))
            })
    }
}

fn load_verified_assets(lock_path: &Path, cache: &Path) -> Result<VerifiedAssets> {
    let document = read_assets_lock(lock_path)?;
    let mut assets = Vec::with_capacity(document.lock.assets.len());
    for asset in &document.lock.assets {
        let entry = verify_cached_asset(asset, cache, &document.lock_sha256, &document.lock_bytes)?;
        assets.push(VerifiedAsset {
            asset: asset.clone(),
            entry,
        });
    }
    if assets.is_empty() {
        return Err(DeliveryError::InvalidPackage(
            "assets lock declares no assets".to_owned(),
        ));
    }
    Ok(VerifiedAssets {
        lock_sha256: document.lock_sha256,
        assets,
    })
}

/// Required non-engine input files, executable allowlist and Zed version.
fn validate_native_input_files(input: &Path, files: &[SourceFile]) -> Result<()> {
    for required in [
        "bin/wxml-lsp",
        "bin/wxml-lsp-install",
        "zed-extension/extension.wasm",
        "zed-extension/extension.toml",
        "THIRD_PARTY_LICENSES.md",
    ] {
        if !files.iter().any(|file| file.entry.path == required) {
            return Err(DeliveryError::InvalidPackage(format!(
                "staged package is missing `{required}`"
            )));
        }
    }
    for file in files {
        if file.entry.mode == "0755"
            && !crate::manifest_v2::ALLOWED_EXECUTABLES.contains(&file.entry.path.as_str())
        {
            return Err(DeliveryError::InvalidPackage(format!(
                "payload `{}` is executable but not in the allowlist",
                file.entry.path
            )));
        }
    }
    for path in ["bin/wxml-lsp", "bin/wxml-lsp-install"] {
        let file = files
            .iter()
            .find(|file| file.entry.path == path)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!("staged package is missing `{path}`"))
            })?;
        if file.entry.mode != "0755" {
            return Err(DeliveryError::InvalidPackage(format!(
                "`{path}` must have mode 0755"
            )));
        }
    }
    zed_extension_version(&input.join("zed-extension/extension.toml"))?;
    Ok(())
}

/// Merges the input files with the lock's verified cache payloads.
///
/// The final `engine/` and `typings/` file sets must equal the lock payload
/// tables exactly; unmatched or extra cache bytes never reach the manifest.
fn assemble_native_files(
    input_files: Vec<SourceFile>,
    assets: &VerifiedAssets,
) -> Result<Vec<SourceFile>> {
    let mut files = input_files;
    let mut seen: BTreeSet<String> = files.iter().map(|file| file.entry.path.clone()).collect();
    let mut cache_files = Vec::new();
    for verified in &assets.assets {
        let payload_root = verified.entry.directory.join("payload");
        for locked in &verified.asset.payload.files {
            let source = payload_root.join(&locked.path);
            if !seen.insert(locked.path.clone()) {
                return Err(DeliveryError::Integrity(format!(
                    "packaged bytes repeat `{}` across the input and assets.lock.json",
                    locked.path
                )));
            }
            let metadata = std::fs::symlink_metadata(&source)
                .map_err(|error| DeliveryError::io(&source, error))?;
            if !metadata.file_type().is_file() {
                return Err(DeliveryError::Integrity(format!(
                    "cached payload `{}` is not a regular file",
                    locked.path
                )));
            }
            if metadata.len() != locked.size {
                return Err(DeliveryError::Integrity(format!(
                    "cached payload `{}` size does not match assets.lock.json",
                    locked.path
                )));
            }
            cache_files.push(SourceFile {
                entry: locked.clone(),
                absolute: source,
            });
        }
    }
    verify_lock_asset_roots(&files, &cache_files, assets)?;
    files.extend(cache_files);
    files.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));
    Ok(files)
}

fn verify_lock_asset_roots(
    input_files: &[SourceFile],
    cache_files: &[SourceFile],
    assets: &VerifiedAssets,
) -> Result<()> {
    for verified in &assets.assets {
        let prefix = format!("{}/", verified.asset.package_root);
        let expected: BTreeMap<&str, &FileEntry> = verified
            .asset
            .payload
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        let mut actual = BTreeMap::new();
        for file in cache_files {
            if file.entry.path.starts_with(&prefix) {
                actual.insert(file.entry.path.as_str(), &file.entry);
            }
        }
        if actual != expected {
            return Err(DeliveryError::Integrity(format!(
                "packaged bytes do not match assets.lock.json for `{}`",
                verified.asset.name
            )));
        }
        if input_files
            .iter()
            .any(|file| file.entry.path.starts_with(&prefix))
        {
            return Err(DeliveryError::Integrity(format!(
                "input files must not supply `{}` bytes; they come from the asset cache",
                prefix.trim_end_matches('/')
            )));
        }
    }
    Ok(())
}

fn native_expected(manifest: &ManifestV2) -> ExpectedManifestV2<'_> {
    ExpectedManifestV2 {
        version: Some(&manifest.package_version),
        platform: Some(&manifest.platform),
        protocol_version: Some(manifest.protocol_version),
    }
}

struct NativeFinalPaths {
    release_name: String,
    archive_name: String,
    release_dir: PathBuf,
    archive: PathBuf,
    sidecar: PathBuf,
    index: PathBuf,
    sums: PathBuf,
}

fn prepare_native_output(
    input: &Path,
    options: &BuildOptionsV2,
) -> Result<(PathBuf, NativeFinalPaths)> {
    std::fs::create_dir_all(&options.output)
        .map_err(|error| DeliveryError::io(&options.output, error))?;
    let output = std::fs::canonicalize(&options.output)
        .map_err(|error| DeliveryError::io(&options.output, error))?;
    if output.starts_with(input) || input.starts_with(&output) {
        return Err(DeliveryError::InvalidArgument(
            "input and output directories must not contain each other".to_owned(),
        ));
    }
    let release_name = format!(
        "{PACKAGE_NAME}-{}-{}",
        options.release_version, NATIVE_PACKAGE_PLATFORM
    );
    let archive_name = archive_name(&options.release_version, NATIVE_PACKAGE_PLATFORM)?;
    let paths = NativeFinalPaths {
        release_dir: output.join(&release_name),
        archive: output.join(&archive_name),
        sidecar: output.join(format!("{archive_name}.sha256")),
        index: output.join("index.json"),
        sums: output.join("SHA256SUMS"),
        release_name,
        archive_name,
    };
    if path_present(&paths.release_dir) {
        if !options.force {
            return Err(DeliveryError::InstallState(format!(
                "release directory `{}` already exists; use --force to replace it",
                paths.release_name
            )));
        }
        require_existing_directory(&paths.release_dir, "release directory")?;
    }
    if path_present(&paths.archive) {
        if !options.force {
            return Err(DeliveryError::InstallState(
                "release archive already exists; use --force to replace it".to_owned(),
            ));
        }
        require_existing_file(&paths.archive, "release archive")?;
    }
    require_optional_file(&paths.sidecar, "archive checksum")?;
    require_optional_file(&paths.index, "index.json")?;
    require_optional_file(&paths.sums, "SHA256SUMS")?;
    Ok((output, paths))
}

fn path_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn require_existing_directory(path: &Path, label: &str) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(DeliveryError::InstallState(format!(
            "existing {label} is not a real directory"
        )));
    }
    Ok(())
}

fn require_existing_file(path: &Path, label: &str) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(DeliveryError::InstallState(format!(
            "existing {label} is not a regular file"
        )));
    }
    Ok(())
}

fn require_optional_file(path: &Path, label: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => require_existing_file(path, label),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeliveryError::io(path, error)),
    }
}

struct NativeStage {
    root: PathBuf,
    preserve: bool,
}

impl NativeStage {
    fn create(output: &Path) -> Result<Self> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = output.join(format!(".native-stage-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).map_err(|error| DeliveryError::io(&root, error))?;
        set_mode(&root, 0o700)?;
        Ok(Self {
            root,
            preserve: false,
        })
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn tree(&self) -> PathBuf {
        self.path("tree")
    }

    fn archive(&self) -> PathBuf {
        self.path("archive.tar.gz")
    }

    fn sidecar(&self) -> PathBuf {
        self.path("archive.sha256")
    }

    fn index(&self) -> PathBuf {
        self.path("index.json")
    }

    fn sums(&self) -> PathBuf {
        self.path("SHA256SUMS")
    }

    fn verification_dir(&self) -> PathBuf {
        self.path("verify")
    }

    const fn keep(&mut self) {
        self.preserve = true;
    }
}

impl Drop for NativeStage {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

fn load_native_index(output: &Path, force: bool) -> Result<IndexV2> {
    let path = output.join("index.json");
    match std::fs::metadata(&path) {
        Ok(_) => match read_index_v2(&path) {
            Ok(index) => Ok(index),
            Err(error) if force => {
                eprintln!("warning: replacing invalid index.json: {error}");
                Ok(IndexV2::empty())
            }
            Err(error) => Err(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(IndexV2::empty()),
        Err(error) => Err(DeliveryError::io(&path, error)),
    }
}

fn native_release_entry(manifest: &ManifestV2, archive: &NativeArchive) -> Result<ReleaseV2> {
    Ok(ReleaseV2 {
        version: manifest.package_version.clone(),
        platform: manifest.platform.clone(),
        protocol_version: manifest.protocol_version,
        engine_version: ENGINE_VERSION.to_owned(),
        engine_source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        engine_protocol_version: manifest.protocol_version,
        engine_api_schema_version: ENGINE_API_SCHEMA_VERSION,
        engine_ast_header_bytes: ENGINE_AST_HEADER_BYTES,
        engine_distribution: ENGINE_DISTRIBUTION.to_owned(),
        engine_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        min_installer_schema: MAX_INSTALLER_SCHEMA,
        archive: archive_name(&manifest.package_version, &manifest.platform)?,
        archive_integrity: archive.integrity.clone(),
        sha256: archive.sha256.clone(),
        size: archive.size,
    })
}

fn native_archive_sums(output: &Path, new_name: &str, new_sha256: &str) -> Result<Vec<u8>> {
    let mut sums = BTreeMap::new();
    let entries = std::fs::read_dir(output).map_err(|error| DeliveryError::io(output, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(output, error))?;
        let path = entry.path();
        if !std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file())
            || path.extension().and_then(|extension| extension.to_str()) != Some("gz")
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("{PACKAGE_NAME}-")))
        {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DeliveryError::InvalidPackage("archive name is not UTF-8".to_owned()))?;
        if name == new_name {
            continue;
        }
        sums.insert(name.to_owned(), sha256_file(&path)?);
    }
    sums.insert(new_name.to_owned(), new_sha256.to_owned());
    Ok(format_sha256sums(&sums))
}

fn collect_native_files(root: &Path) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    collect_native_inner(root, root, &mut files, &mut 0)?;
    files.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));
    Ok(files)
}

fn collect_native_inner(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SourceFile>,
    directories: &mut usize,
) -> Result<()> {
    *directories += 1;
    if *directories > MAX_FILES {
        return Err(DeliveryError::SizeLimit(
            "input tree contains too many directories".to_owned(),
        ));
    }
    let entries =
        std::fs::read_dir(directory).map_err(|error| DeliveryError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(directory, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(DeliveryError::UnsafeArchive(format!(
                "input `{}` is a symbolic link; native packages must be self-contained",
                native_relative(root, &path)?
            )));
        }
        let relative = native_relative(root, &path)?;
        if is_locked_asset_root(&relative) {
            return Err(DeliveryError::InvalidArgument(format!(
                "input `{relative}`: engine/typings must come from the verified asset cache \
                 (pass --assets-lock and --cache); the input tree must not supply them"
            )));
        }
        if metadata.file_type().is_dir() {
            collect_native_inner(root, &path, files, directories)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(DeliveryError::UnsafeArchive(format!(
                "input `{relative}` is not a regular file or directory"
            )));
        }
        validate_relative_path(&relative, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() > 1 {
                return Err(DeliveryError::UnsafeArchive(format!(
                    "input `{relative}` is a hard link; native packages must contain independent files"
                )));
            }
        }
        if relative == GENERATED_PROVENANCE_PATH {
            return Err(DeliveryError::InvalidPackage(format!(
                "`{GENERATED_PROVENANCE_PATH}` is generated by the packager; the prepared tree must not supply it"
            )));
        }
        if let Some(reason) = forbidden_runtime_path(&relative) {
            return Err(DeliveryError::InvalidPackage(format!(
                "input `{relative}` is a Node runtime file: {reason}"
            )));
        }
        if !is_allowed_native_path(&relative) {
            return Err(DeliveryError::InvalidPackage(format!(
                "input `{relative}` is not part of the schema v2 package layout"
            )));
        }
        if files.len() >= MAX_FILES {
            return Err(DeliveryError::SizeLimit(format!(
                "input tree contains more than {MAX_FILES} files"
            )));
        }
        if metadata.len() == 0 {
            return Err(DeliveryError::InvalidPackage(format!(
                "payload `{relative}` is empty"
            )));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "input `{relative}` exceeds the per-file limit"
            )));
        }
        let mode = normalized_mode(&metadata, &relative)?;
        let sha256 = sha256_file(&path)?;
        files.push(SourceFile {
            entry: FileEntry {
                path: relative,
                sha256,
                size: metadata.len(),
                mode,
            },
            absolute: path,
        });
    }
    Ok(())
}

fn native_relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| DeliveryError::InvalidPackage("input path escaped its root".to_owned()))?;
    let text = relative.to_str().ok_or_else(|| {
        DeliveryError::InvalidPackage("input tree contains a non-UTF-8 path".to_owned())
    })?;
    if text.contains('\\') {
        return Err(DeliveryError::UnsafeArchive(
            "input path contains a backslash on a Unix filesystem".to_owned(),
        ));
    }
    Ok(text.to_owned())
}

fn is_locked_asset_root(path: &str) -> bool {
    path == ENGINE_ASSET_ROOT
        || path
            .strip_prefix(ENGINE_ASSET_ROOT)
            .is_some_and(|rest| rest.starts_with('/'))
        || path == "typings"
        || path
            .strip_prefix("typings")
            .is_some_and(|rest| rest.starts_with('/'))
}

fn is_allowed_native_path(path: &str) -> bool {
    if matches!(
        path,
        "bin/wxml-lsp"
            | "bin/wxml-lsp-install"
            | "engine/lib/tsc"
            | "engine/lib/tsc.sig"
            | "engine/LICENSE"
            | "engine/NOTICE.txt"
            | "engine/package.json"
            | "engine/README.md"
            | "THIRD_PARTY_LICENSES.md"
            | "zed-extension/extension.wasm"
            | "zed-extension/extension.toml"
            | "zed-extension/grammars/wxml.wasm"
    ) {
        return true;
    }
    if let Some(name) = path.strip_prefix("engine/lib/") {
        return !name.contains('/') && name.starts_with("lib") && name.ends_with(".d.ts");
    }
    if is_miniprogram_typings_path(path) {
        return is_allowed_miniprogram_typings_path(path);
    }
    path.strip_prefix("zed-extension/languages/")
        .is_some_and(|relative| !relative.is_empty())
}

fn validate_engine_identity(files: &[SourceFile]) -> Result<()> {
    let engine = files
        .iter()
        .find(|file| file.entry.path == "engine/package.json")
        .ok_or_else(|| {
            DeliveryError::InvalidPackage(
                "verified asset payload is missing `engine/package.json`".to_owned(),
            )
        })?;
    let bytes = read_limited(&engine.absolute, MAX_MANIFEST_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("engine/package.json is malformed: {error}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        DeliveryError::InvalidPackage("engine/package.json must be a JSON object".to_owned())
    })?;
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage("engine/package.json is missing `name`".to_owned())
        })?;
    if name != ENGINE_DISTRIBUTION {
        return Err(DeliveryError::InvalidPackage(format!(
            "engine/package.json name `{name}` does not match `{ENGINE_DISTRIBUTION}`"
        )));
    }
    let version = object
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage("engine/package.json is missing `version`".to_owned())
        })?;
    if version != ENGINE_VERSION {
        return Err(DeliveryError::InvalidPackage(format!(
            "engine/package.json version `{version}` does not match `{ENGINE_VERSION}`"
        )));
    }
    let git_head = object
        .get("gitHead")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage("engine/package.json is missing `gitHead`".to_owned())
        })?;
    if git_head != ENGINE_SOURCE_COMMIT {
        return Err(DeliveryError::InvalidPackage(format!(
            "engine/package.json gitHead `{git_head}` does not match `{ENGINE_SOURCE_COMMIT}`"
        )));
    }
    Ok(())
}

fn validate_pinned_engine_binary(files: &[SourceFile]) -> Result<()> {
    let engine = files
        .iter()
        .find(|file| file.entry.path == ENGINE_BINARY_PATH)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage(format!(
                "staged package is missing `{ENGINE_BINARY_PATH}`"
            ))
        })?;
    if engine.entry.size != PINNED_ENGINE_BINARY_SIZE {
        return Err(DeliveryError::Integrity(format!(
            "`{ENGINE_BINARY_PATH}` size {} does not match the pinned expected {PINNED_ENGINE_BINARY_SIZE}",
            engine.entry.size
        )));
    }
    if engine.entry.sha256 != PINNED_ENGINE_BINARY_SHA256 {
        return Err(DeliveryError::Integrity(format!(
            "`{ENGINE_BINARY_PATH}` SHA-256 `{}` does not match the pinned expected `{PINNED_ENGINE_BINARY_SHA256}`",
            engine.entry.sha256
        )));
    }
    if engine.entry.mode != "0755" {
        return Err(DeliveryError::InvalidPackage(
            "pinned engine binary must have mode 0755".to_owned(),
        ));
    }
    Ok(())
}

fn zed_extension_version(path: &Path) -> Result<String> {
    let bytes = read_limited(path, MAX_MANIFEST_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        DeliveryError::InvalidPackage("zed-extension/extension.toml is not UTF-8".to_owned())
    })?;
    let mut version = None;
    let mut in_section = false;
    for line in text.lines() {
        let line = line
            .split_once('#')
            .map_or(line, |(content, _)| content)
            .trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_section = true;
            continue;
        }
        if in_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "version" {
            continue;
        }
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            return Err(DeliveryError::InvalidPackage(
                "zed-extension/extension.toml `version` must be a quoted string".to_owned(),
            ));
        };
        if version.is_some() {
            return Err(DeliveryError::InvalidPackage(
                "zed-extension/extension.toml declares `version` more than once".to_owned(),
            ));
        }
        version = Some(value.to_owned());
    }
    let version = version.ok_or_else(|| {
        DeliveryError::InvalidPackage(
            "zed-extension/extension.toml is missing top-level `version`".to_owned(),
        )
    })?;
    validate_strict(&version)?;
    Ok(version)
}

fn native_provenance_bytes(
    options: &BuildOptionsV2,
    files: &[SourceFile],
    assets: &VerifiedAssets,
) -> Result<(Vec<u8>, FileEntry)> {
    let engine = files
        .iter()
        .find(|file| file.entry.path == ENGINE_BINARY_PATH)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage(format!(
                "staged package is missing `{ENGINE_BINARY_PATH}`"
            ))
        })?;
    let mut asset_values = Vec::with_capacity(assets.assets.len());
    for verified in &assets.assets {
        asset_values.push(serde_json::json!({
            "name": verified.asset.name.as_str(),
            "version": verified.asset.version.as_str(),
            "license": verified.asset.license.as_str(),
            "packageRoot": verified.asset.package_root.as_str(),
            "payloadTreeDigest": verified.asset.payload.tree_digest.as_str(),
            "fileCount": verified.asset.payload.file_count,
            "totalBytes": verified.asset.payload.total_bytes,
            "tarball": {
                "integrity": verified.asset.tarball.integrity.as_str(),
                "sha256": verified.asset.tarball.sha256.as_str(),
                "size": verified.asset.tarball.size,
            },
        }));
    }
    asset_values.sort_by(|left, right| {
        left.get("name")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("name").and_then(serde_json::Value::as_str))
    });
    let value = serde_json::json!({
        "schemaVersion": 1,
        "package": PACKAGE_NAME,
        "packageVersion": options.release_version.as_str(),
        "assetsLockSha256": assets.lock_sha256.as_str(),
        "assets": asset_values,
        "engine": {
            "name": ENGINE_NAME,
            "version": ENGINE_VERSION,
            "sourceCommit": ENGINE_SOURCE_COMMIT,
            "binary": {
                "path": ENGINE_BINARY_PATH,
                "sha256": engine.entry.sha256.as_str(),
                "size": engine.entry.size,
                "mode": engine.entry.mode.as_str(),
                "measurement": "measured"
            },
            "distribution": {
                "integrity": ENGINE_DISTRIBUTION_INTEGRITY,
                "sha256": ENGINE_DISTRIBUTION_SHA256,
                "size": ENGINE_DISTRIBUTION_SIZE,
                "fileCount": ENGINE_DISTRIBUTION_FILE_COUNT,
                "unpackedSize": ENGINE_DISTRIBUTION_UNPACKED_SIZE,
                "measurement": "pinned-expected"
            }
        }
    });
    let mut bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| DeliveryError::InvalidPackage(error.to_string()))?;
    bytes.push(b'\n');
    let size = u64::try_from(bytes.len())
        .map_err(|_| DeliveryError::SizeLimit("provenance size overflow".to_owned()))?;
    if size > MAX_FILE_BYTES {
        return Err(DeliveryError::SizeLimit(
            "generated provenance exceeds the per-file limit".to_owned(),
        ));
    }
    let entry = FileEntry {
        path: GENERATED_PROVENANCE_PATH.to_owned(),
        sha256: sha256_bytes(&bytes),
        size,
        mode: "0644".to_owned(),
    };
    Ok((bytes, entry))
}

fn build_native_manifest(
    options: &BuildOptionsV2,
    input: &Path,
    entries: Vec<FileEntry>,
    assets: &VerifiedAssets,
) -> Result<ManifestV2> {
    let standard = native_standard_lib(&entries)?;
    let typings = native_typings(&entries)?;
    verify_lock_bundles(assets, &standard, &typings)?;
    let zed_version = zed_extension_version(&input.join("zed-extension/extension.toml"))?;
    let shas = native_artifact_shas(&entries)?;
    let manifest = ManifestV2 {
        schema_version: MANIFEST_SCHEMA_V2,
        package: PACKAGE_NAME.to_owned(),
        package_version: options.release_version.clone(),
        platform: NATIVE_PACKAGE_PLATFORM.to_owned(),
        protocol_version: options.protocol_version,
        built_at: options.built_at.clone(),
        engine: native_engine_info(),
        type_assets: native_type_assets(standard, typings),
        binaries: native_binaries(options, shas, zed_version),
        files: entries,
    };
    manifest.validate(native_expected(&manifest))?;
    Ok(manifest)
}

fn verify_lock_bundles(
    assets: &VerifiedAssets,
    standard: &TypeAssetBundle,
    typings: &TypeAssetBundle,
) -> Result<()> {
    let engine = assets.get(ENGINE_ASSET_NAME)?;
    if engine.asset.package_root != ENGINE_ASSET_ROOT {
        return Err(DeliveryError::Integrity(
            "engine asset packageRoot does not match the frozen `engine` root".to_owned(),
        ));
    }
    verify_lock_bundle(&engine.asset, STANDARD_LIB_ASSET, standard)?;
    let typings_asset = assets.get(TYPINGS_ASSET_NAME)?;
    if typings_asset.asset.package_root != MINIPROGRAM_TYPINGS_ROOT {
        return Err(DeliveryError::Integrity(
            "typings asset packageRoot does not match the frozen typings root".to_owned(),
        ));
    }
    verify_lock_bundle(&typings_asset.asset, MINIPROGRAM_TYPINGS_ASSET, typings)
}

fn verify_lock_bundle(asset: &Asset, name: &str, computed: &TypeAssetBundle) -> Result<()> {
    let locked = asset
        .payload
        .bundles
        .iter()
        .find(|bundle| bundle.name == name)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage(format!(
                "assets lock is missing the `{name}` payload bundle"
            ))
        })?;
    if locked.file_count != computed.file_count
        || locked.total_bytes != computed.total_bytes
        || locked.sha256 != computed.sha256
    {
        return Err(DeliveryError::Integrity(format!(
            "type asset bundle `{name}` does not match assets.lock.json"
        )));
    }
    Ok(())
}

fn native_standard_lib(entries: &[FileEntry]) -> Result<TypeAssetBundle> {
    let bundle =
        compute_type_asset_bundle(entries, "engine/lib", &[STANDARD_LIB_INCLUDE.to_owned()])?;
    if bundle.file_count != STANDARD_LIB_FILE_COUNT {
        return Err(DeliveryError::InvalidPackage(format!(
            "engine/lib must contain exactly {STANDARD_LIB_FILE_COUNT} `lib*.d.ts` files, found {}",
            bundle.file_count
        )));
    }
    Ok(bundle)
}

fn native_typings(entries: &[FileEntry]) -> Result<TypeAssetBundle> {
    let bundle = compute_type_asset_bundle(entries, MINIPROGRAM_TYPINGS_ROOT, &["**".to_owned()])?;
    if bundle.file_count != MINIPROGRAM_TYPINGS_FILE_COUNT {
        return Err(DeliveryError::InvalidPackage(format!(
            "`{MINIPROGRAM_TYPINGS_ROOT}` must contain exactly {MINIPROGRAM_TYPINGS_FILE_COUNT} files, found {}",
            bundle.file_count
        )));
    }
    if bundle.total_bytes != MINIPROGRAM_TYPINGS_TOTAL_BYTES
        || bundle.sha256 != MINIPROGRAM_TYPINGS_BUNDLE_DIGEST
    {
        return Err(DeliveryError::InvalidPackage(format!(
            "`{MINIPROGRAM_TYPINGS_ROOT}` payload does not match the frozen contract: \
             expected {MINIPROGRAM_TYPINGS_FILE_COUNT} files/{MINIPROGRAM_TYPINGS_TOTAL_BYTES} B/\
             {MINIPROGRAM_TYPINGS_BUNDLE_DIGEST}, found {} files/{} B/{}",
            bundle.file_count, bundle.total_bytes, bundle.sha256
        )));
    }
    Ok(bundle)
}

struct NativeArtifactShas {
    lsp: String,
    installer: String,
    engine: String,
    zed: String,
}

fn native_artifact_shas(entries: &[FileEntry]) -> Result<NativeArtifactShas> {
    let find = |path: &str| {
        entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!("staged package is missing `{path}`"))
            })
    };
    Ok(NativeArtifactShas {
        lsp: find("bin/wxml-lsp")?.sha256.clone(),
        installer: find("bin/wxml-lsp-install")?.sha256.clone(),
        engine: find(ENGINE_BINARY_PATH)?.sha256.clone(),
        zed: find("zed-extension/extension.wasm")?.sha256.clone(),
    })
}

fn native_engine_info() -> EngineInfo {
    // `distribution*` fields describe the frozen upstream tarball contract
    // ("pinned expected"). They are not measured from a local tarball during
    // this build; only per-file SHA-256/size entries are measured from the
    // prepared tree, and `engine/lib/tsc` is checked against its pinned digest.
    EngineInfo {
        name: ENGINE_NAME.to_owned(),
        version: ENGINE_VERSION.to_owned(),
        source_commit: ENGINE_SOURCE_COMMIT.to_owned(),
        distribution: ENGINE_DISTRIBUTION.to_owned(),
        distribution_url: ENGINE_DISTRIBUTION_URL.to_owned(),
        distribution_integrity: ENGINE_DISTRIBUTION_INTEGRITY.to_owned(),
        distribution_sha256: ENGINE_DISTRIBUTION_SHA256.to_owned(),
        distribution_size: ENGINE_DISTRIBUTION_SIZE,
        distribution_file_count: ENGINE_DISTRIBUTION_FILE_COUNT,
        distribution_unpacked_size: ENGINE_DISTRIBUTION_UNPACKED_SIZE,
        api_schema_version: ENGINE_API_SCHEMA_VERSION,
        ast_header_bytes: ENGINE_AST_HEADER_BYTES,
        license: ENGINE_LICENSE.to_owned(),
        license_files: REQUIRED_ENGINE_LICENSE_FILES
            .iter()
            .map(|path| (*path).to_owned())
            .collect(),
        binary: ENGINE_BINARY_PATH.to_owned(),
        probe: EngineProbe {
            args: vec!["--version".to_owned()],
            expect_prefix: ENGINE_PROBE_PREFIX.to_owned(),
        },
    }
}

fn native_type_assets(standard: TypeAssetBundle, typings: TypeAssetBundle) -> Vec<TypeAsset> {
    vec![
        TypeAsset {
            name: STANDARD_LIB_ASSET.to_owned(),
            version: ENGINE_VERSION.to_owned(),
            license: ENGINE_LICENSE.to_owned(),
            root: "engine/lib".to_owned(),
            include: vec![STANDARD_LIB_INCLUDE.to_owned()],
            file_count: standard.file_count,
            sha256: standard.sha256,
            license_files: REQUIRED_ENGINE_LICENSE_FILES
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
        },
        TypeAsset {
            name: MINIPROGRAM_TYPINGS_ASSET.to_owned(),
            version: MINIPROGRAM_TYPINGS_VERSION.to_owned(),
            license: MINIPROGRAM_TYPINGS_LICENSE.to_owned(),
            root: MINIPROGRAM_TYPINGS_ROOT.to_owned(),
            include: vec!["**".to_owned()],
            file_count: typings.file_count,
            sha256: typings.sha256,
            license_files: vec![format!("{MINIPROGRAM_TYPINGS_ROOT}/LICENSE")],
        },
    ]
}

fn native_binaries(
    options: &BuildOptionsV2,
    shas: NativeArtifactShas,
    zed_version: String,
) -> Vec<NativeBinaryArtifact> {
    vec![
        NativeBinaryArtifact {
            kind: NativeArtifactKind::Rust,
            name: "wxml-lsp".to_owned(),
            path: "bin/wxml-lsp".to_owned(),
            version: options.release_version.clone(),
            protocol_version: None,
            sha256: shas.lsp,
        },
        NativeBinaryArtifact {
            kind: NativeArtifactKind::Rust,
            name: "wxml-lsp-install".to_owned(),
            path: "bin/wxml-lsp-install".to_owned(),
            version: options.release_version.clone(),
            protocol_version: None,
            sha256: shas.installer,
        },
        NativeBinaryArtifact {
            kind: NativeArtifactKind::NativeEngine,
            name: "tsgo".to_owned(),
            path: ENGINE_BINARY_PATH.to_owned(),
            version: ENGINE_VERSION.to_owned(),
            protocol_version: Some(options.protocol_version),
            sha256: shas.engine,
        },
        NativeBinaryArtifact {
            kind: NativeArtifactKind::ZedExtension,
            name: "zed-extension".to_owned(),
            path: "zed-extension/extension.wasm".to_owned(),
            version: zed_version,
            protocol_version: None,
            sha256: shas.zed,
        },
    ]
}

fn write_native_tree(
    release_dir: &Path,
    files: &[SourceFile],
    provenance: &[u8],
    manifest: &ManifestV2,
) -> Result<()> {
    for file in files {
        let destination = release_dir.join(&file.entry.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
        }
        copy_file(&file.absolute, &destination, file.entry.mode_bits()?)?;
    }
    atomic_write(
        &release_dir.join(GENERATED_PROVENANCE_PATH),
        provenance,
        0o644,
    )?;
    atomic_write(
        &release_dir.join("manifest.json"),
        &manifest.to_json()?,
        0o644,
    )?;
    let checksums = checksums_for_tree_v2(release_dir, manifest)?;
    atomic_write(&release_dir.join("SHA256SUMS"), &checksums, 0o644)?;
    normalize_native_directory_modes(release_dir)?;
    verify_tree_v2(release_dir, native_expected(manifest))?;
    Ok(())
}

fn normalize_native_directory_modes(root: &Path) -> Result<()> {
    set_mode(root, 0o755)?;
    let entries = std::fs::read_dir(root).map_err(|error| DeliveryError::io(root, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DeliveryError::io(root, error))?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        if metadata.file_type().is_dir() {
            normalize_native_directory_modes(&path)?;
        }
    }
    Ok(())
}

struct NativeArchive {
    path: PathBuf,
    sha256: String,
    size: u64,
    integrity: String,
}

fn write_native_archive(
    archive_path: &Path,
    release_dir: &Path,
    manifest: &ManifestV2,
) -> Result<NativeArchive> {
    let mut archive_entries = manifest.files.clone();
    for name in ["manifest.json", "SHA256SUMS"] {
        let path = release_dir.join(name);
        let metadata = std::fs::metadata(&path).map_err(|error| DeliveryError::io(&path, error))?;
        archive_entries.push(FileEntry {
            path: name.to_owned(),
            sha256: sha256_file(&path)?,
            size: metadata.len(),
            mode: "0644".to_owned(),
        });
    }
    archive_entries.sort_by(|left, right| left.path.cmp(&right.path));
    create_tar_gz(release_dir, archive_path, &archive_entries, 0)?;
    let sha256 = sha256_file(archive_path)?;
    let size = std::fs::metadata(archive_path)
        .map_err(|error| DeliveryError::io(archive_path, error))?
        .len();
    if size == 0 || size > MAX_ARCHIVE_BYTES {
        return Err(DeliveryError::SizeLimit(
            "generated archive is empty or exceeds the size limit".to_owned(),
        ));
    }
    let integrity = sha512_sri_file(archive_path)?.to_sri();
    Ok(NativeArchive {
        path: archive_path.to_path_buf(),
        sha256,
        size,
        integrity,
    })
}

fn self_verify_native_archive(scratch: &Path, archive: &Path, manifest: &ManifestV2) -> Result<()> {
    let _ = std::fs::remove_dir_all(scratch);
    let mut allowed: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    allowed.insert("manifest.json".to_owned());
    allowed.insert("SHA256SUMS".to_owned());
    let verified = extract_tar_gz_allowlisted(archive, scratch, &allowed)
        .and_then(|_| verify_tree_v2(scratch, native_expected(manifest)).map(|_| ()));
    let _ = std::fs::remove_dir_all(scratch);
    verified
}

/// Commit points used by the failure-injection test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitPoint {
    Backup,
    Release,
    Archive,
    Sidecar,
    Index,
    Sums,
}

struct CommitItem {
    label: &'static str,
    staged: PathBuf,
    final_path: PathBuf,
    backup_name: &'static str,
    point: CommitPoint,
    directory: bool,
}

fn native_commit_items(stage: &NativeStage, paths: &NativeFinalPaths) -> Vec<CommitItem> {
    vec![
        CommitItem {
            label: "release directory",
            staged: stage.tree(),
            final_path: paths.release_dir.clone(),
            backup_name: "release",
            point: CommitPoint::Release,
            directory: true,
        },
        CommitItem {
            label: "release archive",
            staged: stage.archive(),
            final_path: paths.archive.clone(),
            backup_name: "archive",
            point: CommitPoint::Archive,
            directory: false,
        },
        CommitItem {
            label: "archive checksum",
            staged: stage.sidecar(),
            final_path: paths.sidecar.clone(),
            backup_name: "sidecar",
            point: CommitPoint::Sidecar,
            directory: false,
        },
        CommitItem {
            label: "index.json",
            staged: stage.index(),
            final_path: paths.index.clone(),
            backup_name: "index",
            point: CommitPoint::Index,
            directory: false,
        },
        CommitItem {
            label: "SHA256SUMS",
            staged: stage.sums(),
            final_path: paths.sums.clone(),
            backup_name: "sums",
            point: CommitPoint::Sums,
            directory: false,
        },
    ]
}

fn commit_native_stage<F>(stage: &mut NativeStage, items: &[CommitItem], mut fault: F) -> Result<()>
where
    F: FnMut(CommitPoint) -> Result<()>,
{
    let backup_root = stage.path("backup");
    std::fs::create_dir(&backup_root).map_err(|error| DeliveryError::io(&backup_root, error))?;
    set_mode(&backup_root, 0o700)?;

    let mut backed_up = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if !path_present(&item.final_path) {
            continue;
        }
        let backup_path = backup_root.join(item.backup_name);
        if let Err(error) = std::fs::rename(&item.final_path, &backup_path) {
            return rollback_native_commit(
                stage,
                items,
                &[],
                &backed_up,
                &backup_root,
                DeliveryError::io(&item.final_path, error),
            );
        }
        backed_up.push(index);
    }
    if let Err(error) = fault(CommitPoint::Backup) {
        return rollback_native_commit(stage, items, &[], &backed_up, &backup_root, error);
    }

    let mut installed = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if let Err(error) = std::fs::rename(&item.staged, &item.final_path) {
            return rollback_native_commit(
                stage,
                items,
                &installed,
                &backed_up,
                &backup_root,
                DeliveryError::io(&item.staged, error),
            );
        }
        installed.push(index);
        if let Err(error) = fault(item.point) {
            return rollback_native_commit(
                stage,
                items,
                &installed,
                &backed_up,
                &backup_root,
                error,
            );
        }
    }

    let _ = std::fs::remove_dir_all(&backup_root);
    Ok(())
}

fn rollback_native_commit(
    stage: &mut NativeStage,
    items: &[CommitItem],
    installed: &[usize],
    backed_up: &[usize],
    backup_root: &Path,
    original: DeliveryError,
) -> Result<()> {
    let mut rollback_error: Option<(&'static str, std::io::Error)> = None;
    for &index in installed.iter().rev() {
        let item = &items[index];
        let result = if item.directory {
            std::fs::remove_dir_all(&item.final_path)
        } else {
            std::fs::remove_file(&item.final_path)
        };
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::NotFound && rollback_error.is_none() {
                rollback_error = Some((item.label, error));
            }
        }
    }
    for &index in backed_up.iter().rev() {
        let item = &items[index];
        let backup_path = backup_root.join(item.backup_name);
        if let Err(error) = std::fs::rename(&backup_path, &item.final_path) {
            if rollback_error.is_none() {
                rollback_error = Some((item.label, error));
            }
        }
    }
    if let Some((label, error)) = rollback_error {
        stage.keep();
        return Err(DeliveryError::InstallState(format!(
            "failed to restore the previous {label} after an aborted build; the temporary backup \
             is preserved for manual recovery: {error}"
        )));
    }
    let _ = std::fs::remove_dir_all(backup_root);
    Err(original)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets_lock::canonical_tree_digest;
    use crate::manifest::PACKAGE_NAME;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-package-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn fake_tree(base: &Path) -> PathBuf {
        let root = base.join("input");
        std::fs::create_dir_all(root.join("bin/wxml-type-backend/node_modules/typescript"))
            .unwrap();
        std::fs::create_dir_all(
            root.join("bin/wxml-type-backend/node_modules/miniprogram-api-typings"),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("zed-extension/languages/wxml")).unwrap();
        std::fs::write(
            root.join("bin/wxml-lsp"),
            b"#!/bin/sh\nprintf 'wxml-lsp 0.1.0\\n'\n",
        )
        .unwrap();
        set_mode(&root.join("bin/wxml-lsp"), 0o755).unwrap();
        std::fs::write(
            root.join("bin/wxml-lsp-install"),
            b"#!/bin/sh\nprintf 'wxml-lsp-install 0.1.0\\n'\n",
        )
        .unwrap();
        set_mode(&root.join("bin/wxml-lsp-install"), 0o755).unwrap();
        std::fs::write(
            root.join("bin/wxml-type-backend/server.js"),
            b"console.log('backend');\n",
        )
        .unwrap();
        std::fs::write(
            root.join("bin/wxml-type-backend/node_modules/typescript/package.json"),
            b"{}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json"),
            b"{}\n",
        )
        .unwrap();
        std::fs::write(root.join("zed-extension/extension.wasm"), b"\0asm").unwrap();
        std::fs::write(
            root.join("zed-extension/extension.toml"),
            b"id = \"wxml-lsp\"\n",
        )
        .unwrap();
        root
    }

    fn options(input: &Path, output: &Path) -> BuildOptions {
        BuildOptions {
            input: input.to_path_buf(),
            output: output.to_path_buf(),
            release_version: "0.1.0".to_owned(),
            platform: crate::paths::host_platform().unwrap(),
            protocol_version: 2,
            node_minimum: "24.0.0".to_owned(),
            backend_version: "0.1.0".to_owned(),
            zed_version: Some("0.0.1".to_owned()),
            built_at: "2026-01-01T00:00:00Z".to_owned(),
            mtime: 0,
            force: true,
            dry_run: false,
        }
    }

    #[test]
    fn builds_verifies_and_indexes_a_package() {
        let base = scratch("build");
        let input = fake_tree(&base);
        let output = base.join("out");
        let report = build_package(&options(&input, &output)).unwrap();
        assert!(report.archive.as_ref().unwrap().is_file());
        assert!(report.release_dir.as_ref().unwrap().is_dir());
        let index = read_index(&output.join("index.json")).unwrap();
        let release = index.find("0.1.0", &report.manifest.platform).unwrap();
        assert_eq!(release.sha256, report.sha256);
        assert_eq!(release.size, report.size);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let base = scratch("dry");
        let input = fake_tree(&base);
        let output = base.join("out");
        let mut options = options(&input, &output);
        options.dry_run = true;
        let report = build_package(&options).unwrap();
        assert!(report.release_dir.is_none());
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_symlinked_inputs() {
        let base = scratch("symlink");
        let input = fake_tree(&base);
        std::os::unix::fs::symlink("/etc/passwd", input.join("link")).unwrap();
        let error = build_package(&options(&input, &base.join("out"))).unwrap_err();
        assert!(
            matches!(error, DeliveryError::UnsafeArchive(_)),
            "{error:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let base = scratch("force");
        let input = fake_tree(&base);
        let output = base.join("out");
        build_package(&options(&input, &output)).unwrap();
        let mut second = options(&input, &output);
        second.force = false;
        assert!(build_package(&second).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn archive_and_manifest_are_deterministic() {
        let base = scratch("deterministic");
        let input = fake_tree(&base);
        let first = build_package(&options(&input, &base.join("one"))).unwrap();
        let second = build_package(&options(&input, &base.join("two"))).unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(
            std::fs::read(first.archive.unwrap()).unwrap(),
            std::fs::read(second.archive.unwrap()).unwrap()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn manifest_digest_tracks_content_and_never_swallows_errors() {
        let base = scratch("digest");
        let input = fake_tree(&base);
        let report = build_package(&options(&input, &base.join("out"))).unwrap();
        let digest = manifest_digest(&report.manifest).unwrap();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let mut changed = report.manifest.clone();
        changed.package_version = "0.2.0".to_owned();
        assert_ne!(digest, manifest_digest(&changed).unwrap());
        assert_eq!(report.manifest.package, PACKAGE_NAME);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn native_commit_rolls_back_every_injected_failure_point() {
        let points = [
            CommitPoint::Backup,
            CommitPoint::Release,
            CommitPoint::Archive,
            CommitPoint::Sidecar,
            CommitPoint::Index,
            CommitPoint::Sums,
        ];
        for point in points {
            let base = scratch(&format!("native-commit-{point:?}"));
            let output = base.join("output");
            std::fs::create_dir_all(&output).unwrap();
            let paths = NativeFinalPaths {
                release_name: "wxml-lsp-0.1.0-linux-x86_64".to_owned(),
                archive_name: "wxml-lsp-0.1.0-linux-x86_64.tar.gz".to_owned(),
                release_dir: output.join("wxml-lsp-0.1.0-linux-x86_64"),
                archive: output.join("wxml-lsp-0.1.0-linux-x86_64.tar.gz"),
                sidecar: output.join("wxml-lsp-0.1.0-linux-x86_64.tar.gz.sha256"),
                index: output.join("index.json"),
                sums: output.join("SHA256SUMS"),
            };
            std::fs::create_dir_all(&paths.release_dir).unwrap();
            std::fs::write(paths.release_dir.join("old.txt"), b"old release").unwrap();
            std::fs::write(&paths.archive, b"old archive").unwrap();
            std::fs::write(&paths.sidecar, b"old sidecar").unwrap();
            std::fs::write(&paths.index, b"old index").unwrap();
            std::fs::write(&paths.sums, b"old sums").unwrap();

            let mut stage = NativeStage::create(&output).unwrap();
            let tree = stage.tree();
            std::fs::create_dir(&tree).unwrap();
            std::fs::write(tree.join("new.txt"), b"new release").unwrap();
            std::fs::write(stage.archive(), b"new archive").unwrap();
            std::fs::write(stage.sidecar(), b"new sidecar").unwrap();
            std::fs::write(stage.index(), b"new index").unwrap();
            std::fs::write(stage.sums(), b"new sums").unwrap();
            let items = native_commit_items(&stage, &paths);

            let error = commit_native_stage(&mut stage, &items, |candidate| {
                if candidate == point {
                    Err(DeliveryError::Integrity(format!(
                        "injected commit failure at {candidate:?}"
                    )))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert!(
                error.to_string().contains("injected commit failure"),
                "{error}"
            );
            assert_eq!(
                std::fs::read(paths.release_dir.join("old.txt")).unwrap(),
                b"old release"
            );
            assert!(!paths.release_dir.join("new.txt").exists());
            assert_eq!(std::fs::read(&paths.archive).unwrap(), b"old archive");
            assert_eq!(std::fs::read(&paths.sidecar).unwrap(), b"old sidecar");
            assert_eq!(std::fs::read(&paths.index).unwrap(), b"old index");
            assert_eq!(std::fs::read(&paths.sums).unwrap(), b"old sums");
            drop(stage);
            let leftovers = std::fs::read_dir(&output)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".native-stage-")
                });
            assert!(!leftovers, "staging directory was not cleaned");
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    fn fixture_typings_entries() -> Vec<FileEntry> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/miniprogram-api-typings-5.3.1");
        crate::manifest_v2::MINIPROGRAM_TYPINGS_PAYLOAD_PATHS
            .iter()
            .map(|relative| {
                let relative = *relative;
                let bytes = std::fs::read(root.join(relative)).unwrap();
                FileEntry {
                    path: format!("{MINIPROGRAM_TYPINGS_ROOT}/{relative}"),
                    sha256: sha256_bytes(&bytes),
                    size: u64::try_from(bytes.len()).unwrap(),
                    mode: "0644".to_owned(),
                }
            })
            .collect()
    }

    fn refreshed_typings_asset(entries: &[FileEntry]) -> TypeAssetBundle {
        compute_type_asset_bundle(entries, MINIPROGRAM_TYPINGS_ROOT, &["**".to_owned()]).unwrap()
    }

    #[test]
    fn typings_bundle_accepts_the_frozen_payload() {
        let entries = fixture_typings_entries();
        let bundle = native_typings(&entries).unwrap();
        assert_eq!(bundle.file_count, MINIPROGRAM_TYPINGS_FILE_COUNT);
        assert_eq!(bundle.file_count, 23);
        assert_eq!(bundle.total_bytes, MINIPROGRAM_TYPINGS_TOTAL_BYTES);
        assert_eq!(bundle.sha256, MINIPROGRAM_TYPINGS_BUNDLE_DIGEST);
    }

    #[test]
    fn typings_bundle_rejects_count_drift() {
        let entries = fixture_typings_entries();
        let mut short = entries.clone();
        short.remove(0);
        let error = native_typings(&short).unwrap_err();
        assert!(
            error.to_string().contains("must contain exactly 23 files"),
            "{error}"
        );

        let mut long = entries;
        long.push(FileEntry {
            path: format!("{MINIPROGRAM_TYPINGS_ROOT}/types/extra.d.ts"),
            sha256: sha256_bytes(b"extra"),
            size: 5,
            mode: "0644".to_owned(),
        });
        let error = native_typings(&long).unwrap_err();
        assert!(
            error.to_string().contains("must contain exactly 23 files"),
            "{error}"
        );
    }

    #[test]
    fn typings_bundle_rejects_a_substituted_23_file_set() {
        let mut entries = fixture_typings_entries();
        let target = entries
            .iter_mut()
            .find(|entry| entry.path.ends_with("/index.d.ts"))
            .unwrap();
        target.sha256 = "0".repeat(64);
        let error = native_typings(&entries).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("payload does not match the frozen contract"),
            "{error}"
        );
        assert_eq!(
            refreshed_typings_asset(&entries).file_count,
            MINIPROGRAM_TYPINGS_FILE_COUNT
        );
    }

    #[test]
    fn typings_bundle_rejects_payload_byte_drift() {
        let mut entries = fixture_typings_entries();
        let target = entries
            .iter_mut()
            .find(|entry| entry.path.ends_with("/LICENSE"))
            .unwrap();
        target.size += 1;
        let error = native_typings(&entries).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("payload does not match the frozen contract"),
            "{error}"
        );
    }

    fn npm_tarball(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, bytes, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(bytes.len()).unwrap());
            header.set_mode(*mode);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("package/{path}"), *bytes)
                .unwrap();
        }
        builder.finish().unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    fn synthetic_asset_value(
        name: &str,
        package_root: &str,
        files: &[(&str, &[u8], u32)],
    ) -> (serde_json::Value, Vec<u8>) {
        let tarball = npm_tarball(files);
        let upstream: Vec<FileEntry> = files
            .iter()
            .map(|(path, bytes, mode)| FileEntry {
                path: (*path).to_owned(),
                sha256: sha256_bytes(bytes),
                size: u64::try_from(bytes.len()).unwrap(),
                mode: format!("{mode:04o}"),
            })
            .collect();
        let payload: Vec<FileEntry> = upstream
            .iter()
            .map(|file| FileEntry {
                path: format!("{package_root}/{}", file.path),
                ..file.clone()
            })
            .collect();
        let upstream_digest = canonical_tree_digest(&upstream, "").unwrap();
        let payload_digest = canonical_tree_digest(&payload, package_root).unwrap();
        let bundle_include = if name == ENGINE_ASSET_NAME {
            vec![STANDARD_LIB_INCLUDE.to_owned()]
        } else {
            vec!["**".to_owned()]
        };
        let bundle_root = if name == ENGINE_ASSET_NAME {
            "engine/lib".to_owned()
        } else {
            package_root.to_owned()
        };
        let bundle = compute_type_asset_bundle(&payload, &bundle_root, &bundle_include).unwrap();
        let tarball_name = format!("{name}-1.0.0.tgz");
        let total_bytes: u64 = payload.iter().map(|file| file.size).sum();
        let unpacked_size: u64 = upstream.iter().map(|file| file.size).sum();
        (
            serde_json::json!({
                "name": name,
                "kind": "npm-tarball",
                "npmPackage": name,
                "version": "1.0.0",
                "os": ["any"],
                "cpu": ["any"],
                "provenance": {
                    "registry": "https://registry.npmjs.org",
                    "distSha1": "0123456789abcdef0123456789abcdef01234567",
                    "publishedAt": "2025-01-01T00:00:00Z",
                },
                "license": "MIT",
                "licenseFiles": [format!("{package_root}/LICENSE")],
                "packageRoot": package_root,
                "tarball": {
                    "url": format!("https://registry.npmjs.org/{name}/-/{tarball_name}"),
                    "size": tarball.len(),
                    "integrity": crate::sri::sha512_sri_bytes(&tarball).to_sri(),
                    "sha256": sha256_bytes(&tarball),
                },
                "upstream": {
                    "fileCount": upstream.len(),
                    "unpackedSize": unpacked_size,
                    "stripPrefix": "package/",
                    "treeDigest": upstream_digest,
                    "files": upstream,
                },
                "extract": {
                    "include": ["**"],
                    "exclude": [],
                    "executables": files.iter()
                        .filter(|(_, _, mode)| *mode == 0o755)
                        .map(|(path, _, _)| (*path).to_owned())
                        .collect::<Vec<_>>(),
                },
                "payload": {
                    "fileCount": payload.len(),
                    "totalBytes": total_bytes,
                    "treeDigest": payload_digest,
                    "files": payload,
                    "bundles": [{
                        "name": if name == ENGINE_ASSET_NAME {
                            STANDARD_LIB_ASSET.to_owned()
                        } else {
                            name.to_owned()
                        },
                        "root": bundle_root,
                        "include": bundle_include,
                        "fileCount": bundle.file_count,
                        "totalBytes": bundle.total_bytes,
                        "sha256": bundle.sha256,
                    }],
                },
            }),
            tarball,
        )
    }

    struct SyntheticCache {
        cache: PathBuf,
        lock_bytes: Vec<u8>,
        lock_sha256: String,
        assets: VerifiedAssets,
    }

    fn synthetic_cache(base: &Path, label: &str, name: &str, package_root: &str) -> SyntheticCache {
        let (value, tarball) = if name == ENGINE_ASSET_NAME {
            let mut files: Vec<(String, Vec<u8>, u32)> = (0..113)
                .map(|index| {
                    (
                        format!("lib/lib.f{index:03}.d.ts"),
                        format!("// {index}\n").into_bytes(),
                        0o644,
                    )
                })
                .collect();
            files.push(("lib/lib.d.ts".to_owned(), b"// base\n".to_vec(), 0o644));
            files.push(("lib/lib.es5.d.ts".to_owned(), b"// es5\n".to_vec(), 0o644));
            files.push((
                "lib/lib.es2020.d.ts".to_owned(),
                b"// es2020\n".to_vec(),
                0o644,
            ));
            files.push((
                "lib/tsc".to_owned(),
                b"not-the-pinned-engine\n".to_vec(),
                0o755,
            ));
            files.push(("LICENSE".to_owned(), b"license\n".to_vec(), 0o644));
            files.push((
                "package.json".to_owned(),
                b"{\"name\":\"@typescript/typescript-linux-x64\",\"version\":\"7.1.0-beta\",\"gitHead\":\"3e9236361fa97a9e6c685f4833ac457ae2d8c50a\"}\n".to_vec(),
                0o644,
            ));
            let borrowed: Vec<(&str, &[u8], u32)> = files
                .iter()
                .map(|(path, bytes, mode)| (path.as_str(), bytes.as_slice(), *mode))
                .collect();
            synthetic_asset_value(name, package_root, &borrowed)
        } else {
            let files = [
                ("index.d.ts", b"export {};\n".as_slice(), 0o644),
                ("LICENSE", b"license\n".as_slice(), 0o644),
            ];
            synthetic_asset_value(name, package_root, &files)
        };
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "assets": [value],
        }))
        .unwrap();
        let lock = crate::assets_lock::AssetsLock::parse(&bytes).unwrap();
        let lock_sha256 = lock.lock_sha256().unwrap();
        let from_dir = base.join(format!("{label}-from-dir"));
        std::fs::create_dir_all(&from_dir).unwrap();
        std::fs::write(from_dir.join(format!("{name}-1.0.0.tgz")), &tarball).unwrap();
        let cache = base.join(format!("{label}-cache"));
        std::fs::create_dir_all(&cache).unwrap();
        set_mode(&cache, 0o700).unwrap();
        let asset = &lock.assets[0];
        let entry =
            crate::provision::provision_asset(asset, &cache, Some(&from_dir), &lock_sha256, &bytes)
                .unwrap();
        SyntheticCache {
            cache,
            lock_bytes: bytes,
            lock_sha256,
            assets: VerifiedAssets {
                lock_sha256: lock.lock_sha256().unwrap(),
                assets: vec![VerifiedAsset {
                    asset: asset.clone(),
                    entry,
                }],
            },
        }
    }

    #[test]
    fn packager_cache_verification_accepts_exact_payload_and_rejects_tampering() {
        let base = scratch("packager-cache-tamper");
        let fixture = synthetic_cache(&base, "exact", TYPINGS_ASSET_NAME, MINIPROGRAM_TYPINGS_ROOT);
        let asset = &fixture.assets.assets[0].asset;
        verify_cached_asset(
            asset,
            &fixture.cache,
            &fixture.lock_sha256,
            &fixture.lock_bytes,
        )
        .unwrap();
        let assembled = assemble_native_files(Vec::new(), &fixture.assets).unwrap();
        assert_eq!(assembled.len(), 2);

        let payload = fixture.assets.assets[0].entry.directory.join("payload");
        let file = payload.join("typings/miniprogram-api-typings/index.d.ts");
        std::fs::write(&file, b"tampered\n").unwrap();
        let error = verify_cached_asset(
            asset,
            &fixture.cache,
            &fixture.lock_sha256,
            &fixture.lock_bytes,
        )
        .unwrap_err();
        assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");

        let base = scratch("packager-cache-extra");
        let fixture = synthetic_cache(&base, "extra", TYPINGS_ASSET_NAME, MINIPROGRAM_TYPINGS_ROOT);
        let payload = fixture.assets.assets[0].entry.directory.join("payload");
        std::fs::write(
            payload.join("typings/miniprogram-api-typings/extra.d.ts"),
            b"extra\n",
        )
        .unwrap();
        let error = verify_cached_asset(
            &fixture.assets.assets[0].asset,
            &fixture.cache,
            &fixture.lock_sha256,
            &fixture.lock_bytes,
        )
        .unwrap_err();
        assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");

        let base = scratch("packager-cache-missing");
        let fixture = synthetic_cache(
            &base,
            "missing",
            TYPINGS_ASSET_NAME,
            MINIPROGRAM_TYPINGS_ROOT,
        );
        let payload = fixture.assets.assets[0].entry.directory.join("payload");
        std::fs::remove_file(payload.join("typings/miniprogram-api-typings/LICENSE")).unwrap();
        let error = verify_cached_asset(
            &fixture.assets.assets[0].asset,
            &fixture.cache,
            &fixture.lock_sha256,
            &fixture.lock_bytes,
        )
        .unwrap_err();
        assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");

        let base = scratch("packager-cache-mode");
        let fixture = synthetic_cache(&base, "mode", TYPINGS_ASSET_NAME, MINIPROGRAM_TYPINGS_ROOT);
        let payload = fixture.assets.assets[0].entry.directory.join("payload");
        set_mode(
            &payload.join("typings/miniprogram-api-typings/index.d.ts"),
            0o755,
        )
        .unwrap();
        let error = verify_cached_asset(
            &fixture.assets.assets[0].asset,
            &fixture.cache,
            &fixture.lock_sha256,
            &fixture.lock_bytes,
        )
        .unwrap_err();
        assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
    }

    #[test]
    fn packager_rejects_engine_cache_tampering() {
        type CacheMutation = fn(&Path);
        let cases: &[(&str, CacheMutation)] = &[
            ("engine-single", |payload| {
                std::fs::write(payload.join("engine/lib/lib.es5.d.ts"), b"tampered\n").unwrap();
            }),
            ("engine-extra", |payload| {
                std::fs::write(payload.join("engine/lib/extra.d.ts"), b"extra\n").unwrap();
            }),
            ("engine-missing", |payload| {
                std::fs::remove_file(payload.join("engine/LICENSE")).unwrap();
            }),
        ];
        for (label, mutate) in cases {
            let base = scratch(label);
            let fixture = synthetic_cache(&base, label, ENGINE_ASSET_NAME, ENGINE_ASSET_ROOT);
            let payload = fixture.assets.assets[0].entry.directory.join("payload");
            mutate(&payload);
            let error = verify_cached_asset(
                &fixture.assets.assets[0].asset,
                &fixture.cache,
                &fixture.lock_sha256,
                &fixture.lock_bytes,
            )
            .unwrap_err();
            assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");
        }
    }

    #[test]
    fn packager_rejects_wrong_lock_bundle_and_stale_cache() {
        let base = scratch("packager-cache-lock");
        let fixture = synthetic_cache(&base, "lock", TYPINGS_ASSET_NAME, MINIPROGRAM_TYPINGS_ROOT);
        let asset = &fixture.assets.assets[0].asset;

        let mut value: serde_json::Value = serde_json::from_slice(&fixture.lock_bytes).unwrap();
        value["assets"][0]["payload"]["bundles"][0]["sha256"] = serde_json::json!("0".repeat(64));
        let wrong_bytes = serde_json::to_vec_pretty(&value).unwrap();
        let wrong_lock = crate::assets_lock::AssetsLock::parse(&wrong_bytes).unwrap();
        let error = verify_cached_asset(
            &wrong_lock.assets[0],
            &fixture.cache,
            &wrong_lock.lock_sha256().unwrap(),
            &wrong_bytes,
        )
        .unwrap_err();
        assert!(
            matches!(error, DeliveryError::InstallState(_)),
            "a lock change must invalidate the cache key: {error:?}"
        );

        let error =
            verify_cached_asset(asset, &fixture.cache, &fixture.lock_sha256, b"{}\n").unwrap_err();
        assert!(matches!(error, DeliveryError::Integrity(_)), "{error:?}");

        let mut value: serde_json::Value = serde_json::from_slice(&fixture.lock_bytes).unwrap();
        value["assets"][0]["tarball"]["sha256"] = serde_json::json!("1".repeat(64));
        let stale_bytes = serde_json::to_vec_pretty(&value).unwrap();
        let stale_lock = crate::assets_lock::AssetsLock::parse(&stale_bytes).unwrap();
        let error = verify_cached_asset(
            &stale_lock.assets[0],
            &fixture.cache,
            &stale_lock.lock_sha256().unwrap(),
            &stale_bytes,
        )
        .unwrap_err();
        assert!(
            matches!(error, DeliveryError::InstallState(_)),
            "a changed lock must not reuse a stale cache entry: {error:?}"
        );
    }

    #[test]
    fn packager_rejects_unpinned_engine_and_missing_git_head() {
        let base = scratch("packager-engine-pin");
        let fixture = synthetic_cache(&base, "engine", ENGINE_ASSET_NAME, ENGINE_ASSET_ROOT);
        let assembled = assemble_native_files(Vec::new(), &fixture.assets).unwrap();
        validate_engine_identity(&assembled).unwrap();
        let error = validate_pinned_engine_binary(&assembled).unwrap_err();
        assert!(error.to_string().contains("pinned expected"), "{error}");

        let package_json = base.join("missing-git-head.json");
        let bytes = b"{\"name\":\"@typescript/typescript-linux-x64\",\"version\":\"7.1.0-beta\"}\n";
        std::fs::write(&package_json, bytes).unwrap();
        let source = SourceFile {
            entry: FileEntry {
                path: "engine/package.json".to_owned(),
                sha256: sha256_bytes(bytes),
                size: u64::try_from(bytes.len()).unwrap(),
                mode: "0644".to_owned(),
            },
            absolute: package_json,
        };
        let error = validate_engine_identity(&[source]).unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("githead"),
            "{error}"
        );
    }

    #[test]
    fn packager_bundles_must_match_the_lock() {
        let base = scratch("packager-bundle");
        let fixture = synthetic_cache(&base, "bundle", ENGINE_ASSET_NAME, ENGINE_ASSET_ROOT);
        let files = assemble_native_files(Vec::new(), &fixture.assets).unwrap();
        let entries: Vec<FileEntry> = files.iter().map(|file| file.entry.clone()).collect();
        let standard = native_standard_lib(&entries).unwrap();
        let typings = native_typings(&fixture_typings_entries()).unwrap();
        let mut broken = fixture.assets;
        broken.assets[0].asset.payload.bundles[0].sha256 = "0".repeat(64);
        let error = verify_lock_bundles(&broken, &standard, &typings).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match assets.lock.json"),
            "{error}"
        );
    }
}
