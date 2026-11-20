//! Completion query DTOs and native mapping.
//!
//! The pinned 7.1.0-beta API exposes one completion method,
//! `getCompletionsAtPosition`. Its emitted entries carry `name`, `kind` (an LSP
//! `CompletionItemKind` number), `sortText`, optional `insertText` /
//! `filterText` / `detail` / `labelDetails` and, when `includeSymbol:true` was
//! requested on a **prepared** snapshot, a `symbol`.
//!
//! Auto-import entries need both a snapshot created (or updated) with
//! `prepareAutoImports` and `includeSymbol:true`; an unprepared snapshot
//! rejects `includeSymbol:true` with an engine error. The response still has no
//! per-entry auto-import origin (`origin`/`source`/`data`/`isImportStatementCompletion`),
//! no `textEdit`, no `additionalTextEdits`, no `documentation`/`displayParts`
//! and no `getCompletionEntryDetails` method. A prepared query therefore stays
//! `Partial` with [`CompletionGap::AutoImportOriginUnavailable`]: candidates are
//! real engine entries, but none of them can be *marked* as an editable
//! auto-import, and this mapper never synthesizes an import statement.
//!
//! Non-empty unprepared results stay `Partial` with
//! [`CompletionGap::DocumentationUnavailable`] and
//! [`CompletionGap::AutoImportsUnavailable`]. A `null` response is a response
//! gap and maps to an empty `Partial` result with [`CompletionGap::NoResponse`];
//! only an explicit empty list is a checked empty `Supported` answer. A
//! transport or engine failure is propagated instead.

use wxml_types::{CapabilityStatus, CompletionItemKind, MAX_COMPLETION_ITEMS};

use crate::native::wire::RawCompletionList;

/// Whether the snapshot was prepared for auto-imports when the query ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AutoImportContext {
    /// Not prepared (or preparation failed), so no symbol evidence was
    /// requested.
    ///
    /// Auto-import candidates may still be returned through the engine's
    /// request-local lazy clone, but the pinned entry exposes no origin, so
    /// they cannot be identified either way.
    #[default]
    Unavailable,
    /// The snapshot was prepared for this file and `includeSymbol:true` was
    /// requested.
    ///
    /// The pinned entry still has no auto-import origin, so the result reports
    /// [`CompletionGap::AutoImportOriginUnavailable`] and no candidate becomes
    /// an editable auto-import.
    Prepared,
}

/// Options for one native completion request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompletionQueryOptions {
    /// Ask the engine to attach symbol evidence to entries.
    ///
    /// This is only safe on a snapshot prepared with `prepareAutoImports`; on
    /// an unprepared snapshot the engine returns an error instead of a list.
    pub include_symbol: bool,
}

impl CompletionQueryOptions {
    /// Options that request symbol evidence from a prepared snapshot.
    pub const SYMBOLS: Self = Self {
        include_symbol: true,
    };
}

/// A reason a native completion result is not complete.
///
/// The first four variants describe the response itself. The remaining
/// variants are stable capability gaps of the pinned v1 completion path: there
/// is no per-entry documentation, an unprepared snapshot has no prepared
/// auto-import index, a prepared entry still has no auto-import origin/import
/// edit, and snippet insert text cannot be published as plain text.
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
    /// Auto-import entries require a snapshot prepared with `prepareAutoImports`
    /// and `includeSymbol:true`.
    AutoImportsUnavailable,
    /// The snapshot was prepared, but the pinned entry has no auto-import
    /// origin or import edit, so no candidate can be marked as an editable
    /// auto-import.
    AutoImportOriginUnavailable,
    /// A completion `insertText` uses snippet syntax; it is not published as
    /// plain text and the entry falls back to its label.
    InsertTextFormatUnknown,
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
            Self::AutoImportOriginUnavailable => {
                "the pinned completion entry exposes no auto-import origin or import edit, so an auto-import candidate cannot be marked as editable"
            }
            Self::InsertTextFormatUnknown => {
                "a completion insertText uses snippet syntax and is published as plain label text"
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
    /// Native `insertText` when it is non-empty plain text.
    ///
    /// Snippet-shaped values are dropped (and reported as
    /// [`CompletionGap::InsertTextFormatUnknown`]) instead of being inserted
    /// literally.
    pub insert_text: Option<String>,
    /// Native `filterText` when present.
    pub filter_text: Option<String>,
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

/// Maps one raw completion response without auto-import preparation.
///
/// Native `null` is not a checked answer: it maps to an empty `Partial`
/// result with [`CompletionGap::NoResponse`], never to a supported empty list.
/// A list keeps the native order and `sortText`, maps the pinned LSP kind
/// enum, and preserves native `detail` / `labelDetails.detail` when a future
/// artifact emits them. Non-empty results stay `Partial` with explicit
/// [`CompletionGap`]s.
#[must_use]
pub fn map_completions(raw: Option<&RawCompletionList>) -> CompletionResult {
    map_completions_with(raw, AutoImportContext::Unavailable)
}

/// Maps one raw completion response with the auto-import context it was
/// requested under.
///
/// The context only changes the *status gap*: a prepared snapshot replaces
/// [`CompletionGap::AutoImportsUnavailable`] with
/// [`CompletionGap::AutoImportOriginUnavailable`]. No entry is ever marked as
/// an auto-import, and no import edit is invented, because the pinned entry has
/// no origin and no `textEdit`/`data`/`additionalTextEdits`.
#[must_use]
pub fn map_completions_with(
    raw: Option<&RawCompletionList>,
    auto_imports: AutoImportContext,
) -> CompletionResult {
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
    let mut snippet_insert_text = false;
    let entries: Vec<CompletionEntry> = raw
        .entries
        .iter()
        .take(MAX_COMPLETION_ITEMS)
        .map(|entry| {
            let insert_text = entry.insert_text.as_deref().and_then(|text| {
                if plain_insert_text(text) {
                    Some(text.to_owned())
                } else {
                    snippet_insert_text = true;
                    None
                }
            });
            CompletionEntry {
                label: entry.name.clone(),
                detail: entry
                    .detail
                    .clone()
                    .or_else(|| entry.label_details.as_ref()?.detail.clone()),
                native_kind: entry.kind,
                kind: map_kind(entry.kind).unwrap_or(CompletionItemKind::Value),
                sort_text: entry.sort_text.clone(),
                insert_text,
                filter_text: entry
                    .filter_text
                    .as_deref()
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned),
            }
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
    if snippet_insert_text {
        gaps.push(CompletionGap::InsertTextFormatUnknown);
    }
    if !entries.is_empty() {
        gaps.push(CompletionGap::DocumentationUnavailable);
        gaps.push(match auto_imports {
            AutoImportContext::Unavailable => CompletionGap::AutoImportsUnavailable,
            AutoImportContext::Prepared => CompletionGap::AutoImportOriginUnavailable,
        });
    }
    let status = status_from(&gaps);

    CompletionResult {
        entries,
        status,
        gaps,
        truncated,
    }
}

/// Returns whether `insertText` is literal text this mapper may publish.
///
/// Snippet-shaped values (`$1`, `${1:name}`) are rejected so they are never
/// inserted literally; the adapter falls back to the entry label instead.
fn plain_insert_text(text: &str) -> bool {
    !text.is_empty() && !text.contains('$')
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
            insert_text: None,
            filter_text: None,
            detail: None,
            label_details: None,
            symbol: None,
        }
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

    #[test]
    fn a_prepared_list_reports_origin_unavailable_instead_of_auto_imports_unavailable() {
        let raw = list(vec![entry("User", 7)], false);
        let result = map_completions_with(Some(&raw), AutoImportContext::Prepared);
        assert_eq!(
            result.gaps,
            vec![
                CompletionGap::DocumentationUnavailable,
                CompletionGap::AutoImportOriginUnavailable,
            ]
        );
        assert!(!result.gaps.contains(&CompletionGap::AutoImportsUnavailable));
        let reason = result.status.reason().expect("partial reason");
        assert!(reason.contains("auto-import origin"), "{reason}");
        assert!(reason.contains("documentation"), "{reason}");
        // No per-entry auto-import marker exists in the DTO surface.
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].label, "User");
    }

    #[test]
    fn a_prepared_checked_empty_list_is_still_supported() {
        let raw = list(Vec::new(), false);
        let result = map_completions_with(Some(&raw), AutoImportContext::Prepared);
        assert_eq!(result.entries, Vec::new());
        assert_eq!(result.gaps, Vec::new());
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn a_prepared_null_response_is_still_no_response() {
        let result = map_completions_with(None, AutoImportContext::Prepared);
        assert_eq!(result.gaps, vec![CompletionGap::NoResponse]);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn completion_maps_plain_insert_and_filter_text_but_rejects_snippets() {
        let plain = RawCompletionEntry {
            insert_text: Some("User".to_owned()),
            filter_text: Some("User".to_owned()),
            ..entry("User", 7)
        };
        let snippet_text = String::from_utf8(b"User${1:name}".to_vec()).expect("ascii snippet");
        let snippet = RawCompletionEntry {
            insert_text: Some(snippet_text),
            filter_text: Some("User".to_owned()),
            ..entry("User", 7)
        };
        let raw = list(vec![plain, snippet], false);
        let result = map_completions(Some(&raw));
        assert_eq!(result.entries[0].insert_text.as_deref(), Some("User"));
        assert_eq!(result.entries[0].filter_text.as_deref(), Some("User"));
        assert_eq!(result.entries[1].insert_text, None);
        assert_eq!(result.entries[1].filter_text.as_deref(), Some("User"));
        assert_eq!(
            result.gaps,
            vec![
                CompletionGap::InsertTextFormatUnknown,
                CompletionGap::DocumentationUnavailable,
                CompletionGap::AutoImportsUnavailable,
            ]
        );
    }

    #[test]
    fn an_entry_is_never_marked_as_an_auto_import() {
        // The DTO has no auto-import field, so this test freezes the shape:
        // only the result-level gap may mention auto-imports.
        let raw = list(vec![entry("User", 7)], false);
        let result = map_completions_with(Some(&raw), AutoImportContext::Prepared);
        assert_eq!(result.entries[0].label, "User");
        assert_eq!(result.entries[0].insert_text, None);
        assert_eq!(result.entries[0].filter_text, None);
        assert!(
            result
                .gaps
                .contains(&CompletionGap::AutoImportOriginUnavailable)
        );
    }
}
