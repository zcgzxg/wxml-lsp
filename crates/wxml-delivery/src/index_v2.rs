//! Schema v2 release index for the native delivery path.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DeliveryError, Result};
use crate::manifest::{MAX_INDEX_BYTES, PACKAGE_NAME, archive_name, read_limited};
use crate::manifest_v2::{
    ENGINE_API_SCHEMA_VERSION, ENGINE_AST_HEADER_BYTES, ENGINE_DISTRIBUTION,
    ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_SOURCE_COMMIT, ENGINE_VERSION, MAX_INSTALLER_SCHEMA,
    ManifestV2, NATIVE_PACKAGE_PLATFORM, SUPPORTED_NATIVE_PROTOCOL_VERSIONS,
};
use crate::paths::{validate_archive_name, validate_platform, validate_sha256};
use crate::semver::{compare_versions, is_prerelease, validate_strict};
use crate::sri::Sha512Integrity;

/// Native index schema version.
pub const INDEX_SCHEMA_V2: u32 = 2;

/// One release in a schema v2 index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseV2 {
    /// Release version.
    pub version: String,
    /// Target platform.
    pub platform: String,
    /// Engine RPC/frame protocol version.
    pub protocol_version: u64,
    /// Exact engine version.
    pub engine_version: String,
    /// Engine source commit.
    pub engine_source_commit: String,
    /// Engine protocol version; must equal `protocolVersion`.
    pub engine_protocol_version: u64,
    /// Frozen AST/API schema version.
    pub engine_api_schema_version: u64,
    /// Frozen AST header size.
    pub engine_ast_header_bytes: u64,
    /// Engine platform package name.
    pub engine_distribution: String,
    /// Engine tarball SRI.
    pub engine_integrity: String,
    /// Minimum installer schema; must be `<= 2`.
    pub min_installer_schema: u32,
    /// Release archive file name relative to the source base.
    pub archive: String,
    /// Release archive SRI.
    pub archive_integrity: String,
    /// Release archive SHA-256.
    pub sha256: String,
    /// Release archive size.
    pub size: u64,
}

impl ReleaseV2 {
    /// Validates one native release record.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed identity, digests, archive names or
    /// installer schema requirements.
    pub fn validate(&self) -> Result<()> {
        validate_strict(&self.version)?;
        validate_platform(&self.platform)?;
        if self.platform != NATIVE_PACKAGE_PLATFORM {
            return Err(DeliveryError::UnsupportedPlatform(format!(
                "native release platform `{}` is not `{NATIVE_PACKAGE_PLATFORM}`",
                self.platform
            )));
        }
        if !SUPPORTED_NATIVE_PROTOCOL_VERSIONS.contains(&self.protocol_version) {
            return Err(DeliveryError::InvalidPackage(format!(
                "release protocol {} is not supported",
                self.protocol_version
            )));
        }
        if self.engine_version != ENGINE_VERSION
            || self.engine_source_commit != ENGINE_SOURCE_COMMIT
            || self.engine_distribution != ENGINE_DISTRIBUTION
            || self.engine_integrity != ENGINE_DISTRIBUTION_INTEGRITY
            || self.engine_api_schema_version != ENGINE_API_SCHEMA_VERSION
            || self.engine_ast_header_bytes != ENGINE_AST_HEADER_BYTES
        {
            return Err(DeliveryError::InvalidPackage(
                "release engine identity does not match the frozen contract".to_owned(),
            ));
        }
        if self.engine_protocol_version != self.protocol_version {
            return Err(DeliveryError::InvalidPackage(
                "release engineProtocolVersion does not match protocolVersion".to_owned(),
            ));
        }
        if self.min_installer_schema == 0 || self.min_installer_schema > MAX_INSTALLER_SCHEMA {
            return Err(DeliveryError::InvalidPackage(format!(
                "release requires installer schema {}, this build supports at most {MAX_INSTALLER_SCHEMA}",
                self.min_installer_schema
            )));
        }
        validate_archive_name(&self.archive)?;
        if self.archive != archive_name(&self.version, &self.platform)? {
            return Err(DeliveryError::InvalidPackage(
                "release archive name must be `wxml-lsp-<version>-<platform>.tar.gz`".to_owned(),
            ));
        }
        Sha512Integrity::parse(&self.archive_integrity)?;
        validate_sha256(&self.sha256)?;
        if self.size == 0 || self.size > crate::manifest::MAX_ARCHIVE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "release `{}` archive size is outside the accepted range",
                self.version
            )));
        }
        Ok(())
    }

    /// Verifies a downloaded release archive against `archiveIntegrity`,
    /// `sha256` and `size` before extraction.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-regular file, a size mismatch or either
    /// digest mismatch.
    pub fn verify_archive(&self, path: &Path) -> Result<crate::sri::VerifiedAsset> {
        crate::sri::verify_asset_file(path, &self.archive_integrity, &self.sha256, self.size)
    }

    /// Checks that the release identity matches a native manifest.
    #[must_use]
    #[allow(clippy::suspicious_operation_groupings)]
    pub fn matches_manifest(&self, manifest: &ManifestV2) -> bool {
        self.version == manifest.package_version
            && self.platform == manifest.platform
            && self.protocol_version == manifest.protocol_version
            && self.engine_version == manifest.engine.version
            && self.engine_source_commit == manifest.engine.source_commit
            && self.engine_protocol_version == manifest.protocol_version
            && self.engine_api_schema_version == manifest.engine.api_schema_version
            && self.engine_ast_header_bytes == manifest.engine.ast_header_bytes
            && self.engine_distribution == manifest.engine.distribution
            && self.engine_integrity == manifest.engine.distribution_integrity
    }
}

/// Native release index published next to archives.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexV2 {
    /// Index schema; must be [`INDEX_SCHEMA_V2`].
    pub schema_version: u32,
    /// Package identity.
    pub package: String,
    /// Known releases.
    pub releases: Vec<ReleaseV2>,
}

impl IndexV2 {
    /// Creates an empty native index.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema_version: INDEX_SCHEMA_V2,
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
        if self.schema_version != INDEX_SCHEMA_V2 {
            return Err(DeliveryError::InvalidPackage(format!(
                "unsupported native index schema {}",
                self.schema_version
            )));
        }
        if self.package != PACKAGE_NAME {
            return Err(DeliveryError::InvalidPackage(format!(
                "unexpected package `{}`",
                self.package
            )));
        }
        if self.releases.is_empty() {
            return Err(DeliveryError::InvalidPackage(
                "native index contains no releases".to_owned(),
            ));
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
    pub fn find(&self, version: &str, platform: &str) -> Result<&ReleaseV2> {
        self.releases
            .iter()
            .find(|release| release.version == version && release.platform == platform)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!(
                    "no native release {version} for platform {platform}"
                ))
            })
    }

    /// Returns the highest stable native release for a platform.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform has no stable releases.
    pub fn latest(&self, platform: &str) -> Result<&ReleaseV2> {
        self.releases
            .iter()
            .filter(|release| release.platform == platform && !is_prerelease(&release.version))
            .max_by(|left, right| compare_versions(&left.version, &right.version))
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!(
                    "no stable native releases for platform {platform}"
                ))
            })
    }

    /// Inserts or replaces one release, keeping a deterministic order.
    pub fn upsert(&mut self, release: ReleaseV2) {
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

/// Reads and validates a schema v2 index file.
///
/// # Errors
///
/// Returns an error when the file is missing, oversized, malformed or fails
/// validation.
pub fn read_index_v2(path: &Path) -> Result<IndexV2> {
    let bytes = read_limited(path, MAX_INDEX_BYTES)?;
    let index: IndexV2 = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("index.json is malformed: {error}"))
    })?;
    index.validate()?;
    Ok(index)
}
