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
//!
//! `miniprogram-api-typings` maps a component property's instance type from
//! `type` and drops `optionalTypes`. When the script overlay proved the full
//! declared union, the adapter prefers it for the free template root at the
//! cursor: a declared `{ type: String, optionalTypes: [Number] }` hovers as
//! `string | number`. Member accesses and unreadable declarations keep the
//! native answer.

use std::collections::BTreeMap;

use crate::backend_tsrs::query::hover::{HoverEvidence, HoverGap, compose_hover_markdown};
use crate::backend_tsrs::query::type_info::{
    TypeInfo as NativeTypeInfo, TypeQuality as NativeTypeQuality,
};
use wxml_backend_tsrs::native::NativeError;
use wxml_backend_tsrs::native::strings::{Utf16Offset, offset_to_position};
use wxml_types::{
    BackendError, CapabilityStatus, HoverRequest, HoverResult, MarkupKind, TypeQuality,
};

use crate::backend_tsrs::input::EVENT_POINT_FRAGMENT_ID;
use crate::backend_tsrs::map_native_error;
use crate::backend_tsrs::mapping::{MappingError, ShadowLayout};
use crate::backend_tsrs::session::QuerySession;

/// One declaration-derived property type.
///
/// `type` plus `optionalTypes` can declare a union that
/// `miniprogram-api-typings` loses in the instance `data` type. The script
/// overlay proves the union syntactically; hover prefers it for a declared
/// property root so the tooltip keeps the declaration's contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredPropertyType {
    /// Union text such as `string | number`.
    pub(crate) type_text: String,
    /// False when any `type`/`optionalTypes` entry was unreadable or the
    /// surrounding property declaration is incomplete.
    pub(crate) complete: bool,
}

/// Returns one hover result, or an explicit unsupported result when no native
/// evidence path applies.
///
/// `declared` carries the owning script's declaration-derived properties. It
/// only overrides the native type for the free template root at the cursor; a
/// member access such as `user.name` is never relabeled with a property union.
pub(crate) fn hover(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    script_path: &str,
    declared: &BTreeMap<String, DeclaredPropertyType>,
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
    let mut evidence = match session.hover_evidence(script_path, position) {
        Ok(evidence) => evidence,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_hover("native hover evidence is unavailable"));
        }
        Err(error) => return Err(map_native_error(error)),
    };
    apply_declared_property_type(
        &mut evidence,
        declared,
        hovered_root(layout, request, derived).as_deref(),
    );
    if event_point_member(layout, request.offset)
        && (is_unproven_any(&evidence) || !has_renderable_event_content(&evidence))
    {
        // `this.<handler>` only resolves when the handler is a member of the
        // registration instance's own type. `Page.Instance` does not merge
        // behavior methods, so a handler that only a behavior provides comes
        // back as `any` without any symbol or declaration, or as a bare
        // unsupported answer with no type and no documentation. Publishing
        // that would claim a contract the engine never proved.
        return Ok(unsupported_hover(
            "the event handler is not a member of the template owner instance type",
        ));
    }

    compose(layout, request, &evidence, degradation)
}

/// Returns whether the cursor selects the event-handler point fragment.
///
/// The check is deliberately structural: only the member fragment added by
/// [`crate::backend_tsrs::input::build_point_request_plan`] can match, so an
/// ordinary mustache expression is never treated as an event handler.
fn event_point_member(layout: &ShadowLayout, offset: u32) -> bool {
    let Ok(Some(range)) = layout.token_range(offset) else {
        return false;
    };
    layout
        .mount_for_range(range)
        .is_some_and(|mount| mount.fragment_id == EVENT_POINT_FRAGMENT_ID)
}

/// Returns whether the evidence is an unproven `any` answer.
///
/// The native checker reports an unresolved `this.<name>` as type `any` with
/// `Any` quality. That is only a fabrication risk when no symbol and no
/// declaration backs it; a proven `any` value keeps its own hover.
fn is_unproven_any(evidence: &HoverEvidence) -> bool {
    evidence.type_quality == NativeTypeQuality::Any
        && evidence
            .type_info
            .as_ref()
            .is_some_and(|info| info.text.trim() == "any")
        && evidence.symbol.is_none()
        && evidence.declarations.is_empty()
}

/// Returns whether the hover answer renders a type or documentation.
///
/// Mirrors the backend composer's renderability rules: only a `Known` type or
/// the literal `any` reaches the `ts` fence, while documentation renders when
/// its comment or any tag is non-empty. An event handler whose answer has
/// neither is treated as unresolved instead of publishing a degradation note
/// as if it were hover content.
fn has_renderable_event_content(evidence: &HoverEvidence) -> bool {
    let type_rendered = evidence.type_info.as_ref().is_some_and(|info| {
        if info.text.is_empty() {
            return false;
        }
        match evidence.type_quality {
            NativeTypeQuality::Known => true,
            NativeTypeQuality::Any => info.text == "any",
            NativeTypeQuality::Unknown | NativeTypeQuality::Missing => false,
        }
    });
    let documentation_rendered = evidence
        .documentation
        .as_ref()
        .is_some_and(|documentation| {
            documentation
                .comment
                .as_deref()
                .is_some_and(|comment| !comment.trim().is_empty())
                || !documentation.tags.is_empty()
        });
    type_rendered || documentation_rendered
}

/// Replaces the native type with a declared property union.
///
/// The override is deliberately narrow: only a complete declaration for the
/// free template root at the cursor is used. It never invents a type for an
/// unreadable declaration, a member access or an undeclared name.
pub(super) fn apply_declared_property_type(
    evidence: &mut HoverEvidence,
    declared: &BTreeMap<String, DeclaredPropertyType>,
    root: Option<&str>,
) {
    let Some(name) = root else {
        return;
    };
    let Some(fact) = declared.get(name) else {
        return;
    };
    if !fact.complete || fact.type_text.trim().is_empty() {
        return;
    }
    if evidence
        .type_info
        .as_ref()
        .is_some_and(|info| info.text == fact.type_text)
    {
        return;
    }
    evidence.type_info = Some(NativeTypeInfo {
        text: fact.type_text.clone(),
        handle: 0,
        flags: 0,
    });
    evidence.type_quality = NativeTypeQuality::Known;
    // A declaration proves the type even when the native query returned
    // `Missing`; drop only the now-stale "Missing with symbol evidence" gap.
    evidence
        .gaps
        .retain(|gap| !matches!(gap, HoverGap::Blocked { .. }));
    evidence.finalize_status();
}

/// Returns the free template root under the cursor, if the cursor is on one.
///
/// The template token must be mounted behind a free-root or property prefix in
/// the derived script. That excludes two different cases:
///
/// - `user.name` and `user['name']` hover a member, so the character before
///   the template token decides;
/// - a `wx:for` local with the same name as a declared property is emitted
///   without a prefix, so the derived text must end at the exact
///   `__wxml_root.`/`__wxml_props_0.` prefix. A local never receives a
///   declaration-derived property type.
fn hovered_root(
    layout: &ShadowLayout,
    request: &HoverRequest,
    derived_offset: u32,
) -> Option<String> {
    let range = layout.token_range(request.offset).ok().flatten()?;
    let text = request.context.source().text();
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    let token = text.get(start..end)?;
    if token.is_empty()
        || !token
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
    {
        return None;
    }
    let before = text
        .get(..start)?
        .chars()
        .rev()
        .find(|c| !c.is_whitespace())?;
    if matches!(before, '.' | ']' | ')' | '\'' | '"' | '`') {
        return None;
    }
    if !mounted_behind_property_prefix(layout.derived(), derived_offset) {
        return None;
    }
    Some(token.to_owned())
}

/// Returns whether the identifier at `derived_offset` is mounted behind a
/// declaration-derived root prefix.
fn mounted_behind_property_prefix(derived: &str, derived_offset: u32) -> bool {
    let Ok(byte) = wxml_backend_tsrs::shadow::mapping::utf16_to_byte(derived, derived_offset)
    else {
        return false;
    };
    let Some(prefix) = derived.get(..byte) else {
        return false;
    };
    let mut start = prefix.len();
    while let Some((previous, character)) = prefix[..start].char_indices().next_back() {
        if character.is_alphanumeric() || matches!(character, '_' | '$') {
            start = previous;
        } else {
            break;
        }
    }
    let before = prefix.get(..start).unwrap_or_default();
    before.ends_with(wxml_backend_tsrs::shadow::layout::ROOT_PREFIX)
        || before.ends_with(wxml_backend_tsrs::shadow::layout::PROPS_PREFIX)
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

#[cfg(test)]
#[path = "hover/tests.rs"]
mod tests;
