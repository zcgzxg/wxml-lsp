//! Diagnostics mapping from native wire evidence to the internal C0 shape.
//!
//! The engine returns both UTF-16 offsets (`pos`/`end`) and zero-based
//! line/UTF-16 positions. Both are cross-checked against the same text
//! snapshot; a mismatch drops that diagnostic and marks the result `Partial`
//! instead of publishing a possibly shifted range.

use std::collections::BTreeSet;

use wxml_types::CapabilityStatus;

use crate::input::ProjectKey;
use crate::native::strings::{Utf16Offset, Utf16Position, offset_to_position, position_to_offset};
use crate::native::wire::RawDiagnostic;

/// Maximum diagnostics retained for one file.
pub const MAX_DIAGNOSTICS: usize = 1_000;

/// Maximum bytes retained from one diagnostic message.
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// Diagnostics request for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsQuery {
    /// Project the document belongs to.
    pub project: ProjectKey,
    /// Normalized document path.
    pub path: String,
}

/// Diagnostic category mirrored from the native category field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    /// Native error (`category == 1`).
    Error,
    /// Native warning (`category == 0`).
    Warning,
    /// Native suggestion (`category == 2`).
    Suggestion,
    /// Informational message (`category == 3`).
    Message,
    /// A category this slice does not recognize.
    Unknown,
}

/// One native diagnostic with exact code and range evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDiagnostic {
    /// Native diagnostic code.
    pub code: u32,
    /// Sanitized message text.
    pub message: String,
    /// Severity category.
    pub severity: DiagnosticSeverity,
    /// Native exclusive end position.
    pub end: Utf16Position,
    /// Native inclusive start position.
    pub start: Utf16Position,
}

/// Diagnostics result.
///
/// Empty diagnostics mean "clean" only when [`Self::status`] is
/// [`CapabilityStatus::Supported`]. Empty plus any degraded status means the
/// check could not answer and must not be shown as a clean file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsResult {
    /// Collected diagnostics.
    pub diagnostics: Vec<NativeDiagnostic>,
    /// Completeness and availability of the check.
    pub status: CapabilityStatus,
}

/// Maps both raw diagnostic sources, cross-checking ranges against `text`.
///
/// A clean `Supported` result requires both the semantic and syntactic calls
/// to have succeeded and every retained range to be inside `text`. Callers must
/// never pass only the semantic list and present the result as complete.
#[must_use]
pub fn map_diagnostics(
    semantic: &[RawDiagnostic],
    syntactic: &[RawDiagnostic],
    text: &str,
) -> DiagnosticsResult {
    let mut diagnostics = Vec::new();
    let mut seen = BTreeSet::new();
    let mut partial = false;
    let mut truncated = false;

    for diagnostic in semantic.iter().chain(syntactic) {
        let Some(mapped) = map_one(diagnostic, text, &mut partial) else {
            continue;
        };
        let key = (
            mapped.code,
            mapped.start,
            mapped.end,
            mapped.message.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            truncated = true;
            break;
        }
        diagnostics.push(mapped);
    }

    let status = if partial || truncated {
        CapabilityStatus::Partial {
            reason: if truncated {
                "diagnostic cap reached; file may have more results".to_owned()
            } else {
                "some diagnostics were dropped by range or category validation".to_owned()
            },
        }
    } else {
        CapabilityStatus::Supported
    };

    DiagnosticsResult {
        diagnostics,
        status,
    }
}

fn map_one(raw: &RawDiagnostic, text: &str, partial: &mut bool) -> Option<NativeDiagnostic> {
    let Ok(code) = u32::try_from(raw.code) else {
        *partial = true;
        return None;
    };
    let severity = match raw.category {
        0 => DiagnosticSeverity::Warning,
        1 => DiagnosticSeverity::Error,
        2 => DiagnosticSeverity::Suggestion,
        3 => DiagnosticSeverity::Message,
        _ => {
            *partial = true;
            DiagnosticSeverity::Unknown
        }
    };

    let start = if let Some(position) = raw.start_position {
        let Some(position) = to_position(position.line, position.character) else {
            *partial = true;
            return None;
        };
        position
    } else if let Some(position) = offset_position(text, raw.pos) {
        *partial = true;
        position
    } else {
        *partial = true;
        return None;
    };
    let end = if let Some(position) = raw.end_position {
        let Some(position) = to_position(position.line, position.character) else {
            *partial = true;
            return None;
        };
        position
    } else if let Some(position) = offset_position(text, raw.end) {
        *partial = true;
        position
    } else {
        *partial = true;
        return None;
    };

    // A line/column pair is only usable when it exists in the same text
    // snapshot; publishing an out-of-file position would shift every range.
    if position_to_offset(text, start).is_err()
        || position_to_offset(text, end).is_err()
        || end < start
    {
        *partial = true;
        return None;
    }

    // Prefer line/column evidence, but only publish when the UTF-16 offsets
    // agree with the same text snapshot.
    match (raw.pos, raw.end) {
        (Some(pos_offset), Some(end_offset)) => {
            let Some(pos_position) = offset_position(text, Some(pos_offset)) else {
                *partial = true;
                return None;
            };
            let Some(end_position) = offset_position(text, Some(end_offset)) else {
                *partial = true;
                return None;
            };
            if pos_position != start || end_position != end {
                *partial = true;
                return None;
            }
        }
        _ => *partial = true,
    }

    let (message, was_truncated) = truncate_message(&raw.text);
    if was_truncated {
        *partial = true;
    }

    Some(NativeDiagnostic {
        code,
        message,
        severity,
        end,
        start,
    })
}

fn to_position(line: i64, character: i64) -> Option<Utf16Position> {
    Some(Utf16Position {
        line: u32::try_from(line).ok()?,
        character: u32::try_from(character).ok()?,
    })
}

fn offset_position(text: &str, offset: Option<i64>) -> Option<Utf16Position> {
    let offset = u32::try_from(offset?).ok()?;
    offset_to_position(text, Utf16Offset::new(offset)).ok()
}

fn truncate_message(message: &str) -> (String, bool) {
    if message.len() <= MAX_MESSAGE_BYTES {
        return (message.to_owned(), false);
    }
    let mut bounded = String::with_capacity(MAX_MESSAGE_BYTES);
    for character in message.chars() {
        if bounded.len() + character.len_utf8() > MAX_MESSAGE_BYTES {
            break;
        }
        bounded.push(character);
    }
    (bounded, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::wire::RawPosition;

    fn raw(
        code: i64,
        category: i64,
        pos: Option<i64>,
        end: Option<i64>,
        start: Option<(i64, i64)>,
        end_position: Option<(i64, i64)>,
        text: &str,
    ) -> RawDiagnostic {
        RawDiagnostic {
            code,
            category,
            pos,
            end,
            start_position: start.map(|(line, character)| RawPosition { line, character }),
            end_position: end_position.map(|(line, character)| RawPosition { line, character }),
            text: text.to_owned(),
        }
    }

    #[test]
    fn maps_error_and_preserves_utf16_ranges() {
        let text = "const 名字 = 1;\n值;\n";
        // `值` is at line 1, character 0 and UTF-16 offset 14.
        let diagnostic = raw(
            2339,
            1,
            Some(14),
            Some(15),
            Some((1, 0)),
            Some((1, 1)),
            "missing property",
        );
        let result = map_diagnostics(&[diagnostic], &[], text);
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(
            result.diagnostics[0].start,
            Utf16Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn a_syntax_only_error_is_not_reported_clean() {
        let text = "const broken = ;\n";
        let syntax = raw(
            1109,
            1,
            Some(15),
            Some(16),
            Some((0, 15)),
            Some((0, 16)),
            "expected expression",
        );
        let result = map_diagnostics(&[], &[syntax], text);
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, 1109);
    }

    #[test]
    fn out_of_file_positions_are_dropped() {
        let text = "const a = 1;\n";
        let outside = raw(1, 1, None, None, Some((99, 0)), Some((99, 1)), "x");
        let result = map_diagnostics(&[], &[outside], text);
        assert_eq!(result.diagnostics, Vec::new());
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn drops_mismatched_ranges_and_marks_partial() {
        let text = "const a = 1;\n";
        let mismatched = raw(1, 1, Some(0), Some(5), Some((0, 4)), Some((0, 5)), "x");
        let unknown = raw(2, 7, Some(6), Some(7), Some((0, 6)), Some((0, 7)), "y");
        let result = map_diagnostics(&[mismatched], &[unknown], text);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Unknown);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn deduplicates_and_caps() {
        let text = "const a = 1;\n";
        let duplicate = raw(1, 1, Some(6), Some(7), Some((0, 6)), Some((0, 7)), "same");
        let duplicate_syntax = duplicate.clone();
        let result = map_diagnostics(&[duplicate], &[duplicate_syntax], text);
        assert_eq!(result.diagnostics.len(), 1);

        let many: Vec<RawDiagnostic> = (0..MAX_DIAGNOSTICS + 5)
            .map(|index| {
                raw(
                    100 + i64::try_from(index).expect("index"),
                    1,
                    Some(6),
                    Some(7),
                    Some((0, 6)),
                    Some((0, 7)),
                    &format!("m{index}"),
                )
            })
            .collect();
        let result = map_diagnostics(&many, &[], text);
        assert_eq!(result.diagnostics.len(), MAX_DIAGNOSTICS);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }
}
