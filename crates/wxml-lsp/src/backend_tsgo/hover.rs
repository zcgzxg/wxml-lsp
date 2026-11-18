//! Hover composition from native type and documentation evidence.
//!
//! The native engine has no `getQuickInfoAtPosition`; hover is built from
//! `getTypeAtPosition` + `typeToString` + `getDocumentationComment` +
//! `getJsDocTags`. Type quality is preserved and missing documentation is
//! reported as `Partial`, never filled with invented text.

use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::strings::{Utf16Offset, offset_to_position};
use wxml_backend_tsgo::query::type_info::{TypeQuality as NativeTypeQuality, map_type_query};
use wxml_types::{
    BackendError, CapabilityStatus, HoverRequest, HoverResult, MarkupKind, TypeQuality,
};

use crate::backend_tsgo::map_native_error;
use crate::backend_tsgo::mapping::{MappingError, ShadowLayout};
use crate::backend_tsgo::session::{HoverDocumentation, QuerySession};

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
    let query = match session.type_query(script_path, position) {
        Ok(query) => query,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_hover("native type queries are unavailable"));
        }
        Err(error) => return Err(map_native_error(error)),
    };
    let type_result = map_type_query(&query);
    let documentation = match session.hover_documentation(script_path, position) {
        Ok(documentation) => documentation,
        Err(NativeError::Unsupported { .. }) => HoverDocumentation {
            symbol: true,
            result: wxml_backend_tsgo::query::documentation::DocumentationResult {
                comment: None,
                tags: Vec::new(),
                status: CapabilityStatus::Unsupported {
                    reason: "native documentation queries are unavailable".to_owned(),
                },
            },
        },
        // Transport/protocol failures are not documentation-only degradation:
        // the adapter drops the session and reports the honest error.
        Err(error) => return Err(map_native_error(error)),
    };

    compose(layout, request, &type_result, &documentation, degradation)
}

fn compose(
    layout: &ShadowLayout,
    request: &HoverRequest,
    type_result: &wxml_backend_tsgo::query::type_info::TypeResult,
    documentation: &HoverDocumentation,
    degradation: Option<&str>,
) -> Result<HoverResult, BackendError> {
    let quality = map_quality(type_result.quality);
    let type_text = if matches!(quality, TypeQuality::Known | TypeQuality::Any) {
        type_result.info.as_ref().map(|info| info.text.clone())
    } else {
        None
    };
    let docs_text = render_documentation(documentation);
    let has_documents = docs_text.is_some();
    let mut parts = Vec::new();
    if let Some(text) = &type_text {
        parts.push(format!("```ts\n{text}\n```"));
    }
    if let Some(text) = docs_text {
        parts.push(text);
    }
    let contents = parts.join("\n\n");

    let mut status = type_result.status.clone();
    if type_text.is_none() && has_documents {
        status = merge_status(status, Some("the hover type could not be established"));
    }
    if documentation.symbol && !documentation.result.status.is_supported() {
        status = merge_status(status, Some("hover documentation is incomplete"));
    }
    if type_text.is_none() && !has_documents {
        status = CapabilityStatus::Unsupported {
            reason: "no native type or documentation is available at this position".to_owned(),
        };
    }
    if contents.is_empty() {
        status = CapabilityStatus::Unsupported {
            reason: "no native hover evidence is available at this position".to_owned(),
        };
    }
    if let Some(reason) = degradation {
        status = merge_status(status, Some(reason));
    }

    let range = match layout.token_range(request.offset) {
        Ok(range) => range,
        Err(MappingError::NoFragment | MappingError::MappingGap) => {
            status = merge_status(status, Some("the hover range could not be mapped"));
            None
        }
        Err(error) => return Err(mapping_request_error(error)),
    };

    Ok(HoverResult {
        contents,
        markup: MarkupKind::Markdown,
        range,
        status,
        quality,
    })
}

fn render_documentation(documentation: &HoverDocumentation) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(comment) = documentation.result.comment.as_deref() {
        let comment = comment.trim();
        if !comment.is_empty() {
            sections.push(comment.to_owned());
        }
    }
    for tag in &documentation.result.tags {
        let mut line = format!("@{}", tag.name);
        if let Some(text) = tag.text.as_deref().map(str::trim) {
            if !text.is_empty() {
                line.push(' ');
                line.push_str(text);
            }
        }
        sections.push(line);
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

const fn map_quality(quality: NativeTypeQuality) -> TypeQuality {
    match quality {
        NativeTypeQuality::Known => TypeQuality::Known,
        NativeTypeQuality::Any => TypeQuality::Any,
        NativeTypeQuality::Unknown => TypeQuality::Unknown,
        NativeTypeQuality::Missing => TypeQuality::Missing,
    }
}

fn merge_status(status: CapabilityStatus, reason: Option<&str>) -> CapabilityStatus {
    let Some(reason) = reason else {
        return status;
    };
    match status {
        CapabilityStatus::Supported => CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
        other => other,
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
