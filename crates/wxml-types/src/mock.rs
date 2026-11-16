//! In-process [`Backend`] implementation for tests and early integration.
//!
//! [`MockBackend`] records every call and lets tests preset results, replace
//! them between calls (AC-009), inject per-method failures and override
//! capabilities or the epoch. Injected errors take precedence over preset
//! results but are recorded as calls too, so tests can assert both the failure
//! and the request that caused it. The mock never panics on a poisoned lock.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::backend::{
    Backend, BackendCapabilities, BackendError, DefinitionRequest, DefinitionResult,
    DiagnosticsRequest, DiagnosticsResult, HoverRequest, HoverResult,
};
use crate::contract::{BackendEpoch, BackendId};
use crate::status::CapabilityStatus;

/// One query observed by [`MockBackend`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendCall {
    /// A hover query.
    Hover(HoverRequest),
    /// A diagnostics query.
    Diagnostics(DiagnosticsRequest),
    /// A definition query.
    Definition(DefinitionRequest),
}

/// A configurable backend for tests.
#[derive(Debug, Default)]
pub struct MockBackend {
    state: Mutex<MockState>,
}

#[derive(Debug, Default)]
struct MockState {
    hover: Option<HoverResult>,
    diagnostics: Option<DiagnosticsResult>,
    definition: Option<DefinitionResult>,
    capabilities: Option<BackendCapabilities>,
    epoch: Option<BackendEpoch>,
    hover_error: Option<BackendError>,
    diagnostics_error: Option<BackendError>,
    definition_error: Option<BackendError>,
    calls: Vec<BackendCall>,
}

impl MockBackend {
    /// Creates a mock with default capabilities, epoch and no injected errors.
    ///
    /// Diagnostics default to a clean `Supported` result; definition defaults
    /// to `Unsupported`; hover has no result until one is preset.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Presets the result returned by [`Backend::hover`].
    #[must_use]
    pub fn with_hover(self, result: HoverResult) -> Self {
        self.set_hover(result);
        self
    }

    /// Presets the result returned by [`Backend::diagnostics`].
    #[must_use]
    pub fn with_diagnostics(self, result: DiagnosticsResult) -> Self {
        self.set_diagnostics(result);
        self
    }

    /// Presets the result returned by [`Backend::definition`].
    #[must_use]
    pub fn with_definition(self, result: DefinitionResult) -> Self {
        self.set_definition(result);
        self
    }

    /// Makes every query fail with `error`.
    #[must_use]
    pub fn with_error(self, error: BackendError) -> Self {
        self.fail_hover(error.clone());
        self.fail_diagnostics(error.clone());
        self.fail_definition(error);
        self
    }

    /// Replaces the hover result.
    pub fn set_hover(&self, result: HoverResult) {
        self.lock().hover = Some(result);
    }

    /// Replaces the diagnostics result.
    pub fn set_diagnostics(&self, result: DiagnosticsResult) {
        self.lock().diagnostics = Some(result);
    }

    /// Replaces the definition result.
    pub fn set_definition(&self, result: DefinitionResult) {
        self.lock().definition = Some(result);
    }

    /// Overrides the advertised capabilities.
    pub fn set_capabilities(&self, capabilities: BackendCapabilities) {
        self.lock().capabilities = Some(capabilities);
    }

    /// Overrides the reported epoch.
    pub fn set_epoch(&self, epoch: BackendEpoch) {
        self.lock().epoch = Some(epoch);
    }

    /// Makes hover queries fail with `error`.
    pub fn fail_hover(&self, error: BackendError) {
        self.lock().hover_error = Some(error);
    }

    /// Makes diagnostics queries fail with `error`.
    pub fn fail_diagnostics(&self, error: BackendError) {
        self.lock().diagnostics_error = Some(error);
    }

    /// Makes definition queries fail with `error`.
    pub fn fail_definition(&self, error: BackendError) {
        self.lock().definition_error = Some(error);
    }

    /// Removes every injected error and restores preset results.
    pub fn clear_errors(&self) {
        let mut state = self.lock();
        state.hover_error = None;
        state.diagnostics_error = None;
        state.definition_error = None;
    }

    /// Returns a snapshot of every call recorded so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<BackendCall> {
        self.lock().calls.clone()
    }

    /// Locks the state, recovering the data if another thread panicked.
    fn lock(&self) -> MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Backend for MockBackend {
    fn epoch(&self) -> BackendEpoch {
        self.lock().epoch.clone().unwrap_or_else(default_epoch)
    }

    fn hover(&self, request: HoverRequest) -> Result<HoverResult, BackendError> {
        let mut state = self.lock();
        state.calls.push(BackendCall::Hover(request));
        if let Some(error) = state.hover_error.clone() {
            return Err(error);
        }
        state
            .hover
            .clone()
            .ok_or_else(|| BackendError::Unavailable("mock backend has no hover result".to_owned()))
    }

    fn diagnostics(&self, request: DiagnosticsRequest) -> Result<DiagnosticsResult, BackendError> {
        let mut state = self.lock();
        state.calls.push(BackendCall::Diagnostics(request));
        if let Some(error) = state.diagnostics_error.clone() {
            return Err(error);
        }
        Ok(state.diagnostics.clone().unwrap_or_else(clean_diagnostics))
    }

    fn definition(&self, request: DefinitionRequest) -> Result<DefinitionResult, BackendError> {
        let mut state = self.lock();
        state.calls.push(BackendCall::Definition(request));
        if let Some(error) = state.definition_error.clone() {
            return Err(error);
        }
        Ok(state
            .definition
            .clone()
            .unwrap_or_else(unsupported_definition))
    }

    fn capabilities(&self) -> BackendCapabilities {
        self.lock()
            .capabilities
            .clone()
            .unwrap_or_else(default_capabilities)
    }
}

/// Default epoch reported by a mock backend.
fn default_epoch() -> BackendEpoch {
    BackendEpoch::new(BackendId::new("mock"), 1)
}

/// A clean, fully supported diagnostic set.
const fn clean_diagnostics() -> DiagnosticsResult {
    DiagnosticsResult {
        diagnostics: Vec::new(),
        status: CapabilityStatus::Supported,
    }
}

/// The same result the trait default returns for definition.
fn unsupported_definition() -> DefinitionResult {
    DefinitionResult {
        locations: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: "definition is not implemented".to_owned(),
        },
    }
}

/// Hover and diagnostics supported, definition and completion not implemented.
fn default_capabilities() -> BackendCapabilities {
    BackendCapabilities {
        hover: CapabilityStatus::Supported,
        diagnostics: CapabilityStatus::Supported,
        definition: CapabilityStatus::Unsupported {
            reason: "definition is not implemented".to_owned(),
        },
        completion: CapabilityStatus::Unsupported {
            reason: "completion is not implemented".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::backend::Diagnostic;
    use crate::context::{RequestContext, SourceSnapshot};
    use crate::contract::{
        FileId, ProjectId, ProjectRevision, SnapshotId, SourceRevision, ViewRevision,
    };
    use wxml_text::SourceRange;

    fn file() -> FileId {
        FileId::new(ProjectId::new("example"), "pages/index/index.wxml").expect("valid path")
    }

    fn context() -> RequestContext {
        RequestContext::new(
            ProjectRevision::new(ProjectId::new("example"), 1),
            ViewRevision::new(1),
            default_epoch(),
            Arc::new(SourceSnapshot::new(
                file(),
                SourceRevision::new(1),
                SnapshotId::new(1),
                "{{ user.name }}",
            )),
        )
        .expect("matching projects")
    }

    fn hover_request() -> HoverRequest {
        HoverRequest {
            context: context(),
            offset: 3,
        }
    }

    fn definition_request() -> DefinitionRequest {
        DefinitionRequest {
            context: context(),
            offset: 3,
        }
    }

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::from_bounds(start, end).expect("valid range")
    }

    fn hover_result(contents: &str) -> HoverResult {
        HoverResult {
            contents: contents.to_owned(),
            markup: crate::backend::MarkupKind::Markdown,
            range: Some(range(3, 12)),
            status: CapabilityStatus::Supported,
            quality: crate::backend::TypeQuality::Known,
        }
    }

    fn diagnostic() -> Diagnostic {
        Diagnostic {
            range: range(20, 24),
            severity: crate::backend::DiagnosticSeverity::Error,
            message: "Property 'nmae' does not exist".to_owned(),
            code: None,
        }
    }

    fn diagnostics_result() -> DiagnosticsResult {
        DiagnosticsResult {
            diagnostics: vec![diagnostic()],
            status: CapabilityStatus::Supported,
        }
    }

    fn definition_result() -> DefinitionResult {
        DefinitionResult {
            locations: vec![crate::backend::SourceLocation {
                file: FileId::new(ProjectId::new("example"), "pages/index/index.ts")
                    .expect("valid path"),
                range: range(40, 48),
            }],
            status: CapabilityStatus::Supported,
        }
    }

    #[test]
    fn reports_default_epoch() {
        assert_eq!(MockBackend::new().epoch(), default_epoch());
        assert_eq!(MockBackend::new().epoch().backend().as_str(), "mock");
    }

    #[test]
    fn reports_status_based_capabilities() {
        let capabilities = MockBackend::new().capabilities();
        assert!(capabilities.hover.is_supported());
        assert!(capabilities.diagnostics.is_supported());
        assert!(!capabilities.definition.is_supported());
        assert!(!capabilities.completion.is_supported());
    }

    #[test]
    fn returns_preset_hover_and_records_the_call() {
        let backend = MockBackend::new().with_hover(hover_result("string"));
        let result = backend.hover(hover_request()).expect("hover succeeds");
        assert_eq!(result, hover_result("string"));
        assert_eq!(backend.calls(), vec![BackendCall::Hover(hover_request())]);
    }

    #[test]
    fn replaces_a_hover_result_between_calls() {
        let backend = MockBackend::new().with_hover(hover_result("old"));
        assert_eq!(
            backend
                .hover(hover_request())
                .expect("hover succeeds")
                .contents,
            "old"
        );
        backend.set_hover(hover_result("new"));
        assert_eq!(
            backend
                .hover(hover_request())
                .expect("hover succeeds")
                .contents,
            "new"
        );
    }

    #[test]
    fn returns_preset_diagnostics_and_records_the_call() {
        let backend = MockBackend::new().with_diagnostics(diagnostics_result());
        let result = backend
            .diagnostics(DiagnosticsRequest { context: context() })
            .expect("diagnostics succeed");
        assert_eq!(result, diagnostics_result());
        assert_eq!(
            backend.calls(),
            vec![BackendCall::Diagnostics(DiagnosticsRequest {
                context: context()
            })]
        );
    }

    #[test]
    fn returns_preset_definition_and_records_the_call() {
        let backend = MockBackend::new().with_definition(definition_result());
        let result = backend
            .definition(definition_request())
            .expect("definition succeeds");
        assert_eq!(result, definition_result());
        assert_eq!(
            backend.calls(),
            vec![BackendCall::Definition(definition_request())]
        );
    }

    #[test]
    fn definition_defaults_to_unsupported() {
        let result = MockBackend::new()
            .definition(definition_request())
            .expect("mock answers");
        assert_eq!(result.locations, Vec::new());
        assert!(!result.status.is_supported());
    }

    #[test]
    fn diagnostics_default_to_clean_and_supported() {
        let result = MockBackend::new()
            .diagnostics(DiagnosticsRequest { context: context() })
            .expect("diagnostics succeed");
        assert_eq!(result.diagnostics, Vec::new());
        assert!(result.status.is_supported());
    }

    #[test]
    fn fails_honestly_when_no_hover_is_configured() {
        let backend = MockBackend::new();
        let error = backend
            .hover(hover_request())
            .expect_err("no hover configured");
        assert_eq!(
            error,
            BackendError::Unavailable("mock backend has no hover result".to_owned())
        );
    }

    #[test]
    fn hover_can_fail_while_diagnostics_succeed() {
        let backend = MockBackend::new()
            .with_diagnostics(diagnostics_result())
            .with_hover(hover_result("string"));
        backend.fail_hover(BackendError::Timeout);
        assert_eq!(
            backend.hover(hover_request()).expect_err("hover fails"),
            BackendError::Timeout
        );
        assert_eq!(
            backend
                .diagnostics(DiagnosticsRequest { context: context() })
                .expect("diagnostics succeed"),
            diagnostics_result()
        );
    }

    #[test]
    fn injected_errors_fail_every_query_but_are_still_recorded() {
        let backend = MockBackend::new().with_error(BackendError::Timeout);
        assert_eq!(
            backend.hover(hover_request()).expect_err("error injected"),
            BackendError::Timeout
        );
        assert_eq!(
            backend
                .diagnostics(DiagnosticsRequest { context: context() })
                .expect_err("error injected"),
            BackendError::Timeout
        );
        assert_eq!(
            backend
                .definition(definition_request())
                .expect_err("error injected"),
            BackendError::Timeout
        );
        assert_eq!(backend.calls().len(), 3);
    }

    #[test]
    fn errors_can_be_injected_per_method() {
        let backend = MockBackend::new()
            .with_hover(hover_result("string"))
            .with_diagnostics(diagnostics_result());
        backend.fail_definition(BackendError::Unavailable("no node".to_owned()));
        assert!(backend.hover(hover_request()).is_ok());
        assert!(
            backend
                .diagnostics(DiagnosticsRequest { context: context() })
                .is_ok()
        );
        assert_eq!(
            backend
                .definition(definition_request())
                .expect_err("definition fails"),
            BackendError::Unavailable("no node".to_owned())
        );
    }

    #[test]
    fn cleared_errors_restore_the_preset_results() {
        let backend = MockBackend::new()
            .with_hover(hover_result("string"))
            .with_error(BackendError::Cancelled);
        assert!(backend.hover(hover_request()).is_err());
        backend.clear_errors();
        assert_eq!(
            backend.hover(hover_request()).expect("hover succeeds"),
            hover_result("string")
        );
        assert!(
            backend
                .diagnostics(DiagnosticsRequest { context: context() })
                .is_ok()
        );
    }

    #[test]
    fn capabilities_and_epoch_can_be_overridden() {
        let backend = MockBackend::new();
        let mut capabilities = default_capabilities();
        capabilities.hover = CapabilityStatus::Partial {
            reason: "only data roots".to_owned(),
        };
        backend.set_capabilities(capabilities.clone());
        assert_eq!(backend.capabilities(), capabilities);

        let epoch = BackendEpoch::new(BackendId::new("mock"), 7);
        backend.set_epoch(epoch.clone());
        assert_eq!(backend.epoch(), epoch);
    }

    #[test]
    fn records_calls_in_order() {
        let backend = MockBackend::new();
        let first = hover_request();
        let second = HoverRequest {
            context: context(),
            offset: 5,
        };
        let _ = backend.hover(first.clone());
        let _ = backend.hover(second.clone());
        let _ = backend.definition(definition_request());
        assert_eq!(
            backend.calls(),
            vec![
                BackendCall::Hover(first),
                BackendCall::Hover(second),
                BackendCall::Definition(definition_request()),
            ]
        );
    }

    #[test]
    fn shares_a_mutable_backend_across_threads() {
        let backend = Arc::new(MockBackend::new().with_hover(hover_result("string")));
        let handle = std::thread::spawn({
            let backend = Arc::clone(&backend);
            move || backend.hover(hover_request())
        });
        assert!(handle.join().expect("thread joins").is_ok());
        assert_eq!(backend.calls().len(), 1);
    }
}
