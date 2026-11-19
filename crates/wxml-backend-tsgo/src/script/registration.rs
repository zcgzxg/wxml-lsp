//! `Page`/`Component`/`Behavior` registration extraction.
//!
//! Only a call whose first argument is an entire object literal is accepted.
//! The scanner records the exact call/options token ranges, the first-level
//! members and whether the call is top level. A registrar name that is shadowed
//! by a local declaration, an object-like alias call and any non-literal
//! argument are reported as [`UnsupportedRegistration`] instead of being
//! guessed.
//!
//! Selection prefers a top-level canonical call over a top-level alias call,
//! over a nested call, over any remaining candidate. A nested registration
//! is still parsed so callers can show honest facts, but only a single
//! top-level call can support a complete owner table (see
//! `selection_integrity`).
//!
//! Static `const page = Page;` aliases are resolved when the alias is declared
//! exactly once with `const`, the initializer is the unqualified registrar, and
//! neither the registrar nor the alias is shadowed. Shadow checks are
//! precomputed once per script so a large options object cannot turn
//! registration discovery into a quadratic scan. The alias is recorded on
//! [`RegistrationFact`] so downstream owners can tell an alias call from a
//! direct one.

use std::collections::BTreeMap;

use crate::input::SourceFile;
use crate::shadow::mapping::SourceRange;

use super::ScriptError;
use super::scanner::{Member, Script, scan_members};

/// Mini-program registration kind recognized by the script layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistrationKind {
    /// `Page(...)`.
    Page,
    /// `Component(...)`.
    Component,
    /// `Behavior(...)`.
    Behavior,
}

impl RegistrationKind {
    /// Returns the canonical registrar name.
    #[must_use]
    pub const fn registrar(self) -> &'static str {
        match self {
            Self::Page => "Page",
            Self::Component => "Component",
            Self::Behavior => "Behavior",
        }
    }

    /// Returns the stable lower-case label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Component => "component",
            Self::Behavior => "behavior",
        }
    }
}

/// One statically readable registration call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationFact {
    /// Registration kind.
    pub kind: RegistrationKind,
    /// Alias used at the call site (`const p = Page; p({...})`).
    pub alias: Option<String>,
    /// Exact registrar/alias token range.
    pub name_range: SourceRange,
    /// Exact options object literal range, including braces.
    pub options_range: SourceRange,
    /// True when the call is not nested in a block/function/object.
    pub top_level: bool,
}

/// One call that looks like a registration but cannot be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedRegistration {
    /// Registration kind the call was attributed to.
    pub kind: RegistrationKind,
    /// Exact call/identifier range.
    pub range: SourceRange,
    /// Stable reason marker.
    pub reason: String,
}

/// Result of scanning one script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationScan {
    /// Literal registrations in source order.
    pub registrations: Vec<RegistrationFact>,
    /// Rejected calls in source order.
    pub unsupported: Vec<UnsupportedRegistration>,
}

/// A selected registration with its parsed member list.
#[derive(Debug, Clone)]
pub(crate) struct ParsedRegistration {
    /// Public fact.
    pub(crate) fact: RegistrationFact,
    /// First-level options members.
    pub(crate) members: Vec<Member>,
}

/// Scans all registrations in a document.
pub(crate) fn scan(file: &SourceFile) -> Result<RegistrationScan, ScriptError> {
    let script = Script::new(file.text())?;
    let alias_declarations = alias_declarations(&script);
    let shadows = shadow_set(&script, &alias_declarations);
    let aliases = alias_map(&alias_declarations)
        .into_iter()
        .filter(|(_, kind)| !shadows.is_shadowed(kind.registrar()))
        .collect::<BTreeMap<_, _>>();
    let mut registrations = Vec::new();
    let mut unsupported = Vec::new();
    let depths = script.depths();
    for index in 0..script.tokens.len() {
        let Some(text) = script.token_text(index) else {
            continue;
        };
        if script.kind(index) != Some(crate::shadow::expression::TokenKind::Identifier) {
            continue;
        }
        if (index > 0 && script.is_punct(index - 1, '.'))
            || script.is_ident(index.saturating_sub(1), "new")
            || script.is_ident(index.saturating_sub(1), "function")
            || script.is_ident(index.saturating_sub(1), "class")
        {
            continue;
        }
        let (kind, alias) = if let Some(kind) = canonical_kind(text) {
            if shadows.is_shadowed(text) {
                if script.is_punct(index + 1, '(') {
                    unsupported.push(UnsupportedRegistration {
                        kind,
                        range: token_range(&script, index),
                        reason: "shadowed identifier".to_owned(),
                    });
                }
                continue;
            }
            (kind, None)
        } else if let Some(kind) = aliases.get(text) {
            if shadows.is_alias_shadowed(text) {
                // A callsite-shadowed alias is an ordinary local function; it
                // is not a rejected registration-shaped call and must never be
                // selected as a real Page/Component/Behavior.
                continue;
            }
            (*kind, Some(text.to_owned()))
        } else {
            continue;
        };
        if !in_call_position(&script, index) {
            continue;
        }
        let argument = index + 2;
        if !script.is_punct(argument, '{') {
            unsupported.push(UnsupportedRegistration {
                kind,
                range: token_range(&script, index),
                reason: "argument is not an object literal".to_owned(),
            });
            continue;
        }
        let Ok(close) = script.close_of(argument) else {
            unsupported.push(UnsupportedRegistration {
                kind,
                range: token_range(&script, index),
                reason: "unbalanced object literal".to_owned(),
            });
            continue;
        };
        let after = close + 1;
        if !(script.is_punct(after, ',') || script.is_punct(after, ')')) {
            unsupported.push(UnsupportedRegistration {
                kind,
                range: token_range(&script, index),
                reason: "argument is not an entire object literal".to_owned(),
            });
            continue;
        }
        registrations.push(RegistrationFact {
            kind,
            alias,
            name_range: token_range(&script, index),
            options_range: SourceRange::new(
                u32::try_from(script.token_start(argument).unwrap_or(0)).unwrap_or(u32::MAX),
                u32::try_from(script.token_end(close).unwrap_or(0)).unwrap_or(u32::MAX),
            ),
            top_level: depths.get(index).copied().unwrap_or(0) == 0,
        });
    }
    Ok(RegistrationScan {
        registrations,
        unsupported,
    })
}

pub(crate) fn canonical_kind(name: &str) -> Option<RegistrationKind> {
    match name {
        "Page" => Some(RegistrationKind::Page),
        "Component" => Some(RegistrationKind::Component),
        "Behavior" => Some(RegistrationKind::Behavior),
        _ => None,
    }
}

fn token_range(script: &Script<'_>, index: usize) -> SourceRange {
    SourceRange::new(
        u32::try_from(script.token_start(index).unwrap_or(0)).unwrap_or(u32::MAX),
        u32::try_from(script.token_end(index).unwrap_or(0)).unwrap_or(u32::MAX),
    )
}

/// Maximum distinct shadowed/alias names retained before failing closed.
const MAX_SHADOW_NAMES: usize = 4096;

/// Finds `const name = Page|Component|Behavior` aliases by declaration token.
fn alias_declarations(script: &Script<'_>) -> BTreeMap<usize, (String, RegistrationKind)> {
    let mut declarations = BTreeMap::new();
    for index in 0..script.tokens.len() {
        if !script.is_ident(index, "const") {
            continue;
        }
        let Some(name) = script.token_text(index + 1) else {
            continue;
        };
        if script.kind(index + 1) != Some(crate::shadow::expression::TokenKind::Identifier)
            || !script.is_punct(index + 2, '=')
        {
            continue;
        }
        let Some(kind) = script.token_text(index + 3).and_then(canonical_kind) else {
            continue;
        };
        if script.kind(index + 3) != Some(crate::shadow::expression::TokenKind::Identifier) {
            continue;
        }
        if !(script.is_punct(index + 4, ';')
            || script.is_punct(index + 4, ',')
            || index + 4 >= script.tokens.len())
        {
            continue;
        }
        declarations.insert(index, (name.to_owned(), kind));
    }
    declarations
}

fn alias_map(
    declarations: &BTreeMap<usize, (String, RegistrationKind)>,
) -> BTreeMap<String, RegistrationKind> {
    let mut aliases: BTreeMap<String, RegistrationKind> = BTreeMap::new();
    let mut duplicated: Vec<String> = Vec::new();
    for (name, kind) in declarations.values() {
        if aliases.insert(name.clone(), *kind).is_some() {
            duplicated.push(name.clone());
        }
    }
    for name in duplicated {
        aliases.remove(&name);
    }
    aliases
}

/// Precomputed declarations/parameters for shadow checks.
///
/// Building this once avoids rescanning the full token stream for every
/// `Page`/`Component`/`Behavior` candidate. The check is intentionally broad:
/// a name declared in any nested function/block shadows the alias for all
/// calls, so a fake call can never win over a canonical registration.
#[derive(Debug, Default)]
struct ShadowSet {
    non_alias: BTreeMap<String, usize>,
    alias: BTreeMap<String, usize>,
    overflowed: bool,
}

impl ShadowSet {
    fn add_non_alias(&mut self, name: &str) {
        *self.non_alias.entry(name.to_owned()).or_insert(0) += 1;
        self.check_budget();
    }

    fn add_alias(&mut self, name: &str) {
        *self.alias.entry(name.to_owned()).or_insert(0) += 1;
        self.check_budget();
    }

    fn check_budget(&mut self) {
        if self.non_alias.len().saturating_add(self.alias.len()) > MAX_SHADOW_NAMES {
            self.overflowed = true;
        }
    }

    fn is_shadowed(&self, name: &str) -> bool {
        self.overflowed || self.non_alias.contains_key(name) || self.alias.contains_key(name)
    }

    fn is_alias_shadowed(&self, name: &str) -> bool {
        self.overflowed
            || self.non_alias.get(name).is_some_and(|count| *count > 0)
            || self.alias.get(name).is_some_and(|count| *count > 1)
    }
}

fn shadow_set(
    script: &Script<'_>,
    alias_declarations: &BTreeMap<usize, (String, RegistrationKind)>,
) -> ShadowSet {
    let mut shadows = ShadowSet::default();
    for index in 0..script.tokens.len() {
        let Some(token) = script.token_text(index) else {
            continue;
        };
        if matches!(token, "function" | "class" | "const" | "let" | "var")
            && script.kind(index + 1) == Some(crate::shadow::expression::TokenKind::Identifier)
        {
            if let Some(name) = script.token_text(index + 1) {
                if alias_declarations.contains_key(&index) {
                    shadows.add_alias(name);
                } else {
                    shadows.add_non_alias(name);
                }
            }
        }
        if token == "import" && !script.is_punct(index + 1, '(') {
            collect_import_names(script, index + 1, &mut shadows);
        }
        if token == "catch"
            && script.is_punct(index + 1, '(')
            && let Ok(close) = script.close_of(index + 1)
        {
            for name in immediate_identifiers(script, index + 1, close) {
                shadows.add_non_alias(&name);
            }
        }
        if script.kind(index) == Some(crate::shadow::expression::TokenKind::Identifier)
            && script.is_punct(index + 1, '=')
            && script.is_punct(index + 2, '>')
        {
            shadows.add_non_alias(token);
        }
    }
    for index in 0..script.tokens.len() {
        if !script.is_punct(index, '(') {
            continue;
        }
        let Some(close) = script.pairs.get(index).copied().flatten() else {
            continue;
        };
        let arrow = script.is_punct(close + 1, '=') && script.is_punct(close + 2, '>');
        let function = function_keyword_before(script, index);
        let method = script.is_punct(close + 1, '{') && method_name_before(script, index);
        if arrow || function || method {
            for name in immediate_identifiers(script, index, close) {
                shadows.add_non_alias(&name);
            }
        }
    }
    shadows
}

fn collect_import_names(script: &Script<'_>, mut cursor: usize, shadows: &mut ShadowSet) {
    let mut scanned = 0_usize;
    while cursor < script.tokens.len() && scanned < 256 {
        scanned += 1;
        match script.kind(cursor) {
            Some(
                crate::shadow::expression::TokenKind::StringLiteral
                | crate::shadow::expression::TokenKind::Punctuator(';'),
            ) => break,
            Some(crate::shadow::expression::TokenKind::Identifier) => {
                if let Some(name) = script.token_text(cursor) {
                    if !matches!(name, "import" | "type" | "from" | "as") {
                        shadows.add_non_alias(name);
                    }
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    if cursor < script.tokens.len() && scanned >= 256 {
        shadows.overflowed = true;
    }
}

fn immediate_identifiers(script: &Script<'_>, open: usize, close: usize) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = open + 1;
    while cursor < close {
        if script.kind(cursor) == Some(crate::shadow::expression::TokenKind::Identifier)
            && let Some(name) = script.token_text(cursor)
        {
            names.push(name.to_owned());
        }
        if matches!(
            script.kind(cursor),
            Some(
                crate::shadow::expression::TokenKind::Punctuator('(' | '[' | '{')
                    | crate::shadow::expression::TokenKind::TemplateExpressionStart
            )
        ) && let Some(nested_close) = script.pairs.get(cursor).copied().flatten()
        {
            cursor = nested_close + 1;
            continue;
        }
        cursor += 1;
    }
    names
}

fn in_call_position(script: &Script<'_>, index: usize) -> bool {
    if !script.is_punct(index + 1, '(') {
        return false;
    }
    let Ok(close) = script.close_of(index + 1) else {
        return true;
    };
    // `Page({...}) {}` is a class/object method or an invalid expression, not
    // a registration call.
    if script.is_punct(close + 1, '{') {
        return false;
    }
    let member_start = index == 0
        || matches!(
            script.kind(index - 1),
            Some(crate::shadow::expression::TokenKind::Punctuator(
                '{' | '}' | ',' | ';'
            ))
        )
        || script.is_ident(index - 1, "static")
        || script.token_text(index - 1) == Some("async")
        || script.token_text(index - 1) == Some("get")
        || script.token_text(index - 1) == Some("set");
    // A class/object method may carry a return type between `)` and `{`.
    !(member_start && script.is_punct(close + 1, ':'))
}

/// Conservative shadowing check for one canonical registrar name.
fn function_keyword_before(script: &Script<'_>, open: usize) -> bool {
    let mut cursor = open;
    while cursor > 0 {
        cursor -= 1;
        if script.is_ident(cursor, "function") {
            return true;
        }
        if script.is_ident(cursor, "if")
            || script.is_ident(cursor, "while")
            || script.is_ident(cursor, "switch")
            || script.is_ident(cursor, "for")
            || script.is_ident(cursor, "with")
            || script.is_ident(cursor, "catch")
        {
            return false;
        }
        if open - cursor > 3 {
            break;
        }
    }
    false
}

fn method_name_before(script: &Script<'_>, open: usize) -> bool {
    let Some(previous) = open.checked_sub(1) else {
        return false;
    };
    if !matches!(
        script.kind(previous),
        Some(
            crate::shadow::expression::TokenKind::Identifier
                | crate::shadow::expression::TokenKind::StringLiteral
                | crate::shadow::expression::TokenKind::Number
        )
    ) {
        return false;
    }
    !matches!(
        script.token_text(previous),
        Some("if" | "while" | "switch" | "for" | "with" | "catch")
    )
}

/// Selects the registration for a requested root with the shadow fallback rules.
///
/// `Page` prefers a `Page` registration and falls back to `Component`; other
/// roots require their exact kind. Within one kind the preference order is:
/// top-level canonical call, any top-level call, any canonical call, then the
/// first remaining candidate. Source order breaks ties, so the choice is
/// deterministic. Callers must consult [`selection_integrity`] before claiming
/// a complete owner table.
pub(crate) fn select(
    file: &SourceFile,
    scan: &RegistrationScan,
    kind: RegistrationKind,
) -> Result<Option<ParsedRegistration>, ScriptError> {
    let wanted: &[RegistrationKind] = match kind {
        RegistrationKind::Page => &[RegistrationKind::Page, RegistrationKind::Component],
        RegistrationKind::Component | RegistrationKind::Behavior => &[kind],
    };
    let selected = wanted
        .iter()
        .find_map(|wanted_kind| preferred(scan, *wanted_kind))
        .cloned();
    let Some(fact) = selected else {
        return Ok(None);
    };
    let script = Script::new(file.text())?;
    let open = find_options_open(&script, &fact)?;
    let close = script.close_of(open)?;
    let members = scan_members(&script, open + 1, close);
    Ok(Some(ParsedRegistration { fact, members }))
}

/// Picks the best same-kind candidate in tier order, source order within a tier.
fn preferred(scan: &RegistrationScan, kind: RegistrationKind) -> Option<&RegistrationFact> {
    let same = || {
        scan.registrations
            .iter()
            .filter(move |candidate| candidate.kind == kind)
    };
    same()
        .find(|candidate| candidate.top_level && candidate.alias.is_none())
        .or_else(|| same().find(|candidate| candidate.top_level))
        .or_else(|| same().find(|candidate| candidate.alias.is_none()))
        .or_else(|| same().next())
}

/// Whether a selected registration can support a complete owner table.
///
/// Only one top-level call is authoritative: a nested call may be a factory or
/// helper, and two top-level calls cannot be ordered by source position alone.
/// The selected candidate is still used for facts in both cases, but callers
/// must report `Partial`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionIntegrity {
    /// More than one same-kind candidate can claim the owner: multiple
    /// top-level calls, or multiple candidates when none is top-level.
    pub(crate) ambiguous: bool,
    /// The selected call is not top-level.
    pub(crate) not_top_level: bool,
}

impl SelectionIntegrity {
    /// True only for one unambiguous top-level selection.
    pub(crate) const fn is_complete(self) -> bool {
        !self.ambiguous && !self.not_top_level
    }
}

/// Computes the ambiguity/top-level state for `selected` among same-kind calls.
///
/// Ambiguity keeps the historical rule: several top-level candidates are
/// ambiguous, and without any top-level candidate several nested ones are.
pub(crate) fn selection_integrity(
    scan: &RegistrationScan,
    selected: &RegistrationFact,
) -> SelectionIntegrity {
    let mut same_kind = 0_usize;
    let mut top_level = 0_usize;
    for candidate in scan
        .registrations
        .iter()
        .filter(|candidate| candidate.kind == selected.kind)
    {
        same_kind += 1;
        if candidate.top_level {
            top_level += 1;
        }
    }
    let ambiguous = if top_level > 0 {
        top_level > 1
    } else {
        same_kind > 1
    };
    SelectionIntegrity {
        ambiguous,
        not_top_level: !selected.top_level,
    }
}

pub(crate) fn find_options_open(
    script: &Script<'_>,
    fact: &RegistrationFact,
) -> Result<usize, ScriptError> {
    let mut candidates = Vec::new();
    for index in 0..script.tokens.len() {
        if !script.is_punct(index, '{') {
            continue;
        }
        let Some(start) = script.token_start(index) else {
            continue;
        };
        if u32::try_from(start).ok() == Some(fact.options_range.start.get()) {
            candidates.push(index);
        }
    }
    candidates
        .first()
        .copied()
        .ok_or_else(|| ScriptError::Invalid {
            feature: "registration.options",
            reason: "options object token not found".to_owned(),
        })
}

#[cfg(test)]
#[allow(clippy::assert_is_empty)]
mod tests {
    use super::*;
    use crate::input::DocumentRevision;

    fn file(text: &str) -> SourceFile {
        SourceFile::new("index.ts", text, DocumentRevision::new(1)).expect("valid")
    }

    #[test]
    fn scans_literal_registrations_and_aliases() {
        let scan = scan(&file(
            "const page = Page;\npage({ data: {} });\nComponent({ methods: {} });",
        ))
        .expect("scan");
        assert_eq!(scan.registrations.len(), 2);
        assert_eq!(scan.registrations[0].kind, RegistrationKind::Page);
        assert_eq!(scan.registrations[0].alias.as_deref(), Some("page"));
        assert_eq!(scan.registrations[1].kind, RegistrationKind::Component);
    }

    #[test]
    fn reports_shadowed_and_dynamic_registrations() {
        let scan = scan(&file(
            "function Page() {}\nPage({ data: {} });\nconst options = {};\nComponent(options);",
        ))
        .expect("scan");
        assert!(scan.registrations.is_empty());
        assert_eq!(scan.unsupported.len(), 2);
        assert!(scan.unsupported[0].reason.contains("shadowed"));
        assert!(scan.unsupported[1].reason.contains("object literal"));
    }
}
