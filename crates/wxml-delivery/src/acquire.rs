//! Cache-aware package acquisition with explicit download gating.

use std::path::PathBuf;

use crate::cache::{self, CacheEntry, SourceKind};
use crate::error::{DeliveryError, Result};
use crate::install::InstallRoot;
use crate::manifest::{ExpectedManifest, Index, Manifest, Release, now_rfc3339};
use crate::paths::host_platform;
use crate::source::Source;
use crate::verify::verify_tree;

/// What to install.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcquireMode {
    /// An exact version.
    Exact(String),
    /// The newest release for this platform.
    Latest,
}

/// Inputs for one acquisition.
#[derive(Clone, Debug)]
pub struct AcquireOptions<'a> {
    /// Install root used for cache and temporary directories.
    pub root: &'a InstallRoot,
    /// Explicit source; absent for an offline cache-only install.
    pub source: Option<&'a Source>,
    /// Version selection.
    pub mode: AcquireMode,
    /// Whether a remote source may be contacted. A local source is always
    /// explicit and does not need this flag.
    pub allow_download: bool,
    /// Skip all network access and use only verified cache entries.
    pub offline: bool,
}

/// A verified extraction tree plus its manifest.
#[derive(Clone, Debug)]
pub struct AcquiredPackage {
    /// Tree below the install root's `tmp/` directory.
    pub tree: PathBuf,
    /// Validated manifest.
    pub manifest: Manifest,
    /// Selected release metadata.
    pub release: Release,
    /// True when no download/copy was needed.
    pub cache_hit: bool,
}

/// Selects, downloads or reuses a package, then verifies the full tree.
///
/// The returned tree is inside `root/tmp/` and is consumed by
/// [`InstallRoot::install_tree`].
///
/// # Errors
///
/// Returns an error when the source is disabled, missing, untrusted, corrupt,
/// incomplete or does not match the requested version, platform and protocol.
pub fn acquire(options: &AcquireOptions<'_>) -> Result<AcquiredPackage> {
    let platform = host_platform()?;
    if options.offline {
        return acquire_offline(options, &platform);
    }
    let source = options.source.ok_or_else(|| {
        DeliveryError::InvalidArgument("a source is required unless --offline is used".to_owned())
    })?;
    if matches!(source, Source::Remote(_)) && !options.allow_download {
        return Err(DeliveryError::InvalidArgument(
            "download is disabled; pass --enable-download to contact the source".to_owned(),
        ));
    }
    let index = source.read_index()?;
    let release = match &options.mode {
        AcquireMode::Exact(version) => index.find(version, &platform)?.clone(),
        AcquireMode::Latest => index.latest(&platform)?.clone(),
    };
    release.validate()?;

    if let Some(cached) = cache::find_version(
        &options.root.cache_dir(),
        &platform,
        &release.version,
        Some(&release.sha256),
    )? {
        match use_cache(options.root, &cached, &release, &platform) {
            Ok(package) => return Ok(package),
            Err(error) => {
                // A damaged cache entry must never block a fresh verified
                // download. Remove it and fall through.
                let _ = cache::remove(&options.root.cache_dir(), &cached);
                eprintln!("warning: discarding damaged cache entry: {error}");
            }
        }
    }

    let temporary = options.root.new_temp_dir("download")?;
    let result = (|| {
        let archive = temporary.join("archive.tar.gz");
        source.fetch_archive(&release, &archive)?;
        let tree = temporary.join("tree");
        crate::archive::extract_tar_gz(&archive, &tree)?;
        let manifest = verify_tree(
            &tree,
            ExpectedManifest {
                version: Some(&release.version),
                platform: Some(&platform),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        check_release_consistency(&release, &manifest)?;

        let entry =
            CacheEntry::from_release(&release, SourceKind::from_source(source), now_rfc3339());
        cache::store_verified(&options.root.cache_dir(), &archive, &entry)?;
        Ok(AcquiredPackage {
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

fn acquire_offline(options: &AcquireOptions<'_>, platform: &str) -> Result<AcquiredPackage> {
    if let Some(Source::Local(directory)) = options.source {
        // A local release directory is already an offline source; still verify
        // it through the same manifest and checksum path.
        let source = Source::Local(directory.clone());
        let index = source.read_index()?;
        let release = match &options.mode {
            AcquireMode::Exact(version) => index.find(version, platform)?.clone(),
            AcquireMode::Latest => index.latest(platform)?.clone(),
        };
        release.validate()?;
        return offline_from_source(options.root, &source, &release, platform);
    }
    let entry = match &options.mode {
        AcquireMode::Exact(version) => {
            cache::find_version(&options.root.cache_dir(), platform, version, None)?
        }
        AcquireMode::Latest => cache::latest(&options.root.cache_dir(), platform)?,
    }
    .ok_or_else(|| {
        DeliveryError::InstallState(
            "no verified cache entry is available for this version while offline".to_owned(),
        )
    })?;
    let release = release_from_entry(&entry)?;
    use_cache_entry(options.root, &entry, platform).map(|mut package| {
        package.release = release;
        package.cache_hit = true;
        package
    })
}

fn offline_from_source(
    root: &InstallRoot,
    source: &Source,
    release: &Release,
    platform: &str,
) -> Result<AcquiredPackage> {
    let temporary = root.new_temp_dir("local")?;
    let result = (|| {
        let archive = temporary.join("archive.tar.gz");
        source.fetch_archive(release, &archive)?;
        let tree = temporary.join("tree");
        crate::archive::extract_tar_gz(&archive, &tree)?;
        let manifest = verify_tree(
            &tree,
            ExpectedManifest {
                version: Some(&release.version),
                platform: Some(platform),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        check_release_consistency(release, &manifest)?;
        let entry = CacheEntry::from_release(release, SourceKind::LocalDirectory, now_rfc3339());
        cache::store_verified(&root.cache_dir(), &archive, &entry)?;
        Ok(AcquiredPackage {
            tree,
            manifest,
            release: release.clone(),
            cache_hit: false,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

fn use_cache(
    root: &InstallRoot,
    entry: &CacheEntry,
    release: &Release,
    platform: &str,
) -> Result<AcquiredPackage> {
    if entry.protocol_version != release.protocol_version
        || entry.node_minimum_version != release.node_minimum_version
        || entry.platform != release.platform
    {
        return Err(DeliveryError::InstallState(
            "cache metadata does not match the release index".to_owned(),
        ));
    }
    use_cache_entry(root, entry, platform).map(|mut package| {
        package.release = release.clone();
        package.cache_hit = true;
        package
    })
}

fn use_cache_entry(
    root: &InstallRoot,
    entry: &CacheEntry,
    platform: &str,
) -> Result<AcquiredPackage> {
    let archive = cache::load_verified(&root.cache_dir(), entry)?;
    let temporary = root.new_temp_dir("cache")?;
    let result = (|| {
        let tree = temporary.join("tree");
        crate::archive::extract_tar_gz(&archive, &tree)?;
        let manifest = verify_tree(
            &tree,
            ExpectedManifest {
                version: Some(&entry.version),
                platform: Some(platform),
                protocol_version: Some(entry.protocol_version),
            },
        )?;
        let release = release_from_entry(entry)?;
        check_release_consistency(&release, &manifest)?;
        Ok(AcquiredPackage {
            tree,
            manifest,
            release,
            cache_hit: true,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result
}

fn release_from_entry(entry: &CacheEntry) -> Result<Release> {
    Ok(Release {
        version: entry.version.clone(),
        platform: entry.platform.clone(),
        protocol_version: entry.protocol_version,
        node_minimum_version: entry.node_minimum_version.clone(),
        archive: crate::manifest::archive_name(&entry.version, &entry.platform)?,
        sha256: entry.sha256.clone(),
        size: entry.size,
    })
}

fn check_release_consistency(release: &Release, manifest: &Manifest) -> Result<()> {
    let version_matches = manifest.package_version == release.version;
    let platform_matches = manifest.platform == release.platform;
    let protocol_matches = manifest.protocol_version == release.protocol_version;
    let node_matches = manifest.node.minimum_version == release.node_minimum_version;
    if !(version_matches && platform_matches && protocol_matches && node_matches) {
        return Err(DeliveryError::Integrity(
            "manifest metadata does not match the release index".to_owned(),
        ));
    }
    Ok(())
}

/// Reads an index for display commands without acquiring a package.
///
/// # Errors
///
/// See [`Source::read_index`].
pub fn read_versions(source: &Source, offline: bool) -> Result<Index> {
    if offline && matches!(source, Source::Remote(_)) {
        return Err(DeliveryError::InvalidArgument(
            "offline version listing cannot query a remote source".to_owned(),
        ));
    }
    source.read_index()
}

/// Removes a cached entry only if it is under the cache root.
///
/// # Errors
///
/// Returns an error for unsafe metadata or I/O failures.
pub fn discard(root: &InstallRoot, entry: &CacheEntry) -> Result<()> {
    let cache_root = root.cache_dir();
    let directory = cache_root.join(entry.directory_name());
    if !directory.starts_with(&cache_root) {
        return Err(DeliveryError::InstallState(
            "cache path escaped the cache root".to_owned(),
        ));
    }
    cache::remove(&cache_root, entry)
}
