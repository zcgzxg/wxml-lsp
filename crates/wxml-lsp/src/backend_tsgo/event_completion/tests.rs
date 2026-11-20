//! Unit tests for the pure-script event-handler candidate builder.
//!
//! These tests exercise the real `ScriptAnalysis` pipeline through a fake
//! view provider; no native binary, session or checker is involved.

#![allow(
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use wxml_project::ProjectPath;
use wxml_types::{
    BackendEpoch, BackendId, CapabilityStatus, CompletionItemKind, CompletionKind,
    CompletionRequest, CompletionResult, FileId, InsertTextFormat, MAX_COMPLETION_ITEMS, ProjectId,
    ProjectRevision, RequestContext, RootKind, SnapshotId, SourceRevision, SourceSnapshot,
    ViewRevision,
};

use super::{EventCacheKey, completion, declared_event_items};
use crate::backend_tsgo::input::ViewProvider;
use crate::project_host::{HostError, PreparedFile, PreparedView, ViewComponent, ViewTarget};

const SCRIPT_PATH: &str = "pages/index/index.ts";
const BEHAVIOR_PATH: &str = "pages/index/behavior.ts";
const WXML_PATH: &str = "pages/index/index.wxml";
const TEMPLATE: &str = "<view bindtap=\"on|\" />";

fn project_id() -> ProjectId {
    ProjectId::new("demo")
}

fn file_id(path: &str) -> FileId {
    FileId::new(project_id(), path).expect("file id")
}

fn project_path(path: &str) -> ProjectPath {
    ProjectPath::new(path).expect("project path")
}

fn prepared_file(path: &str, text: &str) -> PreparedFile {
    prepared_file_at(path, text, 1)
}

fn prepared_file_at(path: &str, text: &str, revision: u64) -> PreparedFile {
    PreparedFile {
        file: file_id(path),
        path: project_path(path),
        kind: "ts",
        text: Arc::from(text),
        source_revision: revision,
    }
}

fn prepared_view(owner: RootKind, files: Vec<PreparedFile>) -> PreparedView {
    PreparedView {
        view_id: format!("{WXML_PATH}:1"),
        target: Some(ViewTarget {
            wxml: project_path(WXML_PATH),
            owner,
            script: project_path(SCRIPT_PATH),
            script_file: file_id(SCRIPT_PATH),
            view_revision: ViewRevision::new(1),
        }),
        files: Arc::from(files),
        compiler_options: serde_json::Map::new(),
        fragments: Arc::from(Vec::new()),
        components: Arc::from(Vec::new()),
        typing: None,
        events: Arc::from(Vec::new()),
        project_root: PathBuf::from("/work/demo"),
        syntax_diagnostics: Vec::new(),
        status_hint: None,
        source_complete: true,
    }
}

fn context(template: &str) -> RequestContext {
    RequestContext::new(
        ProjectRevision::new(project_id(), 1),
        ViewRevision::new(1),
        BackendEpoch::new(BackendId::new("event-test"), 1),
        Arc::new(SourceSnapshot::new(
            file_id(WXML_PATH),
            SourceRevision::new(1),
            SnapshotId::new(1),
            template,
        )),
    )
    .expect("context")
}

/// Splits a `|`-marked source into text and cursor byte offset.
fn cursor(source: &str) -> (String, u32) {
    source.find('|').map_or_else(
        || {
            (
                source.to_owned(),
                u32::try_from(source.len()).expect("offset"),
            )
        },
        |index| {
            let mut text = String::with_capacity(source.len() - 1);
            text.push_str(&source[..index]);
            text.push_str(&source[index + 1..]);
            (text, u32::try_from(index).expect("offset"))
        },
    )
}

struct FixedView {
    view: PreparedView,
}

impl ViewProvider for FixedView {
    fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Ok(self.view.clone())
    }
}

fn complete(script: &str, template: &str) -> CompletionResult {
    complete_view(
        prepared_view(RootKind::Page, vec![prepared_file(SCRIPT_PATH, script)]),
        template,
        CompletionKind::EventHandler,
    )
}

fn complete_view(view: PreparedView, template: &str, kind: CompletionKind) -> CompletionResult {
    let (text, offset) = cursor(template);
    let provider = FixedView { view };
    let cache = Mutex::new(None);
    completion(
        &provider,
        &CompletionRequest {
            context: context(&text),
            offset,
            kind,
        },
        &cache,
    )
    .expect("event completion")
}

fn labels(result: &CompletionResult) -> Vec<&str> {
    result
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect()
}

fn item<'a>(result: &'a CompletionResult, name: &str) -> &'a wxml_types::CompletionItem {
    result
        .items
        .iter()
        .find(|item| item.label == name)
        .expect("item by name")
}

#[test]
fn page_literal_methods_are_suggested_and_containers_are_not() {
    let script = "Page({\n  data: { onTap: 1 },\n  methods: { hidden() {} },\n  onTap() {},\n  onLoad: function () {},\n  onArrow: () => {},\n});";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["onArrow", "onLoad", "onTap"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
    assert!(!result.truncated);
    let (text, offset) = cursor(TEMPLATE);
    let start = u32::try_from(text.find("on").expect("handler")).expect("start");
    for entry in &result.items {
        assert_eq!(entry.kind, CompletionItemKind::Method);
        assert_eq!(entry.insert_text, entry.label);
        assert_eq!(entry.insert_text_format, InsertTextFormat::PlainText);
        assert_eq!(entry.sort_text, Some(format!("0_{}", entry.label)));
        assert_eq!(entry.filter_text, Some(entry.label.clone()));
        assert_eq!(
            (entry.replace.start(), entry.replace.end()),
            (start, offset)
        );
    }
}

#[test]
fn component_publishes_only_methods_members() {
    let script = "Component({ attached() {}, lifetimes: { created() {} }, methods: { save() {}, onTap: function () {} } });";
    let view = prepared_view(
        RootKind::Component,
        vec![prepared_file(SCRIPT_PATH, script)],
    );
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert_eq!(labels(&result), vec!["onTap", "save"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn component_without_methods_is_a_checked_empty_table() {
    let script = "Component({ attached() {} });";
    let view = prepared_view(
        RootKind::Component,
        vec![prepared_file(SCRIPT_PATH, script)],
    );
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert!(result.items.is_empty());
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn page_with_no_members_is_a_checked_empty_table() {
    let result = complete("Page({ data: {} });", TEMPLATE);
    assert!(result.items.is_empty());
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn non_literal_component_methods_are_partial_empty() {
    let script = "Component({ methods: someObject });";
    let view = prepared_view(
        RootKind::Component,
        vec![prepared_file(SCRIPT_PATH, script)],
    );
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert!(result.items.is_empty());
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn behavior_methods_are_included_and_own_method_documentation_wins() {
    let script = "import base from \"./behavior\";\nComponent({ behaviors: [base], methods: {\n  /** own implementation */\n  onBack() {}\n} });";
    let behavior = "export default Behavior({ methods: {\n  /** behavior implementation */\n  onBack() {},\n  /** second behavior */\n  fromBehavior() {}\n} });";
    let view = prepared_view(
        RootKind::Component,
        vec![
            prepared_file(SCRIPT_PATH, script),
            prepared_file(BEHAVIOR_PATH, behavior),
        ],
    );
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert_eq!(labels(&result), vec!["fromBehavior", "onBack"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
    assert_eq!(
        item(&result, "onBack").documentation.as_deref(),
        Some("own implementation")
    );
    assert_eq!(
        item(&result, "fromBehavior").documentation.as_deref(),
        Some("second behavior")
    );
}

#[test]
fn unresolved_behavior_keeps_own_methods_and_is_partial() {
    let script =
        "import base from \"./missing\";\nComponent({ behaviors: [base], methods: { own() {} } });";
    let view = prepared_view(
        RootKind::Component,
        vec![prepared_file(SCRIPT_PATH, script)],
    );
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert_eq!(labels(&result), vec!["own"]);
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn opaque_handler_is_not_suggested_and_marks_partial() {
    let script = "Page({ onTap: handlerRef, onOther: this.onOther, onLoad() {} });";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["onLoad"]);
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("not literal functions"), "{reason}");
}

#[test]
fn spread_and_computed_members_keep_known_methods_and_are_partial() {
    let script = "Page({ ...mixins, [name]: fn, onTap() {} });";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["onTap"]);
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn multiple_top_level_registrations_use_one_table_and_are_partial() {
    let script = "Page({ onTap() {} });\nPage({ onSave() {} });";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["onTap"]);
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("multiple"), "{reason}");
}

#[test]
fn a_nested_registration_alone_is_partial() {
    let script = "function factory() { Page({ nested() {} }); }";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["nested"]);
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn a_single_top_level_registration_wins_over_a_nested_one() {
    let script = "function factory() { Page({ nested() {} }); }\nPage({ top() {} });";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["top"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn no_registration_is_unsupported_and_empty() {
    let result = complete("const options = { onTap() {} };", TEMPLATE);
    assert!(result.items.is_empty());
    assert!(
        matches!(result.status, CapabilityStatus::Unsupported { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn a_non_literal_registration_is_partial_empty() {
    let result = complete("Page(options);", TEMPLATE);
    assert!(result.items.is_empty());
    assert!(
        matches!(result.status, CapabilityStatus::Partial { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn non_identifier_method_names_are_filtered_without_downgrading() {
    let script = "Page({ \"on-tap\"() {}, $ok() {}, \"onTap\"() {} });";
    let result = complete(script, TEMPLATE);
    assert_eq!(labels(&result), vec!["$ok", "onTap"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn unicode_handler_names_are_suggested() {
    let result = complete("Page({ 保存() {} });", TEMPLATE);
    assert_eq!(labels(&result), vec!["保存"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn cjk_and_emoji_template_prefixes_keep_exact_ranges() {
    let template = "🙂<view>中文</view><view bind:tap=\"保存|\" />";
    let result = complete("Page({ 保存() {} });", template);
    assert_eq!(labels(&result), vec!["保存"]);
    let (text, offset) = cursor(template);
    let start = u32::try_from(text.find("保存").expect("handler")).expect("start");
    let entry = item(&result, "保存");
    assert_eq!(
        (entry.replace.start(), entry.replace.end()),
        (start, offset)
    );
    assert_eq!(&text[start as usize..offset as usize], "保存");
}

#[test]
fn a_static_registrar_alias_registration_is_used() {
    let result = complete("const p = Page;\np({ aliased() {} });", TEMPLATE);
    assert_eq!(labels(&result), vec!["aliased"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn a_page_template_falls_back_to_a_component_registration() {
    let script = "Component({ methods: { componentMethod() {} } });";
    let view = prepared_view(RootKind::Page, vec![prepared_file(SCRIPT_PATH, script)]);
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert_eq!(labels(&result), vec!["componentMethod"]);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn an_empty_script_is_unsupported_not_supported_empty() {
    let result = complete("", TEMPLATE);
    assert!(result.items.is_empty());
    assert!(
        matches!(result.status, CapabilityStatus::Unsupported { .. }),
        "{:?}",
        result.status
    );
}

#[test]
fn detail_and_documentation_are_bounded() {
    let arguments = (0..40)
        .map(|index| format!("argument{index}: string"))
        .collect::<Vec<_>>()
        .join(", ");
    let documentation = "doc ".repeat(1000);
    let script = format!("Page({{\n  /** {documentation} */\n  onTap({arguments}) {{}}\n}});");
    let result = complete(&script, TEMPLATE);
    let entry = item(&result, "onTap");
    let detail = entry.detail.as_deref().expect("detail");
    assert_eq!(detail.chars().count(), 120);
    assert!(detail.ends_with('…'), "{detail}");
    let docs = entry.documentation.as_deref().expect("documentation");
    assert_eq!(docs.chars().count(), 2048);
    assert!(docs.ends_with('…'));
}

#[test]
fn replace_range_is_the_trimmed_handler_prefix() {
    let result = complete("Page({ onTap() {} });", "<view bindtap=\"  on|\" />");
    let entry = item(&result, "onTap");
    let (text, offset) = cursor("<view bindtap=\"  on|\" />");
    let start = u32::try_from(text.find("  on").expect("prefix")).expect("start") + 2;
    assert_eq!(
        (entry.replace.start(), entry.replace.end()),
        (start, offset)
    );
    assert_eq!(&text[start as usize..offset as usize], "on");

    let result = complete("Page({ onTap() {} });", "<view bindtap=\"on|Tap\" />");
    let entry = item(&result, "onTap");
    let (text, offset) = cursor("<view bindtap=\"on|Tap\" />");
    let start = u32::try_from(text.find("onTap").expect("token")).expect("start");
    assert_eq!(
        (entry.replace.start(), entry.replace.end()),
        (start, offset)
    );
}

#[test]
fn an_empty_value_yields_a_zero_width_replacement() {
    let result = complete("Page({ onTap() {} });", "<view bindtap=\"|\" />");
    let entry = item(&result, "onTap");
    let (_, offset) = cursor("<view bindtap=\"|\" />");
    assert_eq!(
        (entry.replace.start(), entry.replace.end()),
        (offset, offset)
    );
}

#[test]
fn custom_and_non_catalog_direct_events_are_served() {
    let script = "Page({ onTap() {} });";
    for template in [
        "<view bind:my-event=\"on|\" />",
        "<view catch:tap=\"on|\" />",
        "<view capture-bind:tap=\"on|\" />",
        "<view mut-bind:tap=\"on|\" />",
        "<view bindgetphonenumber=\"on|\" />",
        "<view bindchooseavatar=\"on|\" />",
        "<view bindplay=\"on|\" />",
        "<view bindpause=\"on|\" />",
    ] {
        let result = complete(script, template);
        assert_eq!(labels(&result), vec!["onTap"], "{template}");
        assert!(
            result.status.is_supported(),
            "{template}: {:?}",
            result.status
        );
    }
}

#[test]
fn malformed_and_dynamic_values_are_unsupported() {
    for template in [
        "<view bindtap=|\"onTap\"",
        "<view bindtap=\"|onTap\" />",
        "<view bindtap=\"onTap |\" />",
        "<view bindtap=\"onT{{x}}|\" />",
        "<view bindtap=\"{{ onT| }}\" />",
        "<view class=\"on|\" />",
        "<view bindex=\"on|\" />",
    ] {
        let result = complete("Page({ onTap() {} });", template);
        assert!(result.items.is_empty(), "{template}");
        assert!(
            matches!(result.status, CapabilityStatus::Unsupported { .. }),
            "{template}: {:?}",
            result.status
        );
    }
}

#[test]
fn a_non_event_kind_is_unsupported() {
    let view = prepared_view(
        RootKind::Page,
        vec![prepared_file(SCRIPT_PATH, "Page({ onTap() {} });")],
    );
    let result = complete_view(view, TEMPLATE, CompletionKind::ExpressionMember);
    assert!(result.items.is_empty());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
}

#[test]
fn a_template_without_an_owning_script_is_unsupported() {
    let mut view = prepared_view(
        RootKind::Page,
        vec![prepared_file(SCRIPT_PATH, "Page({ onTap() {} });")],
    );
    view.target = None;
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert!(result.items.is_empty());
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
}

#[test]
fn a_missing_owning_script_is_unavailable() {
    let view = prepared_view(
        RootKind::Page,
        vec![prepared_file(BEHAVIOR_PATH, "export default Behavior({});")],
    );
    let (text, offset) = cursor(TEMPLATE);
    let provider = FixedView { view };
    let cache = Mutex::new(None);
    let error = completion(
        &provider,
        &CompletionRequest {
            context: context(&text),
            offset,
            kind: CompletionKind::EventHandler,
        },
        &cache,
    )
    .expect_err("missing owning script");
    assert!(matches!(error, wxml_types::BackendError::Unavailable(_)));
}

#[test]
fn an_incomplete_prepared_source_list_keeps_candidates_but_is_partial() {
    let mut view = prepared_view(
        RootKind::Page,
        vec![prepared_file(SCRIPT_PATH, "Page({ onTap() {} });")],
    );
    view.source_complete = false;
    let result = complete_view(view, TEMPLATE, CompletionKind::EventHandler);
    assert_eq!(labels(&result), vec!["onTap"]);
    let reason = result.status.reason().expect("partial reason");
    assert!(
        reason.contains("source file list is incomplete"),
        "{reason}"
    );
}

fn many_methods(count: usize) -> String {
    let mut script = String::from("Page({\n");
    for index in 0..count {
        writeln!(script, "  method{index:03}() {{}},").expect("write");
    }
    script.push_str("});\n");
    script
}

#[test]
fn exactly_the_cap_is_not_truncated() {
    let result = complete(&many_methods(MAX_COMPLETION_ITEMS), TEMPLATE);
    assert_eq!(result.items.len(), MAX_COMPLETION_ITEMS);
    assert!(!result.truncated);
    assert!(result.status.is_supported(), "{:?}", result.status);
}

#[test]
fn one_over_the_cap_truncates_and_is_partial() {
    let result = complete(&many_methods(MAX_COMPLETION_ITEMS + 1), TEMPLATE);
    assert_eq!(result.items.len(), MAX_COMPLETION_ITEMS);
    assert!(result.truncated);
    assert_eq!(
        result.items.last().map(|item| item.label.as_str()),
        Some("method255")
    );
    let reason = result.status.reason().expect("partial reason");
    assert!(reason.contains("truncated"), "{reason}");
}

#[test]
fn cache_key_ignores_the_template_revision_and_cursor() {
    let files = vec![prepared_file(SCRIPT_PATH, "Page({ onTap() {} });")];
    let first = prepared_view(RootKind::Page, files.clone());
    let mut second = prepared_view(RootKind::Page, files);
    second.view_id = format!("{WXML_PATH}:9");
    second.target.as_mut().expect("target").view_revision = ViewRevision::new(9);
    let first_context = context("<view bindtap=\"on|\" />");
    let second_context = context("<view catch:tap=\"onTap|\" />");
    let first_key = EventCacheKey::new(
        &first_context,
        &first,
        first.target.as_ref().expect("target"),
    )
    .expect("first key");
    let second_key = EventCacheKey::new(
        &second_context,
        &second,
        second.target.as_ref().expect("target"),
    )
    .expect("second key");
    assert_eq!(first_key, second_key);
}

#[test]
fn cache_key_changes_with_a_behavior_revision() {
    let files = vec![
        prepared_file(SCRIPT_PATH, "Page({ onTap() {} });"),
        prepared_file(BEHAVIOR_PATH, "export default Behavior({});"),
    ];
    let first = prepared_view(RootKind::Page, files);
    let mut second = prepared_view(
        RootKind::Page,
        vec![
            prepared_file(SCRIPT_PATH, "Page({ onTap() {} });"),
            prepared_file_at(BEHAVIOR_PATH, "export default Behavior({});", 2),
        ],
    );
    second.view_id = format!("{WXML_PATH}:2");
    let request = context("<view bindtap=\"on|\" />");
    let first_key = EventCacheKey::new(&request, &first, first.target.as_ref().expect("target"))
        .expect("first key");
    let second_key = EventCacheKey::new(&request, &second, second.target.as_ref().expect("target"))
        .expect("second key");
    assert_ne!(first_key, second_key);
}

#[test]
fn cache_key_changes_with_the_owner_or_project_root() {
    let files = vec![prepared_file(SCRIPT_PATH, "Page({ onTap() {} });")];
    let page = prepared_view(RootKind::Page, files.clone());
    let component = prepared_view(RootKind::Component, files.clone());
    let mut other_root = prepared_view(RootKind::Page, files);
    other_root.project_root = PathBuf::from("/work/other");
    let request = context("<view bindtap=\"on|\" />");
    let key_of = |view: &PreparedView| {
        EventCacheKey::new(&request, view, view.target.as_ref().expect("target")).expect("key")
    };
    assert_ne!(key_of(&page), key_of(&component));
    assert_ne!(key_of(&page), key_of(&other_root));
}

struct MutableView {
    view: Mutex<PreparedView>,
}

impl MutableView {
    fn new(view: PreparedView) -> Self {
        Self {
            view: Mutex::new(view),
        }
    }

    fn set_files(&self, files: Vec<PreparedFile>) {
        self.view.lock().expect("view").files = Arc::from(files);
    }
}

impl ViewProvider for MutableView {
    fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
        Ok(self.view.lock().expect("view").clone())
    }
}

#[test]
fn repeated_requests_hit_the_cache_until_a_script_revision_changes() {
    let provider = MutableView::new(prepared_view(
        RootKind::Page,
        vec![prepared_file(SCRIPT_PATH, "Page({ first() {} });")],
    ));
    let cache = Mutex::new(None);
    let (text, offset) = cursor(TEMPLATE);
    let request = CompletionRequest {
        context: context(&text),
        offset,
        kind: CompletionKind::EventHandler,
    };
    let first = completion(&provider, &request, &cache).expect("first");
    assert_eq!(labels(&first), vec!["first"]);

    // Same revisions but different text: the cached list is reused.
    provider.set_files(vec![prepared_file(SCRIPT_PATH, "Page({ second() {} });")]);
    let cached = completion(&provider, &request, &cache).expect("cached");
    assert_eq!(labels(&cached), vec!["first"]);

    // A revision change invalidates the entry and re-runs script analysis.
    provider.set_files(vec![prepared_file_at(
        SCRIPT_PATH,
        "Page({ second() {} });",
        2,
    )]);
    let refreshed = completion(&provider, &request, &cache).expect("refreshed");
    assert_eq!(labels(&refreshed), vec!["second"]);
}

// ---- declared event attribute names (task 5.3) ----

/// Builds one declared event.
fn declared_event(
    name: &str,
    detail: Option<&str>,
    proven: bool,
) -> wxml_project::typing::TypingEvent {
    wxml_project::typing::TypingEvent {
        name: name.to_owned(),
        detail_text: detail.map(str::to_owned),
        detail: detail.map(wxml_project::typing::TypeText::parse),
        proven,
    }
}

/// Builds one component with declared events.
fn declared_component(
    events: Vec<wxml_project::typing::TypingEvent>,
    complete: bool,
) -> ViewComponent {
    ViewComponent {
        name: "van-field".to_owned(),
        specifier: "/components/field".to_owned(),
        kind: wxml_project::ComponentTargetKind::Local,
        component: None,
        wxml: None,
        json: None,
        property_facts: wxml_project::ComponentPropertyFacts {
            properties: Vec::new(),
            complete: true,
            reason: None,
            source: wxml_project::PropertySource::Script,
        },
        typing: Some(Arc::new(wxml_project::typing::TypingComponent {
            props: Vec::new(),
            events,
            complete,
            source: project_path("components.d.ts"),
        })),
        origin: crate::project_host::ComponentOrigin::Local { file: None },
    }
}

#[test]
fn declared_event_names_are_suggested_after_a_colon_prefix() {
    let component = declared_component(
        vec![
            declared_event("change", Some("{ index: number }"), true),
            declared_event("input", None, true),
        ],
        true,
    );
    let items = declared_event_items(&component, "bind:", 11).expect("declared events");
    let labels = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["change", "input"]);
    for item in &items {
        assert_eq!(item.kind, CompletionItemKind::Event);
        assert_eq!(item.insert_text, item.label);
        assert_eq!(item.insert_text_format, InsertTextFormat::PlainText);
        // Only the empty event-name fragment is replaced; `bind:` survives.
        assert_eq!((item.replace.start(), item.replace.end()), (16, 16));
    }
    assert_eq!(
        items[0].detail.as_deref(),
        Some("(e: WechatMiniprogram.CustomEvent<{ index: number }>) => void")
    );
}

#[test]
fn a_typed_fragment_replacement_covers_only_the_fragment() {
    let component = declared_component(vec![declared_event("change", None, true)], true);
    let items = declared_event_items(&component, "bind:cha", 11).expect("declared events");
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.label, "change");
    assert_eq!((item.replace.start(), item.replace.end()), (16, 19));
    assert_eq!(
        items[0].detail.as_deref(),
        Some("(e: WechatMiniprogram.CustomEvent<void>) => void")
    );
}

#[test]
fn capture_catch_and_mut_prefixes_keep_their_own_prefix() {
    let component = declared_component(vec![declared_event("change", None, true)], true);
    for (typed, start) in [("catch:", 11), ("capture-bind:", 11), ("mut-bind:", 11)] {
        let items = declared_event_items(&component, typed, start).expect("declared events");
        let expected = start + u32::try_from(typed.len()).expect("len");
        assert_eq!(
            (items[0].replace.start(), items[0].replace.end()),
            (expected, expected),
            "{typed}"
        );
        assert_eq!(items[0].insert_text, "change", "{typed}");
    }
}

#[test]
fn colon_and_compact_prefixes_both_suggest_declared_events() {
    let component = declared_component(vec![declared_event("someEvent", None, true)], true);
    for typed in [
        "bind:",
        "bind",
        "catch:",
        "catch",
        "capture-bind:",
        "capture-bind",
        "capture-catch:",
        "capture-catch",
        "mut-bind:",
        "mut-bind",
    ] {
        let items = declared_event_items(&component, typed, 0).expect(typed);
        assert_eq!(items.len(), 1, "{typed}");
        let end = u32::try_from(typed.len()).expect("len");
        // Only the (empty) event-name fragment is replaced after the prefix.
        assert_eq!(
            (items[0].replace.start(), items[0].replace.end()),
            (end, end),
            "{typed}"
        );
        assert_eq!(items[0].label, "someEvent", "{typed}");
        assert_eq!(items[0].insert_text, "someEvent", "{typed}");
    }
}

#[test]
fn a_kebab_fragment_inserts_the_kebab_spelling_of_a_camel_declaration() {
    let component = declared_component(vec![declared_event("someEvent", None, true)], true);
    for (typed, prefix_len) in [("bind:some-", 5), ("bindsome-", 4)] {
        let items = declared_event_items(&component, typed, 0).expect(typed);
        assert_eq!(items.len(), 1, "{typed}");
        let item = &items[0];
        assert_eq!(item.label, "someEvent", "{typed}");
        assert_eq!(item.insert_text, "some-event", "{typed}");
        assert_eq!(
            u32::try_from(prefix_len).expect("len"),
            item.replace.start(),
            "{typed}"
        );
        assert_eq!(
            u32::try_from(typed.len()).expect("len"),
            item.replace.end(),
            "{typed}"
        );
    }
}

#[test]
fn a_camel_fragment_inserts_the_camel_spelling_of_a_kebab_declaration() {
    let component = declared_component(vec![declared_event("some-event", None, true)], true);
    for (typed, prefix_len) in [("bind:someE", 5), ("bindsomeE", 4)] {
        let items = declared_event_items(&component, typed, 0).expect(typed);
        assert_eq!(items.len(), 1, "{typed}");
        let item = &items[0];
        assert_eq!(item.label, "some-event", "{typed}");
        assert_eq!(item.insert_text, "someEvent", "{typed}");
        assert_eq!(
            u32::try_from(prefix_len).expect("len"),
            item.replace.start(),
            "{typed}"
        );
        assert_eq!(
            u32::try_from(typed.len()).expect("len"),
            item.replace.end(),
            "{typed}"
        );
    }
}

#[test]
fn fragment_filtering_follows_the_kebab_and_camel_equivalence() {
    let camel = declared_component(vec![declared_event("someEvent", None, true)], true);
    assert!(declared_event_items(&camel, "bind:some-", 0).is_some());
    assert!(declared_event_items(&camel, "bindsomeE", 0).is_some());
    // `someevnt` is neither spelling of `someEvent`.
    assert!(declared_event_items(&camel, "bind:someevnt", 0).is_some_and(|items| items.is_empty()));

    let kebab = declared_component(vec![declared_event("some-event", None, true)], true);
    assert!(declared_event_items(&kebab, "bind:some", 0).is_some());
    assert!(declared_event_items(&kebab, "bind:someE", 0).is_some());
}

#[test]
fn the_replacement_range_starts_at_the_fragment_for_every_prefix() {
    let component = declared_component(vec![declared_event("someEvent", None, true)], true);
    for (typed, fragment) in [
        ("capture-bind:some", "some"),
        ("capture-bindsome", "some"),
        ("mut-bind:some", "some"),
        ("mut-bindsome", "some"),
    ] {
        let name_start = 7;
        let items = declared_event_items(&component, typed, name_start).expect(typed);
        let start = name_start + u32::try_from(typed.len() - fragment.len()).expect("len");
        assert_eq!(
            (items[0].replace.start(), items[0].replace.end()),
            (start, name_start + u32::try_from(typed.len()).expect("len")),
            "{typed}"
        );
    }
}

#[test]
fn incomplete_eventless_and_undeclared_components_have_no_declared_items() {
    let template = "bind:";
    assert!(declared_event_items(&declared_component(Vec::new(), true), template, 11).is_none());
    assert!(
        declared_event_items(
            &declared_component(vec![declared_event("change", None, true)], false),
            template,
            11
        )
        .is_none()
    );

    let mut component = declared_component(vec![declared_event("change", None, true)], true);
    component.typing = None;
    assert!(declared_event_items(&component, template, 11).is_none());
}

#[test]
fn non_prefix_and_malformed_names_never_use_declared_events() {
    let component = declared_component(vec![declared_event("change", None, true)], true);
    for typed in [
        "tap",
        "data-bind",
        "class",
        "bind:ch ang",
        "bind:cha!",
        "bind::cha",
        "bind:",
    ] {
        let allowed = declared_event_items(&component, typed, 0);
        if typed == "bind:" {
            assert!(allowed.is_some(), "{typed}");
            continue;
        }
        assert!(allowed.is_none(), "{typed}");
    }
}

#[test]
fn unproven_details_are_marked_in_the_completion_detail() {
    let component = declared_component(
        vec![declared_event("change", Some("SomeLib.Options"), false)],
        true,
    );
    let items = declared_event_items(&component, "bind:", 0).expect("declared events");
    let detail = items[0].detail.as_deref().expect("detail");
    assert!(detail.contains("detail 类型未证明"), "{detail}");
    assert!(detail.contains("SomeLib.Options"), "{detail}");
}
