//! Document-symbol composition from native AST evidence.
//!
//! The native session walks the same-snapshot AST for the prepared view's
//! owning script. That script is sent as the in-place shadow, so every native
//! range is in shadow coordinates. This module validates the native UTF-8 and
//! UTF-16 pair against the exact text the engine saw, maps the declaration and
//! name-token ranges back through [`ShadowLayout`], and drops anything that does
//! not map into the original script. Ranges are never clamped.
//!
//! A symbol declared inside a mounted template fragment has no range in the
//! script document; it is dropped and the result becomes `Partial`. Generated
//! probe text is never folded into a script outline, and WXML outlines stay
//! static: script symbols are not merged into them.
//!
//! A checked empty native answer stays `Supported`. Native `Unsupported`,
//! transport failures, host degradation, truncation, skipped candidates and
//! mapping drops are visible through the result status and never turn into a
//! supported empty outline.

use crate::backend_tsrs::query::document_symbol::map_document_symbols;
use wxml_backend_tsrs::native::NativeError;
use wxml_backend_tsrs::native::strings::{Utf8Offset, utf8_to_utf16};
use wxml_text::SourceRange;
use wxml_types::{
    BackendError, CapabilityStatus, DocumentSymbol, DocumentSymbolGap, DocumentSymbolsResult,
    FileId,
};

use crate::backend_tsrs::input::RequestPlan;
use crate::backend_tsrs::map_native_error;
use crate::backend_tsrs::mapping::{NativeRange, ShadowLayout};
use crate::backend_tsrs::session::QuerySession;

/// Reason for a subject that is not the prepared view's owning script.
pub(crate) const OWNING_SCRIPT_ONLY: &str =
    "document symbols are only available for the prepared view's owning script";

/// Runs one document-symbol query for the prepared view's owning script.
///
/// `subject` must be the owning script of `plan`; every other subject is an
/// explicit `Unsupported` answer. The caller's `degradation` reason (if any) is
/// merged into the result status.
pub(crate) fn document_symbols(
    session: &mut dyn QuerySession,
    plan: &RequestPlan,
    subject: &FileId,
    degradation: Option<&str>,
) -> Result<DocumentSymbolsResult, BackendError> {
    if plan.file_id(&plan.identity.script_path).as_ref() != Some(subject) {
        return Ok(DocumentSymbolsResult::unsupported(OWNING_SCRIPT_ONLY));
    }
    let evidence = match session.document_symbols(&plan.identity.script_path) {
        Ok(evidence) => evidence,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(DocumentSymbolsResult::unsupported(
                "native document symbol queries are unavailable",
            ));
        }
        Err(error) => return Err(map_native_error(error)),
    };

    let native = map_document_symbols(&evidence);
    let backend = native.to_backend_result(plan.layout.derived());
    Ok(map_shadow_ranges(&plan.layout, backend, degradation))
}

/// Maps a C0 result from shadow coordinates to original script bytes.
fn map_shadow_ranges(
    layout: &ShadowLayout,
    backend: DocumentSymbolsResult,
    degradation: Option<&str>,
) -> DocumentSymbolsResult {
    let mut drops = ShadowDrops::default();
    let mut symbols = Vec::with_capacity(backend.symbols.len());
    for symbol in &backend.symbols {
        if let Some(mapped) = map_symbol(layout, symbol, None, &mut drops) {
            symbols.push(mapped);
        }
    }
    let gaps = backend
        .gaps
        .iter()
        .map(|gap| DocumentSymbolGap {
            start: gap.start.and_then(|offset| map_point(layout, offset)),
            reason: gap.reason.clone(),
        })
        .collect();

    let mut result = DocumentSymbolsResult {
        symbols,
        status: backend.status,
        truncated: backend.truncated,
        skipped: backend.skipped.saturating_add(drops.dropped_nodes),
        gaps,
    };
    if let Some(reason) = degradation {
        mark_partial(&mut result.status, reason);
    }
    drops.record(&mut result.status);
    result
}

fn map_symbol(
    layout: &ShadowLayout,
    symbol: &DocumentSymbol,
    parent: Option<SourceRange>,
    drops: &mut ShadowDrops,
) -> Option<DocumentSymbol> {
    let Some(range) = map_range(layout, symbol.range, drops) else {
        drops.drop_subtree(symbol);
        return None;
    };
    let Some(selection_range) = map_range(layout, symbol.selection_range, drops) else {
        drops.drop_subtree(symbol);
        return None;
    };
    if selection_range.start() < range.start() || selection_range.end() > range.end() {
        drops.selection_outside = drops.selection_outside.saturating_add(1);
        drops.drop_subtree(symbol);
        return None;
    }
    if let Some(parent) = parent
        && (range.start() < parent.start() || range.end() > parent.end())
    {
        drops.child_outside = drops.child_outside.saturating_add(1);
        drops.drop_subtree(symbol);
        return None;
    }
    let children = symbol
        .children
        .iter()
        .filter_map(|child| map_symbol(layout, child, Some(range), drops))
        .collect();
    Some(DocumentSymbol {
        name: symbol.name.clone(),
        kind: symbol.kind,
        range,
        selection_range,
        children,
    })
}

/// Maps one shadow byte range back to the original script.
///
/// A template-fragment range has no script owner; a range that crosses
/// generated text cannot be attributed; both are dropped instead of clamped.
fn map_range(
    layout: &ShadowLayout,
    range: SourceRange,
    drops: &mut ShadowDrops,
) -> Option<SourceRange> {
    let (Ok(start), Ok(end)) = (
        utf8_to_utf16(layout.derived(), Utf8Offset::new(range.start())),
        utf8_to_utf16(layout.derived(), Utf8Offset::new(range.end())),
    ) else {
        drops.unmappable = drops.unmappable.saturating_add(1);
        return None;
    };
    match layout.map_native_range(start.get(), end.get()) {
        Ok(NativeRange::Script { range }) => {
            if valid_span(layout.original_script(), range) {
                Some(range)
            } else {
                drops.invalid_script = drops.invalid_script.saturating_add(1);
                None
            }
        }
        Ok(NativeRange::Template { .. }) => {
            drops.template = drops.template.saturating_add(1);
            None
        }
        Err(_) => {
            drops.crossing = drops.crossing.saturating_add(1);
            None
        }
    }
}

/// Maps a shadow gap start back to an original-script byte offset.
///
/// A gap that belongs to a template fragment or generated text stays `None`;
/// an offset is never invented.
fn map_point(layout: &ShadowLayout, offset: u32) -> Option<u32> {
    let units = utf8_to_utf16(layout.derived(), Utf8Offset::new(offset))
        .ok()?
        .get();
    match layout.map_native_range(units, units).ok()? {
        NativeRange::Script { range } if valid_point(layout.original_script(), range.start()) => {
            Some(range.start())
        }
        _ => None,
    }
}

/// `true` when `range` is a non-empty range of UTF-8 character boundaries.
fn valid_span(text: &str, range: SourceRange) -> bool {
    let (Ok(start), Ok(end)) = (usize::try_from(range.start()), usize::try_from(range.end()))
    else {
        return false;
    };
    start < end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}

/// `true` when `offset` is a UTF-8 character boundary inside `text`.
fn valid_point(text: &str, offset: u32) -> bool {
    usize::try_from(offset)
        .is_ok_and(|offset| offset <= text.len() && text.is_char_boundary(offset))
}

/// Number of nodes in one symbol subtree, including the symbol itself.
fn subtree_nodes(symbol: &DocumentSymbol) -> usize {
    symbol.children.iter().fold(1usize, |total, child| {
        total.saturating_add(subtree_nodes(child))
    })
}

/// Symbols that could not be represented in original-script coordinates.
#[derive(Debug, Default)]
struct ShadowDrops {
    /// Every symbol node dropped, including descendants of a dropped parent.
    dropped_nodes: usize,
    template: usize,
    crossing: usize,
    unmappable: usize,
    invalid_script: usize,
    selection_outside: usize,
    child_outside: usize,
}

impl ShadowDrops {
    /// Counts one dropped symbol and its whole subtree.
    fn drop_subtree(&mut self, symbol: &DocumentSymbol) {
        self.dropped_nodes = self.dropped_nodes.saturating_add(subtree_nodes(symbol));
    }

    fn record(&self, status: &mut CapabilityStatus) {
        if self.template > 0 {
            mark_partial(
                status,
                &format!(
                    "{} document symbol(s) were declared inside a template fragment",
                    self.template
                ),
            );
        }
        if self.crossing > 0 {
            mark_partial(
                status,
                &format!(
                    "{} document symbol range(s) crossed generated shadow text",
                    self.crossing
                ),
            );
        }
        if self.unmappable > 0 {
            mark_partial(
                status,
                &format!(
                    "{} document symbol range(s) were not valid shadow offsets",
                    self.unmappable
                ),
            );
        }
        if self.invalid_script > 0 {
            mark_partial(
                status,
                &format!(
                    "{} document symbol range(s) were not valid script text",
                    self.invalid_script
                ),
            );
        }
        if self.selection_outside > 0 {
            mark_partial(
                status,
                &format!(
                    "{} document symbol selection range(s) were outside the declaration range",
                    self.selection_outside
                ),
            );
        }
        if self.child_outside > 0 {
            mark_partial(
                status,
                &format!(
                    "{} document symbol child range(s) were outside the parent range",
                    self.child_outside
                ),
            );
        }
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::assert_is_empty)]
mod tests {
    use wxml_backend_tsrs::shadow::mapping::ShadowMap;
    use wxml_backend_tsrs::shadow::rewrite::{Assembled, EditKind, ShadowEdit, apply_edits};
    use wxml_types::DocumentSymbolKind;

    use super::*;

    fn layout(original: &str, edits: &[ShadowEdit]) -> ShadowLayout {
        let assembled = apply_edits(original, edits).expect("edits");
        let map = ShadowMap::new(original, &assembled);
        ShadowLayout::new("<view/>", assembled.text, map, Vec::new())
    }

    fn symbol(
        name: &str,
        kind: DocumentSymbolKind,
        range: (u32, u32),
        selection: (u32, u32),
        children: Vec<DocumentSymbol>,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: name.to_owned(),
            kind,
            range: SourceRange::from_bounds(range.0, range.1).expect("range"),
            selection_range: SourceRange::from_bounds(selection.0, selection.1).expect("selection"),
            children,
        }
    }

    #[test]
    fn map_range_rejects_unmappable_and_out_of_original_ranges() {
        let non_boundary = layout("用户", &[]);
        let mut drops = ShadowDrops::default();
        assert!(
            map_range(
                &non_boundary,
                SourceRange::from_bounds(1, 4).expect("range"),
                &mut drops
            )
            .is_none()
        );
        assert_eq!(drops.unmappable, 1);

        let assembled = Assembled {
            text: "longer text".to_owned(),
            edits: Vec::new(),
        };
        let shorter_original = ShadowLayout::new(
            "<view/>",
            "longer text",
            ShadowMap::new("short", &assembled),
            Vec::new(),
        );
        let mut drops = ShadowDrops::default();
        assert!(
            map_range(
                &shorter_original,
                SourceRange::from_bounds(0, 6).expect("range"),
                &mut drops
            )
            .is_none()
        );
        assert_eq!(drops.invalid_script, 1);
    }

    #[test]
    fn map_range_reports_a_crossing_edit_as_a_mapping_gap() {
        let edit = ShadowEdit::insert("probe", EditKind::Probe, 8, "x");
        let crossing = layout("class C {}\n", &[edit]);
        let mut drops = ShadowDrops::default();
        assert!(
            map_range(
                &crossing,
                SourceRange::from_bounds(0, 11).expect("range"),
                &mut drops
            )
            .is_none()
        );
        assert_eq!(drops.crossing, 1);
    }

    #[test]
    fn map_symbol_rejects_selection_and_child_containment_violations() {
        let identity = layout("aurora", &[]);
        let mut drops = ShadowDrops::default();
        let outside_selection = symbol("a", DocumentSymbolKind::Class, (2, 4), (0, 1), Vec::new());
        assert!(map_symbol(&identity, &outside_selection, None, &mut drops).is_none());
        assert_eq!(drops.selection_outside, 1);
        assert_eq!(drops.dropped_nodes, 1);

        let mut drops = ShadowDrops::default();
        let child = symbol(
            "b",
            DocumentSymbolKind::Property,
            (4, 6),
            (4, 5),
            Vec::new(),
        );
        let parent = symbol("a", DocumentSymbolKind::Class, (0, 4), (0, 1), vec![child]);
        let mapped = map_symbol(&identity, &parent, None, &mut drops).expect("parent");
        assert!(mapped.children.is_empty());
        assert_eq!(drops.child_outside, 1);
        assert_eq!(drops.dropped_nodes, 1);
    }

    #[test]
    fn map_shadow_ranges_counts_whole_dropped_subtrees_in_skipped() {
        let identity = layout("aurora", &[]);
        let grandchild = symbol(
            "c",
            DocumentSymbolKind::Variable,
            (4, 5),
            (4, 5),
            Vec::new(),
        );
        let doomed_child = symbol(
            "b",
            DocumentSymbolKind::Property,
            (3, 5),
            (3, 5),
            vec![grandchild],
        );
        // The selection range is outside the declaration range, so the whole
        // parent subtree is dropped and must be counted.
        let doomed = symbol(
            "a",
            DocumentSymbolKind::Class,
            (0, 5),
            (0, 6),
            vec![doomed_child],
        );
        // A valid root stays; only the invalid subtree is dropped.
        let kept_child = symbol(
            "d",
            DocumentSymbolKind::Property,
            (4, 6),
            (4, 5),
            Vec::new(),
        );
        let kept = symbol(
            "e",
            DocumentSymbolKind::Function,
            (3, 6),
            (3, 6),
            vec![kept_child],
        );
        let result = DocumentSymbolsResult {
            symbols: vec![doomed, kept],
            status: CapabilityStatus::Supported,
            truncated: false,
            skipped: 2,
            gaps: Vec::new(),
        };
        let mapped = map_shadow_ranges(&identity, result, None);
        assert_eq!(mapped.symbols.len(), 1);
        assert_eq!(mapped.symbols[0].name, "e");
        assert_eq!(mapped.symbols[0].children.len(), 1);
        assert_eq!(mapped.skipped, 2 + 3);
        assert!(matches!(mapped.status, CapabilityStatus::Partial { .. }));
        let reason = mapped.status.reason().expect("partial reason");
        assert!(reason.contains("outside the declaration range"), "{reason}");
        assert!(!reason.contains("template fragment"), "{reason}");
    }

    #[test]
    fn map_point_rejects_non_boundaries_out_of_range_and_templates() {
        let identity = layout("aurora", &[]);
        assert_eq!(map_point(&identity, 0), Some(0));
        assert_eq!(map_point(&identity, 6), Some(6));
        assert_eq!(map_point(&identity, 99), None);
        let multi_byte = layout("用户", &[]);
        assert_eq!(map_point(&multi_byte, 1), None);
    }
}
