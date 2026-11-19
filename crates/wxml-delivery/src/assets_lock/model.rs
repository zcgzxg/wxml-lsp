//! Serde model, canonical form and field-level invariants for `assets.lock.json`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{DeliveryError, Result};
use crate::manifest::{
    FileEntry, MAX_ARCHIVE_BYTES, MAX_EXTRACTED_BYTES, MAX_FILE_BYTES, MAX_FILES, sha256_bytes,
};
use crate::paths::validate_relative_path;
use crate::sri::Sha512Integrity;
use crate::type_assets::{compute_type_asset_bundle, validate_glob_pattern};

use super::{MAX_ASSET_NAME_BYTES, MAX_ASSETS_LOCK_BYTES, MAX_TARBALL_URL_BYTES, NPM_REGISTRY};

/// Kind of an upstream asset. Only npm tarballs are accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    /// Platform-specific npm tarball (the native engine).
    NpmPlatformTarball,
    /// Generic npm tarball (the miniprogram typings).
    NpmTarball,
}

/// Registry metadata kept only for cross-checking, never for trust decisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetProvenance {
    /// Registry origin (`https://registry.npmjs.org`).
    pub registry: String,
    /// Registry `dist.shasum` (informational, never a security digest).
    pub dist_sha1: String,
    /// Registry publish timestamp (informational).
    pub published_at: String,
}

/// Packed tarball identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetTarball {
    /// Exact npm tarball URL.
    pub url: String,
    /// Packed tarball size in bytes.
    pub size: u64,
    /// `sha512-<base64>` SRI of the tarball.
    pub integrity: String,
    /// Lowercase SHA-256 of the tarball.
    pub sha256: String,
}

/// Complete upstream tree after stripping the npm `package/` prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamTree {
    /// Number of `files[]` entries.
    pub file_count: u64,
    /// Sum of `files[].size`.
    pub unpacked_size: u64,
    /// Mandatory npm tarball prefix.
    pub strip_prefix: String,
    /// Canonical tree fingerprint recomputed from `files[]`.
    pub tree_digest: String,
    /// Complete upstream file table.
    pub files: Vec<FileEntry>,
}

/// One upstream file dropped from the delivery payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludedPath {
    /// Upstream-relative path.
    pub path: String,
    /// Why the file does not enter the payload.
    pub reason: String,
}

/// Audit rules that map the upstream tree onto the payload tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetExtract {
    /// Upstream-relative include globs.
    pub include: Vec<String>,
    /// Explicit exclusions; the union must classify every upstream file.
    pub exclude: Vec<ExcludedPath>,
    /// Upstream-relative paths that must be executable.
    pub executables: Vec<String>,
}

/// One type-asset bundle identity declared for the payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetBundle {
    /// Bundle name (must match a manifest `typeAssets[].name`).
    pub name: String,
    /// Bundle root below the payload root.
    pub root: String,
    /// Bundle include globs relative to `root`.
    pub include: Vec<String>,
    /// Number of matched payload files.
    pub file_count: u64,
    /// Sum of matched payload bytes.
    pub total_bytes: u64,
    /// Bundle digest.
    pub sha256: String,
}

/// Exact delivery payload derived from the upstream tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetPayload {
    /// Number of `files[]` entries.
    pub file_count: u64,
    /// Sum of `files[].size`.
    pub total_bytes: u64,
    /// Canonical tree fingerprint recomputed from `files[]`.
    pub tree_digest: String,
    /// Package-relative payload file table.
    pub files: Vec<FileEntry>,
    /// Bundle identities consumed by the manifest.
    pub bundles: Vec<AssetBundle>,
}

/// One locked upstream asset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    /// Stable asset name.
    pub name: String,
    /// Asset kind.
    pub kind: AssetKind,
    /// Exact npm package name.
    pub npm_package: String,
    /// Strict semantic version.
    pub version: String,
    /// Upstream git commit; required for the engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    /// Accepted operating systems.
    pub os: Vec<String>,
    /// Accepted CPU architectures.
    pub cpu: Vec<String>,
    /// Registry cross-check metadata.
    pub provenance: AssetProvenance,
    /// SPDX license identifier.
    pub license: String,
    /// Package-relative license files distributed with the payload.
    pub license_files: Vec<String>,
    /// Final package-relative root.
    pub package_root: String,
    /// Packed tarball identity.
    pub tarball: AssetTarball,
    /// Complete upstream tree.
    pub upstream: UpstreamTree,
    /// Include/exclude audit rules.
    pub extract: AssetExtract,
    /// Exact delivery payload.
    pub payload: AssetPayload,
}

/// Repository-level asset lock.
///
/// The L1 API is not frozen; see the module-level stability note.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetsLock {
    /// Lock schema (`1`).
    pub schema_version: u32,
    /// Locked assets.
    pub assets: Vec<Asset>,
}

impl AssetsLock {
    /// Parses and normalizes a lock without applying semantic validation.
    ///
    /// The input size is limited before parsing. Arrays are sorted into
    /// canonical form, so two semantically equal locks serialize identically
    /// regardless of the original key or array order.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryError::SizeLimit`] for an oversized input and
    /// [`DeliveryError::InvalidPackage`] for malformed JSON or unknown fields.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let size = u64::try_from(bytes.len()).map_err(|_| {
            DeliveryError::SizeLimit("assets lock size does not fit u64".to_owned())
        })?;
        if size > MAX_ASSETS_LOCK_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "assets lock exceeds {MAX_ASSETS_LOCK_BYTES} bytes"
            )));
        }
        let mut lock: Self = serde_json::from_slice(bytes).map_err(|error| {
            DeliveryError::InvalidPackage(format!(
                "assets lock is malformed at line {} column {}",
                error.line(),
                error.column()
            ))
        })?;
        lock.normalize();
        Ok(lock)
    }

    /// Returns one asset by its exact frozen name.
    #[must_use]
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }

    /// Serializes the lock into its canonical byte form.
    ///
    /// Canonical form is 2-space pretty JSON in struct declaration order,
    /// UTF-8, LF line endings and one trailing LF. Arrays are sorted by
    /// `name`/`path` in byte order.
    ///
    /// # Errors
    ///
    /// Returns an error only if the model cannot be serialized.
    pub fn to_canonical_json(&self) -> Result<Vec<u8>> {
        let mut lock = self.clone();
        lock.normalize();
        let mut bytes = serde_json::to_vec_pretty(&lock).map_err(|error| {
            DeliveryError::InvalidPackage(format!("assets lock cannot be serialized: {error}"))
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Returns the SHA-256 of [`Self::to_canonical_json`].
    ///
    /// # Errors
    ///
    /// Returns an error only if the model cannot be serialized.
    pub fn lock_sha256(&self) -> Result<String> {
        Ok(sha256_bytes(&self.to_canonical_json()?))
    }

    fn normalize(&mut self) {
        for asset in &mut self.assets {
            sort_strings(&mut asset.os);
            sort_strings(&mut asset.cpu);
            sort_strings(&mut asset.license_files);
            sort_strings(&mut asset.extract.include);
            sort_strings(&mut asset.extract.executables);
            asset
                .extract
                .exclude
                .sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
            asset
                .upstream
                .files
                .sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
            asset
                .payload
                .files
                .sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
            asset
                .payload
                .bundles
                .sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
            for bundle in &mut asset.payload.bundles {
                sort_strings(&mut bundle.include);
            }
        }
        self.assets
            .sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    }
}

fn sort_strings(values: &mut [String]) {
    values.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
}

impl AssetProvenance {
    pub(super) fn validate(&self) -> Result<()> {
        if self.registry != NPM_REGISTRY {
            return Err(DeliveryError::UntrustedSource(
                "assets lock provenance registry must be `https://registry.npmjs.org`".to_owned(),
            ));
        }
        if self.dist_sha1.len() != 40
            || !self
                .dist_sha1
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DeliveryError::InvalidPackage(
                "assets lock provenance distSha1 must be 40 lowercase hex characters".to_owned(),
            ));
        }
        validate_published_at(&self.published_at)
    }
}

impl AssetTarball {
    pub(super) fn validate(&self, npm_package: &str, version: &str) -> Result<()> {
        validate_tarball_url(&self.url, npm_package, version)?;
        if self.size == 0 {
            return Err(DeliveryError::InvalidPackage(
                "assets lock tarball size is a zero placeholder".to_owned(),
            ));
        }
        if self.size > MAX_ARCHIVE_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "asset archive exceeds {MAX_ARCHIVE_BYTES} bytes"
            )));
        }
        Sha512Integrity::parse(&self.integrity)?;
        require_non_placeholder_digest(&self.sha256, "tarball SHA-256")
    }
}

pub(super) fn validate_asset_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_ASSET_NAME_BYTES {
        return Err(DeliveryError::InvalidPackage(
            "assets lock asset name must be 1-64 bytes".to_owned(),
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(DeliveryError::InvalidPackage(
            "assets lock asset name contains unsupported characters".to_owned(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(DeliveryError::InvalidPackage(
            "assets lock asset name must not start or end with `-`".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_npm_package(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 214 {
        return Err(DeliveryError::InvalidPackage(
            "assets lock npmPackage must be 1-214 bytes".to_owned(),
        ));
    }
    if value.chars().any(char::is_control) || value.contains(char::is_whitespace) {
        return Err(DeliveryError::InvalidPackage(
            "assets lock npmPackage contains whitespace or control characters".to_owned(),
        ));
    }
    let (scope, name) = match value.split_once('/') {
        Some((scope, name)) => {
            if !scope.starts_with('@') || scope.len() == 1 {
                return Err(DeliveryError::InvalidPackage(
                    "assets lock npmPackage has a malformed scope".to_owned(),
                ));
            }
            (Some(scope), name)
        }
        None => (None, value),
    };
    if name.is_empty() || name.contains('/') {
        return Err(DeliveryError::InvalidPackage(
            "assets lock npmPackage has a malformed name".to_owned(),
        ));
    }
    let valid = |part: &str| {
        part.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    };
    if !valid(name) || scope.is_some_and(|scope| !valid(&scope[1..])) {
        return Err(DeliveryError::InvalidPackage(
            "assets lock npmPackage contains unsupported characters".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn require_non_placeholder_digest(value: &str, label: &str) -> Result<()> {
    crate::paths::validate_sha256(value)?;
    if value.bytes().all(|byte| byte == b'0') {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock {label} is an all-zero placeholder"
        )));
    }
    Ok(())
}

fn validate_tarball_url(url: &str, npm_package: &str, version: &str) -> Result<()> {
    let path = tarball_path(url)?;
    validate_tarball_path(path, npm_package, version)
}

fn tarball_path(url: &str) -> Result<&str> {
    if url.is_empty() || url.len() > MAX_TARBALL_URL_BYTES {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL is empty or too long".to_owned(),
        ));
    }
    if url
        .chars()
        .any(|ch| ch.is_control() || ch == '\\' || ch == '?' || ch == '#' || ch == '%')
    {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL contains forbidden characters".to_owned(),
        ));
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL must use https".to_owned(),
        ));
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL has no path".to_owned(),
        ));
    };
    if authority != "registry.npmjs.org" {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL must target registry.npmjs.org without userinfo or port"
                .to_owned(),
        ));
    }
    if path.is_empty()
        || path
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL has an unsafe path".to_owned(),
        ));
    }
    Ok(path)
}

fn validate_tarball_path(path: &str, npm_package: &str, version: &str) -> Result<()> {
    let expected_file = format!(
        "{}-{version}.tgz",
        npm_package
            .rsplit_once('/')
            .map_or(npm_package, |(_, name)| name)
    );
    let segments: Vec<&str> = path.split('/').collect();
    let Some((file, parents)) = segments.split_last() else {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL has no file name".to_owned(),
        ));
    };
    if *file != expected_file {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL file name does not match npmPackage@version".to_owned(),
        ));
    }
    let expected_parents: Vec<&str> = match npm_package.split_once('/') {
        Some((scope, name)) => vec![scope, name, "-"],
        None => vec![npm_package, "-"],
    };
    if parents != expected_parents.as_slice() {
        return Err(DeliveryError::UntrustedSource(
            "assets lock tarball URL path does not match npmPackage".to_owned(),
        ));
    }
    Ok(())
}

fn validate_published_at(value: &str) -> Result<()> {
    let Some((date, time)) = value.split_once('T') else {
        return Err(invalid_published_at());
    };
    let zone_ok = time.ends_with('Z')
        || time.len() >= 6
            && time.get(time.len() - 6..).is_some_and(|zone| {
                (zone.starts_with('+') || zone.starts_with('-'))
                    && zone.as_bytes().get(3) == Some(&b':')
            });
    let shape_ok = (20..=40).contains(&value.len())
        && date.len() == 10
        && date.as_bytes().get(4) == Some(&b'-')
        && date.as_bytes().get(7) == Some(&b'-')
        && value.bytes().all(|byte| {
            byte.is_ascii_digit() || matches!(byte, b'-' | b':' | b'.' | b'+' | b'T' | b'Z')
        });
    if !shape_ok || !zone_ok {
        return Err(invalid_published_at());
    }
    Ok(())
}

fn invalid_published_at() -> DeliveryError {
    DeliveryError::InvalidPackage(
        "assets lock provenance publishedAt must be an RFC 3339-shaped timestamp".to_owned(),
    )
}

pub(super) fn validate_file_table(
    files: &[FileEntry],
    label: &str,
    declared_count: u64,
    declared_bytes: u64,
) -> Result<()> {
    if files.is_empty() {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock {label} file table is empty"
        )));
    }
    if files.len() > MAX_FILES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock {label} has more than {MAX_FILES} files"
        )));
    }
    let count = u64::try_from(files.len())
        .map_err(|_| DeliveryError::SizeLimit(format!("assets lock {label} count overflows")))?;
    if declared_bytes == 0 || declared_bytes > MAX_EXTRACTED_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock {label} bytes are outside the accepted range"
        )));
    }
    let (paths, folded_paths, total) = collect_file_rows(files, label)?;
    validate_ancestor_collisions(files, &paths, &folded_paths, label)?;
    if declared_count != count {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock {label} fileCount {declared_count} does not match {count} files"
        )));
    }
    if total != declared_bytes {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock {label} total bytes {declared_bytes} do not match {total} bytes"
        )));
    }
    Ok(())
}

fn collect_file_rows<'a>(
    files: &'a [FileEntry],
    label: &str,
) -> Result<(BTreeSet<&'a str>, BTreeSet<String>, u64)> {
    let mut paths = BTreeSet::new();
    let mut folded_paths = BTreeSet::new();
    let mut total = 0_u64;
    for file in files {
        validate_file_row(file, label)?;
        if !paths.insert(file.path.as_str()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock {label} contains duplicate path `{}`",
                file.path
            )));
        }
        if !folded_paths.insert(file.path.to_lowercase()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock {label} path `{}` collides with another path on a \
                 case-insensitive filesystem",
                file.path
            )));
        }
        total = total.checked_add(file.size).ok_or_else(|| {
            DeliveryError::SizeLimit(format!("assets lock {label} sizes overflow"))
        })?;
    }
    Ok((paths, folded_paths, total))
}

fn validate_file_row(file: &FileEntry, label: &str) -> Result<()> {
    validate_relative_path(&file.path, false)?;
    require_non_placeholder_digest(&file.sha256, &format!("{label} file SHA-256"))?;
    if file.size == 0 {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock {label} file `{}` has size 0",
            file.path
        )));
    }
    if file.size > MAX_FILE_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock {label} file `{}` exceeds {MAX_FILE_BYTES} bytes",
            file.path
        )));
    }
    file.mode_bits()?;
    Ok(())
}

fn validate_ancestor_collisions(
    files: &[FileEntry],
    paths: &BTreeSet<&str>,
    folded_paths: &BTreeSet<String>,
    label: &str,
) -> Result<()> {
    for file in files {
        for (index, _) in file.path.match_indices('/') {
            let Some(prefix) = file.path.get(..index) else {
                continue;
            };
            if paths.contains(prefix) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock {label} path `{}` is nested below file path `{prefix}`",
                    file.path
                )));
            }
        }
        let folded = file.path.to_lowercase();
        for (index, _) in folded.match_indices('/') {
            let Some(prefix) = folded.get(..index) else {
                continue;
            };
            if folded_paths.contains(prefix) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock {label} path `{}` collides with ancestor `{prefix}` on a \
                     case-insensitive filesystem",
                    file.path
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_label_list(values: &[String], label: &str) -> Result<()> {
    if values.is_empty() || values.len() > 8 {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock {label} must have 1-8 entries"
        )));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        if value.is_empty() || value.len() > 32 {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock {label} entry must be 1-32 bytes"
            )));
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        }) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock {label} entry contains unsupported characters"
            )));
        }
        if !seen.insert(value.as_str()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock {label} repeats `{value}`"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_license_file_list(values: &[String]) -> Result<()> {
    if values.is_empty() {
        return Err(DeliveryError::InvalidPackage(
            "assets lock licenseFiles is empty".to_owned(),
        ));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        validate_relative_path(value, false)?;
        if !seen.insert(value.as_str()) {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock licenseFiles repeats `{value}`"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_commit(value: &str) -> Result<()> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DeliveryError::InvalidPackage(
            "assets lock sourceCommit must be 40 lowercase hex characters".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_bundle(
    bundle: &AssetBundle,
    package_root: &str,
    prefix: &str,
    files: &[FileEntry],
) -> Result<()> {
    validate_asset_name(&bundle.name)?;
    validate_relative_path(&bundle.root, false)?;
    if bundle.root.ends_with('/') {
        return Err(DeliveryError::InvalidPackage(
            "assets lock bundle root must not end with `/`".to_owned(),
        ));
    }
    if bundle.root != package_root && !bundle.root.starts_with(prefix) {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock bundle `{}` root escapes the payload",
            bundle.name
        )));
    }
    if bundle.include.is_empty() {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock bundle `{}` has no include patterns",
            bundle.name
        )));
    }
    for pattern in &bundle.include {
        validate_glob_pattern(pattern)?;
    }
    require_non_placeholder_digest(&bundle.sha256, "bundle SHA-256")?;
    if bundle.file_count == 0 || bundle.file_count > MAX_FILES as u64 {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock bundle `{}` fileCount is outside the accepted range",
            bundle.name
        )));
    }
    let computed = compute_type_asset_bundle(files, &bundle.root, &bundle.include)?;
    if computed.file_count != bundle.file_count
        || computed.total_bytes != bundle.total_bytes
        || computed.sha256 != bundle.sha256
    {
        return Err(DeliveryError::Integrity(format!(
            "type asset bundle `{}` does not match the payload files",
            bundle.name
        )));
    }
    Ok(())
}
