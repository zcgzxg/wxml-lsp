//! Template scope analysis that turns mustaches into C1.1 check fragments.
//!
//! The analyzer walks the `wxml-syntax` AST in source order and produces:
//!
//! - [`CheckFragment`]s for every data mustache, with the `wx:for` bindings and
//!   `wx:if` guard chains that are in scope at the mustache;
//! - [`ComponentUsage`]s for elements whose tag is a configured custom
//!   component;
//! - [`EventHandler`]s for static event bindings;
//! - [`UnsupportedFeature`]s for dynamic events, unresolved expressions and
//!   template forms that this phase does not analyze.
//!
//! Scope rules:
//!
//! - A `wx:for` expression is analyzed **before** its item/index bindings are
//!   pushed, so it sees the enclosing scope. All other attributes and the
//!   element content see the new bindings.
//! - Default bindings are `item` and `index`; `wx:for-item` and
//!   `wx:for-index` replace them. Nested loops shadow outer bindings and the
//!   bindings are popped when the element ends.
//! - A `wx:for` whose value is not exactly one parseable mustache binds
//!   nothing and records [`UnsupportedKind::DynamicForBinding`]; no type is
//!   guessed.
//! - Siblings form `wx:if`/`wx:elif`/`wx:else` chains. Text, whitespace,
//!   comments and mustaches between elements do not break a chain, but any
//!   other element does. A chain whose condition is dynamic or unparseable is
//!   not attached to any fragment at all.
//! - `wx:key` is not a data expression.
//! - `wxs`, `template`, `import` and `include` are reported and their content
//!   is not analyzed. These names are matched exactly; `<WXS>` is an ordinary
//!   element name, matching the syntax crate's raw-text rule.
//!
//! Fragment ids are assigned after traversal in source-range order (`f0`,
//! `f1`, ...), so they stay stable even though `wx:for` and guard attributes
//! are analyzed before other attributes. Identical expressions in different
//! places remain separate fragments.
//!
//! At most [`MAX_FRAGMENTS`] fragments are produced for one document. Beyond
//! that the analyzer records [`UnsupportedKind::FragmentLimit`], sets
//! [`TemplateAnalysis::truncated`] and stops adding fragments, keeping time and
//! memory bounded for hostile templates. The limit is applied in traversal
//! order (a `wx:for` attribute is visited before other attributes so its
//! expression sees the outer scope); ids are assigned afterwards in source
//! order.
//!
//! The analyzer receives only the syntax tree. Diagnostics that the parser
//! reports but does not encode in the tree (for example a trailing token after
//! a complete expression) cannot be detected here; missing nodes, unconsumed
//! expression ranges and unclosed mustaches are detected.

use std::collections::HashSet;

use wxml_syntax::{
    AstNode, Attribute, AttributeValue, Element, Expr, ExprKind, Mustache, SyntaxTree,
};
use wxml_text::{SourceRange, TextRange};

use crate::check::{
    AttributeValueKind, CheckFragment, ComponentAttribute, ComponentUsage, EventBindingKind,
    EventHandler, GuardBranch, GuardChain, GuardCondition, LocalBinding, LocalRole, RootKind,
    TemplateAnalysis, UnsupportedFeature, UnsupportedKind,
};
use crate::contract::SourceRevision;

/// Maximum number of [`CheckFragment`]s produced for one document.
///
/// This mirrors the C1 default `maxFragments` budget. When the limit is
/// reached, later mustaches are skipped, the first skipped expression is
/// recorded as [`UnsupportedKind::FragmentLimit`] and
/// [`TemplateAnalysis::truncated`] is set. The analysis remains usable; callers
/// must not present it as complete.
pub const MAX_FRAGMENTS: usize = 2048;

/// Maximum `wx:for` bindings (one nesting chain) sent for one fragment.
///
/// Mirrors `MAX_LOCAL_BINDINGS` in `backends/typescript/src/protocol.ts`.
pub const MAX_BINDINGS: usize = 64;

/// Maximum conditions in one `wx:if`/`wx:elif`/`wx:else` chain.
///
/// Mirrors `MAX_GUARD_CONDITIONS` in `backends/typescript/src/protocol.ts`.
pub const MAX_GUARD_CONDITIONS: usize = 64;

/// Maximum nested guard chains applied to one fragment.
///
/// Mirrors `MAX_GUARD_CHAINS` in `backends/typescript/src/protocol.ts`.
pub const MAX_GUARD_CHAINS: usize = 32;

/// Maximum custom-component usages collected for one document.
///
/// The request byte budget also bounds the attributes of these usages.
pub const MAX_COMPONENTS: usize = 512;

/// Reusable configuration for template analysis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateAnalyzer {
    root: RootKind,
    source_revision: SourceRevision,
    component_tags: HashSet<String>,
}

impl TemplateAnalyzer {
    /// Creates an analyzer for one page/component root and source revision.
    ///
    /// No custom-component tags are configured; use
    /// [`TemplateAnalyzer::with_component_tags`] to collect
    /// [`ComponentUsage`]s for `usingComponents` entries.
    #[must_use]
    pub fn new(root: RootKind, source_revision: SourceRevision) -> Self {
        Self {
            root,
            source_revision,
            component_tags: HashSet::new(),
        }
    }

    /// Configures the custom-component tags for this analyzer.
    ///
    /// Elements whose name is exactly one of `tags` produce a
    /// [`ComponentUsage`]. Tag matching is case-sensitive and does not include
    /// built-in WXML components.
    #[must_use]
    pub fn with_component_tags(mut self, tags: impl IntoIterator<Item = String>) -> Self {
        self.component_tags = tags.into_iter().collect();
        self
    }

    /// Returns the configured root kind.
    #[must_use]
    pub const fn root(&self) -> RootKind {
        self.root
    }

    /// Returns the configured source revision.
    #[must_use]
    pub const fn source_revision(&self) -> SourceRevision {
        self.source_revision
    }

    /// Analyzes a parsed template.
    ///
    /// This is a pure function of the tree and configuration: analyzing the
    /// same tree twice produces the same fragments, components, events and
    /// unsupported records. Fragment output is capped at [`MAX_FRAGMENTS`];
    /// check [`TemplateAnalysis::truncated`] before treating the result as
    /// complete.
    #[must_use]
    pub fn analyze(&self, tree: &SyntaxTree) -> TemplateAnalysis {
        Engine {
            tree,
            root: self.root,
            revision: self.source_revision.get(),
            component_tags: &self.component_tags,
            bindings: Vec::new(),
            guards: Vec::new(),
            fragments: Vec::new(),
            components: Vec::new(),
            events: Vec::new(),
            unsupported: Vec::new(),
            truncated: false,
            components_truncated: false,
        }
        .run()
    }
}

/// Convenience wrapper around [`TemplateAnalyzer`].
#[must_use]
pub fn analyze_template(
    tree: &SyntaxTree,
    root: RootKind,
    source_revision: SourceRevision,
) -> TemplateAnalysis {
    TemplateAnalyzer::new(root, source_revision).analyze(tree)
}

struct Engine<'a> {
    tree: &'a SyntaxTree,
    root: RootKind,
    revision: u64,
    component_tags: &'a HashSet<String>,
    bindings: Vec<LocalBinding>,
    guards: Vec<GuardChain>,
    fragments: Vec<CheckFragment>,
    components: Vec<ComponentUsage>,
    events: Vec<EventHandler>,
    unsupported: Vec<UnsupportedFeature>,
    truncated: bool,
    components_truncated: bool,
}

/// State of one `wx:if`/`wx:elif`/`wx:else` chain while its siblings are
/// visited.
enum ChainState {
    /// No chain is open.
    Empty,
    /// A chain is open and its conditions are known.
    Resolved(GuardChain),
    /// A chain is open but its condition is dynamic or unparseable, so no
    /// guard is attached to any sibling of it.
    Unresolvable,
}

impl<'a> Engine<'a> {
    fn run(mut self) -> TemplateAnalysis {
        let children: Vec<AstNode<'a>> = self.tree.document().children().collect();
        self.analyze_children(&children);
        self.finish()
    }

    fn finish(mut self) -> TemplateAnalysis {
        self.fragments
            .sort_by_key(|fragment| (fragment.wxml_range.start(), fragment.wxml_range.end()));
        for (index, fragment) in self.fragments.iter_mut().enumerate() {
            fragment.fragment_id = format!("f{index}");
        }
        TemplateAnalysis {
            fragments: self.fragments,
            components: self.components,
            events: self.events,
            unsupported: self.unsupported,
            truncated: self.truncated,
        }
    }

    /// Visits a container's children as a sibling sequence.
    ///
    /// Only elements interact with `wx:if` chains; text, comments and
    /// mustaches are analyzed in place and leave the open chain untouched.
    fn analyze_children(&mut self, children: &[AstNode<'a>]) {
        let mut state = ChainState::Empty;
        for node in children {
            let AstNode::Element(element) = *node else {
                self.analyze_node(*node);
                continue;
            };
            let selected = self.select_guard(&mut state, element);
            self.analyze_element(element, selected);
        }
    }

    /// Advances the sibling guard chain for `element` and returns the chain the
    /// element's own content belongs to, if any.
    fn select_guard(&mut self, state: &mut ChainState, element: Element<'a>) -> Option<GuardChain> {
        match guard_branch(element) {
            Some(GuardBranch::If) => {
                let Some(condition) = self.guard_condition(element, GuardBranch::If) else {
                    self.record_guard_condition(element, GuardBranch::If);
                    *state = ChainState::Unresolvable;
                    return None;
                };
                let chain = GuardChain {
                    conditions: vec![condition],
                    selected: 0,
                };
                *state = ChainState::Resolved(chain.clone());
                Some(chain)
            }
            Some(GuardBranch::Elif) => {
                let ChainState::Resolved(chain) = state else {
                    *state = ChainState::Unresolvable;
                    return None;
                };
                if chain.conditions.len() >= MAX_GUARD_CONDITIONS {
                    self.record(UnsupportedKind::FragmentLimit, element.text_range());
                    *state = ChainState::Unresolvable;
                    return None;
                }
                let Some(condition) = self.guard_condition(element, GuardBranch::Elif) else {
                    self.record_guard_condition(element, GuardBranch::Elif);
                    *state = ChainState::Unresolvable;
                    return None;
                };
                chain.conditions.push(condition);
                chain.selected = chain.conditions.len().saturating_sub(1);
                Some(chain.clone())
            }
            Some(GuardBranch::Else) => {
                // An `wx:else` has no expression and always closes the chain,
                // even when the preceding conditions were unresolvable.
                let selected = match state {
                    ChainState::Resolved(chain) => {
                        chain.selected = chain.conditions.len();
                        Some(chain.clone())
                    }
                    _ => None,
                };
                *state = ChainState::Empty;
                selected
            }
            None => {
                *state = ChainState::Empty;
                None
            }
        }
    }

    /// Records that a conditional chain could not be parsed.
    ///
    /// The element keeps its fragments, but they are analyzed without
    /// narrowing; the record downgrades the view to `Partial` so a missing
    /// guard is never reported as a clean check.
    fn record_guard_condition(&mut self, element: Element<'a>, branch: GuardBranch) {
        if let Some(attribute) = guard_attribute(element, branch) {
            self.record(UnsupportedKind::MalformedExpression, attribute.text_range());
        }
    }

    fn analyze_node(&mut self, node: AstNode<'a>) {
        match node {
            AstNode::Element(element) => self.analyze_element(element, None),
            AstNode::Mustache(mustache) => self.analyze_mustache(mustache),
            _ => {}
        }
    }

    fn analyze_element(&mut self, element: Element<'a>, guard: Option<GuardChain>) {
        let name = element.name().unwrap_or_default();
        if let Some(kind) = unsupported_element(name) {
            self.record(kind, element.text_range());
            return;
        }
        if self.guards.len() >= MAX_GUARD_CHAINS {
            self.record(UnsupportedKind::FragmentLimit, element.text_range());
            return;
        }
        let attributes: Vec<Attribute<'a>> = element.attributes().collect();
        let has_for = attributes
            .iter()
            .any(|attribute| attribute.name() == Some("wx:for"));

        if let Some(attribute) = attributes
            .iter()
            .copied()
            .find(|attribute| attribute.name() == Some("wx:for"))
        {
            // The loop expression belongs to the enclosing scope.
            self.analyze_attribute(attribute);
        }
        let bindings = if has_for {
            self.loop_bindings(&attributes)
        } else {
            Vec::new()
        };
        if self.bindings.len().saturating_add(bindings.len()) > MAX_BINDINGS {
            // Node rejects an oversized chain, which would fail the whole
            // document, so stop before emitting one.
            self.record(UnsupportedKind::FragmentLimit, element.text_range());
            return;
        }
        let pushed_bindings = bindings.len();
        for binding in bindings {
            self.bindings.push(binding);
        }

        // Guard expressions are conditions of the branch, not fragments inside
        // it, so they are analyzed before the chain is pushed.
        for branch in [GuardBranch::If, GuardBranch::Elif] {
            if let Some(attribute) = guard_attribute(element, branch) {
                self.analyze_attribute(attribute);
            }
        }

        let pushed_guard = guard.is_some();
        if let Some(guard) = guard {
            self.guards.push(guard);
        }

        if !has_for {
            for binding_attribute in ["wx:for-item", "wx:for-index"] {
                if let Some(attribute) = attributes
                    .iter()
                    .copied()
                    .find(|attribute| attribute.name() == Some(binding_attribute))
                {
                    if attribute
                        .value()
                        .is_some_and(|value| value.mustaches().next().is_some())
                    {
                        self.record(UnsupportedKind::DynamicForBinding, attribute.text_range());
                    }
                }
            }
        }

        for attribute in &attributes {
            let Some(attribute_name) = attribute.name() else {
                continue;
            };
            if matches!(
                attribute_name,
                "wx:for"
                    | "wx:for-item"
                    | "wx:for-index"
                    | "wx:key"
                    | "wx:if"
                    | "wx:elif"
                    | "wx:else"
            ) {
                continue;
            }
            if let Some((binding, event)) = event_attribute(attribute_name) {
                self.analyze_event(*attribute, binding, event);
            } else if is_empty_event_attribute(attribute_name) {
                self.record(UnsupportedKind::EmptyEvent, attribute.text_range());
            } else {
                self.analyze_attribute(*attribute);
            }
        }

        if self.components.len() < MAX_COMPONENTS {
            self.collect_component(name, &attributes);
        } else if !self.components_truncated && self.component_tags.contains(name) {
            self.components_truncated = true;
            self.record(UnsupportedKind::FragmentLimit, element.text_range());
        }

        let children: Vec<AstNode<'a>> = element.children().collect();
        self.analyze_children(&children);

        if pushed_guard {
            self.guards.pop();
        }
        for _ in 0..pushed_bindings {
            self.bindings.pop();
        }
    }

    fn analyze_attribute(&mut self, attribute: Attribute<'a>) {
        if let Some(value) = attribute.value() {
            for mustache in value.mustaches() {
                self.analyze_mustache(mustache);
            }
        }
    }

    fn analyze_event(
        &mut self,
        attribute: Attribute<'a>,
        binding: EventBindingKind,
        event: &'a str,
    ) {
        let Some(attribute_name) = attribute.name() else {
            return;
        };
        let Some(value) = attribute.value() else {
            self.record(UnsupportedKind::EmptyEvent, attribute.text_range());
            return;
        };
        if value.mustaches().next().is_some() {
            self.record(UnsupportedKind::DynamicEvent, value.text_range());
            return;
        }
        let Some(handler) = value.literal_text().map(str::trim) else {
            self.record(UnsupportedKind::DynamicEvent, value.text_range());
            return;
        };
        if handler.is_empty() {
            self.record(UnsupportedKind::EmptyEvent, attribute.text_range());
            return;
        }
        if !is_static_handler_name(handler) {
            self.record(UnsupportedKind::InvalidEvent, value.text_range());
            return;
        }
        let Some(range) = self.source_range(attribute.text_range()) else {
            return;
        };
        let handler_range = self.handler_range(value, handler).unwrap_or(range);
        self.events.push(EventHandler {
            attribute: attribute_name.to_owned(),
            binding,
            event: event.to_owned(),
            handler: handler.to_owned(),
            range,
            attribute_range: range,
            handler_range,
        });
    }

    /// Returns the byte range of the trimmed handler token inside `value`.
    ///
    /// Returns `None` when the computed range is not a valid slice of the
    /// source or does not equal `handler`; callers fall back to the whole
    /// attribute range.
    fn handler_range(&self, value: AttributeValue<'a>, handler: &str) -> Option<SourceRange> {
        let range = value.text_range();
        let literal = value.literal_text()?;
        let quote = usize::from(value.is_quoted());
        let leading_space = literal.len().checked_sub(literal.trim_start().len())?;
        let offset = u32::try_from(quote.checked_add(leading_space)?).ok()?;
        let length = u32::try_from(handler.len()).ok()?;
        let start = range.start().checked_add(offset)?;
        let end = start.checked_add(length)?;
        if end > range.end() {
            return None;
        }
        let slice_start = usize::try_from(start).ok()?;
        let slice_end = usize::try_from(end).ok()?;
        if self.tree.text().get(slice_start..slice_end)? != handler {
            return None;
        }
        SourceRange::from_bounds(start, end).ok()
    }

    fn analyze_mustache(&mut self, mustache: Mustache<'a>) {
        if self.fragments.len() >= MAX_FRAGMENTS {
            if !self.truncated {
                self.truncated = true;
                self.record(UnsupportedKind::FragmentLimit, mustache.text_range());
            }
            return;
        }
        if !mustache.is_closed() {
            self.record(UnsupportedKind::MalformedExpression, mustache.text_range());
            return;
        }
        let Some(inner) = mustache.inner_range() else {
            self.record(UnsupportedKind::MalformedExpression, mustache.text_range());
            return;
        };
        let Some(expr_text) = self.slice(inner) else {
            self.record(UnsupportedKind::InvalidRange, mustache.text_range());
            return;
        };
        if expr_text.trim().is_empty() {
            self.record(UnsupportedKind::EmptyExpression, inner);
            return;
        }
        if !expression_text_is_complete(expr_text) {
            self.record(UnsupportedKind::MalformedExpression, inner);
            return;
        }
        let Some(expr) = mustache.expr() else {
            self.record(UnsupportedKind::MalformedExpression, inner);
            return;
        };
        if expr.walk().any(Expr::is_missing) || expr.range() != inner {
            self.record(UnsupportedKind::MalformedExpression, inner);
            return;
        }
        let Some(wxml_range) = self.source_range(inner) else {
            self.record(UnsupportedKind::InvalidRange, mustache.text_range());
            return;
        };
        let target = target_of(expr, self.tree.text());
        let bindings = self.bindings_snapshot();
        let guards = self.guards.clone();
        self.fragments.push(CheckFragment {
            fragment_id: String::new(),
            expr_text: expr_text.to_owned(),
            target,
            bindings,
            guards,
            wxml_range,
            fragment_revision: self.revision,
            root: self.root,
        });
    }

    /// Builds the item/index bindings for one `wx:for` element.
    ///
    /// Returns an empty list and records [`UnsupportedKind::DynamicForBinding`]
    /// when the loop value is not exactly one parseable mustache, so no type is
    /// guessed.
    fn loop_bindings(&mut self, attributes: &[Attribute<'a>]) -> Vec<LocalBinding> {
        let Some(attribute) = attributes
            .iter()
            .copied()
            .find(|attribute| attribute.name() == Some("wx:for"))
        else {
            return Vec::new();
        };
        let Some(value) = attribute.value() else {
            self.record(UnsupportedKind::DynamicForBinding, attribute.text_range());
            return Vec::new();
        };
        let Some((expr_text, inner)) = single_mustache_source(value) else {
            self.record(UnsupportedKind::DynamicForBinding, attribute.text_range());
            return Vec::new();
        };
        let Some(source_range) = self.source_range(inner) else {
            self.record(UnsupportedKind::DynamicForBinding, attribute.text_range());
            return Vec::new();
        };
        let mut item = self.for_binding_name(attributes, "wx:for-item", "item");
        let mut index = self.for_binding_name(attributes, "wx:for-index", "index");
        if item == index {
            // Node's level validator rejects a duplicate item/index pair, so
            // make the names distinct instead of losing the whole document.
            let range = attributes
                .iter()
                .copied()
                .find(|candidate| candidate.name() == Some("wx:for-index"))
                .map_or_else(|| attribute.text_range(), Attribute::text_range);
            self.record(UnsupportedKind::DynamicForBinding, range);
            if item == "index" {
                "item".clone_into(&mut item);
            } else {
                "index".clone_into(&mut index);
            }
        }
        vec![
            LocalBinding {
                name: item,
                role: LocalRole::Item,
                source_expr: expr_text.to_owned(),
                source_range,
            },
            LocalBinding {
                name: index,
                role: LocalRole::Index,
                source_expr: expr_text.to_owned(),
                source_range,
            },
        ]
    }

    fn for_binding_name(
        &mut self,
        attributes: &[Attribute<'a>],
        attribute_name: &str,
        default: &'static str,
    ) -> String {
        let Some(attribute) = attributes
            .iter()
            .copied()
            .find(|attribute| attribute.name() == Some(attribute_name))
        else {
            return default.to_owned();
        };
        let static_name = attribute
            .value()
            .and_then(AttributeValue::literal_text)
            .map(str::trim)
            .filter(|name| valid_binding_name(name));
        if let Some(static_name) = static_name {
            return static_name.to_owned();
        }
        self.record(UnsupportedKind::DynamicForBinding, attribute.text_range());
        default.to_owned()
    }

    /// Returns the full binding stack for one fragment.
    ///
    /// The stack is ordered outermost to innermost, with each `wx:for` level
    /// contributing its item followed by its index. Duplicate names are kept:
    /// the backend resolves the chain level by level, with inner declarations
    /// shadowing outer ones, because an inner `source_expr` may depend on the
    /// outer binding it shadows. Effective, deduplicated names are derived
    /// separately when serializing the legacy `locals` field.
    fn bindings_snapshot(&self) -> Vec<LocalBinding> {
        self.bindings.clone()
    }

    /// Returns one guard condition from the element's `wx:if`/`wx:elif` value.
    ///
    /// Returns `None` when the value is not exactly one closed, complete and
    /// parseable mustache; the whole chain is then treated as unresolvable.
    fn guard_condition(&self, element: Element<'a>, branch: GuardBranch) -> Option<GuardCondition> {
        let attribute = guard_attribute(element, branch)?;
        let value = attribute.value()?;
        let (expr_text, inner) = single_mustache_source(value)?;
        let range = self.source_range(inner)?;
        Some(GuardCondition {
            branch,
            expr_text: expr_text.to_owned(),
            range,
        })
    }

    /// Collects one custom-component usage when `tag` is configured.
    fn collect_component(&mut self, tag: &str, attributes: &[Attribute<'a>]) {
        if !self.component_tags.contains(tag) {
            return;
        }
        let mut collected = Vec::new();
        for attribute in attributes {
            let Some(name) = attribute.name() else {
                continue;
            };
            if is_component_ignored_attribute(name) {
                continue;
            }
            if let Some(attribute) = self.component_attribute(*attribute) {
                collected.push(attribute);
            }
        }
        self.components.push(ComponentUsage {
            tag: tag.to_owned(),
            component_file: None,
            attributes: collected,
        });
    }

    /// Classifies one attribute of a custom component.
    fn component_attribute(&self, attribute: Attribute<'a>) -> Option<ComponentAttribute> {
        let name = attribute.name()?.to_owned();
        let attribute_range = self.source_range(attribute.text_range())?;
        let Some(value) = attribute.value() else {
            return Some(ComponentAttribute {
                name,
                value_kind: AttributeValueKind::Boolean,
                expr_text: None,
                expr_range: None,
                attribute_range,
            });
        };
        let (value_kind, expr_text, expr_range) = if value.literal_text().is_some() {
            (AttributeValueKind::Literal, None, None)
        } else {
            match single_mustache(value) {
                Some(mustache) if mustache_covers_value(mustache, value) => (
                    AttributeValueKind::Mustache,
                    Some(mustache.expr_source().to_owned()),
                    mustache
                        .inner_range()
                        .and_then(|range| self.source_range(range)),
                ),
                _ => (AttributeValueKind::Mixed, None, None),
            }
        };
        Some(ComponentAttribute {
            name,
            value_kind,
            expr_text,
            expr_range,
            attribute_range,
        })
    }

    fn record(&mut self, kind: UnsupportedKind, range: TextRange) {
        if let Some(range) = self.source_range(range) {
            self.unsupported.push(UnsupportedFeature::new(kind, range));
        }
    }

    fn source_range(&self, range: TextRange) -> Option<SourceRange> {
        self.slice(range)?;
        Some(SourceRange::new(range))
    }

    fn slice(&self, range: TextRange) -> Option<&'a str> {
        let start = usize::try_from(range.start()).ok()?;
        let end = usize::try_from(range.end()).ok()?;
        self.tree.text().get(start..end)
    }
}

/// Returns the unsupported kind for template forms this phase does not model.
fn unsupported_element(name: &str) -> Option<UnsupportedKind> {
    match name {
        "wxs" => Some(UnsupportedKind::Wxs),
        "template" => Some(UnsupportedKind::Template),
        "import" => Some(UnsupportedKind::Import),
        "include" => Some(UnsupportedKind::Include),
        _ => None,
    }
}

/// Returns the guard branch an element starts, when it has one.
fn guard_branch(element: Element<'_>) -> Option<GuardBranch> {
    if element.attribute("wx:if").is_some() {
        Some(GuardBranch::If)
    } else if element.attribute("wx:elif").is_some() {
        Some(GuardBranch::Elif)
    } else if element.attribute("wx:else").is_some() {
        Some(GuardBranch::Else)
    } else {
        None
    }
}

/// Returns the attribute carrying `branch`'s condition expression.
fn guard_attribute(element: Element<'_>, branch: GuardBranch) -> Option<Attribute<'_>> {
    let name = match branch {
        GuardBranch::If => "wx:if",
        GuardBranch::Elif => "wx:elif",
        GuardBranch::Else => return None,
    };
    element.attribute(name)
}

/// Returns the value's only child when it is a mustache.
fn single_mustache(value: AttributeValue<'_>) -> Option<Mustache<'_>> {
    let mut children = value.children();
    let AstNode::Mustache(mustache) = children.next()? else {
        return None;
    };
    children.next().is_none().then_some(mustache)
}

/// Returns the expression text and inner range of a value that is exactly one
/// closed, complete and parseable mustache.
fn single_mustache_source(value: AttributeValue<'_>) -> Option<(&str, TextRange)> {
    let mustache = single_mustache(value)?;
    if !mustache.is_closed() {
        return None;
    }
    let inner = mustache.inner_range()?;
    let expr_text = mustache.expr_source();
    if expr_text.trim().is_empty() || !expression_text_is_complete(expr_text) {
        return None;
    }
    let expr = mustache.expr()?;
    if expr.walk().any(Expr::is_missing) || expr.range() != inner {
        return None;
    }
    Some((expr_text, inner))
}

/// Returns `true` when the mustache covers the value between its quotes.
fn mustache_covers_value(mustache: Mustache<'_>, value: AttributeValue<'_>) -> bool {
    let range = value.text_range();
    let quote = u32::from(value.is_quoted());
    let Some(start) = range.start().checked_add(quote) else {
        return false;
    };
    let Some(end) = range.end().checked_sub(quote) else {
        return false;
    };
    let mustache = mustache.text_range();
    mustache.start() == start && mustache.end() == end
}

/// Returns `true` for attributes that a custom component never sees as
/// properties: `wx:*` directives, event bindings and `slot`.
fn is_component_ignored_attribute(name: &str) -> bool {
    name.starts_with("wx:")
        || name == "slot"
        || event_attribute(name).is_some()
        || is_empty_event_attribute(name)
        || matches!(
            name,
            "bind" | "catch" | "capture-bind" | "capture-catch" | "mut-bind"
        )
}

const COLON_EVENT_PREFIXES: [(&str, EventBindingKind); 5] = [
    ("capture-bind:", EventBindingKind::CaptureBind),
    ("capture-catch:", EventBindingKind::CaptureCatch),
    ("mut-bind:", EventBindingKind::MutBind),
    ("bind:", EventBindingKind::Bind),
    ("catch:", EventBindingKind::Catch),
];

const COMPACT_EVENT_PREFIXES: [(&str, EventBindingKind); 5] = [
    ("capture-bind", EventBindingKind::CaptureBind),
    ("capture-catch", EventBindingKind::CaptureCatch),
    ("mut-bind", EventBindingKind::MutBind),
    ("bind", EventBindingKind::Bind),
    ("catch", EventBindingKind::Catch),
];

/// Splits an event attribute name into its binding family and event name.
///
/// Compact prefixes are matched literally: `binder` is interpreted as `bind` +
/// event `er`, and `catchup` as `catch` + event `up`. This mirrors WXML's
/// prefix-based event attributes; use the colon form when that ambiguity
/// matters.
fn event_attribute(name: &str) -> Option<(EventBindingKind, &str)> {
    for (prefix, binding) in COLON_EVENT_PREFIXES {
        if let Some(event) = name.strip_prefix(prefix) {
            return (!event.is_empty()).then_some((binding, event));
        }
    }
    for (prefix, binding) in COMPACT_EVENT_PREFIXES {
        if let Some(event) = name.strip_prefix(prefix) {
            if !event.is_empty() && !event.starts_with(':') {
                return Some((binding, event));
            }
        }
    }
    None
}

/// Returns `true` for a binding prefix with an empty event name, such as
/// `bind:` or `capture-catch:`.
fn is_empty_event_attribute(name: &str) -> bool {
    matches!(
        name,
        "bind:" | "catch:" | "capture-bind:" | "capture-catch:" | "mut-bind:"
    )
}

/// Maximum byte length of a static event handler name.
///
/// The name becomes a virtual method lookup in the script, so the request must
/// stay bounded even for hostile templates.
const MAX_HANDLER_NAME_BYTES: usize = 256;

/// Returns `true` when `handler` is a plain static method name.
///
/// Event handlers are resolved as property names on the page/component
/// object, so only a single identifier is meaningful: `item.onTap`, `onTap()`
/// and `a b` are member expressions or code, not method names, and are
/// reported as [`UnsupportedKind::InvalidEvent`] instead of a false "unknown
/// handler" diagnostic. Unicode identifiers, `_` and `$` are accepted, matching
/// `isIdentifierText` in `backends/typescript/src/events.ts`.
fn is_static_handler_name(handler: &str) -> bool {
    if handler.is_empty() || handler.len() > MAX_HANDLER_NAME_BYTES {
        return false;
    }
    let mut characters = handler.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    characters.all(|character| character.is_alphanumeric() || character == '_' || character == '$')
}

/// Names that Node refuses as template locals.
///
/// Mirrors `RESERVED_WORDS` in `backends/typescript/src/expression.ts`
/// (ECMAScript keywords, strict reserved words, `eval`/`arguments`, and the
/// global value names `undefined`/`NaN`/`Infinity`). Builtin globals such as
/// `Math` are intentionally absent: a template local may shadow them.
/// Rejecting a name here degrades to the default loop names instead of sending
/// a request that Node rejects as a whole.
const RESERVED_BINDING_NAMES: [&str; 51] = [
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "await",
    "eval",
    "arguments",
    "undefined",
    "NaN",
    "Infinity",
];

/// Returns `true` when a `wx:for` binding name is a valid, non-empty identifier
/// of at most 64 bytes that Node can use as a template local.
///
/// Rejects keywords, strict reserved words, global values and the
/// `__wxml_`/`__WxmlProps_` bridge prefixes, matching Node's
/// `isBindingIdentifier`.
fn valid_binding_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !RESERVED_BINDING_NAMES.contains(&name)
        && !name.starts_with("__wxml_")
        && !name.starts_with("__WxmlProps_")
}

/// Returns the preferred target token for an expression.
fn target_of(expr: &Expr, source: &str) -> Option<String> {
    match expr.kind() {
        ExprKind::Identifier { name } => identifier_text(source, *name),
        ExprKind::Member { property, .. } => rightmost_identifier(property, source),
        ExprKind::Index { object, .. } => rightmost_identifier(object, source),
        ExprKind::Call { callee, .. } => rightmost_identifier(callee, source),
        _ => None,
    }
}

/// Returns the rightmost identifier of a member/index/call/parenthesized
/// expression.
fn rightmost_identifier(expr: &Expr, source: &str) -> Option<String> {
    match expr.kind() {
        ExprKind::Identifier { name } => identifier_text(source, *name),
        ExprKind::Member { property, .. } => rightmost_identifier(property, source),
        ExprKind::Index { object, .. } => rightmost_identifier(object, source),
        ExprKind::Call { callee, .. } => rightmost_identifier(callee, source),
        ExprKind::Parenthesized { expr } => rightmost_identifier(expr, source),
        _ => None,
    }
}

/// Checks that all brackets and strings in an expression are terminated.
///
/// `wxml-syntax` does not expose its expression diagnostics through the tree,
/// so this small scanner catches the common incomplete forms (`(a`, `[1,`,
/// `a(`, unterminated strings) that otherwise leave a syntactically complete
/// root node.
fn expression_text_is_complete(text: &str) -> bool {
    let mut expected: Vec<char> = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if matches!(c, '\'' | '"' | '`') {
            if !skip_string(&mut chars, c) {
                return false;
            }
            continue;
        }
        if let Some(closer) = opening_delimiter(c) {
            expected.push(closer);
            continue;
        }
        if matches!(c, ')' | ']' | '}') && expected.pop() != Some(c) {
            return false;
        }
    }
    expected.is_empty()
}

const fn opening_delimiter(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

fn skip_string(chars: &mut std::str::Chars<'_>, quote: char) -> bool {
    while let Some(c) = chars.next() {
        if c == '\\' {
            let _ = chars.next();
        } else if c == quote {
            return true;
        }
    }
    false
}

fn identifier_text(source: &str, range: TextRange) -> Option<String> {
    let start = usize::try_from(range.start()).ok()?;
    let end = usize::try_from(range.end()).ok()?;
    source.get(start..end).map(str::to_owned)
}

#[cfg(test)]
#[allow(clippy::assert_is_empty)]
mod tests {
    use super::{
        TemplateAnalysis, TemplateAnalyzer, event_attribute, expression_text_is_complete,
        is_empty_event_attribute, target_of, unsupported_element, valid_binding_name,
    };
    use crate::check::{
        AttributeValueKind, EventBindingKind, GuardBranch, LocalRole, RootKind, UnsupportedKind,
    };
    use crate::contract::SourceRevision;
    use wxml_syntax::expr::parse_expression;

    const REVISION: u64 = 1;

    fn analyze(source: &str) -> (TemplateAnalysis, wxml_syntax::SyntaxTree) {
        let parsed = wxml_syntax::parse(source);
        let analysis = TemplateAnalyzer::new(RootKind::Page, SourceRevision::new(REVISION))
            .analyze(&parsed.tree);
        (analysis, parsed.tree)
    }

    fn analyze_with_tags(
        source: &str,
        tags: &[&str],
    ) -> (TemplateAnalysis, wxml_syntax::SyntaxTree) {
        let parsed = wxml_syntax::parse(source);
        let analysis = TemplateAnalyzer::new(RootKind::Page, SourceRevision::new(REVISION))
            .with_component_tags(tags.iter().map(|tag| (*tag).to_owned()))
            .analyze(&parsed.tree);
        (analysis, parsed.tree)
    }

    fn text(tree: &wxml_syntax::SyntaxTree, range: wxml_text::SourceRange) -> &str {
        let start = usize::try_from(range.start()).expect("range start");
        let end = usize::try_from(range.end()).expect("range end");
        &tree.text()[start..end]
    }

    #[test]
    fn event_attributes_cover_compact_and_colon_forms() {
        for (name, binding, event) in [
            ("bindtap", EventBindingKind::Bind, "tap"),
            ("bind:tap", EventBindingKind::Bind, "tap"),
            ("catchtap", EventBindingKind::Catch, "tap"),
            ("catch:tap", EventBindingKind::Catch, "tap"),
            ("capture-bind:tap", EventBindingKind::CaptureBind, "tap"),
            ("capture-bindtap", EventBindingKind::CaptureBind, "tap"),
            (
                "capture-catch:touchmove",
                EventBindingKind::CaptureCatch,
                "touchmove",
            ),
            ("mut-bind:tap", EventBindingKind::MutBind, "tap"),
        ] {
            assert_eq!(
                event_attribute(name),
                Some((binding, event)),
                "attribute {name:?}"
            );
        }
        for name in ["bind", "catch:", "bind:", "class", "data-x"] {
            assert_eq!(event_attribute(name), None, "attribute {name:?}");
        }
    }

    #[test]
    fn binding_names_follow_identifier_rules() {
        for name in ["item", "row1", "_x", "$x", "名字"] {
            assert!(valid_binding_name(name), "name {name:?}");
        }
        let long = "a".repeat(65);
        for name in ["", "1item", "a b", "item.name", "a-b", long.as_str()] {
            assert!(!valid_binding_name(name), "name {name:?}");
        }
        for name in [
            "class",
            "let",
            "undefined",
            "NaN",
            "Infinity",
            "eval",
            "arguments",
            "__wxml_root",
            "__WxmlProps_card",
        ] {
            assert!(!valid_binding_name(name), "reserved name {name:?}");
        }
        // Globals are legal loop names: the rewriter lets a local shadow them.
        for name in ["Math", "console", "parseInt", "Number"] {
            assert!(valid_binding_name(name), "global name {name:?}");
        }
    }

    #[test]
    fn empty_event_prefixes_are_recognized() {
        for name in [
            "bind:",
            "catch:",
            "capture-bind:",
            "capture-catch:",
            "mut-bind:",
        ] {
            assert!(is_empty_event_attribute(name), "name {name:?}");
        }
        for name in ["bind", "bindtap", "catchtap", "binder", "class"] {
            assert!(!is_empty_event_attribute(name), "name {name:?}");
        }
    }

    #[test]
    fn unsupported_element_names_are_exact_and_case_sensitive() {
        assert_eq!(unsupported_element("wxs"), Some(UnsupportedKind::Wxs));
        assert_eq!(
            unsupported_element("template"),
            Some(UnsupportedKind::Template)
        );
        assert_eq!(unsupported_element("import"), Some(UnsupportedKind::Import));
        assert_eq!(
            unsupported_element("include"),
            Some(UnsupportedKind::Include)
        );
        assert_eq!(unsupported_element("WXS"), None);
        assert_eq!(unsupported_element("view"), None);
    }

    #[test]
    fn completeness_check_rejects_unbalanced_expressions() {
        for text in ["a.b", "(a + b)", "[1, 2]", "{a: 1}", "'a)b'", "a ? b : c"] {
            assert!(expression_text_is_complete(text), "text {text:?}");
        }
        for text in ["(a", "[1,", "a)", "'abc", "{a: 1", "([)]"] {
            assert!(!expression_text_is_complete(text), "text {text:?}");
        }
    }

    #[test]
    fn targets_follow_the_c1_rules() {
        let cases = [
            ("user", Some("user")),
            ("user.name", Some("name")),
            ("user.list[0]", Some("list")),
            ("format(user)", Some("format")),
            ("user.format()", Some("format")),
            ("a.b[c]", Some("b")),
            ("(fn)()", Some("fn")),
            ("1 + 2", None),
            ("'text'", None),
        ];
        for (source, expected) in cases {
            let result = parse_expression(source, 0);
            assert!(
                result.errors.is_empty(),
                "unexpected errors for {source:?}: {:?}",
                result.errors
            );
            assert_eq!(
                target_of(&result.expr, source).as_deref(),
                expected,
                "expression {source:?}"
            );
        }
    }

    #[test]
    fn loop_bindings_record_roles_and_the_source_expression() {
        let source = r#"<view wx:for="{{ groups }}">{{ item.name }} {{ index }}</view>"#;
        let (analysis, tree) = analyze(source);
        let fragment = analysis
            .fragments
            .iter()
            .find(|fragment| fragment.expr_text == "item.name")
            .expect("item.name fragment");
        assert_eq!(fragment.bindings.len(), 2);
        let item = &fragment.bindings[0];
        assert_eq!(item.name, "item");
        assert_eq!(item.role, LocalRole::Item);
        assert_eq!(item.source_expr, "groups");
        assert_eq!(text(&tree, item.source_range), "groups");
        let index = &fragment.bindings[1];
        assert_eq!(index.name, "index");
        assert_eq!(index.role, LocalRole::Index);
        assert_eq!(index.source_expr, "groups");
        assert_eq!(index.source_range, item.source_range);
    }

    fn loop_binding_names<'a>(
        analysis: &'a TemplateAnalysis,
        expr: &str,
    ) -> Vec<(&'a str, LocalRole)> {
        let fragment = analysis
            .fragments
            .iter()
            .find(|fragment| fragment.expr_text == expr)
            .unwrap_or_else(|| panic!("missing fragment {expr}"));
        fragment
            .bindings
            .iter()
            .map(|binding| (binding.name.as_str(), binding.role))
            .collect()
    }

    #[test]
    fn reserved_loop_item_names_fall_back_to_item() {
        let source = r#"<view wx:for="{{ items }}" wx:for-item="class">{{ item }}</view>"#;
        let (analysis, _) = analyze(source);
        assert_eq!(
            loop_binding_names(&analysis, "item"),
            vec![("item", LocalRole::Item), ("index", LocalRole::Index)]
        );
        assert!(
            analysis
                .unsupported
                .iter()
                .any(|feature| feature.kind() == UnsupportedKind::DynamicForBinding)
        );
    }

    #[test]
    fn bridge_reserved_loop_names_fall_back_to_defaults() {
        let source = r#"<view wx:for="{{ items }}" wx:for-item="__wxml_root">{{ item }}</view>"#;
        let (analysis, _) = analyze(source);
        assert_eq!(
            loop_binding_names(&analysis, "item"),
            vec![("item", LocalRole::Item), ("index", LocalRole::Index)]
        );
        assert!(
            analysis
                .unsupported
                .iter()
                .any(|feature| feature.kind() == UnsupportedKind::DynamicForBinding)
        );
    }

    #[test]
    fn colliding_loop_binding_names_are_made_distinct() {
        let source =
            r#"<view wx:for="{{ items }}" wx:for-item="x" wx:for-index="x">{{ x }}</view>"#;
        let (analysis, _) = analyze(source);
        assert_eq!(
            loop_binding_names(&analysis, "x"),
            vec![("x", LocalRole::Item), ("index", LocalRole::Index)]
        );
        let collisions = analysis
            .unsupported
            .iter()
            .filter(|feature| feature.kind() == UnsupportedKind::DynamicForBinding)
            .count();
        assert_eq!(collisions, 1);
    }

    #[test]
    fn custom_loop_names_replace_the_defaults() {
        let (analysis, _) = analyze(
            r#"<view wx:for="{{ rows }}" wx:for-item="row" wx:for-index="i">{{ row.name }}</view>"#,
        );
        let fragment = analysis
            .fragments
            .iter()
            .find(|fragment| fragment.expr_text == "row.name")
            .expect("row.name fragment");
        let names: Vec<(&str, LocalRole)> = fragment
            .bindings
            .iter()
            .map(|binding| (binding.name.as_str(), binding.role))
            .collect();
        assert_eq!(
            names,
            vec![("row", LocalRole::Item), ("i", LocalRole::Index)]
        );
    }

    #[test]
    fn nested_loop_bindings_shadow_and_pop() {
        let source = r#"<view wx:for="{{ outer }}">{{ item.name }}<text wx:for="{{ item.children }}">{{ item.name }}</text></view>{{ item.name }}"#;
        let (analysis, _) = analyze(source);
        let occurrences: Vec<&crate::check::CheckFragment> = analysis
            .fragments
            .iter()
            .filter(|fragment| fragment.expr_text == "item.name")
            .collect();
        assert_eq!(occurrences.len(), 3);

        let outer = occurrences[0];
        assert_eq!(outer.bindings.len(), 2);
        assert_eq!(outer.bindings[0].source_expr, "outer");

        let inner = occurrences[1];
        assert_eq!(
            inner.bindings.len(),
            4,
            "full chain keeps the shadowed outer item"
        );
        let chain: Vec<(&str, LocalRole, &str)> = inner
            .bindings
            .iter()
            .map(|binding| {
                (
                    binding.name.as_str(),
                    binding.role,
                    binding.source_expr.as_str(),
                )
            })
            .collect();
        assert_eq!(
            chain,
            vec![
                ("item", LocalRole::Item, "outer"),
                ("index", LocalRole::Index, "outer"),
                ("item", LocalRole::Item, "item.children"),
                ("index", LocalRole::Index, "item.children"),
            ]
        );

        assert!(
            occurrences[2].bindings.is_empty(),
            "bindings pop after loop"
        );
    }

    #[test]
    fn guard_chains_select_if_elif_and_else() {
        let source = r#"<view wx:if="{{ state.loading }}">A{{ a }}</view><view wx:elif="{{ state.ready }}">B{{ b }}</view><view wx:else>C{{ c }}</view>"#;
        let (analysis, _) = analyze(source);
        let by_expr = |expr: &str| {
            analysis
                .fragments
                .iter()
                .find(|fragment| fragment.expr_text == expr)
                .unwrap_or_else(|| panic!("missing {expr}"))
        };

        let if_fragment = by_expr("a");
        assert_eq!(if_fragment.guards.len(), 1);
        assert_eq!(if_fragment.guards[0].conditions.len(), 1);
        assert_eq!(if_fragment.guards[0].selected, 0);
        assert_eq!(if_fragment.guards[0].conditions[0].branch, GuardBranch::If);
        assert_eq!(
            if_fragment.guards[0].conditions[0].expr_text,
            "state.loading"
        );

        let elif_fragment = by_expr("b");
        assert_eq!(elif_fragment.guards[0].conditions.len(), 2);
        assert_eq!(elif_fragment.guards[0].selected, 1);
        assert_eq!(
            elif_fragment.guards[0].conditions[1].branch,
            GuardBranch::Elif
        );

        let else_fragment = by_expr("c");
        assert_eq!(else_fragment.guards[0].conditions.len(), 2);
        assert_eq!(
            else_fragment.guards[0].selected,
            else_fragment.guards[0].conditions.len()
        );
    }

    #[test]
    fn nested_guard_chains_stack_outermost_first() {
        let source =
            r#"<view wx:if="{{ outer }}"><text wx:if="{{ inner }}">{{ value }}</text></view>"#;
        let (analysis, _) = analyze(source);
        let fragment = analysis
            .fragments
            .iter()
            .find(|fragment| fragment.expr_text == "value")
            .expect("value fragment");
        assert_eq!(fragment.guards.len(), 2);
        assert_eq!(fragment.guards[0].conditions[0].expr_text, "outer");
        assert_eq!(fragment.guards[1].conditions[0].expr_text, "inner");
    }

    #[test]
    fn unresolvable_nested_loops_do_not_pop_outer_bindings() {
        let source = r#"<view wx:for="{{ outer }}"><text wx:for="{{ a }}{{ b }}">{{ item.name }}</text></view>"#;
        let (analysis, _) = analyze(source);
        let fragment = analysis
            .fragments
            .iter()
            .find(|fragment| fragment.expr_text == "item.name")
            .expect("item.name fragment");
        assert_eq!(fragment.bindings.len(), 2);
        assert_eq!(fragment.bindings[0].name, "item");
        assert_eq!(fragment.bindings[0].source_expr, "outer");
        assert!(
            analysis
                .unsupported
                .iter()
                .any(|feature| feature.kind() == UnsupportedKind::DynamicForBinding)
        );
    }

    #[test]
    fn unresolvable_guards_attach_no_chain() {
        let source =
            r#"<view wx:if="{{ a + }}">{{ x }}</view><view wx:elif="{{ b }}">{{ y }}</view>"#;
        let (analysis, _) = analyze(source);
        assert!(!analysis.fragments.is_empty());
        for fragment in &analysis.fragments {
            assert!(
                fragment.guards.is_empty(),
                "unexpected guard on {:?}",
                fragment.expr_text
            );
        }
    }

    #[test]
    fn component_attributes_are_classified_by_value_shape() {
        let source = r#"<card title="hello" count="{{ total }}" mixed="a{{ b }}" disabled wx:if="{{ ok }}" bindtap="onTap" catch:tap="onCatch" slot="x" />"#;
        let (analysis, tree) = analyze_with_tags(source, &["card"]);
        assert_eq!(analysis.components.len(), 1);
        let usage = &analysis.components[0];
        assert_eq!(usage.tag, "card");
        assert_eq!(usage.component_file, None);
        let names: Vec<&str> = usage
            .attributes
            .iter()
            .map(|attribute| attribute.name.as_str())
            .collect();
        assert_eq!(names, vec!["title", "count", "mixed", "disabled"]);

        let title = &usage.attributes[0];
        assert_eq!(title.value_kind, AttributeValueKind::Literal);
        assert_eq!(title.expr_text, None);
        assert_eq!(title.expr_range, None);
        assert_eq!(text(&tree, title.attribute_range), r#"title="hello""#);

        let count = &usage.attributes[1];
        assert_eq!(count.value_kind, AttributeValueKind::Mustache);
        assert_eq!(count.expr_text.as_deref(), Some("total"));
        assert_eq!(
            count.expr_range.map(|range| text(&tree, range)),
            Some("total")
        );

        let mixed = &usage.attributes[2];
        assert_eq!(mixed.value_kind, AttributeValueKind::Mixed);
        assert_eq!(mixed.expr_text, None);
        assert_eq!(mixed.expr_range, None);

        let disabled = &usage.attributes[3];
        assert_eq!(disabled.value_kind, AttributeValueKind::Boolean);
        assert_eq!(disabled.expr_range, None);
        assert_eq!(text(&tree, disabled.attribute_range), "disabled");

        // Component attribute mustaches are still fragments.
        assert!(
            analysis
                .fragments
                .iter()
                .any(|fragment| fragment.expr_text == "total")
        );
    }

    #[test]
    fn unknown_tags_do_not_produce_component_usages() {
        let (analysis, _) = analyze_with_tags(r#"<view title="x" />"#, &["card"]);
        assert!(analysis.components.is_empty());
    }

    #[test]
    fn event_bindings_record_prefix_event_and_exact_ranges() {
        for (attribute, binding, event) in [
            ("bindtap", EventBindingKind::Bind, "tap"),
            ("catchtap", EventBindingKind::Catch, "tap"),
            ("bind:tap", EventBindingKind::Bind, "tap"),
            ("catch:tap", EventBindingKind::Catch, "tap"),
            ("capture-bind:tap", EventBindingKind::CaptureBind, "tap"),
            ("capture-catch:tap", EventBindingKind::CaptureCatch, "tap"),
            ("mut-bind:tap", EventBindingKind::MutBind, "tap"),
        ] {
            let source = format!(r#"<view {attribute}="onTap" />"#);
            let (analysis, tree) = analyze(&source);
            assert!(
                analysis.unsupported.is_empty(),
                "{attribute}: {:?}",
                analysis.unsupported
            );
            let handler = analysis
                .events
                .first()
                .unwrap_or_else(|| panic!("no event for {attribute}"));
            assert_eq!(handler.attribute, attribute);
            assert_eq!(handler.binding, binding, "attribute {attribute}");
            assert_eq!(handler.event, event, "attribute {attribute}");
            assert_eq!(handler.handler, "onTap");

            let start = u32::try_from(source.find(attribute).expect("attribute offset"))
                .expect("u32 offset");
            let name_len = u32::try_from(attribute.len()).expect("name length");
            // `=` plus the two quotes plus the five handler bytes.
            let attribute_range = wxml_text::SourceRange::from_bounds(start, start + name_len + 8)
                .expect("attribute range");
            assert_eq!(handler.attribute_range, attribute_range);
            assert_eq!(handler.range, attribute_range);
            let handler_range =
                wxml_text::SourceRange::from_bounds(start + name_len + 2, start + name_len + 7)
                    .expect("handler range");
            assert_eq!(handler.handler_range, handler_range);
            assert_eq!(
                text(&tree, handler.attribute_range),
                format!(r#"{attribute}="onTap""#)
            );
            assert_eq!(text(&tree, handler.handler_range), "onTap");
        }
    }

    #[test]
    fn event_handler_range_trims_surrounding_whitespace() {
        let source = "<view bindtap=\"  onTap\t\" />";
        let (analysis, tree) = analyze(source);
        let handler = analysis.events.first().expect("event");
        assert_eq!(handler.handler, "onTap");
        assert_eq!(text(&tree, handler.handler_range), "onTap");
        // The whole attribute keeps the quotes and the surrounding spaces.
        assert_eq!(
            text(&tree, handler.attribute_range),
            "bindtap=\"  onTap\t\""
        );
    }

    #[test]
    fn dynamic_empty_and_invalid_event_handlers_are_unsupported() {
        for (value, kind) in [
            ("{{ handler }}", UnsupportedKind::DynamicEvent),
            ("on{{ prefix }}", UnsupportedKind::DynamicEvent),
            ("", UnsupportedKind::EmptyEvent),
            ("   ", UnsupportedKind::EmptyEvent),
            ("item.onTap", UnsupportedKind::InvalidEvent),
            ("onTap()", UnsupportedKind::InvalidEvent),
            ("on Tap", UnsupportedKind::InvalidEvent),
            ("1onTap", UnsupportedKind::InvalidEvent),
            ("on-Tap", UnsupportedKind::InvalidEvent),
            ("this.onTap", UnsupportedKind::InvalidEvent),
        ] {
            let source = format!(r#"<view bindtap="{value}" />"#);
            let (analysis, _) = analyze(&source);
            assert!(analysis.events.is_empty(), "value {value:?}");
            assert_eq!(
                analysis.unsupported.len(),
                1,
                "value {value:?}: {:?}",
                analysis.unsupported
            );
            assert_eq!(analysis.unsupported[0].kind(), kind, "value {value:?}");
        }
    }

    #[test]
    fn unicode_and_underscore_event_handlers_are_accepted() {
        for handler in ["_onTap", "$tap", "on_tap2", "处理点击"] {
            let source = format!(r#"<view bindtap="{handler}" />"#);
            let (analysis, tree) = analyze(&source);
            assert!(analysis.unsupported.is_empty(), "handler {handler:?}");
            let event = analysis
                .events
                .first()
                .unwrap_or_else(|| panic!("no event for {handler:?}"));
            assert_eq!(event.handler, handler);
            assert_eq!(text(&tree, event.handler_range), handler);
        }
    }
}
