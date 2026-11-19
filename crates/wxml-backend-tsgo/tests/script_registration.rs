#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cast_possible_truncation
)]

//! Registration extraction and selection.
//!
//! These are structural tests: the TypeScript ledger does not have a dedicated
//! registration file. They pin the same shapes the existing `events`/`layout`
//! cases depend on (literal options, first registration, aliases, dynamic
//! rejection). Checker-backed diagnostics remain BLOCKED.

use std::time::{Duration, Instant};

use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::script::ScriptAnalysis;
use wxml_backend_tsgo::script::ScriptRequest;
use wxml_backend_tsgo::script::analyze_script;
use wxml_backend_tsgo::script::registration::RegistrationKind;

fn analysis(files: &[(&str, &str)], path: &str, kind: RegistrationKind) -> ScriptAnalysis {
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
    analyze_script(&ScriptRequest::new(&input, path, kind)).expect("script analysis")
}

fn analyze(text: &str, kind: RegistrationKind) -> ScriptAnalysis {
    analysis(&[("index.ts", text)], "index.ts", kind)
}

#[test]
fn structural_page_component_and_behavior_options_are_extracted() {
    let page = analyze(
        "Page({ data: { title: \"t\" }, properties: {}, methods: {} });",
        RegistrationKind::Page,
    );
    assert_eq!(
        page.registration.selected.as_ref().map(|fact| fact.kind),
        Some(RegistrationKind::Page)
    );
    assert!(page.registration.complete);
    assert_eq!(page.data.fields.len(), 1);

    let component = analyze(
        "Component({ data: { count: 1 }, properties: { title: { type: String } }, methods: { save() {} } });",
        RegistrationKind::Component,
    );
    assert_eq!(
        component
            .registration
            .selected
            .as_ref()
            .map(|fact| fact.kind),
        Some(RegistrationKind::Component)
    );
    assert_eq!(component.properties.properties.len(), 1);
    assert_eq!(component.methods.methods.len(), 1);

    let behavior = analyze(
        "Behavior({ data: { shared: true }, methods: { onBack() {} } });",
        RegistrationKind::Behavior,
    );
    assert_eq!(
        behavior
            .registration
            .selected
            .as_ref()
            .map(|fact| fact.kind),
        Some(RegistrationKind::Behavior)
    );
    assert_eq!(behavior.methods.methods[0].name, "onBack");
}

#[test]
fn structural_registrar_const_alias_is_extracted() {
    let result = analyze(
        "const page = Page;\npage({ data: { fromAlias: 1 } });",
        RegistrationKind::Page,
    );
    let selected = result.registration.selected.expect("selected alias");
    assert_eq!(selected.alias.as_deref(), Some("page"));
    assert_eq!(result.data.fields[0].name, "fromAlias");
}

#[test]
fn structural_page_root_falls_back_to_component_but_not_the_reverse() {
    let fallback = analyze(
        "Component({ data: { componentOnly: 1 } });",
        RegistrationKind::Page,
    );
    assert_eq!(
        fallback
            .registration
            .selected
            .as_ref()
            .map(|fact| fact.kind),
        Some(RegistrationKind::Component)
    );
    let reverse = analyze(
        "Page({ data: { pageOnly: 1 } });",
        RegistrationKind::Component,
    );
    assert!(reverse.registration.selected.is_none());
    assert!(!reverse.status.is_supported());
}

#[test]
fn structural_multiple_registrations_select_the_first_and_report_partial() {
    let result = analyze(
        "Page({ data: { first: 1 } });\nPage({ data: { second: 2 } });",
        RegistrationKind::Page,
    );
    assert_eq!(result.registration.candidates.len(), 2);
    assert!(result.registration.selected.is_some());
    assert!(!result.registration.complete);
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("multiple"))
    );
    assert!(
        result
            .data
            .fields
            .iter()
            .all(|field| field.name != "second")
    );
}

#[test]
fn structural_dynamic_registration_is_partial_without_inventing_symbols() {
    let result = analyze(
        "const options = {};\nPage(options);",
        RegistrationKind::Page,
    );
    assert!(result.registration.selected.is_none());
    assert!(matches!(
        result.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(result.data.fields.is_empty());
    assert!(result.properties.properties.is_empty());
    assert!(result.symbols.is_empty());
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("registration is not a literal object"))
    );
}

#[test]
fn structural_shadowed_registrar_is_rejected() {
    let result = analyze(
        "function Page() {}\nPage({ data: { fabricated: 1 } });",
        RegistrationKind::Page,
    );
    assert!(result.registration.selected.is_none());
    assert!(result.data.fields.is_empty());
    assert!(
        result
            .registration
            .unsupported
            .iter()
            .any(|call| call.reason.contains("shadowed"))
    );
}

#[test]
fn structural_non_object_argument_is_rejected() {
    let result = analyze("Page(makeOptions());", RegistrationKind::Page);
    assert!(result.registration.selected.is_none());
    assert!(result.symbols.is_empty());
    assert!(matches!(
        result.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn structural_unbalanced_script_is_a_typed_error() {
    let files = vec![
        SourceFile::new("index.ts", "Page({ data: {", DocumentRevision::new(1)).expect("source"),
    ];
    let input = AnalysisInput::new(
        ProjectKey::new("demo"),
        ProjectRoot::new("/work/demo").expect("root"),
        SnapshotRevision::new(1),
        files,
    )
    .expect("input");
    let error = analyze_script(&ScriptRequest::new(
        &input,
        "index.ts",
        RegistrationKind::Page,
    ))
    .expect_err("unbalanced script must fail");
    assert!(error.to_string().contains("invalid script input"));
}
#[test]
fn structural_component_alias_is_extracted() {
    let result = analyze(
        "const component = Component;\ncomponent({ methods: { save() {} } });",
        RegistrationKind::Component,
    );
    let selected = result.registration.selected.expect("selected alias");
    assert_eq!(selected.alias.as_deref(), Some("component"));
    assert_eq!(result.methods.methods[0].name, "save");
}
#[test]
fn h1_shadowed_alias_call_does_not_inject_fake_fields() {
    let result = analyze(
        "const p = Page;\nfunction helper(p) { p({ data: { fake: 1 } }); }\nPage({ data: { real: 1 } });",
        RegistrationKind::Page,
    );
    let selected = result.registration.selected.expect("real Page selected");
    assert!(selected.alias.is_none());
    assert_eq!(result.data.fields.len(), 1);
    assert_eq!(result.data.fields[0].name, "real");
    assert!(result.data.fields.iter().all(|field| field.name != "fake"));
}

#[test]
fn h1_shadowed_alias_without_real_registration_produces_no_registration() {
    let result = analyze(
        "const p = Page;\nfunction helper(p) { p({ data: { fake: 1 } }); }",
        RegistrationKind::Page,
    );
    assert!(result.registration.selected.is_none());
    assert!(result.data.fields.is_empty());
    assert!(result.symbols.is_empty());
}

#[test]
fn h1_alias_does_not_win_over_a_canonical_page() {
    let result = analyze(
        "const p = Page;\np({ data: { fake: 1 } });\nPage({ data: { real: 1 } });",
        RegistrationKind::Page,
    );
    let selected = result
        .registration
        .selected
        .expect("canonical Page selected");
    assert!(selected.alias.is_none());
    assert_eq!(result.data.fields.len(), 1);
    assert_eq!(result.data.fields[0].name, "real");
}

#[test]
fn h3_shadow_scan_stays_bounded_on_large_component_options() {
    let members = (0..8_000)
        .map(|index| format!("key{index}: {index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("Component({{ {members} }});");
    let start = Instant::now();
    let result = analyze(&script, RegistrationKind::Component);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "8k Component keys took {elapsed:?}"
    );
    assert!(result.registration.selected.is_some());
}

#[test]
fn m7_class_and_object_methods_named_page_are_not_registrations() {
    for script in [
        "class Foo { other() {} Page({ data: { fake: 1 } }) {} }",
        "const obj = { Page({ data: { fake: 1 } }) {} };",
    ] {
        let result = analyze(script, RegistrationKind::Page);
        assert!(result.registration.selected.is_none(), "{script}");
        assert!(result.data.fields.is_empty(), "{script}");
    }
    let inside = analyze(
        "function load() { Page({ data: { inside: 1 } }); }",
        RegistrationKind::Page,
    );
    assert_eq!(inside.data.fields[0].name, "inside");
}

#[test]
fn m4_page_and_component_do_not_report_multiple_page_registrations() {
    let result = analyze(
        "Page({ data: { pageOnly: 1 } });\nComponent({ data: { componentOnly: 2 } });",
        RegistrationKind::Page,
    );
    assert_eq!(
        result.registration.selected.as_ref().map(|fact| fact.kind),
        Some(RegistrationKind::Page)
    );
    assert!(
        result
            .reasons
            .iter()
            .all(|reason| !reason.contains("multiple Page"))
    );
    assert_eq!(result.data.fields.len(), 1);
    assert_eq!(result.data.fields[0].name, "pageOnly");
}
#[test]
fn h1_alias_target_shadowing_invalidates_the_alias() {
    let result = analyze(
        "const Page = fakePage;\nconst p = Page;\np({ data: { fake: 1 } });",
        RegistrationKind::Page,
    );
    assert!(result.registration.selected.is_none());
    assert!(result.data.fields.is_empty());
}

/// S1: a nested `Page` must not beat the real top-level registration.
#[test]
fn s1_nested_page_does_not_win_over_the_top_level_page() {
    let result = analyze(
        "function defineNested() { Page({ data: { nested: 1 }, onNested() {} }); }\nPage({ data: { real: 1 }, onReal() {} });",
        RegistrationKind::Page,
    );
    let selected = result.registration.selected.expect("top-level Page");
    assert!(selected.top_level);
    assert!(selected.alias.is_none());
    assert!(result.registration.complete, "{:?}", result.reasons);
    assert!(result.data.fields.iter().any(|field| field.name == "real"));
    assert!(
        result
            .data
            .fields
            .iter()
            .all(|field| field.name != "nested")
    );
    let methods = result
        .methods
        .methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<Vec<_>>();
    assert!(methods.contains(&"onReal"));
    assert!(!methods.contains(&"onNested"));
    assert!(
        result.methods.status.is_supported(),
        "{:?}",
        result.methods.problems
    );
}

/// S1: a nested-only registration is parsed but can never be `Supported`.
#[test]
fn s1_nested_only_page_is_recognized_but_partial() {
    let result = analyze(
        "function load() { Page({ data: { inner: 1 }, onInner() {} }); }",
        RegistrationKind::Page,
    );
    let selected = result.registration.selected.expect("nested Page");
    assert!(!selected.top_level);
    assert!(!result.registration.complete);
    assert!(result.data.fields.iter().any(|field| field.name == "inner"));
    assert!(
        result
            .methods
            .methods
            .iter()
            .any(|method| method.name == "onInner")
    );
    assert!(!result.methods.status.is_supported());
    assert!(!result.status.is_supported());
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("not top-level"))
    );
}

/// S1: two top-level registrations are ambiguous even when one is selected.
#[test]
fn s1_two_top_level_pages_are_ambiguous_and_partial() {
    let result = analyze(
        "Page({ data: { first: 1 }, onFirst() {} });\nPage({ data: { second: 2 }, onSecond() {} });",
        RegistrationKind::Page,
    );
    let selected = result.registration.selected.expect("first Page");
    assert!(selected.top_level);
    assert_eq!(result.registration.candidates.len(), 2);
    assert!(!result.registration.complete);
    assert!(
        result
            .reasons
            .iter()
            .any(|reason| reason.contains("multiple Page"))
    );
    assert!(
        result
            .methods
            .problems
            .iter()
            .any(|problem| problem.contains("multiple Page registrations"))
    );
    assert!(!result.methods.status.is_supported());
    assert!(!result.status.is_supported());
}

/// S1: kind selection stays correct when only one registrar is top-level.
#[test]
fn s1_top_level_component_wins_when_a_nested_page_also_exists() {
    let component = analyze(
        "function makePage() { Page({ data: { inner: 1 }, onInner() {} }); }\nComponent({ data: { outer: 1 }, methods: { onSave() {} } });",
        RegistrationKind::Component,
    );
    let selected = component
        .registration
        .selected
        .as_ref()
        .expect("top-level Component");
    assert_eq!(selected.kind, RegistrationKind::Component);
    assert!(selected.top_level);
    assert!(component.registration.complete);
    assert!(component.methods.status.is_supported());
    let methods = component
        .methods
        .methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<Vec<_>>();
    assert!(methods.contains(&"onSave"));
    assert!(!methods.contains(&"onInner"));
    assert!(
        component
            .data
            .fields
            .iter()
            .all(|field| field.name != "inner")
    );
}

/// S1: the Page root keeps its own kind, but a nested match stays `Partial`.
#[test]
fn s1_page_root_with_only_a_nested_page_stays_partial() {
    let page = analyze(
        "function makePage() { Page({ data: { inner: 1 }, onInner() {} }); }\nComponent({ data: { outer: 1 }, methods: { onSave() {} } });",
        RegistrationKind::Page,
    );
    let selected = page
        .registration
        .selected
        .as_ref()
        .expect("nested Page is still recognized");
    assert_eq!(selected.kind, RegistrationKind::Page);
    assert!(!selected.top_level);
    assert!(!page.registration.complete);
    assert!(!page.methods.status.is_supported());
    assert!(
        page.methods
            .methods
            .iter()
            .any(|method| method.name == "onInner")
    );
    assert!(
        page.methods
            .methods
            .iter()
            .all(|method| method.name != "onSave")
    );
}

/// S1: a top-level alias still loses to the top-level canonical call.
#[test]
fn s1_top_level_alias_loses_to_the_top_level_canonical_call() {
    let result = analyze(
        "const p = Page;\np({ data: { fromAlias: 1 }, onAlias() {} });\nPage({ data: { fromDirect: 2 }, onDirect() {} });",
        RegistrationKind::Page,
    );
    let selected = result.registration.selected.expect("canonical Page");
    assert!(selected.alias.is_none());
    assert!(selected.top_level);
    let methods = result
        .methods
        .methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<Vec<_>>();
    assert!(methods.contains(&"onDirect"));
    assert!(!methods.contains(&"onAlias"));
    assert!(
        result
            .data
            .fields
            .iter()
            .any(|field| field.name == "fromDirect")
    );
    assert!(
        result
            .data
            .fields
            .iter()
            .all(|field| field.name != "fromAlias")
    );
    // Two top-level same-kind calls stay ambiguous even when one is an alias.
    assert!(!result.methods.status.is_supported());
}
