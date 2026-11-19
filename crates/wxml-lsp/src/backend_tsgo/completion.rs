//! Native completion mapping with explicit details/auto-import degradation.
//!
//! The pinned 7.1.0-beta `getCompletionsAtPosition` response emits `name`,
//! `kind` (an LSP `CompletionItemKind` number), `sortText` and, when
//! `includeSymbol:true` is requested on a snapshot prepared with
//! `prepareAutoImports`, `symbol`. Optional `insertText` / `filterText` /
//! `detail` / `labelDetails` members are used when present.
//!
//! `includeSymbol:true` must never be sent on an unprepared snapshot: the
//! engine rejects the whole request. The adapter therefore prepares the current
//! snapshot for the owning script first and only then requests symbols. A
//! preparation failure keeps completion functional without symbols and reports
//! the bounded reason in the status; it never becomes a supported empty list.
//!
//! The pinned API has no `getCompletionEntryDetails`, no per-entry
//! documentation, no display parts and no auto-import origin or import edit
//! (`data`/`textEdit`/`additionalTextEdits`). A prepared non-empty result is
//! therefore still `Partial` with the typed native gaps, and this module never
//! invents an `import` statement, a text edit or an auto-import marker.

use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::session::AutoImportPreparation;
use wxml_backend_tsgo::native::strings::{Utf16Offset, Utf16Position, offset_to_position};
use wxml_backend_tsgo::query::completion::{
    AutoImportContext, CompletionEntry as NativeCompletionEntry, CompletionQueryOptions,
    map_completions_with,
};
use wxml_types::{
    BackendError, CapabilityStatus, CompletionItem, CompletionKind, CompletionRequest,
    CompletionResult, InsertTextFormat,
};

use crate::backend_tsgo::map_native_error;
use crate::backend_tsgo::mapping::{MappingError, ShadowLayout};
use crate::backend_tsgo::session::QuerySession;

/// Returns completion entries mapped to original template bytes.
pub(crate) fn completion(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    script_path: &str,
    degradation: Option<&str>,
    request: &CompletionRequest,
) -> Result<CompletionResult, BackendError> {
    if request.kind != CompletionKind::ExpressionMember {
        return Ok(unsupported_completion(
            "only expression-member completion is implemented for the native backend",
        ));
    }
    let Some(position) = query_position(layout, request.offset)? else {
        return Ok(unsupported_completion(
            "the cursor is in an incomplete or unmounted template expression; member completion requires a host-parsed fragment",
        ));
    };
    let (options, auto_imports, prepare_reason) = auto_import_request(session, script_path)?;
    let degradation = merge_degradation(degradation, prepare_reason);
    let raw = match session.completions_with_options(script_path, position, options) {
        Ok(raw) => raw,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_completion(
                "native completion queries are unavailable",
            ));
        }
        Err(error) => return Err(map_native_error(error)),
    };
    let mapped = map_completions_with(raw.as_ref(), auto_imports);
    let Some(replace) = completion_replace(layout, request.offset)? else {
        let status = if mapped.entries.is_empty() {
            mapped.status
        } else {
            CapabilityStatus::Partial {
                reason: "the completion replacement range cannot be determined".to_owned(),
            }
        };
        return Ok(CompletionResult {
            items: Vec::new(),
            status: merge_status(status, degradation.as_deref()),
            truncated: mapped.truncated,
        });
    };
    Ok(CompletionResult {
        items: mapped
            .entries
            .into_iter()
            .map(|entry| completion_item(entry, replace))
            .collect(),
        status: merge_status(mapped.status, degradation.as_deref()),
        truncated: mapped.truncated,
    })
}

/// Prepares the snapshot and returns the request options plus its context.
///
/// A preparation failure is downgraded to a status reason: completion still
/// runs without symbols instead of failing the whole request.
fn auto_import_request(
    session: &mut dyn QuerySession,
    script_path: &str,
) -> Result<
    (
        CompletionQueryOptions,
        AutoImportContext,
        Option<&'static str>,
    ),
    BackendError,
> {
    Ok(match session.prepare_auto_imports(script_path) {
        Ok(AutoImportPreparation::Prepared) => (
            CompletionQueryOptions::SYMBOLS,
            AutoImportContext::Prepared,
            None,
        ),
        Ok(AutoImportPreparation::Unavailable { reason }) => (
            CompletionQueryOptions::default(),
            AutoImportContext::Unavailable,
            Some(reason),
        ),
        Ok(AutoImportPreparation::NotSupported) => (
            CompletionQueryOptions::default(),
            AutoImportContext::Unavailable,
            None,
        ),
        Err(NativeError::Unsupported { .. }) => (
            CompletionQueryOptions::default(),
            AutoImportContext::Unavailable,
            Some("auto-import preparation is unavailable in this session"),
        ),
        Err(error) => return Err(map_native_error(error)),
    })
}

/// Maps the original-template offset to a native shadow position.
fn query_position(
    layout: &ShadowLayout,
    offset: u32,
) -> Result<Option<Utf16Position>, BackendError> {
    let derived = match layout.derived_offset(offset, true) {
        Ok(offset) => offset,
        Err(MappingError::NoFragment | MappingError::MappingGap) => return Ok(None),
        Err(MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. }) => {
            return Err(BackendError::Protocol(
                "completion offset is not a valid template character boundary".to_owned(),
            ));
        }
        Err(error) => {
            return Err(BackendError::Protocol(format!(
                "completion mapping failed: {error}"
            )));
        }
    };
    offset_to_position(layout.derived(), Utf16Offset::new(derived))
        .map(Some)
        .map_err(|_| {
            BackendError::Protocol("completion position is invalid in the shadow".to_owned())
        })
}

/// Returns the replacement range, or `None` when the prefix cannot be mapped.
fn completion_replace(
    layout: &ShadowLayout,
    offset: u32,
) -> Result<Option<wxml_text::SourceRange>, BackendError> {
    match layout.completion_replace_range(offset) {
        Ok(range) => Ok(Some(range)),
        Err(MappingError::NoFragment | MappingError::MappingGap) => Ok(None),
        Err(MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. }) => {
            Err(BackendError::Protocol(
                "completion replacement is not a valid template character boundary".to_owned(),
            ))
        }
        Err(error) => Err(BackendError::Protocol(format!(
            "completion replacement mapping failed: {error}"
        ))),
    }
}

/// Publishes one native entry as a C0 completion item.
///
/// `insertText`/`filterText` are native evidence; snippet-shaped insert text
/// was already dropped by the mapper. No `documentation`, import statement or
/// additional text edit can be represented or is invented.
fn completion_item(
    entry: NativeCompletionEntry,
    replace: wxml_text::SourceRange,
) -> CompletionItem {
    let sort_text = (!entry.sort_text.is_empty()).then_some(entry.sort_text);
    let insert_text = entry.insert_text.unwrap_or_else(|| entry.label.clone());
    let filter_text = entry.filter_text.unwrap_or_else(|| entry.label.clone());
    CompletionItem {
        label: entry.label,
        kind: entry.kind,
        detail: entry.detail,
        documentation: None,
        insert_text,
        insert_text_format: InsertTextFormat::PlainText,
        replace,
        sort_text,
        filter_text: Some(filter_text),
    }
}

/// Appends the auto-import preparation reason to a host degradation.
fn merge_degradation(
    degradation: Option<&str>,
    prepare_reason: Option<&'static str>,
) -> Option<String> {
    match (degradation, prepare_reason) {
        (Some(existing), Some(reason)) => Some(format!("{existing}; {reason}")),
        (Some(existing), None) => Some(existing.to_owned()),
        (None, Some(reason)) => Some(reason.to_owned()),
        (None, None) => None,
    }
}

fn merge_status(status: CapabilityStatus, degradation: Option<&str>) -> CapabilityStatus {
    match (status, degradation) {
        (CapabilityStatus::Supported, Some(reason)) => CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
        (CapabilityStatus::Partial { reason }, Some(degradation)) => CapabilityStatus::Partial {
            reason: format!("{reason}; {degradation}"),
        },
        (status, _) => status,
    }
}

fn unsupported_completion(reason: &str) -> CompletionResult {
    CompletionResult {
        items: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
        truncated: false,
    }
}
