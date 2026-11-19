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
//!
//! This module only parses and validates handles; it never decodes an AST and
//! never sends a handle by itself.

use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::native::wire::RawSymbolReference;

/// Monotonic engine session generation, incremented on every restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionGeneration(u64);

impl SessionGeneration {
    /// Creates a session generation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Allocates the next process-wide session generation.
    ///
    /// Every [`crate::native::session::NativeSession`] start receives a new
    /// generation, so a restart can never reuse bindings from the previous
    /// process.
    #[must_use]
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
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
    pub(crate) session_generation: SessionGeneration,
    /// Engine snapshot that produced the handle.
    pub(crate) engine_snapshot_id: EngineSnapshotId,
    /// Normalized file path the handle belongs to.
    pub(crate) normalized_file_path: String,
    /// Content hash or engine node id from the same snapshot.
    pub(crate) content_hash_or_node_id: String,
    /// Encoder index segment of the handle.
    pub(crate) node_index: u32,
    /// Node-kind segment of the handle.
    pub(crate) node_kind: u32,
}

/// Parsed native node handle.
///
/// The wire format is `<encoder index>.<kind>.<path>`. The path may contain
/// dots, so only the first two separators are structural.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHandle {
    /// Encoder index segment.
    pub encoder_index: u32,
    /// Node-kind segment.
    pub kind: u32,
    /// Remaining path segment exactly as received.
    pub path: String,
}

impl NodeHandle {
    /// Parses `<encoder>.<kind>.<path>`.
    ///
    /// # Errors
    ///
    /// Rejects missing segments, non-decimal indices, an empty path and paths
    /// that are not absolute Unix paths or contain `.`/`..` components.
    pub fn parse(value: &str) -> Result<Self, HandleError> {
        let mut parts = value.splitn(3, '.');
        let Some(index) = parts.next() else {
            return Err(HandleError::Malformed {
                reason: "missing encoder index",
            });
        };
        let Some(kind) = parts.next() else {
            return Err(HandleError::Malformed {
                reason: "missing node kind",
            });
        };
        let Some(path) = parts.next() else {
            return Err(HandleError::Malformed {
                reason: "missing node path",
            });
        };

        let encoder_index = index.parse::<u32>().map_err(|_| HandleError::Malformed {
            reason: "encoder index is not a u32",
        })?;
        let kind = kind.parse::<u32>().map_err(|_| HandleError::Malformed {
            reason: "node kind is not a u32",
        })?;
        validate_absolute_path(path)?;

        Ok(Self {
            encoder_index,
            kind,
            path: path.to_owned(),
        })
    }

    /// Reconstructs the wire string.
    #[must_use]
    pub fn as_wire(&self) -> String {
        format!("{}.{}.{}", self.encoder_index, self.kind, self.path)
    }
}

/// A handle plus the epoch/file/node identity it was produced under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundNodeHandle {
    handle: NodeHandle,
    binding: HandleBinding,
}

impl BoundNodeHandle {
    /// Creates a binding from an already parsed handle and identity.
    ///
    /// Crate-private: only the native session may mint a bound handle, so an
    /// external caller cannot forge an epoch for an arbitrary wire string.
    #[must_use]
    pub(crate) const fn new(handle: NodeHandle, binding: HandleBinding) -> Self {
        Self { handle, binding }
    }

    /// Returns the parsed handle.
    #[must_use]
    pub const fn handle(&self) -> &NodeHandle {
        &self.handle
    }

    /// Returns the binding identity.
    #[must_use]
    pub const fn binding(&self) -> &HandleBinding {
        &self.binding
    }

    /// Reconstructs the wire string for a validated send.
    #[must_use]
    pub fn as_wire(&self) -> String {
        self.handle.as_wire()
    }

    /// Returns whether the binding still matches the supplied identity.
    #[must_use]
    pub fn is_current(
        &self,
        generation: SessionGeneration,
        snapshot: EngineSnapshotId,
        content_hash: &str,
    ) -> bool {
        self.binding.session_generation == generation
            && self.binding.engine_snapshot_id == snapshot
            && self.binding.content_hash_or_node_id == content_hash
    }
}

/// Rust-side epoch binding for a symbol returned by the current snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolBinding {
    pub(crate) session_generation: SessionGeneration,
    pub(crate) engine_snapshot_id: EngineSnapshotId,
    pub(crate) normalized_file_path: String,
    pub(crate) content_hash_or_node_id: String,
}

impl SymbolBinding {
    pub(crate) const fn new(
        session_generation: SessionGeneration,
        engine_snapshot_id: EngineSnapshotId,
        normalized_file_path: String,
        content_hash_or_node_id: String,
    ) -> Self {
        Self {
            session_generation,
            engine_snapshot_id,
            normalized_file_path,
            content_hash_or_node_id,
        }
    }

    /// Returns the session generation that produced the symbol.
    #[must_use]
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }

    /// Returns the engine snapshot that produced the symbol.
    #[must_use]
    pub const fn engine_snapshot_id(&self) -> EngineSnapshotId {
        self.engine_snapshot_id
    }

    /// Returns the normalized query file path.
    #[must_use]
    pub fn normalized_file_path(&self) -> &str {
        &self.normalized_file_path
    }

    /// Returns the Rust content hash of the query file.
    #[must_use]
    pub fn content_hash_or_node_id(&self) -> &str {
        &self.content_hash_or_node_id
    }

    fn is_current(
        &self,
        generation: SessionGeneration,
        snapshot: EngineSnapshotId,
        content_hash: &str,
    ) -> bool {
        self.session_generation == generation
            && self.engine_snapshot_id == snapshot
            && self.content_hash_or_node_id == content_hash
    }
}

/// Full epoch binding for a content-addressed symbol reference.
///
/// The reference's own `contentHash`, `parseOptionsKey`, `nodeId`, symbol id
/// and symbol kind are recorded here and compared again before every send, so
/// a mutated or cross-epoch descriptor is rejected locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolReferenceBinding {
    pub(crate) session_generation: SessionGeneration,
    pub(crate) engine_snapshot_id: EngineSnapshotId,
    pub(crate) normalized_file_path: String,
    pub(crate) content_hash_or_node_id: String,
    pub(crate) engine_content_hash: String,
    pub(crate) parse_options_key: String,
    pub(crate) node_id: String,
    pub(crate) symbol_id: u64,
    pub(crate) symbol_kind: u64,
}

impl SymbolReferenceBinding {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        session_generation: SessionGeneration,
        engine_snapshot_id: EngineSnapshotId,
        normalized_file_path: String,
        content_hash_or_node_id: String,
        engine_content_hash: String,
        parse_options_key: String,
        node_id: String,
        symbol_id: u64,
        symbol_kind: u64,
    ) -> Self {
        Self {
            session_generation,
            engine_snapshot_id,
            normalized_file_path,
            content_hash_or_node_id,
            engine_content_hash,
            parse_options_key,
            node_id,
            symbol_id,
            symbol_kind,
        }
    }

    /// Returns the session generation that produced the reference.
    #[must_use]
    pub const fn session_generation(&self) -> SessionGeneration {
        self.session_generation
    }

    /// Returns the engine snapshot that produced the reference.
    #[must_use]
    pub const fn engine_snapshot_id(&self) -> EngineSnapshotId {
        self.engine_snapshot_id
    }

    /// Returns the normalized file path of the referenced source file.
    #[must_use]
    pub fn normalized_file_path(&self) -> &str {
        &self.normalized_file_path
    }

    /// Returns the Rust content hash recorded at bind time.
    #[must_use]
    pub fn content_hash_or_node_id(&self) -> &str {
        &self.content_hash_or_node_id
    }

    fn is_current(
        &self,
        generation: SessionGeneration,
        snapshot: EngineSnapshotId,
        content_hash: &str,
        reference: &RawSymbolReference,
    ) -> bool {
        self.session_generation == generation
            && self.engine_snapshot_id == snapshot
            && self.content_hash_or_node_id == content_hash
            && self.engine_content_hash == reference.file.content_hash
            && self.parse_options_key == reference.file.parse_options_key
            && self.node_id == reference.file.node_id
            && self.symbol_id == reference.id
            && self.symbol_kind == reference.kind
    }
}

/// A symbol reference whose epoch, file and descriptor identity were bound by
/// the current session. Only this type may be sent to documentation calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSymbolReference {
    reference: RawSymbolReference,
    binding: SymbolReferenceBinding,
}

impl BoundSymbolReference {
    #[must_use]
    pub(crate) const fn new(
        reference: RawSymbolReference,
        binding: SymbolReferenceBinding,
    ) -> Self {
        Self { reference, binding }
    }

    /// Returns the underlying wire descriptor.
    #[must_use]
    pub const fn reference(&self) -> &RawSymbolReference {
        &self.reference
    }

    /// Returns the recorded epoch binding.
    #[must_use]
    pub const fn binding(&self) -> &SymbolReferenceBinding {
        &self.binding
    }

    /// Returns whether the binding still matches the supplied identity.
    #[must_use]
    pub fn is_current(
        &self,
        generation: SessionGeneration,
        snapshot: EngineSnapshotId,
        content_hash: &str,
    ) -> bool {
        self.binding
            .is_current(generation, snapshot, content_hash, &self.reference)
    }
}

/// A symbol whose query file, declarations and reference were bound by the
/// current session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSymbol {
    name: String,
    flags: u64,
    declarations: Vec<BoundNodeHandle>,
    value_declaration: Option<BoundNodeHandle>,
    reference: Option<BoundSymbolReference>,
    dropped_declarations: usize,
    reference_dropped: bool,
    binding: SymbolBinding,
}

impl BoundSymbol {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        name: String,
        flags: u64,
        declarations: Vec<BoundNodeHandle>,
        value_declaration: Option<BoundNodeHandle>,
        reference: Option<BoundSymbolReference>,
        dropped_declarations: usize,
        reference_dropped: bool,
        binding: SymbolBinding,
    ) -> Self {
        Self {
            name,
            flags,
            declarations,
            value_declaration,
            reference,
            dropped_declarations,
            reference_dropped,
            binding,
        }
    }

    /// Returns the symbol name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the native symbol flags.
    #[must_use]
    pub const fn flags(&self) -> u64 {
        self.flags
    }

    /// Returns the bound declarations.
    #[must_use]
    pub fn declarations(&self) -> &[BoundNodeHandle] {
        &self.declarations
    }

    /// Returns the bound value declaration.
    #[must_use]
    pub const fn value_declaration(&self) -> Option<&BoundNodeHandle> {
        self.value_declaration.as_ref()
    }

    /// Returns the bound content-addressed reference.
    #[must_use]
    pub const fn reference(&self) -> Option<&BoundSymbolReference> {
        self.reference.as_ref()
    }

    /// Returns how many declaration handles could not be bound.
    #[must_use]
    pub const fn dropped_declarations(&self) -> usize {
        self.dropped_declarations
    }

    /// Returns whether the symbol reference could not be bound.
    #[must_use]
    pub const fn reference_dropped(&self) -> bool {
        self.reference_dropped
    }

    /// Returns the query-file epoch binding.
    #[must_use]
    pub const fn binding(&self) -> &SymbolBinding {
        &self.binding
    }

    /// Returns whether the query-file binding is still current.
    #[must_use]
    pub fn is_current(
        &self,
        generation: SessionGeneration,
        snapshot: EngineSnapshotId,
        content_hash: &str,
    ) -> bool {
        self.binding.is_current(generation, snapshot, content_hash)
    }
}

/// Handle parsing or validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HandleError {
    /// The handle does not follow `<index>.<kind>.<path>`.
    #[error("invalid node handle: {reason}")]
    Malformed {
        /// Static rejection reason.
        reason: &'static str,
    },
}

pub(crate) fn validate_absolute_path(path: &str) -> Result<(), HandleError> {
    if !path.starts_with('/') {
        return Err(HandleError::Malformed {
            reason: "node path is not absolute",
        });
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(HandleError::Malformed {
            reason: "node path contains a forbidden character",
        });
    }
    if path
        .split('/')
        .skip(1)
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(HandleError::Malformed {
            reason: "node path has an invalid component",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> HandleBinding {
        HandleBinding {
            session_generation: SessionGeneration::new(1),
            engine_snapshot_id: EngineSnapshotId::new(3),
            normalized_file_path: "/work/main.ts".to_owned(),
            content_hash_or_node_id: "hash".to_owned(),
            node_index: 7,
            node_kind: 79,
        }
    }

    #[test]
    fn parses_handle_segments_with_dots_in_the_path() {
        let handle = NodeHandle::parse("7.79./work/a.b/main.ts").expect("valid handle");
        assert_eq!(handle.encoder_index, 7);
        assert_eq!(handle.kind, 79);
        assert_eq!(handle.path, "/work/a.b/main.ts");
        assert_eq!(handle.as_wire(), "7.79./work/a.b/main.ts");
    }

    #[test]
    fn rejects_malformed_handles() {
        for value in [
            "",
            "7",
            "7.79",
            "x.79./work/main.ts",
            "7.x./work/main.ts",
            "7.79.relative/main.ts",
            "7.79./work/../main.ts",
            "7.79./work//main.ts",
            "7.79./work\\main.ts",
        ] {
            assert!(
                NodeHandle::parse(value).is_err(),
                "{value:?} must be rejected"
            );
        }
    }

    #[test]
    fn binding_matches_only_the_same_epoch_and_content() {
        let bound = BoundNodeHandle::new(
            NodeHandle::parse("7.79./work/main.ts").expect("handle"),
            binding(),
        );
        assert!(bound.is_current(SessionGeneration::new(1), EngineSnapshotId::new(3), "hash"));
        assert!(!bound.is_current(SessionGeneration::new(1), EngineSnapshotId::new(4), "hash"));
        assert!(!bound.is_current(SessionGeneration::new(2), EngineSnapshotId::new(3), "hash"));
        assert!(!bound.is_current(SessionGeneration::new(1), EngineSnapshotId::new(3), "other"));
    }

    #[test]
    fn session_generations_are_process_wide_and_distinct() {
        let first = SessionGeneration::next();
        let second = SessionGeneration::next();
        assert!(second > first);
    }

    fn reference() -> RawSymbolReference {
        RawSymbolReference {
            id: 42,
            kind: 7,
            file: crate::native::wire::RawSourceFileDescriptor {
                file_name: "/work/main.ts".to_owned(),
                path: "/work/main.ts".to_owned(),
                content_hash: "engine-hash".to_owned(),
                parse_options_key: "0".to_owned(),
                script_kind: 3,
                node_id: "9".to_owned(),
            },
        }
    }

    fn reference_binding() -> SymbolReferenceBinding {
        SymbolReferenceBinding::new(
            SessionGeneration::new(1),
            EngineSnapshotId::new(3),
            "/work/main.ts".to_owned(),
            "rust-hash".to_owned(),
            "engine-hash".to_owned(),
            "0".to_owned(),
            "9".to_owned(),
            42,
            7,
        )
    }

    #[test]
    fn symbol_reference_binding_rejects_any_identity_mismatch() {
        let mut reference = reference();
        let bound = BoundSymbolReference::new(reference.clone(), reference_binding());
        assert!(bound.is_current(
            SessionGeneration::new(1),
            EngineSnapshotId::new(3),
            "rust-hash"
        ));

        reference.kind = 8;
        let mutated = BoundSymbolReference::new(reference, reference_binding());
        assert!(!mutated.is_current(
            SessionGeneration::new(1),
            EngineSnapshotId::new(3),
            "rust-hash"
        ));

        assert!(!bound.is_current(
            SessionGeneration::new(2),
            EngineSnapshotId::new(3),
            "rust-hash"
        ));
        assert!(!bound.is_current(
            SessionGeneration::new(1),
            EngineSnapshotId::new(4),
            "rust-hash"
        ));
        assert!(!bound.is_current(SessionGeneration::new(1), EngineSnapshotId::new(3), "other"));
    }
}
