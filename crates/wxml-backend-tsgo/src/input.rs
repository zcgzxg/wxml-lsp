//! Immutable analysis input, revisions and the host provider seam.
//!
//! `P3-I` owns these DTOs; `wxml-lsp` will implement [`InputProvider`] over its
//! project host in a later package. Nothing here reads the file system, writes a
//! business file or mutates host state: [`AnalysisInput`] is a bounded,
//! immutable snapshot of text that has already been read by the host.
//!
//! # Path rules
//!
//! A [`ProjectRoot`] is a normalized absolute Unix path. A [`SourceFile`] path
//! is project-relative, `/`-separated and contains no empty, `.` or `..`
//! components. Both reject NUL/control characters and backslashes. Duplicate
//! file paths are rejected instead of being silently merged. Paths are stored
//! verbatim after validation; this module never canonicalizes or touches the
//! file system.
//!
//! # Language and text
//!
//! [`Language`] is derived from the extension only. The source text is kept
//! byte-for-byte, including a BOM, CRLF and a hashbang line; the shadow builder
//! is responsible for handling those bytes, not this snapshot.

use std::collections::BTreeSet;

use thiserror::Error;

/// Maximum number of files accepted in one [`AnalysisInput`].
pub const MAX_INPUT_FILES: usize = 20_000;

/// Maximum combined UTF-8 source bytes accepted in one [`AnalysisInput`].
///
/// The frozen native request budget is 32 MiB. JSON escaping can expand a
/// control character to six bytes, so an accepted 48 MiB input cannot always
/// be sent in one snapshot request; `NativeSession` preflights a conservative
/// serialized-size bound and returns a typed `InputTooLarge` error before
/// allocating the body.
pub const MAX_INPUT_BYTES: usize = 48 * 1024 * 1024;

/// Stable key of one discovered mini-program project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectKey(String);

impl ProjectKey {
    /// Creates a project key from a host-provided identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the host identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Normalized absolute root directory of one project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectRoot(String);

impl ProjectRoot {
    /// Validates a host-supplied absolute Unix path.
    ///
    /// The path must start with `/`, must not be the file-system root itself,
    /// must not end with `/` and must not contain empty, `.` or `..`
    /// components, a backslash, a NUL or any other control character. No
    /// canonicalization or symlink resolution is performed.
    pub fn new(path: impl Into<String>) -> Result<Self, InputError> {
        let path = path.into();
        validate_root(&path)?;
        Ok(Self(path))
    }

    /// Returns the path exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Script language inferred from the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// TypeScript (`.ts`, `.mts`, `.cts`, including `.d.ts`).
    ///
    /// `.tsx` is deliberately unsupported until JSX lexing exists.
    TypeScript,
    /// JavaScript (`.js`, `.mjs`, `.cjs`).
    ///
    /// `.jsx` is deliberately unsupported until JSX lexing exists.
    JavaScript,
}

impl Language {
    /// Infers the language from a project-relative path.
    ///
    /// Extensions are matched case-sensitively. Unknown extensions return
    /// `None`; the caller reports [`InputError::UnsupportedLanguage`]. `.tsx`
    /// and `.jsx` are intentionally rejected: the shadow expression lexer does
    /// not implement JSX and must not silently mislex apostrophes/comments.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = path.rsplit_once('.')?.1;
        match extension {
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "js" | "mjs" | "cjs" => Some(Self::JavaScript),
            _ => None,
        }
    }

    /// Returns whether the language is JavaScript.
    #[must_use]
    pub const fn is_javascript(self) -> bool {
        matches!(self, Self::JavaScript)
    }
}

/// Monotonic version of one open document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRevision(u64);

impl DocumentRevision {
    /// Creates a revision from the host's version counter.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Revision of one complete input snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnapshotRevision(u64);

impl SnapshotRevision {
    /// Creates a snapshot revision from the host sequence.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One immutable file snapshot supplied by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    path: String,
    language: Language,
    text: String,
    revision: DocumentRevision,
}

impl SourceFile {
    /// Creates a validated source snapshot.
    ///
    /// `path` must be non-empty, project-relative, `/`-separated and free of
    /// empty, `.`/`..` components, backslashes and control characters. Its
    /// extension must be a TypeScript or JavaScript extension; see
    /// [`Language::from_path`].
    pub fn new(
        path: impl Into<String>,
        text: impl Into<String>,
        revision: DocumentRevision,
    ) -> Result<Self, InputError> {
        let path = path.into();
        validate_file_path(&path)?;
        let language = Language::from_path(&path)
            .ok_or_else(|| InputError::UnsupportedLanguage { path: path.clone() })?;
        Ok(Self {
            path,
            language,
            text: text.into(),
            revision,
        })
    }

    /// Returns the validated path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the language inferred from [`Self::path`].
    #[must_use]
    pub const fn language(&self) -> Language {
        self.language
    }

    /// Returns the exact source text, including any BOM, CRLF or hashbang.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the document revision the text belongs to.
    #[must_use]
    pub const fn revision(&self) -> DocumentRevision {
        self.revision
    }
}

/// Immutable input for one project revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisInput {
    project: ProjectKey,
    root: ProjectRoot,
    revision: SnapshotRevision,
    files: Vec<SourceFile>,
}

impl AnalysisInput {
    /// Builds a bounded, validated input snapshot.
    ///
    /// File count, combined size and duplicate paths are rejected here. Path
    /// and language validation happen in [`SourceFile::new`], so passing
    /// already-constructed files cannot bypass them.
    pub fn new(
        project: ProjectKey,
        root: ProjectRoot,
        revision: SnapshotRevision,
        files: Vec<SourceFile>,
    ) -> Result<Self, InputError> {
        if files.len() > MAX_INPUT_FILES {
            return Err(InputError::TooManyFiles {
                count: files.len(),
                max: MAX_INPUT_FILES,
            });
        }

        let bytes: usize = files.iter().map(|file| file.text.len()).sum();
        if bytes > MAX_INPUT_BYTES {
            return Err(InputError::TooLarge {
                bytes,
                max: MAX_INPUT_BYTES,
            });
        }

        let mut seen = BTreeSet::new();
        for file in &files {
            if !seen.insert(file.path()) {
                return Err(InputError::DuplicatePath {
                    path: file.path().to_owned(),
                });
            }
        }

        Ok(Self {
            project,
            root,
            revision,
            files,
        })
    }

    /// Returns the project this input belongs to.
    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }

    /// Returns the validated project root.
    #[must_use]
    pub const fn root(&self) -> &ProjectRoot {
        &self.root
    }

    /// Returns the snapshot revision.
    #[must_use]
    pub const fn revision(&self) -> SnapshotRevision {
        self.revision
    }

    /// Returns the file snapshots in host order.
    #[must_use]
    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    /// Looks a file up by its exact project-relative path.
    #[must_use]
    pub fn file(&self, path: &str) -> Option<&SourceFile> {
        self.files.iter().find(|file| file.path() == path)
    }
}

/// Supplies immutable inputs for a project without owning native state.
///
/// Implementations must be side-effect free with respect to the host while
/// the call is in flight and must not read outside the project boundary.
pub trait InputProvider: Send + Sync {
    /// Loads the current bounded input for `project`.
    fn load(&self, project: &ProjectKey) -> Result<AnalysisInput, InputError>;
}

/// Failures raised while collecting or validating an [`AnalysisInput`].
#[derive(Debug, Error)]
pub enum InputError {
    /// The host has no project with this key.
    #[error("unknown project: {project}")]
    UnknownProject {
        /// Host identifier that could not be resolved.
        project: String,
    },
    /// The file count exceeds the input budget.
    #[error("input has {count} files, limit is {max}")]
    TooManyFiles {
        /// Observed file count.
        count: usize,
        /// Configured limit.
        max: usize,
    },
    /// The combined source size exceeds the input budget.
    #[error("input has {bytes} bytes, limit is {max}")]
    TooLarge {
        /// Observed byte count.
        bytes: usize,
        /// Configured limit.
        max: usize,
    },
    /// The host cannot currently serve the project.
    #[error("input host unavailable: {reason}")]
    HostUnavailable {
        /// Sanitized reason supplied by the adapter.
        reason: String,
    },
    /// The project root is not a normalized absolute path.
    #[error("invalid project root: {reason}")]
    InvalidProjectRoot {
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// A file path is not a valid project-relative path.
    #[error("invalid input path `{path}`: {reason}")]
    InvalidPath {
        /// Rejected path.
        path: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// Two files use the same project-relative path.
    #[error("duplicate input path: {path}")]
    DuplicatePath {
        /// Duplicated path.
        path: String,
    },
    /// The extension is not a TypeScript or JavaScript script extension.
    #[error("unsupported input language: {path}")]
    UnsupportedLanguage {
        /// Rejected path.
        path: String,
    },
}

fn validate_root(path: &str) -> Result<(), InputError> {
    let reject = |reason| Err(InputError::InvalidProjectRoot { reason });

    if path.is_empty() {
        return reject("empty");
    }
    if !path.starts_with('/') {
        return reject("not absolute");
    }
    if path == "/" {
        return reject("file-system root is not a project root");
    }
    if path.ends_with('/') {
        return reject("trailing separator");
    }
    if path.contains('\\') {
        return reject("backslash separator");
    }
    if path.contains("//") {
        return reject("empty component");
    }
    if path.chars().any(char::is_control) {
        return reject("control character");
    }
    if path
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return reject("dot component");
    }
    Ok(())
}

fn validate_file_path(path: &str) -> Result<(), InputError> {
    let reject = |reason| {
        Err(InputError::InvalidPath {
            path: path.to_owned(),
            reason,
        })
    };

    if path.is_empty() {
        return reject("empty");
    }
    if path.starts_with('/') {
        return reject("absolute");
    }
    if path.ends_with('/') {
        return reject("trailing separator");
    }
    if path.contains('\\') {
        return reject("backslash separator");
    }
    if path.contains("//") {
        return reject("empty component");
    }
    if path.chars().any(char::is_control) {
        return reject("control character");
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return reject("invalid component");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> SourceFile {
        SourceFile::new(path, text, DocumentRevision::new(1)).expect("valid fixture")
    }

    fn input(files: Vec<SourceFile>) -> Result<AnalysisInput, InputError> {
        AnalysisInput::new(
            ProjectKey::new("demo"),
            ProjectRoot::new("/work/demo").expect("valid root"),
            SnapshotRevision::new(7),
            files,
        )
    }

    #[test]
    fn accepts_normalized_project_paths_and_keeps_text_bytes() {
        let result = input(vec![file("src/index.ts", "\u{FEFF}const x = 1;\r\n")]).expect("valid");
        let script = result.file("src/index.ts").expect("file");
        assert_eq!(script.language(), Language::TypeScript);
        assert!(script.text().starts_with('\u{FEFF}'));
        assert!(script.text().ends_with("\r\n"));
    }

    #[test]
    fn rejects_invalid_and_duplicate_paths() {
        for path in [
            "",
            "/abs.ts",
            "../escape.ts",
            "a//b.ts",
            "a/./b.ts",
            "a/../b.ts",
            "a\\b.ts",
            "a\u{0}b.ts",
            "a/",
        ] {
            assert!(
                SourceFile::new(path, "", DocumentRevision::new(1)).is_err(),
                "{path:?} must be rejected"
            );
        }

        let duplicate = input(vec![file("a.ts", ""), file("a.ts", "x")]);
        assert!(matches!(duplicate, Err(InputError::DuplicatePath { .. })));
    }

    #[test]
    fn rejects_non_script_extensions_and_invalid_roots() {
        for path in ["index.wxml", "index.tsx", "index.jsx"] {
            assert!(
                matches!(
                    SourceFile::new(path, "", DocumentRevision::new(1)),
                    Err(InputError::UnsupportedLanguage { .. })
                ),
                "{path} must be rejected until JSX lexing exists"
            );
        }
        for root in [
            "",
            "work",
            "/",
            "/work/",
            "/work//x",
            "/work/../x",
            "/work\\x",
        ] {
            assert!(ProjectRoot::new(root).is_err(), "{root:?} must be rejected");
        }
    }

    #[test]
    fn javascript_is_detected_from_the_extension() {
        let script = file("index.js", "Page({});");
        assert_eq!(script.language(), Language::JavaScript);
        assert!(script.language().is_javascript());
    }

    #[test]
    fn missing_file_lookup_is_none() {
        let result = input(vec![file("a.ts", "")]).expect("valid");
        assert!(result.file("b.ts").is_none());
    }
}
