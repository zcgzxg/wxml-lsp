//! Deterministic package construction from a staged build tree.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::archive::create_tar_gz;
use crate::error::{DeliveryError, Result};
use crate::manifest::{
    ArtifactKind, BinaryArtifact, ExpectedManifest, FileEntry, Index, MAX_ARCHIVE_BYTES,
    MAX_FILE_BYTES, MAX_FILES, Manifest, NodeInfo, PACKAGE_NAME, Release, SCHEMA_VERSION,
    archive_name, atomic_write, format_sha256sums, read_index, set_mode, sha256_bytes, sha256_file,
};
use crate::paths::{validate_platform, validate_relative_path, validate_version};
use crate::verify::{checksums_for_tree, verify_archive, verify_tree};

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
    Ok(relative.to_string_lossy().replace('\\', "/"))
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
#[must_use]
pub fn manifest_digest(manifest: &Manifest) -> String {
    manifest
        .to_json()
        .map_or_else(|_| "unavailable".to_owned(), |bytes| sha256_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn manifest_digest_is_stable() {
        let base = scratch("digest");
        let input = fake_tree(&base);
        let report = build_package(&options(&input, &base.join("out"))).unwrap();
        assert_eq!(
            manifest_digest(&report.manifest),
            manifest_digest(&report.manifest)
        );
        assert_eq!(report.manifest.package, PACKAGE_NAME);
        let _ = std::fs::remove_dir_all(&base);
    }
}
