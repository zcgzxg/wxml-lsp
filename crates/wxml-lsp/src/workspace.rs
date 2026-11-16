//! In-memory document state for the stdio server.
//!
//! The LSP layer is the only place where UTF-16 positions exist; this module
//! converts them to byte offsets and keeps one immutable [`SourceSnapshot`]
//! per accepted revision. Project discovery and on-disk overlays belong to the
//! project layer (P3) and are not used here: every document gets a synthetic,
//! project-relative [`FileId`] so no client-supplied URI ever reaches the file
//! system.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use wxml_text::Position;
use wxml_types::{
    BackendEpoch, ClientVersion, FileId, ProjectId, ProjectRevision, RequestContext, SnapshotId,
    SourceRevision, SourceSnapshot, ViewRevision,
};

/// Synthetic project every stdio document belongs to until the project layer
/// takes over in a later wave.
const PROJECT: &str = "lsp";

/// Largest accepted document URI.
pub(crate) const MAX_URI_BYTES: usize = 8 * 1024;

/// Default number of documents the server keeps open.
const DEFAULT_MAX_DOCUMENTS: usize = 256;

/// Default largest accepted document text.
const DEFAULT_MAX_DOCUMENT_BYTES: usize = crate::jsonrpc::MAX_MESSAGE_BYTES;

/// Estimated bytes one `LineIndex` line start occupies (a `usize` offset).
const LINE_INDEX_ENTRY_BYTES: usize = 8;

/// Default budget for the text plus estimated line-index cost of all open
/// documents (64 MiB).
///
/// This is deliberately an estimate, not an RSS measurement: eight bytes per
/// `\n` approximates `LineIndex`'s `Vec<usize>` of line starts.
const DEFAULT_MAX_TOTAL_COST: usize = 64 * 1024 * 1024;

/// Upper bound on the cumulative scan work of one `didChange`.
///
/// Every non-noop change charges the current text length plus the estimated
/// line-index cost before it is applied, because an incremental edit rebuilds
/// a whole `LineIndex` over the text as it exists at that point. This bounds a
/// notification's wall time without incrementally indexing; Wave 2 replaces
/// the rebuild with an incremental index and can lower the budget again.
pub(crate) const MAX_EDIT_WORK: usize = 64 * 1024 * 1024;

/// Counts line terminators; mirrors the `LineIndex` line-start count.
fn newline_count(text: &str) -> usize {
    text.match_indices('\n').count()
}

/// Estimated memory cost of a document: text plus line-start entries.
fn document_cost(text: &str) -> usize {
    text.len()
        .saturating_add(LINE_INDEX_ENTRY_BYTES.saturating_mul(newline_count(text)))
}

/// One change from a `textDocument/didChange` notification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContentChange {
    /// Replace the whole document.
    Full(String),
    /// Replace `start..end` with `text`; positions are UTF-16.
    Incremental {
        /// Start of the replaced range.
        start: Position,
        /// End of the replaced range.
        end: Position,
        /// Replacement text.
        text: String,
    },
}

/// Why a document operation was rejected.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum WorkspaceError {
    /// The document limit for this workspace was reached.
    #[error("too many open documents")]
    TooManyDocuments,
    /// The document exceeds the configured byte limit.
    #[error("document is too large")]
    DocumentTooLarge,
    /// No document is open for this URI.
    #[error("document is not open")]
    UnknownDocument,
    /// The client sent a version that is not newer than the stored one.
    #[error("document version {received} is not newer than {current}")]
    VersionRegression {
        /// Version currently stored.
        current: u64,
        /// Version received from the client.
        received: u64,
    },
    /// A change range does not exist in the current text.
    #[error("invalid change range")]
    InvalidRange,
    /// The URI is empty or longer than [`MAX_URI_BYTES`].
    #[error("invalid document URI")]
    InvalidUri,
    /// The estimated cost of all open documents would exceed the budget.
    #[error("the open-document cost budget of {limit} bytes would be exceeded")]
    TotalCostExceeded {
        /// Configured workspace cost budget.
        limit: usize,
    },
    /// One `didChange` would require more editing work than allowed.
    #[error("editing work of {work} exceeds the limit of {limit}")]
    TooMuchWork {
        /// Estimated work (`changes × document bytes`).
        work: usize,
        /// Configured work budget.
        limit: usize,
    },
    /// A real file identity belongs to a different project than this workspace.
    #[error("document belongs to a different project")]
    ProjectMismatch,
}

/// One open document at one revision.
#[derive(Clone, Debug)]
pub(crate) struct Document {
    uri: String,
    file: FileId,
    project_revision: ProjectRevision,
    client_version: ClientVersion,
    revision: SourceRevision,
    view_revision: ViewRevision,
    /// Estimated text plus line-index cost, counted in the workspace budget.
    cost: usize,
    snapshot: Arc<SourceSnapshot>,
}

impl Document {
    /// Returns the URI the client used to open the document.
    pub(crate) fn uri(&self) -> &str {
        &self.uri
    }

    /// Returns the synthetic project-relative identity.
    pub(crate) const fn file(&self) -> &FileId {
        &self.file
    }

    /// Returns the last accepted client version.
    pub(crate) const fn client_version(&self) -> ClientVersion {
        self.client_version
    }

    /// Returns the content revision of this snapshot.
    pub(crate) const fn revision(&self) -> SourceRevision {
        self.revision
    }

    /// Returns the view revision of this snapshot.
    pub(crate) const fn view_revision(&self) -> ViewRevision {
        self.view_revision
    }

    /// Returns the immutable snapshot for this revision.
    pub(crate) const fn snapshot(&self) -> &Arc<SourceSnapshot> {
        &self.snapshot
    }

    /// Returns the document text.
    pub(crate) fn text(&self) -> &str {
        self.snapshot.text()
    }

    /// Returns the estimated text plus line-index cost.
    pub(crate) const fn cost(&self) -> usize {
        self.cost
    }

    /// Converts a UTF-16 position into a byte offset.
    pub(crate) fn offset_at(&self, position: Position) -> Option<u32> {
        u32::try_from(self.snapshot.line_index().offset(position)?).ok()
    }

    /// Builds the versioning stamp a result must match to be published.
    pub(crate) fn stamp(&self, epoch: &BackendEpoch) -> AnalysisStamp {
        AnalysisStamp {
            file: self.file.clone(),
            revision: self.revision,
            snapshot: self.snapshot.snapshot(),
            view_revision: self.view_revision,
            project_revision: self.project_revision.clone(),
            epoch: epoch.clone(),
        }
    }

    /// Builds a request context for this exact snapshot.
    pub(crate) fn request_context(&self, epoch: &BackendEpoch) -> Option<RequestContext> {
        RequestContext::new(
            self.project_revision.clone(),
            self.view_revision,
            epoch.clone(),
            Arc::clone(&self.snapshot),
        )
        .ok()
    }
}

/// Versions a backend result depends on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AnalysisStamp {
    /// File the result belongs to.
    pub(crate) file: FileId,
    /// Content revision.
    pub(crate) revision: SourceRevision,
    /// Analysis snapshot identity.
    pub(crate) snapshot: SnapshotId,
    /// View revision.
    pub(crate) view_revision: ViewRevision,
    /// Project revision.
    pub(crate) project_revision: ProjectRevision,
    /// Backend generation.
    pub(crate) epoch: BackendEpoch,
}

/// Open documents and their monotonically increasing identities.
#[derive(Debug)]
pub(crate) struct Workspace {
    project: ProjectId,
    project_revision: ProjectRevision,
    next_document: u64,
    next_snapshot: u64,
    max_documents: usize,
    max_document_bytes: usize,
    max_total_cost: usize,
    total_cost: usize,
    documents: HashMap<String, Document>,
}

impl Workspace {
    /// Creates a workspace with the default limits.
    pub(crate) fn new() -> Self {
        Self::with_limits(
            DEFAULT_MAX_DOCUMENTS,
            DEFAULT_MAX_DOCUMENT_BYTES,
            DEFAULT_MAX_TOTAL_COST,
        )
    }

    /// Creates a workspace with explicit limits.
    pub(crate) fn with_limits(
        max_documents: usize,
        max_document_bytes: usize,
        max_total_cost: usize,
    ) -> Self {
        let project = ProjectId::new(PROJECT);
        Self {
            project_revision: ProjectRevision::new(project.clone(), 1),
            project,
            next_document: 0,
            next_snapshot: 0,
            max_documents,
            max_document_bytes,
            max_total_cost,
            total_cost: 0,
            documents: HashMap::new(),
        }
    }

    /// Returns the number of open documents.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.documents.len()
    }

    /// Rebinds the workspace to a project (or the synthetic root).
    pub(crate) fn set_project(&mut self, project_id: ProjectId, project_revision: ProjectRevision) {
        self.project = project_id;
        self.project_revision = project_revision;
        for document in self.documents.values_mut() {
            if document.file.project() == &self.project {
                document.project_revision = self.project_revision.clone();
            }
        }
    }

    /// Updates documents of the changed project, including an inactive app.
    pub(crate) fn set_project_revision(&mut self, project_revision: &ProjectRevision) {
        if project_revision.project() == &self.project {
            self.project_revision = project_revision.clone();
        }
        for document in self.documents.values_mut() {
            if document.file.project() == project_revision.project() {
                document.project_revision = project_revision.clone();
            }
        }
    }

    /// Sets the view revision of one open document.
    pub(crate) fn set_view_revision(&mut self, uri: &str, view_revision: ViewRevision) {
        if let Some(document) = self.documents.get_mut(uri) {
            document.view_revision = view_revision;
        }
    }

    /// Iterates open documents.
    #[allow(dead_code)] // retained for sibling handler packages during integration
    pub(crate) fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }

    /// Opens or replaces a document with a synthetic file identity.
    ///
    /// The client URI is used only as a map key. The [`FileId`] is synthetic
    /// (`documents/<n>.wxml`) so client-supplied paths can never be
    /// interpreted as filesystem paths.
    pub(crate) fn open(
        &mut self,
        uri: &str,
        text: String,
        version: Option<u64>,
    ) -> Result<(), WorkspaceError> {
        if uri.is_empty() || uri.len() > MAX_URI_BYTES {
            return Err(WorkspaceError::InvalidUri);
        }
        self.next_document = self.next_document.saturating_add(1);
        let path = format!("documents/{}.wxml", self.next_document);
        let file =
            FileId::new(self.project.clone(), path).map_err(|_| WorkspaceError::InvalidUri)?;
        self.insert(uri, file, text, version, ViewRevision::new(1))
    }

    /// Opens or replaces a document with a real project file identity.
    pub(crate) fn open_file(
        &mut self,
        uri: &str,
        file: FileId,
        text: String,
        version: Option<u64>,
        view_revision: ViewRevision,
    ) -> Result<(), WorkspaceError> {
        self.insert(uri, file, text, version, view_revision)
    }

    /// Shared document insertion path.
    fn insert(
        &mut self,
        uri: &str,
        file: FileId,
        text: String,
        version: Option<u64>,
        view_revision: ViewRevision,
    ) -> Result<(), WorkspaceError> {
        if uri.is_empty() || uri.len() > MAX_URI_BYTES {
            return Err(WorkspaceError::InvalidUri);
        }
        if file.project() != &self.project {
            return Err(WorkspaceError::ProjectMismatch);
        }
        if text.len() > self.max_document_bytes {
            return Err(WorkspaceError::DocumentTooLarge);
        }
        if !self.documents.contains_key(uri) && self.documents.len() >= self.max_documents {
            return Err(WorkspaceError::TooManyDocuments);
        }
        let cost = document_cost(&text);
        let previous_cost = self.documents.get(uri).map_or(0, Document::cost);
        let total_cost = self
            .total_cost
            .saturating_sub(previous_cost)
            .saturating_add(cost);
        if total_cost > self.max_total_cost {
            return Err(WorkspaceError::TotalCostExceeded {
                limit: self.max_total_cost,
            });
        }
        let revision = SourceRevision::new(self.next_snapshot.saturating_add(1));
        let snapshot = Self::build_snapshot(&mut self.next_snapshot, file.clone(), revision, text);
        let document = Document {
            uri: uri.to_owned(),
            file,
            project_revision: self.project_revision.clone(),
            client_version: ClientVersion::new(version.unwrap_or(0)),
            revision,
            view_revision,
            cost,
            snapshot,
        };
        self.documents.insert(uri.to_owned(), document);
        self.total_cost = total_cost;
        Ok(())
    }

    /// Applies one `didChange` notification.
    pub(crate) fn change(
        &mut self,
        uri: &str,
        version: u64,
        changes: &[ContentChange],
    ) -> Result<(), WorkspaceError> {
        let document = self
            .documents
            .get(uri)
            .ok_or(WorkspaceError::UnknownDocument)?;
        let current = document.client_version().get();
        if version <= current {
            return Err(WorkspaceError::VersionRegression {
                current,
                received: version,
            });
        }
        let old_cost = document.cost();
        let text = apply_changes(document.text(), changes, self.max_document_bytes)?;
        let cost = document_cost(&text);
        let total_cost = self
            .total_cost
            .saturating_sub(old_cost)
            .saturating_add(cost);
        if total_cost > self.max_total_cost {
            return Err(WorkspaceError::TotalCostExceeded {
                limit: self.max_total_cost,
            });
        }
        let revision = document.revision().next();
        let view_revision = document.view_revision().next();
        let file = document.file().clone();
        let snapshot = Self::build_snapshot(&mut self.next_snapshot, file, revision, text);
        let replacement = Document {
            uri: document.uri.clone(),
            file: document.file.clone(),
            project_revision: document.project_revision.clone(),
            client_version: ClientVersion::new(version),
            revision,
            view_revision,
            cost,
            snapshot,
        };
        self.documents.insert(uri.to_owned(), replacement);
        self.total_cost = total_cost;
        Ok(())
    }

    /// Closes a document and returns it, if it was open.
    pub(crate) fn close(&mut self, uri: &str) -> Option<Document> {
        let document = self.documents.remove(uri)?;
        self.total_cost = self.total_cost.saturating_sub(document.cost());
        Some(document)
    }

    /// Returns an open document.
    pub(crate) fn get(&self, uri: &str) -> Option<&Document> {
        self.documents.get(uri)
    }

    /// Returns the document with this file identity.
    pub(crate) fn get_by_file(&self, file: &FileId) -> Option<&Document> {
        self.documents
            .values()
            .find(|document| document.file == *file)
    }

    /// Returns whether a result with `stamp` still matches the open document.
    pub(crate) fn is_current(&self, stamp: &AnalysisStamp) -> bool {
        self.documents
            .values()
            .find(|document| document.file == stamp.file)
            .is_some_and(|document| {
                document.project_revision == stamp.project_revision
                    && document.revision == stamp.revision
                    && document.view_revision == stamp.view_revision
                    && document.snapshot.snapshot() == stamp.snapshot
            })
    }

    /// Builds a snapshot and advances the snapshot counter.
    fn build_snapshot(
        next_snapshot: &mut u64,
        file: FileId,
        revision: SourceRevision,
        text: String,
    ) -> Arc<SourceSnapshot> {
        *next_snapshot = next_snapshot.saturating_add(1);
        let snapshot = SnapshotId::new(*next_snapshot);
        Arc::new(SourceSnapshot::new(file, revision, snapshot, text))
    }
}

/// Returns `true` for changes that cannot alter the document.
fn is_noop(change: &ContentChange) -> bool {
    matches!(
        change,
        ContentChange::Incremental { start, end, text } if start == end && text.is_empty()
    )
}

/// Applies a list of changes to a fresh copy of `text`.
fn apply_changes(
    text: &str,
    changes: &[ContentChange],
    max_document_bytes: usize,
) -> Result<String, WorkspaceError> {
    let mut current = text.to_owned();
    let mut work = 0_usize;
    for change in changes {
        if is_noop(change) {
            continue;
        }
        // Charge the scan the next incremental edit performs over the text as
        // it exists now, not the text the notification started with. The
        // rebuilt `LineIndex` also allocates one `usize` per line start, so
        // newline-dense text is more expensive than its byte length suggests.
        work = work
            .saturating_add(current.len())
            .saturating_add(LINE_INDEX_ENTRY_BYTES.saturating_mul(newline_count(&current)));
        if work > MAX_EDIT_WORK {
            return Err(WorkspaceError::TooMuchWork {
                work,
                limit: MAX_EDIT_WORK,
            });
        }
        match change {
            ContentChange::Full(text) => current.clone_from(text),
            ContentChange::Incremental { start, end, text } => {
                let index = wxml_text::LineIndex::new(current.as_str());
                let start = index.offset(*start).ok_or(WorkspaceError::InvalidRange)?;
                let end = index.offset(*end).ok_or(WorkspaceError::InvalidRange)?;
                if start > end || current.get(start..end).is_none() {
                    return Err(WorkspaceError::InvalidRange);
                }
                current.replace_range(start..end, text);
            }
        }
        if current.len() > max_document_bytes {
            return Err(WorkspaceError::DocumentTooLarge);
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use wxml_types::BackendId;

    fn uri() -> &'static str {
        "file:///pages/index/index.wxml"
    }

    fn epoch() -> BackendEpoch {
        BackendEpoch::new(BackendId::new("mock"), 1)
    }

    fn pos(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    fn workspace() -> Workspace {
        Workspace::new()
    }

    #[test]
    fn open_tracks_text_version_and_a_synthetic_file() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "{{ user.name }}".to_owned(), Some(1))
            .expect("open");
        let document = workspace.get(uri()).expect("document");
        assert_eq!(document.text(), "{{ user.name }}");
        assert_eq!(document.client_version().get(), 1);
        assert_eq!(document.file().path(), "documents/1.wxml");
        assert_eq!(document.file().project().as_str(), PROJECT);
        assert!(!document.file().path().contains(".."));
        assert_eq!(workspace.len(), 1);
    }

    #[test]
    fn open_defaults_the_version_to_zero() {
        let mut workspace = workspace();
        workspace.open(uri(), String::new(), None).expect("open");
        assert_eq!(
            workspace
                .get(uri())
                .expect("document")
                .client_version()
                .get(),
            0
        );
    }

    #[test]
    fn client_paths_never_reach_the_file_identity() {
        let mut workspace = workspace();
        for suspect in [
            "file:///../../etc/passwd",
            "../../etc/passwd",
            "file:///C:/Windows/system32/evil.wxml",
            "/etc/passwd",
        ] {
            workspace
                .open(suspect, "x".to_owned(), Some(1))
                .expect("open");
            let document = workspace.get(suspect).expect("document");
            let path = document.file().path();
            assert!(path.starts_with("documents/"), "{path}");
            assert!(!path.contains("etc"), "{path}");
            assert!(!path.contains("Windows"), "{path}");
            assert!(!path.starts_with('/'), "{path}");
        }
    }

    #[test]
    fn open_rejects_empty_and_oversized_uris() {
        let mut workspace = workspace();
        assert_eq!(
            workspace.open("", String::new(), None).expect_err("empty"),
            WorkspaceError::InvalidUri
        );
        let long = "a".repeat(MAX_URI_BYTES + 1);
        assert_eq!(
            workspace
                .open(&long, String::new(), None)
                .expect_err("long"),
            WorkspaceError::InvalidUri
        );
    }

    #[test]
    fn open_rejects_too_many_documents() {
        let mut workspace = Workspace::with_limits(1, 1_024, 4_096);
        workspace
            .open("file:///a.wxml", String::new(), None)
            .expect("first");
        assert_eq!(
            workspace
                .open("file:///b.wxml", String::new(), None)
                .expect_err("second"),
            WorkspaceError::TooManyDocuments
        );
    }

    #[test]
    fn open_and_close_respect_the_total_cost_budget() {
        let mut workspace = Workspace::with_limits(10, 100, 10);
        workspace
            .open("file:///a.wxml", "abcdef".to_owned(), Some(1))
            .expect("first");
        workspace
            .open("file:///b.wxml", "ghij".to_owned(), Some(1))
            .expect("second");
        assert_eq!(
            workspace
                .open("file:///c.wxml", "k".to_owned(), Some(1))
                .expect_err("over budget"),
            WorkspaceError::TotalCostExceeded { limit: 10 }
        );
        assert_eq!(
            workspace
                .open("file:///a.wxml", "abcdefg".to_owned(), Some(1))
                .expect_err("growing replacement"),
            WorkspaceError::TotalCostExceeded { limit: 10 }
        );
        assert_eq!(
            workspace.get("file:///a.wxml").expect("document").text(),
            "abcdef"
        );
        workspace.close("file:///a.wxml").expect("close");
        workspace
            .open("file:///c.wxml", "k".to_owned(), Some(1))
            .expect("freed budget");
    }

    #[test]
    fn newline_dense_documents_consume_the_total_budget_faster() {
        let mut workspace = Workspace::new();
        // 4 MB of text with 2M newlines: 4 MB text + 16 MB line starts.
        let text = format!("{}{}", "\n".repeat(2_000_000), "a".repeat(2_000_000));
        for index in 0..3 {
            workspace
                .open(&format!("file:///dense{index}.wxml"), text.clone(), Some(1))
                .expect("three dense documents fit");
        }
        assert!(matches!(
            workspace.open("file:///dense3.wxml", text, Some(1)),
            Err(WorkspaceError::TotalCostExceeded { .. })
        ));
    }

    #[test]
    fn line_index_cost_counts_toward_the_document_budget() {
        let mut workspace = Workspace::with_limits(10, 1_000, 32);
        workspace
            .open("file:///a.wxml", "a".repeat(32), Some(1))
            .expect("32-byte text costs 32");
        assert_eq!(
            workspace
                .open("file:///b.wxml", "b".to_owned(), Some(1))
                .expect_err("over budget"),
            WorkspaceError::TotalCostExceeded { limit: 32 }
        );
        workspace.close("file:///a.wxml").expect("close");
        workspace
            .open("file:///b.wxml", "b".to_owned(), Some(1))
            .expect("cost 1");
        // Four newlines add four line starts: 4 text bytes + 32 index bytes.
        assert_eq!(
            workspace
                .change("file:///b.wxml", 2, &[ContentChange::Full("\n".repeat(4))],)
                .expect_err("line index cost"),
            WorkspaceError::TotalCostExceeded { limit: 32 }
        );
        assert_eq!(
            workspace.get("file:///b.wxml").expect("document").text(),
            "b"
        );
    }

    #[test]
    fn change_respects_the_total_cost_budget() {
        let mut workspace = Workspace::with_limits(10, 100, 10);
        workspace
            .open(uri(), "abc".to_owned(), Some(1))
            .expect("open");
        assert_eq!(
            workspace
                .change(uri(), 2, &[ContentChange::Full("0123456789a".to_owned())],)
                .expect_err("over budget"),
            WorkspaceError::TotalCostExceeded { limit: 10 }
        );
        assert_eq!(workspace.get(uri()).expect("document").text(), "abc");
        workspace
            .change(uri(), 2, &[ContentChange::Full("0123456789".to_owned())])
            .expect("exactly at budget");
    }

    fn insertion() -> ContentChange {
        ContentChange::Incremental {
            start: pos(0, 0),
            end: pos(0, 0),
            text: "x".to_owned(),
        }
    }

    #[test]
    fn excessive_edit_work_is_rejected_atomically() {
        let mut workspace = Workspace::new();
        // 2 MiB × 33 edits exceeds the 64 MiB cumulative budget.
        let text = "a".repeat(2 * 1024 * 1024);
        workspace.open(uri(), text.clone(), Some(1)).expect("open");
        let changes: Vec<ContentChange> = (0..33).map(|_| insertion()).collect();
        assert!(matches!(
            workspace.change(uri(), 2, &changes),
            Err(WorkspaceError::TooMuchWork { .. })
        ));
        assert_eq!(workspace.get(uri()).expect("document").text(), text);

        workspace
            .change(uri(), 2, &changes[..30])
            .expect("edits within the budget");
        assert_eq!(
            workspace.get(uri()).expect("document").text().len(),
            text.len() + 30
        );
    }

    #[test]
    fn edit_work_is_charged_against_the_grown_text() {
        for full_size in [1024 * 1024, 7 * 1024 * 1024] {
            let mut workspace = Workspace::new();
            workspace
                .open(uri(), "ab".to_owned(), Some(1))
                .expect("open");
            let mut changes = vec![ContentChange::Full("x".repeat(full_size))];
            changes.extend((0..255).map(|_| insertion()));
            let started = Instant::now();
            let result = workspace.change(uri(), 2, &changes);
            let elapsed = started.elapsed();
            assert!(
                matches!(result, Err(WorkspaceError::TooMuchWork { .. })),
                "full size {full_size}"
            );
            // The budget is a wall-time guard, not just a counter: the
            // pre-fix version spent >10 s on this input.
            assert!(
                elapsed < Duration::from_secs(5),
                "full size {full_size} took {elapsed:?}"
            );
            assert_eq!(
                workspace.get(uri()).expect("document").text(),
                "ab",
                "full size {full_size}"
            );
        }
    }

    #[test]
    fn newline_dense_edit_is_rejected_within_budget() {
        // 250k line starts cost 2 MB of index entries per edit in addition to
        // the 0.25 MB text, so 255 edits are rejected after roughly 29 of them.
        let mut workspace = Workspace::new();
        let text = "\n".repeat(250_000);
        workspace.open(uri(), text.clone(), Some(1)).expect("open");
        let changes: Vec<ContentChange> = (0..255).map(|_| insertion()).collect();
        let started = Instant::now();
        let result = workspace.change(uri(), 2, &changes);
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(WorkspaceError::TooMuchWork { .. })));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert_eq!(workspace.get(uri()).expect("document").text(), text);
    }

    #[test]
    fn very_dense_edit_is_rejected_atomically() {
        // 4M lines in 7.65 MB: one edit costs ~40 MiB, so the second is over
        // the 64 MiB budget. The looser wall-clock bound keeps this debug-build
        // test stable on slower machines; the budget, not the timer, is the
        // guarantee.
        let mut dense = "\n".repeat(4_000_000);
        dense.push_str(&"a".repeat(3_650_000));
        let mut workspace = Workspace::new();
        workspace
            .open("file:///dense.wxml", dense.clone(), Some(1))
            .expect("open");
        let started = Instant::now();
        let result = workspace.change("file:///dense.wxml", 2, &[insertion(), insertion()]);
        let elapsed = started.elapsed();
        assert!(matches!(result, Err(WorkspaceError::TooMuchWork { .. })));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert_eq!(
            workspace
                .get("file:///dense.wxml")
                .expect("document")
                .text(),
            dense
        );
    }

    #[test]
    fn a_few_edits_on_a_large_document_stay_within_budget() {
        let mut workspace = Workspace::new();
        let text = "a".repeat(7 * 1024 * 1024);
        workspace.open(uri(), text.clone(), Some(1)).expect("open");
        let changes: Vec<ContentChange> = (0..3).map(|_| insertion()).collect();
        workspace.change(uri(), 2, &changes).expect("within budget");
        assert_eq!(
            workspace.get(uri()).expect("document").text().len(),
            text.len() + 3
        );
    }

    #[test]
    fn noop_changes_do_not_consume_the_edit_budget() {
        let mut workspace = Workspace::new();
        let text = "a".repeat(2 * 1024 * 1024);
        workspace.open(uri(), text.clone(), Some(1)).expect("open");
        let changes: Vec<ContentChange> = (0..1_024)
            .map(|_| ContentChange::Incremental {
                start: pos(0, 0),
                end: pos(0, 0),
                text: String::new(),
            })
            .collect();
        workspace
            .change(uri(), 2, &changes)
            .expect("no-ops are filtered");
        assert_eq!(workspace.get(uri()).expect("document").text(), text);
        assert_eq!(
            workspace
                .get(uri())
                .expect("document")
                .client_version()
                .get(),
            2
        );
    }

    #[test]
    fn open_rejects_oversized_documents() {
        let mut workspace = Workspace::with_limits(4, 4, 64);
        assert_eq!(
            workspace
                .open(uri(), "abcde".to_owned(), None)
                .expect_err("too large"),
            WorkspaceError::DocumentTooLarge
        );
    }

    #[test]
    fn full_change_replaces_text_and_advances_versions() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "old".to_owned(), Some(1))
            .expect("open");
        let before = workspace.get(uri()).expect("document").clone();
        workspace
            .change(uri(), 2, &[ContentChange::Full("new text".to_owned())])
            .expect("change");
        let after = workspace.get(uri()).expect("document");
        assert_eq!(after.text(), "new text");
        assert_eq!(after.client_version().get(), 2);
        assert!(after.revision() > before.revision());
        assert!(after.view_revision() > before.view_revision());
        assert_ne!(after.snapshot().snapshot(), before.snapshot().snapshot());
    }

    #[test]
    fn incremental_change_uses_utf16_positions() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "中文😀a\r\nbc".to_owned(), Some(1))
            .expect("open");
        // UTF-16 columns: 中=1, 文=1, 😀=2, a=1. Replace `a` (columns 4..5).
        workspace
            .change(
                uri(),
                2,
                &[ContentChange::Incremental {
                    start: pos(0, 4),
                    end: pos(0, 5),
                    text: "XY".to_owned(),
                }],
            )
            .expect("change");
        assert_eq!(
            workspace.get(uri()).expect("document").text(),
            "中文😀XY\r\nbc"
        );
    }

    #[test]
    fn incremental_change_uses_byte_offsets_of_the_current_snapshot() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "中文😀a\r\nbc".to_owned(), Some(1))
            .expect("open");
        let document = workspace.get(uri()).expect("document");
        assert_eq!(document.offset_at(pos(0, 0)), Some(0));
        assert_eq!(document.offset_at(pos(0, 2)), Some(6));
        assert_eq!(document.offset_at(pos(0, 4)), Some(10));
        assert_eq!(document.offset_at(pos(0, 5)), Some(11));
        assert_eq!(document.offset_at(pos(1, 0)), Some(13));
        assert_eq!(document.offset_at(pos(1, 2)), Some(15));
        // Inside the surrogate pair or past the line content.
        assert_eq!(document.offset_at(pos(0, 3)), None);
        assert_eq!(document.offset_at(pos(0, 6)), None);
        assert_eq!(document.offset_at(pos(2, 0)), None);
    }

    #[test]
    fn multiple_changes_apply_in_order() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "abc".to_owned(), Some(1))
            .expect("open");
        workspace
            .change(
                uri(),
                2,
                &[
                    ContentChange::Incremental {
                        start: pos(0, 1),
                        end: pos(0, 2),
                        text: "X".to_owned(),
                    },
                    ContentChange::Incremental {
                        start: pos(0, 1),
                        end: pos(0, 2),
                        text: "Y".to_owned(),
                    },
                ],
            )
            .expect("change");
        assert_eq!(workspace.get(uri()).expect("document").text(), "aYc");
    }

    #[test]
    fn change_rejects_version_regressions_and_keeps_the_text() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "abc".to_owned(), Some(3))
            .expect("open");
        for version in [3, 2, 0] {
            assert_eq!(
                workspace
                    .change(uri(), version, &[ContentChange::Full("nope".to_owned())],)
                    .expect_err("regression"),
                WorkspaceError::VersionRegression {
                    current: 3,
                    received: version,
                }
            );
            assert_eq!(workspace.get(uri()).expect("document").text(), "abc");
        }
    }

    #[test]
    fn change_rejects_invalid_ranges_and_keeps_the_text() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "abc".to_owned(), Some(1))
            .expect("open");
        for (start, end) in [(pos(1, 0), pos(1, 0)), (pos(0, 2), pos(0, 1))] {
            assert_eq!(
                workspace
                    .change(
                        uri(),
                        2,
                        &[ContentChange::Incremental {
                            start,
                            end,
                            text: "x".to_owned(),
                        }],
                    )
                    .expect_err("invalid range"),
                WorkspaceError::InvalidRange
            );
            assert_eq!(workspace.get(uri()).expect("document").text(), "abc");
        }
    }

    #[test]
    fn change_on_an_unknown_document_fails() {
        let mut workspace = workspace();
        assert_eq!(
            workspace
                .change(uri(), 1, &[ContentChange::Full("x".to_owned())])
                .expect_err("unknown"),
            WorkspaceError::UnknownDocument
        );
        assert_eq!(
            workspace
                .close(uri())
                .map(|document| document.uri().to_owned()),
            None
        );
    }

    #[test]
    fn close_removes_the_document() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "abc".to_owned(), Some(1))
            .expect("open");
        let closed = workspace.close(uri()).expect("closed");
        assert_eq!(closed.text(), "abc");
        assert!(workspace.get(uri()).is_none());
        assert_eq!(workspace.len(), 0);
        assert!(workspace.close(uri()).is_none());
    }

    #[test]
    fn stamps_are_current_until_the_next_change() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "abc".to_owned(), Some(1))
            .expect("open");
        let stamp = workspace.get(uri()).expect("document").stamp(&epoch());
        assert!(workspace.is_current(&stamp));
        workspace
            .change(uri(), 2, &[ContentChange::Full("abcd".to_owned())])
            .expect("change");
        assert!(!workspace.is_current(&stamp));
        let fresh = workspace.get(uri()).expect("document").stamp(&epoch());
        assert!(workspace.is_current(&fresh));
        workspace.close(uri()).expect("closed");
        assert!(!workspace.is_current(&fresh));
    }

    #[test]
    fn request_context_matches_the_snapshot() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "abc".to_owned(), Some(1))
            .expect("open");
        let document = workspace.get(uri()).expect("document");
        let context = document.request_context(&epoch()).expect("context");
        assert_eq!(context.file(), document.file());
        assert_eq!(context.source().text(), "abc");
        assert_eq!(context.source().revision(), document.revision());
        assert_eq!(context.view_revision(), document.view_revision());
        assert_eq!(context.backend_epoch(), epoch());
        assert_eq!(
            context.project_revision().project(),
            document.file().project()
        );
    }

    #[test]
    fn reopen_replaces_the_previous_document() {
        let mut workspace = workspace();
        workspace
            .open(uri(), "old".to_owned(), Some(1))
            .expect("open");
        workspace
            .open(uri(), "new".to_owned(), Some(1))
            .expect("reopen");
        let document = workspace.get(uri()).expect("document");
        assert_eq!(document.text(), "new");
        assert_eq!(workspace.len(), 1);
    }
}
