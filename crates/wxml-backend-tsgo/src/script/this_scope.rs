//! Page-bound `this` scopes and simple `this` aliases.
//!
//! The reference scanner only trusts `this` chains inside function bodies the
//! registration binds to the owner. A nested regular function, class method or
//! nested object method is deliberately outside these scopes.

use std::collections::BTreeMap;

use crate::shadow::expression::TokenKind;

use super::registration::{ParsedRegistration, RegistrationKind};
use super::scanner::{MemberKind, Script, object_bounds, scan_members};

#[derive(Debug, Clone, Copy)]
pub(super) struct AliasInfo {
    pub(super) const_like: bool,
}

#[derive(Debug, Default)]
pub(super) struct Aliases {
    names: BTreeMap<String, AliasInfo>,
}

impl Aliases {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    pub(crate) fn get(&self, name: &str) -> Option<AliasInfo> {
        self.names.get(name).copied()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

type BodyRange = (usize, usize);
type FunctionMemberRange = (usize, usize, Option<BodyRange>);

#[derive(Debug, Default)]
pub(super) struct ThisScopes {
    allowed: Vec<(usize, usize)>,
    bodies: Vec<(usize, usize)>,
    roots: Vec<(usize, usize)>,
    classes: Vec<(usize, usize)>,
}

impl ThisScopes {
    pub(crate) fn page_bound(&self, offset: usize) -> bool {
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

pub(super) fn destructures_from_this(script: &Script<'_>, this_index: usize) -> bool {
    this_index >= 2 && script.is_punct(this_index - 1, '=') && script.is_punct(this_index - 2, '}')
}

pub(super) fn collect_aliases(script: &Script<'_>) -> Aliases {
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

pub(super) fn collect_scopes(script: &Script<'_>, registration: &ParsedRegistration) -> ThisScopes {
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
