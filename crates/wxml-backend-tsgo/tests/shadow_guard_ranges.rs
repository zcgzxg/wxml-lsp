#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Guard-chain range coverage for the shadow layout.
//!
//! A `wx:if`/`wx:elif` condition is analyzed twice by the template analyzer:
//! once as its own `CheckFragment` and once as the control-flow wrapper of
//! every fragment inside the branch. The wrapper copy is generated text, so the
//! layout folds the rendered guard block into the owning fragment's
//! [`MountRecord`] and records each condition's original WXML bytes as source
//! segments. A checker diagnostic on `if (...)` then maps to the same range as
//! the standalone condition fragment instead of being dropped.
//!
//! Folding is fail-closed: the layout needs exactly one other fragment with the
//! same expression text, byte length, outer guard chains and rewritten text.
//! Missing or ambiguous condition fragments leave the wrapper copy outside
//! every mount instead of guessing a range.
//!
//! [`MountRecord`]: wxml_backend_tsgo::shadow::mapping::MountRecord

use wxml_backend_tsgo::input::{DocumentRevision, SourceFile};
use wxml_backend_tsgo::native::strings::Utf16Offset;
use wxml_backend_tsgo::shadow::guards::{GuardChain, GuardCondition, GuardKind};
use wxml_backend_tsgo::shadow::layout::{
    BuildStatus, FragmentSpec, JsCheckPolicy, RootKind, ShadowRequest, build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::locals::{LocalBinding, LocalRole};
use wxml_backend_tsgo::shadow::mapping::{
    MappedSpan, MappingLayer, MountRecord, ShadowMapper as _, SourceRange, map_span_to_source,
    utf16_len,
};

const SCRIPT: &str = "Page({ data: { title: 'x', flag: true, ready: false, items: [{ name: 'Ada', active: true }] } });";

fn source_file(text: &str) -> SourceFile {
    SourceFile::new("index.ts", text, DocumentRevision::new(1)).expect("valid source")
}

fn range_of(wxml: &str, needle: &str) -> SourceRange {
    let start = wxml.find(needle).expect("needle in wxml");
    SourceRange::new(
        u32::try_from(start).expect("small offset"),
        u32::try_from(start + needle.len()).expect("small offset"),
    )
}

fn build(fragments: &[FragmentSpec]) -> wxml_backend_tsgo::shadow::layout::BuiltShadow {
    let script = source_file(SCRIPT);
    build_in_place_shadow(&ShadowRequest {
        script: &script,
        root: RootKind::Page,
        fragments,
        js_check: JsCheckPolicy::Inherit,
        check_js: false,
    })
    .expect("layout")
}

fn condition(branch: GuardKind, expr: &str) -> GuardCondition {
    GuardCondition {
        branch,
        expr: expr.to_owned(),
    }
}

const fn chain(conditions: Vec<GuardCondition>, selected: usize) -> GuardChain {
    GuardChain {
        conditions,
        selected,
    }
}

fn fragment(id: &str, expr: &str, wxml_range: SourceRange) -> FragmentSpec {
    FragmentSpec::new(id, expr, wxml_range)
}

fn mount<'a>(
    built: &'a wxml_backend_tsgo::shadow::layout::BuiltShadow,
    id: &str,
) -> &'a MountRecord {
    built
        .view
        .mounts
        .iter()
        .find(|mount| mount.fragment_id == id)
        .expect("mount")
}

fn contains(mount: &MountRecord, start: u32, end: u32) -> bool {
    mount.start_utf16 <= start && end <= mount.end_utf16().expect("mount end")
}

fn map_nth(mount: &MountRecord, needle: &str, nth: usize) -> MappedSpan {
    let relative = mount
        .text
        .match_indices(needle)
        .nth(nth)
        .expect("needle occurrence in mount")
        .0;
    let start = mount.start_utf16 + utf16_len(&mount.text[..relative]);
    map_span_to_source(mount, Utf16Offset::new(start), utf16_len(needle)).expect("mapped span")
}

/// Maps the condition inside the mount's first `if (...)` wrapper.
fn map_wrapper_condition(mount: &MountRecord, condition: &str) -> MappedSpan {
    let relative = mount.text.find("if (").expect("guard wrapper in mount") + "if (".len();
    let start = mount.start_utf16 + utf16_len(&mount.text[..relative]);
    map_span_to_source(mount, Utf16Offset::new(start), utf16_len(condition))
        .expect("mapped wrapper")
}

#[test]
fn guard_wrapper_copy_maps_through_the_value_mount() {
    let wxml = "<view>\n  <view wx:if=\"{{ title === 1 }}\">\n    <text>{{ title.length }}</text>\n  </view>\n</view>";
    let condition_range = range_of(wxml, "title === 1");
    let value_range = range_of(wxml, "title.length");

    let mut value = fragment("value", "title.length", value_range);
    value.guards = vec![chain(vec![condition(GuardKind::If, "title === 1")], 0)];
    let guard = fragment("guard", "title === 1", condition_range);

    let built = build(&[value, guard]);
    assert_eq!(built.status, BuildStatus::Supported);

    // The value mount starts at the guard wrapper and folds the rewritten
    // condition, then carries the value expression.
    let value_mount = mount(&built, "value");
    assert!(
        value_mount
            .text
            .trim_start()
            .starts_with("if (__wxml_root.title === 1) {"),
        "{}",
        value_mount.text
    );
    assert!(value_mount.text.ends_with("__wxml_root.title.length"));

    // The wrapper copy now maps to exactly the original `wx:if` bytes.
    let wrapper = map_wrapper_condition(value_mount, "__wxml_root.title === 1");
    assert_eq!(wrapper.range, Some(condition_range));
    assert!(!wrapper.trimmed_synthetic_prefix);
    assert!(!wrapper.unattributable);

    // The standalone condition fragment still maps the same bytes.
    let guard_mount = mount(&built, "guard");
    assert_eq!(guard_mount.text, "__wxml_root.title === 1");
    assert_eq!(
        map_nth(guard_mount, "__wxml_root.title === 1", 0).range,
        Some(condition_range)
    );

    // The value expression remains attributable inside the folded mount.
    assert_eq!(
        map_nth(value_mount, "__wxml_root.title.length", 0).range,
        Some(value_range)
    );

    // A checker reports the same diagnostic twice; both copies now map to the
    // same WXML range and the template diagnostics de-duplicate them.
    assert_eq!(wrapper.range, Some(condition_range));
}

#[test]
fn multiple_values_under_one_guard_each_map_their_own_wrapper_copy() {
    let wxml = "<view>\n  <view wx:if=\"{{ flag }}\">\n    <text>{{ title.length }}</text>\n    <text>{{ title.trim() }}</text>\n  </view>\n</view>";
    let flag_range = range_of(wxml, "flag");
    let first_range = range_of(wxml, "title.length");
    let second_range = range_of(wxml, "title.trim()");

    let mut first = fragment("first", "title.length", first_range);
    first.guards = vec![chain(vec![condition(GuardKind::If, "flag")], 0)];
    let mut second = fragment("second", "title.trim()", second_range);
    second.guards = vec![chain(vec![condition(GuardKind::If, "flag")], 0)];
    let guard = fragment("guard", "flag", flag_range);

    let built = build(&[first, second, guard]);
    assert_eq!(built.status, BuildStatus::Supported);
    assert_eq!(built.view.mounts.len(), 3);

    for id in ["first", "second"] {
        let value_mount = mount(&built, id);
        assert!(
            value_mount
                .text
                .trim_start()
                .starts_with("if (__wxml_root.flag) {"),
            "{id}: {}",
            value_mount.text
        );
        let wrapper = map_wrapper_condition(value_mount, "__wxml_root.flag");
        assert_eq!(wrapper.range, Some(flag_range), "{id}");
        assert!(!wrapper.trimmed_synthetic_prefix, "{id}");
    }
}

#[test]
fn elif_and_else_chains_map_every_condition_copy() {
    let wxml = "<view>\n  <view wx:if=\"{{ flag }}\">\n    <text>a</text>\n  </view>\n  <view wx:elif=\"{{ ready }}\">\n    <text>{{ title.length }}</text>\n  </view>\n  <view wx:else>\n    <text>b</text>\n  </view>\n</view>";
    let value_range = range_of(wxml, "title.length");
    let flag_range = range_of(wxml, "flag");
    let ready_range = range_of(wxml, "ready");

    let mut value = fragment("value", "title.length", value_range);
    value.guards = vec![chain(
        vec![
            condition(GuardKind::If, "flag"),
            condition(GuardKind::Elif, "ready"),
        ],
        1,
    )];
    let flag = fragment("flag", "flag", flag_range);
    let ready = fragment("ready", "ready", ready_range);

    let built = build(&[value, flag, ready]);
    assert_eq!(built.status, BuildStatus::Supported);
    let value_mount = mount(&built, "value");
    assert!(
        value_mount
            .text
            .trim_start()
            .starts_with("if (__wxml_root.flag) {")
    );
    assert!(value_mount.text.contains("} else if (__wxml_root.ready) {"));
    assert_eq!(
        map_nth(value_mount, "__wxml_root.flag", 0).range,
        Some(flag_range)
    );
    assert_eq!(
        map_nth(value_mount, "__wxml_root.ready", 0).range,
        Some(ready_range)
    );

    let mut else_value = fragment("else_value", "title.length", value_range);
    else_value.guards = vec![chain(
        vec![
            condition(GuardKind::If, "flag"),
            condition(GuardKind::Elif, "ready"),
            condition(GuardKind::Else, ""),
        ],
        2,
    )];
    let built = build(&[
        else_value,
        fragment("flag", "flag", flag_range),
        fragment("ready", "ready", ready_range),
    ]);
    let value_mount = mount(&built, "else_value");
    assert!(value_mount.text.contains("} else {"));
    assert_eq!(
        map_nth(value_mount, "__wxml_root.flag", 0).range,
        Some(flag_range)
    );
    assert_eq!(
        map_nth(value_mount, "__wxml_root.ready", 0).range,
        Some(ready_range)
    );
    assert!(!built.view.text.contains("if (__wxml_root.title.length)"));
}

#[test]
fn nested_chain_and_loop_bindings_fold_every_guard_condition() {
    let wxml = "<view>\n  <view wx:if=\"{{ flag }}\">\n    <view wx:for=\"{{ items }}\" wx:if=\"{{ item.active }}\">\n      <text>{{ item.name }}</text>\n    </view>\n  </view>\n</view>";
    let value_range = range_of(wxml, "item.name");
    let active_range = range_of(wxml, "item.active");
    let flag_range = range_of(wxml, "flag");

    let bindings = vec![LocalBinding {
        name: "item".to_owned(),
        role: LocalRole::Item,
        source_expr: "items".to_owned(),
    }];
    let mut value = fragment("value", "item.name", value_range);
    value.bindings = bindings.clone();
    value.guards = vec![
        chain(vec![condition(GuardKind::If, "flag")], 0),
        chain(vec![condition(GuardKind::If, "item.active")], 0),
    ];
    let mut guard = fragment("guard", "item.active", active_range);
    guard.bindings = bindings;
    guard.guards = vec![chain(vec![condition(GuardKind::If, "flag")], 0)];
    let flag = fragment("flag", "flag", flag_range);

    let built = build(&[value, guard, flag]);
    assert_eq!(built.status, BuildStatus::Supported);
    let value_mount = mount(&built, "value");
    assert!(
        value_mount
            .text
            .trim_start()
            .starts_with("if (__wxml_root.flag) {")
    );
    assert!(value_mount.text.contains("if (item.active) {"));
    assert!(built.view.text.contains("__wxml_item(__wxml_root.items)"));
    assert_eq!(
        map_nth(value_mount, "__wxml_root.flag", 0).range,
        Some(flag_range)
    );
    assert_eq!(
        map_nth(value_mount, "item.active", 0).range,
        Some(active_range)
    );
    assert_eq!(
        map_nth(value_mount, "item.name", 0).range,
        Some(value_range)
    );

    let guard_mount = mount(&built, "guard");
    // The standalone condition fragment also folds its enclosing guard.
    assert!(guard_mount.text.contains("if (__wxml_root.flag) {"));
    assert!(guard_mount.text.trim_end().ends_with("item.active"));
    assert_eq!(
        map_nth(guard_mount, "item.active", 0).range,
        Some(active_range)
    );
}

#[test]
fn an_outer_guard_with_inner_loop_bindings_still_folds() {
    // The guard fragment has no `item` binding, while the value fragment does.
    // Correlation uses the rewritten text, not binding equality, so the outer
    // condition still maps without inventing a source range.
    let wxml = "<view>\n  <view wx:if=\"{{ flag }}\">\n    <view wx:for=\"{{ items }}\">\n      <text>{{ item.name }}</text>\n    </view>\n  </view>\n</view>";
    let flag_range = range_of(wxml, "flag");
    let value_range = range_of(wxml, "item.name");

    let mut value = fragment("value", "item.name", value_range);
    value.bindings = vec![LocalBinding {
        name: "item".to_owned(),
        role: LocalRole::Item,
        source_expr: "items".to_owned(),
    }];
    value.guards = vec![chain(vec![condition(GuardKind::If, "flag")], 0)];
    let guard = fragment("guard", "flag", flag_range);

    let built = build(&[value, guard]);
    let value_mount = mount(&built, "value");
    assert!(
        value_mount
            .text
            .trim_start()
            .starts_with("if (__wxml_root.flag) {")
    );
    assert_eq!(
        map_nth(value_mount, "__wxml_root.flag", 0).range,
        Some(flag_range)
    );
    assert_eq!(
        map_nth(value_mount, "item.name", 0).range,
        Some(value_range)
    );
}

#[test]
fn bom_crlf_cjk_and_emoji_origins_keep_condition_ranges_exact() {
    let wxml = "\u{FEFF}<view>\r\n  <view wx:if=\"{{ title === '😀中文' }}\">\r\n    <text>{{ title.length }}</text>\r\n  </view>\r\n</view>";
    let condition_text = "title === '😀中文'";
    let condition_range = range_of(wxml, condition_text);
    let value_range = range_of(wxml, "title.length");

    let mut value = fragment("value", "title.length", value_range);
    value.guards = vec![chain(vec![condition(GuardKind::If, condition_text)], 0)];
    let guard = fragment("guard", condition_text, condition_range);

    let built = build(&[value, guard]);
    assert_eq!(built.status, BuildStatus::Supported);
    let value_mount = mount(&built, "value");
    assert!(
        value_mount
            .text
            .trim_start()
            .starts_with("if (__wxml_root.title === '😀中文') {"),
        "{}",
        value_mount.text
    );
    let wrapper = map_wrapper_condition(value_mount, "__wxml_root.title === '😀中文'");
    assert_eq!(wrapper.range, Some(condition_range));
    assert!(!wrapper.trimmed_synthetic_prefix);
    assert_eq!(
        map_nth(value_mount, "__wxml_root.title.length", 0).range,
        Some(value_range)
    );

    let guard_mount = mount(&built, "guard");
    assert_eq!(guard_mount.text, "__wxml_root.title === '😀中文'");
    assert_eq!(
        map_nth(guard_mount, "__wxml_root.title === '😀中文'", 0).range,
        Some(condition_range)
    );
}

#[test]
fn a_missing_condition_fragment_keeps_the_wrapper_unmapped() {
    let wxml = "<view>\n  <view wx:if=\"{{ title === 1 }}\">\n    <text>{{ title.length }}</text>\n  </view>\n</view>";
    let value_range = range_of(wxml, "title.length");

    let mut value = fragment("value", "title.length", value_range);
    value.guards = vec![chain(vec![condition(GuardKind::If, "title === 1")], 0)];

    let built = build(&[value]);
    let value_mount = mount(&built, "value");
    assert_eq!(value_mount.text, "__wxml_root.title.length");

    let wrapper_byte = built
        .view
        .text
        .find("if (__wxml_root.title === 1) {")
        .expect("guard wrapper");
    let wrapper_start = wrapper_byte + "if (".len();
    let wrapper_end = wrapper_start + "__wxml_root.title === 1".len();
    let wrapper_units = utf16_len(&built.view.text[..wrapper_start]);
    let wrapper_units_end = wrapper_units + utf16_len("__wxml_root.title === 1");
    assert!(
        built
            .view
            .mounts
            .iter()
            .all(|mount| !contains(mount, wrapper_units, wrapper_units_end)),
        "an unprovable wrapper range must stay outside every mount"
    );
    assert!(
        built
            .view
            .map
            .to_original(
                MappingLayer::Shadow,
                SourceRange::new(
                    u32::try_from(wrapper_start).expect("small"),
                    u32::try_from(wrapper_end).expect("small"),
                ),
            )
            .is_err(),
        "the wrapper copy must stay unmapped, not be clamped into the value"
    );
}

#[test]
fn ambiguous_condition_fragments_fail_closed() {
    let wxml = "<view>\n  <view wx:if=\"{{ flag }}\">\n    <text>{{ title.length }}</text>\n  </view>\n  <view wx:if=\"{{ flag }}\"></view>\n</view>";
    let first_flag = range_of(wxml, "flag");
    let second_start = wxml.rfind("flag").expect("second flag");
    let second_flag = SourceRange::new(
        u32::try_from(second_start).expect("small"),
        u32::try_from(second_start + "flag".len()).expect("small"),
    );
    let value_range = range_of(wxml, "title.length");

    let mut value = fragment("value", "title.length", value_range);
    value.guards = vec![chain(vec![condition(GuardKind::If, "flag")], 0)];
    let first = fragment("first", "flag", first_flag);
    let second = fragment("second", "flag", second_flag);

    let built = build(&[value, first, second]);
    let value_mount = mount(&built, "value");
    assert_eq!(
        value_mount.text, "__wxml_root.title.length",
        "two identical guard fragments make the range ambiguous"
    );
}
