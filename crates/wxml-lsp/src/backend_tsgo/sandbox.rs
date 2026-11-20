//! Per-session sandbox ownership for the native backend.
//!
//! Every native session gets a fresh, empty `0700` directory below the
//! operating system temporary directory. The directory is never the package
//! root, a version directory, an install transaction directory or the
//! workspace. [`SandboxDir`] removes only its own empty directory on drop;
//! a crash may leave an empty directory behind, which the next start does not
//! reuse.
//!
//! [`SandboxSessionFactory`] builds one sandbox per session and keeps it alive
//! until [`QuerySession::shutdown`] has released the engine. A failed session
//! start drops the sandbox immediately.
//!
//! # Residual race
//!
//! Validation is path-based. A parent component could be swapped between
//! finalization and engine spawn. `EngineSupervisor` re-validates the engine
//! executable immediately before spawn and the engine SHA-256 is pinned, so an
//! engine swap fails closed. Directory-handle/`openat` hardening is not
//! implemented and remains BLOCKED; this module must not claim full race
//! immunity.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::backend_tsgo::query::completion::CompletionQueryOptions;
use crate::backend_tsgo::query::definition::DefinitionResult as NativeDefinitionResult;
use crate::backend_tsgo::query::hover::HoverEvidence;
use wxml_backend_tsgo::input::AnalysisInput;
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::UnavailableReason;
use wxml_backend_tsgo::native::document_symbol::DocumentSymbolsEvidence;
use wxml_backend_tsgo::native::session::{AutoImportPreparation, SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::native::wire::{RawCompletionList, RawDiagnostic};

use crate::backend_tsgo::session::{
    HoverDocumentation, NativeSessionFactory, QuerySession, SessionFactory,
};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// Number of name attempts before a sandbox creation failure is fatal.
const CREATE_ATTEMPTS: u8 = 8;

/// Names reported by [`SandboxRoot`] creation.
static SANDBOX_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Why a private sandbox directory could not be created or validated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SandboxError {
    /// The base path is not absolute.
    #[error("sandbox base must be an absolute path")]
    RelativeBase,
    /// The base path cannot be inspected or canonicalized.
    #[error("sandbox base is unavailable")]
    BaseUnavailable,
    /// The base path is a symlink.
    #[error("sandbox base must not be a symlink")]
    BaseSymlink,
    /// The base path is not a directory.
    #[error("sandbox base must be a directory")]
    BaseNotDirectory,
    /// The base is neither owned by the effective user nor a sticky shared directory.
    #[error(
        "sandbox base must be owned by the effective user unless it is a sticky shared directory"
    )]
    BaseOwnerMismatch,
    /// The base is group/world writable without the sticky bit.
    #[error("sandbox base must not be group or world writable without the sticky bit")]
    BasePermissions,
    /// The effective UID could not be read.
    #[error("sandbox base owner could not be verified")]
    BaseOwnerUnverified,
    /// The base path is inside the installed package tree.
    #[error("sandbox base is inside the installed package")]
    BaseInsideInstall,
    /// A directory with the generated name already exists.
    #[error("sandbox name already exists")]
    AlreadyExists,
    /// The directory could not be created, inspected or chmodded.
    #[error("sandbox directory could not be created or inspected")]
    Create,
    /// The created path is a symlink or not a directory.
    #[error("sandbox path is not a real directory")]
    NotDirectory,
    /// The created directory does not have mode `0700`.
    #[error("sandbox directory must be private (0700)")]
    NotPrivate,
    /// The created directory is not empty.
    #[error("sandbox directory must be empty")]
    NotEmpty,
    /// The created directory canonicalizes outside its base.
    #[error("sandbox directory escaped its base")]
    EscapedBase,
    /// The platform has no supported private-directory permissions.
    #[error("native sandbox is not supported on this platform")]
    UnsupportedPlatform,
}

/// Validated base directory for per-session sandboxes.
#[derive(Debug, Clone)]
pub(crate) struct SandboxRoot {
    base: PathBuf,
}

impl SandboxRoot {
    /// Validates `base` and rejects a base inside `forbidden`.
    ///
    /// The base must be a real directory. It must be owned by the effective
    /// user, except for a root-owned sticky system temporary directory such as
    /// `/tmp`; the sticky bit protects the uniquely named entry we create
    /// there. Group or world write access is only accepted together with the
    /// sticky bit.
    pub(crate) fn new(
        base: impl Into<PathBuf>,
        forbidden: Option<&Path>,
    ) -> Result<Self, SandboxError> {
        let effective_uid = effective_uid().ok_or(SandboxError::BaseOwnerUnverified)?;
        Self::new_with_effective_uid(base, forbidden, effective_uid)
    }

    fn new_with_effective_uid(
        base: impl Into<PathBuf>,
        forbidden: Option<&Path>,
        effective_uid: u32,
    ) -> Result<Self, SandboxError> {
        let base = base.into();
        if !base.is_absolute() {
            return Err(SandboxError::RelativeBase);
        }
        let metadata = fs::symlink_metadata(&base).map_err(|_| SandboxError::BaseUnavailable)?;
        if metadata.file_type().is_symlink() {
            return Err(SandboxError::BaseSymlink);
        }
        if !metadata.is_dir() {
            return Err(SandboxError::BaseNotDirectory);
        }
        let mode = file_mode(&metadata);
        let sticky = mode & 0o1000 != 0;
        if !is_trusted_base_owner(file_owner(&metadata), effective_uid, mode) {
            return Err(SandboxError::BaseOwnerMismatch);
        }
        if mode & 0o022 != 0 && !sticky {
            return Err(SandboxError::BasePermissions);
        }

        let base = fs::canonicalize(&base).map_err(|_| SandboxError::BaseUnavailable)?;
        if let Some(forbidden) = forbidden {
            let forbidden =
                fs::canonicalize(forbidden).map_err(|_| SandboxError::BaseUnavailable)?;
            ensure_outside_install(&base, &forbidden)?;
        }

        Ok(Self { base })
    }

    /// Creates a root below the operating system temporary directory.
    pub(crate) fn from_temp(forbidden: &Path) -> Result<Self, SandboxError> {
        Self::new(std::env::temp_dir(), Some(forbidden))
    }

    /// Test-only root at an explicit base without an install-tree exclusion.
    #[cfg(test)]
    pub(crate) fn at(base: impl Into<PathBuf>) -> Result<Self, SandboxError> {
        Self::new(base, None)
    }

    /// Proves that one private directory can be created and removed.
    pub(crate) fn probe(&self) -> Result<(), SandboxError> {
        let dir = self.create()?;
        drop(dir);
        Ok(())
    }

    /// Creates a unique private sandbox directory.
    pub(crate) fn create(&self) -> Result<SandboxDir, SandboxError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        for _ in 0..CREATE_ATTEMPTS {
            let name = format!(
                "wxml-lsp-{}-{nanos}-{}",
                std::process::id(),
                SANDBOX_COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            match self.create_named(&name) {
                Ok(dir) => return Ok(dir),
                Err(SandboxError::AlreadyExists) => {}
                Err(error) => return Err(error),
            }
        }
        Err(SandboxError::Create)
    }

    fn create_named(&self, name: &str) -> Result<SandboxDir, SandboxError> {
        let path = self.base.join(name);
        match create_private_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(SandboxError::AlreadyExists);
            }
            Err(_) => return Err(SandboxError::Create),
        }
        self.finalize(&path)
    }

    fn finalize(&self, path: &Path) -> Result<SandboxDir, SandboxError> {
        let result = self.finalize_inner(path);
        if result.is_err() {
            let _ = fs::remove_dir(path);
        }
        result
    }

    fn finalize_inner(&self, path: &Path) -> Result<SandboxDir, SandboxError> {
        set_private_mode(path)?;
        let metadata = fs::symlink_metadata(path).map_err(|_| SandboxError::Create)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SandboxError::NotDirectory);
        }
        if file_mode(&metadata) & 0o777 != 0o700 {
            return Err(SandboxError::NotPrivate);
        }
        let mut entries = fs::read_dir(path).map_err(|_| SandboxError::Create)?;
        if entries.next().is_some() {
            return Err(SandboxError::NotEmpty);
        }
        let canonical = fs::canonicalize(path).map_err(|_| SandboxError::Create)?;
        if canonical.parent() != Some(self.base.as_path()) {
            return Err(SandboxError::EscapedBase);
        }
        Ok(SandboxDir {
            path: canonical,
            _base: self.base.clone(),
        })
    }
}

/// Rejects a sandbox base inside the package root, its `versions` container or
/// the inferred install root. A test package that is not laid out as
/// `<install>/versions/<version>` only enforces the package-root check.
fn ensure_outside_install(base: &Path, package_root: &Path) -> Result<(), SandboxError> {
    if base == package_root || base.starts_with(package_root) {
        return Err(SandboxError::BaseInsideInstall);
    }

    let Some(versions) = package_root.parent() else {
        return Ok(());
    };
    if versions.file_name().and_then(|name| name.to_str()) != Some("versions") {
        return Ok(());
    }
    if base == versions || base.starts_with(versions) {
        return Err(SandboxError::BaseInsideInstall);
    }
    let Some(install_root) = versions.parent() else {
        return Ok(());
    };
    if base == install_root || base.starts_with(install_root) {
        return Err(SandboxError::BaseInsideInstall);
    }
    Ok(())
}

/// A private sandbox directory owned by one native session.
///
/// Drop removes only the empty directory. Cleanup errors are intentionally
/// ignored because `Drop` cannot report them; a non-empty directory is left in
/// place rather than recursively deleting unknown contents.
#[derive(Debug)]
pub(crate) struct SandboxDir {
    path: PathBuf,
    _base: PathBuf,
}

impl SandboxDir {
    /// Returns the canonical sandbox path passed as the engine `--cwd`.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SandboxDir {
    fn drop(&mut self) {
        // Only remove the directory we created and only while it is empty.
        let _ = fs::remove_dir(&self.path);
    }
}

/// Session factory that creates a fresh sandbox for every native session.
#[derive(Debug, Clone)]
pub(crate) struct SandboxSessionFactory {
    executable: PathBuf,
    root: SandboxRoot,
}

impl SandboxSessionFactory {
    /// Builds a factory whose base is the OS temporary directory.
    pub(crate) fn new(
        executable: impl Into<PathBuf>,
        package_root: &Path,
    ) -> Result<Self, SandboxError> {
        let root = SandboxRoot::from_temp(package_root)?;
        root.probe()?;
        Ok(Self {
            executable: executable.into(),
            root,
        })
    }

    /// Test-only factory at an explicit sandbox base.
    #[cfg(test)]
    pub(crate) fn with_base(
        executable: impl Into<PathBuf>,
        base: impl Into<PathBuf>,
    ) -> Result<Self, SandboxError> {
        let root = SandboxRoot::at(base)?;
        root.probe()?;
        Ok(Self {
            executable: executable.into(),
            root,
        })
    }
}

impl SessionFactory for SandboxSessionFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        let sandbox = self.root.create().map_err(|_| NativeError::Unavailable {
            // The frozen reason list has no sandbox category; the sandbox is
            // the VFS confinement boundary, so this is the closest stable
            // category and still fails closed.
            reason: UnavailableReason::VfsConfinement,
        })?;
        let options = SessionOptions::new(self.executable.clone(), sandbox.path());
        let inner = NativeSessionFactory::new(options).start()?;
        Ok(Box::new(SandboxedSession { inner, sandbox }))
    }
}

struct SandboxedSession {
    inner: Box<dyn QuerySession>,
    sandbox: SandboxDir,
}

impl QuerySession for SandboxedSession {
    fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.inner.open_project(input)
    }

    fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.inner.update_project(input)
    }

    fn semantic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.inner.semantic_diagnostics(path)
    }

    fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.inner.syntactic_diagnostics(path)
    }

    fn type_query(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<TypeQuery, NativeError> {
        self.inner.type_query(path, position)
    }

    fn hover_documentation(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverDocumentation, NativeError> {
        self.inner.hover_documentation(path, position)
    }

    fn hover_evidence(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError> {
        self.inner.hover_evidence(path, position)
    }

    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.inner.completions(path, position)
    }

    fn completions_with_options(
        &mut self,
        path: &str,
        position: Utf16Position,
        options: CompletionQueryOptions,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.inner.completions_with_options(path, position, options)
    }

    fn prepare_auto_imports(&mut self, path: &str) -> Result<AutoImportPreparation, NativeError> {
        self.inner.prepare_auto_imports(path)
    }

    fn definition(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<NativeDefinitionResult, NativeError> {
        self.inner.definition(path, position)
    }

    fn document_symbols(&mut self, path: &str) -> Result<DocumentSymbolsEvidence, NativeError> {
        self.inner.document_symbols(path)
    }

    fn environment_degradation(&self) -> Option<String> {
        self.inner.environment_degradation()
    }

    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }

    fn shutdown(self: Box<Self>) {
        let Self { inner, sandbox } = *self;
        inner.shutdown();
        drop(sandbox);
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(unix)]
fn set_private_mode(path: &Path) -> Result<(), SandboxError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| SandboxError::Create)
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path) -> Result<(), SandboxError> {
    Err(SandboxError::UnsupportedPlatform)
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn file_owner(metadata: &fs::Metadata) -> u32 {
    metadata.uid()
}

#[cfg(not(unix))]
fn file_owner(_metadata: &fs::Metadata) -> u32 {
    0
}

/// Accepts the effective user or a root-owned sticky system temp directory.
///
/// `/tmp` is commonly root-owned mode `1777`; the sticky bit plus root
/// ownership is the standard shared-temp exception. A sticky directory owned
/// by another unprivileged user is not accepted because that owner could
/// remove our entries.
const fn is_trusted_base_owner(owner: u32, effective_uid: u32, mode: u32) -> bool {
    owner == effective_uid || (mode & 0o1000 != 0 && owner == 0)
}

/// Returns the effective UID from `/proc/self/status`.
///
/// The native contract is Linux-only and this avoids adding a `libc`
/// dependency or an `unsafe` call. A missing or unparseable status file fails
/// closed through [`SandboxError::BaseOwnerUnverified`].
fn effective_uid() -> Option<u32> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let uid_line = status.lines().find(|line| line.starts_with("Uid:"))?;
    uid_line.split_whitespace().nth(2)?.parse().ok()
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use crate::backend_tsgo::query::hover::HoverGap;
    use crate::backend_tsgo::query::type_info::{TypeInfo, TypeQuality};
    use wxml_types::CapabilityStatus;

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(label: &str) -> Self {
            let base =
                std::env::var_os("WXML_TEST_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
            assert!(base.is_absolute(), "WXML_TEST_TMPDIR must be absolute");
            fs::create_dir_all(&base).expect("test base");
            let path = base.join(format!(
                "wxml-lsp-sandbox-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("test root");
            set_mode(&path, 0o700);
            Self { path }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("permissions");
    }

    fn fingerprint(root: &Path) -> Vec<(PathBuf, u32, bool)> {
        let mut entries = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("read directory") {
                let entry = entry.expect("directory entry");
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).expect("metadata");
                entries.push((path.clone(), file_mode(&metadata), metadata.is_dir()));
                if metadata.is_dir() && !metadata.file_type().is_symlink() {
                    pending.push(path);
                }
            }
        }
        entries.sort();
        entries
    }

    /// Inner session that only answers `hover_evidence`.
    ///
    /// Every legacy query fails, so a wrapper that forwards the wrong method
    /// fails the test instead of silently producing hover evidence.
    struct HoverEvidenceSession {
        calls: Arc<AtomicUsize>,
    }

    impl QuerySession for HoverEvidenceSession {
        fn open_project(&mut self, _input: &AnalysisInput) -> Result<(), NativeError> {
            Err(test_unsupported())
        }

        fn update_project(&mut self, _input: &AnalysisInput) -> Result<(), NativeError> {
            Err(test_unsupported())
        }

        fn semantic_diagnostics(&mut self, _path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
            Err(test_unsupported())
        }

        fn syntactic_diagnostics(
            &mut self,
            _path: &str,
        ) -> Result<Vec<RawDiagnostic>, NativeError> {
            Err(test_unsupported())
        }

        fn type_query(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<TypeQuery, NativeError> {
            Err(test_unsupported())
        }

        fn hover_documentation(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<HoverDocumentation, NativeError> {
            Err(test_unsupported())
        }

        fn hover_evidence(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<HoverEvidence, NativeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(gap_evidence())
        }

        fn completions(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<Option<RawCompletionList>, NativeError> {
            Err(test_unsupported())
        }

        fn definition(
            &mut self,
            _path: &str,
            _position: Utf16Position,
        ) -> Result<NativeDefinitionResult, NativeError> {
            Err(test_unsupported())
        }

        fn document_symbols(
            &mut self,
            _path: &str,
        ) -> Result<DocumentSymbolsEvidence, NativeError> {
            Err(test_unsupported())
        }

        fn shutdown(self: Box<Self>) {}
    }

    const fn test_unsupported() -> NativeError {
        NativeError::Unsupported {
            method: "sandbox hover forwarding test",
        }
    }

    /// Evidence with a typed declaration gap, which must survive forwarding.
    fn gap_evidence() -> HoverEvidence {
        let mut evidence = HoverEvidence {
            type_info: Some(TypeInfo {
                text: "string".to_owned(),
                handle: 1,
                flags: 0,
            }),
            type_quality: TypeQuality::Known,
            symbol: None,
            documentation: None,
            declarations: Vec::new(),
            status: CapabilityStatus::Supported,
            gaps: vec![HoverGap::ExternalDeclaration {
                path: Some("lib.d.ts".to_owned()),
            }],
        };
        evidence.finalize_status();
        evidence
    }

    #[test]
    fn sandboxed_session_forwards_hover_evidence() {
        let test = TestRoot::new("hover-forward");
        let root = SandboxRoot::at(&test.path).expect("root");
        let sandbox = root.create().expect("sandbox");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut session = SandboxedSession {
            inner: Box::new(HoverEvidenceSession {
                calls: Arc::clone(&calls),
            }),
            sandbox,
        };

        let evidence = session
            .hover_evidence(
                "main.ts",
                Utf16Position {
                    line: 3,
                    character: 4,
                },
            )
            .expect("forwarded hover evidence");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(evidence, gap_evidence());
        assert_eq!(
            evidence.status,
            CapabilityStatus::Partial {
                reason: "a declaration is outside the project snapshot".to_owned(),
            }
        );
    }

    #[test]
    fn creates_private_empty_non_symlink_directory() {
        let test = TestRoot::new("private");
        let root = SandboxRoot::at(&test.path).expect("root");
        let sandbox = root.create().expect("sandbox");

        let metadata = fs::symlink_metadata(sandbox.path()).expect("metadata");
        assert!(!metadata.file_type().is_symlink());
        assert!(metadata.is_dir());
        assert_eq!(file_mode(&metadata) & 0o777, 0o700);
        assert!(sandbox.path().read_dir().unwrap().next().is_none());
        assert_eq!(
            fs::canonicalize(sandbox.path()).unwrap().parent(),
            Some(fs::canonicalize(&test.path).unwrap().as_path())
        );
    }

    #[test]
    fn unique_per_call() {
        let test = TestRoot::new("unique");
        let root = SandboxRoot::at(&test.path).expect("root");
        let first = root.create().expect("first sandbox");
        let second = root.create().expect("second sandbox");
        assert_ne!(first.path(), second.path());
    }

    #[test]
    fn cleans_up_on_drop() {
        let test = TestRoot::new("cleanup");
        let root = SandboxRoot::at(&test.path).expect("root");
        let sandbox = root.create().expect("sandbox");
        let path = sandbox.path().to_path_buf();
        assert!(path.exists());
        drop(sandbox);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_relative_base() {
        assert!(matches!(
            SandboxRoot::at("relative/path"),
            Err(SandboxError::RelativeBase)
        ));
    }

    #[test]
    fn rejects_symlink_base() {
        let test = TestRoot::new("symlink-base");
        let target = test.path.join("target");
        fs::create_dir(&target).unwrap();
        let link = test.path.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            SandboxRoot::at(&link),
            Err(SandboxError::BaseSymlink)
        ));
    }

    #[test]
    fn rejects_file_base() {
        let test = TestRoot::new("file-base");
        let file = test.path.join("base-file");
        fs::write(&file, b"not a directory").unwrap();
        assert!(matches!(
            SandboxRoot::at(&file),
            Err(SandboxError::BaseNotDirectory)
        ));
    }

    #[test]
    fn effective_uid_matches_the_test_root_owner() {
        let test = TestRoot::new("effective-uid");
        let metadata = fs::metadata(&test.path).unwrap();
        assert_eq!(effective_uid(), Some(file_owner(&metadata)));
    }

    #[test]
    fn rejects_base_owned_by_another_user_without_sticky() {
        let test = TestRoot::new("owner-mismatch");
        let base = test.path.join("base");
        fs::create_dir(&base).unwrap();
        set_mode(&base, 0o755);
        let owner = file_owner(&fs::metadata(&base).unwrap());
        assert!(matches!(
            SandboxRoot::new_with_effective_uid(&base, None, owner.wrapping_add(1)),
            Err(SandboxError::BaseOwnerMismatch)
        ));
        SandboxRoot::new_with_effective_uid(&base, None, owner).expect("owner matches");
    }

    #[test]
    fn permits_a_sticky_base_owned_by_the_effective_user() {
        let test = TestRoot::new("sticky-owned");
        let base = test.path.join("base");
        fs::create_dir(&base).unwrap();
        set_mode(&base, 0o1777);
        let owner = file_owner(&fs::metadata(&base).unwrap());

        let root = SandboxRoot::new_with_effective_uid(&base, None, owner)
            .expect("sticky base owned by the effective user");
        let sandbox = root.create().expect("sandbox");
        let metadata = fs::symlink_metadata(sandbox.path()).unwrap();
        assert_eq!(file_mode(&metadata) & 0o777, 0o700);
    }

    #[test]
    fn rejects_a_sticky_base_owned_by_another_unprivileged_user() {
        let test = TestRoot::new("sticky-foreign");
        let base = test.path.join("base");
        fs::create_dir(&base).unwrap();
        set_mode(&base, 0o1777);
        let owner = file_owner(&fs::metadata(&base).unwrap());
        let other = if owner == 0 { 1 } else { owner.wrapping_add(1) };

        assert!(matches!(
            SandboxRoot::new_with_effective_uid(&base, None, other),
            Err(SandboxError::BaseOwnerMismatch)
        ));
    }

    #[test]
    fn accepts_only_root_owned_or_effective_user_owners() {
        assert!(is_trusted_base_owner(0, 1000, 0o1777));
        assert!(is_trusted_base_owner(1000, 1000, 0o1777));
        assert!(!is_trusted_base_owner(1001, 1000, 0o1777));
        assert!(!is_trusted_base_owner(0, 1000, 0o0755));
    }

    #[test]
    fn rejects_group_or_world_writable_non_sticky_base() {
        let test = TestRoot::new("unsafe-modes");
        let base = test.path.join("base");
        fs::create_dir(&base).unwrap();
        let owner = file_owner(&fs::metadata(&base).unwrap());
        for mode in [0o775, 0o757, 0o777] {
            set_mode(&base, mode);
            assert!(
                matches!(
                    SandboxRoot::new_with_effective_uid(&base, None, owner),
                    Err(SandboxError::BasePermissions)
                ),
                "mode {mode:o} must be rejected without the sticky bit"
            );
        }
    }

    #[test]
    fn rejects_base_inside_the_install_package() {
        let test = TestRoot::new("install-base");
        let package = test.path.join("package");
        let inner = package.join("tmp");
        fs::create_dir_all(&inner).unwrap();
        assert!(matches!(
            SandboxRoot::new(&inner, Some(&package)),
            Err(SandboxError::BaseInsideInstall)
        ));
    }

    #[test]
    fn rejects_base_inside_versions_or_the_install_root() {
        let test = TestRoot::new("versions-base");
        let install = test.path.join("install");
        let package = install.join("versions/1.0.0");
        fs::create_dir_all(&package).unwrap();
        let install_tmp = install.join("tmp");
        fs::create_dir_all(&install_tmp).unwrap();
        let versions_tmp = install.join("versions/tmp");
        fs::create_dir_all(&versions_tmp).unwrap();
        let package_sandbox = package.join("sandbox");
        fs::create_dir_all(&package_sandbox).unwrap();

        assert!(matches!(
            SandboxRoot::new(&install_tmp, Some(&package)),
            Err(SandboxError::BaseInsideInstall)
        ));
        assert!(matches!(
            SandboxRoot::new(&versions_tmp, Some(&package)),
            Err(SandboxError::BaseInsideInstall)
        ));
        assert!(matches!(
            SandboxRoot::new(&package_sandbox, Some(&package)),
            Err(SandboxError::BaseInsideInstall)
        ));
        assert!(SandboxRoot::new(&test.path, Some(&package)).is_ok());
    }

    #[test]
    fn refuses_to_reuse_an_existing_name() {
        let test = TestRoot::new("existing");
        let root = SandboxRoot::at(&test.path).expect("root");
        let taken = test.path.join("taken");
        fs::write(&taken, b"keep me").unwrap();

        assert!(matches!(
            root.create_named("taken"),
            Err(SandboxError::AlreadyExists)
        ));
        assert_eq!(fs::read(&taken).unwrap(), b"keep me");
    }

    #[test]
    fn rejects_a_non_empty_directory_and_keeps_its_contents() {
        let test = TestRoot::new("nonempty");
        let root = SandboxRoot::at(&test.path).expect("root");
        let manual = test.path.join("manual");
        fs::create_dir(&manual).unwrap();
        set_mode(&manual, 0o700);
        fs::write(manual.join("unexpected"), b"x").unwrap();

        assert!(matches!(
            root.finalize(&manual),
            Err(SandboxError::NotEmpty)
        ));
        assert!(manual.join("unexpected").exists());
    }

    #[test]
    fn probe_leaves_no_directory_behind() {
        let test = TestRoot::new("probe");
        let root = SandboxRoot::at(&test.path).expect("root");
        root.probe().expect("probe");
        assert_eq!(fs::read_dir(&test.path).unwrap().count(), 0);
    }

    #[test]
    fn never_writes_the_install_tree() {
        let test = TestRoot::new("install-tree");
        let package = test.path.join("package");
        for directory in [
            package.clone(),
            package.join("bin"),
            package.join("engine/lib"),
        ] {
            fs::create_dir_all(&directory).unwrap();
            set_mode(&directory, 0o755);
        }
        fs::write(package.join("manifest.json"), b"{}").unwrap();
        set_mode(&package.join("manifest.json"), 0o644);
        fs::write(package.join("engine/lib/tsc"), b"engine").unwrap();
        set_mode(&package.join("engine/lib/tsc"), 0o755);
        let before = fingerprint(&package);

        let sandbox_base = test.path.join("sandboxes");
        fs::create_dir(&sandbox_base).unwrap();
        set_mode(&sandbox_base, 0o700);
        let root = SandboxRoot::at(&sandbox_base).expect("root");
        drop(root.create().expect("sandbox"));

        assert_eq!(fingerprint(&package), before);
        assert_eq!(fs::read_dir(&sandbox_base).unwrap().count(), 0);
    }

    #[test]
    fn factory_cleans_up_after_a_failed_session_start() {
        let test = TestRoot::new("factory");
        let factory =
            SandboxSessionFactory::with_base(test.path.join("missing-engine"), &test.path)
                .expect("factory");

        let result = factory.start();
        assert!(matches!(
            result,
            Err(NativeError::Unavailable {
                reason: UnavailableReason::ArtifactMismatch
            })
        ));
        assert_eq!(
            fs::read_dir(&test.path)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .count(),
            0
        );
    }
}
