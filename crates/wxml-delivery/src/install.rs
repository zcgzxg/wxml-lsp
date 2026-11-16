//! Install root, exclusive locking, atomic activation and environment precedence.

use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};

use fs2::FileExt;

use crate::error::{DeliveryError, Result};
use crate::manifest::{ExpectedManifest, Manifest, set_mode};
use crate::paths::{host_platform, validate_version};
use crate::semver::compare_versions;
use crate::verify::verify_tree;

/// Environment variable that always wins over the installed server.
pub const BINARY_ENV: &str = "WXML_LSP_BIN";

/// A directory containing `versions/`, `cache/` and the `current` symlink.
#[derive(Clone, Debug)]
pub struct InstallRoot {
    path: PathBuf,
}

impl InstallRoot {
    /// Creates or opens an installation root and canonicalizes it.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or resolved.
    pub fn create(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path).map_err(|error| DeliveryError::io(path, error))?;
        let canonical =
            std::fs::canonicalize(path).map_err(|error| DeliveryError::io(path, error))?;
        if !canonical.is_dir() {
            return Err(DeliveryError::InvalidArgument(
                "install root is not a directory".to_owned(),
            ));
        }
        set_mode(&canonical, 0o755)?;
        Ok(Self { path: canonical })
    }

    /// Returns the canonical root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the versioned installation directory.
    #[must_use]
    pub fn versions_dir(&self) -> PathBuf {
        self.path.join("versions")
    }

    /// Returns the verified cache directory.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.path.join("cache")
    }

    /// Returns the private temporary directory.
    #[must_use]
    pub fn tmp_dir(&self) -> PathBuf {
        self.path.join("tmp")
    }

    /// Creates a fresh temporary directory below the install root.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created.
    pub fn new_temp_dir(&self, prefix: &str) -> Result<PathBuf> {
        let tmp = self.tmp_dir();
        std::fs::create_dir_all(&tmp).map_err(|error| DeliveryError::io(&tmp, error))?;
        set_mode(&tmp, 0o700)?;
        for _ in 0..32 {
            let candidate = tmp.join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_nanos())
            ));
            match std::fs::create_dir(&candidate) {
                Ok(()) => return Ok(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(DeliveryError::io(&candidate, error)),
            }
        }
        Err(DeliveryError::InstallState(
            "could not allocate a temporary directory".to_owned(),
        ))
    }

    /// Acquires the exclusive install lock.
    ///
    /// The operating system releases the lock if the process exits, so a crash
    /// cannot leave a permanent lock file.
    ///
    /// # Errors
    ///
    /// Returns an error when the lock file cannot be opened or locked.
    pub fn lock(&self) -> Result<InstallLock> {
        let path = self.path.join(".lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| DeliveryError::io(&path, error))?;
        set_mode(&path, 0o600)?;
        FileExt::lock_exclusive(&file).map_err(|error| DeliveryError::io(&path, error))?;
        Ok(InstallLock { file })
    }

    /// Returns the installed versions in ascending numeric order.
    ///
    /// # Errors
    ///
    /// Returns an error when the versions directory cannot be read.
    pub fn versions(&self) -> Result<Vec<String>> {
        let directory = self.versions_dir();
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(DeliveryError::io(&directory, error)),
        };
        let mut versions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| DeliveryError::io(&directory, error))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| DeliveryError::io(&path, error))?;
            if !metadata.file_type().is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if validate_version(name).is_ok() {
                versions.push(name.to_owned());
            }
        }
        versions.sort_by(|left, right| compare_versions(left, right));
        Ok(versions)
    }

    /// Returns the version selected by the `current` symlink.
    ///
    /// # Errors
    ///
    /// Returns an error when the link is malformed or points outside
    /// `versions/`.
    pub fn current_version(&self) -> Result<Option<String>> {
        let link = self.path.join("current");
        let target = match std::fs::read_link(&link) {
            Ok(target) => target,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(DeliveryError::io(&link, error)),
        };
        let mut components = target.components();
        let first = components.next();
        let second = components.next();
        if !matches!(first, Some(Component::Normal(name)) if name == "versions")
            || !matches!(second, Some(Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(DeliveryError::InstallState(
                "current link does not point at a versioned directory".to_owned(),
            ));
        }
        let version = target
            .components()
            .nth(1)
            .and_then(|component| component.as_os_str().to_str())
            .ok_or_else(|| DeliveryError::InstallState("invalid current version".to_owned()))?;
        validate_version(version)?;
        Ok(Some(version.to_owned()))
    }

    /// Returns the active version directory.
    ///
    /// # Errors
    ///
    /// Returns an error when nothing is active.
    pub fn current_dir(&self) -> Result<PathBuf> {
        let version = self
            .current_version()?
            .ok_or_else(|| DeliveryError::InstallState("no version is active".to_owned()))?;
        Ok(self.versions_dir().join(version))
    }

    /// Returns the stable absolute path users configure as `WXML_LSP_BIN`.
    ///
    /// # Errors
    ///
    /// Returns an error when nothing is active.
    pub fn current_binary(&self) -> Result<PathBuf> {
        self.current_version()?;
        Ok(self.path.join("current/bin/wxml-lsp"))
    }

    /// Verifies one installed version.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory, manifest, checksums or files do
    /// not pass verification.
    pub fn verify_installed(&self, version: &str) -> Result<Manifest> {
        validate_version(version)?;
        let directory = self.versions_dir().join(version);
        if !directory.is_dir() {
            return Err(DeliveryError::InstallState(format!(
                "version {version} is not installed"
            )));
        }
        let platform = host_platform()?;
        verify_tree(
            &directory,
            ExpectedManifest {
                version: Some(version),
                platform: Some(&platform),
                protocol_version: None,
            },
        )
    }

    /// Installs an already verified extraction tree and activates it.
    ///
    /// The tree must live under this root's `tmp/` directory so the rename is
    /// atomic. On success the `current` link is switched after the version
    /// directory is complete.
    ///
    /// # Errors
    ///
    /// Returns an error when the tree is outside the root, verification fails,
    /// or the atomic rename or link update fails.
    pub fn install_tree(&self, tree: &Path, version: &str) -> Result<InstallOutcome> {
        validate_version(version)?;
        let platform = host_platform()?;
        let canonical_tree =
            std::fs::canonicalize(tree).map_err(|error| DeliveryError::io(tree, error))?;
        let canonical_tmp = std::fs::canonicalize(self.tmp_dir())
            .map_err(|error| DeliveryError::io(&self.tmp_dir(), error))?;
        if !canonical_tree.starts_with(&canonical_tmp) {
            return Err(DeliveryError::InstallState(
                "staging tree is outside the install root".to_owned(),
            ));
        }
        let manifest = verify_tree(
            &canonical_tree,
            ExpectedManifest {
                version: Some(version),
                platform: Some(&platform),
                protocol_version: None,
            },
        )?;

        let versions = self.versions_dir();
        std::fs::create_dir_all(&versions).map_err(|error| DeliveryError::io(&versions, error))?;
        set_mode(&versions, 0o755)?;
        let destination = versions.join(version);
        let mut outcome = InstallOutcome::Installed;
        if destination.exists() {
            if self.verify_installed(version).is_ok() {
                let _ = std::fs::remove_dir_all(&canonical_tree);
                outcome = InstallOutcome::AlreadyInstalled;
                self.switch(version)?;
                return Ok(outcome);
            }
            let broken = self.new_temp_dir("broken")?;
            let _ = std::fs::remove_dir(&broken);
            std::fs::rename(&destination, &broken)
                .map_err(|error| DeliveryError::io(&destination, error))?;
            let _ = std::fs::remove_dir_all(&broken);
        }
        std::fs::rename(&canonical_tree, &destination)
            .map_err(|error| DeliveryError::io(&destination, error))?;
        set_mode(&destination, 0o755)?;
        self.switch(version)?;
        if manifest.package_version != version {
            return Err(DeliveryError::Integrity(
                "installed manifest version differs from the directory".to_owned(),
            ));
        }
        Ok(outcome)
    }

    /// Atomically switches the `current` symlink to `version`.
    ///
    /// # Errors
    ///
    /// Returns an error when the version is not installed or the link cannot be
    /// replaced.
    pub fn switch(&self, version: &str) -> Result<()> {
        validate_version(version)?;
        if !self.versions_dir().join(version).is_dir() {
            return Err(DeliveryError::InstallState(format!(
                "version {version} is not installed"
            )));
        }
        #[cfg(unix)]
        {
            let temporary = self.path.join(format!(
                ".current.tmp-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_nanos())
            ));
            let target = Path::new("versions").join(version);
            std::os::unix::fs::symlink(&target, &temporary)
                .map_err(|error| DeliveryError::io(&temporary, error))?;
            if let Err(error) = std::fs::rename(&temporary, self.path.join("current")) {
                let _ = std::fs::remove_file(&temporary);
                return Err(DeliveryError::io(&self.path.join("current"), error));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(DeliveryError::UnsupportedPlatform(
                "the installer currently supports Unix hosts only".to_owned(),
            ))
        }
    }
}

/// Result of installing one version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOutcome {
    /// A new version directory was created and activated.
    Installed,
    /// The same verified version was already installed.
    AlreadyInstalled,
}

/// Holds the exclusive install lock until dropped.
#[derive(Debug)]
pub struct InstallLock {
    file: File,
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Returns `WXML_LSP_BIN` when set, after validating it.
///
/// A set but invalid value is always an error. There is no fallback to an
/// installed version or a download.
///
/// # Errors
///
/// Returns an error for an empty, relative, parent-traversing, missing or
/// non-executable value.
pub fn explicit_binary() -> Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(BINARY_ENV) else {
        return Ok(None);
    };
    let raw = value.into_string().map_err(|_| {
        DeliveryError::InstallState(format!("{BINARY_ENV} is set but is not valid UTF-8"))
    })?;
    validate_explicit_binary(&raw).map(Some)
}

fn validate_explicit_binary(raw: &str) -> Result<PathBuf> {
    if raw.is_empty() || raw.trim() != raw {
        return Err(invalid_binary(
            "must not be empty or padded with whitespace",
        ));
    }
    if raw.ends_with('/') {
        return Err(invalid_binary("must name a file, not end with `/`"));
    }
    if raw.starts_with("//") {
        return Err(invalid_binary(
            "UNC-style paths are not accepted on this host",
        ));
    }
    if raw.chars().any(char::is_control) {
        return Err(invalid_binary("must not contain control characters"));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(invalid_binary("must be an absolute path"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid_binary("must not contain `..` components"));
    }
    let metadata =
        std::fs::metadata(&path).map_err(|_| invalid_binary("does not exist or cannot be read"))?;
    if !metadata.file_type().is_file() {
        return Err(invalid_binary("does not point to a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid_binary("is not executable"));
        }
    }
    Ok(path)
}

fn invalid_binary(reason: &str) -> DeliveryError {
    DeliveryError::InstallState(format!("{BINARY_ENV} is set but invalid: {reason}"))
}

/// Returns the binary path that takes precedence, or the active install.
///
/// # Errors
///
/// Returns an error for an invalid `WXML_LSP_BIN` or when nothing is installed.
pub fn effective_binary(root: &InstallRoot) -> Result<PathBuf> {
    if let Some(path) = explicit_binary()? {
        return Ok(path);
    }
    root.current_binary()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let base = std::env::var_os("WXML_TEST_TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let path = base.join(format!(
            "wxml-install-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn validates_explicit_binary_paths() {
        let base = scratch("explicit");
        let executable = base.join("bin");
        std::fs::create_dir_all(&executable).unwrap();
        let binary = executable.join("wxml-lsp");
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
        set_mode(&binary, 0o755).unwrap();
        let absolute = binary.to_str().unwrap().to_owned();
        assert!(validate_explicit_binary(&absolute).is_ok());
        for value in [
            String::new(),
            " ".to_owned(),
            "relative".to_owned(),
            "/abs/../bin".to_owned(),
            format!("{absolute}/"),
        ] {
            assert!(validate_explicit_binary(&value).is_err(), "{value}");
        }
        let missing = executable.join("missing");
        assert!(validate_explicit_binary(missing.to_str().unwrap()).is_err());
        set_mode(&binary, 0o644).unwrap();
        assert!(validate_explicit_binary(&absolute).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_tree_rejects_a_tree_outside_the_root() {
        let base = scratch("outside");
        let root = InstallRoot::create(&base.join("root")).unwrap();
        let outside = base.join("tree");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(root.install_tree(&outside, "0.1.0").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn versions_are_sorted_and_current_requires_a_valid_link() {
        let base = scratch("versions");
        let root = InstallRoot::create(&base.join("root")).unwrap();
        std::fs::create_dir_all(root.versions_dir().join("0.2.0")).unwrap();
        std::fs::create_dir_all(root.versions_dir().join("0.1.0")).unwrap();
        assert_eq!(root.versions().unwrap(), vec!["0.1.0", "0.2.0"]);
        assert!(root.current_version().unwrap().is_none());
        root.switch("0.2.0").unwrap();
        assert_eq!(root.current_version().unwrap().as_deref(), Some("0.2.0"));
        std::fs::remove_dir_all(root.versions_dir().join("0.2.0")).unwrap();
        assert!(root.current_version().is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }
}
