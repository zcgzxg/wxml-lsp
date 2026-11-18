//! Read-only virtual filesystem policy seam.
//!
//! The engine may only see snapshot files and pinned asset roots. `P3-N`
//! implements callback routing, path normalization and validation; this module
//! freezes the vocabulary and the allow-list container. No host path is
//! resolved or validated here.

/// Root that native reads may resolve within.
///
/// The path is stored verbatim. The caller must pass an absolute, normalized,
/// NUL/control-free path with no `.` or `..` components; `P3-N` must reject
/// roots that violate those invariants before serving any callback.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AllowedRoot(String);

impl AllowedRoot {
    /// Wraps an already-normalized absolute root.
    ///
    /// This constructor performs no validation; see the type-level contract.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Returns the path exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a callback read may be answered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadSource {
    /// File content from the current snapshot.
    Snapshot,
    /// Pinned read-only asset from the verified installation.
    Asset,
    /// Path is outside every allowed root; callbacks fail closed.
    Denied,
}

/// Placeholder for the bounded allow-list of roots.
///
/// Each root must already satisfy the [`AllowedRoot`] contract; this policy
/// does not validate or normalize them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VfsPolicy {
    roots: Vec<AllowedRoot>,
}

impl VfsPolicy {
    /// Creates a policy from already-normalized roots.
    #[must_use]
    pub const fn new(roots: Vec<AllowedRoot>) -> Self {
        Self { roots }
    }

    /// Returns the roots in host order.
    #[must_use]
    pub fn roots(&self) -> &[AllowedRoot] {
        &self.roots
    }
}
