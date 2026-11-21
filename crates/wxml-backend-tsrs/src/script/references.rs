//! Conservative syntactic references for data paths and event handlers.
//!
//! This is not a TypeScript checker. It only proves `this.data.<path>` and
//! `this.<handler>` chains inside function bodies that the registration binds to
//! the owner, plus simple lexical `const that = this` aliases. Anything that
//! could hide the target (computed access, destructuring, whole-value use,
//! mutable alias, another receiver, another script's `setData`) makes the scan
//! partial; no guessed token is published.

use wxml_types::{MAX_SYMBOL_ENTRIES, SymbolReferenceRole};

use crate::input::SourceFile;
use crate::shadow::expression::TokenKind;
use crate::shadow::mapping::SourceRange;

use super::registration::ParsedRegistration;
use super::scanner::Script;
use super::symbol_facts::{SymbolReferenceFact, SymbolTokenFact, push_reason};
use super::this_scope::{
    Aliases, ThisScopes, collect_aliases, collect_scopes, destructures_from_this,
};

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
        if !self.aliases.is_empty() {
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
