//! Stable C0 identities: projects, backends, files and revision counters.
//!
//! These types are frozen for the first implementation wave. They carry no
//! project or analysis logic; they only make identities explicit so caches,
//! requests and acknowledgements cannot mix projects, files or versions up.

use std::fmt;

use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

/// An opaque, stable identifier for one project root.
///
/// The value is supplied by the project layer and compared verbatim; it is
/// never derived from a path inside this crate. An empty id is not meaningful
/// and callers must not construct one.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    /// Creates a project id from a stable, non-empty identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the raw identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An opaque, stable identifier for one backend instance.
///
/// An empty id is not meaningful and callers must not construct one.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendId(String);

impl BackendId {
    /// Creates a backend id from a stable, non-empty identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the raw identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BackendId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A canonical, project-relative file identity.
///
/// Canonical rules:
///
/// - relative to the project root, never absolute;
/// - `/` separators, with `\` normalized on construction;
/// - no leading `./`, no `.` or `..` components, no empty components;
/// - case-sensitive, with the original case preserved;
/// - no NUL bytes.
///
/// The path is validated when the id is created and when it is deserialized,
/// so every existing `FileId` satisfies the rules. Equality includes the
/// project, so equal paths in different projects are different files.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize)]
#[serde(try_from = "FileIdWire")]
pub struct FileId {
    project: ProjectId,
    path: String,
    /// Canonical `project:path` key, derived from the two fields above.
    id: String,
}

impl FileId {
    /// Creates a file id after normalizing and validating `path`.
    ///
    /// # Errors
    ///
    /// Returns [`FileIdError`] for empty, absolute, non-canonical or NUL
    /// containing paths.
    pub fn new(project: ProjectId, path: impl Into<String>) -> Result<Self, FileIdError> {
        let raw = path.into();
        if raw.is_empty() {
            return Err(FileIdError::Empty);
        }
        if raw.contains('\0') {
            return Err(FileIdError::Nul);
        }
        let mut normalized = raw.replace('\\', "/");
        while let Some(rest) = normalized.strip_prefix("./") {
            normalized = rest.to_owned();
        }
        if normalized.is_empty() {
            return Err(FileIdError::Empty);
        }
        if is_absolute(&normalized) {
            return Err(FileIdError::Absolute);
        }
        for component in normalized.split('/') {
            if component.is_empty() {
                return Err(FileIdError::EmptyComponent);
            }
            if component == "." {
                return Err(FileIdError::CurrentComponent);
            }
            if component == ".." {
                return Err(FileIdError::ParentComponent);
            }
        }
        let id = format!("{}:{}", project.as_str(), normalized);
        Ok(Self {
            project,
            path: normalized,
            id,
        })
    }

    /// Returns the owning project.
    #[must_use]
    pub const fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Returns the canonical project-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the unambiguous `project:path` key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.id)
    }
}

impl Serialize for FileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("FileId", 2)?;
        state.serialize_field("project", &self.project)?;
        state.serialize_field("path", &self.path)?;
        state.end()
    }
}

#[derive(Deserialize)]
struct FileIdWire {
    project: ProjectId,
    path: String,
}

impl TryFrom<FileIdWire> for FileId {
    type Error = FileIdError;

    fn try_from(wire: FileIdWire) -> Result<Self, Self::Error> {
        Self::new(wire.project, wire.path)
    }
}

/// Why a [`FileId`] could not be created.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum FileIdError {
    /// The path or the id was empty after normalization.
    #[error("file path must not be empty")]
    Empty,
    /// The path contains a NUL byte.
    #[error("file path must not contain NUL")]
    Nul,
    /// The path is absolute (`/`, a drive letter or a UNC path).
    #[error("file path must be project-relative, not absolute")]
    Absolute,
    /// The path produced an empty component (`//` or a trailing `/`).
    #[error("file path must not contain empty components")]
    EmptyComponent,
    /// The path contains a `.` component.
    #[error("file path must not contain `.` components")]
    CurrentComponent,
    /// The path contains a `..` component.
    #[error("file path must not contain `..` components")]
    ParentComponent,
}

/// Returns `true` for POSIX, drive-letter and UNC paths.
fn is_absolute(path: &str) -> bool {
    if path.starts_with('/') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

macro_rules! revision_counter {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Creates a value from a raw revision counter.
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// Returns the raw revision counter.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }

            /// Returns the next revision, saturating at `u64::MAX`.
            ///
            /// Once the value saturates, the coordinator must stop accepting
            /// new edits rather than reuse the maximum value.
            #[must_use]
            pub const fn next(self) -> Self {
                Self(self.0.saturating_add(1))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, formatter)
            }
        }
    };
}

revision_counter!(
    /// Content revision of one document, incremented on every accepted change.
    SourceRevision
);
revision_counter!(
    /// Identity of one immutable analysis input set.
    SnapshotId
);
revision_counter!(
    /// Revision of one view, including its generated text and mappings.
    ViewRevision
);
revision_counter!(
    /// The client-supplied `didChange` version of one document.
    ///
    /// Versions are monotonically increasing; the coordinator rejects older or
    /// duplicate versions instead of applying them again.
    ClientVersion
);

/// Revision of one project's discovery, configuration and dependency state.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ProjectRevision {
    project: ProjectId,
    value: u64,
}

impl ProjectRevision {
    /// Creates a project revision for `project`.
    #[must_use]
    pub const fn new(project: ProjectId, value: u64) -> Self {
        Self { project, value }
    }

    /// Returns the project this revision belongs to.
    #[must_use]
    pub const fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Returns the raw counter value.
    #[must_use]
    pub const fn get(&self) -> u64 {
        self.value
    }

    /// Returns the next revision for the same project.
    ///
    /// The value saturates at `u64::MAX`; callers must stop accepting project
    /// changes once that value is reached.
    #[must_use]
    pub fn next(self) -> Self {
        Self {
            project: self.project,
            value: self.value.saturating_add(1),
        }
    }
}

impl fmt::Display for ProjectRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.project, self.value)
    }
}

/// Generation of one backend process or fully rebuilt backend state.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct BackendEpoch {
    backend: BackendId,
    value: u64,
}

impl BackendEpoch {
    /// Creates an epoch for `backend`.
    #[must_use]
    pub const fn new(backend: BackendId, value: u64) -> Self {
        Self { backend, value }
    }

    /// Returns the backend instance this epoch belongs to.
    #[must_use]
    pub const fn backend(&self) -> &BackendId {
        &self.backend
    }

    /// Returns the raw counter value.
    #[must_use]
    pub const fn get(&self) -> u64 {
        self.value
    }

    /// Returns the next epoch for the same backend.
    ///
    /// The value saturates at `u64::MAX`; a backend must be recreated instead
    /// of reusing the maximum epoch.
    #[must_use]
    pub fn next(self) -> Self {
        Self {
            backend: self.backend,
            value: self.value.saturating_add(1),
        }
    }
}

impl fmt::Display for BackendEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.backend, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectId {
        ProjectId::new("example")
    }

    #[test]
    fn project_and_backend_ids_display_verbatim() {
        let project = ProjectId::new("example");
        assert_eq!(project.as_str(), "example");
        assert_eq!(project.to_string(), "example");
        let backend = BackendId::new("node-1");
        assert_eq!(backend.as_str(), "node-1");
        assert_eq!(backend.to_string(), "node-1");
    }

    #[test]
    fn file_id_normalizes_separators_and_leading_dot_slash() {
        let file = FileId::new(project(), ".\\pages\\index\\index.wxml").expect("valid path");
        assert_eq!(file.path(), "pages/index/index.wxml");
        assert_eq!(file.project(), &project());
        assert_eq!(file.as_str(), "example:pages/index/index.wxml");
        assert_eq!(file.to_string(), "example:pages/index/index.wxml");
    }

    #[test]
    fn file_id_preserves_case() {
        let file = FileId::new(project(), "Pages/Index.WXML").expect("valid path");
        assert_eq!(file.path(), "Pages/Index.WXML");
    }

    #[test]
    fn file_id_rejects_invalid_paths() {
        for (path, expected) in [
            ("", FileIdError::Empty),
            ("./", FileIdError::Empty),
            ("/pages/index.wxml", FileIdError::Absolute),
            ("C:/pages/index.wxml", FileIdError::Absolute),
            ("C:\\pages\\index.wxml", FileIdError::Absolute),
            ("pages//index.wxml", FileIdError::EmptyComponent),
            ("pages/index.wxml/", FileIdError::EmptyComponent),
            ("pages/./index.wxml", FileIdError::CurrentComponent),
            ("pages/../index.wxml", FileIdError::ParentComponent),
            ("pages/index\0.wxml", FileIdError::Nul),
        ] {
            assert_eq!(
                FileId::new(project(), path).expect_err("invalid path"),
                expected,
                "path {path:?}"
            );
        }
    }

    #[test]
    fn file_id_equality_includes_the_project() {
        let first = FileId::new(ProjectId::new("a"), "index.wxml").expect("valid path");
        let second = FileId::new(ProjectId::new("b"), "index.wxml").expect("valid path");
        assert_ne!(first, second);
        assert_ne!(first.as_str(), second.as_str());
    }

    #[test]
    fn file_id_orders_by_project_then_path() {
        let mut ids = [
            FileId::new(ProjectId::new("b"), "a.wxml").expect("valid path"),
            FileId::new(ProjectId::new("a"), "z.wxml").expect("valid path"),
            FileId::new(ProjectId::new("a"), "a.wxml").expect("valid path"),
        ];
        ids.sort();
        assert_eq!(ids[0].path(), "a.wxml");
        assert_eq!(ids[1].path(), "z.wxml");
        assert_eq!(ids[2].project().as_str(), "b");
    }

    #[test]
    fn file_id_round_trips_through_json() {
        let file = FileId::new(project(), "components/card/card.wxml").expect("valid path");
        let json = serde_json::to_string(&file).expect("serialize");
        assert_eq!(
            json,
            r#"{"project":"example","path":"components/card/card.wxml"}"#
        );
        let back: FileId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, file);
    }

    #[test]
    fn file_id_deserialization_runs_validation() {
        for json in [
            r#"{"project":"example","path":"../escape.wxml"}"#,
            r#"{"project":"example","path":"/absolute.wxml"}"#,
            r#"{"project":"example","path":""}"#,
        ] {
            let error = serde_json::from_str::<FileId>(json).expect_err("invalid path");
            assert!(error.to_string().contains("file path"), "{error}");
        }
    }

    #[test]
    fn revisions_start_where_asked_and_advance() {
        assert_eq!(SourceRevision::new(7).get(), 7);
        assert_eq!(SnapshotId::new(8).get(), 8);
        assert_eq!(ViewRevision::new(9).get(), 9);
        assert_eq!(ClientVersion::new(10).get(), 10);
        assert_eq!(SourceRevision::new(1).next().get(), 2);
        assert_eq!(SnapshotId::new(1).next().get(), 2);
        assert_eq!(ViewRevision::new(1).next().get(), 2);
        assert_eq!(ClientVersion::new(1).next().get(), 2);
    }

    #[test]
    fn revisions_saturate_at_the_maximum() {
        assert_eq!(SourceRevision::new(u64::MAX).next().get(), u64::MAX);
        assert_eq!(SnapshotId::new(u64::MAX).next().get(), u64::MAX);
        assert_eq!(ViewRevision::new(u64::MAX).next().get(), u64::MAX);
        assert_eq!(ClientVersion::new(u64::MAX).next().get(), u64::MAX);
    }

    #[test]
    fn revisions_display_their_value() {
        assert_eq!(SourceRevision::new(42).to_string(), "42");
        assert_eq!(SnapshotId::new(42).to_string(), "42");
        assert_eq!(ViewRevision::new(42).to_string(), "42");
        assert_eq!(ClientVersion::new(42).to_string(), "42");
    }

    #[test]
    fn revisions_order_by_value_and_scope() {
        assert!(SourceRevision::new(2) < SourceRevision::new(3));
        assert!(SnapshotId::new(2) < SnapshotId::new(3));
        assert!(ViewRevision::new(2) < ViewRevision::new(3));
        assert!(ClientVersion::new(2) < ClientVersion::new(3));
        assert!(ProjectRevision::new(project(), 1) < ProjectRevision::new(project(), 2));
        assert!(
            BackendEpoch::new(BackendId::new("a"), 1) < BackendEpoch::new(BackendId::new("b"), 0)
        );
    }

    #[test]
    fn revisions_serialize_as_plain_numbers() {
        assert_eq!(
            serde_json::to_string(&ClientVersion::new(42)).expect("serialize"),
            "42"
        );
        let back: SnapshotId = serde_json::from_str("42").expect("deserialize");
        assert_eq!(back.get(), 42);
    }

    #[test]
    fn project_revision_keeps_its_project() {
        let revision = ProjectRevision::new(project(), 3);
        assert_eq!(revision.project(), &project());
        assert_eq!(revision.get(), 3);
        assert_eq!(revision.clone().next().get(), 4);
        assert_eq!(revision.clone().next().project(), &project());
        assert_eq!(
            ProjectRevision::new(project(), u64::MAX).next().get(),
            u64::MAX
        );
        assert_eq!(revision.to_string(), "example:3");
    }

    #[test]
    fn project_revision_serializes_with_project_and_value() {
        let revision = ProjectRevision::new(project(), 3);
        let json = serde_json::to_string(&revision).expect("serialize");
        assert_eq!(json, r#"{"project":"example","value":3}"#);
        let back: ProjectRevision = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, revision);
    }

    #[test]
    fn backend_epoch_keeps_its_backend() {
        let backend = BackendId::new("node-1");
        let epoch = BackendEpoch::new(backend.clone(), 3);
        assert_eq!(epoch.backend(), &backend);
        assert_eq!(epoch.get(), 3);
        assert_eq!(epoch.clone().next().get(), 4);
        assert_eq!(epoch.clone().next().backend(), &backend);
        assert_eq!(BackendEpoch::new(backend, u64::MAX).next().get(), u64::MAX);
        assert_eq!(epoch.to_string(), "node-1:3");
    }

    #[test]
    fn backend_epoch_serializes_with_backend_and_value() {
        let epoch = BackendEpoch::new(BackendId::new("node-1"), 3);
        let json = serde_json::to_string(&epoch).expect("serialize");
        assert_eq!(json, r#"{"backend":"node-1","value":3}"#);
        let back: BackendEpoch = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, epoch);
    }
}
