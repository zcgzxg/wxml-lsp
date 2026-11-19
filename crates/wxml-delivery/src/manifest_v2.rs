//! Manifest schema v2 for the Node-free native delivery path.
//!
//! Schema v1 (the legacy Node package format) stays in [`crate::manifest`].
//! The two schemas are never reinterpreted as one another: a v2 reader rejects
//! schema 1 explicitly, a v1 reader rejects schema 2, and the legacy reader
//! below projects only the four identity fields it needs for migration
//! detection.
//!
//! Structural validation here is byte-free. [`crate::verify_v2::verify_tree_v2`]
//! performs the on-disk hash and allowlist checks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DeliveryError, Result};
use crate::manifest::{
    FileEntry, MAX_EXTRACTED_BYTES, MAX_FILE_BYTES, MAX_FILES, MAX_MANIFEST_BYTES, PACKAGE_NAME,
    read_limited,
};
use crate::paths::{validate_platform, validate_relative_path, validate_sha256};
use crate::semver::validate_strict;
use crate::sri::{SHA512_SRI_PREFIX, Sha512Integrity};

pub use crate::index_v2::{INDEX_SCHEMA_V2, IndexV2, ReleaseV2, read_index_v2};
pub use crate::type_assets::{
    TypeAssetBundle, compute_type_asset_bundle, forbidden_runtime_path, glob_matches,
    validate_glob_pattern,
};

/// Native manifest schema version.
pub const MANIFEST_SCHEMA_V2: u32 = 2;
/// Installer schema this build implements.
pub const MAX_INSTALLER_SCHEMA: u32 = 2;
/// The only native platform supported by this release.
pub const NATIVE_PACKAGE_PLATFORM: &str = "linux-x86_64";
/// Engine name recorded in manifests and state.
pub const ENGINE_NAME: &str = "typescript";
/// Exact native engine version. Floating versions are rejected.
pub const ENGINE_VERSION: &str = "7.1.0-beta";
/// Exact upstream git commit (`gitHead`) for the engine.
pub const ENGINE_SOURCE_COMMIT: &str = "3e9236361fa97a9e6c685f4833ac457ae2d8c50a";
/// Upstream platform package name.
pub const ENGINE_DISTRIBUTION: &str = "@typescript/typescript-linux-x64";
/// Fixed provenance URL. The installer must never request it.
pub const ENGINE_DISTRIBUTION_URL: &str = "https://registry.npmjs.org/@typescript/typescript-linux-x64/-/typescript-linux-x64-7.1.0-beta.tgz";
/// Fixed SRI of the upstream tarball (provenance identity).
pub const ENGINE_DISTRIBUTION_INTEGRITY: &str = "sha512-kcD51ihf+pnI5KT4uQQYG04OpwznaZYBlIQhDhl8fflFd27NktxXsUeguxZyvUMyWfum7IF2bBS6Jv7ox0AgFQ==";
/// Fixed SHA-256 of the upstream tarball.
pub const ENGINE_DISTRIBUTION_SHA256: &str =
    "59128685cb55f44c9e6644ef5edb3cb321618d9547b4ee94c3038598ffd51855";
/// Fixed packed tarball size.
pub const ENGINE_DISTRIBUTION_SIZE: u64 = 10_175_135;
/// Fixed upstream tarball file count.
pub const ENGINE_DISTRIBUTION_FILE_COUNT: u64 = 122;
/// Fixed upstream unpacked size.
pub const ENGINE_DISTRIBUTION_UNPACKED_SIZE: u64 = 28_450_796;
/// Frozen native AST/API schema version.
pub const ENGINE_API_SCHEMA_VERSION: u64 = 9;
/// Frozen AST header size in bytes.
pub const ENGINE_AST_HEADER_BYTES: u64 = 64;
/// Engine license identifier.
pub const ENGINE_LICENSE: &str = "Apache-2.0";
/// Required native engine path inside the package.
pub const ENGINE_BINARY_PATH: &str = "engine/lib/tsc";
/// Required version probe prefix.
pub const ENGINE_PROBE_PREFIX: &str = "Version ";
/// Required standard library type asset name.
pub const STANDARD_LIB_ASSET: &str = "typescript-standard-lib";
/// Required standard library file count (`lib*.d.ts`, including `lib.d.ts`).
pub const STANDARD_LIB_FILE_COUNT: u64 = 116;
/// Required standard library include pattern.
pub const STANDARD_LIB_INCLUDE: &str = "lib*.d.ts";
/// Required miniprogram typings asset name.
pub const MINIPROGRAM_TYPINGS_ASSET: &str = "miniprogram-api-typings";
/// Exact miniprogram typings version.
pub const MINIPROGRAM_TYPINGS_VERSION: &str = "5.3.1";
/// Required miniprogram typings license.
pub const MINIPROGRAM_TYPINGS_LICENSE: &str = "MIT";
/// Required miniprogram typings root.
pub const MINIPROGRAM_TYPINGS_ROOT: &str = "typings/miniprogram-api-typings";
/// Required miniprogram typings payload file count.
///
/// The upstream `miniprogram-api-typings@5.3.1` tree has 26 files; the delivery
/// payload keeps 23 of them and excludes `package.json` (npm metadata),
/// `README-en.md` (English translation) and `typings.json` (legacy `typings`
/// metadata). This is the payload count, not the 20-file runtime `.d.ts`
/// closure pinned by the native backend.
pub const MINIPROGRAM_TYPINGS_FILE_COUNT: u64 = 23;
/// Upstream `miniprogram-api-typings@5.3.1` file count.
pub const MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT: u64 = 26;
/// Upstream `miniprogram-api-typings@5.3.1` unpacked byte count.
pub const MINIPROGRAM_TYPINGS_UPSTREAM_TOTAL_BYTES: u64 = 2_552_052;
/// Frozen delivery payload byte count (23 files, no excluded metadata).
pub const MINIPROGRAM_TYPINGS_TOTAL_BYTES: u64 = 2_548_075;
/// Frozen delivery payload bundle digest.
///
/// Algorithm: sorted `<sha256>  <root-relative path>\n` lines, then SHA-256.
/// This is the 23-file payload identity, not the 20-file runtime `.d.ts`
/// closure pinned by the native backend.
pub const MINIPROGRAM_TYPINGS_BUNDLE_DIGEST: &str =
    "ba3eb83410db79bd80727e9f38eef7cf0f0a3a3c7fde02f8d14467cf95bab46b";
/// Files present upstream but excluded from the delivery payload.
pub const MINIPROGRAM_TYPINGS_EXCLUDED_PATHS: &[&str] =
    &["package.json", "README-en.md", "typings.json"];
/// Exact root-relative allowlist of the frozen 23-file typings payload.
///
/// This is intentionally an exact list: a same-count substitute, a case
/// variant or any extra file must fail the layout check even before the
/// bundle digest is recomputed.
pub const MINIPROGRAM_TYPINGS_PAYLOAD_PATHS: &[&str] = &[
    "CHANGELOG.md",
    "LICENSE",
    "README.md",
    "index.d.ts",
    "types/index.d.ts",
    "types/wx/index.d.ts",
    "types/wx/lib.wx.api.d.ts",
    "types/wx/lib.wx.app.d.ts",
    "types/wx/lib.wx.behavior.d.ts",
    "types/wx/lib.wx.canvas.d.ts",
    "types/wx/lib.wx.cloud.d.ts",
    "types/wx/lib.wx.component.d.ts",
    "types/wx/lib.wx.event.d.ts",
    "types/wx/lib.wx.glass-easel.behavior.d.ts",
    "types/wx/lib.wx.glass-easel.component.d.ts",
    "types/wx/lib.wx.glass-easel.d.ts",
    "types/wx/lib.wx.glass-easel.trait-behavior.d.ts",
    "types/wx/lib.wx.glass-easel.utils.d.ts",
    "types/wx/lib.wx.page.d.ts",
    "types/wx/lib.wx.phys3D.d.ts",
    "types/wx/lib.wx.skyline.d.ts",
    "types/wx/lib.wx.wasm.d.ts",
    "types/wx/lib.wx.xr-frame.d.ts",
];

/// Returns whether `path` is inside the miniprogram typings root.
#[must_use]
pub fn is_miniprogram_typings_path(path: &str) -> bool {
    path == MINIPROGRAM_TYPINGS_ROOT
        || path
            .strip_prefix(MINIPROGRAM_TYPINGS_ROOT)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Returns whether a package-relative path is allowed by the frozen typings
/// payload layout.
///
/// Paths outside the `typings/` subtree return `true`; paths under `typings/`
/// but outside the frozen 23-file root return `false` so excluded names cannot
/// be re-introduced at a parent path. Callers that need the full package
/// layout apply their own rules for other roots.
#[must_use]
pub fn is_allowed_miniprogram_typings_path(path: &str) -> bool {
    let parent = MINIPROGRAM_TYPINGS_ROOT
        .split_once('/')
        .map_or(MINIPROGRAM_TYPINGS_ROOT, |(parent, _)| parent);
    if is_miniprogram_typings_path(path) {
        let relative = path
            .strip_prefix(MINIPROGRAM_TYPINGS_ROOT)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or_default();
        return MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.contains(&relative);
    }
    // Anything else below `typings/` is outside the frozen layout, including
    // `typings/README-en.md` or `typings/typings.json`.
    !(path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('/')))
}

/// Native engine RPC/frame protocol versions this build supports.
pub const SUPPORTED_NATIVE_PROTOCOL_VERSIONS: &[u64] = &[2];
/// Executable paths allowed to carry mode `0755`.
pub const ALLOWED_EXECUTABLES: &[&str] =
    &["bin/wxml-lsp", "bin/wxml-lsp-install", "engine/lib/tsc"];
/// Required engine license files.
pub const REQUIRED_ENGINE_LICENSE_FILES: &[&str] = &["engine/LICENSE", "engine/NOTICE.txt"];

/// Version probe contract for the native engine.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineProbe {
    /// Arguments passed to the engine binary.
    pub args: Vec<String>,
    /// Prefix that must precede the exact engine version.
    pub expect_prefix: String,
}

/// Fixed identity of the bundled native TypeScript engine.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineInfo {
    /// Engine name (`typescript`).
    pub name: String,
    /// Exact engine version.
    pub version: String,
    /// Upstream git commit (`gitHead`).
    pub source_commit: String,
    /// Upstream platform package name.
    pub distribution: String,
    /// Fixed upstream tarball URL; provenance only.
    pub distribution_url: String,
    /// Upstream tarball SRI.
    pub distribution_integrity: String,
    /// Upstream tarball SHA-256.
    pub distribution_sha256: String,
    /// Upstream tarball packed size.
    pub distribution_size: u64,
    /// Upstream tarball file count.
    pub distribution_file_count: u64,
    /// Upstream tarball unpacked size.
    pub distribution_unpacked_size: u64,
    /// Frozen AST/API schema version.
    pub api_schema_version: u64,
    /// Frozen AST header size.
    pub ast_header_bytes: u64,
    /// Engine license identifier.
    pub license: String,
    /// License files that must exist in the payload.
    pub license_files: Vec<String>,
    /// Native engine path inside the package.
    pub binary: String,
    /// Version probe contract.
    pub probe: EngineProbe,
}

/// One type-asset bundle recorded by the manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeAsset {
    /// Stable asset name.
    pub name: String,
    /// Exact asset version.
    pub version: String,
    /// Asset license identifier (`Apache-2.0` or `MIT`).
    pub license: String,
    /// Package-relative directory that contains the asset.
    pub root: String,
    /// Glob patterns relative to `root`.
    pub include: Vec<String>,
    /// Number of files matched by `root` and `include`.
    pub file_count: u64,
    /// Bundle digest over the sorted `<sha256>  <root-relative path>` lines.
    pub sha256: String,
    /// License files that must exist in the payload.
    pub license_files: Vec<String>,
}

/// Artifact kinds accepted in a native manifest.
///
/// `node-backend` is deliberately absent; serde rejects it as an unknown
/// variant, and schema v1 is handled only by the legacy projection reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeArtifactKind {
    /// Native Rust binary.
    Rust,
    /// Native TypeScript engine.
    NativeEngine,
    /// Zed extension WebAssembly module.
    ZedExtension,
}

/// One runnable artifact described by a native manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeBinaryArtifact {
    /// Artifact class.
    pub kind: NativeArtifactKind,
    /// Stable short name.
    pub name: String,
    /// Package-relative path.
    pub path: String,
    /// Artifact version.
    pub version: String,
    /// Engine RPC protocol version; required for `native-engine`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u64>,
    /// SHA-256 copied from the matching payload record.
    pub sha256: String,
}

/// One immutable schema v2 package manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestV2 {
    /// Manifest schema; must be [`MANIFEST_SCHEMA_V2`].
    pub schema_version: u32,
    /// Package identity (`wxml-lsp`).
    pub package: String,
    /// Release version.
    pub package_version: String,
    /// Target platform; must be [`NATIVE_PACKAGE_PLATFORM`].
    pub platform: String,
    /// Rust-to-engine RPC/frame protocol version.
    pub protocol_version: u64,
    /// Build timestamp (UTC, RFC 3339).
    pub built_at: String,
    /// Native engine identity and provenance.
    pub engine: EngineInfo,
    /// Typings and standard-library bundles.
    pub type_assets: Vec<TypeAsset>,
    /// Required artifacts.
    pub binaries: Vec<NativeBinaryArtifact>,
    /// Every payload file except `manifest.json` and `SHA256SUMS`.
    pub files: Vec<FileEntry>,
}

/// Optional expectations when validating a native manifest.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExpectedManifestV2<'a> {
    /// Exact release version.
    pub version: Option<&'a str>,
    /// Exact platform label.
    pub platform: Option<&'a str>,
    /// Exact engine RPC protocol version.
    pub protocol_version: Option<u64>,
}

/// Manifest schema detected on disk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestSchema {
    /// Legacy Node package schema.
    V1,
    /// Native schema.
    V2,
}

/// Minimal read-only projection of a legacy schema v1 manifest.
///
/// Only identity fields are exposed. The old Node fields (`node`,
/// `node-backend`, ...) are intentionally not parsed, and nothing is executed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyManifestProjection {
    /// Legacy schema version (always 1).
    pub schema_version: u32,
    /// Package identity.
    pub package: String,
    /// Legacy release version.
    pub package_version: String,
    /// Legacy platform label.
    pub platform: String,
}

impl ManifestV2 {
    /// Validates every structural invariant without reading payload bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for schema, identity, path, digest, license, artifact,
    /// type-asset or Node-runtime violations.
    pub fn validate(&self, expected: ExpectedManifestV2<'_>) -> Result<()> {
        self.validate_identity(expected)?;
        self.validate_engine()?;
        self.validate_files()?;
        self.validate_type_assets()?;
        self.validate_binaries()?;
        self.validate_required_payload()
    }

    fn validate_identity(&self, expected: ExpectedManifestV2<'_>) -> Result<()> {
        if self.schema_version != MANIFEST_SCHEMA_V2 {
            return Err(DeliveryError::InvalidPackage(format!(
                "unsupported native manifest schema {}",
                self.schema_version
            )));
        }
        if self.package != PACKAGE_NAME {
            return Err(DeliveryError::InvalidPackage(format!(
                "unexpected package `{}`",
                self.package
            )));
        }
        validate_strict(&self.package_version)?;
        if let Some(version) = expected.version {
            if self.package_version != version {
                return Err(DeliveryError::Integrity(format!(
                    "manifest version `{}` does not match requested `{version}`",
                    self.package_version
                )));
            }
        }
        validate_platform(&self.platform)?;
        if self.platform != NATIVE_PACKAGE_PLATFORM {
            return Err(DeliveryError::UnsupportedPlatform(format!(
                "native manifest platform `{}` is not `{NATIVE_PACKAGE_PLATFORM}`",
                self.platform
            )));
        }
        if let Some(platform) = expected.platform {
            if self.platform != platform {
                return Err(DeliveryError::Integrity(format!(
                    "manifest platform `{}` does not match expected `{platform}`",
                    self.platform
                )));
            }
        }
        if !SUPPORTED_NATIVE_PROTOCOL_VERSIONS.contains(&self.protocol_version) {
            return Err(DeliveryError::InvalidPackage(format!(
                "native protocol version {} is not supported",
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
        crate::state::validate_rfc3339_utc(&self.built_at)?;
        Ok(())
    }

    fn validate_engine(&self) -> Result<()> {
        let engine = &self.engine;
        let failures = [
            (engine.name == ENGINE_NAME, "engine.name"),
            (engine.version == ENGINE_VERSION, "engine.version"),
            (
                engine.source_commit == ENGINE_SOURCE_COMMIT,
                "engine.sourceCommit",
            ),
            (
                engine.distribution == ENGINE_DISTRIBUTION,
                "engine.distribution",
            ),
            (
                engine.distribution_url == ENGINE_DISTRIBUTION_URL,
                "engine.distributionUrl",
            ),
            (
                engine.distribution_integrity == ENGINE_DISTRIBUTION_INTEGRITY,
                "engine.distributionIntegrity",
            ),
            (
                engine.distribution_sha256 == ENGINE_DISTRIBUTION_SHA256,
                "engine.distributionSha256",
            ),
            (
                engine.distribution_size == ENGINE_DISTRIBUTION_SIZE,
                "engine.distributionSize",
            ),
            (
                engine.distribution_file_count == ENGINE_DISTRIBUTION_FILE_COUNT,
                "engine.distributionFileCount",
            ),
            (
                engine.distribution_unpacked_size == ENGINE_DISTRIBUTION_UNPACKED_SIZE,
                "engine.distributionUnpackedSize",
            ),
            (
                engine.api_schema_version == ENGINE_API_SCHEMA_VERSION,
                "engine.apiSchemaVersion",
            ),
            (
                engine.ast_header_bytes == ENGINE_AST_HEADER_BYTES,
                "engine.astHeaderBytes",
            ),
            (engine.license == ENGINE_LICENSE, "engine.license"),
            (engine.binary == ENGINE_BINARY_PATH, "engine.binary"),
        ];
        for (valid, field) in failures {
            if !valid {
                return Err(DeliveryError::InvalidPackage(format!(
                    "native manifest `{field}` does not match the frozen engine identity"
                )));
            }
        }
        if engine.distribution_integrity.len() <= SHA512_SRI_PREFIX.len() {
            return Err(DeliveryError::InvalidPackage(
                "engine.distributionIntegrity is empty".to_owned(),
            ));
        }
        Sha512Integrity::parse(&engine.distribution_integrity)?;
        validate_sha256(&engine.distribution_sha256)?;
        if engine.probe.args != ["--version"] {
            return Err(DeliveryError::InvalidPackage(
                "engine.probe.args must be exactly [\"--version\"]".to_owned(),
            ));
        }
        if engine.probe.expect_prefix != ENGINE_PROBE_PREFIX {
            return Err(DeliveryError::InvalidPackage(format!(
                "engine.probe.expectPrefix must be `{ENGINE_PROBE_PREFIX}`"
            )));
        }
        require_license_files(
            &engine.license_files,
            REQUIRED_ENGINE_LICENSE_FILES,
            &self.files,
            "engine.licenseFiles",
        )?;
        let engine_file = self.file(ENGINE_BINARY_PATH).ok_or_else(|| {
            DeliveryError::InvalidPackage("engine.binary is not listed in files[]".to_owned())
        })?;
        if engine_file.mode != "0755" {
            return Err(DeliveryError::InvalidPackage(
                "engine.binary must have mode 0755".to_owned(),
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
        let mut folded_paths = BTreeSet::new();
        let mut total = 0_u64;
        for file in &self.files {
            validate_relative_path(&file.path, false)?;
            validate_sha256(&file.sha256)?;
            let mode = file.mode_bits()?;
            if file.size == 0 {
                return Err(DeliveryError::InvalidPackage(format!(
                    "payload `{}` has an empty size",
                    file.path
                )));
            }
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
            if !folded_paths.insert(file.path.to_ascii_lowercase()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "payload path `{}` collides with another path on a case-insensitive filesystem",
                    file.path
                )));
            }
            if let Some(reason) = forbidden_runtime_path(&file.path) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "payload `{}` is not allowed in a Node-free package: {reason}",
                    file.path
                )));
            }
            if !is_allowed_miniprogram_typings_path(&file.path) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "payload `{}` is not part of the frozen miniprogram typings payload",
                    file.path
                )));
            }
            if mode == 0o755 && !ALLOWED_EXECUTABLES.contains(&file.path.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "payload `{}` is executable but not in the allowlist",
                    file.path
                )));
            }
        }
        Ok(())
    }

    fn validate_type_assets(&self) -> Result<()> {
        if self.type_assets.len() < 2 {
            return Err(DeliveryError::InvalidPackage(
                "manifest must list standard-library and miniprogram type assets".to_owned(),
            ));
        }
        let mut names = BTreeSet::new();
        for asset in &self.type_assets {
            validate_asset_name(&asset.name)?;
            validate_strict(&asset.version)?;
            if !matches!(asset.license.as_str(), "Apache-2.0" | "MIT") {
                return Err(DeliveryError::InvalidPackage(format!(
                    "type asset `{}` has unsupported license `{}`",
                    asset.name, asset.license
                )));
            }
            validate_relative_path(&asset.root, false)?;
            if asset.root.ends_with('/') {
                return Err(DeliveryError::InvalidPackage(format!(
                    "type asset `{}` root must not end with `/`",
                    asset.name
                )));
            }
            if asset.include.is_empty() {
                return Err(DeliveryError::InvalidPackage(format!(
                    "type asset `{}` has no include patterns",
                    asset.name
                )));
            }
            for pattern in &asset.include {
                validate_glob_pattern(pattern)?;
            }
            if asset.file_count == 0 || asset.file_count > MAX_FILES as u64 {
                return Err(DeliveryError::SizeLimit(format!(
                    "type asset `{}` fileCount is outside the accepted range",
                    asset.name
                )));
            }
            validate_sha256(&asset.sha256)?;
            require_license_files(
                &asset.license_files,
                &[],
                &self.files,
                &format!("typeAssets[{}].licenseFiles", asset.name),
            )?;
            let bundle = compute_type_asset_bundle(&self.files, &asset.root, &asset.include)?;
            if bundle.file_count != asset.file_count {
                return Err(DeliveryError::Integrity(format!(
                    "type asset `{}` declares {} files but {} match",
                    asset.name, asset.file_count, bundle.file_count
                )));
            }
            if bundle.sha256 != asset.sha256 {
                return Err(DeliveryError::Integrity(format!(
                    "type asset `{}` bundle digest does not match its files",
                    asset.name
                )));
            }
            if asset.name == MINIPROGRAM_TYPINGS_ASSET
                && bundle.total_bytes != MINIPROGRAM_TYPINGS_TOTAL_BYTES
            {
                return Err(DeliveryError::Integrity(format!(
                    "type asset `{}` payload total bytes {} do not match the frozen {}",
                    asset.name, bundle.total_bytes, MINIPROGRAM_TYPINGS_TOTAL_BYTES
                )));
            }
            if asset.name == MINIPROGRAM_TYPINGS_ASSET
                && bundle.sha256 != MINIPROGRAM_TYPINGS_BUNDLE_DIGEST
            {
                return Err(DeliveryError::Integrity(format!(
                    "type asset `{}` payload digest does not match the frozen bundle digest",
                    asset.name
                )));
            }
            if !names.insert(asset.name.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "duplicate type asset `{}`",
                    asset.name
                )));
            }
        }
        self.validate_required_type_assets()
    }

    fn validate_required_type_assets(&self) -> Result<()> {
        let standard = self
            .type_assets
            .iter()
            .find(|asset| asset.name == STANDARD_LIB_ASSET)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(
                    "manifest is missing `typescript-standard-lib`".to_owned(),
                )
            })?;
        if standard.version != ENGINE_VERSION
            || standard.license != ENGINE_LICENSE
            || standard.root != "engine/lib"
            || standard.include != [STANDARD_LIB_INCLUDE]
            || standard.file_count != STANDARD_LIB_FILE_COUNT
            || standard.license_files != ["engine/LICENSE", "engine/NOTICE.txt"]
        {
            return Err(DeliveryError::InvalidPackage(
                "typescript-standard-lib identity does not match the frozen contract".to_owned(),
            ));
        }
        let typings = self
            .type_assets
            .iter()
            .find(|asset| asset.name == MINIPROGRAM_TYPINGS_ASSET)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(
                    "manifest is missing `miniprogram-api-typings`".to_owned(),
                )
            })?;
        if typings.version != MINIPROGRAM_TYPINGS_VERSION
            || typings.license != MINIPROGRAM_TYPINGS_LICENSE
            || typings.root != MINIPROGRAM_TYPINGS_ROOT
            || typings.include != ["**"]
            || typings.file_count != MINIPROGRAM_TYPINGS_FILE_COUNT
            || typings.sha256 != MINIPROGRAM_TYPINGS_BUNDLE_DIGEST
            || typings.license_files != ["typings/miniprogram-api-typings/LICENSE"]
        {
            return Err(DeliveryError::InvalidPackage(
                "miniprogram-api-typings identity does not match the frozen contract".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_binaries(&self) -> Result<()> {
        let mut rust_names = BTreeSet::new();
        let mut native_engines = 0_usize;
        let mut zed_extensions = 0_usize;
        for artifact in &self.binaries {
            validate_asset_name(&artifact.name)?;
            validate_strict(&artifact.version)?;
            validate_sha256(&artifact.sha256)?;
            validate_relative_path(&artifact.path, false)?;
            let file = self.file(&artifact.path).ok_or_else(|| {
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
                NativeArtifactKind::Rust => {
                    self.validate_rust_artifact(artifact, file, &mut rust_names)?;
                }
                NativeArtifactKind::NativeEngine => {
                    native_engines += 1;
                    self.validate_native_artifact(artifact, file)?;
                }
                NativeArtifactKind::ZedExtension => {
                    zed_extensions += 1;
                    Self::validate_zed_artifact(artifact, file)?;
                }
            }
        }
        if rust_names != BTreeSet::from(["wxml-lsp".to_owned(), "wxml-lsp-install".to_owned()]) {
            return Err(DeliveryError::InvalidPackage(
                "manifest must contain exactly the wxml-lsp and wxml-lsp-install artifacts"
                    .to_owned(),
            ));
        }
        if native_engines != 1 {
            return Err(DeliveryError::InvalidPackage(
                "manifest must describe exactly one native engine".to_owned(),
            ));
        }
        if zed_extensions != 1 {
            return Err(DeliveryError::InvalidPackage(
                "manifest must describe exactly one Zed extension".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_rust_artifact(
        &self,
        artifact: &NativeBinaryArtifact,
        file: &FileEntry,
        names: &mut BTreeSet<String>,
    ) -> Result<()> {
        let expected_path = match artifact.name.as_str() {
            "wxml-lsp" => "bin/wxml-lsp",
            "wxml-lsp-install" => "bin/wxml-lsp-install",
            _ => {
                return Err(DeliveryError::InvalidPackage(format!(
                    "Rust artifact `{}` must be `wxml-lsp` or `wxml-lsp-install`",
                    artifact.name
                )));
            }
        };
        if artifact.path != expected_path {
            return Err(DeliveryError::InvalidPackage(format!(
                "Rust artifact `{}` must be at `{expected_path}`",
                artifact.name
            )));
        }
        if artifact.version != self.package_version {
            return Err(DeliveryError::InvalidPackage(format!(
                "Rust artifact `{}` version does not match the package version",
                artifact.name
            )));
        }
        if file.mode != "0755" {
            return Err(DeliveryError::InvalidPackage(format!(
                "Rust artifact `{}` must have mode 0755",
                artifact.name
            )));
        }
        if !names.insert(artifact.name.clone()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "duplicate Rust artifact `{}`",
                artifact.name
            )));
        }
        Ok(())
    }

    fn validate_native_artifact(
        &self,
        artifact: &NativeBinaryArtifact,
        file: &FileEntry,
    ) -> Result<()> {
        if artifact.name != "tsgo" || artifact.path != self.engine.binary {
            return Err(DeliveryError::InvalidPackage(
                "native engine artifact must be `tsgo` at engine.binary".to_owned(),
            ));
        }
        if artifact.version != self.engine.version {
            return Err(DeliveryError::InvalidPackage(
                "native engine artifact version does not match engine.version".to_owned(),
            ));
        }
        if artifact.protocol_version != Some(self.protocol_version) {
            return Err(DeliveryError::InvalidPackage(
                "native engine protocol does not match the manifest".to_owned(),
            ));
        }
        if file.mode != "0755" {
            return Err(DeliveryError::InvalidPackage(
                "native engine artifact must have mode 0755".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_zed_artifact(artifact: &NativeBinaryArtifact, file: &FileEntry) -> Result<()> {
        if artifact.name != "zed-extension" || artifact.path != "zed-extension/extension.wasm" {
            return Err(DeliveryError::InvalidPackage(
                "Zed extension must be `zed-extension/extension.wasm`".to_owned(),
            ));
        }
        if file.mode != "0644" {
            return Err(DeliveryError::InvalidPackage(
                "Zed extension must not be executable".to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_required_payload(&self) -> Result<()> {
        let required: &[(&str, &str)] = &[
            ("bin/wxml-lsp", "0755"),
            ("bin/wxml-lsp-install", "0755"),
            ("engine/lib/tsc", "0755"),
            ("engine/lib/tsc.sig", "0644"),
            ("engine/lib/lib.d.ts", "0644"),
            ("engine/lib/lib.es5.d.ts", "0644"),
            ("engine/lib/lib.es2020.d.ts", "0644"),
            ("engine/LICENSE", "0644"),
            ("engine/NOTICE.txt", "0644"),
            ("engine/package.json", "0644"),
            ("typings/miniprogram-api-typings/index.d.ts", "0644"),
            ("typings/miniprogram-api-typings/LICENSE", "0644"),
            ("zed-extension/extension.wasm", "0644"),
            ("zed-extension/extension.toml", "0644"),
            ("provenance.json", "0644"),
            ("THIRD_PARTY_LICENSES.md", "0644"),
        ];
        for (path, mode) in required {
            let file = self.file(path).ok_or_else(|| {
                DeliveryError::InvalidPackage(format!("manifest is missing required `{path}`"))
            })?;
            if file.mode != *mode {
                return Err(DeliveryError::InvalidPackage(format!(
                    "required file `{path}` must have mode {mode}"
                )));
            }
        }
        if !self.files.iter().any(|file| {
            file.path
                .starts_with("typings/miniprogram-api-typings/types/")
        }) {
            return Err(DeliveryError::InvalidPackage(
                "miniprogram typings must contain files below `types/`".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns the payload entry for a package-relative path.
    #[must_use]
    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|file| file.path == path)
    }

    /// Returns the native engine artifact.
    #[must_use]
    pub fn native_engine(&self) -> Option<&NativeBinaryArtifact> {
        self.binaries
            .iter()
            .find(|artifact| artifact.kind == NativeArtifactKind::NativeEngine)
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

fn validate_asset_name(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 64 {
        return Err(DeliveryError::InvalidPackage(
            "asset name must be 1-64 bytes".to_owned(),
        ));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(DeliveryError::InvalidPackage(format!(
            "asset name `{value}` contains unsupported characters"
        )));
    }
    Ok(())
}

fn require_license_files(
    declared: &[String],
    required: &[&str],
    files: &[FileEntry],
    label: &str,
) -> Result<()> {
    if declared.is_empty() {
        return Err(DeliveryError::InvalidPackage(format!("{label} is empty")));
    }
    for path in declared {
        validate_relative_path(path, false)?;
        if !files.iter().any(|file| file.path == *path) {
            return Err(DeliveryError::InvalidPackage(format!(
                "{label} `{path}` is not present in files[]"
            )));
        }
    }
    for path in required {
        if !declared.iter().any(|value| value == path) {
            return Err(DeliveryError::InvalidPackage(format!(
                "{label} must include `{path}`"
            )));
        }
    }
    Ok(())
}

/// Reads and validates a schema v2 manifest from a package root.
///
/// The schema is detected before deserializing, so a v1 Node manifest cannot
/// be silently reinterpreted as native.
///
/// # Errors
///
/// Returns an error when the file is missing, too large, malformed, has a
/// non-v2 schema or fails validation.
pub fn read_manifest_v2(root: &Path, expected: ExpectedManifestV2<'_>) -> Result<ManifestV2> {
    let path = root.join("manifest.json");
    let bytes = read_limited(&path, MAX_MANIFEST_BYTES)?;
    parse_manifest_v2(&bytes, expected)
}

/// Parses and validates schema v2 manifest bytes.
///
/// # Errors
///
/// See [`read_manifest_v2`].
pub fn parse_manifest_v2(bytes: &[u8], expected: ExpectedManifestV2<'_>) -> Result<ManifestV2> {
    match detect_schema(bytes)? {
        ManifestSchema::V2 => {}
        ManifestSchema::V1 => {
            return Err(DeliveryError::InvalidPackage(
                "manifest schema 1 is a legacy Node package, not a schema v2 native package"
                    .to_owned(),
            ));
        }
    }
    let manifest: ManifestV2 = serde_json::from_slice(bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("manifest.json is malformed: {error}"))
    })?;
    manifest.validate(expected)?;
    Ok(manifest)
}

/// Detects the manifest schema on disk without parsing the body.
///
/// # Errors
///
/// Returns an error for a missing, oversized, malformed or unknown schema.
pub fn read_manifest_schema(root: &Path) -> Result<ManifestSchema> {
    let bytes = read_limited(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    detect_schema(&bytes)
}

fn detect_schema(bytes: &[u8]) -> Result<ManifestSchema> {
    #[derive(Deserialize)]
    struct SchemaProbe {
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
    }
    let probe: SchemaProbe = serde_json::from_slice(bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("manifest.json is malformed: {error}"))
    })?;
    match probe.schema_version {
        1 => Ok(ManifestSchema::V1),
        MANIFEST_SCHEMA_V2 => Ok(ManifestSchema::V2),
        other => Err(DeliveryError::InvalidPackage(format!(
            "unsupported manifest schema {other}"
        ))),
    }
}

/// Reads only the identity fields of a legacy schema v1 manifest.
///
/// The old Node-specific fields are not parsed and nothing is executed. The
/// directory is left untouched.
///
/// # Errors
///
/// Returns an error when the file is malformed, is not schema 1, has an
/// unexpected package, or has an invalid version/platform.
pub fn read_legacy_manifest(root: &Path) -> Result<LegacyManifestProjection> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Projection {
        schema_version: u32,
        package: String,
        package_version: String,
        platform: String,
    }
    let bytes = read_limited(&root.join("manifest.json"), MAX_MANIFEST_BYTES)?;
    let projection: Projection = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("manifest.json is malformed: {error}"))
    })?;
    if projection.schema_version != 1 {
        return Err(DeliveryError::InvalidPackage(format!(
            "legacy reader only accepts schema 1, found {}",
            projection.schema_version
        )));
    }
    if projection.package != PACKAGE_NAME {
        return Err(DeliveryError::InvalidPackage(format!(
            "unexpected package `{}`",
            projection.package
        )));
    }
    validate_strict(&projection.package_version)?;
    validate_platform(&projection.platform)?;
    if projection.platform != NATIVE_PACKAGE_PLATFORM {
        return Err(DeliveryError::UnsupportedPlatform(format!(
            "legacy manifest platform `{}` is not `{NATIVE_PACKAGE_PLATFORM}`",
            projection.platform
        )));
    }
    Ok(LegacyManifestProjection {
        schema_version: projection.schema_version,
        package: projection.package,
        package_version: projection.package_version,
        platform: projection.platform,
    })
}

/// Returns the manifest payload map for callers that need deterministic order.
#[must_use]
pub fn payload_map(manifest: &ManifestV2) -> BTreeMap<&str, &FileEntry> {
    manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_handles_star_and_double_star() {
        assert!(glob_matches("lib*.d.ts", "lib.d.ts"));
        assert!(glob_matches("lib*.d.ts", "lib.es2020.d.ts"));
        assert!(!glob_matches("lib*.d.ts", "nested/lib.d.ts"));
        assert!(glob_matches("**", "index.d.ts"));
        assert!(glob_matches("**", "types/a/b.d.ts"));
        assert!(glob_matches("types/**", "types/a/b.d.ts"));
        assert!(!glob_matches("types/**", "other/a.d.ts"));
        assert!(glob_matches("a/**/b", "a/b"));
        assert!(glob_matches("a/**/b", "a/x/y/b"));
    }

    #[test]
    fn rejects_unsupported_glob_syntax() {
        for pattern in ["", "/abs", "../x", "a\\b", "a?b", "a[b]", "**/../x", "a//b"] {
            assert!(validate_glob_pattern(pattern).is_err(), "{pattern}");
        }
        validate_glob_pattern("lib*.d.ts").unwrap();
        validate_glob_pattern("**").unwrap();
    }

    #[test]
    fn forbidden_runtime_paths_are_lexically_detected() {
        for path in [
            "node_modules/x",
            "a/node_modules/x",
            "bin/wxml-type-backend/server.js",
            "bin/wxml-type-backend/x",
            "server.js",
            "lib/x.cjs",
            "lib/x.mjs",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "typings/package.json",
            "typings/miniprogram-api-typings/package.json",
            "typings/miniprogram-api-typings/Package.JSON",
        ] {
            assert!(
                forbidden_runtime_path(path).is_some(),
                "path should be forbidden: {path}"
            );
        }
        assert!(forbidden_runtime_path("engine/package.json").is_none());
        assert!(forbidden_runtime_path("engine/lib/lib.d.ts").is_none());
    }

    #[test]
    fn typings_payload_allowlist_is_exact_and_matches_upstream_arithmetic() {
        assert_eq!(
            u64::try_from(MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.len()).unwrap(),
            MINIPROGRAM_TYPINGS_FILE_COUNT
        );
        assert_eq!(
            MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT,
            MINIPROGRAM_TYPINGS_FILE_COUNT
                + u64::try_from(MINIPROGRAM_TYPINGS_EXCLUDED_PATHS.len()).unwrap()
        );
        let unique: BTreeSet<&str> = MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.iter().copied().collect();
        assert_eq!(unique.len(), MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.len());
        for excluded in MINIPROGRAM_TYPINGS_EXCLUDED_PATHS {
            assert!(!MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.contains(excluded));
        }
        for relative in MINIPROGRAM_TYPINGS_PAYLOAD_PATHS {
            assert!(is_allowed_miniprogram_typings_path(&format!(
                "{MINIPROGRAM_TYPINGS_ROOT}/{relative}"
            )));
        }
    }

    #[test]
    fn typings_layout_rejects_excluded_case_variants_and_siblings() {
        for relative in [
            "README-en.md",
            "README-EN.MD",
            "typings.json",
            "Typings.JSON",
            "types/extra.d.ts",
            "Types/index.d.ts",
            "package.json",
        ] {
            let path = format!("{MINIPROGRAM_TYPINGS_ROOT}/{relative}");
            assert!(
                !is_allowed_miniprogram_typings_path(&path),
                "path must be rejected: {path}"
            );
        }
        for path in [
            "typings/README-en.md",
            "typings/TYPINGS.JSON",
            "typings/miniprogram-api-typings-extra/index.d.ts",
        ] {
            assert!(
                !is_allowed_miniprogram_typings_path(path),
                "sibling path must be rejected: {path}"
            );
        }
        assert!(!is_allowed_miniprogram_typings_path(
            MINIPROGRAM_TYPINGS_ROOT
        ));
        assert!(is_allowed_miniprogram_typings_path("engine/lib/lib.d.ts"));
        assert!(!is_miniprogram_typings_path(
            "typings/miniprogram-api-typings-extra/x.d.ts"
        ));
        assert!(is_miniprogram_typings_path(
            "typings/miniprogram-api-typings/types/index.d.ts"
        ));
    }

    #[test]
    fn schema_detection_never_reinterprets_v1() {
        let v1 = br#"{"schemaVersion":1,"package":"wxml-lsp","packageVersion":"0.1.0","platform":"linux-x86_64","node":{}}"#;
        assert_eq!(detect_schema(v1).unwrap(), ManifestSchema::V1);
        let error = parse_manifest_v2(v1, ExpectedManifestV2::default()).unwrap_err();
        assert!(error.to_string().contains("legacy Node package"), "{error}");
        let v2 = br#"{"schemaVersion":2}"#;
        assert_eq!(detect_schema(v2).unwrap(), ManifestSchema::V2);
        assert!(detect_schema(br#"{"schemaVersion":3}"#).is_err());
    }
}
