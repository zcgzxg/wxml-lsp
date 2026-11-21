//! Script-side symbol facts shared by references, rename, code actions and
//! lens queries.
//!
//! The WXML side of a symbol identity is resolved by the LSP layer from the
//! syntax tree, static template facts and project indexes. This module only
//! describes what a type backend may prove about the owning script: the
//! declaration token, TypeScript reference tokens, statically recognized
//! `setData`/`observer` path segments and a safe insertion point for a missing
//! event handler.
//!
//! Nothing here is a workspace edit. Every token is a byte range in an
//! original project file; the LSP layer re-validates it against the current
//! revision and builds a [`crate::Backend`] result only through the unified
//! edit plan.

use thiserror::Error;
use wxml_text::SourceRange;

use crate::context::RequestContext;
use crate::contract::FileId;
use crate::status::CapabilityStatus;

/// Maximum number of reference, occurrence or field entries accepted from one
/// script symbol answer.
pub const MAX_SYMBOL_ENTRIES: usize = 4096;

/// Which script symbol a caller asks about.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolQuery {
    /// Declared members of the registration data root.
    DataRoots,
    /// One field path below the registration data root.
    DataPath {
        /// Root-to-leaf path, for example `["user", "name"]`.
        path: Vec<String>,
    },
    /// One statically bound event handler name.
    EventHandler {
        /// Handler name as written in the template.
        name: String,
    },
}

impl SymbolQuery {
    /// Returns the stable lower-case query kind used by the private protocol.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::DataRoots => "dataRoots",
            Self::DataPath { .. } => "dataPath",
            Self::EventHandler { .. } => "eventHandler",
        }
    }
}

/// A script symbol query bound to one immutable view snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolRequest {
    /// Versions and source snapshot the query depends on.
    pub context: RequestContext,
    /// Script file the query is about.
    ///
    /// It must be one of the files in the prepared view; the backend rejects
    /// an unknown subject instead of resolving a path on its own.
    pub subject: FileId,
    /// Query to answer.
    pub query: SymbolQuery,
}

/// Why a script symbol query could not be answered.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SymbolError {
    /// The backend does not implement script symbol queries.
    #[error("script symbols are not implemented")]
    Unsupported,
    /// The query itself is malformed.
    #[error("invalid script symbol query: {0}")]
    Invalid(String),
}

/// One exact token in an original project file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolToken {
    /// Target file.
    pub file: FileId,
    /// Exact byte range of the token content.
    ///
    /// String literal property names exclude the surrounding quotes so an
    /// editor can replace the content without changing the quoting style.
    pub range: SourceRange,
    /// Exact text in the analysis snapshot, when the fact came from a script
    /// snapshot. The edit plan rechecks it against the current file.
    pub expected_text: Option<String>,
}

/// The role of one reference token.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SymbolReferenceRole {
    /// The declaration name token.
    Declaration,
    /// A read of the symbol.
    Read,
    /// A write to the symbol.
    Write,
}

/// One script reference to the queried symbol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolReference {
    /// Exact target token.
    pub token: SymbolToken,
    /// Whether the occurrence reads, writes or declares the symbol.
    pub role: SymbolReferenceRole,
}

/// A statically recognized API occurrence of a data path.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SymbolOccurrenceKind {
    /// A literal key of a `setData` object argument.
    SetDataKey,
    /// A path segment in a component `observers` key or `observer` value.
    ObserverPath,
}

/// One editable path-segment occurrence in a script string or key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolOccurrence {
    /// Exact range of the affected segment, without quotes or dots.
    pub token: SymbolToken,
    /// Which API the occurrence belongs to.
    pub kind: SymbolOccurrenceKind,
    /// Complete statically written path.
    pub path: Vec<String>,
    /// Zero-based index of [`Self::token`] inside [`Self::path`].
    pub segment: usize,
}

/// A safe zero-width insertion point for a missing event handler.
///
/// The backend computes the formatted member text from the original script
/// layout. The LSP layer validates the anchor, the name occurrence and the
/// surrounding text before it becomes an edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerInsertion {
    /// Script file receiving the insertion.
    pub file: FileId,
    /// UTF-8 byte offset of a zero-width anchor.
    pub offset: u32,
    /// Formatted text inserted at [`Self::offset`].
    pub text: String,
}

/// One declared field below a data root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolField {
    /// Name as declared.
    pub name: String,
    /// Declaration token, when the field has a static declaration.
    pub declaration: Option<SymbolToken>,
    /// `true` when the declaration name was written as a string literal.
    pub quoted: bool,
}

/// Script-side facts for one symbol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolResult {
    /// Availability and completeness of the query.
    pub status: CapabilityStatus,
    /// `true` only when every relevant script occurrence is covered.
    ///
    /// Rename and code actions must reject an incomplete result; references
    /// may publish it as a partial set.
    pub complete: bool,
    /// Why the result is incomplete, when it is.
    pub reason: Option<String>,
    /// Declaration token of the queried symbol.
    pub declaration: Option<SymbolToken>,
    /// TypeScript reference tokens, including writes and the declaration.
    pub references: Vec<SymbolReference>,
    /// Recognized `setData`/`observer` path occurrences.
    pub occurrences: Vec<SymbolOccurrence>,
    /// Insertion anchor for a missing event handler.
    pub insertion: Option<HandlerInsertion>,
    /// Declared sibling fields of the resolved data object.
    pub fields: Vec<SymbolField>,
    /// Names already reserved by the owner (for example event methods).
    pub reserved: Vec<String>,
}

impl SymbolResult {
    /// Creates an unsupported answer with no fabricated facts.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Unsupported {
                reason: reason.into(),
            },
            complete: false,
            reason: None,
            declaration: None,
            references: Vec::new(),
            occurrences: Vec::new(),
            insertion: None,
            fields: Vec::new(),
            reserved: Vec::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::assert_is_empty)]
mod tests {
    use std::sync::Arc;

    use crate::context::SourceSnapshot;
    use crate::contract::{
        BackendEpoch, BackendId, ProjectId, ProjectRevision, SnapshotId, SourceRevision,
        ViewRevision,
    };

    use super::*;

    fn context() -> RequestContext {
        let project = ProjectId::new("example");
        let file = FileId::new(project.clone(), "pages/index/index.wxml").expect("file");
        RequestContext::new(
            ProjectRevision::new(project, 1),
            ViewRevision::new(1),
            BackendEpoch::new(BackendId::new("mock"), 1),
            Arc::new(SourceSnapshot::new(
                file,
                SourceRevision::new(1),
                SnapshotId::new(1),
                "{{ user.name }}",
            )),
        )
        .expect("context")
    }

    #[test]
    fn queries_keep_their_shape() {
        let path = SymbolQuery::DataPath {
            path: vec!["user".to_owned(), "name".to_owned()],
        };
        assert_eq!(path.kind(), "dataPath");
        assert_eq!(SymbolQuery::DataRoots.kind(), "dataRoots");
        assert_eq!(
            SymbolQuery::EventHandler {
                name: "onTap".to_owned()
            }
            .kind(),
            "eventHandler"
        );
        let request = SymbolRequest {
            context: context(),
            subject: FileId::new(ProjectId::new("example"), "pages/index/index.ts").expect("file"),
            query: path,
        };
        assert_eq!(request.subject.path(), "pages/index/index.ts");
    }

    #[test]
    fn unsupported_answers_do_not_claim_completeness() {
        let result = SymbolResult::unsupported("no script");
        assert!(!result.status.is_supported());
        assert!(!result.complete);
        assert!(result.declaration.is_none());
        assert!(result.references.is_empty());
        assert!(result.occurrences.is_empty());
        assert!(result.insertion.is_none());
        assert!(result.fields.is_empty());
    }

    #[test]
    fn entries_keep_exact_tokens() {
        let file = FileId::new(ProjectId::new("example"), "pages/index/index.ts").expect("file");
        let token = SymbolToken {
            file,
            range: SourceRange::from_bounds(10, 14).expect("range"),
            expected_text: None,
        };
        let reference = SymbolReference {
            token: token.clone(),
            role: SymbolReferenceRole::Write,
        };
        assert_eq!(reference.token.range.end(), 14);
        let occurrence = SymbolOccurrence {
            token,
            kind: SymbolOccurrenceKind::SetDataKey,
            path: vec!["user".to_owned(), "name".to_owned()],
            segment: 1,
        };
        assert_eq!(occurrence.segment, 1);
        assert_eq!(
            SymbolOccurrenceKind::ObserverPath,
            SymbolOccurrenceKind::ObserverPath
        );
        assert_ne!(
            SymbolOccurrenceKind::SetDataKey,
            SymbolOccurrenceKind::ObserverPath
        );
        assert_ne!(SymbolReferenceRole::Read, SymbolReferenceRole::Declaration);
    }
}
