//! Expression rewriting and the deterministic edit engine.
//!
//! [`rewrite_expression`] turns one WXML expression into the text mounted in
//! the probe. Free roots get a prefix (none for template locals, the property
//! overlay prefix for declared properties, `__wxml_root.` otherwise); member
//! names, object keys, strings, template text, comments and globals stay
//! verbatim. Object shorthand `{ title }` is expanded to
//! `{ title: __wxml_root.title }` so the key keeps its meaning.
//!
//! [`apply_edits`] is the insertion engine used by the layout. It sorts edits
//! by UTF-16 offset, rejects duplicate ids and overlapping/ambiguous edits and
//! copies the original text verbatim outside the edits. Every generated range is
//! recorded in UTF-16 code units for downstream mapping.
//!
//! # Segment model
//!
//! A [`MountedSegment`] is either synthetic (no source) or a verbatim copy of a
//! byte range of the expression. Segments are merged across adjacent source
//! pieces exactly like the TypeScript oracle so diagnostics can be mapped back
//! without loss.

use std::collections::BTreeSet;

use crate::native::strings::Utf8Offset;
use crate::shadow::ShadowError;
use crate::shadow::expression::{self, ValueIdentifier};
use crate::shadow::mapping::{SourceRange, utf16_len, utf16_to_byte};

const MAX_HOVER_TARGETS: usize = 32;

/// Values that are not template data roots.
const GLOBALS: &[&str] = &[
    "Array",
    "BigInt",
    "Boolean",
    "Date",
    "Infinity",
    "Intl",
    "JSON",
    "Map",
    "Math",
    "NaN",
    "Number",
    "Object",
    "Promise",
    "RegExp",
    "Set",
    "String",
    "Symbol",
    "WeakMap",
    "WeakSet",
    "console",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "undefined",
];

/// Prefix and overlay configuration for one expression rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteOptions {
    /// Prefix inserted before a free data root, for example `__wxml_root.`.
    pub root_prefix: String,
    /// Prefix inserted before a declared component property.
    pub props_prefix: String,
    /// Property names read from the declaration-derived overlay.
    pub property_overlay: BTreeSet<String>,
    /// Behavior `data` names materialized as probe-local values.
    pub data_overlay: BTreeSet<String>,
    /// Template locals that keep their source text unchanged.
    pub locals: BTreeSet<String>,
}

impl Default for RewriteOptions {
    fn default() -> Self {
        Self {
            root_prefix: "__wxml_root.".to_owned(),
            props_prefix: "__wxml_props_0.".to_owned(),
            property_overlay: BTreeSet::new(),
            data_overlay: BTreeSet::new(),
            locals: BTreeSet::new(),
        }
    }
}

/// One piece of a mounted expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedSegment {
    /// Offset inside the mounted expression, UTF-16 code units.
    pub start_utf16: u32,
    /// Length in UTF-16 code units.
    pub length_utf16: u32,
    /// Original WXML byte range for verbatim copies; `None` for synthetic text.
    pub source: Option<SourceRange>,
}

/// One identifier in the mounted expression with its WXML byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverTarget {
    /// Offset inside the mounted expression, UTF-16 code units.
    pub offset_utf16: u32,
    /// Identifier range in the original expression bytes.
    pub source: SourceRange,
}

/// Result of rewriting one expression for the shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewrittenExpression {
    /// Mounted text with prefixes inserted.
    pub text: String,
    /// Segments relative to [`Self::text`].
    pub segments: Vec<MountedSegment>,
    /// Target identifier offset relative to [`Self::text`], UTF-16 units.
    pub target_offset_utf16: u32,
    /// Every identifier that can answer hover in the mounted text.
    pub hover_targets: Vec<HoverTarget>,
    /// Free root identifiers in source order, deduplicated.
    pub roots: Vec<String>,
    /// True when every free root is a template local or data overlay local.
    pub local_root: bool,
}

#[derive(Debug)]
struct Piece {
    text: String,
    source: Option<(usize, usize)>,
}

#[derive(Debug)]
struct MappedPiece {
    source: (usize, usize),
    mounted_start_utf16: u32,
}

/// Rewrites one WXML expression into its shadow-mounted form.
///
/// `target` is an explicit whole identifier; `None` selects the rightmost value
/// identifier, matching the TypeScript oracle. Targets that only occur inside
/// strings/comments are rejected.
#[allow(clippy::too_many_lines)] // linear emitter; splitting would duplicate piece bookkeeping
pub fn rewrite_expression(
    expr: &str,
    target: Option<&str>,
    options: &RewriteOptions,
) -> Result<RewrittenExpression, ShadowError> {
    let collected = expression::collect_value_identifiers(expr)?;

    let mut occurrences: Vec<ValueIdentifier> = collected.roots;
    occurrences.retain(|occurrence| {
        let name = &expr[occurrence.start..occurrence.end];
        !GLOBALS.contains(&name) || options.locals.contains(name)
    });
    occurrences.sort_by_key(|occurrence| occurrence.start);
    for pair in occurrences.windows(2) {
        if pair[0].start == pair[1].start || pair[0].end > pair[1].start {
            return Err(ShadowError::BadExpression {
                detail: format!(
                    "overlapping root identifiers in {}",
                    expression::describe_expression(expr)
                ),
            });
        }
    }
    if occurrences.is_empty() {
        return Err(ShadowError::BadExpression {
            detail: format!(
                "expression has no free root identifier: {}",
                expression::describe_expression(expr)
            ),
        });
    }

    let mut pieces: Vec<Piece> = Vec::new();
    let mut mapped_pieces: Vec<MappedPiece> = Vec::new();
    let mut mounted_utf16 = 0_u32;

    let push_source = |from: usize,
                       to: usize,
                       map: bool,
                       reuse: Option<(usize, usize)>,
                       pieces: &mut Vec<Piece>,
                       mapped_pieces: &mut Vec<MappedPiece>,
                       mounted_utf16: &mut u32|
     -> Option<(usize, usize)> {
        if to <= from {
            return None;
        }
        let source = reuse.unwrap_or((from, to));
        let text = expr[from..to].to_owned();
        let width = utf16_len(&text);
        if map {
            mapped_pieces.push(MappedPiece {
                source,
                mounted_start_utf16: *mounted_utf16,
            });
        }
        pieces.push(Piece {
            text,
            source: Some(source),
        });
        *mounted_utf16 += width;
        Some(source)
    };

    let push_synthetic = |text: &str, pieces: &mut Vec<Piece>, mounted_utf16: &mut u32| {
        if text.is_empty() {
            return;
        }
        let width = utf16_len(text);
        pieces.push(Piece {
            text: text.to_owned(),
            source: None,
        });
        *mounted_utf16 += width;
    };

    let mut cursor = 0;
    for occurrence in &occurrences {
        if occurrence.start < cursor {
            return Err(ShadowError::BadExpression {
                detail: format!(
                    "overlapping root identifiers in {}",
                    expression::describe_expression(expr)
                ),
            });
        }
        push_source(
            cursor,
            occurrence.start,
            true,
            None,
            &mut pieces,
            &mut mapped_pieces,
            &mut mounted_utf16,
        );
        let name = &expr[occurrence.start..occurrence.end];
        let prefix = if options.locals.contains(name) || options.data_overlay.contains(name) {
            ""
        } else if options.property_overlay.contains(name) {
            options.props_prefix.as_str()
        } else {
            options.root_prefix.as_str()
        };
        if occurrence.shorthand {
            let key_range = push_source(
                occurrence.start,
                occurrence.end,
                false,
                None,
                &mut pieces,
                &mut mapped_pieces,
                &mut mounted_utf16,
            );
            let prefix_text = if prefix.is_empty() {
                ": ".to_owned()
            } else {
                format!(": {prefix}")
            };
            push_synthetic(&prefix_text, &mut pieces, &mut mounted_utf16);
            push_source(
                occurrence.start,
                occurrence.end,
                true,
                key_range,
                &mut pieces,
                &mut mapped_pieces,
                &mut mounted_utf16,
            );
        } else {
            push_synthetic(prefix, &mut pieces, &mut mounted_utf16);
            push_source(
                occurrence.start,
                occurrence.end,
                true,
                None,
                &mut pieces,
                &mut mapped_pieces,
                &mut mounted_utf16,
            );
        }
        cursor = occurrence.end;
    }
    push_source(
        cursor,
        expr.len(),
        true,
        None,
        &mut pieces,
        &mut mapped_pieces,
        &mut mounted_utf16,
    );

    let text: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
    let target = resolve_target(expr, target, &collected.values, &collected.all)?;
    let target_offset_utf16 =
        map_source_offset(expr, target.start, &mapped_pieces).ok_or_else(|| {
            ShadowError::BadExpression {
                detail: format!(
                    "target {} is not part of the mounted expression: {}",
                    &expr[target.start..target.end],
                    expression::describe_expression(expr)
                ),
            }
        })?;

    let segments = build_segments(&pieces)?;
    let hover_targets = build_hover_targets(expr, &collected.all, &mapped_pieces)?;
    let mut roots: Vec<String> = Vec::new();
    for occurrence in &occurrences {
        let name = expr[occurrence.start..occurrence.end].to_owned();
        if !roots.contains(&name) {
            roots.push(name);
        }
    }
    let local_root = !roots.is_empty()
        && roots
            .iter()
            .all(|root| options.locals.contains(root) || options.data_overlay.contains(root));

    Ok(RewrittenExpression {
        text,
        segments,
        target_offset_utf16,
        hover_targets,
        roots,
        local_root,
    })
}

fn resolve_target(
    expr: &str,
    target: Option<&str>,
    values: &[ValueIdentifier],
    all: &[ValueIdentifier],
) -> Result<ValueIdentifier, ShadowError> {
    if let Some(name) = target {
        let mut candidates: Vec<&ValueIdentifier> = all
            .iter()
            .filter(|identifier| &expr[identifier.start..identifier.end] == name)
            .collect();
        candidates.sort_by_key(|identifier| identifier.start);
        return candidates
            .first()
            .copied()
            .copied()
            .ok_or_else(|| ShadowError::BadExpression {
                detail: format!(
                    "target {name} is not a whole identifier outside strings/comments: {}",
                    expression::describe_expression(expr)
                ),
            });
    }
    let mut candidates = values.to_vec();
    candidates.sort_by_key(|identifier| identifier.start);
    candidates
        .last()
        .copied()
        .ok_or_else(|| ShadowError::BadExpression {
            detail: format!(
                "expression has no identifier to target: {}",
                expression::describe_expression(expr)
            ),
        })
}

fn map_source_offset(expr: &str, offset: usize, mapped: &[MappedPiece]) -> Option<u32> {
    mapped.iter().find_map(|piece| {
        if offset >= piece.source.0 && offset < piece.source.1 {
            let prefix = &expr[piece.source.0..offset];
            Some(piece.mounted_start_utf16 + utf16_len(prefix))
        } else {
            None
        }
    })
}

fn build_segments(pieces: &[Piece]) -> Result<Vec<MountedSegment>, ShadowError> {
    let mut segments: Vec<MountedSegment> = Vec::new();
    let mut mounted_cursor = 0_u32;
    for piece in pieces {
        let width = utf16_len(&piece.text);
        if let Some(source) = piece.source {
            let range = byte_range(source.0, source.1)?;
            let can_merge = segments.last().is_some_and(|previous| {
                previous.source.is_some_and(|previous_source| {
                    previous.start_utf16 + previous.length_utf16 == mounted_cursor
                        && previous_source.end == range.start
                })
            });
            if can_merge {
                if let Some(previous) = segments.last_mut() {
                    previous.length_utf16 += width;
                    previous.source = Some(SourceRange {
                        start: previous.source.map_or(range.start, |source| source.start),
                        end: range.end,
                    });
                }
            } else {
                segments.push(MountedSegment {
                    start_utf16: mounted_cursor,
                    length_utf16: width,
                    source: Some(range),
                });
            }
        } else {
            segments.push(MountedSegment {
                start_utf16: mounted_cursor,
                length_utf16: width,
                source: None,
            });
        }
        mounted_cursor += width;
    }
    Ok(segments)
}

fn build_hover_targets(
    expr: &str,
    values: &[ValueIdentifier],
    mapped: &[MappedPiece],
) -> Result<Vec<HoverTarget>, ShadowError> {
    let mut sorted = values.to_vec();
    sorted.sort_by_key(|identifier| identifier.start);
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for identifier in sorted {
        if !seen.insert(identifier.start) {
            continue;
        }
        if let Some(offset) = map_source_offset(expr, identifier.start, mapped) {
            targets.push(HoverTarget {
                offset_utf16: offset,
                source: byte_range(identifier.start, identifier.end)?,
            });
            if targets.len() >= MAX_HOVER_TARGETS {
                break;
            }
        }
    }
    Ok(targets)
}

fn byte_range(start: usize, end: usize) -> Result<SourceRange, ShadowError> {
    let start = u32::try_from(start).map_err(|_| ShadowError::PositionOutOfRange {
        position: u32::MAX,
        length: u32::MAX,
        unit: "utf-8",
    })?;
    let end = u32::try_from(end).map_err(|_| ShadowError::PositionOutOfRange {
        position: u32::MAX,
        length: u32::MAX,
        unit: "utf-8",
    })?;
    Ok(SourceRange {
        start: Utf8Offset::new(start),
        end: Utf8Offset::new(end),
    })
}

/// Kind of generated edit, mirrored in insertion records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Generated probe member.
    Probe,
    /// Generated `// @ts-check` / `// @ts-nocheck` pragma.
    Pragma,
    /// BOM removal edit.
    Bom,
}

impl EditKind {
    /// Stable string used by public insertion records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Pragma => "pragma",
            Self::Bom => "bom",
        }
    }
}

/// One exact replacement in the original script, in UTF-16 coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowEdit {
    /// Stable edit identifier.
    pub id: String,
    /// Edit kind.
    pub kind: EditKind,
    /// Inclusive UTF-16 start in the original text.
    pub at_utf16: u32,
    /// Number of original UTF-16 code units replaced (0 for pure insertions).
    pub replace_length_utf16: u32,
    /// Replacement text.
    pub text: String,
}

impl ShadowEdit {
    /// Creates a pure insertion.
    #[must_use]
    pub fn insert(
        id: impl Into<String>,
        kind: EditKind,
        at_utf16: u32,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            at_utf16,
            replace_length_utf16: 0,
            text: text.into(),
        }
    }

    /// Creates a replacement.
    #[must_use]
    pub fn replace(
        id: impl Into<String>,
        kind: EditKind,
        at_utf16: u32,
        replace_length_utf16: u32,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            at_utf16,
            replace_length_utf16,
            text: text.into(),
        }
    }
}

/// One edit applied to the original text with derived coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    /// Stable edit identifier.
    pub id: String,
    /// Edit kind.
    pub kind: EditKind,
    /// Inclusive UTF-16 start in the original text.
    pub at_utf16: u32,
    /// Replaced UTF-16 units in the original text.
    pub replace_length_utf16: u32,
    /// Inclusive UTF-8 byte start in the original text.
    pub at_byte: usize,
    /// Replaced UTF-8 bytes in the original text.
    pub replace_length_bytes: usize,
    /// Inclusive UTF-16 start in the derived text.
    pub inserted_start_utf16: u32,
    /// Inclusive UTF-8 byte start in the derived text.
    pub inserted_start_byte: usize,
    /// Inserted text length in UTF-16 code units.
    pub inserted_length_utf16: u32,
    /// Inserted text length in UTF-8 bytes.
    pub inserted_length_bytes: usize,
}

/// Result of applying a validated edit plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    /// Derived text.
    pub text: String,
    /// Applied edits in original order.
    pub edits: Vec<AppliedEdit>,
}

/// Applies ordered, non-overlapping edits to `original`.
///
/// Edits are sorted by `at_utf16`; duplicate ids, duplicate offsets, overlaps
/// and out-of-range/surrogate-splitting positions are rejected. The original
/// text is copied verbatim outside the edits.
pub fn apply_edits(original: &str, edits: &[ShadowEdit]) -> Result<Assembled, ShadowError> {
    let mut ids = BTreeSet::new();
    for edit in edits {
        if !ids.insert(edit.id.as_str()) {
            return Err(ShadowError::DuplicateEditId {
                id: edit.id.clone(),
            });
        }
    }
    let mut ordered = edits.to_vec();
    ordered.sort_by_key(|edit| edit.at_utf16);
    let text_length = utf16_len(original);
    let mut applied = Vec::with_capacity(ordered.len());
    let mut cursor_byte = 0;
    let mut cursor_utf16 = 0;
    let mut cursor_derived_byte = 0;
    let mut previous: Option<&ShadowEdit> = None;
    for edit in &ordered {
        let replace_end = edit
            .at_utf16
            .checked_add(edit.replace_length_utf16)
            .ok_or_else(|| ShadowError::EditOutOfRange {
                id: edit.id.clone(),
                at: edit.at_utf16,
                length: edit.replace_length_utf16,
                text_length,
            })?;
        if replace_end > text_length {
            return Err(ShadowError::EditOutOfRange {
                id: edit.id.clone(),
                at: edit.at_utf16,
                length: edit.replace_length_utf16,
                text_length,
            });
        }
        if let Some(previous) = previous {
            let previous_end = previous.at_utf16 + previous.replace_length_utf16;
            if edit.at_utf16 < previous_end || edit.at_utf16 == previous.at_utf16 {
                return Err(ShadowError::EditOverlap {
                    first: previous.id.clone(),
                    second: edit.id.clone(),
                });
            }
        }
        let at_byte = utf16_to_byte(original, edit.at_utf16)?;
        let end_byte = utf16_to_byte(original, replace_end)?;
        if at_byte < cursor_byte {
            return Err(ShadowError::EditOverlap {
                first: previous.map_or_else(String::new, |previous| previous.id.clone()),
                second: edit.id.clone(),
            });
        }
        cursor_utf16 += utf16_len(&original[cursor_byte..at_byte]);
        let inserted_length = utf16_len(&edit.text);
        cursor_derived_byte += at_byte - cursor_byte;
        applied.push(AppliedEdit {
            id: edit.id.clone(),
            kind: edit.kind,
            at_utf16: edit.at_utf16,
            replace_length_utf16: edit.replace_length_utf16,
            at_byte,
            replace_length_bytes: end_byte - at_byte,
            inserted_start_utf16: cursor_utf16,
            inserted_start_byte: cursor_derived_byte,
            inserted_length_utf16: inserted_length,
            inserted_length_bytes: edit.text.len(),
        });
        cursor_utf16 += inserted_length;
        cursor_derived_byte += edit.text.len();
        cursor_byte = end_byte;
        previous = Some(edit);
    }

    let mut text = String::with_capacity(original.len());
    let mut cursor = 0;
    for (edit, source) in applied.iter().zip(ordered.iter()) {
        text.push_str(&original[cursor..edit.at_byte]);
        text.push_str(&source.text);
        cursor = edit.at_byte + edit.replace_length_bytes;
    }
    text.push_str(&original[cursor..]);

    Ok(Assembled {
        text,
        edits: applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(expr: &str) -> String {
        rewrite_expression(expr, None, &RewriteOptions::default())
            .expect("valid expression")
            .text
    }

    #[test]
    fn unary_parenthesized_and_multi_root_expressions_are_fully_rewritten() {
        assert_eq!(rewrite("!title"), "!__wxml_root.title");
        assert_eq!(rewrite("(title)"), "(__wxml_root.title)");
        assert_eq!(
            rewrite("title + user.name"),
            "__wxml_root.title + __wxml_root.user.name"
        );
        assert_eq!(rewrite("typeof title"), "typeof __wxml_root.title");
        assert_eq!(rewrite("-title"), "-__wxml_root.title");
        assert_eq!(
            rewrite("title ? title : user.name"),
            "__wxml_root.title ? __wxml_root.title : __wxml_root.user.name"
        );
    }

    #[test]
    fn indexed_array_roots_are_rewritten_before_the_index() {
        assert_eq!(rewrite("users[0].name"), "__wxml_root.users[0].name");
    }

    #[test]
    fn member_names_keys_strings_and_globals_are_not_rewritten() {
        assert_eq!(rewrite("user.name"), "__wxml_root.user.name");
        assert_eq!(
            rewrite("{ title: user.name, user }"),
            "{ title: __wxml_root.user.name, user: __wxml_root.user }"
        );
        assert_eq!(
            rewrite("user.name + \"title\""),
            "__wxml_root.user.name + \"title\""
        );
        assert_eq!(rewrite("Math.max(title)"), "Math.max(__wxml_root.title)");
        assert_eq!(rewrite("user?.name"), "__wxml_root.user?.name");
        assert_eq!(
            rewrite("format(user.name)"),
            "__wxml_root.format(__wxml_root.user.name)"
        );
    }

    #[test]
    fn template_text_stays_verbatim_while_substitutions_are_rewritten() {
        assert_eq!(rewrite("`title ${title}`"), "`title ${__wxml_root.title}`");
        assert_eq!(
            rewrite("`a ${user.name} b`"),
            "`a ${__wxml_root.user.name} b`"
        );
    }

    #[test]
    fn numeric_literals_do_not_become_identifiers() {
        assert_eq!(rewrite("title + 1e10"), "__wxml_root.title + 1e10");
        assert_eq!(rewrite("title + 0x1f"), "__wxml_root.title + 0x1f");
    }

    #[test]
    fn locals_keep_source_text_and_may_shadow_globals() {
        let options = RewriteOptions {
            locals: BTreeSet::from(["item".to_owned(), "Math".to_owned()]),
            ..RewriteOptions::default()
        };
        let result = rewrite_expression("!item.done + Math", None, &options).expect("valid");
        assert_eq!(result.text, "!item.done + Math");
        assert!(result.local_root);
        assert_eq!(result.roots, ["item", "Math"]);
    }

    #[test]
    fn object_shorthand_is_expanded_safely() {
        assert_eq!(rewrite("{ title }"), "{ title: __wxml_root.title }");
        let options = RewriteOptions {
            locals: BTreeSet::from(["Math".to_owned()]),
            ..RewriteOptions::default()
        };
        assert_eq!(
            rewrite_expression("{ Math }", None, &options)
                .expect("valid")
                .text,
            "{ Math: Math }"
        );
    }

    #[test]
    fn explicit_targets_inside_comments_or_strings_are_rejected() {
        for (expr, target) in [
            ("title /* name */", "name"),
            ("title // name", "name"),
            ("title + \"name\"", "name"),
        ] {
            assert!(
                rewrite_expression(expr, Some(target), &RewriteOptions::default()).is_err(),
                "{expr}"
            );
        }
    }

    #[test]
    fn case_analysis_0196_target_must_be_a_whole_identifier_in_the_expression() {
        // Oracle: `analysis.test.ts:196` — an explicit target that is not a
        // whole identifier token in the expression is a structured error, not a
        // silently ignored override.
        for target in ["nmae", "nam", "user.name"] {
            let error = rewrite_expression("user.name", Some(target), &RewriteOptions::default())
                .expect_err("target must be rejected");
            assert!(
                matches!(
                    &error,
                    ShadowError::BadExpression { detail }
                        if detail.contains("not a whole identifier")
                ),
                "{target}: {error}"
            );
        }

        // Positive controls: both the root and a member name are whole
        // identifier tokens, so they resolve to their exact mounted offsets.
        let root = rewrite_expression("user.name", Some("user"), &RewriteOptions::default())
            .expect("root target");
        assert_eq!(root.target_offset_utf16, 12);
        let member = rewrite_expression("user.name", Some("name"), &RewriteOptions::default())
            .expect("member target");
        assert_eq!(member.target_offset_utf16, 17);
    }

    #[test]
    fn rewrites_preserve_source_segments_and_target_offsets() {
        let result = rewrite_expression("!title", None, &RewriteOptions::default()).expect("valid");
        assert_eq!(result.text, "!__wxml_root.title");
        assert_eq!(result.target_offset_utf16, 13);
        assert_eq!(
            result.segments[0].source,
            Some(SourceRange {
                start: Utf8Offset::new(0),
                end: Utf8Offset::new(1)
            }),
            "the leading `!` stays a verbatim copy"
        );
        assert!(
            result
                .segments
                .iter()
                .any(|segment| segment.source.is_none()),
            "the root prefix is synthetic"
        );
        assert_eq!(
            result.segments.last().and_then(|segment| segment.source),
            Some(SourceRange {
                start: Utf8Offset::new(1),
                end: Utf8Offset::new(6)
            })
        );
    }

    #[test]
    fn apply_edits_preserves_original_text_and_maps_offsets() {
        let original = "abc";
        let edits = vec![ShadowEdit::insert("probe", EditKind::Probe, 0, "x ")];
        let assembled = apply_edits(original, &edits).expect("valid edits");
        assert_eq!(assembled.text, "x abc");
        assert_eq!(assembled.edits[0].inserted_start_utf16, 0);
        assert_eq!(assembled.edits[0].inserted_length_utf16, 2);
    }

    #[test]
    fn apply_edits_rejects_overlaps_duplicates_and_out_of_range() {
        let original = "abc";
        let overlap = vec![
            ShadowEdit::insert("a", EditKind::Probe, 1, "x"),
            ShadowEdit::insert("b", EditKind::Probe, 1, "y"),
        ];
        assert!(matches!(
            apply_edits(original, &overlap),
            Err(ShadowError::EditOverlap { .. })
        ));
        let duplicate_ids = vec![
            ShadowEdit::insert("same", EditKind::Probe, 0, "x"),
            ShadowEdit::insert("same", EditKind::Probe, 3, "y"),
        ];
        assert!(matches!(
            apply_edits(original, &duplicate_ids),
            Err(ShadowError::DuplicateEditId { .. })
        ));
        assert!(matches!(
            apply_edits(
                original,
                &[ShadowEdit::insert("far", EditKind::Probe, 4, "z")]
            ),
            Err(ShadowError::EditOutOfRange { .. })
        ));
    }

    #[test]
    fn empty_edit_plan_is_the_identity() {
        let assembled = apply_edits("a😀b", &[]).expect("valid");
        assert_eq!(assembled.text, "a😀b");
        assert_eq!(assembled.edits, Vec::new());
    }

    #[test]
    fn rewriting_is_deterministic_and_idempotent_as_a_value() {
        let first = rewrite_expression("title + user.name", None, &RewriteOptions::default())
            .expect("valid");
        let second = rewrite_expression("title + user.name", None, &RewriteOptions::default())
            .expect("valid");
        assert_eq!(first, second);
    }

    #[test]
    fn expressions_without_a_free_root_are_rejected() {
        for expr in ["1 + 2", "Math.random()", "\"name\""] {
            assert!(
                matches!(
                    rewrite_expression(expr, None, &RewriteOptions::default()),
                    Err(ShadowError::BadExpression { .. })
                ),
                "{expr}"
            );
        }
    }
}
