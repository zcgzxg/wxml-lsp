//! Template scope analysis that turns mustaches into C1.1 check fragments.
//!
//! The analyzer walks the `wxml-syntax` AST in source order and produces:
//!
//! - [`CheckFragment`]s for every data mustache, with the current `wx:for`
//!   bindings in `locals`;
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
//! - `wx:if`/`wx:elif`/`wx:else` expressions are analyzed without changing the
//!   scope. Narrowing is left to the type backend.
//! - `wx:key` is not a data expression.
//! - `wxs`, `template`, `import` and `include` are reported and their content
//!   is not analyzed. These names are matched exactly; `<WXS>` is an ordinary
//!   element name, matching the syntax crate's raw-text rule.
//!
//! Fragment ids are assigned after traversal in source-range order (`f0`,
//! `f1`, ...), so they stay stable even though `wx:for` is analyzed before
//! other attributes. Identical expressions in different places remain separate
//! fragments.
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
    CheckFragment, EventBindingKind, EventHandler, RootKind, TemplateAnalysis, UnsupportedFeature,
    UnsupportedKind,
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

/// Reusable configuration for template analysis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemplateAnalyzer {
    root: RootKind,
    source_revision: SourceRevision,
}

impl TemplateAnalyzer {
    /// Creates an analyzer for one page/component root and source revision.
    #[must_use]
    pub const fn new(root: RootKind, source_revision: SourceRevision) -> Self {
        Self {
            root,
            source_revision,
        }
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
    /// same tree twice produces the same fragments, events and unsupported
    /// records. Fragment output is capped at [`MAX_FRAGMENTS`]; check
    /// [`TemplateAnalysis::truncated`] before treating the result as complete.
    #[must_use]
    pub fn analyze(&self, tree: &SyntaxTree) -> TemplateAnalysis {
        Engine {
            tree,
            root: self.root,
            revision: self.source_revision.get(),
            locals: Vec::new(),
            fragments: Vec::new(),
            events: Vec::new(),
            unsupported: Vec::new(),
            truncated: false,
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
    locals: Vec<String>,
    fragments: Vec<CheckFragment>,
    events: Vec<EventHandler>,
    unsupported: Vec<UnsupportedFeature>,
    truncated: bool,
}

impl<'a> Engine<'a> {
    fn run(mut self) -> TemplateAnalysis {
        let document = self.tree.document();
        for node in document.children() {
            self.analyze_node(node);
        }
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
            events: self.events,
            unsupported: self.unsupported,
            truncated: self.truncated,
        }
    }

    fn analyze_node(&mut self, node: AstNode<'a>) {
        match node {
            AstNode::Element(element) => self.analyze_element(element),
            AstNode::Mustache(mustache) => self.analyze_mustache(mustache),
            _ => {}
        }
    }

    fn analyze_element(&mut self, element: Element<'a>) {
        let name = element.name().unwrap_or_default();
        if let Some(kind) = unsupported_element(name) {
            self.record(kind, element.text_range());
            return;
        }
        let attributes: Vec<Attribute<'a>> = element.attributes().collect();
        let has_for = attributes
            .iter()
            .any(|attribute| attribute.name() == Some("wx:for"));

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

        if has_for {
            // The loop expression belongs to the enclosing scope.
            for attribute in &attributes {
                if attribute.name() == Some("wx:for") {
                    self.analyze_attribute(*attribute);
                }
            }
            let item = self.for_binding_name(&attributes, "wx:for-item", "item");
            let index = self.for_binding_name(&attributes, "wx:for-index", "index");
            self.locals.push(item);
            self.locals.push(index);
        }

        for attribute in &attributes {
            let Some(attribute_name) = attribute.name() else {
                continue;
            };
            if matches!(
                attribute_name,
                "wx:for" | "wx:for-item" | "wx:for-index" | "wx:key"
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

        for child in element.children() {
            self.analyze_node(child);
        }

        if has_for {
            self.locals.pop();
            self.locals.pop();
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
        let Some(range) = self.source_range(attribute.text_range()) else {
            return;
        };
        self.events.push(EventHandler {
            attribute: attribute_name.to_owned(),
            binding,
            event: event.to_owned(),
            handler: handler.to_owned(),
            range,
        });
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
        let locals = self.locals_snapshot();
        self.fragments.push(CheckFragment {
            fragment_id: String::new(),
            expr_text: expr_text.to_owned(),
            target,
            locals,
            wxml_range,
            fragment_revision: self.revision,
            root: self.root,
        });
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

    fn locals_snapshot(&self) -> Vec<String> {
        let mut seen = HashSet::with_capacity(self.locals.len());
        let mut unique = Vec::with_capacity(self.locals.len());
        for name in &self.locals {
            if seen.insert(name.as_str()) {
                unique.push(name.clone());
            }
        }
        unique
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

/// Returns `true` when a `wx:for` binding name is a valid, non-empty identifier
/// of at most 64 bytes.
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
mod tests {
    use super::{
        event_attribute, expression_text_is_complete, is_empty_event_attribute, target_of,
        unsupported_element, valid_binding_name,
    };
    use crate::check::{EventBindingKind, UnsupportedKind};
    use wxml_syntax::expr::parse_expression;

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
}
