//! Snapshot-bound native node handles.
//!
//! Native node handles are `<encoder>.<kind>.<path>` values that carry no
//! snapshot identity; across updates they may silently address a different
//! node. Every handle therefore needs an explicit Rust-side binding before it
//! may be sent. The binding combines the six frozen identity components:
//! `(session_generation, engine_snapshot_id, normalized_file_path,
//! content_hash_or_node_id, node_index, node_kind)`.
//!
//! `EngineSnapshotId` is the opaque identifier returned by the engine.
//! `SnapshotRevision` in [`crate::input`] is the Rust host revision and is not
//! interchangeable with it.

/// Monotonic engine session generation, incremented on every restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionGeneration(u64);

impl SessionGeneration {
    /// Creates a session generation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw generation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque snapshot identifier returned by the native engine.
///
/// This is not a Rust [`crate::input::SnapshotRevision`]; the engine may reuse
/// or reassign its identifiers, so Rust epoch binding is always required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EngineSnapshotId(u64);

impl EngineSnapshotId {
    /// Creates an engine snapshot identifier.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw engine identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Rust-side binding required before a native handle may be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleBinding {
    /// Engine session that produced the handle.
    pub session_generation: SessionGeneration,
    /// Engine snapshot that produced the handle.
    pub engine_snapshot_id: EngineSnapshotId,
    /// Normalized file path the handle belongs to.
    pub normalized_file_path: String,
    /// Content hash or engine node id from the same snapshot.
    pub content_hash_or_node_id: String,
    /// Encoder index segment of the handle.
    pub node_index: u32,
    /// Node-kind segment of the handle.
    pub node_kind: u32,
}

/// Parsed native node handle.
///
/// The scaffold never parses, validates or sends one; the fields document the
/// segments the future parser must check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHandle {
    /// Encoder index segment.
    pub encoder_index: u32,
    /// Node-kind segment.
    pub kind: u32,
    /// Remaining path segment exactly as received.
    pub path: String,
}
