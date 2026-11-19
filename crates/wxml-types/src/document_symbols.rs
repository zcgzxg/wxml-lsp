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
//! [`CapabilityStatus::Supported`]. `Partial`, `Unsupported`, `Disabled`,
//! `Unavailable` and `Unknown` statuses mean the backend could not prove that,
//! so callers must not present the empty list as an exhaustive outline.
//!
//! The caps are script-outline budgets, deliberately independent of the static
//! WXML outline budgets in the LSP layer's `structure.rs`.
//! [`MAX_DOCUMENT_SYMBOLS`] bounds the output tree, [`MAX_DOCUMENT_SYMBOL_DEPTH`]
//! bounds nesting and [`MAX_DOCUMENT_SYMBOL_GAPS`] bounds stored gaps.
//! Truncation and skipped candidates stay visible through
//! [`DocumentSymbolsResult::truncated`], [`DocumentSymbolsResult::skipped`] and
//! [`DocumentSymbolsResult::gaps`]; a capped result is `Partial`, never a
//! fabricated or padded symbol.

use wxml_text::SourceRange;

use crate::context::RequestContext;
use crate::contract::FileId;
use crate::status::CapabilityStatus;

/// Maximum number of script document symbols retained for one document.
///
/// The cap is applied across the whole tree in source order. Reaching it sets
/// [`DocumentSymbolsResult::truncated`].
///
/// This is a script-outline budget. The static WXML outline in the LSP layer's
/// `structure.rs` has its own, larger budget (2,000); the two outlines are
/// separate features and must not share or raise each other's cap.
pub const MAX_DOCUMENT_SYMBOLS: usize = 256;

/// Maximum nesting depth for script document symbols.
///
/// This is independent of the static WXML outline's own depth budget in the
/// LSP layer's `structure.rs`; neither outline may borrow the other's limit.
pub const MAX_DOCUMENT_SYMBOL_DEPTH: usize = 32;

/// Maximum number of distinct gaps retained in one result.
///
/// The cap mirrors the native walker's stored-gap bound so the converted DTO
/// cannot grow without bound. [`DocumentSymbolsResult::skipped`] still counts
/// every skipped candidate beyond this bound.
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
/// range is the verified name token and must be contained by [`Self::range`];
/// the producing backend/adapter enforces that invariant. This DTO does not
/// re-validate ranges, so a consumer must not treat an externally constructed
/// value as checked text. Children are nested declarations and must be
/// contained by the parent range.
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
/// A gap is never a placeholder symbol. `reason` preserves the native
/// decoder's bounded, display-safe reason text so logs keep exact truncation
/// and gap provenance; it is meant for humans and must not be parsed as a
/// stable machine code. `start` is a UTF-8 offset when the skipped candidate
/// maps to the subject text, and `None` when it does not map. Callers must not
/// invent an offset.
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
    ///
    /// `Unknown` is not a checked answer: callers must not present an empty
    /// symbol list as an exhaustive outline for it.
    pub status: CapabilityStatus,
    /// `true` when [`MAX_DOCUMENT_SYMBOLS`] cut the tree short.
    pub truncated: bool,
    /// Total skipped input nodes plus native skipped candidates.
    ///
    /// Conversion accounting counts every input node that is not present in
    /// [`Self::symbols`], including all descendants of a dropped parent.
    /// Native skipped candidates were never decoded into the input tree, so
    /// `total_symbols(symbols) + skipped == conversion_input_nodes +
    /// native_skipped`. Native truncation means later document nodes were
    /// never visited and are therefore outside both counts. This field also
    /// includes candidates beyond [`MAX_DOCUMENT_SYMBOL_GAPS`]. Conversion
    /// drops (invalid ranges, depth/count caps and dropped parents) are counted
    /// here but do not add entries to [`Self::gaps`]; gaps are native
    /// candidates only.
    pub skipped: usize,
    /// First [`MAX_DOCUMENT_SYMBOL_GAPS`] native skipped candidates.
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

    #[test]
    fn document_symbols_caps_are_256_32_16() {
        assert_eq!(MAX_DOCUMENT_SYMBOLS, 256);
        assert_eq!(MAX_DOCUMENT_SYMBOL_DEPTH, 32);
        assert_eq!(MAX_DOCUMENT_SYMBOL_GAPS, 16);
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

    #[test]
    fn unknown_status_is_not_a_checked_empty_outline() {
        let result = DocumentSymbolsResult {
            symbols: Vec::new(),
            status: CapabilityStatus::Unknown { reason: None },
            truncated: false,
            skipped: 0,
            gaps: Vec::new(),
        };
        assert!(result.symbols.is_empty());
        assert!(!result.status.is_supported());
        assert_eq!(result.status.reason(), None);
    }
}
