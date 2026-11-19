//! Document-symbol presentation from native AST evidence.
//!
//! The native session walks the same-snapshot `getSourceFile` AST and returns
//! [`DocumentSymbolsEvidence`]. This module maps frozen node kinds to LSP
//! `SymbolKind` numbers, converts every exact range to both UTF-8 bytes and
//! UTF-16 code units, and makes incompleteness explicit through
//! [`CapabilityStatus`].
//!
//! LSP has no distinct `type alias` kind; `TypeAliasDeclaration` maps to
//! `SymbolKind.Interface` while [`DocumentSymbol::node_kind`] preserves the
//! distinction for callers. `ModuleDeclaration` maps to `SymbolKind.Module`
//! for both `namespace` and `module` spellings.
//!
//! An empty symbol list with [`CapabilityStatus::Supported`] is a checked
//! empty file. Any skipped candidate or truncation makes the result `Partial`;
//! this module never turns dropped ranges into a supported empty answer.
//!
//! The C0 conversion in [`DocumentSymbolResult::to_backend_result`] re-applies
//! the shared symbol and depth caps, verifies both coordinate systems against
//! the source text, and accounts every dropped input node (including a dropped
//! parent's whole subtree) in `skipped`. Native truncation and gap reasons are
//! preserved verbatim in the `Partial` reason.

use std::fmt::Write as _;

use wxml_types::{
    CapabilityStatus, DocumentSymbol as BackendDocumentSymbol,
    DocumentSymbolGap as BackendDocumentSymbolGap, DocumentSymbolKind as BackendDocumentSymbolKind,
    DocumentSymbolsResult as BackendDocumentSymbolsResult, SourceRange as BackendSourceRange,
};

use crate::native::ast::{
    CLASS_DECLARATION_KIND, ENUM_DECLARATION_KIND, ENUM_MEMBER_KIND, FUNCTION_DECLARATION_KIND,
    GET_ACCESSOR_KIND, INTERFACE_DECLARATION_KIND, METHOD_DECLARATION_KIND, METHOD_SIGNATURE_KIND,
    MODULE_DECLARATION_KIND, PROPERTY_DECLARATION_KIND, PROPERTY_SIGNATURE_KIND, SET_ACCESSOR_KIND,
    TYPE_ALIAS_DECLARATION_KIND, VARIABLE_DECLARATION_KIND,
};
use crate::native::document_symbol::{
    DocumentSymbolEvidence, DocumentSymbolGap, DocumentSymbolsEvidence, MAX_DOCUMENT_SYMBOLS,
};
use crate::native::strings::{Utf8Offset, Utf16Offset, utf16_to_utf8};
use crate::script::symbols::Utf16Range;
use crate::shadow::mapping::SourceRange;

/// LSP `SymbolKind` values used by native document symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum DocumentSymbolKind {
    /// `SymbolKind.Module` (2).
    Module = 2,
    /// `SymbolKind.Class` (5).
    Class = 5,
    /// `SymbolKind.Method` (6).
    Method = 6,
    /// `SymbolKind.Property` (7).
    Property = 7,
    /// `SymbolKind.Enum` (10).
    Enum = 10,
    /// `SymbolKind.Interface` (11).
    Interface = 11,
    /// `SymbolKind.Function` (12).
    Function = 12,
    /// `SymbolKind.Variable` (13).
    Variable = 13,
    /// `SymbolKind.EnumMember` (22).
    EnumMember = 22,
}

impl DocumentSymbolKind {
    /// Maps a frozen v9 node kind to its LSP symbol kind.
    #[must_use]
    pub const fn from_node_kind(kind: u32) -> Option<Self> {
        match kind {
            FUNCTION_DECLARATION_KIND => Some(Self::Function),
            CLASS_DECLARATION_KIND => Some(Self::Class),
            INTERFACE_DECLARATION_KIND | TYPE_ALIAS_DECLARATION_KIND => Some(Self::Interface),
            ENUM_DECLARATION_KIND => Some(Self::Enum),
            MODULE_DECLARATION_KIND => Some(Self::Module),
            VARIABLE_DECLARATION_KIND => Some(Self::Variable),
            PROPERTY_DECLARATION_KIND
            | PROPERTY_SIGNATURE_KIND
            | GET_ACCESSOR_KIND
            | SET_ACCESSOR_KIND => Some(Self::Property),
            METHOD_DECLARATION_KIND | METHOD_SIGNATURE_KIND => Some(Self::Method),
            ENUM_MEMBER_KIND => Some(Self::EnumMember),
            _ => None,
        }
    }

    /// Returns the numeric LSP `SymbolKind` value.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// One publishable document symbol with exact UTF-8 and UTF-16 ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    /// Verified symbol name.
    pub name: String,
    /// LSP symbol kind.
    pub kind: DocumentSymbolKind,
    /// Frozen v9 node kind, preserving distinctions LSP cannot express.
    pub node_kind: u32,
    /// Declaration range in UTF-8 bytes (includes leading trivia).
    pub range: SourceRange,
    /// The same declaration range in UTF-16 code units.
    pub utf16: Utf16Range,
    /// Exact name-token range in UTF-8 bytes.
    pub selection_range: SourceRange,
    /// The same name-token range in UTF-16 code units.
    pub selection_utf16: Utf16Range,
    /// Nested members or namespace declarations.
    pub children: Vec<Self>,
}

/// Document-symbol query result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbolResult {
    /// Decoded symbols in deterministic source order.
    pub symbols: Vec<DocumentSymbol>,
    /// Completeness of the answer.
    pub status: CapabilityStatus,
    /// Whether [`MAX_DOCUMENT_SYMBOLS`] cut the tree short.
    pub truncated: bool,
    /// First skipped candidates, bounded by the native gap cap.
    pub gaps: Vec<DocumentSymbolGap>,
    /// Total skipped candidates, including those beyond `gaps`.
    pub skipped: usize,
}

impl DocumentSymbolResult {
    /// Converts this query result to the shared C0 document-symbol DTO.
    ///
    /// `source` must be the exact script text the evidence was decoded from.
    /// Both coordinate systems are re-checked against it: a symbol whose UTF-8
    /// and UTF-16 ranges disagree, whose range is empty or reversed, whose
    /// selection range is outside its declaration range, or whose range is
    /// outside its parent is dropped and the result stays `Partial`. Ranges are
    /// never clamped.
    ///
    /// The conversion re-applies [`wxml_types::MAX_DOCUMENT_SYMBOLS`] and
    /// [`wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH`] instead of trusting native
    /// input. A dropped parent drops its whole subtree; every dropped input
    /// node is counted in the returned `skipped` value. Native `truncated` is
    /// preserved, and conversion over-count also sets it. A `Supported` query
    /// status without gaps, truncation or mapping drops stays `Supported`; any
    /// drop makes it `Partial` with the exact native reason text preserved.
    /// Already-degraded `Unsupported`, `Disabled`, `Unavailable` and `Unknown`
    /// statuses are preserved when nothing is dropped; a conversion drop still
    /// forces `Partial`. Gap starts convert from UTF-16 to UTF-8 when possible
    /// and stay `None` otherwise; gaps are bounded by
    /// [`wxml_types::MAX_DOCUMENT_SYMBOL_GAPS`].
    #[must_use]
    pub fn to_backend_result(&self, source: &str) -> BackendDocumentSymbolsResult {
        let mut mapping = MappingState::new(source);
        let symbols = self
            .symbols
            .iter()
            .filter_map(|symbol| mapping.map_node(symbol, 0, None))
            .collect();
        let gaps = self
            .gaps
            .iter()
            .take(wxml_types::MAX_DOCUMENT_SYMBOL_GAPS)
            .map(|gap| BackendDocumentSymbolGap {
                start: utf16_to_utf8(source, Utf16Offset::new(gap.start_utf16))
                    .ok()
                    .map(Utf8Offset::get),
                reason: gap.reason.to_owned(),
            })
            .collect();
        let truncated = self.truncated || mapping.truncated;
        let skipped = self.skipped.saturating_add(mapping.drops.dropped_nodes);

        let mut status = self.status.clone();
        let drop_reasons = mapping.drops.reasons();
        if status.is_supported() {
            let mut reasons =
                native_incompleteness_reasons(self.truncated, self.skipped, &self.gaps);
            reasons.extend(drop_reasons);
            if !reasons.is_empty() {
                status = CapabilityStatus::Partial {
                    reason: reasons.join("; "),
                };
            }
        } else if let CapabilityStatus::Partial { reason } = &mut status {
            for addition in drop_reasons {
                append_reason(reason, &addition);
            }
        } else if !drop_reasons.is_empty() {
            let mut reasons = Vec::new();
            if let Some(reason) = status.reason() {
                reasons.push(reason.to_owned());
            }
            reasons.extend(drop_reasons);
            status = CapabilityStatus::Partial {
                reason: reasons.join("; "),
            };
        }

        BackendDocumentSymbolsResult {
            symbols,
            status,
            truncated,
            skipped,
            gaps,
        }
    }
}

/// Walks query symbols under the C0 output caps.
struct MappingState<'a> {
    source: &'a str,
    emitted: usize,
    truncated: bool,
    drops: MappingDrops,
}

impl<'a> MappingState<'a> {
    const fn new(source: &'a str) -> Self {
        Self {
            source,
            emitted: 0,
            truncated: false,
            drops: MappingDrops {
                mismatched_ranges: 0,
                selections_outside: 0,
                children_outside: 0,
                over_depth: 0,
                over_count: 0,
                dropped_nodes: 0,
            },
        }
    }

    fn map_node(
        &mut self,
        symbol: &DocumentSymbol,
        depth: usize,
        parent: Option<BackendSourceRange>,
    ) -> Option<BackendDocumentSymbol> {
        if depth >= wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH {
            self.drops.drop_subtree(symbol, DropKind::OverDepth);
            return None;
        }
        if self.emitted >= wxml_types::MAX_DOCUMENT_SYMBOLS {
            self.truncated = true;
            self.drops.drop_subtree(symbol, DropKind::OverCount);
            return None;
        }
        let Some(range) = verified_range(self.source, symbol.range, symbol.utf16) else {
            self.drops.drop_subtree(symbol, DropKind::MismatchedRange);
            return None;
        };
        let Some(selection_range) =
            verified_range(self.source, symbol.selection_range, symbol.selection_utf16)
        else {
            self.drops.drop_subtree(symbol, DropKind::MismatchedRange);
            return None;
        };
        if selection_range.start() < range.start() || selection_range.end() > range.end() {
            self.drops.drop_subtree(symbol, DropKind::SelectionOutside);
            return None;
        }
        if let Some(parent) = parent {
            if range.start() < parent.start() || range.end() > parent.end() {
                self.drops.drop_subtree(symbol, DropKind::ChildOutside);
                return None;
            }
        }
        self.emitted = self.emitted.saturating_add(1);
        let children = symbol
            .children
            .iter()
            .filter_map(|child| self.map_node(child, depth.saturating_add(1), Some(range)))
            .collect();
        Some(BackendDocumentSymbol {
            name: symbol.name.clone(),
            kind: backend_kind(symbol),
            range,
            selection_range,
            children,
        })
    }
}

/// Why one input subtree was not represented in the C0 result.
#[derive(Clone, Copy, Debug)]
enum DropKind {
    MismatchedRange,
    SelectionOutside,
    ChildOutside,
    OverDepth,
    OverCount,
}

/// Counts input nodes that could not be represented in C0 coordinates.
///
/// `dropped_nodes` includes every node in a dropped subtree; the per-reason
/// counters sum to the same total.
#[derive(Debug, Default)]
struct MappingDrops {
    mismatched_ranges: usize,
    selections_outside: usize,
    children_outside: usize,
    over_depth: usize,
    over_count: usize,
    dropped_nodes: usize,
}

impl MappingDrops {
    fn drop_subtree(&mut self, symbol: &DocumentSymbol, kind: DropKind) {
        let nodes = subtree_len(symbol);
        self.dropped_nodes = self.dropped_nodes.saturating_add(nodes);
        let counter = match kind {
            DropKind::MismatchedRange => &mut self.mismatched_ranges,
            DropKind::SelectionOutside => &mut self.selections_outside,
            DropKind::ChildOutside => &mut self.children_outside,
            DropKind::OverDepth => &mut self.over_depth,
            DropKind::OverCount => &mut self.over_count,
        };
        *counter = counter.saturating_add(nodes);
    }

    fn reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.mismatched_ranges > 0 {
            reasons.push(format!(
                "{} document symbol node(s) did not match the script text",
                self.mismatched_ranges
            ));
        }
        if self.selections_outside > 0 {
            reasons.push(format!(
                "{} document symbol node(s) had a selection range outside the declaration range",
                self.selections_outside
            ));
        }
        if self.children_outside > 0 {
            reasons.push(format!(
                "{} document symbol node(s) were outside the parent range",
                self.children_outside
            ));
        }
        if self.over_depth > 0 {
            reasons.push(format!(
                "{} document symbol node(s) exceeded the {}-level conversion depth cap",
                self.over_depth,
                wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH
            ));
        }
        if self.over_count > 0 {
            reasons.push(format!(
                "{} document symbol node(s) were dropped after the {}-symbol conversion cap",
                self.over_count,
                wxml_types::MAX_DOCUMENT_SYMBOLS
            ));
        }
        reasons
    }
}

/// Counts one symbol and all of its descendants.
fn subtree_len(symbol: &DocumentSymbol) -> usize {
    symbol.children.iter().fold(1usize, |total, child| {
        total.saturating_add(subtree_len(child))
    })
}

/// Verifies that the native UTF-8 and UTF-16 ranges describe the same
/// non-empty token in `source`, then returns the UTF-8 range.
///
/// This is the fail-closed boundary for presentation-layer `SourceRange::new`
/// values, which do not validate bounds: empty or reversed ranges and ranges
/// whose two coordinate systems disagree are rejected rather than clamped.
fn verified_range(
    source: &str,
    bytes: SourceRange,
    units: Utf16Range,
) -> Option<BackendSourceRange> {
    let start = utf16_to_utf8(source, units.start).ok()?.get();
    let end = utf16_to_utf8(source, units.end).ok()?.get();
    if start >= end || bytes.start.get() != start || bytes.end.get() != end {
        return None;
    }
    BackendSourceRange::from_bounds(start, end).ok()
}

/// Maps the local LSP kind, keeping type aliases distinct from interfaces.
const fn backend_kind(symbol: &DocumentSymbol) -> BackendDocumentSymbolKind {
    if symbol.node_kind == TYPE_ALIAS_DECLARATION_KIND {
        return BackendDocumentSymbolKind::TypeAlias;
    }
    match symbol.kind {
        DocumentSymbolKind::Module => BackendDocumentSymbolKind::Module,
        DocumentSymbolKind::Class => BackendDocumentSymbolKind::Class,
        DocumentSymbolKind::Method => BackendDocumentSymbolKind::Method,
        DocumentSymbolKind::Property => BackendDocumentSymbolKind::Property,
        DocumentSymbolKind::Enum => BackendDocumentSymbolKind::Enum,
        DocumentSymbolKind::Interface => BackendDocumentSymbolKind::Interface,
        DocumentSymbolKind::Function => BackendDocumentSymbolKind::Function,
        DocumentSymbolKind::Variable => BackendDocumentSymbolKind::Variable,
        DocumentSymbolKind::EnumMember => BackendDocumentSymbolKind::EnumMember,
    }
}

fn append_reason(existing: &mut String, addition: &str) {
    if existing.split("; ").any(|part| part == addition) {
        return;
    }
    if !existing.is_empty() {
        existing.push_str("; ");
    }
    existing.push_str(addition);
}

/// Formats the native truncation and skipped-candidate reasons without
/// dropping their exact text.
fn native_incompleteness_reasons(
    truncated: bool,
    skipped: usize,
    gaps: &[DocumentSymbolGap],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if truncated {
        reasons.push(format!(
            "document symbol list was truncated at {MAX_DOCUMENT_SYMBOLS} entries"
        ));
    }
    if skipped > 0 {
        let mut reason = format!("{skipped} declaration candidate(s) were skipped");
        if let Some(gap) = gaps.first() {
            reason.push_str("; first gap: ");
            reason.push_str(gap.reason);
            if let Some(name_kind) = gap.name_kind {
                let _ = write!(reason, " (name kind {name_kind})");
            }
        }
        reasons.push(reason);
    }
    reasons
}

/// Maps native document-symbol evidence.
///
/// `Supported` requires no gap and no truncation. Every decoded symbol is kept
/// even when another candidate was skipped, so the result stays useful without
/// claiming completeness. This layer preserves native evidence verbatim; C0
/// caps, coordinate checks and conversion accounting are applied by
/// [`DocumentSymbolResult::to_backend_result`].
#[must_use]
pub fn map_document_symbols(evidence: &DocumentSymbolsEvidence) -> DocumentSymbolResult {
    let mut mapping_failures = 0usize;
    let symbols = evidence
        .symbols
        .iter()
        .filter_map(|symbol| map_symbol(symbol, &mut mapping_failures))
        .collect::<Vec<_>>();

    let mut reasons =
        native_incompleteness_reasons(evidence.truncated, evidence.skipped, &evidence.gaps);
    if mapping_failures > 0 {
        reasons.push(format!(
            "{mapping_failures} decoded symbol kind(s) had no LSP mapping"
        ));
    }

    let status = if reasons.is_empty() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: reasons.join("; "),
        }
    };

    DocumentSymbolResult {
        symbols,
        status,
        truncated: evidence.truncated,
        gaps: evidence.gaps.clone(),
        skipped: evidence.skipped,
    }
}

fn map_symbol(
    symbol: &DocumentSymbolEvidence,
    mapping_failures: &mut usize,
) -> Option<DocumentSymbol> {
    let Some(kind) = DocumentSymbolKind::from_node_kind(symbol.node_kind) else {
        *mapping_failures = mapping_failures.saturating_add(1);
        return None;
    };
    let children = symbol
        .children
        .iter()
        .filter_map(|child| map_symbol(child, mapping_failures))
        .collect();
    Some(DocumentSymbol {
        name: symbol.name.clone(),
        kind,
        node_kind: symbol.node_kind,
        range: SourceRange::new(symbol.start_utf8.get(), symbol.end_utf8.get()),
        utf16: Utf16Range::new(symbol.start_utf16.get(), symbol.end_utf16.get()),
        selection_range: SourceRange::new(
            symbol.selection_start_utf8.get(),
            symbol.selection_end_utf8.get(),
        ),
        selection_utf16: Utf16Range::new(
            symbol.selection_start_utf16.get(),
            symbol.selection_end_utf16.get(),
        ),
        children,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::assert_is_empty)]

    use super::*;
    use crate::native::ast::{
        CONSTRUCTOR_KIND, FUNCTION_DECLARATION_KIND, OBJECT_BINDING_PATTERN_KIND,
        STRING_LITERAL_KIND, TYPE_ALIAS_DECLARATION_KIND, VARIABLE_DECLARATION_KIND,
    };
    use crate::native::document_symbol::{
        DocumentSymbolGap, DocumentSymbolsEvidence, MAX_DOCUMENT_SYMBOL_GAPS,
    };
    use crate::native::strings::{Utf8Offset, Utf16Offset};
    use wxml_types::DocumentSymbolKind as C0SymbolKind;

    fn evidence(node_kind: u32, name: &str, start: u32, end: u32) -> DocumentSymbolEvidence {
        DocumentSymbolEvidence {
            name: name.to_owned(),
            node_kind,
            start_utf16: Utf16Offset::new(start),
            end_utf16: Utf16Offset::new(end),
            start_utf8: Utf8Offset::new(start),
            end_utf8: Utf8Offset::new(end),
            selection_start_utf16: Utf16Offset::new(start),
            selection_end_utf16: Utf16Offset::new(end),
            selection_start_utf8: Utf8Offset::new(start),
            selection_end_utf8: Utf8Offset::new(end),
            children: Vec::new(),
        }
    }

    #[test]
    fn maps_frozen_node_kinds_to_lsp_kinds() {
        let cases = [
            (FUNCTION_DECLARATION_KIND, DocumentSymbolKind::Function, 12),
            (CLASS_DECLARATION_KIND, DocumentSymbolKind::Class, 5),
            (
                INTERFACE_DECLARATION_KIND,
                DocumentSymbolKind::Interface,
                11,
            ),
            (
                TYPE_ALIAS_DECLARATION_KIND,
                DocumentSymbolKind::Interface,
                11,
            ),
            (ENUM_DECLARATION_KIND, DocumentSymbolKind::Enum, 10),
            (MODULE_DECLARATION_KIND, DocumentSymbolKind::Module, 2),
            (VARIABLE_DECLARATION_KIND, DocumentSymbolKind::Variable, 13),
            (PROPERTY_DECLARATION_KIND, DocumentSymbolKind::Property, 7),
            (PROPERTY_SIGNATURE_KIND, DocumentSymbolKind::Property, 7),
            (METHOD_DECLARATION_KIND, DocumentSymbolKind::Method, 6),
            (METHOD_SIGNATURE_KIND, DocumentSymbolKind::Method, 6),
            (GET_ACCESSOR_KIND, DocumentSymbolKind::Property, 7),
            (SET_ACCESSOR_KIND, DocumentSymbolKind::Property, 7),
            (ENUM_MEMBER_KIND, DocumentSymbolKind::EnumMember, 22),
        ];
        for (node_kind, expected, number) in cases {
            let mapped = DocumentSymbolKind::from_node_kind(node_kind).expect("mapped kind");
            assert_eq!(mapped, expected);
            assert_eq!(mapped.as_u32(), number);
        }
        assert!(DocumentSymbolKind::from_node_kind(CONSTRUCTOR_KIND).is_none());
    }

    #[test]
    fn empty_evidence_is_a_supported_empty_answer() {
        let result = map_document_symbols(&DocumentSymbolsEvidence {
            symbols: Vec::new(),
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        assert!(result.symbols.is_empty());
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(!result.truncated);
    }

    #[test]
    fn gaps_and_truncation_are_partial_but_keep_decoded_symbols() {
        let result = map_document_symbols(&DocumentSymbolsEvidence {
            symbols: vec![evidence(FUNCTION_DECLARATION_KIND, "alpha", 0, 10)],
            gaps: vec![DocumentSymbolGap {
                node_kind: PROPERTY_DECLARATION_KIND,
                name_kind: Some(STRING_LITERAL_KIND),
                start_utf16: 11,
                reason: "name slot is not an Identifier",
            }],
            skipped: 1,
            truncated: true,
        });
        assert_eq!(result.symbols.len(), 1);
        assert!(result.truncated);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.gaps.len(), 1);
        match result.status {
            CapabilityStatus::Partial { reason } => {
                assert!(reason.contains("truncated at 256"));
                assert!(reason.contains("skipped"));
                assert!(reason.contains("name slot is not an Identifier"));
                assert!(reason.contains("name kind 10"));
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    #[test]
    fn preserves_utf8_and_utf16_ranges_separately() {
        let mut symbol = evidence(CLASS_DECLARATION_KIND, "名字", 2, 6);
        symbol.selection_start_utf16 = Utf16Offset::new(2);
        symbol.selection_end_utf16 = Utf16Offset::new(6);
        symbol.selection_start_utf8 = Utf8Offset::new(4);
        symbol.selection_end_utf8 = Utf8Offset::new(10);
        symbol.start_utf8 = Utf8Offset::new(1);
        symbol.end_utf8 = Utf8Offset::new(12);
        let result = map_document_symbols(&DocumentSymbolsEvidence {
            symbols: vec![symbol],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        let symbol = result.symbols.first().expect("symbol");
        assert_eq!(symbol.utf16.start.get(), 2);
        assert_eq!(symbol.utf16.end.get(), 6);
        assert_eq!(symbol.range.start.get(), 1);
        assert_eq!(symbol.range.end.get(), 12);
        assert_eq!(symbol.selection_utf16.start.get(), 2);
        assert_eq!(symbol.selection_range.start.get(), 4);
        assert_eq!(symbol.selection_range.end.get(), 10);
    }

    fn evidence_at(
        source: &str,
        node_kind: u32,
        name: &str,
        declaration: (usize, usize),
        selection: (usize, usize),
    ) -> DocumentSymbolEvidence {
        let offset = |byte: usize| u32::try_from(byte).expect("byte offset");
        let units = |byte: usize| {
            u32::try_from(source[..byte].encode_utf16().count()).expect("UTF-16 offset")
        };
        DocumentSymbolEvidence {
            name: name.to_owned(),
            node_kind,
            start_utf16: Utf16Offset::new(units(declaration.0)),
            end_utf16: Utf16Offset::new(units(declaration.1)),
            start_utf8: Utf8Offset::new(offset(declaration.0)),
            end_utf8: Utf8Offset::new(offset(declaration.1)),
            selection_start_utf16: Utf16Offset::new(units(selection.0)),
            selection_end_utf16: Utf16Offset::new(units(selection.1)),
            selection_start_utf8: Utf8Offset::new(offset(selection.0)),
            selection_end_utf8: Utf8Offset::new(offset(selection.1)),
            children: Vec::new(),
        }
    }

    fn query_result(evidence: &DocumentSymbolsEvidence) -> DocumentSymbolResult {
        map_document_symbols(evidence)
    }

    fn c0_total(symbols: &[wxml_types::DocumentSymbol]) -> usize {
        symbols
            .iter()
            .map(|symbol| 1usize.saturating_add(c0_total(&symbol.children)))
            .fold(0usize, usize::saturating_add)
    }

    fn c0_names(symbols: &[wxml_types::DocumentSymbol]) -> Vec<String> {
        let mut names = Vec::new();
        for symbol in symbols {
            names.push(symbol.name.clone());
            names.extend(c0_names(&symbol.children));
        }
        names
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn backend_mapping_keeps_cjk_emoji_children_and_kinds() {
        let source = "// 😀 文档\nclass 用户 {\n  name: string;\n  greet😀(): void {}\n}\n\
                      type Shape = { area(): number };\nnamespace A.B { const inner = 1; }\n";
        let class_start = source.find("class ").expect("class");
        let class_end = source.find("\ntype Shape").expect("class end");
        let class_name = source.find("用户").expect("class name");
        let name_start = source.find("name:").expect("property");
        let name_end = name_start + "name: string;".len();
        let method_start = source.find("greet😀").expect("method");
        let method_end = method_start + "greet😀(): void {}".len();
        let mut class = evidence_at(
            source,
            CLASS_DECLARATION_KIND,
            "用户",
            (class_start, class_end),
            (class_name, class_name + "用户".len()),
        );
        class.children = vec![
            evidence_at(
                source,
                PROPERTY_DECLARATION_KIND,
                "name",
                (name_start, name_end),
                (name_start, name_start + 4),
            ),
            evidence_at(
                source,
                METHOD_DECLARATION_KIND,
                "greet😀",
                (method_start, method_end),
                (method_start, method_start + "greet😀".len()),
            ),
        ];

        let alias_start = source.find("type Shape").expect("alias");
        let alias_end = source.find("\nnamespace").expect("alias end");
        let alias_name = source.find("Shape").expect("alias name");
        let alias = evidence_at(
            source,
            TYPE_ALIAS_DECLARATION_KIND,
            "Shape",
            (alias_start, alias_end),
            (alias_name, alias_name + "Shape".len()),
        );

        let namespace_start = source.find("namespace A.B").expect("namespace");
        let namespace_end = source.len();
        let module_a_name = namespace_start + "namespace ".len();
        let module_b_start = source.find("B {").expect("module B");
        let inner_start = source.find("inner").expect("inner name");
        let inner_start_decl = source.find("const inner").expect("inner declaration");
        let inner_end = source.find("; }").expect("inner end") + 1;
        let mut module_b = evidence_at(
            source,
            MODULE_DECLARATION_KIND,
            "B",
            (module_b_start, namespace_end),
            (module_b_start, module_b_start + 1),
        );
        module_b.children = vec![evidence_at(
            source,
            VARIABLE_DECLARATION_KIND,
            "inner",
            (inner_start_decl, inner_end),
            (inner_start, inner_start + "inner".len()),
        )];
        let mut module_a = evidence_at(
            source,
            MODULE_DECLARATION_KIND,
            "A",
            (namespace_start, namespace_end),
            (module_a_name, module_a_name + 1),
        );
        module_a.children = vec![module_b];

        let native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![class, alias, module_a],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        assert_eq!(native.status, CapabilityStatus::Supported);

        let result = native.to_backend_result(source);
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(!result.truncated);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.symbols.len(), 3);
        assert_eq!(
            result
                .symbols
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.kind))
                .collect::<Vec<_>>(),
            vec![
                ("用户", C0SymbolKind::Class),
                ("Shape", C0SymbolKind::TypeAlias),
                ("A", C0SymbolKind::Module),
            ]
        );

        let class = &result.symbols[0];
        assert_eq!(
            class.range.start(),
            u32::try_from(class_start).expect("class start")
        );
        assert_eq!(
            class.range.end(),
            u32::try_from(class_end).expect("class end")
        );
        assert_eq!(
            class.selection_range.start(),
            u32::try_from(class_name).expect("name start")
        );
        assert_eq!(
            class.selection_range.end(),
            u32::try_from(class_name + "用户".len()).expect("name end")
        );
        assert_eq!(class.children.len(), 2);
        assert_eq!(class.children[0].kind, C0SymbolKind::Property);
        assert_eq!(class.children[1].kind, C0SymbolKind::Method);
        assert_eq!(class.children[1].name, "greet😀");
        assert_eq!(class.children[1].selection_range.len(), 9);
        assert!(class.children[1].selection_range.start() >= class.range.start());
        assert!(class.children[1].selection_range.end() <= class.range.end());

        let module_a = &result.symbols[2];
        assert_eq!(module_a.children.len(), 1);
        assert_eq!(module_a.children[0].name, "B");
        assert_eq!(module_a.children[0].kind, C0SymbolKind::Module);
        assert_eq!(module_a.children[0].children.len(), 1);
        assert_eq!(
            module_a.children[0].children[0].kind,
            C0SymbolKind::Variable
        );
        assert_eq!(module_a.children[0].children[0].name, "inner");
        assert!(module_a.range.start() <= module_a.children[0].range.start());
        assert!(module_a.children[0].range.end() <= module_a.range.end());
    }

    #[test]
    fn backend_mapping_keeps_caps_gaps_truncation_and_partial_status() {
        let source = "// 😀 note\nclass C {}\nconst { a } = config;\n";
        let class_start = source.find("class C").expect("class");
        let class_end = source.find("\nconst").expect("class end");
        let class_name = source.find("C {").expect("class name");
        let class = evidence_at(
            source,
            CLASS_DECLARATION_KIND,
            "C",
            (class_start, class_end),
            (class_name, class_name + 1),
        );
        let gap_start = source.find("const").expect("gap");
        let gap_units =
            u32::try_from(source[..gap_start].encode_utf16().count()).expect("UTF-16 offset");
        let gaps = (0..=MAX_DOCUMENT_SYMBOL_GAPS)
            .map(|index| DocumentSymbolGap {
                node_kind: VARIABLE_DECLARATION_KIND,
                name_kind: Some(OBJECT_BINDING_PATTERN_KIND),
                start_utf16: gap_units + u32::try_from(index).expect("gap index"),
                reason: "name slot is not an Identifier",
            })
            .collect::<Vec<_>>();
        let skipped = gaps.len();
        let native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![class],
            gaps,
            skipped,
            truncated: true,
        });
        assert!(matches!(native.status, CapabilityStatus::Partial { .. }));

        let result = native.to_backend_result(source);
        assert!(result.truncated);
        assert_eq!(result.skipped, skipped);
        assert_eq!(c0_total(&result.symbols), 1);
        assert_eq!(result.gaps.len(), wxml_types::MAX_DOCUMENT_SYMBOL_GAPS);
        assert_eq!(
            result.gaps[0].start,
            Some(u32::try_from(gap_start).expect("gap start"))
        );
        assert_eq!(result.gaps[0].reason, "name slot is not an Identifier");
        assert_eq!(
            result.status.reason(),
            Some(
                format!(
                    "document symbol list was truncated at {MAX_DOCUMENT_SYMBOLS} entries; \
                     {skipped} declaration candidate(s) were skipped; first gap: \
                     name slot is not an Identifier (name kind {OBJECT_BINDING_PATTERN_KIND})"
                )
                .as_str()
            )
        );
        assert_eq!(wxml_types::MAX_DOCUMENT_SYMBOLS, MAX_DOCUMENT_SYMBOLS);
        assert_eq!(
            wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH,
            crate::native::document_symbol::MAX_DOCUMENT_SYMBOL_DEPTH
        );
        assert_eq!(
            wxml_types::MAX_DOCUMENT_SYMBOL_GAPS,
            MAX_DOCUMENT_SYMBOL_GAPS
        );
    }

    #[test]
    fn backend_mapping_drops_a_parent_subtree_and_counts_every_node() {
        let source = "class C { prop: number; }\n";
        let mut mismatched = evidence_at(source, FUNCTION_DECLARATION_KIND, "bad", (0, 5), (0, 3));
        // Keep the UTF-16 range but make the UTF-8 range disagree with it.
        mismatched.start_utf8 = Utf8Offset::new(1);
        mismatched.children = vec![evidence_at(
            source,
            VARIABLE_DECLARATION_KIND,
            "bad-child",
            (0, 4),
            (0, 1),
        )];

        let class_name = source.find("C {").expect("class name");
        let prop_start = source.find("prop").expect("property");
        let prop_end = prop_start + "prop: number;".len();
        let mut outside = evidence_at(
            source,
            PROPERTY_DECLARATION_KIND,
            "outside",
            (24, 25),
            (24, 25),
        );
        outside.children = vec![evidence_at(
            source,
            VARIABLE_DECLARATION_KIND,
            "outside-child",
            (24, 25),
            (24, 25),
        )];
        let mut parent = evidence_at(
            source,
            CLASS_DECLARATION_KIND,
            "C",
            (0, 24),
            (class_name, class_name + 1),
        );
        parent.children = vec![
            evidence_at(
                source,
                PROPERTY_DECLARATION_KIND,
                "prop",
                (prop_start, prop_end),
                (prop_start, prop_start + 4),
            ),
            outside,
        ];

        let native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![mismatched, parent],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        let result = native.to_backend_result(source);

        assert_eq!(
            result.status.reason(),
            Some(
                "2 document symbol node(s) did not match the script text; \
                 2 document symbol node(s) were outside the parent range"
            )
        );
        assert_eq!(c0_names(&result.symbols), vec!["C", "prop"]);
        assert_eq!(c0_total(&result.symbols), 2);
        assert_eq!(result.skipped, 4);
        assert_eq!(c0_total(&result.symbols) + result.skipped, 6);
        assert!(!result.truncated);
    }

    #[test]
    fn backend_mapping_keeps_a_checked_empty_answer() {
        let native = query_result(&DocumentSymbolsEvidence {
            symbols: Vec::new(),
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        let result = native.to_backend_result("const x = 1;\n");
        assert!(result.symbols.is_empty());
        assert!(result.gaps.is_empty());
        assert!(!result.truncated);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.status, CapabilityStatus::Supported);
    }

    #[test]
    fn backend_mapping_does_not_upgrade_unsupported() {
        let native = DocumentSymbolResult {
            symbols: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: "native document symbols are unavailable".to_owned(),
            },
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        };
        let result = native.to_backend_result("");
        assert!(!result.status.is_supported());
        assert_eq!(
            result.status.reason(),
            Some("native document symbols are unavailable")
        );
        assert!(result.symbols.is_empty());
        assert!(result.gaps.is_empty());
    }

    #[test]
    fn backend_mapping_forces_partial_when_a_degraded_status_loses_a_node() {
        let source = "class C {}\n";
        let mut native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![evidence_at(
                source,
                FUNCTION_DECLARATION_KIND,
                "empty",
                (5, 5),
                (5, 5),
            )],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        native.status = CapabilityStatus::Unknown { reason: None };

        let result = native.to_backend_result(source);
        assert!(result.symbols.is_empty());
        assert_eq!(result.skipped, 1);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
        assert_eq!(
            result.status.reason(),
            Some("1 document symbol node(s) did not match the script text")
        );
    }

    #[test]
    fn backend_mapping_merges_native_and_conversion_partial_reasons_exactly() {
        let source = "class C {}\nconst x = 1;\n";
        let class_name = source.find("C {").expect("class name");
        let valid = evidence_at(
            source,
            CLASS_DECLARATION_KIND,
            "C",
            (0, 10),
            (class_name, class_name + 1),
        );
        let mut invalid = evidence_at(source, FUNCTION_DECLARATION_KIND, "bad", (0, 5), (0, 3));
        invalid.start_utf8 = Utf8Offset::new(1);
        let gap_start = source.find("const").expect("gap");
        let native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![valid, invalid],
            gaps: vec![DocumentSymbolGap {
                node_kind: VARIABLE_DECLARATION_KIND,
                name_kind: Some(STRING_LITERAL_KIND),
                start_utf16: u32::try_from(source[..gap_start].encode_utf16().count())
                    .expect("gap offset"),
                reason: "name slot is not an Identifier",
            }],
            skipped: 1,
            truncated: true,
        });

        let result = native.to_backend_result(source);
        assert_eq!(
            result.status.reason(),
            Some(
                format!(
                    "document symbol list was truncated at {MAX_DOCUMENT_SYMBOLS} entries; \
                     1 declaration candidate(s) were skipped; first gap: \
                     name slot is not an Identifier (name kind {STRING_LITERAL_KIND}); \
                     1 document symbol node(s) did not match the script text"
                )
                .as_str()
            )
        );
        assert_eq!(c0_names(&result.symbols), vec!["C"]);
        assert_eq!(result.skipped, 2);
        assert_eq!(c0_total(&result.symbols) + result.skipped, 3);
        assert!(result.truncated);
    }

    #[test]
    fn backend_mapping_gap_start_is_none_when_it_is_outside_the_text() {
        let source = "class C {}\n";
        let beyond = u32::try_from(source.encode_utf16().count()).expect("length") + 1;
        let native = query_result(&DocumentSymbolsEvidence {
            symbols: Vec::new(),
            gaps: vec![DocumentSymbolGap {
                node_kind: VARIABLE_DECLARATION_KIND,
                name_kind: None,
                start_utf16: beyond,
                reason: "candidate is outside the text",
            }],
            skipped: 1,
            truncated: false,
        });

        let result = native.to_backend_result(source);
        assert_eq!(result.gaps.len(), 1);
        assert_eq!(result.gaps[0].start, None);
        assert_eq!(result.gaps[0].reason, "candidate is outside the text");
        assert_eq!(result.skipped, 1);
        assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    }

    #[test]
    fn backend_mapping_rejects_empty_and_reversed_ranges() {
        let source = "class C { prop: number; }\n";
        let empty = evidence_at(source, FUNCTION_DECLARATION_KIND, "empty", (5, 5), (5, 5));
        let reversed = evidence_at(
            source,
            FUNCTION_DECLARATION_KIND,
            "reversed",
            (5, 3),
            (5, 3),
        );
        let native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![empty, reversed],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });

        let result = native.to_backend_result(source);
        assert!(result.symbols.is_empty());
        assert_eq!(result.skipped, 2);
        assert!(!result.truncated);
        assert_eq!(
            result.status.reason(),
            Some("2 document symbol node(s) did not match the script text")
        );
    }

    #[test]
    fn backend_mapping_enforces_the_symbol_cap_at_conversion() {
        let source = "function f() {}\n";
        let symbols = (0..=wxml_types::MAX_DOCUMENT_SYMBOLS)
            .map(|index| {
                evidence_at(
                    source,
                    FUNCTION_DECLARATION_KIND,
                    &format!("f{index}"),
                    (0, 10),
                    (9, 10),
                )
            })
            .collect();
        let native = query_result(&DocumentSymbolsEvidence {
            symbols,
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });
        assert_eq!(native.status, CapabilityStatus::Supported);

        let result = native.to_backend_result(source);
        assert!(result.truncated);
        assert_eq!(result.symbols.len(), wxml_types::MAX_DOCUMENT_SYMBOLS);
        assert_eq!(result.skipped, 1);
        assert_eq!(
            result.status.reason(),
            Some(
                format!(
                    "1 document symbol node(s) were dropped after the {}-symbol conversion cap",
                    wxml_types::MAX_DOCUMENT_SYMBOLS
                )
                .as_str()
            )
        );
        assert_eq!(
            c0_total(&result.symbols) + result.skipped,
            wxml_types::MAX_DOCUMENT_SYMBOLS + 1
        );
        assert_eq!(
            result.symbols.last().expect("last symbol").name,
            format!("f{}", wxml_types::MAX_DOCUMENT_SYMBOLS - 1)
        );
    }

    #[test]
    fn backend_mapping_enforces_the_depth_cap_at_conversion() {
        let source = "class C {}\n";
        let mut node = evidence_at(source, CLASS_DECLARATION_KIND, "leaf", (0, 10), (6, 7));
        for _ in 0..wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH {
            let mut parent = evidence_at(source, CLASS_DECLARATION_KIND, "parent", (0, 10), (6, 7));
            parent.children = vec![node];
            node = parent;
        }
        let native = query_result(&DocumentSymbolsEvidence {
            symbols: vec![node],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        });

        let result = native.to_backend_result(source);
        assert!(!result.truncated);
        assert_eq!(
            c0_total(&result.symbols) + result.skipped,
            wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH + 1
        );
        assert_eq!(result.skipped, 1);
        assert_eq!(
            result.status.reason(),
            Some(
                format!(
                    "1 document symbol node(s) exceeded the {}-level conversion depth cap",
                    wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH
                )
                .as_str()
            )
        );
        let mut deepest = result.symbols.first().expect("root");
        for _ in 1..wxml_types::MAX_DOCUMENT_SYMBOL_DEPTH {
            deepest = deepest.children.first().expect("nested symbol");
        }
        assert!(deepest.children.is_empty());
    }
}
