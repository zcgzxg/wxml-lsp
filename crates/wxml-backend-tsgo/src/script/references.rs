//! Conservative syntactic references for data paths and event handlers.
//!
//! This is not a TypeScript checker. It only proves `this.data.<path>` and
//! `this.<handler>` chains inside function bodies that the registration binds to
//! the owner, plus simple lexical `const that = this` aliases. Anything that
//! could hide the target (computed access, destructuring, whole-value use,
//! mutable alias, another receiver, another script's `setData`) makes the scan
//! partial; no guessed token is published.

use std::collections::BTreeMap;

use wxml_types::{MAX_SYMBOL_ENTRIES, SymbolReferenceRole};

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::registration::{ParsedRegistration, RegistrationKind};
use super::scanner::{MemberKind, Script, object_bounds, scan_members};
use super::symbol_facts::{SymbolReferenceFact, SymbolTokenFact, push_reason};

/// Result of one reference scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceScan {
    /// Reference tokens in source order.
    pub(crate) references: Vec<SymbolReferenceFact>,
    /// False when any context could not be proven.
    pub(crate) complete: bool,
    /// Degradation reasons.
    pub(crate) problems: Vec<String>,
}

/// Scans data references in the subject script.
pub(crate) fn scan_data_references(
    file: &SourceFile,
    registration: &ParsedRegistration,
    target: &[String],
    declaration: Option<&SymbolTokenFact>,
) -> ReferenceScan {
    let Ok(script) = Script::new(file.text()) else {
        return partial_scan("the script could not be scanned for data references");
    };
    let mut scanner = Scanner::new(&script, file.path(), registration);
    if let Some(declaration) = declaration {
        if declaration.path == file.path() {
            scanner.add_reference(declaration.clone(), SymbolReferenceRole::Declaration);
        } else {
            scanner.complete = false;
            push_reason(
                &mut scanner.problems,
                "the data declaration is in another script; cross-file references are not proven",
            );
        }
    }
    scanner.scan_data(target);
    scanner.finish()
}

/// Scans handler references in the subject script.
pub(crate) fn scan_handler_references(
    file: &SourceFile,
    registration: &ParsedRegistration,
    name: &str,
    declaration: &SymbolTokenFact,
) -> ReferenceScan {
    let Ok(script) = Script::new(file.text()) else {
        return partial_scan("the script could not be scanned for handler references");
    };
    let mut scanner = Scanner::new(&script, file.path(), registration);
    if declaration.path == file.path() {
        scanner.add_reference(declaration.clone(), SymbolReferenceRole::Declaration);
    } else {
        scanner.complete = false;
        push_reason(
            &mut scanner.problems,
            "the handler declaration is in another script; cross-file references are not proven",
        );
    }
    scanner.scan_handler(name);
    scanner.finish()
}

fn partial_scan(reason: &str) -> ReferenceScan {
    ReferenceScan {
        references: Vec::new(),
        complete: false,
        problems: vec![reason.to_owned()],
    }
}

#[derive(Debug, Clone, Copy)]
struct AliasInfo {
    const_like: bool,
}

#[derive(Debug, Default)]
struct Aliases {
    names: BTreeMap<String, AliasInfo>,
}

impl Aliases {
    fn contains(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    fn get(&self, name: &str) -> Option<AliasInfo> {
        self.names.get(name).copied()
    }
}

type BodyRange = (usize, usize);
type FunctionMemberRange = (usize, usize, Option<BodyRange>);

#[derive(Debug, Default)]
struct ThisScopes {
    allowed: Vec<(usize, usize)>,
    bodies: Vec<(usize, usize)>,
    roots: Vec<(usize, usize)>,
    classes: Vec<(usize, usize)>,
}

impl ThisScopes {
    fn page_bound(&self, offset: usize) -> bool {
        if self
            .classes
            .iter()
            .any(|(start, end)| contains(*start, *end, offset))
        {
            return false;
        }
        if !self
            .allowed
            .iter()
            .any(|(start, end)| contains(*start, *end, offset))
        {
            return false;
        }
        let mut smallest: Option<(usize, usize)> = None;
        for (start, end) in &self.bodies {
            if contains(*start, *end, offset)
                && smallest.is_none_or(|(current_start, current_end)| {
                    end.saturating_sub(*start) < current_end.saturating_sub(current_start)
                })
            {
                smallest = Some((*start, *end));
            }
        }
        smallest.is_none_or(|body| self.roots.contains(&body))
    }
}

const fn contains(start: usize, end: usize, offset: usize) -> bool {
    offset >= start && offset < end
}

fn destructures_from_this(script: &Script<'_>, this_index: usize) -> bool {
    this_index >= 2 && script.is_punct(this_index - 1, '=') && script.is_punct(this_index - 2, '}')
}

struct Scanner<'a> {
    script: &'a Script<'a>,
    file_path: &'a str,
    aliases: Aliases,
    scopes: ThisScopes,
    complete: bool,
    problems: Vec<String>,
    references: Vec<SymbolReferenceFact>,
}

impl<'a> Scanner<'a> {
    fn new(script: &'a Script<'a>, file_path: &'a str, registration: &ParsedRegistration) -> Self {
        Self {
            script,
            file_path,
            aliases: collect_aliases(script),
            scopes: collect_scopes(script, registration),
            complete: true,
            problems: Vec::new(),
            references: Vec::new(),
        }
    }

    fn finish(mut self) -> ReferenceScan {
        if self.references.len() > MAX_SYMBOL_ENTRIES {
            self.references.truncate(MAX_SYMBOL_ENTRIES);
            self.complete = false;
            push_reason(&mut self.problems, "the reference list was truncated");
        }
        ReferenceScan {
            references: self.references,
            complete: self.complete,
            problems: self.problems,
        }
    }

    fn add_reference(&mut self, token: SymbolTokenFact, role: SymbolReferenceRole) {
        if self
            .references
            .iter()
            .any(|reference| reference.token.range == token.range && reference.role == role)
        {
            return;
        }
        self.references.push(SymbolReferenceFact { token, role });
    }

    fn scan_data(&mut self, target: &[String]) {
        if !self.aliases.names.is_empty() {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "a `this` alias exists; alias coverage is partial",
            );
        }
        for index in 0..self.script.tokens.len() {
            let Some(text) = self.script.token_text(index) else {
                continue;
            };
            if self.script.kind(index) != Some(TokenKind::Identifier) {
                continue;
            }
            let is_this = text == "this";
            let alias = self.aliases.get(text);
            if !is_this && alias.is_none() {
                continue;
            }
            if is_this && !self.scopes.page_bound(self.token_start(index)) {
                if (self.script.is_punct(index + 1, '.') && self.script.is_ident(index + 2, "data"))
                    || self.script.is_punct(index + 1, '[')
                {
                    self.complete = false;
                    push_reason(
                        &mut self.problems,
                        "`this` outside a page-bound method may reference this data",
                    );
                }
                continue;
            }
            if self.script.is_punct(index + 1, '.') {
                if self.script.is_ident(index + 2, "data") {
                    self.data_chain(index + 2, target, alias.is_some());
                }
            } else if self.script.is_punct(index + 1, '[') {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "a computed `this`/alias member access may reference this data",
                );
            } else if is_this {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "`this` is used as a whole value; destructuring or aliasing may hide data",
                );
            }
        }
    }

    fn scan_handler(&mut self, name: &str) {
        for index in 0..self.script.tokens.len() {
            let Some(text) = self.script.token_text(index) else {
                continue;
            };
            if self.script.kind(index) != Some(TokenKind::Identifier) {
                continue;
            }
            let is_this = text == "this";
            let alias = self.aliases.get(text);
            if !is_this && alias.is_none() {
                continue;
            }
            if is_this && !self.scopes.page_bound(self.token_start(index)) {
                if (self.script.is_punct(index + 1, '.') && self.script.is_ident(index + 2, name))
                    || (self.script.is_punct(index + 1, '[') && self.bracket_holds(index, name))
                {
                    self.complete = false;
                    push_reason(
                        &mut self.problems,
                        "`this` outside a page-bound method may reference the handler",
                    );
                }
                continue;
            }
            if self.script.is_punct(index + 1, '.') {
                if self.script.is_ident(index + 2, name) {
                    if let Some(alias) = alias
                        && !alias.const_like
                    {
                        self.complete = false;
                        push_reason(
                            &mut self.problems,
                            "a mutable `this` alias is used for a handler reference",
                        );
                    }
                    if let Some(token) = self.token(index + 2) {
                        self.add_reference(
                            token,
                            role_at(self.script, index + 2, self.token_start(index + 2)),
                        );
                    }
                }
            } else if self.script.is_punct(index + 1, '[') && self.bracket_holds(index, name) {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "a computed handler access cannot be proven static",
                );
            } else if is_this && destructures_from_this(self.script, index) {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "destructuring `this` may hide handler references",
                );
            }
        }
        self.scan_other_receivers(name);
    }

    fn scan_other_receivers(&mut self, name: &str) {
        for index in 0..self.script.tokens.len() {
            if index < 2 {
                continue;
            }
            if self.script.token_text(index) != Some(name)
                || self.script.kind(index) != Some(TokenKind::Identifier)
                || !self.script.is_punct(index - 1, '.')
            {
                continue;
            }
            let receiver = self.script.token_text(index - 2);
            let is_primary =
                receiver == Some("this") && self.scopes.page_bound(self.token_start(index - 2));
            let is_alias = receiver.is_some_and(|receiver| self.aliases.contains(receiver));
            if is_primary || is_alias {
                continue;
            }
            self.complete = false;
            push_reason(
                &mut self.problems,
                "a member access on another receiver may reference the handler",
            );
        }
        for index in 0..self.script.tokens.len() {
            if self.script.kind(index) != Some(TokenKind::StringLiteral) {
                continue;
            }
            if self.bracket_holds(index, name) {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "a computed handler access cannot be proven static",
                );
            }
        }
    }

    fn bracket_holds(&self, index: usize, name: &str) -> bool {
        index > 0
            && self.script.is_punct(index - 1, '[')
            && self.script.is_punct(index + 1, ']')
            && super::scanner::unquote(self.script.token_text(index).unwrap_or_default()).as_deref()
                == Some(name)
    }

    fn data_chain(&mut self, data_index: usize, target: &[String], alias: bool) {
        if alias {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "a `this` alias accesses `data`; alias references are not complete",
            );
        }
        let after = data_index + 1;
        if after >= self.script.tokens.len() {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "`this.data` is used as a whole value; dynamic field access is not analyzable",
            );
            return;
        }
        if self.script.is_punct(after, '?') && self.script.is_punct(after + 1, '.') {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "`this.data` uses optional chaining that cannot be proven static",
            );
            return;
        }
        if self.script.is_punct(after, '[') {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "`this.data` uses a computed access that cannot be proven static",
            );
            return;
        }
        if !self.script.is_punct(after, '.') {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "`this.data` is used as a whole value; dynamic field access is not analyzable",
            );
            return;
        }
        let mut segments = Vec::new();
        let mut cursor = after + 1;
        while cursor < self.script.tokens.len() {
            if self.script.kind(cursor) != Some(TokenKind::Identifier) {
                break;
            }
            let Some(name) = self.script.token_text(cursor) else {
                break;
            };
            segments.push((cursor, name.to_owned()));
            let next = cursor + 1;
            if self.script.is_punct(next, '.') {
                cursor = next + 1;
                continue;
            }
            if self.script.is_punct(next, '[') {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "`this.data` uses a computed access that cannot be proven static",
                );
            } else if self.script.is_punct(next, '?') {
                self.complete = false;
                push_reason(
                    &mut self.problems,
                    "`this.data` uses optional chaining that cannot be proven static",
                );
            }
            break;
        }
        if segments.is_empty() {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "`this.data` has no statically readable segments",
            );
            return;
        }
        if segments.len() < target.len() {
            self.complete = false;
            push_reason(
                &mut self.problems,
                "an ancestor of the data path is used as a whole value",
            );
            return;
        }
        for (index, segment) in target.iter().enumerate() {
            if segments.get(index).map(|(_, name)| name.as_str()) != Some(segment.as_str()) {
                return;
            }
        }
        let (token_index, _) = segments[target.len() - 1];
        if let Some(token) = self.token(token_index) {
            self.add_reference(
                token,
                role_at(self.script, token_index, self.token_start(token_index)),
            );
        }
    }

    fn token(&self, index: usize) -> Option<SymbolTokenFact> {
        let start = self.script.token_start(index)?;
        let end = self.script.token_end(index)?;
        let expected = self.script.token_text(index)?.to_owned();
        let range = self
            .script
            .source_range(SourceRange::new(
                u32::try_from(start).ok()?,
                u32::try_from(end).ok()?,
            ))
            .ok()?;
        Some(SymbolTokenFact {
            path: self.file_path.to_owned(),
            range,
            expected_text: Some(expected),
        })
    }

    fn token_start(&self, index: usize) -> usize {
        self.script.token_start(index).unwrap_or(0)
    }
}

fn role_at(script: &Script<'_>, index: usize, start: usize) -> SymbolReferenceRole {
    let Some(end) = script.token_end(index) else {
        return SymbolReferenceRole::Read;
    };
    let suffix = script.text.get(end..).unwrap_or("").trim_start();
    if suffix.starts_with("++") || suffix.starts_with("--") {
        return SymbolReferenceRole::Write;
    }
    if suffix.starts_with('=') && !suffix.starts_with("==") && !suffix.starts_with("=>") {
        return SymbolReferenceRole::Write;
    }
    if [
        "+=", "-=", "*=", "/=", "%=", "**=", "&&=", "||=", "??=", "&=", "|=", "^=", "<<=", ">>=",
        ">>>=",
    ]
    .iter()
    .any(|operator| suffix.starts_with(operator))
    {
        return SymbolReferenceRole::Write;
    }
    let prefix = script.text.get(..start).unwrap_or("").trim_end();
    if prefix.ends_with("++") || prefix.ends_with("--") {
        return SymbolReferenceRole::Write;
    }
    if index > 0 && script.is_ident(index - 1, "delete") {
        return SymbolReferenceRole::Write;
    }
    SymbolReferenceRole::Read
}

fn collect_aliases(script: &Script<'_>) -> Aliases {
    let mut names: BTreeMap<String, AliasInfo> = BTreeMap::new();
    loop {
        let mut changed = false;
        for index in 0..script.tokens.len() {
            let Some(declaration) = script.token_text(index) else {
                continue;
            };
            if !matches!(declaration, "const" | "let" | "var") {
                continue;
            }
            let Some(name) = script.token_text(index + 1) else {
                continue;
            };
            if script.kind(index + 1) != Some(TokenKind::Identifier)
                || !script.is_punct(index + 2, '=')
            {
                continue;
            }
            let Some(initializer) = script.token_text(index + 3) else {
                continue;
            };
            if script.kind(index + 3) != Some(TokenKind::Identifier) {
                continue;
            }
            let from_this = initializer == "this" && initializer_is_exact(script, index + 4);
            let from_alias = initializer != "this"
                && names.contains_key(initializer)
                && initializer_is_exact(script, index + 4);
            if !from_this && !from_alias {
                continue;
            }
            if !names.contains_key(name) {
                names.insert(
                    name.to_owned(),
                    AliasInfo {
                        const_like: declaration == "const",
                    },
                );
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Any later assignment or increment invalidates the simple alias proof.
    for index in 0..script.tokens.len() {
        let Some(name) = script.token_text(index) else {
            continue;
        };
        let Some(info) = names.get_mut(name) else {
            continue;
        };
        if script.kind(index) != Some(TokenKind::Identifier) {
            continue;
        }
        let previous = index
            .checked_sub(1)
            .and_then(|previous| script.token_text(previous));
        if matches!(previous, Some("const" | "let" | "var")) {
            continue;
        }
        if script.is_punct(index + 1, '=')
            || (script.is_punct(index + 1, '+') && script.is_punct(index + 2, '+'))
            || (script.is_punct(index + 1, '-') && script.is_punct(index + 2, '-'))
        {
            info.const_like = false;
        }
    }
    Aliases { names }
}

fn initializer_is_exact(script: &Script<'_>, index: usize) -> bool {
    index >= script.tokens.len()
        || script.is_punct(index, ';')
        || script.is_punct(index, ',')
        || script.is_punct(index, ')')
        || script.is_punct(index, '}')
}

fn collect_scopes(script: &Script<'_>, registration: &ParsedRegistration) -> ThisScopes {
    let mut scopes = ThisScopes::default();
    scopes.bodies.extend(method_bodies(script));
    scopes.bodies.extend(function_bodies(script));
    scopes.classes = class_bodies(script);
    match registration.fact.kind {
        RegistrationKind::Page => {
            for member in &registration.members {
                add_function_scope(script, member, &mut scopes);
            }
        }
        RegistrationKind::Behavior => {
            for member in &registration.members {
                add_function_scope(script, member, &mut scopes);
            }
            if let Some(methods) = super::scanner::member_last(&registration.members, "methods")
                && let Some((open, close)) = object_bounds(script, methods)
            {
                for member in scan_members(script, open + 1, close) {
                    add_function_scope(script, &member, &mut scopes);
                }
            }
        }
        RegistrationKind::Component => {
            for member in &registration.members {
                add_function_scope(script, member, &mut scopes);
            }
            for container in ["methods", "lifetimes", "pageLifetimes", "observers"] {
                if let Some(container) =
                    super::scanner::member_last(&registration.members, container)
                    && let Some((open, close)) = object_bounds(script, container)
                {
                    for member in scan_members(script, open + 1, close) {
                        add_function_scope(script, &member, &mut scopes);
                    }
                }
            }
            if let Some(properties) =
                super::scanner::member_last(&registration.members, "properties")
                && let Some((open, close)) = object_bounds(script, properties)
            {
                for property in scan_members(script, open + 1, close) {
                    if let Some((option_open, option_close)) = object_bounds(script, &property)
                        && let Some(observer) = super::scanner::member_last(
                            &scan_members(script, option_open + 1, option_close),
                            "observer",
                        )
                    {
                        add_function_scope(script, observer, &mut scopes);
                    }
                }
            }
        }
    }
    scopes
}

fn add_function_scope(
    script: &Script<'_>,
    member: &super::scanner::Member,
    scopes: &mut ThisScopes,
) {
    let Some((start, end, root)) = function_member_range(script, member) else {
        return;
    };
    scopes.allowed.push((start, end));
    if let Some(root) = root {
        scopes.roots.push(root);
    }
}

fn function_member_range(
    script: &Script<'_>,
    member: &super::scanner::Member,
) -> Option<FunctionMemberRange> {
    let literal = match member.kind {
        MemberKind::Method => true,
        MemberKind::Property => {
            let mut index = member.value_start;
            if script.is_ident(index, "async") {
                index += 1;
            }
            script.is_ident(index, "function")
                || script.is_punct(index, '(')
                || (script.kind(index) == Some(TokenKind::Identifier)
                    && script.is_punct(index + 1, '=')
                    && script.is_punct(index + 2, '>'))
        }
        _ => false,
    };
    if !literal || member.value_start >= member.value_end {
        return None;
    }
    let start = script.token_start(member.value_start)?;
    let end = script.token_end(member.value_end.saturating_sub(1))?;
    let root = if member.kind == MemberKind::Method {
        method_body_after(script, member.value_start, member.value_end)
    } else {
        let mut index = member.value_start;
        if script.is_ident(index, "async") {
            index += 1;
        }
        if script.is_ident(index, "function") {
            function_body_from(script, index, member.value_end)
        } else {
            arrow_body(script, index, member.value_end)
        }
    };
    Some((start, end, root))
}

fn method_body_after(script: &Script<'_>, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut index = start;
    while index < end && !script.is_punct(index, '(') {
        index += 1;
    }
    if index >= end {
        return None;
    }
    let close = script.close_of(index).ok()?;
    let mut cursor = close + 1;
    while cursor < end {
        match script.kind(cursor) {
            Some(TokenKind::Punctuator('{')) => {
                let body_close = script.close_of(cursor).ok()?;
                return Some((script.token_start(cursor)?, script.token_end(body_close)?));
            }
            Some(TokenKind::Punctuator(';' | '=')) => return None,
            _ => cursor += 1,
        }
    }
    None
}

fn function_body_from(script: &Script<'_>, function: usize, end: usize) -> Option<(usize, usize)> {
    let mut index = function + 1;
    while index < end && !script.is_punct(index, '(') {
        index += 1;
    }
    if index >= end {
        return None;
    }
    let close = script.close_of(index).ok()?;
    let mut cursor = close + 1;
    while cursor < end {
        match script.kind(cursor) {
            Some(TokenKind::Punctuator('{')) => {
                let body_close = script.close_of(cursor).ok()?;
                return Some((script.token_start(cursor)?, script.token_end(body_close)?));
            }
            Some(TokenKind::Punctuator(';' | '=')) => return None,
            _ => cursor += 1,
        }
    }
    None
}

fn arrow_body(script: &Script<'_>, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut index = start;
    while index + 1 < end {
        if script.is_punct(index, '=') && script.is_punct(index + 1, '>') {
            let body = index + 2;
            if script.is_punct(body, '{') {
                let close = script.close_of(body).ok()?;
                return Some((script.token_start(body)?, script.token_end(close)?));
            }
            return None;
        }
        index += 1;
    }
    None
}

fn method_bodies(script: &Script<'_>) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    for index in 0..script.tokens.len() {
        let Some(text) = script.token_text(index) else {
            continue;
        };
        if matches!(
            text,
            "if" | "for" | "while" | "switch" | "catch" | "with" | "function" | "return"
        ) {
            continue;
        }
        if !matches!(
            script.kind(index),
            Some(TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number)
        ) || !script.is_punct(index + 1, '(')
        {
            continue;
        }
        let Ok(close) = script.close_of(index + 1) else {
            continue;
        };
        let mut cursor = close + 1;
        while cursor < script.tokens.len() {
            match script.kind(cursor) {
                Some(TokenKind::Punctuator('{')) => {
                    if let Ok(body_close) = script.close_of(cursor)
                        && let (Some(start), Some(end)) =
                            (script.token_start(cursor), script.token_end(body_close))
                    {
                        bodies.push((start, end));
                    }
                    break;
                }
                Some(TokenKind::Punctuator(';' | '=' | ',' | ')' | ']')) => break,
                _ => cursor += 1,
            }
        }
    }
    bodies
}

fn function_bodies(script: &Script<'_>) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    for index in 0..script.tokens.len() {
        if !script.is_ident(index, "function") {
            continue;
        }
        if !matches!(
            script.kind(index + 1),
            Some(TokenKind::Identifier | TokenKind::Punctuator('*' | '('))
        ) {
            continue;
        }
        let mut cursor = index + 1;
        if script.is_punct(cursor, '*') {
            cursor += 1;
        }
        if script.kind(cursor) == Some(TokenKind::Identifier) {
            cursor += 1;
        }
        while cursor < script.tokens.len() && !script.is_punct(cursor, '(') {
            if script.is_punct(cursor, ';') || script.is_punct(cursor, '=') {
                break;
            }
            cursor += 1;
        }
        let Some(close) = script.close_of(cursor).ok() else {
            continue;
        };
        let mut body = close + 1;
        while body < script.tokens.len() {
            match script.kind(body) {
                Some(TokenKind::Punctuator('{')) => {
                    if let Ok(body_close) = script.close_of(body)
                        && let (Some(start), Some(end)) =
                            (script.token_start(body), script.token_end(body_close))
                    {
                        bodies.push((start, end));
                    }
                    break;
                }
                Some(TokenKind::Punctuator(';' | '=' | ',' | ')')) => break,
                _ => body += 1,
            }
        }
    }
    bodies
}

fn class_bodies(script: &Script<'_>) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    for index in 0..script.tokens.len() {
        if !script.is_ident(index, "class") {
            continue;
        }
        if !matches!(
            script.kind(index + 1),
            Some(TokenKind::Identifier | TokenKind::Punctuator('{'))
        ) && !script.is_ident(index + 1, "extends")
        {
            continue;
        }
        let mut cursor = index + 1;
        while cursor < script.tokens.len() {
            match script.kind(cursor) {
                Some(TokenKind::Punctuator('{')) => {
                    if let Ok(close) = script.close_of(cursor)
                        && let (Some(start), Some(end)) =
                            (script.token_start(cursor), script.token_end(close))
                    {
                        bodies.push((start, end));
                    }
                    break;
                }
                Some(TokenKind::Punctuator(';' | '=' | ',' | ')')) => break,
                _ => cursor += 1,
            }
        }
    }
    bodies
}
