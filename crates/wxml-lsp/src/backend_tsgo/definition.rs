//! Definition composition from native referenced-symbol evidence.
//!
//! The native session resolves the queried expression to an Identifier and
//! returns declaration name tokens. This module requires the cursor to map
//! into a mounted expression, maps the owning shadow script back through
//! [`ShadowLayout`], and converts every other target's UTF-16 token range to
//! UTF-8 against the exact text sent to the engine.
//!
//! Status is never upgraded: a checked empty native answer stays `Supported`,
//! while a decode gap, a target the host cannot represent, a cropped template
//! range or host degradation stays `Partial`. Native `Unsupported` and
//! transport/protocol failures are reported, never turned into an empty
//! success.

use std::collections::BTreeSet;

use crate::backend_tsgo::query::definition::DefinitionTarget as NativeDefinitionTarget;
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::strings::{
    Utf16Offset, Utf16Position, offset_to_position, utf16_to_utf8,
};
use wxml_types::{BackendError, CapabilityStatus, DefinitionResult, FileId, SourceLocation};

use crate::backend_tsgo::input::RequestPlan;
use crate::backend_tsgo::map_native_error;
use crate::backend_tsgo::mapping::{MappingError, NativeRange, ShadowLayout};
use crate::backend_tsgo::session::QuerySession;

/// Resolves the cursor to a UTF-16 position in the derived shadow script.
///
/// Returns `Ok(None)` when the cursor is not inside a mounted expression. That
/// is an explicit unsupported answer, not a checked empty definition, so the
/// caller must not start a native session for it.
pub(crate) fn query_position(
    layout: &ShadowLayout,
    offset: u32,
) -> Result<Option<Utf16Position>, BackendError> {
    let derived = match layout.derived_offset(offset, true) {
        Ok(offset) => offset,
        Err(MappingError::NoFragment | MappingError::MappingGap) => return Ok(None),
        Err(error) => return Err(mapping_request_error(error)),
    };
    offset_to_position(layout.derived(), Utf16Offset::new(derived))
        .map(Some)
        .map_err(|_| {
            BackendError::Protocol(
                "definition position is invalid in the derived script".to_owned(),
            )
        })
}

/// Returns the unsupported result for a cursor outside every mounted expression.
pub(crate) fn outside_expression() -> DefinitionResult {
    unsupported_definition("the cursor is not inside a mounted template expression")
}

/// Runs one definition query and maps every representable target.
pub(crate) fn definition(
    session: &mut dyn QuerySession,
    plan: &RequestPlan,
    position: Utf16Position,
    degradation: Option<&str>,
) -> Result<DefinitionResult, BackendError> {
    let layout = &plan.layout;
    let native = match session.definition(&plan.identity.script_path, position) {
        Ok(native) => native,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_definition(
                "native definition queries are unavailable",
            ));
        }
        Err(error) => return Err(map_native_error(error)),
    };

    let mut status = native.status;
    if native.truncated {
        mark_partial(
            &mut status,
            "the native definition target list was truncated",
        );
    }
    let mut locations = Vec::with_capacity(native.targets.len());
    let mut seen = BTreeSet::new();
    let mut unrepresentable = false;
    let mut duplicates = false;
    let source = &plan.identity.source;
    for target in &native.targets {
        let Some(mapped) = map_target(plan, layout, source, target) else {
            unrepresentable = true;
            continue;
        };
        if mapped.approximate {
            mark_partial(
                &mut status,
                "a definition target was cropped at generated text",
            );
        }
        let location = mapped.location;
        let key = (
            location.file.clone(),
            location.range.start(),
            location.range.end(),
        );
        if !seen.insert(key) {
            duplicates = true;
            continue;
        }
        locations.push(location);
    }
    if unrepresentable {
        mark_partial(
            &mut status,
            "some definition targets could not be represented",
        );
    }
    if duplicates {
        mark_partial(&mut status, "duplicate definition targets were dropped");
    }
    if let Some(reason) = degradation {
        mark_partial(&mut status, reason);
    }

    Ok(DefinitionResult { locations, status })
}

struct MappedTarget {
    location: SourceLocation,
    approximate: bool,
}

/// Maps one native target, or returns `None` when it cannot be represented.
///
/// The owning script is the in-place shadow: its native UTF-16 range must be
/// mapped back to the template or original script bytes. Every other target
/// file was sent verbatim, so its UTF-16 range converts directly and must
/// agree with the byte range from the same native snapshot.
fn map_target(
    plan: &RequestPlan,
    layout: &ShadowLayout,
    source: &FileId,
    target: &NativeDefinitionTarget,
) -> Option<MappedTarget> {
    let file = plan.file_id(&target.path)?;
    let is_script = target.path == plan.identity.script_path;
    let text = if is_script {
        layout.derived()
    } else {
        plan.file_text(&target.path)?
    };
    // The native target carries both coordinate systems. Validate that they
    // agree against the exact text the engine saw, then use UTF-16 for the
    // shadow mapper (its input contract) and UTF-8 only for verbatim files.
    let (start_utf8, end_utf8) = convert_range(text, target)?;
    if is_script {
        let start = target.utf16.start.get();
        let end = target.utf16.end.get();
        return match layout.map_native_range(start, end).ok()? {
            NativeRange::Template { range, approximate } => Some(MappedTarget {
                location: SourceLocation {
                    file: source.clone(),
                    range,
                },
                approximate,
            }),
            NativeRange::Script { range } => Some(MappedTarget {
                location: SourceLocation { file, range },
                approximate: false,
            }),
        };
    }
    let range = wxml_text::SourceRange::from_bounds(start_utf8, end_utf8).ok()?;
    Some(MappedTarget {
        location: SourceLocation { file, range },
        approximate: false,
    })
}

/// Converts the target's UTF-16 token to UTF-8 and verifies the native pair.
fn convert_range(text: &str, target: &NativeDefinitionTarget) -> Option<(u32, u32)> {
    let start = utf16_to_utf8(text, target.utf16.start).ok()?.get();
    let end = utf16_to_utf8(text, target.utf16.end).ok()?.get();
    if start >= end {
        return None;
    }
    if target.range.start.get() != start || target.range.end.get() != end {
        return None;
    }
    Some((start, end))
}

fn mark_partial(status: &mut CapabilityStatus, reason: &str) {
    if status.is_supported() {
        *status = CapabilityStatus::Partial {
            reason: reason.to_owned(),
        };
        return;
    }
    if let CapabilityStatus::Partial { reason: existing } = status
        && !existing.split("; ").any(|part| part == reason)
    {
        existing.push_str("; ");
        existing.push_str(reason);
    }
}

fn unsupported_definition(reason: &str) -> DefinitionResult {
    DefinitionResult {
        locations: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
    }
}

fn mapping_request_error(error: MappingError) -> BackendError {
    match error {
        MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. } => {
            BackendError::Protocol(
                "definition offset is not a valid template character boundary".to_owned(),
            )
        }
        other => BackendError::Protocol(format!("definition mapping failed: {other}")),
    }
}
