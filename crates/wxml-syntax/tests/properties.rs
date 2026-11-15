//! Property-based tests for the whole syntax crate.
//!
//! These tests assert the safety contract that callers rely on: arbitrary
//! UTF-8 input never panics, every node range stays inside the source and on
//! character boundaries, repeated parses are stable, and source-map segments
//! round-trip in both directions.

#![allow(clippy::expect_used)]

use proptest::prelude::*;
use wxml_syntax::expr::parse_expression;
use wxml_syntax::{NodeKind, SyntaxTree, parse};
use wxml_text::{FragmentRange, TextRange};

fn assert_range_in_text(text: &str, range: TextRange) {
    assert!(range.start() <= range.end());
    let start = usize::try_from(range.start()).expect("range offset fits");
    let end = usize::try_from(range.end()).expect("range offset fits");
    assert!(
        end <= text.len(),
        "range {range:?} outside {} bytes",
        text.len()
    );
    assert!(
        text.is_char_boundary(start),
        "start {start} is not a boundary"
    );
    assert!(text.is_char_boundary(end), "end {end} is not a boundary");
}

fn assert_tree_invariants(tree: &SyntaxTree) {
    let source = tree.text();
    assert_eq!(tree.kind(tree.root()), Some(NodeKind::Document));
    for (_, node) in tree.nodes() {
        let range = node.text_range();
        assert_range_in_text(source, range);
        if let Some(parent) = node.parent() {
            let parent_range = tree.text_range(parent).expect("parent range");
            assert!(
                parent_range.start() <= range.start() && range.end() <= parent_range.end(),
                "child {range:?} outside parent {parent_range:?}"
            );
        }
        for child in node.children() {
            let child_range = tree.text_range(*child).expect("child range");
            assert!(
                range.start() <= child_range.start() && child_range.end() <= range.end(),
                "child {child_range:?} outside node {range:?}"
            );
        }
        if let Some(expr) = node.expr() {
            for expression in expr.walk() {
                assert_range_in_text(source, expression.range());
            }
        }
    }
}

fn tree_signature(tree: &SyntaxTree) -> Vec<(NodeKind, u32, u32)> {
    tree.nodes()
        .map(|(_, node)| {
            (
                node.kind(),
                node.text_range().start(),
                node.text_range().end(),
            )
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_text_never_panics_and_keeps_ranges(text in any::<String>()) {
        let result = parse(&text);
        prop_assert_eq!(result.tree.text(), text.as_str());
        assert_tree_invariants(&result.tree);
        for error in &result.errors {
            assert_range_in_text(&text, error.range());
        }
    }

    #[test]
    fn parsing_is_deterministic(text in any::<String>()) {
        let first = parse(&text);
        let second = parse(&text);
        prop_assert_eq!(first.errors, second.errors);
        prop_assert_eq!(tree_signature(&first.tree), tree_signature(&second.tree));
    }

    #[test]
    fn arbitrary_expressions_never_panic(text in any::<String>()) {
        let result = parse_expression(&text, 0);
        assert_range_in_text(&text, result.expr.range());
        for expression in result.expr.walk() {
            assert_range_in_text(&text, expression.range());
        }
        for error in &result.errors {
            assert_range_in_text(&text, error.range());
        }
    }

    #[test]
    fn absolute_expression_ranges_respect_the_base_offset(text in any::<String>()) {
        let base = 10_000;
        let result = parse_expression(&text, base);
        let limit = base + u32::try_from(text.len()).expect("test input fits");
        for expression in result.expr.walk() {
            let range = expression.range();
            prop_assert!(range.start() >= base, "range {range:?} below base");
            prop_assert!(range.end() <= limit, "range {range:?} above limit");
        }
    }
}

/// Generates a small document out of markup fragments so that tag, attribute,
/// mustache and comment paths are exercised more often than by random bytes.
fn document() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        Just("<view>".to_owned()),
        Just("</view>".to_owned()),
        Just("<text/>".to_owned()),
        Just("<wxs>if (a<b) { x = 1 }</wxs>".to_owned()),
        Just("<wxs/>".to_owned()),
        Just("</text>".to_owned()),
        Just("<!-- c -->".to_owned()),
        Just("{{ a.b }}".to_owned()),
        Just("plain".to_owned()),
        Just(" ".to_owned()),
    ];
    prop::collection::vec(atom, 0..12).prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn generated_markup_is_structurally_sound(source in document()) {
        let result = parse(&source);
        prop_assert_eq!(result.tree.text(), source.as_str());
        assert_tree_invariants(&result.tree);
        for error in &result.errors {
            assert_range_in_text(&source, error.range());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn source_map_segments_round_trip(lengths in prop::collection::vec(1u32..32, 1..8)) {
        let mut map = wxml_syntax::SourceMap::new();
        let mut source_offset = 0u32;
        let mut fragment_offset = 0u32;
        for length in &lengths {
            let source = TextRange::new(source_offset, source_offset + length).expect("range");
            let fragment =
                FragmentRange::from_bounds(fragment_offset, fragment_offset + length).expect("range");
            map.push(source, fragment, wxml_syntax::SegmentKind::Text).expect("non-overlapping segment");
            prop_assert_eq!(map.fragment_to_source(fragment), Some(source));
            prop_assert_eq!(map.source_to_fragment(source), Some(fragment));
            prop_assert_eq!(map.source_offset_to_fragment(source_offset), Some(fragment_offset));
            prop_assert_eq!(map.fragment_offset_to_source(fragment_offset), Some(source_offset));
            source_offset += length + 1;
            fragment_offset += length + 3;
        }
        prop_assert_eq!(map.len(), lengths.len());
    }
}
