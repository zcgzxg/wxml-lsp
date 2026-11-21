//! Static Behavior graph resolution.
//!
//! The resolver mirrors the conservative subset of the TypeScript backend:
//!
//! - inline `Behavior({...})` and `const shared = Behavior({...})`;
//! - local relative imports (`import x from "./x"`, named imports, re-export
//!   aliases, `require(...)` and `import x = require(...)`);
//! - nested `behaviors: [...]`;
//! - local `export default`, `export =` and `module.exports =`.
//!
//! Later behaviors override earlier ones, a behavior's own members override
//! its nested behaviors, and the component's own declarations override every
//! behavior (applied by [`super::properties`]). Cycles, factory calls,
//! unresolved imports, dynamic arrays and non-literal `data`/`properties`
//! members are reported and make the affected area incomplete; the resolver
//! never fabricates a field or method.
//!
//! `data` spreads are expanded only when the value is a statically readable
//! object literal or a chain of local/imported `const` objects. A fully
//! readable spread such as `...BASE` therefore stays exact and `Supported`; an
//! unreadable spread is `Partial` with an `any` result.

use std::collections::BTreeSet;

use wxml_types::CapabilityStatus;

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::registration::ParsedRegistration;
use super::scanner::{
    Member, MemberKind, Script, doc_before, member_last, object_bounds, scan_members, signature_of,
};
use super::snapshot::Snapshot;
use super::symbol_facts::dedupe;
use super::symbols::ScriptRange;
use super::values::{self, ExprRange, infer_initializer};

/// Maximum behavior nesting depth; bounds [`resolve_expression`] and
/// [`expand_object`] recursion.
const MAX_DEPTH: usize = 16;

/// One resolved behavior declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorUnitFact {
    /// Project-relative path.
    pub path: String,
    /// Basename used in degradation messages.
    pub label: String,
    /// Exact options object range.
    pub range: ScriptRange,
    /// Import/alias name used by the component, when known.
    pub alias: Option<String>,
}

/// One `data` member contributed by a behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorDataMember {
    /// Field name.
    pub name: String,
    /// Conservative type text.
    pub type_text: String,
    /// False when the initializer is not self-contained.
    pub exact: bool,
    /// `JSDoc` text.
    pub documentation: String,
    /// Declaring file.
    pub path: String,
    /// Exact original name-token range.
    pub range: ScriptRange,
}

/// One component property contributed by a behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorProperty {
    /// Property name.
    pub name: String,
    /// Type union from `type` + `optionalTypes`.
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
}

/// One method contributed by a behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorMethod {
    /// Method name.
    pub name: String,
    /// One-line signature snippet.
    pub signature: String,
    /// `JSDoc` text; Chinese documentation is preserved.
    pub documentation: String,
    /// Declaring file.
    pub path: String,
    /// Exact original key-token range.
    pub range: ScriptRange,
}

/// Merged behavior graph facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorResolution {
    /// Supported only when every reachable behavior was read completely.
    pub status: CapabilityStatus,
    /// Resolved units in depth-first order.
    pub units: Vec<BehaviorUnitFact>,
    /// Merged data members (behavior-only; own data is applied separately).
    pub data: Vec<BehaviorDataMember>,
    /// Merged properties (behavior-only).
    pub properties: Vec<BehaviorProperty>,
    /// Merged methods (behavior-only).
    pub methods: Vec<BehaviorMethod>,
    /// True when any behavior data member was skipped or typed `any`.
    pub data_incomplete: bool,
    /// True when any behavior property was skipped.
    pub properties_incomplete: bool,
    /// True when any behavior method was skipped.
    pub methods_incomplete: bool,
    /// Deduplicated degradation reasons.
    pub problems: Vec<String>,
}

impl BehaviorResolution {
    pub(crate) const fn empty() -> Self {
        Self {
            status: CapabilityStatus::Supported,
            units: Vec::new(),
            data: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
            data_incomplete: false,
            properties_incomplete: false,
            methods_incomplete: false,
            problems: Vec::new(),
        }
    }

    fn finish(mut self) -> Self {
        let incomplete =
            self.data_incomplete || self.properties_incomplete || self.methods_incomplete;
        dedupe(&mut self.problems);
        self.status = if incomplete || !self.problems.is_empty() {
            CapabilityStatus::Partial {
                reason: if self.problems.is_empty() {
                    "behavior graph is incomplete".to_owned()
                } else {
                    self.problems.join("; ")
                },
            }
        } else {
            CapabilityStatus::Supported
        };
        self
    }
}

#[derive(Debug, Clone)]
struct Unit {
    path: String,
    label: String,
    options_start: usize,
    options_end: usize,
    alias: Option<String>,
}

impl Unit {
    fn identity(&self) -> String {
        format!("{}:{}", self.path, self.options_start)
    }
}

/// Resolves the static behavior graph of one component registration.
pub(crate) fn resolve(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    registration: &ParsedRegistration,
) -> BehaviorResolution {
    let Some(behaviors) = member_last(&registration.members, "behaviors") else {
        return BehaviorResolution::empty();
    };
    let Ok(script) = Script::new(file.text()) else {
        return partial_all("the script could not be scanned");
    };
    if behaviors.kind != MemberKind::Property
        || behaviors.value_start >= behaviors.value_end
        || !script.is_punct(behaviors.value_start, '[')
    {
        return partial_all("component `behaviors` is not a static array literal");
    }
    let Ok(close) = script.close_of(behaviors.value_start) else {
        return partial_all("component `behaviors` has an unbalanced array");
    };
    if close + 1 != behaviors.value_end {
        return partial_all("component `behaviors` is not a static array literal");
    }

    let mut problems = Vec::new();
    let mut units = Vec::new();
    let mut stack = BTreeSet::new();
    let mut unresolved = false;
    for (start, end) in values::array_elements(&script, behaviors.value_start, close) {
        if script.is_punct(start, '.')
            && script.is_punct(start + 1, '.')
            && script.is_punct(start + 2, '.')
        {
            problems.push("a behavior array spread is dynamic".to_owned());
            unresolved = true;
            continue;
        }
        if let Some(unit) = resolve_expression(
            snapshot,
            file,
            start,
            end,
            None,
            &mut stack,
            0,
            &mut problems,
        ) {
            collect_unit(snapshot, &unit, &mut units, &mut stack, 0, &mut problems);
        } else {
            unresolved = true;
        }
    }

    let mut merged = merge_units(snapshot, &units, &mut problems);
    if unresolved {
        merged.data_incomplete = true;
        merged.properties_incomplete = true;
        merged.methods_incomplete = true;
    }
    merged.units = units
        .iter()
        .filter_map(|unit| unit_fact(unit, snapshot))
        .collect();
    merged.finish()
}

fn unit_fact(unit: &Unit, snapshot: &Snapshot<'_>) -> Option<BehaviorUnitFact> {
    let file = snapshot.file(&unit.path)?;
    let script = Script::new(file.text()).ok()?;
    Some(BehaviorUnitFact {
        path: unit.path.clone(),
        label: unit.label.clone(),
        range: script
            .source_range(SourceRange::new(
                u32::try_from(unit.options_start).ok()?,
                u32::try_from(unit.options_end).ok()?,
            ))
            .ok()?,
        alias: unit.alias.clone(),
    })
}

fn partial_all(reason: &str) -> BehaviorResolution {
    BehaviorResolution {
        status: CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
        units: Vec::new(),
        data: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
        data_incomplete: true,
        properties_incomplete: true,
        methods_incomplete: true,
        problems: vec![reason.to_owned()],
    }
}

#[allow(clippy::too_many_arguments)] // one recursive resolver; splitting adds state plumbing
fn resolve_expression(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    mut start: usize,
    mut end: usize,
    alias: Option<String>,
    stack: &mut BTreeSet<String>,
    depth: usize,
    problems: &mut Vec<String>,
) -> Option<Unit> {
    if depth > MAX_DEPTH {
        problems.push("behavior nesting is too deep".to_owned());
        return None;
    }
    let script = Script::new(file.text()).ok()?;
    while start < end && script.is_punct(start, '(') {
        let close = script.close_of(start).ok()?;
        if close + 1 != end {
            break;
        }
        start += 1;
        end = close;
    }
    if start >= end {
        return None;
    }
    let first = script.token_text(start)?;
    if is_behavior_call(&script, start, end) {
        return behavior_unit(file, &script, start, end, alias);
    }
    if let Some(unit) = alias_behavior_call(file, &script, start, end, alias.clone()) {
        return Some(unit);
    }
    if first == "require" && script.is_punct(start + 1, '(') {
        let specifier = string_argument(&script, start + 1)?;
        let resolved = values::resolve_export(snapshot, &specifier, None, file, depth + 1)?;
        return resolve_resolved(snapshot, &resolved, alias, stack, depth + 1, problems);
    }
    if script.kind(start) == Some(TokenKind::Identifier) && start + 1 == end {
        // Imported binding first; `values::resolve_binding_in` also covers local
        // variables but not named imports with multiple entries.
        if let Some((module, export)) = values::import_for_name(&script, first) {
            let resolved =
                values::resolve_export(snapshot, &module, export.as_deref(), file, depth + 1)?;
            return resolve_resolved(
                snapshot,
                &resolved,
                Some(first.to_owned()),
                stack,
                depth + 1,
                problems,
            );
        }
        let resolved = values::resolve_binding_in(&script, snapshot, file, first, depth + 1)?;
        return resolve_resolved(snapshot, &resolved, alias, stack, depth + 1, problems);
    }
    problems
        .push("behavior entry is a dynamic expression (factory, conditional or call)".to_owned());
    None
}

fn resolve_resolved(
    snapshot: &Snapshot<'_>,
    resolved: &ExprRange,
    alias: Option<String>,
    stack: &mut BTreeSet<String>,
    depth: usize,
    problems: &mut Vec<String>,
) -> Option<Unit> {
    let target = snapshot.file(&resolved.path)?;
    let script = Script::new(target.text()).ok()?;
    let (start, end) = values::token_bounds(&script, resolved);
    resolve_expression(snapshot, target, start, end, alias, stack, depth, problems)
}

fn is_behavior_call(script: &Script<'_>, start: usize, end: usize) -> bool {
    script.is_ident(start, "Behavior")
        && script.is_punct(start + 1, '(')
        && start + 2 < end
        && script.is_punct(start + 2, '{')
}

fn alias_behavior_call(
    file: &SourceFile,
    script: &Script<'_>,
    start: usize,
    end: usize,
    alias: Option<String>,
) -> Option<Unit> {
    if script.kind(start) != Some(TokenKind::Identifier) || !script.is_punct(start + 1, '(') {
        return None;
    }
    let name = script.token_text(start)?;
    if !behavior_alias(file, name) {
        return None;
    }
    behavior_unit(file, script, start, end, alias)
}

fn behavior_alias(file: &SourceFile, name: &str) -> bool {
    let Ok(script) = Script::new(file.text()) else {
        return false;
    };
    for index in 0..script.tokens.len() {
        if script.is_ident(index, "const")
            && script.is_ident(index + 1, name)
            && script.is_punct(index + 2, '=')
            && script.is_ident(index + 3, "Behavior")
        {
            return true;
        }
    }
    false
}

fn behavior_unit(
    file: &SourceFile,
    script: &Script<'_>,
    start: usize,
    end: usize,
    alias: Option<String>,
) -> Option<Unit> {
    let open = start + 2;
    if !script.is_punct(open, '{') {
        return None;
    }
    let close = script.close_of(open).ok()?;
    // `Behavior({...})`, `Behavior({...},)` and `Behavior({...})` without the
    // closing call token (nested range). Any other argument shape, such as a
    // second argument or a spread, stays unresolved.
    let closes_call = close + 1 == end
        || (close + 2 == end && script.is_punct(close + 1, ')'))
        || (close + 3 == end && script.is_punct(close + 1, ',') && script.is_punct(close + 2, ')'));
    if !closes_call {
        return None;
    }
    Some(Unit {
        path: file.path().to_owned(),
        label: basename(file.path()),
        options_start: script.token_start(open)?,
        options_end: script.token_end(close)?,
        alias,
    })
}

fn string_argument(script: &Script<'_>, open: usize) -> Option<String> {
    let token = script.tokens.get(open + 1)?;
    if token.kind != TokenKind::StringLiteral {
        return None;
    }
    super::scanner::unquote(token.text(script.text))
}

fn collect_unit(
    snapshot: &Snapshot<'_>,
    unit: &Unit,
    units: &mut Vec<Unit>,
    stack: &mut BTreeSet<String>,
    depth: usize,
    problems: &mut Vec<String>,
) {
    if depth > MAX_DEPTH {
        problems.push(format!("behavior nesting is too deep at `{}`", unit.label));
        return;
    }
    if stack.contains(&unit.identity()) {
        problems.push(format!("behavior cycle at `{}`", unit.label));
        return;
    }
    let Some(file) = snapshot.file(&unit.path) else {
        problems.push(format!(
            "behavior `{}` is not part of the request snapshot",
            unit.label
        ));
        return;
    };
    let Ok(script) = Script::new(file.text()) else {
        problems.push(format!(
            "behavior `{}` could not be lexed from the request snapshot",
            unit.label
        ));
        return;
    };
    let Some(members) = unit_members(&script, unit) else {
        problems.push(format!(
            "behavior `{}` options could not be located in the request snapshot",
            unit.label
        ));
        return;
    };
    stack.insert(unit.identity());
    if let Some(nested) = member_last(&members, "behaviors") {
        if nested.kind != MemberKind::Property
            || nested.value_start >= nested.value_end
            || !script.is_punct(nested.value_start, '[')
        {
            problems.push(format!(
                "behavior `{}` has a non-static `behaviors`",
                unit.label
            ));
        } else if let Ok(close) = script.close_of(nested.value_start) {
            if close + 1 == nested.value_end {
                for (start, end) in values::array_elements(&script, nested.value_start, close) {
                    if script.is_punct(start, '.')
                        && script.is_punct(start + 1, '.')
                        && script.is_punct(start + 2, '.')
                    {
                        problems.push(format!(
                            "behavior imports `{}` has a spread array",
                            unit.label
                        ));
                        continue;
                    }
                    if let Some(child) = resolve_expression(
                        snapshot,
                        file,
                        start,
                        end,
                        None,
                        stack,
                        depth + 1,
                        problems,
                    ) {
                        collect_unit(snapshot, &child, units, stack, depth + 1, problems);
                    }
                }
            } else {
                problems.push(format!(
                    "behavior `{}` has a non-static `behaviors`",
                    unit.label
                ));
            }
        }
    }
    units.push(unit.clone());
    stack.remove(&unit.identity());
}

fn unit_members(script: &Script<'_>, unit: &Unit) -> Option<Vec<Member>> {
    let open = (0..script.tokens.len()).find(|index| {
        script.is_punct(*index, '{') && script.token_start(*index) == Some(unit.options_start)
    })?;
    let close = script.close_of(open).ok()?;
    if script.token_end(close) != Some(unit.options_end) {
        return None;
    }
    Some(scan_members(script, open + 1, close))
}

#[allow(clippy::too_many_lines)] // one precedence pass over data/properties/methods
fn merge_units(
    snapshot: &Snapshot<'_>,
    units: &[Unit],
    problems: &mut Vec<String>,
) -> BehaviorResolution {
    let mut data: Vec<BehaviorDataMember> = Vec::new();
    let mut properties: Vec<BehaviorProperty> = Vec::new();
    let mut methods: Vec<BehaviorMethod> = Vec::new();
    let mut data_incomplete = false;
    let mut properties_incomplete = false;
    let mut methods_incomplete = false;

    for unit in units.iter().rev() {
        let Some(file) = snapshot.file(&unit.path) else {
            continue;
        };
        let Ok(script) = Script::new(file.text()) else {
            continue;
        };
        let Some(members) = unit_members(&script, unit) else {
            continue;
        };
        if let Some(member) = member_last(&members, "data") {
            if let Some((open, close)) = object_bounds(&script, member) {
                let entries = scan_members(&script, open + 1, close);
                let mut expanded_here = BTreeSet::new();
                for entry in &entries {
                    if entry.kind == MemberKind::Spread {
                        let empty_object = script.is_punct(entry.value_start, '{')
                            && script
                                .close_of(entry.value_start)
                                .is_ok_and(|close| close == entry.value_start + 1);
                        let (expanded, incomplete) = expand_spread(
                            snapshot,
                            file,
                            &script,
                            entry,
                            &mut data,
                            &mut expanded_here,
                            problems,
                        );
                        if expanded == 0 && !empty_object {
                            problems
                                .push(format!("behavior `{}` has a spread in `data`", unit.label));
                            data_incomplete = true;
                        }
                        if incomplete {
                            data_incomplete = true;
                        }
                        continue;
                    }
                    if entry.kind != MemberKind::Property {
                        problems.push(format!(
                            "behavior `{}` has a shorthand/method data member",
                            unit.label
                        ));
                        data_incomplete = true;
                        continue;
                    }
                    let Some(name) = &entry.name else {
                        problems.push(format!(
                            "behavior `{}` has a computed data member",
                            unit.label
                        ));
                        data_incomplete = true;
                        continue;
                    };
                    if data.iter().any(|field| field.name == *name) && !expanded_here.contains(name)
                    {
                        continue;
                    }
                    if !crate::shadow::expression::is_binding_identifier(name) {
                        problems.push(format!(
                                "behavior data name `{name}` cannot be used as a template root; it is not checked"
                            ));
                        data_incomplete = true;
                        continue;
                    }
                    let inferred = infer_initializer(
                        &script,
                        snapshot,
                        file,
                        entry.value_start,
                        entry.value_end,
                        0,
                    );
                    if !inferred.exact {
                        problems.push(format!(
                                "behavior data member `{name}` is typed any: its initializer is not resolvable in the component scope"
                            ));
                        data_incomplete = true;
                    }
                    let range = name_range(&script, file, entry).ok();
                    let Some(range) = range else {
                        data_incomplete = true;
                        continue;
                    };
                    upsert_data(
                        &mut data,
                        BehaviorDataMember {
                            name: name.clone(),
                            type_text: inferred.text,
                            exact: inferred.exact,
                            documentation: doc_before(script.text, entry.key_start),
                            path: file.path().to_owned(),
                            range,
                        },
                    );
                }
            } else {
                problems.push(format!(
                    "behavior `{}` has a non-object-literal `data` member",
                    unit.label
                ));
                data_incomplete = true;
            }
        }

        if let Some(member) = member_last(&members, "properties") {
            if let Some((open, close)) = object_bounds(&script, member) {
                for entry in scan_members(&script, open + 1, close) {
                    if entry.kind != MemberKind::Property {
                        problems.push(format!(
                            "behavior `{}` has a non-literal `properties` member",
                            unit.label
                        ));
                        properties_incomplete = true;
                        continue;
                    }
                    let Some(name) = &entry.name else {
                        problems.push(format!(
                            "behavior `{}` has a computed/invalid property declaration",
                            unit.label
                        ));
                        properties_incomplete = true;
                        continue;
                    };
                    let Some((option_open, option_close)) = object_bounds(&script, &entry) else {
                        problems.push(format!(
                            "behavior `{}` has a non-literal property `{name}`",
                            unit.label
                        ));
                        properties_incomplete = true;
                        continue;
                    };
                    let options = scan_members(&script, option_open + 1, option_close);
                    let Some((type_text, union_complete)) = values::union_text(&script, &options)
                    else {
                        problems.push(format!("behavior property `{name}` has no readable `type`"));
                        properties_incomplete = true;
                        continue;
                    };
                    if !union_complete {
                        problems.push(format!(
                            "behavior property `{name}` has an unreadable `optionalTypes` entry"
                        ));
                        properties_incomplete = true;
                    }
                    if properties.iter().any(|property| property.name == *name) {
                        continue;
                    }
                    let Some(range) = name_range(&script, file, &entry).ok() else {
                        properties_incomplete = true;
                        continue;
                    };
                    let default_text = member_last(&options, "value")
                        .and_then(|value| value.value_range(&script))
                        .and_then(|range| range_text(file.text(), range))
                        .map(|text| text.trim().to_owned());
                    properties.push(BehaviorProperty {
                        name: name.clone(),
                        type_text,
                        type_complete: union_complete,
                        has_optional_types: member_last(&options, "optionalTypes").is_some(),
                        documentation: doc_before(script.text, entry.key_start),
                        default_text,
                        path: file.path().to_owned(),
                        range,
                    });
                }
            } else {
                problems.push(format!(
                    "behavior `{}` has a non-object-literal `properties` member",
                    unit.label
                ));
                properties_incomplete = true;
            }
        }

        if let Some(member) = member_last(&members, "methods") {
            if let Some((open, close)) = object_bounds(&script, member) {
                for entry in scan_members(&script, open + 1, close) {
                    if entry.kind == MemberKind::Spread {
                        problems.push(format!(
                            "behavior `{}` has a spread in `methods`",
                            unit.label
                        ));
                        methods_incomplete = true;
                        continue;
                    }
                    let Some(name) = &entry.name else {
                        problems.push(format!(
                            "behavior `{}` has a computed method name",
                            unit.label
                        ));
                        methods_incomplete = true;
                        continue;
                    };
                    if methods.iter().any(|method| method.name == *name) {
                        continue;
                    }
                    if !is_literal_function(&script, &entry) {
                        problems.push(format!(
                            "behavior `{}` method `{name}` is not a literal function",
                            unit.label
                        ));
                        methods_incomplete = true;
                        continue;
                    }
                    let Some(range) = name_range(&script, file, &entry).ok() else {
                        methods_incomplete = true;
                        continue;
                    };
                    methods.push(BehaviorMethod {
                        name: name.clone(),
                        signature: signature_of(&script, &entry),
                        documentation: doc_before(script.text, entry.key_start),
                        path: file.path().to_owned(),
                        range,
                    });
                }
            } else {
                problems.push(format!(
                    "behavior `{}` has a non-object-literal `methods` member",
                    unit.label
                ));
                methods_incomplete = true;
            }
        }
    }

    BehaviorResolution {
        status: CapabilityStatus::Supported,
        units: Vec::new(),
        data,
        properties,
        methods,
        data_incomplete,
        properties_incomplete,
        methods_incomplete,
        problems: problems.clone(),
    }
}

fn is_literal_function(script: &Script<'_>, member: &Member) -> bool {
    match member.kind {
        MemberKind::Method => true,
        MemberKind::Property => {
            if member.value_start >= member.value_end {
                return false;
            }
            script.is_punct(member.value_start, '(')
                || script.is_ident(member.value_start, "function")
                || (script.is_ident(member.value_start, "async")
                    && (script.is_punct(member.value_start + 1, '(')
                        || script.is_ident(member.value_start + 1, "function")))
                || (script.kind(member.value_start) == Some(TokenKind::Identifier)
                    && script.is_punct(member.value_start + 1, '='))
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)] // recursion carries the shared table state
fn expand_spread(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    script: &Script<'_>,
    member: &Member,
    data: &mut Vec<BehaviorDataMember>,
    expanded_here: &mut BTreeSet<String>,
    problems: &mut Vec<String>,
) -> (usize, bool) {
    let mut visited = BTreeSet::new();
    let mut fields = Vec::new();
    let mut incomplete = false;
    let expanded = expand_object(
        snapshot,
        file,
        script,
        member.value_start,
        member.value_end,
        0,
        &mut visited,
        &mut fields,
        &mut incomplete,
        problems,
    );
    for field in fields {
        if data.iter().any(|existing| existing.name == field.name)
            && !expanded_here.contains(&field.name)
        {
            continue;
        }
        upsert_data(data, field.clone());
        expanded_here.insert(field.name);
    }
    (expanded, incomplete)
}

#[allow(clippy::too_many_arguments)] // recursive object expansion with bounded visited set
#[allow(clippy::too_many_lines)] // one bounded recursive expansion
fn expand_object(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    script: &Script<'_>,
    mut start: usize,
    mut end: usize,
    depth: usize,
    visited: &mut BTreeSet<String>,
    out: &mut Vec<BehaviorDataMember>,
    incomplete: &mut bool,
    problems: &mut Vec<String>,
) -> usize {
    if depth > MAX_DEPTH {
        *incomplete = true;
        return 0;
    }
    while start < end && script.is_punct(start, '(') {
        let Ok(close) = script.close_of(start) else {
            break;
        };
        if close + 1 != end {
            break;
        }
        start += 1;
        end = close;
    }
    let Some(first) = script.kind(start) else {
        return 0;
    };
    if first == TokenKind::Punctuator('{') {
        let Ok(close) = script.close_of(start) else {
            *incomplete = true;
            return 0;
        };
        if close + 1 != end {
            *incomplete = true;
            return 0;
        }
        let mut count = 0_usize;
        for entry in scan_members(script, start + 1, close) {
            if entry.kind == MemberKind::Spread {
                count += expand_object(
                    snapshot,
                    file,
                    script,
                    entry.value_start,
                    entry.value_end,
                    depth + 1,
                    visited,
                    out,
                    incomplete,
                    problems,
                );
                continue;
            }
            if entry.kind != MemberKind::Property {
                *incomplete = true;
                continue;
            }
            let Some(name) = &entry.name else {
                *incomplete = true;
                continue;
            };
            if !crate::shadow::expression::is_binding_identifier(name) {
                *incomplete = true;
                continue;
            }
            let inferred = infer_initializer(
                script,
                snapshot,
                file,
                entry.value_start,
                entry.value_end,
                depth + 1,
            );
            if !inferred.exact {
                *incomplete = true;
            }
            let Ok(range) = name_range(script, file, &entry) else {
                *incomplete = true;
                continue;
            };
            out.push(BehaviorDataMember {
                name: name.clone(),
                type_text: inferred.text,
                exact: inferred.exact,
                documentation: doc_before(script.text, entry.key_start),
                path: file.path().to_owned(),
                range,
            });
            count += 1;
        }
        return count;
    }
    if first != TokenKind::Identifier || start + 1 != end {
        problems.push("behavior data spread is not a static object".to_owned());
        *incomplete = true;
        return 0;
    }
    let Some(name) = script.token_text(start) else {
        return 0;
    };
    let resolved = values::resolve_binding_in(script, snapshot, file, name, depth + 1);
    let Some(resolved) = resolved else {
        problems.push("behavior data spread is not a static object".to_owned());
        *incomplete = true;
        return 0;
    };
    let identity = format!("{}:{}", resolved.path, resolved.start);
    if !visited.insert(identity) {
        problems.push("behavior data spread has a cycle".to_owned());
        *incomplete = true;
        return 0;
    }
    let Some(target) = snapshot.file(&resolved.path) else {
        return 0;
    };
    let Ok(target_script) = Script::new(target.text()) else {
        return 0;
    };
    let (start, end) = values::token_bounds(&target_script, &resolved);
    expand_object(
        snapshot,
        target,
        &target_script,
        start,
        end,
        depth + 1,
        visited,
        out,
        incomplete,
        problems,
    )
}

fn upsert_data(data: &mut Vec<BehaviorDataMember>, field: BehaviorDataMember) {
    if let Some(existing) = data.iter_mut().find(|existing| existing.name == field.name) {
        *existing = field;
    } else {
        data.push(field);
    }
}

fn name_range(
    script: &Script<'_>,
    _file: &SourceFile,
    member: &Member,
) -> Result<ScriptRange, super::ScriptError> {
    script.source_range(member.key_range())
}

fn range_text(text: &str, range: SourceRange) -> Option<String> {
    text.get(range.start.get() as usize..range.end.get() as usize)
        .map(str::to_owned)
}

/// Expands a statically readable object expression into data fields.
///
/// Used by the own component `data` overlay; behavior `data` spreads use the
/// same expansion internally. A partially readable expression returns the
/// fields it could prove plus `incomplete = true`.
pub(crate) fn expand_static_fields(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    script: &Script<'_>,
    start: usize,
    end: usize,
) -> (Vec<BehaviorDataMember>, bool, Vec<String>) {
    let mut visited = BTreeSet::new();
    let mut fields = Vec::new();
    let mut incomplete = false;
    let mut problems = Vec::new();
    expand_object(
        snapshot,
        file,
        script,
        start,
        end,
        0,
        &mut visited,
        &mut fields,
        &mut incomplete,
        &mut problems,
    );
    (fields, incomplete, problems)
}

fn basename(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(path, |(_, name)| name)
        .to_owned()
}
