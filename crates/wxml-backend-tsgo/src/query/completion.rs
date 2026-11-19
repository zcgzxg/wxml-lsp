//! Completion query DTOs and native mapping.
//!
//! The pinned 7.1.0-beta API exposes one completion method,
//! `getCompletionsAtPosition`. Its emitted entries carry only `name`, `kind`
//! (an LSP `CompletionItemKind` number) and `sortText`; optional `detail` /
//! `labelDetails` fields are tolerated when present in a future pinned
//! artifact but are not expected from 7.1.0-beta. The API has no
//! `getCompletionEntryDetails`, no per-entry documentation field and no
//! display parts. Auto-import entries require `includeSymbol:true` plus a
//! snapshot created with `prepareAutoImports`; this adapter creates snapshots
//! without that parameter and does not request symbols. Non-empty results
//! therefore stay `Partial` with typed
//! [`CompletionGap::DocumentationUnavailable`] and
//! [`CompletionGap::AutoImportsUnavailable`] gaps. A `null` response is a
//! response gap and maps to an empty `Partial` result with
//! [`CompletionGap::NoResponse`]; only an explicit empty list is a checked
//! empty `Supported` answer. A transport or engine failure is propagated
//! instead.

use wxml_types::{CapabilityStatus, CompletionItemKind, MAX_COMPLETION_ITEMS};

use crate::input::ProjectKey;
use crate::native::strings::Utf16Position;
use crate::native::wire::RawCompletionList;

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

/// A reason a native completion result is not complete.
///
/// The first four variants describe the response itself. The last two are
/// stable capability gaps of the pinned v1 completion path: the API has no
/// per-entry documentation, and auto-imports need a snapshot created with
/// `prepareAutoImports`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionGap {
    /// The engine answered `null` instead of a completion list, so the result
    /// is not a checked empty answer.
    NoResponse,
    /// The engine marked the list incomplete.
    IncompleteList,
    /// The shared completion cap cut the candidate list short.
    Truncated,
    /// At least one entry kind is absent (`0`) or outside the pinned LSP enum.
    UnknownKind,
    /// The pinned completion API exposes no per-entry documentation.
    DocumentationUnavailable,
    /// Auto-import entries require `includeSymbol:true` and a snapshot created
    /// with `prepareAutoImports`.
    AutoImportsUnavailable,
}

impl CompletionGap {
    /// Returns the stable human-readable reason for this gap.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NoResponse => "the engine returned null instead of a completion list",
            Self::IncompleteList => "the engine marked the completion list incomplete",
            Self::Truncated => "the shared completion item cap truncated the list",
            Self::UnknownKind => {
                "completion entries have an absent or unknown kind outside the pinned LSP CompletionItemKind enum"
            }
            Self::DocumentationUnavailable => {
                "the pinned completion API exposes no per-entry documentation"
            }
            Self::AutoImportsUnavailable => {
                "auto-import completion entries require a snapshot created with prepareAutoImports"
            }
        }
    }
}

/// One completion entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEntry {
    /// Display label.
    pub label: String,
    /// Native type/signature detail when present; the pinned 7.1.0-beta
    /// artifact emits none.
    pub detail: Option<String>,
    /// Native LSP `CompletionItemKind` number.
    pub native_kind: i64,
    /// Presentation kind mapped from the pinned LSP enum.
    pub kind: CompletionItemKind,
    /// Native sort text.
    pub sort_text: String,
}

/// Completion result.
///
/// Empty entries mean "checked, nothing to suggest" only when [`Self::status`]
/// is [`CapabilityStatus::Supported`]; an empty `Partial` result (for example
/// [`CompletionGap::NoResponse`]) is not a checked answer. [`Self::truncated`]
/// is `true` when the shared cap cut the candidate list short. [`Self::gaps`]
/// is the typed reason behind a `Partial` status and is empty for `Supported`
/// results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    /// Entries in native order.
    pub entries: Vec<CompletionEntry>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
    /// Typed reasons the result is partial.
    pub gaps: Vec<CompletionGap>,
    /// Whether the shared C0 cap cut the candidate list short.
    pub truncated: bool,
}

/// Maps one raw completion response.
///
/// Native `null` is not a checked answer: it maps to an empty `Partial`
/// result with [`CompletionGap::NoResponse`], never to a supported empty list.
/// A list keeps the native order and `sortText`, maps the pinned LSP kind
/// enum, and preserves native `detail` / `labelDetails.detail` when a future
/// artifact emits them. The pinned API has no documentation and this path does
/// not request auto-imports, so non-empty results stay `Partial` with explicit
/// [`CompletionGap`]s.
#[must_use]
pub fn map_completions(raw: Option<&RawCompletionList>) -> CompletionResult {
    let Some(raw) = raw else {
        let gaps = vec![CompletionGap::NoResponse];
        return CompletionResult {
            entries: Vec::new(),
            status: status_from(&gaps),
            gaps,
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
            detail: entry
                .detail
                .clone()
                .or_else(|| entry.label_details.as_ref()?.detail.clone()),
            native_kind: entry.kind,
            kind: map_kind(entry.kind).unwrap_or(CompletionItemKind::Value),
            sort_text: entry.sort_text.clone(),
        })
        .collect();

    let mut gaps = Vec::new();
    if raw.is_incomplete {
        gaps.push(CompletionGap::IncompleteList);
    }
    if truncated {
        gaps.push(CompletionGap::Truncated);
    }
    if entries
        .iter()
        .any(|entry| map_kind(entry.native_kind).is_none())
    {
        gaps.push(CompletionGap::UnknownKind);
    }
    if !entries.is_empty() {
        gaps.push(CompletionGap::DocumentationUnavailable);
        gaps.push(CompletionGap::AutoImportsUnavailable);
    }
    let status = status_from(&gaps);

    CompletionResult {
        entries,
        status,
        gaps,
        truncated,
    }
}

/// Maps a pinned LSP `CompletionItemKind` number to the C0 presentation kind.
///
/// `None` means the number is absent (`0`) or outside the pinned `1..=25`
/// enum; callers map it to plain text and report
/// [`CompletionGap::UnknownKind`].
const fn map_kind(native_kind: i64) -> Option<CompletionItemKind> {
    match native_kind {
        // LSP Method, Function, Constructor.
        2..=4 => Some(CompletionItemKind::Method),
        // LSP Field, Property.
        5 | 10 => Some(CompletionItemKind::Property),
        // LSP Keyword.
        14 => Some(CompletionItemKind::Keyword),
        // LSP Event.
        23 => Some(CompletionItemKind::Event),
        // Remaining pinned kinds are plain text in a WXML member list.
        1 | 6..=9 | 11..=13 | 15..=22 | 24 | 25 => Some(CompletionItemKind::Value),
        _ => None,
    }
}

fn status_from(gaps: &[CompletionGap]) -> CapabilityStatus {
    if gaps.is_empty() {
        return CapabilityStatus::Supported;
    }
    let reason = gaps
        .iter()
        .map(|gap| gap.reason())
        .collect::<Vec<_>>()
        .join("; ");
    CapabilityStatus::Partial { reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::wire::{RawCompletionEntry, RawCompletionLabelDetails, RawCompletionList};
    use wxml_types::CompletionItemKind;

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
            detail: None,
            label_details: None,
        }
    }

    #[test]
    fn completion_cap_uses_the_shared_c0_limit() {
        assert_eq!(completion_limit(), 256);
        assert_eq!(completion_limit(), MAX_COMPLETION_ITEMS);
    }

    #[test]
    fn maps_the_pinned_lsp_completion_item_kinds() {
        for (native, expected) in [
            (1, CompletionItemKind::Value),
            (2, CompletionItemKind::Method),
            (3, CompletionItemKind::Method),
            (4, CompletionItemKind::Method),
            (5, CompletionItemKind::Property),
            (6, CompletionItemKind::Value),
            (10, CompletionItemKind::Property),
            (14, CompletionItemKind::Keyword),
            (20, CompletionItemKind::Value),
            (21, CompletionItemKind::Value),
            (23, CompletionItemKind::Event),
            (25, CompletionItemKind::Value),
        ] {
            let raw = list(vec![entry("member", native)], false);
            let result = map_completions(Some(&raw));
            assert_eq!(
                result.entries[0].kind, expected,
                "native LSP kind {native} must map to {expected:?}"
            );
            assert!(
                !result.gaps.contains(&CompletionGap::UnknownKind),
                "native LSP kind {native} is part of the pinned enum"
            );
        }
    }

    #[test]
    fn unknown_kind_is_a_plain_value_with_an_explicit_gap() {
        for native in [0, 99] {
            let raw = list(vec![entry("member", native)], false);
            let result = map_completions(Some(&raw));
            assert_eq!(result.entries[0].kind, CompletionItemKind::Value);
            assert!(result.gaps.contains(&CompletionGap::UnknownKind));
            assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
        }
    }

    #[test]
    fn tolerates_native_detail_and_label_detail_fields() {
        let with_detail = RawCompletionEntry {
            detail: Some("name: string".to_owned()),
            ..entry("name", 5)
        };
        let with_label_detail = RawCompletionEntry {
            label_details: Some(RawCompletionLabelDetails {
                detail: Some("(value: string): void".to_owned()),
                description: None,
            }),
            ..entry("setName", 2)
        };
        let raw = list(vec![with_detail, with_label_detail], false);
        let result = map_completions(Some(&raw));
        assert_eq!(result.entries[0].detail.as_deref(), Some("name: string"));
        assert_eq!(
            result.entries[1].detail.as_deref(),
            Some("(value: string): void")
        );
    }

    #[test]
    fn a_non_empty_list_reports_documentation_and_auto_import_gaps() {
        let raw = list(vec![entry("name", 5)], false);
        let result = map_completions(Some(&raw));
        assert_eq!(
            result.gaps,
            vec![
                CompletionGap::DocumentationUnavailable,
                CompletionGap::AutoImportsUnavailable,
            ]
        );
        let reason = result.status.reason().expect("partial reason");
        assert!(reason.contains("documentation"), "{reason}");
        assert!(reason.contains("auto-import"), "{reason}");
    }

    #[test]
    fn a_checked_empty_list_has_no_gaps() {
        let raw = list(Vec::new(), false);
        let result = map_completions(Some(&raw));
        assert_eq!(result.entries, Vec::new());
        assert_eq!(result.gaps, Vec::new());
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn an_incomplete_empty_list_is_partial_but_has_no_entry_gaps() {
        let raw = list(Vec::new(), true);
        let result = map_completions(Some(&raw));
        assert_eq!(result.entries, Vec::new());
        assert_eq!(result.gaps, vec![CompletionGap::IncompleteList]);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
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
    fn null_is_a_partial_no_response_answer() {
        let result = map_completions(None);
        assert_eq!(result.entries, Vec::new());
        assert_eq!(result.gaps, vec![CompletionGap::NoResponse]);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
        let reason = result.status.reason().expect("partial reason");
        assert!(reason.contains("null"), "{reason}");
        assert!(!result.truncated);
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
        assert_eq!(
            result.gaps,
            vec![
                CompletionGap::Truncated,
                CompletionGap::DocumentationUnavailable,
                CompletionGap::AutoImportsUnavailable,
            ]
        );
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }
}
