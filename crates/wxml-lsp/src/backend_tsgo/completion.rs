//! Native completion mapping with explicit details/truncation degradation.
//!
//! The pinned 7.1.0-beta `getCompletionsAtPosition` response emits only
//! `name`, `kind` (an LSP `CompletionItemKind` number) and `sortText`; optional
//! `detail` / `labelDetails` fields are tolerated if a future pinned artifact
//! adds them but are not expected today. The adapter maps those real fields
//! and computes the replacement range from the current WXML token. The pinned
//! API has no `getCompletionEntryDetails`, no per-entry documentation and no
//! display parts; auto-imports need `includeSymbol:true` plus a snapshot
//! created with `prepareAutoImports`, which this adapter does not send.
//! Non-empty results remain `Partial` with the typed native gaps, as does a
//! `null` response (a response gap, never a checked empty answer); details are
//! never synthesized from type/documentation queries. A failed query is an
//! error or an explicit `Unsupported` result, never a supported empty list.

use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::strings::{Utf16Offset, offset_to_position};
use wxml_backend_tsgo::query::completion::map_completions;
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
    let derived = match layout.derived_offset(request.offset, true) {
        Ok(offset) => offset,
        Err(MappingError::NoFragment | MappingError::MappingGap) => {
            return Ok(unsupported_completion(
                "the cursor is in an incomplete or unmounted template expression; member completion requires a host-parsed fragment",
            ));
        }
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
    let position =
        offset_to_position(layout.derived(), Utf16Offset::new(derived)).map_err(|_| {
            BackendError::Protocol("completion position is invalid in the shadow".to_owned())
        })?;
    let raw = match session.completions(script_path, position) {
        Ok(raw) => raw,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_completion(
                "native completion queries are unavailable",
            ));
        }
        Err(error) => return Err(map_native_error(error)),
    };
    let mapped = map_completions(raw.as_ref());
    let replace = match layout.completion_replace_range(request.offset) {
        Ok(range) => Some(range),
        Err(MappingError::NoFragment | MappingError::MappingGap) => None,
        Err(MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. }) => {
            return Err(BackendError::Protocol(
                "completion replacement is not a valid template character boundary".to_owned(),
            ));
        }
        Err(error) => {
            return Err(BackendError::Protocol(format!(
                "completion replacement mapping failed: {error}"
            )));
        }
    };
    if !mapped.entries.is_empty() && replace.is_none() {
        let missing_replace = CapabilityStatus::Partial {
            reason: "the completion replacement range cannot be determined".to_owned(),
        };
        return Ok(CompletionResult {
            items: Vec::new(),
            status: merge_status(missing_replace, degradation),
            truncated: mapped.truncated,
        });
    }
    let Some(replace) = replace else {
        return Ok(CompletionResult {
            items: Vec::new(),
            status: merge_status(mapped.status, degradation),
            truncated: mapped.truncated,
        });
    };
    let items = mapped
        .entries
        .into_iter()
        .map(|entry| {
            let sort_text = (!entry.sort_text.is_empty()).then_some(entry.sort_text);
            CompletionItem {
                label: entry.label.clone(),
                kind: entry.kind,
                detail: entry.detail,
                documentation: None,
                insert_text: entry.label.clone(),
                insert_text_format: InsertTextFormat::PlainText,
                replace,
                sort_text,
                filter_text: Some(entry.label),
            }
        })
        .collect();
    Ok(CompletionResult {
        items,
        status: merge_status(mapped.status, degradation),
        truncated: mapped.truncated,
    })
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
