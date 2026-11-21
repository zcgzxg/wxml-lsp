//! Symbol query orchestration over the reusable fact DTOs.

use std::collections::BTreeSet;

use wxml_types::{CapabilityStatus, MAX_SYMBOL_ENTRIES, SymbolQuery};

use crate::input::{AnalysisInput, SourceFile};
use crate::shadow::expression;
use crate::shadow::mapping::SourceRange;

use super::events::EventMethod;
use super::registration::{self, ParsedRegistration, RegistrationKind};
use super::scanner::{Script, scan_members};
use super::snapshot::Snapshot;
use super::symbol_facts::{
    SymbolFacts, SymbolFieldFact, SymbolTokenFact, finish, key_token, push_reason, range_text,
    validate_data_path,
};
use super::symbols::ScriptRange;
use super::{ScriptAnalysis, data_path, insertion, occurrences, references};

/// Answers one symbol query from existing [`ScriptAnalysis`] facts.
///
/// The analysis must belong to `analysis.path`; the input supplies sibling
/// script files for conservative cross-file escape checks. This function never
/// opens a session and never writes to disk.
#[must_use]
pub fn query_symbol_facts(
    analysis: &ScriptAnalysis,
    input: &AnalysisInput,
    query: &SymbolQuery,
) -> SymbolFacts {
    let Some(fact) = analysis.registration.selected.clone() else {
        return SymbolFacts::unsupported(registration_gap(analysis));
    };
    let Some(file) = input.file(&analysis.path) else {
        return SymbolFacts::unsupported("the analyzed script is not part of the request snapshot");
    };
    let Ok(script) = Script::new(file.text()) else {
        return partial_only("the subject script could not be scanned");
    };
    let Ok(open) = registration::find_options_open(&script, &fact) else {
        return partial_only("the registration options could not be located");
    };
    let Ok(close) = script.close_of(open) else {
        return partial_only("the registration options are not balanced");
    };
    let registration = ParsedRegistration {
        fact,
        members: scan_members(&script, open + 1, close),
    };
    match query {
        SymbolQuery::DataRoots => data_roots(analysis, input, file, &registration),
        SymbolQuery::DataPath { path } => {
            data_path_facts(analysis, input, file, &script, &registration, path)
        }
        SymbolQuery::EventHandler { name } => {
            event_handler_facts(analysis, input, file, &script, &registration, name)
        }
        _ => SymbolFacts::unsupported("unsupported script symbol query"),
    }
}

fn registration_gap(analysis: &ScriptAnalysis) -> String {
    analysis.reasons.first().cloned().unwrap_or_else(|| {
        "no local Page/Component/Behavior registration with an object literal".to_owned()
    })
}

fn partial_only(reason: &str) -> SymbolFacts {
    SymbolFacts {
        status: CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
        complete: false,
        reason: Some(reason.to_owned()),
        declaration: None,
        references: Vec::new(),
        occurrences: Vec::new(),
        insertion: None,
        fields: Vec::new(),
        reserved: Vec::new(),
    }
}

fn data_roots(
    analysis: &ScriptAnalysis,
    input: &AnalysisInput,
    file: &SourceFile,
    registration: &ParsedRegistration,
) -> SymbolFacts {
    let component = registration.fact.kind == RegistrationKind::Component;
    let mut complete = analysis.registration.complete
        && analysis.data.status.is_supported()
        && analysis.data.complete
        && analysis.methods.status.is_supported()
        && analysis
            .data
            .fields
            .iter()
            .all(|field| field.path == file.path());
    let mut problems = Vec::new();
    if component {
        complete &= analysis.properties.status.is_supported()
            && analysis.properties.complete
            && analysis
                .properties
                .properties
                .iter()
                .all(|property| property.path == file.path());
        if !analysis.properties.status.is_supported() {
            push_reason(&mut problems, "the property overlay is partial");
        }
    }
    if !analysis.registration.complete {
        push_reason(
            &mut problems,
            "the registration is ambiguous or not top-level",
        );
    }
    if !analysis.data.status.is_supported() {
        push_reason(&mut problems, "the data overlay is partial");
    }
    if !analysis.methods.status.is_supported() {
        push_reason(&mut problems, "the handler owner table is partial");
    }

    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    for field in &analysis.data.fields {
        if append_field(
            &mut fields,
            &mut seen,
            input,
            file,
            &field.name,
            field.range,
            &field.path,
            &mut problems,
        ) {
            continue;
        }
        complete = false;
    }
    if component {
        for property in &analysis.properties.properties {
            if append_field(
                &mut fields,
                &mut seen,
                input,
                file,
                &property.name,
                property.range,
                &property.path,
                &mut problems,
            ) {
                continue;
            }
            complete = false;
        }
    }
    if fields.len() > MAX_SYMBOL_ENTRIES {
        fields.truncate(MAX_SYMBOL_ENTRIES);
        complete = false;
        push_reason(&mut problems, "the field list was truncated");
    }

    let (reserved, reserved_complete) = owner_reserved(analysis, file, registration);
    complete &= reserved_complete;
    finish(
        complete,
        problems,
        SymbolFacts {
            status: CapabilityStatus::Supported,
            complete,
            reason: None,
            declaration: None,
            references: Vec::new(),
            occurrences: Vec::new(),
            insertion: None,
            fields,
            reserved,
        },
    )
}

#[allow(clippy::too_many_arguments)] // one overlay field conversion
fn append_field(
    fields: &mut Vec<SymbolFieldFact>,
    seen: &mut BTreeSet<String>,
    input: &AnalysisInput,
    subject: &SourceFile,
    name: &str,
    range: ScriptRange,
    path: &str,
    problems: &mut Vec<String>,
) -> bool {
    if name.is_empty() {
        push_reason(problems, "a data member name is empty");
        return false;
    }
    if !seen.insert(name.to_owned()) {
        push_reason(problems, "a data member is declared more than once");
        return false;
    }
    let Some(source) = input.file(path) else {
        push_reason(
            problems,
            "a data member is declared in an unavailable script",
        );
        fields.push(SymbolFieldFact {
            name: name.to_owned(),
            declaration: None,
            quoted: false,
        });
        return false;
    };
    let Ok(script) = Script::new(source.text()) else {
        push_reason(problems, "a declaring script could not be scanned");
        fields.push(SymbolFieldFact {
            name: name.to_owned(),
            declaration: None,
            quoted: false,
        });
        return false;
    };
    let Some((token, quoted, escaped)) = key_token(
        &script,
        path,
        SourceRange::new(range.utf8.start.get(), range.utf8.end.get()),
    ) else {
        push_reason(problems, "a data member token could not be mapped");
        fields.push(SymbolFieldFact {
            name: name.to_owned(),
            declaration: None,
            quoted: false,
        });
        return false;
    };
    if escaped {
        push_reason(problems, "a quoted data key contains escape sequences");
    }
    fields.push(SymbolFieldFact {
        name: name.to_owned(),
        declaration: Some(token),
        quoted,
    });
    path == subject.path() && !escaped
}

fn data_path_facts(
    analysis: &ScriptAnalysis,
    input: &AnalysisInput,
    file: &SourceFile,
    script: &Script<'_>,
    registration: &ParsedRegistration,
    path: &[String],
) -> SymbolFacts {
    if let Err(reason) = validate_data_path(path) {
        return SymbolFacts::unsupported(reason);
    }
    let snapshot = Snapshot::new(input, None);
    let mut resolution = data_path::resolve(&snapshot, file, analysis, registration, path);
    let mut complete = analysis.registration.complete
        && analysis.data.status.is_supported()
        && analysis.data.complete
        && analysis.methods.status.is_supported()
        && resolution.complete;
    let mut problems = std::mem::take(&mut resolution.problems);
    if !analysis.registration.complete {
        push_reason(
            &mut problems,
            "the registration is ambiguous or not top-level",
        );
    }
    if !analysis.data.complete {
        push_reason(&mut problems, "the data overlay is incomplete");
    }
    if !analysis.methods.status.is_supported() {
        push_reason(&mut problems, "the handler owner table is partial");
    }
    if registration.fact.kind == RegistrationKind::Component {
        complete &= analysis.properties.status.is_supported() && analysis.properties.complete;
        if !analysis.properties.status.is_supported() {
            push_reason(&mut problems, "the property overlay is partial");
        }
    }

    let reference_scan = resolution.declaration.as_ref().map_or_else(
        || references::ReferenceScan {
            references: Vec::new(),
            complete: true,
            problems: Vec::new(),
        },
        |declaration| references::scan_data_references(file, registration, path, Some(declaration)),
    );
    complete &= reference_scan.complete;
    problems.extend(reference_scan.problems);

    let occurrence_scan = occurrences::scan_set_data(file, path);
    complete &= occurrence_scan.complete;
    problems.extend(occurrence_scan.problems);
    if let Some(reason) = occurrences::other_script_conflict(input, file.path(), path) {
        complete = false;
        push_reason(&mut problems, &reason);
    }
    if let Some(reason) = occurrences::observer_escape(script, registration) {
        complete = false;
        push_reason(&mut problems, &reason);
    }
    if let Some(declaration) = &resolution.declaration
        && declaration.path != file.path()
    {
        complete = false;
        push_reason(
            &mut problems,
            "shared data declarations are not complete for rename",
        );
    }

    let (reserved, reserved_complete) = owner_reserved(analysis, file, registration);
    complete &= reserved_complete;
    finish(
        complete,
        problems,
        SymbolFacts {
            status: CapabilityStatus::Supported,
            complete,
            reason: None,
            declaration: resolution.declaration,
            references: reference_scan.references,
            occurrences: occurrence_scan.occurrences,
            insertion: None,
            fields: resolution.fields,
            reserved,
        },
    )
}

#[allow(clippy::too_many_lines)] // one handler-state matrix
fn event_handler_facts(
    analysis: &ScriptAnalysis,
    input: &AnalysisInput,
    file: &SourceFile,
    script: &Script<'_>,
    registration: &ParsedRegistration,
    name: &str,
) -> SymbolFacts {
    if !expression::is_binding_identifier(name) {
        return SymbolFacts::unsupported("the event handler name is not a static identifier");
    }
    let mut problems = Vec::new();
    let mut complete = analysis.registration.complete
        && analysis.data.status.is_supported()
        && analysis.data.complete;
    if !analysis.registration.complete {
        push_reason(
            &mut problems,
            "the registration is ambiguous or not top-level",
        );
    }
    if !analysis.data.status.is_supported() || !analysis.data.complete {
        push_reason(&mut problems, "the data overlay is partial");
    }
    let taken = taken_names(&analysis.methods);
    let mut fields: Vec<SymbolFieldFact> = taken
        .iter()
        .map(|name| SymbolFieldFact {
            name: name.clone(),
            declaration: None,
            quoted: false,
        })
        .collect();
    let reserved = taken;
    if analysis
        .methods
        .methods
        .iter()
        .any(|method| method.path != file.path())
    {
        complete = false;
        push_reason(
            &mut problems,
            "a handler is declared in another script; cross-file references are not proven",
        );
    }

    if !analysis.methods.status.is_supported() {
        complete = false;
        push_reason(&mut problems, "the handler owner table is partial");
    }
    let declaration = analysis
        .methods
        .methods
        .iter()
        .find(|method| method.name == name)
        .and_then(|method| method_token(input, method));
    let mut references = Vec::new();
    let mut insertion = None;
    if let Some(declaration) = &declaration {
        let scan = references::scan_handler_references(file, registration, name, declaration);
        complete &= scan.complete;
        problems.extend(scan.problems);
        references = scan.references;
    } else if analysis.methods.opaque.iter().any(|opaque| opaque == name) {
        complete = false;
        push_reason(
            &mut problems,
            "the handler exists with a non-literal value and is not provably missing",
        );
    } else if !analysis.methods.status.is_supported() {
        complete = false;
        push_reason(
            &mut problems,
            "the handler owner table is partial; a missing handler is not proven",
        );
    } else if complete && analysis.registration.complete {
        match insertion::build(file, script, registration, name) {
            Ok(built) => insertion = Some(built),
            Err(reason) => {
                complete = false;
                push_reason(&mut problems, &reason);
            }
        }
    } else {
        complete = false;
    }

    if fields
        .iter()
        .any(|field| !expression::is_binding_identifier(&field.name))
    {
        complete = false;
        push_reason(
            &mut problems,
            "the owner table contains a non-identifier name",
        );
        fields.retain(|field| expression::is_binding_identifier(&field.name));
    }
    fields.truncate(MAX_SYMBOL_ENTRIES);
    finish(
        complete,
        problems,
        SymbolFacts {
            status: CapabilityStatus::Supported,
            complete,
            reason: None,
            declaration,
            references,
            occurrences: Vec::new(),
            insertion,
            fields,
            reserved,
        },
    )
}

fn method_token(input: &AnalysisInput, method: &EventMethod) -> Option<SymbolTokenFact> {
    let file = input.file(&method.path)?;
    let script = Script::new(file.text()).ok()?;
    let range = SourceRange::new(method.range.utf8.start.get(), method.range.utf8.end.get());
    let text = range_text(script.text, range)?;
    let (range, expected) = if text.starts_with('"') || text.starts_with('\'') {
        let quote = text.chars().next()?;
        if text.len() < 2 || !text.ends_with(quote) {
            return None;
        }
        let content = SourceRange::new(range.start.get() + 1, range.end.get() - 1);
        (content, range_text(script.text, content)?)
    } else {
        (range, text)
    };
    Some(SymbolTokenFact {
        path: method.path.clone(),
        range: script.source_range(range).ok()?,
        expected_text: Some(expected.to_owned()),
    })
}

fn taken_names(methods: &super::events::MethodTable) -> Vec<String> {
    let mut names = BTreeSet::new();
    for method in &methods.methods {
        names.insert(method.name.clone());
    }
    for opaque in &methods.opaque {
        names.insert(opaque.clone());
    }
    names.into_iter().collect()
}

fn owner_reserved(
    analysis: &ScriptAnalysis,
    file: &SourceFile,
    registration: &ParsedRegistration,
) -> (Vec<String>, bool) {
    let mut complete = analysis.methods.status.is_supported();
    let mut names = BTreeSet::new();
    for method in &analysis.methods.methods {
        if method.path != file.path() {
            complete = false;
        }
        if valid_reserved(&method.name) {
            names.insert(method.name.clone());
        } else {
            complete = false;
        }
    }
    for opaque in &analysis.methods.opaque {
        if valid_reserved(opaque) {
            names.insert(opaque.clone());
        } else {
            complete = false;
        }
    }
    if registration.fact.kind == RegistrationKind::Component {
        complete &=
            analysis.data.status.is_supported() && analysis.properties.status.is_supported();
        for field in &analysis.data.fields {
            if field.path != file.path() {
                complete = false;
            }
            if valid_reserved(&field.name) {
                names.insert(field.name.clone());
            } else {
                complete = false;
            }
        }
        for property in &analysis.properties.properties {
            if property.path != file.path() {
                complete = false;
            }
            if valid_reserved(&property.name) {
                names.insert(property.name.clone());
            } else {
                complete = false;
            }
        }
    }
    let mut reserved = names.into_iter().collect::<Vec<_>>();
    if reserved.len() > MAX_SYMBOL_ENTRIES {
        reserved.truncate(MAX_SYMBOL_ENTRIES);
        complete = false;
    }
    (reserved, complete)
}

fn valid_reserved(name: &str) -> bool {
    !name.is_empty() && expression::is_binding_identifier(name)
}
