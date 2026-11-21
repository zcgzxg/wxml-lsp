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

use wxml_backend_tsrs::input::{DocumentRevision, SourceFile};
use wxml_backend_tsrs::native::strings::Utf16Offset;
use wxml_backend_tsrs::shadow::ShadowError;
use wxml_backend_tsrs::shadow::layout::{
    FragmentSpec, RootKind, ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsrs::shadow::mapping::{MountRecord, SourceRange, map_span_to_source, utf16_len};
use wxml_backend_tsrs::shadow::rewrite::{
    EditKind, RewriteOptions, ShadowEdit, apply_edits, rewrite_expression,
};

// Checker-dependent cases stay here. `TS-REWRITE-0197` is no longer blocked:
// its oracle asserts only the published WXML byte range, which
// `case_rewrite_0197_a_member_typo_keeps_the_exact_wxml_byte_range` produces
// from the real shadow build. The checker contributes only the diagnostic's
// existence, which this file deliberately does not claim.
const BLOCKED_CASES: &[(&str, &str)] = &[
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

/// The frozen `backends/typescript/samples/page-basic/index.ts` script used by
/// the `rewrite.test.ts` `pageRequest()` fixture.
const PAGE_BASIC_SCRIPT: &str = r"Page({
  data: {
    /** 页面标题 */
    title: '首页',
    /** 当前用户 */
    user: {
      /** 用户显示名称 */
      name: 'Ada',
      /** 用户年龄 */
      age: 36,
    },
  },
  onShow() {
    this.setData({ title: '欢迎回来' });
  },
});
";

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
    for (expr, name, offset, source) in [
        ("a.a", "a", 14, SourceRange::new(2, 3)),
        ("user.user", "user", 17, SourceRange::new(5, 9)),
        ("title + title", "title", 32, SourceRange::new(8, 13)),
    ] {
        let rewritten =
            rewrite_expression(expr, None, &RewriteOptions::default()).expect("valid expression");
        assert_eq!(
            rewritten.target_offset_utf16, offset,
            "target offset of {expr}"
        );
        let length = utf16_len(name);
        assert_eq!(
            utf16_slice(&rewritten.text, rewritten.target_offset_utf16, length),
            name
        );
        let record = MountRecord {
            mount_id: "f#0".to_owned(),
            fragment_id: "f".to_owned(),
            insertion_id: "probe".to_owned(),
            text: rewritten.text,
            start_utf16: 0,
            target_start_utf16: offset,
            target_length_utf16: length,
            segments: rewritten.segments,
        };
        let mapped = map_span_to_source(
            &record,
            Utf16Offset::new(record.target_start_utf16),
            record.target_length_utf16,
        )
        .expect("mapped target");
        assert_eq!(mapped.range, Some(source), "mapped range of {expr}");
        assert!(
            !mapped.trimmed_synthetic_prefix,
            "{expr}: the target starts in source text"
        );
        assert!(!mapped.unattributable, "{expr}");
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
    // Oracle: TS-REWRITE-0207. TS asserts RequestError code `bad-fragment`;
    // native replaces that code with the typed `Invalid { feature:
    // "fragment.range" }`. The same expression with the correct byte length is
    // the positive control, so the rejection is length-specific.
    let script = source_file("Page({ data: { title: \"x\" } });");
    let valid = FragmentSpec::new("f0", "title", SourceRange::new(0, 5));
    let valid_fragments = [valid];
    let mut valid_request = ShadowRequest::new(&script, RootKind::Page);
    valid_request.fragments = &valid_fragments;
    assert!(build_in_place_shadow(&valid_request).is_ok());

    let invalid = FragmentSpec::new("f0", "title", SourceRange::new(0, 99));
    let invalid_fragments = [invalid];
    let mut invalid_request = ShadowRequest::new(&script, RootKind::Page);
    invalid_request.fragments = &invalid_fragments;
    let error = build_in_place_shadow(&invalid_request).expect_err("wrong wxmlRange length");
    match error {
        ShadowError::Invalid { feature, reason } => {
            assert_eq!(feature, "fragment.range");
            assert_eq!(
                reason,
                "wxmlRange length 99 does not match expression byte length 5"
            );
        }
        other => panic!("expected the typed fragment.range error, got {other:?}"),
    }
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
    let fragments = [first, second];
    let mut request = ShadowRequest::new(&script, RootKind::Page);
    request.fragments = &fragments;
    let built = build_in_place_shadow(&request).expect("layout");
    let bang = built
        .view
        .mounts
        .iter()
        .find(|mount| mount.fragment_id == "bang")
        .expect("bang mount");
    let mapped = map_span_to_source(
        bang,
        Utf16Offset::new(bang.target_start_utf16),
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
        Utf16Offset::new(sum.target_start_utf16),
        sum.target_length_utf16,
    )
    .expect("mapped");
    assert_eq!(
        mapped.range,
        Some(SourceRange::new(origin + 10 + 13, origin + 10 + 17)),
        "`title + user.name` maps the rightmost target"
    );

    // Start inside the first synthetic prefix, end in the first source piece:
    // the inserted prefix has no original bytes, so the source projection is
    // exact rather than an approximate crop.
    let trimmed =
        map_span_to_source(sum, Utf16Offset::new(sum.start_utf16 + 5), 9).expect("trimmed prefix");
    assert!(!trimmed.trimmed_synthetic_prefix);
    assert_eq!(
        trimmed.range,
        Some(SourceRange::new(origin + 10, origin + 12))
    );

    // The second synthetic prefix (`user`) is attributed the same way.
    let second =
        map_span_to_source(sum, Utf16Offset::new(sum.start_utf16 + 25), 10).expect("second prefix");
    assert!(!second.trimmed_synthetic_prefix);
    assert_eq!(
        second.range,
        Some(SourceRange::new(origin + 10 + 8, origin + 10 + 11))
    );

    // A span crossing a synthetic gap is cropped to the target (a real
    // operand) instead of being silently dropped.
    let crossed = map_span_to_source(
        sum,
        Utf16Offset::new(sum.start_utf16 + 10),
        utf16_len(&sum.text) - 10,
    )
    .expect("crossed gap");
    assert!(crossed.trimmed_synthetic_prefix);
    assert_eq!(
        crossed.range,
        Some(SourceRange::new(origin + 10 + 13, origin + 10 + 17)),
        "`user.name` is the target operand"
    );
    assert!(!crossed.unattributable);

    let unary = map_span_to_source(
        bang,
        Utf16Offset::new(bang.start_utf16),
        utf16_len(&bang.text),
    )
    .expect("whole unary mount");
    assert!(unary.trimmed_synthetic_prefix);
    assert_eq!(
        unary.range,
        Some(SourceRange::new(origin + 1, origin + 6)),
        "`!title` crops to `title`"
    );
}

#[test]
fn case_rewrite_0197_a_member_typo_keeps_the_exact_wxml_byte_range() {
    // Oracle: `rewrite.test.ts:197`. The request is the frozen `page-basic`
    // fixture; "the checker found a member typo" is only an arrange premise,
    // and the assertion is the published WXML byte range. The range is produced
    // by the real shadow build and reverse mapping, not by a fake session.
    let script = source_file(PAGE_BASIC_SCRIPT);
    let fragments = [
        FragmentSpec::new("title", "title", SourceRange::new(35, 40)),
        FragmentSpec::new("user.name", "user.name", SourceRange::new(85, 94)),
        // The oracle request has no explicit target; the default is the
        // rightmost value identifier, which is `nmae`.
        FragmentSpec::new("user.nmae", "user.nmae", SourceRange::new(129, 138)),
    ];
    let mut request = ShadowRequest::new(&script, RootKind::Page);
    request.fragments = &fragments;
    let built = build_in_place_shadow(&request).expect("layout");
    let mount = built
        .view
        .mounts
        .iter()
        .find(|mount| mount.fragment_id == "user.nmae")
        .expect("user.nmae mount");
    assert_eq!(
        mount.target_length_utf16, 4,
        "default target must be `nmae`"
    );
    assert_eq!(
        utf16_slice(
            &mount.text,
            mount.target_start_utf16 - mount.start_utf16,
            mount.target_length_utf16,
        ),
        "nmae"
    );

    // The checker's tight typo span (`nmae`) maps one-to-one.
    let tight = map_span_to_source(
        mount,
        Utf16Offset::new(mount.target_start_utf16),
        mount.target_length_utf16,
    )
    .expect("mapped target");
    assert_eq!(tight.range, Some(SourceRange::new(134, 138)));
    assert!(!tight.trimmed_synthetic_prefix);
    assert!(!tight.unattributable);

    // A diagnostic that covers the whole mounted expression does not exist in
    // the oracle; it is a Rust-side control. `user.nmae` has no synthetic gap
    // *inside* the expression, so the leading `__wxml_root.` prefix is an exact
    // projection (not a crop) and the published range is the whole expression.
    // This pins that the mapping stays span-proportional instead of always
    // forcing the target token.
    let whole = map_span_to_source(
        mount,
        Utf16Offset::new(mount.start_utf16),
        utf16_len(&mount.text),
    )
    .expect("whole mount");
    assert_eq!(whole.range, Some(SourceRange::new(129, 138)));
    assert!(!whole.trimmed_synthetic_prefix);
    assert!(!whole.unattributable);
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
    // 0197 now has a real mapping test and stays out of the blocked list; the
    // remaining entries still represent checker-only assertions.
    assert!(
        !BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-REWRITE-0197")
    );
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-REWRITE-0349")
    );
    assert!(
        !BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-REWRITE-0051")
    );
}
