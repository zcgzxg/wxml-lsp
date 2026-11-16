//! Local diagnostics for data-driven attribute rules.
//!
//! These complement the Node-backed binding diagnostics without replacing
//! them. The same [`ModeRule`] table drives completion and this bounded check,
//! so a picker/input mode never hides a candidate without explaining why.

use serde_json::{Value, json};
use wxml_text::{SourceRange, TextRange};

use crate::completion::rules::{self, ModeValue};
use crate::handlers::source_range_value;
use crate::workspace::Document;

/// Builds diagnostics for invalid mode values and attributes unavailable in
/// the selected mode.
pub(crate) fn diagnostics(document: &Document) -> Vec<Value> {
    let text = document.text();
    let parsed = wxml_syntax::parse(text);
    let document_ast = parsed.tree.document();
    let mut diagnostics = Vec::new();
    for element in document_ast.elements() {
        let Some(tag) = element.name() else {
            continue;
        };
        let Some(rule) = rules::rule(tag) else {
            continue;
        };
        let attributes: Vec<_> = element.attributes().collect();
        let controlling = attributes
            .iter()
            .find(|attribute| attribute.name() == Some(rule.attribute));
        let mode = controlling.map_or(ModeValue::Missing, |attribute| {
            let Some(value) = attribute.value() else {
                return ModeValue::Missing;
            };
            match value.literal_text() {
                Some(text) if !text.trim().is_empty() => ModeValue::Literal(text.trim()),
                Some(_) | None => ModeValue::Dynamic,
            }
        });

        if let ModeValue::Literal(value_text) = mode {
            if !rule.knows_value(value_text) {
                if let Some(attribute) = controlling {
                    if let Some(value) = attribute.value() {
                        push(
                            document,
                            &mut diagnostics,
                            literal_range(value),
                            &mode_code(rule.tag, "value"),
                            &format!(
                                "`{value_text}` is not a known `{} {}` value",
                                rule.tag, rule.attribute
                            ),
                        );
                    }
                }
            }
        }

        let hidden = rule.hidden(mode);
        if hidden.is_empty() {
            continue;
        }
        for attribute in &attributes {
            let Some(name) = attribute.name() else {
                continue;
            };
            if name == rule.attribute || !hidden.contains(&name) {
                continue;
            }
            let Some(range) = attribute.name_range() else {
                continue;
            };
            let mode_text = match mode {
                ModeValue::Missing => format!("{} (default)", rule.default_value),
                ModeValue::Literal(value) => value.to_owned(),
                ModeValue::Dynamic => continue,
            };
            push(
                document,
                &mut diagnostics,
                range,
                &mode_code(rule.tag, "attribute"),
                &format!(
                    "`{name}` is not available when `{}` is `{mode_text}`",
                    rule.attribute
                ),
            );
        }
    }
    diagnostics
}

/// Stable diagnostic code for one rule category.
fn mode_code(tag: &str, suffix: &str) -> String {
    format!("wxml-{tag}-mode-{suffix}")
}

/// Converts one source range and appends the diagnostic.
fn push(
    document: &Document,
    diagnostics: &mut Vec<Value>,
    range: TextRange,
    code: &str,
    message: &str,
) {
    let Some(range) = source_range_value(document, SourceRange::new(range)) else {
        return;
    };
    diagnostics.push(json!({
        "range": range,
        "severity": 2,
        "source": "wxml-lsp",
        "code": code,
        "message": message,
    }));
}

/// Returns the value token range without its surrounding quotes.
fn literal_range(value: wxml_syntax::ast::AttributeValue<'_>) -> TextRange {
    let range = value.text_range();
    if value.is_quoted() && range.end() > range.start().saturating_add(1) {
        TextRange::new(range.start() + 1, range.end() - 1).unwrap_or(range)
    } else {
        range
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::workspace::Workspace;

    fn diagnostics_for(text: &str) -> Vec<Value> {
        let mut workspace = Workspace::new();
        workspace
            .open("file:///a.wxml", text.to_owned(), Some(1))
            .expect("open");
        let document = workspace.get("file:///a.wxml").expect("document");
        diagnostics(document)
    }

    fn codes(values: &[Value]) -> Vec<&str> {
        values
            .iter()
            .filter_map(|value| value["code"].as_str())
            .collect()
    }

    #[test]
    fn reports_attributes_hidden_by_the_default_picker_mode() {
        let values = diagnostics_for("<picker start=\"1\" />");
        assert_eq!(codes(&values), ["wxml-picker-mode-attribute"]);
        assert_eq!(
            values[0]["range"],
            json!({
                "start": {"line": 0, "character": 8},
                "end": {"line": 0, "character": 13},
            })
        );
        assert!(
            values[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains("selector (default)"))
        );
    }

    #[test]
    fn date_mode_accepts_start_and_end_but_rejects_range() {
        assert_eq!(
            diagnostics_for("<picker mode=\"date\" start=\"1\" />").len(),
            0
        );
        let values = diagnostics_for("<picker mode=\"date\" range=\"1\" />");
        assert_eq!(codes(&values), ["wxml-picker-mode-attribute"]);
    }

    #[test]
    fn invalid_mode_values_and_input_types_are_reported() {
        let picker = diagnostics_for("<picker mode=\"future\" />");
        assert_eq!(codes(&picker), ["wxml-picker-mode-value"]);
        assert_eq!(
            picker[0]["range"],
            json!({
                "start": {"line": 0, "character": 14},
                "end": {"line": 0, "character": 20},
            })
        );

        let input = diagnostics_for("<input type=\"number\" confirm-type=\"go\" />");
        assert_eq!(codes(&input), ["wxml-input-mode-attribute"]);
        let invalid = diagnostics_for("<input type=\"future\" />");
        assert_eq!(codes(&invalid), ["wxml-input-mode-value"]);
    }

    #[test]
    fn dynamic_modes_do_not_report_false_positives() {
        assert_eq!(
            diagnostics_for("<picker mode=\"{{ pickerMode }}\" start=\"1\" />").len(),
            0
        );
        assert_eq!(
            diagnostics_for("<input type=\"{{ inputType }}\" confirm-type=\"go\" />").len(),
            0
        );
    }

    #[test]
    fn valid_modes_are_quiet() {
        assert_eq!(
            diagnostics_for("<picker mode=\"selector\" range=\"1\" />").len(),
            0
        );
        assert_eq!(
            diagnostics_for("<input type=\"text\" confirm-type=\"go\" />").len(),
            0
        );
    }
}
