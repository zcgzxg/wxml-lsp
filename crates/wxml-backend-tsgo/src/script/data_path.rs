//! Nested `data`/`properties` path resolution.
//!
//! The resolver walks object literals and statically readable `const`/imported
//! objects. It never invents a field: a dynamic member, a non-object parent, a
//! duplicate key or a cross-file declaration makes the answer partial. Token
//! ranges come from the original file, so quoted keys exclude their quotes and
//! CJK/emoji keys keep exact UTF-8 and UTF-16 offsets.

use std::collections::BTreeSet;

use wxml_types::{MAX_SYMBOL_ENTRIES, MAX_SYMBOL_NAME_BYTES};

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::ScriptAnalysis;
use super::registration::{ParsedRegistration, RegistrationKind};
use super::scanner::{MemberKind, Script, member_last, object_bounds, scan_members};
use super::snapshot::Snapshot;
use super::symbol_facts::{SymbolFieldFact, SymbolTokenFact, key_token, push_reason};
use super::values;

/// Maximum object nesting followed for one path.
const MAX_DEPTH: usize = 16;

/// One resolved object literal in a project file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectContext {
    path: String,
    open: usize,
    close: usize,
}

/// One statically readable member of an object literal.
#[derive(Debug, Clone)]
struct FieldEntry {
    name: String,
    token: SymbolTokenFact,
    quoted: bool,
    value_path: String,
    value_start: usize,
    value_end: usize,
}

/// One object's member set plus its proof state.
#[derive(Debug)]
struct FieldSet {
    entries: Vec<FieldEntry>,
    duplicates: BTreeSet<String>,
    complete: bool,
    problems: Vec<String>,
}

impl FieldSet {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            duplicates: BTreeSet::new(),
            complete: true,
            problems: Vec::new(),
        }
    }

    fn upsert(&mut self, entry: FieldEntry) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|existing| existing.name == entry.name)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    fn get(&self, name: &str) -> Option<&FieldEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    fn unique_fields(&mut self) -> Vec<SymbolFieldFact> {
        let mut fields = Vec::new();
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            if entry.name.is_empty() || entry.name.len() > MAX_SYMBOL_NAME_BYTES {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "a data member name is empty or too long",
                );
                continue;
            }
            if self.duplicates.contains(&entry.name) || !seen.insert(entry.name.clone()) {
                self.complete = false;
                continue;
            }
            fields.push(SymbolFieldFact {
                name: entry.name.clone(),
                declaration: Some(entry.token.clone()),
                quoted: entry.quoted,
            });
        }
        if fields.len() > MAX_SYMBOL_ENTRIES {
            fields.truncate(MAX_SYMBOL_ENTRIES);
            self.complete = false;
            push_reason(&mut self.problems, "the data field list was truncated");
        }
        fields
    }
}

/// Result of resolving one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataPathResolution {
    /// Leaf declaration.
    pub(crate) declaration: Option<SymbolTokenFact>,
    /// Sibling fields of the resolved object.
    pub(crate) fields: Vec<SymbolFieldFact>,
    /// False when any shape was dynamic, duplicated or cross-file.
    pub(crate) complete: bool,
    /// Degradation reasons.
    pub(crate) problems: Vec<String>,
}

impl DataPathResolution {
    fn partial(reason: &str) -> Self {
        Self {
            declaration: None,
            fields: Vec::new(),
            complete: false,
            problems: vec![reason.to_owned()],
        }
    }
}

/// Resolves one data path against the selected registration.
#[allow(clippy::too_many_lines)] // one root/segment resolution matrix
pub(crate) fn resolve(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    analysis: &ScriptAnalysis,
    registration: &ParsedRegistration,
    path: &[String],
) -> DataPathResolution {
    let Ok(script) = Script::new(file.text()) else {
        return DataPathResolution::partial("the script could not be scanned");
    };
    let mut problems = Vec::new();
    let mut complete = true;

    let data_set = data_members(snapshot, file, registration, &mut complete, &mut problems);
    let properties_set = property_members(
        snapshot,
        file,
        &script,
        registration,
        &mut complete,
        &mut problems,
    );
    let behavior_data = behavior_data_fields(snapshot, analysis, &mut problems);
    let behavior_properties = behavior_property_fields(snapshot, analysis, &mut problems);
    if !behavior_data.is_empty() || !behavior_properties.is_empty() {
        complete = false;
        push_reason(
            &mut problems,
            "behavior data/properties are not resolvable for nested paths",
        );
    }

    let data_hit = data_set
        .as_ref()
        .and_then(|set| set.get(&path[0]))
        .map(|entry| entry.token.clone());
    let property_hit = properties_set
        .as_ref()
        .and_then(|set| set.get(&path[0]))
        .map(|entry| entry.token.clone());
    if data_hit.is_some() && property_hit.is_some() {
        push_reason(
            &mut problems,
            "a data member is declared by both `data` and `properties`",
        );
        return DataPathResolution {
            declaration: None,
            fields: Vec::new(),
            complete: false,
            problems,
        };
    }
    if data_hit.is_some() {
        let Some(set) = data_set else {
            return DataPathResolution::partial("the data root disappeared during resolution");
        };
        return walk_path(snapshot, set, path, complete, problems);
    }
    if property_hit.is_some() {
        let Some(mut set) = properties_set else {
            return DataPathResolution::partial("the property root disappeared during resolution");
        };
        let fields = set.unique_fields();
        complete &= set.complete;
        problems.extend(set.problems);
        if path.len() > 1 {
            complete = false;
            push_reason(
                &mut problems,
                "component property options are not data fields",
            );
            return DataPathResolution {
                declaration: None,
                fields,
                complete,
                problems,
            };
        }
        return DataPathResolution {
            declaration: property_hit,
            fields,
            complete,
            problems,
        };
    }
    if data_hit.is_none() && property_hit.is_none() {
        let behavior_hit = behavior_data
            .iter()
            .find(|entry| entry.name == path[0])
            .and_then(|entry| entry.declaration.clone())
            .or_else(|| {
                behavior_properties
                    .iter()
                    .find(|entry| entry.name == path[0])
                    .and_then(|entry| entry.declaration.clone())
            });
        if let Some(declaration) = behavior_hit {
            let mut fields = behavior_data;
            fields.extend(behavior_properties);
            let mut seen = BTreeSet::new();
            fields.retain(|field| seen.insert(field.name.clone()));
            if fields.len() > MAX_SYMBOL_ENTRIES {
                fields.truncate(MAX_SYMBOL_ENTRIES);
            }
            if path.len() > 1 {
                push_reason(
                    &mut problems,
                    "a behavior data declaration cannot prove nested fields",
                );
                return DataPathResolution {
                    declaration: None,
                    fields,
                    complete: false,
                    problems,
                };
            }
            return DataPathResolution {
                declaration: Some(declaration),
                fields,
                complete: false,
                problems,
            };
        }
    }

    // The root name is absent. Return sibling fields so a typo quick fix has
    // candidates, but do not invent a declaration.
    if let Some(mut set) = data_set {
        let fields = set.unique_fields();
        complete &= set.complete;
        problems.extend(set.problems);
        return DataPathResolution {
            declaration: None,
            fields,
            complete,
            problems,
        };
    }
    if let Some(mut set) = properties_set {
        let fields = set.unique_fields();
        complete &= set.complete;
        problems.extend(set.problems);
        if path.len() > 1 {
            complete = false;
        }
        return DataPathResolution {
            declaration: None,
            fields,
            complete,
            problems,
        };
    }
    if !behavior_data.is_empty() || !behavior_properties.is_empty() {
        let mut fields = behavior_data;
        fields.extend(behavior_properties);
        return DataPathResolution {
            declaration: None,
            fields,
            complete: false,
            problems,
        };
    }

    push_reason(&mut problems, "the registration has no data root");
    DataPathResolution {
        declaration: None,
        fields: Vec::new(),
        complete: false,
        problems,
    }
}

fn data_members(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    registration: &ParsedRegistration,
    complete: &mut bool,
    problems: &mut Vec<String>,
) -> Option<FieldSet> {
    let member = member_last(&registration.members, "data")?;
    if member.kind != MemberKind::Property {
        *complete = false;
        push_reason(problems, "`data` is not a property assignment");
        return None;
    }
    let Some(context) = resolve_value_context(
        snapshot,
        file.path(),
        member.value_start,
        member.value_end,
        0,
        problems,
    ) else {
        *complete = false;
        push_reason(
            problems,
            "`data` is not a statically resolvable object literal",
        );
        return None;
    };
    let set = collect_fields(snapshot, &context, 0, problems);
    *complete &= set.complete;
    if context.path != file.path() {
        *complete = false;
    }
    Some(set)
}

fn property_members(
    snapshot: &Snapshot<'_>,
    file: &SourceFile,
    script: &Script<'_>,
    registration: &ParsedRegistration,
    complete: &mut bool,
    problems: &mut Vec<String>,
) -> Option<FieldSet> {
    if registration.fact.kind != RegistrationKind::Component {
        return None;
    }
    let member = member_last(&registration.members, "properties")?;
    if member.kind != MemberKind::Property {
        *complete = false;
        push_reason(problems, "`properties` is not a property assignment");
        return None;
    }
    let Some((open, close)) = object_bounds(script, member) else {
        *complete = false;
        push_reason(problems, "`properties` is not an object literal");
        return None;
    };
    let context = ObjectContext {
        path: file.path().to_owned(),
        open,
        close,
    };
    let set = collect_fields(snapshot, &context, 0, problems);
    *complete &= set.complete;
    Some(set)
}

fn walk_path(
    snapshot: &Snapshot<'_>,
    mut set: FieldSet,
    path: &[String],
    mut complete: bool,
    mut problems: Vec<String>,
) -> DataPathResolution {
    for index in 0..path.len() {
        if set.duplicates.contains(&path[index]) {
            push_reason(&mut problems, "a data member is declared more than once");
            let fields = set.unique_fields();
            return DataPathResolution {
                declaration: None,
                fields,
                complete: false,
                problems,
            };
        }
        let Some(entry) = set.get(&path[index]).cloned() else {
            let fields = set.unique_fields();
            complete &= set.complete;
            problems.extend(set.problems);
            return DataPathResolution {
                declaration: None,
                fields,
                complete,
                problems,
            };
        };
        if index + 1 == path.len() {
            let fields = set.unique_fields();
            complete &= set.complete;
            problems.extend(set.problems);
            return DataPathResolution {
                declaration: Some(entry.token),
                fields,
                complete,
                problems,
            };
        }
        let Some(next) = resolve_value_context(
            snapshot,
            &entry.value_path,
            entry.value_start,
            entry.value_end,
            0,
            &mut problems,
        ) else {
            push_reason(
                &mut problems,
                "a data member is not a statically resolvable object",
            );
            let fields = set.unique_fields();
            return DataPathResolution {
                declaration: None,
                fields,
                complete: false,
                problems,
            };
        };
        if next.path != entry.value_path {
            complete = false;
            push_reason(
                &mut problems,
                "a nested data object is declared in another script",
            );
        }
        let child = collect_fields(snapshot, &next, 0, &mut problems);
        complete &= child.complete;
        set = child;
    }
    let fields = set.unique_fields();
    complete &= set.complete;
    problems.extend(set.problems);
    DataPathResolution {
        declaration: None,
        fields,
        complete,
        problems,
    }
}

fn collect_fields(
    snapshot: &Snapshot<'_>,
    context: &ObjectContext,
    depth: usize,
    problems: &mut Vec<String>,
) -> FieldSet {
    let mut set = FieldSet::new();
    if depth > MAX_DEPTH {
        set.complete = false;
        push_reason(problems, "data object nesting is too deep");
        return set;
    }
    let Some(file) = snapshot.file(&context.path) else {
        set.complete = false;
        push_reason(problems, "a data object is outside the request snapshot");
        return set;
    };
    let Ok(script) = Script::new(file.text()) else {
        set.complete = false;
        push_reason(problems, "a data object could not be scanned");
        return set;
    };
    let mut explicit = BTreeSet::new();
    for member in scan_members(&script, context.open + 1, context.close) {
        match member.kind {
            MemberKind::Spread => {
                let Some(child) = resolve_value_context(
                    snapshot,
                    &context.path,
                    member.value_start,
                    member.value_end,
                    depth + 1,
                    problems,
                ) else {
                    set.complete = false;
                    push_reason(problems, "a data spread is not statically resolvable");
                    continue;
                };
                if child.path != context.path {
                    set.complete = false;
                    push_reason(problems, "a data spread is declared in another script");
                }
                let child_set = collect_fields(snapshot, &child, depth + 1, problems);
                set.complete &= child_set.complete;
                set.problems.extend(child_set.problems);
                for entry in child_set.entries {
                    set.upsert(entry);
                }
                set.duplicates.extend(child_set.duplicates);
            }
            MemberKind::Property => {
                let Some(name) = &member.name else {
                    set.complete = false;
                    push_reason(problems, "a data member has no static name");
                    continue;
                };
                if name.is_empty() || name.len() > MAX_SYMBOL_NAME_BYTES {
                    set.complete = false;
                    push_reason(problems, "a data member name is empty or too long");
                    continue;
                }
                if !explicit.insert(name.clone()) {
                    set.duplicates.insert(name.clone());
                    set.complete = false;
                    push_reason(problems, "a data member is declared more than once");
                }
                let Some((token, quoted, escaped)) =
                    key_token(&script, &context.path, member.key_range())
                else {
                    set.complete = false;
                    push_reason(problems, "a data member key could not be mapped");
                    continue;
                };
                if escaped {
                    set.complete = false;
                    push_reason(problems, "a quoted data key contains escape sequences");
                }
                set.upsert(FieldEntry {
                    name: name.clone(),
                    token,
                    quoted,
                    value_path: context.path.clone(),
                    value_start: member.value_start,
                    value_end: member.value_end,
                });
            }
            MemberKind::Computed
            | MemberKind::Shorthand
            | MemberKind::Method
            | MemberKind::Accessor
            | MemberKind::Unknown => {
                set.complete = false;
                push_reason(problems, "a data object contains a dynamic member");
            }
        }
    }
    if set.entries.len() > MAX_SYMBOL_ENTRIES {
        set.entries.truncate(MAX_SYMBOL_ENTRIES);
        set.complete = false;
        push_reason(problems, "the data field list was truncated");
    }
    set
}

fn resolve_value_context(
    snapshot: &Snapshot<'_>,
    path: &str,
    mut start: usize,
    mut end: usize,
    depth: usize,
    problems: &mut Vec<String>,
) -> Option<ObjectContext> {
    if depth > MAX_DEPTH {
        push_reason(problems, "data value nesting is too deep");
        return None;
    }
    let file = snapshot.file(path)?;
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
    if script.is_punct(start, '{') {
        let close = script.close_of(start).ok()?;
        if close + 1 != end {
            return None;
        }
        return Some(ObjectContext {
            path: path.to_owned(),
            open: start,
            close,
        });
    }
    if start + 1 != end || script.kind(start) != Some(TokenKind::Identifier) {
        return None;
    }
    let name = script.token_text(start)?;
    let resolved = values::resolve_binding_in(&script, snapshot, file, name, depth + 1)?;
    let target = snapshot.file(&resolved.path)?;
    let target_script = Script::new(target.text()).ok()?;
    let (start, end) = values::token_bounds(&target_script, &resolved);
    resolve_value_context(snapshot, &resolved.path, start, end, depth + 1, problems)
}

fn behavior_data_fields(
    snapshot: &Snapshot<'_>,
    analysis: &ScriptAnalysis,
    problems: &mut Vec<String>,
) -> Vec<SymbolFieldFact> {
    let mut fields = Vec::new();
    for field in &analysis.behavior.data {
        let Some(source) = snapshot.file(&field.path) else {
            push_reason(problems, "a behavior data file is unavailable");
            continue;
        };
        let Ok(script) = Script::new(source.text()) else {
            push_reason(problems, "a behavior data file could not be scanned");
            continue;
        };
        let range = SourceRange::new(field.range.utf8.start.get(), field.range.utf8.end.get());
        let Some((token, quoted, escaped)) = key_token(&script, &field.path, range) else {
            push_reason(problems, "a behavior data key could not be mapped");
            continue;
        };
        if escaped {
            push_reason(problems, "a behavior data key contains escape sequences");
        }
        fields.push(SymbolFieldFact {
            name: field.name.clone(),
            declaration: Some(token),
            quoted,
        });
    }
    fields
}

fn behavior_property_fields(
    snapshot: &Snapshot<'_>,
    analysis: &ScriptAnalysis,
    problems: &mut Vec<String>,
) -> Vec<SymbolFieldFact> {
    let mut fields = Vec::new();
    for property in &analysis.behavior.properties {
        let Some(source) = snapshot.file(&property.path) else {
            push_reason(problems, "a behavior property file is unavailable");
            continue;
        };
        let Ok(script) = Script::new(source.text()) else {
            push_reason(problems, "a behavior property file could not be scanned");
            continue;
        };
        let range = SourceRange::new(
            property.range.utf8.start.get(),
            property.range.utf8.end.get(),
        );
        let Some((token, quoted, escaped)) = key_token(&script, &property.path, range) else {
            push_reason(problems, "a behavior property key could not be mapped");
            continue;
        };
        if escaped {
            push_reason(
                problems,
                "a behavior property key contains escape sequences",
            );
        }
        fields.push(SymbolFieldFact {
            name: property.name.clone(),
            declaration: Some(token),
            quoted,
        });
    }
    fields
}
