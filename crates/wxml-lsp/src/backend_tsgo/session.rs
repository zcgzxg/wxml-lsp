//! Native session seam used by the adapter.
//!
//! Production sessions are real [`NativeSession`] values. Tests substitute a
//! fake [`QuerySession`] so hover/diagnostics/completion/definition mapping and
//! epoch invalidation can be exercised without starting a native process.

use wxml_backend_tsgo::input::AnalysisInput;
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::document_symbol::DocumentSymbolsEvidence;
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::native::wire::{RawCompletionList, RawDiagnostic};
use wxml_backend_tsgo::query::definition::{
    DefinitionResult as NativeDefinitionResult, map_definition,
};
use wxml_backend_tsgo::query::documentation::{DocumentationResult, map_documentation};
use wxml_types::CapabilityStatus;

/// Documentation evidence for one hover position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HoverDocumentation {
    /// Whether the engine found a symbol at the position.
    pub(crate) symbol: bool,
    /// Documentation and `JSDoc` tags, or an explicit failure status.
    pub(crate) result: DocumentationResult,
}

impl HoverDocumentation {
    /// No symbol exists at the position; this is a checked empty answer.
    #[must_use]
    pub(crate) const fn no_symbol() -> Self {
        Self {
            symbol: false,
            result: DocumentationResult {
                comment: None,
                tags: Vec::new(),
                status: CapabilityStatus::Supported,
            },
        }
    }

    /// A symbol exists but has no content-addressed reference descriptor.
    #[must_use]
    pub(crate) fn symbol_without_reference() -> Self {
        Self {
            symbol: true,
            result: DocumentationResult {
                comment: None,
                tags: Vec::new(),
                status: CapabilityStatus::Partial {
                    reason: "the native symbol has no documentation reference".to_owned(),
                },
            },
        }
    }
}

/// Operations the adapter needs from one native session.
pub(crate) trait QuerySession: Send {
    /// Opens a native snapshot for `input`.
    fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError>;

    /// Advances the open snapshot to `input`.
    fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError>;

    /// Returns semantic diagnostics for one project-relative script path.
    fn semantic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError>;

    /// Returns syntactic diagnostics for one project-relative script path.
    fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError>;

    /// Returns the type evidence at one UTF-16 position.
    fn type_query(&mut self, path: &str, position: Utf16Position)
    -> Result<TypeQuery, NativeError>;

    /// Returns bound documentation evidence for one UTF-16 position.
    fn hover_documentation(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverDocumentation, NativeError>;

    /// Returns native completions at one UTF-16 position.
    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError>;

    /// Returns decoded definition targets at one UTF-16 position.
    fn definition(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<NativeDefinitionResult, NativeError>;

    /// Returns raw document-symbol evidence for one project-relative path.
    ///
    /// The ranges are in the exact text sent for `path`; the caller owns the
    /// shadow-to-original mapping.
    fn document_symbols(&mut self, path: &str) -> Result<DocumentSymbolsEvidence, NativeError>;

    /// Releases the snapshot and stops the process.
    fn shutdown(self: Box<Self>);
}

/// Starts one [`QuerySession`].
pub(crate) trait SessionFactory: Send + Sync {
    /// Starts a validated native session.
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError>;
}

/// Production factory for the pinned `tsgo` executable.
#[derive(Debug, Clone)]
pub(crate) struct NativeSessionFactory {
    options: SessionOptions,
}

impl NativeSessionFactory {
    /// Creates a factory from a verified executable and an empty sandbox.
    #[must_use]
    pub(crate) const fn new(options: SessionOptions) -> Self {
        Self { options }
    }
}

impl SessionFactory for NativeSessionFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        let session = NativeSession::start(self.options.clone())?;
        Ok(Box::new(LiveQuerySession { session }))
    }
}

struct LiveQuerySession {
    session: NativeSession,
}

impl QuerySession for LiveQuerySession {
    fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.session.open_project(input)
    }

    fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.session.update_project(input)
    }

    fn semantic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.session.semantic_diagnostics(path)
    }

    fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.session.syntactic_diagnostics(path)
    }

    fn type_query(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<TypeQuery, NativeError> {
        self.session.type_query(path, position)
    }

    fn hover_documentation(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverDocumentation, NativeError> {
        let Some(symbol) = self.session.symbol_at_position(path, position)? else {
            return Ok(HoverDocumentation::no_symbol());
        };
        let Some(reference) = symbol.reference().cloned() else {
            return Ok(HoverDocumentation::symbol_without_reference());
        };
        let raw = self.session.documentation(&reference)?;
        Ok(HoverDocumentation {
            symbol: true,
            result: map_documentation(&raw),
        })
    }

    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.session.completions(path, position)
    }

    fn definition(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<NativeDefinitionResult, NativeError> {
        let evidence = self.session.definition(path, position)?;
        Ok(map_definition(&evidence))
    }

    fn document_symbols(&mut self, path: &str) -> Result<DocumentSymbolsEvidence, NativeError> {
        self.session.document_symbols(path)
    }

    fn shutdown(self: Box<Self>) {
        let Self { session } = *self;
        let _ = session.shutdown();
    }
}
