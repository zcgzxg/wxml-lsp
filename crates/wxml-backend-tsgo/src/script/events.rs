//! Event binding forms and owner-method resolution.
//!
//! The existing template semantics layer owns WXML attribute scanning and
//! sends the static handler name; this module does not create a second template
//! parser. [`parse_event_attribute`] is a validation/normalization helper for
//! that adapter and for contract tests. This module owns the script side: it
//! builds the page/component method table (including behavior-contributed
//! methods) and resolves one handler per binding.
//!
//! Honesty rules mirror the TypeScript oracle:
//!
//! - a missing handler is an error only when the method table is complete;
//! - multiple/non-literal registrations, spread/computed members, non-literal
//!   `Component.methods` and an incomplete behavior graph make the table
//!   `Partial`, so a missing name is never proven missing;
//! - a name that exists with a non-literal value is `Partial` but not an error;
//! - dynamic/non-identifier handlers are `Partial` and never diagnostics.

use std::collections::{BTreeMap, BTreeSet};

use wxml_types::CapabilityStatus;

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::behavior::BehaviorMethod;
use super::registration::{ParsedRegistration, RegistrationKind, RegistrationScan};
use super::scanner::{
    Member, MemberKind, Script, doc_before, object_bounds, scan_members, signature_of,
};
use super::symbols::ScriptRange;

/// Defensive cap for one event batch.
pub const MAX_EVENT_RESULTS: usize = 1024;
/// Maximum signature length in characters.
pub const MAX_SIGNATURE_LENGTH: usize = 512;
/// Maximum own handler-table members processed per registration.
const MAX_OWN_METHODS: usize = 512;

/// Result of truncating one event batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventBindingForm {
    /// `bindtap`.
    Bind,
    /// `bind:tap`.
    BindColon,
    /// `catchtap`.
    Catch,
    /// `catch:tap`.
    CatchColon,
    /// `capture-bind:tap`.
    CaptureBind,
    /// `capture-catch:tap`.
    CaptureCatch,
    /// `mut-bind:tap`.
    MutBind,
}

impl EventBindingForm {
    /// Returns the stable lower-case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bind => "bind",
            Self::BindColon => "bind-colon",
            Self::Catch => "catch",
            Self::CatchColon => "catch-colon",
            Self::CaptureBind => "capture-bind",
            Self::CaptureCatch => "capture-catch",
            Self::MutBind => "mut-bind",
        }
    }
}

/// One static event binding supplied by the template layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBinding {
    /// Stable event id.
    pub event_id: String,
    /// Original attribute name, for example `bind:tap`.
    pub attribute: String,
    /// Event name without the binding prefix, for example `tap`.
    pub event: String,
    /// Static handler name; non-identifiers are `Partial`.
    pub handler: String,
    /// Exact WXML range of the handler token (UTF-8 bytes).
    pub handler_range: SourceRange,
}

impl EventBinding {
    /// Creates a binding from the template facts.
    #[must_use]
    pub fn new(
        event_id: impl Into<String>,
        attribute: impl Into<String>,
        event: impl Into<String>,
        handler: impl Into<String>,
        handler_range: SourceRange,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            attribute: attribute.into(),
            event: event.into(),
            handler: handler.into(),
            handler_range,
        }
    }
}

/// One event method in the owner table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMethod {
    /// Method name.
    pub name: String,
    /// One-line signature snippet.
    pub signature: String,
    /// `JSDoc` text; Chinese docs are preserved.
    pub documentation: String,
    /// Declaring file.
    pub path: String,
    /// Exact original name-token range.
    pub range: ScriptRange,
}

/// Page/component method table used by event resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodTable {
    /// Supported only when no ambiguity was found.
    pub status: CapabilityStatus,
    /// Owner label used in missing-handler messages (`page`/`component`).
    pub owner: &'static str,
    /// Literal-function methods.
    pub methods: Vec<EventMethod>,
    /// Names present with a non-literal value.
    pub opaque: Vec<String>,
    /// Degradation reasons.
    pub problems: Vec<String>,
}

/// Definition of one resolved handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDefinition {
    /// Project-relative declaring file.
    pub path: String,
    /// Exact original name range.
    pub range: ScriptRange,
}

/// One missing-handler diagnostic at the WXML handler range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDiagnostic {
    /// Exact WXML handler range.
    pub range: SourceRange,
    /// Always `error`.
    pub severity: &'static str,
    /// Bounded message.
    pub message: String,
}

/// One resolved event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventResolution {
    /// Stable event id.
    pub event_id: String,
    /// Supported/Partial state of this result.
    pub status: CapabilityStatus,
    /// Handler definition when resolved.
    pub definition: Option<EventDefinition>,
    /// Signature when resolved.
    pub signature: Option<String>,
    /// Documentation when resolved.
    pub documentation: Option<String>,
    /// Missing-handler error, only when the table is complete.
    pub diagnostic: Option<EventDiagnostic>,
    /// Degradation reason.
    pub reason: Option<String>,
}

/// One batch of event results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCollection {
    /// Resolved events in request order.
    pub results: Vec<EventResolution>,
    /// Events beyond the cap or otherwise skipped.
    pub skipped: usize,
    /// Up to three bounded skip examples.
    pub skipped_details: Vec<String>,
}

/// Parses a WXML event attribute name.
///
/// The recognized prefixes are `capture-bind:`, `capture-catch:`, `mut-bind:`,
/// `bind:`, `catch:`, `bind` and `catch`. The event name may contain letters,
/// digits, `-`, `_` and `:` but must be non-empty.
#[must_use]
pub fn parse_event_attribute(attribute: &str) -> Option<(EventBindingForm, String)> {
    const PREFIXES: &[(&str, EventBindingForm)] = &[
        ("capture-bind:", EventBindingForm::CaptureBind),
        ("capture-catch:", EventBindingForm::CaptureCatch),
        ("mut-bind:", EventBindingForm::MutBind),
        ("bind:", EventBindingForm::BindColon),
        ("catch:", EventBindingForm::CatchColon),
        ("bind", EventBindingForm::Bind),
        ("catch", EventBindingForm::Catch),
    ];
    for (prefix, form) in PREFIXES {
        if let Some(event) = attribute.strip_prefix(prefix) {
            if event.is_empty()
                || !event.chars().all(|character| {
                    character.is_alphanumeric() || matches!(character, '-' | '_' | ':')
                })
            {
                return None;
            }
            return Some((*form, event.to_owned()));
        }
    }
    None
}

/// Builds the owner method table for one selected registration.
pub(crate) fn build_table(
    file: &SourceFile,
    scan: &RegistrationScan,
    registration: &ParsedRegistration,
    behavior_methods: &[BehaviorMethod],
    behavior_incomplete: bool,
) -> MethodTable {
    let kind = registration.fact.kind;
    let mut problems = Vec::new();
    let mut opaque = BTreeSet::new();
    let mut methods: BTreeMap<String, EventMethod> = BTreeMap::new();

    let same_kind: Vec<_> = scan
        .registrations
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .collect();
    let top_level = same_kind
        .iter()
        .filter(|candidate| candidate.top_level)
        .count();
    let ambiguous = if top_level > 0 {
        top_level > 1
    } else {
        same_kind.len() > 1
    };
    if ambiguous {
        problems.push(format!("multiple {} registrations", kind.registrar()));
    }
    if scan
        .unsupported
        .iter()
        .any(|candidate| candidate.kind == kind)
    {
        problems.push("registration is not a literal object".to_owned());
    }

    let Ok(script) = Script::new(file.text()) else {
        return MethodTable {
            status: CapabilityStatus::Partial {
                reason: "the script could not be scanned".to_owned(),
            },
            owner: owner_label(kind),
            methods: Vec::new(),
            opaque: Vec::new(),
            problems: vec!["the script could not be scanned".to_owned()],
        };
    };

    if kind == RegistrationKind::Page {
        collect_page(
            &script,
            file,
            &registration.members,
            &mut methods,
            &mut opaque,
            &mut problems,
        );
    } else {
        collect_component(
            &script,
            file,
            &registration.members,
            &mut methods,
            &mut opaque,
            &mut problems,
        );
    }

    for method in behavior_methods {
        if methods.contains_key(&method.name) || opaque.contains(&method.name) {
            continue;
        }
        methods.insert(
            method.name.clone(),
            EventMethod {
                name: method.name.clone(),
                signature: method.signature.clone(),
                documentation: method.documentation.clone(),
                path: method.path.clone(),
                range: method.range,
            },
        );
    }
    if behavior_incomplete {
        problems.push("behavior methods are partially resolved".to_owned());
    }

    let status = if problems.is_empty() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: problems.join("; "),
        }
    };
    MethodTable {
        status,
        owner: owner_label(kind),
        methods: methods.into_values().collect(),
        opaque: opaque.into_iter().collect(),
        problems,
    }
}

/// Page option keys that describe the framework container, not handlers.
const PAGE_RESERVED: &[&str] = &[
    "data",
    "properties",
    "methods",
    "observers",
    "lifetimes",
    "pageLifetimes",
    "behaviors",
    "options",
    "externalClasses",
    "relations",
];

const fn owner_label(kind: RegistrationKind) -> &'static str {
    match kind {
        RegistrationKind::Page => "page",
        RegistrationKind::Component => "component",
        RegistrationKind::Behavior => "behavior",
    }
}

fn collect_page(
    script: &Script<'_>,
    file: &SourceFile,
    members: &[Member],
    methods: &mut BTreeMap<String, EventMethod>,
    opaque: &mut BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    let mut processed = 0_usize;
    for member in members {
        if member.kind == MemberKind::Spread {
            push_reason(problems, "options contain spread members");
            continue;
        }
        let Some(name) = &member.name else {
            push_reason(problems, "registration contains computed member names");
            continue;
        };
        if name.starts_with("__wxml_") || PAGE_RESERVED.contains(&name.as_str()) {
            continue;
        }
        if processed >= MAX_OWN_METHODS {
            push_reason(
                problems,
                "page options exceed the handler-table processing budget",
            );
            break;
        }
        processed += 1;
        add_member(script, file, name, member, methods, opaque);
    }
}

fn collect_component(
    script: &Script<'_>,
    file: &SourceFile,
    members: &[Member],
    methods: &mut BTreeMap<String, EventMethod>,
    opaque: &mut BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    let mut methods_member = None;
    for member in members {
        if member.kind == MemberKind::Spread {
            push_reason(problems, "options contain spread members");
            continue;
        }
        if member.name.as_deref() == Some("methods") {
            methods_member = Some(member);
        }
    }
    let Some(member) = methods_member else {
        // A component without `methods` genuinely has no handlers: the empty
        // table is complete, so a missing handler stays a real error.
        return;
    };
    let Some((open, close)) = object_bounds(script, member) else {
        push_reason(problems, "component `methods` is not a literal object");
        return;
    };
    let mut processed = 0_usize;
    for entry in scan_members(script, open + 1, close) {
        if entry.kind == MemberKind::Spread {
            push_reason(problems, "`methods` contains spread members");
            continue;
        }
        let Some(name) = &entry.name else {
            push_reason(problems, "`methods` contains computed member names");
            continue;
        };
        if name.starts_with("__wxml_") {
            continue;
        }
        if processed >= MAX_OWN_METHODS {
            push_reason(
                problems,
                "component `methods` exceeds the handler-table processing budget",
            );
            break;
        }
        processed += 1;
        add_member(script, file, name, &entry, methods, opaque);
    }
}

fn add_member(
    script: &Script<'_>,
    file: &SourceFile,
    name: &str,
    member: &Member,
    methods: &mut BTreeMap<String, EventMethod>,
    opaque: &mut BTreeSet<String>,
) {
    let literal = match member.kind {
        MemberKind::Method => true,
        MemberKind::Property => {
            member.value_start < member.value_end
                && (script.is_punct(member.value_start, '(')
                    || script.is_ident(member.value_start, "function")
                    || (script.is_ident(member.value_start, "async")
                        && (script.is_punct(member.value_start + 1, '(')
                            || script.is_ident(member.value_start + 1, "function")))
                    || (script.kind(member.value_start) == Some(TokenKind::Identifier)
                        && script.is_punct(member.value_start + 1, '=')))
        }
        _ => false,
    };
    if !literal {
        // Shorthand/identifier/accessor: the name is known, its value is not
        // statically a function. Never invent a resolution, never report it
        // missing.
        opaque.insert(name.to_owned());
        return;
    }
    let Ok(range) = script_range(script, member.key_range()) else {
        opaque.insert(name.to_owned());
        return;
    };
    methods.insert(
        name.to_owned(),
        EventMethod {
            name: name.to_owned(),
            signature: cap_signature(&signature_of(script, member)),
            documentation: doc_before(script.text, member.key_start),
            path: file.path().to_owned(),
            range,
        },
    );
}

/// Resolves event bindings against a method table.
#[must_use]
pub fn resolve_events(table: &MethodTable, events: &[EventBinding]) -> EventCollection {
    let mut results = Vec::new();
    let mut skipped = 0_usize;
    let mut skipped_details = Vec::new();
    if events.len() > MAX_EVENT_RESULTS {
        skipped += events.len() - MAX_EVENT_RESULTS;
        skipped_details.push("event list is truncated".to_owned());
    }
    for binding in events.iter().take(MAX_EVENT_RESULTS) {
        results.push(resolve_one(table, binding));
    }
    EventCollection {
        results,
        skipped,
        skipped_details,
    }
}

fn resolve_one(table: &MethodTable, binding: &EventBinding) -> EventResolution {
    let handler = binding.handler.trim();
    if !is_identifier_text(handler) {
        return partial(
            binding,
            "event handler is not a static identifier",
            None,
            None,
        );
    }
    if binding.handler_range.start.get() > binding.handler_range.end.get() {
        return partial(binding, "event binding range is invalid", None, None);
    }
    if let Some(method) = table.methods.iter().find(|method| method.name == handler) {
        return EventResolution {
            event_id: binding.event_id.clone(),
            status: table.status.clone(),
            definition: Some(EventDefinition {
                path: method.path.clone(),
                range: method.range,
            }),
            signature: (!method.signature.is_empty()).then(|| method.signature.clone()),
            documentation: (!method.documentation.is_empty()).then(|| method.documentation.clone()),
            diagnostic: None,
            reason: reason_of(&table.status),
        };
    }
    if table.opaque.iter().any(|name| name == handler) {
        return partial(
            binding,
            format!(
                "event handler \"{}\" is not a literal function",
                bounded_utf8(handler, 160)
            ),
            None,
            None,
        );
    }
    if !table.status.is_supported() {
        return partial(
            binding,
            reason_of(&table.status).unwrap_or_else(|| "the method table is partial".to_owned()),
            None,
            None,
        );
    }
    let message = missing_handler_message(handler, &binding.event, table.owner);
    EventResolution {
        event_id: binding.event_id.clone(),
        status: CapabilityStatus::Supported,
        definition: None,
        signature: None,
        documentation: None,
        diagnostic: Some(EventDiagnostic {
            range: binding.handler_range,
            severity: "error",
            message,
        }),
        reason: None,
    }
}

fn partial(
    binding: &EventBinding,
    reason: impl Into<String>,
    definition: Option<EventDefinition>,
    diagnostic: Option<EventDiagnostic>,
) -> EventResolution {
    let reason = reason.into();
    EventResolution {
        event_id: binding.event_id.clone(),
        status: CapabilityStatus::Partial {
            reason: reason.clone(),
        },
        definition,
        signature: None,
        documentation: None,
        diagnostic,
        reason: Some(reason),
    }
}

fn reason_of(status: &CapabilityStatus) -> Option<String> {
    match status {
        CapabilityStatus::Supported => None,
        CapabilityStatus::Partial { reason }
        | CapabilityStatus::Unsupported { reason }
        | CapabilityStatus::Disabled { reason }
        | CapabilityStatus::Unavailable { reason } => Some(reason.clone()),
        CapabilityStatus::Unknown { reason } => reason.clone(),
        _ => Some("script capability is degraded".to_owned()),
    }
}

fn missing_handler_message(handler: &str, event: &str, owner: &str) -> String {
    let handler = bounded_utf8(&sanitize_name(handler), 160);
    let event = bounded_utf8(&sanitize_name(event), 80);
    bounded_utf8(
        &format!("Event handler \"{handler}\" for \"{event}\" is not defined on this {owner}"),
        512,
    )
}

fn sanitize_name(text: &str) -> String {
    text.chars()
        .map(|character| {
            if matches!(character, '"' | '\\') || character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn is_identifier_text(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_alphabetic()) {
        return false;
    }
    characters.all(|character| character == '_' || character == '$' || character.is_alphanumeric())
}

fn cap_signature(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_SIGNATURE_LENGTH {
        return text.to_owned();
    }
    let mut capped = text
        .chars()
        .take(MAX_SIGNATURE_LENGTH - 1)
        .collect::<String>();
    capped.truncate(capped.trim_end().len());
    capped.push('…');
    capped
}

fn bounded_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn script_range(
    script: &Script<'_>,
    range: SourceRange,
) -> Result<ScriptRange, super::ScriptError> {
    script.source_range(range)
}

fn push_reason(problems: &mut Vec<String>, reason: &str) {
    if !problems.iter().any(|existing| existing == reason) {
        problems.push(reason.to_owned());
    }
}
