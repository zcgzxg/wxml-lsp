#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cast_possible_truncation
)]

//! Backend symbol facts for data paths, references, `setData` occurrences and
//! missing-handler insertions.
//!
//! Every assertion stays inside the syntactic evidence this crate can prove:
//! exact original tokens, conservative `Partial` answers and no fabricated
//! write role or rename edit.

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsrs::script::registration::RegistrationKind;
use wxml_backend_tsrs::script::symbol_facts::{SymbolFacts, query_symbol_facts};
use wxml_backend_tsrs::script::{ScriptRequest, analyze_script};
use wxml_types::{
    CapabilityStatus, MAX_SYMBOL_ENTRIES, SymbolOccurrenceKind, SymbolQuery, SymbolReferenceRole,
};

#[allow(clippy::needless_pass_by_value)] // one fixture helper keeps every call site terse
fn facts(
    files: &[(&str, &str)],
    path: &str,
    kind: RegistrationKind,
    query: SymbolQuery,
) -> SymbolFacts {
    let files = files
        .iter()
        .map(|(file_path, text)| {
            SourceFile::new(*file_path, *text, DocumentRevision::new(1)).expect("valid source")
        })
        .collect();
    let input = AnalysisInput::new(
        ProjectKey::new("demo"),
        ProjectRoot::new("/work/demo").expect("valid root"),
        SnapshotRevision::new(1),
        files,
    )
    .expect("valid input");
    let analysis =
        analyze_script(&ScriptRequest::new(&input, path, kind)).expect("script analysis");
    query_symbol_facts(&analysis, &input, &query)
}

fn data_path(segments: &[&str]) -> SymbolQuery {
    SymbolQuery::DataPath {
        path: segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
    }
}

fn handler(name: &str) -> SymbolQuery {
    SymbolQuery::EventHandler {
        name: name.to_owned(),
    }
}

#[test]
fn page_data_path_reads_references_and_setdata_segments() {
    let script = "Page({\n  data: { user: { name: \"Ada\" } },\n  onLoad() {\n    const label = this.data.user.name;\n    this.setData({ \"user.name\": label });\n  },\n  onTap() {},\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert_eq!(result.status, CapabilityStatus::Supported);
    assert!(result.complete, "{:?}", result.reason);
    let declaration = result.declaration.expect("leaf declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("name"));
    let expected = script.find("name: \"Ada\"").expect("declaration token") as u32;
    assert_eq!(declaration.range.utf8.start.get(), expected);
    assert_eq!(declaration.range.utf8.end.get(), expected + 4);

    let read = result
        .references
        .iter()
        .find(|reference| reference.role == SymbolReferenceRole::Read)
        .expect("this.data read");
    assert_eq!(read.token.expected_text.as_deref(), Some("name"));
    let read_at = script
        .find("user.name;")
        .expect("read token")
        .saturating_add("user.".len()) as u32;
    assert_eq!(read.token.range.utf8.start.get(), read_at);

    let occurrence = result
        .occurrences
        .iter()
        .find(|occurrence| occurrence.kind == SymbolOccurrenceKind::SetDataKey)
        .expect("setData occurrence");
    assert_eq!(occurrence.path, vec!["user".to_owned(), "name".to_owned()]);
    assert_eq!(occurrence.segment, 1);
    assert_eq!(occurrence.token.expected_text.as_deref(), Some("name"));
    assert!(result.insertion.is_none());
}

#[test]
fn component_missing_handler_uses_existing_methods_without_wrapping() {
    let script = "Component({\n  properties: { title: { type: String } },\n  methods: {\n    onTap() {},\n  },\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Component,
        handler("onSave"),
    );
    assert!(result.complete, "{:?}", result.reason);
    assert!(result.declaration.is_none());
    let insertion = result.insertion.expect("missing handler insertion");
    assert!(insertion.text.contains("onSave()"), "{}", insertion.text);
    assert!(!insertion.text.contains("methods: {"), "{}", insertion.text);
    let close = script.rfind("  }").expect("methods close") + 2;
    assert_eq!(insertion.offset as usize, close);
    assert_eq!(script.as_bytes()[close], b'}');
}

#[test]
fn component_missing_handler_wraps_a_new_methods_object() {
    let script = "Component({\n  properties: { title: { type: String } },\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Component,
        handler("onSave"),
    );
    assert!(result.complete, "{:?}", result.reason);
    let insertion = result.insertion.expect("missing handler insertion");
    assert!(insertion.text.contains("methods: {"), "{}", insertion.text);
    assert!(insertion.text.contains("onSave()"), "{}", insertion.text);
    let close = script.rfind("});").expect("options close");
    assert_eq!(insertion.offset as usize, close);
}

#[test]
fn page_missing_handler_keeps_crlf_and_trailing_commas() {
    let script = "Page({\r\n  data: {},\r\n  onLoad() {},\r\n});\r\n";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        handler("onSave"),
    );
    assert!(result.complete, "{:?}", result.reason);
    let insertion = result.insertion.expect("missing handler insertion");
    assert!(insertion.text.contains("\r\n"), "{:?}", insertion.text);
    assert!(!insertion.text.contains(",,"), "{:?}", insertion.text);
    let close = script.rfind("});").expect("options close");
    assert_eq!(insertion.offset as usize, close);
}

#[test]
fn alias_registration_and_cjk_emoji_ranges_are_exact() {
    let script = "const P = Page;\r\nP({\r\n  data: { 用户: { 名称: \"值😀\" } },\r\n  onLoad() { const v = this.data.用户.名称; },\r\n});\r\n";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["用户", "名称"]),
    );
    assert!(result.complete, "{:?}", result.reason);
    let declaration = result.declaration.expect("declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("名称"));
    let expected = script.find("名称: \"值😀\"").expect("declaration") as u32;
    assert_eq!(declaration.range.utf8.start.get(), expected);
    assert_eq!(
        declaration.range.utf8.end.get(),
        expected + "名称".len() as u32
    );
    let utf16_start = script[..expected as usize].encode_utf16().count() as u32;
    assert_eq!(declaration.range.utf16.start.get(), utf16_start);
    let read = result
        .references
        .iter()
        .find(|reference| reference.role == SymbolReferenceRole::Read)
        .expect("CJK read");
    assert_eq!(read.token.expected_text.as_deref(), Some("名称"));
}

#[test]
fn behavior_registration_resolves_own_data_and_method_references() {
    let script = "Behavior({\r\n  data: { shared: { count: 1 } },\r\n  methods: { onCount() { const c = this.data.shared.count; } },\r\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Behavior,
        data_path(&["shared", "count"]),
    );
    assert!(result.complete, "{:?}", result.reason);
    let declaration = result.declaration.expect("behavior declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("count"));
    let read = result
        .references
        .iter()
        .find(|reference| reference.role == SymbolReferenceRole::Read)
        .expect("behavior method read");
    assert_eq!(read.token.expected_text.as_deref(), Some("count"));
}

#[test]
fn setdata_nested_objects_have_segment_tokens() {
    let script =
        "Page({ data: { user: { name: 1 } }, onLoad() { this.setData({ user: { name: 2 } }); } });";
    let nested = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(nested.complete, "{:?}", nested.reason);
    assert_eq!(nested.occurrences.len(), 1);
    assert_eq!(nested.occurrences[0].segment, 1);
    assert_eq!(
        nested.occurrences[0].token.expected_text.as_deref(),
        Some("name")
    );

    let root = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user"]),
    );
    assert!(root.complete, "{:?}", root.reason);
    assert_eq!(root.occurrences.len(), 1);
    assert_eq!(root.occurrences[0].segment, 0);
    assert_eq!(
        root.occurrences[0].token.expected_text.as_deref(),
        Some("user")
    );
}

#[test]
fn page_reserved_names_do_not_include_data_fields() {
    let script = "Page({ data: { title: 1 }, onTap() {} });";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        SymbolQuery::DataRoots,
    );
    assert!(result.complete, "{:?}", result.reason);
    assert!(result.fields.iter().any(|field| field.name == "title"));
    assert!(result.reserved.iter().any(|name| name == "onTap"));
    assert!(!result.reserved.iter().any(|name| name == "title"));
}

#[test]
fn nested_regular_functions_are_partial_even_when_they_do_not_add_reads() {
    let script = "Page({ data: { user: { name: 1 } }, onLoad() { function helper() { return this.data.user.name; } return this.data.user.name; } });";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert_eq!(
        result
            .references
            .iter()
            .filter(|reference| reference.role == SymbolReferenceRole::Read)
            .count(),
        1
    );
}

#[test]
fn quoted_and_escaped_keys_keep_content_ranges() {
    let quoted = "Component({ properties: { \"card-title\": { type: String } } })";
    let result = facts(
        &[("index.ts", quoted)],
        "index.ts",
        RegistrationKind::Component,
        data_path(&["card-title"]),
    );
    let declaration = result.declaration.expect("quoted declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("card-title"));
    assert!(!declaration.expected_text.as_deref().unwrap().contains('"'));
    let field = result
        .fields
        .iter()
        .find(|field| field.name == "card-title")
        .expect("property field");
    assert!(field.quoted);
    assert!(
        !result.complete,
        "a quoted property conflicts with rename names"
    );

    let escaped = "Page({ data: { \"user\\u006Eame\": 1 }, onLoad() {} })";
    let result = facts(
        &[("index.ts", escaped)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["username"]),
    );
    let declaration = result.declaration.expect("escaped declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("user\\u006Eame"));
    assert!(!result.complete);
    assert!(
        result
            .reason
            .as_deref()
            .is_some_and(|reason| !reason.is_empty())
    );
}

#[test]
fn static_spread_resolves_and_dynamic_spread_is_partial() {
    let static_script =
        "const base = { user: { name: \"x\" } };\nPage({ data: { ...base }, onLoad() {} });";
    let result = facts(
        &[("index.ts", static_script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(result.complete, "{:?}", result.reason);
    let declaration = result.declaration.expect("spread declaration");
    assert_eq!(declaration.expected_text.as_deref(), Some("name"));

    let dynamic = "Page({ data: { ...getData() }, onLoad() {} });";
    let result = facts(
        &[("index.ts", dynamic)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert!(result.declaration.is_none());
}

#[test]
fn dynamic_data_members_and_escaped_this_data_are_partial() {
    let script = "Page({\n  data: { user: { name: 1 }, [key]: 2 },\n  onLoad() {\n    const direct = this.data[key];\n    const { user } = this.data;\n    this.dataSet.user;\n    this.properties.user;\n  },\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(
        result
            .references
            .iter()
            .filter(|reference| reference.role == SymbolReferenceRole::Read)
            .count(),
        0
    );
    assert!(
        result.reason.as_deref().is_some_and(|reason| {
            reason.contains("computed") || reason.contains("whole value")
        })
    );
}

#[test]
fn setdata_computed_keys_are_partial_but_literal_keys_remain() {
    let script = "Page({\n  data: { user: { name: 1 } },\n  onLoad() { this.setData({ \"user.name\": 2, [key]: 3 }); },\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert_eq!(result.occurrences.len(), 1);
    assert_eq!(result.occurrences[0].path, vec!["user", "name"]);
    assert_eq!(result.occurrences[0].segment, 1);

    let other =
        "Page({ data: { user: { name: 1 } }, onLoad() { other.setData({ \"user.name\": 2 }); } });";
    let result = facts(
        &[("index.ts", other)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert!(result.occurrences.is_empty());
}

#[test]
fn aliases_destructuring_and_bare_setdata_are_partial() {
    let script = "Page({ data: { user: { name: 1 } }, onLoad() { const that = this; const { user } = this; const write = setData.bind(this); } });";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert!(result.references.is_empty() || result.reason.is_some());
    assert!(
        result.reason.as_deref().is_some_and(|reason| {
            reason.contains("alias")
                || reason.contains("whole value")
                || reason.contains("destructuring")
                || reason.contains("setData")
        }),
        "{:?}",
        result.reason
    );

    let destructured_handler =
        "Page({ onTap() {}, onLoad() { const { onTap } = this; onTap(); } });";
    let result = facts(
        &[("index.ts", destructured_handler)],
        "index.ts",
        RegistrationKind::Page,
        handler("onTap"),
    );
    assert!(!result.complete);
    assert!(
        result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("destructuring")),
        "{:?}",
        result.reason
    );
}

#[test]
fn another_script_setdata_blocks_complete() {
    let subject = "Page({ data: { user: { name: 1 } }, onLoad() {} });";
    let helper = "export function update(page) { page.setData({ \"user.name\": 2 }); }";
    let result = facts(
        &[("index.ts", subject), ("helper.ts", helper)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert!(!result.complete);
    assert!(
        result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("another script")),
        "{:?}",
        result.reason
    );
}

#[test]
fn comment_before_a_closing_brace_blocks_the_insertion() {
    let script = "Page({ onLoad() {} /* trailing */ });";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        handler("onSave"),
    );
    assert!(!result.complete);
    assert!(result.insertion.is_none());
    assert!(
        result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("comments")),
        "{:?}",
        result.reason
    );
}

#[test]
fn opaque_or_dynamic_owner_tables_never_get_an_insertion() {
    let opaque = "Page({ onSave: handlerRef, data: {} });";
    let result = facts(
        &[("index.ts", opaque)],
        "index.ts",
        RegistrationKind::Page,
        handler("onSave"),
    );
    assert!(!result.complete);
    assert!(result.declaration.is_none());
    assert!(result.insertion.is_none());

    let spread = "Page({ ...extra });";
    let result = facts(
        &[("index.ts", spread)],
        "index.ts",
        RegistrationKind::Page,
        handler("onSave"),
    );
    assert!(!result.complete);
    assert!(result.insertion.is_none());
}

#[test]
fn component_data_roots_reserve_data_properties_and_methods() {
    let script = "Component({\n  data: { shared: 1 },\n  properties: { title: { type: String } },\n  methods: { onTap() {} },\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Component,
        SymbolQuery::DataRoots,
    );
    assert!(result.complete, "{:?}", result.reason);
    let names = result
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["shared", "title"]);
    for reserved in ["shared", "title", "onTap"] {
        assert!(result.reserved.iter().any(|name| name == reserved));
    }
}

#[test]
fn handler_alias_reads_are_published_but_computed_access_is_partial() {
    let script = "Page({\n  onTap() {},\n  onLoad() { const that = this; that.onTap(); this['onTap'](); },\n});";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Page,
        handler("onTap"),
    );
    assert!(!result.complete);
    assert!(matches!(result.status, CapabilityStatus::Partial { .. }));
    assert!(result.declaration.is_some());
    let read = result
        .references
        .iter()
        .find(|reference| reference.role == SymbolReferenceRole::Read)
        .expect("alias read");
    assert_eq!(read.token.expected_text.as_deref(), Some("onTap"));
    assert!(result.insertion.is_none());
}

#[test]
fn behavior_data_is_visible_but_cross_file_complete_is_denied() {
    let behavior = "export default Behavior({ data: { shared: { count: 1 } } });";
    let component =
        "import base from \"./base\";\nComponent({ behaviors: [base], data: {}, methods: {} });";
    let top = facts(
        &[("index.ts", component), ("base.ts", behavior)],
        "index.ts",
        RegistrationKind::Component,
        data_path(&["shared"]),
    );
    assert!(!top.complete);
    assert!(matches!(top.status, CapabilityStatus::Partial { .. }));
    let declaration = top.declaration.expect("behavior declaration");
    assert_eq!(declaration.path, "base.ts");
    assert_eq!(declaration.expected_text.as_deref(), Some("shared"));

    let nested = facts(
        &[("index.ts", component), ("base.ts", behavior)],
        "index.ts",
        RegistrationKind::Component,
        data_path(&["shared", "count"]),
    );
    assert!(!nested.complete);
    assert!(nested.declaration.is_none());
}

#[test]
fn component_observers_make_data_paths_partial() {
    let script = "Component({ properties: { title: { type: String, observer: \"title\" } }, observers: { \"title, open\": function () {} }, methods: {} })";
    let result = facts(
        &[("index.ts", script)],
        "index.ts",
        RegistrationKind::Component,
        data_path(&["title"]),
    );
    assert!(result.declaration.is_some());
    assert!(!result.complete);
    assert!(
        result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("observer")),
        "{:?}",
        result.reason
    );
}

#[test]
fn reference_lists_are_capped_and_the_reason_is_reported() {
    let mut script = String::from("Page({ data: { user: { name: 1 } }, onLoad() {\n");
    for _ in 0..(MAX_SYMBOL_ENTRIES + 8) {
        script.push_str("  this.data.user.name;\n");
    }
    script.push_str("} });");
    let result = facts(
        &[("index.ts", &script)],
        "index.ts",
        RegistrationKind::Page,
        data_path(&["user", "name"]),
    );
    assert_eq!(result.references.len(), MAX_SYMBOL_ENTRIES);
    assert!(!result.complete);
    let reason = result.reason.expect("bounded reason");
    assert!(reason.contains("truncated"));
}

#[test]
fn invalid_queries_are_unsupported_without_panicking() {
    let script = "Page({ data: { user: { name: 1 } } });";
    for query in [
        SymbolQuery::DataPath { path: Vec::new() },
        data_path(&["a[b]"]),
        data_path(&["a b"]),
        handler("not-an-identifier"),
    ] {
        let result = facts(
            &[("index.ts", script)],
            "index.ts",
            RegistrationKind::Page,
            query,
        );
        assert!(matches!(
            result.status,
            CapabilityStatus::Unsupported { .. }
        ));
        assert!(!result.complete);
        assert!(result.references.is_empty());
        assert!(result.occurrences.is_empty());
        assert!(result.insertion.is_none());
    }
}

#[test]
fn no_literal_registration_is_unsupported_not_empty_supported() {
    let result = facts(
        &[("index.ts", "const options = {}; Page(options);")],
        "index.ts",
        RegistrationKind::Page,
        SymbolQuery::DataRoots,
    );
    assert!(matches!(
        result.status,
        CapabilityStatus::Unsupported { .. }
    ));
    assert!(!result.complete);
    assert!(result.fields.is_empty());
}
