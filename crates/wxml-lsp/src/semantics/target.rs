//! WXML symbol identities, exact tokens and reference coverage.
//!
//! Every identity starts from the parsed syntax tree or from T's template
//! analysis; script facts are added later through the Node symbol query. The
//! module distinguishes data paths, loop locals, component properties, event
//! handlers, tag pairs, template names and style classes so a same-named token
//! in another context is never edited.

use std::collections::{BTreeMap, BTreeSet};

use wxml_project::{ProjectPath, StyleScope};
use wxml_syntax::expr::{Expr, parse_expression};
use wxml_syntax::{AstNode, AttributeValue, Element, SyntaxTree};
use wxml_text::{SourceRange, TextRange};
use wxml_types::{LocalRole, RootKind};

use super::facts::{
    LoopIdentity, QueryDoc, ViewFacts, effective_binding, effective_loop_binding, load_project_doc,
    loop_for_binding, project_views, range_text, scan_expression,
};
use crate::project_host::ViewComponent;
use crate::server::Server;

/// Exact token style used to derive a per-target replacement.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenStyle {
    /// A JavaScript/TypeScript identifier or a bare WXML binding name.
    Identifier,
    /// A WXML attribute-name token (component property).
    Attribute,
    /// A `<template name>`/`<template is>` value token.
    Template,
    /// A CSS class-name fragment.
    Class,
    /// A custom-component tag name.
    Tag,
    /// A path segment inside a quoted script key.
    PathSegment,
}

/// One exact source token collected for a symbol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolToken {
    /// Owning file.
    pub(crate) file: wxml_types::FileId,
    /// Exact byte range.
    pub(crate) range: SourceRange,
    /// Replacement grammar derived from the token's source context.
    pub(crate) style: TokenStyle,
}

/// The target of one references/rename request.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Target {
    /// A data-root path such as `name` or `user.name`.
    Data {
        /// Owning script.
        subject: wxml_types::FileId,
        /// Registration root.
        root: RootKind,
        /// Root-to-leaf path.
        path: Vec<String>,
        /// Token under the cursor.
        token: SourceRange,
    },
    /// A component property declared in another script and used as an attribute.
    Property {
        /// Component script declaring the property.
        subject: wxml_types::FileId,
        /// Declared name in the component script.
        declared: String,
        /// Tag name in the current view.
        tag: String,
        /// Token under the cursor.
        token: SourceRange,
        /// Component whose facts produced the declaration.
        component: ProjectPath,
    },
    /// A statically bound event handler.
    Event {
        /// Owning script.
        subject: wxml_types::FileId,
        /// Handler name.
        name: String,
        /// Token under the cursor.
        token: SourceRange,
    },
    /// A `wx:for` item/index binding.
    Loop {
        /// Binding name.
        name: String,
        /// Scope identity.
        identity: LoopIdentity,
        /// Loop index inside the document.
        loop_index: usize,
        /// Explicit declaration token.
        declaration: Option<SourceRange>,
        /// Placeholder token for prepareRename.
        placeholder: SourceRange,
    },
    /// A custom-component tag pair.
    Tag {
        /// Tag name.
        name: String,
        /// Start-tag name token.
        start: SourceRange,
        /// End-tag name token.
        end: SourceRange,
    },
    /// A static template name (`<template name>` / `<template is>`).
    Template {
        /// Template name.
        name: String,
        /// Token under the cursor.
        token: SourceRange,
    },
    /// A style class used in a `class` attribute.
    Class {
        /// Class name.
        name: String,
        /// Token under the cursor.
        token: SourceRange,
    },
}

impl Target {
    /// Returns the exact token shown by `prepareRename`.
    pub(crate) fn placeholder(&self) -> SourceRange {
        match self {
            Self::Data { token, .. }
            | Self::Property { token, .. }
            | Self::Event { token, .. }
            | Self::Template { token, .. }
            | Self::Class { token, .. } => *token,
            Self::Loop {
                declaration,
                placeholder,
                ..
            } => declaration.unwrap_or(*placeholder),
            Self::Tag { start, .. } => *start,
        }
    }

    /// Returns the placeholder text for `prepareRename`.
    pub(crate) fn placeholder_text(&self, doc: &QueryDoc) -> Option<String> {
        if let Self::Loop {
            name, declaration, ..
        } = self
        {
            if declaration.is_none() {
                return Some(name.clone());
            }
        }
        let range = self.placeholder();
        range_text(doc, range).map(str::to_owned)
    }
}

/// One collected symbol token plus its style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Tokens {
    /// Collected tokens in source order.
    pub(crate) items: Vec<SymbolToken>,
    /// `true` only when every occurrence is covered.
    pub(crate) complete: bool,
    /// Short path-free reason for incomplete coverage.
    pub(crate) reason: Option<&'static str>,
}

impl Tokens {
    pub(crate) const fn new() -> Self {
        Self {
            items: Vec::new(),
            complete: true,
            reason: None,
        }
    }

    const fn reject(&mut self, reason: &'static str) {
        self.complete = false;
        if self.reason.is_none() {
            self.reason = Some(reason);
        }
    }

    pub(crate) fn push(
        &mut self,
        file: &wxml_types::FileId,
        range: SourceRange,
        style: TokenStyle,
    ) {
        if range.is_empty() {
            return;
        }
        if !self
            .items
            .iter()
            .any(|token| token.file == *file && token.range == range)
        {
            self.items.push(SymbolToken {
                file: file.clone(),
                range,
                style,
            });
        }
    }
}

/// Converts a camelCase property name to its WXML kebab spelling.
pub(crate) fn kebab_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_uppercase() {
            output.push('-');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

/// Converts a kebab-case spelling to camelCase; `None` for invalid input.
pub(crate) fn camel_case(name: &str) -> Option<String> {
    let mut output = String::new();
    for (index, part) in name.split('-').enumerate() {
        if part.is_empty() {
            return None;
        }
        if index == 0 {
            output.push_str(part);
        } else {
            let mut chars = part.chars();
            let first = chars.next()?;
            output.push(first.to_ascii_uppercase());
            output.push_str(chars.as_str());
        }
    }
    (!output.is_empty()).then_some(output)
}

/// Returns `true` when `text` is the component property's kebab or camel form.
fn property_spelling(declared: &str, text: &str) -> bool {
    text == declared || (text.contains('-') && text == kebab_case(declared))
}

/// Resolves the symbol target under one byte offset.
pub(crate) fn target_at(server: &Server, facts: &ViewFacts, offset: u32) -> Option<Target> {
    let doc = &facts.doc;
    if let Some(target) = loop_declaration_at(facts, offset) {
        return Some(target);
    }
    if let Some(target) = tag_or_attribute_at(server, facts, offset) {
        return Some(target);
    }
    if let Some(target) = directive_expression_at(facts, offset) {
        return Some(target);
    }
    if let Some(target) = mustache_target(facts, offset) {
        return Some(target);
    }
    let _ = (doc, server);
    None
}

/// Resolves a `wx:for-item`/`wx:for-index` declaration token.
fn loop_declaration_at(facts: &ViewFacts, offset: u32) -> Option<Target> {
    for (index, loop_info) in facts.loops.iter().enumerate() {
        for (role, binding) in [
            (LocalRole::Item, &loop_info.item),
            (LocalRole::Index, &loop_info.index),
        ] {
            let Some(declaration) = binding.declaration else {
                continue;
            };
            if declaration.start() <= offset && offset <= declaration.end() {
                return Some(Target::Loop {
                    name: binding.name.clone(),
                    identity: LoopIdentity {
                        file: loop_info.file.clone(),
                        source_start: loop_info.source_range.start(),
                        role,
                    },
                    loop_index: index,
                    declaration: Some(declaration),
                    placeholder: declaration,
                });
            }
        }
    }
    None
}

/// Resolves a tag name, component property or event/class/template value.
fn tag_or_attribute_at(server: &Server, facts: &ViewFacts, offset: u32) -> Option<Target> {
    let mut found = None;
    for_each_element(facts.doc.tree(), &mut |element| {
        if found.is_some() {
            return;
        }
        let name = element.name().unwrap_or_default();
        if let Some(range) = element.name_range() {
            if range.contains(offset) {
                if let Some(target) = tag_target(facts, name, offset) {
                    found = Some(target);
                }
                return;
            }
        }
        for attribute in element.attributes() {
            let Some(attribute_name) = attribute.name() else {
                continue;
            };
            let Some(name_range) = attribute.name_range() else {
                continue;
            };
            if name_range.contains(offset) {
                if attribute_name == "class" || attribute_name == "id" {
                    continue;
                }
                if let Some(target) = property_target(facts, name, attribute_name, name_range) {
                    found = Some(target);
                }
                return;
            }
            let Some(value) = attribute.value() else {
                continue;
            };
            let value_range = value.text_range();
            if !(value_range.start() <= offset && offset < value_range.end()) {
                continue;
            }
            if name == "template" && matches!(attribute_name, "name" | "is") {
                if let Some(target) = template_target(value, offset) {
                    found = Some(target);
                    return;
                }
            }
            if attribute_name == "class" {
                if let Some(target) = class_target(server, facts, value, offset) {
                    found = Some(target);
                    return;
                }
            }
            if crate::completion::catalog::is_event_attribute(attribute_name) {
                if let Some(event) = facts.analysis.events.iter().find(|event| {
                    event.handler_range.start() <= offset && offset <= event.handler_range.end()
                }) {
                    found = Some(Target::Event {
                        subject: facts.target.as_ref().map_or_else(
                            || facts.doc.file.clone(),
                            |target| target.script_file.clone(),
                        ),
                        name: event.handler.clone(),
                        token: event.handler_range,
                    });
                }
                return;
            }
        }
    });
    found
}

fn tag_target(facts: &ViewFacts, name: &str, offset: u32) -> Option<Target> {
    if crate::metadata::is_builtin_tag(name) {
        return None;
    }
    let component = facts
        .components
        .iter()
        .find(|component| component.name == name)?;
    let _ = component;
    let (first, second) = facts.doc.structure.linked_editing_range(offset)?;
    Some(Target::Tag {
        name: name.to_owned(),
        start: SourceRange::new(first),
        end: SourceRange::new(second),
    })
}

fn property_target(
    facts: &ViewFacts,
    tag: &str,
    attribute: &str,
    name_range: TextRange,
) -> Option<Target> {
    let component = facts
        .components
        .iter()
        .find(|component| component.name == tag)?;
    let property = component
        .property_facts
        .properties
        .iter()
        .find(|property| property_spelling(&property.name, attribute))?;
    let subject = server_file(&property.script, facts)?;
    Some(Target::Property {
        subject,
        declared: property.name.clone(),
        tag: tag.to_owned(),
        token: SourceRange::new(name_range),
        component: component
            .component
            .clone()
            .or_else(|| component.wxml.clone())?,
    })
}

fn server_file(path: &ProjectPath, facts: &ViewFacts) -> Option<wxml_types::FileId> {
    let project = facts.target.as_ref().map_or_else(
        || facts.doc.file.project().clone(),
        |target| target.script_file.project().clone(),
    );
    wxml_types::FileId::new(project, path.as_str()).ok()
}

fn template_target(value: AttributeValue<'_>, offset: u32) -> Option<Target> {
    let literal = value.literal_text()?.trim();
    if literal.is_empty() || !is_template_name(literal) {
        return None;
    }
    let token = literal_token(value, literal)?;
    if !(token.start() <= offset && offset <= token.end()) {
        return None;
    }
    Some(Target::Template {
        name: literal.to_owned(),
        token,
    })
}

fn is_template_name(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Public template-name grammar used by rename validation.
pub(crate) fn is_template_name_public(text: &str) -> bool {
    is_template_name(text)
}

/// Public class-name grammar used by rename validation.
pub(crate) fn is_class_name(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Returns `true` when `name` is already used in a `class` attribute.
pub(crate) fn class_in_use(facts: &ViewFacts, name: &str) -> bool {
    let mut found = false;
    for doc in std::iter::once(&facts.doc).chain(facts.includes.iter()) {
        for_each_element(doc.tree(), &mut |element| {
            let Some(attribute) = element.attribute("class") else {
                return;
            };
            let Some(value) = attribute.value() else {
                return;
            };
            for child in value.children() {
                let AstNode::Text(text) = child else {
                    continue;
                };
                if text.text().split_whitespace().any(|token| token == name) {
                    found = true;
                }
            }
        });
    }
    found
}

/// Returns the trimmed value token of a literal attribute.
pub(crate) fn literal_token(value: AttributeValue<'_>, trimmed: &str) -> Option<SourceRange> {
    let literal = value.literal_text()?;
    let leading = literal.len().saturating_sub(literal.trim_start().len());
    let quote = usize::from(value.is_quoted());
    let range = value.text_range();
    let start = range
        .start()
        .checked_add(u32::try_from(quote + leading).ok()?)?;
    let end = start.checked_add(u32::try_from(trimmed.len()).ok()?)?;
    SourceRange::from_bounds(start, end).ok()
}

fn class_target(
    server: &Server,
    facts: &ViewFacts,
    value: AttributeValue<'_>,
    offset: u32,
) -> Option<Target> {
    let token = class_token_at(&facts.doc, value, offset)?;
    let name = range_text(&facts.doc, token)?.to_owned();
    let style = server.host.style_facts_for(&facts.doc.file)?;
    if style.definitions(&name).is_empty() {
        return None;
    }
    Some(Target::Class { name, token })
}

/// Extracts the class-name token under `offset` from any text child.
fn class_token_at(doc: &QueryDoc, value: AttributeValue<'_>, offset: u32) -> Option<SourceRange> {
    for child in value.children() {
        let AstNode::Text(text) = child else {
            continue;
        };
        let range = text.text_range();
        let content = text.text();
        let bytes = content.as_bytes();
        let mut index = 0_usize;
        while index < bytes.len() {
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if start >= index {
                break;
            }
            let token_start = range.start().checked_add(u32::try_from(start).ok()?)?;
            let token_end = range.start().checked_add(u32::try_from(index).ok()?)?;
            if token_start <= offset && offset <= token_end {
                let token = SourceRange::from_bounds(token_start, token_end).ok()?;
                let name = range_text(doc, token)?;
                if !name.is_empty() && !name.contains("{{") {
                    return Some(token);
                }
            }
        }
    }
    None
}

/// Resolves an identifier in `wx:for`, `wx:if` or `wx:elif`.
fn directive_expression_at(facts: &ViewFacts, offset: u32) -> Option<Target> {
    let mut found = None;
    for_each_element(facts.doc.tree(), &mut |element| {
        if found.is_some() {
            return;
        }
        for attribute_name in ["wx:for", "wx:if", "wx:elif"] {
            let Some(attribute) = element.attribute(attribute_name) else {
                continue;
            };
            let Some(value) = attribute.value() else {
                continue;
            };
            let Some(inner) = mustache_inner(value) else {
                continue;
            };
            if !(inner.start() <= offset && offset < inner.end()) {
                continue;
            }
            let scope = directive_scope(
                facts,
                element,
                attribute_name == "wx:if" || attribute_name == "wx:elif",
            );
            let Some(expr) = expression_at(facts, inner) else {
                continue;
            };
            let scope = BindingScope::Loops(scope);
            found = target_in_expression(facts, &expr, offset, &scope);
            if matches!(found, Some(Target::Data { .. }))
                && inside_template_definition(
                    facts.doc.tree(),
                    SourceRange::new(element.text_range()),
                )
            {
                found = None;
            }
        }
    });
    found
}

/// Resolves an identifier in a fragment expression.
fn mustache_target(facts: &ViewFacts, offset: u32) -> Option<Target> {
    for fragment in &facts.analysis.fragments {
        if !(fragment.wxml_range.start() <= offset && offset <= fragment.wxml_range.end()) {
            continue;
        }
        let Some(expr) = expression_for_fragment(fragment) else {
            continue;
        };
        let scope = fragment.bindings.clone();
        return target_in_expression(facts, &expr, offset, &BindingScope::Fragment(&scope));
    }
    None
}

/// A scope for resolving one expression's loop locals.
enum BindingScope<'a> {
    /// A fragment's analyzer binding chain.
    Fragment(&'a [wxml_types::LocalBinding]),
    /// Ancestor loops for one directive attribute, innermost last.
    Loops(Vec<usize>),
}

fn target_in_expression(
    facts: &ViewFacts,
    expr: &Expr,
    offset: u32,
    scope: &BindingScope<'_>,
) -> Option<Target> {
    let scan = scan_expression(&facts.doc, expr);
    for chain in &scan.chains {
        for (index, (name, range)) in chain.segments.iter().enumerate() {
            if !(range.start() <= offset && offset <= range.end()) {
                continue;
            }
            if let Some(binding) = local_binding(facts, scope, name) {
                if index == 0 {
                    return Some(binding);
                }
                return None;
            }
            if facts
                .semantics
                .wxs
                .iter()
                .any(|block| block.module.as_deref() == Some(String::as_str(name)))
            {
                return None;
            }
            if name == "this"
                || matches!(
                    String::as_str(name),
                    "true" | "false" | "null" | "undefined"
                )
            {
                return None;
            }
            let path = chain
                .segments
                .iter()
                .take(index + 1)
                .map(|(name, _)| name.clone())
                .collect();
            return Some(Target::Data {
                subject: facts.target.as_ref().map_or_else(
                    || facts.doc.file.clone(),
                    |target| target.script_file.clone(),
                ),
                root: facts.root_kind(),
                path,
                token: *range,
            });
        }
    }
    None
}

fn local_binding(facts: &ViewFacts, scope: &BindingScope<'_>, name: &str) -> Option<Target> {
    match scope {
        BindingScope::Fragment(bindings) => {
            let binding = effective_binding(bindings, name)?;
            let index = loop_for_binding(&facts.loops, binding)?;
            loop_target(facts, index, binding.role)
        }
        BindingScope::Loops(parents) => {
            let (index, role) = effective_loop_binding(&facts.loops, parents, name)?;
            loop_target(facts, index, role)
        }
    }
}

fn loop_target(facts: &ViewFacts, index: usize, role: LocalRole) -> Option<Target> {
    let info = facts.loops.get(index)?;
    let binding = match role {
        LocalRole::Item => &info.item,
        LocalRole::Index => &info.index,
        _ => return None,
    };
    Some(Target::Loop {
        name: binding.name.clone(),
        identity: LoopIdentity {
            file: info.file.clone(),
            source_start: info.source_range.start(),
            role,
        },
        loop_index: index,
        declaration: binding.declaration,
        placeholder: binding
            .declaration
            .or(binding.attribute)
            .unwrap_or(info.element_range),
    })
}

/// Returns the ancestor loop chain for one element (innermost last).
fn directive_scope(facts: &ViewFacts, element: Element<'_>, include_self: bool) -> Vec<usize> {
    let range = element.text_range();
    let mut chain: Vec<usize> = facts
        .loops
        .iter()
        .enumerate()
        .filter(|(_, info)| {
            info.element_range.start() <= range.start()
                && info.element_range.end() >= range.end()
                && (include_self || info.element_range.start() != range.start())
        })
        .map(|(index, _)| index)
        .collect();
    chain.sort_by_key(|index| {
        facts
            .loops
            .get(*index)
            .map_or(0, |info| info.source_range.start())
    });
    chain
}

fn mustache_inner(value: AttributeValue<'_>) -> Option<SourceRange> {
    let mut mustaches = value.mustaches();
    let mustache = mustaches.next()?;
    mustaches.next()?;
    mustache.inner_range().map(SourceRange::new)
}

fn expression_at(facts: &ViewFacts, inner: SourceRange) -> Option<Expr> {
    let text = range_text(&facts.doc, inner)?;
    let parsed = parse_expression(text, inner.start());
    if !parsed.errors.is_empty() {
        return None;
    }
    Some(parsed.expr)
}

fn expression_for_fragment(fragment: &wxml_types::CheckFragment) -> Option<Expr> {
    let parsed = parse_expression(&fragment.expr_text, fragment.wxml_range.start());
    if !parsed.errors.is_empty() {
        return None;
    }
    Some(parsed.expr)
}

/* ------------------------------------------------------------------ traversals */

/// Visits every element in the tree in source order.
pub(crate) fn for_each_element(tree: &SyntaxTree, visit: &mut impl FnMut(Element<'_>)) {
    fn walk(node: AstNode<'_>, visit: &mut impl FnMut(Element<'_>)) {
        if let AstNode::Element(element) = node {
            visit(element);
            for child in element.children() {
                walk(child, visit);
            }
        }
    }
    for child in tree.document().children() {
        walk(child, visit);
    }
}

/* -------------------------------------------------------------------- coverage */

/// Collects WXML references of one data-root path.
pub(crate) fn collect_data_refs(facts: &ViewFacts, path: &[String]) -> Tokens {
    let mut tokens = Tokens::new();
    if path.is_empty() {
        tokens.reject("data path is empty");
        return tokens;
    }
    if !facts.includes.is_empty() || !facts.includes_complete {
        tokens.reject("static includes hide caller data references");
    }
    if facts.analysis.truncated {
        tokens.reject("template fragment budget was reached");
    }
    for unsupported in &facts.analysis.unsupported {
        if matches!(
            unsupported.kind(),
            wxml_types::UnsupportedKind::MalformedExpression
                | wxml_types::UnsupportedKind::InvalidRange
                | wxml_types::UnsupportedKind::FragmentLimit
                | wxml_types::UnsupportedKind::DynamicForBinding
        ) {
            tokens.reject("an expression could not be analyzed completely");
        }
    }
    for fragment in &facts.analysis.fragments {
        let Some(expr) = expression_for_fragment(fragment) else {
            tokens.reject("an expression could not be parsed completely");
            continue;
        };
        if fragment
            .bindings
            .iter()
            .any(|binding| binding.name == path[0])
        {
            continue;
        }
        collect_path_in_expression(facts, &expr, path, &mut tokens);
    }
    collect_directive_path_refs(facts, path, &mut tokens);
    tokens
}

fn collect_path_in_expression(
    facts: &ViewFacts,
    expr: &Expr,
    path: &[String],
    tokens: &mut Tokens,
) {
    let scan = scan_expression(&facts.doc, expr);
    for chain in &scan.chains {
        if chain.segments.len() < path.len() {
            continue;
        }
        if !chain
            .segments
            .iter()
            .take(path.len())
            .zip(path.iter())
            .all(|((name, _), expected)| name == expected)
        {
            continue;
        }
        let (_, range) = &chain.segments[path.len() - 1];
        tokens.push(&facts.doc.file, *range, TokenStyle::Identifier);
        if chain.shorthand {
            tokens.reject("an object shorthand cannot be renamed safely");
        }
    }
    for access in &scan.computed {
        if access.prefix.len() + 1 != path.len() {
            continue;
        }
        if !access
            .prefix
            .iter()
            .zip(path.iter())
            .all(|(name, expected)| name == expected)
        {
            continue;
        }
        let expected = &path[path.len() - 1];
        match access.key.as_deref() {
            Some(key) if key == expected => {
                tokens.push(&facts.doc.file, access.range, TokenStyle::PathSegment);
                tokens.reject("a computed string access cannot be renamed safely");
            }
            None => tokens.reject("a dynamic computed access may reference this field"),
            _ => {}
        }
    }
}

fn collect_directive_path_refs(facts: &ViewFacts, path: &[String], tokens: &mut Tokens) {
    for_each_element(facts.doc.tree(), &mut |element| {
        if inside_template_definition(facts.doc.tree(), SourceRange::new(element.text_range())) {
            return;
        }
        for attribute_name in ["wx:for", "wx:if", "wx:elif"] {
            let Some(attribute) = element.attribute(attribute_name) else {
                continue;
            };
            let Some(value) = attribute.value() else {
                continue;
            };
            let Some(inner) = mustache_inner(value) else {
                continue;
            };
            let Some(expr) = expression_at(facts, inner) else {
                tokens.reject("a directive expression could not be parsed");
                continue;
            };
            let scope = directive_scope(facts, element, attribute_name != "wx:for");
            if let Some((index, _)) = effective_loop_binding(&facts.loops, &scope, &path[0]) {
                let _ = index;
                continue;
            }
            let scan = scan_expression(&facts.doc, &expr);
            for chain in &scan.chains {
                if chain.segments.len() < path.len() {
                    continue;
                }
                if chain
                    .segments
                    .iter()
                    .take(path.len())
                    .zip(path.iter())
                    .all(|((name, _), expected)| name == expected)
                {
                    let (_, range) = &chain.segments[path.len() - 1];
                    tokens.push(&facts.doc.file, *range, TokenStyle::Identifier);
                }
            }
        }
    });
}

/// Collects all static event bindings of one handler in the current document.
pub(crate) fn collect_event_refs(facts: &ViewFacts, name: &str) -> Tokens {
    let mut tokens = Tokens::new();
    if !facts.includes.is_empty() || !facts.includes_complete {
        tokens.reject("static includes hide event bindings");
    }
    if facts.has_template_definitions() {
        tokens.reject("template definitions hide event bindings");
    }
    if facts.analysis.truncated {
        tokens.reject("template fragment budget was reached");
    }
    for unsupported in &facts.analysis.unsupported {
        if matches!(
            unsupported.kind(),
            wxml_types::UnsupportedKind::DynamicEvent
                | wxml_types::UnsupportedKind::InvalidRange
                | wxml_types::UnsupportedKind::FragmentLimit
        ) {
            tokens.reject("a dynamic event binding may reference this handler");
        }
    }
    for event in &facts.analysis.events {
        if event.handler == name {
            tokens.push(&facts.doc.file, event.handler_range, TokenStyle::Identifier);
        }
    }
    tokens
}

/// Collects tag-pair tokens.
pub(crate) fn collect_tag_refs(facts: &ViewFacts, start: SourceRange, end: SourceRange) -> Tokens {
    let mut tokens = Tokens::new();
    tokens.push(&facts.doc.file, start, TokenStyle::Tag);
    tokens.push(&facts.doc.file, end, TokenStyle::Tag);
    tokens
}

/// A style class definition token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClassDefinitionToken {
    /// Defining file.
    pub(crate) file: wxml_types::FileId,
    /// Exact class-name range.
    pub(crate) range: SourceRange,
}

/// One class symbol collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClassTokens {
    /// Definitions in style files.
    pub(crate) definitions: Vec<ClassDefinitionToken>,
    /// WXML usages.
    pub(crate) usages: Vec<SymbolToken>,
    /// `true` only when every style source and usage is covered.
    pub(crate) complete: bool,
    /// Short path-free reason.
    pub(crate) reason: Option<&'static str>,
    /// `true` when the style chain is provably local to this view.
    pub(crate) rename_safe: bool,
}

/// Collects class definitions and WXML usages for the current view.
pub(crate) fn collect_class_refs(server: &Server, facts: &ViewFacts, name: &str) -> ClassTokens {
    let mut result = ClassTokens {
        definitions: Vec::new(),
        usages: Vec::new(),
        complete: false,
        reason: None,
        rename_safe: false,
    };
    let mut tokens = Tokens::new();
    if !facts.includes.is_empty() || !facts.includes_complete {
        tokens.reject("static includes hide class usages");
    }
    collect_class_usages_in_doc(&facts.doc, name, &mut tokens);
    for include in &facts.includes {
        collect_class_usages_in_doc(include, name, &mut tokens);
    }
    result.usages = tokens.items;
    let Some(style) = server.host.style_facts_for(&facts.doc.file) else {
        result.reason = Some("no style index is available for this view");
        return result;
    };
    let definitions = style.definitions(name);
    if definitions.is_empty() {
        result.reason = Some("the class has no indexed definition");
        return result;
    }
    let project = facts.doc.file.project().clone();
    let mut complete = style.complete && tokens.complete;
    let mut rename_safe = style.complete && tokens.complete;
    for definition in definitions {
        if !definition.rename_safe() {
            complete = false;
            rename_safe = false;
        }
        if definition.imported {
            rename_safe = false;
        }
        if !matches!(definition.scope, StyleScope::Page | StyleScope::Component) {
            rename_safe = false;
        }
        if !matches!(
            style.isolation,
            wxml_project::StyleIsolation::Default
                | wxml_project::StyleIsolation::Isolated
                | wxml_project::StyleIsolation::PageIsolated
        ) {
            rename_safe = false;
        }
        let Ok(file) = wxml_types::FileId::new(project.clone(), definition.file.as_str()) else {
            complete = false;
            continue;
        };
        result.definitions.push(ClassDefinitionToken {
            file,
            range: definition.range(),
        });
    }
    result.complete = complete;
    result.rename_safe = rename_safe;
    if !complete && result.reason.is_none() {
        result.reason = Some("style coverage is incomplete");
    }
    result
}

fn collect_class_usages_in_doc(doc: &QueryDoc, name: &str, tokens: &mut Tokens) {
    for_each_element(doc.tree(), &mut |element| {
        let Some(attribute) = element.attribute("class") else {
            return;
        };
        let Some(value) = attribute.value() else {
            return;
        };
        for child in value.children() {
            let AstNode::Text(text) = child else {
                continue;
            };
            let range = text.text_range();
            let content = text.text();
            let bytes = content.as_bytes();
            let mut index = 0_usize;
            while index < bytes.len() {
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                let start = index;
                while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                if start >= index {
                    break;
                }
                let Some(token_start) = range
                    .start()
                    .checked_add(u32::try_from(start).unwrap_or(u32::MAX))
                else {
                    continue;
                };
                let Some(token_end) = range
                    .start()
                    .checked_add(u32::try_from(index).unwrap_or(u32::MAX))
                else {
                    continue;
                };
                let Ok(token) = SourceRange::from_bounds(token_start, token_end) else {
                    continue;
                };
                if range_text(doc, token) == Some(name) {
                    tokens.push(&doc.file, token, TokenStyle::Class);
                }
            }
        }
    });
}

/* ------------------------------------------------------------ template names */

/// One template symbol's definition and visible call sites.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TemplateTokens {
    /// Definition token, when a reachable definition exists.
    pub(crate) definition: Option<SymbolToken>,
    /// Call-site tokens.
    pub(crate) calls: Vec<SymbolToken>,
    /// `true` only when every import/dynamic call is covered.
    pub(crate) complete: bool,
    /// Short path-free reason.
    pub(crate) reason: Option<&'static str>,
}

/// Returns `true` when any project view already defines or calls `name`.
pub(crate) fn template_name_in_use(server: &Server, facts: &ViewFacts, name: &str) -> Option<bool> {
    let views = project_views(server, facts);
    if !views.complete {
        return None;
    }
    for view in &views.views {
        if view
            .semantics
            .definitions
            .iter()
            .any(|definition| definition.name == name)
            || view.semantics.calls.iter().any(|call| call.name == name)
        {
            return Some(true);
        }
        for import in &view.semantics.imports {
            if import.kind != wxml_types::StaticImportKind::Import {
                continue;
            }
            let path = wxml_types::resolve_static_specifier(view.doc.file.path(), &import.src)?;
            let doc = load_project_doc(server, view.doc.file.project(), &path)?;
            let semantics = wxml_types::analyze_template_semantics(doc.tree());
            if semantics
                .definitions
                .iter()
                .any(|definition| definition.name == name)
                || semantics.calls.iter().any(|call| call.name == name)
            {
                return Some(true);
            }
            if semantics.issues.iter().any(|issue| {
                matches!(
                    issue.kind,
                    wxml_types::StaticIssueKind::DynamicImportSrc
                        | wxml_types::StaticIssueKind::LimitReached
                )
            }) {
                return None;
            }
        }
    }
    Some(false)
}

/// Returns `true` when a component tag already carries `declared` as an attribute.
pub(crate) fn property_name_conflict(
    server: &Server,
    facts: &ViewFacts,
    component_path: &ProjectPath,
    declared: &str,
) -> Option<bool> {
    let views = project_views(server, facts);
    if !views.complete {
        return None;
    }
    let kebab = kebab_case(declared);
    for view in &views.views {
        for component in &view.components {
            if !same_component(component_path, component) {
                continue;
            }
            let mut conflict = false;
            for_each_element(view.doc.tree(), &mut |element| {
                if element.name() != Some(component.name.as_str()) {
                    return;
                }
                if element.attributes().any(|attribute| {
                    attribute
                        .name()
                        .is_some_and(|name| name == declared || name == kebab)
                }) {
                    conflict = true;
                }
            });
            if conflict {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// Collects a template definition and every statically visible call.
#[allow(clippy::too_many_lines)] // one linear visibility scan
pub(crate) fn collect_template_refs(
    server: &Server,
    facts: &ViewFacts,
    name: &str,
) -> TemplateTokens {
    let mut result = TemplateTokens {
        definition: None,
        calls: Vec::new(),
        complete: true,
        reason: None,
    };
    let views = project_views(server, facts);
    let mut complete = views.complete;
    let mut reason = views.reason;
    let mut definition = None;
    // Local definitions in the queried file win.
    if let Some(definition_facts) = facts.semantics.definition(name) {
        definition = Some(SymbolToken {
            file: facts.doc.file.clone(),
            range: definition_facts.name_range,
            style: TokenStyle::Template,
        });
        if facts
            .semantics
            .definitions
            .iter()
            .filter(|candidate| candidate.name == name)
            .count()
            > 1
        {
            complete = false;
            reason.get_or_insert("duplicate template definitions");
        }
    } else {
        for import in &facts.semantics.imports {
            if import.kind != wxml_types::StaticImportKind::Import {
                continue;
            }
            let Some(path) =
                wxml_types::resolve_static_specifier(facts.doc.file.path(), &import.src)
            else {
                complete = false;
                reason.get_or_insert("a template import path is dynamic");
                continue;
            };
            let Some(doc) = load_project_doc(server, facts.doc.file.project(), &path) else {
                complete = false;
                reason.get_or_insert("a template import could not be read");
                continue;
            };
            let semantics = wxml_types::analyze_template_semantics(doc.tree());
            if let Some(definition_facts) = semantics.definition(name) {
                definition = Some(SymbolToken {
                    file: doc.file,
                    range: definition_facts.name_range,
                    style: TokenStyle::Template,
                });
                break;
            }
        }
    }
    if definition.is_none() {
        complete = false;
        reason.get_or_insert("no reachable template definition");
    }
    let Some(definition_token) = definition else {
        result.complete = false;
        result.reason = reason.or(Some("no reachable template definition"));
        return result;
    };
    if !facts.includes.is_empty() || !facts.includes_complete {
        complete = false;
        reason.get_or_insert("static includes hide template calls");
    }
    let definition_file = definition_token.file.clone();
    // A shared template file may call another template in its own body while
    // not being a discovered page/component view.
    if let Some(def_doc) =
        load_project_doc(server, definition_file.project(), definition_file.path())
    {
        let semantics = wxml_types::analyze_template_semantics(def_doc.tree());
        for call in &semantics.calls {
            if call.name == name {
                result.calls.push(SymbolToken {
                    file: def_doc.file.clone(),
                    range: call.name_range,
                    style: TokenStyle::Template,
                });
            }
        }
        if semantics
            .imports
            .iter()
            .any(|import| import.kind == wxml_types::StaticImportKind::Include)
            || semantics.issues.iter().any(|issue| {
                matches!(
                    issue.kind,
                    wxml_types::StaticIssueKind::DynamicTemplateIs
                        | wxml_types::StaticIssueKind::DynamicImportSrc
                        | wxml_types::StaticIssueKind::LimitReached
                )
            })
        {
            complete = false;
            reason.get_or_insert("the template definition file has dynamic calls");
        }
    }
    // Cache imported definitions by file so visibility checks do not reread.
    let mut import_cache: BTreeMap<wxml_types::FileId, BTreeSet<String>> = BTreeMap::new();
    for view in &views.views {
        if view
            .semantics
            .imports
            .iter()
            .any(|import| import.kind == wxml_types::StaticImportKind::Include)
        {
            complete = false;
            reason.get_or_insert("static includes hide template calls");
        }
        if !view.semantics.issues.is_empty()
            && view.semantics.issues.iter().any(|issue| {
                matches!(
                    issue.kind,
                    wxml_types::StaticIssueKind::DynamicTemplateIs
                        | wxml_types::StaticIssueKind::DynamicImportSrc
                        | wxml_types::StaticIssueKind::LimitReached
                )
            })
        {
            complete = false;
            reason.get_or_insert("template import/dynamic call coverage is incomplete");
        }
        let local = view
            .semantics
            .definitions
            .iter()
            .find(|candidate| candidate.name == name);
        let mut winner = local.map(|_| view.doc.file.clone());
        if winner.is_none() {
            for import in &view.semantics.imports {
                if import.kind != wxml_types::StaticImportKind::Import {
                    continue;
                }
                let Some(path) =
                    wxml_types::resolve_static_specifier(view.doc.file.path(), &import.src)
                else {
                    continue;
                };
                let Some(doc) = load_project_doc(server, &definition_file.project().clone(), &path)
                else {
                    continue;
                };
                let entry = import_cache.entry(doc.file.clone()).or_insert_with(|| {
                    wxml_types::analyze_template_semantics(doc.tree())
                        .definitions
                        .into_iter()
                        .map(|definition| definition.name)
                        .collect()
                });
                if entry.contains(name) {
                    winner = Some(doc.file.clone());
                    break;
                }
            }
        }
        if winner.as_ref() != Some(&definition_file) {
            continue;
        }
        for call in &view.semantics.calls {
            if call.name == name {
                result.calls.push(SymbolToken {
                    file: view.doc.file.clone(),
                    range: call.name_range,
                    style: TokenStyle::Template,
                });
            }
        }
    }
    result
        .calls
        .sort_by_key(|call| (call.file.clone(), call.range.start()));
    result
        .calls
        .dedup_by(|left, right| left.file == right.file && left.range == right.range);
    result.definition = Some(definition_token);
    result.complete = complete;
    result.reason = if complete { None } else { reason };
    result
}

/* --------------------------------------------------------- component props */

/// Component property reference coverage across project views.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PropertyTokens {
    /// Attribute-name tokens in every view that registers the component.
    pub(crate) items: Vec<SymbolToken>,
    /// `true` only when every view/component fact is complete.
    pub(crate) complete: bool,
    /// Short path-free reason.
    pub(crate) reason: Option<&'static str>,
}

/// Collects WXML attribute usages of one component property.
pub(crate) fn collect_property_refs(
    server: &Server,
    facts: &ViewFacts,
    declared: &str,
    component_path: &ProjectPath,
) -> PropertyTokens {
    let views = project_views(server, facts);
    let mut result = PropertyTokens {
        items: Vec::new(),
        complete: views.complete,
        reason: views.reason,
    };
    let mut tokens = Tokens::new();
    for view in &views.views {
        let components: Vec<&ViewComponent> = view
            .components
            .iter()
            .filter(|component| same_component(component_path, component))
            .collect();
        if components.is_empty() {
            continue;
        }
        if !view.doc.structure.text().is_empty()
            && wxml_syntax::parse(view.doc.text())
                .tree
                .document()
                .children()
                .count()
                == 0
        {
            tokens.reject("a project view could not be parsed");
            continue;
        }
        for component in components {
            if !component.property_facts.complete {
                tokens.reject("a component property table is incomplete");
            }
            if !component
                .property_facts
                .properties
                .iter()
                .any(|property| property.name == declared)
            {
                tokens.reject("the component property is missing in one view");
                continue;
            }
            for_each_element(view.doc.tree(), &mut |element| {
                if element.name() != Some(component.name.as_str()) {
                    return;
                }
                for attribute in element.attributes() {
                    let Some(name) = attribute.name() else {
                        continue;
                    };
                    if !property_spelling(declared, name) {
                        continue;
                    }
                    let Some(range) = attribute.name_range() else {
                        continue;
                    };
                    tokens.push(
                        &view.doc.file,
                        SourceRange::new(range),
                        TokenStyle::Attribute,
                    );
                }
            });
        }
    }
    result.items = tokens.items;
    if !tokens.complete {
        result.complete = false;
        result.reason = tokens.reason.or(result.reason);
    }
    result
}

/// Compares a property declaration script with one registered component.
fn same_component(declaration: &ProjectPath, component: &ViewComponent) -> bool {
    let declaration = declaration.as_str();
    let stem = declaration
        .rsplit_once('.')
        .map_or(declaration, |(stem, _)| stem);
    let matches =
        |candidate: &ProjectPath| candidate.as_str() == stem || candidate.as_str() == declaration;
    component
        .component
        .as_ref()
        .is_some_and(|candidate| matches(candidate) || prefix_match(candidate, stem))
        || component
            .wxml
            .as_ref()
            .is_some_and(|candidate| prefix_match(candidate, stem))
}

fn prefix_match(candidate: &ProjectPath, stem: &str) -> bool {
    let value = candidate.as_str();
    value == stem
        || value
            .strip_suffix(".wxml")
            .is_some_and(|without| without == stem)
}

/* ------------------------------------------------------------------- loops */

/// Loop-local reference coverage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoopTokens {
    /// Declaration token (explicit only).
    pub(crate) declaration: Option<SymbolToken>,
    /// Identifier references.
    pub(crate) references: Vec<SymbolToken>,
    /// `true` only when every in-scope reference is covered.
    pub(crate) complete: bool,
    /// Short path-free reason.
    pub(crate) reason: Option<&'static str>,
    /// `true` when the loop has no explicit declaration token and rename must
    /// insert the matching `wx:for-item`/`wx:for-index` attribute.
    pub(crate) needs_attribute: bool,
}

/// Collects every reference to one loop binding.
#[allow(clippy::too_many_lines)] // one linear scope scan
pub(crate) fn collect_loop_refs(
    facts: &ViewFacts,
    identity: &LoopIdentity,
    name: &str,
) -> LoopTokens {
    let Some((loop_index, loop_info)) = facts.loops.iter().enumerate().find(|(_, info)| {
        info.file == identity.file && info.source_range.start() == identity.source_start
    }) else {
        return LoopTokens {
            declaration: None,
            references: Vec::new(),
            complete: false,
            reason: Some("the loop scope is unavailable"),
            needs_attribute: false,
        };
    };
    let mut reasons: Vec<&'static str> = Vec::new();
    if !facts.includes.is_empty() || !facts.includes_complete {
        reasons.push("static includes hide loop references");
    }
    if facts.analysis.truncated {
        reasons.push("template fragment budget was reached");
    }
    if has_dynamic_loop(facts.doc.tree()) {
        reasons.push("a dynamic wx:for binding may shadow this loop local");
    }
    if loop_info.item.name == loop_info.index.name {
        reasons.push("duplicate loop item/index names cannot be renamed safely");
    }
    let binding = match identity.role {
        LocalRole::Item => &loop_info.item,
        LocalRole::Index => &loop_info.index,
        _ => {
            return LoopTokens {
                declaration: None,
                references: Vec::new(),
                complete: false,
                reason: Some("unknown loop binding role"),
                needs_attribute: false,
            };
        }
    };
    let declaration = binding.declaration.map(|range| SymbolToken {
        file: facts.doc.file.clone(),
        range,
        style: TokenStyle::Identifier,
    });
    let mut references = Vec::new();
    for fragment in &facts.analysis.fragments {
        let Some(binding) = effective_binding(&fragment.bindings, name) else {
            continue;
        };
        let Some(fragment_loop) = loop_for_binding(&facts.loops, binding) else {
            continue;
        };
        if fragment_loop != loop_index {
            continue;
        }
        let Some(expr) = expression_for_fragment(fragment) else {
            reasons.push("an expression could not be parsed completely");
            continue;
        };
        let scan = scan_expression(&facts.doc, &expr);
        for chain in &scan.chains {
            let Some((root, range)) = chain.segments.first() else {
                continue;
            };
            if root != name {
                continue;
            }
            references.push(SymbolToken {
                file: facts.doc.file.clone(),
                range: *range,
                style: TokenStyle::Identifier,
            });
            if chain.shorthand {
                reasons.push("an object shorthand cannot be renamed safely");
            }
        }
    }
    // Template definition bodies have no analyzer fragment; scan their
    // mustaches with the same loop ancestry so a local rename stays complete.
    let tree = facts.doc.tree();
    for (id, node) in tree.nodes() {
        if node.kind() != wxml_syntax::NodeKind::MustacheExpr {
            continue;
        }
        let Some(expr) = tree.expr(id) else {
            continue;
        };
        let expr_range = SourceRange::new(node.text_range());
        let scope: Vec<usize> = facts
            .loops
            .iter()
            .enumerate()
            .filter(|(_, info)| {
                info.element_range.start() <= expr_range.start()
                    && info.element_range.end() >= expr_range.end()
            })
            .map(|(index, _)| index)
            .collect();
        let Some((owner, role)) = effective_loop_binding(&facts.loops, &scope, name) else {
            continue;
        };
        if owner != loop_index || role != identity.role {
            continue;
        }
        let scan = scan_expression(&facts.doc, expr);
        for chain in &scan.chains {
            let Some((root, range)) = chain.segments.first() else {
                continue;
            };
            if root == name {
                references.push(SymbolToken {
                    file: facts.doc.file.clone(),
                    range: *range,
                    style: TokenStyle::Identifier,
                });
                if chain.shorthand {
                    reasons.push("an object shorthand cannot be renamed safely");
                }
            }
        }
    }
    // `wx:for` source expressions of descendant loops and guard attributes.
    for_each_element(facts.doc.tree(), &mut |element| {
        for attribute_name in ["wx:for", "wx:if", "wx:elif"] {
            let Some(attribute) = element.attribute(attribute_name) else {
                continue;
            };
            let Some(value) = attribute.value() else {
                continue;
            };
            let Some(inner) = mustache_inner(value) else {
                continue;
            };
            let Some(expr) = expression_at(facts, inner) else {
                reasons.push("a directive expression could not be parsed");
                continue;
            };
            let scope = directive_scope(facts, element, attribute_name != "wx:for");
            let Some((owner, role)) = effective_loop_binding(&facts.loops, &scope, name) else {
                continue;
            };
            if owner != loop_index || role != identity.role {
                continue;
            }
            let scan = scan_expression(&facts.doc, &expr);
            for chain in &scan.chains {
                let Some((root, range)) = chain.segments.first() else {
                    continue;
                };
                if root == name {
                    references.push(SymbolToken {
                        file: facts.doc.file.clone(),
                        range: *range,
                        style: TokenStyle::Identifier,
                    });
                }
            }
        }
    });
    references.sort_by_key(|token| (token.file.clone(), token.range.start(), token.range.end()));
    references.dedup_by(|left, right| left.file == right.file && left.range == right.range);
    LoopTokens {
        declaration,
        references,
        complete: reasons.is_empty(),
        reason: reasons.first().copied(),
        needs_attribute: binding.declaration.is_none(),
    }
}

fn has_dynamic_loop(tree: &SyntaxTree) -> bool {
    let mut dynamic = false;
    for_each_element(tree, &mut |element| {
        if element.attribute("wx:for").is_some() {
            let has_static = element
                .attribute("wx:for")
                .and_then(wxml_syntax::Attribute::value)
                .and_then(|value| value.mustaches().next())
                .and_then(wxml_syntax::Mustache::inner_range)
                .is_some();
            if !has_static {
                dynamic = true;
            }
        }
    });
    dynamic
}

fn inside_template_definition(tree: &SyntaxTree, range: SourceRange) -> bool {
    let mut inside = false;
    for_each_element(tree, &mut |element| {
        if element.name() == Some("template") && element.attribute("name").is_some() {
            let element_range = element.text_range();
            if element_range.start() <= range.start() && element_range.end() >= range.end() {
                inside = true;
            }
        }
    });
    inside
}
