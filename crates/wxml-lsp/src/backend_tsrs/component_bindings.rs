//! Component property binding diagnostics.
//!
//! The template analyzer already records every custom-component usage and
//! classifies each non-event attribute value, and the project host prepares
//! declaration-first property facts. This pass keeps the checks that need no
//! type engine:
//!
//! - a static `name="value"` literal or a valueless attribute against the
//!   declared property type;
//! - an attribute the component declaration does not mention;
//! - coverage (`Partial`) reasons for unreadable component facts, incomplete
//!   behaviors, mixed text and the per-tag cap.
//!
//! A pure mustache expression is checked by the engine through the contract
//! probe the request plan attaches to the fragment, so this pass never queries
//! an expression type and never publishes a type diagnostic for one. When the
//! facts are incomplete or the component script is missing, the usage keeps its
//! existing `Partial`/skip result; the mustache itself is not re-judged here.
//!
//! Event bindings are excluded by `ComponentUsage::attributes` upstream; this
//! pass never inspects raw element attributes, so `bind*`/`catch*`/
//! `capture-*`/`mut-bind`/`slot`/`wx:*` cannot become property diagnostics.

use std::collections::{BTreeMap, BTreeSet};

use wxml_backend_tsrs::script::properties::PropertyFact;
use wxml_project::typing::{PrimitiveType, TypeText, TypingComponent, TypingProperty};
use wxml_types::{
    AttributeValueKind, CapabilityStatus, ComponentAttribute, ComponentUsage, Diagnostic,
    DiagnosticSeverity, SourceRange,
};

use crate::backend_tsrs::component_facts::{
    ComponentFactsCache, ComponentUsageFacts, ScriptPropertyFacts,
};
use crate::backend_tsrs::mapping::ShadowLayout;
use crate::project_host::PreparedView;

/// Maximum distinct component tags checked in one template.
const MAX_TAGS: usize = 64;
/// Maximum published diagnostics for one component tag.
const MAX_DIAGNOSTICS_PER_TAG: usize = 8;
/// Maximum `tag.attribute` examples quoted in the `Partial` reason.
const MAX_SKIPPED_DETAILS: usize = 3;
/// Maximum published message length after sanitization.
const MAX_MESSAGE_LENGTH: usize = 512;

/// Stable prefix shared with the Node backend's `Partial` reason.
const SKIPPED_PREFIX: &str = "some component bindings could not be resolved and were not checked";

/// Diagnostics produced by the component binding pass.
pub(crate) struct ComponentBindingDiagnostics {
    /// Errors in original template bytes, deduplicated and sorted.
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// A `Partial` status when coverage is incomplete, `None` otherwise.
    pub(crate) status: Option<CapabilityStatus>,
}

/// Runs the component binding pass over one prepared view.
///
/// The pass only reads the original template and the shared component facts:
/// pure mustache compatibility belongs to the engine contract probe.
pub(crate) fn diagnostics(
    layout: &ShadowLayout,
    view: &PreparedView,
) -> ComponentBindingDiagnostics {
    if view.components.is_empty() {
        return ComponentBindingDiagnostics {
            diagnostics: Vec::new(),
            status: None,
        };
    }
    let mut pass = Pass::new(layout, view);
    pass.run();
    pass.finish()
}

/// One declared script property and its static-literal contract.
#[derive(Clone, Debug)]
struct DeclaredBinding {
    /// Declaration text shown in the message.
    text: String,
    /// What a static WXML literal must satisfy.
    contract: LiteralContract,
}

/// One value shape a declared property can accept from a WXML literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiteralKind {
    /// Primitive `string`.
    String,
    /// Primitive `number`.
    Number,
    /// Primitive `boolean`.
    Boolean,
    /// Array-like value.
    Array,
    /// Object-like value.
    Object,
}

/// What one declared property accepts from a static WXML literal value.
///
/// Only the literal forms are modelled: a quoted attribute is always a string
/// and a valueless attribute is always a boolean, so shapes that need a
/// structural comparison (array contents, object members, conditionals) are not
/// represented here. Pure mustache values are checked by the engine instead.
#[derive(Clone, Debug, Eq, PartialEq)]
enum LiteralContract {
    /// The declaration is unproven or not representable: never diagnosed.
    Unknown,
    /// `any` accepts every value.
    Any,
    /// One or more primitive/structural shapes.
    Shapes(Vec<LiteralKind>),
    /// A union of string literals; only the listed values are accepted.
    StringLiterals(Vec<String>),
    /// A union of numeric literals; a WXML literal is always a string and can
    /// never satisfy it.
    NumberLiterals,
}

impl LiteralContract {
    /// Maps one declared member to a literal contract.
    ///
    /// `proven == false` always yields [`LiteralContract::Unknown`]: the
    /// declaration parser did not understand the whole expression.
    fn from_property(property: &TypingProperty) -> Self {
        if !property.proven {
            return Self::Unknown;
        }
        Self::from_type(&property.ty)
    }

    /// Maps one parsed type expression to a literal contract.
    fn from_type(ty: &TypeText) -> Self {
        match ty {
            TypeText::Primitive(primitive) => Self::from_primitive(*primitive),
            TypeText::StringLiteral(value) => Self::StringLiterals(vec![value.clone()]),
            TypeText::NumberLiteral(_) => Self::NumberLiterals,
            TypeText::BooleanLiteral(_) => Self::Shapes(vec![LiteralKind::Boolean]),
            TypeText::Array(_) => Self::Shapes(vec![LiteralKind::Array]),
            TypeText::Record(_) | TypeText::Object(_) => Self::Shapes(vec![LiteralKind::Object]),
            TypeText::Union(parts) => Self::from_union(parts),
            TypeText::Unproven => Self::Unknown,
        }
    }

    /// Maps one primitive type to a literal contract.
    fn from_primitive(primitive: PrimitiveType) -> Self {
        match primitive {
            PrimitiveType::String => Self::Shapes(vec![LiteralKind::String]),
            PrimitiveType::Number => Self::Shapes(vec![LiteralKind::Number]),
            PrimitiveType::Boolean => Self::Shapes(vec![LiteralKind::Boolean]),
            PrimitiveType::Object => Self::Shapes(vec![LiteralKind::Object]),
            PrimitiveType::Any => Self::Any,
            // `null`/`undefined`/`void` alone admit no bound value, and
            // `unknown`/`never` cannot be narrowed into a literal shape.
            _ => Self::Unknown,
        }
    }

    /// Maps a union to a literal contract, treating nullish alternatives as
    /// absence.
    fn from_union(parts: &[TypeText]) -> Self {
        let mut alternatives = Vec::new();
        for part in parts {
            flatten_union(part, &mut alternatives);
        }
        let mut shapes = Vec::new();
        let mut strings: Vec<String> = Vec::new();
        let mut numbers = false;
        for part in alternatives {
            match part {
                // Nullish alternatives only widen optionality; `never`
                // contributes no value.
                TypeText::Primitive(
                    PrimitiveType::Null
                    | PrimitiveType::Undefined
                    | PrimitiveType::Void
                    | PrimitiveType::Never,
                ) => {}
                TypeText::Primitive(PrimitiveType::String) => {
                    push_shape(&mut shapes, LiteralKind::String);
                }
                TypeText::Primitive(PrimitiveType::Number) => {
                    push_shape(&mut shapes, LiteralKind::Number);
                }
                TypeText::Primitive(PrimitiveType::Boolean) | TypeText::BooleanLiteral(_) => {
                    push_shape(&mut shapes, LiteralKind::Boolean);
                }
                TypeText::Primitive(PrimitiveType::Any) => return Self::Any,
                TypeText::StringLiteral(value) => push_unique(&mut strings, value),
                TypeText::NumberLiteral(_) => numbers = true,
                TypeText::Array(_) => push_shape(&mut shapes, LiteralKind::Array),
                TypeText::Primitive(PrimitiveType::Object)
                | TypeText::Record(_)
                | TypeText::Object(_) => push_shape(&mut shapes, LiteralKind::Object),
                // An empty nested union, an unsupported primitive or an
                // unproven part cannot be narrowed into a contract.
                TypeText::Primitive(_) | TypeText::Union(_) | TypeText::Unproven => {
                    return Self::Unknown;
                }
            }
        }
        if shapes.is_empty() && strings.is_empty() && numbers {
            return Self::NumberLiterals;
        }
        if shapes.is_empty() && !strings.is_empty() && !numbers {
            return Self::StringLiterals(strings);
        }
        // Mixed primitive kinds or several shapes cannot be expressed as one
        // literal contract; a single shape may absorb same-domain literals.
        if shapes.len() != 1 {
            return Self::Unknown;
        }
        let representable = match shapes[0] {
            // `string | 'a' | 'b'` still accepts the whole string domain.
            LiteralKind::String => !numbers,
            // A numeric shape absorbs numeric literals; a string literal never
            // matches it.
            LiteralKind::Number => strings.is_empty(),
            LiteralKind::Boolean | LiteralKind::Array | LiteralKind::Object => {
                strings.is_empty() && !numbers
            }
        };
        if representable {
            Self::Shapes(shapes)
        } else {
            Self::Unknown
        }
    }

    /// Maps one script `properties` union to a literal contract.
    ///
    /// The overlay only normalizes the five `WeChat` constructors below. Every
    /// other text (`any`, `Function`, a non-literal `properties` object) stays
    /// unrepresented so callers keep it as a coverage gap instead of guessing.
    fn from_script_text(text: &str) -> Option<Self> {
        let parts = split_union(text);
        if parts.is_empty() {
            return None;
        }
        let mut shapes = Vec::with_capacity(parts.len());
        for part in parts {
            let shape = match part.trim() {
                "string" => LiteralKind::String,
                "number" => LiteralKind::Number,
                "boolean" => LiteralKind::Boolean,
                "any[]" => LiteralKind::Array,
                "Record<string, any>" => LiteralKind::Object,
                _ => return None,
            };
            push_shape(&mut shapes, shape);
        }
        Some(Self::Shapes(shapes))
    }

    /// Judges one static literal attribute value.
    ///
    /// `None` means the declaration cannot be represented, which keeps the
    /// attribute a coverage gap instead of an error.
    fn judge_literal(&self, value: LiteralValue<'_>) -> Option<bool> {
        match self {
            Self::Unknown => None,
            Self::Any => Some(true),
            Self::Shapes(shapes) => Some(shapes.contains(&value.shape())),
            Self::StringLiterals(allowed) => Some(match value {
                LiteralValue::String(text) => allowed.iter().any(|allowed| allowed == text),
                LiteralValue::Boolean => false,
            }),
            // A WXML literal attribute is always a string, never a number.
            Self::NumberLiterals => Some(false),
        }
    }
}

/// One static attribute value.
#[derive(Clone, Copy, Debug)]
enum LiteralValue<'a> {
    /// A quoted attribute value, for example `type="primary"`.
    String(&'a str),
    /// A valueless attribute, for example `disabled`.
    Boolean,
}

impl LiteralValue<'_> {
    /// Returns the value shape a declared contract is matched against.
    const fn shape(self) -> LiteralKind {
        match self {
            Self::String(_) => LiteralKind::String,
            Self::Boolean => LiteralKind::Boolean,
        }
    }
}

/// Runs the component binding pass with declaration-first property facts.
struct Pass<'a> {
    layout: &'a ShadowLayout,
    view: &'a PreparedView,
    /// Declaration-first property facts, cached by component path.
    facts: ComponentFactsCache<'a>,
    tags: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    per_tag: BTreeMap<String, usize>,
    details: Vec<String>,
    /// 组件 behaviors 未能完全解析、但自身 properties 完整时，记录按自身声明
    /// 校验过的 tag；这些 tag 的结果可用但不完整。
    behavior_incomplete: BTreeSet<String>,
    skipped: usize,
    partial: bool,
}

impl<'a> Pass<'a> {
    fn new(layout: &'a ShadowLayout, view: &'a PreparedView) -> Self {
        Self {
            layout,
            view,
            facts: ComponentFactsCache::new(view),
            tags: BTreeSet::new(),
            diagnostics: Vec::new(),
            per_tag: BTreeMap::new(),
            details: Vec::new(),
            behavior_incomplete: BTreeSet::new(),
            skipped: 0,
            partial: false,
        }
    }

    fn run(&mut self) {
        for usage in self.view.components.iter() {
            if usage.attributes.is_empty() {
                continue;
            }
            if !self.tags.contains(&usage.tag) {
                if self.tags.len() >= MAX_TAGS {
                    self.partial = true;
                    self.skip(&usage.tag);
                    continue;
                }
                self.tags.insert(usage.tag.clone());
            }
            // A structurally complete declaration is the whole attribute
            // surface: the script `properties` object is never consulted.
            match self.facts.resolve(usage) {
                ComponentUsageFacts::Declared(typing) => self.check_declared_usage(usage, &typing),
                ComponentUsageFacts::Script(properties) => {
                    self.check_script_usage(usage, &properties);
                }
                ComponentUsageFacts::Unavailable => {
                    // No project root or no representable file set: every script
                    // derived usage is unproven, but an empty attribute list still
                    // needs no reason.
                    if !self.facts.has_input() {
                        self.partial = true;
                    }
                    self.skip(&usage.tag);
                }
            }
        }
    }

    /// Checks one usage whose component is described by script `properties`.
    ///
    /// An attribute the script does not declare stays ignored: it may come from
    /// an unreadable behavior or belong to the host element, exactly like the
    /// Node backend.
    fn check_script_usage(&mut self, usage: &ComponentUsage, properties: &ScriptPropertyFacts) {
        if properties.is_empty() {
            // A component without declared properties makes the binding
            // surface unproven, not clean.
            self.skip(&usage.tag);
            return;
        }
        if !properties.complete() {
            // behaviors 未能解析：只按组件自身声明的属性校验，未声明的属性可能来自
            // behavior，因此不能报错，只能标记结果不完整。
            self.behavior_incomplete.insert(usage.tag.clone());
        }
        for attribute in &usage.attributes {
            match declared_property(properties, &attribute.name) {
                // `class`, `style`, `id` and undeclared attributes are not
                // component properties.
                DeclaredLookup::Undeclared => {}
                DeclaredLookup::Unproven => {
                    self.partial = true;
                    self.skip(&format!("{}.{}", usage.tag, attribute.name));
                }
                DeclaredLookup::Checkable(declared) => {
                    self.check_static_attribute(
                        &usage.tag,
                        attribute,
                        &declared.contract,
                        &declared.text,
                    );
                }
            }
        }
    }

    /// Checks one attribute of a component that has declaration facts.
    ///
    /// Each member is judged on its own `proven` flag: an unproven member is a
    /// coverage gap, while the remaining declared members are still checked.
    fn check_declared_usage(&mut self, usage: &ComponentUsage, typing: &TypingComponent) {
        for attribute in &usage.attributes {
            match declared_typing_property(typing, &attribute.name) {
                // The declared `props` set is the whole attribute surface, so
                // an attribute it does not mention is an error except for the
                // universal WXML attributes.
                None => {
                    if !is_reserved_attribute(&attribute.name) {
                        self.publish_undeclared(&usage.tag, attribute);
                    }
                }
                Some(property) => self.check_declared_attribute(&usage.tag, attribute, property),
            }
        }
    }

    /// Checks one declared attribute against its literal contract.
    fn check_declared_attribute(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        property: &TypingProperty,
    ) {
        let contract = LiteralContract::from_property(property);
        self.check_static_attribute(tag, attribute, &contract, &property.type_text);
    }

    /// Checks one attribute whose value form needs no type engine.
    ///
    /// A pure mustache is delegated to the engine contract probe: it is neither
    /// checked nor recorded as a coverage gap here.
    fn check_static_attribute(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        contract: &LiteralContract,
        type_text: &str,
    ) {
        match attribute.value_kind {
            AttributeValueKind::Literal => {
                let value =
                    literal_value(self.layout.original_wxml(), attribute).map(str::to_owned);
                let Some(value) = value else {
                    self.partial = true;
                    self.skip(&format!("{tag}.{}", attribute.name));
                    return;
                };
                self.check_literal(
                    tag,
                    attribute,
                    contract,
                    type_text,
                    LiteralValue::String(&value),
                );
            }
            AttributeValueKind::Boolean => {
                self.check_literal(tag, attribute, contract, type_text, LiteralValue::Boolean);
            }
            AttributeValueKind::Mustache => {}
            // `Mixed` needs a string conversion and a future value kind is
            // unknown: both are coverage gaps, never errors.
            _ => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
        }
    }

    /// Judges one static literal value against its contract.
    fn check_literal(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        contract: &LiteralContract,
        type_text: &str,
        value: LiteralValue<'_>,
    ) {
        match contract.judge_literal(value) {
            Some(true) => {}
            None => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
            Some(false) => {
                let got = match value {
                    LiteralValue::String(_) => "string literal",
                    LiteralValue::Boolean => "boolean literal",
                };
                let message = format!(
                    "component property \"{}\" expects {type_text}, got {got}",
                    attribute.name,
                );
                self.publish(tag, &attribute.name, attribute.attribute_range, &message);
            }
        }
    }

    /// Publishes the diagnostic for an attribute the declaration omits.
    fn publish_undeclared(&mut self, tag: &str, attribute: &ComponentAttribute) {
        let message = format!(
            "component property \"{}\" is not declared by the component declaration",
            attribute.name,
        );
        self.publish(
            tag,
            &attribute.name,
            attribute_name_range(attribute),
            &message,
        );
    }

    fn publish(&mut self, tag: &str, attribute: &str, range: SourceRange, message: &str) {
        let count = self.per_tag.entry(tag.to_owned()).or_default();
        if *count >= MAX_DIAGNOSTICS_PER_TAG {
            self.partial = true;
            self.skip(&format!("{tag}.{attribute}"));
            return;
        }
        *count = count.saturating_add(1);
        self.diagnostics.push(Diagnostic {
            range,
            severity: DiagnosticSeverity::Error,
            message: truncate(message, MAX_MESSAGE_LENGTH).to_owned(),
            code: None,
        });
    }

    fn skip(&mut self, detail: &str) {
        self.skipped = self.skipped.saturating_add(1);
        if self.details.len() < MAX_SKIPPED_DETAILS
            && !self.details.iter().any(|item| item == detail)
        {
            self.details.push(detail.to_owned());
        }
    }

    fn finish(mut self) -> ComponentBindingDiagnostics {
        let mut seen = BTreeSet::new();
        self.diagnostics.retain(|diagnostic| {
            seen.insert((
                diagnostic.range.start(),
                diagnostic.range.end(),
                diagnostic.message.clone(),
            ))
        });
        self.diagnostics
            .sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
        let mut reasons = Vec::new();
        if self.partial || self.skipped > 0 {
            let detail = if self.details.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.details.join(", "))
            };
            reasons.push(format!("{SKIPPED_PREFIX}{detail}"));
        }
        if !self.behavior_incomplete.is_empty() {
            reasons.push(behavior_partial_reason(&self.behavior_incomplete));
        }
        let status = if reasons.is_empty() {
            None
        } else {
            Some(CapabilityStatus::Partial {
                reason: reasons.join("; "),
            })
        };
        ComponentBindingDiagnostics {
            diagnostics: self.diagnostics,
            status,
        }
    }
}

/// 生成 behavior 不可解析时的 `Partial` 说明。
///
/// tag 列表与既有 skip 明细同风格：最多引用 [`MAX_SKIPPED_DETAILS`] 个类名。
fn behavior_partial_reason(tags: &BTreeSet<String>) -> String {
    let names = tags
        .iter()
        .take(MAX_SKIPPED_DETAILS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if tags.len() > MAX_SKIPPED_DETAILS {
        " 等"
    } else {
        ""
    };
    format!(
        "组件 behaviors 未能完全解析，仅校验了组件自身 properties 声明的属性（{names}{suffix}）"
    )
}

/// Parses one `PropertyFact` into a checkable literal binding.
fn declared_binding(fact: &PropertyFact) -> Option<DeclaredBinding> {
    if !fact.type_complete {
        return None;
    }
    Some(DeclaredBinding {
        text: fact.type_text.clone(),
        contract: LiteralContract::from_script_text(&fact.type_text)?,
    })
}

/// Returns the declaration for `attribute` using the shared kebab/camel rule.
///
/// The lookup distinguishes an undeclared attribute (ignore it) from a
/// declared but unproven one (skip it and report partial coverage).
fn declared_property(properties: &ScriptPropertyFacts, attribute: &str) -> DeclaredLookup {
    let Some(fact) = properties.member(attribute) else {
        return DeclaredLookup::Undeclared;
    };
    declared_binding(fact).map_or(DeclaredLookup::Unproven, DeclaredLookup::Checkable)
}

/// Result of one component attribute name lookup.
enum DeclaredLookup {
    /// The attribute is not a declared property; ignore it.
    Undeclared,
    /// The property is declared but its type could not be proven.
    Unproven,
    /// The property has a checkable literal contract.
    Checkable(DeclaredBinding),
}

/// Returns one declared member, honouring the `WeChat` kebab/camel mapping.
fn declared_typing_property<'a>(
    typing: &'a TypingComponent,
    attribute: &str,
) -> Option<&'a TypingProperty> {
    typing.prop_named(attribute)
}

/// Flattens a possibly nested union into its alternatives.
fn flatten_union<'a>(ty: &'a TypeText, out: &mut Vec<&'a TypeText>) {
    match ty {
        TypeText::Union(parts) => {
            for part in parts {
                flatten_union(part, out);
            }
        }
        other => out.push(other),
    }
}

/// Pushes a shape once, preserving declaration order.
fn push_shape(shapes: &mut Vec<LiteralKind>, shape: LiteralKind) {
    if !shapes.contains(&shape) {
        shapes.push(shape);
    }
}

/// Pushes a literal value once, preserving declaration order.
fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|kept| kept == value) {
        values.push(value.to_owned());
    }
}

/// Returns the literal value of one `name="value"` attribute.
///
/// `document` is the original template, so the slice is exactly what the
/// analyzer recorded; an unquoted value is returned verbatim.
fn literal_value<'a>(document: &'a str, attribute: &ComponentAttribute) -> Option<&'a str> {
    let start = usize::try_from(attribute.attribute_range.start()).ok()?;
    let end = usize::try_from(attribute.attribute_range.end()).ok()?;
    let text = document.get(start..end)?;
    let (_, raw) = text.split_once('=')?;
    let raw = raw.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = raw
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return Some(inner);
        }
    }
    Some(raw)
}

/// Returns the byte range of one attribute name inside its attribute range.
fn attribute_name_range(attribute: &ComponentAttribute) -> SourceRange {
    let start = attribute.attribute_range.start();
    let length = u32::try_from(attribute.name.len()).unwrap_or(u32::MAX);
    let end = start
        .saturating_add(length)
        .min(attribute.attribute_range.end());
    SourceRange::from_bounds(start, end).unwrap_or(attribute.attribute_range)
}

/// Returns whether an attribute is a universal WXML attribute, never a property.
fn is_reserved_attribute(name: &str) -> bool {
    matches!(name, "class" | "style" | "id" | "hidden" | "slot" | "role")
        || name.starts_with("data-")
        || name.starts_with("aria-")
        || name.starts_with("wx:")
}

/// Splits a top-level union on `|`, respecting nesting and quoted strings.
fn split_union(text: &str) -> Vec<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut depth = 0_u32;
    let mut quote = None;
    let mut start = 0_usize;
    for (index, character) in trimmed.char_indices() {
        if let Some(active) = quote {
            if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => quote = Some(character),
            '(' | '[' | '{' | '<' => depth = depth.saturating_add(1),
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                parts.push(trimmed[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(trimmed[start..].trim());
    parts.retain(|part| !part.is_empty());
    parts
}

/// Truncates to a UTF-8-safe prefix of at most `limit` bytes.
fn truncate(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}

#[cfg(test)]
#[allow(
    clippy::assert_is_empty,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use wxml_backend_tsrs::shadow::mapping::ShadowMap;
    use wxml_backend_tsrs::shadow::rewrite::Assembled;
    use wxml_project::ComponentTypingFacts;
    use wxml_project::ProjectPath;
    use wxml_types::{
        ComponentUsage, FileId, ProjectId, RootKind, SnapshotId, SourceRevision, SourceSnapshot,
        ViewRevision,
    };

    use super::*;
    use crate::project_host::{PreparedFile, ProjectHost, ProjectState, ViewTarget};

    const SCRIPT_PATH: &str = "pages/index/index.ts";
    const CARD_PATH: &str = "components/card/card.ts";
    const BADGE_PATH: &str = "components/badge/badge.ts";
    const BARE_PATH: &str = "bare.ts";
    const PLAIN_PATH: &str = "plain.ts";
    const SPREAD_PATH: &str = "spread.ts";
    const BEHAVIOR_CARD_PATH: &str = "behavior-card.ts";
    /// Component whose behavior is imported through a tsconfig `paths` alias.
    const ALIAS_CARD_PATH: &str = "components/alias-card/alias-card.ts";
    /// Behavior target of the `@/*` alias.
    const ALIAS_BEHAVIOR_PATH: &str = "behaviors/platform.ts";
    /// Fixed declaration file next to the project `tsconfig.json`.
    const DECLARATION_PATH: &str = "components.d.ts";
    const PAGE_SCRIPT: &str =
        "Page({\n  data: { pageTitle: '首页', total: 3, user: { name: 'Ada' } },\n});\n";
    const BEHAVIOR_CARD_SCRIPT: &str = "Component({\n  behaviors: [Behavior({\n    properties: {\n      cardTitle: { type: String },\n      score: { type: String, optionalTypes: [Number], required: true, value: 1, observer() {} },\n      loose: { type: null },\n      items: { type: Array },\n      defaulted: { type: Number, value: \"not-a-number\" },\n    },\n  })],\n});\n";
    const CARD_SCRIPT: &str = "Component({\n  properties: {\n    title: { type: String },\n    count: { type: Number },\n  },\n});\n";
    const MATRIX_CARD_SCRIPT: &str = "Component({\n  properties: {\n    title: { type: String },\n    count: { type: Number },\n    active: { type: Boolean },\n    score: { type: String, optionalTypes: [Number] },\n    loose: { type: null },\n    items: { type: Array },\n    meta: { type: Object },\n    handler: { type: Function },\n  },\n});\n";
    const PLAIN_COMPONENT_SCRIPT: &str = "const shared = {};\nComponent({ properties: shared });\n";
    const SPREAD_COMPONENT_SCRIPT: &str = "const base = { title: { type: String } };\nComponent({ properties: { ...base, count: { type: Number } } });\n";
    const ALIAS_CARD_SCRIPT: &str = "import platform from \"@/behaviors/platform\";\nComponent({\n  behaviors: [platform],\n  properties: {\n    title: { type: String },\n  },\n});\n";
    const ALIAS_BEHAVIOR_SCRIPT: &str =
        "export default Behavior({\n  properties: {\n    score: { type: Number },\n  },\n});\n";

    fn source_range(start: usize, end: usize) -> SourceRange {
        SourceRange::from_bounds(
            u32::try_from(start).expect("start"),
            u32::try_from(end).expect("end"),
        )
        .expect("range")
    }

    fn project() -> ProjectId {
        ProjectId::new("demo")
    }

    fn file_id(path: &str) -> FileId {
        FileId::new(project(), path).expect("file id")
    }

    fn prepared_file(path: &str, text: &str) -> PreparedFile {
        PreparedFile {
            file: file_id(path),
            path: ProjectPath::new(path).expect("project path"),
            kind: "ts",
            text: Arc::from(text),
            source_revision: 1,
        }
    }

    /// Builds one declaration file the way `collect_sources` reports it.
    fn prepared_declaration(path: &str, text: &str) -> PreparedFile {
        PreparedFile {
            file: file_id(path),
            path: ProjectPath::new(path).expect("project path"),
            kind: "d.ts",
            text: Arc::from(text),
            source_revision: 1,
        }
    }

    /// Builds one proven declared property from its type text.
    fn typing_property(name: &str, type_text: &str) -> TypingProperty {
        let ty = TypeText::parse(type_text);
        TypingProperty {
            name: name.to_owned(),
            documentation: None,
            type_text: type_text.to_owned(),
            optional: false,
            proven: ty.is_proven(),
            ty,
        }
    }

    /// Builds one declared property whose type the subset cannot prove.
    fn unproven_property(name: &str, type_text: &str) -> TypingProperty {
        TypingProperty {
            name: name.to_owned(),
            documentation: None,
            type_text: type_text.to_owned(),
            optional: false,
            proven: false,
            ty: TypeText::Unproven,
        }
    }

    /// Returns the byte range of one attribute name.
    fn name_range(attribute: &ComponentAttribute) -> SourceRange {
        let start = usize::try_from(attribute.attribute_range.start()).expect("start");
        source_range(start, start + attribute.name.len())
    }

    fn build_view(files: Vec<PreparedFile>, components: Vec<ComponentUsage>) -> PreparedView {
        PreparedView {
            view_id: "pages/index/index.wxml:1".to_owned(),
            target: Some(ViewTarget {
                wxml: ProjectPath::new("pages/index/index.wxml").expect("wxml path"),
                owner: RootKind::Page,
                script: ProjectPath::new(SCRIPT_PATH).expect("script path"),
                script_file: file_id(SCRIPT_PATH),
                view_revision: ViewRevision::new(1),
            }),
            files: Arc::from(files),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(Vec::new()),
            wxs_modules: Arc::from(Vec::new()),
            components: Arc::from(components),
            typing: None,
            events: Arc::from(Vec::new()),
            project_root: PathBuf::from("/work/demo"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        }
    }

    /// 与生产 `PreparedView::compiler_options` 一致的 tsconfig `paths` 别名。
    fn alias_options() -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "paths": { "@/*": ["*"] },
        })
        .as_object()
        .cloned()
        .expect("compiler options object")
    }

    /// Attaches declaration facts parsed from one declaration text.
    fn typing_facts(declaration: &str) -> Arc<ComponentTypingFacts> {
        let source = wxml_project::SourceFile {
            path: ProjectPath::new(DECLARATION_PATH).expect("declaration path"),
            kind: wxml_project::SourceKind::Dts,
            text: declaration.to_owned(),
            source_revision: 1,
        };
        Arc::new(wxml_project::parse_component_typing_facts(
            &wxml_project::SourceSet {
                files: vec![source],
                truncated: false,
                issues: Vec::new(),
            },
        ))
    }

    /// Builds one view carrying declaration facts.
    fn typed_view(
        files: Vec<PreparedFile>,
        components: Vec<ComponentUsage>,
        declaration: &str,
    ) -> PreparedView {
        let mut view = build_view(files, components);
        view.typing = Some(typing_facts(declaration));
        view
    }

    /// Builds one view over the shared declared `card` fixture.
    fn declared_view(components: Vec<ComponentUsage>) -> PreparedView {
        typed_view(declared_files(), components, DECLARATION)
    }

    fn usage(
        tag: &str,
        component: Option<&str>,
        attributes: Vec<ComponentAttribute>,
    ) -> ComponentUsage {
        ComponentUsage {
            tag: tag.to_owned(),
            component_file: component.map(file_id),
            attributes,
        }
    }

    fn mustache_attr(wxml: &str, name: &str, source: &str, expression: &str) -> ComponentAttribute {
        let start = wxml.find(source).expect("attribute text");
        let expression_start = start + source.find(expression).expect("expression text");
        ComponentAttribute {
            name: name.to_owned(),
            value_kind: AttributeValueKind::Mustache,
            expr_text: Some(expression.to_owned()),
            expr_range: Some(source_range(
                expression_start,
                expression_start + expression.len(),
            )),
            attribute_range: source_range(start, start + source.len()),
        }
    }

    fn literal_attr(wxml: &str, name: &str, source: &str) -> ComponentAttribute {
        let start = wxml.find(source).expect("attribute text");
        ComponentAttribute {
            name: name.to_owned(),
            value_kind: AttributeValueKind::Literal,
            expr_text: None,
            expr_range: None,
            attribute_range: source_range(start, start + source.len()),
        }
    }

    fn boolean_attr(wxml: &str, name: &str, source: &str) -> ComponentAttribute {
        let start = wxml.find(source).expect("attribute text");
        ComponentAttribute {
            name: name.to_owned(),
            value_kind: AttributeValueKind::Boolean,
            expr_text: None,
            expr_range: None,
            attribute_range: source_range(start, start + source.len()),
        }
    }

    fn mixed_attr(wxml: &str, name: &str, source: &str) -> ComponentAttribute {
        let start = wxml.find(source).expect("attribute text");
        ComponentAttribute {
            name: name.to_owned(),
            value_kind: AttributeValueKind::Mixed,
            expr_text: None,
            expr_range: None,
            attribute_range: source_range(start, start + source.len()),
        }
    }

    /// Builds the layout this pass reads: the original template and a shadow
    /// without mounts, because no expression type is queried any more.
    fn shadow(wxml: &str) -> ShadowLayout {
        let derived = "let __wxml_root = {};\n";
        let assembled = Assembled {
            text: derived.to_owned(),
            edits: Vec::new(),
        };
        ShadowLayout::new(
            wxml.to_owned(),
            derived.to_owned(),
            ShadowMap::new(derived, &assembled),
            Vec::new(),
        )
    }

    fn run(layout: &ShadowLayout, view: &PreparedView) -> ComponentBindingDiagnostics {
        diagnostics(layout, view)
    }

    fn page_and_card() -> Vec<PreparedFile> {
        vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, CARD_SCRIPT),
        ]
    }

    fn assert_partial(result: &ComponentBindingDiagnostics) {
        let Some(CapabilityStatus::Partial { reason }) = result.status.as_ref() else {
            panic!("expected Partial, got {:?}", result.status);
        };
        assert!(reason.starts_with(SKIPPED_PREFIX), "{reason}");
        assert!(
            !reason.contains('/'),
            "the reason must stay path-free: {reason}"
        );
    }

    /// One attribute in the static-value matrix.
    #[derive(Clone, Copy)]
    enum MatrixValue {
        Mustache(&'static str),
        Literal(&'static str),
        Boolean,
        Mixed(&'static str),
    }

    #[derive(Clone, Copy)]
    struct MatrixAttribute {
        name: &'static str,
        value: MatrixValue,
    }

    struct MatrixUsage {
        tag: &'static str,
        component: Option<&'static str>,
        attributes: Vec<MatrixAttribute>,
    }

    struct MatrixFixture {
        wxml: String,
        usages: Vec<ComponentUsage>,
    }

    fn matrix_attribute(name: &'static str, value: MatrixValue) -> MatrixAttribute {
        MatrixAttribute { name, value }
    }

    fn matrix_specs() -> Vec<MatrixUsage> {
        vec![
            MatrixUsage {
                tag: "valid-literal",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("title", MatrixValue::Literal("hello"))],
            },
            MatrixUsage {
                tag: "valid-boolean",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("active", MatrixValue::Boolean)],
            },
            MatrixUsage {
                tag: "bad-literal",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("count", MatrixValue::Literal("3"))],
            },
            MatrixUsage {
                tag: "bad-boolean",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("count", MatrixValue::Boolean)],
            },
            MatrixUsage {
                tag: "array-prop",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("items", MatrixValue::Literal("x"))],
            },
            MatrixUsage {
                tag: "object-prop",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("meta", MatrixValue::Literal("x"))],
            },
            MatrixUsage {
                tag: "union-literal-ok",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("score", MatrixValue::Literal("7"))],
            },
            MatrixUsage {
                tag: "union-boolean-bad",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("score", MatrixValue::Boolean)],
            },
            MatrixUsage {
                tag: "undeclared",
                component: Some(CARD_PATH),
                attributes: vec![
                    matrix_attribute("title", MatrixValue::Literal("hello")),
                    matrix_attribute("class", MatrixValue::Literal("x")),
                    matrix_attribute("unknown", MatrixValue::Literal("x")),
                ],
            },
            MatrixUsage {
                tag: "any-prop",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("loose", MatrixValue::Literal("x"))],
            },
            MatrixUsage {
                tag: "function-prop",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("handler", MatrixValue::Literal("x"))],
            },
            MatrixUsage {
                tag: "mixed",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mixed("Hi {{ total }}"),
                )],
            },
            // A pure mustache is the engine probe's job: this pass stays silent.
            MatrixUsage {
                tag: "mustache",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "missing-component",
                component: Some("missing.ts"),
                attributes: vec![matrix_attribute("title", MatrixValue::Literal("hello"))],
            },
            MatrixUsage {
                tag: "no-properties",
                component: Some(PLAIN_PATH),
                attributes: vec![matrix_attribute("title", MatrixValue::Literal("hello"))],
            },
            MatrixUsage {
                tag: "incomplete-properties",
                component: Some(SPREAD_PATH),
                attributes: vec![matrix_attribute("title", MatrixValue::Literal("hello"))],
            },
            MatrixUsage {
                tag: "no-file-id",
                component: None,
                attributes: vec![matrix_attribute("title", MatrixValue::Literal("hello"))],
            },
        ]
    }

    /// Builds the WXML text and usages for a spec set.
    ///
    /// The WXML text is generated together with the byte ranges so every
    /// `ComponentAttribute` is a real slice of the template, exactly as the
    /// template analyzer would publish it.
    fn matrix_fixture() -> MatrixFixture {
        let mut wxml = String::from("<view>");
        let mut usages = Vec::new();
        for spec in matrix_specs() {
            wxml.push('<');
            wxml.push_str(spec.tag);
            let mut attributes = Vec::new();
            for attribute in spec.attributes {
                wxml.push(' ');
                let attribute_start = wxml.len();
                wxml.push_str(attribute.name);
                let (value_kind, expr_text, expr_range) = match attribute.value {
                    MatrixValue::Mustache(expr) => {
                        wxml.push_str("=\"{{ ");
                        let start = wxml.len();
                        wxml.push_str(expr);
                        let end = wxml.len();
                        wxml.push_str(" }}\"");
                        let range = source_range(start, end);
                        (
                            AttributeValueKind::Mustache,
                            Some(expr.to_owned()),
                            Some(range),
                        )
                    }
                    MatrixValue::Literal(value) => {
                        wxml.push_str("=\"");
                        wxml.push_str(value);
                        wxml.push('"');
                        (AttributeValueKind::Literal, None, None)
                    }
                    MatrixValue::Boolean => (AttributeValueKind::Boolean, None, None),
                    MatrixValue::Mixed(text) => {
                        wxml.push_str("=\"");
                        wxml.push_str(text);
                        wxml.push('"');
                        (AttributeValueKind::Mixed, None, None)
                    }
                };
                attributes.push(ComponentAttribute {
                    name: attribute.name.to_owned(),
                    value_kind,
                    expr_text,
                    expr_range,
                    attribute_range: source_range(attribute_start, wxml.len()),
                });
            }
            wxml.push_str(" />");
            usages.push(usage(spec.tag, spec.component, attributes));
        }
        wxml.push_str("</view>");
        MatrixFixture { wxml, usages }
    }

    fn matrix_files() -> Vec<PreparedFile> {
        vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, MATRIX_CARD_SCRIPT),
            prepared_file(PLAIN_PATH, PLAIN_COMPONENT_SCRIPT),
            prepared_file(SPREAD_PATH, SPREAD_COMPONENT_SCRIPT),
        ]
    }

    fn run_matrix(fixture: &MatrixFixture) -> ComponentBindingDiagnostics {
        let layout = shadow(&fixture.wxml);
        let view = build_view(matrix_files(), fixture.usages.clone());
        run(&layout, &view)
    }

    /// Runs one usage of `fixture` against an explicit file set.
    fn run_usage_with_files(
        fixture: &MatrixFixture,
        files: Vec<PreparedFile>,
        tag: &str,
    ) -> ComponentBindingDiagnostics {
        let usage = fixture
            .usages
            .iter()
            .find(|usage| usage.tag == tag)
            .expect("matrix usage")
            .clone();
        let layout = shadow(&fixture.wxml);
        let view = build_view(files, vec![usage]);
        run(&layout, &view)
    }

    fn matrix_attribute_of<'a>(
        fixture: &'a MatrixFixture,
        tag: &str,
        name: &str,
    ) -> &'a ComponentAttribute {
        fixture
            .usages
            .iter()
            .find(|usage| usage.tag == tag)
            .and_then(|usage| {
                usage
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name == name)
            })
            .expect("matrix attribute")
    }

    /// One declared property value pair in the static matrix.
    fn binding_error(result: &ComponentBindingDiagnostics, range: SourceRange) -> &Diagnostic {
        result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range == range)
            .unwrap_or_else(|| panic!("missing diagnostic at {range:?}: {:?}", result.diagnostics))
    }

    #[test]
    fn literal_and_boolean_values_use_the_attribute_range() {
        let wxml = r#"<view><card count="3" title disabled /></view>"#;
        let count = literal_attr(wxml, "count", r#"count="3""#);
        let title = boolean_attr(wxml, "title", "title");
        let disabled = boolean_attr(wxml, "disabled", "disabled");
        let layout = shadow(wxml);
        let view = build_view(
            page_and_card(),
            vec![usage(
                "card",
                Some(CARD_PATH),
                vec![count.clone(), title.clone(), disabled],
            )],
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
        let count_error = binding_error(&result, count.attribute_range);
        assert_eq!(
            count_error.message,
            "component property \"count\" expects number, got string literal"
        );
        let title_error = binding_error(&result, title.attribute_range);
        assert_eq!(
            title_error.message,
            "component property \"title\" expects string, got boolean literal"
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn mustache_bindings_are_not_checked_by_this_pass() {
        let wxml = r#"<view><card title="{{ total }}" count="{{ pageTitle }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let count = mustache_attr(wxml, "count", r#"count="{{ pageTitle }}""#, "pageTitle");
        let layout = shadow(wxml);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title, count])],
        );
        let result = run(&layout, &view);
        assert!(
            result.diagnostics.is_empty(),
            "a mustache mismatch belongs to the engine contract probe: {:?}",
            result.diagnostics
        );
        assert!(
            result.status.is_none(),
            "nothing was skipped, so the status must stay clean: {:?}",
            result.status
        );
    }

    #[test]
    fn mustache_on_an_unproven_script_property_stays_a_coverage_gap() {
        // `loose` is script `any`, which this pass cannot represent. The value
        // itself is delegated to the probe, but the fact gap is still a
        // coverage reason instead of a clean result.
        let wxml = r#"<view><card loose="{{ total }}" /></view>"#;
        let loose = mustache_attr(wxml, "loose", r#"loose="{{ total }}""#, "total");
        let layout = shadow(wxml);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, MATRIX_CARD_SCRIPT),
        ];
        let view = build_view(files, vec![usage("card", Some(CARD_PATH), vec![loose])]);
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial, got {:?}", result.status);
        };
        assert_eq!(reason, &format!("{SKIPPED_PREFIX} (card.loose)"));
    }

    #[test]
    fn aliased_behavior_facts_keep_literals_checkable() {
        let wxml = r#"<view><alias-card title="{{ total }}" score="7" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let score = literal_attr(wxml, "score", r#"score="7""#);
        let layout = shadow(wxml);
        let mut view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(ALIAS_CARD_PATH, ALIAS_CARD_SCRIPT),
                prepared_file(ALIAS_BEHAVIOR_PATH, ALIAS_BEHAVIOR_SCRIPT),
            ],
            vec![usage(
                "alias-card",
                Some(ALIAS_CARD_PATH),
                vec![title, score.clone()],
            )],
        );
        view.compiler_options = alias_options();
        let result = run(&layout, &view);
        // A clean status proves the aliased behavior resolved completely: an
        // unreadable behavior would add the behavior `Partial` reason.
        assert!(result.status.is_none(), "{:?}", result.status);
        let diagnostic = binding_error(&result, score.attribute_range);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(
            diagnostic.message,
            "component property \"score\" expects number, got string literal"
        );
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
    }

    /// 别名目标缺失时回退到组件自身声明，未声明属性不得报错。
    #[test]
    fn unresolved_aliased_behavior_falls_back_to_own_properties() {
        let wxml = r#"<view><alias-card title="{{ total }}" score="7" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let score = literal_attr(wxml, "score", r#"score="7""#);
        let layout = shadow(wxml);
        let mut view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(ALIAS_CARD_PATH, ALIAS_CARD_SCRIPT),
            ],
            vec![usage(
                "alias-card",
                Some(ALIAS_CARD_PATH),
                vec![title, score],
            )],
        );
        view.compiler_options = alias_options();
        let result = run(&layout, &view);
        // `score` may come from the unreadable behavior, so it is ignored.
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial, got {:?}", result.status);
        };
        assert_eq!(
            reason,
            "组件 behaviors 未能完全解析，仅校验了组件自身 properties 声明的属性（alias-card）"
        );
        assert!(!reason.contains('/'), "{reason}");
    }

    /// 配置了 `paths` 但没有 behavior 的组件仍保持完整校验，不产生 Partial。
    #[test]
    fn paths_options_without_behaviors_stay_complete() {
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml);
        let mut view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title])],
        );
        view.compiler_options = alias_options();
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn static_value_matrix_covers_literals_undeclared_and_degradation() {
        let fixture = matrix_fixture();
        let result = run_matrix(&fixture);
        assert!(result.diagnostics.len() == 5, "{:?}", result.diagnostics);

        let expected = [
            (
                "bad-literal",
                "count",
                "component property \"count\" expects number, got string literal",
            ),
            (
                "bad-boolean",
                "count",
                "component property \"count\" expects number, got boolean literal",
            ),
            (
                "array-prop",
                "items",
                "component property \"items\" expects any[], got string literal",
            ),
            (
                "object-prop",
                "meta",
                "component property \"meta\" expects Record<string, any>, got string literal",
            ),
            (
                "union-boolean-bad",
                "score",
                "component property \"score\" expects string | number, got boolean literal",
            ),
        ];
        for (tag, name, message) in expected {
            let attribute = matrix_attribute_of(&fixture, tag, name);
            let diagnostic = binding_error(&result, attribute.attribute_range);
            assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
            assert_eq!(diagnostic.message, message, "{tag}.{name}");
        }

        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial");
        };
        assert_eq!(
            reason,
            "some component bindings could not be resolved and were not checked (any-prop.loose, function-prop.handler, mixed.title)"
        );

        // Tags whose static values match are checked and clean.
        for tag in [
            "valid-literal",
            "valid-boolean",
            "union-literal-ok",
            "undeclared",
            "mustache",
        ] {
            let clean = run_usage_with_files(&fixture, matrix_files(), tag);
            assert!(
                clean.diagnostics.is_empty(),
                "{tag}: {:?}",
                clean.diagnostics
            );
            assert!(clean.status.is_none(), "{tag}: {:?}", clean.status);
        }

        // The degraded usages must not become clean results.
        let skipped = [
            ("any-prop", "any-prop.loose"),
            ("function-prop", "function-prop.handler"),
            ("mixed", "mixed.title"),
            ("missing-component", "missing-component"),
            ("no-properties", "no-properties"),
            ("incomplete-properties", "incomplete-properties"),
            ("no-file-id", "no-file-id"),
        ];
        for (tag, detail) in skipped {
            let skipped_result = run_usage_with_files(&fixture, matrix_files(), tag);
            assert!(
                skipped_result.diagnostics.is_empty(),
                "{tag}: {:?}",
                skipped_result.diagnostics
            );
            let CapabilityStatus::Partial { reason } =
                skipped_result.status.as_ref().expect("partial")
            else {
                panic!("{tag} must be Partial");
            };
            assert_eq!(reason, &format!("{SKIPPED_PREFIX} ({detail})"), "{tag}");
        }
    }

    #[test]
    fn component_diagnostics_never_leak_a_path() {
        let fixture = matrix_fixture();
        let result = run_matrix(&fixture);
        assert!(!result.diagnostics.is_empty());
        for diagnostic in &result.diagnostics {
            assert!(
                !diagnostic.message.contains('/'),
                "path-free message: {}",
                diagnostic.message
            );
        }
        if let Some(CapabilityStatus::Partial { reason }) = &result.status {
            assert!(!reason.contains('/'), "{reason}");
        }
    }

    #[test]
    fn literal_binding_without_any_mustache_is_checked() {
        let wxml = r#"<view><badge count="oops" /></view>"#;
        assert!(!wxml.contains("{{"));
        let count = literal_attr(wxml, "count", r#"count="oops""#);
        let start = usize::try_from(count.attribute_range.start()).expect("start");
        let end = usize::try_from(count.attribute_range.end()).expect("end");
        assert_eq!(
            wxml.get(start..end),
            Some(r#"count="oops""#),
            "the diagnostic range must be the attribute slice"
        );
        let layout = shadow(wxml);
        let view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(
                    BADGE_PATH,
                    "Component({ properties: { count: { type: Number } } });\n",
                ),
            ],
            vec![usage("badge", Some(BADGE_PATH), vec![count.clone()])],
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, count.attribute_range);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(
            diagnostic.message,
            "component property \"count\" expects number, got string literal"
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn component_without_properties_member_is_skipped_not_guessed() {
        let wxml = r#"<view><bare title="hello" /></view>"#;
        let title = literal_attr(wxml, "title", r#"title="hello""#);
        let layout = shadow(wxml);
        let view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(BARE_PATH, "Component({ data: { state: 'idle' } });\n"),
            ],
            vec![usage("bare", Some(BARE_PATH), vec![title])],
        );
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial");
        };
        assert_eq!(reason, &format!("{SKIPPED_PREFIX} (bare)"));
        assert!(!reason.contains('/'), "{reason}");
    }

    #[test]
    fn undeclared_and_event_attributes_are_ignored() {
        let wxml = r#"<view><card class="x" style="y" bind:tap="onTap" bindtap="onTap" /></view>"#;
        let attributes = vec![
            literal_attr(wxml, "class", r#"class="x""#),
            literal_attr(wxml, "style", r#"style="y""#),
            literal_attr(wxml, "bind:tap", r#"bind:tap="onTap""#),
            literal_attr(wxml, "bindtap", r#"bindtap="onTap""#),
        ];
        let layout = shadow(wxml);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), attributes)],
        );
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn mixed_binding_is_partial_not_an_error() {
        let wxml = r#"<view><card title="Hi {{ total }}" /></view>"#;
        let title = mixed_attr(wxml, "title", r#"title="Hi {{ total }}""#);
        let layout = shadow(wxml);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title])],
        );
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);
    }

    #[test]
    fn missing_component_or_incomplete_properties_is_partial() {
        let wxml = r#"<view><card title="hello" /></view>"#;
        let title = literal_attr(wxml, "title", r#"title="hello""#);
        let layout = shadow(wxml);

        let missing = build_view(
            page_and_card(),
            vec![usage("card", None, vec![title.clone()])],
        );
        let result = run(&layout, &missing);
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);

        let spread = "Component({ properties: { ...base, title: { type: String } } });\n";
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, spread),
        ];
        let view = build_view(
            files,
            vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
        );
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);

        let nonliteral =
            "const options = { properties: { title: { type: String } } };\nComponent(options);\n";
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, nonliteral),
        ];
        let view = build_view(files, vec![usage("card", Some(CARD_PATH), vec![title])]);
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);
    }

    #[test]
    fn two_components_with_the_same_property_keep_their_own_types() {
        let badge_script = "Component({ properties: { title: { type: Number } } });\n";
        let wxml = r#"<view><card title="x" /><badge title="x" /></view>"#;
        let card = literal_attr(wxml, "title", r#"<card title="x" />"#);
        let badge = literal_attr(wxml, "title", r#"<badge title="x" />"#);
        let layout = shadow(wxml);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, CARD_SCRIPT),
            prepared_file(BADGE_PATH, badge_script),
        ];
        let view = build_view(
            files,
            vec![
                usage("card", Some(CARD_PATH), vec![card]),
                usage("badge", Some(BADGE_PATH), vec![badge.clone()]),
            ],
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, badge.attribute_range);
        assert_eq!(
            result.diagnostics[0].message,
            "component property \"title\" expects number, got string literal"
        );
    }

    #[test]
    fn cjk_crlf_attribute_range_is_exact() {
        let wxml = "<view>标题😀\r\n<card count=\"3\" /></view>";
        let count = literal_attr(wxml, "count", r#"count="3""#);
        let range = count.attribute_range;
        assert_eq!(
            wxml.get(range.start() as usize..range.end() as usize),
            Some(r#"count="3""#)
        );
        let layout = shadow(wxml);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![count])],
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, range);
    }

    #[test]
    fn caps_and_dedup() {
        let mut wxml = String::from("<view>");
        let mut usages = Vec::new();
        for _ in 0..10 {
            let attribute_start = wxml.len() + "<card ".len();
            wxml.push_str(r#"<card count="3" />"#);
            usages.push(usage(
                "card",
                Some(CARD_PATH),
                vec![ComponentAttribute {
                    name: "count".to_owned(),
                    value_kind: AttributeValueKind::Literal,
                    expr_text: None,
                    expr_range: None,
                    attribute_range: source_range(
                        attribute_start,
                        attribute_start + r#"count="3""#.len(),
                    ),
                }],
            ));
        }
        wxml.push_str("</view>");
        let layout = shadow(&wxml);
        let view = build_view(page_and_card(), usages);
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), MAX_DIAGNOSTICS_PER_TAG);
        assert_partial(&result);

        let wxml = r#"<view><card count="3" /></view>"#;
        let attribute = literal_attr(wxml, "count", r#"count="3""#);
        let layout = shadow(wxml);
        let view = build_view(
            page_and_card(),
            vec![usage(
                "card",
                Some(CARD_PATH),
                vec![attribute.clone(), attribute],
            )],
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    /// End-to-end pass over the real `component-bindings` fixture: the real
    /// host prepares `ComponentUsage`/fragments, and every binding is a pure
    /// mustache, so this pass publishes nothing and leaves it to the probe.
    #[test]
    fn fixture_component_usages_publish_no_property_error() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/component-bindings")
            .canonicalize()
            .expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open fixture root");
        let path = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        let text = std::fs::read_to_string(root.join("pages/index/index.wxml")).expect("wxml text");
        let file = host.file_id(&path).expect("wxml file id");
        let _target = host.view_for(&path).expect("view target");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            text.clone(),
        ));
        let view = host.analysis(&snapshot).expect("prepared view");
        assert_eq!(view.components.len(), 2);
        let attributes: Vec<Vec<&str>> = view
            .components
            .iter()
            .map(|usage| {
                usage
                    .attributes
                    .iter()
                    .map(|item| item.name.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(attributes, vec![vec!["title", "count"], vec!["title"]]);
        assert!(
            view.components
                .iter()
                .all(|usage| usage.component_file.is_some())
        );

        let layout = shadow(&text);
        let result = run(&layout, &view);
        assert!(
            result.diagnostics.is_empty(),
            "pure mustache bindings are engine probe work: {:?}",
            result.diagnostics
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    // -----------------------------------------------------------------------
    // Declared component properties (`components.d.ts`)
    // -----------------------------------------------------------------------

    /// Declaration used by the unit tests below.
    const DECLARATION: &str = "declare global {\n  interface WxmlComponentMap {\n    card: CardDeclaration;\n  }\n\n  interface CardDeclaration {\n    props: {\n      type?: 'primary' | 'default' | 'warning';\n      size?: 1 | 2;\n      loading?: boolean;\n      text: string;\n      plain: string | undefined;\n      loose: any;\n      ref: SomeLib.Options;\n    };\n  }\n}\n\nexport {};\n";

    /// A script whose `properties` deliberately disagrees with [`DECLARATION`].
    const DECLARED_CARD_SCRIPT: &str =
        "Component({\n  properties: {\n    type: { type: Boolean },\n  },\n});\n";

    fn declared_files() -> Vec<PreparedFile> {
        vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, DECLARED_CARD_SCRIPT),
            prepared_declaration(DECLARATION_PATH, DECLARATION),
        ]
    }

    /// Writes a temporary project with a page, one local card component and
    /// optional canonical/nested declaration files.
    fn declaration_project(
        canonical: Option<&str>,
        nested: Option<&str>,
    ) -> (tempfile::TempDir, PathBuf) {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let dir = tempfile::Builder::new()
            .prefix("wxml-bindings-declaration-")
            .tempdir_in(base)
            .expect("create temp dir");
        let root = dir.path().to_path_buf();
        std::fs::write(
            root.join("app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{"card":"/components/card/card"}}"#,
        )
        .expect("write app.json");
        std::fs::write(root.join("tsconfig.json"), "{}").expect("write tsconfig");
        if let Some(canonical) = canonical {
            std::fs::write(root.join("components.d.ts"), canonical).expect("canonical declaration");
        }
        if let Some(nested) = nested {
            std::fs::create_dir_all(root.join("sub")).expect("nested dir");
            std::fs::write(root.join("sub/components.d.ts"), nested).expect("nested declaration");
        }
        std::fs::create_dir_all(root.join("components/card")).expect("component dir");
        std::fs::write(root.join("components/card/card.wxml"), "<view />").expect("component wxml");
        std::fs::write(root.join("components/card/card.json"), "{}").expect("component json");
        std::fs::write(
            root.join("components/card/card.ts"),
            "Component({ properties: { type: { type: String } } });\n",
        )
        .expect("component script");
        std::fs::create_dir_all(root.join("pages/index")).expect("page dir");
        std::fs::write(
            root.join("pages/index/index.wxml"),
            r#"<card type="primary" size="small" />"#,
        )
        .expect("page wxml");
        std::fs::write(root.join("pages/index/index.json"), "{}").expect("page json");
        std::fs::write(root.join("pages/index/index.ts"), "Page({ data: {} });\n")
            .expect("page script");
        (dir, root)
    }

    /// Opens a temporary project and prepares its index page view.
    fn analyze_declaration_project(root: &std::path::Path) -> (PreparedView, String) {
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open project root");
        let path = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        let text = std::fs::read_to_string(root.join("pages/index/index.wxml")).expect("wxml text");
        let file = host.file_id(&path).expect("wxml file id");
        let _target = host.view_for(&path).expect("view target");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            text.clone(),
        ));
        let view = host.analysis(&snapshot).expect("prepared view");
        (view, text)
    }

    #[test]
    fn nested_declaration_file_does_not_participate_in_binding_checks() {
        const CANONICAL: &str = "declare global {\n  interface WxmlComponentMap {\n    card: CardProps;\n  }\n  interface CardProps {\n    props: {\n      type?: 'primary' | 'default';\n    };\n  }\n}\nexport {};\n";
        // A nested declaration that would both add `size` and keep the tag
        // resolvable. It must never reach the binding pass.
        const NESTED: &str = "declare global {\n  interface WxmlComponentMap {\n    card: CardProps;\n  }\n  interface CardProps {\n    props: {\n      type?: 'primary' | 'default';\n      size?: string;\n    };\n  }\n}\nexport {};\n";
        let (_dir, root) = declaration_project(Some(CANONICAL), Some(NESTED));
        let (view, text) = analyze_declaration_project(&root);

        let facts = view
            .typing
            .as_ref()
            .expect("the canonical declaration must supply facts");
        let card = facts.component("card").expect("declared tag");
        assert!(card.prop("type").is_some());
        assert!(
            card.prop("size").is_none(),
            "the nested declaration must not add members"
        );

        let layout = shadow(&text);
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let size_start = text.find(r#"size="small""#).expect("size attribute");
        assert_eq!(
            result.diagnostics[0].range,
            source_range(size_start, size_start + "size".len())
        );
        assert!(
            result.diagnostics[0].message.contains("not declared"),
            "{}",
            result.diagnostics[0].message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn without_a_canonical_declaration_other_declaration_files_are_ignored() {
        const NESTED: &str = "declare global {\n  interface WxmlComponentMap {\n    card: CardProps;\n  }\n  interface CardProps {\n    props: {\n      type?: 'primary' | 'default';\n      size?: string;\n    };\n  }\n}\nexport {};\n";
        let (_dir, root) = declaration_project(None, Some(NESTED));
        let (view, text) = analyze_declaration_project(&root);
        assert!(
            view.typing.is_none(),
            "only the tsconfig-sibling file may supply facts"
        );

        let layout = shadow(&text);
        let result = run(&layout, &view);
        // Script fallback: `type` matches `String` and an undeclared `size` is
        // ignored exactly as before declarations existed.
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_literal_contract_maps_each_type_expression() {
        let cases = [
            ("string", LiteralContract::Shapes(vec![LiteralKind::String])),
            ("number", LiteralContract::Shapes(vec![LiteralKind::Number])),
            (
                "boolean",
                LiteralContract::Shapes(vec![LiteralKind::Boolean]),
            ),
            (
                "string[]",
                LiteralContract::Shapes(vec![LiteralKind::Array]),
            ),
            (
                "Record<string, string>",
                LiteralContract::Shapes(vec![LiteralKind::Object]),
            ),
            (
                "{ a: string }",
                LiteralContract::Shapes(vec![LiteralKind::Object]),
            ),
            ("any", LiteralContract::Any),
            ("unknown", LiteralContract::Unknown),
            (
                "'primary' | 'default'",
                LiteralContract::StringLiterals(vec!["primary".to_owned(), "default".to_owned()]),
            ),
            ("1 | 2", LiteralContract::NumberLiterals),
            // Nullish alternatives only widen optionality.
            (
                "boolean | undefined",
                LiteralContract::Shapes(vec![LiteralKind::Boolean]),
            ),
            (
                "string | 'a'",
                LiteralContract::Shapes(vec![LiteralKind::String]),
            ),
            // Mixed primitive kinds have no single literal contract.
            ("string | number", LiteralContract::Unknown),
            ("'a' | 1", LiteralContract::Unknown),
        ];
        for (text, expected) in cases {
            let property = typing_property("value", text);
            assert_eq!(
                LiteralContract::from_property(&property),
                expected,
                "type = {text:?}"
            );
        }

        // An unproven member is always `Unknown`, even when the text parses.
        let unproven = unproven_property("value", "'a'");
        assert_eq!(
            LiteralContract::from_property(&unproven),
            LiteralContract::Unknown
        );

        // The script text whitelist treats `any`/`Function` as unrepresented.
        assert_eq!(
            LiteralContract::from_script_text("number"),
            Some(LiteralContract::Shapes(vec![LiteralKind::Number]))
        );
        assert_eq!(
            LiteralContract::from_script_text("string | number"),
            Some(LiteralContract::Shapes(vec![
                LiteralKind::String,
                LiteralKind::Number
            ]))
        );
        assert_eq!(LiteralContract::from_script_text("any"), None);
        assert_eq!(
            LiteralContract::from_script_text("(...args: any[]) => any"),
            None
        );
    }

    #[test]
    fn declared_literal_contract_judges_only_static_values() {
        let string = LiteralContract::Shapes(vec![LiteralKind::String]);
        assert_eq!(string.judge_literal(LiteralValue::String("x")), Some(true));
        assert_eq!(string.judge_literal(LiteralValue::Boolean), Some(false));
        assert_eq!(
            LiteralContract::NumberLiterals.judge_literal(LiteralValue::String("1")),
            Some(false)
        );
        assert_eq!(
            LiteralContract::Unknown.judge_literal(LiteralValue::String("x")),
            None
        );
    }

    #[test]
    fn declared_props_replace_script_properties_and_keep_reserved_attributes() {
        let wxml = r#"<view><card type="medium" class="x" data-id="1" aria-label="l" /></view>"#;
        let type_attribute = literal_attr(wxml, "type", r#"type="medium""#);
        let class_attribute = literal_attr(wxml, "class", r#"class="x""#);
        let data_attribute = literal_attr(wxml, "data-id", r#"data-id="1""#);
        let aria_attribute = literal_attr(wxml, "aria-label", r#"aria-label="l""#);
        let layout = shadow(wxml);
        let view = declared_view(vec![usage(
            "card",
            Some(CARD_PATH),
            vec![
                type_attribute.clone(),
                class_attribute,
                data_attribute,
                aria_attribute,
            ],
        )]);
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, type_attribute.attribute_range);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(
            diagnostic.message,
            "component property \"type\" expects 'primary' | 'default' | 'warning', got string literal"
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_component_reports_undeclared_attributes_on_the_name() {
        let wxml = r#"<view><card round="{{ total }}" /></view>"#;
        let round = mustache_attr(wxml, "round", r#"round="{{ total }}""#, "total");
        let layout = shadow(wxml);
        let view = declared_view(
            // The component script is absent on purpose: declarations alone are
            // enough to check attribute names.
            vec![usage("card", None, vec![round.clone()])],
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, name_range(&round));
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(
            diagnostic.message,
            "component property \"round\" is not declared by the component declaration"
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_number_literals_reject_the_wxml_string_literal() {
        // A WXML literal attribute is a string and cannot satisfy `1 | 2`.
        let wxml = r#"<view><card size="1" /></view>"#;
        let literal = literal_attr(wxml, "size", r#"size="1""#);
        let layout = shadow(wxml);
        let view = declared_view(vec![usage("card", None, vec![literal.clone()])]);
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, literal.attribute_range);
        assert_eq!(
            diagnostic.message,
            "component property \"size\" expects 1 | 2, got string literal"
        );
    }

    #[test]
    fn declared_optional_members_accept_matching_literals() {
        let wxml = r#"<view><card type="primary" loading /></view>"#;
        let type_attribute = literal_attr(wxml, "type", r#"type="primary""#);
        let loading = boolean_attr(wxml, "loading", "loading");
        let layout = shadow(wxml);
        let view = declared_view(vec![usage("card", None, vec![type_attribute, loading])]);
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_unproven_member_is_skipped_while_others_are_checked() {
        let wxml = r#"<view><card ref="x" text="hello" /></view>"#;
        let reference = literal_attr(wxml, "ref", r#"ref="x""#);
        let text = literal_attr(wxml, "text", r#"text="hello""#);
        let layout = shadow(wxml);
        let view = declared_view(vec![usage("card", None, vec![reference, text])]);
        let result = run(&layout, &view);
        // The unproven `ref` member is a coverage gap; `text` still matches.
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial, got {:?}", result.status);
        };
        assert!(reason.contains("card.ref"), "{reason}");
    }

    #[test]
    fn declared_any_member_accepts_every_literal() {
        let wxml = r#"<view><card loose="anything" /></view>"#;
        let loose = literal_attr(wxml, "loose", r#"loose="anything""#);
        let layout = shadow(wxml);
        let view = declared_view(vec![usage("card", None, vec![loose])]);
        let result = run(&layout, &view);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_members_use_the_camel_kebab_mapping() {
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    item: ItemDeclaration;\n  }\n\n  interface ItemDeclaration {\n    props: {\n      cardTitle?: 'small' | 'large';\n    };\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><item card-title="medium" /></view>"#;
        let card_title = literal_attr(wxml, "card-title", r#"card-title="medium""#);
        let layout = shadow(wxml);
        let view = typed_view(
            vec![prepared_declaration(DECLARATION_PATH, declaration)],
            vec![usage("item", None, vec![card_title.clone()])],
            declaration,
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, card_title.attribute_range);
        assert_eq!(
            result.diagnostics[0].message,
            "component property \"card-title\" expects 'small' | 'large', got string literal"
        );
    }

    #[test]
    fn declared_kebab_members_accept_the_camel_spelling() {
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    item: ItemDeclaration;\n  }\n\n  interface ItemDeclaration {\n    props: {\n      'card-title'?: 'small' | 'large';\n    };\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><item cardTitle="medium" /></view>"#;
        let card_title = literal_attr(wxml, "cardTitle", r#"cardTitle="medium""#);
        let layout = shadow(wxml);
        let view = typed_view(
            vec![prepared_declaration(DECLARATION_PATH, declaration)],
            vec![usage("item", None, vec![card_title.clone()])],
            declaration,
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, card_title.attribute_range);
        assert_eq!(
            result.diagnostics[0].message,
            "component property \"cardTitle\" expects 'small' | 'large', got string literal"
        );
    }

    #[test]
    fn declared_members_keep_case_only_variants_unresolved() {
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    item: ItemDeclaration;\n  }\n\n  interface ItemDeclaration {\n    props: {\n      someProp?: 'small' | 'large';\n    };\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><item someprop="medium" /></view>"#;
        let written = literal_attr(wxml, "someprop", r#"someprop="medium""#);
        let layout = shadow(wxml);
        let view = typed_view(
            vec![prepared_declaration(DECLARATION_PATH, declaration)],
            vec![usage("item", None, vec![written.clone()])],
            declaration,
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, name_range(&written));
        assert_eq!(
            diagnostic.message,
            "component property \"someprop\" is not declared by the component declaration"
        );
    }

    #[test]
    fn script_declared_camel_members_accept_the_kebab_spelling() {
        const KEBAB_DECLARATION_SCRIPT: &str =
            "Component({\n  properties: {\n    cardTitle: { type: String },\n  },\n});\n";
        let wxml = "<view><card card-title /></view>";
        let card_title = boolean_attr(wxml, "card-title", "card-title");
        let layout = shadow(wxml);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, KEBAB_DECLARATION_SCRIPT),
        ];
        let view = build_view(
            files,
            vec![usage("card", Some(CARD_PATH), vec![card_title.clone()])],
        );
        let result = run(&layout, &view);
        // The camel declaration is found through the kebab spelling, so the
        // boolean value is judged against `string`.
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, card_title.attribute_range);
        assert_eq!(
            result.diagnostics[0].message,
            "component property \"card-title\" expects string, got boolean literal"
        );
    }

    #[test]
    fn script_declared_members_stay_resolved_only_for_equivalent_spelling() {
        const KEBAB_DECLARATION_SCRIPT: &str =
            "Component({\n  properties: {\n    someProp: { type: String },\n  },\n});\n";
        let wxml = "<view><card someprop /></view>";
        let written = boolean_attr(wxml, "someprop", "someprop");
        let layout = shadow(wxml);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, KEBAB_DECLARATION_SCRIPT),
        ];
        let view = build_view(files, vec![usage("card", Some(CARD_PATH), vec![written])]);
        let result = run(&layout, &view);
        assert!(
            result.diagnostics.is_empty(),
            "undeclared script attributes stay ignored: {:?}",
            result.diagnostics
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_components_with_a_structurally_incomplete_map_fall_back_to_scripts() {
        // The mapped interface is missing, so the tag cannot be declared and
        // the script facts must keep driving the check.
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    card: MissingDeclaration;\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><card count="3" /></view>"#;
        let count = literal_attr(wxml, "count", r#"count="3""#);
        let layout = shadow(wxml);
        let view = typed_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(CARD_PATH, CARD_SCRIPT),
                prepared_declaration(DECLARATION_PATH, declaration),
            ],
            vec![usage("card", Some(CARD_PATH), vec![count.clone()])],
            declaration,
        );
        let result = run(&layout, &view);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, count.attribute_range);
        assert_eq!(
            result.diagnostics[0].message,
            "component property \"count\" expects number, got string literal"
        );
    }

    /// Oracle: TS-COMPONENT-0292, literal form.
    ///
    /// The component declares its properties inside an inline `Behavior`, so
    /// the pass can only see them through the behavior overlay. Kebab/camel
    /// lookup, the `string | number` union and the unrepresented `any` member
    /// stay covered without any expression query.
    #[test]
    fn case_component_0292_behavior_properties_and_kebab_camel_bindings_are_checked() {
        let wxml = r#"<view><behavior-ok card-title="hello" score="7" /></view>"#;
        let card_title = literal_attr(wxml, "card-title", r#"card-title="hello""#);
        let score = literal_attr(wxml, "score", r#"score="7""#);
        let layout = shadow(wxml);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(BEHAVIOR_CARD_PATH, BEHAVIOR_CARD_SCRIPT),
        ];
        let ok = run(
            &layout,
            &build_view(
                files.clone(),
                vec![usage(
                    "behavior-ok",
                    Some(BEHAVIOR_CARD_PATH),
                    vec![card_title, score],
                )],
            ),
        );
        assert!(ok.diagnostics.is_empty(), "{:?}", ok.diagnostics);
        assert!(ok.status.is_none(), "{:?}", ok.status);

        let wxml = r#"<view><behavior-bad card-title defaulted="nope" score /></view>"#;
        let card_title = boolean_attr(wxml, "card-title", "card-title");
        let defaulted = literal_attr(wxml, "defaulted", r#"defaulted="nope""#);
        let score = boolean_attr(wxml, "score", "score");
        let layout = shadow(wxml);
        let bad = run(
            &layout,
            &build_view(
                files.clone(),
                vec![usage(
                    "behavior-bad",
                    Some(BEHAVIOR_CARD_PATH),
                    vec![card_title.clone(), defaulted.clone(), score.clone()],
                )],
            ),
        );
        assert!(bad.status.is_none(), "{:?}", bad.status);
        let expected = [
            (
                card_title.attribute_range,
                "component property \"card-title\" expects string, got boolean literal",
            ),
            (
                defaulted.attribute_range,
                "component property \"defaulted\" expects number, got string literal",
            ),
            (
                score.attribute_range,
                "component property \"score\" expects string | number, got boolean literal",
            ),
        ];
        let actual = bad
            .diagnostics
            .iter()
            .map(|diagnostic| {
                assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
                (diagnostic.range, diagnostic.message.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);

        // `loose` is script `any`, which the literal path cannot represent.
        let wxml = r#"<view><behavior-loose loose="x" /></view>"#;
        let loose = literal_attr(wxml, "loose", r#"loose="x""#);
        let layout = shadow(wxml);
        let loose_result = run(
            &layout,
            &build_view(
                files.clone(),
                vec![usage(
                    "behavior-loose",
                    Some(BEHAVIOR_CARD_PATH),
                    vec![loose],
                )],
            ),
        );
        assert!(
            loose_result.diagnostics.is_empty(),
            "{:?}",
            loose_result.diagnostics
        );
        let CapabilityStatus::Partial { reason } = loose_result
            .status
            .as_ref()
            .expect("behavior-loose must be Partial")
        else {
            panic!("expected Partial");
        };
        assert_eq!(reason, &format!("{SKIPPED_PREFIX} (behavior-loose.loose)"));

        let wxml = "<view><behavior-missing-required /></view>";
        let layout = shadow(wxml);
        let missing = run(
            &layout,
            &build_view(
                files,
                vec![usage(
                    "behavior-missing-required",
                    Some(BEHAVIOR_CARD_PATH),
                    Vec::new(),
                )],
            ),
        );
        assert!(missing.diagnostics.is_empty(), "{:?}", missing.diagnostics);
        assert!(
            missing.status.is_none(),
            "a zero-attribute usage is checked and clean: {:?}",
            missing.status
        );
    }

    /// The real fixture reaches the pass through the project host, so the
    /// canonical declaration seam is exercised without any engine.
    #[test]
    fn fixture_prepared_view_carries_component_declarations() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/components-typings")
            .canonicalize()
            .expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open fixture root");
        let path = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        let text = std::fs::read_to_string(root.join("pages/index/index.wxml")).expect("wxml text");
        let file = host.file_id(&path).expect("wxml file id");
        let _target = host.view_for(&path).expect("view target");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            text,
        ));
        let view = host.analysis(&snapshot).expect("prepared view");
        assert!(
            view.components
                .iter()
                .any(|usage| usage.tag == "van-button"),
            "the declared tag must still be analyzed as a custom component"
        );
        let facts = view
            .typing
            .as_ref()
            .expect("the project host must attach declaration facts");
        let card = facts.component("van-button").expect("declared fixture tag");
        let property = card.prop("type").expect("declared type property");
        assert_eq!(property.type_text, "'primary' | 'default' | 'warning'");
        assert!(property.proven);
    }

    #[test]
    fn fixture_declared_bindings_publish_only_the_undeclared_attribute() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/components-typings")
            .canonicalize()
            .expect("fixture root");
        let host = ProjectState::new();
        host.open_root(Some(&format!("file://{}", root.display())))
            .expect("open fixture root");
        let path = ProjectPath::new("pages/index/index.wxml").expect("wxml path");
        let text = std::fs::read_to_string(root.join("pages/index/index.wxml")).expect("wxml text");
        let file = host.file_id(&path).expect("wxml file id");
        let _target = host.view_for(&path).expect("view target");
        let snapshot = Arc::new(SourceSnapshot::new(
            file,
            SourceRevision::new(1),
            SnapshotId::new(1),
            text.clone(),
        ));
        let view = host.analysis(&snapshot).expect("prepared view");
        assert_eq!(view.components.len(), 4, "{:?}", view.components);

        let layout = shadow(&text);
        let result = run(&layout, &view);
        let round_start = text
            .find(r#"round="{{ true }}""#)
            .expect("undeclared attribute");
        let round_range = source_range(round_start, round_start + "round".len());

        // `text="{{ loading }}"` is a type mismatch for the engine probe; this
        // pass only reports the undeclared `round` attribute name.
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let undeclared = &result.diagnostics[0];
        assert_eq!(undeclared.range, round_range);
        assert_eq!(undeclared.severity, DiagnosticSeverity::Error);
        assert_eq!(
            undeclared.message,
            "component property \"round\" is not declared by the component declaration"
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }
}
