//! Diagnostics from both native diagnostic sources, mapped to template bytes.
//!
//! Host parser diagnostics are the base of every result. A clean `Supported`
//! result requires the semantic and syntactic calls to complete and every
//! published native range to map back to the template. A fatal native failure
//! is an `Unavailable` result with the host syntax evidence, never an `Err`
//! that makes the server reschedule immediately. A dropped/mismatched range or
//! a single unavailable method degrades the result to `Partial`.

use std::collections::BTreeSet;

use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::strings::{Utf16Offset, position_to_offset};
use wxml_backend_tsgo::query::diagnostics::{
    DiagnosticSeverity as NativeSeverity, map_diagnostics,
};
use wxml_types::{CapabilityStatus, Diagnostic, DiagnosticSeverity, DiagnosticsResult};

use crate::backend_tsgo::mapping::{NativeRange, ShadowLayout};
use crate::backend_tsgo::session::QuerySession;

/// One diagnostics run plus whether the native session must be dropped.
#[derive(Debug)]
pub(crate) struct DiagnosticsRun {
    /// Result to publish.
    pub(crate) result: DiagnosticsResult,
    /// The session hit a fatal transport/protocol failure and is unreachable.
    pub(crate) fatal: bool,
}

/// Runs both diagnostic sources and maps every publishable range.
#[allow(
    clippy::too_many_lines,
    reason = "one linear diagnostics mapping pipeline"
)]
pub(crate) fn diagnostics(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    script_path: &str,
    syntax: &[Diagnostic],
    degradation: Option<&str>,
) -> DiagnosticsRun {
    let semantic = session.semantic_diagnostics(script_path);
    let syntactic = session.syntactic_diagnostics(script_path);
    let (semantic, syntactic, source_status) = match (semantic, syntactic) {
        (Ok(semantic), Ok(syntactic)) => (semantic, syntactic, CapabilityStatus::Supported),
        (Ok(semantic), Err(error)) => {
            if is_unsupported(&error) {
                (semantic, Vec::new(), partial_for_source("syntactic"))
            } else {
                return unavailable(syntax);
            }
        }
        (Err(error), Ok(syntactic)) => {
            if is_unsupported(&error) {
                (Vec::new(), syntactic, partial_for_source("semantic"))
            } else {
                return unavailable(syntax);
            }
        }
        (Err(semantic), Err(syntactic)) => {
            if is_unsupported(&semantic) && is_unsupported(&syntactic) {
                return DiagnosticsRun {
                    result: DiagnosticsResult {
                        diagnostics: syntax.to_vec(),
                        status: CapabilityStatus::Unsupported {
                            reason: "native diagnostics are unavailable".to_owned(),
                        },
                    },
                    fatal: false,
                };
            }
            return unavailable(syntax);
        }
    };

    let mapped = map_diagnostics(&semantic, &syntactic, layout.derived());
    let mut status = merge_status(source_status, mapped.status);
    let mut diagnostics = syntax.to_vec();
    let mut seen: BTreeSet<(Option<String>, u32, u32, String)> = diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code.clone(),
                diagnostic.range.start(),
                diagnostic.range.end(),
                diagnostic.message.clone(),
            )
        })
        .collect();
    for diagnostic in mapped.diagnostics {
        let Some(start) = diagnostic_offset(layout, diagnostic.start) else {
            mark_partial(
                &mut status,
                "a native diagnostic position is outside the shadow",
            );
            continue;
        };
        let Some(end) = diagnostic_offset(layout, diagnostic.end) else {
            mark_partial(
                &mut status,
                "a native diagnostic position is outside the shadow",
            );
            continue;
        };
        let Some(location) = map_range(layout, start, end, &mut status) else {
            continue;
        };
        if let NativeRange::Template { range, approximate } = location {
            if approximate {
                mark_partial(
                    &mut status,
                    "a native diagnostic range was cropped at generated text",
                );
            }
            let code = Some(diagnostic.code.to_string());
            if seen.insert((
                code.clone(),
                range.start(),
                range.end(),
                diagnostic.message.clone(),
            )) {
                diagnostics.push(Diagnostic {
                    range,
                    severity: map_severity(diagnostic.severity, &mut status),
                    message: diagnostic.message,
                    code,
                });
            }
        } else {
            // A script diagnostic belongs to the original script, not this
            // WXML request. It is not a template diagnostic and cannot be
            // represented in this result without a wrong file identity.
        }
    }
    if let Some(reason) = degradation {
        status = merge_status(
            status,
            CapabilityStatus::Partial {
                reason: reason.to_owned(),
            },
        );
    }

    DiagnosticsRun {
        result: DiagnosticsResult {
            diagnostics,
            status,
        },
        fatal: false,
    }
}

fn unavailable(syntax: &[Diagnostic]) -> DiagnosticsRun {
    DiagnosticsRun {
        result: DiagnosticsResult {
            diagnostics: syntax.to_vec(),
            status: CapabilityStatus::Unavailable {
                reason: "type backend unavailable".to_owned(),
            },
        },
        fatal: true,
    }
}

fn diagnostic_offset(
    layout: &ShadowLayout,
    position: wxml_backend_tsgo::native::strings::Utf16Position,
) -> Option<u32> {
    position_to_offset(layout.derived(), position)
        .ok()
        .map(Utf16Offset::get)
}

fn map_range(
    layout: &ShadowLayout,
    start: u32,
    end: u32,
    status: &mut CapabilityStatus,
) -> Option<NativeRange> {
    layout.map_native_range(start, end).map_or_else(
        |_| {
            mark_partial(status, "a native diagnostic range could not be mapped");
            None
        },
        Some,
    )
}

fn mark_partial(status: &mut CapabilityStatus, reason: &str) {
    *status = merge_status(
        status.clone(),
        CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
    );
}

fn map_severity(severity: NativeSeverity, status: &mut CapabilityStatus) -> DiagnosticSeverity {
    match severity {
        NativeSeverity::Error => DiagnosticSeverity::Error,
        NativeSeverity::Warning => DiagnosticSeverity::Warning,
        NativeSeverity::Suggestion => DiagnosticSeverity::Hint,
        NativeSeverity::Message => DiagnosticSeverity::Information,
        NativeSeverity::Unknown => {
            mark_partial(status, "a native diagnostic used an unknown severity");
            DiagnosticSeverity::Information
        }
    }
}

fn partial_for_source(source: &str) -> CapabilityStatus {
    CapabilityStatus::Partial {
        reason: format!("native {source} diagnostics are unavailable"),
    }
}

const fn is_unsupported(error: &NativeError) -> bool {
    matches!(error, NativeError::Unsupported { .. })
}

fn merge_status(status: CapabilityStatus, other: CapabilityStatus) -> CapabilityStatus {
    match (status, other) {
        (CapabilityStatus::Supported, other) => other,
        (status, CapabilityStatus::Supported) => status,
        (CapabilityStatus::Partial { reason }, CapabilityStatus::Partial { reason: next }) => {
            if reason.contains(&next) {
                CapabilityStatus::Partial { reason }
            } else {
                CapabilityStatus::Partial {
                    reason: format!("{reason}; {next}"),
                }
            }
        }
        (CapabilityStatus::Partial { reason }, _) => CapabilityStatus::Partial { reason },
        (status, _) => status,
    }
}
