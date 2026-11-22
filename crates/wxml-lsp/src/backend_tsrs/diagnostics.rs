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

use crate::backend_tsrs::query::diagnostics::{
    DiagnosticSeverity as NativeSeverity, map_diagnostics_for_file,
};
use wxml_backend_tsrs::native::NativeError;
use wxml_backend_tsrs::native::strings::{
    Utf16Offset, Utf16Position, position_to_offset, utf16_to_utf8,
};
use wxml_backend_tsrs::native::wire::RawDiagnostic;
use wxml_backend_tsrs::script::symbols::Utf16Range;
use wxml_backend_tsrs::shadow::layout::ContractAnchor;
use wxml_types::{
    CapabilityStatus, Diagnostic, DiagnosticSeverity, DiagnosticsResult, SourceRange,
    SourceSnapshot,
};

use crate::backend_tsrs::component_bindings;
use crate::backend_tsrs::mapping::{NativeRange, ShadowLayout};
use crate::backend_tsrs::session::QuerySession;
use crate::project_host::PreparedView;

/// Assignment diagnostics whose range may land on a generated probe name.
///
/// `2739`/`2741` report a missing required property, which is still an
/// assignment failure for a WXML object binding. Like `2322`/`2345`, the
/// engine anchors them on the generated probe name, so they need the same
/// re-anchoring path.
const ASSIGNMENT_CODES: [u32; 4] = [2322, 2345, 2739, 2741];
/// Excess-property diagnostics, which WXML structural compatibility allows.
const EXCESS_PROPERTY_CODE: u32 = 2353;
/// Unused-local and synthetic-declaration noise produced by generated code.
const SYNTHETIC_NOISE_CODES: [u32; 2] = [6133, 6196];
/// Diagnostics for an expected type the engine cannot resolve.
const UNRESOLVED_EXPECTED_CODES: [u32; 2] = [2304, 2503];
/// Maximum published contract message length in bytes.
const MAX_CONTRACT_MESSAGE_LENGTH: usize = 512;

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
    let mut contract_bindings: BTreeSet<(u32, u32)> = BTreeSet::new();
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

        let contract = contract_hit(layout, start, end);

        // Generated declarations are consumed by `void`, so unused or
        // empty-declaration noise that still lands on a probe describes
        // synthetic text only. A script-level unused local keeps the existing
        // exclusion accounting.
        if SYNTHETIC_NOISE_CODES.contains(&diagnostic.code) && contract.is_some() {
            continue;
        }

        // WXML property binding is structural: an object literal may carry
        // fields the declared type does not name. The remaining value and
        // missing-property errors are still reported by `ASSIGNMENT_CODES` on
        // the same binding.
        if diagnostic.code == EXCESS_PROPERTY_CODE && contract.is_some() {
            continue;
        }

        if let Some(anchor) = contract
            && ASSIGNMENT_CODES.contains(&diagnostic.code)
        {
            let binding = (anchor.anchor.start.get(), anchor.anchor.end.get());
            if !contract_bindings.insert(binding) {
                continue;
            }
            // Only the generated probe name is re-anchored. A diagnostic
            // inside the bound expression keeps its precise `map_native_range`
            // position, but still counts as the one message of this binding.
            if probe_overlaps(anchor.probe, start, end) {
                let Some(range) = SourceRange::from_bounds(binding.0, binding.1).ok() else {
                    mark_partial(
                        &mut status,
                        "a component binding anchor is not a valid template range",
                    );
                    continue;
                };
                let message = truncate_contract_message(&diagnostic.message);
                let code = Some(diagnostic.code.to_string());
                if seen.insert((code.clone(), range.start(), range.end(), message.clone())) {
                    diagnostics.push(Diagnostic {
                        range,
                        severity: map_severity(diagnostic.severity, &mut status),
                        message,
                        code,
                    });
                }
                continue;
            }
        }

        let reason = if UNRESOLVED_EXPECTED_CODES.contains(&diagnostic.code) {
            "a component contract expected type could not be resolved and the binding was \
             not checked"
        } else {
            "a native diagnostic in generated text was not published"
        };
        let Some(location) = map_range(layout, start, end, &mut status, reason) else {
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
    let component = component_bindings::diagnostics(layout, view);
    for diagnostic in component.diagnostics {
        // The engine contract probe and the component pass describe the same
        // binding. The engine owns the type verdict now, so a second message at
        // the same binding range is never published.
        if contract_bindings.contains(&(diagnostic.range.start(), diagnostic.range.end())) {
            continue;
        }
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

/// Returns the contract whose generated probe text owns the derived range.
///
/// A range is attributed in two steps, both exact:
///
/// 1. it overlaps the generated variable name (`ContractAnchor::probe`), which
///    is where an assignment diagnostic lands;
/// 2. it lies inside the mount of the contract's fragment, which covers
///    diagnostics reported on the bound expression itself.
///
/// A range in any other generated text (type annotation, declaration prefix)
/// has no owner and is never guessed onto a nearby token.
fn contract_hit(layout: &ShadowLayout, start: u32, end: u32) -> Option<&ContractAnchor> {
    if let Some(anchor) = layout
        .contracts()
        .iter()
        .find(|anchor| probe_overlaps(anchor.probe, start, end))
    {
        return Some(anchor);
    }
    layout
        .contracts()
        .iter()
        .find(|anchor| contract_mount_contains(layout, anchor, start, end))
}

const fn probe_overlaps(probe: Utf16Range, start: u32, end: u32) -> bool {
    if start == end {
        probe.start.get() <= start && start <= probe.end.get()
    } else {
        start < probe.end.get() && probe.start.get() < end
    }
}

fn contract_mount_contains(
    layout: &ShadowLayout,
    anchor: &ContractAnchor,
    start: u32,
    end: u32,
) -> bool {
    let Ok(target) = SourceRange::from_bounds(anchor.anchor.start.get(), anchor.anchor.end.get())
    else {
        return false;
    };
    let Some(mount) = layout.mount_for_range(target) else {
        return false;
    };
    let Ok(mount_end) = mount.end_utf16() else {
        return false;
    };
    mount.start_utf16 <= start && end <= mount_end
}

/// Truncates one contract message at a UTF-8 boundary.
fn truncate_contract_message(message: &str) -> String {
    if message.len() <= MAX_CONTRACT_MESSAGE_LENGTH {
        return message.to_owned();
    }
    let mut end = MAX_CONTRACT_MESSAGE_LENGTH;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    message
        .get(..end)
        .map_or_else(String::new, |text| format!("{text}…"))
}

fn map_range(
    layout: &ShadowLayout,
    start: u32,
    end: u32,
    status: &mut CapabilityStatus,
    reason: &str,
) -> Option<NativeRange> {
    layout.map_native_range(start, end).map_or_else(
        |_| {
            mark_partial(status, reason);
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
