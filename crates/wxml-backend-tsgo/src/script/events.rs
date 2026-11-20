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
use super::registration::{
    ParsedRegistration, RegistrationKind, RegistrationScan, selection_integrity,
};
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
    /// Type annotation of the first parameter, exactly as written.
    ///
    /// `None` means the handler has no annotated first parameter, either
    /// because it has no parameters at all (see [`Self::arity`]) or because the
    /// parameter is unannotated. Callers must not infer a parameter type.
    pub parameter: Option<String>,
    /// Number of declared parameters.
    ///
    /// `0` means the handler takes no argument, so any event detail is
    /// acceptable. Shapes that cannot be read keep `1`, which suppresses a
    /// type-match claim instead of inventing a zero-argument handler.
    pub arity: usize,
}

/// Page/component method table used by event resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodTable {
    /// Supported only for one unambiguous top-level registration with a
    /// complete literal table.
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

    let integrity = selection_integrity(scan, &registration.fact);
    if integrity.ambiguous {
        problems.push(format!("multiple {} registrations", kind.registrar()));
    }
    if integrity.not_top_level {
        problems.push(format!(
            "the selected {} registration is not top-level",
            kind.registrar()
        ));
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
        let (parameter, arity) = handler_parameters(&method.signature);
        methods.insert(
            method.name.clone(),
            EventMethod {
                name: method.name.clone(),
                signature: method.signature.clone(),
                documentation: method.documentation.clone(),
                path: method.path.clone(),
                range: method.range,
                parameter,
                arity,
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
    let signature = signature_of(script, member);
    let (parameter, arity) = handler_parameters(&signature);
    methods.insert(
        name.to_owned(),
        EventMethod {
            name: name.to_owned(),
            signature: cap_signature(&signature),
            documentation: doc_before(script.text, member.key_start),
            path: file.path().to_owned(),
            range,
            parameter,
            arity,
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

/// Extracts the first parameter annotation and the parameter count.
///
/// The input is a normalized one-line signature from
/// [`signature_of`](super::scanner::signature_of), so method shorthand,
/// `name: function (...)`, parenthesized arrows and bare arrows all arrive in
/// the same shape. Anything that cannot be read is reported as one unknown
/// parameter: `arity == 0` is only returned when an empty parameter list was
/// actually found, because callers use it to accept any event detail.
pub(crate) fn handler_parameters(signature: &str) -> (Option<String>, usize) {
    if let Some(open) = signature.find('(') {
        if let Some(close) = matching_paren(signature, open) {
            let inner = signature.get(open + 1..close).unwrap_or("");
            let parameters = split_top_level(inner);
            let arity = parameters
                .iter()
                .filter(|parameter| !parameter.trim().is_empty())
                .count();
            let first = parameters
                .iter()
                .map(|parameter| parameter.trim())
                .find(|parameter| !parameter.is_empty());
            let parameter = first.and_then(first_parameter_type);
            return (parameter, arity);
        }
        return (None, 1);
    }
    // A bare arrow parameter (`onTap: e => ...`) is one unannotated parameter.
    if signature.contains("=>") {
        return (None, 1);
    }
    (None, 1)
}

/// Returns the index of the `)` matching the `(` at `open`.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Splits a parameter list on top-level commas.
fn split_top_level(inner: &str) -> Vec<&str> {
    let bytes = inner.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0_usize;
    let mut index = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(inner.get(start..index).unwrap_or(""));
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    parts.push(inner.get(start..).unwrap_or(""));
    parts
}

/// Returns the type annotation of one parameter, or `None` when unannotated.
///
/// A top-level default value is dropped so `e: T = value` still yields `T`;
/// `=>` is kept because it belongs to a function type.
fn first_parameter_type(parameter: &str) -> Option<String> {
    let bytes = parameter.as_bytes();
    let mut depth = 0_usize;
    let mut colon = None;
    let mut default = None;
    let mut index = 0_usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = string_end(bytes, index);
                continue;
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
            b':' if depth == 0 && colon.is_none() => colon = Some(index),
            b'=' if depth == 0
                && bytes.get(index + 1) != Some(&b'>')
                && bytes.get(index.wrapping_sub(1)) != Some(&b'=') =>
            {
                default = Some(index);
                break;
            }
            _ => {}
        }
        index += 1;
    }
    let colon = colon?;
    let end = default.filter(|end| *end > colon).unwrap_or(bytes.len());
    let annotation = parameter.get(colon + 1..end)?.trim();
    (!annotation.is_empty()).then(|| annotation.to_owned())
}

/// Returns the byte after the string literal starting at `index`.
fn string_end(bytes: &[u8], index: usize) -> usize {
    let Some(&quote) = bytes.get(index) else {
        return index;
    };
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = cursor.saturating_add(2),
            byte if byte == quote => return cursor + 1,
            _ => cursor += 1,
        }
    }
    bytes.len()
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::handler_parameters;

    /// Splits `signature` into the annotation and arity a caller would see.
    fn shape(signature: &str) -> (Option<String>, usize) {
        handler_parameters(signature)
    }

    #[test]
    fn method_shorthand_records_the_annotation_and_arity() {
        assert_eq!(
            shape("onChange(e: WechatMiniprogram.CustomEvent<{ index: number }>)"),
            (
                Some("WechatMiniprogram.CustomEvent<{ index: number }>".to_owned()),
                1
            )
        );
        assert_eq!(
            shape("onTap(a: string, b: number)"),
            (Some("string".to_owned()), 2)
        );
        assert_eq!(shape("onTap()"), (None, 0));
        assert_eq!(shape("onTap(e)"), (None, 1));
    }

    #[test]
    fn function_and_arrow_properties_record_the_annotation() {
        assert_eq!(
            shape("onChange: function (e: CustomEvent<{ index: number }>) ..."),
            (Some("CustomEvent<{ index: number }>".to_owned()), 1)
        );
        assert_eq!(
            shape("onChange: (e: CustomEvent<{ index: number }>) =>..."),
            (Some("CustomEvent<{ index: number }>".to_owned()), 1)
        );
        assert_eq!(
            shape("onChange: async (e: CustomEvent<string>) =>..."),
            (Some("CustomEvent<string>".to_owned()), 1)
        );
        assert_eq!(shape("onTap: () =>..."), (None, 0));
        assert_eq!(shape("onTap: e =>..."), (None, 1));
        assert_eq!(
            shape("onTap: (event: WechatMiniprogram.TouchEvent) =>..."),
            (Some("WechatMiniprogram.TouchEvent".to_owned()), 1)
        );
    }

    #[test]
    fn optional_destructured_and_defaulted_parameters_are_handled() {
        assert_eq!(
            shape("onChange(e?: CustomEvent<{ index: number }>)"),
            (Some("CustomEvent<{ index: number }>".to_owned()), 1)
        );
        assert_eq!(
            shape("onChange({ detail }: CustomEvent<{ index: number }>)"),
            (Some("CustomEvent<{ index: number }>".to_owned()), 1)
        );
        assert_eq!(
            shape("onChange(e: CustomEvent<{ index: number }> = fallback)"),
            (Some("CustomEvent<{ index: number }>".to_owned()), 1)
        );
        assert_eq!(
            shape("onChange(e: (value: string) => void)"),
            (Some("(value: string) => void".to_owned()), 1)
        );
    }

    #[test]
    fn unreadable_shapes_never_claim_a_zero_argument_handler() {
        assert_eq!(shape("onChange: function"), (None, 1));
        assert_eq!(shape("onChange: handlerRef"), (None, 1));
        assert_eq!(shape(""), (None, 1));
    }
}
