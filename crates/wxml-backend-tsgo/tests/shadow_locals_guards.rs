#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::missing_const_for_fn
)]

//! Integration tests for typed `wx:for` locals and guard rendering.
//!
//! Case-prefixed tests carry the TypeScript oracle case id and assert only the
//! rendering/status layer. Structural guard tests are named
//! `guards_<id>_rendered_structure_only_*`; control-flow narrowing itself needs
//! the checker and is listed in [`BLOCKED_CASES`].

use std::collections::BTreeSet;

use wxml_backend_tsgo::input::{DocumentRevision, SourceFile};
use wxml_backend_tsgo::shadow::guards::{GuardChain, GuardCondition, GuardKind, render_guards};
use wxml_backend_tsgo::shadow::layout::{
    BuildStatus, FragmentSpec, JsCheckApplied, JsCheckPolicy, RootKind, ShadowRequest,
    build_in_place_shadow,
};
use wxml_backend_tsgo::shadow::locals::{
    DEFAULT_LOCAL_BUDGET, LOCAL_HELPERS_TS, LocalBinding, LocalRole, Syntax, group_local_levels,
    local_helpers, render_locals,
};
use wxml_backend_tsgo::shadow::mapping::SourceRange;
use wxml_backend_tsgo::shadow::rewrite::{RewriteOptions, rewrite_expression};

const BLOCKED_CASES: &[(&str, &str)] = &[
    (
        "TS-LOCALS-0261",
        "end-to-end hover types require the checker",
    ),
    ("TS-LOCALS-0288", "TDZ diagnostics require the checker"),
    (
        "TS-LOCALS-0312",
        "JS hover/diagnostic status requires the checker",
    ),
    (
        "TS-GUARDS-0128",
        "truthiness narrowing requires the checker",
    ),
    (
        "TS-GUARDS-0152",
        "cumulative narrowing requires the checker",
    ),
    (
        "TS-GUARDS-0174",
        "loop-local narrowing requires the checker",
    ),
    (
        "TS-GUARDS-0218",
        "truthiness narrowing requires the checker",
    ),
    ("TS-GUARDS-0302", "diagnostics status requires the checker"),
    ("TS-GUARDS-0321", "checkJs diagnostics require the checker"),
    ("TS-GUARDS-0329", "JSDoc hover requires the checker"),
    (
        "TS-GUARDS-0097",
        "control-flow narrowing requires the checker; only rendered structure is asserted",
    ),
    (
        "TS-GUARDS-0218",
        "control-flow narrowing requires the checker; only rendered structure is asserted",
    ),
    (
        "TS-GUARDS-0261",
        "control-flow narrowing requires the checker; only rendered structure is asserted",
    ),
    (
        "FEATURE-BEHAVIOR-GRAPH",
        "behavior data/properties overlay resolution is BLOCKED",
    ),
];

fn source_file(path: &str, text: &str) -> SourceFile {
    SourceFile::new(path, text, DocumentRevision::new(1)).expect("valid source")
}

fn build(
    path: &str,
    text: &str,
    root: RootKind,
    fragments: &[FragmentSpec],
    js_check: JsCheckPolicy,
    check_js: bool,
) -> wxml_backend_tsgo::shadow::layout::BuiltShadow {
    let script = source_file(path, text);
    let mut request = ShadowRequest::new(&script, root);
    request.fragments = fragments;
    request.js_check = js_check;
    request.check_js = check_js;
    build_in_place_shadow(&request).expect("layout")
}

fn binding(name: &str, role: LocalRole, source: &str) -> LocalBinding {
    LocalBinding {
        name: name.to_owned(),
        role,
        source_expr: source.to_owned(),
    }
}

fn rewrite_source(expr: &str, outer: &BTreeSet<String>) -> Option<String> {
    let options = RewriteOptions {
        locals: outer.clone(),
        ..RewriteOptions::default()
    };
    rewrite_expression(expr, None, &options)
        .ok()
        .map(|rewritten| rewritten.text)
}

fn render(
    bindings: &[LocalBinding],
    indent: &str,
    syntax: Syntax,
) -> wxml_backend_tsgo::shadow::locals::RenderedLocals {
    render_locals(
        bindings,
        indent,
        syntax,
        DEFAULT_LOCAL_BUDGET,
        &mut rewrite_source,
    )
    .expect("valid locals")
}

fn condition(branch: GuardKind, expr: &str) -> GuardCondition {
    GuardCondition {
        branch,
        expr: expr.to_owned(),
    }
}

fn fragment(id: &str, expr: &str, origin: u32) -> FragmentSpec {
    FragmentSpec::new(
        id,
        expr,
        SourceRange::new(
            origin,
            origin + u32::try_from(expr.len()).expect("small expression"),
        ),
    )
}

#[test]
fn case_locals_0048_renderlocals_emits_nested_helper_declarations() {
    let rendered = render(
        &[
            binding("item", LocalRole::Item, "users"),
            binding("index", LocalRole::Index, "users"),
        ],
        "    ",
        Syntax::TypeScript,
    );
    assert!(!rendered.unresolved);
    assert_eq!(
        rendered.text,
        "    {\n      const item = __wxml_item(__wxml_root.users);\n      const index = __wxml_index(__wxml_root.users);\n"
    );
    assert_eq!(rendered.close, "    }\n");
}

#[test]
fn case_locals_0063_nested_levels_indent_and_closers_are_innermost_first() {
    let rendered = render(
        &[
            binding("group", LocalRole::Item, "groups"),
            binding("groupIndex", LocalRole::Index, "groups"),
            binding("member", LocalRole::Item, "group.members"),
        ],
        "  ",
        Syntax::TypeScript,
    );
    assert!(!rendered.unresolved);
    assert_eq!(
        rendered.text,
        [
            "  {",
            "    const group = __wxml_item(__wxml_root.groups);",
            "    const groupIndex = __wxml_index(__wxml_root.groups);",
            "    {",
            "      const member = __wxml_item(group.members);",
            "",
        ]
        .join("\n")
    );
    assert_eq!(rendered.close, "    }\n  }\n");
}

#[test]
fn case_locals_0089_a_same_name_nested_chain_captures_the_outer_binding_no_tdz() {
    let rendered = render(
        &[
            binding("item", LocalRole::Item, "groups"),
            binding("index", LocalRole::Index, "groups"),
            binding("item", LocalRole::Item, "item.members"),
            binding("index", LocalRole::Index, "item.members"),
        ],
        "",
        Syntax::TypeScript,
    );
    assert!(!rendered.unresolved);
    assert_eq!(
        rendered.text,
        [
            "{",
            "  const item = __wxml_item(__wxml_root.groups);",
            "  const index = __wxml_index(__wxml_root.groups);",
            "  const __wxml_capture_item_0 = item;",
            "  {",
            "    const item = __wxml_item(__wxml_capture_item_0.members);",
            "    const index = __wxml_index(__wxml_capture_item_0.members);",
            "",
        ]
        .join("\n")
    );
    assert_eq!(rendered.close, "  }\n}\n");
    assert!(
        !rendered
            .text
            .contains("const item = __wxml_item(item.members)")
    );
}

#[test]
fn case_locals_0124_an_unparseable_source_expression_falls_back_to_any() {
    let rendered = render(
        &[binding("item", LocalRole::Item, "users[")],
        "",
        Syntax::TypeScript,
    );
    assert!(rendered.unresolved);
    assert!(
        rendered
            .text
            .contains("const item = __wxml_item(undefined as any);")
    );
}

#[test]
fn case_locals_0130_javascript_mode_binds_locals_as_any_without_helper_types() {
    let rendered = render(
        &[binding("item", LocalRole::Item, "users")],
        "",
        Syntax::JavaScript,
    );
    assert!(rendered.unresolved);
    assert_eq!(
        rendered.text,
        "{\n  const item = /** @type {any} */ (undefined);\n"
    );
    assert_eq!(local_helpers(Syntax::JavaScript), "");
    assert_eq!(local_helpers(Syntax::TypeScript), LOCAL_HELPERS_TS);
}

#[test]
fn case_locals_0138_invalid_binding_names_are_ignored_defensively() {
    let rendered = render(
        &[binding("class", LocalRole::Item, "users")],
        "",
        Syntax::TypeScript,
    );
    assert!(rendered.text.is_empty());
    assert!(rendered.close.is_empty());
}

#[test]
fn case_locals_0144_levels_are_grouped_by_array_order_not_source_expression() {
    let levels = group_local_levels(
        &[
            binding("item", LocalRole::Item, "users"),
            binding("index", LocalRole::Index, "users"),
            binding("member", LocalRole::Item, "users"),
            binding("memberIndex", LocalRole::Index, "users"),
        ],
        DEFAULT_LOCAL_BUDGET,
    )
    .expect("levels");
    assert_eq!(levels.len(), 2);
    assert_eq!(
        levels[0].item.as_ref().map(|item| item.name.as_str()),
        Some("item")
    );
    assert_eq!(
        levels[0].index.as_ref().map(|index| index.name.as_str()),
        Some("index")
    );
    assert_eq!(
        levels[1].item.as_ref().map(|item| item.name.as_str()),
        Some("member")
    );
}

#[test]
fn case_locals_0334_an_extremely_deep_wx_for_source_degrades_instead_of_overflowing() {
    let deep = vec!["a"; 40_000].join("+");
    let rendered = render(
        &[binding("item", LocalRole::Item, &deep)],
        "",
        Syntax::TypeScript,
    );
    assert!(rendered.unresolved);
    assert!(
        rendered
            .text
            .contains("const item = __wxml_item(undefined as any);")
    );
}

#[test]
fn case_locals_0343_a_locals_only_request_still_degrades_to_partial_with_a_typed_reason() {
    // Oracle: TS-LOCALS-0343. Build-level: Partial, the exact stable reason,
    // and the `undefined as any` fallback. The adapter diagnostics status and
    // the TS per-fragment diagnostics DTO are not available here.
    let mut spec = fragment("legacy", "item.name", 0);
    spec.locals = vec!["item".to_owned()];
    let built = build(
        "index.ts",
        "Page({ data: { users: [{ name: 'Ada' }] } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.status, BuildStatus::Partial);
    assert_eq!(
        built.reason.as_deref(),
        Some("some template bindings or guards could not be resolved and were not checked")
    );
    assert!(
        built.notes.iter().any(|note| note
            == "unresolved `wx:for` bindings or `wx:if` guards are mounted as `any`/unnarrowed and not checked"),
        "{:?}",
        built.notes
    );
    assert!(built.view.text.contains("const item = undefined as any;"));
}

#[test]
fn case_guards_0047_renderguards_opens_if_elif_and_closes_one_block() {
    let rendered = render_guards(
        &GuardChain {
            conditions: vec![
                condition(GuardKind::If, "a"),
                condition(GuardKind::Elif, "b"),
            ],
            selected: 1,
        },
        "  ",
        &mut |expr| Some(format!("__wxml_root.{expr}")),
    );
    assert!(!rendered.unresolved);
    assert_eq!(
        rendered.open,
        "  if (__wxml_root.a) {\n  } else if (__wxml_root.b) {\n"
    );
    assert_eq!(rendered.close, "  }\n");
}

#[test]
fn case_guards_0057_renderguards_renders_the_else_branch_as_a_trailing_else() {
    let rendered = render_guards(
        &GuardChain {
            conditions: vec![
                condition(GuardKind::If, "a"),
                condition(GuardKind::Elif, "b"),
            ],
            selected: 2,
        },
        "",
        &mut |expr| Some(expr.to_owned()),
    );
    assert_eq!(rendered.open, "if (a) {\n} else if (b) {\n} else {\n");
    assert_eq!(rendered.close, "}\n");
}

#[test]
fn case_guards_0066_renderguards_never_renders_a_condition_after_the_selected_branch() {
    let rendered = render_guards(
        &GuardChain {
            conditions: vec![
                condition(GuardKind::If, "a"),
                condition(GuardKind::Elif, "b"),
            ],
            selected: 0,
        },
        "",
        &mut |expr| Some(expr.to_owned()),
    );
    assert_eq!(rendered.open, "if (a) {\n");
    assert!(!rendered.open.contains('b'));
}

#[test]
fn case_guards_0075_plain_value_level_or_guards_are_not_degraded() {
    let rendered = render_guards(
        &GuardChain {
            conditions: vec![condition(GuardKind::If, "flagA || flagB")],
            selected: 0,
        },
        "",
        &mut |expr| Some(expr.to_owned()),
    );
    assert_eq!(rendered.open, "if (flagA || flagB) {\n");
    assert_eq!(rendered.close, "}\n");
    assert!(!rendered.unresolved);
}

#[test]
fn case_guards_0087_an_unrewritable_condition_yields_no_narrowing_block() {
    let rendered = render_guards(
        &GuardChain {
            conditions: vec![condition(GuardKind::If, "a")],
            selected: 0,
        },
        "",
        &mut |_| None,
    );
    assert!(rendered.open.is_empty());
    assert!(rendered.close.is_empty());
    assert!(rendered.unresolved);
}

#[test]
fn guards_0097_rendered_structure_only_narrowing_blocked() {
    let mut spec = fragment("ready", "state.user.name", 10);
    spec.guards = vec![GuardChain {
        conditions: vec![
            condition(GuardKind::If, "state.kind === 'loading'"),
            condition(GuardKind::Elif, "state.kind === 'ready'"),
        ],
        selected: 1,
    }];
    let built = build(
        "index.ts",
        "Page({ data: { state: { kind: 'loading' } } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(
        built
            .view
            .text
            .contains("if (__wxml_root.state.kind === 'loading') {")
    );
    assert!(
        built
            .view
            .text
            .contains("} else if (__wxml_root.state.kind === 'ready') {")
    );
    assert!(!built.view.text.contains("} else {"));
    assert!(built.view.text.contains("__wxml_root.state.user.name;"));
}

#[test]
fn case_guards_0201_an_unparseable_guard_does_not_fabricate_a_diagnostic_and_reports_partial() {
    // Oracle: TS-GUARDS-0201. Build-level: Partial, the exact stable reason,
    // no rendered `if` and the `@ts-ignore` unnarrowed mount. The adapter
    // diagnostics status, `unresolvedGuards` count and empty top-level /
    // per-fragment diagnostics are not available at this layer.
    let mut spec = fragment("broken", "state.user.name", 0);
    spec.guards = vec![GuardChain {
        conditions: vec![condition(GuardKind::If, "state.kind === ")],
        selected: 0,
    }];
    let built = build(
        "index.ts",
        "Page({ data: { state: { kind: 'loading' } } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.status, BuildStatus::Partial);
    assert_eq!(
        built.reason.as_deref(),
        Some("some template bindings or guards could not be resolved and were not checked")
    );
    assert!(
        built.notes.iter().any(|note| note
            == "unresolved `wx:for` bindings or `wx:if` guards are mounted as `any`/unnarrowed and not checked"),
        "{:?}",
        built.notes
    );
    assert!(!built.view.text.contains("if ("));
    // `@ts-ignore` sits on the line immediately before the unnarrowed mount.
    let ignore = built.view.text.find("// @ts-ignore").expect("@ts-ignore");
    let mount = built
        .view
        .text
        .find("__wxml_root.state.user.name;")
        .expect("mount");
    assert!(ignore < mount, "{:?}", built.view.text);
    let between = built
        .view
        .text
        .get(ignore + "// @ts-ignore".len()..mount)
        .expect("between");
    assert!(
        between.trim().is_empty(),
        "the ignore must guard the mount, not another line: {between:?}"
    );
}

#[test]
fn guards_0218_rendered_structure_only_narrowing_blocked() {
    let mut and_spec = fragment("and", "maybeUser.name", 0);
    and_spec.guards = vec![GuardChain {
        conditions: vec![condition(GuardKind::If, "maybeUser && maybeUser.name")],
        selected: 0,
    }];
    let mut or_spec = fragment("or", "state.user.name", 20);
    or_spec.guards = vec![GuardChain {
        conditions: vec![condition(
            GuardKind::If,
            "state.kind !== 'ready' || state.user",
        )],
        selected: 0,
    }];
    let built = build(
        "index.ts",
        "Page({ data: { maybeUser: null, state: { kind: 'loading' } } });",
        RootKind::Page,
        &[and_spec, or_spec],
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.status, BuildStatus::Partial);
    assert!(
        built
            .view
            .text
            .contains("if (__wxml_root.maybeUser && __wxml_root.maybeUser.name) {")
    );
    assert!(
        !built
            .view
            .text
            .contains("if (__wxml_root.state.kind !== 'ready' ||")
    );
    assert!(built.view.text.contains("// @ts-ignore"));
}

#[test]
fn guards_0261_rendered_structure_only_narrowing_blocked() {
    let mut spec = fragment("inner", "item.name", 0);
    spec.bindings = vec![
        binding("item", LocalRole::Item, "groups"),
        binding("index", LocalRole::Index, "groups"),
        binding("item", LocalRole::Item, "item.members"),
        binding("index", LocalRole::Index, "item.members"),
    ];
    spec.guards = vec![GuardChain {
        conditions: vec![condition(GuardKind::If, "item.active")],
        selected: 0,
    }];
    let built = build(
        "index.ts",
        "Page({ data: { groups: [{ active: true, members: [{ name: 'Ada' }] }] } });",
        RootKind::Page,
        &[spec],
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(built.status, BuildStatus::Supported);
    assert!(
        built
            .view
            .text
            .contains("const item = __wxml_item(__wxml_root.groups);")
    );
    assert!(
        built
            .view
            .text
            .contains("const __wxml_capture_item_0 = item;")
    );
    assert!(
        built
            .view
            .text
            .contains("const item = __wxml_item(__wxml_capture_item_0.members);")
    );
    assert!(built.view.text.contains("if (item.active) {"));
    assert!(built.view.text.contains("item.name;"));
    // Closers: guard, then inner local, then outer local, then the block.
    let mount = "item.name;";
    let mount_end = built.view.text.find(mount).expect("mount") + mount.len();
    let tail = &built.view.text[mount_end..];
    assert!(
        tail.starts_with("\n              }\n            }\n          }\n        }\n"),
        "guard/local/block closers must be innermost first: {tail:?}"
    );
}

#[test]
fn case_guards_0312_pragma_injects_a_shadow_only_ts_check() {
    let built = build(
        "index.js",
        "Page({ data: { user: { name: 'Ada' } } });",
        RootKind::Page,
        &[fragment("typo", "user.nmae", 0)],
        JsCheckPolicy::Pragma,
        false,
    );
    assert_eq!(built.js_check_applied, JsCheckApplied::Pragma);
    assert!(built.view.text.starts_with("// @ts-check\nPage({"));
    assert!(built.notes.iter().any(|note| note.contains("@ts-check")));
}

#[test]
fn js_check_policy_values_match_the_oracle_without_publishing_diagnostics() {
    let off = build(
        "index.js",
        "Page({ data: {} });",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        false,
    );
    assert_eq!(off.js_check_applied, JsCheckApplied::Off);

    let inherited = build(
        "index.js",
        "Page({ data: {} });",
        RootKind::Page,
        &[],
        JsCheckPolicy::Inherit,
        true,
    );
    assert_eq!(inherited.js_check_applied, JsCheckApplied::Inherit);
}

#[test]
fn blocked_cases_are_documented_and_not_claimed() {
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-GUARDS-0128")
    );
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-GUARDS-0097")
    );
    assert!(
        BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-GUARDS-0261")
    );
    assert!(
        !BLOCKED_CASES
            .iter()
            .any(|(case, _)| *case == "TS-GUARDS-0047")
    );
}
