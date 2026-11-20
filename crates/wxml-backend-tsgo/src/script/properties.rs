//! `data` and component `properties` overlays.
//!
//! The overlay restores what `miniprogram-api-typings` loses: the declared
//! `type | optionalTypes` union, the property default, and `JSDoc` (including
//! Chinese text). It also merges behavior contributions with the `WeChat`
//! precedence rules:
//!
//! - the component's own `data`/`properties` win over every behavior;
//! - later behaviors win over earlier ones;
//! - a behavior's own members win over its nested behaviors (the behavior
//!   resolver already applies this order).
//!
//! A non-literal `data` root, a dynamic property key or an unreadable behavior
//! member is reported as `Partial`; known fields remain available and no field
//! is invented. A same-file `interface`/`type` used as `data` is `Unsupported`
//! because no runtime object exists.

use wxml_types::CapabilityStatus;

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::behavior::{
    BehaviorDataMember, BehaviorProperty, BehaviorResolution, expand_static_fields,
};
use super::registration::ParsedRegistration;
use super::scanner::{MemberKind, Script, doc_before, member_last, object_bounds, scan_members};
use super::snapshot::Snapshot;
use super::symbols::ScriptRange;
use super::values;

/// Maximum own `data`/`properties` members processed per registration.
const MAX_OWN_MEMBERS: usize = 512;

/// Classification of the registration's `data` member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRootKind {
    /// Inline object literal.
    Object,
    /// No `data` member.
    Absent,
    /// Non-literal value expression; fields are not enumerated.
    Expression,
    /// Explicitly unsupported member shape.
    Unsupported,
    /// Same-file interface/type alias used as a value.
    TypeReference,
}

/// One `data` overlay field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataField {
    /// Field name.
    pub name: String,
    /// Conservative type text.
    pub type_text: String,
    /// False when the initializer depends on module scope.
    pub exact: bool,
    /// `JSDoc` text.
    pub documentation: String,
    /// Declaring file.
    pub path: String,
    /// Exact original name-token range.
    pub range: ScriptRange,
    /// True when the component's own data shadows a behavior field.
    pub own_override: bool,
}

/// One component property overlay entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFact {
    /// Property name.
    pub name: String,
    /// Union text from `type` + `optionalTypes`.
    pub type_text: String,
    /// True when every `type`/`optionalTypes` entry was readable.
    pub type_complete: bool,
    /// True when `optionalTypes` is present.
    pub has_optional_types: bool,
    /// `JSDoc` text.
    pub documentation: String,
    /// Exact `value` initializer source text, when present.
    pub default_text: Option<String>,
    /// Declaring file.
    pub path: String,
    /// Exact original key-token range.
    pub range: ScriptRange,
    /// True when the component's own property shadows a behavior property.
    pub own_override: bool,
}

/// Data overlay result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataOverlay {
    /// Supported only when the root and every field were read completely.
    pub status: CapabilityStatus,
    /// Root classification.
    pub root: DataRootKind,
    /// Known fields with own/behavior precedence applied.
    pub fields: Vec<DataField>,
    /// True when every root field was read exactly.
    pub complete: bool,
    /// Degradation reasons.
    pub problems: Vec<String>,
}

/// Property overlay result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyOverlay {
    /// Supported only when every declaration was read.
    pub status: CapabilityStatus,
    /// Properties with own/behavior precedence applied.
    pub properties: Vec<PropertyFact>,
    /// False when a spread/computed/non-literal declaration was skipped.
    pub complete: bool,
    /// Degradation reasons.
    pub problems: Vec<String>,
}

/// Builds both overlays for one selected registration.
pub(crate) fn build(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    registration: &ParsedRegistration,
    behavior: &BehaviorResolution,
) -> (DataOverlay, PropertyOverlay) {
    let data = build_data(snapshot, file, registration, behavior);
    let properties = build_properties(snapshot, file, registration, behavior);
    (data, properties)
}

fn build_data(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    registration: &ParsedRegistration,
    behavior: &BehaviorResolution,
) -> DataOverlay {
    let mut problems = behavior
        .problems
        .iter()
        .filter(|problem| problem.contains("data") || problem.contains("behavior"))
        .cloned()
        .collect::<Vec<_>>();
    let mut fields: Vec<DataField> = behavior
        .data
        .iter()
        .map(|field| data_from_behavior(field, false))
        .collect();
    let options_spread = registration
        .members
        .iter()
        .any(|member| member.kind == MemberKind::Spread);
    let mut complete = !behavior.data_incomplete && !options_spread;
    if options_spread {
        problems.push(
            "options contain spread members; data may be contributed outside this literal"
                .to_owned(),
        );
    }

    let Some(member) = member_last(&registration.members, "data") else {
        if behavior.status.is_supported() && complete {
            return DataOverlay {
                status: CapabilityStatus::Supported,
                root: DataRootKind::Absent,
                fields,
                complete,
                problems,
            };
        }
        return DataOverlay {
            status: CapabilityStatus::Partial {
                reason: problem_text(&problems, "behavior data is incomplete"),
            },
            root: DataRootKind::Absent,
            fields,
            complete: false,
            problems,
        };
    };
    let Ok(script) = Script::new(file.text()) else {
        return data_partial(DataRootKind::Unsupported, fields, problems);
    };
    if member.kind != MemberKind::Property {
        problems.push("`data` member is not a property assignment".to_owned());
        return data_partial(DataRootKind::Unsupported, fields, problems);
    }
    let Some((open, close)) = object_bounds(&script, member) else {
        if is_same_file_type(&script, member) {
            problems.push(
                "data root is a type-only reference (interface/type alias); no runtime data object exists"
                    .to_owned(),
            );
            return data_unsupported(DataRootKind::TypeReference, fields, problems);
        }
        problems.push(
            "`data` member is not an inline object literal; field-level roots are not enumerated"
                .to_owned(),
        );
        return data_partial(DataRootKind::Expression, fields, problems);
    };

    let (own_fields, incomplete, mut expansion_problems) =
        expand_static_fields(snapshot, file, &script, open, close + 1);
    problems.append(&mut expansion_problems);
    complete &= !incomplete;
    for field in own_fields {
        let own_override = fields.iter().any(|existing| existing.name == field.name);
        if !field.exact {
            complete = false;
        }
        let overlay = data_from_behavior(&field, own_override);
        if let Some(existing) = fields
            .iter_mut()
            .find(|existing| existing.name == overlay.name)
        {
            *existing = overlay;
        } else {
            fields.push(overlay);
        }
    }

    let status = if complete && behavior.status.is_supported() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: problem_text(&problems, "data overlay is incomplete"),
        }
    };
    DataOverlay {
        status,
        root: DataRootKind::Object,
        fields,
        complete,
        problems,
    }
}

#[allow(clippy::too_many_lines)] // one linear declaration pass
fn build_properties(
    _snapshot: &Snapshot<'_>,
    file: &SourceFile,
    registration: &ParsedRegistration,
    behavior: &BehaviorResolution,
) -> PropertyOverlay {
    let mut problems = behavior
        .problems
        .iter()
        .filter(|problem| problem.contains("propert"))
        .cloned()
        .collect::<Vec<_>>();
    let mut properties: Vec<PropertyFact> = behavior
        .properties
        .iter()
        .map(|property| property_from_behavior(property, false))
        .collect();
    let options_spread = registration
        .members
        .iter()
        .any(|member| member.kind == MemberKind::Spread);
    let mut complete = !behavior.properties_incomplete && !options_spread;
    if options_spread {
        problems.push(
            "options contain spread members; properties may be contributed outside this literal"
                .to_owned(),
        );
    }

    let Some(member) = member_last(&registration.members, "properties") else {
        return PropertyOverlay {
            status: if complete && behavior.status.is_supported() {
                CapabilityStatus::Supported
            } else {
                CapabilityStatus::Partial {
                    reason: problem_text(&problems, "behavior properties are incomplete"),
                }
            },
            properties,
            complete,
            problems,
        };
    };
    if member.kind != MemberKind::Property {
        problems.push("component `properties` is not a property assignment".to_owned());
        return property_partial(properties, problems);
    }
    let Ok(script) = Script::new(file.text()) else {
        return property_partial(properties, problems);
    };
    let Some((open, close)) = object_bounds(&script, member) else {
        problems.push("component `properties` is not an object literal".to_owned());
        return property_partial(properties, problems);
    };
    let entries = scan_members(&script, open + 1, close);
    if entries.len() > MAX_OWN_MEMBERS {
        problems.push(format!(
            "component `properties` exceeds the {MAX_OWN_MEMBERS}-member processing budget"
        ));
        complete = false;
    }
    for entry in entries.into_iter().take(MAX_OWN_MEMBERS) {
        if entry.kind == MemberKind::Computed {
            problems.push("`properties` has a computed member name".to_owned());
            complete = false;
            continue;
        }
        if entry.kind != MemberKind::Property {
            problems.push(
                "`properties` uses a spread, shorthand or method member; declared properties are incomplete"
                    .to_owned(),
            );
            complete = false;
            continue;
        }
        let Some(name) = &entry.name else {
            problems.push("`properties` has a computed member name".to_owned());
            complete = false;
            continue;
        };
        let Some((option_open, option_close)) = object_bounds(&script, &entry) else {
            problems.push(format!(
                "property `{name}` is not declared with an object literal"
            ));
            complete = false;
            continue;
        };
        let options = scan_members(&script, option_open + 1, option_close);
        let Some((type_text, union_complete)) = values::union_text(&script, &options) else {
            problems.push(format!(
                "property `{name}` has no readable `type` declaration"
            ));
            complete = false;
            continue;
        };
        if !union_complete {
            problems.push(format!(
                "property `{name}` has an unreadable `type`/`optionalTypes` entry"
            ));
            complete = false;
        }
        let Some(range) = property_range(&script, &entry) else {
            complete = false;
            continue;
        };
        let default_text = member_last(&options, "value")
            .and_then(|value| value.value_range(&script))
            .and_then(|range| text_of(file.text(), range))
            .map(|text| text.trim().to_owned());
        let own_override = properties.iter().any(|existing| existing.name == *name);
        let property = PropertyFact {
            name: name.clone(),
            type_text,
            type_complete: union_complete,
            has_optional_types: member_last(&options, "optionalTypes").is_some(),
            documentation: doc_before(script.text, entry.key_start),
            default_text,
            path: file.path().to_owned(),
            range,
            own_override,
        };
        if let Some(existing) = properties
            .iter_mut()
            .find(|existing| existing.name == property.name)
        {
            *existing = property;
        } else {
            properties.push(property);
        }
    }

    let status = if complete && behavior.status.is_supported() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: problem_text(&problems, "property overlay is incomplete"),
        }
    };
    PropertyOverlay {
        status,
        properties,
        complete,
        problems,
    }
}

fn data_from_behavior(field: &BehaviorDataMember, own_override: bool) -> DataField {
    DataField {
        name: field.name.clone(),
        type_text: field.type_text.clone(),
        exact: field.exact,
        documentation: field.documentation.clone(),
        path: field.path.clone(),
        range: field.range,
        own_override,
    }
}

fn property_from_behavior(property: &BehaviorProperty, own_override: bool) -> PropertyFact {
    PropertyFact {
        name: property.name.clone(),
        type_text: property.type_text.clone(),
        type_complete: property.type_complete,
        has_optional_types: property.has_optional_types,
        documentation: property.documentation.clone(),
        default_text: property.default_text.clone(),
        path: property.path.clone(),
        range: property.range,
        own_override,
    }
}

fn data_partial(root: DataRootKind, fields: Vec<DataField>, problems: Vec<String>) -> DataOverlay {
    DataOverlay {
        status: CapabilityStatus::Partial {
            reason: problem_text(&problems, "data overlay is incomplete"),
        },
        root,
        fields,
        complete: false,
        problems,
    }
}

fn data_unsupported(
    root: DataRootKind,
    fields: Vec<DataField>,
    problems: Vec<String>,
) -> DataOverlay {
    DataOverlay {
        status: CapabilityStatus::Unsupported {
            reason: problem_text(&problems, "data overlay is unsupported"),
        },
        root,
        fields,
        complete: false,
        problems,
    }
}

fn property_partial(properties: Vec<PropertyFact>, problems: Vec<String>) -> PropertyOverlay {
    PropertyOverlay {
        status: CapabilityStatus::Partial {
            reason: problem_text(&problems, "property overlay is incomplete"),
        },
        properties,
        complete: false,
        problems,
    }
}

fn problem_text(problems: &[String], fallback: &str) -> String {
    if problems.is_empty() {
        fallback.to_owned()
    } else {
        problems.join("; ")
    }
}

fn property_range(script: &Script<'_>, member: &super::scanner::Member) -> Option<ScriptRange> {
    script.source_range(member.key_range()).ok()
}

fn text_of(text: &str, range: SourceRange) -> Option<String> {
    text.get(range.start.get() as usize..range.end.get() as usize)
        .map(str::to_owned)
}

fn is_same_file_type(script: &Script<'_>, member: &super::scanner::Member) -> bool {
    if member.value_start + 1 != member.value_end {
        return false;
    }
    let Some(name) = script.token_text(member.value_start) else {
        return false;
    };
    if script.kind(member.value_start) != Some(TokenKind::Identifier) {
        return false;
    }
    (0..script.tokens.len()).any(|index| {
        (script.is_ident(index, "interface") || script.is_ident(index, "type"))
            && script.is_ident(index + 1, name)
    })
}
