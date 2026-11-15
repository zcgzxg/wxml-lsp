//! Source-map integration tests against ranges produced by the real parser.
//!
//! These tests check the contract the template layer depends on: a verbatim
//! copy of a parsed mustache expression or attribute value can be located in
//! the generated fragment and mapped back to the exact original bytes.

#![allow(clippy::expect_used)]

use wxml_syntax::{FragmentBuilder, SegmentKind, SourceMap, parse};
use wxml_text::TextRange;

fn slice(source: &str, range: TextRange) -> &str {
    let start = usize::try_from(range.start()).expect("range fits");
    let end = usize::try_from(range.end()).expect("range fits");
    &source[start..end]
}

#[test]
fn maps_a_mustache_expression_back_to_the_template() {
    let source = "<view>{{ user.name }}</view>";
    let result = parse(source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let tree = result.tree;
    let view = tree.document().elements().next().expect("view element");
    let mustache = view.mustaches().next().expect("mustache");
    let inner = mustache.inner_range().expect("inner expression range");
    assert_eq!(slice(source, inner), "user.name");

    let mut builder = FragmentBuilder::new();
    builder.push_synthetic("const __wxml_root = {};\n");
    let copied = builder
        .push_verbatim(slice(source, inner), inner, SegmentKind::MustacheExpression)
        .expect("verbatim copy");
    let (fragment_text, map) = builder.finish();

    assert_eq!(slice(&fragment_text, copied.range()), "user.name");
    assert_eq!(map.fragment_to_source(copied), Some(inner));
    assert_eq!(map.source_to_fragment(inner), Some(copied));
    assert_eq!(
        map.fragment_offset_to_source(copied.start()),
        Some(inner.start())
    );
    // Synthetic text does not map.
    assert_eq!(
        map.fragment_to_source(wxml_text::FragmentRange::from_bounds(0, 6).expect("range")),
        None
    );
}

#[test]
fn maps_attribute_values_with_multibyte_text_and_crlf() {
    let source = "<view class=\"页面\r\n卡片\">{{ title }}</view>";
    let result = parse(source);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let tree = result.tree;
    let view = tree.document().elements().next().expect("view element");
    let value = view
        .attribute("class")
        .and_then(wxml_syntax::Attribute::value)
        .expect("class value");
    let range = value.text_range();
    assert_eq!(slice(source, range), "\"页面\r\n卡片\"");

    let mut builder = FragmentBuilder::new();
    builder.push_synthetic("const classes = ");
    let copied = builder
        .push_verbatim(slice(source, range), range, SegmentKind::AttributeValue)
        .expect("verbatim copy");
    builder.push_synthetic(";");
    let (fragment_text, map) = builder.finish();

    assert_eq!(slice(&fragment_text, copied.range()), "\"页面\r\n卡片\"");
    assert_eq!(map.fragment_to_source(copied), Some(range));
    assert_eq!(map.source_to_fragment(range), Some(copied));
}

#[test]
fn rejects_segments_that_are_not_one_to_one() {
    let mut map = SourceMap::new();
    let source = TextRange::new(0, 4).expect("range");
    let fragment = wxml_text::FragmentRange::from_bounds(0, 5).expect("range");
    let error = map
        .push(source, fragment, SegmentKind::Text)
        .expect_err("length mismatch");
    assert!(matches!(
        error,
        wxml_syntax::SourceMapError::LengthMismatch { .. }
    ));
    assert!(map.is_empty());
}
