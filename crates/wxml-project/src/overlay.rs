//! In-memory document overlays that take priority over disk contents.

use std::collections::BTreeMap;
use std::path::Path;

use thiserror::Error;

use crate::files::{self, FileError, FileKey, MAX_FILE_BYTES, ProjectPath};

/// Why an overlay update was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum OverlayError {
    /// The attempted revision is not newer than the stored one.
    #[error("overlay revision {attempted} is not newer than stored revision {stored}")]
    StaleRevision {
        /// Revision currently stored for the path.
        stored: u64,
        /// Revision supplied by the caller.
        attempted: u64,
    },
    /// The overlay text exceeds the project file limit.
    #[error("overlay text is too large: {size} bytes")]
    TooLarge {
        /// Size of the rejected text in bytes.
        size: u64,
    },
}

#[derive(Clone, Debug)]
struct Entry {
    path: ProjectPath,
    text: String,
    revision: u64,
}

/// Editor buffers for one project, keyed by canonical project path.
///
/// Overlay text always wins over disk contents; removing an entry makes the
/// project fall back to disk. Revisions are per path and strictly monotonic:
/// [`Overlay::set`] rejects a revision that is not greater than the stored one
/// and keeps the existing text. [`Overlay::remove`] deletes the entry and its
/// revision history, so a client that reopens a document with a restarted
/// version counter can set it again with any revision.
///
/// Text is bounded by [`MAX_FILE_BYTES`], the same limit used for disk reads.
/// Larger buffers are rejected with [`OverlayError::TooLarge`] before they are
/// stored.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    entries: BTreeMap<FileKey, Entry>,
}

impl Overlay {
    /// Creates an empty overlay.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Inserts or replaces the text for `path`.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TooLarge`] when the text exceeds
    /// [`MAX_FILE_BYTES`], and [`OverlayError::StaleRevision`] when `revision`
    /// is less than or equal to the revision already stored for the same path.
    /// The existing text is left untouched in both cases.
    pub fn set(
        &mut self,
        path: &ProjectPath,
        text: impl Into<String>,
        revision: u64,
    ) -> Result<(), OverlayError> {
        let text = text.into();
        let size = u64::try_from(text.len()).unwrap_or(u64::MAX);
        if size > MAX_FILE_BYTES {
            return Err(OverlayError::TooLarge { size });
        }
        let key = FileKey::new(path);
        if let Some(entry) = self.entries.get(&key) {
            if revision <= entry.revision {
                return Err(OverlayError::StaleRevision {
                    stored: entry.revision,
                    attempted: revision,
                });
            }
        }
        self.entries.insert(
            key,
            Entry {
                path: path.clone(),
                text,
                revision,
            },
        );
        Ok(())
    }

    /// Removes the overlay entry, returning whether one existed.
    ///
    /// Disk contents become visible again immediately. The stored revision is
    /// forgotten; see the type documentation.
    pub fn remove(&mut self, path: &ProjectPath) -> bool {
        self.entries.remove(&FileKey::new(path)).is_some()
    }

    /// Returns overlay text, or `None` when the path is not overlaid.
    #[must_use]
    pub fn text(&self, path: &ProjectPath) -> Option<&str> {
        self.entries
            .get(&FileKey::new(path))
            .map(|entry| entry.text.as_str())
    }

    /// Returns the revision stored for `path`, if any.
    #[must_use]
    pub fn revision(&self, path: &ProjectPath) -> Option<u64> {
        self.entries
            .get(&FileKey::new(path))
            .map(|entry| entry.revision)
    }

    /// Returns whether `path` has an overlay entry.
    #[must_use]
    pub fn contains(&self, path: &ProjectPath) -> bool {
        self.entries.contains_key(&FileKey::new(path))
    }

    /// Returns the number of overlaid paths.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the overlay is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates over overlaid paths in canonical order.
    pub fn paths(&self) -> impl Iterator<Item = &ProjectPath> + '_ {
        self.entries.values().map(|entry| &entry.path)
    }

    /// Reads the effective text for `path`: overlay first, then disk.
    ///
    /// Returns `Ok(None)` when the file is absent from both sources. `root`
    /// must be an absolute, canonical project root.
    pub(crate) fn read_text(
        &self,
        root: &Path,
        path: &ProjectPath,
    ) -> Result<Option<String>, FileError> {
        if let Some(text) = self.text(path) {
            return Ok(Some(text.to_owned()));
        }
        match files::read_file(root, path) {
            Ok(text) => Ok(Some(text)),
            Err(FileError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    #[test]
    fn stores_text_and_revision() {
        let mut overlay = Overlay::new();
        let page = path("pages/index/index.wxml");
        overlay.set(&page, "<view/>", 1).expect("set");

        assert_eq!(overlay.text(&page), Some("<view/>"));
        assert_eq!(overlay.revision(&page), Some(1));
        assert!(overlay.contains(&page));
        assert_eq!(overlay.len(), 1);
        assert!(!overlay.is_empty());
    }

    #[test]
    fn newer_revision_replaces_text() {
        let mut overlay = Overlay::new();
        let page = path("a.wxml");
        overlay.set(&page, "one", 1).expect("first");
        overlay.set(&page, "two", 2).expect("second");
        assert_eq!(overlay.text(&page), Some("two"));
        assert_eq!(overlay.revision(&page), Some(2));
    }

    #[test]
    fn stale_revision_is_rejected_without_clobbering() {
        let mut overlay = Overlay::new();
        let page = path("a.wxml");
        overlay.set(&page, "new", 5).expect("set");

        assert_eq!(
            overlay.set(&page, "old", 4),
            Err(OverlayError::StaleRevision {
                stored: 5,
                attempted: 4
            })
        );
        assert_eq!(
            overlay.set(&page, "same", 5),
            Err(OverlayError::StaleRevision {
                stored: 5,
                attempted: 5
            })
        );
        assert_eq!(overlay.text(&page), Some("new"));
        assert_eq!(overlay.revision(&page), Some(5));
    }

    #[test]
    fn remove_falls_back_and_forgets_revision() {
        let mut overlay = Overlay::new();
        let page = path("a.wxml");
        overlay.set(&page, "unsaved", 7).expect("set");

        assert!(overlay.remove(&page));
        assert!(!overlay.remove(&page));
        assert_eq!(overlay.text(&page), None);
        assert_eq!(overlay.revision(&page), None);
        assert!(overlay.is_empty());

        overlay.set(&page, "reopened", 1).expect("reopen resets");
        assert_eq!(overlay.text(&page), Some("reopened"));
    }

    #[test]
    fn rejects_text_above_the_file_limit() {
        let mut overlay = Overlay::new();
        let page = path("a.wxml");
        let oversized = "x".repeat(usize::try_from(MAX_FILE_BYTES).expect("fits usize") + 1);
        assert_eq!(
            overlay.set(&page, oversized, 1),
            Err(OverlayError::TooLarge {
                size: MAX_FILE_BYTES + 1
            })
        );
        assert_eq!(overlay.text(&page), None);
    }

    #[test]
    fn iterates_paths_in_order() {
        let mut overlay = Overlay::new();
        overlay.set(&path("b.wxml"), "b", 1).expect("set");
        overlay.set(&path("a.wxml"), "a", 1).expect("set");
        let paths: Vec<&str> = overlay.paths().map(ProjectPath::as_str).collect();
        assert_eq!(paths, ["a.wxml", "b.wxml"]);
    }
}
