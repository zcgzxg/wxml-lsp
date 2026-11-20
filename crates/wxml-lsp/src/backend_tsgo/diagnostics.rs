//! Diagnostics from both native diagnostic sources, mapped to template bytes.
//!
//! Host parser diagnostics are the base of every result. A clean `Supported`
//! result requires the semantic and syntactic calls to complete and every
//! published native range to map back to the template. A fatal native failure
//! is an `Unavailable` result with the host syntax evidence, never an `Err`
//! that makes the server reschedule immediately. A dropped/mismatched range or
//! a single unavailable method degrades the result to `Partial`.
//!
//! Standalone `.ts`/`.js` documents use [`script_diagnostics`]: no template
//! syntax is merged, ranges map directly into the script bytes, and diagnostics
//! attributed to another file are excluded with a `Partial` reason.

use std::collections::BTreeSet;

use crate::backend_tsgo::query::diagnostics::{
    DiagnosticSeverity as NativeSeverity, map_diagnostics_for_file,
};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::strings::{
    Utf16Offset, Utf16Position, position_to_offset, utf16_to_utf8,
};
use wxml_backend_tsgo::native::wire::RawDiagnostic;
use wxml_types::{
    CapabilityStatus, Diagnostic, DiagnosticSeverity, DiagnosticsResult, SourceRange,
    SourceSnapshot,
};

use crate::backend_tsgo::component_bindings;
use crate::backend_tsgo::mapping::{NativeRange, ShadowLayout};
use crate::backend_tsgo::session::QuerySession;
use crate::project_host::PreparedView;

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
    view: &PreparedView,
    script_path: &str,
    project_root: &str,
    syntax: &[Diagnostic],
    degradation: Option<&str>,
) -> DiagnosticsRun {
    let semantic = session.semantic_diagnostics(script_path);
    let syntactic = session.syntactic_diagnostics(script_path);
    let (semantic, syntactic, source_status) = match diagnostic_sources(semantic, syntactic) {
        DiagnosticSources::Available {
            semantic,
            syntactic,
            status,
        } => (semantic, syntactic, status),
        DiagnosticSources::Unsupported => {
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
        DiagnosticSources::Fatal => return unavailable(syntax),
    };

    let mapped = map_diagnostics_for_file(
        &semantic,
        &syntactic,
        layout.derived(),
        project_root,
        script_path,
    );
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
    let mut script_excluded = 0usize;
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
            script_excluded = script_excluded.saturating_add(1);
        }
    }
    if script_excluded > 0 {
        mark_partial(
            &mut status,
            &format!(
                "{script_excluded} native diagnostic(s) belong to script code and were \
                 excluded from this template result"
            ),
        );
    }
    let component = component_bindings::diagnostics(session, layout, view, script_path);
    for diagnostic in component.diagnostics {
        let key = (
            diagnostic.code.clone(),
            diagnostic.range.start(),
            diagnostic.range.end(),
            diagnostic.message.clone(),
        );
        if seen.insert(key) {
            diagnostics.push(diagnostic);
        }
    }
    if let Some(component_status) = component.status {
        status = merge_status(status, component_status);
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

/// Runs both native sources for one standalone script document.
///
/// No template syntax diagnostics are merged and no shadow mapping is used:
/// every published range is in the request snapshot's own bytes. Diagnostics
/// that belong to another file are excluded by the query layer and degrade the
/// result to `Partial`; a missing engine is `Unavailable`, never a supported
/// empty result.
pub(crate) fn script_diagnostics(
    session: &mut dyn QuerySession,
    project_root: &str,
    path: &str,
    source: &SourceSnapshot,
    degradation: Option<&str>,
) -> DiagnosticsRun {
    let semantic = session.semantic_diagnostics(path);
    let syntactic = session.syntactic_diagnostics(path);
    let (semantic, syntactic, source_status) = match diagnostic_sources(semantic, syntactic) {
        DiagnosticSources::Available {
            semantic,
            syntactic,
            status,
        } => (semantic, syntactic, status),
        DiagnosticSources::Unsupported => {
            return DiagnosticsRun {
                result: DiagnosticsResult {
                    diagnostics: Vec::new(),
                    status: CapabilityStatus::Unsupported {
                        reason: "native diagnostics are unavailable".to_owned(),
                    },
                },
                fatal: false,
            };
        }
        DiagnosticSources::Fatal => {
            return DiagnosticsRun {
                result: DiagnosticsResult {
                    diagnostics: Vec::new(),
                    status: CapabilityStatus::Unavailable {
                        reason: "type backend unavailable".to_owned(),
                    },
                },
                fatal: true,
            };
        }
    };

    let mapped = map_diagnostics_for_file(&semantic, &syntactic, source.text(), project_root, path);
    let mut status = merge_status(source_status, mapped.status);
    let mut diagnostics = Vec::with_capacity(mapped.diagnostics.len());
    for diagnostic in mapped.diagnostics {
        let Some(range) = script_range(source, diagnostic.start, diagnostic.end) else {
            mark_partial(
                &mut status,
                "a native diagnostic range is outside the script",
            );
            continue;
        };
        diagnostics.push(Diagnostic {
            range,
            severity: map_severity(diagnostic.severity, &mut status),
            message: diagnostic.message,
            code: Some(diagnostic.code.to_string()),
        });
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

/// Converts one validated native position pair into script bytes.
fn script_range(
    source: &SourceSnapshot,
    start: Utf16Position,
    end: Utf16Position,
) -> Option<SourceRange> {
    let start = position_to_offset(source.text(), start).ok()?;
    let end = position_to_offset(source.text(), end).ok()?;
    let start = utf16_to_utf8(source.text(), start).ok()?.get();
    let end = utf16_to_utf8(source.text(), end).ok()?.get();
    let range = wxml_text::TextRange::new(start, end).ok()?;
    source.source_range(range)
}

/// Combined availability of the semantic and syntactic calls.
enum DiagnosticSources {
    /// At least one source is usable; `status` records a missing source.
    Available {
        semantic: Vec<RawDiagnostic>,
        syntactic: Vec<RawDiagnostic>,
        status: CapabilityStatus,
    },
    /// Both sources report that the method is not available.
    Unsupported,
    /// A transport or engine failure makes the whole check unavailable.
    Fatal,
}

fn diagnostic_sources(
    semantic: Result<Vec<RawDiagnostic>, NativeError>,
    syntactic: Result<Vec<RawDiagnostic>, NativeError>,
) -> DiagnosticSources {
    match (semantic, syntactic) {
        (Ok(semantic), Ok(syntactic)) => DiagnosticSources::Available {
            semantic,
            syntactic,
            status: CapabilityStatus::Supported,
        },
        (Ok(semantic), Err(error)) => {
            if is_unsupported(&error) {
                DiagnosticSources::Available {
                    semantic,
                    syntactic: Vec::new(),
                    status: partial_for_source("syntactic"),
                }
            } else {
                DiagnosticSources::Fatal
            }
        }
        (Err(error), Ok(syntactic)) => {
            if is_unsupported(&error) {
                DiagnosticSources::Available {
                    semantic: Vec::new(),
                    syntactic,
                    status: partial_for_source("semantic"),
                }
            } else {
                DiagnosticSources::Fatal
            }
        }
        (Err(semantic), Err(syntactic)) => {
            if is_unsupported(&semantic) && is_unsupported(&syntactic) {
                DiagnosticSources::Unsupported
            } else {
                DiagnosticSources::Fatal
            }
        }
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

fn diagnostic_offset(layout: &ShadowLayout, position: Utf16Position) -> Option<u32> {
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
