//! Shared fixtures for integration tests that need a temporary project root.

#![allow(dead_code, clippy::redundant_pub_crate)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A temporary project directory removed on drop.
pub(super) struct TempProject {
    root: PathBuf,
}

impl TempProject {
    /// Creates a unique project directory under `TMPDIR` (or `/tmp/opencode`).
    pub(super) fn new(prefix: &str) -> Self {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!(
            "{prefix}-{stamp}-{}",
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("temp project");
        Self { root }
    }

    /// Returns the project root.
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// Writes one project file, creating parent directories.
    pub(super) fn write(&self, path: &str, text: &str) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(target, text).expect("fixture");
    }

    /// Returns the `file://` URI of the project root.
    pub(super) fn root_uri(&self) -> String {
        format!("file://{}", self.root.display())
    }

    /// Returns the `file://` URI of a project-relative path.
    pub(super) fn uri(&self, path: &str) -> String {
        format!("file://{}", self.root.join(path).display())
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
