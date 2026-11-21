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

use crate::shadow::mapping::utf16_len;
use crate::shadow::rewrite::{MountedSegment, RewrittenExpression};

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

/// One condition rendered into a planned guard block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedGuardCondition {
    /// Index into the chain's conditions.
    pub condition_index: usize,
    /// Offset of [`Self::text`] inside [`RenderedGuards::open`], UTF-16 units.
    pub open_start_utf16: u32,
    /// Rewritten condition text.
    pub text: String,
    /// Segments relative to [`Self::text`].
    pub segments: Vec<MountedSegment>,
    /// Target identifier offset relative to [`Self::text`].
    pub target_offset_utf16: u32,
}

/// Planned guard rendering: the rendered text plus one plan per condition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlannedGuards {
    /// Rendered `if` / `else if` / `else` text.
    pub rendered: RenderedGuards,
    /// Condition plans in rendering order.
    pub conditions: Vec<RenderedGuardCondition>,
}

struct RenderedCondition {
    text: String,
    segments: Vec<MountedSegment>,
    target_offset_utf16: u32,
}

/// Renders one chain up to (and including) the selected branch.
pub fn render_guards(
    chain: &GuardChain,
    indent: &str,
    rewrite_condition: &mut dyn FnMut(&str) -> Option<String>,
) -> RenderedGuards {
    let mut rewrite = |expr: &str| {
        rewrite_condition(expr).map(|text| RenderedCondition {
            text,
            segments: Vec::new(),
            target_offset_utf16: 0,
        })
    };
    render_guards_planned_with(chain, indent, &mut rewrite).rendered
}

/// Renders one chain and keeps every condition's rewritten source segments.
///
/// The rendered text is byte-identical to [`render_guards`]. Each plan records
/// where its condition text lives inside [`RenderedGuards::open`] so a layout
/// can map a diagnostic on the `if (...)` wrapper copy back to the original
/// WXML condition bytes.
pub fn render_guards_planned(
    chain: &GuardChain,
    indent: &str,
    rewrite_condition: &mut dyn FnMut(&str) -> Option<RewrittenExpression>,
) -> PlannedGuards {
    let mut rewrite = |expr: &str| {
        rewrite_condition(expr).map(|rewritten| RenderedCondition {
            text: rewritten.text,
            segments: rewritten.segments,
            target_offset_utf16: rewritten.target_offset_utf16,
        })
    };
    render_guards_planned_with(chain, indent, &mut rewrite)
}

fn render_guards_planned_with(
    chain: &GuardChain,
    indent: &str,
    rewrite_condition: &mut dyn FnMut(&str) -> Option<RenderedCondition>,
) -> PlannedGuards {
    let conditions = &chain.conditions;
    if conditions.is_empty() {
        return PlannedGuards::default();
    }
    let selected = chain.selected.min(conditions.len());
    let last = if selected < conditions.len() {
        selected
    } else {
        conditions.len() - 1
    };
    let mut lines: Vec<String> = Vec::new();
    let mut plans: Vec<RenderedGuardCondition> = Vec::new();
    let mut open_len = 0_u32;
    let mut terminated = false;

    for (index, condition) in conditions.iter().enumerate().take(last + 1) {
        if condition.branch == GuardKind::Else {
            let line = if index == 0 {
                format!("{indent}if (true) {{")
            } else {
                format!("{indent}}} else {{")
            };
            lines.push(line);
            terminated = true;
            break;
        }
        let Some(rewritten) = rewrite_condition(&condition.expr) else {
            return PlannedGuards {
                rendered: RenderedGuards {
                    open: String::new(),
                    close: String::new(),
                    unresolved: true,
                },
                conditions: Vec::new(),
            };
        };
        if contains_logical_or(&rewritten.text) && contains_member_access(&condition.expr) {
            return PlannedGuards {
                rendered: RenderedGuards {
                    open: String::new(),
                    close: String::new(),
                    unresolved: true,
                },
                conditions: Vec::new(),
            };
        }
        if !lines.is_empty() {
            open_len += 1;
        }
        let line_prefix = format!("{indent}{} (", if index == 0 { "if" } else { "} else if" });
        plans.push(RenderedGuardCondition {
            condition_index: index,
            open_start_utf16: open_len + utf16_len(&line_prefix),
            text: rewritten.text.clone(),
            segments: rewritten.segments,
            target_offset_utf16: rewritten.target_offset_utf16,
        });
        let line = format!("{line_prefix}{}) {{", rewritten.text);
        open_len += utf16_len(&line);
        lines.push(line);
    }

    if selected == conditions.len() && !terminated {
        lines.push(format!("{indent}}} else {{"));
    }
    PlannedGuards {
        rendered: RenderedGuards {
            open: format!("{}\n", lines.join("\n")),
            close: format!("{indent}}}\n"),
            unresolved: false,
        },
        conditions: plans,
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
    use crate::shadow::rewrite::rewrite_expression;

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
    fn planned_guards_report_condition_offsets_and_source_segments() {
        let planned = render_guards_planned(
            &chain(
                vec![
                    condition(GuardKind::If, "a"),
                    condition(GuardKind::Elif, "b"),
                ],
                1,
            ),
            "  ",
            &mut |expr| {
                rewrite_expression(
                    expr,
                    None,
                    &crate::shadow::rewrite::RewriteOptions::default(),
                )
                .ok()
            },
        );
        assert_eq!(
            planned.rendered.open,
            "  if (__wxml_root.a) {\n  } else if (__wxml_root.b) {\n"
        );
        assert_eq!(planned.rendered.close, "  }\n");
        assert!(!planned.rendered.unresolved);
        assert_eq!(planned.conditions.len(), 2);

        let first = &planned.conditions[0];
        assert_eq!(first.condition_index, 0);
        assert_eq!(first.open_start_utf16, utf16_len("  if ("));
        assert_eq!(first.text, "__wxml_root.a");

        let second = &planned.conditions[1];
        let second_byte = planned
            .rendered
            .open
            .find("__wxml_root.b")
            .expect("second condition");
        assert_eq!(
            second.open_start_utf16,
            utf16_len(&planned.rendered.open[..second_byte])
        );
        assert_eq!(second.condition_index, 1);
        assert_eq!(second.text, "__wxml_root.b");
        assert_eq!(second.target_offset_utf16, 12);
        assert_eq!(
            second.segments.last().and_then(|segment| segment.source),
            Some(crate::shadow::mapping::SourceRange::new(0, 1)),
            "the condition's `b` points at the original expression byte"
        );
    }

    #[test]
    fn planned_guards_do_not_plan_an_else_branch() {
        let planned = render_guards_planned(
            &chain(
                vec![
                    condition(GuardKind::If, "a"),
                    condition(GuardKind::Else, ""),
                ],
                1,
            ),
            "",
            &mut |expr| {
                rewrite_expression(
                    expr,
                    None,
                    &crate::shadow::rewrite::RewriteOptions::default(),
                )
                .ok()
            },
        );
        assert_eq!(planned.rendered.open, "if (__wxml_root.a) {\n} else {\n");
        assert_eq!(planned.conditions.len(), 1);
        assert_eq!(planned.conditions[0].condition_index, 0);
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
