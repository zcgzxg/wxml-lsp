//! Source snapshots and the context carried by every backend request.
//!
//! A [`SourceSnapshot`] owns the exact text a request refers to; a
//! [`RequestContext`] binds that snapshot to the project, view and backend
//! versions it was analyzed with. Backends receive both instead of a bare
//! `FileId`, so a result can be checked against the versions it depended on.

use std::sync::Arc;

use thiserror::Error;
use wxml_text::{LineIndex, SourceRange, TextRange};

use crate::contract::{
    BackendEpoch, FileId, ProjectId, ProjectRevision, SnapshotId, SourceRevision, ViewRevision,
};

/// One immutable source-file snapshot.
///
/// The text is owned through [`Arc`], so cloning a snapshot is cheap and the
/// [`LineIndex`] can never be applied to a different revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSnapshot {
    file: FileId,
    revision: SourceRevision,
    snapshot: SnapshotId,
    line_index: Arc<LineIndex>,
}

impl SourceSnapshot {
    /// Creates a snapshot that owns `text`.
    #[must_use]
    pub fn new(
        file: FileId,
        revision: SourceRevision,
        snapshot: SnapshotId,
        text: impl Into<Arc<str>>,
    ) -> Self {
        Self::from_index(file, revision, snapshot, Arc::new(LineIndex::new(text)))
    }

    /// Creates a snapshot from an existing immutable line index.
    #[must_use]
    pub const fn from_index(
        file: FileId,
        revision: SourceRevision,
        snapshot: SnapshotId,
        text: Arc<LineIndex>,
    ) -> Self {
        Self {
            file,
            revision,
            snapshot,
            line_index: text,
        }
    }

    /// Returns the file this snapshot belongs to.
    #[must_use]
    pub const fn file(&self) -> &FileId {
        &self.file
    }

    /// Returns the document content revision.
    #[must_use]
    pub const fn revision(&self) -> SourceRevision {
        self.revision
    }

    /// Returns the analysis snapshot id this text was captured in.
    #[must_use]
    pub const fn snapshot(&self) -> SnapshotId {
        self.snapshot
    }

    /// Returns the snapshot text.
    #[must_use]
    pub fn text(&self) -> &str {
        self.line_index.text()
    }

    /// Returns the line index for this exact text.
    #[must_use]
    pub fn line_index(&self) -> &LineIndex {
        &self.line_index
    }

    /// Validates `range` against this text and turns it into a [`SourceRange`].
    ///
    /// Returns `None` when the end is past the text or either bound is not on
    /// a UTF-8 character boundary. Bounds are never clamped.
    #[must_use]
    pub fn source_range(&self, range: TextRange) -> Option<SourceRange> {
        let start = usize::try_from(range.start()).ok()?;
        let end = usize::try_from(range.end()).ok()?;
        let text = self.text();
        if end > text.len() {
            return None;
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return None;
        }
        Some(SourceRange::new(range))
    }
}

/// The versions and source snapshot one backend request depends on.
///
/// The context is immutable and cheap to clone. Constructing it checks that
/// the project revision and the source file belong to the same project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestContext {
    project_revision: ProjectRevision,
    view_revision: ViewRevision,
    backend_epoch: BackendEpoch,
    source: Arc<SourceSnapshot>,
}

impl RequestContext {
    /// Creates a context after checking that all identities agree.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError::ProjectMismatch`] when the project revision and
    /// the source file belong to different projects.
    pub fn new(
        project_revision: ProjectRevision,
        view_revision: ViewRevision,
        backend_epoch: BackendEpoch,
        source: Arc<SourceSnapshot>,
    ) -> Result<Self, ContextError> {
        let revision_project = project_revision.project();
        let source_project = source.file().project();
        if revision_project != source_project {
            return Err(ContextError::ProjectMismatch {
                revision: revision_project.clone(),
                source_project: source_project.clone(),
            });
        }
        Ok(Self {
            project_revision,
            view_revision,
            backend_epoch,
            source,
        })
    }

    /// Returns the project revision.
    #[must_use]
    pub fn project_revision(&self) -> ProjectRevision {
        self.project_revision.clone()
    }

    /// Returns the view revision.
    #[must_use]
    pub const fn view_revision(&self) -> ViewRevision {
        self.view_revision
    }

    /// Returns the backend epoch.
    #[must_use]
    pub fn backend_epoch(&self) -> BackendEpoch {
        self.backend_epoch.clone()
    }

    /// Returns the source snapshot.
    #[must_use]
    pub fn source(&self) -> &SourceSnapshot {
        &self.source
    }

    /// Returns the file the request targets.
    #[must_use]
    pub fn file(&self) -> &FileId {
        self.source.file()
    }
}

/// Why a [`RequestContext`] could not be constructed.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ContextError {
    /// The project revision and the source snapshot belong to different
    /// projects.
    #[error(
        "project revision belongs to `{revision}` but the source file belongs to `{source_project}`"
    )]
    ProjectMismatch {
        /// Project recorded in the revision.
        revision: ProjectId,
        /// Project recorded in the source file.
        source_project: ProjectId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::BackendId;

    fn project() -> ProjectId {
        ProjectId::new("example")
    }

    fn file() -> FileId {
        FileId::new(project(), "pages/index/index.wxml").expect("valid path")
    }

    fn snapshot(text: &str) -> SourceSnapshot {
        SourceSnapshot::new(file(), SourceRevision::new(1), SnapshotId::new(2), text)
    }

    fn context(text: &str) -> RequestContext {
        RequestContext::new(
            ProjectRevision::new(project(), 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("mock"), 1),
            Arc::new(snapshot(text)),
        )
        .expect("matching projects")
    }

    #[test]
    fn snapshot_exposes_file_revision_and_text() {
        let snapshot = snapshot("hello");
        assert_eq!(snapshot.file(), &file());
        assert_eq!(snapshot.revision().get(), 1);
        assert_eq!(snapshot.snapshot().get(), 2);
        assert_eq!(snapshot.text(), "hello");
        assert_eq!(
            snapshot.line_index().position(0),
            Some(wxml_text::Position::new(0, 0))
        );
    }

    #[test]
    fn snapshots_share_the_line_index() {
        let index = Arc::new(LineIndex::new("a\nb"));
        let first = SourceSnapshot::from_index(
            file(),
            SourceRevision::new(1),
            SnapshotId::new(1),
            Arc::clone(&index),
        );
        let second =
            SourceSnapshot::from_index(file(), SourceRevision::new(2), SnapshotId::new(2), index);
        assert_eq!(first.line_index(), second.line_index());
        assert_ne!(first.revision(), second.revision());
    }

    #[test]
    fn source_range_accepts_valid_bounds() {
        let snapshot = snapshot("你好😀");
        let range = TextRange::new(0, 6).expect("valid range");
        let source = snapshot.source_range(range).expect("valid source range");
        assert_eq!(source.range(), range);
    }

    #[test]
    fn source_range_rejects_out_of_bounds() {
        let snapshot = snapshot("abc");
        let range = TextRange::new(1, 4).expect("valid range");
        assert_eq!(snapshot.source_range(range), None);
    }

    #[test]
    fn source_range_rejects_split_characters() {
        let snapshot = snapshot("a😀b");
        // The emoji occupies bytes 1..5; splitting it is not a valid range.
        let split_start = TextRange::new(2, 4).expect("valid range");
        let split_end = TextRange::new(1, 4).expect("valid range");
        assert_eq!(snapshot.source_range(split_start), None);
        assert_eq!(snapshot.source_range(split_end), None);
        let whole = TextRange::new(1, 5).expect("valid range");
        assert!(snapshot.source_range(whole).is_some());
    }

    #[test]
    fn request_context_exposes_versions_and_file() {
        let context = context("hello");
        assert_eq!(context.project_revision().get(), 1);
        assert_eq!(context.view_revision().get(), 1);
        assert_eq!(context.backend_epoch().get(), 1);
        assert_eq!(context.file(), &file());
        assert_eq!(context.source().text(), "hello");
    }

    #[test]
    fn request_context_rejects_project_mismatch() {
        let source = Arc::new(SourceSnapshot::new(
            FileId::new(ProjectId::new("other"), "index.wxml").expect("valid path"),
            SourceRevision::new(1),
            SnapshotId::new(1),
            "text",
        ));
        let error = RequestContext::new(
            ProjectRevision::new(project(), 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("mock"), 1),
            source,
        )
        .expect_err("projects differ");
        assert!(matches!(error, ContextError::ProjectMismatch { .. }));
        assert_eq!(
            error.to_string(),
            "project revision belongs to `example` but the source file belongs to `other`"
        );
    }

    #[test]
    fn contexts_compare_by_identity_and_text() {
        let first = context("hello");
        let second = context("hello");
        let third = context("other");
        assert_eq!(first, second);
        assert_ne!(first, third);
    }
}
