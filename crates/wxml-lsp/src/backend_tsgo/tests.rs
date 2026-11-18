//! Adapter-level tests with a fake host view and fake native session.
//!
//! These tests exercise the real `wxml_types::Backend` implementation, shadow
//! construction and coordinate mapping; they do not require a native binary.
//! The real-engine smoke test at the end is opt-in through `TSGO_BIN`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::input::{AnalysisInput, SourceFile as InputSourceFile};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::UnavailableReason;
use wxml_backend_tsgo::native::session::DocumentationQuery;
use wxml_backend_tsgo::native::session::{SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::{
    Utf16Offset, Utf16Position, offset_to_position, utf16_len,
};
use wxml_backend_tsgo::native::wire::{
    RawCompletionEntry, RawCompletionList, RawDiagnostic, RawJsDocTag, RawPosition, RawType,
};
use wxml_backend_tsgo::query::documentation::map_documentation;
use wxml_project::ProjectPath;
use wxml_text::SourceRange;
use wxml_types::{
    Backend, BackendEpoch, BackendError, BackendId, CapabilityStatus, CheckFragment,
    CompletionKind, CompletionRequest, DefinitionRequest, Diagnostic, DiagnosticSeverity,
    DiagnosticsRequest, FileId, HoverRequest, ProjectId, ProjectRevision, RequestContext, RootKind,
    SnapshotId, SourceRevision, SourceSnapshot, SymbolQuery, SymbolRequest, ViewRevision,
};

use super::input::ViewProvider;
use super::session::{HoverDocumentation, QuerySession};
use super::{BACKEND_ID, TsgoBackendAdapter};
use crate::project_host::{HostError, PreparedFile, PreparedView, ViewTarget};

const WXML: &str = "<view>{{ user.name }}</view>";
const SCRIPT: &str = "Page({\n  data: { user: { name: \"Ada\", age: 42 } },\n});\n";
const SCRIPT_PATH: &str = "pages/index/index.ts";
const WXML_PATH: &str = "pages/index/index.wxml";
const COMMENT: &str = "Display name.";

fn project() -> ProjectId {
    ProjectId::new("demo")
}

fn wxml_file() -> FileId {
    FileId::new(project(), WXML_PATH).expect("wxml file")
}

fn script_file() -> FileId {
    FileId::new(project(), SCRIPT_PATH).expect("script file")
}

fn project_path(value: &str) -> ProjectPath {
    ProjectPath::new(value).expect("project path")
}

fn fragment(wxml: &str, expression: &str, id: &str) -> CheckFragment {
    let start = wxml.find(expression).expect("expression in template");
    let end = start + expression.len();
    CheckFragment {
        fragment_id: id.to_owned(),
        expr_text: expression.to_owned(),
        target: None,
        bindings: Vec::new(),
        guards: Vec::new(),
        wxml_range: SourceRange::from_bounds(
            u32::try_from(start).expect("start"),
            u32::try_from(end).expect("end"),
        )
        .expect("fragment range"),
        fragment_revision: 1,
        root: RootKind::Page,
    }
}

fn prepared_file(path: &str, text: &str) -> PreparedFile {
    PreparedFile {
        file: FileId::new(project(), path).expect("file"),
        path: project_path(path),
        kind: "ts",
        text: Arc::from(text),
        source_revision: 1,
    }
}

fn prepared_view(wxml: &str, script: &str, expression: &str) -> PreparedView {
    prepared_view_with_files(wxml, expression, vec![prepared_file(SCRIPT_PATH, script)])
}

fn prepared_view_with_files(
    wxml: &str,
    expression: &str,
    files: Vec<PreparedFile>,
) -> PreparedView {
    prepared_view_with_fragments(wxml, files, vec![fragment(wxml, expression, "f0")])
}

fn prepared_view_with_fragments(
    wxml: &str,
    files: Vec<PreparedFile>,
    fragments: Vec<CheckFragment>,
) -> PreparedView {
    let mut view = empty_view(wxml, files);
    view.fragments = Arc::from(fragments);
    view
}

fn empty_view(wxml: &str, files: Vec<PreparedFile>) -> PreparedView {
    let _ = wxml;
    PreparedView {
        view_id: format!("{WXML_PATH}:1"),
        target: Some(ViewTarget {
            wxml: project_path(WXML_PATH),
            owner: RootKind::Page,
            script: project_path(SCRIPT_PATH),
            script_file: script_file(),
            view_revision: ViewRevision::new(1),
        }),
        files: Arc::from(files),
        compiler_options: serde_json::Map::new(),
        fragments: Arc::from(Vec::new()),
        components: Arc::from(Vec::new()),
        events: Arc::from(Vec::new()),
        project_root: PathBuf::from("/work/demo"),
        syntax_diagnostics: Vec::new(),
        status_hint: None,
        source_complete: true,
    }
}

fn syntax_diagnostic(wxml: &str, needle: &str, message: &str) -> Diagnostic {
    let start = offset_of(wxml, needle);
    let end = start + u32::try_from(needle.len()).expect("needle length");
    Diagnostic {
        range: SourceRange::from_bounds(start, end).expect("syntax range"),
        severity: DiagnosticSeverity::Error,
        message: message.to_owned(),
        code: Some("syntax".to_owned()),
    }
}

struct FakeViews {
    view: PreparedView,
}

impl FakeViews {
    fn new(view: PreparedView) -> Self {
        Self { view }
    }
}

impl ViewProvider for FakeViews {
    fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Ok(self.view.clone())
    }
}

struct FailingViews;

impl ViewProvider for FailingViews {
    fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Err(HostError::NoProject)
    }
}

#[derive(Clone, Debug)]
enum FakeFailure {
    Timeout,
    Exited,
    InvalidParams,
    Unsupported(&'static str),
}

impl FakeFailure {
    fn native(&self) -> NativeError {
        match self {
            Self::Timeout => NativeError::Timeout(10),
            Self::Exited => NativeError::Unavailable {
                reason: UnavailableReason::ProcessExited,
            },
            Self::InvalidParams => NativeError::InvalidParams {
                reason: "file is not in the open snapshot",
            },
            Self::Unsupported(feature) => NativeError::Unsupported { method: feature },
        }
    }
}

#[derive(Clone, Debug)]
enum FakeTypeOutcome {
    Null,
    Known(String),
    Any(String),
    Failure(FakeFailure),
}

#[derive(Clone, Debug)]
enum FakeDocsOutcome {
    NoSymbol,
    Documented(DocumentationQuery),
    Failure(FakeFailure),
}

#[derive(Clone, Debug)]
enum FakeDiagnosticOutcome {
    Empty,
    Diagnostics(Vec<FakeDiagnostic>),
    Failure(FakeFailure),
}

#[derive(Clone, Debug)]
struct FakeDiagnostic {
    needle: &'static str,
    code: i64,
    category: i64,
    message: &'static str,
}

#[derive(Clone, Debug)]
enum FakeCompletionOutcome {
    Null,
    List {
        entries: Vec<RawCompletionEntry>,
        incomplete: bool,
    },
    Failure(FakeFailure),
}

#[derive(Debug, Default)]
struct SlowGate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl SlowGate {
    fn enter_and_wait(&self) {
        let mut state = self.state.lock().expect("gate");
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self.changed.wait(state).expect("gate wait");
        }
        drop(state);
    }

    fn wait_until_entered(&self) {
        let mut state = self.state.lock().expect("gate");
        while !state.0 {
            state = self.changed.wait(state).expect("gate wait");
        }
        drop(state);
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("gate");
        state.1 = true;
        self.changed.notify_all();
        drop(state);
    }
}

#[derive(Clone, Debug)]
struct FakeBehavior {
    type_outcome: FakeTypeOutcome,
    docs_outcome: FakeDocsOutcome,
    semantic: FakeDiagnosticOutcome,
    syntactic: FakeDiagnosticOutcome,
    completions: FakeCompletionOutcome,
    update_failure: Option<FakeFailure>,
    type_gate: Option<Arc<SlowGate>>,
}

impl Default for FakeBehavior {
    fn default() -> Self {
        Self {
            type_outcome: FakeTypeOutcome::Known("string".to_owned()),
            docs_outcome: FakeDocsOutcome::NoSymbol,
            semantic: FakeDiagnosticOutcome::Empty,
            syntactic: FakeDiagnosticOutcome::Empty,
            completions: FakeCompletionOutcome::Null,
            update_failure: None,
            type_gate: None,
        }
    }
}

fn documented_behavior() -> FakeBehavior {
    FakeBehavior {
        docs_outcome: FakeDocsOutcome::Documented(DocumentationQuery {
            comment: Some(COMMENT.to_owned()),
            tags: vec![RawJsDocTag {
                name: "deprecated".to_owned(),
                text: Some("use title".to_owned()),
            }],
            comment_failed: false,
            tags_failed: false,
        }),
        ..FakeBehavior::default()
    }
}

#[derive(Debug, Default)]
struct FakeLog {
    starts: usize,
    opens: usize,
    updates: usize,
    shutdowns: usize,
    semantic_calls: usize,
    syntactic_calls: usize,
    type_calls: Vec<(String, Utf16Position)>,
    completion_calls: Vec<(String, Utf16Position)>,
    opened_inputs: Vec<AnalysisInput>,
    updated_inputs: Vec<AnalysisInput>,
}

#[derive(Clone)]
struct FakeFactory {
    behavior: FakeBehavior,
    log: Arc<Mutex<FakeLog>>,
}

impl FakeFactory {
    fn new(behavior: FakeBehavior) -> (Self, Arc<Mutex<FakeLog>>) {
        let log = Arc::new(Mutex::new(FakeLog::default()));
        (
            Self {
                behavior,
                log: Arc::clone(&log),
            },
            log,
        )
    }
}

impl super::session::SessionFactory for FakeFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        self.log.lock().expect("log").starts += 1;
        Ok(Box::new(FakeSession {
            behavior: self.behavior.clone(),
            log: Arc::clone(&self.log),
            input: None,
        }))
    }
}

struct FakeSession {
    behavior: FakeBehavior,
    log: Arc<Mutex<FakeLog>>,
    input: Option<AnalysisInput>,
}

impl FakeSession {
    fn text(&self, path: &str) -> Option<&str> {
        self.input.as_ref()?.file(path).map(InputSourceFile::text)
    }

    fn raw_diagnostics(
        &self,
        path: &str,
        outcome: &FakeDiagnosticOutcome,
    ) -> Result<Vec<RawDiagnostic>, NativeError> {
        match outcome {
            FakeDiagnosticOutcome::Empty => Ok(Vec::new()),
            FakeDiagnosticOutcome::Failure(failure) => Err(failure.native()),
            FakeDiagnosticOutcome::Diagnostics(specs) => {
                let Some(text) = self.text(path) else {
                    return Ok(Vec::new());
                };
                Ok(specs
                    .iter()
                    .filter_map(|spec| {
                        let start = text.find(spec.needle)?;
                        let end = start.checked_add(spec.needle.len())?;
                        let start_units = utf16_len(text.get(..start)?);
                        let end_units = utf16_len(text.get(..end)?);
                        let start_position =
                            offset_to_position(text, Utf16Offset::new(start_units)).ok()?;
                        let end_position =
                            offset_to_position(text, Utf16Offset::new(end_units)).ok()?;
                        Some(RawDiagnostic {
                            code: spec.code,
                            category: spec.category,
                            pos: Some(i64::from(start_units)),
                            end: Some(i64::from(end_units)),
                            start_position: Some(RawPosition {
                                line: i64::from(start_position.line),
                                character: i64::from(start_position.character),
                            }),
                            end_position: Some(RawPosition {
                                line: i64::from(end_position.line),
                                character: i64::from(end_position.character),
                            }),
                            text: spec.message.to_owned(),
                        })
                    })
                    .collect())
            }
        }
    }
}

impl QuerySession for FakeSession {
    fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        let mut log = self.log.lock().expect("log");
        log.opens += 1;
        log.opened_inputs.push(input.clone());
        drop(log);
        self.input = Some(input.clone());
        Ok(())
    }

    fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        let mut log = self.log.lock().expect("log");
        log.updates += 1;
        log.updated_inputs.push(input.clone());
        drop(log);
        if let Some(failure) = &self.behavior.update_failure {
            return Err(failure.native());
        }
        self.input = Some(input.clone());
        Ok(())
    }

    fn semantic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.log.lock().expect("log").semantic_calls += 1;
        self.raw_diagnostics(path, &self.behavior.semantic)
    }

    fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        self.log.lock().expect("log").syntactic_calls += 1;
        self.raw_diagnostics(path, &self.behavior.syntactic)
    }

    fn type_query(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<TypeQuery, NativeError> {
        self.log
            .lock()
            .expect("log")
            .type_calls
            .push((path.to_owned(), position));
        if let Some(gate) = &self.behavior.type_gate {
            gate.enter_and_wait();
        }
        match &self.behavior.type_outcome {
            FakeTypeOutcome::Null => Ok(TypeQuery {
                info: None,
                text: None,
            }),
            FakeTypeOutcome::Known(text) | FakeTypeOutcome::Any(text) => Ok(TypeQuery {
                info: Some(RawType {
                    id: 1,
                    flags: 0,
                    intrinsic_name: (text == "any").then(|| "any".to_owned()),
                }),
                text: Some(text.clone()),
            }),
            FakeTypeOutcome::Failure(failure) => Err(failure.native()),
        }
    }

    fn hover_documentation(
        &mut self,
        _path: &str,
        _position: Utf16Position,
    ) -> Result<HoverDocumentation, NativeError> {
        match &self.behavior.docs_outcome {
            FakeDocsOutcome::NoSymbol => Ok(HoverDocumentation::no_symbol()),
            FakeDocsOutcome::Documented(query) => Ok(HoverDocumentation {
                symbol: true,
                result: map_documentation(query),
            }),
            FakeDocsOutcome::Failure(failure) => Err(failure.native()),
        }
    }

    fn completions(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        self.log
            .lock()
            .expect("log")
            .completion_calls
            .push((path.to_owned(), position));
        match &self.behavior.completions {
            FakeCompletionOutcome::Null => Ok(None),
            FakeCompletionOutcome::List {
                entries,
                incomplete,
            } => Ok(Some(RawCompletionList {
                is_incomplete: *incomplete,
                entries: entries.clone(),
            })),
            FakeCompletionOutcome::Failure(failure) => Err(failure.native()),
        }
    }

    fn shutdown(self: Box<Self>) {
        self.log.lock().expect("log").shutdowns += 1;
    }
}

fn adapter_with(
    views: Arc<dyn ViewProvider>,
    behavior: FakeBehavior,
) -> (TsgoBackendAdapter, Arc<Mutex<FakeLog>>) {
    let (factory, log) = FakeFactory::new(behavior);
    let adapter = TsgoBackendAdapter::new(PinnedEngine::pinned(), views, Arc::new(factory));
    (adapter, log)
}

fn context(adapter: &TsgoBackendAdapter, wxml: &str) -> RequestContext {
    context_with_epoch(adapter, wxml, adapter.epoch())
}

fn context_with_epoch(
    adapter: &TsgoBackendAdapter,
    wxml: &str,
    epoch: BackendEpoch,
) -> RequestContext {
    let _ = adapter;
    RequestContext::new(
        ProjectRevision::new(project(), 1),
        ViewRevision::new(1),
        epoch,
        Arc::new(SourceSnapshot::new(
            wxml_file(),
            SourceRevision::new(1),
            SnapshotId::new(1),
            wxml,
        )),
    )
    .expect("context")
}

fn offset_of(wxml: &str, needle: &str) -> u32 {
    u32::try_from(wxml.find(needle).expect("needle in template")).expect("offset")
}

#[allow(clippy::significant_drop_tightening)]
fn derived_position(log: &Arc<Mutex<FakeLog>>, needle: &str, token: Option<&str>) -> Utf16Position {
    let log = log.lock().expect("log");
    let input = log.opened_inputs.first().expect("opened input");
    let text = input.file(SCRIPT_PATH).expect("shadow script").text();
    let base = text.find(needle).expect("derived needle");
    let start = token.map_or_else(
        || base + needle.len(),
        |token| base + needle.find(token).expect("token in needle"),
    );
    offset_to_position(text, Utf16Offset::new(utf16_len(&text[..start]))).expect("position")
}

#[test]
fn hover_composes_type_documentation_and_jsdoc() {
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        documented_behavior(),
    );
    let context = context(&adapter, WXML);
    let result = adapter
        .hover(HoverRequest {
            context,
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert!(result.contents.contains("```ts\nstring\n```"));
    assert!(result.contents.contains(COMMENT));
    assert!(result.contents.contains("@deprecated use title"));
    let range = result.range.expect("hover range");
    assert_eq!(
        range.range(),
        wxml_text::TextRange::new(offset_of(WXML, "name"), offset_of(WXML, "name") + 4)
            .expect("range")
    );
}

#[test]
fn hover_any_preserves_quality_but_not_a_concrete_type() {
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Any("any".to_owned()),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert_eq!(result.quality, wxml_types::TypeQuality::Any);
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert!(result.contents.contains("any"));
}

#[test]
fn hover_without_type_or_documentation_is_unsupported_not_empty_success() {
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Null,
        docs_outcome: FakeDocsOutcome::NoSymbol,
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert_eq!(result.contents, "");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(result.quality, wxml_types::TypeQuality::Missing);
}

#[test]
fn hover_documentation_failure_is_partial_not_fake_known_docs() {
    let behavior = documented_behavior();
    let behavior = FakeBehavior {
        docs_outcome: FakeDocsOutcome::Documented(DocumentationQuery {
            comment: None,
            tags: Vec::new(),
            comment_failed: true,
            tags_failed: false,
        }),
        ..behavior
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert!(result.contents.contains("string"));
    assert!(!result.contents.contains(COMMENT));
}

#[test]
fn an_unsupported_documentation_query_is_partial_type_only() {
    let behavior = FakeBehavior {
        docs_outcome: FakeDocsOutcome::Failure(FakeFailure::Unsupported("documentation")),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert!(result.contents.contains("string"));
}

#[test]
fn a_fatal_documentation_query_drops_the_session_and_returns_the_error() {
    let behavior = FakeBehavior {
        docs_outcome: FakeDocsOutcome::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let before = adapter.epoch();
    let error = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("fatal documentation failure");
    assert!(matches!(error, BackendError::Timeout));
    assert_eq!(log.lock().expect("log").shutdowns, 1);
    assert!(adapter.epoch().get() > before.get());
}

#[test]
fn hover_degrades_when_the_prepared_source_list_is_incomplete() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert!(result.contents.contains("string"));
}

#[test]
fn hover_rejects_a_non_character_boundary_offset() {
    let wxml = "<view>{{ 用户.名字 }}</view>";
    let script = "Page({ data: { 用户: { 名字: \"阿\" } } });\n";
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, script, "用户.名字"))),
        FakeBehavior::default(),
    );
    let inside_name = offset_of(wxml, "用户.名字") + 3 + 1 + 1;
    let error = adapter
        .hover(HoverRequest {
            context: context(&adapter, wxml),
            offset: inside_name,
        })
        .expect_err("non-boundary offset");
    assert!(matches!(error, BackendError::Protocol(_)));
    assert_eq!(log.lock().expect("log").type_calls.len(), 0);
}

#[test]
fn diagnostics_map_semantic_and_syntactic_ranges() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "user.name",
            code: 2339,
            category: 1,
            message: "Property 'name' does not exist.",
        }]),
        syntactic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "user.name",
            code: 1109,
            category: 1,
            message: "syntax",
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 2);
    let codes: Vec<&str> = result
        .diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.code.as_deref())
        .collect();
    assert!(codes.contains(&"2339"));
    assert!(codes.contains(&"1109"));
    for diagnostic in &result.diagnostics {
        assert_eq!(diagnostic.range.start(), offset_of(WXML, "user.name"));
        assert_eq!(diagnostic.range.end(), offset_of(WXML, "user.name") + 9);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    }
}

#[test]
fn a_fatal_diagnostic_source_returns_unavailable_without_err_or_epoch_bump() {
    let behavior = FakeBehavior {
        syntactic: FakeDiagnosticOutcome::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let before = adapter.epoch();
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics must not return Err");
    assert_eq!(result.diagnostics, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(adapter.epoch(), before);
    assert_eq!(log.lock().expect("log").shutdowns, 1);
}

#[test]
fn both_fatal_diagnostic_sources_return_unavailable_without_err() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Failure(FakeFailure::Timeout),
        syntactic: FakeDiagnosticOutcome::Failure(FakeFailure::Exited),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics must not return Err");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
}

#[test]
fn diagnostics_with_an_incomplete_source_list_are_partial() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn a_generated_only_diagnostic_range_is_partial_and_dropped() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "__wxml_check_0",
            code: 1000,
            category: 1,
            message: "generated probe error",
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn completion_maps_entries_and_replaces_only_the_typed_prefix() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![
                RawCompletionEntry {
                    name: "name".to_owned(),
                    kind: 5,
                    sort_text: "11".to_owned(),
                },
                RawCompletionEntry {
                    name: "age".to_owned(),
                    kind: 5,
                    sort_text: "12".to_owned(),
                },
            ],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );
    let cursor = offset_of(wxml, "na") + 2;
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(result.items.len(), 2);
    assert!(!result.truncated);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    for item in &result.items {
        assert_eq!(item.replace.start(), offset_of(wxml, "na"));
        assert_eq!(item.replace.end(), cursor);
        assert_eq!(item.insert_text, item.label);
        assert_eq!(
            item.insert_text_format,
            wxml_types::InsertTextFormat::PlainText
        );
    }
}

#[test]
fn completion_truncation_is_reported() {
    let entries: Vec<RawCompletionEntry> = (0..=wxml_types::MAX_COMPLETION_ITEMS)
        .map(|index| RawCompletionEntry {
            name: format!("item{index}"),
            kind: 5,
            sort_text: "1".to_owned(),
        })
        .collect();
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries,
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let wxml = "<view>{{ user.na }}</view>";
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert!(result.truncated);
    assert_eq!(result.items.len(), wxml_types::MAX_COMPLETION_ITEMS);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn a_failed_completion_query_is_an_error_not_a_supported_empty_list() {
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let wxml = "<view>{{ user.na }}</view>";
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );
    let error = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect_err("completion must fail");
    assert!(matches!(error, BackendError::Timeout));
}

#[test]
fn an_unsupported_native_completion_is_explicitly_unsupported() {
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::Failure(FakeFailure::Unsupported("completion")),
        ..FakeBehavior::default()
    };
    let wxml = "<view>{{ user.na }}</view>";
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(result.items, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
}

#[test]
fn a_non_expression_completion_kind_is_explicitly_unsupported() {
    let wxml = "<view>{{ user.na }}</view>";
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::Tag,
        })
        .expect("completion");
    assert_eq!(result.items, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn definition_and_symbols_are_explicitly_unsupported() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let context = context(&adapter, WXML);
    let definition = adapter
        .definition(DefinitionRequest {
            context: context.clone(),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(definition.locations, Vec::new());
    assert!(matches!(
        definition.status,
        CapabilityStatus::Unsupported { .. }
    ));
    let symbols = adapter
        .symbols(SymbolRequest {
            context,
            subject: script_file(),
            query: SymbolQuery::DataRoots,
        })
        .expect("symbols");
    assert!(!symbols.complete);
    assert!(matches!(
        symbols.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn capabilities_advertise_only_implemented_features() {
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let capabilities = adapter.capabilities();
    assert!(matches!(
        capabilities.hover,
        CapabilityStatus::Partial { .. }
    ));
    assert!(matches!(
        capabilities.diagnostics,
        CapabilityStatus::Partial { .. }
    ));
    assert!(matches!(
        capabilities.definition,
        CapabilityStatus::Unsupported { .. }
    ));
    assert!(matches!(
        capabilities.completion,
        CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn invalidate_drops_the_session_and_advances_epoch() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let before = adapter.epoch();
    adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert_eq!(log.lock().expect("log").starts, 1);
    let after_hover = adapter.epoch();
    assert_eq!(
        after_hover, before,
        "a successful lazy start must not invalidate the in-flight stamp"
    );
    adapter.invalidate();
    let after_invalidate = adapter.epoch();
    assert!(after_invalidate.get() > after_hover.get());
    assert_eq!(log.lock().expect("log").starts, 1);
    adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover after invalidation");
    assert_eq!(log.lock().expect("log").starts, 2);
}

#[test]
fn a_new_template_revision_updates_the_native_snapshot() {
    let wxml_v1 = "<view>{{ user.name }}</view>";
    let wxml_v2 = "<view>{{ user.name }}!</view>";
    let view = prepared_view(wxml_v1, SCRIPT, "user.name");
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    adapter
        .hover(HoverRequest {
            context: context(&adapter, wxml_v1),
            offset: offset_of(wxml_v1, "name"),
        })
        .expect("hover v1");
    let epoch_after_first = adapter.epoch();
    let second = RequestContext::new(
        ProjectRevision::new(project(), 1),
        ViewRevision::new(2),
        adapter.epoch(),
        Arc::new(SourceSnapshot::new(
            wxml_file(),
            SourceRevision::new(2),
            SnapshotId::new(2),
            wxml_v2,
        )),
    )
    .expect("context v2");
    adapter
        .hover(HoverRequest {
            context: second,
            offset: offset_of(wxml_v2, "name"),
        })
        .expect("hover v2");
    let (starts, opens, updates, update_count, shadow_uses_root) = {
        let log = log.lock().expect("log");
        let shadow_uses_root = log
            .opened_inputs
            .first()
            .and_then(|input| input.file(SCRIPT_PATH))
            .is_some_and(|file| file.text().contains("__wxml_root"));
        (
            log.starts,
            log.opens,
            log.updates,
            log.updated_inputs.len(),
            shadow_uses_root,
        )
    };
    assert_eq!(starts, 1);
    assert_eq!(opens, 1);
    assert_eq!(updates, 1);
    assert_eq!(update_count, 1);
    assert!(shadow_uses_root);
    assert_eq!(
        adapter.epoch(),
        epoch_after_first,
        "a snapshot update must not invalidate an in-flight request stamp"
    );
}

#[test]
fn a_missing_project_view_is_unavailable_and_starts_no_session() {
    let (factory, log) = FakeFactory::new(FakeBehavior::default());
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(FailingViews),
        Arc::new(factory),
    );
    let error = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("missing view");
    assert!(matches!(error, BackendError::Unavailable(_)));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn a_missing_target_script_is_unavailable() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.files = Arc::from(Vec::new());
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let error = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("missing script");
    assert!(matches!(error, BackendError::Unavailable(_)));
}

#[test]
fn an_unsupported_native_type_query_is_explicitly_unsupported() {
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Failure(FakeFailure::Unsupported("typeToString")),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
}

#[test]
fn fake_session_sees_exact_derived_positions() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    let expected = derived_position(&log, "__wxml_root.user.name", Some("name"));
    let calls = &log.lock().expect("log").type_calls;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, SCRIPT_PATH);
    assert_eq!(calls[0].1, expected);

    let cjk_wxml = "<view>{{ 用户.名字 }}</view>";
    let cjk_script = "Page({ data: { 用户: { 名字: \"阿\" } } });\n";
    let (cjk_adapter, cjk_log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            cjk_wxml,
            cjk_script,
            "用户.名字",
        ))),
        FakeBehavior::default(),
    );
    cjk_adapter
        .hover(HoverRequest {
            context: context(&cjk_adapter, cjk_wxml),
            offset: offset_of(cjk_wxml, "名字"),
        })
        .expect("cjk hover");
    let expected = derived_position(&cjk_log, "__wxml_root.用户.名字", Some("名字"));
    let calls = &cjk_log.lock().expect("log").type_calls;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, expected);

    let completion_wxml = "<view>{{ user.na }}</view>";
    let (completion_adapter, completion_log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            completion_wxml,
            SCRIPT,
            "user.na",
        ))),
        FakeBehavior::default(),
    );
    completion_adapter
        .completion(CompletionRequest {
            context: context(&completion_adapter, completion_wxml),
            offset: offset_of(completion_wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    let expected = derived_position(&completion_log, "__wxml_root.user.na", None);
    let calls = &completion_log.lock().expect("log").completion_calls;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, SCRIPT_PATH);
    assert_eq!(calls[0].1, expected);
}

#[test]
fn hover_at_the_fragment_end_still_maps_to_the_last_identifier() {
    let end = offset_of(WXML, "user.name") + u32::try_from("user.name".len()).expect("length");
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: end,
        })
        .expect("hover at fragment end");
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert!(result.contents.contains("string"));
    assert_eq!(
        result.range.expect("range").start(),
        offset_of(WXML, "name")
    );
}

#[test]
fn host_syntax_diagnostics_are_merged_into_native_results() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.syntax_diagnostics = vec![syntax_diagnostic(WXML, "user", "unclosed tag")];
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "unclosed tag");
    assert!(!(result.status.is_supported() && result.diagnostics.is_empty()));
}

#[test]
fn host_syntax_diagnostics_survive_a_fatal_native_failure() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.syntax_diagnostics = vec![syntax_diagnostic(WXML, "user", "unclosed tag")];
    let behavior = FakeBehavior {
        syntactic: FakeDiagnosticOutcome::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics must not return Err");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "unclosed tag");
}

#[test]
fn a_malformed_template_with_a_view_but_no_host_syntax_is_still_not_clean() {
    static MALFORMED: &str = "<view>{{ user.name </view>";
    let view = empty_view(MALFORMED, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, MALFORMED),
        })
        .expect("diagnostics");
    assert!(
        !(result.status.is_supported() && result.diagnostics.is_empty()),
        "malformed template was reported clean"
    );
    assert_ne!(result.diagnostics, Vec::new());
}

#[test]
fn a_malformed_template_without_a_view_is_unavailable_and_not_clean() {
    static MALFORMED: &str = "<view>{{ user.name </view>";
    let (factory, _log) = FakeFactory::new(FakeBehavior::default());
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(FailingViews),
        Arc::new(factory),
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, MALFORMED),
        })
        .expect("diagnostics must not return Err");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_ne!(result.diagnostics, Vec::new());
}

#[test]
fn literal_and_global_only_fragments_do_not_abort_the_shadow() {
    let wxml = "<view>{{ false }}{{ Math.PI }}{{ 1 + 2 }}{{ user.name }}</view>";
    let fragments = vec![
        fragment(wxml, "false", "f0"),
        fragment(wxml, "Math.PI", "f1"),
        fragment(wxml, "1 + 2", "f2"),
        fragment(wxml, "user.name", "f3"),
    ];
    let view =
        prepared_view_with_fragments(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)], fragments);
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let hover = adapter
        .hover(HoverRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "name"),
        })
        .expect("hover must survive literal fragments");
    assert_eq!(hover.quality, wxml_types::TypeQuality::Known);
    assert!(hover.contents.contains("string"));
    assert!(matches!(hover.status, CapabilityStatus::Partial { .. }));
    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("diagnostics");
    assert_eq!(diagnostics.diagnostics, Vec::new());
    assert!(matches!(
        diagnostics.status,
        CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn a_literal_only_fragment_is_unsupported_not_a_shadow_failure() {
    let wxml = "<view>{{ false }}</view>";
    let view = prepared_view_with_fragments(
        wxml,
        vec![prepared_file(SCRIPT_PATH, SCRIPT)],
        vec![fragment(wxml, "false", "f0")],
    );
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "false"),
        })
        .expect("hover must not fail the whole shadow");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
}

#[test]
fn completion_survives_literal_fragments() {
    let wxml = "<view>{{ false }}{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "name".to_owned(),
                kind: 5,
                sort_text: "11".to_owned(),
            }],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let view = prepared_view_with_fragments(
        wxml,
        vec![prepared_file(SCRIPT_PATH, SCRIPT)],
        vec![
            fragment(wxml, "false", "f0"),
            fragment(wxml, "user.na", "f1"),
        ],
    );
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(result.items.len(), 1);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn completion_in_an_unclosed_expression_is_explicitly_unsupported() {
    let wxml = "<view>{{ user.</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "user.") + u32::try_from("user.".len()).expect("length"),
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion must not be a silent empty success");
    assert_eq!(result.items, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert!(
        result
            .status
            .reason()
            .is_some_and(|reason| reason.contains("incomplete")),
        "{:?}",
        result.status
    );
    assert_eq!(log.lock().expect("log").completion_calls.len(), 0);
}

#[test]
fn stale_request_epochs_are_cancelled() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let stale = BackendEpoch::new(
        BackendId::new(BACKEND_ID),
        adapter.epoch().get().saturating_add(1),
    );
    let context = context_with_epoch(&adapter, WXML, stale);
    let hover = adapter
        .hover(HoverRequest {
            context: context.clone(),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("stale hover");
    assert!(matches!(hover, BackendError::Cancelled));
    let completion = adapter
        .completion(CompletionRequest {
            context: context.clone(),
            offset: offset_of(WXML, "name"),
            kind: CompletionKind::ExpressionMember,
        })
        .expect_err("stale completion");
    assert!(matches!(completion, BackendError::Cancelled));
    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest { context })
        .expect_err("stale diagnostics");
    assert!(matches!(diagnostics, BackendError::Cancelled));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn invalidate_does_not_block_on_a_slow_query() {
    let gate = Arc::new(SlowGate::default());
    let behavior = FakeBehavior {
        type_gate: Some(Arc::clone(&gate)),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let adapter = Arc::new(adapter);
    let first_context = context(adapter.as_ref(), WXML);
    let worker = Arc::clone(&adapter);
    let handle = thread::spawn(move || {
        worker.hover(HoverRequest {
            context: first_context,
            offset: offset_of(WXML, "name"),
        })
    });
    gate.wait_until_entered();
    let started = Instant::now();
    adapter.invalidate();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "invalidate blocked for {elapsed:?}"
    );
    gate.release();
    let result = handle.join().expect("hover thread");
    assert!(matches!(result, Err(BackendError::Cancelled)));
    assert_eq!(log.lock().expect("log").shutdowns, 1);

    adapter
        .hover(HoverRequest {
            context: context(adapter.as_ref(), WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover after invalidation");
    assert_eq!(log.lock().expect("log").starts, 2);
}

#[test]
fn a_failed_snapshot_update_reopens_a_fresh_session() {
    let behavior = FakeBehavior {
        update_failure: Some(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("first hover");
    let epoch = adapter.epoch();
    let second = RequestContext::new(
        ProjectRevision::new(project(), 1),
        ViewRevision::new(2),
        adapter.epoch(),
        Arc::new(SourceSnapshot::new(
            wxml_file(),
            SourceRevision::new(2),
            SnapshotId::new(2),
            WXML,
        )),
    )
    .expect("second context");
    adapter
        .hover(HoverRequest {
            context: second,
            offset: offset_of(WXML, "name"),
        })
        .expect("hover after update failure");
    let (starts, opens, updates, shutdowns) = {
        let log = log.lock().expect("log");
        (log.starts, log.opens, log.updates, log.shutdowns)
    };
    assert_eq!(starts, 2);
    assert_eq!(opens, 2);
    assert_eq!(updates, 1);
    assert_eq!(shutdowns, 1);
    assert_eq!(adapter.epoch(), epoch);
}

#[test]
fn an_unknown_file_path_is_a_typed_error_not_a_clean_list() {
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::Failure(FakeFailure::InvalidParams),
        type_outcome: FakeTypeOutcome::Failure(FakeFailure::InvalidParams),
        ..FakeBehavior::default()
    };
    let wxml = "<view>{{ user.na }}</view>";
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );
    let completion = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect_err("unknown path");
    assert!(matches!(completion, BackendError::Protocol(_)));
    let hover = adapter
        .hover(HoverRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na"),
        })
        .expect_err("unknown path hover");
    assert!(matches!(hover, BackendError::Protocol(_)));
}

#[test]
fn a_script_owned_diagnostic_is_dropped_from_a_template_result() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "data: { user",
            code: 999,
            category: 1,
            message: "script error",
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    assert_eq!(result.status, CapabilityStatus::Supported);
}

#[test]
fn optional_real_engine_hover_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine adapter test skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox =
        std::env::temp_dir().join(format!("wxml-tsgo-adapter-{}-{count}", std::process::id()));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(
            "globals.d.ts",
            "declare function Page(options: {\n  data: { user: { name: string } };\n  __wxml_check_0(this: { data: { user: { name: string } } }): void;\n}): void;\n",
        ),
    ];
    let views = Arc::new(FakeViews::new(prepared_view_with_files(
        WXML,
        "user.name",
        files,
    )));
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        views,
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("real engine hover");
    eprintln!(
        "real engine adapter hover: quality={:?} status={:?} contents={:?}",
        result.quality, result.status, result.contents
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert!(
        result.status.is_supported() || matches!(result.status, CapabilityStatus::Partial { .. }),
        "real engine hover must not be silently unsupported: {:?}",
        result.status
    );
    let range = result.range.expect("real engine hover range");
    assert_eq!(range.start(), offset_of(WXML, "name"));
    assert_eq!(
        range.end(),
        offset_of(WXML, "name") + u32::try_from("name".len()).expect("length")
    );
    assert!(result.contents.contains("string"));
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
