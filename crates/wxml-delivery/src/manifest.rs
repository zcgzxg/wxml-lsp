//! Release manifest and index schemas, plus integrity helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{DeliveryError, Result};
use crate::paths::{
    validate_archive_name, validate_platform, validate_relative_path, validate_sha256,
    validate_version,
};
use crate::semver::{Semver, compare_versions, is_prerelease};

/// Package identity written into manifests and index files.
pub const PACKAGE_NAME: &str = "wxml-lsp";
/// Current manifest/index schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Type-bridge protocol versions this installer accepts.
///
/// This must be updated together with `wxml-lsp` and the Node backend when the
/// Rust/Node protocol changes.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u64] = &[2];

/// Maximum accepted archive file size.
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum accepted size of one file after extraction.
pub const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum accepted total size of all extracted files.
pub const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
/// Maximum number of files accepted in one package.
pub const MAX_FILES: usize = 8192;
/// Maximum accepted `manifest.json` size.
pub const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum accepted `index.json` size.
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;

/// Node backend metadata required by the server.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeInfo {
    /// Minimum Node.js version (`major.minor.patch`).
    pub minimum_version: String,
    /// Version reported by the packaged Node backend package.
    pub backend_version: String,
}

/// One payload file record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileEntry {
    /// Package-relative path using `/` separators.
    pub path: String,
    /// Lowercase SHA-256 of the file bytes.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
    /// Explicit POSIX mode: `0644` or `0755`.
    pub mode: String,
}

impl FileEntry {
    /// Parses the explicit mode.
    ///
    /// # Errors
    ///
    /// Returns an error for any mode other than `0644` or `0755`.
    pub fn mode_bits(&self) -> Result<u32> {
        match self.mode.as_str() {
            "0644" => Ok(0o644),
            "0755" => Ok(0o755),
            _ => Err(DeliveryError::InvalidPackage(format!(
                "file `{}` has unsupported mode `{}`",
                self.path, self.mode
            ))),
        }
    }
}

/// Kind of a runnable or editor artifact described by the manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    /// Native Rust binary.
    Rust,
    /// Node.js backend entry point.
    NodeBackend,
    /// Zed extension WebAssembly module.
    ZedExtension,
}

/// Metadata for one artifact that must have a matching payload file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BinaryArtifact {
    /// Artifact class.
    pub kind: ArtifactKind,
    /// Stable short name used in startup and diagnostics.
    pub name: String,
    /// Package-relative path.
    pub path: String,
    /// Artifact version.
    pub version: String,
    /// Type-bridge protocol version for the Node backend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u64>,
    /// SHA-256 copied from the matching payload record.
    pub sha256: String,
}

/// One immutable package manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Manifest schema.
    pub schema_version: u32,
    /// Package identity.
    pub package: String,
    /// Release version.
    pub package_version: String,
    /// Target platform label.
    pub platform: String,
    /// Rust/Node type-bridge protocol version.
    pub protocol_version: u64,
    /// Node runtime and backend metadata.
    pub node: NodeInfo,
    /// Build timestamp (UTC, RFC 3339).
    pub built_at: String,
    /// Required artifacts.
    pub binaries: Vec<BinaryArtifact>,
    /// Every payload file except `manifest.json` and `SHA256SUMS`.
    pub files: Vec<FileEntry>,
}

/// Optional expectations when validating a manifest.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExpectedManifest<'a> {
    /// Exact release version.
    pub version: Option<&'a str>,
    /// Exact platform label.
    pub platform: Option<&'a str>,
    /// Exact protocol version.
    pub protocol_version: Option<u64>,
}

impl Manifest {
    /// Validates all structural, path and integrity metadata invariants.
    ///
    /// This does not hash files; [`crate::verify::verify_tree`] performs the
    /// byte-level verification.
    ///
    /// # Errors
    ///
    /// Returns an error for schema, version, path, mode, size or artifact
    /// mismatches.
    pub fn validate(&self, expected: ExpectedManifest<'_>) -> Result<()> {
        self.validate_identity(expected)?;
        self.validate_files()?;
        self.validate_artifacts()
    }

    fn validate_identity(&self, expected: ExpectedManifest<'_>) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(DeliveryError::InvalidPackage(format!(
                "unsupported manifest schema {}",
                self.schema_version
            )));
        }
        if self.package != PACKAGE_NAME {
            return Err(DeliveryError::InvalidPackage(format!(
                "unexpected package `{}`",
                self.package
            )));
        }
        validate_version(&self.package_version)?;
        if let Some(version) = expected.version {
            if self.package_version != version {
                return Err(DeliveryError::Integrity(format!(
                    "manifest version `{}` does not match requested `{version}`",
                    self.package_version
                )));
            }
        }
        validate_platform(&self.platform)?;
        if let Some(platform) = expected.platform {
            if self.platform != platform {
                return Err(DeliveryError::Integrity(format!(
                    "manifest platform `{}` does not match host `{platform}`",
                    self.platform
                )));
            }
        }
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&self.protocol_version) {
            return Err(DeliveryError::InvalidPackage(format!(
                "protocol version {} is not supported",
                self.protocol_version
            )));
        }
        if let Some(protocol) = expected.protocol_version {
            if self.protocol_version != protocol {
                return Err(DeliveryError::Integrity(format!(
                    "manifest protocol {} does not match expected {protocol}",
                    self.protocol_version
                )));
            }
        }
        Semver::parse(&self.node.minimum_version)?;
        validate_version(&self.node.backend_version)?;
        if self.built_at.is_empty() || self.built_at.len() > 64 {
            return Err(DeliveryError::InvalidPackage(
                "builtAt must be 1-64 bytes".to_owned(),
            ));
        }
        if self.built_at.chars().any(char::is_control) {
            return Err(DeliveryError::InvalidPackage(
                "builtAt contains control characters".to_owned(),
            ));
        }

        Ok(())
    }

    fn validate_files(&self) -> Result<()> {
        if self.files.is_empty() {
            return Err(DeliveryError::InvalidPackage(
                "manifest contains no payload files".to_owned(),
            ));
        }
        if self.files.len() > MAX_FILES {
            return Err(DeliveryError::SizeLimit(format!(
                "manifest lists more than {MAX_FILES} files"
            )));
        }
        let mut paths = BTreeSet::new();
        let mut total = 0_u64;
        for file in &self.files {
            validate_relative_path(&file.path, false)?;
            validate_sha256(&file.sha256)?;
            file.mode_bits()?;
            if file.size > MAX_FILE_BYTES {
                return Err(DeliveryError::SizeLimit(format!(
                    "file `{}` exceeds the per-file limit",
                    file.path
                )));
            }
            total = total
                .checked_add(file.size)
                .ok_or_else(|| DeliveryError::SizeLimit("file sizes overflow".to_owned()))?;
            if total > MAX_EXTRACTED_BYTES {
                return Err(DeliveryError::SizeLimit(
                    "manifest payload exceeds the total extracted-size limit".to_owned(),
                ));
            }
            if !paths.insert(file.path.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "duplicate payload path `{}`",
                    file.path
                )));
            }
        }

        Ok(())
    }

    fn validate_artifacts(&self) -> Result<()> {
        let (node_backends, zed_extensions) = self.validate_artifact_entries()?;
        self.validate_required_artifacts(node_backends, zed_extensions)
    }

    fn validate_artifact_entries(&self) -> Result<(usize, usize)> {
        let mut rust_names = BTreeSet::new();
        let mut node_backends = 0;
        let mut zed_extensions = 0;
        for artifact in &self.binaries {
            if artifact.name.is_empty() || artifact.name.len() > 64 {
                return Err(DeliveryError::InvalidPackage(
                    "artifact name must be 1-64 bytes".to_owned(),
                ));
            }
            if !artifact
                .name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            {
                return Err(DeliveryError::InvalidPackage(format!(
                    "artifact name `{}` contains unsupported characters",
                    artifact.name
                )));
            }
            validate_version(&artifact.version)?;
            validate_sha256(&artifact.sha256)?;
            let file = self
                .files
                .iter()
                .find(|file| file.path == artifact.path)
                .ok_or_else(|| {
                    DeliveryError::InvalidPackage(format!(
                        "artifact `{}` has no payload file",
                        artifact.name
                    ))
                })?;
            if file.sha256 != artifact.sha256 {
                return Err(DeliveryError::Integrity(format!(
                    "artifact `{}` hash does not match its payload entry",
                    artifact.name
                )));
            }
            match artifact.kind {
                ArtifactKind::Rust => {
                    if !rust_names.insert(artifact.name.as_str()) {
                        return Err(DeliveryError::InvalidPackage(format!(
                            "duplicate Rust artifact `{}`",
                            artifact.name
                        )));
                    }
                }
                ArtifactKind::NodeBackend => {
                    node_backends += 1;
                    if artifact.path != "bin/wxml-type-backend/server.js" {
                        return Err(DeliveryError::InvalidPackage(
                            "Node backend must be `bin/wxml-type-backend/server.js`".to_owned(),
                        ));
                    }
                    if artifact.protocol_version != Some(self.protocol_version) {
                        return Err(DeliveryError::InvalidPackage(
                            "Node backend protocol does not match the manifest".to_owned(),
                        ));
                    }
                    if artifact.version != self.node.backend_version {
                        return Err(DeliveryError::InvalidPackage(
                            "Node backend version does not match node.backendVersion".to_owned(),
                        ));
                    }
                }
                ArtifactKind::ZedExtension => {
                    zed_extensions += 1;
                    if artifact.path != "zed-extension/extension.wasm" {
                        return Err(DeliveryError::InvalidPackage(
                            "Zed extension must be `zed-extension/extension.wasm`".to_owned(),
                        ));
                    }
                }
            }
        }

        Ok((node_backends, zed_extensions))
    }

    fn validate_required_artifacts(
        &self,
        node_backends: usize,
        zed_extensions: usize,
    ) -> Result<()> {
        let server = self
            .binaries
            .iter()
            .find(|artifact| artifact.kind == ArtifactKind::Rust && artifact.name == "wxml-lsp")
            .ok_or_else(|| {
                DeliveryError::InvalidPackage("manifest is missing the wxml-lsp binary".to_owned())
            })?;
        if server.path != "bin/wxml-lsp" {
            return Err(DeliveryError::InvalidPackage(
                "wxml-lsp binary must be `bin/wxml-lsp`".to_owned(),
            ));
        }
        if server.version != self.package_version {
            return Err(DeliveryError::InvalidPackage(
                "wxml-lsp binary version does not match the package version".to_owned(),
            ));
        }
        if node_backends != 1 {
            return Err(DeliveryError::InvalidPackage(
                "manifest must describe exactly one Node backend".to_owned(),
            ));
        }
        if zed_extensions > 1 {
            return Err(DeliveryError::InvalidPackage(
                "manifest must describe at most one Zed extension".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns the payload entry for a package-relative path.
    #[must_use]
    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|file| file.path == path)
    }

    /// Serializes the manifest as pretty JSON with a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| DeliveryError::InvalidPackage(error.to_string()))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// One release available from an index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    /// Release version.
    pub version: String,
    /// Target platform.
    pub platform: String,
    /// Type-bridge protocol version.
    pub protocol_version: u64,
    /// Minimum Node.js version.
    pub node_minimum_version: String,
    /// Archive file name relative to the source.
    pub archive: String,
    /// Lowercase SHA-256 of the archive.
    pub sha256: String,
    /// Archive size in bytes.
    pub size: u64,
}

impl Release {
    /// Validates one release record.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed versions, paths, hashes or sizes.
    pub fn validate(&self) -> Result<()> {
        validate_version(&self.version)?;
        validate_platform(&self.platform)?;
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&self.protocol_version) {
            return Err(DeliveryError::InvalidPackage(format!(
                "release protocol {} is not supported",
                self.protocol_version
            )));
        }
        Semver::parse(&self.node_minimum_version)?;
        validate_archive_name(&self.archive)?;
        validate_sha256(&self.sha256)?;
        if self.size == 0 || self.size > MAX_ARCHIVE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "release `{}` archive size is outside the accepted range",
                self.version
            )));
        }
        Ok(())
    }
}

/// The release index published next to archives.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Index {
    /// Index schema.
    pub schema_version: u32,
    /// Package identity.
    pub package: String,
    /// Known releases.
    pub releases: Vec<Release>,
}

impl Index {
    /// Creates an empty index for this package.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            package: PACKAGE_NAME.to_owned(),
            releases: Vec::new(),
        }
    }

    /// Validates the index and every release.
    ///
    /// # Errors
    ///
    /// Returns an error for schema, package, duplicate or malformed releases.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(DeliveryError::InvalidPackage(format!(
                "unsupported index schema {}",
                self.schema_version
            )));
        }
        if self.package != PACKAGE_NAME {
            return Err(DeliveryError::InvalidPackage(format!(
                "unexpected package `{}`",
                self.package
            )));
        }
        let mut seen = BTreeSet::new();
        for release in &self.releases {
            release.validate()?;
            if !seen.insert((release.version.as_str(), release.platform.as_str())) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "duplicate release {}/{}",
                    release.version, release.platform
                )));
            }
        }
        Ok(())
    }

    /// Finds one release by exact version and platform.
    ///
    /// # Errors
    ///
    /// Returns an error when no matching release exists.
    pub fn find(&self, version: &str, platform: &str) -> Result<&Release> {
        self.releases
            .iter()
            .find(|release| release.version == version && release.platform == platform)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!(
                    "no release {version} for platform {platform}"
                ))
            })
    }

    /// Returns the highest version for a platform.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform has no releases.
    pub fn latest(&self, platform: &str) -> Result<&Release> {
        self.releases
            .iter()
            .filter(|release| release.platform == platform && !is_prerelease(&release.version))
            .max_by(|left, right| compare_versions(&left.version, &right.version))
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!("no stable releases for platform {platform}"))
            })
    }

    /// Inserts or replaces one release, keeping a deterministic order.
    pub fn upsert(&mut self, release: Release) {
        self.releases
            .retain(|entry| entry.version != release.version || entry.platform != release.platform);
        self.releases.push(release);
        self.releases.sort_by(|left, right| {
            left.platform
                .cmp(&right.platform)
                .then_with(|| compare_versions(&left.version, &right.version))
        });
    }

    /// Serializes the index as pretty JSON with a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization fails.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| DeliveryError::InvalidPackage(error.to_string()))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// Reads a size-bounded file.
///
/// # Errors
///
/// Returns an error when the file cannot be read or exceeds `maximum`.
pub fn read_limited(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{}` must be a regular file",
            crate::paths::redact(path)
        )));
    }
    if metadata.len() > maximum {
        return Err(DeliveryError::SizeLimit(format!(
            "`{}` exceeds {} bytes",
            crate::paths::redact(path),
            maximum
        )));
    }
    let file = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| DeliveryError::io(path, error))?;
    if bytes.len() as u64 > maximum {
        return Err(DeliveryError::SizeLimit(format!(
            "`{}` changed while reading",
            crate::paths::redact(path)
        )));
    }
    Ok(bytes)
}

/// Reads and validates a manifest from a package root.
///
/// # Errors
///
/// Returns an error when the file is missing, too large, malformed or fails
/// validation.
pub fn read_manifest(root: &Path, expected: ExpectedManifest<'_>) -> Result<Manifest> {
    let path = root.join("manifest.json");
    let bytes = read_limited(&path, MAX_MANIFEST_BYTES)?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("manifest.json is malformed: {error}"))
    })?;
    manifest.validate(expected)?;
    Ok(manifest)
}

/// Reads and validates an index file.
///
/// # Errors
///
/// Returns an error when the file is missing, too large, malformed or fails
/// validation.
pub fn read_index(path: &Path) -> Result<Index> {
    parse_index(&read_limited(path, MAX_INDEX_BYTES)?)
}

/// Parses and validates a size-bounded schema v1 index.
///
/// # Errors
///
/// Returns an error when the bytes are malformed or fail validation.
pub fn parse_index(bytes: &[u8]) -> Result<Index> {
    let index: Index = serde_json::from_slice(bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("index.json is malformed: {error}"))
    })?;
    index.validate()?;
    Ok(index)
}

/// Computes the SHA-256 of a file.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or read.
pub fn sha256_file(path: &Path) -> Result<String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| DeliveryError::io(path, error))?;
    if !metadata.file_type().is_file() {
        return Err(DeliveryError::InvalidArgument(format!(
            "`{}` must be a regular file",
            crate::paths::redact(path)
        )));
    }
    let mut file = File::open(path).map_err(|error| DeliveryError::io(path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DeliveryError::io(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

/// Computes the SHA-256 of a byte slice.
#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(&hasher.finalize())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// Parses a `sha256sum`-compatible checksum file.
///
/// # Errors
///
/// Returns an error for malformed lines, unsafe paths or duplicate paths.
pub fn parse_sha256sums(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| DeliveryError::InvalidPackage("SHA256SUMS is not UTF-8".to_owned()))?;
    let mut entries = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let (digest, path) = line.split_once("  ").ok_or_else(|| {
            DeliveryError::InvalidPackage(format!("SHA256SUMS line {} is malformed", index + 1))
        })?;
        validate_sha256(digest)?;
        validate_relative_path(path, true)?;
        if entries.insert(path.to_owned(), digest.to_owned()).is_some() {
            return Err(DeliveryError::InvalidPackage(format!(
                "SHA256SUMS lists `{path}` twice"
            )));
        }
    }
    Ok(entries)
}

/// Formats checksum entries in stable path order.
#[must_use]
pub fn format_sha256sums(entries: &BTreeMap<String, String>) -> Vec<u8> {
    let mut output = String::new();
    for (path, digest) in entries {
        let _ = writeln!(output, "{digest}  {path}");
    }
    output.into_bytes()
}

/// Writes bytes to `path` atomically within the same directory.
///
/// # Errors
///
/// Returns an error when the temporary file or rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| DeliveryError::InvalidArgument("output path has no parent".to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|error| DeliveryError::io(parent, error))?;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    );
    let name = path.file_name().map_or_else(
        || "output".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temporary = parent.join(format!(".{name}.tmp-{nonce}"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&temporary)
        .map_err(|error| DeliveryError::io(&temporary, error))?;
    if let Err(error) = file.write_all(bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(DeliveryError::io(&temporary, error));
    }
    if let Err(error) = file.sync_all() {
        let _ = std::fs::remove_file(&temporary);
        return Err(DeliveryError::io(&temporary, error));
    }
    drop(file);
    set_mode(&temporary, mode)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(DeliveryError::io(path, error));
    }
    Ok(())
}

/// Sets a file mode on Unix.
///
/// # Errors
///
/// Returns an error on non-Unix hosts or when the mode cannot be set.
pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, permissions).map_err(|error| DeliveryError::io(path, error))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Err(DeliveryError::UnsupportedPlatform(
            "the installer currently supports Unix hosts only".to_owned(),
        ))
    }
}

/// Returns the current UTC time in RFC 3339 format.
#[must_use]
pub fn now_rfc3339() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| "1970-01-01T00:00:00Z".to_owned(),
        |duration| rfc3339_from_unix(duration.as_secs()),
    )
}

/// Formats Unix seconds as UTC RFC 3339.
#[must_use]
pub fn rfc3339_from_unix(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = remainder / 3_600;
    let minute = (remainder % 3_600) / 60;
    let second = remainder % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil-from-days algorithm.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_prime + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    })
    .unwrap_or(1);
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

/// Returns the archive name for a release.
///
/// # Errors
///
/// Returns an error when the version or platform is invalid.
pub fn archive_name(version: &str, platform: &str) -> Result<String> {
    validate_version(version)?;
    validate_platform(platform)?;
    Ok(format!("{PACKAGE_NAME}-{version}-{platform}.tar.gz"))
}

/// Joins a package-relative path below a root and checks the result stays there.
///
/// # Errors
///
/// Returns an error for unsafe relative paths.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative, false)?;
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, mode: &str) -> FileEntry {
        FileEntry {
            path: path.to_owned(),
            sha256: "0".repeat(64),
            size: 1,
            mode: mode.to_owned(),
        }
    }

    fn valid_manifest() -> Manifest {
        Manifest {
            schema_version: SCHEMA_VERSION,
            package: PACKAGE_NAME.to_owned(),
            package_version: "0.1.0".to_owned(),
            platform: "linux-x86_64".to_owned(),
            protocol_version: 2,
            node: NodeInfo {
                minimum_version: "24.0.0".to_owned(),
                backend_version: "0.1.0".to_owned(),
            },
            built_at: "2026-01-01T00:00:00Z".to_owned(),
            binaries: vec![
                BinaryArtifact {
                    kind: ArtifactKind::Rust,
                    name: "wxml-lsp".to_owned(),
                    path: "bin/wxml-lsp".to_owned(),
                    version: "0.1.0".to_owned(),
                    protocol_version: None,
                    sha256: "0".repeat(64),
                },
                BinaryArtifact {
                    kind: ArtifactKind::NodeBackend,
                    name: "node-backend".to_owned(),
                    path: "bin/wxml-type-backend/server.js".to_owned(),
                    version: "0.1.0".to_owned(),
                    protocol_version: Some(2),
                    sha256: "0".repeat(64),
                },
            ],
            files: vec![
                file("bin/wxml-lsp", "0755"),
                file("bin/wxml-type-backend/server.js", "0644"),
            ],
        }
    }

    #[test]
    fn valid_manifest_passes() {
        valid_manifest()
            .validate(ExpectedManifest::default())
            .unwrap();
    }

    #[test]
    fn rejects_expected_version_and_protocol_mismatches() {
        let manifest = valid_manifest();
        assert!(
            manifest
                .validate(ExpectedManifest {
                    version: Some("9.9.9"),
                    ..ExpectedManifest::default()
                })
                .is_err()
        );
        assert!(
            manifest
                .validate(ExpectedManifest {
                    protocol_version: Some(3),
                    ..ExpectedManifest::default()
                })
                .is_err()
        );
    }

    #[test]
    fn rejects_reserved_or_unsafe_payload_paths() {
        let mut manifest = valid_manifest();
        manifest.files[0].path = "../escape".to_owned();
        assert!(manifest.validate(ExpectedManifest::default()).is_err());

        let mut manifest = valid_manifest();
        manifest.files[0].path = "manifest.json".to_owned();
        assert!(manifest.validate(ExpectedManifest::default()).is_err());
    }

    #[test]
    fn rejects_bad_modes_and_duplicate_paths() {
        let mut manifest = valid_manifest();
        manifest.files[0].mode = "0777".to_owned();
        assert!(manifest.validate(ExpectedManifest::default()).is_err());

        let mut manifest = valid_manifest();
        manifest.files.push(manifest.files[0].clone());
        assert!(manifest.validate(ExpectedManifest::default()).is_err());
    }

    #[test]
    fn rejects_payload_over_the_per_file_limit() {
        let mut manifest = valid_manifest();
        manifest.files[0].size = MAX_FILE_BYTES + 1;
        assert!(manifest.validate(ExpectedManifest::default()).is_err());
    }

    #[test]
    fn rejects_manifest_without_server_or_backend() {
        let mut manifest = valid_manifest();
        manifest
            .binaries
            .retain(|artifact| artifact.name != "wxml-lsp");
        assert!(manifest.validate(ExpectedManifest::default()).is_err());

        let mut manifest = valid_manifest();
        manifest
            .binaries
            .retain(|artifact| artifact.kind != ArtifactKind::NodeBackend);
        assert!(manifest.validate(ExpectedManifest::default()).is_err());
    }

    #[test]
    fn index_upsert_is_deterministic_and_rejects_duplicates() {
        let mut index = Index::empty();
        for (version, archive) in [("0.2.0", "b.tar.gz"), ("0.1.0", "a.tar.gz")] {
            index.upsert(Release {
                version: version.to_owned(),
                platform: "linux-x86_64".to_owned(),
                protocol_version: 2,
                node_minimum_version: "24.0.0".to_owned(),
                archive: archive.to_owned(),
                sha256: "0".repeat(64),
                size: 10,
            });
        }
        index.validate().unwrap();
        assert_eq!(index.latest("linux-x86_64").unwrap().version, "0.2.0");
        index.upsert(Release {
            version: "0.2.0".to_owned(),
            platform: "linux-x86_64".to_owned(),
            protocol_version: 2,
            node_minimum_version: "24.0.0".to_owned(),
            archive: "c.tar.gz".to_owned(),
            sha256: "1".repeat(64),
            size: 11,
        });
        assert_eq!(index.releases.len(), 2);
        assert_eq!(
            index.find("0.2.0", "linux-x86_64").unwrap().archive,
            "c.tar.gz"
        );
    }

    #[test]
    fn rejects_bad_checksum_lines() {
        for text in [
            "deadbeef  file",
            "nothex  file",
            format!("{} file", "0".repeat(64)).as_str(),
        ] {
            assert!(parse_sha256sums(text.as_bytes()).is_err(), "{text}");
        }
        let digest = "0".repeat(64);
        let parsed = parse_sha256sums(format!("{digest}  a/b\n").as_bytes()).unwrap();
        assert_eq!(parsed.get("a/b").unwrap(), &digest);
    }

    #[test]
    fn reader_helpers_reject_missing_and_malformed_files() {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-manifest-read-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        assert!(read_manifest(&path, ExpectedManifest::default()).is_err());
        std::fs::write(path.join("manifest.json"), b"{").unwrap();
        assert!(read_manifest(&path, ExpectedManifest::default()).is_err());
        assert!(read_index(&path.join("missing.json")).is_err());
        std::fs::write(path.join("index.json"), b"{").unwrap();
        assert!(read_index(&path.join("index.json")).is_err());
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn formats_rfc3339_dates() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
