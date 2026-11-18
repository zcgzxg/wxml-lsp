//! Schema v2 native package acquisition from an explicit local or remote source.
//!
//! Acquisition is deliberately separate from the transaction in
//! [`crate::native_install`]: it downloads/copies an archive, verifies its SRI,
//! SHA-256 and size, extracts it with the formal-release rules, and hashes the
//! whole tree before the caller can activate it.
//!
//! A native archive cache is not implemented yet. `--offline` therefore
//! requires an explicit `--from-dir` release directory; a missing source fails
//! closed instead of touching the network.

use std::path::PathBuf;

use crate::archive::extract_tar_gz;
use crate::error::{DeliveryError, Result};
use crate::index_v2::{IndexV2, ReleaseV2};
use crate::install::InstallRoot;
use crate::manifest_v2::{
    ExpectedManifestV2, ManifestV2, NATIVE_PACKAGE_PLATFORM, read_manifest_v2,
};
use crate::paths::host_platform;
use crate::source::Source;
use crate::verify_v2::verify_tree_v2;

/// What to select from the schema v2 index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcquireModeV2 {
    /// An exact release version.
    Exact(String),
    /// The newest stable release for this platform.
    Latest,
}

/// Inputs for one schema v2 acquisition.
#[derive(Clone, Debug)]
pub struct AcquireOptionsV2<'a> {
    /// Install root used for temporary staging directories.
    pub root: &'a InstallRoot,
    /// Explicit source; required for every acquisition.
    pub source: Option<&'a Source>,
    /// Already validated index read from the same source.
    pub index: &'a IndexV2,
    /// Version selection.
    pub mode: AcquireModeV2,
    /// Whether a remote source may be contacted. A local source never needs it.
    pub allow_download: bool,
    /// Reject network access even if a remote source was supplied.
    pub offline: bool,
}

/// A verified schema v2 extraction tree plus its manifest and release record.
#[derive(Clone, Debug)]
pub struct AcquiredPackageV2 {
    /// Tree below the install root's `tmp/` directory.
    pub tree: PathBuf,
    /// Validated native manifest.
    pub manifest: ManifestV2,
    /// Selected release metadata.
    pub release: ReleaseV2,
    /// True when bytes came from a local verified cache. Currently always
    /// false because the native cache is not implemented.
    pub cache_hit: bool,
}

/// Selects and verifies one native release.
///
/// # Errors
///
/// Returns an error for a missing source, an offline remote source, a disabled
/// download, an unsupported host, a missing/unstable release, an unsafe
/// archive, a digest/size mismatch, a failed tree verification or a manifest
/// that disagrees with the index.
pub fn acquire_v2(options: &AcquireOptionsV2<'_>) -> Result<AcquiredPackageV2> {
    let platform = host_platform()?;
    if platform != NATIVE_PACKAGE_PLATFORM {
        return Err(DeliveryError::UnsupportedPlatform(format!(
            "native packages require {NATIVE_PACKAGE_PLATFORM}, host is {platform}"
        )));
    }
    let source = options.source.ok_or_else(|| {
        DeliveryError::InstallState(
            "a native source is required; offline installs need --from-dir because no native archive cache is implemented"
                .to_owned(),
        )
    })?;
    if options.offline && matches!(source, Source::Remote(_)) {
        return Err(DeliveryError::InvalidArgument(
            "offline version selection cannot query a remote source; pass --from-dir".to_owned(),
        ));
    }
    if matches!(source, Source::Remote(_)) && !options.allow_download {
        return Err(DeliveryError::InvalidArgument(
            "download is disabled; pass --enable-download to contact the source".to_owned(),
        ));
    }
    options.index.validate()?;
    let release = match &options.mode {
        AcquireModeV2::Exact(version) => options.index.find(version, &platform)?.clone(),
        AcquireModeV2::Latest => options.index.latest(&platform)?.clone(),
    };
    release.validate()?;

    let temporary = options.root.new_temp_dir("native")?;
    let result = (|| {
        let archive = temporary.join("archive.tar.gz");
        source.fetch_archive_v2(&release, &archive)?;
        let tree = temporary.join("tree");
        extract_tar_gz(&archive, &tree)?;
        let report = verify_tree_v2(
            &tree,
            ExpectedManifestV2 {
                version: Some(&release.version),
                platform: Some(NATIVE_PACKAGE_PLATFORM),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        if !release.matches_manifest(&report.manifest) {
            return Err(DeliveryError::Integrity(
                "native manifest metadata does not match the release index".to_owned(),
            ));
        }
        // Re-read from disk so the caller gets exactly the verified bytes, not
        // a manifest value that could have been reconstructed in memory.
        let manifest = read_manifest_v2(
            &tree,
            ExpectedManifestV2 {
                version: Some(&release.version),
                platform: Some(NATIVE_PACKAGE_PLATFORM),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        Ok(AcquiredPackageV2 {
            tree,
            manifest,
            release,
            cache_hit: false,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-acquire-v2-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn offline_without_a_source_fails_closed() {
        let base = scratch("offline");
        let root = InstallRoot::create(&base.join("root")).unwrap();
        let index = IndexV2::empty();
        let error = acquire_v2(&AcquireOptionsV2 {
            root: &root,
            source: None,
            index: &index,
            mode: AcquireModeV2::Exact("0.1.0".to_owned()),
            allow_download: false,
            offline: true,
        })
        .unwrap_err();
        assert!(error.to_string().contains("--from-dir"), "{error}");
        assert!(!root.tmp_dir().exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
