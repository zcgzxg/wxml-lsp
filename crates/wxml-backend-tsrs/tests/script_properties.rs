#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cast_possible_truncation
)]

//! `properties`/`data` overlay facts.
//!
//! Structural parity with the TS behavior overlay cases. Hover text,
//! assignability diagnostics and `this.data` instance types need the native
//! checker and are not asserted here.

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsrs::script::ScriptAnalysis;
use wxml_backend_tsrs::script::ScriptRequest;
use wxml_backend_tsrs::script::analyze_script;
use wxml_backend_tsrs::script::properties::DataRootKind;
use wxml_backend_tsrs::script::registration::RegistrationKind;

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

fn property<'a>(
    analysis: &'a ScriptAnalysis,
    name: &str,
) -> &'a wxml_backend_tsrs::script::properties::PropertyFact {
    analysis
        .properties
        .properties
        .iter()
        .find(|property| property.name == name)
        .unwrap_or_else(|| panic!("property {name} missing"))
}

/// Checker/oracle layers that these script-fact tests do not exercise.
#[allow(dead_code)]
const BLOCKED_CASES: &[(&str, &str)] = &[
    (
        "TS-COMPONENT-0292",
        "the TS case asserts component-binding diagnostics; this test asserts declaration facts only",
    ),
    (
        "TS-COMPONENT-0676",
        "the TS case asserts skipped checks; this test asserts the own-property overlay only",
    ),
    (
        "TS-COMPONENT-0498",
        "the TS case asserts checker types; this test asserts spread completeness facts only",
    ),
];

#[test]
fn case_component_0292_only_optional_types_and_defaults_are_preserved() {
    // Oracle: TS-COMPONENT-0292. BLOCKED: declaration facts only; the
    // component-binding diagnostic is not asserted.
    let result = analysis(
        &[(
            "index.ts",
            "Component({\n  properties: {\n    /** 标题，支持字符串或数字 */\n    cardTitle: { type: String, optionalTypes: [Number], required: true, value: \"x\" },\n    score: { type: String, optionalTypes: [Number], required: true, value: 1 },\n    loose: { type: null },\n    items: { type: Array },\n    defaulted: { type: Number, value: \"not-a-number\" },\n  },\n});",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let title = property(&result, "cardTitle");
    assert_eq!(title.type_text, "string | number");
    assert!(title.has_optional_types);
    assert_eq!(title.documentation, "标题，支持字符串或数字");
    assert_eq!(title.default_text.as_deref(), Some("\"x\""));
    assert_eq!(
        property(&result, "score").default_text.as_deref(),
        Some("1")
    );
    assert_eq!(property(&result, "loose").type_text, "any");
    assert_eq!(property(&result, "items").type_text, "any[]");
    assert_eq!(property(&result, "defaulted").type_text, "number");
    assert!(result.properties.complete);
}

#[test]
fn case_analysis_0139_declared_property_types_survive_an_unresolved_default_value() {
    // Oracle: TS-ANALYSIS-0139. The declared `Number` type must stay readable
    // even though the default value is an imported enum member the analysis
    // cannot resolve.
    let result = analysis(
        &[(
            "index.ts",
            "import { MissingEnum } from './missing';\nComponent({\n  properties: {\n    approvalStatus: { type: Number, value: MissingEnum.Preview },\n  },\n  data: {},\n  methods: {},\n});\n",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let approval = property(&result, "approvalStatus");
    assert_eq!(approval.type_text, "number");
    assert!(approval.type_complete);
    assert_eq!(
        approval.default_text.as_deref(),
        Some("MissingEnum.Preview"),
        "the unresolved default is kept verbatim, never folded into the type"
    );
    assert!(result.properties.complete);
}

#[test]
fn case_component_0514_literal_data_types_are_conservative_and_exact() {
    // Oracle: TS-COMPONENT-0514. Structural: literal inference without a
    // checker; anything unreadable must be `any` and incomplete.
    let result = analysis(
        &[(
            "index.ts",
            "Page({ data: { flag: true, count: 1, title: \"x\", list: [1, 2], nested: { label: \"y\" }, dynamic: make() } });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    let field = |name: &str| {
        result
            .data
            .fields
            .iter()
            .find(|field| field.name == name)
            .expect("field")
    };
    assert_eq!(field("flag").type_text, "boolean");
    assert_eq!(field("count").type_text, "number");
    assert_eq!(field("title").type_text, "string");
    assert_eq!(field("list").type_text, "number[]");
    assert_eq!(field("nested").type_text, r#"{ "label": string }"#);
    assert_eq!(field("dynamic").type_text, "any");
    assert!(!field("dynamic").exact);
    assert!(!result.data.complete);
}

#[test]
fn case_component_0607_non_literal_behavior_members_mark_the_overlay_partial() {
    // Oracle: TS-COMPONENT-0607.
    let result = analysis(
        &[(
            "index.ts",
            "const shared = { known: 1 };\nconst sharedProps = { title: { type: String } };\nComponent({ behaviors: [Behavior({ data: shared, properties: sharedProps })], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(matches!(
        result.behavior.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(result.behavior.data_incomplete);
    assert!(result.behavior.properties_incomplete);
    assert!(
        result
            .behavior
            .problems
            .iter()
            .any(|problem| problem.contains("non-object-literal"))
    );
    assert!(result.data.fields.is_empty());
}

#[test]
fn case_component_0676_only_unresolved_external_behavior_keeps_own_property() {
    // Oracle: TS-COMPONENT-0676. BLOCKED: own-property overlay only; skipped
    // component checks are not asserted.
    let result = analysis(
        &[(
            "index.ts",
            "import { externalBehavior } from \"some-lib\";\nComponent({ behaviors: [externalBehavior], properties: { title: { type: String } } });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(property(&result, "title").type_text, "string");
    assert!(!result.behavior.status.is_supported());
    assert!(
        result
            .properties
            .properties
            .iter()
            .any(|property| property.name == "title")
    );
}

#[test]
fn case_component_0498_only_property_spread_keeps_known_properties() {
    // Oracle: TS-COMPONENT-0498 (properties half). BLOCKED: checker types;
    // spread completeness facts only.
    let result = analysis(
        &[(
            "index.ts",
            "const base = {};\nComponent({ properties: { ...base, title: { type: String } } });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(!result.properties.complete);
    assert_eq!(property(&result, "title").type_text, "string");
    assert!(matches!(
        result.properties.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn property_without_readable_type_is_skipped_not_invented() {
    let result = analysis(
        &[(
            "index.ts",
            "Component({ properties: { custom: { type: MyType } } });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(result.properties.properties.is_empty());
    assert!(!result.properties.complete);
    assert!(
        result
            .properties
            .problems
            .iter()
            .any(|problem| problem.contains("no readable `type`"))
    );
}

#[test]
fn data_root_expression_absent_and_type_reference_are_distinct() {
    let expression = analysis(
        &[("index.ts", "const shared = {};\nPage({ data: shared });")],
        "index.ts",
        RegistrationKind::Page,
    );
    assert_eq!(expression.data.root, DataRootKind::Expression);
    assert!(matches!(
        expression.data.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));

    let absent = analysis(
        &[("index.ts", "Page({})")],
        "index.ts",
        RegistrationKind::Page,
    );
    assert_eq!(absent.data.root, DataRootKind::Absent);
    assert!(absent.data.complete);

    let reference = analysis(
        &[(
            "index.ts",
            "interface User { name: string }\nPage({ data: User });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    assert_eq!(reference.data.root, DataRootKind::TypeReference);
    assert!(matches!(
        reference.data.status,
        wxml_types::CapabilityStatus::Unsupported { .. }
    ));
}

#[test]
fn data_shorthand_is_partial_and_does_not_invent_a_type() {
    let result = analysis(
        &[(
            "index.ts",
            "const title = \"x\";\nPage({ data: { title } });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    assert!(!result.data.complete);
    assert!(result.data.fields.is_empty());
    assert!(matches!(
        result.data.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}
#[test]
fn dynamic_property_keys_are_partial_and_never_become_symbols() {
    let result = analysis(
        &[(
            "index.ts",
            "const key = \"title\";\nComponent({ properties: { [key]: { type: String }, known: { type: Number } } });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(!result.properties.complete);
    assert!(
        result
            .properties
            .problems
            .iter()
            .any(|problem| problem.contains("computed"))
    );
    assert_eq!(property(&result, "known").type_text, "number");
    assert!(result.symbols.iter().all(|symbol| symbol.kind
        != wxml_backend_tsrs::script::symbols::SymbolKind::Property
        || symbol.name == "known"));
}
#[test]
fn m2_options_spread_degrades_data_and_properties_completeness() {
    let result = analysis(
        &[(
            "index.ts",
            "const base = {};\nComponent({ ...base, data: { known: 1 }, properties: { title: { type: String } } });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(!result.data.complete);
    assert!(!result.properties.complete);
    assert!(matches!(
        result.data.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(matches!(
        result.properties.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert_eq!(result.data.fields[0].name, "known");
    assert_eq!(property(&result, "title").type_text, "string");
}

#[test]
fn m3_scalar_casts_are_not_treated_as_exact_literals() {
    let result = analysis(
        &[(
            "index.ts",
            "Page({ data: { numCast: 1 as any, strCast: \"s\" as any } });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    for name in ["numCast", "strCast"] {
        let field = result
            .data
            .fields
            .iter()
            .find(|field| field.name == name)
            .expect("cast field");
        assert_eq!(field.type_text, "any");
        assert!(!field.exact);
    }
    assert!(!result.data.complete);
}

#[test]
fn m5_unreadable_optional_types_degrade_instead_of_claiming_complete() {
    let result = analysis(
        &[(
            "index.ts",
            "Component({ properties: {\n  a: { type: String, optionalTypes: dynamicTypes },\n  b: { type: String, optionalTypes: [Number, UnknownType] },\n} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(!result.properties.complete);
    assert!(matches!(
        result.properties.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert_eq!(property(&result, "a").type_text, "string");
    assert_eq!(property(&result, "b").type_text, "string | number");
    assert!(property(&result, "b").has_optional_types);
}
#[test]
fn data_and_properties_are_built_from_one_scan_and_degrade_independently() {
    // One component declaring both members: a single lex must still produce
    // each overlay exactly as before. `properties` has a computed key, so its
    // degradation must not leak into `data`.
    let result = analysis(
        &[(
            "index.ts",
            "const key = \"dynamic\";\nComponent({\n  data: {\n    /** 计数 */\n    count: 1,\n    label: \"x\",\n  },\n  properties: {\n    /** 标题文本 */\n    title: { type: String, value: \"\" },\n    [key]: { type: String },\n  },\n});",
        )],
        "index.ts",
        RegistrationKind::Component,
    );

    let data_fields = result
        .data
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(data_fields, ["count", "label"]);
    assert_eq!(result.data.fields[0].type_text, "number");
    assert_eq!(result.data.fields[0].documentation, "计数");
    assert_eq!(result.data.fields[1].type_text, "string");
    assert!(result.data.status.is_supported());
    assert!(result.data.complete);
    assert!(result.data.problems.is_empty());

    let property_names = result
        .properties
        .properties
        .iter()
        .map(|property| property.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(property_names, ["title"]);
    let title = property(&result, "title");
    assert_eq!(title.type_text, "string");
    assert_eq!(title.default_text.as_deref(), Some("\"\""));
    assert_eq!(title.documentation, "标题文本");
    assert!(!result.properties.status.is_supported());
    assert!(!result.properties.complete);
    assert!(!result.properties.own_complete);
    assert_eq!(
        result.properties.problems,
        vec!["`properties` has a computed member name".to_owned()]
    );
}

#[test]
fn h4_every_own_property_is_processed() {
    let properties = (0..2_000)
        .map(|index| format!("prop{index}: {{ type: String }}"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("Component({{ properties: {{ {properties} }} }});");
    let result = analysis(
        &[("index.ts", &script)],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(result.properties.properties.len(), 2_000);
    assert!(result.properties.own_complete);
    assert!(result.properties.complete);
    assert!(result.properties.status.is_supported());
}
