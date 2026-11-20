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
    clippy::significant_drop_tightening,
    clippy::too_many_lines
)]

use std::collections::VecDeque;
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
use wxml_backend_tsgo::native::session::{AutoImportPreparation, DocumentationQuery};
use wxml_backend_tsgo::native::session::{SessionOptions, TypeQuery};
use wxml_backend_tsgo::native::strings::{
    Utf8Offset, Utf16Offset, Utf16Position, offset_to_position, utf16_len,
};
use wxml_backend_tsgo::native::wire::{
    RawCompletionEntry, RawCompletionLabelDetails, RawCompletionList, RawDiagnostic, RawJsDocTag,
    RawPosition, RawType,
};
use wxml_backend_tsgo::query::completion::CompletionQueryOptions;
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
    AttributeValueKind, Backend, BackendEpoch, BackendError, BackendId, CapabilityStatus,
    CheckFragment, CompletionItemKind, CompletionKind, CompletionRequest, CompletionResult,
    ComponentAttribute, ComponentUsage, DefinitionRequest, Diagnostic, DiagnosticSeverity,
    DiagnosticsRequest, DocumentSymbolKind, DocumentSymbolsRequest, FileId, HoverRequest,
    MarkupKind, ProjectId, ProjectRevision, RequestContext, RootKind, SnapshotId, SourceRevision,
    SourceSnapshot, SymbolQuery, SymbolRequest, ViewRevision,
};

use super::hover::resolved_status;
use super::input::ViewProvider;
use super::session::{HoverDocumentation, QuerySession};
use super::{BACKEND_ID, HOST_ONLY_REASON, TsgoBackendAdapter};
use crate::project_host::{
    HostError, PreparedFile, PreparedView, ProjectHost, ProjectState, ViewTarget,
};

const WXML: &str = "<view>{{ user.name }}</view>";
const SCRIPT: &str = "Page({\n  data: { user: { name: \"Ada\", age: 42 } },\n});\n";
const UNICODE_SCRIPT: &str =
    "// 😀 中文\nconst prefix = 1;\nPage({\n  data: { user: { name: \"Ada\", age: 42 } },\n});\n";
const SCRIPT_PATH: &str = "pages/index/index.ts";
const WXML_PATH: &str = "pages/index/index.wxml";
const MODEL_PATH: &str = "model.ts";
const MODEL: &str = "// 😀 模型\nexport interface User {\n  name: string;\n}\n";
const COMMENT: &str = "Display name.";

/// The frozen `page-basic` template from
/// `backends/typescript/src/scenarios.ts` (`PAGE_BASIC_WXML`).
const PAGE_BASIC_WXML: &str = r#"<view class="page">
  <text class="title">{{ title }}</text>
  <text class="name">{{ user.name }}</text>
  <text class="typo">{{ user.nmae }}</text>
</view>
"#;

/// The frozen `backends/typescript/samples/page-basic/index.ts` script.
const PAGE_BASIC_SCRIPT: &str = r"Page({
  data: {
    /** 页面标题 */
    title: '首页',
    /** 当前用户 */
    user: {
      /** 用户显示名称 */
      name: 'Ada',
      /** 用户年龄 */
      age: 36,
    },
  },
  onShow() {
    this.setData({ title: '欢迎回来' });
  },
});
";

/// The same script with the unsaved edit from
/// `backends/typescript/test/analysis.test.ts:161`: a new `JSDoc` and
/// `name: 42` replacing the on-disk `name: 'Ada'`.
const PAGE_BASIC_UNSAVED_SCRIPT: &str = r"Page({
  data: {
    /** 页面标题 */
    title: '首页',
    /** 当前用户 */
    user: {
      /** 未保存的名称 */
      name: 42,
      /** 用户年龄 */
      age: 36,
    },
  },
  onShow() {
    this.setData({ title: '欢迎回来' });
  },
});
";

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

/// Builds one `page-basic` fragment with the frozen WXML byte range.
///
/// `fragment()` locates the expression with `str::find`, which is ambiguous for
/// `title` because `class="title"` precedes `{{ title }}`; the page-basic
/// cases therefore pin the absolute ranges explicitly.
fn page_basic_fragment(
    id: &str,
    expression: &str,
    start: u32,
    end: u32,
    target: Option<&str>,
) -> CheckFragment {
    CheckFragment {
        fragment_id: id.to_owned(),
        expr_text: expression.to_owned(),
        target: target.map(str::to_owned),
        bindings: Vec::new(),
        guards: Vec::new(),
        wxml_range: SourceRange::from_bounds(start, end).expect("fragment range"),
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

fn component_attribute(wxml: &str, name: &str, expression: &str) -> ComponentAttribute {
    let start = offset_of(wxml, expression);
    let range = SourceRange::from_bounds(
        start,
        start + u32::try_from(expression.len()).expect("expression length"),
    )
    .expect("attribute range");
    ComponentAttribute {
        name: name.to_owned(),
        value_kind: AttributeValueKind::Mustache,
        expr_text: Some(expression.to_owned()),
        expr_range: Some(range),
        attribute_range: range,
    }
}

fn component_usage(
    tag: &str,
    component: &str,
    attributes: Vec<ComponentAttribute>,
) -> ComponentUsage {
    ComponentUsage {
        tag: tag.to_owned(),
        component_file: Some(FileId::new(project(), component).expect("component file")),
        attributes,
    }
}

/// Builds a page view with one mounted `expression` and one custom component.
fn prepared_component_view(
    wxml: &str,
    script: &str,
    expression: &str,
    component_path: &str,
    component_script: &str,
    attributes: Vec<ComponentAttribute>,
) -> PreparedView {
    let mut view = prepared_view_with_files(
        wxml,
        expression,
        vec![
            prepared_file(SCRIPT_PATH, script),
            prepared_file(component_path, component_script),
        ],
    );
    view.components = Arc::from(vec![component_usage("card", component_path, attributes)]);
    view
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

    fn prepare_script(
        &self,
        _file: &FileId,
    ) -> Result<PreparedView, super::input::ScriptViewError> {
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
    file_name: Option<&'static str>,
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

/// Auto-import preparation outcome for the fake session.
#[derive(Clone, Debug, Default)]
enum FakeAutoImportPreparation {
    /// The session does not model preparation (historical behavior).
    #[default]
    NotSupported,
    Prepared,
    Unavailable(&'static str),
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
    auto_imports: FakeAutoImportPreparation,
    definition: FakeDefinitionOutcome,
    document_symbols: FakeDocumentSymbolOutcome,
    update_failure: Option<FakeFailure>,
    start_failure: Option<FakeFailure>,
    /// Reports the fake engine process as dead for the one-shot retry path.
    dead: bool,
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
            auto_imports: FakeAutoImportPreparation::default(),
            definition: FakeDefinitionOutcome::CheckedEmpty,
            document_symbols: FakeDocumentSymbolOutcome::CheckedEmpty,
            update_failure: None,
            start_failure: None,
            dead: false,
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
    semantic_paths: Vec<String>,
    syntactic_paths: Vec<String>,
    type_calls: Vec<(String, Utf16Position)>,
    hover_evidence_calls: Vec<(String, Utf16Position)>,
    hover_documentation_calls: usize,
    completion_calls: Vec<(String, Utf16Position)>,
    completion_options: Vec<CompletionQueryOptions>,
    prepare_auto_import_calls: Vec<String>,
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
        if let Some(failure) = &self.behavior.start_failure {
            return Err(failure.native());
        }
        Ok(Box::new(FakeSession {
            behavior: self.behavior.clone(),
            log: Arc::clone(&self.log),
            input: None,
        }))
    }
}

/// A session factory that returns a different behavior for each start.
struct SequencedFactory {
    behaviors: Mutex<VecDeque<FakeBehavior>>,
    log: Arc<Mutex<FakeLog>>,
}

impl SequencedFactory {
    fn new(behaviors: Vec<FakeBehavior>) -> (Self, Arc<Mutex<FakeLog>>) {
        let log = Arc::new(Mutex::new(FakeLog::default()));
        (
            Self {
                behaviors: Mutex::new(behaviors.into()),
                log: Arc::clone(&log),
            },
            log,
        )
    }
}

impl super::session::SessionFactory for SequencedFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        self.log.lock().expect("log").starts += 1;
        let behavior = self
            .behaviors
            .lock()
            .expect("behaviors")
            .pop_front()
            .unwrap_or_default();
        Ok(Box::new(FakeSession {
            behavior,
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
                            file_name: spec.file_name.map(str::to_owned),
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
        {
            let mut log = self.log.lock().expect("log");
            log.semantic_calls += 1;
            log.semantic_paths.push(path.to_owned());
        }
        self.raw_diagnostics(path, &self.behavior.semantic)
    }

    fn syntactic_diagnostics(&mut self, path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        {
            let mut log = self.log.lock().expect("log");
            log.syntactic_calls += 1;
            log.syntactic_paths.push(path.to_owned());
        }
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
        self.completions_with_options(path, position, CompletionQueryOptions::default())
    }

    fn completions_with_options(
        &mut self,
        path: &str,
        position: Utf16Position,
        options: CompletionQueryOptions,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        let mut log = self.log.lock().expect("log");
        log.completion_calls.push((path.to_owned(), position));
        log.completion_options.push(options);
        drop(log);
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

    fn prepare_auto_imports(&mut self, path: &str) -> Result<AutoImportPreparation, NativeError> {
        self.log
            .lock()
            .expect("log")
            .prepare_auto_import_calls
            .push(path.to_owned());
        match &self.behavior.auto_imports {
            FakeAutoImportPreparation::NotSupported => Ok(AutoImportPreparation::NotSupported),
            FakeAutoImportPreparation::Prepared => Ok(AutoImportPreparation::Prepared),
            FakeAutoImportPreparation::Unavailable(reason) => {
                Ok(AutoImportPreparation::Unavailable { reason })
            }
            FakeAutoImportPreparation::Failure(failure) => Err(failure.native()),
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

    fn is_alive(&mut self) -> bool {
        !self.behavior.dead
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

fn script_context(adapter: &TsgoBackendAdapter, path: &str, text: &str) -> RequestContext {
    let file = FileId::new(project(), path).expect("script file");
    RequestContext::new(
        ProjectRevision::new(project(), 1),
        ViewRevision::new(1),
        adapter.epoch(),
        Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(2),
            text,
        )),
    )
    .expect("script context")
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
fn hover_declaration_gap_redacts_asset_and_host_paths() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Evidence(FakeHoverEvidence {
            type_text: Some("string"),
            quality: NativeTypeQuality::Known,
            documentation: None,
            gaps: vec![HoverGap::ExternalDeclaration {
                path: Some("/home/alice/.wxml/engine/lib/lib.d.ts".to_owned()),
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
            reason: "a declaration is outside the project snapshot".to_owned(),
        }
    );
    let reason = result.status.reason().expect("partial reason");
    assert!(
        !reason.contains('/') && !reason.contains("alice") && !reason.contains("engine"),
        "hover reason leaked a host or asset path: {reason}"
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
            reason: "a declaration is outside the project snapshot; \
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
            reason: "a declaration is outside the project snapshot".to_owned(),
        },
        "```ts\nstring\n```",
        None,
        true,
    );
    assert_eq!(
        status,
        CapabilityStatus::Partial {
            reason: "a declaration is outside the project snapshot; \
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
            file_name: None,
        }]),
        syntactic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "user.name",
            code: 1109,
            category: 1,
            message: "syntax",
            file_name: None,
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
            file_name: None,
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
                    ..RawCompletionEntry::default()
                },
                RawCompletionEntry {
                    name: "age".to_owned(),
                    kind: 5,
                    sort_text: "12".to_owned(),
                    detail: None,
                    label_details: None,
                    ..RawCompletionEntry::default()
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
fn completion_prepares_auto_imports_and_requests_symbols_without_fabricating_edits() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "User".to_owned(),
                kind: 7,
                sort_text: "16".to_owned(),
                insert_text: Some("User".to_owned()),
                filter_text: Some("User".to_owned()),
                detail: Some("interface User".to_owned()),
                label_details: Some(RawCompletionLabelDetails {
                    detail: None,
                    description: Some("./model".to_owned()),
                }),
                symbol: Some(wxml_backend_tsgo::native::wire::RawSymbol {
                    name: "User".to_owned(),
                    flags: 5_242_880,
                    declarations: vec!["22.262./work/model.ts".to_owned()],
                    value_declaration: Some("22.262./work/model.ts".to_owned()),
                    reference: None,
                }),
            }],
            incomplete: false,
        },
        auto_imports: FakeAutoImportPreparation::Prepared,
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
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

    let log = log.lock().expect("log");
    assert_eq!(log.prepare_auto_import_calls, vec![SCRIPT_PATH.to_owned()]);
    assert_eq!(
        log.completion_options,
        vec![CompletionQueryOptions {
            include_symbol: true
        }],
        "a prepared snapshot must request symbol evidence"
    );
    drop(log);

    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    assert_eq!(item.label, "User");
    assert_eq!(item.insert_text, "User");
    assert_eq!(
        item.insert_text_format,
        wxml_types::InsertTextFormat::PlainText
    );
    assert_eq!(item.documentation, None);
    // No import statement and no second edit are representable or invented.
    assert!(!item.insert_text.contains("import "));
    assert_eq!(item.filter_text.as_deref(), Some("User"));
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("auto-import origin"), "{reason}");
    assert!(
        !reason.contains("prepareAutoImports"),
        "a prepared query must not claim auto-imports are unprepared: {reason}"
    );
    assert!(reason.contains("documentation"), "{reason}");
}

#[test]
fn completion_without_preparation_keeps_auto_imports_unavailable() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "name".to_owned(),
                kind: 5,
                sort_text: "11".to_owned(),
                ..RawCompletionEntry::default()
            }],
            incomplete: false,
        },
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

    let log = log.lock().expect("log");
    assert_eq!(log.prepare_auto_import_calls, vec![SCRIPT_PATH.to_owned()]);
    assert_eq!(
        log.completion_options,
        vec![CompletionQueryOptions {
            include_symbol: false
        }],
        "an unprepared snapshot must not request symbols"
    );
    drop(log);

    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("prepareAutoImports"), "{reason}");
    assert_eq!(result.items[0].documentation, None);
}

#[test]
fn completion_prepare_failure_is_reported_and_never_supported_empty() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::EmptyList,
        auto_imports: FakeAutoImportPreparation::Unavailable(
            "the pinned engine rejected auto-import preparation for this file",
        ),
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

    let log = log.lock().expect("log");
    assert_eq!(
        log.completion_options,
        vec![CompletionQueryOptions {
            include_symbol: false
        }]
    );
    drop(log);

    assert!(result.items.is_empty());
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "a failed preparation must not be a supported empty answer"
    );
    let reason = result.status.reason().expect("partial reason");
    assert!(
        reason.contains("rejected auto-import preparation"),
        "{reason}"
    );
}

#[test]
fn completion_prepare_error_is_a_backend_error_not_empty_supported() {
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::EmptyList,
        auto_imports: FakeAutoImportPreparation::Failure(FakeFailure::Timeout),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );
    let error = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect_err("a hard prepare failure must not fabricate a completion result");
    assert!(matches!(error, BackendError::Timeout));
    let log = log.lock().expect("log");
    assert!(log.completion_calls.is_empty());
    drop(log);
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
                ..RawCompletionEntry::default()
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
                ..RawCompletionEntry::default()
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
                ..RawCompletionEntry::default()
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
                ..RawCompletionEntry::default()
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
            ..RawCompletionEntry::default()
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
fn symbols_answer_data_roots_without_starting_a_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .symbols(SymbolRequest {
            context: context(&adapter, WXML),
            subject: script_file(),
            query: SymbolQuery::DataRoots,
        })
        .expect("symbols");
    assert!(result.complete, "{:?}", result.reason);
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert!(result.fields.iter().any(|field| field.name == "user"));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn symbols_answer_a_nested_data_path_without_starting_a_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .symbols(SymbolRequest {
            context: context(&adapter, WXML),
            subject: script_file(),
            query: SymbolQuery::DataPath {
                path: vec!["user".to_owned(), "name".to_owned()],
            },
        })
        .expect("symbols");
    assert!(result.complete, "{:?}", result.reason);
    assert_eq!(result.status, CapabilityStatus::Supported);
    let declaration = result.declaration.expect("nested declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("name"));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn symbols_create_a_missing_handler_insertion_without_starting_a_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .symbols(SymbolRequest {
            context: context(&adapter, WXML),
            subject: script_file(),
            query: SymbolQuery::EventHandler {
                name: "onSave".to_owned(),
            },
        })
        .expect("symbols");
    assert!(result.complete, "{:?}", result.reason);
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert!(result.declaration.is_none());
    let insertion = result.insertion.expect("handler insertion");
    assert!(insertion.text.contains("onSave()"), "{}", insertion.text);
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn symbols_reject_a_subject_outside_the_prepared_view() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let other = FileId::new(project(), "other.ts").expect("other script");
    let result = adapter
        .symbols(SymbolRequest {
            context: context(&adapter, WXML),
            subject: other,
            query: SymbolQuery::DataRoots,
        })
        .expect("symbols");
    assert!(!result.complete);
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn symbols_with_an_incomplete_source_are_partial() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .symbols(SymbolRequest {
            context: context(&adapter, WXML),
            subject: script_file(),
            query: SymbolQuery::DataRoots,
        })
        .expect("symbols");
    assert!(!result.complete);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert!(result.reason.is_some());
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
fn case_analysis_0315_a_partial_definition_does_not_degrade_later_hover_or_completion() {
    // Oracle: `analysis.test.ts:315` drops one definition target and asserts
    // only `definition` becomes partial; hover and diagnostics stay supported.
    // The adapter keeps no run-wide capability state, so a definition request
    // that drops a target must not leak `Partial` into the next request.
    let wxml = "<view>{{ user.na }}</view>";
    let behavior = FakeBehavior {
        definition: FakeDefinitionOutcome::TargetInScript {
            needle: "__wxml_check_0",
        },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(wxml, SCRIPT, "user.na"))),
        behavior,
    );

    let definition = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na"),
        })
        .expect("definition");
    assert_eq!(definition.locations, Vec::new());
    assert_eq!(
        definition.status,
        CapabilityStatus::Partial {
            reason: "some definition targets could not be represented".to_owned(),
        },
        "a generated-only target is dropped and reported on this request only"
    );

    let hover = adapter
        .hover(HoverRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na"),
        })
        .expect("hover");
    assert_eq!(hover.status, CapabilityStatus::Supported);
    assert_eq!(hover.quality, wxml_types::TypeQuality::Known);
    assert_eq!(hover.contents, "```ts\nstring\n```");
    let range = hover.range.expect("hover range");
    assert_eq!(range.start(), offset_of(wxml, "na"));
    assert_eq!(range.end(), offset_of(wxml, "na") + 2);

    let completion = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na") + 2,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert_eq!(completion.status, CapabilityStatus::Supported);
    assert!(completion.items.is_empty());
    assert!(!completion.truncated);

    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("diagnostics");
    assert_eq!(diagnostics.status, CapabilityStatus::Supported);
    assert!(diagnostics.diagnostics.is_empty());

    let second = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "na"),
        })
        .expect("second definition");
    assert_eq!(second.status, definition.status);
    assert_eq!(second.locations, Vec::new());

    let log = log.lock().expect("log");
    assert_eq!(
        log.starts, 1,
        "the partial definition must not restart or replace the session"
    );
    assert_eq!(log.shutdowns, 0);
    assert_eq!(log.definition_calls.len(), 2);
    assert_eq!(log.hover_evidence_calls.len(), 1);
    assert_eq!(log.completion_calls.len(), 1);
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

#[test]
fn a_non_owner_overlay_content_change_updates_the_same_session() {
    let initial_files = vec![
        prepared_file(SCRIPT_PATH, SCRIPT),
        prepared_file(MODEL_PATH, MODEL),
    ];
    let views = Arc::new(MutableViews::new(prepared_view_with_files(
        WXML,
        "user.name",
        initial_files,
    )));
    let (adapter, log) = adapter_with(
        Arc::clone(&views) as Arc<dyn ViewProvider>,
        FakeBehavior::default(),
    );
    let first = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("first hover");
    assert_eq!(first.contents, "```ts\nstring\n```");
    assert_eq!(log.lock().expect("log").starts, 1);
    assert_eq!(log.lock().expect("log").updates, 0);

    let overlay = "// 中文😀\r\nexport interface User {\r\n  name: \"OVERLAY\";\r\n}\r\n";
    views.replace(prepared_view_with_files(
        WXML,
        "user.name",
        vec![
            prepared_file(SCRIPT_PATH, SCRIPT),
            prepared_file(MODEL_PATH, overlay),
        ],
    ));
    let second = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("second hover");
    assert_eq!(second.contents, "```ts\nstring\n```");
    {
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 1, "a non-owner change must not restart");
        assert_eq!(log.updates, 1, "a non-owner change must update in place");
    }
    assert_eq!(
        log.lock()
            .expect("log")
            .updated_inputs
            .last()
            .expect("updated input")
            .file(MODEL_PATH)
            .expect("model input")
            .text(),
        overlay,
        "the updated native input must carry the non-owner overlay text"
    );

    adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("third hover");
    assert_eq!(
        log.lock().expect("log").updates,
        1,
        "an unchanged non-owner input must reuse the open snapshot"
    );
}

#[test]
fn multiple_non_owner_overlays_update_independently() {
    const EXTRA_PATH: &str = "extra.ts";
    const EXTRA_DISK: &str = "export interface Extra { count: number }\n";
    const EXTRA_OVERLAY: &str = "export interface Extra { count: 7 }\n";
    let files = |model: &str, extra: &str| {
        vec![
            prepared_file(SCRIPT_PATH, SCRIPT),
            prepared_file(MODEL_PATH, model),
            prepared_file(EXTRA_PATH, extra),
        ]
    };
    let views = Arc::new(MutableViews::new(prepared_view_with_files(
        WXML,
        "user.name",
        files(MODEL, EXTRA_DISK),
    )));
    let (adapter, log) = adapter_with(
        Arc::clone(&views) as Arc<dyn ViewProvider>,
        FakeBehavior::default(),
    );
    let hover = || {
        adapter
            .hover(HoverRequest {
                context: context(&adapter, WXML),
                offset: offset_of(WXML, "name"),
            })
            .expect("hover")
    };
    hover();

    views.replace(prepared_view_with_files(
        WXML,
        "user.name",
        files("export interface User { name: \"A\" }\n", EXTRA_DISK),
    ));
    hover();
    {
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 1);
        assert_eq!(log.updates, 1);
        let input = log.updated_inputs.last().expect("updated input");
        assert_eq!(input.file(EXTRA_PATH).expect("extra").text(), EXTRA_DISK);
    }

    views.replace(prepared_view_with_files(
        WXML,
        "user.name",
        files("export interface User { name: \"A\" }\n", EXTRA_OVERLAY),
    ));
    hover();
    {
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 1);
        assert_eq!(log.updates, 2);
        let input = log.updated_inputs.last().expect("updated input");
        assert_eq!(
            input.file(MODEL_PATH).expect("model").text(),
            "export interface User { name: \"A\" }\n"
        );
        assert_eq!(input.file(EXTRA_PATH).expect("extra").text(), EXTRA_OVERLAY);
    }

    // Closing the second overlay restores its disk text without touching the
    // first overlay.
    views.replace(prepared_view_with_files(
        WXML,
        "user.name",
        files("export interface User { name: \"A\" }\n", EXTRA_DISK),
    ));
    hover();
    {
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 1);
        assert_eq!(log.updates, 3);
        let input = log.updated_inputs.last().expect("updated input");
        assert_eq!(
            input.file(MODEL_PATH).expect("model").text(),
            "export interface User { name: \"A\" }\n"
        );
        assert_eq!(input.file(EXTRA_PATH).expect("extra").text(), EXTRA_DISK);
    }
}

#[test]
fn a_cjk_crlf_non_owner_overlay_change_updates_and_maps() {
    const WXML_CJK: &str = "中文😀\r\n<view>{{ user.name }}</view>";
    let views = Arc::new(MutableViews::new(prepared_view_with_files(
        WXML_CJK,
        "user.name",
        vec![
            prepared_file(SCRIPT_PATH, SCRIPT),
            prepared_file(MODEL_PATH, MODEL),
        ],
    )));
    let (adapter, log) = adapter_with(
        Arc::clone(&views) as Arc<dyn ViewProvider>,
        FakeBehavior::default(),
    );
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML_CJK),
            offset: offset_of(WXML_CJK, "name"),
        })
        .expect("cjk hover");
    let range = result.range.expect("hover range");
    assert_eq!(range.start(), offset_of(WXML_CJK, "name"));
    assert_eq!(
        range.end(),
        offset_of(WXML_CJK, "name") + u32::try_from("name".len()).expect("length")
    );

    views.replace(prepared_view_with_files(
        WXML_CJK,
        "user.name",
        vec![
            prepared_file(SCRIPT_PATH, SCRIPT),
            prepared_file(
                MODEL_PATH,
                "// 😀\r\nexport interface User {\r\n  name: \"OVERLAY\";\r\n}\r\n",
            ),
        ],
    ));
    adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML_CJK),
            offset: offset_of(WXML_CJK, "name"),
        })
        .expect("cjk overlay hover");
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 1);
    assert_eq!(log.updates, 1);
}

#[test]
fn a_dead_session_is_restarted_once_for_the_current_request() {
    let behaviors = vec![
        FakeBehavior {
            hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Exited),
            dead: true,
            ..FakeBehavior::default()
        },
        FakeBehavior::default(),
    ];
    let (factory, log) = SequencedFactory::new(behaviors);
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        Arc::new(factory),
    );
    let before = adapter.epoch();
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("the one-shot retry must answer the current request");
    assert_eq!(result.contents, "```ts\nstring\n```");
    assert_eq!(
        adapter.epoch(),
        before,
        "a successful retry must not invalidate the stamp"
    );
    assert_eq!(log.lock().expect("log").starts, 2);
    assert_eq!(log.lock().expect("log").shutdowns, 1);
}

#[test]
fn a_dead_session_retry_is_bounded_to_one_attempt() {
    let behaviors = vec![
        FakeBehavior {
            hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Exited),
            dead: true,
            ..FakeBehavior::default()
        },
        FakeBehavior {
            hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Exited),
            dead: true,
            ..FakeBehavior::default()
        },
    ];
    let (factory, log) = SequencedFactory::new(behaviors);
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        Arc::new(factory),
    );
    let before = adapter.epoch();
    let error = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect_err("two dead sessions must fail");
    assert!(matches!(error, BackendError::Unavailable(_)));
    assert_eq!(log.lock().expect("log").starts, 2);
    assert_eq!(log.lock().expect("log").shutdowns, 2);
    assert!(adapter.epoch().get() > before.get());
}

#[test]
fn an_unsupported_result_is_not_retried_as_a_new_query() {
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Unsupported("hover")),
        dead: true,
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
        .expect("unsupported hover is a checked result");
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 1);
    assert_eq!(log.lock().expect("log").shutdowns, 0);
}

#[test]
fn an_external_invalidation_is_not_retried() {
    let gate = Arc::new(SlowGate::default());
    let behavior = FakeBehavior {
        hover_outcome: FakeHoverOutcome::Failure(FakeFailure::Exited),
        dead: true,
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
    adapter.invalidate();
    gate.release();
    let result = handle.join().expect("hover thread");
    assert!(matches!(result, Err(BackendError::Cancelled)));
    assert_eq!(log.lock().expect("log").starts, 1);
    assert_eq!(log.lock().expect("log").shutdowns, 1);
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
    assert_eq!(result.skipped, 1);
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
    assert_eq!(result.skipped, 1);
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
    assert_eq!(result.skipped, 1);
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

/// A non-literal `Page(options)` registration: the shadow has no owner object
/// to analyze, so every template capability must answer `Unsupported` without
/// starting a session. This is the E2E-21 non-literal status class.
#[test]
fn a_non_literal_page_registration_is_unsupported_and_starts_no_session() {
    const NONLITERAL_SCRIPT: &str = "const options = {\n  data: { user: { name: \"Ada\" } },\n  onTap() {},\n};\nPage(options);\n";
    let view = prepared_view(WXML, NONLITERAL_SCRIPT, "user.name");
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());

    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics must not return Err");
    assert!(
        diagnostics.diagnostics.is_empty(),
        "a statically unanalyzable owner must not fabricate diagnostics: {:?}",
        diagnostics.diagnostics
    );
    let CapabilityStatus::Unsupported { reason } = &diagnostics.status else {
        panic!(
            "non-literal registration must be Unsupported, got {:?}",
            diagnostics.status
        );
    };
    assert!(reason.contains("registration"), "{reason}");
    assert!(reason.contains("object literal"), "{reason}");
    assert!(
        !reason.contains('/'),
        "the unsupported reason must be path-free: {reason}"
    );

    let hover = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("hover must answer, not fail");
    assert!(
        matches!(hover.status, CapabilityStatus::Unsupported { .. }),
        "{:?}",
        hover.status
    );
    assert!(hover.contents.is_empty());

    let definition = adapter
        .definition(DefinitionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("definition must answer, not fail");
    assert!(
        matches!(definition.status, CapabilityStatus::Unsupported { .. }),
        "{:?}",
        definition.status
    );
    assert!(definition.locations.is_empty());

    let completion = adapter
        .completion(CompletionRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion must answer, not fail");
    assert!(
        matches!(completion.status, CapabilityStatus::Unsupported { .. }),
        "{:?}",
        completion.status
    );
    assert!(completion.items.is_empty());

    let symbols = adapter
        .document_symbols(DocumentSymbolsRequest {
            context: context(&adapter, WXML),
            subject: script_file(),
        })
        .expect("document symbols must answer, not fail");
    assert!(
        matches!(symbols.status, CapabilityStatus::Unsupported { .. }),
        "{:?}",
        symbols.status
    );
    assert!(symbols.symbols.is_empty());

    assert_eq!(
        log.lock().expect("log").starts,
        0,
        "a static owner limit must not start a native session"
    );
}

/// A malformed owner script is an invalid input, not an unsupported language
/// form: it must stay `Unavailable` so real planning failures are not
/// downgraded.
#[test]
fn a_malformed_owner_script_stays_unavailable() {
    const MALFORMED_SCRIPT: &str = "Page({ data: { user: { name: \"Ada\" } }";
    let view = prepared_view(WXML, MALFORMED_SCRIPT, "user.name");
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics must not return Err");
    assert!(
        matches!(diagnostics.status, CapabilityStatus::Unavailable { .. }),
        "malformed input must stay Unavailable, got {:?}",
        diagnostics.status
    );
    assert_eq!(log.lock().expect("log").starts, 0);
}

/// Reproduces the frozen E2E-21 fixture: `pages/nonliteral/nonliteral.wxml`
/// is owned by `Page(options)`. The real project host prepares the view and
/// the classification happens before any native session exists.
#[test]
fn the_e2e21_non_literal_page_fixture_is_unsupported_not_unavailable() {
    use crate::project_host::{ProjectHost, ProjectState};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures/events-basic")
        .canonicalize()
        .expect("events-basic fixture root");
    let host = Arc::new(ProjectState::new());
    host.open_root(Some(&format!("file://{}", root.display())))
        .expect("open the events-basic fixture");

    let (factory, log) = FakeFactory::new(FakeBehavior::default());
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(super::input::HostViewProvider::new(host.clone())),
        Arc::new(factory),
    );
    let context_for = |relative: &str| {
        let path = project_path(relative);
        let file = ProjectHost::file_id(host.as_ref(), &path).expect("fixture file id");
        let target = ProjectHost::view_for(host.as_ref(), &path).expect("fixture view");
        let text = std::fs::read_to_string(root.join(relative)).expect("fixture template");
        RequestContext::new(
            ProjectHost::project_revision(host.as_ref()),
            target.view_revision,
            adapter.epoch(),
            Arc::new(SourceSnapshot::new(
                file,
                SourceRevision::new(1),
                SnapshotId::new(1),
                text,
            )),
        )
        .expect("fixture request context")
    };

    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context_for("pages/nonliteral/nonliteral.wxml"),
        })
        .expect("diagnostics must not return Err");
    assert!(
        result.diagnostics.is_empty(),
        "the non-literal fixture must not produce noise: {:?}",
        result.diagnostics
    );
    let CapabilityStatus::Unsupported { reason } = &result.status else {
        panic!(
            "the non-literal fixture must be Unsupported, got {:?}",
            result.status
        );
    };
    assert!(reason.contains("object literal"), "{reason}");
    assert!(
        !reason.contains('/'),
        "the unsupported reason must be path-free: {reason}"
    );
    assert_eq!(
        log.lock().expect("log").starts,
        0,
        "the fixture classification must not start a native session"
    );

    // The multi-registration fixture keeps its `Partial` class; only the
    // non-literal owner shape is `Unsupported`.
    let multi = adapter
        .diagnostics(DiagnosticsRequest {
            context: context_for("pages/multi/multi.wxml"),
        })
        .expect("multi diagnostics must not return Err");
    assert!(
        multi.diagnostics.is_empty(),
        "the multi fixture must not produce noise: {:?}",
        multi.diagnostics
    );
    assert!(
        matches!(multi.status, CapabilityStatus::Partial { .. }),
        "multi registrations must stay Partial, got {:?}",
        multi.status
    );
}

/// `HostOnly` must keep static template and script facts and answer every
/// session-backed query with the stable `Unavailable` reason.
///
/// This is the adapter-level contract behind `wxml-lsp --stdio` when the
/// executable is not inside an installed package: no engine, no type assets and
/// no legacy Node environment, but no fabricated type success either.
#[test]
fn host_only_serves_static_facts_and_reports_type_queries_unavailable() {
    use crate::project_host::{ProjectHost, ProjectState};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures/events-basic")
        .canonicalize()
        .expect("events-basic fixture root");
    let host = Arc::new(ProjectState::new());
    host.open_root(Some(&format!("file://{}", root.display())))
        .expect("open the events-basic fixture");
    let adapter = TsgoBackendAdapter::without_engine(host.clone());

    let capabilities = adapter.capabilities();
    for status in [
        &capabilities.hover,
        &capabilities.diagnostics,
        &capabilities.definition,
        &capabilities.completion,
    ] {
        assert!(
            matches!(status, CapabilityStatus::Partial { .. }),
            "HostOnly must advertise a partial, never complete, capability: {status:?}"
        );
        assert!(
            status
                .reason()
                .is_some_and(|reason| reason.contains(HOST_ONLY_REASON)),
            "HostOnly capability reason must name the missing engine: {status:?}"
        );
    }

    let relative = "pages/index/index.wxml";
    let path = project_path(relative);
    let file = ProjectHost::file_id(host.as_ref(), &path).expect("fixture file id");
    let target = ProjectHost::view_for(host.as_ref(), &path).expect("fixture view");
    let template = std::fs::read_to_string(root.join(relative)).expect("fixture template");
    let context_for = |text: &str, snapshot: u64| {
        RequestContext::new(
            ProjectHost::project_revision(host.as_ref()),
            target.view_revision,
            adapter.epoch(),
            Arc::new(SourceSnapshot::new(
                file.clone(),
                SourceRevision::new(1),
                SnapshotId::new(snapshot),
                text,
            )),
        )
        .expect("request context")
    };

    let hover = adapter
        .hover(HoverRequest {
            context: context_for(&template, 1),
            offset: offset_of(&template, "label"),
        })
        .expect_err("HostOnly hover must not fabricate a type");
    assert!(
        matches!(&hover, BackendError::Unavailable(reason) if reason == HOST_ONLY_REASON),
        "{hover:?}"
    );

    let definition = adapter
        .definition(DefinitionRequest {
            context: context_for(&template, 2),
            offset: offset_of(&template, "label"),
        })
        .expect_err("HostOnly definition must not fabricate a location");
    assert!(
        matches!(&definition, BackendError::Unavailable(reason) if reason == HOST_ONLY_REASON),
        "{definition:?}"
    );

    let diagnostics = adapter
        .diagnostics(DiagnosticsRequest {
            context: context_for(&template, 3),
        })
        .expect("diagnostics never return Err");
    assert!(
        matches!(diagnostics.status, CapabilityStatus::Unavailable { .. }),
        "HostOnly diagnostics must not claim a completed type check: {:?}",
        diagnostics.status
    );

    let malformed = "<view>";
    let syntax = adapter
        .diagnostics(DiagnosticsRequest {
            context: context_for(malformed, 4),
        })
        .expect("syntax diagnostics never return Err");
    assert!(
        matches!(syntax.status, CapabilityStatus::Unavailable { .. }),
        "HostOnly syntax diagnostics must stay Unavailable for the type part: {:?}",
        syntax.status
    );
    assert!(
        !syntax.diagnostics.is_empty(),
        "template syntax diagnostics must keep working in HostOnly"
    );

    let event_text = "<view bindtap=\"on\" />";
    let event_completion = adapter
        .completion(CompletionRequest {
            context: context_for(event_text, 5),
            offset: offset_of(event_text, "on") + 2,
            kind: CompletionKind::EventHandler,
        })
        .expect("event completion does not open a session");
    assert!(
        event_completion
            .items
            .iter()
            .any(|item| item.label == "onShorthandTap"),
        "script method facts must keep working: {:?}",
        event_completion.items
    );

    let script_path = project_path("pages/index/index.ts");
    let script_file =
        ProjectHost::file_id(host.as_ref(), &script_path).expect("fixture script file id");
    let symbols = adapter
        .symbols(SymbolRequest {
            context: context_for(&template, 6),
            subject: script_file,
            query: SymbolQuery::DataRoots,
        })
        .expect("script symbol facts do not open a session");
    assert!(
        symbols.fields.iter().any(|field| field.name == "label"),
        "script data roots must keep working: {:?}",
        symbols.fields
    );
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
fn case_analysis_0139_declared_property_types_survive_an_unresolved_default_value() {
    const WXML: &str = "<view>{{ approvalStatus }}</view>";
    const SCRIPT: &str = "import { MissingEnum } from './missing';\nComponent({\n  properties: {\n    approvalStatus: { type: Number, value: MissingEnum.Preview },\n  },\n  data: {},\n  methods: {},\n});\n";
    let mut view = prepared_view_with_files(
        WXML,
        "approvalStatus",
        vec![prepared_file(SCRIPT_PATH, SCRIPT)],
    );
    view.target.as_mut().expect("target").owner = RootKind::Component;
    // The native answer says `string`; only the declaration-derived property
    // `type: Number` may produce the oracle's `number` hover.
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Known("string".to_owned()),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "approvalStatus"),
        })
        .expect("hover");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert_eq!(result.contents, "```ts\nnumber\n```");
    let start = offset_of(WXML, "approvalStatus");
    let range = result.range.expect("hover range");
    assert_eq!(
        range.range(),
        wxml_text::TextRange::new(start, start + 14).expect("range")
    );

    let log = log.lock().expect("log");
    let input = log.opened_inputs.first().expect("opened input");
    assert!(
        input.file(SCRIPT_PATH).is_some(),
        "the oracle fixture's owning script reaches the session"
    );
    let derived = input.file(SCRIPT_PATH).expect("script").text();
    assert!(
        derived.contains("__wxml_props_0.approvalStatus"),
        "a declared property root is mounted behind the props prefix: {derived}"
    );
}

#[test]
fn case_analysis_0174_hover_never_reads_the_project_root_tsconfig() {
    const WXML: &str = "<view>{{ title }}</view>";
    const SCRIPT: &str = "Page({ data: { title: \"首页\" } });\n";
    const PROJECT_ROOT: &str = "/nonexistent/wxml-analysis-0174-project-root";
    assert!(
        !std::path::Path::new(PROJECT_ROOT).exists(),
        "the fixture root must not exist on disk"
    );
    let mut view = prepared_view(WXML, SCRIPT, "title");
    view.project_root = PathBuf::from(PROJECT_ROOT);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "title"),
        })
        .expect("hover with a nonexistent project root");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.quality, wxml_types::TypeQuality::Known);
    assert_eq!(result.contents, "```ts\nstring\n```");
    assert_eq!(
        result.range.expect("range").start(),
        offset_of(WXML, "title")
    );

    let log = log.lock().expect("log");
    let input = log.opened_inputs.first().expect("opened input");
    assert_eq!(input.root().as_str(), PROJECT_ROOT);
    assert_eq!(
        input
            .files()
            .iter()
            .map(wxml_backend_tsgo::input::SourceFile::path)
            .collect::<Vec<_>>(),
        vec![SCRIPT_PATH],
        "only the in-memory snapshot reaches the session; no project-root file is read"
    );
}

#[test]
fn case_analysis_0161_unsaved_script_text_reaches_the_session_without_a_disk_read() {
    // Oracle: `analysis.test.ts:161` rewrites the request's script text before
    // analysis. The native architecture hands the host's overlay-first prepared
    // text to the session, so this test proves the input half exactly: the
    // unsaved JSDoc/`name: 42` reach the engine input, the project root does not
    // exist, and only the in-memory snapshot is submitted. The oracle's
    // `hover.typeOnly`/`hover.documentation` values are checker products and are
    // deliberately not asserted here (see the wave-10 conditional plan).
    const PROJECT_ROOT: &str = "/nonexistent/wxml-analysis-0161-project-root";
    assert!(
        !std::path::Path::new(PROJECT_ROOT).exists(),
        "the fixture root must not exist on disk"
    );
    let mut view = prepared_view(PAGE_BASIC_WXML, PAGE_BASIC_UNSAVED_SCRIPT, "user.name");
    view.project_root = PathBuf::from(PROJECT_ROOT);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let name = offset_of(PAGE_BASIC_WXML, "user.name")
        + u32::try_from("user.".len()).expect("prefix length");
    let hover = adapter
        .hover(HoverRequest {
            context: context(&adapter, PAGE_BASIC_WXML),
            offset: name,
        })
        .expect("hover with unsaved script text");
    let range = hover.range.expect("hover range");
    assert_eq!(range.start(), name);
    assert_eq!(range.end(), name + 4);

    let log = log.lock().expect("log");
    let input = log.opened_inputs.first().expect("opened input");
    assert_eq!(input.root().as_str(), PROJECT_ROOT);
    let text = input.file(SCRIPT_PATH).expect("owner script").text();
    assert!(text.contains("/** 未保存的名称 */"), "{text}");
    assert!(text.contains("name: 42"), "{text}");
    assert!(
        !text.contains("用户显示名称"),
        "the on-disk JSDoc must not reach the engine: {text}"
    );
    assert_eq!(
        input
            .files()
            .iter()
            .map(wxml_backend_tsgo::input::SourceFile::path)
            .collect::<Vec<_>>(),
        vec![SCRIPT_PATH],
        "only the in-memory snapshot reaches the session; no project-root file is read"
    );
}

#[test]
fn case_analysis_0184_page_basic_typo_maps_to_wxml_134_138() {
    // Oracle: `analysis.test.ts:184` asserts only the published `nmae` WXML
    // byte range. The fake session supplies the diagnostic as an arrange input;
    // the published range is computed by the real shadow mapping pipeline.
    let fragments = vec![
        page_basic_fragment("title", "title", 35, 40, Some("title")),
        page_basic_fragment("user.name", "user.name", 85, 94, Some("name")),
        page_basic_fragment("user.nmae", "user.nmae", 129, 138, Some("nmae")),
    ];
    let view = prepared_view_with_fragments(
        PAGE_BASIC_WXML,
        vec![prepared_file(SCRIPT_PATH, PAGE_BASIC_SCRIPT)],
        fragments,
    );
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "nmae",
            code: 2339,
            category: 1,
            message: "Property 'nmae' does not exist.",
            file_name: None,
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, PAGE_BASIC_WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.range.start(), 134);
    assert_eq!(diagnostic.range.end(), 138);
}

#[test]
fn case_analysis_0184_wider_spans_map_to_the_expression_range() {
    // Rust-side control, not an oracle assertion. `user.nmae` has no synthetic
    // gap inside the expression, so a checker span covering the whole mounted
    // expression is an exact projection of `user.nmae` (`129..138`) and stays
    // `Supported`; only a span on the `nmae` token publishes `134..138`.
    let fragments = vec![
        page_basic_fragment("title", "title", 35, 40, Some("title")),
        page_basic_fragment("user.name", "user.name", 85, 94, Some("name")),
        page_basic_fragment("user.nmae", "user.nmae", 129, 138, Some("nmae")),
    ];
    let view = prepared_view_with_fragments(
        PAGE_BASIC_WXML,
        vec![prepared_file(SCRIPT_PATH, PAGE_BASIC_SCRIPT)],
        fragments,
    );
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "__wxml_root.user.nmae",
            code: 2339,
            category: 1,
            message: "Property 'nmae' does not exist.",
            file_name: None,
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, PAGE_BASIC_WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.range.start(), 129);
    assert_eq!(diagnostic.range.end(), 138);
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
                ..RawCompletionEntry::default()
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
fn completion_in_an_unclosed_expression_completes_the_typed_prefix() {
    let wxml = "<view>{{ user.</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![RawCompletionEntry {
                name: "name".to_owned(),
                kind: 5,
                sort_text: "11".to_owned(),
                ..RawCompletionEntry::default()
            }],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let cursor = offset_of(wxml, "user.") + u32::try_from("user.".len()).expect("length");
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion must not be a silent empty success");
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["name"]
    );
    assert_eq!(result.items[0].replace.start(), cursor);
    assert_eq!(result.items[0].replace.end(), cursor);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(log.lock().expect("log").completion_calls.len(), 1);
}

#[test]
fn completion_in_an_unclosed_partial_member_replaces_only_the_typed_prefix() {
    let wxml = "<view>😀{{ user.na</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![
                RawCompletionEntry {
                    name: "name".to_owned(),
                    kind: 5,
                    sort_text: "11".to_owned(),
                    ..RawCompletionEntry::default()
                },
                RawCompletionEntry {
                    name: "age".to_owned(),
                    kind: 5,
                    sort_text: "12".to_owned(),
                    ..RawCompletionEntry::default()
                },
            ],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let typed = offset_of(wxml, "na");
    let cursor = typed + 2;
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("unclosed partial member completion");
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["name", "age"]
    );
    for item in &result.items {
        assert_eq!(
            item.replace.start(),
            typed,
            "only the typed member prefix `na` is replaced"
        );
        assert_eq!(item.replace.end(), cursor);
    }
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 1);
    let input = log.opened_inputs.first().expect("opened input");
    let derived = input.file(SCRIPT_PATH).expect("shadow script").text();
    assert!(
        derived.contains("__wxml_root.user.__wxml_completion"),
        "the partial prefix is mounted as its bare root: {derived}"
    );
    assert!(
        !derived.contains("user.na"),
        "the incomplete typed suffix must not be mounted: {derived}"
    );
    let needle = "__wxml_root.user.__wxml_completion";
    let query = derived.find(needle).expect("synthetic needle") + "__wxml_root.user.".len();
    let expected = offset_to_position(derived, Utf16Offset::new(utf16_len(&derived[..query])))
        .expect("position");
    assert_eq!(
        log.completion_calls,
        vec![(SCRIPT_PATH.to_owned(), expected)],
        "the native query is the synthetic dot after the data root"
    );
}

#[test]
fn unclosed_partial_member_completion_is_utf16_exact_with_cjk_crlf_and_emoji() {
    let wxml = "<view>\r\n😀{{ 用户.名字\r\n</view>";
    let script = "Page({\n  data: { 用户: { 名字: \"你好\", 年龄: 3 } },\n});\n";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, script)]);
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![
                RawCompletionEntry {
                    name: "名字".to_owned(),
                    kind: 5,
                    sort_text: "11".to_owned(),
                    ..RawCompletionEntry::default()
                },
                RawCompletionEntry {
                    name: "年龄".to_owned(),
                    kind: 5,
                    sort_text: "12".to_owned(),
                    ..RawCompletionEntry::default()
                },
            ],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let typed = offset_of(wxml, "名字");
    let typed_len = u32::try_from("名字".len()).expect("cjk bytes");
    let cursor = typed + typed_len;
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("cjk unclosed partial member completion");
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["名字", "年龄"]
    );
    for item in &result.items {
        assert_eq!(item.replace.start(), typed);
        assert_eq!(item.replace.end(), cursor);
    }
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 1);
    let input = log.opened_inputs.first().expect("opened input");
    let derived = input.file(SCRIPT_PATH).expect("shadow script").text();
    let needle = "__wxml_root.用户.__wxml_completion";
    assert!(derived.contains(needle), "{derived}");
    let query = derived.find(needle).expect("synthetic needle") + "__wxml_root.用户.".len();
    let expected = offset_to_position(derived, Utf16Offset::new(utf16_len(&derived[..query])))
        .expect("position");
    assert_eq!(
        log.completion_calls,
        vec![(SCRIPT_PATH.to_owned(), expected)],
        "the query position must be exact in UTF-16 after CJK, emoji and CRLF text"
    );
}

#[test]
fn unclosed_member_completion_does_not_swallow_host_syntax_diagnostics() {
    let wxml = "<view>{{ user.na</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("diagnostics");
    assert!(
        !(result.status.is_supported() && result.diagnostics.is_empty()),
        "the unclosed mustache must not be reported clean: {:?}",
        result.status
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("mustache")),
        "the host unclosed-mustache diagnostic must survive: {:?}",
        result.diagnostics
    );
}

#[test]
fn unsupported_unclosed_member_shapes_are_explicit() {
    for (wxml, needle) in [
        ("<view>{{ .</view>", "{{ ."),
        ("<view>{{ user. </view>", "user. "),
        ("<view>{{ user.\n</view>", "user.\n"),
        ("<view>{{ user.name.</view>", "user.name."),
        ("<view>{{ item.</view>", "item."),
    ] {
        let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
        let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
        let start = offset_of(wxml, needle);
        let cursor = start + u32::try_from(needle.len()).expect("prefix length");
        let result = adapter
            .completion(CompletionRequest {
                context: context(&adapter, wxml),
                offset: cursor,
                kind: CompletionKind::ExpressionMember,
            })
            .expect("completion");
        assert_eq!(result.items, Vec::new(), "{wxml}");
        assert!(
            matches!(result.status, CapabilityStatus::Unsupported { .. }),
            "{wxml}: {:?}",
            result.status
        );
        assert!(
            result.status.reason().is_some(),
            "{wxml} needs an explicit reason"
        );
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 0, "{wxml}");
        assert!(log.completion_calls.is_empty(), "{wxml}");
    }
}

#[test]
fn root_completion_uses_the_script_overlay_without_a_native_session() {
    for wxml in ["<view>{{  }}</view>", "<view>{{   }}</view>"] {
        let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
        let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
        let cursor = offset_of(wxml, "{{") + 3;
        let result = adapter
            .completion(CompletionRequest {
                context: context(&adapter, wxml),
                offset: cursor,
                kind: CompletionKind::ExpressionMember,
            })
            .expect("root completion");
        assert_eq!(
            result
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["user"],
            "root candidates come from the data overlay"
        );
        assert_eq!(result.status, CapabilityStatus::Supported);
        assert!(!result.truncated);
        let item = &result.items[0];
        assert_eq!(item.replace.start(), cursor);
        assert_eq!(item.replace.end(), cursor);
        assert_eq!(item.kind, CompletionItemKind::Property);
        assert_eq!(item.insert_text, "user");
        assert_eq!(
            item.insert_text_format,
            wxml_types::InsertTextFormat::PlainText
        );
        assert_eq!(item.sort_text.as_deref(), Some("0_user"));
        assert_eq!(item.filter_text.as_deref(), Some("user"));
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 0, "root completion must not start a session");
        assert!(log.completion_calls.is_empty());
    }
}

#[test]
fn root_completion_with_a_cjk_prefix_replaces_only_the_prefix() {
    let wxml = "<view>😀{{ 标题 }}</view>";
    let script = "Page({ data: { 标题: '你好' } });";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, script)]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let title_start = offset_of(wxml, "标题");
    let cursor = title_start + u32::try_from("标题".len()).expect("cjk length");
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("cjk root completion");
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["标题"]
    );
    assert_eq!(result.items[0].replace.start(), title_start);
    assert_eq!(result.items[0].replace.end(), cursor);
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 0);
    assert!(log.completion_calls.is_empty());
}

#[test]
fn root_completion_without_a_data_member_is_a_checked_empty_answer() {
    let wxml = "<view>{{  }}</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, "Page({});")]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "{{") + 3,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("root completion");
    assert_eq!(result.items, Vec::new());
    assert_eq!(
        result.status,
        CapabilityStatus::Supported,
        "no data member is a checked empty root list, not a failed query"
    );
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn root_completion_with_dynamic_data_is_partial() {
    let wxml = "<view>{{  }}</view>";
    let script = "const make = () => ({});\nPage({ data: make() });";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, script)]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "{{") + 3,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("root completion");
    assert_eq!(result.items, Vec::new());
    let reason = result.status.reason().expect("partial reason");
    assert!(
        reason.contains("inline object literal") || reason.contains("field-level roots"),
        "{reason}"
    );
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn root_completion_without_an_owning_view_is_explicitly_unsupported() {
    let wxml = "<view>{{  }}</view>";
    let mut view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    view.target = None;
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "{{") + 3,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("root completion");
    assert_eq!(result.items, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    let reason = result.status.reason().expect("unsupported reason");
    assert!(reason.contains("no page or component script"), "{reason}");
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn trailing_dot_completion_mounts_a_synthetic_query_and_keeps_the_range() {
    let wxml = "<view>😀{{ user. }}</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let behavior = FakeBehavior {
        completions: FakeCompletionOutcome::List {
            entries: vec![
                RawCompletionEntry {
                    name: "name".to_owned(),
                    kind: 5,
                    sort_text: "11".to_owned(),
                    ..RawCompletionEntry::default()
                },
                RawCompletionEntry {
                    name: "age".to_owned(),
                    kind: 5,
                    sort_text: "12".to_owned(),
                    ..RawCompletionEntry::default()
                },
            ],
            incomplete: false,
        },
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let cursor = offset_of(wxml, "user.") + u32::try_from("user.".len()).expect("length");
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("trailing-dot completion");
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["name", "age"]
    );
    for item in &result.items {
        assert_eq!(
            item.replace.start(),
            cursor,
            "only the typed prefix after the dot is replaced"
        );
        assert_eq!(item.replace.end(), cursor);
    }
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 1);
    let input = log.opened_inputs.first().expect("opened input");
    let derived = input.file(SCRIPT_PATH).expect("shadow script").text();
    assert!(
        derived.contains("__wxml_root.user.__wxml_completion"),
        "the completion plan mounts a synthetic member query: {derived}"
    );
    let needle = "__wxml_root.user.__wxml_completion";
    let query = derived.find(needle).expect("synthetic needle") + "__wxml_root.user.".len();
    let expected = offset_to_position(derived, Utf16Offset::new(utf16_len(&derived[..query])))
        .expect("position");
    assert_eq!(
        log.completion_calls,
        vec![(SCRIPT_PATH.to_owned(), expected)]
    );
}

#[test]
fn trailing_dot_completion_for_an_unknown_root_is_explicitly_unsupported() {
    let wxml = "<view>{{ missing. }}</view>";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let cursor = offset_of(wxml, "missing.") + u32::try_from("missing.".len()).expect("length");
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("trailing-dot completion");
    assert_eq!(result.items, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    let reason = result.status.reason().expect("unsupported reason");
    assert!(reason.contains("not a data root"), "{reason}");
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 0, "an unknown root needs no native session");
    assert!(log.completion_calls.is_empty());
}

#[test]
fn trailing_dot_completion_with_dynamic_data_is_partial() {
    let wxml = "<view>{{ record. }}</view>";
    let script = "const makeData = () => ({});\nPage({ data: makeData() });";
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, script)]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let cursor = offset_of(wxml, "record.") + u32::try_from("record.".len()).expect("length");
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: cursor,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("trailing-dot completion");
    assert_eq!(result.items, Vec::new());
    let reason = result.status.reason().expect("partial reason");
    assert!(
        reason.contains("incomplete script data overlay"),
        "{reason}"
    );
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn unclosed_quotes_and_nested_incomplete_expressions_are_explicitly_unsupported() {
    for (wxml, typed) in [
        ("<view>{{ \"unclosed }}</view>", "\"unclosed"),
        ("<view>{{ (user. }}</view>", "user."),
    ] {
        let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, SCRIPT)]);
        let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
        let cursor = offset_of(wxml, typed) + u32::try_from(typed.len()).expect("length");
        let result = adapter
            .completion(CompletionRequest {
                context: context(&adapter, wxml),
                offset: cursor,
                kind: CompletionKind::ExpressionMember,
            })
            .expect("completion");
        assert_eq!(result.items, Vec::new());
        assert!(
            matches!(
                result.status,
                CapabilityStatus::Unsupported { .. } | CapabilityStatus::Partial { .. }
            ),
            "an unsafe expression must not be a silent empty success: {:?}",
            result.status
        );
        assert!(
            result.status.reason().is_some(),
            "an unsafe expression needs an explicit reason"
        );
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 0);
        assert!(log.completion_calls.is_empty());
    }
}

#[test]
fn root_completion_includes_component_properties() {
    let wxml = "<view>{{  }}</view>";
    let script =
        "Component({ properties: { label: { type: String, value: '' } }, data: { count: 1 } });";
    let mut view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, script)]);
    view.target.as_mut().expect("target").owner = RootKind::Component;
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "{{") + 3,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("component root completion");
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        vec!["count", "label"],
        "component data and properties are both WXML roots, sorted by name"
    );
    assert!(result.status.is_supported(), "{:?}", result.status);
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn root_completion_dedupes_and_caps_at_the_shared_limit() {
    let wxml = "<view>{{  }}</view>";
    let fields = (0..(wxml_types::MAX_COMPLETION_ITEMS + 4))
        .map(|index| format!("f{index}: {index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("Page({{ data: {{ {fields} }} }});");
    let view = empty_view(wxml, vec![prepared_file(SCRIPT_PATH, &script)]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "{{") + 3,
            kind: CompletionKind::ExpressionMember,
        })
        .expect("root completion");
    assert_eq!(result.items.len(), wxml_types::MAX_COMPLETION_ITEMS);
    assert!(result.truncated);
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("truncated"), "{reason}");
    let labels = result
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    let unique: std::collections::BTreeSet<&str> = labels.iter().copied().collect();
    assert_eq!(
        unique.len(),
        labels.len(),
        "root candidates must be deduped"
    );
    assert_eq!(labels.first(), Some(&"f0"));
    assert_eq!(log.lock().expect("log").starts, 0);
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
fn a_script_owned_diagnostic_is_excluded_and_marks_the_template_partial() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "data: { user",
            code: 999,
            category: 1,
            message: "script error",
            file_name: None,
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
    let CapabilityStatus::Partial { reason } = result.status else {
        panic!("script-owned diagnostics must not leave a clean Supported template");
    };
    assert!(reason.contains("script code"), "{reason}");
}

#[test]
fn a_diagnostic_for_another_file_is_not_published_as_a_template_diagnostic() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "user.name",
            code: 2304,
            category: 1,
            message: "external declaration error",
            file_name: Some("/work/demo/node_modules/typings/index.d.ts"),
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
    let CapabilityStatus::Partial { reason } = result.status else {
        panic!("external-file diagnostics must degrade the template");
    };
    assert!(reason.contains("other files"), "{reason}");
    assert!(!reason.contains('/'), "reason leaked a path: {reason}");
}

#[test]
fn script_diagnostics_publish_the_script_itself_and_ignore_template_facts() {
    const SCRIPT_ERROR: &str =
        "const x: string = 1;\nPage({ data: { user: { name: \"Ada\" } } });\n";
    let mut view = prepared_view(WXML, SCRIPT_ERROR, "user.name");
    view.syntax_diagnostics = vec![syntax_diagnostic(WXML, "view", "template syntax")];
    view.status_hint = Some(CapabilityStatus::Partial {
        reason: "template-only hint".to_owned(),
    });
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "x: string",
            code: 2322,
            category: 1,
            message: "Type 'number' is not assignable to type 'string'.",
            file_name: Some("/work/demo/pages/index/index.ts"),
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT_ERROR),
        })
        .expect("script diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    let expected = offset_of(SCRIPT_ERROR, "x: string");
    assert_eq!(diagnostic.range.start(), expected);
    assert_eq!(
        diagnostic.range.end(),
        expected + u32::try_from("x: string".len()).expect("range length")
    );
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    assert_eq!(diagnostic.code.as_deref(), Some("2322"));
    assert_eq!(
        diagnostic.message,
        "Type 'number' is not assignable to type 'string'."
    );
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "template syntax")
    );
    assert!(
        !result
            .status
            .reason()
            .is_some_and(|reason| reason.contains("template-only")),
        "template degradation must not leak into a script result: {:?}",
        result.status
    );

    let log = log.lock().expect("log");
    assert_eq!(log.starts, 1);
    assert_eq!(log.opens, 1);
    assert_eq!(log.semantic_paths, vec![SCRIPT_PATH.to_owned()]);
    assert_eq!(log.syntactic_paths, vec![SCRIPT_PATH.to_owned()]);
    let input = log.opened_inputs.first().expect("opened input");
    let file = input.file(SCRIPT_PATH).expect("script in input");
    assert_eq!(file.text(), SCRIPT_ERROR);
    assert!(
        !file.text().contains("__wxml_check_0"),
        "script diagnostics must not send the template shadow"
    );
}

#[test]
fn script_diagnostics_map_utf16_positions_to_script_bytes() {
    const SCRIPT_UNICODE: &str = "// 😀 中文\nconst 名字: string = 1;\n";
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "名字: string",
            code: 2322,
            category: 1,
            message: "unicode type error",
            file_name: Some("/work/demo/pages/index/index.ts"),
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SCRIPT_UNICODE,
            "user.name",
        ))),
        behavior,
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT_UNICODE),
        })
        .expect("script diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    let expected = u32::try_from(SCRIPT_UNICODE.find("名字").expect("name token")).expect("offset");
    assert_eq!(result.diagnostics[0].range.start(), expected);
    assert_eq!(
        result.diagnostics[0].range.end(),
        expected + u32::try_from("名字: string".len()).expect("length")
    );
}

#[test]
fn script_diagnostics_exclude_other_file_diagnostics() {
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![
            FakeDiagnostic {
                needle: "Page",
                code: 2322,
                category: 1,
                message: "requested file error",
                file_name: Some("/work/demo/pages/index/index.ts"),
            },
            FakeDiagnostic {
                needle: "data: { user",
                code: 2304,
                category: 1,
                message: "external declaration error",
                file_name: Some("/work/demo/node_modules/typings/index.d.ts"),
            },
        ]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("script diagnostics");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.as_deref(), Some("2322"));
    let CapabilityStatus::Partial { reason } = result.status else {
        panic!("external diagnostics must degrade the script result");
    };
    assert!(reason.contains("other files"), "{reason}");
    assert!(!reason.contains('/'), "reason leaked a path: {reason}");
}

#[test]
fn script_diagnostics_reject_non_script_documents_without_opening_a_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("script diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    let CapabilityStatus::Unsupported { reason } = result.status else {
        panic!("a template document must not be checked as a script");
    };
    assert!(reason.contains("TypeScript"), "{reason}");
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn script_and_template_diagnostics_share_one_session_without_mixing_inputs() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let template = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("template diagnostics");
    assert_eq!(template.status, CapabilityStatus::Supported);
    let script = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("script diagnostics");
    assert_eq!(script.status, CapabilityStatus::Supported);

    {
        let log = log.lock().expect("log");
        assert_eq!(log.starts, 1);
        assert_eq!(log.opens, 1);
        assert_eq!(log.updates, 1);
        let input = log.updated_inputs.last().expect("script update");
        assert_eq!(input.file(SCRIPT_PATH).expect("script file").text(), SCRIPT);
        assert!(
            !input
                .file(SCRIPT_PATH)
                .expect("script file")
                .text()
                .contains("__wxml_check_0")
        );
    }

    let _ = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("cached script diagnostics");
    {
        let log = log.lock().expect("log");
        assert_eq!(
            log.updates, 1,
            "an identical script plan must reuse the snapshot"
        );
    }

    let _ = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("template diagnostics again");
    {
        let log = log.lock().expect("log");
        assert_eq!(log.updates, 2);
        let input = log.updated_inputs.last().expect("template update");
        assert!(
            input
                .file(SCRIPT_PATH)
                .expect("shadow script")
                .text()
                .contains("__wxml_check_0"),
            "a template request must restore the shadow input"
        );
    }
}

#[test]
fn script_diagnostics_report_a_non_project_script_as_unsupported() {
    let (adapter, log) = adapter_with(Arc::new(FailingViews), FakeBehavior::default());
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("script diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn script_diagnostics_fail_closed_when_the_engine_is_missing() {
    let behavior = FakeBehavior {
        start_failure: Some(FakeFailure::Exited),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        behavior,
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("script diagnostics must not return Err");
    assert_eq!(result.diagnostics, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 1);
}

#[test]
fn script_diagnostics_missing_target_is_not_a_supported_empty_result() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(empty_view(WXML, Vec::new()))),
        FakeBehavior::default(),
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("script diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unavailable { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn script_diagnostics_with_an_incomplete_source_list_are_partial() {
    let mut view = prepared_view(WXML, SCRIPT, "user.name");
    view.source_complete = false;
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT),
        })
        .expect("script diagnostics");
    assert_eq!(result.diagnostics, Vec::new());
    let CapabilityStatus::Partial { reason } = result.status else {
        panic!("an incomplete project source list must not be a clean script");
    };
    assert!(reason.contains("incomplete"), "{reason}");
}

#[test]
fn script_diagnostics_accept_allow_js_documents() {
    const JS_PATH: &str = "pages/index/index.js";
    const JS: &str = "const x = 1;\nPage({ data: {} });\n";
    let mut view = empty_view(WXML, vec![prepared_file(JS_PATH, JS)]);
    view.compiler_options =
        serde_json::Map::from_iter([("allowJs".to_owned(), serde_json::Value::Bool(true))]);
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "x = 1",
            code: 2322,
            category: 1,
            message: "javascript type error",
            file_name: Some("/work/demo/pages/index/index.js"),
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, JS_PATH, JS),
        })
        .expect("javascript diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.as_deref(), Some("2322"));
    assert_eq!(
        log.lock().expect("log").semantic_paths,
        vec![JS_PATH.to_owned()]
    );
}

#[test]
fn backend_trait_script_diagnostics_reaches_the_script_seam() {
    const SCRIPT_ERROR: &str = "const x: string = 1;\n";
    let behavior = FakeBehavior {
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "x: string",
            code: 2322,
            category: 1,
            message: "trait seam type error",
            file_name: Some("/work/demo/pages/index/index.ts"),
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            SCRIPT_ERROR,
            "user.name",
        ))),
        behavior,
    );
    let result = Backend::script_diagnostics(
        &adapter,
        DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, SCRIPT_ERROR),
        },
    )
    .expect("trait script diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.as_deref(), Some("2322"));
}

#[test]
fn optional_real_engine_hover_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
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

/// Real-engine opt-in: a non-literal `Page(options)` owner must be classified
/// as `Unsupported` before the native engine is started.
///
/// The classification is pure Rust planning, so the engine must stay untouched
/// (the sandbox stays empty). This test fails closed when
/// `WXML_REQUIRE_REAL_ENGINE=1` and no engine is configured.
#[test]
fn optional_real_engine_non_literal_page_is_unsupported_without_an_engine() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const NONLITERAL_SCRIPT: &str = "const options = {\n  data: { user: { name: \"Ada\" } },\n  onTap() {},\n};\nPage(options);\n";
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
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
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-nonliteral-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(FakeViews::new(prepared_view(
            WXML,
            NONLITERAL_SCRIPT,
            "user.name",
        ))),
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox.clone()),
        )),
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics must not return Err");
    assert!(
        matches!(result.status, CapabilityStatus::Unsupported { .. }),
        "a real engine must not change the static class: {:?}",
        result.status
    );
    assert!(result.diagnostics.is_empty());
    assert_eq!(
        std::fs::read_dir(&sandbox).expect("sandbox").count(),
        0,
        "the static class must be decided before the engine starts"
    );
}

/// Returns every descendant PID of `root` using `/proc` (Linux only).
#[cfg(target_os = "linux")]
fn descendants(root: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut seen = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if seen.contains(&pid) {
            continue;
        }
        seen.push(pid);
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let children = std::path::Path::new("/proc")
                .join(pid.to_string())
                .join("task")
                .join(task.file_name())
                .join("children");
            let Ok(text) = std::fs::read_to_string(&children) else {
                continue;
            };
            for child in text
                .split_whitespace()
                .filter_map(|value| value.parse::<u32>().ok())
            {
                found.push(child);
                queue.push(child);
            }
        }
    }
    found
}

/// Kills the engine child whose executable and cwd match the test sandbox.
#[cfg(target_os = "linux")]
fn kill_engine_child(executable: &std::path::Path, sandbox: &std::path::Path) -> bool {
    let executable = std::fs::canonicalize(executable).expect("canonical executable");
    let sandbox = std::fs::canonicalize(sandbox).expect("canonical sandbox");
    for pid in descendants(std::process::id()) {
        let Ok(child_executable) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        if child_executable != executable {
            continue;
        }
        let Ok(child_cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        if child_cwd != sandbox {
            continue;
        }
        let status = std::process::Command::new("kill")
            .arg("-9")
            .arg(pid.to_string())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
#[test]
fn optional_real_engine_kill_restart_answers_first_hover_with_overlay() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine restart test skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-adapter-restart-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let owner = "import type { User } from \"../../model\";\nPage({ data: { user: null as unknown as User } });\n";
    let model = "export interface User { name: \"OVERLAY\" }\n";
    let globals = "declare function Page<T>(options: {\n  data: T;\n  __wxml_check_0(this: { data: T }): void;\n}): void;\n";
    let files = vec![
        prepared_file(SCRIPT_PATH, owner),
        prepared_file(MODEL_PATH, model),
        prepared_file("globals.d.ts", globals),
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
            SessionOptions::new(executable.clone(), sandbox.clone()),
        )),
    );
    let first = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("initial real engine hover");
    assert!(
        first.contents.contains("OVERLAY"),
        "the non-owner overlay must be visible before restart: {:?}",
        first.contents
    );
    assert_eq!(
        first.range.expect("initial range").start(),
        offset_of(WXML, "name")
    );

    assert!(
        kill_engine_child(&executable, &sandbox),
        "the native engine child under the test sandbox was not found"
    );
    thread::sleep(Duration::from_millis(50));
    let before = adapter.epoch();
    let restarted = adapter
        .hover(HoverRequest {
            context: context(&adapter, WXML),
            offset: offset_of(WXML, "name"),
        })
        .expect("the first hover after a killed engine must retry and succeed");
    assert!(
        restarted.contents.contains("OVERLAY"),
        "the restarted engine must replay the non-owner overlay: {:?}",
        restarted.contents
    );
    let range = restarted.range.expect("restart range");
    assert_eq!(range.start(), offset_of(WXML, "name"));
    assert_eq!(
        range.end(),
        offset_of(WXML, "name") + u32::try_from("name".len()).expect("length")
    );
    assert_eq!(
        adapter.epoch(),
        before,
        "a successful one-shot retry must not invalidate the request stamp"
    );
}

#[test]
fn optional_real_engine_script_diagnostics_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine script-diagnostics test skipped: set TSGO_BIN to the pinned \
             7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-adapter-script-diagnostics-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let wxml = "<view>{{ title }}</view>";
    let script = "const x: string = 1;\nPage({ data: { title: \"hi\" } });\n";
    let globals = "declare function Page(options: {\n  data: { title: string };\n  __wxml_check_0(this: { data: { title: string } }): void;\n}): void;\n";
    let files = vec![
        prepared_file(SCRIPT_PATH, script),
        prepared_file("globals.d.ts", globals),
    ];
    let views = Arc::new(FakeViews::new(prepared_view_with_files(
        wxml, "title", files,
    )));
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        views,
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, script),
        })
        .expect("real engine script diagnostics");
    eprintln!(
        "real engine script diagnostics: status={:?} diagnostics={:?}",
        result.status, result.diagnostics
    );
    assert_eq!(
        result.status,
        CapabilityStatus::Supported,
        "the real engine must answer both script diagnostic sources"
    );
    let error = result
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code.as_deref() == Some("2322"))
        .expect("the real engine must report TS2322 for the script type error");
    let expected = offset_of(script, "x: string");
    assert_eq!(error.range.start(), expected);
    assert_eq!(
        error.range.end(),
        expected + u32::try_from("x".len()).expect("length")
    );
    assert_eq!(error.severity, DiagnosticSeverity::Error);
    assert!(
        error.message.contains("not assignable to type 'string'"),
        "{}",
        error.message
    );

    let template = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("template diagnostics");
    assert!(
        !template
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.as_deref() == Some("2322")),
        "the script type error must not be published against the template: {:?}",
        template.diagnostics
    );
}

#[test]
fn optional_real_engine_definition_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
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
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
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
fn optional_real_engine_trailing_dot_completion_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine trailing-dot completion test skipped: set TSGO_BIN to the \
             pinned 7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-adapter-trailing-dot-completion-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    // `{{ user. }}` and the unclosed `{{ user.na` have no mountable
    // fragment. The adapter must mount the bare `user` root and ask the engine
    // after a synthetic dot without changing the published replacement range.
    let wxml = "<view>😀{{ user. }}</view>";
    let script = "Page({ data: { user: { name: \"Ada\", age: 36 } } });";
    let globals = "declare function Page(options: {\n  data: { user: { name: string; age: number } };\n  __wxml_check_0(this: { data: { user: { name: string; age: number } } }): void;\n}): void;\n";
    let files = vec![
        prepared_file(SCRIPT_PATH, script),
        prepared_file("globals.d.ts", globals),
    ];
    let views = Arc::new(FakeViews::new(empty_view(wxml, files)));
    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        views,
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );
    for (wxml, needle, prefix_len) in [
        ("<view>😀{{ user. }}</view>", "user.", 0_u32),
        ("<view>😀{{ user.na</view>", "user.na", 2),
    ] {
        let cursor = offset_of(wxml, needle) + u32::try_from(needle.len()).expect("length");
        let result = adapter
            .completion(CompletionRequest {
                context: context(&adapter, wxml),
                offset: cursor,
                kind: CompletionKind::ExpressionMember,
            })
            .expect("real engine trailing-dot completion");
        let labels: Vec<&str> = result
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        eprintln!(
            "real engine trailing-dot completion at {cursor}: status={:?} labels={labels:?}",
            result.status
        );
        assert!(
            labels.contains(&"name") && labels.contains(&"age"),
            "the real engine must complete the synthetic member access: {labels:?}"
        );
        for item in &result.items {
            assert_eq!(item.replace.start(), cursor - prefix_len);
            assert_eq!(item.replace.end(), cursor);
        }
    }
}

#[test]
fn optional_real_engine_adapter_document_symbols_smoke() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
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

// Event-handler completion: pure script facts, no native session.

const EVENT_WXML: &str = "<view bindtap=\"on\" />";

fn prepared_file_at(path: &str, text: &str, revision: u64) -> PreparedFile {
    let mut prepared = prepared_file(path, text);
    prepared.source_revision = revision;
    prepared
}

fn event_completion_view(owner: RootKind, script: &str) -> PreparedView {
    let mut view = empty_view(EVENT_WXML, vec![prepared_file(SCRIPT_PATH, script)]);
    view.target.as_mut().expect("target").owner = owner;
    view
}

fn event_completion_offset() -> u32 {
    offset_of(EVENT_WXML, "on") + 2
}

fn event_completion_request(adapter: &TsgoBackendAdapter, offset: u32) -> CompletionRequest {
    CompletionRequest {
        context: context(adapter, EVENT_WXML),
        offset,
        kind: CompletionKind::EventHandler,
    }
}

struct MutableViews {
    view: Mutex<PreparedView>,
}

impl MutableViews {
    fn new(view: PreparedView) -> Self {
        Self {
            view: Mutex::new(view),
        }
    }

    fn replace(&self, view: PreparedView) {
        *self.view.lock().expect("view") = view;
    }

    fn set_script(&self, text: &str, revision: u64) {
        let mut view = self.view.lock().expect("view");
        view.files = Arc::from(vec![prepared_file_at(SCRIPT_PATH, text, revision)]);
    }
}

impl ViewProvider for MutableViews {
    fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Ok(self.view.lock().expect("view").clone())
    }
}

#[test]
fn event_completion_uses_script_methods_without_starting_a_native_session() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(event_completion_view(
            RootKind::Page,
            "Page({ data: { onTap: 1 }, onTap() {}, onLoad: function () {} });",
        ))),
        FakeBehavior::default(),
    );
    let offset = event_completion_offset();
    let result = adapter
        .completion(event_completion_request(&adapter, offset))
        .expect("event completion");
    let labels: Vec<&str> = result
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, vec!["onLoad", "onTap"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
    assert!(!result.truncated);
    for item in &result.items {
        assert_eq!(item.kind, CompletionItemKind::Method);
        assert_eq!(item.replace.start(), offset - 2);
        assert_eq!(item.replace.end(), offset);
        assert_eq!(item.insert_text, item.label);
    }
    let log = log.lock().expect("log");
    assert_eq!(log.starts, 0, "event completion must not start a session");
    assert_eq!(log.opens, 0);
    assert!(log.completion_calls.is_empty());
    assert_eq!(log.hover_documentation_calls, 0);
}

#[test]
fn event_completion_returns_unavailable_when_the_view_is_unavailable() {
    let (adapter, log) = adapter_with(Arc::new(FailingViews), FakeBehavior::default());
    let error = adapter
        .completion(event_completion_request(
            &adapter,
            event_completion_offset(),
        ))
        .expect_err("host view failure");
    assert!(matches!(error, BackendError::Unavailable(_)), "{error:?}");
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn event_completion_rejects_a_stale_epoch_before_any_view_prepare() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(event_completion_view(
            RootKind::Page,
            "Page({ onTap() {} });",
        ))),
        FakeBehavior::default(),
    );
    let stale = BackendEpoch::new(BackendId::new(BACKEND_ID), 99);
    let request = CompletionRequest {
        context: context_with_epoch(&adapter, EVENT_WXML, stale),
        offset: event_completion_offset(),
        kind: CompletionKind::EventHandler,
    };
    assert!(matches!(
        adapter.completion(request).expect_err("stale epoch"),
        BackendError::Cancelled
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn event_completion_serves_component_methods() {
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(event_completion_view(
            RootKind::Component,
            "Component({ attached() {}, methods: { save() {} } });",
        ))),
        FakeBehavior::default(),
    );
    let result = adapter
        .completion(event_completion_request(
            &adapter,
            event_completion_offset(),
        ))
        .expect("event completion");
    let labels: Vec<&str> = result
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect();
    assert_eq!(labels, vec!["save"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn event_completion_without_an_owning_script_is_unsupported_and_empty() {
    let mut view = event_completion_view(RootKind::Page, "Page({ onTap() {} });");
    view.target = None;
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    let result = adapter
        .completion(event_completion_request(
            &adapter,
            event_completion_offset(),
        ))
        .expect("event completion");
    assert!(result.items.is_empty());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn event_completion_reports_a_missing_owning_script_as_unavailable() {
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(empty_view(EVENT_WXML, Vec::new()))),
        FakeBehavior::default(),
    );
    let error = adapter
        .completion(event_completion_request(
            &adapter,
            event_completion_offset(),
        ))
        .expect_err("missing owning script");
    assert!(matches!(error, BackendError::Unavailable(_)), "{error:?}");
}

#[test]
fn event_completion_cache_hits_until_a_script_revision_changes() {
    let views = Arc::new(MutableViews {
        view: Mutex::new(event_completion_view(
            RootKind::Page,
            "Page({ first() {} });",
        )),
    });
    let (adapter, log) = adapter_with(views.clone(), FakeBehavior::default());
    let offset = event_completion_offset();
    let labels = |result: &CompletionResult| {
        result
            .items
            .iter()
            .map(|item| item.label.clone())
            .collect::<Vec<_>>()
    };
    let first = adapter
        .completion(event_completion_request(&adapter, offset))
        .expect("first");
    assert_eq!(labels(&first), vec!["first"]);

    // Same revisions with different text: the candidate list is cached.
    views.set_script("Page({ second() {} });", 1);
    let cached = adapter
        .completion(event_completion_request(&adapter, offset))
        .expect("cached");
    assert_eq!(labels(&cached), vec!["first"]);

    // A revision change invalidates the entry.
    views.set_script("Page({ second() {} });", 2);
    let refreshed = adapter
        .completion(event_completion_request(&adapter, offset))
        .expect("refreshed");
    assert_eq!(labels(&refreshed), vec!["second"]);
    assert_eq!(log.lock().expect("log").starts, 0);
}

#[test]
fn event_completion_rejected_value_is_unsupported_not_supported_empty() {
    let wxml = "<view bindtap=\"onTap\"";
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(event_completion_view(
            RootKind::Page,
            "Page({ onTap() {} });",
        ))),
        FakeBehavior::default(),
    );
    let offset = u32::try_from(wxml.find('"').expect("quote")).expect("offset");
    let result = adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset,
            kind: CompletionKind::EventHandler,
        })
        .expect("event completion");
    assert!(result.items.is_empty());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
}

// ---------------------------------------------------------------------------
// Component property binding diagnostics (E2E-20)
// ---------------------------------------------------------------------------

const COMPONENT_CARD: &str =
    "Component({\n  properties: {\n    title: { type: String },\n  },\n});\n";
const COMPONENT_PATH: &str = "components/card/card.ts";
const COMPONENT_SCRIPT: &str = "Page({\n  data: { total: 3 },\n});\n";

#[test]
fn component_binding_diagnostics_are_published_with_shadow_ranges() {
    let wxml = r#"<view><card title="{{ total }}" /></view>"#;
    let view = prepared_component_view(
        wxml,
        COMPONENT_SCRIPT,
        "total",
        COMPONENT_PATH,
        COMPONENT_CARD,
        vec![component_attribute(wxml, "title", "total")],
    );
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Known("number".to_owned()),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.range.start(), offset_of(wxml, "total"));
    assert_eq!(
        diagnostic.range.end(),
        offset_of(wxml, "total") + u32::try_from("total".len()).expect("length")
    );
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    assert!(diagnostic.message.contains("component property \"title\""));
    assert!(diagnostic.message.contains("expects string, got number"));
    let log = log.lock().expect("log");
    assert_eq!(log.type_calls.len(), 1);
    assert_eq!(log.type_calls[0].0, SCRIPT_PATH);
}

#[test]
fn component_binding_any_type_degrades_to_partial() {
    let wxml = r#"<view><card title="{{ total }}" /></view>"#;
    let view = prepared_component_view(
        wxml,
        COMPONENT_SCRIPT,
        "total",
        COMPONENT_PATH,
        COMPONENT_CARD,
        vec![component_attribute(wxml, "title", "total")],
    );
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Any("any".to_owned()),
        ..FakeBehavior::default()
    };
    let (adapter, _log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("diagnostics");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let CapabilityStatus::Partial { reason } = &result.status else {
        panic!("expected Partial, got {:?}", result.status);
    };
    assert!(
        reason.starts_with("some component bindings could not be resolved and were not checked"),
        "{reason}"
    );
    assert!(
        !reason.contains('/'),
        "the reason must stay path-free: {reason}"
    );
}

#[test]
fn component_binding_type_query_failure_keeps_backend_diagnostics() {
    let wxml = r#"<view><card title="{{ total }}" /></view>"#;
    let view = prepared_component_view(
        wxml,
        COMPONENT_SCRIPT,
        "total",
        COMPONENT_PATH,
        COMPONENT_CARD,
        vec![component_attribute(wxml, "title", "total")],
    );
    let behavior = FakeBehavior {
        type_outcome: FakeTypeOutcome::Failure(FakeFailure::Timeout),
        semantic: FakeDiagnosticOutcome::Diagnostics(vec![FakeDiagnostic {
            needle: "total",
            code: 2339,
            category: 1,
            message: "backend semantic error",
            file_name: None,
        }]),
        ..FakeBehavior::default()
    };
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), behavior);
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, wxml),
        })
        .expect("diagnostics must not return Err");
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(result.diagnostics[0].code.as_deref(), Some("2339"));
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(log.lock().expect("log").shutdowns, 0);
}

#[test]
fn component_binding_pass_is_not_run_for_script_diagnostics() {
    let wxml = r#"<view><card title="{{ total }}" /></view>"#;
    let view = prepared_component_view(
        wxml,
        COMPONENT_SCRIPT,
        "total",
        COMPONENT_PATH,
        COMPONENT_CARD,
        vec![component_attribute(wxml, "title", "total")],
    );
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(view)),
        FakeBehavior {
            type_outcome: FakeTypeOutcome::Known("number".to_owned()),
            ..FakeBehavior::default()
        },
    );
    let result = adapter
        .script_diagnostics(&DiagnosticsRequest {
            context: script_context(&adapter, SCRIPT_PATH, COMPONENT_SCRIPT),
        })
        .expect("script diagnostics");
    assert!(result.status.is_supported(), "{:?}", result.status);
    assert!(log.lock().expect("log").type_calls.is_empty());
}

#[test]
fn component_binding_pass_is_not_run_when_no_custom_components() {
    let (adapter, log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let result = adapter
        .diagnostics(DiagnosticsRequest {
            context: context(&adapter, WXML),
        })
        .expect("diagnostics");
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert!(result.diagnostics.is_empty());
    assert!(log.lock().expect("log").type_calls.is_empty());
}

#[test]
fn completion_plan_does_not_query_component_types() {
    let wxml = "<view>{{ user.name }}</view>";
    let mut view = prepared_view(wxml, SCRIPT, "user.name");
    view.components = Arc::from(vec![component_usage(
        "card",
        COMPONENT_PATH,
        vec![component_attribute(wxml, "title", "user.name")],
    )]);
    let (adapter, log) = adapter_with(Arc::new(FakeViews::new(view)), FakeBehavior::default());
    adapter
        .completion(CompletionRequest {
            context: context(&adapter, wxml),
            offset: offset_of(wxml, "name"),
            kind: CompletionKind::ExpressionMember,
        })
        .expect("completion");
    assert!(log.lock().expect("log").type_calls.is_empty());
}

#[test]
fn capabilities_do_not_claim_component_diagnostics_uncovered() {
    let (adapter, _log) = adapter_with(
        Arc::new(FakeViews::new(prepared_view(WXML, SCRIPT, "user.name"))),
        FakeBehavior::default(),
    );
    let capabilities = adapter.capabilities();
    let CapabilityStatus::Partial { reason } = &capabilities.diagnostics else {
        panic!("diagnostics capability must stay Partial");
    };
    assert!(
        !reason.contains("component diagnostics are not covered"),
        "{reason}"
    );
    assert!(
        reason.contains("component property binding diagnostics are wired"),
        "{reason}"
    );
    assert!(
        reason.contains("event-handler diagnostics are served by host static semantics"),
        "{reason}"
    );
}

/// Real-engine opt-in end-to-end check for the `component-bindings` fixture.
///
/// The view is prepared by the real `ProjectState` from the checked-in fixture;
/// a minimal `globals.d.ts` supplies `Page`/`this.data` typing because the test
/// does not load the mini-program typings package. This verifies the exact
/// property error range and both hover types through the pinned engine. Event
/// diagnostics and definitions are produced by the static LSP layer and are
/// covered by the existing integration tests, not by this adapter seam.
#[test]
fn optional_real_engine_component_binding_fixture() {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const GLOBALS: &str = "declare function Page(options: {\n  data: { pageTitle: string; total: number };\n  __wxml_check_0(this: { data: { pageTitle: string; total: number } }): void;\n  onCardTap(): void;\n}): void;\n";
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        let required = matches!(
            std::env::var("WXML_REQUIRE_REAL_ENGINE").as_deref(),
            Ok("1" | "true")
        );
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1"
        );
        eprintln!(
            "optional real-engine component binding test skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc"
        );
        return;
    };
    let executable = PathBuf::from(&bin);
    assert!(executable.is_file(), "TSGO_BIN does not point at a file");

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/fixtures/component-bindings")
        .canonicalize()
        .expect("fixture root");
    let host = ProjectState::new();
    host.open_root(Some(&format!("file://{}", root.display())))
        .expect("open fixture root");
    let path = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
    let text = std::fs::read_to_string(root.join("pages/index/index.wxml")).expect("wxml text");
    let file = host.file_id(&path).expect("wxml file id");
    let target = host.view_for(&path).expect("view target");
    let snapshot = Arc::new(SourceSnapshot::new(
        file,
        SourceRevision::new(1),
        SnapshotId::new(1),
        text.clone(),
    ));
    let mut view = host.analysis(&snapshot).expect("prepared view");
    let mut files: Vec<PreparedFile> = view.files.iter().cloned().collect();
    files.push(PreparedFile {
        file: FileId::new(host.project_id(), "globals.d.ts").expect("globals id"),
        path: ProjectPath::new("globals.d.ts").expect("globals path"),
        kind: "ts",
        text: Arc::from(GLOBALS),
        source_revision: 1,
    });
    view.files = Arc::from(files);
    assert_eq!(view.components.len(), 2);

    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let sandbox = std::env::temp_dir().join(format!(
        "wxml-tsgo-component-{}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _cleanup = Cleanup(sandbox.clone());

    let adapter = TsgoBackendAdapter::new(
        PinnedEngine::pinned(),
        Arc::new(FakeViews::new(view)),
        Arc::new(super::session::NativeSessionFactory::new(
            SessionOptions::new(executable, sandbox),
        )),
    );
    let context = RequestContext::new(
        host.project_revision(),
        target.view_revision,
        adapter.epoch(),
        snapshot,
    )
    .expect("fixture context");

    let page_title = text.find("pageTitle").expect("pageTitle");
    let hover = adapter
        .hover(HoverRequest {
            context: context.clone(),
            offset: u32::try_from(page_title + 1).expect("offset"),
        })
        .expect("pageTitle hover");
    assert!(
        hover.contents.contains("string"),
        "pageTitle hover: {:?}",
        hover.contents
    );

    let count_total =
        text.find(r#"count="{{ total }}""#).expect("count total") + r#"count="{{ "#.len();
    let hover = adapter
        .hover(HoverRequest {
            context: context.clone(),
            offset: u32::try_from(count_total + 1).expect("offset"),
        })
        .expect("count hover");
    assert!(
        hover.contents.contains("number"),
        "count hover: {:?}",
        hover.contents
    );

    let result = adapter
        .diagnostics(DiagnosticsRequest { context })
        .expect("component diagnostics");
    assert_eq!(
        result.status,
        CapabilityStatus::Supported,
        "{:?}",
        result.status
    );
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    let diagnostic = &result.diagnostics[0];
    let title_total =
        text.rfind(r#"title="{{ total }}""#).expect("second title") + r#"title="{{ "#.len();
    assert_eq!(
        diagnostic.range.start(),
        u32::try_from(title_total).expect("start")
    );
    assert_eq!(
        diagnostic.range.end(),
        u32::try_from(title_total + "total".len()).expect("end")
    );
    assert!(diagnostic.message.contains("component property \"title\""));
    assert!(diagnostic.message.contains("expects string"));
    assert!(diagnostic.message.contains("number"));
}
