//! Symbol query presentation.
//!
//! Symbol evidence only leaves the native session through [`BoundSymbol`],
//! which carries epoch-bound declaration handles and a bound reference. This
//! module never exposes a raw wire handle as a supported symbol.

use wxml_types::CapabilityStatus;

use crate::native::handles::{BoundNodeHandle, BoundSymbol, BoundSymbolReference};

/// One symbol answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolInfo {
    /// Symbol name.
    pub name: String,
    /// Native symbol flags.
    pub flags: u64,
    /// Bound declaration node handles.
    pub declarations: Vec<BoundNodeHandle>,
    /// Bound value-declaration node handle.
    pub value_declaration: Option<BoundNodeHandle>,
    /// Bound content-addressed reference descriptor.
    pub reference: Option<BoundSymbolReference>,
}

/// Symbol query result.
///
/// `symbol: None` plus [`CapabilityStatus::Supported`] means "no symbol at this
/// position"; it is not a failed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolResult {
    /// Symbol evidence, when one exists.
    pub symbol: Option<SymbolInfo>,
    /// Completeness of the answer.
    pub status: CapabilityStatus,
}

/// Maps a bound symbol response.
///
/// `Supported` requires every declaration and the reference, when present, to
/// have been bound to the current epoch. `Partial` means the symbol exists but
/// some handles could not be bound and were dropped.
#[must_use]
pub fn map_symbol(raw: Option<BoundSymbol>) -> SymbolResult {
    let Some(symbol) = raw else {
        return SymbolResult {
            symbol: None,
            status: CapabilityStatus::Supported,
        };
    };

    let status = if symbol.dropped_declarations() == 0 && !symbol.reference_dropped() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: "some symbol handles could not be bound to the current snapshot".to_owned(),
        }
    };

    SymbolResult {
        symbol: Some(SymbolInfo {
            name: symbol.name().to_owned(),
            flags: symbol.flags(),
            declarations: symbol.declarations().to_vec(),
            value_declaration: symbol.value_declaration().cloned(),
            reference: symbol.reference().cloned(),
        }),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::handles::{
        EngineSnapshotId, HandleBinding, NodeHandle, SessionGeneration, SymbolBinding,
    };
    use crate::native::wire::{RawSourceFileDescriptor, RawSymbolReference};

    fn handle(index: u32) -> BoundNodeHandle {
        BoundNodeHandle::new(
            NodeHandle::parse(&format!("{index}.79./work/main.ts")).expect("handle"),
            HandleBinding {
                session_generation: SessionGeneration::new(1),
                engine_snapshot_id: EngineSnapshotId::new(1),
                normalized_file_path: "/work/main.ts".to_owned(),
                content_hash_or_node_id: "hash".to_owned(),
                node_index: index,
                node_kind: 79,
            },
        )
    }

    fn binding() -> SymbolBinding {
        SymbolBinding::new(
            SessionGeneration::new(1),
            EngineSnapshotId::new(1),
            "/work/main.ts".to_owned(),
            "hash".to_owned(),
        )
    }

    fn symbol(dropped: usize, reference_dropped: bool) -> BoundSymbol {
        BoundSymbol::new(
            "u".to_owned(),
            2,
            vec![handle(15)],
            None,
            Some(BoundSymbolReference::new(
                RawSymbolReference {
                    id: 7,
                    kind: 0,
                    file: RawSourceFileDescriptor {
                        file_name: "/work/main.ts".to_owned(),
                        path: "/work/main.ts".to_owned(),
                        content_hash: "engine-hash".to_owned(),
                        parse_options_key: "0".to_owned(),
                        script_kind: 3,
                        node_id: "1".to_owned(),
                    },
                },
                crate::native::handles::SymbolReferenceBinding::new(
                    SessionGeneration::new(1),
                    EngineSnapshotId::new(1),
                    "/work/main.ts".to_owned(),
                    "hash".to_owned(),
                    "engine-hash".to_owned(),
                    "0".to_owned(),
                    "1".to_owned(),
                    7,
                    0,
                ),
            )),
            dropped,
            reference_dropped,
            binding(),
        )
    }

    #[test]
    fn null_is_no_symbol_but_supported() {
        let result = map_symbol(None);
        assert!(result.symbol.is_none());
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn bound_handles_are_supported() {
        let result = map_symbol(Some(symbol(0, false)));
        assert_eq!(result.status, CapabilityStatus::Supported);
        let info = result.symbol.expect("symbol");
        assert_eq!(info.name, "u");
        assert_eq!(info.declarations.len(), 1);
        assert_eq!(info.declarations[0].handle().encoder_index, 15);
        assert!(info.reference.is_some());
    }

    #[test]
    fn dropped_handles_make_the_symbol_partial() {
        let result = map_symbol(Some(symbol(1, false)));
        let CapabilityStatus::Partial { reason } = &result.status else {
            panic!("expected partial");
        };
        assert!(!reason.contains("outside the current snapshot"));
        assert!(reason.contains("could not be bound"));
        let result = map_symbol(Some(symbol(0, true)));
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }
}
