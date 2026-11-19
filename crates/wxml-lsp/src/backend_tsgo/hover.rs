//! Hover mapping from native evidence to the LSP-facing result.
//!
//! The adapter maps the template offset to the shadow script, asks the native
//! session for one [`HoverEvidence`] answer and publishes the backend's
//! composed Markdown unchanged: the ` ```ts ` type fence, the documentation
//! comment and the `JSDoc` tags keep the backend order and status. The adapter
//! only adds what the backend cannot know: the shadow position, the original
//! template token range and host degradation.
//!
//! The adapter also enforces two hover contracts: an empty Markdown answer is
//! never published as success, and `Unsupported` with rendered content is
//! never published as `Supported`. Both contradictions become explicit
//! degraded statuses instead of being smoothed over.

use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::strings::{Utf16Offset, offset_to_position};
use wxml_backend_tsgo::query::hover::{HoverEvidence, compose_hover_markdown};
use wxml_backend_tsgo::query::type_info::TypeQuality as NativeTypeQuality;
use wxml_types::{
    BackendError, CapabilityStatus, HoverRequest, HoverResult, MarkupKind, TypeQuality,
};

use crate::backend_tsgo::map_native_error;
use crate::backend_tsgo::mapping::{MappingError, ShadowLayout};
use crate::backend_tsgo::session::QuerySession;

/// Returns one hover result, or an explicit unsupported result when no native
/// evidence path applies.
pub(crate) fn hover(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    script_path: &str,
    degradation: Option<&str>,
    request: &HoverRequest,
) -> Result<HoverResult, BackendError> {
    let derived = match layout.derived_offset(request.offset, true) {
        Ok(offset) => offset,
        Err(MappingError::NoFragment | MappingError::MappingGap) => {
            return Ok(unsupported_hover(
                "the cursor is not inside a mounted template expression",
            ));
        }
        Err(error) => return Err(mapping_request_error(error)),
    };
    let position =
        offset_to_position(layout.derived(), Utf16Offset::new(derived)).map_err(|_| {
            BackendError::Protocol("hover position is invalid in the derived script".to_owned())
        })?;
    let evidence = match session.hover_evidence(script_path, position) {
        Ok(evidence) => evidence,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_hover("native hover evidence is unavailable"));
        }
        Err(error) => return Err(map_native_error(error)),
    };

    compose(layout, request, &evidence, degradation)
}

fn compose(
    layout: &ShadowLayout,
    request: &HoverRequest,
    evidence: &HoverEvidence,
    degradation: Option<&str>,
) -> Result<HoverResult, BackendError> {
    let contents = compose_hover_markdown(evidence);
    let quality = map_quality(evidence.type_quality);
    let (range, range_failed) = match layout.token_range(request.offset) {
        Ok(range) => (range, false),
        Err(MappingError::NoFragment | MappingError::MappingGap) => (None, true),
        Err(error) => return Err(mapping_request_error(error)),
    };
    let status = resolved_status(&evidence.status, &contents, degradation, range_failed);

    Ok(HoverResult {
        contents,
        markup: MarkupKind::Markdown,
        range,
        status,
        quality,
    })
}

/// Resolves the published status from the backend status and adapter facts.
///
/// Contract normalization happens before host facts are appended:
///
/// - empty Markdown always becomes `Unsupported`, keeping a readable reason;
/// - `Unsupported` with rendered content becomes `Partial`, because publishing
///   proven content as unsupported (or as supported) would be dishonest.
///
/// Host degradation and range-mapping failures are then appended without
/// dropping the backend reason. `Supported` is promoted to `Partial`; an
/// existing `Partial`/`Unsupported` reason is kept and extended; the remaining
/// statuses are left untouched.
///
/// Visible to the adapter tests so the range-failure branch can be exercised
/// directly: a consistent layout that already produced a derived position
/// cannot make [`ShadowLayout::token_range`] return a mapping gap.
pub(super) fn resolved_status(
    backend: &CapabilityStatus,
    contents: &str,
    degradation: Option<&str>,
    range_failed: bool,
) -> CapabilityStatus {
    let mut status = if contents.is_empty() {
        normalize_empty_status(backend)
    } else if matches!(backend, CapabilityStatus::Unsupported { .. }) {
        let backend_reason = backend.reason().unwrap_or("no reason reported");
        CapabilityStatus::Partial {
            reason: format!(
                "native hover evidence is unsupported despite rendered content: {backend_reason}"
            ),
        }
    } else {
        backend.clone()
    };
    if let Some(reason) = degradation {
        status = append_reason(status, reason);
    }
    if range_failed {
        status = append_reason(status, "the hover range could not be mapped");
    }
    status
}

fn normalize_empty_status(status: &CapabilityStatus) -> CapabilityStatus {
    let reason = match status {
        CapabilityStatus::Supported => {
            "native hover evidence is empty but the backend reported it as supported".to_owned()
        }
        CapabilityStatus::Partial { reason }
        | CapabilityStatus::Unsupported { reason }
        | CapabilityStatus::Disabled { reason }
        | CapabilityStatus::Unavailable { reason } => usable_reason(reason),
        CapabilityStatus::Unknown { reason } => reason
            .as_deref()
            .map_or_else(default_empty_reason, usable_reason),
        // Non-exhaustive: future statuses are normalized like an empty answer.
        _ => default_empty_reason(),
    };
    CapabilityStatus::Unsupported { reason }
}

fn usable_reason(reason: &str) -> String {
    if reason.trim().is_empty() {
        default_empty_reason()
    } else {
        reason.to_owned()
    }
}

fn default_empty_reason() -> String {
    "no native hover evidence is available at this position".to_owned()
}

fn append_reason(status: CapabilityStatus, reason: &str) -> CapabilityStatus {
    let reason = reason.trim();
    if reason.is_empty() {
        return status;
    }
    match status {
        CapabilityStatus::Supported => CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
        CapabilityStatus::Partial { reason: existing } => CapabilityStatus::Partial {
            reason: join_reasons(&existing, reason),
        },
        CapabilityStatus::Unsupported { reason: existing } => CapabilityStatus::Unsupported {
            reason: join_reasons(&existing, reason),
        },
        // These statuses carry policy/availability meaning; adding a partial
        // reason would promote them to a stronger claim.
        other => other,
    }
}

fn join_reasons(existing: &str, addition: &str) -> String {
    if existing.split("; ").any(|reason| reason == addition) {
        return existing.to_owned();
    }
    if existing.trim().is_empty() {
        return addition.to_owned();
    }
    format!("{existing}; {addition}")
}

const fn map_quality(quality: NativeTypeQuality) -> TypeQuality {
    match quality {
        NativeTypeQuality::Known => TypeQuality::Known,
        NativeTypeQuality::Any => TypeQuality::Any,
        NativeTypeQuality::Unknown => TypeQuality::Unknown,
        NativeTypeQuality::Missing => TypeQuality::Missing,
    }
}

fn unsupported_hover(reason: &str) -> HoverResult {
    HoverResult {
        contents: String::new(),
        markup: MarkupKind::Markdown,
        range: None,
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
        quality: TypeQuality::Unknown,
    }
}

fn mapping_request_error(error: MappingError) -> BackendError {
    match error {
        MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. } => {
            BackendError::Protocol(
                "hover offset is not a valid template character boundary".to_owned(),
            )
        }
        other => BackendError::Protocol(format!("hover mapping failed: {other}")),
    }
}
