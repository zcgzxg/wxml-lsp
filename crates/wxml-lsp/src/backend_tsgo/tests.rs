//! Adapter-level tests with a fake host view and fake native session.
//!
//! These tests exercise the real `wxml_types::Backend` implementation, shadow
//! construction and coordinate mapping; they do not require a native binary.
//! The real-engine smoke test at the end is opt-in through `TSGO_BIN`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::too_many_lines
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use wxml_backend_tsgo::config::PinnedEngine;
use wxml_backend_tsgo::input::{AnalysisInput, SourceFile as InputSourceFile};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::UnavailableReason;
use wxml_backend_tsgo::native::ast::{
    CLASS_DECLARATION_KIND, FUNCTION_DECLARATION_KIND, METHOD_DECLARATION_KIND,
    PROPERTY_DECLARATION_KIND, TYPE_ALIAS_DECLARATION_KIND, VARIABLE_DECLARATION_KIND,
};
use wxml_backend_tsgo::native::document_symbol::{
    DocumentSymbolEvidence, DocumentSymbolGap as NativeDocumentSymbolGap, DocumentSymbolsEvidence,
};
use wxml_backend_tsgo::native::session::DocumentationQuery;
use wxml_backend_tsgo::native::session::{SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::{
    Utf8Offset, Utf16Offset, Utf16Position, offset_to_position, utf16_len,
};
use wxml_backend_tsgo::native::wire::{
    RawCompletionEntry, RawCompletionLabelDetails, RawCompletionList, RawDiagnostic, RawJsDocTag,
    RawPosition, RawType,
};
use wxml_backend_tsgo::query::definition::{
    DefinitionResult as NativeDefinitionResult, DefinitionTarget as NativeDefinitionTarget,
};
use wxml_backend_tsgo::query::documentation::{
    DocumentationResult as NativeDocumentationResult, map_documentation,
};
use wxml_backend_tsgo::query::hover::{HoverEvidence, HoverGap};
use wxml_backend_tsgo::query::type_info::{
    TypeInfo as NativeTypeInfo, TypeQuality as NativeTypeQuality, map_type_query,
};
use wxml_backend_tsgo::script::symbols::Utf16Range;
use wxml_backend_tsgo::shadow::mapping::SourceRange as NativeSourceRange;
use wxml_project::ProjectPath;
use wxml_text::SourceRange;
use wxml_types::{
    Backend, BackendEpoch, BackendError, BackendId, CapabilityStatus, CheckFragment,
    CompletionItemKind, CompletionKind, CompletionRequest, CompletionResult, DefinitionRequest,
    Diagnostic, DiagnosticSeverity, DiagnosticsRequest, DocumentSymbolKind, DocumentSymbolsRequest,
    FileId, HoverRequest, MarkupKind, ProjectId, ProjectRevision, RequestContext, RootKind,
    SnapshotId, SourceRevision, SourceSnapshot, SymbolQuery, SymbolRequest, ViewRevision,
};

use super::hover::resolved_status;
use super::input::ViewProvider;
use super::session::{HoverDocumentation, QuerySession};
use super::{BACKEND_ID, TsgoBackendAdapter};
use crate::project_host::{HostError, PreparedFile, PreparedView, ViewTarget};

const WXML: &str = "<view>{{ user.name }}</view>";
const SCRIPT: &str = "Page({\n  data: { user: { name: \"Ada\", age: 42 } },\n});\n";
const UNICODE_SCRIPT: &str =
    "// 😀 中文\nconst prefix = 1;\nPage({\n  data: { user: { name: \"Ada\", age: 42 } },\n});\n";
const SCRIPT_PATH: &str = "pages/index/index.ts";
const WXML_PATH: &str = "pages/index/index.wxml";
const MODEL_PATH: &str = "model.ts";
const MODEL: &str = "// 😀 模型\nexport interface User {\n  name: string;\n}\n";
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
enum FakeHoverOutcome {
    /// Derives evidence from `type_outcome` and `docs_outcome` the way the
    /// native session composes its facts.
    Derived,
    /// Returns hand-built evidence, including backend contract violations.
    Evidence(FakeHoverEvidence),
    Failure(FakeFailure),
}

#[derive(Clone, Debug)]
struct FakeHoverEvidence {
    type_text: Option<&'static str>,
    quality: NativeTypeQuality,
    documentation: Option<DocumentationQuery>,
    gaps: Vec<HoverGap>,
    status: Option<CapabilityStatus>,
}

impl FakeHoverEvidence {
    fn evidence(&self) -> HoverEvidence {
        let mut evidence = HoverEvidence {
            type_info: self.type_text.map(|text| NativeTypeInfo {
                text: text.to_owned(),
                handle: 1,
                flags: 0,
            }),
            type_quality: self.quality,
            symbol: None,
            documentation: self.documentation.as_ref().map(map_documentation),
            declarations: Vec::new(),
            status: self.status.clone().unwrap_or(CapabilityStatus::Supported),
            gaps: self.gaps.clone(),
        };
        if self.status.is_none() {
            evidence.finalize_status();
        }
        evidence
    }
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
    NoResponse,
    EmptyList,
    List {
        entries: Vec<RawCompletionEntry>,
        incomplete: bool,
    },
    Failure(FakeFailure),
}

#[derive(Clone, Debug)]
enum FakeDefinitionOutcome {
    CheckedEmpty,
    Targets {
        targets: Vec<NativeDefinitionTarget>,
        status: CapabilityStatus,
        truncated: bool,
    },
    TargetInScript {
        needle: &'static str,
    },
    Failure(FakeFailure),
}

/// One fake document-symbol declaration, resolved against the session text.
#[derive(Clone, Debug)]
struct FakeSymbolSpec {
    node_kind: u32,
    name: &'static str,
    declaration: &'static str,
    declaration_end: Option<&'static str>,
    selection: &'static str,
    children: Vec<Self>,
}

impl FakeSymbolSpec {
    fn new(node_kind: u32, name: &'static str, declaration: &'static str) -> Self {
        Self {
            node_kind,
            name,
            declaration,
            declaration_end: None,
            selection: name,
            children: Vec::new(),
        }
    }

    fn with_children(
        node_kind: u32,
        name: &'static str,
        declaration: &'static str,
        children: Vec<Self>,
    ) -> Self {
        Self {
            node_kind,
            name,
            declaration,
            declaration_end: None,
            selection: name,
            children,
        }
    }
}

/// One fake skipped declaration candidate, resolved against the session text.
#[derive(Clone, Debug)]
struct FakeGapSpec {
    start: &'static str,
    reason: &'static str,
}

#[derive(Clone, Debug)]
enum FakeDocumentSymbolOutcome {
    CheckedEmpty,
    Script {
        symbols: Vec<FakeSymbolSpec>,
        gaps: Vec<FakeGapSpec>,
        skipped: usize,
        truncated: bool,
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
    hover_outcome: FakeHoverOutcome,
    type_outcome: FakeTypeOutcome,
    docs_outcome: FakeDocsOutcome,
    semantic: FakeDiagnosticOutcome,
    syntactic: FakeDiagnosticOutcome,
    completions: FakeCompletionOutcome,
    definition: FakeDefinitionOutcome,
    document_symbols: FakeDocumentSymbolOutcome,
    update_failure: Option<FakeFailure>,
    type_gate: Option<Arc<SlowGate>>,
    definition_gate: Option<Arc<SlowGate>>,
}

impl Default for FakeBehavior {
    fn default() -> Self {
        Self {
            hover_outcome: FakeHoverOutcome::Derived,
            type_outcome: FakeTypeOutcome::Known("string".to_owned()),
            docs_outcome: FakeDocsOutcome::NoSymbol,
            semantic: FakeDiagnosticOutcome::Empty,
            syntactic: FakeDiagnosticOutcome::Empty,
            completions: FakeCompletionOutcome::EmptyList,
            definition: FakeDefinitionOutcome::CheckedEmpty,
            document_symbols: FakeDocumentSymbolOutcome::CheckedEmpty,
            update_failure: None,
            type_gate: None,
            definition_gate: None,
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
    hover_evidence_calls: Vec<(String, Utf16Position)>,
    hover_documentation_calls: usize,
    completion_calls: Vec<(String, Utf16Position)>,
    definition_calls: Vec<(String, Utf16Position)>,
    document_symbol_calls: Vec<String>,
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

    fn type_query_value(&self) -> Result<TypeQuery, NativeError> {
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

    fn hover_documentation_value(&self) -> Result<HoverDocumentation, NativeError> {
        match &self.behavior.docs_outcome {
            FakeDocsOutcome::NoSymbol => Ok(HoverDocumentation::no_symbol()),
            FakeDocsOutcome::Documented(query) => Ok(HoverDocumentation {
                symbol: true,
                result: map_documentation(query),
            }),
            FakeDocsOutcome::Failure(failure) => Err(failure.native()),
        }
    }

    /// Mirrors the native session's composed evidence for the fake facts.
    ///
    /// An unsupported documentation query stays a documentation gap instead of
    /// failing the whole hover, matching the backend contract that
    /// documentation gaps live in the evidence status.
    fn derived_hover_evidence(&self) -> Result<HoverEvidence, NativeError> {
        let type_result = map_type_query(&self.type_query_value()?);
        let documentation = match self.hover_documentation_value() {
            Ok(documentation) => documentation,
            Err(NativeError::Unsupported { .. }) => HoverDocumentation {
                symbol: true,
                result: NativeDocumentationResult {
                    comment: None,
                    tags: Vec::new(),
                    status: CapabilityStatus::Unsupported {
                        reason: "native documentation queries are unavailable".to_owned(),
                    },
                },
            },
            Err(error) => return Err(error),
        };
        let mut gaps = Vec::new();
        if let Some(reason) = type_result.status.reason() {
            gaps.push(HoverGap::Unsupported {
                reason: format!("type query is incomplete: {reason}"),
            });
        }
        let mut evidence = HoverEvidence {
            type_info: type_result.info,
            type_quality: type_result.quality,
            symbol: None,
            documentation: documentation.symbol.then_some(documentation.result),
            declarations: Vec::new(),
            status: CapabilityStatus::Supported,
            gaps,
        };
        evidence.finalize_status();
        Ok(evidence)
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
        self.type_query_value()
    }

    fn hover_documentation(
        &mut self,
        _path: &str,
        _position: Utf16Position,
    ) -> Result<HoverDocumentation, NativeError> {
        self.log.lock().expect("log").hover_documentation_calls += 1;
        self.hover_documentation_value()
    }

    fn hover_evidence(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError> {
        self.log
            .lock()
            .expect("log")
            .hover_evidence_calls
            .push((path.to_owned(), position));
        if let Some(gate) = &self.behavior.type_gate {
            gate.enter_and_wait();
        }
        match &self.behavior.hover_outcome {
            FakeHoverOutcome::Derived => self.derived_hover_evidence(),
            FakeHoverOutcome::Evidence(evidence) => Ok(evidence.evidence()),
            FakeHoverOutcome::Failure(failure) => Err(failure.native()),
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
            FakeCompletionOutcome::NoResponse => Ok(None),
            FakeCompletionOutcome::EmptyList => Ok(Some(RawCompletionList {
                is_incomplete: false,
                entries: Vec::new(),
            })),
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

    fn definition(
        &mut self,
        path: &str,
        position: Utf16Position,
    ) -> Result<NativeDefinitionResult, NativeError> {
        self.log
            .lock()
            .expect("log")
            .definition_calls
            .push((path.to_owned(), position));
        if let Some(gate) = &self.behavior.definition_gate {
            gate.enter_and_wait();
        }
        match &self.behavior.definition {
            FakeDefinitionOutcome::CheckedEmpty => Ok(NativeDefinitionResult {
                targets: Vec::new(),
                status: CapabilityStatus::Supported,
                truncated: false,
            }),
            FakeDefinitionOutcome::Targets {
                targets,
                status,
                truncated,
            } => Ok(NativeDefinitionResult {
                targets: targets.clone(),
                status: status.clone(),
                truncated: *truncated,
            }),
            FakeDefinitionOutcome::TargetInScript { needle } => {
                let text = self.text(SCRIPT_PATH).ok_or(NativeError::InvalidParams {
                    reason: "script is not in the open snapshot",
                })?;
                Ok(NativeDefinitionResult {
                    targets: vec![native_target(SCRIPT_PATH, text, needle)],
                    status: CapabilityStatus::Supported,
                    truncated: false,
                })
            }
            FakeDefinitionOutcome::Failure(failure) => Err(failure.native()),
        }
    }

    fn document_symbols(&mut self, path: &str) -> Result<DocumentSymbolsEvidence, NativeError> {
        self.log
            .lock()
            .expect("log")
            .document_symbol_calls
            .push(path.to_owned());
        match &self.behavior.document_symbols {
            FakeDocumentSymbolOutcome::CheckedEmpty => Ok(DocumentSymbolsEvidence {
                symbols: Vec::new(),
                gaps: Vec::new(),
                skipped: 0,
                truncated: false,
            }),
            FakeDocumentSymbolOutcome::Script {
                symbols,
                gaps,
                skipped,
                truncated,
            } => {
                let text = self.text(path).ok_or(NativeError::InvalidParams {
                    reason: "file is not in the open snapshot",
                })?;
                Ok(fake_document_symbols(
                    text, symbols, gaps, *skipped, *truncated,
                ))
            }
            FakeDocumentSymbolOutcome::Failure(failure) => Err(failure.native()),
        }
    }

    fn shutdown(self: Box<Self>) {
        self.log.lock().expect("log").shutdowns += 1;
    }
}

fn fake_document_symbols(
    text: &str,
    symbols: &[FakeSymbolSpec],
    gaps: &[FakeGapSpec],
    skipped: usize,
    truncated: bool,
) -> DocumentSymbolsEvidence {
    DocumentSymbolsEvidence {
        symbols: symbols
            .iter()
            .map(|symbol| fake_document_symbol(text, symbol))
            .collect(),
        gaps: gaps
            .iter()
            .map(|gap| {
                let start = text
                    .find(gap.start)
                    .expect("fake document symbol gap needle");
                NativeDocumentSymbolGap {
                    node_kind: VARIABLE_DECLARATION_KIND,
                    name_kind: None,
                    start_utf16: utf16_len(&text[..start]),
                    reason: gap.reason,
                }
            })
            .collect(),
        skipped,
        truncated,
    }
}

fn fake_document_symbol(text: &str, spec: &FakeSymbolSpec) -> DocumentSymbolEvidence {
    let start = text
        .find(spec.declaration)
        .expect("fake document symbol declaration needle");
    let end = spec
        .declaration_end
        .map_or(start + spec.declaration.len(), |end| {
            text.find(end).expect("fake document symbol end needle") + end.len()
        });
    let selection_start = spec
        .declaration
        .find(spec.selection)
        .map(|offset| start + offset)
        .or_else(|| text.find(spec.selection))
        .expect("fake document symbol selection");
    let selection_end = selection_start + spec.selection.len();
    DocumentSymbolEvidence {
        name: spec.name.to_owned(),
        node_kind: spec.node_kind,
        start_utf16: Utf16Offset::new(utf16_len(&text[..start])),
        end_utf16: Utf16Offset::new(utf16_len(&text[..end])),
        start_utf8: Utf8Offset::new(u32::try_from(start).expect("start")),
        end_utf8: Utf8Offset::new(u32::try_from(end).expect("end")),
        selection_start_utf16: Utf16Offset::new(utf16_len(&text[..selection_start])),
        selection_end_utf16: Utf16Offset::new(utf16_len(&text[..selection_end])),
        selection_start_utf8: Utf8Offset::new(
            u32::try_from(selection_start).expect("selection start"),
        ),
        selection_end_utf8: Utf8Offset::new(u32::try_from(selection_end).expect("selection end")),
        children: spec
            .children
            .iter()
            .map(|child| fake_document_symbol(text, child))
            .collect(),
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

fn native_target(path: &str, text: &str, needle: &str) -> NativeDefinitionTarget {
    let start = text.find(needle).expect("target needle");
    let end = start + needle.len();
    let start_utf16 = utf16_len(&text[..start]);
    let end_utf16 = utf16_len(&text[..end]);
    NativeDefinitionTarget {
        path: path.to_owned(),
        range: NativeSourceRange::new(
            u32::try_from(start).expect("target start"),
            u32::try_from(end).expect("target end"),
        ),
        utf16: Utf16Range::new(start_utf16, end_utf16),
    }
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
    let (adapter, log) = adapter_with(
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
    // The exact backend composition order: type fence, comment, JSDoc tags.
    assert_eq!(
        result.contents,
        format!("```ts\nstring\n```\n\n{COMMENT}\n\n@deprecated use title")
    );
    let range = result.range.expect("hover range");
    assert_eq!(
        range.range(),
        wxml_text::TextRange::new(offset_of(WXML, "name"), offset_of(WXML, "name") + 4)
            .expect("range")
    );
    let log = log.lock().expect("log");
    assert_eq!(log.hover_evidence_calls.len(), 1);
    assert!(log.type_calls.is_empty());
    assert_eq!(log.hover_documentation_calls, 0);
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
    assert_eq!(result.contents, "```ts\nany\n```");
}

#[test]
fn hover_unknown_quality_keeps_backend_partial_without_a_type_fence() {
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Known("unknown".to_owned()),
        docs_outcome: documented_behavior().docs_outcome,
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
    assert_eq!(result.quality, wxml_types::TypeQuality::Unknown);
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "the hover type is unknown".to_owned(),
        }
    );
    assert_eq!(
        result.contents,
        format!("{COMMENT}\n\n@deprecated use title")
    );
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
    assert_eq!(
        result.status,
        CapabilityStatus::Unsupported {
            reason: "no type or documentation evidence is available at the position".to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Missing);
    // The range still comes from the template token, not from the backend.
    assert_eq!(
        result.range.expect("hover range").start(),
        offset_of(WXML, "name")
    );
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
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "documentation: one documentation method failed".to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert_eq!(result.contents, "```ts\nstring\n```");
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
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "documentation: native documentation queries are unavailable".to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert_eq!(result.contents, "```ts\nstring\n```");
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
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "the prepared source file list is incomplete".to_owned(),
        }
    );
    assert_eq!(result.contents, "```ts\nstring\n```");
}

#[test]
fn hover_declaration_gap_keeps_the_backend_partial_reason() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Evidence(FakeHoverEvidence {
            type_text: Some("string"),
            quality: NativeTypeQuality::Known,
            documentation: None,
            gaps: vec![HoverGap::ExternalDeclaration {
                path: Some("lib.d.ts".to_owned()),
            }],
            status: None,
        }),
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
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "declaration `lib.d.ts` is outside the project snapshot".to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert_eq!(result.contents, "```ts\nstring\n```");
}

#[test]
fn a_fatal_hover_evidence_query_drops_the_session_and_returns_the_error() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Timeout),
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
        .expect_err("fatal hover evidence failure");
    assert!(matches!(error, BackendError::Timeout));
    assert_eq!(log.lock().expect("log").shutdowns, 1);
    assert!(adapter.epoch().get() > before.get());
}

#[test]
fn hover_evidence_that_contradicts_its_markdown_is_partial_not_supported() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Evidence(FakeHoverEvidence {
            type_text: Some("string"),
            quality: NativeTypeQuality::Known,
            documentation: None,
            gaps: Vec::new(),
            status: Some(CapabilityStatus::Unsupported {
                reason: "the backend refused this position".to_owned(),
            }),
        }),
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
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "native hover evidence is unsupported despite rendered content: \
                     the backend refused this position"
                .to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert_eq!(result.contents, "```ts\nstring\n```");
}

#[test]
fn backend_partial_keeps_the_host_degradation_reason() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Evidence(FakeHoverEvidence {
            type_text: Some("string"),
            quality: NativeTypeQuality::Known,
            documentation: None,
            gaps: vec![HoverGap::ExternalDeclaration {
                path: Some("lib.d.ts".to_owned()),
            }],
            status: None,
        }),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: "declaration `lib.d.ts` is outside the project snapshot; \
                     the prepared source file list is incomplete"
                .to_owned(),
        }
    );
    assert_eq!(result.contents, "```ts\nstring\n```");
}

#[test]
fn backend_partial_and_range_failure_keep_both_reasons() {
    let status = resolved_status(
        &CapabilityStatus::Partial {
            reason: "declaration `lib.d.ts` is outside the project snapshot".to_owned(),
        },
        "```ts\nstring\n```",
        None,
        true,
    );
    assert_eq!(
        status,
        CapabilityStatus::Partial {
            reason: "declaration `lib.d.ts` is outside the project snapshot; \
                     the hover range could not be mapped"
                .to_owned(),
        }
    );
}

#[test]
fn an_empty_partial_hover_is_normalized_to_unsupported_with_reasons() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Evidence(FakeHoverEvidence {
            type_text: None,
            quality: NativeTypeQuality::Missing,
            documentation: None,
            gaps: Vec::new(),
            status: Some(CapabilityStatus::Partial {
                reason: "the backend had no renderable facts".to_owned(),
            }),
        }),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover");
    assert_eq!(result.contents, "");
    assert_eq!(
        result.status,
        CapabilityStatus::Unsupported {
            reason: "the backend had no renderable facts; \
                     the prepared source file list is incomplete"
                .to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Missing);
}

#[test]
fn an_empty_supported_hover_is_a_contract_violation_not_success() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Evidence(FakeHoverEvidence {
            type_text: None,
            quality: NativeTypeQuality::Missing,
            documentation: None,
            gaps: Vec::new(),
            status: Some(CapabilityStatus::Supported),
        }),
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
    assert_eq!(
        result.status,
        CapabilityStatus::Unsupported {
            reason: "native hover evidence is empty but the backend reported it as supported"
                .to_owned(),
        }
    );
    assert_eq!(result.quality, wxml_types::TypeQuality::Missing);
}

#[test]
fn an_unsupported_hover_evidence_query_exits_before_composition() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Unsupported("hoverEvidence")),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
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
    assert_eq!(result.markup, MarkupKind::Markdown);
    assert_eq!(result.range, None);
    assert_eq!(result.quality, wxml_types::TypeQuality::Unknown);
    assert_eq!(
        result.status,
        CapabilityStatus::Unsupported {
            reason: "native hover evidence is unavailable".to_owned(),
        }
    );
    let log = log.lock().expect("log");
    assert_eq!(log.hover_evidence_calls.len(), 1);
    assert!(log.type_calls.is_empty());
    assert_eq!(log.hover_documentation_calls, 0);
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
    let log = log.lock().expect("log");
    assert!(log.hover_evidence_calls.is_empty());
    assert!(log.type_calls.is_empty());
    assert_eq!(log.hover_documentation_calls, 0);
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
                    detail: Some("name: string".to_owned()),
                    label_details: None,
                },
                RawCompletionEntry {
                    name: "age".to_owned(),
                    kind: 5,
                    sort_text: "12".to_owned(),
                    detail: None,
                    label_details: None,
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
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["name", "age"],
        "native order must be preserved"
    );
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: concat!(
                "the pinned completion API exposes no per-entry documentation; ",
                "auto-import completion entries require a snapshot created with prepareAutoImports",
            )
            .to_owned(),
        }
    );
    for item in &result.items {
        assert_eq!(item.replace.start(), offset_of(wxml, "na"));
        assert_eq!(item.replace.end(), cursor);
        assert_eq!(item.insert_text, item.label);
        assert_eq!(
            item.insert_text_format,
            wxml_types::InsertTextFormat::PlainText
        );
        assert_eq!(item.kind, CompletionItemKind::Property);
        assert_eq!(item.documentation, None);
        assert_eq!(item.filter_text.as_deref(), Some(item.label.as_str()));
    }
    assert_eq!(result.items[0].detail.as_deref(), Some("name: string"));
    assert_eq!(result.items[0].sort_text.as_deref(), Some("11"));
    assert_eq!(result.items[1].detail, None);
    assert_eq!(result.items[1].sort_text.as_deref(), Some("12"));
}

#[test]
fn completion_does_not_publish_an_empty_sort_text() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "name".to_owned(),
                kind: 5,
                sort_text: String::new(),
                detail: None,
                label_details: None,
            }],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
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
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].label, "name");
    assert_eq!(
        result.items[0].sort_text, None,
        "an absent native sortText must not become an empty sort key"
    );
}

#[test]
fn completion_tolerates_label_details_if_present() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "setName".to_owned(),
                kind: 2,
                sort_text: "13".to_owned(),
                detail: None,
                label_details: Some(RawCompletionLabelDetails {
                    detail: Some("(value: string): void".to_owned()),
                    description: None,
                }),
            }],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
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
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].label, "setName");
    assert_eq!(result.items[0].kind, CompletionItemKind::Method);
    assert_eq!(
        result.items[0].detail.as_deref(),
        Some("(value: string): void")
    );
    assert_eq!(result.items[0].documentation, None);
}

#[test]
fn completion_unknown_kind_is_plain_value_and_partial() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "mystery".to_owned(),
                kind: 99,
                sort_text: "14".to_owned(),
                detail: None,
                label_details: None,
            }],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
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
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].label, "mystery");
    assert_eq!(result.items[0].kind, CompletionItemKind::Value);
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("CompletionItemKind"), "{reason}");
}

#[test]
fn completion_keeps_host_degradation_alongside_native_gaps() {
    let wxml = "<view>{{ user.na }}</view>";
    let mut view = prepared_view(wxml, SCRIPT, "user.na");
    view.source_complete = false;
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "name".to_owned(),
                kind: 5,
                sort_text: "11".to_owned(),
                detail: None,
                label_details: None,
            }],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].label, "name");
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("documentation"), "{reason}");
    assert!(reason.contains("auto-import"), "{reason}");
    assert!(
        reason.contains("source file list is incomplete"),
        "the host degradation must survive a native Partial: {reason}"
    );
}

#[test]
fn a_checked_empty_completion_keeps_host_degradation() {
    let wxml = "<view>{{ user.na }}</view>";
    let mut view = prepared_view(wxml, SCRIPT, "user.na");
    view.source_complete = false;
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::EmptyList,
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(result.items, Vec::new());
    assert!(!result.truncated);
    let reason = result.status.reason().expect("degradation reason");
    assert!(
        reason.contains("source file list is incomplete"),
        "{reason}"
    );
}

#[test]
fn a_checked_empty_completion_is_supported() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::EmptyList,
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
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
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert!(!result.truncated);
    assert_eq!(log.lock().expect("log").completion_calls.len(), 1);
}

#[test]
fn a_null_completion_is_partial_not_a_checked_empty_answer() {
    let wxml = "<view>{{ user.na }}</view>";
    let mut view = prepared_view(wxml, SCRIPT, "user.na");
    view.source_complete = false;
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::NoResponse,
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(result.items, Vec::new());
    assert!(!result.truncated);
    let reason = result.status.reason().expect("partial reason");
    assert!(
        reason.contains("null"),
        "a null response must be a typed partial gap: {reason}"
    );
    assert!(
        reason.contains("source file list is incomplete"),
        "host degradation must survive a null response: {reason}"
    );
}

#[test]
fn completion_truncation_is_reported() {
    let entries: Vec<RawCompletionEntry> = (0..=wxml_types::MAX_COMPLETION_ITEMS)
        .map(|index| RawCompletionEntry {
            name: format!("item{index}"),
            kind: 5,
            sort_text: "1".to_owned(),
            detail: None,
            label_details: None,
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
    assert_eq!(
        result.items.first().map(|item| item.label.as_str()),
        Some("item0"),
        "truncation must keep the native prefix order"
    );
    assert_eq!(
        result.items.last().map(|item| item.label.as_str()),
        Some("item255")
    );
    assert_eq!(
        result.status,
        CapabilityStatus::Partial {
            reason: concat!(
                "the shared completion item cap truncated the list; ",
                "the pinned completion API exposes no per-entry documentation; ",
                "auto-import completion entries require a snapshot created with prepareAutoImports",
            )
            .to_owned(),
        }
    );
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
fn symbols_are_explicitly_unsupported() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let context = context(&adapter, WXML);
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
fn definition_maps_a_target_file_utf16_range_to_utf8() {
    let target = native_target(MODEL_PATH, MODEL, "name");
    let expected_start = u32::try_from(MODEL.find("name").expect("model name")).expect("offset");
    let expected_end =
        u32::try_from(MODEL.find("name").expect("model name") + "name".len()).expect("offset");
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Targets {
            targets: vec![target],
            status: CapabilityStatus::Supported,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(MODEL_PATH, MODEL),
    ];
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view_with_files(
            WXML,
            "user.name",
            files,
        ))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.locations.len(), 1);
    let location = &result.locations[0];
    assert_eq!(
        location.file,
        FileId::new(project(), MODEL_PATH).expect("model file")
    );
    assert_eq!(location.range.start(), expected_start);
    assert_eq!(location.range.end(), expected_end);
    let expected_position = derived_position(&log, "user.name", Some("name"));
    let calls = log.lock().expect("log").definition_calls.clone();
    assert_eq!(calls, vec![(SCRIPT_PATH.to_owned(), expected_position)]);
}

#[test]
fn a_native_partial_empty_definition_is_not_a_checked_empty_success() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Targets {
            targets: Vec::new(),
            status: CapabilityStatus::Partial {
                reason: "some declarations could not be decoded".to_owned(),
            },
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
}

#[test]
fn definition_maps_a_shadow_script_target_back_to_the_original_script() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::TargetInScript { needle: "age" },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            UNICODE_SCRIPT,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.locations.len(), 1);
    let location = &result.locations[0];
    assert_eq!(location.file, script_file());
    let expected = offset_of(UNICODE_SCRIPT, "age");
    let expected_utf16 =
        utf16_len(&UNICODE_SCRIPT[..usize::try_from(expected).expect("original byte offset")]);
    assert_ne!(
        expected, expected_utf16,
        "the fixture must prove UTF-16 and UTF-8 offsets differ"
    );
    assert_eq!(location.range.start(), expected);
    assert_eq!(
        location.range.end(),
        expected + u32::try_from("age".len()).expect("length")
    );
    let expected_position = derived_position(&log, "user.name", Some("name"));
    let calls = log.lock().expect("log").definition_calls.clone();
    assert_eq!(calls, vec![(SCRIPT_PATH.to_owned(), expected_position)]);
}

#[test]
fn an_inconsistent_native_definition_byte_range_is_partial() {
    let mut target = native_target(MODEL_PATH, MODEL, "name");
    let shifted = NativeSourceRange::new(target.range.start.get() + 1, target.range.end.get() + 1);
    target.range = shifted;
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Targets {
            targets: vec![target],
            status: CapabilityStatus::Supported,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(MODEL_PATH, MODEL),
    ];
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view_with_files(
            WXML,
            "user.name",
            files,
        ))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Partial { ref reason }
            if reason.contains("could not be represented")
    ));
}

#[test]
fn a_truncated_native_definition_is_partial_and_keeps_mapped_targets() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Targets {
            targets: vec![native_target(MODEL_PATH, MODEL, "name")],
            status: CapabilityStatus::Supported,
            truncated: true,
        },
        ..FakeBehavior::default()
    };
    let files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(MODEL_PATH, MODEL),
    ];
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view_with_files(
            WXML,
            "user.name",
            files,
        ))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations.len(), 1);
    assert!(matches!(
        result.status,
        CapabilityStatus::Partial { ref reason } if reason.contains("truncated")
    ));
}

#[test]
fn duplicate_mapped_definition_targets_are_folded_and_reported() {
    let target = native_target(MODEL_PATH, MODEL, "name");
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Targets {
            targets: vec![target.clone(), target],
            status: CapabilityStatus::Supported,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(MODEL_PATH, MODEL),
    ];
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view_with_files(
            WXML,
            "user.name",
            files,
        ))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations.len(), 1);
    assert!(matches!(
        result.status,
        CapabilityStatus::Partial { ref reason } if reason.contains("duplicate")
    ));
}

#[test]
fn definition_degrades_when_the_prepared_source_list_is_incomplete() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Partial { ref reason } if reason.contains("incomplete")
    ));
}

#[test]
fn a_definition_target_in_generated_shadow_text_is_partial_and_dropped() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::TargetInScript {
            needle: "__wxml_check_0",
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Partial { ref reason }
            if reason.contains("could not be represented")
    ));
}

#[test]
fn definition_with_no_native_symbol_is_a_checked_empty_success() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert_eq!(result.status, CapabilityStatus::Supported);
    let second = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("second definition");
    assert_eq!(second.status, CapabilityStatus::Supported);
    let log = log.lock().expect("log");
    assert_eq!(log.definition_calls.len(), 2);
    assert_eq!(log.starts, 1, "an unchanged request must reuse the session");
}

#[test]
fn an_unrepresentable_definition_target_is_partial_but_keeps_valid_targets() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Targets {
            targets: vec![
                native_target(MODEL_PATH, MODEL, "name"),
                native_target("missing.ts", MODEL, "name"),
            ],
            status: CapabilityStatus::Supported,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(MODEL_PATH, MODEL),
    ];
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view_with_files(
            WXML,
            "user.name",
            files,
        ))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations.len(), 1);
    assert!(matches!(
        result.status,
        CapabilityStatus::Partial { ref reason }
            if reason.contains("could not be represented")
    ));
}

#[test]
fn an_unsupported_native_definition_is_explicitly_unsupported() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Failure(FakeFailure::Unsupported("definition")),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 1);
}

#[test]
fn a_failed_definition_query_is_an_error_not_a_supported_empty_result() {
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let before = adapter.epoch();
    let error = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("definition failure");
    assert!(matches!(error, BackendError::Timeout));
    assert_eq!(log.lock().expect("log").shutdowns, 1);
    assert!(adapter.epoch().get() > before.get());
}

#[test]
fn definition_outside_a_mounted_expression_is_unsupported_without_a_query() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "<view>"),
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
    assert_eq!(log.lock().expect("log").definition_calls.len(), 0);
}

#[test]
fn definition_at_the_expression_end_still_queries_the_native_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let expression_end =
        offset_of(WXML, "user.name") + u32::try_from("user.name".len()).expect("length");
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: expression_end,
        })
        .expect("definition");
    assert_eq!(result.locations, Vec::new());
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(log.lock().expect("log").definition_calls.len(), 1);
}

#[test]
fn definition_rejects_a_stale_epoch_before_starting_a_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let stale = BackendEpoch::new(
        BackendId::new(BACKEND_ID),
        adapter.epoch().get().saturating_add(1),
    );
    let error = adapter
        .definition(DefinitionRequest {
            context: context_with_epoch(&adapter, WXML, stale),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("stale epoch");
    assert!(matches!(error, BackendError::Cancelled));
    assert_eq!(log.lock().expect("log").starts, 0);
    assert_eq!(log.lock().expect("log").definition_calls.len(), 0);
}

#[test]
fn a_slow_definition_is_cancelled_by_invalidation_and_drops_the_session() {
    let gate = Arc::new(SlowGate::default());
    let behavior = FakeBehavior {
        definition_gate: Some(Arc::clone(&gate)),
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
        worker.definition(DefinitionRequest {
            context: first_context,
            offset: offset_of(WXML, "name"),
        })
    });
    gate.wait_until_entered();
    adapter.invalidate();
    gate.release();
    let result = handle.join().expect("definition thread");
    assert!(matches!(result, Err(BackendError::Cancelled)));
    assert_eq!(log.lock().expect("log").shutdowns, 1);
    assert_eq!(log.lock().expect("log").definition_calls.len(), 1);
    assert_eq!(log.lock().expect("log").starts, 1);
}

const SYMBOL_SCRIPT: &str = "// 😀 文档\nclass 用户 { // 😀\n  name: string = \"😀\";\n  greet(): void {}\n}\ntype Shape = { area(): number };\nPage({\n  data: { user: { name: \"Ada\", age: 42 } },\n});\nfunction after(): number {\n  return 1;\n}\n";
const SYMBOL_CLASS: &str = "class 用户 { // 😀\n  name: string = \"😀\";\n  greet(): void {}\n}";
const SYMBOL_PROPERTY: &str = "name: string = \"😀\";";
const SYMBOL_ALIAS: &str = "type Shape = { area(): number };";
const SYMBOL_AFTER: &str = "function after(): number {\n  return 1;\n}";

fn symbol_span(text: &str, needle: &str) -> (u32, u32) {
    let start = u32::try_from(text.find(needle).expect("needle in text")).expect("offset");
    let length = u32::try_from(needle.len()).expect("needle length");
    (start, start + length)
}

fn document_symbols_request(
    adapter: &TsgoBackendAdapter,
    subject: FileId,
) -> DocumentSymbolsRequest {
    DocumentSymbolsRequest {
        context: context(adapter, WXML),
        subject,
    }
}

#[test]
fn document_symbols_map_shadow_ranges_back_to_original_script() {
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Script {
            symbols: vec![
                FakeSymbolSpec::with_children(
                    CLASS_DECLARATION_KIND,
                    "用户",
                    SYMBOL_CLASS,
                    vec![
                        FakeSymbolSpec::new(PROPERTY_DECLARATION_KIND, "name", SYMBOL_PROPERTY),
                        FakeSymbolSpec::new(METHOD_DECLARATION_KIND, "greet", "greet(): void {}"),
                    ],
                ),
                FakeSymbolSpec::new(TYPE_ALIAS_DECLARATION_KIND, "Shape", SYMBOL_ALIAS),
                FakeSymbolSpec::new(FUNCTION_DECLARATION_KIND, "after", SYMBOL_AFTER),
            ],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SYMBOL_SCRIPT,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert_eq!(
        log.lock().expect("log").document_symbol_calls,
        vec![SCRIPT_PATH.to_owned()]
    );
    assert_eq!(
        result.status,
        CapabilityStatus::Supported,
        "{:?}",
        result.status
    );
    assert!(!result.truncated);
    assert_eq!(result.skipped, 0);
    assert!(result.gaps.is_empty());
    assert_eq!(
        result
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind))
            .collect::<Vec<_>>(),
        vec![
            ("用户", DocumentSymbolKind::Class),
            ("Shape", DocumentSymbolKind::TypeAlias),
            ("after", DocumentSymbolKind::Function),
        ]
    );

    let class = &result.symbols[0];
    assert_eq!(
        (class.range.start(), class.range.end()),
        symbol_span(SYMBOL_SCRIPT, SYMBOL_CLASS)
    );
    assert_eq!(
        (class.selection_range.start(), class.selection_range.end()),
        symbol_span(SYMBOL_SCRIPT, "用户")
    );
    assert_eq!(class.children.len(), 2);
    assert_eq!(class.children[0].name, "name");
    assert_eq!(class.children[0].kind, DocumentSymbolKind::Property);
    assert_eq!(
        (
            class.children[0].range.start(),
            class.children[0].range.end()
        ),
        symbol_span(SYMBOL_SCRIPT, SYMBOL_PROPERTY)
    );
    assert_eq!(
        (
            class.children[0].selection_range.start(),
            class.children[0].selection_range.end()
        ),
        symbol_span(SYMBOL_SCRIPT, "name")
    );
    assert_eq!(class.children[1].name, "greet");
    assert_eq!(class.children[1].kind, DocumentSymbolKind::Method);
    assert_eq!(
        (
            class.children[1].selection_range.start(),
            class.children[1].selection_range.end()
        ),
        symbol_span(SYMBOL_SCRIPT, "greet")
    );
    for child in &class.children {
        assert!(class.range.start() <= child.range.start());
        assert!(child.range.end() <= class.range.end());
        assert!(child.range.start() <= child.selection_range.start());
        assert!(child.selection_range.end() <= child.range.end());
    }

    let alias = &result.symbols[1];
    assert_eq!(
        (alias.range.start(), alias.range.end()),
        symbol_span(SYMBOL_SCRIPT, SYMBOL_ALIAS)
    );
    assert_eq!(
        (alias.selection_range.start(), alias.selection_range.end()),
        symbol_span(SYMBOL_SCRIPT, "Shape")
    );

    // `function after` follows the generated probe mount, so its derived
    // offset differs from the original script; the mapped range must be the
    // original one.
    let after = &result.symbols[2];
    assert_eq!(
        (after.range.start(), after.range.end()),
        symbol_span(SYMBOL_SCRIPT, SYMBOL_AFTER)
    );
    assert_eq!(
        (after.selection_range.start(), after.selection_range.end()),
        symbol_span(SYMBOL_SCRIPT, "after")
    );
    assert!(
        after.range.start() <= after.selection_range.start()
            && after.selection_range.end() <= after.range.end()
    );
}

#[test]
fn document_symbols_in_a_template_fragment_are_dropped_and_partial() {
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Script {
            symbols: vec![
                FakeSymbolSpec::new(CLASS_DECLARATION_KIND, "user", "user.name"),
                FakeSymbolSpec::new(TYPE_ALIAS_DECLARATION_KIND, "Shape", SYMBOL_ALIAS),
            ],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SYMBOL_SCRIPT,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("template fragment"), "{reason}");
    assert_eq!(result.symbols.len(), 1);
    assert_eq!(result.symbols[0].name, "Shape");
    assert_eq!(result.symbols[0].kind, DocumentSymbolKind::TypeAlias);
}

#[test]
fn document_symbols_keep_gaps_truncation_and_skip_counts() {
    let mut gaps = vec![FakeGapSpec {
        start: "user.name",
        reason: "name slot is not an Identifier",
    }];
    for _ in 0..wxml_types::MAX_DOCUMENT_SYMBOL_GAPS {
        gaps.push(FakeGapSpec {
            start: "type Shape",
            reason: "name slot is not an Identifier",
        });
    }
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Script {
            symbols: vec![FakeSymbolSpec::new(
                FUNCTION_DECLARATION_KIND,
                "after",
                SYMBOL_AFTER,
            )],
            gaps,
            skipped: wxml_types::MAX_DOCUMENT_SYMBOL_GAPS + 1,
            truncated: true,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SYMBOL_SCRIPT,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert!(result.truncated);
    assert_eq!(result.skipped, wxml_types::MAX_DOCUMENT_SYMBOL_GAPS + 1);
    assert_eq!(result.symbols.len(), 1);
    assert_eq!(result.gaps.len(), wxml_types::MAX_DOCUMENT_SYMBOL_GAPS);
    // The first gap is inside the mounted template expression; a template
    // fragment has no original-script offset and must stay `None`.
    assert_eq!(result.gaps[0].start, None);
    assert_eq!(
        result.gaps[1].start,
        Some(symbol_span(SYMBOL_SCRIPT, "type Shape").0)
    );
    assert_eq!(result.gaps[1].reason, "name slot is not an Identifier");
}

#[test]
fn document_symbols_with_an_outside_selection_are_dropped_not_clamped() {
    let mut invalid = FakeSymbolSpec::new(FUNCTION_DECLARATION_KIND, "after", SYMBOL_AFTER);
    invalid.selection = "用户";
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Script {
            symbols: vec![invalid],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SYMBOL_SCRIPT,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("outside the declaration range"), "{reason}");
    assert!(result.symbols.is_empty());
}

#[test]
fn document_symbols_crossing_generated_text_are_dropped_and_partial() {
    let mut crossing = FakeSymbolSpec::new(FUNCTION_DECLARATION_KIND, "Page", "Page(");
    crossing.declaration_end = Some("data:");
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Script {
            symbols: vec![crossing],
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SYMBOL_SCRIPT,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("crossed generated shadow text"), "{reason}");
    assert!(result.symbols.is_empty());
}

#[test]
fn document_symbols_merge_host_degradation_into_the_status() {
    let mut view = prepared_view(WXML, SYMBOL_SCRIPT, "user.name");
    view.source_complete = false;
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(result.symbols.is_empty());
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    let reason = result.status.reason().expect("partial reason");
    assert!(
        reason.contains("prepared source file list is incomplete"),
        "{reason}"
    );
}

#[test]
fn document_symbols_reject_a_subject_outside_the_prepared_view() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let other = FileId::new(project(), MODEL_PATH).expect("other file");
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, other))
        .expect("document symbols");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert!(result.symbols.is_empty());
    assert_eq!(log.lock().expect("log").starts, 0);
    assert!(log.lock().expect("log").document_symbol_calls.is_empty());
}

#[test]
fn an_unsupported_native_document_symbol_query_is_explicitly_unsupported() {
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Failure(FakeFailure::Unsupported(
            "getSourceFile document symbols",
        )),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert!(result.symbols.is_empty());
    assert!(result.gaps.is_empty());
    assert_eq!(log.lock().expect("log").starts, 1);
    assert_eq!(log.lock().expect("log").shutdowns, 0);
}

#[test]
fn a_failed_document_symbol_query_is_an_error_not_a_supported_empty_result() {
    let behavior = FakeBehavior {
        document_symbols: FakeDocumentSymbolOutcome::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let before = adapter.epoch();
    let error = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect_err("fatal document-symbol failure");
    assert!(matches!(error, BackendError::Timeout));
    assert_eq!(log.lock().expect("log").starts, 1);
    assert_eq!(log.lock().expect("log").shutdowns, 1);
    assert!(adapter.epoch().get() > before.get());
}

#[test]
fn an_empty_native_document_symbol_answer_is_a_checked_empty_success() {
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("document symbols");
    assert!(result.symbols.is_empty());
    assert!(result.gaps.is_empty());
    assert!(!result.truncated);
    assert_eq!(result.skipped, 0);
    assert_eq!(result.status, CapabilityStatus::Supported);
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
        CapabilityStatus::Partial { .. }
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
    let calls = &log.lock().expect("log").hover_evidence_calls;
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
    let calls = &cjk_log.lock().expect("log").hover_evidence_calls;
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
                detail: None,
                label_details: None,
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
    assert_eq!(result.items[0].label, "name");
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

#[test]
fn optional_real_engine_definition_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine adapter definition test skipped: set TSGO_BIN to the pinned \
             7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-adapter-definition-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let wxml = "<view>{{ u.name }}</view>";
    let script = r#"import type { User } from "./model";
const u: User = { name: "Ada", age: 42 };
Page({
  data: { u },
});
"#;
    let model = r"export interface User {
    /** Display name. */
    name: string;
    age: number;
}
";
    let globals = r#"import type { User } from "./model";
declare global {
  function Page(options: {
    data: { u: User };
    __wxml_check_0(this: { data: { u: User } }): void;
  }): void;
}
export {};
"#;
    let files = vec![
        prepared_file(SCRIPT_PATH, script),
        prepared_file("model.ts", model),
        prepared_file("globals.d.ts", globals),
    ];
    let views = Arc::new(FakeViews::new(prepared_view_with_files(
        wxml, "u.name", files,
    )));
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        views,
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );
    let result = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "name"),
        })
        .expect("real engine definition");
    eprintln!(
        "real engine adapter definition: status={:?} locations={:?}",
        result.status, result.locations
    );
    assert_eq!(
        result.status,
        CapabilityStatus::Supported,
        "the real engine must decode the model.ts declaration token"
    );
    let location = result
        .locations
        .iter()
        .find(|location| location.file.path() == "model.ts")
        .expect("the property declaration in model.ts");
    let expected_start =
        u32::try_from(model.find("name: string").expect("model property")).expect("model offset");
    assert_eq!(location.range.start(), expected_start);
    assert_eq!(
        location.range.end(),
        expected_start + u32::try_from("name".len()).expect("length")
    );
}

#[test]
fn optional_real_engine_completion_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine adapter completion test skipped: set TSGO_BIN to the pinned \
             7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-adapter-completion-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    // `u.name` is a valid fragment, so both cursors are real member-completion
    // positions: after `u.` (empty prefix) and after `u.na` (partial prefix).
    // No unclosed-expression synthesis is involved.
    let wxml = "<view>{{ u.name }}</view>";
    let script = r#"import type { User } from "./model";
const u: User = { name: "Ada", age: 42 };
Page({
  data: { u },
});
"#;
    let model = r"export interface User {
    /** Display name. */
    name: string;
    age: number;
}
";
    let globals = r#"import type { User } from "./model";
declare global {
  function Page(options: {
    data: { u: User };
    __wxml_check_0(this: { data: { u: User } }): void;
  }): void;
}
export {};
"#;
    let files = vec![
        prepared_file(SCRIPT_PATH, script),
        prepared_file("model.ts", model),
        prepared_file("globals.d.ts", globals),
    ];
    let views = Arc::new(FakeViews::new(prepared_view_with_files(
        wxml, "u.name", files,
    )));
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        views,
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );

    let cursors = [
        (offset_of(wxml, ".") + 1, 0_u32),
        (offset_of(wxml, "na") + 2, 2_u32),
    ];
    for (cursor, prefix_len) in cursors {
        let result = adapter
            .completion(CompletionRequest {
                context: context(&adapter, wxml),
                offset: cursor,
                kind: CompletionKind::ExpressionMember,
            })
            .expect("real engine completion");
        assert_real_engine_completion(&result, cursor, prefix_len);
    }
}

#[test]
fn optional_real_engine_document_symbols_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref() == Ok("1");
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine adapter document-symbol test skipped: set TSGO_BIN to the \
             pinned 7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-adapter-document-symbols-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let script = "interface User {\n  name: string;\n}\ntype Alias = User;\nclass Demo {\n  value: number = 1;\n  method(): void {}\n}\nfunction helper(): number {\n  return 1;\n}\nconst answer = 42;\nPage({\n  data: { user: { name: \"Ada\" } },\n});\n";
    let wxml = "<view>{{ user.name }}</view>";
    let views = Arc::new(FakeViews::new(prepared_view(wxml, script, "user.name")));
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        views,
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );
    let result = adapter
        .document_symbols(document_symbols_request(&adapter, script_file()))
        .expect("real engine document symbols");
    eprintln!(
        "real engine adapter document symbols: status={:?} symbols={:?}",
        result.status,
        result
            .symbols
            .iter()
            .map(|symbol| (
                symbol.name.as_str(),
                symbol.kind,
                symbol.range.start(),
                symbol.range.end(),
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "the registration call is an explicit walker gap: {:?}",
        result.status
    );
    assert_eq!(
        result
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind))
            .collect::<Vec<_>>(),
        vec![
            ("User", DocumentSymbolKind::Interface),
            ("Alias", DocumentSymbolKind::TypeAlias),
            ("Demo", DocumentSymbolKind::Class),
            ("helper", DocumentSymbolKind::Function),
            ("answer", DocumentSymbolKind::Variable),
        ]
    );
    let user = &result.symbols[0];
    assert_eq!(user.children.len(), 1);
    assert_eq!(user.children[0].name, "name");
    assert_eq!(user.children[0].kind, DocumentSymbolKind::Property);
    let demo = &result.symbols[2];
    assert_eq!(
        demo.children
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind))
            .collect::<Vec<_>>(),
        vec![
            ("value", DocumentSymbolKind::Property),
            ("method", DocumentSymbolKind::Method),
        ]
    );

    let expected = [
        ("User", DocumentSymbolKind::Interface, 0, 34),
        ("Alias", DocumentSymbolKind::TypeAlias, 34, 53),
        ("Demo", DocumentSymbolKind::Class, 53, 109),
        ("helper", DocumentSymbolKind::Function, 109, 151),
        ("answer", DocumentSymbolKind::Variable, 157, 169),
    ];
    for (symbol, (name, kind, range_start, range_end)) in result.symbols.iter().zip(expected) {
        assert_eq!(symbol.name, name);
        assert_eq!(symbol.kind, kind);
        assert_eq!(
            (symbol.range.start(), symbol.range.end()),
            (range_start, range_end),
            "{name} must keep the pinned engine's full-start declaration range"
        );
        let (selection_start, selection_end) = symbol_span(script, name);
        assert_eq!(symbol.selection_range.start(), selection_start, "{name}");
        assert_eq!(symbol.selection_range.end(), selection_end, "{name}");
        assert!(symbol.range.start() <= symbol.selection_range.start());
        assert!(symbol.selection_range.end() <= symbol.range.end());
    }
    assert!(result.skipped >= 1);
    assert!(!result.gaps.is_empty());
}

fn assert_real_engine_completion(result: &CompletionResult, cursor: u32, prefix_len: u32) {
    let labels: Vec<&str> = result
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    let sort_texts: Vec<Option<&str>> = result
        .items
        .iter()
        .map(|item| item.sort_text.as_deref())
        .collect();
    eprintln!(
        "real engine adapter completion at {cursor}: status={:?} labels={labels:?} \
         sortTexts={sort_texts:?}",
        result.status
    );
    assert_eq!(
        labels,
        vec!["name", "age"],
        "the real engine must return exactly the User member labels in native order at cursor \
         {cursor}"
    );
    assert!(!result.truncated);
    let reason = result.status.reason().expect("real completion is partial");
    assert!(reason.contains("documentation"), "{reason}");
    assert!(reason.contains("auto-import"), "{reason}");
    for item in &result.items {
        assert_eq!(item.replace.start(), cursor - prefix_len);
        assert_eq!(item.replace.end(), cursor);
        assert_eq!(item.insert_text, item.label);
        assert_eq!(item.kind, CompletionItemKind::Property);
        assert_eq!(item.documentation, None);
        assert!(
            item.sort_text
                .as_deref()
                .is_some_and(|sort| !sort.is_empty()),
            "the real engine must expose a non-empty native sortText: {item:?}"
        );
        assert_eq!(item.filter_text.as_deref(), Some(item.label.as_str()));
    }
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
