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

use std::fmt::Write as _;

use wxml_types::CapabilityStatus;

use crate::native::ast::{
    CLASS_DECLARATION_KIND, ENUM_DECLARATION_KIND, ENUM_MEMBER_KIND, FUNCTION_DECLARATION_KIND,
    GET_ACCESSOR_KIND, INTERFACE_DECLARATION_KIND, METHOD_DECLARATION_KIND, METHOD_SIGNATURE_KIND,
    MODULE_DECLARATION_KIND, PROPERTY_DECLARATION_KIND, PROPERTY_SIGNATURE_KIND, SET_ACCESSOR_KIND,
    TYPE_ALIAS_DECLARATION_KIND, VARIABLE_DECLARATION_KIND,
};
use crate::native::document_symbol::{
    DocumentSymbolEvidence, DocumentSymbolGap, DocumentSymbolsEvidence, MAX_DOCUMENT_SYMBOLS,
};
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

/// Maps native document-symbol evidence.
///
/// `Supported` requires no gap and no truncation. Every decoded symbol is kept
/// even when another candidate was skipped, so the result stays useful without
/// claiming completeness.
#[must_use]
pub fn map_document_symbols(evidence: &DocumentSymbolsEvidence) -> DocumentSymbolResult {
    let mut mapping_failures = 0usize;
    let symbols = evidence
        .symbols
        .iter()
        .filter_map(|symbol| map_symbol(symbol, &mut mapping_failures))
        .collect::<Vec<_>>();

    let mut reasons = Vec::new();
    if evidence.truncated {
        reasons.push(format!(
            "document symbol list was truncated at {MAX_DOCUMENT_SYMBOLS} entries"
        ));
    }
    if evidence.skipped > 0 {
        let mut reason = format!("{} declaration candidate(s) were skipped", evidence.skipped);
        if let Some(gap) = evidence.gaps.first() {
            reason.push_str("; first gap: ");
            reason.push_str(gap.reason);
            if let Some(name_kind) = gap.name_kind {
                let _ = write!(reason, " (name kind {name_kind})");
            }
        }
        reasons.push(reason);
    }
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
    use crate::native::ast::{CONSTRUCTOR_KIND, STRING_LITERAL_KIND};
    use crate::native::document_symbol::{DocumentSymbolGap, DocumentSymbolsEvidence};
    use crate::native::strings::{Utf8Offset, Utf16Offset};

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
}
