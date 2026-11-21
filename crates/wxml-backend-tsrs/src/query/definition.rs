//! Definition presentation from native referenced-symbol evidence.
//!
//! Definition has no direct native API. The native session resolves the
//! position to an Identifier, calls `getReferencedSymbolsForNode`, and decodes
//! every declaration handle into the declaration's name token in the same AST
//! snapshot. This module maps that evidence to the internal DTO and keeps
//! incompleteness explicit through [`CapabilityStatus`].
//!
//! Empty targets mean "checked, no definition" only when [`DefinitionResult::status`]
//! is [`CapabilityStatus::Supported`]. Empty plus any degraded status means the
//! backend could not answer or could not decode a candidate.

use wxml_types::CapabilityStatus;

use crate::native::session::{DefinitionEvidence, DefinitionTargetEvidence};
use crate::script::symbols::Utf16Range;
use crate::shadow::mapping::SourceRange;

/// One definition target in original coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionTarget {
    /// Normalized project-relative target path.
    pub path: String,
    /// Original UTF-8 byte range of the identifier token.
    pub range: SourceRange,
    /// The same token range in UTF-16 code units.
    pub utf16: Utf16Range,
}

/// Definition result.
///
/// Empty targets mean "checked, no definition" only when [`Self::status`] is
/// [`CapabilityStatus::Supported`]. Empty plus any degraded status means the
/// backend could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionResult {
    /// Targets that mapped back to real source text.
    pub targets: Vec<DefinitionTarget>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
    /// Whether the shared target cap cut the candidate list short.
    pub truncated: bool,
}

/// Maps decoded native definition evidence.
///
/// `Supported` requires a symbol and Identifier at the position, at least one
/// referenced-symbol group, and every declaration candidate to decode into a
/// verified source token. A missing symbol is a checked empty answer. A symbol
/// without an Identifier node is `Partial`, because a quoted member access or
/// another non-Identifier node cannot prove "no definition". Any decode
/// failure, invalid range or truncation is `Partial`; successfully decoded
/// targets are preserved.
#[must_use]
pub fn map_definition(raw: &DefinitionEvidence) -> DefinitionResult {
    if raw.symbol.is_none() {
        return DefinitionResult {
            targets: Vec::new(),
            status: CapabilityStatus::Supported,
            truncated: false,
        };
    }
    if raw.identifier.is_none() {
        // The position has a symbol but no Identifier node (for example a
        // quoted member access such as `obj["name"]`). Returning a Supported
        // empty answer would claim a checked "no definition"; the composition
        // cannot prove that.
        return DefinitionResult {
            targets: Vec::new(),
            status: CapabilityStatus::Partial {
                reason: "symbol has no Identifier node at the queried position".to_owned(),
            },
            truncated: false,
        };
    }
    if raw.groups == 0 {
        return DefinitionResult {
            targets: Vec::new(),
            status: CapabilityStatus::Partial {
                reason: "no referenced symbol evidence was returned for the identifier".to_owned(),
            },
            truncated: false,
        };
    }

    let mut targets = Vec::new();
    let mut rejected = false;
    for evidence in &raw.targets {
        if !valid_target(evidence) {
            rejected = true;
            continue;
        }
        let target = DefinitionTarget {
            path: evidence.path.clone(),
            range: SourceRange::new(evidence.start_utf8.get(), evidence.end_utf8.get()),
            utf16: Utf16Range::new(evidence.start_utf16.get(), evidence.end_utf16.get()),
        };
        if !targets.contains(&target) {
            targets.push(target);
        }
    }

    let truncated = raw.truncated;

    let status = if raw.decode_failures > 0 || rejected {
        CapabilityStatus::Partial {
            reason: "some definition declarations could not be decoded".to_owned(),
        }
    } else if truncated {
        CapabilityStatus::Partial {
            reason: "definition target list was truncated".to_owned(),
        }
    } else if targets.is_empty() {
        CapabilityStatus::Partial {
            reason: "no definition target was decoded for the identifier".to_owned(),
        }
    } else {
        CapabilityStatus::Supported
    };

    DefinitionResult {
        targets,
        status,
        truncated,
    }
}

fn valid_target(target: &DefinitionTargetEvidence) -> bool {
    !target.path.is_empty()
        && target.start_utf16 < target.end_utf16
        && target.start_utf8 < target.end_utf8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::handles::{
        BoundNodeHandle, BoundSymbol, EngineSnapshotId, HandleBinding, NodeHandle,
        SessionGeneration, SymbolBinding,
    };
    use crate::native::strings::{Utf8Offset, Utf16Offset};

    fn generation() -> SessionGeneration {
        SessionGeneration::new(1)
    }

    fn snapshot() -> EngineSnapshotId {
        EngineSnapshotId::new(3)
    }

    fn handle(index: u32) -> BoundNodeHandle {
        BoundNodeHandle::new(
            NodeHandle::parse(&format!("{index}.79./work/main.ts")).expect("handle"),
            HandleBinding {
                session_generation: generation(),
                engine_snapshot_id: snapshot(),
                normalized_file_path: "/work/main.ts".to_owned(),
                content_hash_or_node_id: "hash".to_owned(),
                node_index: index,
                node_kind: 79,
            },
        )
    }

    fn symbol() -> BoundSymbol {
        BoundSymbol::new(
            "name".to_owned(),
            4,
            vec![handle(9)],
            None,
            None,
            0,
            false,
            SymbolBinding::new(
                generation(),
                snapshot(),
                "/work/main.ts".to_owned(),
                "hash".to_owned(),
            ),
        )
    }

    fn target(path: &str, start: u32, end: u32) -> DefinitionTargetEvidence {
        DefinitionTargetEvidence {
            path: path.to_owned(),
            start_utf16: Utf16Offset::new(start),
            end_utf16: Utf16Offset::new(end),
            start_utf8: Utf8Offset::new(start),
            end_utf8: Utf8Offset::new(end),
        }
    }

    fn evidence(targets: Vec<DefinitionTargetEvidence>) -> DefinitionEvidence {
        DefinitionEvidence {
            symbol: Some(symbol()),
            identifier: Some(handle(31)),
            targets,
            groups: 1,
            decode_failures: 0,
            truncated: false,
        }
    }

    #[test]
    fn missing_symbol_is_supported_empty_but_a_symbol_without_identifier_is_partial() {
        let no_symbol = DefinitionEvidence {
            symbol: None,
            identifier: Some(handle(31)),
            targets: vec![target("model.ts", 53, 57)],
            groups: 1,
            decode_failures: 0,
            truncated: false,
        };
        let result = map_definition(&no_symbol);
        assert_eq!(result.targets, Vec::<DefinitionTarget>::new());
        assert_eq!(result.status, CapabilityStatus::Supported);

        let no_identifier = DefinitionEvidence {
            symbol: Some(symbol()),
            identifier: None,
            targets: Vec::new(),
            groups: 0,
            decode_failures: 0,
            truncated: false,
        };
        let result = map_definition(&no_identifier);
        assert_eq!(result.targets, Vec::<DefinitionTarget>::new());
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn preserves_multiple_declarations_and_deduplicates_exact_targets() {
        let result = map_definition(&evidence(vec![
            target("model.ts", 53, 57),
            target("model.ts", 53, 57),
            target("other.ts", 10, 14),
        ]));
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert_eq!(result.targets.len(), 2);
        assert_eq!(result.targets[0].path, "model.ts");
        assert_eq!(result.targets[0].utf16.start.get(), 53);
        assert_eq!(result.targets[0].utf16.end.get(), 57);
        assert_eq!(result.targets[1].path, "other.ts");
    }

    #[test]
    fn decode_failures_and_invalid_ranges_make_the_result_partial() {
        let mut raw = evidence(vec![target("model.ts", 53, 57)]);
        raw.decode_failures = 1;
        let result = map_definition(&raw);
        assert_eq!(result.targets.len(), 1);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));

        let mut raw = evidence(vec![target("model.ts", 57, 53)]);
        raw.decode_failures = 0;
        let result = map_definition(&raw);
        assert_eq!(result.targets, Vec::<DefinitionTarget>::new());
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn no_groups_is_partial_not_a_supported_empty() {
        let mut raw = evidence(Vec::new());
        raw.groups = 0;
        let result = map_definition(&raw);
        assert_eq!(result.targets, Vec::<DefinitionTarget>::new());
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn groups_without_decoded_targets_are_partial() {
        let raw = evidence(Vec::new());
        let result = map_definition(&raw);
        assert_eq!(result.targets, Vec::<DefinitionTarget>::new());
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn session_truncation_flag_is_partial_and_keeps_every_decoded_target() {
        let targets = (0..4)
            .map(|index| {
                let start = index * 10;
                target("model.ts", start, start + 4)
            })
            .collect();
        let mut raw = evidence(targets);
        raw.truncated = true;
        let result = map_definition(&raw);
        assert_eq!(result.targets.len(), 4);
        assert!(result.truncated);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }
}
