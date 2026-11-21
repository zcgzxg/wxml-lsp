//! Shared fake host/session seams for the environment adapter tests.
//!
//! These helpers are deliberately test-only. The real install gate
//! (`environment/gate.rs`) uses a real `NativeSession` instead.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::backend_tsrs::query::definition::DefinitionResult as NativeDefinitionResult;
use crate::backend_tsrs::query::hover::HoverEvidence;
use crate::backend_tsrs::query::type_info::{TypeInfo, TypeQuality as NativeTypeQuality};
use serde_json::{Map, Value};
use wxml_backend_tsrs::config::PinnedEngine;
use wxml_backend_tsrs::input::{AnalysisInput, TypeAssets};
use wxml_backend_tsrs::native::NativeError;
use wxml_backend_tsrs::native::document_symbol::DocumentSymbolsEvidence;
use wxml_backend_tsrs::native::session::TypeQuery;
use wxml_backend_tsrs::native::strings::Utf16Position;
use wxml_backend_tsrs::native::wire::{RawCompletionList, RawDiagnostic};
use wxml_project::ProjectPath;
use wxml_text::SourceRange;
use wxml_types::{
    Backend, CapabilityStatus, CheckFragment, FileId, HoverRequest, ProjectId, ProjectRevision,
    RequestContext, RootKind, SnapshotId, SourceRevision, SourceSnapshot, ViewRevision,
};

use super::Environment;
use crate::backend_tsrs::TsrsBackendAdapter;
use crate::backend_tsrs::input::{RequestPlan, ViewProvider, build_request_plan};
use crate::backend_tsrs::session::{QuerySession, SessionFactory};
use crate::project_host::{HostError, PreparedFile, PreparedView, ViewTarget};

pub(super) const WXML: &str = "<view>{{ user.name }}</view>";
pub(super) const SCRIPT: &str = "Page({ data: { user: { name: \"Ada\" } } });\n";
pub(super) const SCRIPT_PATH: &str = "pages/index/index.ts";
pub(super) const WXML_PATH: &str = "pages/index/index.wxml";
pub(super) const ENV_REASON: &str =
    "the native type environment is partial: typeRoots (entry outside the virtual snapshot)";

pub(super) fn project() -> ProjectId {
    ProjectId::new("demo")
}

pub(super) fn script_file() -> FileId {
    FileId::new(project(), SCRIPT_PATH).expect("script file")
}

pub(super) fn unique_temp(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    temp_base().join(format!(
        "wxml-lsp-env-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Base directory for environment tests: `TMPDIR`, else `/tmp/opencode`.
pub(super) fn temp_base() -> PathBuf {
    std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from)
}

/// Owned temporary directory for the duration of one environment test.
pub(super) struct TempDir(tempfile::TempDir);

impl TempDir {
    pub(super) fn new(tag: &str) -> Self {
        let base = temp_base();
        fs::create_dir_all(&base).expect("temp base");
        let prefix = format!("wxml-lsp-env-{tag}-");
        Self(
            tempfile::Builder::new()
                .prefix(prefix.as_str())
                .tempdir_in(&base)
                .expect("temp dir"),
        )
    }

    pub(super) fn path(&self) -> &std::path::Path {
        self.0.path()
    }
}

pub(super) fn fragment() -> CheckFragment {
    let start = WXML.find("user.name").expect("fragment");
    let end = start + "user.name".len();
    CheckFragment {
        fragment_id: "f0".to_owned(),
        expr_text: "user.name".to_owned(),
        target: None,
        bindings: Vec::new(),
        guards: Vec::new(),
        wxml_range: SourceRange::from_bounds(
            u32::try_from(start).expect("start"),
            u32::try_from(end).expect("end"),
        )
        .expect("range"),
        fragment_revision: 1,
        root: RootKind::Page,
    }
}

pub(super) fn prepared_file(path: &str, text: &str) -> PreparedFile {
    PreparedFile {
        file: FileId::new(project(), path).expect("file"),
        path: ProjectPath::new(path).expect("project path"),
        kind: "ts",
        text: Arc::from(text),
        source_revision: 1,
    }
}

pub(super) fn prepared_view(options: Map<String, Value>) -> PreparedView {
    prepared_view_with_files(options, vec![prepared_file(SCRIPT_PATH, SCRIPT)])
}

pub(super) fn prepared_view_with_files(
    options: Map<String, Value>,
    files: Vec<PreparedFile>,
) -> PreparedView {
    let path = ProjectPath::new(SCRIPT_PATH).expect("script path");
    PreparedView {
        view_id: format!("{WXML_PATH}:1"),
        target: Some(ViewTarget {
            wxml: ProjectPath::new(WXML_PATH).expect("wxml path"),
            owner: RootKind::Page,
            script: path,
            script_file: script_file(),
            view_revision: ViewRevision::new(1),
        }),
        files: Arc::from(files),
        compiler_options: options,
        fragments: Arc::from(vec![fragment()]),
        wxs_modules: Arc::from(Vec::new()),
        components: Arc::from(Vec::new()),
        typing: None,
        events: Arc::from(Vec::new()),
        project_root: PathBuf::from("/work/demo"),
        syntax_diagnostics: Vec::new(),
        status_hint: None,
        source_complete: true,
    }
}

pub(super) struct TestViews {
    view: Arc<Mutex<PreparedView>>,
}

impl TestViews {
    pub(super) fn new(view: PreparedView) -> Arc<Self> {
        Arc::new(Self {
            view: Arc::new(Mutex::new(view)),
        })
    }

    pub(super) fn set(&self, view: PreparedView) {
        *self.view.lock().expect("view") = view;
    }
}

impl ViewProvider for TestViews {
    fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Ok(self.view.lock().expect("view").clone())
    }
}

pub(super) fn context(adapter: &TsrsBackendAdapter, source_revision: u64) -> RequestContext {
    let file = FileId::new(project(), WXML_PATH).expect("wxml file");
    RequestContext::new(
        ProjectRevision::new(project(), 1),
        ViewRevision::new(1),
        adapter.epoch(),
        Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(source_revision),
            SnapshotId::new(source_revision),
            WXML,
        )),
    )
    .expect("context")
}

pub(super) fn offset_of(wxml: &str, needle: &str) -> u32 {
    u32::try_from(wxml.find(needle).expect("needle")).expect("offset")
}

#[derive(Default)]
pub(super) struct Counts {
    pub(super) starts: AtomicUsize,
    pub(super) opens: AtomicUsize,
    pub(super) updates: AtomicUsize,
    pub(super) shutdowns: AtomicUsize,
}

#[derive(Clone)]
pub(super) struct TestFactory {
    counts: Arc<Counts>,
    reason: Option<String>,
}

impl TestFactory {
    pub(super) fn new(reason: Option<String>) -> (Self, Arc<Counts>) {
        let counts = Arc::new(Counts::default());
        (
            Self {
                counts: Arc::clone(&counts),
                reason,
            },
            counts,
        )
    }
}

impl SessionFactory for TestFactory {
    fn start(&self) -> Result<Box<dyn QuerySession>, NativeError> {
        self.counts.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TestSession {
            counts: Arc::clone(&self.counts),
            reason: self.reason.clone(),
            input: None,
        }))
    }
}

pub(super) struct TestSession {
    counts: Arc<Counts>,
    reason: Option<String>,
    input: Option<AnalysisInput>,
}

impl QuerySession for TestSession {
    fn open_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.counts.opens.fetch_add(1, Ordering::SeqCst);
        self.input = Some(input.clone());
        Ok(())
    }

    fn update_project(&mut self, input: &AnalysisInput) -> Result<(), NativeError> {
        self.counts.updates.fetch_add(1, Ordering::SeqCst);
        self.input = Some(input.clone());
        Ok(())
    }

    fn semantic_diagnostics(&mut self, _path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        Ok(Vec::new())
    }

    fn syntactic_diagnostics(&mut self, _path: &str) -> Result<Vec<RawDiagnostic>, NativeError> {
        Ok(Vec::new())
    }

    fn type_query(
        &mut self,
        _path: &str,
        _position: Utf16Position,
    ) -> Result<TypeQuery, NativeError> {
        Err(unsupported())
    }

    fn hover_evidence(
        &mut self,
        _path: &str,
        _position: Utf16Position,
    ) -> Result<HoverEvidence, NativeError> {
        Ok(HoverEvidence {
            type_info: Some(TypeInfo {
                text: "string".to_owned(),
                handle: 1,
                flags: 0,
            }),
            type_quality: NativeTypeQuality::Known,
            symbol: None,
            documentation: None,
            declarations: Vec::new(),
            status: CapabilityStatus::Supported,
            gaps: Vec::new(),
        })
    }

    fn completions(
        &mut self,
        _path: &str,
        _position: Utf16Position,
    ) -> Result<Option<RawCompletionList>, NativeError> {
        Ok(Some(RawCompletionList {
            is_incomplete: false,
            entries: Vec::new(),
        }))
    }

    fn definition(
        &mut self,
        _path: &str,
        _position: Utf16Position,
    ) -> Result<NativeDefinitionResult, NativeError> {
        Ok(NativeDefinitionResult {
            targets: Vec::new(),
            status: CapabilityStatus::Supported,
            truncated: false,
        })
    }

    fn document_symbols(&mut self, _path: &str) -> Result<DocumentSymbolsEvidence, NativeError> {
        Ok(DocumentSymbolsEvidence {
            symbols: Vec::new(),
            gaps: Vec::new(),
            skipped: 0,
            truncated: false,
        })
    }

    fn environment_degradation(&self) -> Option<String> {
        // A session only owns a VFS degradation after a successful open or
        // update; before that there is no open environment to report.
        self.input.as_ref().and_then(|_| self.reason.clone())
    }

    fn shutdown(self: Box<Self>) {
        self.counts.shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}

pub(super) const fn unsupported() -> NativeError {
    NativeError::Unsupported {
        method: "test seam",
    }
}

pub(super) fn trusted_adapter(
    views: Arc<TestViews>,
    assets: Arc<TypeAssets>,
    reason: Option<String>,
) -> (TsrsBackendAdapter, Arc<Counts>) {
    let (factory, counts) = TestFactory::new(reason);
    let adapter = TsrsBackendAdapter::new_with_environment(
        PinnedEngine::pinned(),
        views,
        Arc::new(factory),
        Environment::trusted(assets),
    );
    (adapter, counts)
}

pub(super) fn plan_with(
    views: &TestViews,
    adapter: &TsrsBackendAdapter,
    environment: &Environment,
    source_revision: u64,
) -> RequestPlan {
    build_request_plan(views, &context(adapter, source_revision), environment).expect("plan")
}

pub(super) fn hover_request(adapter: &TsrsBackendAdapter, source_revision: u64) -> HoverRequest {
    HoverRequest {
        context: context(adapter, source_revision),
        offset: offset_of(WXML, "name"),
    }
}
