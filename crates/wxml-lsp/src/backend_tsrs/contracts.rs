//! Component property and event contracts for the in-place shadow.
//!
//! Wave 1 added the shadow-side probe facilities (`ContractSpec` and
//! `ContractAnchor`) and the shared component property facts. This module turns
//! one prepared view into probe specifications:
//!
//! - a pure-mustache component attribute reuses the fragment that the template
//!   analyzer already produced for the same expression range,
//! - a static event handler gets a member fragment (`this.<handler>`) with the
//!   declared event signature as its expected type.
//!
//! Only the probe specification is produced here. Publishing the engine
//! diagnostics that land on a probe is Wave 3's responsibility.
//!
//! The expected types reference the original declaration instead of printing a
//! `TypeText`, so generics, aliases and imported types keep their identity. A
//! declaration file that is a module exposes the map through a type-level
//! `import(...)` reference; a global map (including `declare global`) is
//! referenced by name.

use std::collections::{BTreeMap, BTreeSet};

use wxml_backend_tsrs::script::events::MethodTable;
use wxml_backend_tsrs::shadow::expression::{TokenKind, lex_script};
use wxml_backend_tsrs::shadow::layout::{ContractKind, ContractSpec, FragmentSpec};
use wxml_backend_tsrs::shadow::mapping::SourceRange as ShadowSourceRange;
use wxml_project::ProjectPath;
use wxml_project::typing::{PrimitiveType, TypeText, TypingComponent, TypingEvent};
use wxml_types::{
    AttributeValueKind, CheckFragment, ComponentAttribute, ComponentUsage, EventHandler,
};

use crate::backend_tsrs::component_facts::{ComponentFactsCache, ComponentUsageFacts};
use crate::project_host::{PreparedFile, PreparedView};

/// Fragment id prefix used by event contract probes.
const EVENT_FRAGMENT_PREFIX: &str = "contract_event_";

/// Degradation reasons collected by contract planning.
///
/// Reasons are categories with a count, so a template with many skipped
/// bindings cannot grow the `Partial` string without bound.
const REASON_JS_CHECK_DISABLED: &str =
    "JavaScript type checking is disabled (`checkJs` or a `@ts-check` pragma is required)";
const REASON_DECLARATION_UNAVAILABLE: &str =
    "the component declaration file is not part of the shadow input";
const REASON_DECLARATION_UNREADABLE: &str = "the component declaration file could not be scanned";
const REASON_SCRIPT_FACTS_UNAVAILABLE: &str = "component script facts are unavailable";
const REASON_SCRIPT_PROPERTY_INCOMPLETE: &str = "a script property type is not completely readable";
const REASON_SCRIPT_FACTS_INCOMPLETE: &str =
    "component behaviors are not completely resolved; declared members are still checked";
const REASON_EXPECTED_UNKNOWN: &str = "the expected property type is `unknown`";
const REASON_EXPECTED_NEVER: &str = "the expected property type is `never`";
const REASON_EVENT_DETAIL_MISSING: &str = "a declared event has no detail type";
const REASON_EVENT_METHOD_TABLE: &str = "the event method table is unavailable";
const REASON_EVENT_HANDLER_OPAQUE: &str = "an event handler is not a literal function";
const REASON_EVENT_HANDLER_UNANNOTATED: &str =
    "an event handler parameter has no explicit type annotation";
const REASON_EVENT_RANGE: &str = "an event handler range does not match its token";

/// Contracts planned for one prepared view.
#[derive(Debug, Default)]
pub(crate) struct ContractPlan {
    /// Property contract keyed by the template fragment that hosts it.
    ///
    /// A fragment is mounted once, so the map also deduplicates repeated
    /// bindings of the same expression.
    pub(crate) properties: BTreeMap<String, ContractSpec>,
    /// Fully built event contract fragments.
    pub(crate) events: Vec<FragmentSpec>,
    /// Skip categories and their counts.
    pub(crate) reasons: BTreeMap<&'static str, usize>,
}

impl ContractPlan {
    fn skip(&mut self, reason: &'static str) {
        *self.reasons.entry(reason).or_insert(0) += 1;
    }
}

/// Shared inputs of one contract planning pass.
struct PlanContext<'a> {
    view: &'a PreparedView,
    /// Owning script path, used for module-level declaration references.
    script_path: &'a str,
    /// Owner method table; `None` when script analysis did not run or failed.
    methods: Option<&'a MethodTable>,
    /// Declaration-qualifier cache keyed by declaration path.
    ///
    /// `Err` carries the stable skip reason, so a repeated usage never reports
    /// a different cause than the first one.
    qualifiers: BTreeMap<String, Result<String, &'static str>>,
}

/// Returns whether an event contract needs the owner method table.
///
/// `collect_script_facts` otherwise skips script analysis when the script has
/// no `properties` or `behaviors` member. A declared component event with a
/// detail type is enough to require the method table.
pub(crate) fn needs_methods(view: &PreparedView) -> bool {
    view.events.iter().any(|event| {
        declared_event(view, event).is_some_and(|(_, declared)| declared.detail_text.is_some())
    })
}

/// Plans every contract probe for one prepared view.
///
/// `checking_enabled` is `false` for a JavaScript owner without `checkJs` and
/// without a file-level `@ts-check` pragma: no probe is generated and the
/// disabled state becomes one `Partial` reason.
pub(crate) fn plan(
    view: &PreparedView,
    cache: &mut ComponentFactsCache<'_>,
    methods: Option<&MethodTable>,
    checking_enabled: bool,
    script_path: &str,
) -> ContractPlan {
    let mut plan = ContractPlan::default();
    if !checking_enabled {
        if has_candidates(view) {
            plan.skip(REASON_JS_CHECK_DISABLED);
        }
        return plan;
    }
    let mut context = PlanContext {
        view,
        script_path,
        methods,
        qualifiers: BTreeMap::new(),
    };
    plan_properties(&mut context, cache, &mut plan);
    plan_events(&mut context, &mut plan);
    plan
}

/// Returns whether a disabled JavaScript view had any contract candidate.
fn has_candidates(view: &PreparedView) -> bool {
    view.components.iter().any(|usage| {
        usage
            .attributes
            .iter()
            .any(|attribute| attribute.value_kind == AttributeValueKind::Mustache)
    }) || view
        .events
        .iter()
        .any(|event| declared_event(view, event).is_some())
}

/// Plans one property contract per pure-mustache component attribute.
fn plan_properties(
    context: &mut PlanContext<'_>,
    cache: &mut ComponentFactsCache<'_>,
    plan: &mut ContractPlan,
) {
    for usage in context.view.components.iter() {
        for attribute in &usage.attributes {
            if attribute.value_kind != AttributeValueKind::Mustache {
                continue;
            }
            let Some(range) = attribute.expr_range else {
                continue;
            };
            let Some(fragment) = fragment_for(context.view.fragments.as_ref(), range) else {
                // The expression is not mounted (malformed or over budget);
                // the existing fragment degradation already reports that.
                continue;
            };
            if plan.properties.contains_key(&fragment.fragment_id) {
                continue;
            }
            let Some(contract) =
                property_contract(context, cache, usage, attribute, fragment, plan)
            else {
                continue;
            };
            plan.properties
                .insert(fragment.fragment_id.clone(), contract);
        }
    }
}

/// Builds one property contract, or records why it cannot be built.
fn property_contract(
    context: &mut PlanContext<'_>,
    cache: &mut ComponentFactsCache<'_>,
    usage: &ComponentUsage,
    attribute: &ComponentAttribute,
    fragment: &CheckFragment,
    plan: &mut ContractPlan,
) -> Option<ContractSpec> {
    let facts = cache.resolve(usage);
    let expected = match facts {
        ComponentUsageFacts::Declared(component) => {
            let property = component.prop_named(&attribute.name)?;
            match classify(&property.ty) {
                Expectation::Any => return None,
                Expectation::Unrepresentable(reason) => {
                    plan.skip(reason);
                    return None;
                }
                Expectation::Checkable => {}
            }
            let qualifier = declaration_qualifier(context, &component.source, plan)?;
            let mut expected = format!(
                "{qualifier}['{}']['props']['{}']",
                escape_string(&usage.tag),
                escape_string(&property.name)
            );
            if property.optional {
                expected.push_str(" | undefined");
            }
            expected
        }
        ComponentUsageFacts::Script(facts) => {
            let property = facts.member(&attribute.name)?;
            if !property.type_complete {
                plan.skip(REASON_SCRIPT_PROPERTY_INCOMPLETE);
                return None;
            }
            if !facts.complete() {
                // Unlisted members may come from a behavior, but this member
                // is complete and stays checkable.
                plan.skip(REASON_SCRIPT_FACTS_INCOMPLETE);
            }
            match classify(&TypeText::parse(&property.type_text)) {
                Expectation::Any => return None,
                Expectation::Unrepresentable(reason) => {
                    plan.skip(reason);
                    return None;
                }
                Expectation::Checkable => {}
            }
            property.type_text.clone()
        }
        ComponentUsageFacts::Unavailable => {
            plan.skip(REASON_SCRIPT_FACTS_UNAVAILABLE);
            return None;
        }
    };
    Some(ContractSpec {
        kind: ContractKind::Property,
        expected: expected.clone(),
        display: expected,
        anchor: ShadowSourceRange::new(fragment.wxml_range.start(), fragment.wxml_range.end()),
    })
}

/// Plans one event contract per declared component event with a static handler.
fn plan_events(context: &mut PlanContext<'_>, plan: &mut ContractPlan) {
    let mut used_ids: BTreeSet<String> = context
        .view
        .fragments
        .iter()
        .map(|fragment| fragment.fragment_id.clone())
        .collect();
    for event in context.view.events.iter() {
        let Some((component, declared)) = declared_event(context.view, event) else {
            continue;
        };
        let Some(contract) = event_contract(context, event, component, declared, plan) else {
            continue;
        };
        let mut index = plan.events.len();
        let id = loop {
            let candidate = format!("{EVENT_FRAGMENT_PREFIX}{index}");
            if used_ids.insert(candidate.clone()) {
                break candidate;
            }
            index += 1;
        };
        let mut spec = FragmentSpec::new(
            id,
            event.handler.clone(),
            ShadowSourceRange::new(event.handler_range.start(), event.handler_range.end()),
        );
        spec.member = true;
        spec.contract = Some(contract);
        plan.events.push(spec);
    }
}

/// Builds one event contract, or records why it cannot be built.
fn event_contract(
    context: &mut PlanContext<'_>,
    event: &EventHandler,
    component: &TypingComponent,
    declared: &TypingEvent,
    plan: &mut ContractPlan,
) -> Option<ContractSpec> {
    if !is_member_name(&event.handler) {
        return None;
    }
    let expected_len = u32::try_from(event.handler.len()).ok()?;
    if event.handler_range.len() != expected_len {
        plan.skip(REASON_EVENT_RANGE);
        return None;
    }
    if declared.detail_text.is_none() {
        plan.skip(REASON_EVENT_DETAIL_MISSING);
        return None;
    }
    let Some(methods) = context.methods else {
        plan.skip(REASON_EVENT_METHOD_TABLE);
        return None;
    };
    let Some(method) = methods
        .methods
        .iter()
        .find(|method| method.name == event.handler)
    else {
        if methods.opaque.contains(&event.handler) {
            plan.skip(REASON_EVENT_HANDLER_OPAQUE);
        }
        // A genuinely missing handler keeps the existing event diagnostic.
        return None;
    };
    if method.parameter.is_none() && method.arity != 0 {
        plan.skip(REASON_EVENT_HANDLER_UNANNOTATED);
        return None;
    }
    let qualifier = declaration_qualifier(context, &component.source, plan)?;
    let expected = format!(
        "(event: WechatMiniprogram.CustomEvent<{qualifier}['{}']['events']['{}']['detail']>) => void",
        escape_string(&event.tag),
        escape_string(&declared.name)
    );
    Some(ContractSpec {
        kind: ContractKind::Event,
        expected: expected.clone(),
        display: expected,
        anchor: ShadowSourceRange::new(event.handler_range.start(), event.handler_range.end()),
    })
}

/// Returns the declared component and event for one static handler.
fn declared_event<'a>(
    view: &'a PreparedView,
    event: &EventHandler,
) -> Option<(&'a TypingComponent, &'a TypingEvent)> {
    let typing = view.typing.as_deref()?;
    let component = typing.component(&event.tag)?;
    let declared = component.event_named(&event.event)?;
    Some((component, declared))
}

/// Finds the fragment whose WXML range is exactly the attribute expression.
fn fragment_for(
    fragments: &[CheckFragment],
    range: wxml_types::SourceRange,
) -> Option<&CheckFragment> {
    fragments
        .iter()
        .find(|fragment| fragment.wxml_range == range)
}

/// Whether an expected type is checkable at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// The engine can check the expression against this type.
    Checkable,
    /// `any` accepts every value: skip and treat the binding as clean.
    Any,
    /// `unknown`/`never` cannot be meaningfully checked.
    Unrepresentable(&'static str),
}

/// Classifies one expected type by the `any`/`unknown`/`never` rules.
fn classify(ty: &TypeText) -> Expectation {
    match ty {
        TypeText::Primitive(PrimitiveType::Any) => Expectation::Any,
        TypeText::Primitive(PrimitiveType::Unknown) => {
            Expectation::Unrepresentable(REASON_EXPECTED_UNKNOWN)
        }
        TypeText::Primitive(PrimitiveType::Never) => {
            Expectation::Unrepresentable(REASON_EXPECTED_NEVER)
        }
        TypeText::Union(parts) => {
            if parts.iter().any(is_any) {
                Expectation::Any
            } else if parts.iter().any(is_unknown) {
                Expectation::Unrepresentable(REASON_EXPECTED_UNKNOWN)
            } else if parts.iter().all(is_never) {
                Expectation::Unrepresentable(REASON_EXPECTED_NEVER)
            } else {
                Expectation::Checkable
            }
        }
        _ => Expectation::Checkable,
    }
}

const fn is_any(ty: &TypeText) -> bool {
    matches!(ty, TypeText::Primitive(PrimitiveType::Any))
}

const fn is_unknown(ty: &TypeText) -> bool {
    matches!(ty, TypeText::Primitive(PrimitiveType::Unknown))
}

const fn is_never(ty: &TypeText) -> bool {
    matches!(ty, TypeText::Primitive(PrimitiveType::Never))
}

/// Resolves the type-level qualifier of a declaration file's component map.
///
/// A map visible in the global scope is referenced by name. A module-level map
/// is referenced through a type-level `import(...)` from the owning script, so
/// the declaration keeps its real identity. A declaration file that is absent
/// from the shadow input is a `Partial` gap, never a guessed reference.
fn declaration_qualifier(
    context: &mut PlanContext<'_>,
    source: &ProjectPath,
    plan: &mut ContractPlan,
) -> Option<String> {
    if let Some(cached) = context.qualifiers.get(source.as_str()) {
        return match cached {
            Ok(qualifier) => Some(qualifier.clone()),
            Err(reason) => {
                let reason = *reason;
                plan.skip(reason);
                None
            }
        };
    }
    let computed = declaration_file(context.view, source.as_str()).map_or(
        Err(REASON_DECLARATION_UNAVAILABLE),
        |file| {
            let (global_map, is_module) =
                declaration_shape(file.text.as_ref()).ok_or(REASON_DECLARATION_UNREADABLE)?;
            if global_map || !is_module {
                return Ok("WxmlComponentMap".to_owned());
            }
            let specifier = relative_module_specifier(context.script_path, source.as_str())
                .ok_or(REASON_DECLARATION_UNREADABLE)?;
            Ok(format!(
                "import('{}').WxmlComponentMap",
                escape_string(&specifier)
            ))
        },
    );
    if let Err(reason) = &computed {
        plan.skip(reason);
    }
    context
        .qualifiers
        .insert(source.as_str().to_owned(), computed.clone());
    computed.ok()
}

fn declaration_file<'a>(view: &'a PreparedView, path: &str) -> Option<&'a PreparedFile> {
    view.files.iter().find(|file| file.path.as_str() == path)
}

/// Returns whether the declaration file declares a globally visible
/// `WxmlComponentMap` and whether it is an ES module.
///
/// Comments and strings are skipped by the shared script lexer, so a comment
/// mentioning `import`/`export` never changes the classification.
fn declaration_shape(text: &str) -> Option<(bool, bool)> {
    let tokens = lex_script(text).ok()?;
    let mut depth_at = Vec::with_capacity(tokens.len());
    let mut open_at: Vec<Option<usize>> = vec![None; tokens.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut depth = 0_i32;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Punctuator('}') {
            depth -= 1;
            if let Some(open) = stack.pop() {
                open_at[open] = Some(index);
            }
        }
        depth_at.push(depth);
        if token.kind == TokenKind::Punctuator('{') {
            stack.push(index);
            depth += 1;
        }
    }

    let mut global_ranges: Vec<(usize, usize)> = Vec::new();
    for index in 0..tokens.len().saturating_sub(2) {
        if tokens[index].kind != TokenKind::Identifier
            || tokens[index].text(text) != "declare"
            || tokens[index + 1].kind != TokenKind::Identifier
            || tokens[index + 1].text(text) != "global"
            || tokens[index + 2].kind != TokenKind::Punctuator('{')
        {
            continue;
        }
        if let Some(close) = open_at[index + 2] {
            global_ranges.push((index + 2, close));
        }
    }

    let mut global_map = false;
    let mut is_module = false;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let word = token.text(text);
        if word == "WxmlComponentMap"
            && global_ranges
                .iter()
                .any(|(open, close)| *open < index && index < *close)
        {
            global_map = true;
        }
        if depth_at[index] == 0 {
            if word == "export" {
                is_module = true;
            }
            if word == "import"
                && !tokens
                    .get(index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Punctuator('('))
            {
                is_module = true;
            }
        }
    }
    (global_map, is_module).into()
}

/// Builds the project-relative module specifier of one declaration file.
fn relative_module_specifier(from: &str, target: &str) -> Option<String> {
    let from_parts: Vec<&str> = from.split('/').collect();
    let from_dir = from_parts
        .get(..from_parts.len().saturating_sub(1))
        .unwrap_or_default();
    let target = strip_declaration_suffix(target);
    let target_parts: Vec<&str> = target.split('/').collect();
    let mut common = 0_usize;
    while common < from_dir.len()
        && common < target_parts.len()
        && from_dir[common] == target_parts[common]
    {
        common += 1;
    }
    let mut parts: Vec<&str> = Vec::new();
    parts.extend(std::iter::repeat_n("..", from_dir.len() - common));
    parts.extend_from_slice(&target_parts[common..]);
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let mut specifier = parts.join("/");
    if !specifier.starts_with('.') {
        specifier.insert_str(0, "./");
    }
    Some(specifier)
}

fn strip_declaration_suffix(path: &str) -> &str {
    for suffix in [".d.ts", ".d.mts", ".d.cts"] {
        if let Some(stripped) = path.strip_suffix(suffix) {
            return stripped;
        }
    }
    path
}

/// Escapes one declaration key for a single-quoted TypeScript string literal.
fn escape_string(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Returns whether `name` can be mounted after a `this.` member prefix.
fn is_member_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first.is_alphabetic() || matches!(first, '_' | '$')) {
        return false;
    }
    characters.all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
}

/// Returns whether a JavaScript file opts into checking with `@ts-check`.
///
/// Only leading comments count; the pragma after the first statement has no
/// effect in TypeScript either.
pub(crate) fn has_ts_check(text: &str) -> bool {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut cursor = if text.starts_with("#!") {
        text.find('\n').map_or(text.len(), |offset| offset + 1)
    } else {
        0
    };
    loop {
        while let Some(character) = text[cursor..].chars().next() {
            if !character.is_whitespace() {
                break;
            }
            cursor += character.len_utf8();
        }
        if text[cursor..].starts_with("//") {
            let end = text[cursor..]
                .find('\n')
                .map_or(text.len(), |offset| cursor + offset);
            if text[cursor..end].contains("@ts-check") {
                return true;
            }
            cursor = end;
            continue;
        }
        if text[cursor..].starts_with("/*") {
            let end = text[cursor + 2..]
                .find("*/")
                .map_or(text.len(), |offset| cursor + 2 + offset + 2);
            if text[cursor..end].contains("@ts-check") {
                return true;
            }
            cursor = end;
            continue;
        }
        return false;
    }
}
