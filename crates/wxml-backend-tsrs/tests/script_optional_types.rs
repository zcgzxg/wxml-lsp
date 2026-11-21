#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_const_for_fn,
    clippy::assert_is_empty,
    clippy::too_many_lines
)]

//! `type` + `optionalTypes` union semantics and their shadow/hover wiring.
//!
//! The Node overlay reads `type` first, then every `optionalTypes` entry in
//! array order, removes duplicate texts and maps `null` to `any`. These tests
//! freeze the same text for the Rust script overlay, including behavior
//! contributions, and prove that the declaration-derived shadow type uses it.

use std::path::{Path, PathBuf};

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, MINIPROGRAM_TYPINGS, ProjectKey, ProjectRoot,
    SnapshotRevision, SourceFile, TypeAssets,
};
use wxml_backend_tsrs::script::ScriptAnalysis;
use wxml_backend_tsrs::script::ScriptRequest;
use wxml_backend_tsrs::script::analyze_script;
use wxml_backend_tsrs::script::registration::RegistrationKind;
use wxml_backend_tsrs::shadow::layout::{
    BehaviorPropertyOverlay, BuildStatus, FragmentSpec, RootKind, ShadowRequest,
    build_in_place_shadow, build_in_place_shadow_with_behavior,
};
use wxml_backend_tsrs::shadow::mapping::SourceRange;

#[path = "support/typings_fixture.rs"]
mod typings_fixture;

/// Whether the real-engine gate must fail instead of skipping.
fn require_real_engine() -> bool {
    std::env::var("WXML_REQUIRE_REAL_ENGINE")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Normalizes `TSRS_TYPINGS_ROOT`, accepting either the parent root or the
/// `miniprogram-api-typings` package directory itself.
fn normalize_typings_root(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some(MINIPROGRAM_TYPINGS) {
        path.parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    } else {
        path.to_path_buf()
    }
}

/// A validated typings root plus the staged copy that keeps it alive.
struct RealTypings {
    _root: PathBuf,
    _staged: Option<typings_fixture::Staged>,
}

fn prepare_real_typings() -> Option<RealTypings> {
    let required = require_real_engine();
    let (root, staged) = if let Some(raw) = std::env::var_os("TSRS_TYPINGS_ROOT") {
        (normalize_typings_root(Path::new(&raw)), None)
    } else if let Some(staged) = typings_fixture::stage() {
        (staged.root().to_path_buf(), Some(staged))
    } else {
        assert!(
            !required,
            "TSRS_TYPINGS_ROOT is required when WXML_REQUIRE_REAL_ENGINE=1; \
             point it at miniprogram-api-typings or its parent directory"
        );
        eprintln!(
            "real-engine optionalTypes test skipped: set TSRS_TYPINGS_ROOT to \
             miniprogram-api-typings or its parent directory \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return None;
    };
    if let Err(error) = TypeAssets::load_pinned_typings(&root) {
        assert!(
            !required,
            "invalid typings under {} when WXML_REQUIRE_REAL_ENGINE=1: {error}",
            root.display()
        );
        eprintln!(
            "real-engine optionalTypes test skipped: cannot verify typings under {}: {error}",
            root.display()
        );
        return None;
    }
    Some(RealTypings {
        _root: root,
        _staged: staged,
    })
}

fn analysis(files: &[(&str, &str)], path: &str, kind: RegistrationKind) -> ScriptAnalysis {
    let files = files
        .iter()
        .map(|(file_path, text)| {
            SourceFile::new(*file_path, *text, DocumentRevision::new(1)).expect("valid source")
        })
        .collect();
    let input = AnalysisInput::new(
        ProjectKey::new("optional-types"),
        ProjectRoot::new("/work/optional-types").expect("valid root"),
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

fn component(declarations: &str) -> ScriptAnalysis {
    let source = format!("Component({{ properties: {{ {declarations} }} }});");
    analysis(
        &[("index.ts", &source)],
        "index.ts",
        RegistrationKind::Component,
    )
}

#[test]
fn single_type_maps_every_supported_constructor() {
    let result = component(
        "s: { type: String }, n: { type: Number }, b: { type: Boolean }, a: { type: Array }, o: { type: Object }, f: { type: Function }, z: { type: null }",
    );
    assert_eq!(property(&result, "s").type_text, "string");
    assert_eq!(property(&result, "n").type_text, "number");
    assert_eq!(property(&result, "b").type_text, "boolean");
    assert_eq!(property(&result, "a").type_text, "any[]");
    assert_eq!(property(&result, "o").type_text, "Record<string, any>");
    assert_eq!(property(&result, "f").type_text, "(...args: any[]) => any");
    assert_eq!(property(&result, "z").type_text, "any");
    for name in ["s", "n", "b", "a", "o", "f", "z"] {
        assert!(!property(&result, name).has_optional_types, "{name}");
        assert!(property(&result, name).type_complete, "{name}");
    }
}

#[test]
fn optional_types_single_and_duplicate_keep_node_order() {
    let result = component(
        "one: { type: String, optionalTypes: [Number] }, same: { type: String, optionalTypes: [String] }, twice: { type: String, optionalTypes: [Number, Number] }",
    );
    assert_eq!(property(&result, "one").type_text, "string | number");
    assert!(property(&result, "one").has_optional_types);
    // The primary `type` wins the duplicate and only one text is emitted.
    assert_eq!(property(&result, "same").type_text, "string");
    assert_eq!(property(&result, "twice").type_text, "string | number");
}

#[test]
fn optional_types_multiple_and_number_plus_string_order_is_preserved() {
    let result = component(
        "a: { type: String, optionalTypes: [Number, Boolean] }, b: { type: Number, optionalTypes: [String] }, c: { type: Boolean, optionalTypes: [Number, Object, Array] }",
    );
    assert_eq!(
        property(&result, "a").type_text,
        "string | number | boolean"
    );
    assert_eq!(property(&result, "b").type_text, "number | string");
    assert_eq!(
        property(&result, "c").type_text,
        "boolean | number | Record<string, any> | any[]"
    );
}

#[test]
fn null_and_undefined_match_the_node_constructor_lookup() {
    let result = component(
        "with_null: { type: String, optionalTypes: [null] }, null_first: { type: null, optionalTypes: [String] }, undefined_entry: { type: String, optionalTypes: [undefined] }, undefined_type: { type: undefined, optionalTypes: [Number] }",
    );
    assert_eq!(property(&result, "with_null").type_text, "string | any");
    assert_eq!(property(&result, "null_first").type_text, "any | string");
    // `undefined` contributes nothing but is a known value, not a guess.
    assert_eq!(property(&result, "undefined_entry").type_text, "string");
    assert!(property(&result, "undefined_entry").type_complete);
    assert_eq!(property(&result, "undefined_type").type_text, "number");
}

#[test]
fn unreadable_entries_degrade_without_inventing_a_union() {
    let result = component("g: { type: SomeConstructor, optionalTypes: [Number] }");
    let property = property(&result, "g");
    assert_eq!(property.type_text, "number");
    assert!(!property.type_complete);
    assert!(matches!(
        result.properties.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn no_optional_types_keeps_the_plain_type_text() {
    let result = component("plain: { type: String, value: '' }");
    let property = property(&result, "plain");
    assert_eq!(property.type_text, "string");
    assert!(!property.has_optional_types);
    assert!(property.type_complete);
}

#[test]
fn behavior_properties_keep_optional_types_and_jsdoc() {
    let result = analysis(
        &[(
            "components/card/card.ts",
            r"Component({
  behaviors: [Behavior({
    properties: {
      /** 卡片标题，支持字符串或数字 */
      cardTitle: { type: String, optionalTypes: [Number] },
    },
  })],
  data: {},
});",
        )],
        "components/card/card.ts",
        RegistrationKind::Component,
    );
    let property = property(&result, "cardTitle");
    assert_eq!(property.type_text, "string | number");
    assert!(property.type_complete);
    assert_eq!(property.documentation, "卡片标题，支持字符串或数字");
    assert_eq!(property.path, "components/card/card.ts");
    assert!(result.properties.complete);
    assert!(result.behavior.status.is_supported());
}

#[test]
fn own_property_overrides_a_behavior_property() {
    let result = analysis(
        &[(
            "components/card/card.ts",
            r"Component({
  behaviors: [Behavior({
    properties: { cardTitle: { type: String, optionalTypes: [Number] } },
  })],
  properties: { cardTitle: { type: Boolean } },
});",
        )],
        "components/card/card.ts",
        RegistrationKind::Component,
    );
    let property = property(&result, "cardTitle");
    assert_eq!(property.type_text, "boolean");
    assert!(property.own_override);
}

#[test]
fn imported_behavior_property_optional_types_resolve_across_files() {
    let result = analysis(
        &[
            (
                "components/card/card.ts",
                r#"import shared from "./shared";
Component({ behaviors: [shared], data: {} });"#,
            ),
            (
                "components/card/shared.ts",
                r"export default Behavior({
  properties: { cardTitle: { type: String, optionalTypes: [Number] } },
});",
            ),
        ],
        "components/card/card.ts",
        RegistrationKind::Component,
    );
    let property = property(&result, "cardTitle");
    assert_eq!(property.type_text, "string | number");
    assert_eq!(property.path, "components/card/shared.ts");
    assert!(result.properties.complete);
}

#[test]
fn dynamic_behavior_spread_stays_partial_and_does_not_fabricate_fields() {
    let result = analysis(
        &[(
            "components/card/card.ts",
            r"const behaviors = pickBehaviors();
Component({ behaviors: [...behaviors], data: {} });",
        )],
        "components/card/card.ts",
        RegistrationKind::Component,
    );
    assert!(result.properties.properties.is_empty());
    assert!(!result.properties.complete);
    assert!(!result.behavior.status.is_supported());
    assert!(matches!(
        result.properties.status,
        wxml_types::CapabilityStatus::Partial { .. }
    ));
}

#[test]
fn emoji_and_cjk_documentation_survive_the_union_overlay() {
    let result = component(
        "/** 标题 🚀，支持 string 与 number */ title: { type: String, optionalTypes: [Number] }",
    );
    let property = property(&result, "title");
    assert_eq!(property.documentation, "标题 🚀，支持 string 与 number");
    assert_eq!(property.type_text, "string | number");
}

fn shadow_request<'a>(script: &'a SourceFile, fragments: &'a [FragmentSpec]) -> ShadowRequest<'a> {
    let mut request = ShadowRequest::new(script, RootKind::Component);
    request.fragments = fragments;
    request
}

fn fragment(expr: &str, start: u32) -> FragmentSpec {
    FragmentSpec::new(
        "f0",
        expr,
        SourceRange::new(start, start + u32::try_from(expr.len()).expect("length")),
    )
}

#[test]
fn shadow_behavior_properties_use_the_synthesized_union() {
    let source = r"Component({
  behaviors: [Behavior({
    data: { sharedCount: 3 },
    properties: {
      /** 共享标题 */
      cardTitle: { type: String, optionalTypes: [Number] },
    },
  })],
  data: {},
});";
    let analysis = analysis(
        &[("components/card/card.ts", source)],
        "components/card/card.ts",
        RegistrationKind::Component,
    );
    let overlay = BehaviorPropertyOverlay {
        properties: &analysis.behavior.properties,
        complete: !analysis.behavior.properties_incomplete
            && analysis.behavior.status.is_supported(),
        reason: analysis.behavior.status.reason(),
    };
    let script = SourceFile::new("components/card/card.ts", source, DocumentRevision::new(1))
        .expect("valid source");
    let fragments = [fragment("cardTitle", 0)];
    let request = shadow_request(&script, &fragments);
    let built = build_in_place_shadow_with_behavior(&request, Some(&overlay)).expect("shadow");
    assert!(
        built.view.text.contains("cardTitle: string | number;"),
        "{}",
        built.view.text
    );
    assert!(built.view.text.contains("__wxml_props_0.cardTitle"));
    assert!(
        built
            .reason
            .as_deref()
            .is_none_or(|reason| { !reason.contains("behaviors are partially resolved") })
    );
    assert_eq!(built.status, BuildStatus::Supported);
}

#[test]
fn shadow_without_behavior_overlay_keeps_the_partial_contract() {
    let source =
        "Component({ behaviors: [Behavior({ properties: { a: { type: String } } })], data: {} });";
    let script =
        SourceFile::new("index.ts", source, DocumentRevision::new(1)).expect("valid source");
    let fragments = [fragment("a", 0)];
    let request = shadow_request(&script, &fragments);
    let built = build_in_place_shadow(&request).expect("shadow");
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(
        built
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("behaviors are partially resolved"))
    );
}

/// Real-engine proof that the declaration-derived shadow type is readable.
///
/// Opt-in through `TSRS_TYPINGS_ROOT`; `WXML_REQUIRE_REAL_ENGINE=1` turns
/// missing or invalid typings into a hard failure. The in-process engine
/// supplies the standard library. The minimal `Component`/`Behavior` shims are
/// enough because the hovered type is the generated local alias, not the
/// mini-program instance type.
#[test]
fn real_engine_hover_reads_the_generated_union() {
    use wxml_backend_tsrs::native::session::{NativeSession, SessionOptions};
    use wxml_backend_tsrs::native::strings::{Utf16Offset, offset_to_position, utf16_len};

    let Some(_typings) = prepare_real_typings() else {
        return;
    };

    let source = r"Component({
  behaviors: [Behavior({
    properties: {
      /** 共享标题 */
      cardTitle: { type: String, optionalTypes: [Number] },
    },
  })],
  data: {},
});";
    let analysis = analysis(
        &[("components/card/card.ts", source)],
        "components/card/card.ts",
        RegistrationKind::Component,
    );
    let overlay = BehaviorPropertyOverlay {
        properties: &analysis.behavior.properties,
        complete: true,
        reason: None,
    };
    let script = SourceFile::new("components/card/card.ts", source, DocumentRevision::new(1))
        .expect("valid source");
    let fragments = [fragment("cardTitle", 0)];
    let request = shadow_request(&script, &fragments);
    let built = build_in_place_shadow_with_behavior(&request, Some(&overlay)).expect("shadow");
    let derived = built.view.text;

    let shims = "declare function Component(options: any): void;\ndeclare function Behavior(options: any): any;\n";
    let project_root = format!("/tmp/opencode/optional-types-{}", std::process::id());
    let input = AnalysisInput::new(
        ProjectKey::new("optional-types-real"),
        ProjectRoot::new(&project_root).expect("project root"),
        SnapshotRevision::new(1),
        vec![
            SourceFile::new("main.ts", derived.clone(), DocumentRevision::new(1)).expect("main.ts"),
            SourceFile::new("globals.d.ts", shims, DocumentRevision::new(1)).expect("globals.d.ts"),
        ],
    )
    .expect("analysis input");
    let sandbox =
        std::env::temp_dir().join(format!("optional-types-sandbox-{}", std::process::id()));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let mut session = NativeSession::start(SessionOptions::new(PathBuf::new(), sandbox.clone()))
        .expect("native session");
    session.open_project(&input).expect("open project");

    let mount = derived.find("__wxml_props_0.cardTitle").expect("mount");
    let cursor = mount + "__wxml_props_0.cardTitle".len() - 1;
    let units = utf16_len(derived.get(..cursor).expect("char boundary"));
    let position = offset_to_position(&derived, Utf16Offset::new(units)).expect("position");
    let evidence = session
        .hover_evidence("main.ts", position)
        .expect("hover evidence");
    let text = evidence
        .type_info
        .as_ref()
        .map_or_default(|info| info.text.as_str());
    assert_eq!(text, "string | number", "{evidence:?}");
    assert!(
        evidence
            .documentation
            .as_ref()
            .and_then(|docs| docs.comment.as_deref())
            .is_some_and(|comment| comment.contains("共享标题")),
        "{evidence:?}"
    );
    session.shutdown().expect("clean shutdown");
    let _ = std::fs::remove_dir_all(&sandbox);
}
