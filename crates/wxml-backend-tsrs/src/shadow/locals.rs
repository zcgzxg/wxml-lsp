//! Typed `wx:for` locals and fragment scope composition.
//!
//! Each fragment carries an ordered binding chain (an item optionally followed
//! by an index, per nesting level). The renderer turns that chain into nested
//! blocks whose declarations reuse the `wx:for` source expression, so the
//! TypeScript checker can derive the element/key type instead of the backend
//! guessing it.
//!
//! A deeper level that shadows an outer name while its source still reads it
//! (`wx:for="{{ item.members }}"` with `wx:for-item="item"`) captures the outer
//! value in the enclosing block; otherwise the generated declaration would hit
//! the temporal dead zone.
//!
//! JavaScript projects have no helper types here, so every binding falls back
//! to `any` and the renderer reports `unresolved` — a coverage gap, never a
//! clean check.

#![allow(clippy::format_push_string)] // generated helper text is tiny; format! stays readable

use std::collections::BTreeSet;

use crate::shadow::ShadowError;
use crate::shadow::expression;

/// Maximum nested `wx:for` scope depth materialized in one shadow.
///
/// The generated probe nests one block per level, so this bound also caps the
/// block nesting the TypeScript checker has to parse.
pub const MAX_LOCAL_DEPTH: usize = 32;

/// Script syntax of the generated probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// TypeScript: helper types and `undefined as any` fallbacks.
    TypeScript,
    /// JavaScript: `JSDoc` casts and `any` bindings.
    JavaScript,
}

/// Role of one `wx:for` binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalRole {
    /// Loop element.
    Item,
    /// Loop key/index.
    Index,
}

/// One local binding occurrence selected for the shadow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBinding {
    /// Original identifier text; must be a valid binding identifier.
    pub name: String,
    /// Binding role.
    pub role: LocalRole,
    /// `wx:for` source expression this binding iterates.
    pub source_expr: String,
}

/// One `wx:for` nesting level: an item, optionally followed by an index.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalLevel {
    /// Element binding.
    pub item: Option<LocalBinding>,
    /// Key/index binding.
    pub index: Option<LocalBinding>,
}

/// Rendered locals: opening block text, matching closers, coverage flag.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderedLocals {
    /// Opening `{` plus declarations, indented; empty when there are no levels.
    pub text: String,
    /// Closing braces, innermost first; empty when there are no levels.
    pub close: String,
    /// True when some declaration fell back to `any`.
    pub unresolved: bool,
}

/// One-time helper block for typed locals (TypeScript only).
pub const LOCAL_HELPERS_TS: &str = concat!(
    "type __WxmlIsAny<T> = 0 extends (1 & T) ? true : false;\n",
    "type __WxmlItem<T> = __WxmlIsAny<T> extends true ? any : T extends readonly (infer U)[] ? U : T extends object ? T[keyof T] : any;\n",
    "type __WxmlIndex<T> = __WxmlIsAny<T> extends true ? any : T extends readonly unknown[] ? number : T extends object ? Extract<keyof T, string> : number;\n",
    "function __wxml_item<T>(value: T): __WxmlItem<T> { return undefined as unknown as __WxmlItem<T>; }\n",
    "function __wxml_index<T>(value: T): __WxmlIndex<T> { return undefined as unknown as __WxmlIndex<T>; }",
);

/// Returns the helper block for `syntax`; JavaScript has no helper types.
#[must_use]
pub const fn local_helpers(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::TypeScript => LOCAL_HELPERS_TS,
        Syntax::JavaScript => "",
    }
}

/// Prefixes every line of `text` with `indent` (no trailing newline added).
#[must_use]
pub fn indent_lines(text: &str, indent: &str) -> String {
    text.split('\n')
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Declarations for template locals whose type is unknown.
#[must_use]
pub fn local_declarations(names: &[String], indent: &str, syntax: Syntax) -> String {
    let mut text = String::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for name in names {
        // Defensive API-boundary validation: never emit an arbitrary name into
        // the generated probe, even if a caller bypasses FragmentSpec.
        if !expression::is_binding_identifier(name) {
            continue;
        }
        if !seen.insert(name.as_str()) {
            continue;
        }
        match syntax {
            Syntax::TypeScript => {
                text.push_str(&format!("{indent}const {name} = undefined as any;\n"));
            }
            Syntax::JavaScript => {
                text.push_str(&format!(
                    "{indent}const {name} = /** @type {{any}} */ (undefined);\n"
                ));
            }
        }
    }
    text
}

/// Groups structured bindings into `wx:for` nesting levels by array order.
///
/// A new level starts at every item binding; an index attaches to the level it
/// follows. Grouping deliberately does not compare source expressions: two
/// nested loops may iterate the same list.
pub fn group_local_levels(bindings: &[LocalBinding]) -> Result<Vec<LocalLevel>, ShadowError> {
    let mut levels: Vec<LocalLevel> = Vec::new();
    for binding in bindings {
        if !expression::is_binding_identifier(&binding.name) {
            continue;
        }
        match binding.role {
            LocalRole::Item => {
                if let Some(current) = levels.last_mut()
                    && current.item.is_none()
                {
                    if current.index.as_ref().map(|index| &index.name) != Some(&binding.name) {
                        current.item = Some(binding.clone());
                    }
                    continue;
                }
                levels.push(LocalLevel {
                    item: Some(binding.clone()),
                    index: None,
                });
            }
            LocalRole::Index => {
                if let Some(current) = levels.last_mut() {
                    if current.index.is_some()
                        || current.item.as_ref().map(|item| &item.name) == Some(&binding.name)
                    {
                        continue;
                    }
                    current.index = Some(binding.clone());
                } else {
                    levels.push(LocalLevel {
                        item: None,
                        index: Some(binding.clone()),
                    });
                }
            }
        }
        if levels.len() > MAX_LOCAL_DEPTH {
            return Err(ShadowError::BudgetExceeded {
                budget: "locals.depth",
            });
        }
    }
    Ok(levels)
}

/// Renders the nested `wx:for` blocks of one fragment.
///
/// `rewrite_source` rewrites a loop source expression with the outer locals in
/// scope; `None` means the expression cannot be rewritten safely and the
/// binding falls back to `any`.
#[allow(clippy::too_many_lines)] // one linear pass over nested levels; split only with a contract change
pub fn render_locals(
    bindings: &[LocalBinding],
    indent: &str,
    syntax: Syntax,
    rewrite_source: &mut dyn FnMut(&str, &BTreeSet<String>) -> Option<String>,
) -> Result<RenderedLocals, ShadowError> {
    let levels = group_local_levels(bindings)?;
    if levels.is_empty() {
        return Ok(RenderedLocals::default());
    }

    let mut lines: Vec<String> = Vec::new();
    let mut closers: Vec<String> = Vec::new();
    let mut declared: Vec<BTreeSet<String>> = Vec::new();
    let mut unresolved = false;

    for (depth, level) in levels.iter().enumerate() {
        let level_indent = format!("{indent}{}", "  ".repeat(depth));
        let body_indent = format!("{level_indent}  ");
        let mut outer_names: BTreeSet<String> = BTreeSet::new();
        for names in &declared {
            outer_names.extend(names.iter().cloned());
        }
        let mut own_names: BTreeSet<String> = BTreeSet::new();
        if let Some(item) = &level.item {
            own_names.insert(item.name.clone());
        }
        if let Some(index) = &level.index {
            own_names.insert(index.name.clone());
        }
        declared.push(own_names.clone());

        let source_expr = level
            .item
            .as_ref()
            .map(|item| item.source_expr.clone())
            .or_else(|| level.index.as_ref().map(|index| index.source_expr.clone()))
            .unwrap_or_default();
        let mut source_text: Option<String> = None;
        let mut level_unresolved = false;
        let mut capture_lines: Vec<String> = Vec::new();

        match syntax {
            Syntax::JavaScript => {
                level_unresolved = true;
            }
            Syntax::TypeScript => {
                if let Some(referenced) = value_identifier_names(&source_expr) {
                    let mut rename_list: Vec<(String, String)> = Vec::new();
                    for name in &own_names {
                        if !referenced.contains(name) {
                            continue;
                        }
                        let Some(outer_level) = nearest_outer_level(&declared, depth, name) else {
                            continue;
                        };
                        let alias = capture_name(name, outer_level);
                        rename_list.push((name.clone(), alias.clone()));
                        outer_names.insert(alias.clone());
                        capture_lines.push(format!("const {alias} = {name};"));
                    }
                    let mut prepared = source_expr.clone();
                    if !rename_list.is_empty() {
                        match rename_value_identifiers(&source_expr, &rename_list) {
                            Some(renamed_source) => prepared = renamed_source,
                            None => level_unresolved = true,
                        }
                    }
                    if !level_unresolved {
                        source_text = rewrite_source(&prepared, &outer_names);
                        if source_text.is_none() {
                            level_unresolved = true;
                        }
                    }
                } else {
                    level_unresolved = true;
                }
                if let Some(text) = &source_text
                    && let Some(leftover) = value_identifier_names(text)
                    && own_names.iter().any(|name| leftover.contains(name))
                {
                    source_text = None;
                    level_unresolved = true;
                }
            }
        }
        if level_unresolved {
            unresolved = true;
        }

        for line in capture_lines {
            lines.push(format!("{level_indent}{line}"));
        }
        lines.push(format!("{level_indent}{{"));
        let fallback = match syntax {
            Syntax::TypeScript => "undefined as any",
            Syntax::JavaScript => "undefined",
        };
        if let Some(item) = &level.item {
            lines.push(format!(
                "{body_indent}{}",
                declaration(
                    syntax,
                    &item.name,
                    LocalRole::Item,
                    source_text.as_deref().unwrap_or(fallback)
                )
            ));
        }
        if let Some(index) = &level.index {
            lines.push(format!(
                "{body_indent}{}",
                declaration(
                    syntax,
                    &index.name,
                    LocalRole::Index,
                    source_text.as_deref().unwrap_or(fallback)
                )
            ));
        }
        closers.push(format!("{level_indent}}}"));
    }

    closers.reverse();
    Ok(RenderedLocals {
        text: format!("{}\n", lines.join("\n")),
        close: format!("{}\n", closers.join("\n")),
        unresolved,
    })
}

fn declaration(syntax: Syntax, name: &str, role: LocalRole, source: &str) -> String {
    match syntax {
        Syntax::JavaScript => format!("const {name} = /** @type {{any}} */ (undefined);"),
        Syntax::TypeScript => {
            let helper = match role {
                LocalRole::Item => "item",
                LocalRole::Index => "index",
            };
            format!("const {name} = __wxml_{helper}({source});")
        }
    }
}

fn capture_name(name: &str, outer_level: usize) -> String {
    format!("__wxml_capture_{name}_{outer_level}")
}

fn nearest_outer_level(declared: &[BTreeSet<String>], depth: usize, name: &str) -> Option<usize> {
    (0..depth)
        .rev()
        .find(|outer| declared[*outer].contains(name))
}

fn value_identifier_names(expr: &str) -> Option<BTreeSet<String>> {
    let collected = expression::collect_value_identifiers(expr).ok()?;
    Some(
        collected
            .roots
            .iter()
            .map(|identifier| expr[identifier.start..identifier.end].to_owned())
            .collect(),
    )
}

fn rename_value_identifiers(expr: &str, renames: &[(String, String)]) -> Option<String> {
    let collected = expression::collect_value_identifiers(expr).ok()?;
    let mut edits: Vec<(usize, usize, &str, bool)> = collected
        .roots
        .iter()
        .filter_map(|identifier| {
            let name = &expr[identifier.start..identifier.end];
            renames
                .iter()
                .find(|(from, _)| from == name)
                .map(|(_, alias)| {
                    (
                        identifier.start,
                        identifier.end,
                        alias.as_str(),
                        identifier.shorthand,
                    )
                })
        })
        .collect();
    edits.sort_by_key(|edit| edit.0);
    if edits.is_empty() {
        return Some(expr.to_owned());
    }
    let mut output = String::new();
    let mut cursor = 0;
    for (start, end, alias, shorthand) in edits {
        if start < cursor || end > expr.len() {
            return None;
        }
        output.push_str(&expr[cursor..start]);
        if shorthand {
            output.push_str(&expr[start..end]);
            output.push_str(": ");
        }
        output.push_str(alias);
        cursor = end;
    }
    output.push_str(&expr[cursor..]);
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadow::rewrite::{RewriteOptions, rewrite_expression};

    fn rewrite_source(expr: &str, outer: &BTreeSet<String>) -> Option<String> {
        let options = RewriteOptions {
            locals: outer.clone(),
            ..RewriteOptions::default()
        };
        rewrite_expression(expr, None, &options)
            .ok()
            .map(|rewritten| rewritten.text)
    }

    fn binding(name: &str, role: LocalRole, source: &str) -> LocalBinding {
        LocalBinding {
            name: name.to_owned(),
            role,
            source_expr: source.to_owned(),
        }
    }

    fn render(bindings: &[LocalBinding], indent: &str, syntax: Syntax) -> RenderedLocals {
        render_locals(bindings, indent, syntax, &mut rewrite_source).expect("valid locals")
    }

    #[test]
    fn render_locals_emits_nested_helper_declarations() {
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
    fn nested_levels_indent_and_closers_are_innermost_first() {
        let rendered = render(
            &[
                binding("group", LocalRole::Item, "groups"),
                binding("groupIndex", LocalRole::Index, "groups"),
                binding("member", LocalRole::Item, "group.members"),
            ],
            "  ",
            Syntax::TypeScript,
        );
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
    fn same_name_nested_chain_captures_the_outer_binding() {
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
    }

    #[test]
    fn unparseable_source_falls_back_to_any() {
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
    fn javascript_binds_locals_as_any_without_helpers() {
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
    fn invalid_binding_names_are_ignored() {
        let rendered = render(
            &[binding("class", LocalRole::Item, "users")],
            "",
            Syntax::TypeScript,
        );
        assert_eq!(rendered.text, "");
        assert_eq!(rendered.close, "");
    }

    #[test]
    fn case_protocol_0126_a_same_name_nested_chain_is_accepted_and_groups_two_levels() {
        // Oracle: `protocol.test.ts:126` — an inner loop may shadow an outer
        // name; array order encodes nesting, not uniqueness.
        let bindings = [
            binding("item", LocalRole::Item, "groups"),
            binding("index", LocalRole::Index, "groups"),
            binding("item", LocalRole::Item, "item.members"),
            binding("index", LocalRole::Index, "item.members"),
        ];
        let levels = group_local_levels(&bindings).expect("same-name chain accepted");
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
            Some("item")
        );
        assert_eq!(
            levels[1].index.as_ref().map(|index| index.name.as_str()),
            Some("index")
        );
        assert_eq!(
            levels[1]
                .item
                .as_ref()
                .map(|item| item.source_expr.as_str()),
            Some("item.members")
        );

        // The inner declaration captures the outer binding, so the generated
        // code has no temporal dead zone.
        let rendered = render(&bindings, "", Syntax::TypeScript);
        assert!(!rendered.unresolved);
        assert!(
            rendered
                .text
                .contains("const __wxml_capture_item_0 = item;")
        );
        assert!(
            rendered
                .text
                .contains("const item = __wxml_item(__wxml_capture_item_0.members);")
        );
        assert!(
            !rendered
                .text
                .contains("const item = __wxml_item(item.members)"),
            "the inner declaration must not reference its own binding (TDZ)"
        );
    }

    #[test]
    fn levels_are_grouped_by_array_order_not_source_expression() {
        let levels = group_local_levels(&[
            binding("item", LocalRole::Item, "users"),
            binding("index", LocalRole::Index, "users"),
            binding("member", LocalRole::Item, "users"),
            binding("memberIndex", LocalRole::Index, "users"),
        ])
        .expect("valid levels");
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
    fn long_flat_source_is_rewritten_without_overflowing() {
        let deep = vec!["a"; 8_000].join("+");
        let rendered = render(
            &[binding("item", LocalRole::Item, &deep)],
            "",
            Syntax::TypeScript,
        );
        assert!(!rendered.unresolved);
        assert!(
            rendered
                .text
                .contains("const item = __wxml_item(__wxml_root.a+__wxml_root.a")
        );
    }

    #[test]
    fn local_declarations_filters_invalid_and_duplicate_names() {
        let rendered = local_declarations(
            &[
                "item".to_owned(),
                "item".to_owned(),
                "class".to_owned(),
                "bad-name".to_owned(),
                "type".to_owned(),
            ],
            "  ",
            Syntax::TypeScript,
        );
        assert_eq!(
            rendered,
            "  const item = undefined as any;\n  const type = undefined as any;\n"
        );
    }

    #[test]
    fn depth_budget_is_a_typed_error() {
        let too_deep = (0..=MAX_LOCAL_DEPTH)
            .map(|index| binding(&format!("item{index}"), LocalRole::Item, "users"))
            .collect::<Vec<_>>();
        assert!(matches!(
            group_local_levels(&too_deep),
            Err(ShadowError::BudgetExceeded {
                budget: "locals.depth"
            })
        ));
    }
}
