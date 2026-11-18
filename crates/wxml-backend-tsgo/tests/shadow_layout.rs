#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::missing_const_for_fn
)]

//! Integration tests for the in-place shadow layout.
//!
//! Case-prefixed tests name the TypeScript oracle case and the layer they
//! assert. Tests whose name contains `_only_` state that the diagnostic/hover
//! layer is not asserted. Cases that need a real TypeScript checker are listed
//! in [`BLOCKED_CASES`] and are not claimed as migrated.

use wxml_backend_tsgo::input::{DocumentRevision, SourceFile};
use wxml_backend_tsgo::native::strings::Utf16Offset;
use wxml_backend_tsgo::shadow::ShadowError;
use wxml_backend_tsgo::shadow::expression::MAX_SCRIPT_DEPTH;
use wxml_backend_tsgo::shadow::layout::{
    BuildStatus, DataRootKind, FragmentSpec, JsCheckApplied, JsCheckPolicy, RootKind,
    ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::locals::{DEFAULT_LOCAL_BUDGET, LocalBinding, LocalRole};
use wxml_backend_tsgo::shadow::mapping::SourceRange;
use wxml_backend_tsgo::shadow::rewrite::{RewriteOptions, rewrite_expression};

const BLOCKED_CASES: &[(&str, &str)] = &[
    ("TS-LAYOUTS-0062", "diagnostic 2353 requires the checker"),
    (
        "TS-LAYOUTS-0084",
        "hover/documentation requires the checker",
    ),
    ("TS-LAYOUTS-0101", "instance data type requires the checker"),
    ("TS-LAYOUTS-0117", "JSDoc hover requires the checker"),
    (
        "TS-LAYOUTS-0147",
        "diagnostic 2339/hover quality requires the checker",
    ),
    (
        "FEATURE-BEHAVIOR-GRAPH",
        "behavior data/properties overlay resolution is BLOCKED",
    ),
];

fn source_file(path: &str, text: &str) -> SourceFile {
    SourceFile::new(path, text, DocumentRevision::new(1)).expect("valid source file")
}

fn build(
    path: &str,
    text: &str,
    root: RootKind,
    fragments: &[FragmentSpec],
    js_check: JsCheckPolicy,
    check_js: bool,
) -> Result<wxml_backend_tsgo::shadow::layout::BuiltShadow, ShadowError> {
    let script = source_file(path, text);
    let request = ShadowRequest {
        script: &script,
        root,
        fragments,
        js_check,
        check_js,
    };
    build_in_place_shadow(&request)
}

fn utf16_slice(text: &str, start: u32, length: u32) -> String {
    let units: Vec<u16> = text
        .encode_utf16()
        .skip(usize::try_from(start).expect("offset"))
        .take(usize::try_from(length).expect("length"))
        .collect();
    String::from_utf16(&units).expect("valid UTF-16 slice")
}

#[test]
fn case_layouts_0049_page_options_accept_an_injected_probe_member() {
    let built = build(
        "index.ts",
        "Page({\n  data: { n: 1 },\n});\n",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Supported);
    assert_eq!(built.data_root, DataRootKind::Object);
    assert!(
        built
            .view
            .text
            .starts_with("Page({\n    __wxml_check_0() {\n        const __wxml_root = this.data;")
    );
    let root = utf16_slice(&built.view.text, built.view.probe_anchors.root_offset, 5);
    assert_eq!(root, "data;", "`this.data` stays the root expression");

    // The original code is preserved verbatim: removing the probe insertion
    // yields the input text byte-for-byte.
    let probe = built
        .view
        .insertions
        .iter()
        .find(|insertion| insertion.insertion_id == "probe")
        .expect("probe insertion");
    let mut reconstructed: Vec<u16> = built.view.text.encode_utf16().collect();
    let start = usize::try_from(probe.start_utf16).expect("offset");
    let length = usize::try_from(probe.length_utf16).expect("length");
    reconstructed.drain(start..start + length);
    assert_eq!(
        String::from_utf16(&reconstructed).expect("valid text"),
        "Page({\n  data: { n: 1 },\n});\n"
    );
}

#[test]
fn case_layouts_0072_component_methods_accept_the_injected_probe() {
    let text = "Component({\n  properties: {\n    title: { type: String, value: '' },\n  },\n  data: { state: 'idle' },\n  methods: {\n    tap() {},\n  },\n});\n";
    let built = build(
        "index.ts",
        text,
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert!(built
        .view
        .text
        .starts_with("Component({\n  properties: {\n    title: { type: String, value: '' },\n  },\n  data: { state: 'idle' },\n  methods: {\n    __wxml_check_0() {"));
    assert!(
        built
            .view
            .text
            .contains("        const __wxml_root = this.data;\n        type __WxmlProps_0 = {")
    );
}

#[test]
fn layouts_0062_placement_only_diagnostic_blocked() {
    // The TypeScript case asserts diagnostic 2353 for a probe placed directly
    // in Component options. That diagnostic still needs the checker (BLOCKED).
    // This test pins the layout rule that makes it unnecessary: production
    // always mounts inside `methods`, wrapping one when absent.
    let with_methods = build(
        "index.ts",
        "Component({ methods: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    let options_open = with_methods.view.text.find('{').expect("options open");
    let probe = with_methods
        .view
        .text
        .find("__wxml_check_0")
        .expect("probe");
    assert!(
        probe > options_open + 1,
        "probe must not be a direct option member"
    );
    assert!(
        with_methods
            .view
            .text
            .starts_with("Component({ methods: {\n    __wxml_check_0() {")
    );

    let wrapped = build(
        "index.ts",
        "Component({ data: { n: 1 } });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert!(
        wrapped
            .view
            .text
            .starts_with("Component({\n    methods: {\n  \n    __wxml_check_0() {")
    );
    assert!(wrapped.view.text.contains("data: { n: 1 } });"));
}

#[test]
fn case_layouts_0167_optionaltypes_boundary_mapping() {
    let text = "Component({\n  properties: {\n    a: { type: null },\n    b: { type: Object },\n    c: { type: Array },\n    d: { type: Function },\n    e: { type: Number, optionalTypes: [String, Number] },\n  },\n  data: {},\n});\n";
    let built = build(
        "index.ts",
        text,
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert!(built.properties.complete, "{:?}", built.properties.problems);
    let types: std::collections::BTreeMap<&str, &str> = built
        .properties
        .properties
        .iter()
        .map(|property| (property.name.as_str(), property.type_text.as_str()))
        .collect();
    assert_eq!(types.get("a"), Some(&"any"));
    assert_eq!(types.get("b"), Some(&"Record<string, any>"));
    assert_eq!(types.get("c"), Some(&"any[]"));
    assert_eq!(types.get("d"), Some(&"(...args: any[]) => any"));
    assert_eq!(types.get("e"), Some(&"number | string"));
}

#[test]
fn case_layouts_0190_unknown_property_shapes_degrade_instead_of_guessing() {
    let text = "Component({\n  properties: {\n    a: { type: SomeConstructor },\n    b: { type: String },\n  },\n  data: {},\n});\n";
    let built = build(
        "index.ts",
        text,
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert!(!built.properties.complete);
    assert!(
        built
            .properties
            .problems
            .iter()
            .any(|problem| problem.contains("no readable"))
    );
    assert_eq!(
        built
            .properties
            .properties
            .iter()
            .map(|property| property.name.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
    assert_eq!(built.status, BuildStatus::Partial);
}

#[test]
fn case_layouts_0206_non_literal_properties_is_reported_never_narrowed_silently() {
    let built = build(
        "index.ts",
        "Component({ properties: sharedProperties, data: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(!built.properties.complete);
    assert!(
        built
            .properties
            .problems
            .iter()
            .any(|problem| problem.contains("not an object literal"))
    );
    assert!(built.properties.properties.is_empty());
}

#[test]
fn case_layouts_0216_the_mounted_target_is_a_whole_identifier_outside_string_literals() {
    let rewritten = rewrite_expression(
        "user[\"name\"].name",
        Some("name"),
        &RewriteOptions::default(),
    )
    .expect("valid expression");
    assert_eq!(rewritten.text, "__wxml_root.user[\"name\"].name");
    let target = utf16_slice(
        &rewritten.text,
        rewritten.target_offset_utf16,
        u32::try_from("name".len()).expect("length"),
    );
    assert_eq!(target, "name");
    let offset = usize::try_from(rewritten.target_offset_utf16).expect("offset");
    assert_eq!(&rewritten.text[offset..offset + 4], "name");
    assert_eq!(
        rewritten.text.find("name").expect("first name"),
        18,
        "the string literal occurrence is skipped"
    );

    assert!(
        rewrite_expression("\"name\".length", Some("name"), &RewriteOptions::default()).is_err()
    );
    assert!(rewrite_expression("user.name", Some("nmae"), &RewriteOptions::default()).is_err());
}

#[test]
fn case_layouts_0224_quoted_property_names_keep_the_definition_target_inside_the_quotes() {
    let built = build(
        "index.ts",
        "Component({\n  properties: {\n    \"a-b\": { type: String, optionalTypes: [Number], value: \"\" },\n  },\n  data: {},\n});\n",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    let declaration = built
        .view
        .generated_declarations
        .iter()
        .find(|declaration| declaration.name == "a-b")
        .expect("generated declaration");
    assert_eq!(
        utf16_slice(
            &built.view.text,
            declaration.start_utf16,
            declaration.length_utf16
        ),
        "a-b"
    );
    let offset = usize::try_from(declaration.start_utf16).expect("offset");
    assert_eq!(&built.view.text[offset - 1..offset], "\"");
    assert_eq!(&built.view.text[offset + 3..offset + 4], "\"");
}

#[test]
fn unsupported_registration_shapes_are_typed() {
    for (text, expected_feature) in [
        ("const x = 1;", "registration"),
        (
            "const Page = createPage();\nPage({ data: {} });",
            "registration",
        ),
        ("Page(sharedOptions);", "registration"),
        ("function f(Page) { Page({ data: {} }); }", "registration"),
        ("Component({ methods: shared });", "component.methods"),
    ] {
        let error = build(
            "index.ts",
            text,
            if text.starts_with("Component") {
                RootKind::Component
            } else {
                RootKind::Page
            },
            &[],
            JsCheckPolicy::Inherit,
            false,
        )
        .expect_err("unsupported layout");
        assert!(
            matches!(
                error,
                ShadowError::Unsupported { feature, .. } if feature == expected_feature
            ),
            "{text:?}: {error}"
        );
    }
}

#[test]
fn multiple_registrations_are_partial_and_choose_the_first() {
    let built = build(
        "index.ts",
        "Page({ data: { n: 1 } });\nPage({ data: { m: 2 } });",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Partial);
    assert_eq!(built.registration.count, 2);
    assert!(
        built
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("2 page registrations"))
    );
    assert_eq!(built.view.mounts.len(), 0);
}

#[test]
fn behavior_registration_uses_component_rules() {
    let built = build(
        "index.js",
        "Behavior({ methods: {} });",
        RootKind::Behavior,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert!(
        built
            .view
            .text
            .starts_with("Behavior({ methods: {\n    __wxml_check_0() {")
    );
    assert_eq!(built.registration.kind, RootKind::Behavior);
}

#[test]
fn build_is_deterministic_and_keeps_lf_crlf_verbatim() {
    let text = "Page({\r\n  data: { n: 1 },\r\n});\r\n";
    let first = build(
        "index.ts",
        text,
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    let second = build(
        "index.ts",
        text,
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(
        first.view, second.view,
        "the whole derived view is deterministic"
    );
    assert!(
        first
            .view
            .text
            .ends_with("\r\n  data: { n: 1 },\r\n});\r\n")
    );
}

#[test]
fn layouts_0117_text_overlay_only_hover_blocked() {
    let built = build(
        "index.js",
        "// @ts-check
Component({\n  properties: {\n    /** 标题，支持字符串或数字 */\n    title: { type: String, optionalTypes: [Number], value: '' },\n  },\n  methods: {},\n});\n",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        true,
    )
    .expect("layout");
    assert!(
        built
            .view
            .text
            .contains(" * @typedef {Object} __WxmlProps_0")
    );
    assert!(
        built
            .view
            .text
            .contains(" * @property {string | number} title 标题，支持字符串或数字")
    );
    assert!(
        built
            .view
            .text
            .contains("const __wxml_props_0 = /** @type {__WxmlProps_0} */ (__wxml_props_raw_0);")
    );
    assert!(
        !built
            .view
            .text
            .contains("const __wxml_props_0 = null as unknown as")
    );
}

#[test]
fn behaviors_make_the_build_partial_instead_of_guessing() {
    let built = build(
        "index.ts",
        "Component({ behaviors: [sharedBehavior], data: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(
        built
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("behaviors"))
    );
}

#[test]
fn an_empty_behavior_list_is_not_incomplete() {
    let built = build(
        "index.ts",
        "Component({ behaviors: [], data: {} });",
        RootKind::Component,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Supported);
}

#[test]
fn a_page_route_registered_with_component_uses_the_component_rules() {
    let built = build(
        "index.ts",
        "Component({ data: { n: 1 } });",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.registration.kind, RootKind::Component);
    assert!(built.view.text.starts_with("Component({\n    methods: {"));
}

#[test]
fn data_absent_is_partial_and_keeps_the_empty_root() {
    let built = build(
        "index.ts",
        "Page({});",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.data_root, DataRootKind::Absent);
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(
        built
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no `data` member"))
    );
}

#[test]
fn source_ts_check_is_neutralised_when_the_policy_is_off() {
    let built = build(
        "index.js",
        "// @ts-check\nPage({ data: {} });",
        RootKind::Page,
        &[],
        JsCheckPolicy::Off,
        false,
    )
    .expect("layout");
    assert_eq!(built.js_check_applied, JsCheckApplied::Off);
    assert!(built.view.text.contains("// @ts-check"));
    assert!(built.view.text.contains("// @ts-nocheck"));
    assert!(built.notes.iter().any(|note| note.contains("neutralised")));
}

#[test]
fn scripts_are_not_subject_to_the_expression_node_budget() {
    let mut text = String::from("Page({ data: {");
    for index in 0..6_000 {
        text.push_str("key");
        text.push_str(&index.to_string());
        text.push_str(": 1,");
    }
    text.push_str("} });");
    let built = build(
        "index.ts",
        &text,
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("large script");
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(built.view.text.contains("__wxml_check_0"));
}

#[test]
fn malformed_scripts_are_invalid_not_expressions() {
    let error = build(
        "index.ts",
        "Page({ data: {}",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect_err("malformed script");
    assert!(matches!(
        error,
        ShadowError::Invalid {
            feature: "script.scan",
            ..
        }
    ));
}

#[test]
fn registration_argument_must_be_the_entire_object_literal() {
    for text in [
        "Page({ data: {} } as const);",
        "Page({ data: {} } satisfies Options);",
        "Page({ data: {} }.foo);",
        "Page(({ data: {} }));",
        "Page(cond ? {} : {});",
        "Page({ data: {} }!);",
    ] {
        let error = build(
            "index.ts",
            text,
            RootKind::Page,
            &[],
            JsCheckPolicy::Inherit,
            false,
        )
        .expect_err("non-whole argument must be rejected");
        assert!(
            matches!(
                error,
                ShadowError::Unsupported {
                    feature: "registration",
                    ..
                }
            ),
            "{text}: {error}"
        );
    }
    // A trailing argument or trailing comma keeps the first argument intact.
    for text in ["Page({ data: {} }, extra);", "Page({ data: {} },);"] {
        let built = build(
            "index.ts",
            text,
            RootKind::Page,
            &[],
            JsCheckPolicy::Inherit,
            false,
        )
        .expect("whole first argument");
        assert_eq!(built.status, BuildStatus::Supported);
    }
}

#[test]
fn asi_imports_do_not_shadow_the_registrar() {
    for text in [
        "import x from './x'\nPage({ data: {} });",
        "import './side-effect'\nPage({ data: {} });",
        "import type { Options } from './types'\nPage({ data: {} });",
        "import {\n  Component,\n} from './x'\nPage({ data: {} });",
        "if (Page) { Page({ data: {} }); }",
        "while (Page) { Page({ data: {} }); }",
        "switch (Page) { default: Page({ data: {} }); }",
        "const modulePromise = import('./x')\nPage({ data: {} });",
    ] {
        let built = build(
            "index.ts",
            text,
            RootKind::Page,
            &[],
            JsCheckPolicy::Inherit,
            false,
        )
        .unwrap_or_else(|error| panic!("{text}: {error}"));
        assert_eq!(built.status, BuildStatus::Supported, "{text}");
    }
    for text in [
        "function f(Page) { Page({ data: {} }); }",
        "(Page) => Page({ data: {} });",
        "import Page from './page';\nPage({ data: {} });",
        "import { Page } from './page';\nPage({ data: {} });",
        "import Page = require('./page');\nPage({ data: {} });",
    ] {
        let error = build(
            "index.ts",
            text,
            RootKind::Page,
            &[],
            JsCheckPolicy::Inherit,
            false,
        )
        .expect_err("shadowed registrar");
        assert!(
            matches!(
                error,
                ShadowError::Unsupported {
                    feature: "registration",
                    ..
                }
            ),
            "{text}: {error}"
        );
    }
}

#[test]
fn many_semicolonless_imports_scan_in_bounded_time() {
    let mut text = String::new();
    for _ in 0..20_000 {
        text.push_str("import x\n");
    }
    text.push_str("Page({ data: {} });");
    let built = build(
        "index.ts",
        &text,
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("ASI imports");
    assert_eq!(built.status, BuildStatus::Supported);
}

#[test]
fn malformed_single_line_import_scan_is_bounded() {
    let text = format!("{}Page({{ data: {{}} }});", "import ".repeat(5_000));
    let error = build(
        "index.ts",
        &text,
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect_err("malformed import fails closed");
    assert!(matches!(
        error,
        ShadowError::Unsupported {
            feature: "registration",
            ..
        }
    ));
}

#[test]
fn deeply_nested_parens_are_bounded() {
    let allowed_depth = MAX_SCRIPT_DEPTH - 8;
    let allowed = format!(
        "{}Page({{ data: {{}} }}){}",
        "(".repeat(allowed_depth),
        ")".repeat(allowed_depth)
    );
    let built = build(
        "index.ts",
        &allowed,
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("depth at the script limit");
    assert_eq!(built.status, BuildStatus::Supported);

    let rejected = format!(
        "{}Page({{ data: {{}} }}){}",
        "(".repeat(MAX_SCRIPT_DEPTH + 1),
        ")".repeat(MAX_SCRIPT_DEPTH + 1)
    );
    assert!(matches!(
        build(
            "index.ts",
            &rejected,
            RootKind::Page,
            &[],
            JsCheckPolicy::Inherit,
            false
        ),
        Err(ShadowError::Invalid {
            feature: "script.scan",
            ..
        })
    ));
}

#[test]
fn invalid_legacy_locals_are_dropped_and_reported_partial() {
    let mut spec = FragmentSpec::new("locals", "item.name", SourceRange::new(0, 9));
    spec.locals = vec![
        "item".to_owned(),
        "item".to_owned(),
        "class".to_owned(),
        "bad-name".to_owned(),
        "x); alert(1); (".to_owned(),
        "type".to_owned(),
    ];
    let built = build(
        "index.ts",
        "Page({ data: { item: { name: 'x' } } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(built.view.text.contains("const item = undefined as any;"));
    assert!(built.view.text.contains("const type = undefined as any;"));
    assert!(!built.view.text.contains("const class"));
    assert!(!built.view.text.contains("const bad-name"));
    assert!(!built.view.text.contains("alert(1)"));
    assert!(
        built
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("invalid local"))
    );
}

#[test]
fn legacy_locals_are_bounded() {
    let mut spec = FragmentSpec::new("locals", "item.name", SourceRange::new(0, 9));
    spec.locals = (0..=DEFAULT_LOCAL_BUDGET.max_bindings)
        .map(|index| format!("item{index}"))
        .collect();
    let error = build(
        "index.ts",
        "Page({ data: { item: { name: 'x' } } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect_err("legacy locals budget");
    assert!(matches!(
        error,
        ShadowError::BudgetExceeded { budget: "locals" }
    ));
}

#[test]
fn invalid_bindings_are_dropped_and_reported_partial() {
    let mut spec = FragmentSpec::new("bindings", "item.name", SourceRange::new(0, 9));
    spec.bindings = vec![LocalBinding {
        name: "class".to_owned(),
        role: LocalRole::Item,
        source_expr: "users".to_owned(),
    }];
    let built = build(
        "index.ts",
        "Page({ data: { users: [{ name: 'x' }] } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(!built.view.text.contains("const class"));
    assert!(
        built
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("invalid `wx:for`"))
    );
}

#[test]
fn type_is_allowed_as_a_loop_binding_name() {
    let mut spec = FragmentSpec::new("bindings", "type.name", SourceRange::new(0, 9));
    spec.bindings = vec![LocalBinding {
        name: "type".to_owned(),
        role: LocalRole::Item,
        source_expr: "users".to_owned(),
    }];
    let built = build(
        "index.ts",
        "Page({ data: { users: [{ name: 'x' }] } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("layout");
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(
        built
            .view
            .text
            .contains("const type = __wxml_item(__wxml_root.users);")
    );
    assert!(built.view.text.contains("type.name;"));
}

#[test]
fn data_field_named_type_is_mounted_normally() {
    let mut spec = FragmentSpec::new("type", "type", SourceRange::new(0, 4));
    spec.target = Some("type".to_owned());
    let built = build(
        "index.ts",
        "Page({ data: { type: 'x' } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("type is an ordinary data field");
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(built.view.text.contains("__wxml_root.type;"));
}

#[test]
fn keyword_like_object_key_and_member_targets_are_not_rejected() {
    let mut keyed = FragmentSpec::new("keyed", "{ type: value }", SourceRange::new(0, 15));
    keyed.target = Some("value".to_owned());
    let built = build(
        "index.ts",
        "Page({ data: { value: 'x' } });",
        RootKind::Page,
        &[keyed],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("type key");
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(built.view.text.contains("{ type: __wxml_root.value };"));
}

#[test]
fn hashbang_scripts_are_scanned_without_hashbang_tokens() {
    let built = build(
        "index.js",
        "#!/usr/bin/env node\nPage({ data: {} });",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    )
    .expect("hashbang script");
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(built.view.text.contains("__wxml_check_0"));
}

#[test]
fn blocked_cases_are_documented_and_not_claimed() {
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, reason)| *case == "TS-LAYOUTS-0084" && reason.contains("checker"))
    );
    assert!(
        !BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-LAYOUTS-0049")
    );
    assert_eq!(Utf16Offset::new(0).get(), 0);
}
