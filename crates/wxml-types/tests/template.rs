//! Integration tests for C1.1 template analysis.

#![allow(clippy::assert_is_empty, clippy::expect_used)]

use wxml_syntax::{SyntaxTree, parse};
use wxml_types::{
    CheckFragment, EventBindingKind, MAX_FRAGMENTS, RootKind, SourceRevision, TemplateAnalysis,
    TemplateAnalyzer, UnsupportedFeature, UnsupportedKind, analyze_template,
};

const REVISION: u64 = 7;

fn analyze(source: &str) -> (TemplateAnalysis, SyntaxTree) {
    let parsed = parse(source);
    let analysis = analyze_template(&parsed.tree, RootKind::Page, SourceRevision::new(REVISION));
    (analysis, parsed.tree)
}

fn fragment<'a>(
    analysis: &'a TemplateAnalysis,
    expr: &str,
    occurrence: usize,
) -> &'a CheckFragment {
    analysis
        .fragments
        .iter()
        .filter(|fragment| fragment.expr_text == expr)
        .nth(occurrence)
        .unwrap_or_else(|| panic!("missing fragment {expr:?} #{occurrence}"))
}

fn locals(fragment: &CheckFragment) -> Vec<&str> {
    fragment.locals.iter().map(String::as_str).collect()
}

fn slice<'a>(tree: &'a SyntaxTree, fragment: &CheckFragment) -> &'a str {
    let start = usize::try_from(fragment.wxml_range.start()).expect("range start fits");
    let end = usize::try_from(fragment.wxml_range.end()).expect("range end fits");
    &tree.text()[start..end]
}

fn unsupported_kinds(analysis: &TemplateAnalysis) -> Vec<UnsupportedKind> {
    analysis
        .unsupported
        .iter()
        .map(UnsupportedFeature::kind)
        .collect()
}

fn unsupported_count(analysis: &TemplateAnalysis, kind: UnsupportedKind) -> usize {
    analysis
        .unsupported
        .iter()
        .filter(|feature| feature.kind() == kind)
        .count()
}

#[test]
fn extracts_root_member_access() {
    let (analysis, tree) = analyze("<view>{{ user.name }}</view>");
    assert_eq!(analysis.fragments.len(), 1);
    let fragment = &analysis.fragments[0];
    assert_eq!(fragment.fragment_id, "f0");
    assert_eq!(fragment.expr_text, "user.name");
    assert_eq!(fragment.target.as_deref(), Some("name"));
    assert!(fragment.locals.is_empty());
    assert_eq!(fragment.root, RootKind::Page);
    assert_eq!(fragment.fragment_revision, REVISION);
    assert_eq!(slice(&tree, fragment), "user.name");
    assert!(analysis.events.is_empty());
    assert!(analysis.unsupported.is_empty());
}

#[test]
fn chooses_the_right_target_token() {
    let source = "<view>{{ user.name }}{{ format(user) }}{{ user.format() }}{{ items[index] }}{{ user[0].name }}{{ (fn)() }}{{ a.b[c] }}{{ title }}{{ 1 + 2 }}</view>";
    let (analysis, _) = analyze(source);
    let expected = [
        ("user.name", Some("name")),
        ("format(user)", Some("format")),
        ("user.format()", Some("format")),
        ("items[index]", Some("items")),
        ("user[0].name", Some("name")),
        ("(fn)()", Some("fn")),
        ("a.b[c]", Some("b")),
        ("title", Some("title")),
        ("1 + 2", None),
    ];
    assert_eq!(analysis.fragments.len(), expected.len());
    for (fragment, (expr, target)) in analysis.fragments.iter().zip(expected) {
        assert_eq!(fragment.expr_text, expr);
        assert_eq!(fragment.target.as_deref(), target, "expression {expr:?}");
    }
}

#[test]
fn default_for_bindings_apply_to_attributes_and_content() {
    let source = r#"<view wx:for="{{ items }}" wx:if="{{ item.visible }}">{{ item.name }} {{ index }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 4);

    let items = fragment(&analysis, "items", 0);
    assert!(items.locals.is_empty());
    assert_eq!(items.target.as_deref(), Some("items"));

    let visible = fragment(&analysis, "item.visible", 0);
    assert_eq!(locals(visible), vec!["item", "index"]);
    assert_eq!(visible.target.as_deref(), Some("visible"));

    let name = fragment(&analysis, "item.name", 0);
    assert_eq!(locals(name), vec!["item", "index"]);
    assert_eq!(name.target.as_deref(), Some("name"));

    let index = fragment(&analysis, "index", 0);
    assert_eq!(locals(index), vec!["item", "index"]);
    assert_eq!(index.target.as_deref(), Some("index"));
}

#[test]
fn custom_for_bindings_replace_defaults() {
    let source = r#"<view wx:for="{{ rows }}" wx:for-item="row" wx:for-index="i">{{ row.name }} {{ i }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 3);
    assert!(fragment(&analysis, "rows", 0).locals.is_empty());
    assert_eq!(locals(fragment(&analysis, "row.name", 0)), vec!["row", "i"]);
    assert_eq!(locals(fragment(&analysis, "i", 0)), vec!["row", "i"]);
}

#[test]
fn nested_loops_shadow_and_deduplicate_locals() {
    let source = r#"<view wx:for="{{ outer }}">{{ item.name }}<text wx:for="{{ item.children }}">{{ item.name }}</text></view>{{ item.name }}"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 5);

    assert!(fragment(&analysis, "outer", 0).locals.is_empty());
    assert_eq!(
        locals(fragment(&analysis, "item.name", 0)),
        vec!["item", "index"]
    );
    assert_eq!(
        locals(fragment(&analysis, "item.children", 0)),
        vec!["item", "index"]
    );
    let inner = fragment(&analysis, "item.name", 1);
    assert_eq!(locals(inner), vec!["item", "index"]);
    assert_eq!(
        inner
            .locals
            .iter()
            .filter(|name| name.as_str() == "item")
            .count(),
        1,
        "shadowed item must appear once"
    );
    assert!(
        fragment(&analysis, "item.name", 2).locals.is_empty(),
        "bindings must be popped after the loop"
    );
}

#[test]
fn condition_chain_fragments_do_not_change_scope() {
    let source =
        r#"<view wx:if="{{ a }}">A</view><view wx:elif="{{ b }}">B</view><view wx:else>C</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 2);
    assert_eq!(analysis.fragments[0].expr_text, "a");
    assert_eq!(analysis.fragments[1].expr_text, "b");
    assert!(
        analysis
            .fragments
            .iter()
            .all(|fragment| fragment.locals.is_empty())
    );
}

#[test]
fn attribute_values_can_mix_text_and_mustaches() {
    let source = r#"<view class="x {{ user.className }} y {{ count }}"></view>"#;
    let (analysis, tree) = analyze(source);
    assert_eq!(analysis.fragments.len(), 2);
    assert_eq!(analysis.fragments[0].expr_text, "user.className");
    assert_eq!(analysis.fragments[0].target.as_deref(), Some("className"));
    assert_eq!(slice(&tree, &analysis.fragments[0]), "user.className");
    assert_eq!(analysis.fragments[1].expr_text, "count");
    assert_eq!(slice(&tree, &analysis.fragments[1]), "count");
}

#[test]
fn static_events_are_collected_without_fragments() {
    let source = r#"<view bindtap="onTap" bind:touchstart="onStart" catchtap="onCatch" capture-bind:tap="onCapture" capture-catch:touchmove="onMove" mut-bind:tap="onMut" />"#;
    let (analysis, tree) = analyze(source);
    assert!(analysis.fragments.is_empty());
    assert_eq!(analysis.events.len(), 6);
    let expected = [
        ("bindtap", EventBindingKind::Bind, "tap", "onTap"),
        (
            "bind:touchstart",
            EventBindingKind::Bind,
            "touchstart",
            "onStart",
        ),
        ("catchtap", EventBindingKind::Catch, "tap", "onCatch"),
        (
            "capture-bind:tap",
            EventBindingKind::CaptureBind,
            "tap",
            "onCapture",
        ),
        (
            "capture-catch:touchmove",
            EventBindingKind::CaptureCatch,
            "touchmove",
            "onMove",
        ),
        ("mut-bind:tap", EventBindingKind::MutBind, "tap", "onMut"),
    ];
    for (event, (attribute, binding, name, handler)) in analysis.events.iter().zip(expected) {
        assert_eq!(event.attribute, attribute);
        assert_eq!(event.binding, binding);
        assert_eq!(event.event, name);
        assert_eq!(event.handler, handler);
        let start = usize::try_from(event.range.start()).expect("range");
        let end = usize::try_from(event.range.end()).expect("range");
        assert!(tree.text()[start..end].starts_with(attribute));
    }
}

#[test]
fn dynamic_and_empty_events_are_unsupported() {
    let source = r#"<view bindtap="{{ handler }}" catch:tap="on{{name}}" bindtap="   "></view>"#;
    let (analysis, _) = analyze(source);
    assert!(analysis.events.is_empty());
    assert!(analysis.fragments.is_empty());
    assert_eq!(
        unsupported_kinds(&analysis),
        vec![
            UnsupportedKind::DynamicEvent,
            UnsupportedKind::DynamicEvent,
            UnsupportedKind::EmptyEvent,
        ]
    );
}

#[test]
fn unsupported_template_forms_are_reported_and_not_analyzed() {
    let source = r#"<wxs module="m">var x = {{y}}</wxs><template name="t">{{ z }}</template><import src="a.wxml"/><include src="b.wxml"/>"#;
    let (analysis, tree) = analyze(source);
    assert!(
        analysis.fragments.is_empty(),
        "no fragments from unsupported forms"
    );
    assert_eq!(
        unsupported_kinds(&analysis),
        vec![
            UnsupportedKind::Wxs,
            UnsupportedKind::Template,
            UnsupportedKind::Import,
            UnsupportedKind::Include,
        ]
    );
    let prefixes = ["<wxs", "<template", "<import", "<include"];
    for (feature, prefix) in analysis.unsupported.iter().zip(prefixes) {
        let start = usize::try_from(feature.range().start()).expect("range");
        let end = usize::try_from(feature.range().end()).expect("range");
        assert!(&tree.text()[start..end].starts_with(prefix));
        assert!(!feature.reason().is_empty());
    }
}

#[test]
fn malformed_and_empty_expressions_are_skipped() {
    let (empty, _) = analyze("<view>{{ }}</view>");
    assert!(empty.fragments.is_empty());
    assert_eq!(
        unsupported_kinds(&empty),
        vec![UnsupportedKind::EmptyExpression]
    );

    for source in [
        "<view>{{ a + }}</view>",
        "<view>{{ . }}</view>",
        "<view>{{ a b }}</view>",
    ] {
        let (analysis, _) = analyze(source);
        assert!(
            analysis.fragments.is_empty(),
            "malformed {source:?} produced {:?}",
            analysis.fragments
        );
        assert_eq!(
            unsupported_kinds(&analysis),
            vec![UnsupportedKind::MalformedExpression],
            "source {source:?}"
        );
    }

    let (unclosed, _) = analyze("{{ a");
    assert!(unclosed.fragments.is_empty());
    assert!(unsupported_kinds(&unclosed).contains(&UnsupportedKind::MalformedExpression));
}

#[test]
fn repeated_expressions_stay_separate_and_stable() {
    let source = "<view>{{ x }}</view><view>{{ x }}</view>";
    let (first, _) = analyze(source);
    assert_eq!(first.fragments.len(), 2);
    assert_eq!(first.fragments[0].expr_text, "x");
    assert_eq!(first.fragments[1].expr_text, "x");
    assert_ne!(
        first.fragments[0].fragment_id,
        first.fragments[1].fragment_id
    );
    assert_eq!(first.fragments[0].fragment_id, "f0");
    assert_eq!(first.fragments[1].fragment_id, "f1");

    let (second, _) = analyze(source);
    assert_eq!(first, second);
}

#[test]
fn fragment_ids_follow_source_order_even_when_wx_for_is_later() {
    let source = r#"<view class="{{ a }}" wx:for="{{ b }}">{{ c }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 3);
    let by_expr = |expr: &str| fragment(&analysis, expr, 0);
    assert_eq!(by_expr("a").fragment_id, "f0");
    assert_eq!(by_expr("b").fragment_id, "f1");
    assert_eq!(by_expr("c").fragment_id, "f2");
    assert!(
        by_expr("b").locals.is_empty(),
        "wx:for uses the outer scope"
    );
    assert_eq!(locals(by_expr("a")), vec!["item", "index"]);
    assert_eq!(locals(by_expr("c")), vec!["item", "index"]);
}

#[test]
fn wx_key_does_not_generate_a_fragment() {
    let source = r#"<view wx:for="{{ items }}" wx:key="id">{{ item }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 2);
    assert_eq!(analysis.fragments[0].expr_text, "items");
    assert_eq!(analysis.fragments[1].expr_text, "item");
    assert!(
        analysis
            .fragments
            .iter()
            .all(|fragment| fragment.expr_text != "id")
    );
}

#[test]
fn dynamic_for_binding_names_are_reported_and_defaulted() {
    let source = r#"<view wx:for="{{ items }}" wx:for-item="{{ name }}">{{ item }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(
        unsupported_kinds(&analysis),
        vec![UnsupportedKind::DynamicForBinding]
    );
    let item = fragment(&analysis, "item", 0);
    assert_eq!(locals(item), vec!["item", "index"]);
}

#[test]
fn crlf_and_emoji_ranges_stay_on_character_boundaries() {
    let source = "<view>\r\n😀 {{ user.name }} 😀</view>";
    let (analysis, tree) = analyze(source);
    assert_eq!(analysis.fragments.len(), 1);
    let fragment = &analysis.fragments[0];
    assert_eq!(slice(&tree, fragment), "user.name");
    let start = usize::try_from(fragment.wxml_range.start()).expect("range");
    let end = usize::try_from(fragment.wxml_range.end()).expect("range");
    assert!(source.is_char_boundary(start));
    assert!(source.is_char_boundary(end));

    let source = r#"<view class="😀{{ x }}"></view>"#;
    let (analysis, tree) = analyze(source);
    assert_eq!(analysis.fragments.len(), 1);
    assert_eq!(slice(&tree, &analysis.fragments[0]), "x");
    assert_eq!(analysis.fragments[0].target.as_deref(), Some("x"));
}

#[test]
fn root_kind_and_revision_are_propagated() {
    let parsed = parse("<view>{{ x }}</view>");
    let analysis = analyze_template(&parsed.tree, RootKind::Component, SourceRevision::new(42));
    assert_eq!(analysis.fragments.len(), 1);
    assert_eq!(analysis.fragments[0].root, RootKind::Component);
    assert_eq!(analysis.fragments[0].fragment_revision, 42);
    assert_eq!(RootKind::Page.as_str(), "page");
    assert_eq!(RootKind::Component.as_str(), "component");
}

#[test]
fn empty_document_produces_an_empty_analysis() {
    let (analysis, _) = analyze("");
    assert!(analysis.fragments.is_empty());
    assert!(analysis.events.is_empty());
    assert!(analysis.unsupported.is_empty());
    assert!(analysis.is_empty());
}

#[test]
fn components_accept_page_and_component_roots_without_changing_fragments() {
    let source = "<view>{{ user.name }}</view>";
    let parsed = parse(source);
    let page = analyze_template(&parsed.tree, RootKind::Page, SourceRevision::new(1));
    let component = analyze_template(&parsed.tree, RootKind::Component, SourceRevision::new(1));
    assert_eq!(page.fragments.len(), component.fragments.len());
    assert_eq!(
        page.fragments[0].expr_text,
        component.fragments[0].expr_text
    );
    assert_eq!(page.fragments[0].target, component.fragments[0].target);
    assert_eq!(
        page.fragments[0].wxml_range,
        component.fragments[0].wxml_range
    );
    assert_ne!(page.fragments[0].root, component.fragments[0].root);
}

#[test]
fn custom_nested_bindings_accumulate_outer_to_inner() {
    let source = r#"<view wx:for="{{ outer }}" wx:for-item="row" wx:for-index="i"><text wx:for="{{ row.children }}">{{ item }}</text></view>"#;
    let (analysis, _) = analyze(source);
    assert!(fragment(&analysis, "outer", 0).locals.is_empty());
    assert_eq!(
        locals(fragment(&analysis, "row.children", 0)),
        vec!["row", "i"]
    );
    assert_eq!(
        locals(fragment(&analysis, "item", 0)),
        vec!["row", "i", "item", "index"]
    );
}

#[test]
fn dynamic_for_binding_does_not_produce_a_fragment() {
    let source = r#"<view wx:for="{{ items }}" wx:for-item="{{ name }}">{{ item }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 2);
    assert!(
        analysis
            .fragments
            .iter()
            .all(|fragment| fragment.expr_text != "name")
    );
}

#[test]
fn template_analyzer_exposes_configuration() {
    let analyzer = TemplateAnalyzer::new(RootKind::Component, SourceRevision::new(9));
    assert_eq!(analyzer.root(), RootKind::Component);
    assert_eq!(analyzer.source_revision(), SourceRevision::new(9));
    let parsed = parse("<view>{{ x }}</view>");
    let analysis = analyzer.analyze(&parsed.tree);
    assert_eq!(analysis.fragments.len(), 1);
    assert_eq!(analysis.fragments[0].fragment_revision, 9);
    assert_eq!(analysis.fragments[0].root, RootKind::Component);
}

#[test]
fn unsupported_features_expose_kind_range_and_reason() {
    let (analysis, _) = analyze("<wxs>x</wxs>");
    let feature = analysis.unsupported.first().expect("unsupported feature");
    assert_eq!(feature.kind(), UnsupportedKind::Wxs);
    assert_eq!(feature.reason(), UnsupportedKind::Wxs.reason());
    assert!(feature.range().start() < feature.range().end());
}

#[test]
fn unicode_identifiers_and_targets_keep_byte_ranges() {
    let source = "<view>{{ 用户.名字 }}</view>";
    let (analysis, tree) = analyze(source);
    assert_eq!(analysis.fragments.len(), 1);
    let fragment = &analysis.fragments[0];
    assert_eq!(fragment.expr_text, "用户.名字");
    assert_eq!(fragment.target.as_deref(), Some("名字"));
    assert_eq!(slice(&tree, fragment), "用户.名字");
}

#[test]
fn top_level_mustaches_are_analyzed() {
    let (analysis, _) = analyze("{{ user.name }}");
    assert_eq!(analysis.fragments.len(), 1);
    assert_eq!(analysis.fragments[0].expr_text, "user.name");
    assert_eq!(analysis.fragments[0].target.as_deref(), Some("name"));
    assert!(analysis.fragments[0].locals.is_empty());
}

#[test]
fn identical_item_and_index_names_are_deduplicated() {
    let source =
        r#"<view wx:for="{{ items }}" wx:for-item="value" wx:for-index="value">{{ value }}</view>"#;
    let (analysis, _) = analyze(source);
    assert_eq!(locals(fragment(&analysis, "value", 0)), vec!["value"]);
}

#[test]
fn balanced_object_expressions_are_kept() {
    let source = "<view>{{ { a: 1, b: [2] } }}</view>";
    let (analysis, _) = analyze(source);
    assert_eq!(analysis.fragments.len(), 1);
    assert_eq!(analysis.fragments[0].expr_text, "{ a: 1, b: [2] }");
    assert_eq!(analysis.fragments[0].target, None);
    assert!(
        analysis.unsupported.is_empty(),
        "{:?}",
        analysis.unsupported
    );
}

#[test]
fn fragment_budget_truncates_and_marks_analysis() {
    let source = format!("<view>{}</view>", "{{ x }}".repeat(MAX_FRAGMENTS + 10));
    let (analysis, _) = analyze(&source);
    assert_eq!(analysis.fragments.len(), MAX_FRAGMENTS);
    assert!(analysis.truncated);
    assert!(analysis.is_truncated());
    assert_eq!(
        unsupported_count(&analysis, UnsupportedKind::FragmentLimit),
        1
    );
    assert_eq!(analysis.fragments[0].fragment_id, "f0");
    let last = format!("f{}", MAX_FRAGMENTS - 1);
    assert_eq!(analysis.fragments[MAX_FRAGMENTS - 1].fragment_id, last);
}

#[test]
fn deep_scopes_hit_the_fragment_cap_with_bounded_work() {
    let mut source = String::new();
    for level in 0..256 {
        source.push_str("<view wx:for=\"{{ xs }}\" wx:for-item=\"item");
        source.push_str(&level.to_string());
        source.push_str("\" wx:for-index=\"index");
        source.push_str(&level.to_string());
        source.push_str("\">");
        for _ in 0..8 {
            source.push_str("{{ a.b }}");
        }
    }
    source.push_str(&"</view>".repeat(256));

    let start = std::time::Instant::now();
    let (analysis, _) = analyze(&source);
    let elapsed = start.elapsed();
    assert_eq!(analysis.fragments.len(), MAX_FRAGMENTS);
    assert!(analysis.truncated);
    assert_eq!(
        unsupported_count(&analysis, UnsupportedKind::FragmentLimit),
        1
    );
    assert!(
        analysis
            .fragments
            .iter()
            .all(|fragment| fragment.locals.len() <= 512)
    );
    assert!(
        analysis
            .fragments
            .iter()
            .any(|fragment| fragment.locals.len() > 256),
        "the cap should be reached deep in the scope stack"
    );
    // Generous debug bound; the previous implementation took seconds and
    // gigabytes for a smaller input.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "deep analysis took {elapsed:?}"
    );
}

#[test]
fn invalid_for_binding_names_fall_back_to_defaults() {
    for invalid in ["a b", "item.name", "1item", "a-b"] {
        let source = format!(
            "<view wx:for=\"{{{{ xs }}}}\" wx:for-item=\"{invalid}\" wx:for-index=\"{invalid}\">{{{{ item }}}}</view>"
        );
        let (analysis, _) = analyze(&source);
        assert_eq!(
            unsupported_count(&analysis, UnsupportedKind::DynamicForBinding),
            2,
            "invalid name {invalid:?}"
        );
        assert_eq!(
            locals(fragment(&analysis, "item", 0)),
            vec!["item", "index"]
        );
    }

    let long = "a".repeat(65);
    let source =
        format!("<view wx:for=\"{{{{ xs }}}}\" wx:for-item=\"{long}\">{{{{ item }}}}</view>");
    let (analysis, _) = analyze(&source);
    assert_eq!(
        unsupported_count(&analysis, UnsupportedKind::DynamicForBinding),
        1
    );
    assert_eq!(
        locals(fragment(&analysis, "item", 0)),
        vec!["item", "index"]
    );
}

#[test]
fn valid_custom_binding_names_are_kept() {
    let source = r#"<view wx:for="{{ xs }}" wx:for-item="$row" wx:for-index="_i">{{ $row.name }} {{ _i }}</view>"#;
    let (analysis, _) = analyze(source);
    assert!(
        analysis.unsupported.is_empty(),
        "{:?}",
        analysis.unsupported
    );
    assert_eq!(
        locals(fragment(&analysis, "$row.name", 0)),
        vec!["$row", "_i"]
    );
    assert_eq!(locals(fragment(&analysis, "_i", 0)), vec!["$row", "_i"]);
}

#[test]
fn for_binding_mustaches_without_a_loop_are_reported() {
    let source = r#"<view wx:for-item="{{ name }}" wx:for-index="{{ idx }}"></view>"#;
    let (analysis, _) = analyze(source);
    assert!(analysis.fragments.is_empty());
    assert_eq!(
        unsupported_count(&analysis, UnsupportedKind::DynamicForBinding),
        2
    );
}

#[test]
fn empty_event_names_are_reported() {
    let source =
        r#"<view bind:="x" catch:="y" capture-bind:="z" capture-catch:="w" mut-bind:="v"></view>"#;
    let (analysis, _) = analyze(source);
    assert!(analysis.fragments.is_empty());
    assert!(analysis.events.is_empty());
    assert_eq!(unsupported_count(&analysis, UnsupportedKind::EmptyEvent), 5);
}
