//! Literal `setData` path occurrences and conservative escape checks.
//!
//! Only `this.setData({...})` with a literal object is inspectable. The scanner
//! understands dotted string keys and nested objects, and maps every segment to
//! its exact original byte range. A spread, computed key, shorthand, method or
//! escaped string makes the answer partial; a call on another receiver is
//! never treated as this owner's `setData`.

use wxml_types::{MAX_SYMBOL_ENTRIES, SymbolOccurrenceKind};

use crate::input::{AnalysisInput, SourceFile};
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::registration::{ParsedRegistration, RegistrationKind};
use super::scanner::{MemberKind, Script, member_last, object_bounds, scan_members, unquote};
use super::symbol_facts::{SymbolOccurrenceFact, SymbolTokenFact, push_reason};

/// One literal `setData` path in one object member.
#[derive(Debug, Clone)]
struct LiteralPath {
    path: Vec<String>,
    /// UTF-8 start of the raw key content (quotes excluded).
    site_start: usize,
    /// Segments written by this member's own key.
    segments: Vec<String>,
    /// Number of path segments inherited from parent objects.
    prefix_len: usize,
    /// True when the value is a nested object literal.
    nested: bool,
}

/// Result of scanning the subject script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OccurrenceScan {
    /// Occurrences of the queried path.
    pub(crate) occurrences: Vec<SymbolOccurrenceFact>,
    /// False when any dynamic member or receiver was seen.
    pub(crate) complete: bool,
    /// Degradation reasons.
    pub(crate) problems: Vec<String>,
}

/// Scans literal `setData` calls in the subject script.
#[allow(clippy::too_many_lines)] // one call/argument/occurrence matrix
pub(crate) fn scan_set_data(file: &SourceFile, target: &[String]) -> OccurrenceScan {
    let Ok(script) = Script::new(file.text()) else {
        return OccurrenceScan {
            occurrences: Vec::new(),
            complete: false,
            problems: vec!["the script could not be scanned for setData".to_owned()],
        };
    };
    let mut complete = true;
    let mut problems = Vec::new();
    let mut occurrences = Vec::new();
    scan_computed_set_data(&script, &mut complete, &mut problems);
    scan_bare_set_data(&script, &mut complete, &mut problems);
    for index in 0..script.tokens.len() {
        if index == 0 || !script.is_ident(index, "setData") || !script.is_punct(index - 1, '.') {
            continue;
        }
        let Some(receiver) = index
            .checked_sub(2)
            .and_then(|index| script.token_text(index))
        else {
            complete = false;
            push_reason(&mut problems, "a setData call has no readable receiver");
            continue;
        };
        if receiver != "this" {
            complete = false;
            push_reason(
                &mut problems,
                "a setData call on an unknown receiver may reference this data",
            );
            continue;
        }
        if !script.is_punct(index + 1, '(') {
            complete = false;
            push_reason(
                &mut problems,
                "setData is used as a value, not a literal call",
            );
            continue;
        }
        let argument = index + 2;
        if !script.is_punct(argument, '{') {
            complete = false;
            push_reason(
                &mut problems,
                "a setData call does not use a literal object argument",
            );
            continue;
        }
        let Ok(close) = script.close_of(argument) else {
            complete = false;
            push_reason(&mut problems, "a setData object literal is not balanced");
            continue;
        };
        if !(script.is_punct(close + 1, ')') || script.is_punct(close + 1, ',')) {
            complete = false;
            push_reason(
                &mut problems,
                "a setData call has an unsupported argument shape",
            );
            continue;
        }
        let mut entries = Vec::new();
        collect_paths(
            &script,
            argument,
            close,
            &[],
            &mut entries,
            &mut complete,
            &mut problems,
        );
        for entry in &entries {
            if entry.path.len() < target.len()
                && paths_intersect(&entry.path, target)
                && !entry.nested
            {
                complete = false;
                push_reason(&mut problems, "a setData key hides a nested target path");
            }
            collect_occurrence(&script, file.path(), entry, target, &mut occurrences);
        }
    }
    occurrences.sort_by_key(|occurrence| {
        (
            occurrence.token.range.utf8.start.get(),
            occurrence.token.range.utf8.end.get(),
        )
    });
    occurrences
        .dedup_by(|left, right| left.token.range == right.token.range && left.path == right.path);
    if occurrences.len() > MAX_SYMBOL_ENTRIES {
        occurrences.truncate(MAX_SYMBOL_ENTRIES);
        complete = false;
        push_reason(&mut problems, "the setData occurrence list was truncated");
    }
    OccurrenceScan {
        occurrences,
        complete,
        problems,
    }
}

fn scan_bare_set_data(script: &Script<'_>, complete: &mut bool, problems: &mut Vec<String>) {
    for index in 0..script.tokens.len() {
        if index == 0 || !script.is_ident(index, "setData") || script.is_punct(index - 1, '.') {
            continue;
        }
        if script.is_punct(index + 1, '(') {
            *complete = false;
            push_reason(
                problems,
                "setData is called without a `this` receiver and may reference this data",
            );
        }
    }
}

fn scan_computed_set_data(script: &Script<'_>, complete: &mut bool, problems: &mut Vec<String>) {
    for index in 0..script.tokens.len() {
        if script.kind(index) != Some(TokenKind::StringLiteral) {
            continue;
        }
        if unquote(script.token_text(index).unwrap_or_default()).as_deref() != Some("setData") {
            continue;
        }
        if index > 0
            && script.is_punct(index - 1, '[')
            && script.is_punct(index + 1, ']')
            && script.is_punct(index + 2, '(')
        {
            *complete = false;
            push_reason(
                problems,
                "a computed `setData` member access may reference this data",
            );
        }
    }
}

#[allow(clippy::too_many_lines)] // one nested-path member walk
fn collect_paths(
    script: &Script<'_>,
    open: usize,
    close: usize,
    prefix: &[String],
    out: &mut Vec<LiteralPath>,
    complete: &mut bool,
    problems: &mut Vec<String>,
) {
    for member in scan_members(script, open + 1, close) {
        match member.kind {
            MemberKind::Spread => {
                *complete = false;
                push_reason(problems, "a setData object contains a spread member");
            }
            MemberKind::Computed => {
                *complete = false;
                push_reason(problems, "a setData object contains a computed key");
            }
            MemberKind::Shorthand
            | MemberKind::Method
            | MemberKind::Accessor
            | MemberKind::Unknown => {
                *complete = false;
                push_reason(
                    problems,
                    "a setData object contains a shorthand or method member",
                );
            }
            MemberKind::Property => {
                let Some(name) = &member.name else {
                    *complete = false;
                    push_reason(problems, "a setData key has no static name");
                    continue;
                };
                let Some(site_start) = key_content_start(script, &member) else {
                    *complete = false;
                    push_reason(problems, "a setData key could not be mapped");
                    continue;
                };
                let key_text = key_content_text(script, &member);
                let Some(key_text) = key_text else {
                    *complete = false;
                    push_reason(problems, "a setData key could not be mapped");
                    continue;
                };
                if key_text.contains('\\') {
                    *complete = false;
                    push_reason(
                        problems,
                        "a setData key contains escape sequences; segment offsets are not proven",
                    );
                    continue;
                }
                if key_text.len() != name.len() {
                    *complete = false;
                    push_reason(problems, "a setData key cannot be mapped exactly");
                    continue;
                }
                if name.is_empty()
                    || name.contains(['[', ']'])
                    || name.starts_with('.')
                    || name.ends_with('.')
                    || name.contains("..")
                {
                    *complete = false;
                    push_reason(
                        problems,
                        "a setData key uses an unsupported bracket or dotted path",
                    );
                    continue;
                }
                let segments = name.split('.').map(str::to_owned).collect::<Vec<_>>();
                if segments.iter().any(String::is_empty) {
                    *complete = false;
                    push_reason(problems, "a setData key contains an empty segment");
                    continue;
                }
                let path_segments = prefix
                    .iter()
                    .cloned()
                    .chain(segments.iter().cloned())
                    .collect::<Vec<_>>();
                let nested = script.is_punct(member.value_start, '{')
                    && script
                        .close_of(member.value_start)
                        .is_ok_and(|close| close + 1 == member.value_end);
                out.push(LiteralPath {
                    path: path_segments.clone(),
                    site_start,
                    segments,
                    prefix_len: prefix.len(),
                    nested,
                });
                if nested && let Ok(inner_close) = script.close_of(member.value_start) {
                    collect_paths(
                        script,
                        member.value_start,
                        inner_close,
                        &path_segments,
                        out,
                        complete,
                        problems,
                    );
                }
            }
        }
    }
}

fn key_content_start(script: &Script<'_>, member: &super::scanner::Member) -> Option<usize> {
    let text = script.text.get(member.key_start..member.key_end)?;
    if text.starts_with('"') || text.starts_with('\'') {
        Some(member.key_start + 1)
    } else {
        Some(member.key_start)
    }
}

fn key_content_text<'a>(
    script: &'a Script<'_>,
    member: &super::scanner::Member,
) -> Option<&'a str> {
    let text = script.text.get(member.key_start..member.key_end)?;
    if text.starts_with('"') || text.starts_with('\'') {
        if text.len() < 2 {
            return None;
        }
        text.get(1..text.len() - 1)
    } else {
        Some(text)
    }
}

fn collect_occurrence(
    script: &Script<'_>,
    path: &str,
    entry: &LiteralPath,
    target: &[String],
    occurrences: &mut Vec<SymbolOccurrenceFact>,
) {
    if entry.path.len() < target.len() {
        if paths_intersect(&entry.path, target) && !entry.nested {
            // The caller turns this into a partial reason via `complete`; the
            // reason is emitted once in `scan_set_data` below.
        }
        return;
    }
    for (index, segment) in target.iter().enumerate() {
        if entry.path.get(index) != Some(segment) {
            return;
        }
    }
    let segment = target.len() - 1;
    if segment < entry.prefix_len {
        return;
    }
    let local = segment - entry.prefix_len;
    let Some(local_segment) = entry.segments.get(local) else {
        return;
    };
    let mut offset = 0_usize;
    for previous in entry.segments.iter().take(local) {
        offset = offset.saturating_add(previous.len()).saturating_add(1);
    }
    let start = entry.site_start.saturating_add(offset);
    let end = start.saturating_add(local_segment.len());
    let Ok(start) = u32::try_from(start) else {
        return;
    };
    let Ok(end) = u32::try_from(end) else {
        return;
    };
    let Some(range) = script.source_range(SourceRange::new(start, end)).ok() else {
        return;
    };
    occurrences.push(SymbolOccurrenceFact {
        token: SymbolTokenFact {
            path: path.to_owned(),
            range,
            expected_text: Some(local_segment.clone()),
        },
        kind: SymbolOccurrenceKind::SetDataKey,
        path: entry.path.clone(),
        segment,
    });
}

fn paths_intersect(left: &[String], right: &[String]) -> bool {
    let depth = left.len().min(right.len());
    (0..depth).all(|index| left[index] == right[index])
}

/// Returns a reason when another script writes the same data path.
pub(crate) fn other_script_conflict(
    input: &AnalysisInput,
    subject: &str,
    target: &[String],
) -> Option<String> {
    for file in input.files() {
        if file.path() == subject {
            continue;
        }
        let Ok(script) = Script::new(file.text()) else {
            return Some("another script could not be scanned for setData".to_owned());
        };
        let own_registration =
            super::registration::scan(file).is_ok_and(|scan| !scan.registrations.is_empty());
        for index in 0..script.tokens.len() {
            if index == 0 || !script.is_ident(index, "setData") || !script.is_punct(index - 1, '.')
            {
                continue;
            }
            let receiver = index
                .checked_sub(2)
                .and_then(|index| script.token_text(index));
            if own_registration && receiver == Some("this") {
                continue;
            }
            let argument = index + 2;
            if !script.is_punct(argument, '{') {
                return Some("another script calls setData with a dynamic argument".to_owned());
            }
            let Ok(close) = script.close_of(argument) else {
                return Some("another script calls setData with a dynamic argument".to_owned());
            };
            let mut entries = Vec::new();
            let mut complete = true;
            let mut problems = Vec::new();
            collect_paths(
                &script,
                argument,
                close,
                &[],
                &mut entries,
                &mut complete,
                &mut problems,
            );
            if !complete {
                return Some("another script calls setData with a dynamic argument".to_owned());
            }
            if entries
                .iter()
                .any(|entry| paths_intersect(&entry.path, target))
            {
                return Some("another script calls setData with this data path".to_owned());
            }
        }
    }
    None
}

/// Rejects component observers that this slice does not analyze.
pub(crate) fn observer_escape(
    script: &Script<'_>,
    registration: &ParsedRegistration,
) -> Option<String> {
    if registration.fact.kind != RegistrationKind::Component {
        return None;
    }
    if member_last(&registration.members, "observers").is_some() {
        return Some("component observer paths are not analyzed".to_owned());
    }
    let member = member_last(&registration.members, "properties")?;
    let (open, close) = object_bounds(script, member)?;
    for property in scan_members(script, open + 1, close) {
        if property.kind != MemberKind::Property {
            continue;
        }
        let Some((option_open, option_close)) = object_bounds(script, &property) else {
            continue;
        };
        if member_last(
            &scan_members(script, option_open + 1, option_close),
            "observer",
        )
        .is_some()
        {
            return Some("component property observer paths are not analyzed".to_owned());
        }
    }
    None
}
