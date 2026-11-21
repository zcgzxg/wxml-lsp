//! Shared WXML fact loading for the semantic tools.
//!
//! This module only orchestrates existing facts: `StructureFacts` for the
//! lossless tree/tag pairs, `analyze_template` for fragments/events/bindings,
//! `analyze_template_semantics` for template/import/WXS facts and the project
//! host for effective text, component metadata and bounded project views.
//! Nothing here parses TypeScript, re-implements component JSON or creates a
//! second source map.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use wxml_project::ProjectPath;
use wxml_syntax::expr::{Expr, ExprKind};
use wxml_text::{SourceRange, TextRange};
use wxml_types::{
    LocalBinding, LocalRole, RootKind, SourceRevision, TemplateAnalysis, TemplateSemantics,
    analyze_template_semantics,
};

use crate::project_host::{ViewComponent, ViewTarget};
use crate::server::Server;
use crate::structure::StructureFacts;
use crate::workspace::Document;

/// Maximum WXML documents included through `<include>` for one view.
pub(crate) const MAX_INCLUDED_DOCS: usize = 32;

/// Maximum project views scanned for references and workspace symbols.
pub(crate) const MAX_PROJECT_VIEWS: usize = 512;

/// Maximum estimated bytes of project WXML parsed for one query.
pub(crate) const MAX_PROJECT_WXML_BYTES: usize = 16 * 1024 * 1024;

/// Maximum distinct imported template documents read for one request.
pub(crate) const MAX_IMPORT_DOCS: usize = 64;

/// Maximum imported template bytes read for one request.
pub(crate) const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;

/// One request-scoped imported template document.
#[derive(Debug)]
pub(crate) struct ImportedDoc {
    /// Project-bound identity.
    pub(crate) file: wxml_types::FileId,
    /// Full effective text.
    pub(crate) text: Arc<str>,
    /// Template/import facts.
    pub(crate) semantics: TemplateSemantics,
}

/// Request-scoped cache for statically imported template documents.
///
/// Lookups are keyed by `FileId` and happen before any read or parse. The
/// cache enforces a total document/byte budget; once either bound is reached,
/// every later miss fails and callers must mark their result incomplete instead
/// of continuing to touch the filesystem.
#[derive(Debug, Default)]
pub(crate) struct ImportCache {
    docs: BTreeMap<wxml_types::FileId, Option<ImportedDoc>>,
    bytes: usize,
    exhausted: bool,
}

impl ImportCache {
    /// Loads one static import path, reusing an earlier parse of the same file.
    ///
    /// Returns `None` for an invalid path, an unreadable/unparseable file, or
    /// an exhausted budget. Check [`Self::is_exhausted`] to distinguish a
    /// budget stop from a per-file failure.
    pub(crate) fn load(
        &mut self,
        server: &Server,
        project: &wxml_types::ProjectId,
        path: &str,
    ) -> Option<&ImportedDoc> {
        let path = ProjectPath::new(path).ok()?;
        let file = server.host.file_id_in(project, &path).ok()?;
        if !self.docs.contains_key(&file) {
            if self.exhausted || self.docs.len() >= MAX_IMPORT_DOCS {
                self.exhausted = true;
                return None;
            }
            let loaded = server.host.file_text(&file).and_then(|text| {
                if self.bytes.saturating_add(text.len()) > MAX_IMPORT_BYTES {
                    self.exhausted = true;
                    return None;
                }
                self.bytes = self.bytes.saturating_add(text.len());
                parse_doc(file.clone(), &text).map(|doc| {
                    let semantics = analyze_template_semantics(doc.tree());
                    ImportedDoc {
                        file: doc.file,
                        text,
                        semantics,
                    }
                })
            });
            self.docs.insert(file.clone(), loaded);
        }
        self.docs.get(&file).and_then(Option::as_ref)
    }

    /// Returns whether a budget stop occurred.
    pub(crate) const fn is_exhausted(&self) -> bool {
        self.exhausted
    }
}

/// One parsed WXML document and its reusable structural facts.
#[derive(Debug)]
pub(crate) struct QueryDoc {
    /// Project-bound identity.
    pub(crate) file: wxml_types::FileId,
    /// Lossless tree plus linked-editing/document-symbol helpers.
    pub(crate) structure: StructureFacts,
}

impl QueryDoc {
    /// Returns the source text.
    pub(crate) fn text(&self) -> &str {
        self.structure.text()
    }

    /// Returns the lossless tree.
    pub(crate) const fn tree(&self) -> &wxml_syntax::SyntaxTree {
        self.structure.tree()
    }

    /// Returns a source range after validating it against this document.
    pub(crate) fn source_range(&self, range: TextRange) -> Option<SourceRange> {
        let start = usize::try_from(range.start()).ok()?;
        let end = usize::try_from(range.end()).ok()?;
        self.text().get(start..end).map(|_| SourceRange::new(range))
    }
}

/// Parses one document without accepting a truncated tree.
pub(crate) fn parse_doc(file: wxml_types::FileId, text: &str) -> Option<QueryDoc> {
    let parsed = wxml_syntax::parse(text);
    if parsed.truncated {
        return None;
    }
    Some(QueryDoc {
        file,
        structure: StructureFacts::from_tree(parsed.tree),
    })
}

/// One enclosing `<template name>` definition.
#[derive(Clone, Copy, Debug)]
struct TemplateBoundary {
    range: SourceRange,
    parent: Option<usize>,
}

/// Precomputed template-definition boundaries for one view.
///
/// Built with one pre-order element pass. Queries are `O(log n + depth)`, so
/// per-element/per-mustache scope checks no longer rescan the whole tree.
#[derive(Debug, Default)]
pub(crate) struct TemplateIndex {
    definitions: Vec<TemplateBoundary>,
    has_dynamic_loop: bool,
}

impl TemplateIndex {
    /// Builds the index in one element traversal.
    pub(crate) fn new(tree: &wxml_syntax::SyntaxTree) -> Self {
        let mut definitions: Vec<TemplateBoundary> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut has_dynamic_loop = false;
        for element in tree.document().elements() {
            let start = element.text_range().start();
            while let Some(top) = stack.last().copied() {
                if definitions
                    .get(top)
                    .is_some_and(|boundary| boundary.range.end() < start)
                {
                    stack.pop();
                } else {
                    break;
                }
            }
            if element.name() == Some("template") && element.attribute("name").is_some() {
                let Some(range) = SourceRange::from_bounds(
                    element.text_range().start(),
                    element.text_range().end(),
                )
                .ok() else {
                    continue;
                };
                definitions.push(TemplateBoundary {
                    range,
                    parent: stack.last().copied(),
                });
                stack.push(definitions.len().saturating_sub(1));
            }
            if element.attribute("wx:for").is_some() {
                let has_static = element
                    .attribute("wx:for")
                    .and_then(wxml_syntax::Attribute::value)
                    .and_then(|value| value.mustaches().next())
                    .and_then(wxml_syntax::Mustache::inner_range)
                    .is_some();
                if !has_static {
                    has_dynamic_loop = true;
                }
            }
        }
        Self {
            definitions,
            has_dynamic_loop,
        }
    }

    /// Returns the innermost template definition containing `range`.
    pub(crate) fn boundary(&self, range: SourceRange) -> Option<SourceRange> {
        let position = self
            .definitions
            .partition_point(|boundary| boundary.range.start() <= range.start());
        let mut current = position.checked_sub(1);
        while let Some(index) = current {
            let boundary = self.definitions.get(index)?;
            if boundary.range.start() <= range.start() && boundary.range.end() >= range.end() {
                return Some(boundary.range);
            }
            current = boundary.parent;
        }
        None
    }

    /// Returns whether `range` is inside any template definition body.
    pub(crate) fn inside(&self, range: SourceRange) -> bool {
        self.boundary(range).is_some()
    }

    /// Returns whether a `wx:for` has no static mustache source.
    pub(crate) const fn has_dynamic_loop(&self) -> bool {
        self.has_dynamic_loop
    }
}

/// Static facts for one view template.
#[derive(Debug)]
pub(crate) struct ViewFacts {
    /// Parsed template.
    pub(crate) doc: QueryDoc,
    /// Discovered owner, when the template is a page/component.
    pub(crate) target: Option<ViewTarget>,
    /// Custom components registered for this view.
    pub(crate) components: Vec<ViewComponent>,
    /// Fragments, events and component usages from T.
    pub(crate) analysis: TemplateAnalysis,
    /// Template/import/WXS facts from T.
    pub(crate) semantics: TemplateSemantics,
    /// Static `<include>` documents in the caller's scope.
    pub(crate) includes: Vec<QueryDoc>,
    /// `true` only when every include resolved and was parsed.
    pub(crate) includes_complete: bool,
    /// Loop scopes in source order.
    pub(crate) loops: Vec<LoopInfo>,
    /// Precomputed `<template name>` boundaries and dynamic loop flag.
    pub(crate) template_index: TemplateIndex,
}

impl ViewFacts {
    /// Returns the root kind of the view, defaulting to a page.
    pub(crate) fn root_kind(&self) -> RootKind {
        self.target
            .as_ref()
            .map_or(RootKind::Page, |target| target.owner)
    }

    /// Returns `true` when template bodies could contain references this view
    /// cannot attribute to the caller's data/methods.
    pub(crate) const fn has_template_definitions(&self) -> bool {
        !self.semantics.definitions.is_empty()
    }

    /// Returns the innermost template definition containing `range`.
    pub(crate) fn template_boundary(&self, range: SourceRange) -> Option<SourceRange> {
        self.template_index.boundary(range)
    }

    /// Returns whether `range` is inside a template definition body.
    pub(crate) fn inside_template_definition(&self, range: SourceRange) -> bool {
        self.template_index.inside(range)
    }

    /// Returns whether any `wx:for` lacks a static source expression.
    pub(crate) const fn has_dynamic_loop(&self) -> bool {
        self.template_index.has_dynamic_loop()
    }
}

/// Builds static facts for an open template.
pub(crate) fn view_facts(server: &Server, document: &Document) -> Option<ViewFacts> {
    view_facts_from(
        server,
        document.file(),
        document.text(),
        document.revision().get(),
    )
}

/// Builds view facts for a project file, using overlay-first effective text.
pub(crate) fn view_facts_for_file(server: &Server, file: &wxml_types::FileId) -> Option<ViewFacts> {
    let text = server.host.file_text(file)?;
    view_facts_from(server, file, &text, 1)
}

/// Shared parser/analyzer setup for open and closed views.
fn view_facts_from(
    server: &Server,
    file: &wxml_types::FileId,
    text: &str,
    revision: u64,
) -> Option<ViewFacts> {
    let doc = parse_doc(file.clone(), text)?;
    let target = server.host.view_for_file(file).ok();
    let components = live_components(server, file);
    let root = target
        .as_ref()
        .map_or(RootKind::Page, |target| target.owner);
    let analysis = wxml_types::TemplateAnalyzer::new(root, SourceRevision::new(revision))
        .with_component_tags(components.iter().map(|component| component.name.clone()))
        .analyze(doc.tree());
    let semantics = analyze_template_semantics(doc.tree());
    let template_index = TemplateIndex::new(doc.tree());
    let loops = collect_loops(&doc, &template_index);
    let (includes, includes_complete) = collect_includes(server, file, &semantics);
    Some(ViewFacts {
        doc,
        target,
        components,
        analysis,
        semantics,
        includes,
        includes_complete,
        loops,
        template_index,
    })
}

/// Returns the view's component table with live (overlay-first) properties.
fn live_components(server: &Server, file: &wxml_types::FileId) -> Vec<ViewComponent> {
    let mut components = server.host.view_components_for(file);
    for component in &mut components {
        if let Some(facts) = server.host.component_properties_for(file, &component.name) {
            component.apply_live_facts(facts);
        }
    }
    components
}

/// Resolves static `<include>` documents, bounded and without escaping the
/// project.
pub(crate) fn collect_includes(
    server: &Server,
    file: &wxml_types::FileId,
    semantics: &TemplateSemantics,
) -> (Vec<QueryDoc>, bool) {
    let mut docs = Vec::new();
    let mut complete = true;
    let mut visited = BTreeSet::new();
    let project = file.project().clone();
    for import in &semantics.imports {
        if import.kind != wxml_types::StaticImportKind::Include {
            continue;
        }
        if docs.len() >= MAX_INCLUDED_DOCS {
            complete = false;
            break;
        }
        let Some(path) = wxml_types::resolve_static_specifier(file.path(), &import.src) else {
            complete = false;
            continue;
        };
        let Ok(path) = ProjectPath::new(&path) else {
            complete = false;
            continue;
        };
        let Ok(included_file) = server.host.file_id_in(&project, &path) else {
            complete = false;
            continue;
        };
        if !visited.insert(included_file.clone()) {
            continue;
        }
        let Some(text) = server.host.file_text(&included_file) else {
            complete = false;
            continue;
        };
        match parse_doc(included_file, &text) {
            Some(doc) => docs.push(doc),
            None => complete = false,
        }
    }
    (docs, complete)
}

/// One project view template plus the component table of its own JSON scope.
#[derive(Debug)]
pub(crate) struct ProjectView {
    /// Parsed template.
    pub(crate) doc: QueryDoc,
    /// Registered custom components.
    pub(crate) components: Vec<ViewComponent>,
    /// Template/import facts for name visibility.
    pub(crate) semantics: TemplateSemantics,
}

/// Bounded project-wide view list.
#[derive(Debug, Default)]
pub(crate) struct ProjectViews {
    /// Views in path order.
    pub(crate) views: Vec<ProjectView>,
    /// `false` when a budget or unreadable file hid a view.
    pub(crate) complete: bool,
    /// Short path-free reason for incompleteness.
    pub(crate) reason: Option<&'static str>,
}

/// Loads every discovered project view, including the current open document.
pub(crate) fn project_views(server: &Server, current: &ViewFacts) -> ProjectViews {
    let mut views = Vec::new();
    let mut complete;
    let mut reason;
    let mut seen = BTreeSet::new();
    let mut bytes = 0_usize;
    seen.insert(current.doc.file.clone());
    views.push(ProjectView {
        doc: QueryDoc {
            file: current.doc.file.clone(),
            structure: StructureFacts::from_tree(wxml_syntax::parse(current.doc.text()).tree),
        },
        components: current.components.clone(),
        semantics: current.semantics.clone(),
    });
    if let Some(templates) = server.host.view_templates(&current.doc.file) {
        complete = templates.complete;
        reason = templates.reason;
        for template in templates.templates {
            if seen.contains(&template.file) {
                continue;
            }
            if views.len() >= MAX_PROJECT_VIEWS
                || bytes.saturating_add(template.text.len()) > MAX_PROJECT_WXML_BYTES
            {
                complete = false;
                reason = Some("the project WXML list is truncated");
                break;
            }
            bytes = bytes.saturating_add(template.text.len());
            let Some(doc) = parse_doc(template.file.clone(), &template.text) else {
                complete = false;
                reason = Some("a project template could not be parsed");
                continue;
            };
            let semantics = analyze_template_semantics(doc.tree());
            let components = live_components(server, &template.file);
            seen.insert(template.file);
            views.push(ProjectView {
                doc,
                components,
                semantics,
            });
        }
    } else {
        complete = false;
        reason = Some("the project has no queryable WXML index");
    }
    ProjectViews {
        views,
        complete,
        reason,
    }
}

/// One `wx:for` binding declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BindingName {
    /// Name as written.
    pub(crate) name: String,
    /// Explicit `wx:for-item`/`wx:for-index` value token.
    pub(crate) declaration: Option<SourceRange>,
    /// Whole attribute range, used as a rename placeholder for default names.
    pub(crate) attribute: Option<SourceRange>,
    /// Value content range (inside quotes), including an empty value.
    pub(crate) value: Option<SourceRange>,
}

/// One `wx:for` element scope.
#[derive(Clone, Debug)]
pub(crate) struct LoopInfo {
    /// Owning template file.
    pub(crate) file: wxml_types::FileId,
    /// Whole element range.
    pub(crate) element_range: SourceRange,
    /// Range of the static `wx:for` expression.
    pub(crate) source_range: SourceRange,
    /// Item binding.
    pub(crate) item: BindingName,
    /// Index binding.
    pub(crate) index: BindingName,
    /// End of the tag-name token, used for default-binding attribute insertion.
    pub(crate) tag_name_end: u32,
    /// Whole `wx:for` attribute range, used as a default binding placeholder.
    pub(crate) for_attribute_range: SourceRange,
    /// Enclosing loop in the same template definition scope.
    pub(crate) parent: Option<usize>,
    /// Enclosing loop ignoring template-definition boundaries.
    pub(crate) outer_parent: Option<usize>,
    /// Innermost template definition containing this loop element.
    pub(crate) boundary: Option<SourceRange>,
}

/// Identity of one loop binding, independent of its current name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct LoopIdentity {
    /// Owning file.
    pub(crate) file: wxml_types::FileId,
    /// Start offset of the `wx:for` expression.
    pub(crate) source_start: u32,
    /// Item or index role.
    pub(crate) role: LocalRole,
}

/// Returns the `wx:for` expression range of an element, if it has one.
pub(crate) fn loop_source_range(element: wxml_syntax::Element<'_>) -> Option<TextRange> {
    let attribute = element.attribute("wx:for")?;
    let value = attribute.value()?;
    let mustache = value.mustaches().next()?;
    mustache.inner_range()
}

/// Returns the trimmed value token of a literal attribute.
fn literal_token(attribute: wxml_syntax::Attribute<'_>) -> Option<SourceRange> {
    let value = attribute.value()?;
    let literal = value.literal_text()?;
    let trimmed = literal.trim();
    if trimmed.is_empty() {
        return None;
    }
    let leading = literal.len().saturating_sub(literal.trim_start().len());
    let quote = usize::from(value.is_quoted());
    let range = value.text_range();
    let start = range
        .start()
        .checked_add(u32::try_from(quote + leading).ok()?)?;
    let end = start.checked_add(u32::try_from(trimmed.len()).ok()?)?;
    SourceRange::from_bounds(start, end).ok()
}

/// Collects loop scopes with their ancestor chains in source order.
pub(crate) fn collect_loops(doc: &QueryDoc, index: &TemplateIndex) -> Vec<LoopInfo> {
    let tree = doc.tree();
    let mut loops = Vec::new();
    let mut stack = Vec::new();
    let mut outer_stack = Vec::new();
    let children: Vec<wxml_syntax::AstNode<'_>> = tree.document().children().collect();
    for child in children {
        visit_loop_node(doc, index, child, &mut stack, &mut outer_stack, &mut loops);
    }
    loops
}

fn visit_loop_node(
    doc: &QueryDoc,
    template_index: &TemplateIndex,
    node: wxml_syntax::AstNode<'_>,
    stack: &mut Vec<usize>,
    outer_stack: &mut Vec<usize>,
    loops: &mut Vec<LoopInfo>,
) {
    let wxml_syntax::AstNode::Element(element) = node else {
        return;
    };
    let parent = stack.last().copied();
    let outer_parent = outer_stack.last().copied();
    let pushed = if let Some(source) = loop_source_range(element) {
        let Some(source_range) = doc.source_range(source) else {
            return;
        };
        let item_attr = element.attribute("wx:for-item");
        let index_attr = element.attribute("wx:for-index");
        let item = binding_name(item_attr, "item");
        let index = binding_name(index_attr, "index");
        let Some(element_range) = doc.source_range(element.text_range()) else {
            return;
        };
        let tag_name_end = element
            .name_range()
            .map_or_else(|| element_range.start(), |range| range.end());
        let for_attribute_range = element
            .attribute("wx:for")
            .and_then(|attribute| {
                SourceRange::from_bounds(
                    attribute.text_range().start(),
                    attribute.text_range().end(),
                )
                .ok()
            })
            .unwrap_or(element_range);
        loops.push(LoopInfo {
            file: doc.file.clone(),
            element_range,
            source_range,
            item,
            index,
            tag_name_end,
            for_attribute_range,
            parent,
            outer_parent,
            boundary: template_index.boundary(element_range),
        });
        let loop_index = loops.len().saturating_sub(1);
        stack.push(loop_index);
        outer_stack.push(loop_index);
        true
    } else {
        false
    };
    let is_template_definition =
        element.name() == Some("template") && element.attribute("name").is_some();
    let saved_stack = if is_template_definition {
        Some(std::mem::take(stack))
    } else {
        None
    };
    let children: Vec<wxml_syntax::AstNode<'_>> = element.children().collect();
    for child in children {
        visit_loop_node(doc, template_index, child, stack, outer_stack, loops);
    }
    if let Some(saved) = saved_stack {
        *stack = saved;
    }
    if pushed {
        stack.pop();
        outer_stack.pop();
    }
}

fn binding_name(attribute: Option<wxml_syntax::Attribute<'_>>, default: &str) -> BindingName {
    let Some(attribute) = attribute else {
        return BindingName {
            name: default.to_owned(),
            declaration: None,
            attribute: None,
            value: None,
        };
    };
    let attribute_range =
        SourceRange::from_bounds(attribute.text_range().start(), attribute.text_range().end()).ok();
    let Some(value) = attribute.value() else {
        return BindingName {
            name: default.to_owned(),
            declaration: None,
            attribute: attribute_range,
            value: None,
        };
    };
    let literal = value.literal_text().map_or_default(str::trim);
    let quote = u32::from(value.is_quoted());
    let value_range = SourceRange::from_bounds(
        value.text_range().start().saturating_add(quote),
        value.text_range().end().saturating_sub(quote),
    )
    .ok();
    if literal.is_empty() {
        return BindingName {
            name: default.to_owned(),
            declaration: None,
            attribute: attribute_range,
            value: value_range,
        };
    }
    BindingName {
        name: literal.to_owned(),
        declaration: literal_token(attribute),
        attribute: attribute_range,
        value: value_range,
    }
}

/// Finds the loop scope that produced one analyzer binding.
pub(crate) fn loop_for_binding(loops: &[LoopInfo], binding: &LocalBinding) -> Option<usize> {
    loops.iter().position(|loop_info| {
        loop_info.source_range == binding.source_range
            && match binding.role {
                LocalRole::Item => loop_info.item.name == binding.name,
                LocalRole::Index => loop_info.index.name == binding.name,
                _ => false,
            }
    })
}

/// Returns the effective binding identity of `name` in one fragment chain.
pub(crate) fn effective_binding<'a>(
    bindings: &'a [LocalBinding],
    name: &str,
) -> Option<&'a LocalBinding> {
    bindings.iter().rev().find(|binding| binding.name == name)
}

/// Returns the effective loop binding for `name` in an ancestor chain.
pub(crate) fn effective_loop_binding(
    loops: &[LoopInfo],
    parents: &[usize],
    name: &str,
) -> Option<(usize, LocalRole)> {
    for index in parents.iter().rev() {
        let loop_info = loops.get(*index)?;
        if loop_info.item.name == name {
            return Some((*index, LocalRole::Item));
        }
        if loop_info.index.name == name {
            return Some((*index, LocalRole::Index));
        }
    }
    None
}

/// One identifier chain rooted at a bare identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Chain {
    /// Identifier segments, root first.
    pub(crate) segments: Vec<(String, SourceRange)>,
    /// The chain came from an object shorthand and cannot be renamed alone.
    pub(crate) shorthand: bool,
}

/// One computed access whose dynamic/static key could hide the target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComputedAccess {
    /// Identifier prefix before the index.
    pub(crate) prefix: Vec<String>,
    /// Index range (string content or whole expression).
    pub(crate) range: SourceRange,
    /// `Some(name)` for a static string index, `None` for a dynamic one.
    pub(crate) key: Option<String>,
}

/// All identifier chains and computed accesses in one expression.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ChainScan {
    /// Chains rooted at a bare identifier.
    pub(crate) chains: Vec<Chain>,
    /// Computed accesses after an identifier prefix.
    pub(crate) computed: Vec<ComputedAccess>,
}

/// Scans one parsed expression using the existing syntax AST.
pub(crate) fn scan_chains(expr: &Expr) -> ChainScan {
    let mut scan = ChainScan::default();
    visit_value(expr, &mut scan);
    scan
}

fn visit_value(expr: &Expr, scan: &mut ChainScan) {
    match expr.kind() {
        ExprKind::Identifier { name } => {
            scan.chains.push(Chain {
                segments: vec![(String::new(), SourceRange::new(*name))],
                shorthand: false,
            });
        }
        ExprKind::Member { .. } => match flatten_chain(expr) {
            FlatChain::Rooted(segments) => {
                if !segments.is_empty() {
                    scan.chains.push(Chain {
                        segments,
                        shorthand: false,
                    });
                }
            }
            FlatChain::NotRooted(base) => visit_value(base, scan),
        },
        ExprKind::Index { object, index } => {
            if let FlatChain::Rooted(segments) = flatten_chain(object) {
                let names: Vec<String> = segments.iter().map(|(name, _)| name.clone()).collect();
                match index.kind() {
                    ExprKind::String { content, .. } => {
                        scan.computed.push(ComputedAccess {
                            prefix: names,
                            range: SourceRange::new(*content),
                            key: Some(String::new()),
                        });
                    }
                    _ => {
                        scan.computed.push(ComputedAccess {
                            prefix: names,
                            range: SourceRange::new(index.range()),
                            key: None,
                        });
                    }
                }
            }
            visit_value(object, scan);
            visit_value(index, scan);
        }
        ExprKind::Call { callee, arguments } => {
            visit_value(callee, scan);
            for argument in arguments {
                visit_value(argument, scan);
            }
        }
        ExprKind::Unary { operand, .. } => visit_value(operand, scan),
        ExprKind::Binary { left, right, .. } | ExprKind::Logical { left, right, .. } => {
            visit_value(left, scan);
            visit_value(right, scan);
        }
        ExprKind::Conditional {
            condition,
            then_expr,
            else_expr,
        } => {
            visit_value(condition, scan);
            visit_value(then_expr, scan);
            visit_value(else_expr, scan);
        }
        ExprKind::Parenthesized { expr } => visit_value(expr, scan),
        ExprKind::Array(items) => {
            for item in items {
                visit_value(item, scan);
            }
        }
        ExprKind::Object(entries) => {
            for entry in entries {
                let before = scan.chains.len();
                visit_value(&entry.value, scan);
                if entry.shorthand {
                    for chain in scan.chains.iter_mut().skip(before) {
                        chain.shorthand = true;
                    }
                }
            }
        }
        _ => {}
    }
}

/// The result of flattening one member chain.
enum FlatChain<'a> {
    /// A chain rooted at a bare identifier.
    Rooted(Vec<(String, SourceRange)>),
    /// The base expression that is not a bare identifier.
    NotRooted(&'a Expr),
}

fn flatten_chain(expr: &Expr) -> FlatChain<'_> {
    match expr.kind() {
        ExprKind::Identifier { name } => {
            FlatChain::Rooted(vec![(String::new(), SourceRange::new(*name))])
        }
        ExprKind::Member {
            object, property, ..
        } => {
            let mut chain = match flatten_chain(object) {
                FlatChain::Rooted(chain) => chain,
                FlatChain::NotRooted(base) => return FlatChain::NotRooted(base),
            };
            let ExprKind::Identifier { name } = property.kind() else {
                return FlatChain::NotRooted(property);
            };
            chain.push((String::new(), SourceRange::new(*name)));
            FlatChain::Rooted(chain)
        }
        _ => FlatChain::NotRooted(expr),
    }
}

/// Returns the source text of a range from a document.
pub(crate) fn range_text(doc: &QueryDoc, range: SourceRange) -> Option<&str> {
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    doc.text().get(start..end)
}

/// Resolves the text of each chain segment and computed access.
///
/// [`scan_chains`] intentionally cannot borrow the document because the parsed
/// `Expr` owns ranges only. This second pass fills names from the same exact
/// ranges and drops chains whose text is not a valid identifier.
pub(crate) fn resolve_chain_text(doc: &QueryDoc, scan: &mut ChainScan) {
    scan.chains.retain_mut(|chain| {
        chain
            .segments
            .retain(|(_, range)| range_text(doc, *range).is_some_and(is_identifier_text));
        !chain.segments.is_empty()
            && chain
                .segments
                .iter()
                .all(|(_, range)| range_text(doc, *range).is_some())
    });
    for chain in &mut scan.chains {
        for (name, range) in &mut chain.segments {
            range_text(doc, *range).unwrap_or_default().clone_into(name);
        }
    }
    for access in &mut scan.computed {
        if let Some(key) = access.key.as_mut() {
            if let Some(text) = range_text(doc, access.range) {
                text.clone_into(key);
            } else {
                access.key = None;
            }
        }
    }
}

/// Returns `true` for an identifier-shaped ASCII token.
pub(crate) fn is_identifier_text(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
}

/// Changed from an empty chain scan to the document-aware variant.
pub(crate) fn scan_expression(doc: &QueryDoc, expr: &Expr) -> ChainScan {
    let mut scan = scan_chains(expr);
    resolve_chain_text(doc, &mut scan);
    scan
}

/// Loads a project-relative WXML document for template import resolution.
pub(crate) fn load_project_doc(
    server: &Server,
    project: &wxml_types::ProjectId,
    path: &str,
) -> Option<QueryDoc> {
    let path = ProjectPath::new(path).ok()?;
    let file = server.host.file_id_in(project, &path).ok()?;
    let text = server.host.file_text(&file)?;
    parse_doc(file, &text)
}
