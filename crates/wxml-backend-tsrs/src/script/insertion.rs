//! Safe insertion anchors for a missing event handler.
//!
//! The builder mirrors the event owner table: a `Page` gets the member in its
//! options object, a `Component` uses the existing literal `methods` object or
//! wraps a new one. The anchor is only produced when the closing brace has a
//! clean gap (whitespace or a trailing comma), so a comment or an unexpected
//! token can never be moved by the quick fix.

use crate::input::SourceFile;
use crate::shadow::expression;

use super::registration::{ParsedRegistration, RegistrationKind};
use super::scanner::{Script, member_last, object_bounds, scan_members};
use super::symbol_facts::HandlerInsertionFact;

/// Builds one missing-handler insertion.
pub(crate) fn build(
    file: &SourceFile,
    script: &Script<'_>,
    registration: &ParsedRegistration,
    name: &str,
) -> Result<HandlerInsertionFact, String> {
    if !expression::is_binding_identifier(name) {
        return Err("the handler name is not a valid identifier".to_owned());
    }
    let kind = registration.fact.kind;
    if !matches!(kind, RegistrationKind::Page | RegistrationKind::Component) {
        return Err("a behavior registration has no event-handler container".to_owned());
    }
    let open = super::registration::find_options_open(script, &registration.fact)
        .map_err(|_| "the registration options could not be located".to_owned())?;
    let close = script
        .close_of(open)
        .map_err(|_| "the registration options are not balanced".to_owned())?;

    let (container_open, container_close, wrap_methods) = if kind == RegistrationKind::Page {
        (open, close, false)
    } else {
        match member_last(&registration.members, "methods") {
            Some(member) => {
                let (open, close) = object_bounds(script, member)
                    .ok_or_else(|| "component `methods` is not a literal object".to_owned())?;
                (open, close, false)
            }
            None => (open, close, true),
        }
    };

    let properties = scan_members(script, container_open + 1, container_close);
    let text = script.text;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let container_start = script
        .token_start(container_open)
        .ok_or_else(|| "the container has no byte offset".to_owned())?;
    let container_indent = line_indent(text, container_start);
    let indent = properties.first().map_or_else(
        || format!("{container_indent}  "),
        |first| {
            let line_start = text
                .get(..first.key_start)
                .and_then(|prefix| prefix.rfind('\n'))
                .map_or(0, |index| index + 1);
            let candidate = text.get(line_start..first.key_start).unwrap_or("");
            if candidate
                .chars()
                .all(|character| matches!(character, ' ' | '\t'))
            {
                candidate.to_owned()
            } else {
                format!("{container_indent}  ")
            }
        },
    );

    let member = if wrap_methods {
        let inner = format!("{indent}  ");
        format!(
            "methods: {{{newline}{inner}{name}() {{{newline}{inner}  {newline}{inner}}},{newline}{indent}}}"
        )
    } else {
        format!("{name}() {{{newline}{indent}  {newline}{indent}}}")
    };

    let (prefix, trailing_comma) = if let Some(last) = properties.last() {
        let last_end = script
            .token_end(last.value_end.saturating_sub(1))
            .ok_or_else(|| "the last member has no byte range".to_owned())?;
        let close_start = script
            .token_start(container_close)
            .ok_or_else(|| "the closing brace has no byte offset".to_owned())?;
        if last_end > close_start {
            return Err("the last member overlaps the closing brace".to_owned());
        }
        let gap = text.get(last_end..close_start).unwrap_or("");
        if gap.contains("/*") || gap.contains("//") {
            return Err(
                "comments before the closing brace make the insertion anchor ambiguous".to_owned(),
            );
        }
        let trimmed = gap.trim();
        let needs_comma = trimmed.is_empty();
        if !needs_comma && trimmed != "," {
            return Err("the closing brace is not a clean insertion anchor".to_owned());
        }
        (if needs_comma { "," } else { "" }, ",")
    } else {
        ("", "")
    };

    let insert_text =
        format!("{prefix}{newline}{indent}{member}{trailing_comma}{newline}{container_indent}");
    if insert_text.contains('\0') {
        return Err("the handler insertion text contains a NUL".to_owned());
    }
    let offset = script
        .token_start(container_close)
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| "the insertion offset cannot be mapped".to_owned())?;
    Ok(HandlerInsertionFact {
        path: file.path().to_owned(),
        offset,
        text: insert_text,
    })
}

fn line_indent(text: &str, position: usize) -> String {
    let start = text
        .get(..position)
        .and_then(|prefix| prefix.rfind('\n'))
        .map_or(0, |index| index + 1);
    let mut end = start;
    for (offset, character) in text.get(start..).unwrap_or("").char_indices() {
        if matches!(character, ' ' | '\t') {
            end = start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    text.get(start..end).unwrap_or("").to_owned()
}
