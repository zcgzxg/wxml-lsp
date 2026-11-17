//! Shared WXML fact loading for the semantic tools.
//!
//! This module only orchestrates existing facts: `StructureFacts` for the
//! lossless tree/tag pairs, `analyze_template` for fragments/events/bindings,
//! `analyze_template_semantics` for template/import/WXS facts and the project
//! host for effective text, component metadata and bounded project views.
//! Nothing here parses TypeScript, re-implements component JSON or creates a
//! second source map.

use std::collections::BTreeSet;

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
    pub(crate) fn has_template_definitions(&self) -> bool {
        !self.semantics.definitions.is_empty()
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
    let loops = collect_loops(&doc);
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
pub(crate) fn collect_loops(doc: &QueryDoc) -> Vec<LoopInfo> {
    let tree = doc.tree();
    let mut loops = Vec::new();
    let mut stack = Vec::new();
    let children: Vec<wxml_syntax::AstNode<'_>> = tree.document().children().collect();
    for child in children {
        visit_loop_node(doc, child, &mut stack, &mut loops);
    }
    loops
}

fn visit_loop_node(
    doc: &QueryDoc,
    node: wxml_syntax::AstNode<'_>,
    stack: &mut Vec<usize>,
    loops: &mut Vec<LoopInfo>,
) {
    let wxml_syntax::AstNode::Element(element) = node else {
        return;
    };
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
        });
        let index = loops.len().saturating_sub(1);
        stack.push(index);
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
        visit_loop_node(doc, child, stack, loops);
    }
    if let Some(saved) = saved_stack {
        *stack = saved;
    }
    if pushed {
        stack.pop();
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
    let literal = value.literal_text().map(str::trim).unwrap_or_default();
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
