//! Structural and frozen-constant validation for `assets.lock.json`.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{DeliveryError, Result};
use crate::manifest::{FileEntry, MAX_FILES};
use crate::manifest_v2::{
    ENGINE_BINARY_PATH, ENGINE_DISTRIBUTION, ENGINE_DISTRIBUTION_FILE_COUNT,
    ENGINE_DISTRIBUTION_INTEGRITY, ENGINE_DISTRIBUTION_SHA256, ENGINE_DISTRIBUTION_SIZE,
    ENGINE_DISTRIBUTION_UNPACKED_SIZE, ENGINE_DISTRIBUTION_URL, ENGINE_LICENSE,
    ENGINE_SOURCE_COMMIT, ENGINE_VERSION, MINIPROGRAM_TYPINGS_ASSET,
    MINIPROGRAM_TYPINGS_BUNDLE_DIGEST, MINIPROGRAM_TYPINGS_FILE_COUNT, MINIPROGRAM_TYPINGS_LICENSE,
    MINIPROGRAM_TYPINGS_PAYLOAD_PATHS, MINIPROGRAM_TYPINGS_ROOT, MINIPROGRAM_TYPINGS_TOTAL_BYTES,
    MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT, MINIPROGRAM_TYPINGS_UPSTREAM_TOTAL_BYTES,
    MINIPROGRAM_TYPINGS_VERSION, REQUIRED_ENGINE_LICENSE_FILES, STANDARD_LIB_ASSET,
    STANDARD_LIB_FILE_COUNT, STANDARD_LIB_INCLUDE, is_allowed_miniprogram_typings_path,
};
use crate::package::{PINNED_ENGINE_BINARY_SHA256, PINNED_ENGINE_BINARY_SIZE};
use crate::paths::validate_relative_path;
use crate::semver::validate_strict;
use crate::sri::Sha512Integrity;
use crate::type_assets::{forbidden_runtime_path, glob_matches, validate_glob_pattern};

use super::digest::{canonical_tree_digest, compute_runtime_closure};
use super::model::{
    Asset, AssetExtract, AssetPayload, AssetsLock, UpstreamTree, require_non_placeholder_digest,
    validate_asset_name, validate_bundle, validate_commit, validate_file_table,
    validate_label_list, validate_license_file_list, validate_npm_package,
};
use super::{
    ASSETS_LOCK_SCHEMA_VERSION, ENGINE_ASSET_NAME, ENGINE_ASSET_ROOT, ENGINE_EXECUTABLES,
    ENGINE_EXTRACT_INCLUDE, MAX_EXCLUDE_REASON_BYTES, MINIPROGRAM_TYPINGS_EXTRACT_EXCLUDE,
    MINIPROGRAM_TYPINGS_EXTRACT_INCLUDE, MINIPROGRAM_TYPINGS_RUNTIME_BUNDLE_DIGEST,
    MINIPROGRAM_TYPINGS_RUNTIME_FILE_COUNT, MINIPROGRAM_TYPINGS_RUNTIME_TOTAL_BYTES,
    NPM_STRIP_PREFIX, STANDARD_LIB_BUNDLE_DIGEST, STANDARD_LIB_ROOT, STANDARD_LIB_TOTAL_BYTES,
    TYPINGS_ASSET_NAME, TYPINGS_NPM_PACKAGE,
};

impl AssetsLock {
    /// Validates schema, both frozen assets and every internal file table.
    ///
    /// # Errors
    ///
    /// Returns a typed [`DeliveryError`] for schema drift, unknown asset names,
    /// unsafe paths, placeholders, count/digest mismatches and any difference
    /// from the frozen engine, typings, standard-library or runtime constants.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ASSETS_LOCK_SCHEMA_VERSION {
            return Err(DeliveryError::InvalidPackage(format!(
                "unsupported assets lock schema {}",
                self.schema_version
            )));
        }
        if self.assets.len() != 2 {
            return Err(DeliveryError::InvalidPackage(
                "assets lock must contain exactly the two frozen assets".to_owned(),
            ));
        }
        let mut names = BTreeSet::new();
        for asset in &self.assets {
            validate_asset_name(&asset.name)?;
            if !names.insert(asset.name.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock contains duplicate asset `{}`",
                    asset.name
                )));
            }
        }
        let engine = self.asset(ENGINE_ASSET_NAME).ok_or_else(|| {
            DeliveryError::InvalidPackage("assets lock is missing the native engine".to_owned())
        })?;
        let typings = self.asset(TYPINGS_ASSET_NAME).ok_or_else(|| {
            DeliveryError::InvalidPackage(
                "assets lock is missing the miniprogram typings".to_owned(),
            )
        })?;
        engine.validate_common()?;
        engine.validate_engine()?;
        typings.validate_common()?;
        typings.validate_typings()?;
        Ok(())
    }
}

impl Asset {
    fn validate_common(&self) -> Result<()> {
        validate_asset_name(&self.name)?;
        validate_npm_package(&self.npm_package)?;
        validate_strict(&self.version)?;
        if let Some(commit) = &self.source_commit {
            validate_commit(commit)?;
        }
        validate_label_list(&self.os, "os")?;
        validate_label_list(&self.cpu, "cpu")?;
        self.provenance.validate()?;
        if !matches!(self.license.as_str(), "Apache-2.0" | "MIT") {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock asset `{}` has unsupported license `{}`",
                self.name, self.license
            )));
        }
        validate_license_file_list(&self.license_files)?;
        validate_relative_path(&self.package_root, false)?;
        if self.package_root.ends_with('/') {
            return Err(DeliveryError::InvalidPackage(
                "assets lock packageRoot must not end with `/`".to_owned(),
            ));
        }
        self.tarball.validate(&self.npm_package, &self.version)?;
        self.upstream.validate()?;
        self.extract.validate(&self.upstream)?;
        self.payload.validate(&self.package_root)?;
        self.validate_mapping()
    }

    fn validate_engine(&self) -> Result<()> {
        if self.kind != super::AssetKind::NpmPlatformTarball {
            return Err(frozen("engine kind"));
        }
        expect_eq(&self.npm_package, ENGINE_DISTRIBUTION, "engine npmPackage")?;
        expect_eq(&self.version, ENGINE_VERSION, "engine version")?;
        expect_eq(&self.license, ENGINE_LICENSE, "engine license")?;
        expect_eq(&self.package_root, ENGINE_ASSET_ROOT, "engine packageRoot")?;
        expect_list(&self.os, &["linux"], "engine os")?;
        expect_list(&self.cpu, &["x64"], "engine cpu")?;
        expect_list(
            &self.license_files,
            REQUIRED_ENGINE_LICENSE_FILES,
            "engine licenseFiles",
        )?;
        match self.source_commit.as_deref() {
            Some(commit) if commit == ENGINE_SOURCE_COMMIT => {}
            _ => return Err(frozen("engine sourceCommit")),
        }
        self.validate_engine_tarball()?;
        if self.upstream.file_count != ENGINE_DISTRIBUTION_FILE_COUNT {
            return Err(frozen("engine upstream fileCount"));
        }
        if self.upstream.unpacked_size != ENGINE_DISTRIBUTION_UNPACKED_SIZE {
            return Err(frozen("engine upstream unpackedSize"));
        }
        if self.payload.file_count != ENGINE_DISTRIBUTION_FILE_COUNT {
            return Err(frozen("engine payload fileCount"));
        }
        if self.payload.total_bytes != ENGINE_DISTRIBUTION_UNPACKED_SIZE {
            return Err(frozen("engine payload totalBytes"));
        }
        self.validate_engine_extract()?;
        self.validate_engine_payload()
    }

    fn validate_engine_tarball(&self) -> Result<()> {
        if self.tarball.url != ENGINE_DISTRIBUTION_URL {
            return Err(frozen("engine tarball URL"));
        }
        if self.tarball.size != ENGINE_DISTRIBUTION_SIZE {
            return Err(frozen("engine tarball size"));
        }
        let expected_sri = Sha512Integrity::parse(ENGINE_DISTRIBUTION_INTEGRITY)?;
        let actual_sri = Sha512Integrity::parse(&self.tarball.integrity)?;
        if actual_sri != expected_sri {
            return Err(frozen("engine tarball integrity"));
        }
        expect_eq(
            &self.tarball.sha256,
            ENGINE_DISTRIBUTION_SHA256,
            "engine tarball SHA-256",
        )
    }

    fn validate_engine_extract(&self) -> Result<()> {
        self.validate_payload_runtime_policy()?;
        expect_list(
            &self.extract.include,
            ENGINE_EXTRACT_INCLUDE,
            "engine extract include",
        )?;
        if !self.extract.exclude.is_empty() {
            return Err(frozen("engine extract exclude"));
        }
        expect_list(
            &self.extract.executables,
            ENGINE_EXECUTABLES,
            "engine extract executables",
        )
    }

    fn validate_engine_payload(&self) -> Result<()> {
        self.require_payload_file(
            ENGINE_BINARY_PATH,
            PINNED_ENGINE_BINARY_SHA256,
            PINNED_ENGINE_BINARY_SIZE,
            "0755",
        )?;
        for required in ["engine/LICENSE", "engine/NOTICE.txt", "engine/package.json"] {
            if !self.payload.files.iter().any(|file| file.path == required) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock engine payload is missing `{required}`"
                )));
            }
        }
        self.require_single_bundle(&BundleExpectation {
            name: STANDARD_LIB_ASSET,
            root: STANDARD_LIB_ROOT,
            include: STANDARD_LIB_INCLUDE,
            file_count: STANDARD_LIB_FILE_COUNT,
            total_bytes: STANDARD_LIB_TOTAL_BYTES,
            sha256: STANDARD_LIB_BUNDLE_DIGEST,
            label: "standard library",
        })
    }

    fn validate_typings(&self) -> Result<()> {
        if self.kind != super::AssetKind::NpmTarball {
            return Err(frozen("typings kind"));
        }
        expect_eq(&self.npm_package, TYPINGS_NPM_PACKAGE, "typings npmPackage")?;
        expect_eq(
            &self.version,
            MINIPROGRAM_TYPINGS_VERSION,
            "typings version",
        )?;
        expect_eq(
            &self.license,
            MINIPROGRAM_TYPINGS_LICENSE,
            "typings license",
        )?;
        expect_eq(
            &self.package_root,
            MINIPROGRAM_TYPINGS_ROOT,
            "typings packageRoot",
        )?;
        expect_list(&self.os, &["any"], "typings os")?;
        expect_list(&self.cpu, &["any"], "typings cpu")?;
        if self.upstream.file_count != MINIPROGRAM_TYPINGS_UPSTREAM_FILE_COUNT {
            return Err(frozen("typings upstream fileCount"));
        }
        if self.upstream.unpacked_size != MINIPROGRAM_TYPINGS_UPSTREAM_TOTAL_BYTES {
            return Err(frozen("typings upstream unpackedSize"));
        }
        if self.payload.file_count != MINIPROGRAM_TYPINGS_FILE_COUNT {
            return Err(frozen("typings payload fileCount"));
        }
        if self.payload.total_bytes != MINIPROGRAM_TYPINGS_TOTAL_BYTES {
            return Err(frozen("typings payload totalBytes"));
        }
        self.validate_typings_extract()?;
        self.validate_typings_payload()?;
        self.require_runtime_closure()
    }

    fn validate_typings_extract(&self) -> Result<()> {
        self.validate_payload_runtime_policy()?;
        let license = format!("{MINIPROGRAM_TYPINGS_ROOT}/LICENSE");
        expect_list(
            &self.license_files,
            &[license.as_str()],
            "typings licenseFiles",
        )?;
        expect_list(
            &self.extract.include,
            MINIPROGRAM_TYPINGS_EXTRACT_INCLUDE,
            "typings extract include",
        )?;
        expect_list(
            &self
                .extract
                .exclude
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            MINIPROGRAM_TYPINGS_EXTRACT_EXCLUDE,
            "typings extract exclude",
        )?;
        if !self.extract.executables.is_empty() {
            return Err(frozen("typings extract executables"));
        }
        Ok(())
    }

    fn validate_typings_payload(&self) -> Result<()> {
        let prefix = format!("{MINIPROGRAM_TYPINGS_ROOT}/");
        let payload_paths: BTreeSet<&str> = self
            .payload
            .files
            .iter()
            .filter_map(|file| file.path.strip_prefix(&prefix))
            .collect();
        let expected_paths: BTreeSet<&str> =
            MINIPROGRAM_TYPINGS_PAYLOAD_PATHS.iter().copied().collect();
        if payload_paths != expected_paths {
            return Err(frozen("typings payload path set"));
        }
        if !self.payload.files.iter().any(|file| {
            file.path
                .starts_with("typings/miniprogram-api-typings/types/")
        }) {
            return Err(DeliveryError::InvalidPackage(
                "assets lock typings payload has no `types/**` files".to_owned(),
            ));
        }
        self.require_single_bundle(&BundleExpectation {
            name: MINIPROGRAM_TYPINGS_ASSET,
            root: MINIPROGRAM_TYPINGS_ROOT,
            include: "**",
            file_count: MINIPROGRAM_TYPINGS_FILE_COUNT,
            total_bytes: MINIPROGRAM_TYPINGS_TOTAL_BYTES,
            sha256: MINIPROGRAM_TYPINGS_BUNDLE_DIGEST,
            label: "miniprogram typings",
        })
    }

    fn require_payload_file(&self, path: &str, sha256: &str, size: u64, mode: &str) -> Result<()> {
        let file = self
            .payload
            .files
            .iter()
            .find(|file| file.path == path)
            .ok_or_else(|| {
                DeliveryError::InvalidPackage(format!(
                    "assets lock payload is missing frozen file `{path}`"
                ))
            })?;
        if file.sha256 != sha256 || file.size != size || file.mode != mode {
            return Err(DeliveryError::InvalidPackage(format!(
                "assets lock does not match the frozen payload file `{path}`"
            )));
        }
        Ok(())
    }

    fn require_single_bundle(&self, expected: &BundleExpectation<'_>) -> Result<()> {
        if self.payload.bundles.len() != 1 {
            return Err(frozen(&format!("{} bundle count", expected.label)));
        }
        let bundle = &self.payload.bundles[0];
        if bundle.name != expected.name
            || bundle.root != expected.root
            || bundle.include.len() != 1
            || bundle.include[0] != expected.include
            || bundle.file_count != expected.file_count
            || bundle.total_bytes != expected.total_bytes
            || bundle.sha256 != expected.sha256
        {
            return Err(frozen(&format!("{} bundle identity", expected.label)));
        }
        Ok(())
    }

    fn require_runtime_closure(&self) -> Result<()> {
        let runtime = compute_runtime_closure(&self.payload.files, &self.package_root)?;
        if runtime.file_count != MINIPROGRAM_TYPINGS_RUNTIME_FILE_COUNT
            || runtime.total_bytes != MINIPROGRAM_TYPINGS_RUNTIME_TOTAL_BYTES
            || runtime.sha256 != MINIPROGRAM_TYPINGS_RUNTIME_BUNDLE_DIGEST
        {
            return Err(frozen("miniprogram typings runtime closure"));
        }
        Ok(())
    }

    fn validate_mapping(&self) -> Result<()> {
        let (upstream_by_path, included) = self.classify_upstream()?;
        let executables: BTreeSet<&str> = self
            .extract
            .executables
            .iter()
            .map(String::as_str)
            .collect();
        self.validate_upstream_modes(&executables)?;
        self.validate_payload_table(&upstream_by_path, &included, &executables)?;
        self.validate_payload_license()
    }

    fn classify_upstream(&self) -> Result<(BTreeMap<&str, &FileEntry>, BTreeSet<&str>)> {
        let mut upstream_by_path = BTreeMap::new();
        for file in &self.upstream.files {
            upstream_by_path.insert(file.path.as_str(), file);
        }
        let excluded: BTreeSet<&str> = self
            .extract
            .exclude
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        let mut included = BTreeSet::new();
        for file in &self.upstream.files {
            let matched = self
                .extract
                .include
                .iter()
                .any(|pattern| glob_matches(pattern, &file.path));
            let is_excluded = excluded.contains(file.path.as_str());
            match (matched, is_excluded) {
                (true, false) => {
                    included.insert(file.path.as_str());
                }
                (false, true) => {}
                (true, true) => {
                    return Err(DeliveryError::InvalidPackage(format!(
                        "assets lock classifies `{}` as both included and excluded",
                        file.path
                    )));
                }
                (false, false) => {
                    return Err(DeliveryError::InvalidPackage(format!(
                        "assets lock does not classify upstream file `{}`",
                        file.path
                    )));
                }
            }
        }
        self.ensure_include_patterns_match()?;
        Ok((upstream_by_path, included))
    }

    fn ensure_include_patterns_match(&self) -> Result<()> {
        for pattern in &self.extract.include {
            if !self
                .upstream
                .files
                .iter()
                .any(|file| glob_matches(pattern, &file.path))
            {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock include pattern `{pattern}` matches no upstream file"
                )));
            }
        }
        Ok(())
    }

    fn validate_upstream_modes(&self, executables: &BTreeSet<&str>) -> Result<()> {
        for file in &self.upstream.files {
            let expected = if executables.contains(file.path.as_str()) {
                "0755"
            } else {
                "0644"
            };
            if file.mode != expected {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock upstream `{}` mode `{}` does not match normalized `{expected}`",
                    file.path, file.mode
                )));
            }
        }
        Ok(())
    }

    fn validate_payload_table(
        &self,
        upstream_by_path: &BTreeMap<&str, &FileEntry>,
        included: &BTreeSet<&str>,
        executables: &BTreeSet<&str>,
    ) -> Result<()> {
        let prefix = format!("{}/", self.package_root);
        let mut payload_paths = BTreeSet::new();
        for file in &self.payload.files {
            let relative = payload_relative(file, &prefix)?;
            validate_payload_row(file, relative, upstream_by_path, included, executables)?;
            payload_paths.insert(relative);
        }
        if payload_paths != *included {
            return Err(DeliveryError::Integrity(
                "payload file set does not match `extract.include` minus `extract.exclude`"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_payload_license(&self) -> Result<()> {
        for license in &self.license_files {
            let Some(file) = self
                .payload
                .files
                .iter()
                .find(|file| file.path.as_str() == license.as_str())
            else {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock license file `{license}` is not in the payload"
                )));
            };
            if file.mode != "0644" {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock license file `{license}` must be mode 0644"
                )));
            }
        }
        Ok(())
    }

    fn validate_payload_runtime_policy(&self) -> Result<()> {
        for file in &self.payload.files {
            if let Some(reason) = forbidden_runtime_path(&file.path) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock payload `{}` is forbidden: {reason}",
                    file.path
                )));
            }
            if !is_allowed_miniprogram_typings_path(&file.path) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock payload `{}` is outside the frozen typings layout",
                    file.path
                )));
            }
        }
        Ok(())
    }
}

impl UpstreamTree {
    fn validate(&self) -> Result<()> {
        if self.strip_prefix != NPM_STRIP_PREFIX {
            return Err(DeliveryError::InvalidPackage(
                "assets lock upstream stripPrefix must be `package/`".to_owned(),
            ));
        }
        validate_file_table(&self.files, "upstream", self.file_count, self.unpacked_size)?;
        require_non_placeholder_digest(&self.tree_digest, "upstream treeDigest")?;
        let actual = canonical_tree_digest(&self.files, "")?;
        if actual != self.tree_digest {
            return Err(DeliveryError::Integrity(
                "upstream tree digest does not match the upstream file table".to_owned(),
            ));
        }
        Ok(())
    }
}

impl AssetExtract {
    fn validate(&self, upstream: &UpstreamTree) -> Result<()> {
        validate_extract_budget(self)?;
        self.validate_include()?;
        self.validate_exclude(upstream)?;
        self.validate_executables(upstream)
    }

    fn validate_include(&self) -> Result<()> {
        if self.include.is_empty() {
            return Err(DeliveryError::InvalidPackage(
                "assets lock extract.include is empty".to_owned(),
            ));
        }
        let mut patterns = BTreeSet::new();
        for pattern in &self.include {
            validate_glob_pattern(pattern)?;
            if !patterns.insert(pattern.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock extract.include repeats `{pattern}`"
                )));
            }
        }
        Ok(())
    }

    fn validate_exclude(&self, upstream: &UpstreamTree) -> Result<()> {
        let mut exclusions = BTreeSet::new();
        for exclusion in &self.exclude {
            validate_relative_path(&exclusion.path, false)?;
            if !exclusions.insert(exclusion.path.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock extract.exclude repeats `{}`",
                    exclusion.path
                )));
            }
            validate_exclusion_reason(exclusion)?;
            if !upstream
                .files
                .iter()
                .any(|file| file.path == exclusion.path)
            {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock extract.exclude `{}` is not an upstream file",
                    exclusion.path
                )));
            }
        }
        Ok(())
    }

    fn validate_executables(&self, upstream: &UpstreamTree) -> Result<()> {
        let mut executables = BTreeSet::new();
        for path in &self.executables {
            validate_relative_path(path, false)?;
            if !executables.insert(path.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock extract.executables repeats `{path}`"
                )));
            }
            if !upstream.files.iter().any(|file| file.path == *path) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock extract.executables `{path}` is not an upstream file"
                )));
            }
        }
        Ok(())
    }
}

impl AssetPayload {
    fn validate(&self, package_root: &str) -> Result<()> {
        validate_file_table(&self.files, "payload", self.file_count, self.total_bytes)?;
        require_non_placeholder_digest(&self.tree_digest, "payload treeDigest")?;
        let actual = canonical_tree_digest(&self.files, package_root)?;
        if actual != self.tree_digest {
            return Err(DeliveryError::Integrity(
                "payload tree digest does not match the payload file table".to_owned(),
            ));
        }
        if self.bundles.len() != 1 {
            return Err(DeliveryError::InvalidPackage(
                "assets lock payload must declare exactly one bundle".to_owned(),
            ));
        }
        let prefix = format!("{package_root}/");
        for file in &self.files {
            if !file.path.starts_with(&prefix) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock payload `{}` is outside `{package_root}`",
                    file.path
                )));
            }
        }
        let mut names = BTreeSet::new();
        for bundle in &self.bundles {
            validate_bundle(bundle, package_root, &prefix, &self.files)?;
            if !names.insert(bundle.name.as_str()) {
                return Err(DeliveryError::InvalidPackage(format!(
                    "assets lock payload repeats bundle `{}`",
                    bundle.name
                )));
            }
        }
        Ok(())
    }
}

fn validate_extract_budget(extract: &AssetExtract) -> Result<()> {
    if extract.include.len() > MAX_FILES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock extract.include exceeds {MAX_FILES} patterns"
        )));
    }
    if extract.exclude.len() > MAX_FILES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock extract.exclude exceeds {MAX_FILES} entries"
        )));
    }
    if extract.executables.len() > MAX_FILES {
        return Err(DeliveryError::SizeLimit(format!(
            "assets lock extract.executables exceeds {MAX_FILES} entries"
        )));
    }
    Ok(())
}

fn validate_exclusion_reason(exclusion: &super::ExcludedPath) -> Result<()> {
    if exclusion.reason.trim().is_empty() {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock extract.exclude `{}` has an empty reason",
            exclusion.path
        )));
    }
    if exclusion.reason.len() > MAX_EXCLUDE_REASON_BYTES
        || exclusion.reason.chars().any(char::is_control)
    {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock extract.exclude `{}` has an invalid reason",
            exclusion.path
        )));
    }
    Ok(())
}

fn payload_relative<'a>(file: &'a FileEntry, prefix: &str) -> Result<&'a str> {
    let Some(relative) = file.path.strip_prefix(prefix) else {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock payload `{}` is outside the package root",
            file.path
        )));
    };
    if relative.is_empty() {
        return Err(DeliveryError::InvalidPackage(
            "assets lock payload path has an empty package-relative part".to_owned(),
        ));
    }
    Ok(relative)
}

fn validate_payload_row(
    file: &FileEntry,
    relative: &str,
    upstream_by_path: &BTreeMap<&str, &FileEntry>,
    included: &BTreeSet<&str>,
    executables: &BTreeSet<&str>,
) -> Result<()> {
    let Some(upstream) = upstream_by_path.get(relative) else {
        return Err(DeliveryError::Integrity(format!(
            "assets lock payload `{}` has no upstream record",
            file.path
        )));
    };
    if upstream.sha256 != file.sha256 || upstream.size != file.size {
        return Err(DeliveryError::Integrity(format!(
            "assets lock payload `{}` does not match its upstream bytes",
            file.path
        )));
    }
    if !included.contains(relative) {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock payload `{}` is not selected by `extract.include`",
            file.path
        )));
    }
    let expected = if executables.contains(relative) {
        "0755"
    } else {
        "0644"
    };
    if file.mode != expected {
        return Err(DeliveryError::InvalidPackage(format!(
            "assets lock payload `{}` mode `{}` does not match normalized `{expected}`",
            file.path, file.mode
        )));
    }
    Ok(())
}

fn expect_eq(actual: &str, expected: &str, label: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(frozen(label))
    }
}

fn expect_list<S: AsRef<str>>(actual: &[S], expected: &[&str], label: &str) -> Result<()> {
    let mut actual: Vec<&str> = actual.iter().map(AsRef::as_ref).collect();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    if actual == expected {
        Ok(())
    } else {
        Err(frozen(label))
    }
}

fn frozen(label: &str) -> DeliveryError {
    DeliveryError::InvalidPackage(format!("assets lock does not match the frozen {label}"))
}

struct BundleExpectation<'a> {
    name: &'a str,
    root: &'a str,
    include: &'a str,
    file_count: u64,
    total_bytes: u64,
    sha256: &'a str,
    label: &'a str,
}
