#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_const_for_fn,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::cast_possible_truncation
)]

//! Symbol model and rename safety.
//!
//! The slice only proves original-token symbols in the analyzed file. Imported
//! behavior symbols and any incomplete overlay are unsafe and produce no edit
//! plan; this is deliberately stricter than a text search.

use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::script::registration::RegistrationKind;
use wxml_backend_tsgo::script::symbols::{DefinitionOrigin, RenameBlock, SymbolKind};
use wxml_backend_tsgo::script::{ScriptAnalysis, ScriptRequest, analyze_script};

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

#[test]
fn safe_own_symbols_keep_utf8_and_utf16_ranges_and_expected_text() {
    let script =
        "const note = \"中文注释😀\";\nPage({\n  onTap() {},\n  data: { title: \"x\" },\n});";
    let result = analysis(&[("index.ts", script)], "index.ts", RegistrationKind::Page);
    let method = result
        .symbol("onTap", SymbolKind::Method)
        .expect("method symbol");
    assert!(method.rename_safe());
    let byte_start = script.find("onTap()").expect("method") as u32;
    let utf16_start = script[..script.find("onTap()").expect("method")]
        .encode_utf16()
        .count() as u32;
    assert_eq!(method.range.utf8.start.get(), byte_start);
    assert_eq!(method.range.utf16.start.get(), utf16_start);
    let edit = method.rename_edit("onLoad").expect("safe edit");
    assert_eq!(edit.expected_text, "onTap");
    assert_eq!(edit.new_text, "onLoad");
    assert_eq!(edit.range, method.range);

    let data = result
        .symbol("title", SymbolKind::Data)
        .expect("data symbol");
    assert!(data.rename_safe());
    assert_eq!(data.type_text.as_deref(), Some("string"));
}

#[test]
fn incomplete_data_evidence_is_unsafe_for_rename() {
    let result = analysis(
        &[("index.ts", "Page({ data: { dynamic: makeValue() } })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let dynamic = result
        .symbol("dynamic", SymbolKind::Data)
        .expect("field is known, its type is not");
    assert!(!dynamic.rename_safe());
    assert!(
        dynamic
            .safety
            .blocks
            .contains(&RenameBlock::IncompleteEvidence)
    );
    assert!(dynamic.rename_edit("renamed").is_none());
}

#[test]
fn quoted_property_keys_are_unsafe() {
    let result = analysis(
        &[(
            "index.ts",
            "Component({ properties: { \"card-title\": { type: String } } })",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    let property = result
        .symbol("card-title", SymbolKind::Property)
        .expect("quoted symbol");
    assert!(!property.rename_safe());
    assert!(property.safety.blocks.contains(&RenameBlock::QuotedKey));
    assert!(property.rename_edit("cardTitle").is_none());
}

#[test]
fn ambiguous_registrations_make_every_symbol_unsafe() {
    let result = analysis(
        &[(
            "index.ts",
            "Page({ onTap() {} });\nPage({ data: { title: \"x\" } });",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    for symbol in &result.symbols {
        assert!(!symbol.rename_safe(), "{symbol:?}");
        assert!(symbol.rename_edit("renamed").is_none());
    }
}

#[test]
fn imported_behavior_symbols_point_at_the_behavior_file_and_are_cross_file_unsafe() {
    let behavior = "export default Behavior({\n  data: {\n    /** 共享计数 */\n    sharedCount: 3,\n  },\n  methods: {\n    /** 返回上一页 */\n    onBack() {},\n  },\n});";
    let result = analysis(
        &[
            (
                "index.ts",
                "import base from \"./base\";\nComponent({ behaviors: [base], data: {}, methods: {} });",
            ),
            ("base.ts", behavior),
        ],
        "index.ts",
        RegistrationKind::Component,
    );
    let field = result
        .symbol("sharedCount", SymbolKind::Data)
        .expect("behavior data symbol");
    assert_eq!(
        field.origin,
        DefinitionOrigin::ImportedBehavior {
            path: "base.ts".to_owned(),
            alias: Some("base".to_owned()),
        }
    );
    assert_eq!(field.documentation, "共享计数");
    assert!(
        field
            .safety
            .blocks
            .contains(&RenameBlock::CrossFileEvidence)
    );
    assert!(field.rename_edit("renamed").is_none());

    let method = result
        .symbol("onBack", SymbolKind::BehaviorMethod)
        .expect("behavior method symbol");
    assert_eq!(method.documentation, "返回上一页");
    assert_eq!(
        method.origin,
        DefinitionOrigin::ImportedBehavior {
            path: "base.ts".to_owned(),
            alias: Some("base".to_owned()),
        }
    );
    assert!(method.rename_edit("goBack").is_none());
}

#[test]
fn missing_behavior_produces_no_symbol_at_all() {
    let result = analysis(
        &[(
            "index.ts",
            "import base from \"./missing\";\nComponent({ behaviors: [base], methods: {} });",
        )],
        "index.ts",
        RegistrationKind::Component,
    );
    assert!(
        result
            .symbols
            .iter()
            .all(|symbol| symbol.kind != SymbolKind::BehaviorMethod)
    );
    assert!(result.symbols.iter().all(|symbol| symbol.name != "onBack"));
}

#[test]
fn registration_symbol_is_not_a_user_rename_target() {
    let result = analysis(
        &[("index.ts", "Page({ data: {} })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let registration = result
        .symbol("Page", SymbolKind::Registration)
        .expect("registration symbol");
    assert!(!registration.rename_safe());
    assert!(
        registration
            .safety
            .blocks
            .contains(&RenameBlock::RegistrationName)
    );
}

#[test]
fn invalid_new_names_never_produce_an_edit() {
    let result = analysis(
        &[("index.ts", "Page({ onTap() {} })")],
        "index.ts",
        RegistrationKind::Page,
    );
    let method = result
        .symbol("onTap", SymbolKind::Method)
        .expect("method symbol");
    assert!(method.rename_edit("1bad").is_none());
    assert!(method.rename_edit("").is_none());
}
#[test]
fn a_known_field_in_an_incomplete_overlay_is_not_rename_safe() {
    let result = analysis(
        &[(
            "index.ts",
            "Page({ data: { known: 1, dynamic: makeValue() } })",
        )],
        "index.ts",
        RegistrationKind::Page,
    );
    let known = result
        .symbol("known", SymbolKind::Data)
        .expect("known field");
    assert!(!known.rename_safe());
    assert!(
        known
            .safety
            .blocks
            .contains(&RenameBlock::IncompleteEvidence)
    );
    assert!(known.rename_edit("renamed").is_none());
}
