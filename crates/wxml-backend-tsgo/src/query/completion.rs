//! Completion query DTOs and native mapping.
//!
//! Native completion details and auto-imports are not part of the frozen v1
//! contract. Entries keep the native kind and sort text; details and
//! replacement edit plans remain absent. A non-empty list is therefore
//! `Partial` until details are contracted. A `null` response means "no
//! completions here" and maps to a supported empty result; a transport or
//! engine failure is propagated instead.

use wxml_types::{CapabilityStatus, MAX_COMPLETION_ITEMS};

use crate::input::ProjectKey;
use crate::native::strings::Utf16Position;
use crate::native::wire::RawCompletionList;
use crate::shadow::mapping::SourceRange;

/// Returns the shared C0 completion cap (`wxml_types::MAX_COMPLETION_ITEMS`).
#[must_use]
pub const fn completion_limit() -> usize {
    MAX_COMPLETION_ITEMS
}

/// Completion request in native coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
    /// Native UTF-16 position.
    pub position: Utf16Position,
}

/// One completion entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEntry {
    /// Display label.
    pub label: String,
    /// Optional type/detail text. Always `None` until details are contracted.
    pub detail: Option<String>,
    /// Replacement range when it maps to original text.
    pub replacement: Option<SourceRange>,
    /// Native completion kind.
    pub native_kind: i64,
    /// Native sort text.
    pub sort_text: String,
}

/// Completion result.
///
/// Empty entries mean "checked, nothing to suggest" only when [`Self::status`]
/// is [`CapabilityStatus::Supported`]. [`Self::truncated`] is `true` when the
/// shared cap cut the candidate list short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    /// Entries in native order.
    pub entries: Vec<CompletionEntry>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
    /// Whether the shared C0 cap cut the candidate list short.
    pub truncated: bool,
}

/// Maps one raw completion response.
///
/// Native `null` means "no completions here" and is a supported empty answer.
/// Entry details are not part of the frozen contract, so a non-empty list is
/// always `Partial` until details are contracted.
#[must_use]
pub fn map_completions(raw: Option<&RawCompletionList>) -> CompletionResult {
    let Some(raw) = raw else {
        return CompletionResult {
            entries: Vec::new(),
            status: CapabilityStatus::Supported,
            truncated: false,
        };
    };

    let truncated = raw.entries.len() > MAX_COMPLETION_ITEMS;
    let entries: Vec<CompletionEntry> = raw
        .entries
        .iter()
        .take(MAX_COMPLETION_ITEMS)
        .map(|entry| CompletionEntry {
            label: entry.name.clone(),
            detail: None,
            replacement: None,
            native_kind: entry.kind,
            sort_text: entry.sort_text.clone(),
        })
        .collect();

    let unknown_kind = entries.iter().any(|entry| entry.native_kind != 5);
    let status = if raw.is_incomplete || truncated {
        CapabilityStatus::Partial {
            reason: "completion list is incomplete or truncated".to_owned(),
        }
    } else if entries.is_empty() {
        CapabilityStatus::Supported
    } else if unknown_kind {
        CapabilityStatus::Partial {
            reason: "completion entries use kinds outside the reviewed member kind".to_owned(),
        }
    } else {
        CapabilityStatus::Partial {
            reason: "completion entry details are not contracted".to_owned(),
        }
    };

    CompletionResult {
        entries,
        status,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::wire::{RawCompletionEntry, RawCompletionList};

    fn list(entries: Vec<RawCompletionEntry>, incomplete: bool) -> RawCompletionList {
        RawCompletionList {
            is_incomplete: incomplete,
            entries,
        }
    }

    fn entry(name: &str, kind: i64) -> RawCompletionEntry {
        RawCompletionEntry {
            name: name.to_owned(),
            kind,
            sort_text: "11".to_owned(),
        }
    }

    #[test]
    fn completion_cap_uses_the_shared_c0_limit() {
        assert_eq!(completion_limit(), 256);
        assert_eq!(completion_limit(), MAX_COMPLETION_ITEMS);
    }

    #[test]
    fn maps_member_entries_and_keeps_native_order() {
        let raw = list(vec![entry("name", 5), entry("age", 5)], false);
        let result = map_completions(Some(&raw));
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
        assert!(!result.truncated);
        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            vec!["name", "age"]
        );
        assert_eq!(result.entries[0].sort_text, "11");
    }

    #[test]
    fn null_and_unknown_kinds_are_not_fake_success() {
        assert_eq!(map_completions(None).status, CapabilityStatus::Supported);
        let raw = list(vec![entry("const", 21)], false);
        assert!(matches!(
            map_completions(Some(&raw)).status,
            CapabilityStatus::Partial { .. }
        ));
    }

    #[test]
    fn a_complete_empty_list_is_supported() {
        let raw = list(Vec::new(), false);
        let result = map_completions(Some(&raw));
        assert_eq!(result.entries, Vec::new());
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(!result.truncated);
    }

    #[test]
    fn truncation_marks_partial() {
        let entries = (0..=MAX_COMPLETION_ITEMS)
            .map(|index| entry(&format!("e{index}"), 5))
            .collect();
        let raw = list(entries, false);
        let result = map_completions(Some(&raw));
        assert!(result.truncated);
        assert_eq!(result.entries.len(), MAX_COMPLETION_ITEMS);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }
}
