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

/// One expression that a type backend should analyze.
///
/// `wxml_range` covers the trimmed expression inside `{{ ... }}`. `expr_text` is
/// exactly the same source slice; `locals` lists the template bindings that were
/// in scope at the mustache so the backend can avoid rooting them.
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
    /// Template bindings in scope, outermost first and deduplicated.
    pub locals: Vec<String>,
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
    pub range: SourceRange,
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
