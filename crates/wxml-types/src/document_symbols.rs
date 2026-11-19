//! Document-symbol (outline) DTOs shared by type backends and the LSP layer.
//!
//! Document symbols are a per-document outline, not script symbol facts. They
//! are separate from [`crate::SymbolResult`] on purpose: an outline entry has
//! a kind, a declaration range, a name-token selection range and hierarchical
//! children, while a [`crate::SymbolResult`] answers data-path, reference and
//! handler questions. Mixing the two would give rename and code actions a
//! completeness contract they cannot use.
//!
//! Ranges are UTF-8 bytes in the original subject file. The LSP layer converts
//! them to UTF-16 positions at the protocol edge. Children are nested
//! declarations and must be contained by their parent's [`DocumentSymbol::range`];
//! ordering is the backend's deterministic source order.
//!
//! Empty [`DocumentSymbolsResult::symbols`] means "checked, this document
//! declares nothing" only when [`DocumentSymbolsResult::status`] is
//! [`CapabilityStatus::Supported`]. An `Unsupported`, `Unavailable`, `Partial`
//! or `Disabled` status means the backend could not prove that, so callers must
//! not present the empty list as an exhaustive outline.
//!
//! The caps mirror the native walker that produced the frozen evidence:
//! [`MAX_DOCUMENT_SYMBOLS`] entries per document, [`MAX_DOCUMENT_SYMBOL_DEPTH`]
//! nesting levels and [`MAX_DOCUMENT_SYMBOL_GAPS`] stored gaps. Truncation and
//! skipped candidates stay visible through [`DocumentSymbolsResult::truncated`],
//! [`DocumentSymbolsResult::skipped`] and [`DocumentSymbolsResult::gaps`]; a
//! capped result is `Partial`, never a fabricated or padded symbol.

use wxml_text::SourceRange;

use crate::context::RequestContext;
use crate::contract::FileId;
use crate::status::CapabilityStatus;

/// Maximum number of document symbols retained for one document.
///
/// The cap is applied across the whole tree in source order. Reaching it sets
/// [`DocumentSymbolsResult::truncated`].
pub const MAX_DOCUMENT_SYMBOLS: usize = 256;

/// Maximum nesting depth for document symbols.
pub const MAX_DOCUMENT_SYMBOL_DEPTH: usize = 32;

/// Maximum number of distinct gaps retained in one result.
///
/// [`DocumentSymbolsResult::skipped`] still counts every gap beyond this bound.
pub const MAX_DOCUMENT_SYMBOL_GAPS: usize = 16;

/// The kind of one document symbol.
///
/// The values follow the LSP `SymbolKind` vocabulary, except that
/// [`Self::TypeAlias`] is kept distinct. LSP has no dedicated type-alias kind
/// and maps it to `Interface`; keeping the distinction here lets the LSP layer
/// prove the mapping instead of losing it early.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DocumentSymbolKind {
    /// A TypeScript `namespace` or `module` declaration.
    Module,
    /// A class declaration.
    Class,
    /// A class/interface method or method signature.
    Method,
    /// A class/interface property, property signature or accessor.
    Property,
    /// An enum declaration.
    Enum,
    /// An interface declaration.
    Interface,
    /// A TypeScript type alias.
    TypeAlias,
    /// A function declaration.
    Function,
    /// A variable declaration.
    Variable,
    /// One enum member.
    EnumMember,
}

/// A document-symbol query for one document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbolsRequest {
    /// Versions and source snapshot the query depends on.
    pub context: RequestContext,
    /// Document whose outline is requested.
    ///
    /// It must be one of the files in the prepared view; the backend rejects
    /// an unknown subject instead of resolving a path on its own.
    pub subject: FileId,
}

/// One document symbol with its exact declaration and name-token ranges.
///
/// Both ranges are UTF-8 bytes in the original subject file. The selection
/// range is the verified name token and must be contained by
/// [`Self::range`]; children are nested declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbol {
    /// Verified symbol name.
    pub name: String,
    /// Symbol kind.
    pub kind: DocumentSymbolKind,
    /// Declaration range in UTF-8 bytes.
    pub range: SourceRange,
    /// Name-token range in UTF-8 bytes, contained by [`Self::range`].
    pub selection_range: SourceRange,
    /// Nested declarations in deterministic source order.
    pub children: Vec<Self>,
}

/// One declaration candidate that could not be published.
///
/// A gap is never a placeholder symbol. `reason` is bounded, display-safe
/// text; `start` is a UTF-8 offset when the skipped candidate maps to the
/// subject text, and `None` when it does not map. Callers must not invent an
/// offset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbolGap {
    /// UTF-8 offset of the skipped candidate in the subject, when mappable.
    pub start: Option<u32>,
    /// Bounded, display-safe description of why the candidate was skipped.
    pub reason: String,
}

/// Document-symbol result for one document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSymbolsResult {
    /// Decoded symbols in deterministic source order.
    pub symbols: Vec<DocumentSymbol>,
    /// Completeness and availability of the query.
    pub status: CapabilityStatus,
    /// `true` when [`MAX_DOCUMENT_SYMBOLS`] cut the tree short.
    pub truncated: bool,
    /// Total skipped candidates, including gaps beyond
    /// [`MAX_DOCUMENT_SYMBOL_GAPS`].
    pub skipped: usize,
    /// First [`MAX_DOCUMENT_SYMBOL_GAPS`] skipped candidates.
    pub gaps: Vec<DocumentSymbolGap>,
}

impl DocumentSymbolsResult {
    /// Creates an unsupported answer with no fabricated symbols.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            symbols: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: reason.into(),
            },
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::assert_is_empty)]

    use super::*;

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::from_bounds(start, end).expect("valid range")
    }

    fn symbol(
        name: &str,
        kind: DocumentSymbolKind,
        bounds: (u32, u32),
        selection: (u32, u32),
        children: Vec<DocumentSymbol>,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: name.to_owned(),
            kind,
            range: range(bounds.0, bounds.1),
            selection_range: range(selection.0, selection.1),
            children,
        }
    }

    #[test]
    fn document_symbols_result_keeps_tree_ranges_and_children() {
        // "用户" is six UTF-8 bytes and two UTF-16 units; "😀" is four and two.
        let result = DocumentSymbolsResult {
            symbols: vec![symbol(
                "用户",
                DocumentSymbolKind::Class,
                (4, 90),
                (10, 16),
                vec![
                    symbol(
                        "name",
                        DocumentSymbolKind::Property,
                        (30, 44),
                        (30, 34),
                        Vec::new(),
                    ),
                    symbol(
                        "greet😀",
                        DocumentSymbolKind::Method,
                        (50, 88),
                        (50, 59),
                        Vec::new(),
                    ),
                ],
            )],
            status: CapabilityStatus::Supported,
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        };

        let class = &result.symbols[0];
        assert_eq!(class.name, "用户");
        assert_eq!(class.kind, DocumentSymbolKind::Class);
        assert_eq!(class.range.start(), 4);
        assert_eq!(class.range.end(), 90);
        assert_eq!(class.selection_range.start(), 10);
        assert_eq!(class.selection_range.end(), 16);
        assert!(class.range.contains(class.selection_range.start()));
        assert_eq!(class.children.len(), 2);
        assert_eq!(class.children[0].name, "name");
        assert_eq!(class.children[0].kind, DocumentSymbolKind::Property);
        assert_eq!(class.children[1].name, "greet😀");
        assert_eq!(class.children[1].kind, DocumentSymbolKind::Method);
        assert!(class.range.start() <= class.children[1].range.start());
        assert!(class.children[1].range.end() <= class.range.end());
    }

    #[test]
    fn document_symbols_caps_are_256_32_16() {
        assert_eq!(MAX_DOCUMENT_SYMBOLS, 256);
        assert_eq!(MAX_DOCUMENT_SYMBOL_DEPTH, 32);
        assert_eq!(MAX_DOCUMENT_SYMBOL_GAPS, 16);
    }

    #[test]
    fn type_alias_stays_distinct_from_interface() {
        assert_ne!(DocumentSymbolKind::TypeAlias, DocumentSymbolKind::Interface);
    }

    #[test]
    fn unsupported_answer_has_no_fabricated_symbols() {
        let result = DocumentSymbolsResult::unsupported("not implemented");
        assert!(result.symbols.is_empty());
        assert!(result.gaps.is_empty());
        assert!(!result.truncated);
        assert_eq!(result.skipped, 0);
        assert!(!result.status.is_supported());
        assert_eq!(result.status.reason(), Some("not implemented"));
    }
}
