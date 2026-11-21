#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cast_possible_truncation
)]

//! Event handler resolution.
//!
//! Structural parity with the TS `events` cases. Handler tables, statuses,
//! exact UTF-8/UTF-16 definition ranges and Chinese documentation are asserted;
//! no TypeScript checker result is claimed.

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsrs::script::events::{
    EventBinding, EventBindingForm, MAX_EVENT_RESULTS, parse_event_attribute,
};
use wxml_backend_tsrs::script::registration::RegistrationKind;
use wxml_backend_tsrs::script::{ModuleResolver, ScriptAnalysis, ScriptRequest, analyze_script};
use wxml_backend_tsrs::shadow::mapping::SourceRange;

struct Aliases;

impl ModuleResolver for Aliases {
    fn resolve(&self, specifier: &str, _from_path: &str) -> Option<String> {
        specifier
            .strip_prefix("@/")
            .map(|rest| format!("{rest}.ts"))
    }
}

fn analysis(files: &[(&str, &str)], path: &str, kind: RegistrationKind) -> ScriptAnalysis {
    analysis_with(files, path, kind, None)
}

fn analysis_with(
    files: &[(&str, &str)],
    path: &str,
    kind: RegistrationKind,
    resolver: Option<&dyn ModuleResolver>,
) -> ScriptAnalysis {
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
    let request = ScriptRequest::new(&input, path, kind);
    let request = resolver.map_or(request, |resolver| request.with_resolver(resolver));
    analyze_script(&request).expect("script analysis")
}

fn binding(handler: &str) -> EventBinding {
    let end = u32::try_from(10 + handler.len()).expect("small fixture");
    EventBinding::new("e0", "bindtap", "tap", handler, SourceRange::new(10, end))
}

/// Checker/template layers that this script-fact test does not exercise.
#[allow(dead_code)]
const BLOCKED_CASES: &[(&str, &str)] = &[
    (
        "TS-EVENTS-0108",
        "the full template event chain is outside this layer; only attribute classification is asserted",
    ),
    (
        "TS-EVENTS-0188",
        "the TS case uses a hand-built stub view; this test resolves the script facts only",
    ),
];

#[test]
fn case_events_0071_page_method_shorthand_function_and_arrow_resolve() {
    // Oracle: TS-EVENTS-0071.
    let script = "Page({\n  onTap() {},\n  onLoad: function () {},\n  onArrow: () => {},\n})";
    let result = analysis(&[("index.ts", script)], "index.ts", RegistrationKind::Page);
    let collection =
        result.resolve_events(&[binding("onTap"), binding("onLoad"), binding("onArrow")]);
    assert_eq!(collection.skipped, 0);
    for (offset, resolution) in collection.results.iter().enumerate() {
        assert!(resolution.status.is_supported());
        assert!(resolution.diagnostic.is_none());
        let definition = resolution.definition.as_ref().expect("definition");
        assert_eq!(definition.path, "index.ts");
        let name = ["onTap", "onLoad", "onArrow"][offset];
        let start = u32::try_from(script.find(name).expect("name")).expect("offset");
        assert_eq!(definition.range.utf8.start.get(), start);
        assert_eq!(
            definition.range.utf8.end.get(),
            start + u32::try_from(name.len()).unwrap()
        );
        assert!(
            resolution
                .signature
                .as_deref()
                .is_some_and(|text| text.contains(name))
        );
    }
}

#[test]
fn case_events_0093_component_methods_shorthand_and_arrow_property_resolve() {
    // Oracle: TS-EVENTS-0093.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ methods: { save() {}, load: () => {} }, data: {} })",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let collection = result.resolve_events(&[binding("save"), binding("load")]);
    assert_eq!(collection.skipped, 0);
    for resolution in &collection.results {
        assert!(resolution.status.is_supported());
        assert!(resolution.definition.is_some());
    }
}

#[test]
fn case_events_0108_only_binding_forms_are_classified() {
    // Oracle: TS-EVENTS-0108. BLOCKED: full template event chain; only parser classification is asserted.
    for attribute in [
        "bindtap",
        "catchtap",
        "bind:tap",
        "catch:tap",
        "capture-bind:tap",
        "capture-catch:tap",
        "mut-bind:tap",
    ] {
        let parsed = parse_event_attribute(attribute).expect("recognized attribute");
        assert_eq!(parsed.1, "tap");
        assert!(matches!(
            parsed.0,
            EventBindingForm::Bind
                | EventBindingForm::Catch
                | EventBindingForm::BindColon
                | EventBindingForm::CatchColon
                | EventBindingForm::CaptureBind
                | EventBindingForm::CaptureCatch
                | EventBindingForm::MutBind
        ));
    }
    assert!(parse_event_attribute("bind:").is_none());
    assert!(parse_event_attribute("tap").is_none());
}

#[test]
fn case_events_0130_definition_ranges_are_utf8_bytes_and_utf16_units() {
    // Oracle: TS-EVENTS-0130.
    let script = "const note = \"中文注释😀\";\nPage({\n  onTap() {},\n})";
    let result = analysis(&[("index.ts", script)], "index.ts", RegistrationKind::Page);
    let resolution = result.resolve_events(&[binding("onTap")]).results.remove(0);
    let definition = resolution.definition.expect("definition");
    let byte_start = script.find("onTap()").expect("handler") as u32;
    let utf16_start = script[..script.find("onTap()").expect("handler")]
        .encode_utf16()
        .count() as u32;
    assert_eq!(definition.range.utf8.start.get(), byte_start);
    assert_eq!(
        definition.range.utf8.end.get(),
        byte_start + u32::try_from("onTap".len()).unwrap()
    );
    assert_eq!(definition.range.utf16.start.get(), utf16_start);
    assert_eq!(
        definition.range.utf16.end.get(),
        utf16_start + u32::try_from("onTap".encode_utf16().count()).unwrap()
    );
}

#[test]
fn case_events_0143_missing_handler_reports_the_exact_handler_range() {
    // Oracle: TS-EVENTS-0143.
    let result = analysis(
        &[("index.ts", "Page({ data: {} })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let binding = binding("missingHandler");
    let handler_range = binding.handler_range;
    let resolution = result.resolve_events(&[binding]).results.remove(0);
    assert!(resolution.status.is_supported());
    assert!(resolution.definition.is_none());
    let diagnostic = resolution.diagnostic.expect("missing handler diagnostic");
    assert_eq!(diagnostic.range, handler_range);
    assert_eq!(diagnostic.severity, "error");
    assert!(diagnostic.message.contains("missingHandler"));
    assert!(diagnostic.message.contains("\"tap\""));
    assert!(diagnostic.message.contains("page"));
    assert!(!diagnostic.message.contains("/work"));
}

#[test]
fn case_events_0162_multiple_page_registrations_are_partial_and_use_the_first() {
    // Oracle: TS-EVENTS-0162.
    let script = "Page({ onTap() {} });\nPage({ onTap() {} });";
    let result = analysis(&[("index.ts", script)], "index.ts", RegistrationKind::Page);
    let collection = result.resolve_events(&[binding("onTap"), binding("noSuchHandler")]);
    let known = &collection.results[0];
    assert!(matches!(
        known.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(known.definition.is_some());
    assert!(known.diagnostic.is_none());
    let missing = &collection.results[1];
    assert!(matches!(
        missing.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(missing.diagnostic.is_none());
}

#[test]
fn case_events_0176_non_literal_registration_is_partial_without_a_diagnostic() {
    // Oracle: TS-EVENTS-0176.
    for script in [
        "const options = {};\nPage(options);",
        "const options: any = {};\nPage(options);",
    ] {
        let result = analysis(&[("index.ts", script)], "index.ts", RegistrationKind::Page);
        let resolution = result
            .resolve_events(&[binding("noSuchHandler")])
            .results
            .remove(0);
        assert!(matches!(
            resolution.status,
            wxml_types::CapabilityStatus::Partial { .. }
        ));
        assert!(resolution.diagnostic.is_none());
    }
}

#[test]
fn case_events_0259_empty_literal_behaviors_keep_the_methods_table_complete() {
    // Oracle: TS-EVENTS-0259.
    let result = analysis(
        &[("index.ts", "Component({ behaviors: [], methods: {} })")],
        "index.ts",
        RegistrationKind::Component,
    );
    let resolution = result
        .resolve_events(&[binding("noSuchHandler")])
        .results
        .remove(0);
    assert!(resolution.status.is_supported());
    assert_eq!(
        resolution
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.severity),
        Some("error")
    );
}

#[test]
fn case_events_0268_literal_registration_is_partial_when_a_non_literal_one_exists() {
    // Oracle: TS-EVENTS-0268.
    let script = "const options = {};\nPage({ onTap() {} });\nPage(options);";
    let result = analysis(&[("index.ts", script)], "index.ts", RegistrationKind::Page);
    let collection = result.resolve_events(&[binding("onTap"), binding("noSuchHandler")]);
    assert!(collection.results[0].definition.is_some());
    assert!(collection.results[0].diagnostic.is_none());
    assert!(collection.results[1].diagnostic.is_none());
}

#[test]
fn case_events_0280_spread_in_options_or_methods_is_partial_but_known_handlers_resolve() {
    // Oracle: TS-EVENTS-0280.
    let page_script = "const base = { data: {} };\nPage({ ...base, onTap() {} })";
    let page = analysis(
        &[("index.ts", page_script)],
        "index.ts",
        RegistrationKind::Page,
    );
    let page_collection = page.resolve_events(&[binding("onTap"), binding("noSuchHandler")]);
    assert!(page_collection.results[0].definition.is_some());
    assert!(page_collection.results[0].diagnostic.is_none());
    assert!(page_collection.results[1].diagnostic.is_none());

    let component_script = "const base = {};\nComponent({ methods: { ...base, save() {} } })";
    let component = analysis(
        &[("index.ts", component_script)],
        "index.ts",
        RegistrationKind::Component,
    );
    let component_collection =
        component.resolve_events(&[binding("save"), binding("noSuchHandler")]);
    assert!(component_collection.results[0].definition.is_some());
    assert!(component_collection.results[1].diagnostic.is_none());
}

#[test]
fn case_events_0301_component_without_methods_reports_a_real_missing_handler() {
    // Oracle: TS-EVENTS-0301.
    let result = analysis(
        &[("index.ts", "Component({ data: {} })")],
        "index.ts",
        RegistrationKind::Component,
    );
    let resolution = result
        .resolve_events(&[binding("noSuchHandler")])
        .results
        .remove(0);
    assert!(resolution.status.is_supported());
    assert!(resolution.definition.is_none());
    assert_eq!(
        resolution
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.severity),
        Some("error")
    );
    assert!(
        resolution
            .diagnostic
            .as_ref()
            .is_some_and(|diagnostic| diagnostic.message.contains("component"))
    );
}

#[test]
fn case_events_0312_non_literal_component_methods_are_partial() {
    // Oracle: TS-EVENTS-0312.
    let result = analysis(
        &[(
            "index.ts",
            "const buildMethods = () => ({});\nComponent({ methods: buildMethods() })",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let resolution = result
        .resolve_events(&[binding("noSuchHandler")])
        .results
        .remove(0);
    assert!(matches!(
        resolution.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(resolution.diagnostic.is_none());
    assert!(
        result
            .methods
            .problems
            .iter()
            .any(|problem| problem.contains("`methods` is not a literal object"))
    );
}

#[test]
fn case_events_0322_reserved_page_keys_and_probe_members_are_ignored() {
    // Oracle: TS-EVENTS-0322.
    let result = analysis(
        &[(
            "index.ts",
            "Page({ data: {}, lifetimes: {}, __wxml_root: 1, onTap() {} })",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    let collection = result.resolve_events(&[
        binding("data"),
        binding("lifetimes"),
        binding("__wxml_root"),
        binding("onTap"),
    ]);
    for resolution in &collection.results[..3] {
        assert!(resolution.status.is_supported());
        assert_eq!(
            resolution
                .diagnostic
                .as_ref()
                .map(|diagnostic| diagnostic.severity),
            Some("error")
        );
    }
    assert!(collection.results[3].status.is_supported());
    assert!(collection.results[3].definition.is_some());
}

#[test]
fn case_events_0339_non_identifier_and_non_literal_handlers_never_become_diagnostics() {
    // Oracle: TS-EVENTS-0339.
    let result = analysis(
        &[("index.ts", "Page({ onTap: handlerRef })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let collection = result.resolve_events(&[binding("a.b"), binding("onTap")]);
    assert!(matches!(
        collection.results[0].status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(collection.results[0].diagnostic.is_none());
    assert!(collection.results[1].diagnostic.is_none());
    assert!(collection.results[1].definition.is_none());
}

#[test]
fn case_events_0352_long_signatures_stay_one_line_and_complete() {
    // Oracle: TS-EVENTS-0352.
    let parameters = (0..80)
        .map(|index| format!("arg{index}: string"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("Page({{ onTap({parameters}) {{}} }})");
    let result = analysis(&[("index.ts", &script)], "index.ts", RegistrationKind::Page);
    let signature = result
        .resolve_events(&[binding("onTap")])
        .results
        .remove(0)
        .signature
        .expect("signature");
    assert!(!signature.is_empty());
    assert!(!signature.contains('\n'));
    assert!(signature.contains("arg79: string)"));
}

#[test]
fn case_events_0363_results_are_capped() {
    // Oracle: TS-EVENTS-0363.
    let result = analysis(
        &[("index.ts", "Page({ onTap() {} })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let events = (0..=MAX_EVENT_RESULTS)
        .map(|index| {
            EventBinding::new(
                format!("e{index}"),
                "bindtap",
                "tap",
                "onTap",
                SourceRange::new(10, 15),
            )
        })
        .collect::<Vec<_>>();
    let collection = result.resolve_events(&events);
    assert_eq!(collection.results.len(), MAX_EVENT_RESULTS);
    assert_eq!(collection.skipped, 1);
    assert!(
        collection
            .skipped_details
            .iter()
            .any(|detail| detail.contains("truncated"))
    );
}

#[test]
fn case_events_0425_aliased_behavior_handler_points_at_the_behavior_file() {
    // Oracle: TS-EVENTS-0425 / dirty D2. Structural: the definition file,
    // exact range and Chinese documentation are asserted; no native query.
    let result = analysis_with(
        &[
            (
                "index.ts",
                "import base from \"@/behaviors/base\";\nComponent({ behaviors: [base], data: {}, methods: {} });",
            ),
            (
                "behaviors/base.ts",
                "export default Behavior({\n  methods: {\n    /** 返回上一页 */\n    onBack(e: { detail: string }) {}\n  }\n});",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
        Some(&Aliases),
    );
    let resolution = result
        .resolve_events(&[binding("onBack")])
        .results
        .remove(0);
    assert!(resolution.status.is_supported());
    assert!(resolution.diagnostic.is_none());
    let definition = resolution.definition.expect("definition");
    assert_eq!(definition.path, "behaviors/base.ts");
    assert_eq!(resolution.documentation.as_deref(), Some("返回上一页"));
    assert!(
        resolution
            .signature
            .as_deref()
            .is_some_and(|text| text.contains("onBack"))
    );
    let behavior = "export default Behavior({\n  methods: {\n    /** 返回上一页 */\n    onBack(e: { detail: string }) {}\n  }\n});";
    let start = behavior.find("onBack").expect("handler") as u32;
    assert_eq!(definition.range.utf8.start.get(), start);
    assert_eq!(
        definition.range.utf8.end.get(),
        start + u32::try_from("onBack".len()).unwrap()
    );
}

#[test]
fn case_events_0461_component_method_overrides_the_same_named_behavior_method() {
    // Oracle: TS-EVENTS-0461 / dirty D2.
    let result = analysis_with(
        &[
            (
                "index.ts",
                "import base from \"@/behaviors/base\";\nComponent({\n  behaviors: [base],\n  data: {},\n  methods: {\n    /** 页面自己的实现 */\n    onBack(e: { detail: number }) {}\n  }\n});",
            ),
            (
                "behaviors/base.ts",
                "export default Behavior({\n  methods: {\n    /** 来自 behavior */\n    onBack(e: { detail: string }) {}\n  }\n});",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
        Some(&Aliases),
    );
    let resolution = result
        .resolve_events(&[binding("onBack")])
        .results
        .remove(0);
    assert!(resolution.status.is_supported());
    let definition = resolution.definition.expect("definition");
    assert_eq!(definition.path, "index.ts");
    assert_eq!(resolution.documentation.as_deref(), Some("页面自己的实现"));
    assert!(
        resolution
            .signature
            .as_deref()
            .is_some_and(|text| text.contains("detail: number"))
    );
}

#[test]
fn case_events_0498_unresolved_behavior_alias_stays_partial_and_never_reports_missing() {
    // Oracle: TS-EVENTS-0498 / dirty D2.
    let result = analysis_with(
        &[(
            "index.ts",
            "import base from \"@/behaviors/missing\";\nComponent({ behaviors: [base], data: {}, methods: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
        Some(&Aliases),
    );
    let resolution = result
        .resolve_events(&[binding("onBack")])
        .results
        .remove(0);
    assert!(matches!(
        resolution.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(resolution.diagnostic.is_none());
    assert!(resolution.definition.is_none());
}
#[test]
fn case_events_0188_only_inline_behavior_methods_resolve() {
    // Oracle: TS-EVENTS-0188 / dirty D2. BLOCKED: hand-built stub view; script facts only.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ behaviors: [Behavior({ methods: { \n/** 来自 behavior */\nfromBehavior() {} } })], methods: {} })",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let resolution = result
        .resolve_events(&[binding("fromBehavior")])
        .results
        .remove(0);
    assert!(resolution.status.is_supported());
    assert!(resolution.definition.is_some());
    assert_eq!(resolution.documentation.as_deref(), Some("来自 behavior"));
    assert!(resolution.diagnostic.is_none());
}

#[test]
fn case_events_0188_imported_behavior_handler_matches_the_oracle_path_and_signature() {
    // Oracle: TS-EVENTS-0188. The TS fixture resolves `fromBehavior` from
    // `src/behaviors/base.ts`; this uses the same project-relative path so the
    // TS `definition.fileId` maps one-to-one to `EventDefinition.path`.
    //
    // The oracle source carries a trailing comma after the `Behavior({...})`
    // options object. `behavior_unit` now accepts that exact `}` `,` `)` form,
    // so this fixture keeps the comma instead of masking the parser gap. The
    // only remaining difference is the `fileId` -> `EventDefinition.path`
    // naming, which is why this case stays near-exact rather than green.
    let index = "import base from \"./src/behaviors/base\";\nComponent({ behaviors: [base], data: {}, methods: {} })";
    let behavior = "export default Behavior({\n  methods: {\n    /** 来自 behavior */\n    fromBehavior(event: { detail: { x: number } }) {}\n  }\n},);";
    let result = analysis(
        &[("index.ts", index), ("src/behaviors/base.ts", behavior)],
        "index.ts",
        RegistrationKind::Component,
    );
    let resolution = result
        .resolve_events(&[binding("fromBehavior")])
        .results
        .remove(0);
    assert!(resolution.status.is_supported(), "{:?}", resolution.status);
    assert!(resolution.diagnostic.is_none());
    let definition = resolution.definition.as_ref().expect("definition");
    assert_eq!(definition.path, "src/behaviors/base.ts");
    let start = behavior.find("fromBehavior").expect("handler") as u32;
    assert_eq!(definition.range.utf8.start.get(), start);
    assert_eq!(
        definition.range.utf8.end.get(),
        start + u32::try_from("fromBehavior".len()).unwrap()
    );
    let signature = resolution.signature.as_deref().expect("signature");
    assert!(signature.contains("fromBehavior"), "{signature}");
    assert!(signature.contains("detail"), "{signature}");
    assert_eq!(resolution.documentation.as_deref(), Some("来自 behavior"));
}

#[test]
fn case_events_0236_unresolved_behavior_graph_keeps_missing_handlers_unproven() {
    // Oracle: TS-EVENTS-0236 / dirty D2.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ behaviors: [externalBehavior], methods: {} })",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let resolution = result
        .resolve_events(&[binding("fromBehavior")])
        .results
        .remove(0);
    assert!(matches!(
        resolution.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(resolution.diagnostic.is_none());
    assert!(resolution.definition.is_none());
}
#[test]
fn m1_method_signature_stops_before_a_return_type_body() {
    let result = analysis(
        &[("index.ts", "Page({ onTap(): void { return 1; } })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let signature = result
        .resolve_events(&[binding("onTap")])
        .results
        .remove(0)
        .signature
        .expect("signature");
    assert!(signature.contains("onTap"));
    assert!(signature.contains("void"));
    assert!(!signature.contains("return"));
    assert!(!signature.contains('1'));
}

#[test]
fn m1_behavior_contributed_signature_stays_complete() {
    let parameters = (0..80)
        .map(|index| format!("arg{index}: string"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "Component({{ behaviors: [Behavior({{ methods: {{ onBack({parameters}) {{}} }} }})], methods: {{}} }});"
    );
    let result = analysis(
        &[("index.ts", &script)],
        "index.ts",
        RegistrationKind::Component,
    );
    let signature = result
        .resolve_events(&[binding("onBack")])
        .results
        .remove(0)
        .signature
        .expect("behavior signature");
    assert!(!signature.contains('\n'));
    assert!(signature.contains("arg79: string)"));
}
#[test]
fn h4_own_methods_are_all_processed() {
    let methods = (0..2_000)
        .map(|index| format!("method{index}() {{}}"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("Component({{ methods: {{ {methods} }} }});");
    let result = analysis(
        &[("index.ts", &script)],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(result.methods.methods.len(), 2_000);
    assert!(result.methods.status.is_supported());
    let found = result
        .resolve_events(&[binding("method1999")])
        .results
        .remove(0);
    assert!(found.status.is_supported());
    assert!(found.definition.is_some());
}

/// S1: the event table is authoritative only for the top-level registration.
#[test]
fn s1_event_table_comes_from_the_top_level_registration() {
    let result = analysis(
        &[(
            "index.ts",
            "function defineNested() { Page({ onNested() {} }); }\nPage({ onReal() {} });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    let collection = result.resolve_events(&[binding("onReal"), binding("onNested")]);
    assert!(collection.results[0].status.is_supported());
    assert!(collection.results[0].definition.is_some());
    // The nested table is not the page table, so its handler is really missing.
    assert!(collection.results[1].status.is_supported());
    assert!(collection.results[1].definition.is_none());
    assert_eq!(
        collection.results[1]
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.severity),
        Some("error")
    );
}

/// S1: a nested-only registration keeps known handlers but never reports missing.
#[test]
fn s1_nested_only_registration_never_reports_missing_handlers() {
    let result = analysis(
        &[("index.ts", "function load() { Page({ onInner() {} }); }")],
        "index.ts",
        RegistrationKind::Page,
    );
    let collection = result.resolve_events(&[binding("onInner"), binding("noSuchHandler")]);
    assert!(collection.results[0].definition.is_some());
    assert!(matches!(
        collection.results[0].status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(collection.results[1].definition.is_none());
    assert!(collection.results[1].diagnostic.is_none());
}
