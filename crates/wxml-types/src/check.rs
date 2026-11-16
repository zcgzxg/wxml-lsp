//! C1.1 template check DTOs.
//!
//! These types describe the template fragments that the engine sends to a type
//! backend. They deliberately contain only data: expression text, the preferred
//! target token, the template-scope bindings, the original WXML byte range and
//! identity/version fields. The backend decides how to prefix roots and what
//! types the expression has.
//!
//! Fragment ids are assigned by [`crate::template`] in source order (`f0`,
//! `f1`, ...). They do not depend on expression text, so the same expression
//! used in two places produces two distinct fragments.

use wxml_text::SourceRange;

use crate::contract::FileId;

/// Which script data root supplies top-level template identifiers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RootKind {
    /// A page's `Page({ data })` root.
    Page,
    /// A component's `Component({ data, properties })` root.
    Component,
}

impl RootKind {
    /// Returns the stable lower-case name used by the C1 protocol.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Component => "component",
        }
    }
}

/// The template role of one [`LocalBinding`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LocalRole {
    /// The current `wx:for` element.
    Item,
    /// The array index or object key of the current `wx:for`.
    Index,
}

impl LocalRole {
    /// Returns the stable lower-case name used by the C1 protocol.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Item => "item",
            Self::Index => "index",
        }
    }
}

/// One `wx:for` binding in scope at a fragment.
///
/// `source_expr` and `source_range` describe the `wx:for` expression that
/// created the binding, so a backend can resolve the element/key type of that
/// one loop instead of guessing from the name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalBinding {
    /// Binding name as written, for example `item` or `row`.
    pub name: String,
    /// Whether the binding is the element or the index/key.
    pub role: LocalRole,
    /// Exact `wx:for` expression text that produced the binding.
    pub source_expr: String,
    /// Byte range of the `wx:for` expression in the original template.
    pub source_range: SourceRange,
}

/// Which branch of a `wx:if`/`wx:elif`/`wx:else` chain a fragment sits in.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GuardBranch {
    /// The leading `wx:if`.
    If,
    /// A following `wx:elif`.
    Elif,
    /// The final `wx:else`.
    Else,
}

impl GuardBranch {
    /// Returns the stable lower-case name used by the C1 protocol.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::If => "if",
            Self::Elif => "elif",
            Self::Else => "else",
        }
    }
}

/// One condition in a `wx:if`/`wx:elif`/`wx:else` chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardCondition {
    /// Branch kind of this condition.
    pub branch: GuardBranch,
    /// Condition expression text as written, without `{{` and `}}`.
    pub expr_text: String,
    /// Byte range of the condition expression in the original template.
    pub range: SourceRange,
}

/// Every condition of one `wx:if`/`wx:elif`/`wx:else` chain, in source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardChain {
    /// Conditions on the chain, in source order.
    pub conditions: Vec<GuardCondition>,
    /// Index of the branch the fragment sits in; `conditions.len()` means
    /// `wx:else`.
    pub selected: usize,
}

/// How the value of a [`ComponentAttribute`] is written.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AttributeValueKind {
    /// A plain string such as `title="hello"`.
    Literal,
    /// A single mustache such as `title="{{ pageTitle }}"`.
    Mustache,
    /// Text mixed with mustaches such as `title="Hi {{ name }}"`.
    Mixed,
    /// A bare attribute such as `disabled`.
    Boolean,
}

impl AttributeValueKind {
    /// Returns the stable lower-case name used by the C1 protocol.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Mustache => "mustache",
            Self::Mixed => "mixed",
            Self::Boolean => "boolean",
        }
    }
}

/// One attribute on a custom-component usage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentAttribute {
    /// Attribute name as written.
    pub name: String,
    /// How the value is written.
    pub value_kind: AttributeValueKind,
    /// The sole mustache expression text for `Mustache` values.
    pub expr_text: Option<String>,
    /// Byte range of [`Self::expr_text`] in the original template.
    pub expr_range: Option<SourceRange>,
    /// Byte range of the whole attribute.
    pub attribute_range: SourceRange,
}

/// One custom-component usage in a template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentUsage {
    /// Tag name as written, for example `card`.
    pub tag: String,
    /// Resolved component script, when `usingComponents` pointed at a local
    /// file.
    pub component_file: Option<FileId>,
    /// Attributes in source order.
    pub attributes: Vec<ComponentAttribute>,
}

/// One expression that a type backend should analyze.
///
/// `wxml_range` covers the trimmed expression inside `{{ ... }}`. `expr_text` is
/// exactly the same source slice; `locals` lists the template bindings that were
/// in scope at the mustache so the backend can avoid rooting them.
///
/// `bindings` and `guards` are the structured successors of `locals` and the
/// future `wx:if` narrowing input. They are present so the C1 protocol and the
/// backends agree on the shape before the template analyzer fills them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckFragment {
    /// Stable fragment identity within this template revision, for example
    /// `f0`.
    pub fragment_id: String,
    /// Original expression text without the surrounding `{{` and `}}`.
    pub expr_text: String,
    /// Preferred token for hover and diagnostics, for example `name` for
    /// `user.name`. `None` lets the backend use its default (the first
    /// identifier).
    pub target: Option<String>,
    /// Template binding names in scope, outermost first and deduplicated.
    ///
    /// Kept for C1.1 compatibility; [`Self::bindings`] is the structured form.
    pub locals: Vec<String>,
    /// Structured `wx:for` bindings in scope, outermost first.
    pub bindings: Vec<LocalBinding>,
    /// Enclosing `wx:if`/`wx:elif`/`wx:else` chains, outermost first.
    pub guards: Vec<GuardChain>,
    /// Byte range of the expression in the original WXML source.
    pub wxml_range: SourceRange,
    /// Revision of the source document this fragment was produced from.
    pub fragment_revision: u64,
    /// Script data root for the owning page or component.
    pub root: RootKind,
}

/// The attribute prefix that produced an event binding.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventBindingKind {
    /// `bindtap` or `bind:tap`.
    Bind,
    /// `catchtap` or `catch:tap`.
    Catch,
    /// `capture-bind:tap` or `capture-bindtap`.
    CaptureBind,
    /// `capture-catch:tap` or `capture-catchtap`.
    CaptureCatch,
    /// `mut-bind:tap` or `mut-bindtap`.
    MutBind,
}

impl EventBindingKind {
    /// Returns the stable name of the binding prefix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bind => "bind",
            Self::Catch => "catch",
            Self::CaptureBind => "capture-bind",
            Self::CaptureCatch => "capture-catch",
            Self::MutBind => "mut-bind",
        }
    }
}

/// A statically bound event handler.
///
/// Dynamic handlers (`bindtap="{{ handler }}"`) are reported as
/// [`UnsupportedKind::DynamicEvent`] instead of an `EventHandler`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventHandler {
    /// Raw attribute name, for example `capture-bind:tap`.
    pub attribute: String,
    /// Binding prefix family.
    pub binding: EventBindingKind,
    /// Normalized event name, for example `tap`.
    pub event: String,
    /// Static handler name as written, trimmed.
    pub handler: String,
    /// Byte range of the whole attribute.
    ///
    /// Kept as the whole-attribute range for C1.1 consumers;
    /// [`Self::attribute_range`] carries the same value under the explicit name.
    pub range: SourceRange,
    /// Byte range of the whole attribute.
    pub attribute_range: SourceRange,
    /// Byte range of the handler token only, without quotes or surrounding
    /// whitespace.
    pub handler_range: SourceRange,
}

/// Why a template construct was not turned into a fragment or event.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UnsupportedKind {
    /// `<wxs>` blocks are not analyzed in this phase.
    Wxs,
    /// `<template>` definitions are not analyzed in this phase.
    Template,
    /// `<import>` statements are not analyzed in this phase.
    Import,
    /// `<include>` statements are not analyzed in this phase.
    Include,
    /// An event handler is a mustache or mixed dynamic value.
    DynamicEvent,
    /// An event binding has no non-empty static handler name.
    EmptyEvent,
    /// A `wx:for-item`/`wx:for-index` value is dynamic or empty.
    DynamicForBinding,
    /// The expression tree contains missing nodes or unconsumed tokens.
    MalformedExpression,
    /// A mustache contains only whitespace.
    EmptyExpression,
    /// An expression range is not a valid UTF-8 slice of the source.
    InvalidRange,
    /// The fragment budget was exhausted and later mustaches were skipped.
    FragmentLimit,
}

impl UnsupportedKind {
    /// Returns a short, stable explanation suitable for logs and diagnostics.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Wxs => "wxs blocks are not analyzed in this phase",
            Self::Template => "template definitions are not analyzed in this phase",
            Self::Import => "template imports are not analyzed in this phase",
            Self::Include => "template includes are not analyzed in this phase",
            Self::DynamicEvent => "dynamic event handlers are not analyzed",
            Self::EmptyEvent => "event binding has no static handler",
            Self::DynamicForBinding => "wx:for binding names must be static and non-empty",
            Self::MalformedExpression => "expression could not be parsed completely",
            Self::EmptyExpression => "mustache contains no expression",
            Self::InvalidRange => "expression range is not on UTF-8 character boundaries",
            Self::FragmentLimit => "fragment limit reached; later mustaches were skipped",
        }
    }
}

/// A template construct that produced no fragment or event.
///
/// Every variant carries the byte range of the construct. The category is also
/// available through [`UnsupportedFeature::kind`] for code that groups records
/// without matching every variant.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnsupportedFeature {
    /// `<wxs>` blocks are not analyzed in this phase.
    Wxs {
        /// Byte range of the element in the original WXML source.
        range: SourceRange,
    },
    /// `<template>` definitions are not analyzed in this phase.
    Template {
        /// Byte range of the element in the original WXML source.
        range: SourceRange,
    },
    /// `<import>` statements are not analyzed in this phase.
    Import {
        /// Byte range of the element in the original WXML source.
        range: SourceRange,
    },
    /// `<include>` statements are not analyzed in this phase.
    Include {
        /// Byte range of the element in the original WXML source.
        range: SourceRange,
    },
    /// An event handler is a mustache or mixed dynamic value.
    DynamicEvent {
        /// Byte range of the dynamic value or attribute.
        range: SourceRange,
    },
    /// An event binding has no non-empty static handler name.
    EmptyEvent {
        /// Byte range of the event attribute.
        range: SourceRange,
    },
    /// A `wx:for-item`/`wx:for-index` value is dynamic or empty.
    DynamicForBinding {
        /// Byte range of the binding attribute.
        range: SourceRange,
    },
    /// The expression tree contains missing nodes or unconsumed tokens.
    MalformedExpression {
        /// Byte range of the expression.
        range: SourceRange,
    },
    /// A mustache contains only whitespace.
    EmptyExpression {
        /// Byte range of the empty expression.
        range: SourceRange,
    },
    /// An expression range is not a valid UTF-8 slice of the source.
    InvalidRange {
        /// Byte range that failed validation.
        range: SourceRange,
    },
    /// The fragment budget was exhausted and this mustache was skipped.
    FragmentLimit {
        /// Byte range of the first skipped expression.
        range: SourceRange,
    },
}

impl UnsupportedFeature {
    /// Builds a feature record with the standard reason for `kind`.
    #[must_use]
    pub const fn new(kind: UnsupportedKind, range: SourceRange) -> Self {
        match kind {
            UnsupportedKind::Wxs => Self::Wxs { range },
            UnsupportedKind::Template => Self::Template { range },
            UnsupportedKind::Import => Self::Import { range },
            UnsupportedKind::Include => Self::Include { range },
            UnsupportedKind::DynamicEvent => Self::DynamicEvent { range },
            UnsupportedKind::EmptyEvent => Self::EmptyEvent { range },
            UnsupportedKind::DynamicForBinding => Self::DynamicForBinding { range },
            UnsupportedKind::MalformedExpression => Self::MalformedExpression { range },
            UnsupportedKind::EmptyExpression => Self::EmptyExpression { range },
            UnsupportedKind::InvalidRange => Self::InvalidRange { range },
            UnsupportedKind::FragmentLimit => Self::FragmentLimit { range },
        }
    }

    /// Returns the category of this record.
    #[must_use]
    pub const fn kind(&self) -> UnsupportedKind {
        match self {
            Self::Wxs { .. } => UnsupportedKind::Wxs,
            Self::Template { .. } => UnsupportedKind::Template,
            Self::Import { .. } => UnsupportedKind::Import,
            Self::Include { .. } => UnsupportedKind::Include,
            Self::DynamicEvent { .. } => UnsupportedKind::DynamicEvent,
            Self::EmptyEvent { .. } => UnsupportedKind::EmptyEvent,
            Self::DynamicForBinding { .. } => UnsupportedKind::DynamicForBinding,
            Self::MalformedExpression { .. } => UnsupportedKind::MalformedExpression,
            Self::EmptyExpression { .. } => UnsupportedKind::EmptyExpression,
            Self::InvalidRange { .. } => UnsupportedKind::InvalidRange,
            Self::FragmentLimit { .. } => UnsupportedKind::FragmentLimit,
        }
    }

    /// Returns the byte range of the construct.
    #[must_use]
    pub const fn range(&self) -> SourceRange {
        match self {
            Self::Wxs { range }
            | Self::Template { range }
            | Self::Import { range }
            | Self::Include { range }
            | Self::DynamicEvent { range }
            | Self::EmptyEvent { range }
            | Self::DynamicForBinding { range }
            | Self::MalformedExpression { range }
            | Self::EmptyExpression { range }
            | Self::InvalidRange { range }
            | Self::FragmentLimit { range } => *range,
        }
    }

    /// Returns the short static explanation for this record.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.kind().reason()
    }
}
/// Everything the template analyzer learned from one document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateAnalysis {
    /// Fragments in source order with ids `f0`, `f1`, ...
    pub fragments: Vec<CheckFragment>,
    /// Static event handlers in traversal order.
    pub events: Vec<EventHandler>,
    /// Constructs that were skipped, in traversal order.
    pub unsupported: Vec<UnsupportedFeature>,
    /// `true` when [`crate::template::MAX_FRAGMENTS`] was reached and later
    /// data mustaches were skipped. Event bindings and unsupported elements are
    /// still collected.
    ///
    /// The analysis is still usable: events and unsupported records are complete
    /// up to the traversal point that hit the limit, and the first skipped
    /// mustache is recorded as [`UnsupportedKind::FragmentLimit`]. Consumers
    /// must not treat the result as complete.
    pub truncated: bool,
}

impl TemplateAnalysis {
    /// Returns `true` when the template produced no fragments, events or
    /// unsupported records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
            && self.events.is_empty()
            && self.unsupported.is_empty()
            && !self.truncated
    }

    /// Returns `true` when the fragment limit stopped analysis early.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ProjectId;

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::from_bounds(start, end).expect("valid range")
    }

    #[test]
    fn local_bindings_record_role_and_loop_source() {
        let binding = LocalBinding {
            name: "row".to_owned(),
            role: LocalRole::Item,
            source_expr: "groups".to_owned(),
            source_range: range(20, 26),
        };
        assert_eq!(binding.role, LocalRole::Item);
        assert_eq!(binding.role.as_str(), "item");
        assert_eq!(binding.name, "row");
        assert_eq!(binding.source_expr, "groups");
        assert_eq!(binding.source_range.start(), 20);
        assert_eq!(LocalRole::Index.as_str(), "index");
    }

    #[test]
    fn guard_chains_encode_if_elif_else_selection() {
        let chain = GuardChain {
            conditions: vec![
                GuardCondition {
                    branch: GuardBranch::If,
                    expr_text: "state.kind === 'loading'".to_owned(),
                    range: range(10, 35),
                },
                GuardCondition {
                    branch: GuardBranch::Elif,
                    expr_text: "state.kind === 'ready'".to_owned(),
                    range: range(50, 73),
                },
            ],
            selected: 2,
        };
        assert_eq!(chain.conditions.len(), 2);
        assert_eq!(chain.conditions[0].branch.as_str(), "if");
        assert_eq!(chain.conditions[1].branch.as_str(), "elif");
        assert_eq!(chain.selected, chain.conditions.len());
        assert_eq!(GuardBranch::Else.as_str(), "else");
    }

    #[test]
    fn component_usage_keeps_value_kinds_and_ranges() {
        let project = ProjectId::new("example");
        let file = FileId::new(project, "components/card/card.ts").expect("file");
        let usage = ComponentUsage {
            tag: "card".to_owned(),
            component_file: Some(file.clone()),
            attributes: vec![
                ComponentAttribute {
                    name: "title".to_owned(),
                    value_kind: AttributeValueKind::Mustache,
                    expr_text: Some("pageTitle".to_owned()),
                    expr_range: Some(range(40, 49)),
                    attribute_range: range(30, 52),
                },
                ComponentAttribute {
                    name: "disabled".to_owned(),
                    value_kind: AttributeValueKind::Boolean,
                    expr_text: None,
                    expr_range: None,
                    attribute_range: range(53, 61),
                },
            ],
        };
        assert_eq!(usage.component_file, Some(file));
        assert_eq!(usage.attributes[0].value_kind.as_str(), "mustache");
        assert_eq!(usage.attributes[0].expr_text.as_deref(), Some("pageTitle"));
        assert_eq!(usage.attributes[1].value_kind, AttributeValueKind::Boolean);
        assert_eq!(usage.attributes[1].attribute_range.start(), 53);
        assert_eq!(usage.attributes[1].expr_range, None);
    }

    #[test]
    fn attribute_value_kinds_have_stable_names() {
        assert_eq!(AttributeValueKind::Literal.as_str(), "literal");
        assert_eq!(AttributeValueKind::Mixed.as_str(), "mixed");
        assert_eq!(AttributeValueKind::Boolean.as_str(), "boolean");
    }
}
