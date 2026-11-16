//! Data-driven conditional attribute rules.
//!
//! Some builtin components accept different attributes per mode. Keeping the
//! table here (instead of branching in the completion handler) lets the rules
//! be tested in isolation and extended without touching the LSP mapping.

use super::context::AttributeOccurrence;

/// One value of a controlling attribute and the attributes it disables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModeCase {
    /// Literal value of the controlling attribute.
    pub(crate) value: &'static str,
    /// Attributes that are not available in this mode.
    pub(crate) hidden: &'static [&'static str],
}

/// A rule that makes some attributes conditional on another attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModeRule {
    /// Tag the rule applies to.
    pub(crate) tag: &'static str,
    /// Attribute that selects the mode.
    pub(crate) attribute: &'static str,
    /// Mode used when the controlling attribute is absent.
    pub(crate) default_value: &'static str,
    /// Known mode values.
    pub(crate) cases: &'static [ModeCase],
}

impl ModeRule {
    /// Returns the attributes hidden for one controlling value.
    ///
    /// An unknown literal or a dynamic `{{ ... }}` expression returns an empty
    /// slice: every statically legal candidate stays available instead of
    /// guessing one branch.
    pub(crate) fn hidden(&self, value: ModeValue<'_>) -> &'static [&'static str] {
        match value {
            ModeValue::Missing => self
                .case(self.default_value)
                .map_or(&[], |case| case.hidden),
            ModeValue::Literal(value) => self.case(value).map_or(&[], |case| case.hidden),
            ModeValue::Dynamic => &[],
        }
    }

    /// Returns whether `value` is a known mode value.
    pub(crate) fn knows_value(&self, value: &str) -> bool {
        self.case(value).is_some()
    }

    /// Finds one known mode case.
    fn case(&self, value: &str) -> Option<&'static ModeCase> {
        self.cases.iter().find(|case| case.value == value)
    }
}

/// How the controlling attribute's value could be classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModeValue<'a> {
    /// The attribute is absent; the tag default applies.
    Missing,
    /// A plain literal value.
    Literal(&'a str),
    /// A mustache or otherwise non-static value.
    Dynamic,
}

/// `picker` mode-dependent attributes.
///
/// Source: Mini Program `picker` documentation. `mode` itself stays available
/// in every case because it is the controlling attribute.
const PICKER_RULE: ModeRule = ModeRule {
    tag: "picker",
    attribute: "mode",
    default_value: "selector",
    cases: &[
        ModeCase {
            value: "selector",
            hidden: &[
                "start",
                "end",
                "fields",
                "custom-item",
                "level",
                "bindcolumnchange",
            ],
        },
        ModeCase {
            value: "multiSelector",
            hidden: &["start", "end", "fields", "custom-item", "level"],
        },
        ModeCase {
            value: "time",
            hidden: &[
                "range",
                "range-key",
                "fields",
                "custom-item",
                "level",
                "bindcolumnchange",
            ],
        },
        ModeCase {
            value: "date",
            hidden: &[
                "range",
                "range-key",
                "custom-item",
                "level",
                "bindcolumnchange",
            ],
        },
        ModeCase {
            value: "region",
            hidden: &[
                "range",
                "range-key",
                "start",
                "end",
                "fields",
                "bindcolumnchange",
            ],
        },
    ],
};

/// `input` type-dependent attributes.
///
/// `confirm-type` is only meaningful for `type="text"`; the remaining types
/// hide it instead of offering an attribute the platform ignores.
const INPUT_RULE: ModeRule = ModeRule {
    tag: "input",
    attribute: "type",
    default_value: "text",
    cases: &[
        ModeCase {
            value: "text",
            hidden: &[],
        },
        ModeCase {
            value: "number",
            hidden: &["confirm-type"],
        },
        ModeCase {
            value: "idcard",
            hidden: &["confirm-type"],
        },
        ModeCase {
            value: "digit",
            hidden: &["confirm-type"],
        },
        ModeCase {
            value: "safe-password",
            hidden: &["confirm-type"],
        },
        ModeCase {
            value: "nickname",
            hidden: &["confirm-type"],
        },
    ],
};

/// All rules, keyed by tag.
const RULES: &[&ModeRule] = &[&PICKER_RULE, &INPUT_RULE];

/// Finds the rule for a tag.
pub(crate) fn rule(tag: &str) -> Option<&'static ModeRule> {
    RULES.iter().copied().find(|rule| rule.tag == tag)
}

/// Reads the controlling attribute value from the current tag.
pub(crate) fn mode_value<'a>(
    rule: &ModeRule,
    attributes: &'a [AttributeOccurrence],
) -> ModeValue<'a> {
    let Some(attribute) = attributes
        .iter()
        .find(|attribute| attribute.name.eq_ignore_ascii_case(rule.attribute))
    else {
        return ModeValue::Missing;
    };
    let Some(value) = attribute.value.as_deref() else {
        return ModeValue::Missing;
    };
    let value = value.trim();
    if value.is_empty() || value.contains("{{") {
        return ModeValue::Dynamic;
    }
    ModeValue::Literal(value)
}

/// Returns whether `attribute` is unavailable for the current mode.
pub(crate) fn is_hidden(tag: &str, attribute: &str, occurrences: &[AttributeOccurrence]) -> bool {
    let Some(rule) = rule(tag) else {
        return false;
    };
    let value = mode_value(rule, occurrences);
    rule.hidden(value)
        .iter()
        .any(|hidden| hidden.eq_ignore_ascii_case(attribute))
}

#[cfg(test)]
mod tests {
    use wxml_text::TextRange;

    use super::*;

    fn occurrence(name: &str, value: Option<&str>) -> AttributeOccurrence {
        AttributeOccurrence {
            name: name.to_owned(),
            value: value.map(str::to_owned),
            range: TextRange::new(0, 1).expect("range"),
        }
    }

    #[test]
    fn picker_default_mode_is_selector() {
        let occurrences = Vec::new();
        assert!(is_hidden("picker", "start", &occurrences));
        assert!(!is_hidden("picker", "range", &occurrences));
    }

    #[test]
    fn picker_modes_change_the_available_attributes() {
        let date = [occurrence("mode", Some("date"))];
        assert!(!is_hidden("picker", "start", &date));
        assert!(!is_hidden("picker", "end", &date));
        assert!(!is_hidden("picker", "fields", &date));
        assert!(is_hidden("picker", "range", &date));
        assert!(is_hidden("picker", "bindcolumnchange", &date));

        let region = [occurrence("mode", Some("region"))];
        assert!(!is_hidden("picker", "custom-item", &region));
        assert!(is_hidden("picker", "start", &region));

        let multi = [occurrence("mode", Some("multiSelector"))];
        assert!(!is_hidden("picker", "bindcolumnchange", &multi));
        assert!(is_hidden("picker", "fields", &multi));
    }

    #[test]
    fn dynamic_or_unknown_modes_keep_every_candidate() {
        for value in [Some("{{ pickerMode }}"), Some("futureMode")] {
            let occurrences = [occurrence("mode", value)];
            let hidden = rule("picker")
                .expect("rule")
                .hidden(mode_value(rule("picker").expect("rule"), &occurrences));
            assert!(hidden.is_empty(), "{value:?}");
        }
        // An attribute without a value falls back to the default mode.
        let missing = [occurrence("mode", None)];
        assert!(is_hidden("picker", "start", &missing));
    }

    #[test]
    fn input_confirm_type_follows_the_text_type() {
        let text = [occurrence("type", Some("text"))];
        assert!(!is_hidden("input", "confirm-type", &text));
        let digit = [occurrence("type", Some("digit"))];
        assert!(is_hidden("input", "confirm-type", &digit));
        let default = Vec::new();
        assert!(!is_hidden("input", "confirm-type", &default));
    }

    #[test]
    fn rules_and_catalog_enumerations_stay_in_sync() {
        for rule in RULES {
            let values = super::super::catalog::values_for(rule.tag, rule.attribute);
            assert!(!values.is_empty(), "{} has no controlling enum", rule.tag);
            for case in rule.cases {
                assert!(
                    values.iter().any(|value| value.value == case.value),
                    "{}.{} is missing catalog value {}",
                    rule.tag,
                    rule.attribute,
                    case.value
                );
                for hidden in case.hidden {
                    assert!(
                        super::super::catalog::tag_attribute(rule.tag, hidden).is_some(),
                        "{}.{} hides unknown attribute {}",
                        rule.tag,
                        rule.attribute,
                        hidden
                    );
                }
            }
            for value in values {
                assert!(
                    rule.cases.iter().any(|case| case.value == value.value),
                    "{}.{} catalog value {} has no rule case",
                    rule.tag,
                    rule.attribute,
                    value.value
                );
            }
        }
    }

    #[test]
    fn unknown_tags_have_no_rule() {
        assert!(rule("view").is_none());
        assert!(!is_hidden("view", "hover-class", &[]));
    }
}
