//! Static semantics for WXML `<template>`, `<import>`, `<include>` and `<wxs>`.
//!
//! The template type pipeline turns mustaches into C1 fragments, but WXML also
//! has a small module system that is entirely static:
//!
//! - `<template name="card">...</template>` defines a reusable block;
//! - `<template is="card" data="{{ { title: pageTitle } }}" />` instantiates it;
//! - `<import src="./shared.wxml" />` makes imported template definitions
//!   available, while `<include src="./part.wxml" />` inlines markup;
//! - `<wxs module="utils">...</wxs>` and `<wxs module="utils" src="./utils.wxs" />`
//!   declare a WXS module usable inside mustache expressions.
//!
//! This module collects those facts as plain data with exact byte ranges. It
//! never reads other files; callers resolve `src` with
//! [`resolve_static_specifier`] (which rejects paths escaping the project) and
//! merge definitions themselves. The analysis is bounded and tolerant of
//! malformed input: what cannot be proven is reported as a [`StaticIssue`], not
//! guessed.
//!
//! The WXS *body* is summarized separately by [`crate::wxs`].

use std::collections::HashSet;

use wxml_syntax::{
    AstNode, Attribute, AttributeValue, Element, Expr, ExprKind, ObjectKey, SyntaxTree,
};
use wxml_text::{SourceRange, TextRange};

use crate::wxs::WxsSummary;

/// Maximum template definitions collected from one document.
pub const MAX_TEMPLATE_DEFINITIONS: usize = 512;
/// Maximum template calls collected from one document.
pub const MAX_TEMPLATE_CALLS: usize = 1024;
/// Maximum import/include/WXS elements collected from one document.
pub const MAX_STATIC_MODULES: usize = 256;
/// Maximum data keys per template instance or definition.
pub const MAX_TEMPLATE_DATA_KEYS: usize = 256;
/// Maximum data-key uses collected from one template definition body.
pub const MAX_TEMPLATE_DATA_USES: usize = 2048;
/// Maximum data uses collected for one template definition.
pub const MAX_TEMPLATE_DATA_USES_PER_TEMPLATE: usize = 256;

/// Which WXML statement imported a template or markup fragment.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StaticImportKind {
    /// `<import src="..." />`: template definitions become available.
    Import,
    /// `<include src="..." />`: the target markup is inlined.
    Include,
}

impl StaticImportKind {
    /// Stable lower-case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Include => "include",
        }
    }
}

/// One `<template name="...">` definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateDefinition {
    /// Template name as written (trimmed).
    pub name: String,
    /// Byte range of the name token.
    pub name_range: SourceRange,
    /// Byte range of the whole `<template>` element.
    pub element_range: SourceRange,
    /// Root data identifiers read by the definition body, in source order and
    /// deduplicated by name. Loop locals are excluded.
    pub data_uses: Vec<TemplateDataUse>,
    /// `true` when the body contains a construct that prevents a complete
    /// data-key summary (nested template definition, dynamic loop binding,
    /// budget truncation).
    pub incomplete: Option<String>,
}

/// One root data identifier read inside a template definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateDataUse {
    /// Identifier as written.
    pub name: String,
    /// Byte range of this occurrence in the WXML source.
    pub range: SourceRange,
}

/// One `<template is="...">` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateCall {
    /// Template name as written (trimmed).
    pub name: String,
    /// Byte range of the name token.
    pub name_range: SourceRange,
    /// Byte range of the whole `<template>` element.
    pub element_range: SourceRange,
    /// The `data` attribute, when present.
    pub data: Option<TemplateData>,
}

/// The `data="{{ ... }}"` object of one template call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateData {
    /// Statically known object keys.
    pub keys: Vec<TemplateDataKey>,
    /// Byte range of the expression inside `{{ }}`.
    pub range: SourceRange,
    /// Set when keys cannot be determined completely (non-object `data`,
    /// spread syntax, parse recovery or budget truncation).
    pub dynamic: Option<String>,
}

/// One statically known key of a template instance data object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateDataKey {
    /// Key as written, without quotes.
    pub name: String,
    /// Byte range of the key token.
    pub range: SourceRange,
}

/// One `<import>` or `<include>` statement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticImport {
    /// Statement kind.
    pub kind: StaticImportKind,
    /// `src` value as written, trimmed.
    pub src: String,
    /// Byte range of the `src` value without quotes.
    pub src_range: SourceRange,
    /// Byte range of the whole element.
    pub element_range: SourceRange,
}

/// One `<wxs>` block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WxsBlock {
    /// `module` value as written, when static.
    pub module: Option<String>,
    /// Byte range of the `module` value without quotes.
    pub module_range: Option<SourceRange>,
    /// `src` value as written, when present.
    pub src: Option<String>,
    /// Byte range of the `src` value without quotes.
    pub src_range: Option<SourceRange>,
    /// Byte range of the inline body (without the `<wxs>` tags), when present
    /// and non-empty.
    pub body_range: Option<SourceRange>,
    /// Byte range of the whole element.
    pub element_range: SourceRange,
}

/// Why a static template/WXS form is not completely summarized.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StaticIssueKind {
    /// A `<template>` has no static `name` attribute.
    MissingTemplateName,
    /// A `<template is>` has no static `is` value.
    MissingTemplateIs,
    /// A template call name is a mustache or mixed value.
    DynamicTemplateIs,
    /// Two definitions in one document share a name.
    DuplicateTemplateName,
    /// An `<import>`/`<include>` has no static `src`.
    MissingImportSrc,
    /// An `<import>`/`<include>` `src` is dynamic.
    DynamicImportSrc,
    /// A template call `data` attribute is not a static object literal.
    DynamicTemplateData,
    /// A `<wxs>` block has no static `module` name.
    MissingWxsModule,
    /// A `<wxs>` block has neither a static `src` nor an inline body.
    EmptyWxsBlock,
    /// A `<wxs>` block has both `src` and an inline body.
    AmbiguousWxsBlock,
    /// A collection budget was reached; later facts are missing.
    LimitReached,
}

impl StaticIssueKind {
    /// Short, stable explanation.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::MissingTemplateName => "template definition has no static name",
            Self::MissingTemplateIs => "template call has no static `is` value",
            Self::DynamicTemplateIs => "template call name is dynamic and is not analyzed",
            Self::DuplicateTemplateName => "duplicate template name; the first definition wins",
            Self::MissingImportSrc => "import/include has no static `src`",
            Self::DynamicImportSrc => "import/include `src` is dynamic and is not resolved",
            Self::DynamicTemplateData => {
                "template instance `data` is not a static object literal; keys are not inferred"
            }
            Self::MissingWxsModule => "wxs block has no static module name",
            Self::EmptyWxsBlock => "wxs block has neither an inline body nor a static `src`",
            Self::AmbiguousWxsBlock => "wxs block has both `src` and an inline body; `src` wins",
            Self::LimitReached => "static template/WXS budget reached; later facts are missing",
        }
    }
}

/// One degraded static form with an exact source range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticIssue {
    /// Issue category.
    pub kind: StaticIssueKind,
    /// Byte range of the affected attribute or element.
    pub range: SourceRange,
}

/// Everything static template/WXS analysis found in one document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateSemantics {
    /// Template definitions in source order.
    pub definitions: Vec<TemplateDefinition>,
    /// Template calls in source order.
    pub calls: Vec<TemplateCall>,
    /// Import/include statements in source order.
    pub imports: Vec<StaticImport>,
    /// WXS blocks in source order.
    pub wxs: Vec<WxsBlock>,
    /// Degraded forms with exact ranges.
    pub issues: Vec<StaticIssue>,
    /// `true` when a collection limit stopped the analysis.
    pub truncated: bool,
}

impl TemplateSemantics {
    /// Finds a definition by name (first definition wins).
    #[must_use]
    pub fn definition(&self, name: &str) -> Option<&TemplateDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name == name)
    }

    /// Iterates over definitions with distinct names.
    pub fn distinct_definitions(&self) -> impl Iterator<Item = &TemplateDefinition> {
        let mut seen = HashSet::new();
        self.definitions
            .iter()
            .filter(move |definition| seen.insert(definition.name.as_str()))
    }
}

/// Resolves a WXML `src`/WXS `require` specifier against the referencing file.
///
/// Only local relative specifiers are accepted. Absolute paths, URL schemes and
/// paths that escape the project root return `None`; the result is normalized
/// (`a/./b`, `a/../b`) and uses `/` separators. `current` is a project-relative
/// path such as `pages/index/index.wxml`.
#[must_use]
pub fn resolve_static_specifier(current: &str, specifier: &str) -> Option<String> {
    let specifier = specifier.trim();
    if specifier.is_empty()
        || specifier.contains('\0')
        || specifier.contains("://")
        || specifier.starts_with('/')
        || specifier.starts_with('\\')
        || specifier.chars().any(char::is_control)
    {
        return None;
    }
    let directory: Vec<&str> = current.split('/').collect();
    let mut components: Vec<&str> = directory
        .get(..directory.len().saturating_sub(1))
        .unwrap_or_default()
        .to_vec();
    let normalized = specifier.replace('\\', "/");
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value),
        }
    }
    if components.is_empty() {
        return None;
    }
    Some(components.join("/"))
}

/// Analyzes static template/import/include/WXS facts in one syntax tree.
///
/// The result is a pure function of the tree. It collects exact ranges and
/// bounded summaries; it does not resolve imports or read other files.
#[must_use]
pub fn analyze_template_semantics(tree: &SyntaxTree) -> TemplateSemantics {
    let mut engine = Engine {
        tree,
        text: tree.text(),
        semantics: TemplateSemantics::default(),
        names: HashSet::new(),
    };
    let children: Vec<AstNode<'_>> = tree.document().children().collect();
    engine.visit_children(&children, &HashSet::new());
    engine.finish()
}

/// Extracts the inline body range of a `<wxs>` element.
///
/// The parser keeps WXS content as text; using the element range and the first
/// `>` avoids depending on raw-text node shapes. Quoted attribute values are
/// skipped while looking for the tag end.
#[must_use]
pub fn wxs_body_range(tree: &SyntaxTree, element: Element<'_>) -> Option<SourceRange> {
    let range = element.text_range();
    let slice = text_range_slice(tree.text(), range)?;
    if element.is_self_closing() {
        return None;
    }
    let start_tag_end = find_tag_end(slice)?;
    let end_tag = slice.rfind("</")?;
    if end_tag <= start_tag_end {
        return None;
    }
    let body_start = range.start() as usize + start_tag_end + 1;
    let body_end = range.start() as usize + end_tag;
    if body_end <= body_start {
        return None;
    }
    let body = tree.text().get(body_start..body_end)?;
    let leading = body.len() - body.trim_start().len();
    let trailing = body.len() - body.trim_end().len();
    let trimmed_start = body_start + leading;
    let trimmed_end = body_end.saturating_sub(trailing);
    if trimmed_end <= trimmed_start {
        return None;
    }
    SourceRange::from_bounds(
        u32::try_from(trimmed_start).ok()?,
        u32::try_from(trimmed_end).ok()?,
    )
    .ok()
}

struct Engine<'a> {
    tree: &'a SyntaxTree,
    text: &'a str,
    semantics: TemplateSemantics,
    names: HashSet<String>,
}

impl Engine<'_> {
    fn finish(self) -> TemplateSemantics {
        self.semantics
    }

    fn issue(&mut self, kind: StaticIssueKind, range: SourceRange) {
        if self.semantics.issues.len() < 4096 {
            self.semantics.issues.push(StaticIssue { kind, range });
        }
    }

    fn visit_children(&mut self, children: &[AstNode<'_>], locals: &HashSet<String>) {
        for node in children {
            self.visit_node(*node, locals);
        }
    }

    fn visit_node(&mut self, node: AstNode<'_>, locals: &HashSet<String>) {
        if let AstNode::Element(element) = node {
            self.visit_element(element, locals);
        }
    }

    fn visit_element(&mut self, element: Element<'_>, locals: &HashSet<String>) {
        match element.name().unwrap_or_default() {
            "template" => {
                self.visit_template_element(element, locals);
                return;
            }
            "import" => {
                self.collect_import(element, StaticImportKind::Import);
                self.visit_children(&collect_children(element), locals);
                return;
            }
            "include" => {
                self.collect_import(element, StaticImportKind::Include);
                self.visit_children(&collect_children(element), locals);
                return;
            }
            "wxs" => {
                self.collect_wxs(element);
                return;
            }
            _ => {}
        }

        let mut child_locals = locals.clone();
        if element.attribute("wx:for").is_some() {
            child_locals.insert("item".to_owned());
            child_locals.insert("index".to_owned());
        }
        for attribute in element.attributes() {
            let Some(name) = attribute.name() else {
                continue;
            };
            if matches!(name, "wx:for-item" | "wx:for-index") {
                if let Some(value) = attribute.value().and_then(AttributeValue::literal_text) {
                    let value = value.trim();
                    if !value.is_empty() {
                        child_locals.insert(value.to_owned());
                    }
                }
            }
        }
        let _ = locals;
        self.visit_children(&collect_children(element), &child_locals);
    }

    fn visit_template_element(&mut self, element: Element<'_>, locals: &HashSet<String>) {
        let attributes: Vec<Attribute<'_>> = element.attributes().collect();
        let name = attributes
            .iter()
            .copied()
            .find(|attribute| attribute.name() == Some("name"));
        let is = attributes
            .iter()
            .copied()
            .find(|attribute| attribute.name() == Some("is"));
        if let Some(name_attribute) = name {
            self.collect_definition(element, name_attribute);
            return;
        }
        if let Some(is_attribute) = is {
            self.collect_call(element, is_attribute, &attributes);
            return;
        }
        // A `<template>` without `name` or `is` is malformed; analyze its body
        // as ordinary markup so nested mustaches are not silently skipped.
        let mut child_locals = locals.clone();
        for attribute in &attributes {
            if matches!(attribute.name(), Some("wx:for-item" | "wx:for-index")) {
                if let Some(value) = attribute.value().and_then(AttributeValue::literal_text) {
                    let value = value.trim();
                    if !value.is_empty() {
                        child_locals.insert(value.to_owned());
                    }
                }
            }
        }
        let _ = attributes;
        self.visit_children(&collect_children(element), &child_locals);
    }

    fn collect_definition(&mut self, element: Element<'_>, attribute: Attribute<'_>) {
        if self.semantics.definitions.len() >= MAX_TEMPLATE_DEFINITIONS {
            self.semantics.truncated = true;
            self.issue(
                StaticIssueKind::LimitReached,
                as_source_range(element.text_range()),
            );
            return;
        }
        let Some((value, value_range)) = static_value(attribute) else {
            self.issue(
                StaticIssueKind::MissingTemplateName,
                as_source_range(attribute.text_range()),
            );
            return;
        };
        let Some(name_range) = value_range else {
            self.issue(
                StaticIssueKind::MissingTemplateName,
                as_source_range(attribute.text_range()),
            );
            return;
        };
        if !self.names.insert(value.clone()) {
            self.issue(StaticIssueKind::DuplicateTemplateName, name_range);
        }
        let mut uses = HashSet::new();
        let mut data_uses = Vec::new();
        let mut incomplete = None;
        self.collect_body_uses(
            &collect_children(element),
            &HashSet::new(),
            &mut uses,
            &mut data_uses,
            &mut incomplete,
        );
        self.semantics.definitions.push(TemplateDefinition {
            name: value,
            name_range,
            element_range: as_source_range(element.text_range()),
            data_uses,
            incomplete,
        });
    }

    fn collect_call(
        &mut self,
        element: Element<'_>,
        is_attribute: Attribute<'_>,
        attributes: &[Attribute<'_>],
    ) {
        if self.semantics.calls.len() >= MAX_TEMPLATE_CALLS {
            self.semantics.truncated = true;
            self.issue(
                StaticIssueKind::LimitReached,
                as_source_range(element.text_range()),
            );
            return;
        }
        let Some((name, name_range)) =
            static_value(is_attribute).and_then(|(value, range)| range.map(|range| (value, range)))
        else {
            self.issue(
                StaticIssueKind::MissingTemplateIs,
                as_source_range(is_attribute.text_range()),
            );
            return;
        };
        let data = attributes
            .iter()
            .copied()
            .find(|attribute| attribute.name() == Some("data"))
            .map(|attribute| self.collect_instance_data(element, attribute));
        self.semantics.calls.push(TemplateCall {
            name,
            name_range,
            element_range: as_source_range(element.text_range()),
            data,
        });
    }

    fn collect_instance_data(
        &mut self,
        element: Element<'_>,
        attribute: Attribute<'_>,
    ) -> TemplateData {
        let attribute_range = as_source_range(attribute.text_range());
        let Some(value) = attribute.value() else {
            self.issue(StaticIssueKind::DynamicTemplateData, attribute_range);
            return TemplateData {
                keys: Vec::new(),
                range: attribute_range,
                dynamic: Some("`data` has no value".to_owned()),
            };
        };
        let mut mustaches = value.mustaches();
        let Some(mustache) = mustaches.next() else {
            self.issue(StaticIssueKind::DynamicTemplateData, attribute_range);
            return TemplateData {
                keys: Vec::new(),
                range: as_source_range(value.text_range()),
                dynamic: Some("`data` is not a mustache".to_owned()),
            };
        };
        if mustaches.next().is_some() || !mustache.is_closed() {
            self.issue(StaticIssueKind::DynamicTemplateData, attribute_range);
            return TemplateData {
                keys: Vec::new(),
                range: as_source_range(value.text_range()),
                dynamic: Some("`data` mixes mustaches or is unclosed".to_owned()),
            };
        }
        let Some(inner) = mustache.inner_range() else {
            self.issue(StaticIssueKind::DynamicTemplateData, attribute_range);
            return TemplateData {
                keys: Vec::new(),
                range: as_source_range(value.text_range()),
                dynamic: Some("`data` expression has no range".to_owned()),
            };
        };
        let Some(expr) = mustache.expr() else {
            self.issue(StaticIssueKind::DynamicTemplateData, as_source_range(inner));
            return TemplateData {
                keys: Vec::new(),
                range: as_source_range(inner),
                dynamic: Some("`data` expression could not be parsed".to_owned()),
            };
        };
        let ExprKind::Object(entries) = expr.kind() else {
            let _ = element;
            self.issue(StaticIssueKind::DynamicTemplateData, as_source_range(inner));
            return TemplateData {
                keys: Vec::new(),
                range: as_source_range(inner),
                dynamic: Some("`data` is not an object literal".to_owned()),
            };
        };
        let mut keys = Vec::new();
        let mut dynamic =
            (entries.len() > MAX_TEMPLATE_DATA_KEYS).then(|| "too many `data` keys".to_owned());
        for entry in entries.iter().take(MAX_TEMPLATE_DATA_KEYS) {
            let Some((name, range)) = object_key(&entry.key, self.text) else {
                dynamic.get_or_insert_with(|| "`data` has a computed key".to_owned());
                continue;
            };
            if keys.iter().any(|key: &TemplateDataKey| key.name == name) {
                continue;
            }
            keys.push(TemplateDataKey { name, range });
        }
        if dynamic.is_some() {
            self.issue(StaticIssueKind::DynamicTemplateData, as_source_range(inner));
        }
        TemplateData {
            keys,
            range: as_source_range(inner),
            dynamic,
        }
    }

    fn collect_body_uses(
        &mut self,
        children: &[AstNode<'_>],
        locals: &HashSet<String>,
        seen: &mut HashSet<String>,
        uses: &mut Vec<TemplateDataUse>,
        incomplete: &mut Option<String>,
    ) {
        for node in children {
            let AstNode::Element(element) = *node else {
                if let AstNode::Mustache(mustache) = *node {
                    self.push_mustache_uses(mustache, locals, seen, uses, incomplete);
                }
                continue;
            };
            if element.name() == Some("template") && element.attribute("name").is_some() {
                incomplete.get_or_insert_with(|| "nested template definition".to_owned());
                continue;
            }
            let mut child_locals = locals.clone();
            if element.attribute("wx:for").is_some() {
                child_locals.insert("item".to_owned());
                child_locals.insert("index".to_owned());
            }
            for attribute in element.attributes() {
                if matches!(attribute.name(), Some("wx:for-item" | "wx:for-index")) {
                    if let Some(value) = attribute.value().and_then(AttributeValue::literal_text) {
                        let value = value.trim();
                        if !value.is_empty() {
                            child_locals.insert(value.to_owned());
                        }
                    }
                }
            }
            for attribute in element.attributes() {
                if let Some(value) = attribute.value() {
                    for mustache in value.mustaches() {
                        self.push_mustache_uses(mustache, locals, seen, uses, incomplete);
                    }
                }
            }
            self.collect_body_uses(
                &collect_children(element),
                &child_locals,
                seen,
                uses,
                incomplete,
            );
        }
    }

    fn push_mustache_uses(
        &self,
        mustache: wxml_syntax::Mustache<'_>,
        locals: &HashSet<String>,
        seen: &mut HashSet<String>,
        uses: &mut Vec<TemplateDataUse>,
        incomplete: &mut Option<String>,
    ) {
        let Some(expr) = mustache.expr() else {
            incomplete.get_or_insert_with(|| "an expression could not be parsed".to_owned());
            return;
        };
        let mut roots = Vec::new();
        collect_expression_roots(expr, self.text, &mut roots);
        for (name, range) in roots {
            if locals.contains(&name) || is_global(&name) {
                continue;
            }
            if uses.len() >= MAX_TEMPLATE_DATA_USES_PER_TEMPLATE {
                incomplete.get_or_insert_with(|| "too many data keys".to_owned());
                return;
            }
            if seen.insert(name.clone()) {
                uses.push(TemplateDataUse { name, range });
            }
            if uses.len() >= MAX_TEMPLATE_DATA_USES || uses.len() % 256 == 0 {
                // Keep the overall bound; per-template bound above is the
                // actionable one.
            }
        }
    }

    fn collect_import(&mut self, element: Element<'_>, kind: StaticImportKind) {
        if self.semantics.imports.len() >= MAX_STATIC_MODULES {
            self.semantics.truncated = true;
            self.issue(
                StaticIssueKind::LimitReached,
                as_source_range(element.text_range()),
            );
            return;
        }
        let Some(attribute) = element.attribute("src") else {
            self.issue(
                StaticIssueKind::MissingImportSrc,
                as_source_range(element.text_range()),
            );
            return;
        };
        let Some((src, src_range)) = static_value(attribute) else {
            self.issue(
                StaticIssueKind::MissingImportSrc,
                as_source_range(attribute.text_range()),
            );
            return;
        };
        let Some(src_range) = src_range else {
            self.issue(
                StaticIssueKind::DynamicImportSrc,
                as_source_range(attribute.text_range()),
            );
            return;
        };
        self.semantics.imports.push(StaticImport {
            kind,
            src,
            src_range,
            element_range: as_source_range(element.text_range()),
        });
    }

    fn collect_wxs(&mut self, element: Element<'_>) {
        if self.semantics.wxs.len() >= MAX_STATIC_MODULES {
            self.semantics.truncated = true;
            self.issue(
                StaticIssueKind::LimitReached,
                as_source_range(element.text_range()),
            );
            return;
        }
        let module_attribute = element.attribute("module");
        let module = module_attribute.and_then(static_value);
        let src_attribute = element.attribute("src");
        let src = src_attribute.and_then(static_value);
        let body_range = wxs_body_range(self.tree, element);
        if module.is_none() {
            self.issue(
                StaticIssueKind::MissingWxsModule,
                as_source_range(
                    module_attribute.map_or_else(|| element.text_range(), Attribute::text_range),
                ),
            );
        }
        if src.is_none() && body_range.is_none() {
            self.issue(
                StaticIssueKind::EmptyWxsBlock,
                as_source_range(element.text_range()),
            );
        }
        if src.is_some() && body_range.is_some() {
            self.issue(
                StaticIssueKind::AmbiguousWxsBlock,
                as_source_range(element.text_range()),
            );
        }
        let module_pair = module.and_then(|(value, range)| range.map(|range| (value, range)));
        let src_pair = src.and_then(|(value, range)| range.map(|range| (value, range)));
        self.semantics.wxs.push(WxsBlock {
            module: module_pair.as_ref().map(|(value, _)| value.clone()),
            module_range: module_pair.map(|(_, range)| range),
            src: src_pair.as_ref().map(|(value, _)| value.clone()),
            src_range: src_pair.map(|(_, range)| range),
            body_range,
            element_range: as_source_range(element.text_range()),
        });
    }
}

/// Parsed WXS text plus its summary, for callers that own the source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WxsModuleSource {
    /// Project-relative source path, or `None` for an inline block.
    pub path: Option<String>,
    /// WXS source text as written.
    pub text: String,
    /// Bounded static summary.
    pub summary: WxsSummary,
}

/// Summarizes WXS blocks after the caller has read their inline or `src` text.
#[must_use]
pub fn summarize_wxs_block(path: Option<String>, text: Option<&str>) -> Option<WxsModuleSource> {
    let text = text?;
    Some(WxsModuleSource {
        path,
        text: text.to_owned(),
        summary: crate::wxs::summarize_wxs(text),
    })
}

fn collect_children(element: Element<'_>) -> Vec<AstNode<'_>> {
    element.children().collect()
}

/// Returns the static value of an attribute plus the token range of the value.
///
/// `None` means the value is dynamic, mixed or missing.
fn static_value(attribute: Attribute<'_>) -> Option<(String, Option<SourceRange>)> {
    let value = attribute.value()?;
    let literal = value.literal_text()?;
    let trimmed = literal.trim();
    if trimmed.is_empty() {
        return Some((String::new(), None));
    }
    if value.mustaches().next().is_some() {
        return None;
    }
    let range = value.text_range();
    let quote = u32::from(value.is_quoted());
    let leading = u32::try_from(literal.len() - literal.trim_start().len()).ok()?;
    let start = range.start().checked_add(quote)?.checked_add(leading)?;
    let end = start.checked_add(u32::try_from(trimmed.len()).ok()?)?;
    if end > range.end() {
        return None;
    }
    Some((
        trimmed.to_owned(),
        SourceRange::from_bounds(start, end).ok(),
    ))
}

fn object_key(key: &ObjectKey, text: &str) -> Option<(String, SourceRange)> {
    match key {
        ObjectKey::Identifier { name } => {
            let range = SourceRange::new(*name);
            Some((
                text.get(range.start() as usize..range.end() as usize)?
                    .to_owned(),
                range,
            ))
        }
        ObjectKey::String { content, .. } => {
            let range = SourceRange::new(*content);
            Some((
                text.get(range.start() as usize..range.end() as usize)?
                    .to_owned(),
                range,
            ))
        }
        ObjectKey::Number { literal } => {
            let range = SourceRange::new(*literal);
            Some((
                text.get(range.start() as usize..range.end() as usize)?
                    .to_owned(),
                range,
            ))
        }
        _ => None,
    }
}

/// Collects the root data identifiers of an expression.
///
/// Property names and object keys are not data roots; call arguments and
/// index expressions are.
fn collect_expression_roots(expr: &Expr, text: &str, roots: &mut Vec<(String, SourceRange)>) {
    match expr.kind() {
        ExprKind::Identifier { name } => {
            let range = SourceRange::new(*name);
            if let Some(value) = text.get(range.start() as usize..range.end() as usize) {
                roots.push((value.to_owned(), range));
            }
        }
        ExprKind::Member { object, .. } => collect_expression_roots(object, text, roots),
        ExprKind::Index { object, index } => {
            collect_expression_roots(object, text, roots);
            collect_expression_roots(index, text, roots);
        }
        ExprKind::Call { callee, arguments } => {
            collect_expression_roots(callee, text, roots);
            for argument in arguments {
                collect_expression_roots(argument, text, roots);
            }
        }
        ExprKind::Unary { operand, .. } | ExprKind::Parenthesized { expr: operand } => {
            collect_expression_roots(operand, text, roots);
        }
        ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
            collect_expression_roots(left, text, roots);
            collect_expression_roots(right, text, roots);
        }
        ExprKind::Conditional {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_expression_roots(condition, text, roots);
            collect_expression_roots(then_expr, text, roots);
            collect_expression_roots(else_expr, text, roots);
        }
        ExprKind::Array(items) => {
            for item in items {
                collect_expression_roots(item, text, roots);
            }
        }
        ExprKind::Object(entries) => {
            for entry in entries {
                collect_expression_roots(&entry.value, text, roots);
            }
        }
        _ => {}
    }
}

/// Builtin names that are never template data keys.
fn is_global(name: &str) -> bool {
    matches!(
        name,
        "Math"
            | "JSON"
            | "Date"
            | "Number"
            | "String"
            | "Boolean"
            | "Array"
            | "Object"
            | "parseInt"
            | "parseFloat"
            | "isNaN"
            | "isFinite"
            | "console"
            | "undefined"
            | "NaN"
            | "Infinity"
    )
}

const fn as_source_range(range: TextRange) -> SourceRange {
    SourceRange::new(range)
}

fn text_range_slice(text: &str, range: TextRange) -> Option<&str> {
    text.get(range.start() as usize..range.end() as usize)
}

/// Finds the `>` closing a start tag, skipping quoted attribute values.
fn find_tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in text.char_indices() {
        match character {
            '"' | '\'' if quote.is_none() => quote = Some(character),
            character if Some(character) == quote => quote = None,
            '>' if quote.is_none() => return Some(index),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use wxml_syntax::parse;

    fn analyze(source: &str) -> (TemplateSemantics, SyntaxTree) {
        let parsed = parse(source);
        let semantics = analyze_template_semantics(&parsed.tree);
        (semantics, parsed.tree)
    }

    fn slice(tree: &SyntaxTree, range: SourceRange) -> &str {
        &tree.text()[range.start() as usize..range.end() as usize]
    }

    #[test]
    fn collects_definitions_calls_and_data_keys() {
        let source = r#"<template name="card"><view>{{ title }}</view><text wx:for="{{ rows }}">{{ item.name }}{{ index }}</text></template>
<template is="card" data="{{ { title: pageTitle, count: 1 } }}" />"#;
        let (semantics, tree) = analyze(source);
        assert_eq!(semantics.definitions.len(), 1);
        let definition = &semantics.definitions[0];
        assert_eq!(definition.name, "card");
        assert_eq!(slice(&tree, definition.name_range), "card");
        let names: Vec<&str> = definition
            .data_uses
            .iter()
            .map(|use_| use_.name.as_str())
            .collect();
        assert_eq!(names, vec!["title", "rows"]);
        assert_eq!(semantics.calls.len(), 1);
        let call = &semantics.calls[0];
        assert_eq!(call.name, "card");
        let data = call.data.as_ref().expect("data");
        let keys: Vec<&str> = data.keys.iter().map(|key| key.name.as_str()).collect();
        assert_eq!(keys, vec!["title", "count"]);
        assert_eq!(slice(&tree, data.keys[0].range), "title");
        assert!(data.dynamic.is_none());
        assert!(semantics.issues.is_empty(), "{:?}", semantics.issues);
    }

    #[test]
    fn collects_imports_includes_and_sources() {
        let source = r#"<import src="./shared.wxml" /><include src="parts/body.wxml" />"#;
        let (semantics, tree) = analyze(source);
        assert_eq!(semantics.imports.len(), 2);
        assert_eq!(semantics.imports[0].kind, StaticImportKind::Import);
        assert_eq!(semantics.imports[0].src, "./shared.wxml");
        assert_eq!(
            slice(&tree, semantics.imports[0].src_range),
            "./shared.wxml"
        );
        assert_eq!(semantics.imports[1].kind, StaticImportKind::Include);
        assert_eq!(semantics.imports[1].src, "parts/body.wxml");
    }

    #[test]
    fn collects_inline_and_src_wxs_blocks() {
        let source = r#"<wxs module="utils">
function format(value) { return value; }
module.exports = { format };
</wxs>
<wxs module="ext" src="./ext.wxs" />"#;
        let (semantics, tree) = analyze(source);
        assert_eq!(semantics.wxs.len(), 2);
        let inline = &semantics.wxs[0];
        assert_eq!(inline.module.as_deref(), Some("utils"));
        assert!(inline.src.is_none());
        let body = inline.body_range.expect("inline body");
        assert!(slice(&tree, body).starts_with("function format"));
        let external = &semantics.wxs[1];
        assert_eq!(external.src.as_deref(), Some("./ext.wxs"));
        assert!(external.body_range.is_none());
        assert_eq!(
            slice(&tree, external.src_range.expect("src range")),
            "./ext.wxs"
        );
    }

    #[test]
    fn resolves_only_local_specifiers() {
        assert_eq!(
            resolve_static_specifier("pages/index/index.wxml", "../shared.wxml").as_deref(),
            Some("pages/shared.wxml")
        );
        assert_eq!(
            resolve_static_specifier("index.wxml", "./parts/a.wxml").as_deref(),
            Some("parts/a.wxml")
        );
        for specifier in [
            "/abs.wxml",
            "https://example.com/a.wxml",
            "../../../outside.wxml",
            "",
        ] {
            assert_eq!(
                resolve_static_specifier("pages/index/index.wxml", specifier),
                None,
                "{specifier:?}"
            );
        }
    }

    #[test]
    fn dynamic_forms_are_reported_not_guessed() {
        let source = r#"<template is="{{ name }}" /><template is="card" data="{{ payload }}" /><import src="{{ target }}" /><wxs>1</wxs>"#;
        let (semantics, _) = analyze(source);
        let kinds: Vec<StaticIssueKind> = semantics.issues.iter().map(|issue| issue.kind).collect();
        assert!(kinds.contains(&StaticIssueKind::MissingTemplateIs));
        assert!(kinds.contains(&StaticIssueKind::DynamicTemplateData));
        assert!(kinds.contains(&StaticIssueKind::MissingImportSrc));
        assert!(kinds.contains(&StaticIssueKind::MissingWxsModule));
    }

    #[test]
    fn duplicate_template_names_keep_the_first() {
        let source = r#"<template name="x">a</template><template name="x">b</template>"#;
        let (semantics, _) = analyze(source);
        assert_eq!(semantics.definitions.len(), 2);
        assert_eq!(semantics.definition("x").expect("definition").name, "x");
        assert!(
            semantics
                .issues
                .iter()
                .any(|issue| issue.kind == StaticIssueKind::DuplicateTemplateName)
        );
    }

    #[test]
    fn template_body_uses_exclude_loop_and_global_names() {
        let source = r#"<template name="row"><view wx:for="{{ list }}" wx:for-item="row">{{ row.title }} {{ Math.max(row.count, total) }}</view></template>"#;
        let (semantics, _) = analyze(source);
        let names: Vec<&str> = semantics.definitions[0]
            .data_uses
            .iter()
            .map(|use_| use_.name.as_str())
            .collect();
        assert_eq!(names, vec!["list", "total"]);
    }
}
