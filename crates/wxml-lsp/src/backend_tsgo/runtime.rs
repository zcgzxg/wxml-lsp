//! Stable virtual working directory for the in-process type session.
//!
//! The type engine runs inside the language-server process and reads files only
//! through the snapshot virtual file system. It does not need a real working
//! directory, so this module deliberately creates nothing: [`virtual_cwd`]
//! returns one fixed path below the operating system temporary directory and
//! [`VirtualSessionFactory`] passes it to [`SessionOptions`] without requiring
//! the path to exist. There is no `0700` sandbox directory, no per-session
//! directory, no cleanup and no host fallback.
//!
//! `SessionOptions::executable` is retained for compatibility with the session
//! type but is no longer used by the in-process engine; the factory passes an
//! empty path.

use std::path::PathBuf;

use wxml_backend_tsgo::input::AnalysisInput;
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::document_symbol::DocumentSymbolsEvidence;
use wxml_backend_tsgo::native::session::{AutoImportPreparation, SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::Utf16Position;
use wxml_backend_tsgo::native::wire::{RawCompletionList, RawDiagnostic};

use crate::backend_tsgo::query::completion::CompletionQueryOptions;
use crate::backend_tsgo::query::definition::DefinitionResult as NativeDefinitionResult;
use crate::backend_tsgo::query::hover::HoverEvidence;
use crate::backend_tsgo::session::{NativeSessionFactory, QuerySession, SessionFactory};

/// Directory name appended to the OS temporary directory for the virtual cwd.
pub(crate) const VIRTUAL_CWD_DIRECTORY: &str = "wxml-lsp-tsrs-cwd";

/// Returns the stable virtual working directory.
///
/// The path is deterministic for the process environment and is never created,
/// inspected or written by this crate. Two sessions may share it because the
/// in-process engine does not resolve relative paths against it.
#[must_use]
pub(crate) fn virtual_cwd() -> PathBuf {
    std::env::temp_dir().join(VIRTUAL_CWD_DIRECTORY)
}

/// Returns whether the legacy real-type gate is enabled.
///
/// The variable names are kept for CI compatibility; the engine itself is
/// in-process and no external executable is involved.
#[cfg(test)]
pub(crate) fn real_engine_required() -> bool {
    matches!(
        std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
        Ok("1" | "true")
    )
}

/// Prints one skip line and returns `true` unless the gate is required.
///
/// Tests call this at the top and return when it reports a skip. With
/// `WXML_REQUIRE_REAL_ENGINE=1|true` it panics, so a missing capability is a
/// hard failure instead of a silent pass.
#[cfg(test)]
pub(crate) fn skip_unless_real_engine(scope: &str) -> bool {
    if real_engine_required() {
        return false;
    }
    eprintln!(
        "optional real-type test skipped: set WXML_REQUIRE_REAL_ENGINE=1 to require the \
         in-process engine ({scope})"
    );
    true
}

/// Session factory for the in-process engine.
///
/// Every session receives the same stable virtual cwd. No directory is created
/// and no process is spawned by this factory.
#[derive(Debug, Clone)]
pub(crate) struct VirtualSessionFactory {
    options: SessionOptions,
}

impl VirtualSessionFactory {
    /// Creates a factory with the stable virtual cwd.
    ///
    /// `SessionOptions::executable` is unused by the in-process engine, so an
    /// empty path is passed and no install tree is required.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            options: SessionOptions::new(PathBuf::new(), virtual_cwd()),
        }
    }

    /// Creates a factory with an explicit executable placeholder.
    ///
    /// Retained for tests that construct a legacy [`SessionOptions`] value
    /// directly. Production callers use [`Self::new`].
    #[cfg(test)]
    pub(crate) fn with_executable(executable: impl Into<PathBuf>) -> Self {
        Self {
            options: SessionOptions::new(executable.into(), virtual_cwd()),
        }
    }

    /// Returns the virtual cwd passed to the session.
    #[cfg(test)]
    pub(crate) fn cwd(&self) -> &std::path::Path {
        &self.options.sandbox_root
    }
}

impl Default for VirtualSessionFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionFactory for VirtualSessionFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        let inner = NativeSessionFactory::new(self.options.clone()).start()?;
        Ok(Box::new(VirtualSession { inner }))
    }
}

/// Session decorator used by this factory.
///
/// It is a thin pass-through today; keeping the wrapper makes the factory the
/// single place that owns the virtual cwd and leaves room for a future
/// in-process-specific lifecycle without changing the adapter.
struct VirtualSession {
    inner: Box<dyn QuerySession>,
}

impl QuerySession for VirtualSession {
    fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.inner.open_project(input)
    }

    fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.inner.update_project(input)
    }

    fn semantic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.inner.semantic_diagnostics(path)
    }

    fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.inner.syntactic_diagnostics(path)
    }

    fn type_query(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<TypeQuery, NativeError> {
        self.inner.type_query(path, position)
    }

    fn hover_evidence(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError> {
        self.inner.hover_evidence(path, position)
    }

    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.inner.completions(path, position)
    }

    fn completions_with_options(
        &mut self,
        path: &str,
        position: Utf16Position,
        options: CompletionQueryOptions,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.inner.completions_with_options(path, position, options)
    }

    fn prepare_auto_imports(&mut self, path: &str) -> Result<AutoImportPreparation, NativeError> {
        self.inner.prepare_auto_imports(path)
    }

    fn definition(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<NativeDefinitionResult, NativeError> {
        self.inner.definition(path, position)
    }

    fn document_symbols(&mut self, path: &str) -> Result<DocumentSymbolsEvidence, NativeError> {
        self.inner.document_symbols(path)
    }

    fn environment_degradation(&self) -> Option<String> {
        self.inner.environment_degradation()
    }

    fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }

    fn shutdown(self: Box<Self>) {
        self.inner.shutdown();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn virtual_cwd_is_a_stable_absolute_path() {
        let first = virtual_cwd();
        let second = virtual_cwd();
        assert_eq!(first, second);
        assert!(first.is_absolute(), "{}", first.display());
        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some(VIRTUAL_CWD_DIRECTORY)
        );
    }

    #[test]
    fn factory_does_not_require_the_directory_to_exist() {
        let factory = VirtualSessionFactory::new();
        assert_eq!(factory.cwd(), virtual_cwd().as_path());
    }
}
