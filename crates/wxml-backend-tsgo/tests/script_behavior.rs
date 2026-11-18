#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cast_possible_truncation
)]

//! Behavior-graph facts.
//!
//! These tests assert the script-fact layer only. They intentionally do not
//! claim checker parity (hover types, diagnostics, `this.data` instance types);
//! each case names its TypeScript oracle and is marked structural. Structural
//! equivalence means the same fields/methods/status are selected from the same
//! source shapes.

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

fn data<'a>(
    analysis: &'a ScriptAnalysis,
    name: &str,
) -> &'a wxml_backend_tsgo::script::properties::DataField {
    analysis
        .data
        .fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("data field {name} missing"))
}

fn property<'a>(
    analysis: &'a ScriptAnalysis,
    name: &str,
) -> &'a wxml_backend_tsgo::script::properties::PropertyFact {
    analysis
        .properties
        .properties
        .iter()
        .find(|property| property.name == name)
        .unwrap_or_else(|| panic!("property {name} missing"))
}

/// Checker/oracle layers that this script-fact test does not exercise.
#[allow(dead_code)]
const BLOCKED_CASES: &[(&str, &str)] = &[
    (
        "TS-COMPONENT-0570",
        "the TS case asserts checker diagnostics; this test asserts behavior facts only",
    ),
    (
        "TS-ANALYSIS-0380",
        "the TS case expands through the checker; this test resolves exported const objects syntactically",
    ),
];

#[test]
fn case_analysis_0380_imported_behavior_data_spread_stays_exact_and_supported() {
    // Oracle: TS-ANALYSIS-0380 / dirty D1. Structural only: the TS case expands
    // through the checker; this slice resolves the same exported const object
    // syntactically. The asserted boolean/Supported outcome is the observable
    // requirement, not a checker claim.
    let result = analysis(
        &[
            (
                "index.ts",
                "import { BASE } from \"./base\";\nconst shared = Behavior({ data: { ...BASE } });\nComponent({ behaviors: [shared], data: {} });",
            ),
            (
                "base.ts",
                "export const BASE = { isFlag: true, title: \"x\" };",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    let is_flag = data(&result, "isFlag");
    assert_eq!(is_flag.type_text, "boolean");
    assert!(is_flag.exact);
    assert_eq!(data(&result, "title").type_text, "string");
    assert!(result.status.is_supported(), "{:?}", result.reasons);
    assert!(!result.behavior.data_incomplete);
}

#[test]
fn case_component_0357_inline_behavior_data_and_properties_join_the_overlay() {
    // Oracle: TS-COMPONENT-0357. Structural: overlay facts, not hover text.
    let result = analysis(
        &[(
            "index.ts",
            "Component({\n  behaviors: [Behavior({\n    data: {\n      /** 共享计数 */\n      sharedCount: 3,\n      nested: { label: \"x\" },\n    },\n    properties: { cardTitle: { type: String } },\n  })],\n  data: { own: \"own\" },\n  properties: { ownProp: { type: Number } },\n});",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "sharedCount").type_text, "number");
    assert_eq!(data(&result, "sharedCount").documentation, "共享计数");
    assert_eq!(data(&result, "nested").type_text, r#"{ "label": string }"#);
    assert_eq!(property(&result, "cardTitle").type_text, "string");
    assert_eq!(property(&result, "ownProp").type_text, "number");
    assert_eq!(data(&result, "own").type_text, "string");
    assert!(result.status.is_supported(), "{:?}", result.reasons);
}

#[test]
fn case_component_0403_component_own_data_wins_over_a_behavior_field() {
    // Oracle: TS-COMPONENT-0403.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ behaviors: [Behavior({ data: { title: 1 } })], data: { title: \"own\" } });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let title = data(&result, "title");
    assert_eq!(title.type_text, "string");
    assert!(title.own_override);
}

#[test]
fn case_component_0416_imported_default_export_contributes_data_and_properties() {
    // Oracle: TS-COMPONENT-0416.
    let result = analysis(
        &[
            (
                "index.ts",
                "import shared from \"./shared\";\nComponent({ behaviors: [shared], data: {} });",
            ),
            (
                "shared.ts",
                "const shared = Behavior({ data: { importedCount: 7 }, properties: { importedTitle: { type: String, optionalTypes: [Number] } } });\nexport default shared;",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "importedCount").type_text, "number");
    assert_eq!(data(&result, "importedCount").path, "shared.ts");
    let title = property(&result, "importedTitle");
    assert_eq!(title.type_text, "string | number");
    assert_eq!(title.path, "shared.ts");
    assert!(result.status.is_supported(), "{:?}", result.reasons);
}

#[test]
fn case_component_0452_nested_behaviors_apply_before_their_owner_and_later_ones_win() {
    // Oracle: TS-COMPONENT-0452.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ behaviors: [\n  Behavior({ data: { later: \"later\", nested: { value: 1 } } }),\n  Behavior({\n    behaviors: [Behavior({ data: { nested: { value: \"inner\" }, onlyNested: 1 } })],\n    data: { later: 2 },\n  }),\n], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "later").type_text, "number");
    assert_eq!(data(&result, "nested").type_text, r#"{ "value": string }"#);
    assert_eq!(data(&result, "onlyNested").type_text, "number");
}

#[test]
fn case_component_0476_cycles_and_factory_calls_degrade_without_recursion() {
    // Oracle: TS-COMPONENT-0476. Structural: a cycle is reported as Partial and
    // still keeps the reachable data facts.
    let result = analysis(
        &[
            (
                "index.ts",
                "import a from \"./a\";\nComponent({ behaviors: [a, createBehavior()], data: {} });",
            ),
            (
                "a.ts",
                "import b from \"./b\"; export default Behavior({ behaviors: [b], data: { fromA: 1 } });",
            ),
            (
                "b.ts",
                "import a from \"./a\"; export default Behavior({ behaviors: [a], data: { fromB: 2 } });",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(matches!(
        result.behavior.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert_eq!(data(&result, "fromA").type_text, "number");
    assert_eq!(data(&result, "fromB").type_text, "number");
    assert!(
        result
            .behavior
            .problems
            .iter()
            .any(|problem| problem.contains("cycle")),
        "{:?}",
        result.behavior.problems
    );
    assert!(
        result
            .behavior
            .problems
            .iter()
            .any(|problem| problem.contains("dynamic")),
        "{:?}",
        result.behavior.problems
    );
}

#[test]
fn case_component_0498_spread_and_dynamic_data_members_stay_partial_and_any() {
    // Oracle: TS-COMPONENT-0498.
    let result = analysis(
        &[(
            "index.ts",
            "const extra = { value: 1 };\nComponent({ behaviors: [Behavior({ data: { ...extra, dynamic: makeValue() } })], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "value").type_text, "number");
    let dynamic = data(&result, "dynamic");
    assert_eq!(dynamic.type_text, "any");
    assert!(!dynamic.exact);
    assert!(result.behavior.data_incomplete);
}

#[test]
fn case_component_0521_named_imports_and_reexport_aliases_resolve_statically() {
    // Oracle: TS-COMPONENT-0521.
    let result = analysis(
        &[
            (
                "index.ts",
                "import { shared } from \"./shared\";\nComponent({ behaviors: [shared], data: {} });",
            ),
            (
                "shared.ts",
                "const implementation = Behavior({ data: { namedCount: 4 } });\nexport { implementation as shared };",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "namedCount").type_text, "number");
    assert!(
        result.behavior.status.is_supported(),
        "{:?}",
        result.behavior.problems
    );
}

#[test]
fn case_component_0570_only_unresolved_behavior_is_partial_and_invents_no_field() {
    // Oracle: TS-COMPONENT-0570. BLOCKED: checker diagnostics/2339 are not asserted.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ behaviors: [factory()], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(result.behavior.data_incomplete);
    assert!(matches!(
        result.behavior.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(result.data.fields.is_empty());
}

#[test]
fn case_component_0591_commonjs_require_resolves_statically() {
    // Oracle: TS-COMPONENT-0591.
    let result = analysis(
        &[
            (
                "index.ts",
                "const shared = require(\"./shared\");\nComponent({ behaviors: [shared], data: {} });",
            ),
            (
                "shared.js",
                "const shared = Behavior({ data: { importedCount: 7 } });\nmodule.exports = shared;",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "importedCount").type_text, "number");
    assert!(!result.behavior.data_incomplete);
}

#[test]
fn case_component_0654_page_and_component_use_the_selected_registration() {
    // Oracle: TS-COMPONENT-0654.
    let result = analysis(
        &[(
            "index.ts",
            "Page({ data: { pageOnly: \"p\" } });\nComponent({ behaviors: [Behavior({ data: { leaked: 1 } })], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    assert_eq!(
        result.registration.selected.as_ref().map(|fact| fact.kind),
        Some(RegistrationKind::Page)
    );
    assert_eq!(data(&result, "pageOnly").type_text, "string");
    assert!(
        result
            .data
            .fields
            .iter()
            .all(|field| field.name != "leaked")
    );
}

#[test]
fn case_component_0724_import_equals_alias_does_not_hijack_behavior_binding() {
    // Oracle: TS-COMPONENT-0724.
    let result = analysis(
        &[
            (
                "index.ts",
                "import ext = require(\"./ext\");\nimport shared = require(\"./shared\");\nComponent({ behaviors: [shared], data: {} });",
            ),
            (
                "ext.ts",
                "const b = Behavior({ data: { wrong: 1 } }); export = b;",
            ),
            (
                "shared.ts",
                "const b = Behavior({ data: { correct: 2 } }); export = b;",
            ),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "correct").type_text, "number");
    assert!(!result.data.fields.iter().any(|field| field.name == "wrong"));
}

#[test]
fn case_component_0763_non_probe_double_underscore_data_names_remain_usable() {
    // Oracle: TS-COMPONENT-0763.
    let result = analysis(
        &[(
            "index.ts",
            "Component({ behaviors: [Behavior({ data: { __state: 1 } })], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "__state").type_text, "number");
    assert!(!result.behavior.data_incomplete);
}

#[test]
fn case_component_0539_local_const_data_member_keeps_its_static_shape() {
    // Oracle: TS-COMPONENT-0539. Structural: the TS case gets a checked type;
    // this slice reads the same local object literal.
    let result = analysis(
        &[(
            "index.ts",
            "const initial = { label: \"x\" };\nComponent({ behaviors: [Behavior({ data: { fromConst: initial } })], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(data(&result, "fromConst").type_text.contains("label"));
}
#[test]
fn structural_behavior_registrar_alias_resolves() {
    let result = analysis(
        &[(
            "index.ts",
            "const B = Behavior;\nComponent({ behaviors: [B({ data: { aliasField: true } })], data: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert_eq!(data(&result, "aliasField").type_text, "boolean");
    assert!(result.status.is_supported(), "{:?}", result.reasons);
}
#[test]
fn h2_behavior_root_resolves_nested_behaviors_for_data_and_methods() {
    let result = analysis(
        &[(
            "index.ts",
            "Behavior({\n  behaviors: [Behavior({ data: { shared: 1 }, methods: { nested() {} } })],\n  data: { own: 1 },\n  methods: { ownMethod() {} },\n});",
        )],
        "index.ts",
        RegistrationKind::Behavior,
    );
    assert_eq!(data(&result, "shared").type_text, "number");
    assert_eq!(data(&result, "own").type_text, "number");
    let methods = result
        .methods
        .methods
        .iter()
        .map(|method| method.name.as_str())
        .collect::<Vec<_>>();
    assert!(methods.contains(&"nested"));
    assert!(methods.contains(&"ownMethod"));
    assert!(result.status.is_supported(), "{:?}", result.reasons);
}

#[test]
fn h2_behavior_root_dynamic_behaviors_degrade_instead_of_claiming_complete() {
    let dynamic = analysis(
        &[(
            "index.ts",
            "Behavior({ behaviors: [factory()], data: { own: 1 } });",
        )],
        "index.ts",
        RegistrationKind::Behavior,
    );
    assert!(matches!(
        dynamic.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(dynamic.behavior.data_incomplete);
    assert_eq!(data(&dynamic, "own").type_text, "number");

    let non_static = analysis(
        &[(
            "index.ts",
            "Behavior({ behaviors: makeBehaviors(), data: { own: 1 } });",
        )],
        "index.ts",
        RegistrationKind::Behavior,
    );
    assert!(matches!(
        non_static.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(!non_static.behavior.status.is_supported());
}

#[test]
fn h4_own_data_member_processing_is_capped_and_partial() {
    let fields = (0..4_000)
        .map(|index| format!("field{index}: {index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("Page({{ data: {{ {fields} }} }});");
    let start = Instant::now();
    let result = analysis(&[("index.ts", &script)], "index.ts", RegistrationKind::Page);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "4k data fields took {elapsed:?}"
    );
    assert!(result.data.fields.len() <= 512);
    assert!(!result.data.complete);
    assert!(matches!(
        result.data.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}
#[test]
fn h2_page_behaviors_are_resolved_or_degraded_not_claimed_complete() {
    let result = analysis(
        &[(
            "index.ts",
            "Page({ behaviors: [factory()], data: { own: 1 } });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    assert!(matches!(
        result.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
    assert!(!result.behavior.status.is_supported());
    assert_eq!(data(&result, "own").type_text, "number");
    assert!(result.behavior.data_incomplete);
}
#[test]
fn h4_spread_expansion_is_capped_and_partial() {
    let fields = (0..2_000)
        .map(|index| format!("spread{index}: {index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!("const base = {{ {fields} }};\nPage({{ data: {{ ...base }} }});");
    let result = analysis(&[("index.ts", &script)], "index.ts", RegistrationKind::Page);
    assert!(result.data.fields.len() <= 512);
    assert!(!result.data.complete);
    assert!(matches!(
        result.data.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}
