//! Component property binding diagnostics.
//!
//! The template analyzer already records every custom-component usage and
//! classifies each non-event attribute value, and the shadow mounts every
//! mustache. This pass adds the missing check: read the component's declared
//! `properties` from the prepared script facts, query the bound expression's
//! type through the pinned native `type_query`, and publish an error only when
//! the mismatch is provable.
//!
//! The comparison is deliberately conservative. The pinned API has no
//! `isTypeAssignableTo`, so a complex expression, an `any`/`unknown`/missing
//! type, an unresolved component script, an incomplete `properties` literal or
//! a structural array/object question degrades the result to `Partial` through
//! the stable reason prefix. Nothing is guessed and no error is fabricated.
//!
//! Event bindings are excluded by `ComponentUsage::attributes` upstream; this
//! pass never inspects raw element attributes, so `bind*`/`catch*`/
//! `capture-*`/`mut-bind`/`slot`/`wx:*` cannot become property diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::backend_tsgo::query::type_info::{TypeQuality, TypeResult, map_type_query};
use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::session::TypeQuery;
use wxml_backend_tsgo::native::strings::{
    Utf16Offset, Utf16Position, offset_to_position, utf16_len,
};
use wxml_backend_tsgo::script::properties::PropertyFact;
use wxml_backend_tsgo::script::registration::RegistrationKind;
use wxml_backend_tsgo::script::{ScriptRequest, analyze_script};
use wxml_project::ComponentTypingFacts;
use wxml_project::typing::{PrimitiveType, TypeText, TypingComponent, TypingProperty};
use wxml_syntax::{Expr, ExprKind, parse_expression};
use wxml_text::TextRange;
use wxml_types::{
    AttributeValueKind, CapabilityStatus, ComponentAttribute, ComponentUsage, Diagnostic,
    DiagnosticSeverity, SourceRange,
};

use crate::backend_tsgo::mapping::ShadowLayout;
use crate::backend_tsgo::session::QuerySession;
use crate::project_host::PreparedView;
use crate::semantics::tsconfig_paths::TsconfigPaths;

/// Maximum distinct component tags checked in one template.
const MAX_TAGS: usize = 64;
/// Maximum published diagnostics for one component tag.
const MAX_DIAGNOSTICS_PER_TAG: usize = 8;
/// Maximum native expression queries for one diagnostics run.
const MAX_QUERIES: usize = 256;
/// Maximum `tag.attribute` examples quoted in the `Partial` reason.
const MAX_SKIPPED_DETAILS: usize = 3;
/// Maximum quoted expression length inside a message.
const MAX_EXPRESSION_TEXT: usize = 80;
/// Maximum published message length after sanitization.
const MAX_MESSAGE_LENGTH: usize = 512;

/// Stable prefix shared with the Node backend's `Partial` reason.
const SKIPPED_PREFIX: &str = "some component bindings could not be resolved and were not checked";

/// Pinned `tsgo` 7.1.0-beta `TypeFlags` bits observed through the public wire
/// `flags` field of `getTypeAtPosition`.
///
/// The pinned engine does not document these values in the protocol, so they
/// are used only as corroboration after the `typeToString` text: a shape the
/// text already proves always wins, and an unknown flag combination leaves the
/// type unclassified instead of producing a diagnostic.
mod type_flags {
    /// `any`.
    pub(super) const ANY: u64 = 1;
    /// `unknown`.
    pub(super) const UNKNOWN: u64 = 2;
    /// `undefined`.
    pub(super) const UNDEFINED: u64 = 4;
    /// `null`.
    pub(super) const NULL: u64 = 8;
    /// Primitive `string`.
    pub(super) const STRING: u64 = 32;
    /// Primitive `number`.
    pub(super) const NUMBER: u64 = 64;
    /// Primitive `boolean`.
    pub(super) const BOOLEAN: u64 = 256;
    /// String literal.
    pub(super) const STRING_LITERAL: u64 = 1024;
    /// Numeric literal.
    pub(super) const NUMBER_LITERAL: u64 = 2048;
    /// Boolean literal.
    pub(super) const BOOLEAN_LITERAL: u64 = 8192;
    /// `never`.
    pub(super) const NEVER: u64 = 262_144;
    /// Object-like type, including arrays and functions.
    pub(super) const OBJECT: u64 = 1_048_576;
}

/// Diagnostics produced by the component binding pass.
pub(crate) struct ComponentBindingDiagnostics {
    /// Errors in original template bytes, deduplicated and sorted.
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// A `Partial` status when coverage is incomplete, `None` otherwise.
    pub(crate) status: Option<CapabilityStatus>,
}

/// Runs the component binding pass through a live query session.
pub(crate) fn diagnostics(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    view: &PreparedView,
    script_path: &str,
) -> ComponentBindingDiagnostics {
    diagnostics_with(layout, view, script_path, |path, position| {
        session.type_query(path, position)
    })
}

/// Runs the component binding pass with an explicit expression type source.
///
/// The query seam keeps the comparison unit-testable without a native process.
pub(crate) fn diagnostics_with(
    layout: &ShadowLayout,
    view: &PreparedView,
    script_path: &str,
    query: impl FnMut(&str, Utf16Position) -> Result<TypeQuery, NativeError>,
) -> ComponentBindingDiagnostics {
    if view.components.is_empty() {
        return ComponentBindingDiagnostics {
            diagnostics: Vec::new(),
            status: None,
        };
    }
    let mut pass = Pass::new(layout, view, script_path, query);
    pass.run();
    pass.finish()
}

/// One declared component property and its conservative target set.
#[derive(Clone, Debug)]
struct DeclaredBinding {
    /// Declaration text shown in the message.
    text: String,
    /// Every allowed target; empty means "cannot prove" and the attribute is skipped.
    targets: Vec<Target>,
}

/// Declared properties of one component script.
#[derive(Clone, Debug)]
struct ComponentDeclarations {
    /// Property name to declaration. A present name with `None` targets means
    /// the declaration is readable but not checkable.
    properties: BTreeMap<String, Option<DeclaredBinding>>,
    /// `false` when only the component's own `properties` were enumerated and
    /// behaviors may contribute unlisted names. The listed declarations remain
    /// checkable, but an attribute outside them is not an error.
    complete: bool,
}

/// What a declared property accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// Primitive `string`.
    String,
    /// Primitive `number`.
    Number,
    /// Primitive `boolean`.
    Boolean,
    /// Array-like value.
    Array,
    /// Object-like value; arrays are accepted like the Node Object flag check.
    Object,
}

impl Target {
    /// Returns whether `actual` satisfies this target.
    const fn accepts(self, actual: Actual) -> bool {
        match actual {
            Actual::String => matches!(self, Self::String),
            Actual::Number => matches!(self, Self::Number),
            Actual::Boolean => matches!(self, Self::Boolean),
            // An `Object` declaration accepts arrays, matching the Node
            // backend's Object type flag check.
            Actual::Array => matches!(self, Self::Array | Self::Object),
            Actual::Object => matches!(self, Self::Object),
            Actual::Never | Actual::Unset | Actual::Unknown => false,
        }
    }
}

/// A classified bound type constituent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Actual {
    /// Primitive or literal string.
    String,
    /// Primitive or literal number.
    Number,
    /// Primitive or literal boolean.
    Boolean,
    /// Array-like value.
    Array,
    /// Object-like value.
    Object,
    /// `never` is assignable to every declared target.
    Never,
    /// `undefined`/`null`/`void` mean the property is not set.
    Unset,
    /// `any`, `unknown`, a missing type or an unsupported shape.
    Unknown,
}

/// Three-state comparison result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Every defined constituent matches.
    Accept,
    /// At least one constituent cannot match; this wins over `Unknown`.
    Reject,
    /// At least one constituent cannot be classified.
    Unknown,
}

/// Runs the component binding pass with precomputed declarations.
struct Pass<'a, Q> {
    layout: &'a ShadowLayout,
    view: &'a PreparedView,
    script_path: &'a str,
    input: Option<AnalysisInput>,
    /// Declaration facts attached to the view by the project host.
    ///
    /// `None` means the project has no usable `components.d.ts`; consumers then
    /// keep the script-derived behaviour unchanged.
    typing: Option<Arc<ComponentTypingFacts>>,
    declarations: BTreeMap<String, Option<ComponentDeclarations>>,
    tags: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    per_tag: BTreeMap<String, usize>,
    details: Vec<String>,
    /// 组件 behaviors 未能完全解析、但自身 properties 完整时，记录按自身声明
    /// 校验过的 tag；这些 tag 的结果可用但不完整。
    behavior_incomplete: BTreeSet<String>,
    skipped: usize,
    queries: usize,
    partial: bool,
    stopped: bool,
    query: Q,
}

impl<'a, Q> Pass<'a, Q>
where
    Q: FnMut(&str, Utf16Position) -> Result<TypeQuery, NativeError>,
{
    fn new(
        layout: &'a ShadowLayout,
        view: &'a PreparedView,
        script_path: &'a str,
        query: Q,
    ) -> Self {
        Self {
            layout,
            view,
            script_path,
            input: component_input(view),
            typing: view.typing.clone(),
            declarations: BTreeMap::new(),
            tags: BTreeSet::new(),
            diagnostics: Vec::new(),
            per_tag: BTreeMap::new(),
            details: Vec::new(),
            behavior_incomplete: BTreeSet::new(),
            skipped: 0,
            queries: 0,
            partial: false,
            stopped: false,
            query,
        }
    }

    #[allow(clippy::too_many_lines, reason = "one linear usage check pipeline")]
    fn run(&mut self) {
        for usage in self.view.components.iter() {
            if self.stopped {
                break;
            }
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
            // surface: the script `properties` object is never consulted, so
            // `analyze_component` is skipped for this tag.
            let declared = self
                .typing
                .as_ref()
                .and_then(|facts| facts.component(&usage.tag))
                .cloned();
            if let Some(typing) = declared
                && typing.complete
            {
                self.check_declared_usage(usage, &typing);
                continue;
            }
            if self.input.is_none() {
                // No project root or no representable file set: every script
                // derived usage is unproven, but an empty attribute list still
                // needs no reason.
                self.partial = true;
                self.skip(&usage.tag);
                continue;
            }
            let Some(component_file) = usage.component_file.as_ref() else {
                self.skip(&usage.tag);
                continue;
            };
            let path = component_file.path();
            if !self
                .view
                .files
                .iter()
                .any(|file| file.path.as_str() == path)
            {
                self.skip(&usage.tag);
                continue;
            }
            let Some(declarations) = self.declarations(path) else {
                self.skip(&usage.tag);
                continue;
            };
            if declarations.properties.is_empty() {
                // A component without declared properties makes the binding
                // surface unproven, not clean.
                self.skip(&usage.tag);
                continue;
            }
            if !declarations.complete {
                // behaviors 未能解析：只按组件自身声明的属性校验，未声明的
                // 属性可能来自 behavior，因此不能报错，只能标记结果不完整。
                self.behavior_incomplete.insert(usage.tag.clone());
            }
            for attribute in &usage.attributes {
                if self.stopped {
                    break;
                }
                let declared = match declared_property(&declarations, &attribute.name) {
                    // `class`, `style`, `id` and undeclared attributes are not
                    // component properties, exactly like the Node backend.
                    DeclaredLookup::Undeclared => continue,
                    DeclaredLookup::Unproven => {
                        self.skip(&format!("{}.{}", usage.tag, attribute.name));
                        self.partial = true;
                        continue;
                    }
                    DeclaredLookup::Checkable(declared) => declared,
                };
                self.check_attribute(&usage.tag, attribute, declared);
            }
        }
    }

    /// Returns the declarations for one component path, cached by path.
    fn declarations(&mut self, path: &str) -> Option<ComponentDeclarations> {
        if let Some(cached) = self.declarations.get(path) {
            return cached.clone();
        }
        let parsed = self
            .input
            .as_ref()
            .and_then(|input| analyze_component(input, self.view, path));
        self.declarations.insert(path.to_owned(), parsed.clone());
        parsed
    }

    /// Checks one attribute of a component that has declaration facts.
    ///
    /// Each member is judged on its own `proven` flag: an unproven member is a
    /// coverage gap, while the remaining declared members are still checked.
    fn check_declared_usage(&mut self, usage: &ComponentUsage, typing: &TypingComponent) {
        for attribute in &usage.attributes {
            if self.stopped {
                break;
            }
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

    /// Checks one declared attribute against its contract.
    fn check_declared_attribute(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        property: &TypingProperty,
    ) {
        let contract = PropertyContract::from_property(property);
        if matches!(contract, PropertyContract::Unknown) {
            // An unproven member, or a type the contract cannot represent, is
            // never guessed: it is a coverage gap instead of a diagnostic.
            self.partial = true;
            self.skip(&format!("{tag}.{}", attribute.name));
            return;
        }
        if matches!(contract, PropertyContract::Any) {
            // `any` accepts every value, so no query is needed and the result
            // is proven clean.
            return;
        }
        let allows_undefined = property.optional || contains_undefined(&property.ty);
        match attribute.value_kind {
            AttributeValueKind::Literal => {
                let value =
                    literal_value(self.layout.original_wxml(), attribute).map(str::to_owned);
                let Some(value) = value else {
                    self.partial = true;
                    self.skip(&format!("{tag}.{}", attribute.name));
                    return;
                };
                self.check_declared_literal(
                    tag,
                    attribute,
                    &contract,
                    &property.type_text,
                    LiteralValue::String(&value),
                );
            }
            AttributeValueKind::Boolean => {
                self.check_declared_literal(
                    tag,
                    attribute,
                    &contract,
                    &property.type_text,
                    LiteralValue::Boolean,
                );
            }
            AttributeValueKind::Mustache => self.check_declared_mustache(
                tag,
                attribute,
                &contract,
                allows_undefined,
                &property.type_text,
            ),
            // `Mixed` needs a string conversion and a future value kind is
            // unknown: both are coverage gaps, never errors.
            _ => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
        }
    }

    /// Checks one literal attribute value against a declared contract.
    fn check_declared_literal(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        contract: &PropertyContract,
        type_text: &str,
        value: LiteralValue<'_>,
    ) {
        match contract.judge_literal(value) {
            Verdict::Accept => {}
            Verdict::Unknown => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
            Verdict::Reject => {
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

    /// Checks one bound expression against a declared contract.
    fn check_declared_mustache(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        contract: &PropertyContract,
        allows_undefined: bool,
        type_text: &str,
    ) {
        let Some((result, expr_range)) = self.query_bound_type(tag, attribute) else {
            return;
        };
        match compare_contract(contract, allows_undefined, &result) {
            Verdict::Accept => {}
            Verdict::Unknown => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
            Verdict::Reject => {
                let got = result
                    .info
                    .as_ref()
                    .map(|info| info.text.trim())
                    .filter(|text| !text.is_empty())
                    .unwrap_or("unknown");
                let expr_text = attribute.expr_text.as_deref().unwrap_or_default();
                let message = format!(
                    "component property \"{}\" expects {}, got {} (bound expression \"{}\")",
                    attribute.name,
                    type_text,
                    truncate(got, MAX_EXPRESSION_TEXT),
                    truncate(expr_text, MAX_EXPRESSION_TEXT),
                );
                self.publish(tag, &attribute.name, expr_range, &message);
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

    fn check_attribute(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        declared: &DeclaredBinding,
    ) {
        if declared.targets.is_empty() {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return;
        }
        match attribute.value_kind {
            AttributeValueKind::Literal => {
                self.publish_literal_rejection(
                    tag,
                    attribute,
                    declared,
                    Actual::String,
                    "string literal",
                );
            }
            AttributeValueKind::Boolean => {
                self.publish_literal_rejection(
                    tag,
                    attribute,
                    declared,
                    Actual::Boolean,
                    "boolean literal",
                );
            }
            AttributeValueKind::Mustache => self.check_mustache(tag, attribute, declared),
            // `Mixed` needs a string conversion and a future value kind is
            // unknown: both are coverage gaps, never errors.
            _ => {
                self.skip(&format!("{tag}.{}", attribute.name));
                self.partial = true;
            }
        }
    }

    fn check_mustache(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        declared: &DeclaredBinding,
    ) {
        let Some((result, expr_range)) = self.query_bound_type(tag, attribute) else {
            return;
        };
        let actual = classify_type(&result);
        match compare(&declared.targets, &actual) {
            Verdict::Accept => {}
            Verdict::Unknown => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
            Verdict::Reject => {
                let got = result
                    .info
                    .as_ref()
                    .map(|info| info.text.trim())
                    .filter(|text| !text.is_empty())
                    .unwrap_or("unknown");
                let expr_text = attribute.expr_text.as_deref().unwrap_or_default();
                let message = format!(
                    "component property \"{}\" expects {}, got {} (bound expression \"{}\")",
                    attribute.name,
                    declared.text,
                    truncate(got, MAX_EXPRESSION_TEXT),
                    truncate(expr_text, MAX_EXPRESSION_TEXT),
                );
                self.publish(tag, &attribute.name, expr_range, &message);
            }
        }
    }

    /// Queries the type of the single bound expression of one attribute.
    ///
    /// Every unprovable step records a coverage gap and returns `None`, so the
    /// caller never fabricates a diagnostic from a missing answer.
    #[allow(clippy::too_many_lines, reason = "one linear bound expression query")]
    fn query_bound_type(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
    ) -> Option<(TypeResult, SourceRange)> {
        let (Some(expr_text), Some(expr_range)) =
            (attribute.expr_text.as_deref(), attribute.expr_range)
        else {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        };
        if !self.expr_matches(expr_range, expr_text) {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        }
        if self.layout.mount_for_range(expr_range).is_none() {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        }
        let parsed = parse_expression(expr_text, expr_range.start());
        if !parsed.errors.is_empty() {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        }
        let Some(target) = chain_target(&parsed.expr) else {
            // Only a free-root identifier or a member chain can be queried at
            // one position. Calls, operators, indexes and conditionals would
            // make the queried token's type differ from the expression type.
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        };
        if target.start() < expr_range.start() || target.end() > expr_range.end() {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        }
        let Ok(derived) = self.layout.derived_offset(target.start(), false) else {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        };
        let Some(mount) = self.layout.mount_for_range(expr_range) else {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        };
        let Some(mount_end) = mount.start_utf16.checked_add(utf16_len(&mount.text)) else {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        };
        if derived < mount.start_utf16 || derived >= mount_end {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        }
        let Ok(position) = offset_to_position(self.layout.derived(), Utf16Offset::new(derived))
        else {
            self.skip(&format!("{tag}.{}", attribute.name));
            self.partial = true;
            return None;
        };
        if self.queries >= MAX_QUERIES {
            self.partial = true;
            self.stopped = true;
            self.skip(&format!("{tag}.{}", attribute.name));
            return None;
        }
        self.queries += 1;
        let raw = match (self.query)(self.script_path, position) {
            Ok(raw) => raw,
            Err(NativeError::Unsupported { .. }) => {
                self.partial = true;
                self.stopped = true;
                self.skip(&format!("{tag}.{}", attribute.name));
                return None;
            }
            Err(_) => {
                // A hard query failure keeps the already-mapped backend
                // diagnostics and is a coverage gap, never a fake error.
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
                return None;
            }
        };
        Some((map_type_query(&raw), expr_range))
    }

    fn publish_literal_rejection(
        &mut self,
        tag: &str,
        attribute: &ComponentAttribute,
        declared: &DeclaredBinding,
        actual: Actual,
        got: &str,
    ) {
        match compare(&declared.targets, &[actual]) {
            Verdict::Accept => {}
            Verdict::Unknown => {
                self.partial = true;
                self.skip(&format!("{tag}.{}", attribute.name));
            }
            Verdict::Reject => {
                let message = format!(
                    "component property \"{}\" expects {}, got {got}",
                    attribute.name, declared.text,
                );
                self.publish(tag, &attribute.name, attribute.attribute_range, &message);
            }
        }
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

    fn expr_matches(&self, range: SourceRange, text: &str) -> bool {
        let (Ok(start), Ok(end)) = (usize::try_from(range.start()), usize::try_from(range.end()))
        else {
            return false;
        };
        self.layout.original_wxml().get(start..end) == Some(text)
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

/// Builds the pure-Rust analysis input for component scripts.
fn component_input(view: &PreparedView) -> Option<AnalysisInput> {
    let root = view
        .project_root
        .to_str()
        .and_then(|root| ProjectRoot::new(root).ok())?;
    let mut files = Vec::with_capacity(view.files.len());
    for prepared in view.files.iter() {
        if let Ok(file) = SourceFile::new(
            prepared.path.as_str(),
            prepared.text.to_string(),
            DocumentRevision::new(prepared.source_revision),
        ) {
            files.push(file);
        }
    }
    AnalysisInput::new(
        ProjectKey::new("wxml-lsp-component-bindings"),
        root,
        SnapshotRevision::new(1),
        files,
    )
    .ok()
}

/// Analyzes one component script and extracts its declared property contract.
///
/// `view` supplies the tsconfig `paths` resolver: a behavior imported through an
/// alias would otherwise read as unresolved and hide every property it
/// contributes.
fn analyze_component(
    input: &AnalysisInput,
    view: &PreparedView,
    path: &str,
) -> Option<ComponentDeclarations> {
    let resolver = TsconfigPaths::from_view(view);
    let request = ScriptRequest::new(input, path, RegistrationKind::Component);
    let request = resolver
        .as_ref()
        .map_or(request, |resolver| request.with_resolver(resolver));
    let analysis = analyze_script(&request).ok()?;
    if !analysis.registration.complete {
        return None;
    }
    // behaviors 不可读但自身 properties 已完整枚举时，仍可用自身声明做校验；
    // 未声明的属性可能由 behavior 贡献，调用方不得报错。
    if !analysis.properties.complete && !analysis.properties.own_complete {
        return None;
    }
    let properties = analysis
        .properties
        .properties
        .iter()
        .map(|fact| (fact.name.clone(), declared_binding(fact)))
        .collect();
    Some(ComponentDeclarations {
        properties,
        complete: analysis.properties.complete,
    })
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

/// Parses one `PropertyFact` into a checkable contract.
fn declared_binding(fact: &PropertyFact) -> Option<DeclaredBinding> {
    if !fact.type_complete {
        return None;
    }
    let targets = declared_targets(&fact.type_text)?;
    if targets.is_empty() {
        return None;
    }
    Some(DeclaredBinding {
        text: fact.type_text.clone(),
        targets,
    })
}

/// Splits a declaration into its allowed targets.
///
/// `any`, `Function` and every unknown constructor return `None`: the whole
/// property is unproven rather than checked against a guessed target.
fn declared_targets(text: &str) -> Option<Vec<Target>> {
    let parts = split_union(text);
    if parts.is_empty() {
        return None;
    }
    let mut targets = Vec::with_capacity(parts.len());
    for part in parts {
        let target = match part.trim() {
            "string" => Target::String,
            "number" => Target::Number,
            "boolean" => Target::Boolean,
            "any[]" => Target::Array,
            "Record<string, any>" => Target::Object,
            _ => return None,
        };
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Some(targets)
}

/// Returns the declaration for `attribute` using the shared kebab/camel rule.
///
/// The lookup distinguishes an undeclared attribute (ignore it) from a
/// declared but unproven one (skip it and report partial coverage).
fn declared_property<'a>(
    declarations: &'a ComponentDeclarations,
    attribute: &str,
) -> DeclaredLookup<'a> {
    if let Some(found) = declarations.properties.get(attribute) {
        return lookup(found.as_ref());
    }
    declarations
        .properties
        .iter()
        .find(|(name, _)| wxml_project::component_name_eq(name, attribute))
        .map_or(DeclaredLookup::Undeclared, |(_, found)| {
            lookup(found.as_ref())
        })
}

/// Result of one component attribute name lookup.
enum DeclaredLookup<'a> {
    /// The attribute is not a declared property; ignore it.
    Undeclared,
    /// The property is declared but its type could not be proven.
    Unproven,
    /// The property has a checkable contract.
    Checkable(&'a DeclaredBinding),
}

const fn lookup(found: Option<&DeclaredBinding>) -> DeclaredLookup<'_> {
    match found {
        Some(binding) => DeclaredLookup::Checkable(binding),
        None => DeclaredLookup::Unproven,
    }
}

/// What one declared component attribute accepts.
///
/// The contract keeps only what the pinned engine can prove through
/// `getTypeAtPosition`: a primitive or structural target, or a finite literal
/// set. Every other type expression stays [`PropertyContract::Unknown`] and is
/// never diagnosed.
#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertyContract {
    /// The member type is unproven or cannot be represented exactly.
    Unknown,
    /// `any` accepts every bound value.
    Any,
    /// One primitive or structural target.
    Kind(Target),
    /// A union of string literals; only the listed values are accepted.
    StringLiterals(Vec<String>),
    /// A union of numeric literals; only the listed values are accepted.
    NumberLiterals(Vec<String>),
}

impl PropertyContract {
    /// Maps one declared member to a checkable contract.
    ///
    /// `proven == false` always yields [`PropertyContract::Unknown`]: the
    /// declaration parser did not understand the whole expression.
    fn from_property(property: &TypingProperty) -> Self {
        if !property.proven {
            return Self::Unknown;
        }
        Self::from_type(&property.ty)
    }

    /// Maps one parsed type expression to a contract.
    fn from_type(ty: &TypeText) -> Self {
        match ty {
            TypeText::Primitive(primitive) => Self::from_primitive(*primitive),
            TypeText::StringLiteral(value) => Self::StringLiterals(vec![value.clone()]),
            TypeText::NumberLiteral(value) => Self::NumberLiterals(vec![value.clone()]),
            // Boolean literals have no literal contract variant; a plain
            // boolean target accepts every boolean without a false positive.
            TypeText::BooleanLiteral(_) => Self::Kind(Target::Boolean),
            TypeText::Array(_) => Self::Kind(Target::Array),
            TypeText::Record(_) | TypeText::Object(_) => Self::Kind(Target::Object),
            TypeText::Union(parts) => Self::from_union(parts),
            TypeText::Unproven => Self::Unknown,
        }
    }

    /// Maps one primitive type to a contract.
    const fn from_primitive(primitive: PrimitiveType) -> Self {
        match primitive {
            PrimitiveType::String => Self::Kind(Target::String),
            PrimitiveType::Number => Self::Kind(Target::Number),
            PrimitiveType::Boolean => Self::Kind(Target::Boolean),
            PrimitiveType::Object => Self::Kind(Target::Object),
            PrimitiveType::Any => Self::Any,
            // `null`/`undefined`/`void` alone admit no bound value, and
            // `unknown`/`never` cannot be narrowed into a target.
            _ => Self::Unknown,
        }
    }

    /// Maps a union to a contract, treating nullish alternatives as absence.
    fn from_union(parts: &[TypeText]) -> Self {
        let mut alternatives = Vec::new();
        for part in parts {
            flatten_union(part, &mut alternatives);
        }
        let mut targets = Vec::new();
        let mut strings: Vec<String> = Vec::new();
        let mut numbers: Vec<String> = Vec::new();
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
                    push_target(&mut targets, Target::String);
                }
                TypeText::Primitive(PrimitiveType::Number) => {
                    push_target(&mut targets, Target::Number);
                }
                TypeText::Primitive(PrimitiveType::Boolean) | TypeText::BooleanLiteral(_) => {
                    push_target(&mut targets, Target::Boolean);
                }
                TypeText::Primitive(PrimitiveType::Any) => return Self::Any,
                TypeText::StringLiteral(value) => push_unique(&mut strings, value),
                TypeText::NumberLiteral(value) => push_unique(&mut numbers, value),
                TypeText::Array(_) => push_target(&mut targets, Target::Array),
                TypeText::Primitive(PrimitiveType::Object)
                | TypeText::Record(_)
                | TypeText::Object(_) => push_target(&mut targets, Target::Object),
                // An empty nested union, an unsupported primitive or an
                // unproven part cannot be narrowed into a contract.
                TypeText::Primitive(_) | TypeText::Union(_) | TypeText::Unproven => {
                    return Self::Unknown;
                }
            }
        }
        match targets.as_slice() {
            // `string | 'a' | 'b'` still accepts the whole string domain.
            [Target::String] if numbers.is_empty() => Self::Kind(Target::String),
            [Target::Number] if strings.is_empty() => Self::Kind(Target::Number),
            [target] if strings.is_empty() && numbers.is_empty() => Self::Kind(*target),
            [] if !strings.is_empty() && numbers.is_empty() => Self::StringLiterals(strings),
            [] if strings.is_empty() && !numbers.is_empty() => Self::NumberLiterals(numbers),
            // Mixed primitive kinds or literals of several kinds cannot be
            // expressed as one contract.
            _ => Self::Unknown,
        }
    }

    /// Judges one proven literal attribute value.
    fn judge_literal(&self, value: LiteralValue<'_>) -> Verdict {
        match self {
            Self::Unknown => Verdict::Unknown,
            Self::Any => Verdict::Accept,
            Self::Kind(target) => {
                let actual = match value {
                    LiteralValue::String(_) => Actual::String,
                    LiteralValue::Boolean => Actual::Boolean,
                };
                if target.accepts(actual) {
                    Verdict::Accept
                } else {
                    Verdict::Reject
                }
            }
            Self::StringLiterals(allowed) => match value {
                LiteralValue::String(text) => {
                    if allowed.iter().any(|allowed| allowed == text) {
                        Verdict::Accept
                    } else {
                        Verdict::Reject
                    }
                }
                LiteralValue::Boolean => Verdict::Reject,
            },
            // A WXML literal attribute is always a string, never a number.
            Self::NumberLiterals(_) => Verdict::Reject,
        }
    }

    /// Judges one classified constituent of a queried bound type.
    fn judge(&self, part: Constituent<'_>) -> Verdict {
        match self {
            Self::Unknown => Verdict::Unknown,
            Self::Any => Verdict::Accept,
            Self::Kind(target) => {
                let actual = part.actual();
                if target.accepts(actual) {
                    return Verdict::Accept;
                }
                // An object that is not textually an array may still be
                // array-like. The pinned API has no `isArrayLikeType`, so an
                // `Array` declaration cannot prove a rejection here.
                if matches!(actual, Actual::Object) && *target == Target::Array {
                    return Verdict::Unknown;
                }
                Verdict::Reject
            }
            Self::StringLiterals(allowed) => match part {
                Constituent::String(Some(value)) => {
                    if allowed.iter().any(|allowed| allowed == value) {
                        Verdict::Accept
                    } else {
                        Verdict::Reject
                    }
                }
                // A plain `string` may hold any value: unprovable, not clean.
                Constituent::String(None) => Verdict::Unknown,
                _ => Verdict::Reject,
            },
            Self::NumberLiterals(allowed) => match part {
                Constituent::Number(Some(value)) => {
                    if allowed.iter().any(|allowed| number_eq(allowed, value)) {
                        Verdict::Accept
                    } else {
                        Verdict::Reject
                    }
                }
                Constituent::Number(None) => Verdict::Unknown,
                _ => Verdict::Reject,
            },
        }
    }
}

/// One proven literal attribute value.
#[derive(Clone, Copy, Debug)]
enum LiteralValue<'a> {
    /// A quoted attribute value, for example `type="primary"`.
    String(&'a str),
    /// A valueless attribute, for example `disabled`.
    Boolean,
}

/// One classified constituent of a queried bound type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Constituent<'a> {
    /// A string value; the inner text is present for a literal type.
    String(Option<&'a str>),
    /// A number value; the inner text is present for a literal type.
    Number(Option<&'a str>),
    /// A boolean value.
    Boolean,
    /// An array value.
    Array,
    /// An object value.
    Object,
    /// `undefined`/`void`; allowed only when the declaration says so.
    Undefined,
    /// `null` is rendered as empty, like the script-derived path.
    Null,
    /// `never` is assignable to every declared target.
    Never,
    /// `any`, `unknown` or an unsupported shape.
    Unknown,
}

impl Constituent<'_> {
    /// Maps the constituent to the script path's coarse classification.
    const fn actual(self) -> Actual {
        match self {
            Self::String(_) => Actual::String,
            Self::Number(_) => Actual::Number,
            Self::Boolean => Actual::Boolean,
            Self::Array => Actual::Array,
            Self::Object => Actual::Object,
            Self::Undefined | Self::Null => Actual::Unset,
            Self::Never => Actual::Never,
            Self::Unknown => Actual::Unknown,
        }
    }
}

/// Compares one queried bound type with a declared contract.
///
/// `allows_undefined` is `true` for optional members and for types that already
/// include `undefined`/`void`; every other `undefined` constituent is a proven
/// mismatch.
fn compare_contract(
    contract: &PropertyContract,
    allows_undefined: bool,
    result: &TypeResult,
) -> Verdict {
    if matches!(contract, PropertyContract::Unknown) {
        return Verdict::Unknown;
    }
    if matches!(contract, PropertyContract::Any) {
        return Verdict::Accept;
    }
    let Some(parts) = constituents(result) else {
        return Verdict::Unknown;
    };
    let mut unknown = false;
    for part in parts {
        match part {
            Constituent::Never | Constituent::Null => {}
            Constituent::Undefined if allows_undefined => {}
            Constituent::Undefined => return Verdict::Reject,
            Constituent::Unknown => unknown = true,
            defined => match contract.judge(defined) {
                Verdict::Accept => {}
                Verdict::Reject => return Verdict::Reject,
                Verdict::Unknown => unknown = true,
            },
        }
    }
    if unknown {
        Verdict::Unknown
    } else {
        Verdict::Accept
    }
}

/// Classifies every constituent of a queried bound type.
///
/// Returns `None` when the answer is not known, so the caller can report a
/// coverage gap instead of a type.
fn constituents(result: &TypeResult) -> Option<Vec<Constituent<'_>>> {
    if result.quality != TypeQuality::Known {
        return None;
    }
    let info = result.info.as_ref()?;
    let parts = split_union(&info.text);
    if parts.len() > 1 {
        return Some(parts.iter().map(|part| constituent(part, 0)).collect());
    }
    Some(vec![constituent(info.text.trim(), info.flags)])
}

/// Classifies one type text with optional pinned-engine flags.
fn constituent(text: &str, flags: u64) -> Constituent<'_> {
    let text = text.trim();
    if text.is_empty() {
        return Constituent::Unknown;
    }
    match text {
        "any" | "unknown" => return Constituent::Unknown,
        "undefined" | "void" => return Constituent::Undefined,
        "null" => return Constituent::Null,
        "never" => return Constituent::Never,
        "string" => return Constituent::String(None),
        "number" => return Constituent::Number(None),
        "boolean" => return Constituent::Boolean,
        _ => {}
    }
    if flags & (type_flags::ANY | type_flags::UNKNOWN) != 0 {
        return Constituent::Unknown;
    }
    if flags & type_flags::UNDEFINED != 0 {
        return Constituent::Undefined;
    }
    if flags & type_flags::NULL != 0 {
        return Constituent::Null;
    }
    if flags & type_flags::NEVER != 0 {
        return Constituent::Never;
    }
    if is_quoted(text) {
        return Constituent::String(string_literal(text));
    }
    if text == "true" || text == "false" {
        return Constituent::Boolean;
    }
    if is_number_literal(text) {
        return Constituent::Number(Some(text));
    }
    if is_array_text(text) {
        return Constituent::Array;
    }
    if text.starts_with('{') || text.starts_with("Record<") {
        return Constituent::Object;
    }
    if flags & type_flags::STRING_LITERAL != 0 {
        return Constituent::String(string_literal(text));
    }
    if flags & type_flags::STRING != 0 {
        return Constituent::String(None);
    }
    if flags & type_flags::NUMBER_LITERAL != 0 {
        return Constituent::Number(Some(text));
    }
    if flags & type_flags::NUMBER != 0 {
        return Constituent::Number(None);
    }
    if flags & (type_flags::BOOLEAN | type_flags::BOOLEAN_LITERAL) != 0 {
        return Constituent::Boolean;
    }
    if flags & type_flags::OBJECT != 0 {
        return Constituent::Object;
    }
    Constituent::Unknown
}

/// Returns one declared member, honouring the `WeChat` kebab/camel mapping.
fn declared_typing_property<'a>(
    typing: &'a TypingComponent,
    attribute: &str,
) -> Option<&'a TypingProperty> {
    typing.prop_named(attribute)
}

/// Returns whether a declared type already admits `undefined`.
fn contains_undefined(ty: &TypeText) -> bool {
    match ty {
        TypeText::Primitive(primitive) => {
            matches!(primitive, PrimitiveType::Undefined | PrimitiveType::Void)
        }
        TypeText::Union(parts) => parts.iter().any(contains_undefined),
        _ => false,
    }
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

/// Pushes a target once, preserving declaration order.
fn push_target(targets: &mut Vec<Target>, target: Target) {
    if !targets.contains(&target) {
        targets.push(target);
    }
}

/// Pushes a literal value once, preserving declaration order.
fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|kept| kept == value) {
        values.push(value.to_owned());
    }
}

/// Compares two numeric literal spellings by value when both parse.
fn number_eq(expected: &str, actual: &str) -> bool {
    match (expected.trim().parse::<f64>(), actual.trim().parse::<f64>()) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => expected.trim() == actual.trim(),
    }
}

/// Returns the inner text of a quoted string literal type.
fn string_literal(text: &str) -> Option<&str> {
    if !is_quoted(text) {
        return None;
    }
    text.get(1..text.len().checked_sub(1)?)
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

/// Classifies every constituent of a bound type.
fn classify_type(result: &TypeResult) -> Vec<Actual> {
    if result.quality != TypeQuality::Known {
        return vec![Actual::Unknown];
    }
    let Some(info) = result.info.as_ref() else {
        return vec![Actual::Unknown];
    };
    let parts = split_union(&info.text);
    if parts.len() > 1 {
        return parts.iter().map(|part| classify_part(part, 0)).collect();
    }
    vec![classify_part(info.text.trim(), info.flags)]
}

/// Classifies one type text with optional pinned-engine flags.
fn classify_part(text: &str, flags: u64) -> Actual {
    let text = text.trim();
    if text.is_empty() {
        return Actual::Unknown;
    }
    match text {
        "any" | "unknown" => return Actual::Unknown,
        "undefined" | "null" | "void" => return Actual::Unset,
        "never" => return Actual::Never,
        "string" => return Actual::String,
        "number" => return Actual::Number,
        "boolean" => return Actual::Boolean,
        _ => {}
    }
    if flags & type_flags::ANY != 0 || flags & type_flags::UNKNOWN != 0 {
        return Actual::Unknown;
    }
    if flags & (type_flags::UNDEFINED | type_flags::NULL) != 0 {
        return Actual::Unset;
    }
    if flags & type_flags::NEVER != 0 {
        return Actual::Never;
    }
    if is_quoted(text) {
        return Actual::String;
    }
    if text == "true" || text == "false" {
        return Actual::Boolean;
    }
    if is_number_literal(text) {
        return Actual::Number;
    }
    if is_array_text(text) {
        return Actual::Array;
    }
    if text.starts_with('{') || text.starts_with("Record<") {
        return Actual::Object;
    }
    if flags & (type_flags::STRING | type_flags::STRING_LITERAL) != 0 {
        return Actual::String;
    }
    if flags & (type_flags::NUMBER | type_flags::NUMBER_LITERAL) != 0 {
        return Actual::Number;
    }
    if flags & (type_flags::BOOLEAN | type_flags::BOOLEAN_LITERAL) != 0 {
        return Actual::Boolean;
    }
    if flags & type_flags::OBJECT != 0 {
        return Actual::Object;
    }
    Actual::Unknown
}

/// Compares the defined constituents with the declared targets.
fn compare(targets: &[Target], actual: &[Actual]) -> Verdict {
    let mut unknown = false;
    for part in actual {
        match part {
            Actual::Unset | Actual::Never => {}
            Actual::Unknown => unknown = true,
            _ => {
                if targets.iter().any(|target| target.accepts(*part)) {
                    continue;
                }
                // An object that is not textually an array may still be
                // array-like. The pinned API has no `isArrayLikeType`, so an
                // `Array` declaration cannot prove a rejection here.
                if *part == Actual::Object && targets.contains(&Target::Array) {
                    unknown = true;
                    continue;
                }
                return Verdict::Reject;
            }
        }
    }
    if unknown {
        Verdict::Unknown
    } else {
        Verdict::Accept
    }
}

/// Returns the final identifier of a free-root chain.
fn chain_target(expr: &Expr) -> Option<TextRange> {
    match expr.kind() {
        ExprKind::Identifier { name } => Some(*name),
        ExprKind::Member {
            object, property, ..
        } => {
            if !matches!(
                object.kind(),
                ExprKind::Identifier { .. } | ExprKind::Member { .. }
            ) {
                return None;
            }
            chain_target(property)
        }
        _ => None,
    }
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

fn is_quoted(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !matches!(first, '"' | '\'' | '`') {
        return false;
    }
    text.len() >= 2 && text.ends_with(first)
}

fn is_number_literal(text: &str) -> bool {
    let mut digits = 0_usize;
    let valid = text.chars().all(|character| {
        if character.is_ascii_digit() {
            digits += 1;
            true
        } else {
            matches!(character, '.' | '-' | '+' | 'e' | 'E')
        }
    });
    valid && digits > 0
}

fn is_array_text(text: &str) -> bool {
    text.ends_with("[]")
        || text.starts_with("Array<")
        || text.starts_with("ReadonlyArray<")
        || (text.starts_with('[') && text.ends_with(']'))
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
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Arc;

    use wxml_backend_tsgo::config::PinnedEngine;
    use wxml_backend_tsgo::native::strings::position_to_offset;
    use wxml_backend_tsgo::native::wire::RawType;
    use wxml_backend_tsgo::shadow::mapping::{
        MountRecord, ShadowMap, SourceRange as ShadowSourceRange,
    };
    use wxml_backend_tsgo::shadow::rewrite::{Assembled, MountedSegment};
    use wxml_project::ProjectPath;
    use wxml_types::{
        Backend, CheckFragment, ComponentUsage, DiagnosticsRequest, FileId, ProjectId,
        ProjectRevision, RequestContext, RootKind, SnapshotId, SourceRevision, SourceSnapshot,
        ViewRevision,
    };

    use super::*;
    use crate::backend_tsgo::TsgoBackendAdapter;
    use crate::backend_tsgo::input::{ScriptViewError, ViewProvider};
    use crate::project_host::{HostError, PreparedFile, ProjectHost, ProjectState, ViewTarget};

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
    const BEHAVIOR_PAGE_SCRIPT: &str = "Page({\n  data: { pageTitle: 'hello', total: 3, flag: true, list: [] as string[] },\n});\n";
    const BEHAVIOR_CARD_SCRIPT: &str = "Component({\n  behaviors: [Behavior({\n    properties: {\n      cardTitle: { type: String },\n      score: { type: String, optionalTypes: [Number], required: true, value: 1, observer() {} },\n      loose: { type: null },\n      items: { type: Array },\n      defaulted: { type: Number, value: \"not-a-number\" },\n    },\n  })],\n});\n";
    const CARD_SCRIPT: &str = "Component({\n  properties: {\n    title: { type: String },\n    count: { type: Number },\n  },\n});\n";
    const MATRIX_CARD_SCRIPT: &str = "Component({\n  properties: {\n    title: { type: String },\n    count: { type: Number },\n    active: { type: Boolean },\n    score: { type: String, optionalTypes: [Number] },\n    loose: { type: null },\n    items: { type: Array },\n    meta: { type: Object },\n    handler: { type: Function },\n  },\n});\n";
    const PLAIN_COMPONENT_SCRIPT: &str = "const shared = {};\nComponent({ properties: shared });\n";
    const SPREAD_COMPONENT_SCRIPT: &str = "const base = { title: { type: String } };\nComponent({ properties: { ...base, count: { type: Number } } });\n";
    const ALIAS_CARD_SCRIPT: &str = "import platform from \"@/behaviors/platform\";\nComponent({\n  behaviors: [platform],\n  properties: {\n    title: { type: String },\n  },\n});\n";
    const ALIAS_BEHAVIOR_SCRIPT: &str =
        "export default Behavior({\n  properties: {\n    score: { type: Number },\n  },\n});\n";

    type Answers = Vec<Option<(&'static str, u64)>>;

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
    fn typing_property(name: &str, type_text: &str, optional: bool) -> TypingProperty {
        let ty = TypeText::parse(type_text);
        TypingProperty {
            name: name.to_owned(),
            documentation: None,
            type_text: type_text.to_owned(),
            optional,
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
    ///
    /// Production fills `PreparedView::typing` from the project host; this
    /// helper keeps the unit tests focused on the binding comparison.
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
        mustache_attr_at(wxml, name, source, expression, 0).0
    }

    /// Builds one mustache attribute starting the search at `from` and returns
    /// the exclusive end of the attribute text so repeated attributes can be
    /// disambiguated.
    fn mustache_attr_at(
        wxml: &str,
        name: &str,
        source: &str,
        expression: &str,
        from: usize,
    ) -> (ComponentAttribute, usize) {
        let start = from + wxml[from..].find(source).expect("attribute text");
        let expression_start = start + source.find(expression).expect("expression text");
        (
            ComponentAttribute {
                name: name.to_owned(),
                value_kind: AttributeValueKind::Mustache,
                expr_text: Some(expression.to_owned()),
                expr_range: Some(source_range(
                    expression_start,
                    expression_start + expression.len(),
                )),
                attribute_range: source_range(start, start + source.len()),
            },
            start + source.len(),
        )
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

    /// Builds a layout with one verbatim mount per expression range.
    fn shadow(wxml: &str, expressions: &[SourceRange]) -> ShadowLayout {
        const PREFIX: &str = "__wxml_root.";
        let mut derived = String::from("let __wxml_root = {};\n");
        let mut fragments = Vec::new();
        for (index, range) in expressions.iter().enumerate() {
            let text = wxml
                .get(
                    usize::try_from(range.start()).expect("start")
                        ..usize::try_from(range.end()).expect("end"),
                )
                .expect("expression bytes");
            let start = utf16_len(&derived);
            let prefix = utf16_len(PREFIX);
            let length = utf16_len(text);
            let mount_text = format!("{PREFIX}{text}");
            derived.push_str(&mount_text);
            derived.push(';');
            let mount = MountRecord {
                mount_id: format!("fragment#{index}"),
                fragment_id: format!("f{index}"),
                insertion_id: "probe".to_owned(),
                text: mount_text,
                start_utf16: start,
                target_start_utf16: start + prefix,
                target_length_utf16: length,
                segments: vec![MountedSegment {
                    start_utf16: start + prefix,
                    length_utf16: length,
                    source: Some(ShadowSourceRange::new(range.start(), range.end())),
                }],
            };
            fragments.push(crate::backend_tsgo::mapping::FragmentMap {
                wxml_range: *range,
                mount,
            });
        }
        let assembled = Assembled {
            text: derived.clone(),
            edits: Vec::new(),
        };
        ShadowLayout::new(
            wxml.to_owned(),
            derived.clone(),
            ShadowMap::new(&derived, &assembled),
            fragments,
        )
    }

    fn run(
        layout: &ShadowLayout,
        view: &PreparedView,
        answers: Answers,
    ) -> (ComponentBindingDiagnostics, Vec<(String, Utf16Position)>) {
        let mut answers = VecDeque::from(answers);
        let mut calls = Vec::new();
        let result = diagnostics_with(layout, view, SCRIPT_PATH, |path, position| {
            calls.push((path.to_owned(), position));
            let answer = answers.pop_front().expect("unexpected type query");
            match answer {
                Some((text, flags)) => Ok(TypeQuery {
                    info: Some(RawType {
                        id: 1,
                        flags,
                        intrinsic_name: None,
                    }),
                    text: Some(text.to_owned()),
                }),
                None => Ok(TypeQuery {
                    info: None,
                    text: None,
                }),
            }
        });
        (result, calls)
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

    /// One attribute in the wave-6 `componentBindings.test.ts` matrix.
    #[derive(Clone, Copy)]
    enum MatrixValue {
        Mustache(&'static str),
        /// A mustache whose recorded range is deliberately not mounted.
        UnmountedMustache(&'static str, usize, usize),
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
        ranges: Vec<SourceRange>,
    }

    fn matrix_attribute(name: &'static str, value: MatrixValue) -> MatrixAttribute {
        MatrixAttribute { name, value }
    }

    /// Builds the same usage set as the TS per-tag component-binding case.
    ///
    /// The WXML text is generated together with the byte ranges so every
    /// `ComponentAttribute` is a real slice of the template, exactly as the
    /// template analyzer would publish it.
    fn matrix_fixture() -> MatrixFixture {
        matrix_fixture_from(matrix_specs())
    }

    fn matrix_specs() -> Vec<MatrixUsage> {
        vec![
            MatrixUsage {
                tag: "valid",
                component: Some(CARD_PATH),
                attributes: vec![
                    matrix_attribute("title", MatrixValue::Mustache("pageTitle")),
                    matrix_attribute("count", MatrixValue::Mustache("total")),
                    matrix_attribute("active", MatrixValue::Mustache("flag")),
                    matrix_attribute("score", MatrixValue::Mustache("pageTitle")),
                ],
            },
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
                tag: "bad-mustache",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("title", MatrixValue::Mustache("total"))],
            },
            MatrixUsage {
                tag: "bad-union",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("score", MatrixValue::Mustache("flag"))],
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
                tag: "any-prop",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("loose", MatrixValue::Mustache("total"))],
            },
            MatrixUsage {
                tag: "function-prop",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute(
                    "handler",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "array-ok",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("items", MatrixValue::Mustache("list"))],
            },
            MatrixUsage {
                tag: "array-bad",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute(
                    "items",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "object-ok",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("meta", MatrixValue::Mustache("meta"))],
            },
            MatrixUsage {
                tag: "object-bad",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute("meta", MatrixValue::Mustache("total"))],
            },
            MatrixUsage {
                tag: "undeclared",
                component: Some(CARD_PATH),
                attributes: vec![
                    matrix_attribute("title", MatrixValue::Mustache("pageTitle")),
                    matrix_attribute("class", MatrixValue::Literal("x")),
                    matrix_attribute("unknown", MatrixValue::Mustache("total")),
                ],
            },
            MatrixUsage {
                tag: "mixed",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mixed("Hi {{ total }}"),
                )],
            },
            MatrixUsage {
                tag: "missing-component",
                component: Some("missing.ts"),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "no-properties",
                component: Some(PLAIN_PATH),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "incomplete-properties",
                component: Some(SPREAD_PATH),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "no-file-id",
                component: None,
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::Mustache("pageTitle"),
                )],
            },
            MatrixUsage {
                tag: "no-match",
                component: Some(CARD_PATH),
                attributes: vec![matrix_attribute(
                    "title",
                    MatrixValue::UnmountedMustache("total", 9999, 10_005),
                )],
            },
        ]
    }

    /// Builds the WXML text, real byte ranges and usages for a spec set.
    ///
    /// The WXML text is generated together with the byte ranges so every
    /// `ComponentAttribute` is a real slice of the template, exactly as the
    /// template analyzer would publish it.
    fn matrix_fixture_from(specs: Vec<MatrixUsage>) -> MatrixFixture {
        let mut wxml = String::from("<view>");
        let mut usages = Vec::new();
        let mut ranges = Vec::new();
        for spec in specs {
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
                        ranges.push(range);
                        (
                            AttributeValueKind::Mustache,
                            Some(expr.to_owned()),
                            Some(range),
                        )
                    }
                    MatrixValue::UnmountedMustache(expr, start, end) => {
                        wxml.push_str("=\"{{ ");
                        wxml.push_str(expr);
                        wxml.push_str(" }}\"");
                        (
                            AttributeValueKind::Mustache,
                            Some(expr.to_owned()),
                            Some(source_range(start, end)),
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
        MatrixFixture {
            wxml,
            usages,
            ranges,
        }
    }

    /// Builds the `TS-COMPONENT-0292` usage set from the Node oracle.
    ///
    /// The component declares its properties inside an inline `Behavior`, so
    /// the pass can only see them through the behavior overlay.
    fn behavior_binding_specs() -> Vec<MatrixUsage> {
        vec![
            MatrixUsage {
                tag: "behavior-ok",
                component: Some(BEHAVIOR_CARD_PATH),
                attributes: vec![
                    matrix_attribute("card-title", MatrixValue::Mustache("pageTitle")),
                    matrix_attribute("score", MatrixValue::Mustache("total")),
                    matrix_attribute("items", MatrixValue::Mustache("list")),
                    matrix_attribute("defaulted", MatrixValue::Mustache("total")),
                    // Undeclared: never a property and never queried.
                    matrix_attribute("unknown", MatrixValue::Mustache("total")),
                ],
            },
            MatrixUsage {
                tag: "behavior-bad",
                component: Some(BEHAVIOR_CARD_PATH),
                attributes: vec![
                    matrix_attribute("card-title", MatrixValue::Mustache("total")),
                    matrix_attribute("score", MatrixValue::Mustache("flag")),
                    // A string literal is accepted by `string | number`.
                    matrix_attribute("score", MatrixValue::Literal("literal")),
                    matrix_attribute("defaulted", MatrixValue::Literal("literal")),
                ],
            },
            MatrixUsage {
                tag: "behavior-loose",
                component: Some(BEHAVIOR_CARD_PATH),
                attributes: vec![matrix_attribute("loose", MatrixValue::Mustache("total"))],
            },
            MatrixUsage {
                tag: "behavior-missing-required",
                component: Some(BEHAVIOR_CARD_PATH),
                attributes: Vec::new(),
            },
        ]
    }

    fn behavior_binding_fixture() -> MatrixFixture {
        matrix_fixture_from(behavior_binding_specs())
    }

    fn behavior_binding_files() -> Vec<PreparedFile> {
        vec![
            prepared_file(SCRIPT_PATH, BEHAVIOR_PAGE_SCRIPT),
            prepared_file(BEHAVIOR_CARD_PATH, BEHAVIOR_CARD_SCRIPT),
        ]
    }

    fn matrix_files() -> Vec<PreparedFile> {
        vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, MATRIX_CARD_SCRIPT),
            prepared_file(PLAIN_PATH, PLAIN_COMPONENT_SCRIPT),
            prepared_file(SPREAD_PATH, SPREAD_COMPONENT_SCRIPT),
        ]
    }

    fn matrix_answers() -> Answers {
        vec![
            Some(("string", 0)),
            Some(("number", 0)),
            Some(("boolean", 0)),
            Some(("string", 0)),
            Some(("number", 0)),
            Some(("boolean", 0)),
            Some(("string[]", 0)),
            Some(("string", 0)),
            Some(("{ a: string }", 0)),
            Some(("number", 0)),
            Some(("string", 0)),
        ]
    }

    fn run_matrix(
        fixture: &MatrixFixture,
    ) -> (ComponentBindingDiagnostics, Vec<(String, Utf16Position)>) {
        let layout = shadow(&fixture.wxml, &fixture.ranges);
        let view = build_view(matrix_files(), fixture.usages.clone());
        run(&layout, &view, matrix_answers())
    }

    fn run_matrix_usage(
        fixture: &MatrixFixture,
        tag: &str,
        answers: Answers,
    ) -> (ComponentBindingDiagnostics, Vec<(String, Utf16Position)>) {
        run_usage_with_files(fixture, matrix_files(), tag, answers)
    }

    /// Runs one usage of `fixture` against an explicit file set.
    ///
    /// Native has no per-tag response array, so each usage is pinned
    /// separately, exactly like the `TS-COMPONENT-0147` precedent.
    fn run_usage_with_files(
        fixture: &MatrixFixture,
        files: Vec<PreparedFile>,
        tag: &str,
        answers: Answers,
    ) -> (ComponentBindingDiagnostics, Vec<(String, Utf16Position)>) {
        let usage = fixture
            .usages
            .iter()
            .find(|usage| usage.tag == tag)
            .expect("matrix usage")
            .clone();
        let layout = shadow(&fixture.wxml, &fixture.ranges);
        let view = build_view(files, vec![usage]);
        run(&layout, &view, answers)
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

    #[test]
    fn matching_root_binding_is_clean() {
        let wxml = r#"<view><card title="{{ pageTitle }}" count="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ pageTitle }}""#, "pageTitle");
        let count = mustache_attr(wxml, "count", r#"count="{{ total }}""#, "total");
        let layout = shadow(
            wxml,
            &[
                title.expr_range.expect("title expr"),
                count.expr_range.expect("count expr"),
            ],
        );
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title, count])],
        );
        let (result, calls) = run(
            &layout,
            &view,
            vec![Some(("string", 0)), Some(("number", 0))],
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(path, _)| path == SCRIPT_PATH));
    }

    /// 别名导入的 behavior 需经 tsconfig `paths` 解析后才贡献属性。
    ///
    /// 未挂 resolver 时 behavior 解析失败、整个组件被 skip；这里断言 behavior
    /// 声明与自身声明都参与校验，且没有 Partial。
    #[test]
    fn aliased_behavior_properties_are_checked() {
        let wxml = r#"<view><alias-card title="{{ total }}" score="{{ pageTitle }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let score = mustache_attr(wxml, "score", r#"score="{{ pageTitle }}""#, "pageTitle");
        let layout = shadow(
            wxml,
            &[
                title.expr_range.expect("title expr"),
                score.expr_range.expect("score expr"),
            ],
        );
        let mut view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(ALIAS_CARD_PATH, ALIAS_CARD_SCRIPT),
                prepared_file(ALIAS_BEHAVIOR_PATH, ALIAS_BEHAVIOR_SCRIPT),
            ],
            vec![usage(
                "alias-card",
                Some(ALIAS_CARD_PATH),
                vec![title.clone(), score.clone()],
            )],
        );
        view.compiler_options = alias_options();
        let (result, calls) = run(
            &layout,
            &view,
            vec![Some(("number", 0)), Some(("string", 0))],
        );
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(result.status.is_none(), "{:?}", result.status);
        let actual = result
            .diagnostics
            .iter()
            .map(|diagnostic| {
                assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
                (diagnostic.range, diagnostic.message.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            vec![
                (
                    title.expr_range.expect("title expr"),
                    "component property \"title\" expects string, got number (bound expression \"total\")",
                ),
                (
                    score.expr_range.expect("score expr"),
                    "component property \"score\" expects number, got string (bound expression \"pageTitle\")",
                ),
            ]
        );
    }

    /// 别名目标缺失时回退到组件自身声明，未声明属性不得报错。
    #[test]
    fn unresolved_aliased_behavior_falls_back_to_own_properties() {
        let wxml = r#"<view><alias-card title="{{ total }}" score="{{ pageTitle }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let score = mustache_attr(wxml, "score", r#"score="{{ pageTitle }}""#, "pageTitle");
        let layout = shadow(
            wxml,
            &[
                title.expr_range.expect("title expr"),
                score.expr_range.expect("score expr"),
            ],
        );
        let mut view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(ALIAS_CARD_PATH, ALIAS_CARD_SCRIPT),
            ],
            vec![usage(
                "alias-card",
                Some(ALIAS_CARD_PATH),
                vec![title.clone(), score],
            )],
        );
        view.compiler_options = alias_options();
        let (result, calls) = run(&layout, &view, vec![Some(("number", 0))]);
        // 只有自身声明的 `title` 被查询；`score` 可能来自 behavior，直接忽略。
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, title.expr_range.expect("expr"));
        assert_eq!(
            result.diagnostics[0].message,
            "component property \"title\" expects string, got number (bound expression \"total\")"
        );
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
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        let mut view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
        );
        view.compiler_options = alias_options();
        let (result, calls) = run(&layout, &view, vec![Some(("number", 0))]);
        assert_eq!(calls.len(), 1);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, title.expr_range.expect("expr"));
        assert!(
            result.diagnostics[0].message.contains("expects string"),
            "{}",
            result.diagnostics[0].message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn mismatched_root_binding_is_an_error_at_the_expression() {
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
        );
        let (result, _) = run(&layout, &view, vec![Some(("number", 0))]);
        assert!(result.status.is_none(), "{:?}", result.status);
        assert_eq!(result.diagnostics.len(), 1);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, title.expr_range.expect("expr"));
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert!(diagnostic.message.contains("component property \"title\""));
        assert!(diagnostic.message.contains("expects string"));
        assert!(diagnostic.message.contains("got number"));
        assert!(diagnostic.message.contains("bound expression \"total\""));
    }

    #[test]
    fn declared_union_accepts_each_member() {
        let script = "Component({\n  properties: {\n    title: { type: String, optionalTypes: [Number] },\n  },\n});\n";
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, script),
        ];
        for (actual, clean) in [("string", true), ("number", true), ("boolean", false)] {
            let view = build_view(
                files.clone(),
                vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
            );
            let (result, _) = run(&layout, &view, vec![Some((actual, 0))]);
            if clean {
                assert!(
                    result.diagnostics.is_empty(),
                    "{actual}: {:?}",
                    result.diagnostics
                );
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{actual}");
                assert!(
                    result.diagnostics[0]
                        .message
                        .contains("expects string | number"),
                    "{}",
                    result.diagnostics[0].message
                );
            }
        }
    }

    #[test]
    fn literal_and_boolean_checks_use_the_attribute_range() {
        let wxml = r#"<view><card count="3" title disabled /></view>"#;
        let count = literal_attr(wxml, "count", r#"count="3""#);
        let title = boolean_attr(wxml, "title", "title");
        let disabled = boolean_attr(wxml, "disabled", "disabled");
        let layout = shadow(wxml, &[]);
        let view = build_view(
            page_and_card(),
            vec![usage(
                "card",
                Some(CARD_PATH),
                vec![count.clone(), title.clone(), disabled],
            )],
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert_eq!(result.diagnostics.len(), 2);
        let count_error = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range == count.attribute_range)
            .expect("literal diagnostic");
        assert!(
            count_error.message.contains("got string literal"),
            "{}",
            count_error.message
        );
        let title_error = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range == title.attribute_range)
            .expect("boolean diagnostic");
        assert!(
            title_error.message.contains("got boolean literal"),
            "{}",
            title_error.message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn component_binding_matrix_matches_the_typescript_oracle() {
        let fixture = matrix_fixture();
        let (result, calls) = run_matrix(&fixture);
        assert_eq!(calls.len(), 11, "{calls:?}");
        assert_eq!(result.diagnostics.len(), 6, "{:?}", result.diagnostics);

        let expected = [
            (
                "bad-mustache",
                "title",
                "component property \"title\" expects string, got number (bound expression \"total\")",
            ),
            (
                "bad-union",
                "score",
                "component property \"score\" expects string | number, got boolean (bound expression \"flag\")",
            ),
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
                "array-bad",
                "items",
                "component property \"items\" expects any[], got string (bound expression \"pageTitle\")",
            ),
            (
                "object-bad",
                "meta",
                "component property \"meta\" expects Record<string, any>, got number (bound expression \"total\")",
            ),
        ];
        for (tag, name, message) in expected {
            let attribute = matrix_attribute_of(&fixture, tag, name);
            let range = attribute.expr_range.unwrap_or(attribute.attribute_range);
            let diagnostic = result
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.range == range)
                .unwrap_or_else(|| panic!("{tag}.{name}: {:?}", result.diagnostics));
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

        // Every tag that TS reports with an empty diagnostic list is checked
        // and clean; a native skip would turn the status Partial, so a None
        // status proves the usage was not silently dropped.
        let clean: Vec<(&str, Answers, usize)> = vec![
            (
                "valid",
                vec![
                    Some(("string", 0)),
                    Some(("number", 0)),
                    Some(("boolean", 0)),
                    Some(("string", 0)),
                ],
                4,
            ),
            ("valid-literal", Vec::new(), 0),
            ("valid-boolean", Vec::new(), 0),
            ("undeclared", vec![Some(("string", 0))], 1),
            ("array-ok", vec![Some(("string[]", 0))], 1),
            ("object-ok", vec![Some(("{ a: string }", 0))], 1),
        ];
        for (tag, answers, queries) in clean {
            let (clean_result, clean_calls) = run_matrix_usage(&fixture, tag, answers);
            assert!(
                clean_result.diagnostics.is_empty(),
                "{tag}: {:?}",
                clean_result.diagnostics
            );
            assert!(
                clean_result.status.is_none(),
                "{tag}: {:?}",
                clean_result.status
            );
            assert_eq!(clean_calls.len(), queries, "{tag}");
        }

        // The eight skipped usages must not become clean results. Native has
        // no per-tag response array, so the exact coverage gap is pinned for
        // each usage separately.
        let skipped = vec![
            ("any-prop", "any-prop.loose"),
            ("function-prop", "function-prop.handler"),
            ("mixed", "mixed.title"),
            ("missing-component", "missing-component"),
            ("no-properties", "no-properties"),
            ("incomplete-properties", "incomplete-properties"),
            ("no-file-id", "no-file-id"),
            ("no-match", "no-match.title"),
        ];
        for (tag, detail) in skipped {
            let (skipped_result, skipped_calls) = run_matrix_usage(&fixture, tag, Vec::new());
            assert!(skipped_calls.is_empty(), "{tag}");
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
    fn fully_checkable_component_request_stays_supported() {
        // Oracle: TS-COMPONENT-0193. The adapter overall status and the Node
        // `probeProblems` array have no native counterpart; this pins the
        // binding pass: Supported (status None), zero diagnostics, three real
        // queries at the three mounted expression ranges, no skipped check.
        let wxml = r#"<view><valid title="{{ pageTitle }}" count="{{ total }}" items="{{ list }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ pageTitle }}""#, "pageTitle");
        let count = mustache_attr(wxml, "count", r#"count="{{ total }}""#, "total");
        let items = mustache_attr(wxml, "items", r#"items="{{ list }}""#, "list");
        let ranges = [
            title.expr_range.expect("title"),
            count.expr_range.expect("count"),
            items.expr_range.expect("items"),
        ];
        let layout = shadow(wxml, &ranges);
        assert_eq!(layout.fragment_count(), 3);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, MATRIX_CARD_SCRIPT),
        ];
        let view = build_view(
            files,
            vec![usage("valid", Some(CARD_PATH), vec![title, count, items])],
        );
        let (result, calls) = run(
            &layout,
            &view,
            vec![
                Some(("string", 0)),
                Some(("number", 0)),
                Some(("string[]", 0)),
            ],
        );
        assert_eq!(calls.len(), 3);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);

        // Every query goes to the owning script at the exact mount target, in
        // template order: the clean result is not a skipped usage.
        let expected = ranges
            .iter()
            .map(|range| {
                let mount = layout.mount_for_range(*range).expect("mount");
                offset_to_position(layout.derived(), Utf16Offset::new(mount.target_start_utf16))
                    .expect("position")
            })
            .collect::<Vec<_>>();
        let positions = calls
            .iter()
            .map(|(path, position)| {
                assert_eq!(path, SCRIPT_PATH);
                *position
            })
            .collect::<Vec<_>>();
        assert_eq!(positions, expected);
    }

    /// Oracle: TS-COMPONENT-0292.
    ///
    /// The component declares its properties inside an inline `Behavior`.
    /// Production merges them into the property overlay, and this pins every
    /// TS assertion per usage: the clean usage, the three exact mismatch
    /// messages at the expression/attribute ranges, the single
    /// `behavior-loose.loose` skip and the zero-attribute usage. Native has no
    /// per-tag response array, so the per-usage form follows the exact
    /// `TS-COMPONENT-0147` precedent.
    #[test]
    fn case_component_0292_behavior_properties_and_kebab_camel_bindings_are_checked() {
        let fixture = behavior_binding_fixture();
        let files = behavior_binding_files();

        let (ok, ok_calls) = run_usage_with_files(
            &fixture,
            files.clone(),
            "behavior-ok",
            vec![
                Some(("string", 0)),
                Some(("number", 0)),
                Some(("string[]", 0)),
                Some(("number", 0)),
            ],
        );
        assert!(ok.diagnostics.is_empty(), "{:?}", ok.diagnostics);
        assert!(ok.status.is_none(), "{:?}", ok.status);
        assert_eq!(ok_calls.len(), 4, "{ok_calls:?}");
        assert!(ok_calls.iter().all(|(path, _)| path == SCRIPT_PATH));
        // The undeclared `unknown` attribute is not a property and is never
        // queried; the four declared attributes are checked.
        assert_eq!(
            fixture
                .usages
                .iter()
                .find(|usage| usage.tag == "behavior-ok")
                .expect("usage")
                .attributes
                .len(),
            5
        );

        let (bad, bad_calls) = run_usage_with_files(
            &fixture,
            files.clone(),
            "behavior-bad",
            vec![Some(("number", 0)), Some(("boolean", 0))],
        );
        assert!(bad.status.is_none(), "{:?}", bad.status);
        assert_eq!(bad_calls.len(), 2, "{bad_calls:?}");
        let card_title = matrix_attribute_of(&fixture, "behavior-bad", "card-title");
        let score = matrix_attribute_of(&fixture, "behavior-bad", "score");
        let defaulted = matrix_attribute_of(&fixture, "behavior-bad", "defaulted");
        let mut expected = vec![
            (
                card_title.expr_range.expect("card-title expression"),
                "component property \"card-title\" expects string, got number (bound expression \"total\")",
            ),
            (
                score.expr_range.expect("score expression"),
                "component property \"score\" expects string | number, got boolean (bound expression \"flag\")",
            ),
            (
                defaulted.attribute_range,
                "component property \"defaulted\" expects number, got string literal",
            ),
        ];
        expected.sort_by_key(|(range, _)| range.start());
        let actual = bad
            .diagnostics
            .iter()
            .map(|diagnostic| {
                assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
                (diagnostic.range, diagnostic.message.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);

        let (loose, loose_calls) =
            run_usage_with_files(&fixture, files.clone(), "behavior-loose", Vec::new());
        assert!(loose_calls.is_empty(), "{loose_calls:?}");
        assert!(loose.diagnostics.is_empty(), "{:?}", loose.diagnostics);
        let CapabilityStatus::Partial { reason } = loose
            .status
            .as_ref()
            .expect("behavior-loose must be Partial")
        else {
            panic!("expected Partial");
        };
        // `skipped == 1` and `skippedDetails == ["behavior-loose.loose"]`.
        assert_eq!(reason, &format!("{SKIPPED_PREFIX} (behavior-loose.loose)"));

        let (missing, missing_calls) =
            run_usage_with_files(&fixture, files, "behavior-missing-required", Vec::new());
        assert!(missing_calls.is_empty(), "{missing_calls:?}");
        assert!(missing.diagnostics.is_empty(), "{:?}", missing.diagnostics);
        assert!(
            missing.status.is_none(),
            "a zero-attribute usage is checked and clean: {:?}",
            missing.status
        );
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
        let layout = shadow(wxml, &[]);
        assert_eq!(layout.fragment_count(), 0);
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
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
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
    fn optional_named_union_members_remain_partial_not_guessed() {
        let wxml = r#"<view><card meta="{{ maybeUser }}" /><card items="{{ maybeList }}" /><card count="{{ nothing }}" /><card title="{{ maybeUser }}" /></view>"#;
        let (object, next) = mustache_attr_at(
            wxml,
            "meta",
            r#"<card meta="{{ maybeUser }}" />"#,
            "maybeUser",
            0,
        );
        let (array, next) = mustache_attr_at(
            wxml,
            "items",
            r#"<card items="{{ maybeList }}" />"#,
            "maybeList",
            next,
        );
        let (absent, next) = mustache_attr_at(
            wxml,
            "count",
            r#"<card count="{{ nothing }}" />"#,
            "nothing",
            next,
        );
        let (mismatch, _) = mustache_attr_at(
            wxml,
            "title",
            r#"<card title="{{ maybeUser }}" />"#,
            "maybeUser",
            next,
        );
        let ranges = [
            object.expr_range.expect("object"),
            array.expr_range.expect("array"),
            absent.expr_range.expect("absent"),
            mismatch.expr_range.expect("mismatch"),
        ];
        let layout = shadow(wxml, &ranges);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, MATRIX_CARD_SCRIPT),
        ];
        let view = build_view(
            files,
            vec![
                usage("optional-object", Some(CARD_PATH), vec![object]),
                usage("optional-array", Some(CARD_PATH), vec![array]),
                usage("absent", Some(CARD_PATH), vec![absent]),
                usage("optional-mismatch", Some(CARD_PATH), vec![mismatch]),
            ],
        );
        let (result, calls) = run(
            &layout,
            &view,
            vec![
                Some(("User | undefined", 0)),
                Some(("Item[] | undefined", 0)),
                Some(("undefined", 0)),
                Some(("User | undefined", 0)),
            ],
        );
        assert_eq!(calls.len(), 4);
        // `Item[] | undefined` and pure `undefined` are proven clean, but a
        // named `User` constituent carries no structural flag on the pinned
        // wire. TS expects it to satisfy Object and to reject String; native
        // must stay Partial instead of guessing.
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial");
        };
        assert_eq!(
            reason,
            "some component bindings could not be resolved and were not checked (optional-object.meta, optional-mismatch.title)"
        );
    }

    #[test]
    fn component_without_properties_member_is_skipped_not_guessed() {
        let wxml = r#"<view><bare title="{{ pageTitle }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ pageTitle }}""#, "pageTitle");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        let view = build_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(BARE_PATH, "Component({ data: { state: 'idle' } });\n"),
            ],
            vec![usage("bare", Some(BARE_PATH), vec![title])],
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial");
        };
        assert_eq!(reason, &format!("{SKIPPED_PREFIX} (bare)"));
        assert!(!reason.contains('/'), "{reason}");
    }

    #[test]
    fn component_diagnostics_never_leak_a_path() {
        let fixture = matrix_fixture();
        let (result, _) = run_matrix(&fixture);
        assert_eq!(result.diagnostics.len(), 6, "{:?}", result.diagnostics);
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
    fn undeclared_and_event_attributes_are_ignored() {
        let wxml = r#"<view><card class="x" style="y" bind:tap="onTap" bindtap="onTap" /></view>"#;
        let attributes = vec![
            literal_attr(wxml, "class", r#"class="x""#),
            literal_attr(wxml, "style", r#"style="y""#),
            literal_attr(wxml, "bind:tap", r#"bind:tap="onTap""#),
            literal_attr(wxml, "bindtap", r#"bindtap="onTap""#),
        ];
        let layout = shadow(wxml, &[]);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), attributes)],
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn mixed_binding_is_partial_not_an_error() {
        let wxml = r#"<view><card title="Hi {{ total }}" /></view>"#;
        let title = mixed_attr(wxml, "title", r#"title="Hi {{ total }}""#);
        let layout = shadow(wxml, &[]);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title])],
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);
    }

    #[test]
    fn missing_component_or_incomplete_properties_is_partial() {
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);

        let mut missing = build_view(
            page_and_card(),
            vec![usage("card", None, vec![title.clone()])],
        );
        missing.components = Arc::from(vec![usage("card", None, vec![title.clone()])]);
        let (result, calls) = run(&layout, &missing, Vec::new());
        assert!(calls.is_empty());
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
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);

        let nonliteral =
            "const options = { properties: { title: { type: String } } };\nComponent(options);\n";
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, nonliteral),
        ];
        let view = build_view(files, vec![usage("card", Some(CARD_PATH), vec![title])]);
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty());
        assert_partial(&result);
    }

    #[test]
    fn any_unknown_and_missing_types_are_partial() {
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        for answer in [Some(("any", 0)), Some(("unknown", 0)), None] {
            let view = build_view(
                page_and_card(),
                vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
            );
            let (result, calls) = run(&layout, &view, vec![answer]);
            assert_eq!(calls.len(), 1);
            assert!(result.diagnostics.is_empty());
            assert_partial(&result);
        }
    }

    #[test]
    fn two_components_with_the_same_property_keep_their_own_types() {
        let badge_script = "Component({ properties: { title: { type: Number } } });\n";
        let wxml = r#"<view><card title="{{ total }}" /><badge title="{{ total }}" /></view>"#;
        let card = mustache_attr(wxml, "title", r#"<card title="{{ total }}" />"#, "total");
        let badge = mustache_attr(wxml, "title", r#"<badge title="{{ total }}" />"#, "total");
        let layout = shadow(
            wxml,
            &[
                card.expr_range.expect("card expr"),
                badge.expr_range.expect("badge expr"),
            ],
        );
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, CARD_SCRIPT),
            prepared_file(BADGE_PATH, badge_script),
        ];
        let view = build_view(
            files,
            vec![
                usage("card", Some(CARD_PATH), vec![card.clone()]),
                usage("badge", Some(BADGE_PATH), vec![badge]),
            ],
        );
        let (result, calls) = run(
            &layout,
            &view,
            vec![Some(("number", 0)), Some(("number", 0))],
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].range,
            card.expr_range.expect("card expr")
        );
        assert!(result.diagnostics[0].message.contains("expects string"));
    }

    #[test]
    fn cjk_crlf_expression_range_is_exact() {
        let wxml = "<view>标题😀\r\n<card title=\"{{ total }}\" /></view>";
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let range = title.expr_range.expect("expr");
        assert_eq!(
            wxml.get(range.start() as usize..range.end() as usize),
            Some("total")
        );
        let layout = shadow(wxml, &[range]);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title])],
        );
        let (result, _) = run(&layout, &view, vec![Some(("number", 0))]);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].range, range);
    }

    #[test]
    fn duplicate_expression_uses_the_exact_expression_range() {
        let wxml = r#"<view><card title="{{ total }}" /><card title="{{ total }}" /></view>"#;
        let source = r#"<card title="{{ total }}" />"#;
        let (first, next) = mustache_attr_at(wxml, "title", source, "total", 0);
        let (second, _) = mustache_attr_at(wxml, "title", source, "total", next);
        assert_ne!(first.expr_range, second.expr_range);
        let layout = shadow(
            wxml,
            &[
                first.expr_range.expect("first expr"),
                second.expr_range.expect("second expr"),
            ],
        );
        let view = build_view(
            page_and_card(),
            vec![
                usage("card", Some(CARD_PATH), vec![first]),
                usage("card", Some(CARD_PATH), vec![second.clone()]),
            ],
        );
        let (result, calls) = run(
            &layout,
            &view,
            vec![Some(("string", 0)), Some(("boolean", 0))],
        );
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].1, calls[1].1);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].range,
            second.expr_range.expect("second expr")
        );
        assert!(result.diagnostics[0].message.contains("got boolean"));
    }

    #[test]
    fn caps_and_dedup() {
        let wxml = r#"<view><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /><card title="{{ total }}" /></view>"#;
        let mut usages = Vec::new();
        let mut ranges = Vec::new();
        let source = r#"<card title="{{ total }}" />"#;
        let mut search_from = 0;
        for _ in 0..10 {
            let (attribute, next) = mustache_attr_at(wxml, "title", source, "total", search_from);
            ranges.push(attribute.expr_range.expect("expr"));
            usages.push(usage("card", Some(CARD_PATH), vec![attribute]));
            search_from = next;
        }
        let layout = shadow(wxml, &ranges);
        let view = build_view(page_and_card(), usages);
        let answers = (0..10).map(|_| Some(("number", 0))).collect();
        let (result, calls) = run(&layout, &view, answers);
        assert_eq!(calls.len(), 10);
        assert_eq!(result.diagnostics.len(), MAX_DIAGNOSTICS_PER_TAG);
        assert_partial(&result);

        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let attribute = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[attribute.expr_range.expect("expr")]);
        let view = build_view(
            page_and_card(),
            vec![usage(
                "card",
                Some(CARD_PATH),
                vec![attribute.clone(), attribute],
            )],
        );
        let (result, _) = run(
            &layout,
            &view,
            vec![Some(("number", 0)), Some(("number", 0))],
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn complex_expressions_degrade_to_partial() {
        for (wxml, source, expression) in [
            (
                r#"<view><card title="{{ total + 1 }}" /></view>"#,
                r#"title="{{ total + 1 }}""#,
                "total + 1",
            ),
            (
                r#"<view><card title="{{ format(total) }}" /></view>"#,
                r#"title="{{ format(total) }}""#,
                "format(total)",
            ),
        ] {
            let attribute = mustache_attr(wxml, "title", source, expression);
            let layout = shadow(wxml, &[attribute.expr_range.expect("expr")]);
            let view = build_view(
                page_and_card(),
                vec![usage("card", Some(CARD_PATH), vec![attribute])],
            );
            let (result, calls) = run(&layout, &view, Vec::new());
            assert!(calls.is_empty(), "{expression}");
            assert!(result.diagnostics.is_empty(), "{expression}");
            assert_partial(&result);
        }
    }

    #[test]
    fn a_member_chain_queries_the_final_identifier() {
        let wxml = r#"<view><card title="{{ user.name }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ user.name }}""#, "user.name");
        let range = title.expr_range.expect("expr");
        let layout = shadow(wxml, &[range]);
        let expected = layout
            .derived_offset(
                range.start() + u32::try_from("user.".len()).expect("offset"),
                false,
            )
            .expect("final identifier");
        let expected =
            offset_to_position(layout.derived(), Utf16Offset::new(expected)).expect("position");
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title])],
        );
        let (result, calls) = run(&layout, &view, vec![Some(("string", 0))]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, expected);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn array_and_object_targets_are_conservative() {
        let script =
            "Component({ properties: { items: { type: Array }, options: { type: Object } } });\n";
        let wxml = r#"<view><card items="{{ state }}" /></view>"#;
        let items = mustache_attr(wxml, "items", r#"items="{{ state }}""#, "state");
        let layout = shadow(wxml, &[items.expr_range.expect("expr")]);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, script),
        ];
        // An object is not provably non-array: Partial, not an error.
        let view = build_view(
            files.clone(),
            vec![usage("card", Some(CARD_PATH), vec![items.clone()])],
        );
        let (result, _) = run(&layout, &view, vec![Some(("{ a: 1 }", 0))]);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_partial(&result);
        // A textual array is provable.
        let view = build_view(
            files.clone(),
            vec![usage("card", Some(CARD_PATH), vec![items])],
        );
        let (result, _) = run(&layout, &view, vec![Some(("number[]", 0))]);
        assert!(result.diagnostics.is_empty());
        assert!(result.status.is_none(), "{:?}", result.status);

        let wxml = r#"<view><card options="{{ total }}" /></view>"#;
        let options = mustache_attr(wxml, "options", r#"options="{{ total }}""#, "total");
        let layout = shadow(wxml, &[options.expr_range.expect("expr")]);
        let view = build_view(
            files,
            vec![usage("card", Some(CARD_PATH), vec![options.clone()])],
        );
        let (result, _) = run(&layout, &view, vec![Some(("number", 0))]);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].range,
            options.expr_range.expect("expr")
        );
        assert!(
            result.diagnostics[0]
                .message
                .contains("expects Record<string, any>")
        );
    }

    /// End-to-end pass over the real `component-bindings` fixture: the real
    /// host prepares `ComponentUsage`/fragments, and the pass publishes exactly
    /// the second card's `title="{{ total }}"` error.
    #[test]
    fn fixture_component_usages_publish_one_property_error() {
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

        let ranges: Vec<SourceRange> = view.fragments.iter().map(|item| item.wxml_range).collect();
        let layout = shadow(&text, &ranges);
        let mut calls = 0_usize;
        let result = diagnostics_with(&layout, &view, SCRIPT_PATH, |path, position| {
            assert_eq!(path, SCRIPT_PATH);
            calls += 1;
            let offset = position_to_offset(layout.derived(), position).expect("derived offset");
            let range = match layout
                .map_native_range(offset.get(), offset.get() + 1)
                .expect("identifier range")
            {
                crate::backend_tsgo::mapping::NativeRange::Template { range, .. } => range,
                crate::backend_tsgo::mapping::NativeRange::Script { .. } => {
                    panic!("component query position is not in a mount")
                }
            };
            let name_start = usize::try_from(range.start()).expect("name start");
            let mut name_end = name_start;
            while let Some(character) = text[name_end..].chars().next() {
                if character.is_alphanumeric() || matches!(character, '_' | '$') {
                    name_end += character.len_utf8();
                } else {
                    break;
                }
            }
            let name = text.get(name_start..name_end).expect("identifier bytes");
            let answer = match name {
                "pageTitle" => "string",
                "total" => "number",
                other => panic!("unexpected component query for {other}"),
            };
            Ok(TypeQuery {
                info: Some(RawType {
                    id: 1,
                    flags: 0,
                    intrinsic_name: None,
                }),
                text: Some(answer.to_owned()),
            })
        });
        assert_eq!(
            calls, 3,
            "two first-card attributes and the mismatched title"
        );
        assert!(result.status.is_none(), "{:?}", result.status);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        let second_total = text.rfind(r#"title="{{ total }}""#).expect("second title");
        let expr_start = second_total + r#"title="{{ "#.len();
        assert_eq!(
            diagnostic.range,
            source_range(expr_start, expr_start + "total".len())
        );
        assert!(diagnostic.message.contains("component property \"title\""));
        assert!(diagnostic.message.contains("expects string, got number"));
        assert!(diagnostic.message.contains("bound expression \"total\""));
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

        let ranges: Vec<SourceRange> = view.fragments.iter().map(|item| item.wxml_range).collect();
        let layout = shadow(&text, &ranges);
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
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

        let ranges: Vec<SourceRange> = view.fragments.iter().map(|item| item.wxml_range).collect();
        let layout = shadow(&text, &ranges);
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        // Script fallback: `type` matches `String` and an undeclared `size` is
        // ignored exactly as before declarations existed.
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_property_contract_maps_each_type_expression() {
        let cases = [
            ("string", PropertyContract::Kind(Target::String)),
            ("number", PropertyContract::Kind(Target::Number)),
            ("boolean", PropertyContract::Kind(Target::Boolean)),
            ("string[]", PropertyContract::Kind(Target::Array)),
            (
                "Record<string, string>",
                PropertyContract::Kind(Target::Object),
            ),
            ("{ a: string }", PropertyContract::Kind(Target::Object)),
            ("any", PropertyContract::Any),
            ("unknown", PropertyContract::Unknown),
            (
                "'primary' | 'default'",
                PropertyContract::StringLiterals(vec!["primary".to_owned(), "default".to_owned()]),
            ),
            (
                "1 | 2",
                PropertyContract::NumberLiterals(vec!["1".to_owned(), "2".to_owned()]),
            ),
            // Nullish alternatives only widen optionality.
            (
                "boolean | undefined",
                PropertyContract::Kind(Target::Boolean),
            ),
            ("string | 'a'", PropertyContract::Kind(Target::String)),
            // Mixed primitive kinds have no single contract.
            ("string | number", PropertyContract::Unknown),
            ("'a' | 1", PropertyContract::Unknown),
        ];
        for (text, expected) in cases {
            let property = typing_property("value", text, false);
            assert_eq!(
                PropertyContract::from_property(&property),
                expected,
                "type = {text:?}"
            );
        }

        // An unproven member is always `Unknown`, even when the text parses.
        let unproven = unproven_property("value", "'a'");
        assert_eq!(
            PropertyContract::from_property(&unproven),
            PropertyContract::Unknown
        );

        assert!(contains_undefined(&TypeText::parse("string | undefined")));
        assert!(contains_undefined(&TypeText::parse("void")));
        assert!(!contains_undefined(&TypeText::parse("string")));
    }

    #[test]
    fn declared_props_replace_script_properties_and_keep_reserved_attributes() {
        let wxml = r#"<view><card type="medium" class="x" data-id="1" aria-label="l" /></view>"#;
        let type_attribute = literal_attr(wxml, "type", r#"type="medium""#);
        let class_attribute = literal_attr(wxml, "class", r#"class="x""#);
        let data_attribute = literal_attr(wxml, "data-id", r#"data-id="1""#);
        let aria_attribute = literal_attr(wxml, "aria-label", r#"aria-label="l""#);
        let layout = shadow(wxml, &[]);
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
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(
            calls.is_empty(),
            "declarations must skip the script inference"
        );
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, type_attribute.attribute_range);
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert!(
            diagnostic
                .message
                .contains("expects 'primary' | 'default' | 'warning'"),
            "{}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("got string literal"),
            "{}",
            diagnostic.message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_component_reports_undeclared_attributes_on_the_name() {
        let wxml = r#"<view><card round="{{ total }}" /></view>"#;
        let round = mustache_attr(wxml, "round", r#"round="{{ total }}""#, "total");
        let layout = shadow(wxml, &[round.expr_range.expect("expr")]);
        let view = declared_view(
            // The component script is absent on purpose: declarations alone are
            // enough to check bindings.
            vec![usage("card", None, vec![round.clone()])],
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, name_range(&round));
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert!(
            diagnostic.message.contains("component property \"round\""),
            "{}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("not declared"),
            "{}",
            diagnostic.message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_mustache_mismatch_lands_on_the_bound_expression() {
        let wxml = r#"<view><card text="{{ loading }}" /></view>"#;
        let text = mustache_attr(wxml, "text", r#"text="{{ loading }}""#, "loading");
        let layout = shadow(wxml, &[text.expr_range.expect("expr")]);
        let view = declared_view(vec![usage("card", None, vec![text.clone()])]);
        let (result, calls) = run(&layout, &view, vec![Some(("boolean", 0))]);
        assert_eq!(calls.len(), 1);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, text.expr_range.expect("expr"));
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert!(diagnostic.message.contains("expects string"));
        assert!(diagnostic.message.contains("got boolean"));
        assert!(
            diagnostic.message.contains("bound expression \"loading\""),
            "{}",
            diagnostic.message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_number_literals_check_the_bound_literal_value() {
        let wxml = r#"<view><card size="{{ total }}" /></view>"#;
        let size = mustache_attr(wxml, "size", r#"size="{{ total }}""#, "total");
        let layout = shadow(wxml, &[size.expr_range.expect("expr")]);
        for (answer, clean) in [("3", false), ("2", true), ("1.0", true)] {
            let view = declared_view(vec![usage("card", None, vec![size.clone()])]);
            let (result, _) = run(
                &layout,
                &view,
                vec![Some((answer, type_flags::NUMBER_LITERAL))],
            );
            if clean {
                assert!(
                    result.diagnostics.is_empty(),
                    "{answer}: {:?}",
                    result.diagnostics
                );
                assert!(result.status.is_none(), "{answer}: {:?}", result.status);
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{answer}");
                assert_eq!(
                    result.diagnostics[0].range,
                    size.expr_range.expect("expr"),
                    "{answer}"
                );
                assert!(
                    result.diagnostics[0].message.contains("expects 1 | 2"),
                    "{}",
                    result.diagnostics[0].message
                );
            }
        }

        // A WXML literal attribute is a string and cannot satisfy `1 | 2`.
        let literal_wxml = r#"<view><card size="1" /></view>"#;
        let literal = literal_attr(literal_wxml, "size", r#"size="1""#);
        let layout = shadow(literal_wxml, &[]);
        let view = declared_view(vec![usage("card", None, vec![literal.clone()])]);
        let (result, _) = run(&layout, &view, Vec::new());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, literal.attribute_range);
        assert!(
            result.diagnostics[0].message.contains("got string literal"),
            "{}",
            result.diagnostics[0].message
        );
    }

    #[test]
    fn declared_optional_members_allow_absence() {
        let wxml = r#"<view><card type="primary" /></view>"#;
        let type_attribute = literal_attr(wxml, "type", r#"type="primary""#);
        let layout = shadow(wxml, &[]);
        let view = declared_view(vec![usage("card", None, vec![type_attribute])]);
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_optional_and_undefined_members_accept_undefined() {
        for (name, clean) in [("type", true), ("plain", true), ("text", false)] {
            let wxml = format!(r#"<view><card {name}="{{{{ nothing }}}}" /></view>"#);
            let attribute = mustache_attr(
                &wxml,
                name,
                &format!(r#"{name}="{{{{ nothing }}}}""#),
                "nothing",
            );
            let layout = shadow(&wxml, &[attribute.expr_range.expect("expr")]);
            let view = declared_view(vec![usage("card", None, vec![attribute])]);
            let (result, _) = run(&layout, &view, vec![Some(("undefined", 0))]);
            if clean {
                assert!(
                    result.diagnostics.is_empty(),
                    "{name}: {:?}",
                    result.diagnostics
                );
            } else {
                assert_eq!(result.diagnostics.len(), 1, "{name}");
                assert!(
                    result.diagnostics[0].message.contains("got undefined"),
                    "{}",
                    result.diagnostics[0].message
                );
            }
        }
    }

    #[test]
    fn declared_unproven_member_is_skipped_while_others_are_checked() {
        let wxml = r#"<view><card ref="{{ total }}" text="{{ loading }}" /></view>"#;
        let reference = mustache_attr(wxml, "ref", r#"ref="{{ total }}""#, "total");
        let text = mustache_attr(wxml, "text", r#"text="{{ loading }}""#, "loading");
        let layout = shadow(
            wxml,
            &[
                reference.expr_range.expect("ref"),
                text.expr_range.expect("text"),
            ],
        );
        let view = declared_view(vec![usage("card", None, vec![reference, text.clone()])]);
        let (result, calls) = run(&layout, &view, vec![Some(("boolean", 0))]);
        // The unproven `ref` member is not queried; the proven `text` member is.
        assert_eq!(calls.len(), 1);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, text.expr_range.expect("text"));
        let CapabilityStatus::Partial { reason } = result.status.as_ref().expect("partial") else {
            panic!("expected Partial, got {:?}", result.status);
        };
        assert!(reason.contains("card.ref"), "{reason}");
    }

    #[test]
    fn declared_any_member_accepts_every_binding_without_a_query() {
        let wxml = r#"<view><card loose="{{ total }}" /></view>"#;
        let loose = mustache_attr(wxml, "loose", r#"loose="{{ total }}""#, "total");
        let layout = shadow(wxml, &[loose.expr_range.expect("expr")]);
        let view = declared_view(vec![usage("card", None, vec![loose])]);
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn components_without_declarations_keep_the_script_behaviour() {
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        let view = build_view(
            page_and_card(),
            vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
        );
        let (result, calls) = run(&layout, &view, vec![Some(("number", 0))]);
        assert_eq!(calls.len(), 1);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, title.expr_range.expect("expr"));
        assert!(diagnostic.message.contains("expects string, got number"));
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_members_use_the_camel_kebab_mapping() {
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    item: ItemDeclaration;\n  }\n\n  interface ItemDeclaration {\n    props: {\n      cardTitle?: 'small' | 'large';\n    };\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><item card-title="medium" /></view>"#;
        let card_title = literal_attr(wxml, "card-title", r#"card-title="medium""#);
        let layout = shadow(wxml, &[]);
        let view = typed_view(
            vec![prepared_declaration(DECLARATION_PATH, declaration)],
            vec![usage("item", None, vec![card_title.clone()])],
            declaration,
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, card_title.attribute_range);
        assert!(
            result.diagnostics[0]
                .message
                .contains("expects 'small' | 'large'"),
            "{}",
            result.diagnostics[0].message
        );
    }

    #[test]
    fn declared_kebab_members_accept_the_camel_spelling() {
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    item: ItemDeclaration;\n  }\n\n  interface ItemDeclaration {\n    props: {\n      'card-title'?: 'small' | 'large';\n    };\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><item cardTitle="medium" /></view>"#;
        let card_title = literal_attr(wxml, "cardTitle", r#"cardTitle="medium""#);
        let layout = shadow(wxml, &[]);
        let view = typed_view(
            vec![prepared_declaration(DECLARATION_PATH, declaration)],
            vec![usage("item", None, vec![card_title.clone()])],
            declaration,
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, card_title.attribute_range);
        assert!(
            result.diagnostics[0]
                .message
                .contains("expects 'small' | 'large'"),
            "{}",
            result.diagnostics[0].message
        );
    }

    #[test]
    fn declared_members_keep_case_only_variants_unresolved() {
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    item: ItemDeclaration;\n  }\n\n  interface ItemDeclaration {\n    props: {\n      someProp?: 'small' | 'large';\n    };\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><item someprop="medium" /></view>"#;
        let written = literal_attr(wxml, "someprop", r#"someprop="medium""#);
        let layout = shadow(wxml, &[]);
        let view = typed_view(
            vec![prepared_declaration(DECLARATION_PATH, declaration)],
            vec![usage("item", None, vec![written.clone()])],
            declaration,
        );
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(calls.is_empty());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.range, name_range(&written));
        assert!(
            diagnostic
                .message
                .contains("component property \"someprop\" is not declared"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn script_declared_camel_members_accept_the_kebab_spelling() {
        const KEBAB_DECLARATION_SCRIPT: &str =
            "Component({\n  properties: {\n    cardTitle: { type: String },\n  },\n});\n";
        let wxml = r#"<view><card card-title="{{ total }}" /></view>"#;
        let card_title = mustache_attr(wxml, "card-title", r#"card-title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[card_title.expr_range.expect("expr")]);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, KEBAB_DECLARATION_SCRIPT),
        ];
        let view = build_view(
            files,
            vec![usage("card", Some(CARD_PATH), vec![card_title.clone()])],
        );
        let (result, calls) = run(&layout, &view, vec![Some(("number", 0))]);
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(
            result.diagnostics[0].range,
            card_title.expr_range.expect("expr")
        );
        assert!(
            result.diagnostics[0].message.contains("expects string")
                && result.diagnostics[0].message.contains("got number"),
            "{}",
            result.diagnostics[0].message
        );
    }

    #[test]
    fn script_declared_members_stay_resolved_only_for_equivalent_spelling() {
        const KEBAB_DECLARATION_SCRIPT: &str =
            "Component({\n  properties: {\n    someProp: { type: String },\n  },\n});\n";
        let wxml = r#"<view><card someprop="{{ total }}" /></view>"#;
        let written = mustache_attr(wxml, "someprop", r#"someprop="{{ total }}""#, "total");
        let layout = shadow(wxml, &[written.expr_range.expect("expr")]);
        let files = vec![
            prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
            prepared_file(CARD_PATH, KEBAB_DECLARATION_SCRIPT),
        ];
        let view = build_view(files, vec![usage("card", Some(CARD_PATH), vec![written])]);
        let (result, calls) = run(&layout, &view, Vec::new());
        assert!(
            calls.is_empty(),
            "a case-only variant must not be looked up as the declared property: {calls:?}"
        );
        assert!(
            result.diagnostics.is_empty(),
            "undeclared script attributes stay ignored: {:?}",
            result.diagnostics
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
    fn fixture_declared_bindings_publish_diagnostics_without_an_engine() {
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

        let ranges: Vec<SourceRange> = view.fragments.iter().map(|item| item.wxml_range).collect();
        let layout = shadow(&text, &ranges);
        let mut calls = 0_usize;
        let result = diagnostics_with(&layout, &view, SCRIPT_PATH, |path, position| {
            assert_eq!(path, SCRIPT_PATH);
            calls += 1;
            let offset = position_to_offset(layout.derived(), position).expect("derived offset");
            let range = match layout
                .map_native_range(offset.get(), offset.get() + 1)
                .expect("identifier range")
            {
                crate::backend_tsgo::mapping::NativeRange::Template { range, .. } => range,
                crate::backend_tsgo::mapping::NativeRange::Script { .. } => {
                    panic!("component query position is not in a mount")
                }
            };
            let start = usize::try_from(range.start()).expect("start");
            let mut end = start;
            while let Some(character) = text[end..].chars().next() {
                if character.is_alphanumeric() || matches!(character, '_' | '$') {
                    end += character.len_utf8();
                } else {
                    break;
                }
            }
            let answer = match text.get(start..end).expect("identifier bytes") {
                "buttonText" => "string",
                "loading" => "boolean",
                other => panic!("unexpected component query for {other}"),
            };
            Ok(TypeQuery {
                info: Some(RawType {
                    id: 1,
                    flags: 0,
                    intrinsic_name: None,
                }),
                text: Some(answer.to_owned()),
            })
        });
        assert_eq!(calls, 5, "one query per proven mustache binding");

        let mismatch_start =
            text.find(r#"text="{{ loading }}""#).expect("mismatch") + r#"text="{{ "#.len();
        let mismatch_range = source_range(mismatch_start, mismatch_start + "loading".len());
        let round_start = text
            .find(r#"round="{{ true }}""#)
            .expect("undeclared attribute");
        let round_range = source_range(round_start, round_start + "round".len());

        assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
        let mismatch = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range == mismatch_range)
            .unwrap_or_else(|| panic!("missing mismatch: {:?}", result.diagnostics));
        assert_eq!(mismatch.severity, DiagnosticSeverity::Error);
        assert!(
            mismatch.message.contains("expects string, got boolean"),
            "{}",
            mismatch.message
        );
        let undeclared = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range == round_range)
            .unwrap_or_else(|| panic!("missing undeclared: {:?}", result.diagnostics));
        assert_eq!(undeclared.severity, DiagnosticSeverity::Error);
        assert!(
            undeclared
                .message
                .contains("component property \"round\" is not declared"),
            "{}",
            undeclared.message
        );
        assert!(result.status.is_none(), "{:?}", result.status);
    }

    #[test]
    fn declared_components_with_a_structurally_incomplete_map_fall_back_to_scripts() {
        // The mapped interface is missing, so the tag cannot be declared and
        // the script facts must keep driving the check.
        let declaration = "declare global {\n  interface WxmlComponentMap {\n    card: MissingDeclaration;\n  }\n}\n\nexport {};\n";
        let wxml = r#"<view><card title="{{ total }}" /></view>"#;
        let title = mustache_attr(wxml, "title", r#"title="{{ total }}""#, "total");
        let layout = shadow(wxml, &[title.expr_range.expect("expr")]);
        let view = typed_view(
            vec![
                prepared_file(SCRIPT_PATH, PAGE_SCRIPT),
                prepared_file(CARD_PATH, CARD_SCRIPT),
                prepared_declaration(DECLARATION_PATH, declaration),
            ],
            vec![usage("card", Some(CARD_PATH), vec![title.clone()])],
            declaration,
        );
        let (result, calls) = run(&layout, &view, vec![Some(("number", 0))]);
        assert_eq!(calls.len(), 1);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].range, title.expr_range.expect("expr"));
        assert!(result.diagnostics[0].message.contains("expects string"));
    }

    // -----------------------------------------------------------------------
    // Real type engine (opt-in through `WXML_REQUIRE_REAL_ENGINE=1`)
    // -----------------------------------------------------------------------

    /// View provider backed by one prepared view.
    struct DeclaredViews {
        view: PreparedView,
    }

    impl ViewProvider for DeclaredViews {
        fn prepare(&self, _snapshot: &SourceSnapshot) -> Result<PreparedView, HostError> {
            Ok(self.view.clone())
        }

        fn prepare_script(&self, _file: &FileId) -> Result<PreparedView, ScriptViewError> {
            Ok(self.view.clone())
        }
    }

    #[test]
    fn optional_real_engine_declared_property_mismatch() {
        const REAL_WXML: &str = r#"<view><card type="primary" text="{{ loading }}" /></view>"#;
        const REAL_SCRIPT: &str = "Page({\n  data: { buttonText: 'ok', loading: true },\n});\n";
        const REAL_GLOBALS: &str = "declare namespace WechatMiniprogram {\n  interface CustomEvent<T> {\n    detail: T;\n  }\n}\n\ndeclare function Page(options: {\n  data: { buttonText: string; loading: boolean };\n  __wxml_check_0(this: { data: { buttonText: string; loading: boolean } }): void;\n}): void;\n";
        const REAL_DECLARATION: &str = "declare global {\n  interface WxmlComponentMap {\n    card: CardDeclaration;\n  }\n\n  interface CardDeclaration {\n    props: {\n      type?: 'primary' | 'default';\n      text: string;\n    };\n  }\n}\n\nexport {};\n";

        if crate::backend_tsgo::runtime::skip_unless_real_engine("component bindings") {
            return;
        }

        let text = mustache_attr(REAL_WXML, "text", r#"text="{{ loading }}""#, "loading");
        let expr_range = text.expr_range.expect("expression range");
        let fragment = CheckFragment {
            fragment_id: "f0".to_owned(),
            expr_text: "loading".to_owned(),
            target: None,
            bindings: Vec::new(),
            guards: Vec::new(),
            wxml_range: expr_range,
            fragment_revision: 1,
            root: RootKind::Page,
        };
        let mut view = typed_view(
            vec![
                prepared_file(SCRIPT_PATH, REAL_SCRIPT),
                prepared_file("globals.d.ts", REAL_GLOBALS),
                prepared_declaration(DECLARATION_PATH, REAL_DECLARATION),
            ],
            vec![usage("card", None, vec![text])],
            REAL_DECLARATION,
        );
        view.fragments = Arc::from(vec![fragment]);

        let adapter = TsgoBackendAdapter::new(
            PinnedEngine::pinned(),
            Arc::new(DeclaredViews { view }),
            Arc::new(crate::backend_tsgo::runtime::VirtualSessionFactory::new()),
        );
        let context = RequestContext::new(
            ProjectRevision::new(project(), 1),
            ViewRevision::new(1),
            adapter.epoch(),
            Arc::new(SourceSnapshot::new(
                file_id("pages/index/index.wxml"),
                SourceRevision::new(1),
                SnapshotId::new(1),
                REAL_WXML,
            )),
        )
        .expect("context");
        let result = adapter
            .diagnostics(DiagnosticsRequest { context })
            .expect("declared property diagnostics");
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range == expr_range)
            .unwrap_or_else(|| {
                panic!(
                    "missing declared property diagnostic: {:?}",
                    result.diagnostics
                )
            });
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert!(
            diagnostic.message.contains("expects string"),
            "{}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("got boolean"),
            "{}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("bound expression \"loading\""),
            "{}",
            diagnostic.message
        );
    }
}
