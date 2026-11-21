//! Native session seam used by the adapter.
//!
//! Production sessions are real [`NativeSession`] values. Tests substitute a
//! fake [`QuerySession`] so hover/diagnostics/completion/definition mapping and
//! epoch invalidation can be exercised without starting a native process.

use crate::backend_tsgo::query::completion::CompletionQueryOptions;
use crate::backend_tsgo::query::definition::{
    DefinitionResult as NativeDefinitionResult, map_definition,
};
use crate::backend_tsgo::query::hover::HoverEvidence;
use wxml_backend_tsgo::input::AnalysisInput;
use wxml_backend_tsgo::native::document_symbol::DocumentSymbolsEvidence;
use wxml_backend_tsgo::native::session::{
    AutoImportPreparation, NativeSession, SessionOptions, TypeQuery,
};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::native::wire::{RawCompletionList, RawDiagnostic};
use wxml_backend_tsgo::native::{NativeError, UnavailableReason};

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

    /// Legacy: returns the type evidence at one UTF-16 position.
    ///
    /// The hover adapter uses [`Self::hover_evidence`]; this individual query
    /// remains for session decorators and test fakes that mirror the native
    /// surface.
    fn type_query(&mut self, path: &str, position: Utf16Position)
    -> Result<TypeQuery, NativeError>;

    /// Returns composed hover evidence for one UTF-16 position.
    ///
    /// Required: every session must return the backend's type, documentation,
    /// declaration and typed-gap evidence. There is deliberately no fallback
    /// that could report `Supported` without declaration evidence.
    fn hover_evidence(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError>;

    /// Returns native completions at one UTF-16 position.
    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError>;

    /// Returns native completions with explicit request options.
    ///
    /// The default ignores the options so fakes and decorators that do not
    /// model `includeSymbol` keep their historical behavior. Production
    /// sessions override it and forward to
    /// [`NativeSession::completions_with_options`].
    fn completions_with_options(
        &mut self,
        path: &str,
        position: Utf16Position,
        _options: CompletionQueryOptions,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.completions(path, position)
    }

    /// Enables auto-import preparation for one project-relative script path.
    ///
    /// The default reports [`AutoImportPreparation::NotSupported`] so a fake or
    /// decorator that does not model native snapshots does not change the
    /// historical completion status. [`LiveQuerySession`] forwards to
    /// [`NativeSession::prepare_auto_imports`].
    fn prepare_auto_imports(&mut self, _path: &str) -> Result<AutoImportPreparation, NativeError> {
        Ok(AutoImportPreparation::NotSupported)
    }

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

    /// Returns a stable reason when the open type environment is degraded.
    ///
    /// The default `None` keeps legacy and fake sessions unchanged. Production
    /// sessions forward the explicit VFS degradations of the currently open
    /// snapshot; the reason never contains host paths or user option values.
    fn environment_degradation(&self) -> Option<String> {
        None
    }

    /// Returns whether the underlying engine session is still usable.
    ///
    /// The adapter consults this only after a fatal query result to decide
    /// whether one rebuild-and-retry is justified. A real native session
    /// reports a poisoned, exited or non-ready process as dead. Fakes and
    /// decorators default to `true` so historical behavior is unchanged.
    fn is_alive(&mut self) -> bool {
        true
    }

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

/// Session factory for the `HostOnly` selector.
///
/// `HostOnly` means the running executable is not inside an installed package,
/// so there is no engine to start. The adapter short-circuits every
/// session-backed query before this factory is consulted; returning a typed
/// `Unavailable` here keeps that invariant fail-closed if the short-circuit is
/// ever bypassed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HostOnlySessionFactory;

impl SessionFactory for HostOnlySessionFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        Err(NativeError::Unavailable {
            reason: UnavailableReason::MissingAsset,
        })
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

    fn hover_evidence(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError> {
        self.session.hover_evidence(path, position)
    }

    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.session.completions(path, position)
    }

    fn completions_with_options(
        &mut self,
        path: &str,
        position: Utf16Position,
        options: CompletionQueryOptions,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.session
            .completions_with_options(path, position, options)
    }

    fn prepare_auto_imports(&mut self, path: &str) -> Result<AutoImportPreparation, NativeError> {
        self.session.prepare_auto_imports(path)
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

    fn environment_degradation(&self) -> Option<String> {
        super::environment::degradation_reason(self.session.environment_degradations())
    }

    fn is_alive(&mut self) -> bool {
        self.session.is_alive()
    }

    fn shutdown(self: Box<Self>) {
        let Self { session } = *self;
        let _ = session.shutdown();
    }
}
