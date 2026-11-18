//! Immutable analysis input, revisions and the host provider seam.
//!
//! `P3-I` owns these DTOs; `wxml-lsp` will implement [`InputProvider`] over its
//! project host in a later package. Nothing here reads the file system or
//! mutates host state.

use thiserror::Error;

/// Maximum number of files accepted in one [`AnalysisInput`].
pub const MAX_INPUT_FILES: usize = 20_000;

/// Maximum combined UTF-8 source bytes accepted in one [`AnalysisInput`].
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
    text: String,
    revision: DocumentRevision,
}

impl SourceFile {
    /// Creates a source snapshot.
    ///
    /// `path` is stored verbatim; it must already be a non-empty,
    /// project-relative, `/`-separated path with no `.`/`..` components and no
    /// NUL/control characters. The host adapter owns that normalization and
    /// the project boundary; this scaffold does not validate the path.
    #[must_use]
    pub fn new(
        path: impl Into<String>,
        text: impl Into<String>,
        revision: DocumentRevision,
    ) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
            revision,
        }
    }

    /// Returns the path exactly as supplied.
    ///
    /// See [`Self::new`] for the required format.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the exact source text, including any BOM or CRLF bytes.
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
    revision: SnapshotRevision,
    files: Vec<SourceFile>,
}

impl AnalysisInput {
    /// Builds a bounded input snapshot.
    ///
    /// Only size budgets are enforced here. Each [`SourceFile`] path must
    /// already satisfy the format documented on [`SourceFile::new`]; the
    /// provider owns membership and normalization.
    pub fn new(
        project: ProjectKey,
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

        Ok(Self {
            project,
            revision,
            files,
        })
    }

    /// Returns the project this input belongs to.
    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
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
}

/// Supplies immutable inputs for a project without owning native state.
///
/// Implementations must be side-effect free with respect to the host while
/// the call is in flight and must not read outside the project boundary.
pub trait InputProvider: Send + Sync {
    /// Loads the current bounded input for `project`.
    fn load(&self, project: &ProjectKey) -> Result<AnalysisInput, InputError>;
}

/// Failures raised while collecting an [`AnalysisInput`].
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
}
