//! Referenced-symbol presentation.
//!
//! The native API returns node handles, not source ranges. Decoding a handle
//! into a token range requires the same-snapshot AST, which this slice does not
//! decode. Therefore a non-empty result is explicitly `Partial`: the handles
//! are evidence for later mapping, never a publishable definition/rename
//! range. A `null` symbol, or an AST with no Identifier at the position, is a
//! genuine checked-empty result.

use wxml_types::CapabilityStatus;

use crate::native::session::ReferencesQuery;

/// Maximum reference handles retained across all groups.
pub const MAX_REFERENCE_HANDLES: usize = 1_000;

/// One definition/reference group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceGroup {
    /// Definition node handle.
    pub definition: String,
    /// Reference node handles.
    pub references: Vec<String>,
    /// Symbol name, when the engine included it.
    pub symbol_name: Option<String>,
}

/// Referenced-symbol result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencesResult {
    /// Groups returned by the engine.
    pub groups: Vec<ReferenceGroup>,
    /// Completeness of the answer.
    pub status: CapabilityStatus,
    /// Whether the handle cap cut the evidence short.
    pub truncated: bool,
}

/// Maps raw referenced-symbol evidence.
#[must_use]
pub fn map_references(raw: &ReferencesQuery) -> ReferencesResult {
    if raw.symbol.is_none() && raw.groups.is_empty() {
        return ReferencesResult {
            groups: Vec::new(),
            status: CapabilityStatus::Supported,
            truncated: false,
        };
    }
    if raw.identifier.is_none() {
        // The AST had no Identifier node at the queried position. This is a
        // checked empty answer, not a wrong-node query or a partial failure.
        return ReferencesResult {
            groups: Vec::new(),
            status: CapabilityStatus::Supported,
            truncated: false,
        };
    }
    if raw.groups.is_empty() {
        return ReferencesResult {
            groups: Vec::new(),
            status: CapabilityStatus::Partial {
                reason: "no reference evidence was returned for the identifier".to_owned(),
            },
            truncated: false,
        };
    }

    let mut groups = Vec::new();
    let mut remaining = MAX_REFERENCE_HANDLES;
    let mut truncated = false;
    for group in &raw.groups {
        if truncated {
            break;
        }
        let take = group.references.len().min(remaining);
        if take < group.references.len() {
            truncated = true;
        }
        remaining = remaining.saturating_sub(take);
        groups.push(ReferenceGroup {
            definition: group.definition.clone(),
            references: group.references.iter().take(take).cloned().collect(),
            symbol_name: group.symbol.as_ref().map(|symbol| symbol.name.clone()),
        });
    }

    ReferencesResult {
        groups,
        status: CapabilityStatus::Partial {
            reason: "reference handles are not decoded into source ranges yet".to_owned(),
        },
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::handles::{
        BoundNodeHandle, EngineSnapshotId, HandleBinding, NodeHandle, SessionGeneration,
    };
    use crate::native::wire::{RawReferencedSymbol, RawSymbol};

    fn identifier() -> BoundNodeHandle {
        BoundNodeHandle::new(
            NodeHandle::parse("7.79./work/main.ts").expect("handle"),
            HandleBinding {
                session_generation: SessionGeneration::new(1),
                engine_snapshot_id: EngineSnapshotId::new(3),
                normalized_file_path: "/work/main.ts".to_owned(),
                content_hash_or_node_id: "hash".to_owned(),
                node_index: 7,
                node_kind: 79,
            },
        )
    }

    fn group(name: &str, references: usize) -> RawReferencedSymbol {
        RawReferencedSymbol {
            definition: "1.79./work/main.ts".to_owned(),
            references: (0..references)
                .map(|index| format!("{}.79./work/main.ts", index + 2))
                .collect(),
            symbol: Some(RawSymbol {
                name: name.to_owned(),
                flags: 2,
                declarations: Vec::new(),
                value_declaration: None,
                reference: None,
            }),
        }
    }

    #[test]
    fn no_symbol_is_supported_empty() {
        let result = map_references(&ReferencesQuery {
            symbol: None,
            groups: Vec::new(),
            identifier: None,
        });
        assert_eq!(result.groups, Vec::<ReferenceGroup>::new());
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn references_are_partial_without_ast_ranges() {
        let result = map_references(&ReferencesQuery {
            symbol: None,
            groups: vec![group("u", 2)],
            identifier: Some(identifier()),
        });
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].references.len(), 2);
        assert_eq!(result.groups[0].symbol_name.as_deref(), Some("u"));
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn caps_reference_handles() {
        let result = map_references(&ReferencesQuery {
            symbol: None,
            groups: vec![group("u", MAX_REFERENCE_HANDLES + 10)],
            identifier: Some(identifier()),
        });
        assert!(result.truncated);
        assert_eq!(
            result
                .groups
                .iter()
                .map(|group| group.references.len())
                .sum::<usize>(),
            MAX_REFERENCE_HANDLES
        );
    }
}
