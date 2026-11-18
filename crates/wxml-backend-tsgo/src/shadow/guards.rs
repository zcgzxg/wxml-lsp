//! `wx:if` / `wx:elif` / `wx:else` guard rendering.
//!
//! Turning a conditional chain into real `if` / `else if` / `else` statements
//! lets the checker narrow the mounted expression. Only the branches above the
//! selected one are rendered, so a fragment in the `wx:else` branch is checked
//! with every previous condition false, never with a later condition that does
//! not apply to it.
//!
//! A condition that cannot be rewritten safely never narrows and never
//! fabricates a check. The risky `||` shape over a member access degrades to
//! `unresolved` because control-flow narrowing cannot justify it.

/// Branch kind of one conditional attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    /// Leading `wx:if` branch.
    If,
    /// Subsequent `wx:elif` branch.
    Elif,
    /// Final `wx:else` branch.
    Else,
}

/// One condition in a guard chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardCondition {
    /// Attribute branch.
    pub branch: GuardKind,
    /// Condition expression text; empty for `wx:else`.
    pub expr: String,
}

/// One enclosing conditional chain, outermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardChain {
    /// Conditions in WXML order.
    pub conditions: Vec<GuardCondition>,
    /// Index of the selected branch; `conditions.len()` selects `wx:else`.
    pub selected: usize,
}

/// Rendered guard blocks for one chain.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderedGuards {
    /// `if (...) {` plus `} else if (...) {` lines; empty when unresolved.
    pub open: String,
    /// Matching closing brace; empty when unresolved.
    pub close: String,
    /// True when a condition could not be narrowed.
    pub unresolved: bool,
}

/// Renders one chain up to (and including) the selected branch.
pub fn render_guards(
    chain: &GuardChain,
    indent: &str,
    rewrite_condition: &mut dyn FnMut(&str) -> Option<String>,
) -> RenderedGuards {
    let conditions = &chain.conditions;
    if conditions.is_empty() {
        return RenderedGuards::default();
    }
    let selected = chain.selected.min(conditions.len());
    let last = if selected < conditions.len() {
        selected
    } else {
        conditions.len() - 1
    };
    let mut lines: Vec<String> = Vec::new();
    let mut terminated = false;

    for (index, condition) in conditions.iter().enumerate().take(last + 1) {
        if condition.branch == GuardKind::Else {
            lines.push(if index == 0 {
                format!("{indent}if (true) {{")
            } else {
                format!("{indent}}} else {{")
            });
            terminated = true;
            break;
        }
        let Some(rewritten) = rewrite_condition(&condition.expr) else {
            return RenderedGuards {
                open: String::new(),
                close: String::new(),
                unresolved: true,
            };
        };
        if contains_logical_or(&rewritten) && contains_member_access(&condition.expr) {
            return RenderedGuards {
                open: String::new(),
                close: String::new(),
                unresolved: true,
            };
        }
        lines.push(format!(
            "{indent}{} ({rewritten}) {{",
            if index == 0 { "if" } else { "} else if" }
        ));
    }

    if selected == conditions.len() && !terminated {
        lines.push(format!("{indent}}} else {{"));
    }
    RenderedGuards {
        open: format!("{}\n", lines.join("\n")),
        close: format!("{indent}}}\n"),
        unresolved: false,
    }
}

/// True when `text` contains `||` outside strings and comments.
fn contains_logical_or(text: &str) -> bool {
    let characters: Vec<char> = text.chars().collect();
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if let Some(active) = quote {
            if character == '\\' {
                index += 2;
                continue;
            }
            if character == active {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            index += 1;
            continue;
        }
        if character == '/' && characters.get(index + 1) == Some(&'/') {
            match characters
                .iter()
                .skip(index)
                .position(|value| *value == '\n')
            {
                Some(offset) => index += offset + 1,
                None => return false,
            }
            continue;
        }
        if character == '/' && characters.get(index + 1) == Some(&'*') {
            let rest: String = characters[index + 2..].iter().collect();
            match rest.find("*/") {
                Some(offset) => {
                    index += 2 + rest[..offset].chars().count() + 2;
                }
                None => return false,
            }
            continue;
        }
        if character == '|' && characters.get(index + 1) == Some(&'|') {
            return true;
        }
        index += 1;
    }
    false
}

/// True when an expression accesses a member/index outside strings.
fn contains_member_access(text: &str) -> bool {
    let characters: Vec<char> = text.chars().collect();
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if let Some(active) = quote {
            if character == '\\' {
                index += 2;
                continue;
            }
            if character == active {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            index += 1;
            continue;
        }
        if character == '.' || character == '[' {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(branch: GuardKind, expr: &str) -> GuardCondition {
        GuardCondition {
            branch,
            expr: expr.to_owned(),
        }
    }

    fn chain(conditions: Vec<GuardCondition>, selected: usize) -> GuardChain {
        GuardChain {
            conditions,
            selected,
        }
    }

    fn rewrite(expr: &str) -> Option<String> {
        (!expr.is_empty()).then(|| format!("__wxml_root.{expr}"))
    }

    #[test]
    fn render_guards_opens_if_elif_and_closes_one_block() {
        let rendered = render_guards(
            &chain(
                vec![
                    condition(GuardKind::If, "a"),
                    condition(GuardKind::Elif, "b"),
                ],
                1,
            ),
            "  ",
            &mut rewrite,
        );
        assert!(!rendered.unresolved);
        assert_eq!(
            rendered.open,
            "  if (__wxml_root.a) {\n  } else if (__wxml_root.b) {\n"
        );
        assert_eq!(rendered.close, "  }\n");
    }

    #[test]
    fn render_guards_renders_the_else_branch_as_a_trailing_else() {
        let rendered = render_guards(
            &chain(
                vec![
                    condition(GuardKind::If, "a"),
                    condition(GuardKind::Elif, "b"),
                ],
                2,
            ),
            "",
            &mut |expr| Some(expr.to_owned()),
        );
        assert_eq!(rendered.open, "if (a) {\n} else if (b) {\n} else {\n");
        assert_eq!(rendered.close, "}\n");
    }

    #[test]
    fn render_guards_never_renders_a_condition_after_the_selected_branch() {
        let rendered = render_guards(
            &chain(
                vec![
                    condition(GuardKind::If, "a"),
                    condition(GuardKind::Elif, "b"),
                ],
                0,
            ),
            "",
            &mut |expr| Some(expr.to_owned()),
        );
        assert_eq!(rendered.open, "if (a) {\n");
        assert!(!rendered.open.contains('b'));
    }

    #[test]
    fn plain_value_level_or_guards_are_not_degraded() {
        let rendered = render_guards(
            &chain(vec![condition(GuardKind::If, "flagA || flagB")], 0),
            "",
            &mut |expr| Some(expr.to_owned()),
        );
        assert_eq!(
            rendered,
            RenderedGuards {
                open: "if (flagA || flagB) {\n".to_owned(),
                close: "}\n".to_owned(),
                unresolved: false,
            }
        );
    }

    #[test]
    fn an_unrewritable_condition_yields_no_narrowing_block() {
        let rendered = render_guards(
            &chain(vec![condition(GuardKind::If, "a")], 0),
            "",
            &mut |_| None,
        );
        assert_eq!(
            rendered,
            RenderedGuards {
                open: String::new(),
                close: String::new(),
                unresolved: true,
            }
        );
    }

    #[test]
    fn risky_or_over_member_access_is_unresolved() {
        let rendered = render_guards(
            &chain(
                vec![condition(
                    GuardKind::If,
                    "state.kind !== 'ready' || state.user",
                )],
                0,
            ),
            "",
            &mut |expr| Some(expr.to_owned()),
        );
        assert!(rendered.unresolved);
        assert_eq!(rendered.open, "");
    }

    #[test]
    fn an_else_condition_terminates_the_chain() {
        let rendered = render_guards(
            &chain(
                vec![
                    condition(GuardKind::If, "a"),
                    condition(GuardKind::Else, ""),
                    condition(GuardKind::Elif, "b"),
                ],
                2,
            ),
            "",
            &mut |expr| Some(expr.to_owned()),
        );
        assert_eq!(rendered.open, "if (a) {\n} else {\n");
    }
}
