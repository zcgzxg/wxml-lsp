//! Canonical project-relative paths, file keys and `file://` URI handling.
//!
//! Every path that crosses the project boundary is first parsed into
//! [`ProjectPath`]. The type guarantees the path is relative to the project
//! root, uses `/` separators and contains no `.`, `..`, empty or NUL
//! components, so file-system access can join it to the root without a second
//! traversal check. [`read_file`] still resolves symlinks and rejects targets
//! outside the root because lexical checks cannot see them.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

/// Maximum size of a file returned by [`read_file`].
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum bytes kept from a user-supplied diagnostic string.
pub(crate) const MAX_MESSAGE_BYTES: usize = 256;

/// A canonical, project-relative path.
///
/// Construction normalizes `\` to `/`, removes leading `./` segments and
/// validates every component. The path is case-sensitive and keeps its
/// original case and Unicode form.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProjectPath(String);

impl ProjectPath {
    /// Creates a canonical project path.
    ///
    /// # Errors
    ///
    /// Returns [`PathError`] for empty input, NUL bytes, absolute or drive
    /// paths, and empty, `.` or `..` components.
    pub fn new(raw: impl Into<String>) -> Result<Self, PathError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        if raw.contains('\0') {
            return Err(PathError::Nul);
        }
        if raw.chars().any(char::is_control) {
            return Err(PathError::ControlCharacter);
        }
        if raw.chars().any(is_forbidden_format_character) {
            return Err(PathError::FormatCharacter);
        }
        let mut normalized = raw.replace('\\', "/");
        while let Some(rest) = normalized.strip_prefix("./") {
            normalized = rest.to_owned();
        }
        if normalized.is_empty() {
            return Err(PathError::Empty);
        }
        if is_absolute(&normalized) {
            return Err(PathError::Absolute);
        }
        for component in normalized.split('/') {
            match component {
                "" => return Err(PathError::EmptyComponent),
                "." => return Err(PathError::CurrentComponent),
                ".." => return Err(PathError::ParentComponent),
                _ => {}
            }
        }
        Ok(Self(normalized))
    }

    /// Returns the canonical `/`-separated path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the extension without the leading dot.
    #[must_use]
    pub fn extension(&self) -> Option<&str> {
        Path::new(self.0.as_str())
            .extension()
            .and_then(std::ffi::OsStr::to_str)
    }

    /// Returns the final path component.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(self.0.as_str())
    }

    /// Returns the parent directory, or `None` for a top-level path.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.0
            .rsplit_once('/')
            .map(|(parent, _)| Self(parent.to_owned()))
    }

    /// Returns a copy with `extension` replaced or appended.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::InvalidExtension`] when `extension` is empty or
    /// contains a path separator or NUL.
    pub fn with_extension(&self, extension: &str) -> Result<Self, PathError> {
        if extension.is_empty()
            || extension.contains('/')
            || extension.contains('\\')
            || extension.contains('\0')
        {
            return Err(PathError::InvalidExtension);
        }
        let mut path = PathBuf::from(self.0.as_str());
        path.set_extension(extension);
        let Some(path) = path.to_str() else {
            return Err(PathError::InvalidExtension);
        };
        Self::new(path)
    }

    /// Returns a copy with `extension` appended to the final name.
    ///
    /// Unlike [`ProjectPath::with_extension`], a base whose last component
    /// already contains a dot keeps it: `card.v2` becomes `card.v2.wxml`.
    /// Page/component routes use this form because the route is the full base
    /// name, not a filename with an extension.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::InvalidExtension`] for an empty extension or one
    /// containing a path separator or NUL.
    pub fn with_appended_extension(&self, extension: &str) -> Result<Self, PathError> {
        if extension.is_empty()
            || extension.contains('/')
            || extension.contains('\\')
            || extension.contains('\0')
        {
            return Err(PathError::InvalidExtension);
        }
        Self::new(format!("{}.{}", self.0, extension))
    }

    /// Resolves a path specifier against a base directory.
    ///
    /// A specifier starting with `/` is resolved against the project root and
    /// ignores `base_dir`; otherwise `base_dir` is used, or the project root
    /// when it is `None`. `.` components are dropped and `..` pops one
    /// component. Other path forms are normalized rather than rejected, which
    /// makes this suitable for `usingComponents` values.
    ///
    /// Drive-letter and UNC forms are rejected even when they start with `/`
    /// (for example `/C:/x`), because the result is validated as a canonical
    /// project path.
    ///
    /// # Errors
    ///
    /// Returns [`PathError`] when the specifier is empty, contains NUL or
    /// control characters, is a drive or UNC path, or resolves above the
    /// project root.
    pub fn resolve(base_dir: Option<&Self>, specifier: &str) -> Result<Self, PathError> {
        if specifier.is_empty() {
            return Err(PathError::Empty);
        }
        if specifier.contains('\0') {
            return Err(PathError::Nul);
        }
        let normalized = specifier.replace('\\', "/");
        let root_relative = normalized.starts_with('/');
        if !root_relative && is_absolute(&normalized) {
            return Err(PathError::Absolute);
        }
        if normalized.starts_with("//") {
            return Err(PathError::Absolute);
        }

        let mut components: Vec<&str> = if root_relative {
            Vec::new()
        } else {
            base_dir.map_or_else(Vec::new, |base| base.0.split('/').collect())
        };
        for component in normalized.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    if components.pop().is_none() {
                        return Err(PathError::EscapesRoot);
                    }
                }
                other => components.push(other),
            }
        }
        if components.is_empty() {
            return Err(PathError::Empty);
        }
        // Route through `new` so absolute forms such as `/C:/x` (drive letters
        // become the first component after dropping the leading slash) and
        // control characters are rejected by the canonical-path rules.
        Self::new(components.join("/"))
    }
}

impl fmt::Display for ProjectPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A cheap, owned key for one file inside one project.
///
/// The key is an interned canonical path. It deliberately carries no project
/// identifier: an [`Overlay`](crate::overlay::Overlay) belongs to one project,
/// and the LSP layer maps the key to its own `FileId`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileKey(Arc<str>);

impl FileKey {
    /// Creates a key from a canonical project path.
    #[must_use]
    pub fn new(path: &ProjectPath) -> Self {
        Self(Arc::from(path.as_str()))
    }

    /// Returns the canonical project-relative path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&ProjectPath> for FileKey {
    fn from(path: &ProjectPath) -> Self {
        Self::new(path)
    }
}

impl From<ProjectPath> for FileKey {
    fn from(path: ProjectPath) -> Self {
        Self::new(&path)
    }
}

impl fmt::Display for FileKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a [`ProjectPath`] could not be created or resolved.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum PathError {
    /// The path is empty after normalization.
    #[error("path must not be empty")]
    Empty,
    /// The path contains a NUL byte.
    #[error("path must not contain NUL")]
    Nul,
    /// The path contains an ASCII or Unicode control character.
    #[error("path must not contain control characters")]
    ControlCharacter,
    /// The path contains a bidirectional or zero-width format character.
    #[error("path must not contain bidirectional or zero-width format characters")]
    FormatCharacter,
    /// The path is absolute (`/`, a drive letter or a UNC path).
    #[error("path must be project-relative, not absolute")]
    Absolute,
    /// The path contains an empty component (`//` or a trailing `/`).
    #[error("path must not contain empty components")]
    EmptyComponent,
    /// The path contains a `.` component.
    #[error("path must not contain `.` components")]
    CurrentComponent,
    /// The path contains a `..` component.
    #[error("path must not contain `..` components")]
    ParentComponent,
    /// Resolution would leave the project root.
    #[error("path must stay inside the project root")]
    EscapesRoot,
    /// The requested extension is not a plain file extension.
    #[error("extension must be a non-empty name without separators")]
    InvalidExtension,
}

/// Why a `file://` URI could not be converted to a [`ProjectPath`].
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum UriError {
    /// The URI does not start with the `file` scheme.
    #[error("URI scheme must be `file`")]
    NotFileScheme,
    /// The URI carries a query or fragment.
    #[error("file URI must not contain a query or fragment")]
    UnsupportedComponent,
    /// The URI names a remote host instead of the local file system.
    #[error("file URI must not name a remote host")]
    RemoteHost,
    /// The URI path is not absolute.
    #[error("file URI must contain an absolute path")]
    RelativePath,
    /// The URI contains a malformed percent escape.
    #[error("file URI contains an invalid percent escape")]
    InvalidPercentEscape,
    /// The decoded URI path is not valid UTF-8.
    #[error("file URI path is not valid UTF-8")]
    InvalidUtf8,
    /// The URI path is outside the project root.
    #[error("file URI path is outside the project root")]
    OutsideRoot,
    /// The URI path contains a `..` component.
    #[error("file URI path must not contain `..`")]
    ParentComponent,
    /// The URI path contains a NUL byte.
    #[error("file URI path must not contain NUL")]
    Nul,
    /// The URI path contains a control or bidirectional format character.
    #[error("file URI path contains control or format characters")]
    InvalidCharacter,
    /// The relative part of the URI is not a valid project path.
    #[error(transparent)]
    Path(#[from] PathError),
}

/// Why a project file could not be read.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum FileError {
    /// The file does not exist.
    #[error("file not found: {path}")]
    NotFound {
        /// Project-relative path of the missing file.
        path: ProjectPath,
    },
    /// A symlink or path component leads outside the project root.
    #[error("path escapes the project root: {path}")]
    EscapesRoot {
        /// Project-relative path that escaped the root.
        path: ProjectPath,
    },
    /// The file exists but is not valid UTF-8.
    #[error("file is not valid UTF-8: {path}")]
    InvalidUtf8 {
        /// Project-relative path of the file.
        path: ProjectPath,
    },
    /// The file is larger than [`MAX_FILE_BYTES`].
    #[error("file is too large: {path} ({size} bytes)")]
    TooLarge {
        /// Project-relative path of the file.
        path: ProjectPath,
        /// Observed size in bytes.
        size: u64,
    },
    /// The file system reported an error.
    #[error("failed to read {path}: {kind:?}")]
    Io {
        /// Project-relative path of the file.
        path: ProjectPath,
        /// Operating-system error category. The raw message may embed a
        /// host-specific absolute path, so it is not retained.
        kind: std::io::ErrorKind,
    },
}

/// Converts an absolute `file://` URI into a project-relative path.
///
/// `root` must be an absolute, canonical directory (as produced by
/// [`std::fs::canonicalize`]). Percent escapes are decoded, `localhost` and an
/// empty authority are accepted, and the decoded path must stay inside `root`.
/// A symlink target outside `root` is rejected when the target exists.
///
/// Query and fragment parts are rejected because a file identity never
/// includes them. Windows drive-letter URIs are normalized on Windows,
/// including verbatim `\\?\` prefixes from [`std::fs::canonicalize`]; UNC and
/// device paths are rejected. Windows handling is **unverified on this host**
/// (Linux) and fails closed. On other platforms drive-letter input is reported
/// as [`UriError::OutsideRoot`].
///
/// Percent escapes are decoded before validation, so an encoded `..` or NUL is
/// rejected. Backslash characters are normalized to `/`, matching
/// [`ProjectPath`] canonicalization. This function is subject to the same
/// time-of-check/time-of-use limitations as any path check: a symlink can
/// change between this call and a later read. [`read_file`] re-resolves and
/// re-checks before reading.
///
/// # Errors
///
/// Returns [`UriError`] for non-`file` schemes, remote authorities, malformed
/// percent escapes, non-UTF-8 or control-character paths, `..` components, NUL
/// bytes, and paths outside `root`.
pub fn project_path_from_uri(root: &Path, uri: &str) -> Result<ProjectPath, UriError> {
    let normalized = decoded_file_uri_path(uri)?;
    #[cfg(windows)]
    let normalized = strip_windows_drive_prefix(&normalized);

    let absolute = Path::new(&normalized);
    #[cfg(windows)]
    let path = windows_project_path(root, absolute)?;
    #[cfg(not(windows))]
    let path = {
        let relative = absolute
            .strip_prefix(root)
            .map_err(|_| UriError::OutsideRoot)?;
        let Some(relative) = relative.to_str() else {
            return Err(UriError::InvalidUtf8);
        };
        ProjectPath::new(relative)?
    };

    match std::fs::canonicalize(absolute) {
        Ok(canonical) if !path_within(root, &canonical) => Err(UriError::OutsideRoot),
        Err(_) if parent_escapes_root(root, absolute) => Err(UriError::OutsideRoot),
        _ => Ok(path),
    }
}

/// Converts a `file://` URI into an absolute host path for a project root.
///
/// Only the local `file` scheme is accepted. An empty authority and
/// `localhost` are allowed; remote hosts, query/fragment parts, relative paths,
/// `..` components, NUL/control/format characters, malformed percent escapes
/// and non-UTF-8 paths are rejected. The result is **not** canonicalized; the
/// caller (for example [`crate::Project::open`]) applies its normal
/// canonicalization and symlink rules.
///
/// On Windows the drive-letter form `file:///C:/...` is accepted and verbatim
/// prefixes are rejected. Windows handling is implemented defensively but
/// **unverified on this host** (Linux) and fails closed for UNC/device paths.
///
/// # Errors
///
/// Returns [`UriError`] for the conditions listed above.
pub fn root_path_from_uri(uri: &str) -> Result<PathBuf, UriError> {
    let normalized = decoded_file_uri_path(uri)?;
    #[cfg(windows)]
    {
        let normalized = strip_windows_drive_prefix(&normalized);
        let bytes = normalized.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/'
        {
            Ok(PathBuf::from(normalized))
        } else {
            Err(UriError::OutsideRoot)
        }
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(normalized))
    }
}

/// Decodes and validates the path portion of a local `file://` URI.
///
/// Returns a normalized absolute path using `/` separators, with `\` mapped to
/// `/` for consistency with project paths.
fn decoded_file_uri_path(uri: &str) -> Result<String, UriError> {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return Err(UriError::NotFileScheme);
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Err(UriError::NotFileScheme);
    }
    let raw_path = if let Some(after_slashes) = rest.strip_prefix("//") {
        let (authority, path) = after_slashes.split_once('/').map_or_else(
            || (after_slashes, String::from("/")),
            |(authority, path)| (authority, format!("/{path}")),
        );
        if !(authority.is_empty() || authority.eq_ignore_ascii_case("localhost")) {
            return Err(UriError::RemoteHost);
        }
        path
    } else {
        rest.to_owned()
    };
    if raw_path.contains('?') || raw_path.contains('#') {
        return Err(UriError::UnsupportedComponent);
    }

    let decoded = percent_decode(&raw_path)?;
    if decoded.contains('\0') {
        return Err(UriError::Nul);
    }
    if decoded
        .chars()
        .any(|character| character.is_control() || is_forbidden_format_character(character))
    {
        return Err(UriError::InvalidCharacter);
    }
    let normalized = decoded.replace('\\', "/");
    if normalized.split('/').any(|component| component == "..") {
        return Err(UriError::ParentComponent);
    }
    if !normalized.starts_with('/') {
        return Err(UriError::RelativePath);
    }
    Ok(normalized)
}

/// Builds a `file://` URI for a project-relative path.
///
/// Returns `None` when the project root or the resulting absolute path cannot
/// be represented as UTF-8, or on Windows when the path does not use a
/// drive-letter prefix (UNC and verbatim prefixes are not emitted). The path is
/// percent-encoded; `/` separators are preserved. `root` must be absolute and
/// canonical for the result to map back through [`project_path_from_uri`].
///
/// On Windows the result uses `file:///C:/...` form. On Unix it uses
/// `file:///path` form.
#[must_use]
pub fn uri_for_project_path(root: &Path, path: &ProjectPath) -> Option<String> {
    let absolute = absolute_path(root, path);
    #[cfg(windows)]
    {
        let normalized = normalized_windows_text(&absolute)?;
        let bytes = normalized.as_bytes();
        let is_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if !is_drive {
            return None;
        }
        Some(format!("file:///{}", percent_encode(&normalized)))
    }
    #[cfg(not(windows))]
    {
        let text = absolute.to_str()?;
        Some(format!("file://{}", percent_encode(text)))
    }
}

/// Reads a project file and rejects symlink escapes.
///
/// The path is resolved with [`std::fs::canonicalize`]; if the real target is
/// not inside `root`, [`FileError::EscapesRoot`] is returned. When the target
/// does not exist, its parent directory is resolved so a symlinked directory
/// pointing outside the root is still reported as an escape. `root` must be an
/// absolute, canonical directory.
///
/// Files larger than [`MAX_FILE_BYTES`] are rejected before content is read.
/// The read itself is capped, so a file that grows concurrently cannot exceed
/// the limit in memory. This is still a time-of-check/time-of-use boundary: a
/// symlink can be swapped between canonicalization and open. Callers that need
/// stronger guarantees must hold OS-level handles or re-verify after reading.
///
/// Backslash characters in `ProjectPath` are normalized to `/`; therefore a
/// Unix file name that literally contains a backslash is not addressable
/// through this API.
///
/// # Errors
///
/// Returns [`FileError`] when the file is missing, escapes the root, is too
/// large, is not UTF-8, or cannot be read.
pub fn read_file(root: &Path, path: &ProjectPath) -> Result<String, FileError> {
    let absolute = absolute_path(root, path);
    let canonical = match std::fs::canonicalize(&absolute) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if parent_escapes_root(root, &absolute) {
                return Err(FileError::EscapesRoot { path: path.clone() });
            }
            return Err(FileError::NotFound { path: path.clone() });
        }
        Err(error) => {
            return Err(FileError::Io {
                path: path.clone(),
                kind: error.kind(),
            });
        }
    };
    if !path_within(root, &canonical) {
        return Err(FileError::EscapesRoot { path: path.clone() });
    }

    let file = File::open(&canonical).map_err(|error| FileError::Io {
        path: path.clone(),
        kind: error.kind(),
    })?;
    let metadata = file.metadata().map_err(|error| FileError::Io {
        path: path.clone(),
        kind: error.kind(),
    })?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge {
            path: path.clone(),
            size: metadata.len(),
        });
    }
    let mut bytes = Vec::new();
    let mut limited = file.take(MAX_FILE_BYTES + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| FileError::Io {
            path: path.clone(),
            kind: error.kind(),
        })?;
    let size = bytes.len() as u64;
    if size > MAX_FILE_BYTES {
        return Err(FileError::TooLarge {
            path: path.clone(),
            size,
        });
    }
    String::from_utf8(bytes).map_err(|_| FileError::InvalidUtf8 { path: path.clone() })
}

/// Returns whether the parent of `absolute` resolves outside `root`.
///
/// Used when the target itself does not exist: a symlinked directory can still
/// point outside even if the final file is absent.
fn parent_escapes_root(root: &Path, absolute: &Path) -> bool {
    absolute.parent().is_some_and(|parent| {
        std::fs::canonicalize(parent).is_ok_and(|canonical| !path_within(root, &canonical))
    })
}

/// Returns whether `candidate` is `root` or below it.
fn path_within(root: &Path, candidate: &Path) -> bool {
    #[cfg(windows)]
    {
        windows_within(root, candidate)
    }
    #[cfg(not(windows))]
    {
        candidate.starts_with(root)
    }
}

/// Truncates owned diagnostic text to `max_bytes` on a UTF-8 boundary.
pub(crate) fn truncate_owned(mut input: String, max_bytes: usize) -> String {
    if input.len() > max_bytes {
        let end = truncate_message(&input, max_bytes).len();
        input.truncate(end);
    }
    input
}

/// Truncates a diagnostic string to `max_bytes` on a UTF-8 boundary.
pub(crate) fn truncate_message(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

/// Converts `/C:/path` into `C:/path` on Windows.
#[cfg(windows)]
fn strip_windows_drive_prefix(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        path[1..].to_owned()
    } else {
        path.to_owned()
    }
}

/// Returns a URI path for a Windows drive-letter path, or `None` otherwise.
///
/// Verbatim `\\?\` prefixes are stripped for comparisons, and the drive letter
/// is case-normalized; UNC and device paths are rejected rather than guessed.
///
/// Windows support is implemented defensively but **unverified on this host**
/// (the crate is tested on Linux).
#[cfg(windows)]
fn normalized_windows_text(path: &Path) -> Option<String> {
    let text = path.to_str()?.replace('\\', "/");
    let text = if let Some(rest) = text.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = text.strip_prefix("//?/") {
        rest.to_owned()
    } else {
        text
    };
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let mut characters = text.chars();
        let drive = characters.next()?.to_ascii_uppercase();
        Some(format!("{drive}{}", characters.as_str()))
    } else {
        Some(text)
    }
}

/// Returns whether `candidate` is `root` or below it on Windows.
#[cfg(windows)]
fn windows_within(root: &Path, candidate: &Path) -> bool {
    match (
        normalized_windows_text(root),
        normalized_windows_text(candidate),
    ) {
        (Some(root), Some(candidate)) => {
            candidate == root
                || candidate
                    .strip_prefix(&root)
                    .is_some_and(|rest| root.ends_with('/') || rest.starts_with('/'))
        }
        _ => false,
    }
}

/// Resolves the project-relative part of a Windows URI path.
#[cfg(windows)]
fn windows_project_path(root: &Path, absolute: &Path) -> Result<ProjectPath, UriError> {
    let root = normalized_windows_text(root).ok_or(UriError::OutsideRoot)?;
    let absolute = normalized_windows_text(absolute).ok_or(UriError::InvalidUtf8)?;
    let relative = absolute
        .strip_prefix(&root)
        .filter(|rest| root.ends_with('/') || rest.starts_with('/'))
        .map(|rest| rest.trim_start_matches('/'))
        .ok_or(UriError::OutsideRoot)?;
    if relative.is_empty() {
        return Err(UriError::OutsideRoot);
    }
    ProjectPath::new(relative).map_err(UriError::from)
}

/// Returns whether `path` names a regular file inside `root`.
///
/// Symlinks are resolved; a target outside `root` counts as missing.
#[must_use]
pub fn is_file(root: &Path, path: &ProjectPath) -> bool {
    std::fs::canonicalize(absolute_path(root, path))
        .is_ok_and(|canonical| path_within(root, &canonical) && canonical.is_file())
}

/// Returns whether `path` names a directory inside `root`.
///
/// Symlinks are resolved; a target outside `root` counts as missing.
#[must_use]
pub fn is_dir(root: &Path, path: &ProjectPath) -> bool {
    std::fs::canonicalize(absolute_path(root, path))
        .is_ok_and(|canonical| path_within(root, &canonical) && canonical.is_dir())
}

/// Returns whether `path` resolves outside `root`.
///
/// Used for resource checks: a symlink pointing outside the project must not
/// become a clickable target even though the lexical path looks local. An
/// absent path is not an escape, but an absent path below a symlinked parent
/// that leaves the root is.
#[must_use]
pub(crate) fn escapes_root(root: &Path, path: &ProjectPath) -> bool {
    let absolute = absolute_path(root, path);
    std::fs::canonicalize(&absolute).map_or_else(
        |_| parent_escapes_root(root, &absolute),
        |canonical| !path_within(root, &canonical),
    )
}

/// Joins a project root and a project path for file-system access.
///
/// The result is absolute when `root` is absolute. Do not publish it to
/// clients; use [`uri_for_project_path`] or [`ProjectPath::as_str`] instead.
#[must_use]
pub fn absolute_path(root: &Path, path: &ProjectPath) -> PathBuf {
    root.join(path.as_str())
}

/// Returns `true` for POSIX, drive-letter and UNC paths.
fn is_absolute(path: &str) -> bool {
    if path.starts_with('/') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Returns `true` for bidirectional-embedding/override and zero-width format
/// characters that can disguise paths in logs or terminals.
const fn is_forbidden_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Decodes percent escapes into UTF-8.
fn percent_decode(input: &str) -> Result<String, UriError> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes
                .get(index + 1)
                .copied()
                .ok_or(UriError::InvalidPercentEscape)?;
            let low = bytes
                .get(index + 2)
                .copied()
                .ok_or(UriError::InvalidPercentEscape)?;
            let high = hex_value(high).ok_or(UriError::InvalidPercentEscape)?;
            let low = hex_value(low).ok_or(UriError::InvalidPercentEscape)?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| UriError::InvalidUtf8)
}

/// Returns the numeric value of one ASCII hex digit.
const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encodes all bytes except URI path-safe characters.
fn percent_encode(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                output.push(char::from(byte));
            }
            other => {
                output.push('%');
                output.push(char::from(HEX[usize::from(other >> 4)]));
                output.push(char::from(HEX[usize::from(other & 0x0f)]));
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// Base for temporary directories: `TMPDIR` when set, otherwise `/tmp/opencode`.
    fn temp_base() -> PathBuf {
        std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from)
    }

    struct TempDir {
        /// Owns creation and recursive cleanup.
        _dir: tempfile::TempDir,
        /// Directory handle used by the existing helpers.
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = tempfile::Builder::new()
                .prefix(name)
                .tempdir_in(temp_base())
                .expect("create temp dir");
            let path = dir.path().to_path_buf();
            Self { _dir: dir, path }
        }
    }

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    #[test]
    fn canonicalizes_separators_leading_dot_and_dot_components() {
        assert_eq!(
            path("./pages\\index\\index.wxml").as_str(),
            "pages/index/index.wxml"
        );
        assert_eq!(
            ProjectPath::new("./a//b")
                .expect_err("empty component")
                .to_string(),
            "path must not contain empty components"
        );
    }

    #[test]
    fn rejects_empty_absolute_parent_and_nul() {
        assert_eq!(ProjectPath::new(""), Err(PathError::Empty));
        assert_eq!(ProjectPath::new("/etc/passwd"), Err(PathError::Absolute));
        assert_eq!(ProjectPath::new("C:/x"), Err(PathError::Absolute));
        assert_eq!(ProjectPath::new("a/../b"), Err(PathError::ParentComponent));
        assert_eq!(ProjectPath::new("a/./b"), Err(PathError::CurrentComponent));
        assert_eq!(ProjectPath::new("a\0b"), Err(PathError::Nul));
        assert_eq!(ProjectPath::new("a/"), Err(PathError::EmptyComponent));
        assert_eq!(ProjectPath::new("a\nb"), Err(PathError::ControlCharacter));
        assert_eq!(ProjectPath::new("a\rb"), Err(PathError::ControlCharacter));
        assert_eq!(
            ProjectPath::new("a\u{1b}b"),
            Err(PathError::ControlCharacter)
        );
        assert_eq!(
            ProjectPath::new("a\u{7f}b"),
            Err(PathError::ControlCharacter)
        );
        for character in [
            '\u{200B}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202E}', '\u{2066}',
            '\u{2069}',
        ] {
            assert_eq!(
                ProjectPath::new(format!("a{character}b")),
                Err(PathError::FormatCharacter),
                "U+{:04X} must be rejected",
                u32::from(character)
            );
        }
    }

    #[test]
    fn exposes_name_extension_and_parent() {
        let p = path("pages/index/index.wxml");
        assert_eq!(p.file_name(), "index.wxml");
        assert_eq!(p.extension(), Some("wxml"));
        assert_eq!(p.parent().expect("parent").as_str(), "pages/index");
        assert_eq!(path("index.wxml").parent(), None);
        assert_eq!(path("index").extension(), None);
    }

    #[test]
    fn replaces_or_appends_extensions() {
        assert_eq!(
            path("pages/index/index.wxml")
                .with_extension("json")
                .expect("valid")
                .as_str(),
            "pages/index/index.json"
        );
        assert_eq!(
            path("pages/index/index")
                .with_extension("ts")
                .expect("valid")
                .as_str(),
            "pages/index/index.ts"
        );
        assert_eq!(
            path("components/card.v2")
                .with_appended_extension("wxml")
                .expect("valid")
                .as_str(),
            "components/card.v2.wxml"
        );
        assert_eq!(
            path("a").with_extension("a/b"),
            Err(PathError::InvalidExtension)
        );
        assert_eq!(
            path("a").with_extension(""),
            Err(PathError::InvalidExtension)
        );
    }

    #[test]
    fn resolves_relative_root_relative_and_dot_segments() {
        let base = path("pages/index");
        assert_eq!(
            ProjectPath::resolve(Some(&base), "../shared/x").expect("resolved"),
            path("pages/shared/x")
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "/components/card/card").expect("resolved"),
            path("components/card/card")
        );
        assert_eq!(
            ProjectPath::resolve(None, "components/card/card").expect("resolved"),
            path("components/card/card")
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "./child").expect("resolved"),
            path("pages/index/child")
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "../../../etc/passwd"),
            Err(PathError::EscapesRoot)
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "C:/x"),
            Err(PathError::Absolute)
        );
        assert_eq!(ProjectPath::resolve(None, ""), Err(PathError::Empty));
        assert_eq!(ProjectPath::resolve(None, "a\0b"), Err(PathError::Nul));
        assert_eq!(
            ProjectPath::resolve(None, "//server/share"),
            Err(PathError::Absolute)
        );
        assert_eq!(
            ProjectPath::resolve(None, "/C:/x"),
            Err(PathError::Absolute)
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "/C:/x"),
            Err(PathError::Absolute)
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "/pages/a").expect("root relative"),
            path("pages/a")
        );
        assert_eq!(
            ProjectPath::resolve(Some(&base), "a\u{1}b"),
            Err(PathError::ControlCharacter)
        );
    }

    #[test]
    fn file_key_round_trips() {
        let p = path("pages/index/index.wxml");
        let key = FileKey::from(&p);
        assert_eq!(key.as_str(), "pages/index/index.wxml");
        assert_eq!(key, FileKey::from(&p));
        assert_eq!(key.to_string(), "pages/index/index.wxml");
    }

    #[test]
    fn uri_decodes_percent_escapes_and_strips_root() {
        let temp = TempDir::new("uri");
        let root = temp.path.canonicalize().expect("canonical root");
        let uri = format!(
            "file://{}/pages/index/index.wxml",
            root.to_str().expect("utf8 root")
        );
        assert_eq!(
            project_path_from_uri(&root, &uri).expect("decoded"),
            path("pages/index/index.wxml")
        );

        let encoded = format!(
            "file://{}/pages/index/my%20page%2Ewxml",
            root.to_str().expect("utf8 root")
        );
        assert_eq!(
            project_path_from_uri(&root, &encoded).expect("decoded"),
            path("pages/index/my page.wxml")
        );

        let localhost = format!(
            "file://localhost{}/a.wxml",
            root.to_str().expect("utf8 root")
        );
        assert_eq!(
            project_path_from_uri(&root, &localhost).expect("localhost"),
            path("a.wxml")
        );
    }

    #[test]
    fn uri_rejects_bad_inputs() {
        let temp = TempDir::new("uri-errors");
        let root = temp.path.canonicalize().expect("canonical root");
        let root_str = root.to_str().expect("utf8 root");

        assert_eq!(
            project_path_from_uri(&root, "https://example.com/a.wxml"),
            Err(UriError::NotFileScheme)
        );
        assert_eq!(
            project_path_from_uri(&root, "file://example.com/a.wxml"),
            Err(UriError::RemoteHost)
        );
        assert_eq!(
            project_path_from_uri(&root, &format!("file://{root_str}/a%2.wxml")),
            Err(UriError::InvalidPercentEscape)
        );
        assert_eq!(
            project_path_from_uri(&root, &format!("file://{root_str}/../etc/passwd")),
            Err(UriError::ParentComponent)
        );
        assert_eq!(
            project_path_from_uri(&root, &format!("file://{root_str}/a.wxml?x=1")),
            Err(UriError::UnsupportedComponent)
        );
        assert_eq!(
            project_path_from_uri(&root, &format!("file://{root_str}/a.wxml#frag")),
            Err(UriError::UnsupportedComponent)
        );
        assert_eq!(
            project_path_from_uri(&root, "file:relative.wxml"),
            Err(UriError::RelativePath)
        );
        assert_eq!(
            project_path_from_uri(&root, &format!("file://{root_str}/a%00b")),
            Err(UriError::Nul)
        );
        assert_eq!(
            project_path_from_uri(&root, "file:///etc/passwd"),
            Err(UriError::OutsideRoot)
        );
    }

    #[test]
    fn uri_round_trips_paths_with_special_characters() {
        let temp = TempDir::new("uri-round-trip");
        let root = temp.path.canonicalize().expect("canonical root");
        let original = path("pages/weird name/a#b?c%d.wxml");
        let uri = uri_for_project_path(&root, &original).expect("uri");
        assert!(uri.contains("%20"), "space must be encoded: {uri}");
        assert!(uri.contains("%23"), "# must be encoded: {uri}");
        assert!(uri.contains("%3F"), "? must be encoded: {uri}");
        assert!(uri.contains("%25"), "% must be encoded: {uri}");
        assert_eq!(
            project_path_from_uri(&root, &uri).expect("round trip"),
            original
        );
    }

    #[test]
    fn reads_files_and_reports_missing_files() {
        let temp = TempDir::new("read");
        let root = temp.path.canonicalize().expect("canonical root");
        let dir = root.join("pages");
        fs::create_dir_all(&dir).expect("create pages");
        fs::write(dir.join("index.wxml"), "<view/>").expect("write");

        assert_eq!(
            read_file(&root, &path("pages/index.wxml")).expect("read"),
            "<view/>"
        );
        assert_eq!(
            read_file(&root, &path("pages/missing.wxml")),
            Err(FileError::NotFound {
                path: path("pages/missing.wxml")
            })
        );
        assert!(is_file(&root, &path("pages/index.wxml")));
        assert!(!is_file(&root, &path("pages/missing.wxml")));
        assert!(!is_file(&root, &path("pages")));
    }

    #[test]
    fn rejects_files_above_the_size_limit() {
        let temp = TempDir::new("too-large");
        let root = temp.path.canonicalize().expect("canonical root");
        fs::create_dir_all(root.join("pages")).expect("create pages");
        let large = root.join("pages/large.ts");
        let file = File::create(&large).expect("create large");
        file.set_len(MAX_FILE_BYTES + 1).expect("set length");

        assert_eq!(
            read_file(&root, &path("pages/large.ts")),
            Err(FileError::TooLarge {
                path: path("pages/large.ts"),
                size: MAX_FILE_BYTES + 1
            })
        );
    }

    #[test]
    fn truncates_messages_on_utf8_boundaries() {
        assert_eq!(truncate_message("abcdef", 3), "abc");
        assert_eq!(truncate_message("abc", 10), "abc");
        assert_eq!(truncate_message("中文", 4), "中");
        assert_eq!(truncate_message("中文", 5), "中");
        assert_eq!(truncate_message("中文", 6), "中文");
        assert_eq!(truncate_message("中文", 0), "");
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_windows_verbatim_paths() {
        let verbatim = Path::new(r"\\?\C:\Proj\Pages\A.wxml");
        assert_eq!(
            normalized_windows_text(verbatim).as_deref(),
            Some("C:/Proj/Pages/A.wxml")
        );
        assert!(windows_within(Path::new(r"C:\Proj"), verbatim));
        assert!(!windows_within(
            Path::new(r"C:\Proj"),
            Path::new(r"C:\Project\a")
        ));
        assert_eq!(
            normalized_windows_text(Path::new(r"\\server\share\a")).as_deref(),
            Some("//server/share/a")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_root_requires_a_separator() {
        assert!(root_path_from_uri("file:///C:relative").is_err());
        assert!(root_path_from_uri("file:///C:").is_err());
        assert!(root_path_from_uri("file:///C:/tmp").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_uris_round_trip() {
        let temp = TempDir::new("windows-uri");
        let root = temp.path.canonicalize().expect("canonical root");
        let page = path("pages/index/index.wxml");
        let uri = uri_for_project_path(&root, &page).expect("drive-letter uri");
        assert!(uri.starts_with("file:///"), "unexpected URI: {uri}");
        assert_eq!(
            project_path_from_uri(&root, &uri).expect("round trip"),
            page
        );
        assert!(
            project_path_from_uri(&root, "file://server/share/x.wxml").is_err(),
            "UNC input must not map into a drive-rooted project"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new("symlink");
        let base = temp.path.canonicalize().expect("canonical base");
        let root = base.join("project");
        let outside = base.join("outside");
        fs::create_dir_all(&root).expect("create root");
        fs::create_dir_all(&outside).expect("create outside");
        fs::write(outside.join("secret.wxml"), "secret").expect("write secret");
        symlink(&outside, root.join("link")).expect("symlink dir");
        symlink(outside.join("secret.wxml"), root.join("secret-link.wxml")).expect("symlink file");

        assert_eq!(
            read_file(&root, &path("link/secret.wxml")),
            Err(FileError::EscapesRoot {
                path: path("link/secret.wxml")
            })
        );
        assert_eq!(
            read_file(&root, &path("secret-link.wxml")),
            Err(FileError::EscapesRoot {
                path: path("secret-link.wxml")
            })
        );
        assert!(!is_file(&root, &path("link/secret.wxml")));

        assert_eq!(
            read_file(&root, &path("link/missing.wxml")),
            Err(FileError::EscapesRoot {
                path: path("link/missing.wxml")
            })
        );
        assert_eq!(
            read_file(&root, &path("missing-dir/missing.wxml")),
            Err(FileError::NotFound {
                path: path("missing-dir/missing.wxml")
            })
        );

        let uri = uri_for_project_path(&root, &path("link/secret.wxml")).expect("uri");
        assert_eq!(
            project_path_from_uri(&root, &uri),
            Err(UriError::OutsideRoot)
        );
        let missing_uri = uri_for_project_path(&root, &path("link/missing.wxml")).expect("uri");
        assert_eq!(
            project_path_from_uri(&root, &missing_uri),
            Err(UriError::OutsideRoot)
        );
    }
}
