#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::missing_const_for_fn
)]

//! Integration tests for expression rewriting and the edit engine.
//!
//! Test names carry the TypeScript oracle case id. Checker-dependent cases are
//! listed in [`BLOCKED_CASES`]; they are deliberately not claimed as migrated.

use wxml_backend_tsgo::input::{DocumentRevision, SourceFile};
use wxml_backend_tsgo::shadow::ShadowError;
use wxml_backend_tsgo::shadow::layout::{
    FragmentSpec, JsCheckPolicy, RootKind, ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::mapping::{SourceRange, map_span_to_source};
use wxml_backend_tsgo::shadow::rewrite::{
    EditKind, RewriteOptions, ShadowEdit, apply_edits, rewrite_expression,
};

const BLOCKED_CASES: &[(&str, &str)] = &[
    (
        "TS-REWRITE-0197",
        "exact diagnostic range requires the checker",
    ),
    (
        "TS-REWRITE-0349",
        "cross-gap diagnostics require the checker",
    ),
    (
        "TS-REWRITE-0368",
        "no fabricated diagnostics requires the checker",
    ),
    ("TS-REWRITE-0378", "1 MiB / 4 MiB timing gate belongs to P5"),
    (
        "TS-REWRITE-0413",
        "hover/diagnostic mapping requires the checker",
    ),
];

fn rewrite(expr: &str) -> String {
    rewrite_expression(expr, None, &RewriteOptions::default())
        .expect("valid expression")
        .text
}

fn source_file(text: &str) -> SourceFile {
    SourceFile::new("index.ts", text, DocumentRevision::new(1)).expect("valid source")
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
fn case_rewrite_0051_unary_parenthesized_and_multi_root_expressions_are_fully_rewritten() {
    assert_eq!(rewrite("!title"), "!__wxml_root.title");
    assert_eq!(rewrite("(title)"), "(__wxml_root.title)");
    assert_eq!(
        rewrite("title + user.name"),
        "__wxml_root.title + __wxml_root.user.name"
    );
    assert_eq!(rewrite("typeof title"), "typeof __wxml_root.title");
    assert_eq!(rewrite("-title"), "-__wxml_root.title");
    assert_eq!(
        rewrite("title ? title : user.name"),
        "__wxml_root.title ? __wxml_root.title : __wxml_root.user.name"
    );
}

#[test]
fn case_rewrite_0078_indexed_array_roots_are_rewritten_before_the_index() {
    assert_eq!(rewrite("users[0].name"), "__wxml_root.users[0].name");
}

#[test]
fn case_rewrite_0088_template_locals_are_not_prefixed_anywhere_in_the_expression() {
    let options = RewriteOptions {
        locals: std::collections::BTreeSet::from(["item".to_owned()]),
        ..RewriteOptions::default()
    };
    for expr in ["!item.done", "(item).name", "item.done + item.name"] {
        let result = rewrite_expression(expr, None, &options).expect("valid");
        assert_eq!(result.text, expr);
        assert!(result.local_root);
    }
}

#[test]
fn case_rewrite_0101_template_locals_may_shadow_global_builtins() {
    let options = RewriteOptions {
        locals: std::collections::BTreeSet::from(["Math".to_owned()]),
        ..RewriteOptions::default()
    };
    let examples = [
        ("Math", "Math"),
        ("Math.max(title)", "Math.max(__wxml_root.title)"),
        ("{ Math }", "{ Math: Math }"),
    ];
    for (expr, expected) in examples {
        assert_eq!(
            rewrite_expression(expr, None, &options)
                .expect("valid")
                .text,
            expected
        );
    }
}

#[test]
fn case_rewrite_0118_string_and_template_text_stay_verbatim_while_substitutions_are_rewritten() {
    assert_eq!(rewrite("`title ${title}`"), "`title ${__wxml_root.title}`");
    assert_eq!(
        rewrite("\"name\" + user.name"),
        "\"name\" + __wxml_root.user.name"
    );
}

#[test]
fn case_rewrite_0127_numeric_literals_do_not_become_default_targets() {
    assert_eq!(rewrite("title + 1e10"), "__wxml_root.title + 1e10");
    assert_eq!(rewrite("title + 0x1f"), "__wxml_root.title + 0x1f");
}

#[test]
fn case_rewrite_0136_member_names_object_keys_strings_and_globals_are_not_rewritten() {
    assert_eq!(rewrite("user.name"), "__wxml_root.user.name");
    assert_eq!(
        rewrite("{ title: user.name, user }"),
        "{ title: __wxml_root.user.name, user: __wxml_root.user }"
    );
    assert_eq!(
        rewrite("user.name + \"title\""),
        "__wxml_root.user.name + \"title\""
    );
    assert_eq!(rewrite("Math.max(title)"), "Math.max(__wxml_root.title)");
    assert_eq!(rewrite("user?.name"), "__wxml_root.user?.name");
    assert_eq!(
        rewrite("format(user.name)"),
        "__wxml_root.format(__wxml_root.user.name)"
    );
}

#[test]
fn case_rewrite_0156_repeated_identifiers_keep_the_rightmost_target_offset() {
    for (expr, offset, name) in [
        ("a.a", 14, "a"),
        ("user.user", 17, "user"),
        ("title + title", 32, "title"),
    ] {
        let rewritten =
            rewrite_expression(expr, None, &RewriteOptions::default()).expect("valid expression");
        assert_eq!(
            rewritten.target_offset_utf16, offset,
            "target offset of {expr}"
        );
        assert_eq!(
            utf16_slice(
                &rewritten.text,
                rewritten.target_offset_utf16,
                u32::try_from(name.len()).expect("length")
            ),
            name
        );
    }
}

#[test]
fn case_rewrite_0183_object_keys_are_not_default_targets() {
    let rewritten = rewrite_expression(
        "{ title: user.name, flag: true }",
        None,
        &RewriteOptions::default(),
    )
    .expect("valid expression");
    assert_eq!(
        utf16_slice(&rewritten.text, rewritten.target_offset_utf16, 4),
        "name"
    );
}

#[test]
fn explicit_object_key_targets_are_mapped_when_the_token_is_whole() {
    let rewritten = rewrite_expression(
        "{ title: user.name }",
        Some("title"),
        &RewriteOptions::default(),
    )
    .expect("valid expression");
    assert_eq!(
        utf16_slice(&rewritten.text, rewritten.target_offset_utf16, 5),
        "title"
    );
}

#[test]
fn case_rewrite_0190_object_shorthand_is_expanded_safely() {
    assert_eq!(rewrite("{ title }"), "{ title: __wxml_root.title }");
}

#[test]
fn case_rewrite_0207_wxmlrange_must_match_the_expression_byte_length() {
    let script = source_file("Page({ data: {} });");
    let fragment = FragmentSpec::new("f0", "title", SourceRange::new(0, 99));
    let request = ShadowRequest {
        script: &script,
        root: RootKind::Page,
        fragments: &[fragment],
        js_check: JsCheckPolicy::Inherit,
        check_js: false,
    };
    assert!(matches!(
        build_in_place_shadow(&request),
        Err(ShadowError::Invalid {
            feature: "fragment.range",
            ..
        })
    ));
}

#[test]
fn case_rewrite_0224_targets_inside_comments_or_strings_are_rejected() {
    for (expr, target) in [
        ("title /* name */", "name"),
        ("title // name", "name"),
        ("title + \"name\"", "name"),
    ] {
        assert!(
            rewrite_expression(expr, Some(target), &RewriteOptions::default()).is_err(),
            "{expr}"
        );
    }
}

#[test]
fn case_rewrite_0239_wrapper_escapes_and_extra_statements_are_rejected() {
    for expr in ["title); (user", "title) + (user", "title);"] {
        assert!(
            rewrite_expression(expr, None, &RewriteOptions::default()).is_err(),
            "{expr}"
        );
    }
}

#[test]
fn case_rewrite_0250_unsupported_expression_shapes_fail_closed() {
    for expr in [
        "new Foo()",
        "title satisfies string",
        "foo<Bar>()",
        "title as string",
    ] {
        assert!(
            rewrite_expression(expr, None, &RewriteOptions::default()).is_err(),
            "{expr}"
        );
    }
}

#[test]
fn case_rewrite_0260_depth_limit_is_counted_from_the_parsed_root_expression() {
    let at_limit = format!("{}title{}", "(".repeat(256), ")".repeat(256));
    assert!(rewrite_expression(&at_limit, None, &RewriteOptions::default()).is_ok());
    let over_limit = format!("{}title{}", "(".repeat(257), ")".repeat(257));
    assert!(rewrite_expression(&over_limit, None, &RewriteOptions::default()).is_err());
}

#[test]
fn case_rewrite_0274_deeply_nested_and_malformed_expressions_are_bad_fragment_not_rangeerror() {
    let deep = format!("{}title{}", "(".repeat(300), ")".repeat(300));
    for expr in [deep.as_str(), "title)"] {
        let error =
            rewrite_expression(expr, None, &RewriteOptions::default()).expect_err("bad fragment");
        assert!(matches!(error, ShadowError::BadExpression { .. }));
    }
}

#[test]
fn case_rewrite_0285_error_messages_truncate_huge_expressions() {
    let expr = "(".repeat(5_000);
    let error = rewrite_expression(&expr, None, &RewriteOptions::default()).expect_err("bad");
    let message = error.to_string();
    assert!(message.len() < 300, "message was {} chars", message.len());
    assert!(message.contains("total 5000 UTF-16 chars"), "{message}");
}

#[test]
fn case_rewrite_0299_expressions_without_a_free_root_identifier_are_rejected() {
    for expr in ["1 + 2", "Math.random()"] {
        assert!(
            rewrite_expression(expr, None, &RewriteOptions::default()).is_err(),
            "{expr}"
        );
    }
}

#[test]
fn case_rewrite_0309_rewritten_mounts_map_target_spans_and_trim_synthetic_prefixes() {
    let script = source_file("Page({ data: { title: 'x', user: { name: 'Ada' } } });");
    let origin = 200_u32;
    let first = FragmentSpec::new("bang", "!title", SourceRange::new(origin, origin + 6));
    let second = FragmentSpec::new(
        "sum",
        "title + user.name",
        SourceRange::new(origin + 10, origin + 27),
    );
    let request = ShadowRequest {
        script: &script,
        root: RootKind::Page,
        fragments: &[first, second],
        js_check: JsCheckPolicy::Inherit,
        check_js: false,
    };
    let built = build_in_place_shadow(&request).expect("layout");
    let bang = built
        .view
        .mounts
        .iter()
        .find(|mount| mount.fragment_id == "bang")
        .expect("bang mount");
    let mapped = map_span_to_source(
        bang,
        wxml_backend_tsgo::native::strings::Utf16Offset::new(bang.target_start_utf16),
        bang.target_length_utf16,
    )
    .expect("mapped");
    assert_eq!(mapped.range, Some(SourceRange::new(origin + 1, origin + 6)));
    assert!(!mapped.trimmed_synthetic_prefix);

    let sum = built
        .view
        .mounts
        .iter()
        .find(|mount| mount.fragment_id == "sum")
        .expect("sum mount");
    let mapped = map_span_to_source(
        sum,
        wxml_backend_tsgo::native::strings::Utf16Offset::new(sum.target_start_utf16),
        sum.target_length_utf16,
    )
    .expect("mapped");
    assert_eq!(
        mapped.range,
        Some(SourceRange::new(origin + 10 + 13, origin + 10 + 17))
    );
}

#[test]
fn edit_plan_is_ordered_and_rejects_ambiguous_offsets() {
    let assembled = apply_edits(
        "abcd",
        &[
            ShadowEdit::insert("late", EditKind::Probe, 3, "Z"),
            ShadowEdit::insert("early", EditKind::Probe, 1, "XY"),
        ],
    )
    .expect("valid plan");
    assert_eq!(assembled.text, "aXYbcZd");
    assert_eq!(assembled.edits[0].id, "early");
    assert_eq!(assembled.edits[1].id, "late");

    let duplicate_offset = apply_edits(
        "abcd",
        &[
            ShadowEdit::insert("a", EditKind::Probe, 1, "X"),
            ShadowEdit::insert("b", EditKind::Probe, 1, "Y"),
        ],
    );
    assert!(matches!(
        duplicate_offset,
        Err(ShadowError::EditOverlap { .. })
    ));
}

#[test]
fn blocked_cases_are_documented_and_not_claimed() {
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-REWRITE-0197")
    );
    assert!(
        !BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-REWRITE-0051")
    );
}
