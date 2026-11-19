//! Production in-place shadow layout for `Page`/`Component`/`Behavior`.
//!
//! The original script stays the type environment. A probe member is inserted
//! *inside* the registration's options object (or inside its `methods` object)
//! so fragments are checked against `this.data` while the options literal
//! remains the direct argument of `Page`/`Component`/`Behavior`: contextual
//! typing, generic inference and `this` typing stay intact.
//!
//! Everything is a deterministic text edit. The original code is preserved
//! verbatim outside the probe (and optional pragma/BOM edits). The layout never
//! falls back to a non-shadow view: an unsupported registration shape returns
//! [`ShadowError::Unsupported`] and a malformed request returns
//! [`ShadowError::Invalid`].
//!
//! # What is scanned
//!
//! A small conservative script scanner locates registration calls, their
//! object-literal argument and first-level members. A registrar name that is
//! also declared with `function`/`class`/`const`/`let`/`var` is treated as
//! shadowed and rejected. The scanner fails closed on unbalanced script text.
//!
//! `data` is classified as an inline object, absent, a same-file type-only
//! reference (rejected) or a general expression (reported `Partial`, because
//! field-level roots cannot be enumerated without a checker). `properties` is
//! read syntactically (`type` + `optionalTypes` + `JSDoc`) for the overlay.
//! [`build_in_place_shadow_with_behavior`] additionally merges the statically
//! resolved behavior properties from [`crate::script::behavior`], so the
//! generated `__WxmlProps_0` keeps behavior-declared unions. A dynamic or
//! unresolved behavior graph stays `Partial`; the shadow never invents a
//! property.

#![allow(clippy::format_push_string)] // generated probe text is tiny; format! stays readable

use std::collections::BTreeSet;

use crate::input::SourceFile;
use crate::script::behavior::BehaviorProperty;
use crate::shadow::ShadowError;
use crate::shadow::expression::{self, Token, TokenKind};
use crate::shadow::guards::{GuardChain, PlannedGuards, render_guards_planned};
use crate::shadow::locals::{
    DEFAULT_LOCAL_BUDGET, LocalBinding, Syntax, group_local_levels, indent_lines,
    local_declarations, local_helpers, render_locals,
};
use crate::shadow::mapping::{
    MountRecord, ShadowMap, SourceRange, byte_to_utf16, utf16_len, utf16_to_byte,
};
use crate::shadow::rewrite::{
    AppliedEdit, EditKind, MountedSegment, RewriteOptions, RewrittenExpression, ShadowEdit,
    apply_edits, rewrite_expression,
};

/// Probe member name inserted into the registration options.
pub const IN_PLACE_PROBE: &str = "__wxml_check_0";
/// Root local that aliases `this.data`.
pub const ROOT_NAME: &str = "__wxml_root";
/// Prefix inserted before a free data root.
pub const ROOT_PREFIX: &str = "__wxml_root.";
/// Declaration-derived properties value name.
pub const PROPS_VALUE_NAME: &str = "__wxml_props_0";
/// Declaration-derived properties type name.
pub const PROPS_TYPE_NAME: &str = "__WxmlProps_0";
/// Declaration-derived properties raw value name (JavaScript only).
pub const PROPS_RAW_NAME: &str = "__wxml_props_raw_0";
/// Prefix inserted before a declared component property.
pub const PROPS_PREFIX: &str = "__wxml_props_0.";

const METHOD_INDENT: &str = "      ";
const BODY_INDENT: &str = "        ";
/// Bound one malformed import scan; exceeding it fails closed as "shadowed".
const MAX_IMPORT_SCAN_TOKENS: usize = 4_096;

/// Registration root requested by the template owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    /// `Page(...)` registration.
    Page,
    /// `Component(...)` registration.
    Component,
    /// `Behavior(...)` registration.
    Behavior,
}

impl RootKind {
    const fn registrar(self) -> &'static str {
        match self {
            Self::Page => "Page",
            Self::Component => "Component",
            Self::Behavior => "Behavior",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Component => "component",
            Self::Behavior => "behavior",
        }
    }
}

/// Build status of the derived view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    /// Every required declaration was read and every fragment was mounted.
    Supported,
    /// The probe is mounted, but coverage has an explicit gap (see `reason`).
    Partial,
}

/// JavaScript checking policy from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsCheckPolicy {
    /// Inherit project options.
    Inherit,
    /// Add a shadow-only `// @ts-check` when JS checking is otherwise off.
    Pragma,
    /// Disable template diagnostics.
    Off,
}

/// Effective JavaScript checking policy of the derived view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsCheckApplied {
    /// Project options decide.
    Inherit,
    /// Shadow-only `// @ts-check` was injected.
    Pragma,
    /// Template diagnostics are disabled.
    Off,
}

/// Classification of the registration's `data` member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRootKind {
    /// Inline object literal.
    Object,
    /// No `data` member.
    Absent,
    /// Non-literal value expression; field-level roots are not enumerated.
    Expression,
    /// Explicitly unsupported member shape.
    Unsupported,
    /// Same-file interface/type alias used as a value.
    TypeReference,
}

/// One WXML fragment to mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentSpec {
    /// Stable fragment id.
    pub id: String,
    /// Expression text.
    pub expr: String,
    /// Byte range of the expression in the original WXML text.
    ///
    /// Invariant: the range length must equal `expr`'s UTF-8 byte length;
    /// [`build_in_place_shadow`] returns [`ShadowError::Invalid`] otherwise.
    pub wxml_range: SourceRange,
    /// Explicit target identifier; `None` selects the rightmost value identifier.
    pub target: Option<String>,
    /// Compatibility name-only locals (used when `bindings` is empty).
    pub locals: Vec<String>,
    /// Structured `wx:for` bindings, outermost first.
    pub bindings: Vec<LocalBinding>,
    /// Enclosing guard chains, outermost first.
    pub guards: Vec<GuardChain>,
}

impl FragmentSpec {
    /// Creates a fragment with no target, locals, bindings or guards.
    ///
    /// `wxml_range` is not validated here; the build validates that its length
    /// equals the expression's UTF-8 byte length.
    #[must_use]
    pub fn new(id: impl Into<String>, expr: impl Into<String>, wxml_range: SourceRange) -> Self {
        Self {
            id: id.into(),
            expr: expr.into(),
            wxml_range,
            target: None,
            locals: Vec::new(),
            bindings: Vec::new(),
            guards: Vec::new(),
        }
    }
}

/// Input for one in-place shadow build.
#[derive(Debug, Clone)]
pub struct ShadowRequest<'a> {
    /// Original script snapshot.
    pub script: &'a SourceFile,
    /// Requested registration root.
    pub root: RootKind,
    /// Fragments to mount.
    pub fragments: &'a [FragmentSpec],
    /// JavaScript checking policy.
    pub js_check: JsCheckPolicy,
    /// Effective `checkJs` project option.
    pub check_js: bool,
}

impl<'a> ShadowRequest<'a> {
    /// Creates an empty request for one script and root kind.
    #[must_use]
    pub const fn new(script: &'a SourceFile, root: RootKind) -> Self {
        Self {
            script,
            root,
            fragments: &[],
            js_check: JsCheckPolicy::Inherit,
            check_js: false,
        }
    }
}

/// One declared component property read syntactically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredProperty {
    /// Property name.
    pub name: String,
    /// Union text derived from `type` plus `optionalTypes`.
    pub type_text: String,
    /// True when the declaration lists `optionalTypes`.
    pub has_optional_types: bool,
    /// `JSDoc` text, whitespace-collapsed.
    pub doc: String,
    /// Byte range of the property key in the original owning script.
    ///
    /// `None` for a property contributed by a behavior in another file; the
    /// generated declaration then has no original-script definition target.
    pub source: Option<SourceRange>,
}

/// Syntactic `properties` scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyScan {
    /// Declared properties in source order.
    pub properties: Vec<DeclaredProperty>,
    /// False when spread/shorthand/computed members hide declarations.
    pub complete: bool,
    /// Human-readable completeness problems.
    pub problems: Vec<String>,
}

impl PropertyScan {
    const fn empty() -> Self {
        Self {
            properties: Vec::new(),
            complete: true,
            problems: Vec::new(),
        }
    }
}

/// Resolved behavior contribution for the declaration-derived `properties`
/// overlay.
///
/// `properties` is the precedence-merged behavior table from
/// [`crate::script::behavior`]; the component's own `properties` still win when
/// both declare the same name. A dynamic or partly unresolved behavior graph
/// sets `complete = false`, so the shadow reports `Partial` instead of
/// pretending the overlay covers every property.
#[derive(Debug, Clone, Copy)]
pub struct BehaviorPropertyOverlay<'a> {
    /// Merged behavior properties, later behaviors first.
    pub properties: &'a [BehaviorProperty],
    /// True when every reachable behavior and its properties were read.
    pub complete: bool,
    /// Degradation reason when `complete` is false.
    pub reason: Option<&'a str>,
}

/// Registration selected by the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationInfo {
    /// Actual registrar kind (a page may be registered with `Component`).
    pub kind: RootKind,
    /// Number of registrations of the requested kind.
    pub count: usize,
}

/// One generated insertion record in derived UTF-16 coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertionRecord {
    /// Stable insertion id.
    pub insertion_id: String,
    /// Edit kind.
    pub kind: EditKind,
    /// Inclusive derived UTF-16 start.
    pub start_utf16: u32,
    /// Inserted length in UTF-16 code units.
    pub length_utf16: u32,
    /// Replaced original length in UTF-16 code units.
    pub replace_length_utf16: u32,
}

/// One generated declaration mapped back to its original script range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedDeclaration {
    /// Declared name.
    pub name: String,
    /// Inclusive derived UTF-16 start.
    pub start_utf16: u32,
    /// Name length in UTF-16 code units.
    pub length_utf16: u32,
    /// Original script byte range of the declaration.
    pub source: Option<SourceRange>,
}

/// Offsets of probe helpers in the derived script.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProbeAnchors {
    /// Probe method name offset.
    pub probe_offset: u32,
    /// `data` identifier offset in `this.data`.
    pub root_offset: u32,
    /// Properties value offset, when an overlay exists.
    pub props_offset: Option<u32>,
    /// Named helper offsets.
    pub helpers: Vec<(&'static str, u32)>,
}

/// Derived script plus every mapping artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedView {
    /// Original script text.
    pub original: String,
    /// Derived script text.
    pub text: String,
    /// Applied edits in original order.
    pub edits: Vec<AppliedEdit>,
    /// Public insertion records.
    pub insertions: Vec<InsertionRecord>,
    /// Public mount records.
    pub mounts: Vec<MountRecord>,
    /// Generated declarations.
    pub generated_declarations: Vec<GeneratedDeclaration>,
    /// Probe helper offsets.
    pub probe_anchors: ProbeAnchors,
    /// Original ⇄ derived offset map.
    pub map: ShadowMap,
}

/// Complete result of one in-place shadow build.
#[derive(Debug, Clone)]
pub struct BuiltShadow {
    /// Script path.
    pub script_path: String,
    /// Build status.
    pub status: BuildStatus,
    /// Combined partial reason.
    pub reason: Option<String>,
    /// Explicit notes for covered gaps and pragma edits.
    pub notes: Vec<String>,
    /// Effective JS checking policy.
    pub js_check_applied: JsCheckApplied,
    /// Selected registration.
    pub registration: RegistrationInfo,
    /// Data-root classification.
    pub data_root: DataRootKind,
    /// Syntactic properties scan.
    pub properties: PropertyScan,
    /// Derived view and mappings.
    pub view: DerivedView,
}

#[derive(Debug, Clone)]
struct Registration {
    kind: RootKind,
    options_open: usize,
    members: Vec<Member>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberKind {
    Property,
    Method,
    Shorthand,
    Spread,
    Computed,
    Accessor,
    Unknown,
}

#[derive(Debug, Clone)]
struct Member {
    name: Option<String>,
    kind: MemberKind,
    key_start: usize,
    key_end: usize,
    value_start: usize,
    value_end: usize,
}

impl Member {
    fn key_range(&self) -> SourceRange {
        SourceRange::new(
            u32::try_from(self.key_start).unwrap_or(u32::MAX),
            u32::try_from(self.key_end).unwrap_or(u32::MAX),
        )
    }
}

struct Script<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    pairs: Vec<Option<usize>>,
}

impl<'a> Script<'a> {
    fn new(text: &'a str) -> Result<Self, ShadowError> {
        let tokens = expression::lex_script(text).map_err(script_scan_error)?;
        let pairs = expression::matching_pairs_for_script(&tokens).map_err(script_scan_error)?;
        Ok(Self {
            text,
            tokens,
            pairs,
        })
    }

    fn kind(&self, index: usize) -> TokenKind {
        self.tokens[index].kind
    }

    fn token_text(&self, index: usize) -> &'a str {
        self.tokens[index].text(self.text)
    }

    fn close_of(&self, index: usize) -> Result<usize, ShadowError> {
        self.pairs[index].ok_or_else(|| ShadowError::Invalid {
            feature: "script.scan",
            reason: "unbalanced delimiter".to_owned(),
        })
    }

    fn is_punct(&self, index: usize, character: char) -> bool {
        self.tokens
            .get(index)
            .is_some_and(|token| token.kind == TokenKind::Punctuator(character))
    }

    fn is_ident(&self, index: usize, name: &str) -> bool {
        self.tokens.get(index).is_some_and(|token| {
            token.kind == TokenKind::Identifier && token.text(self.text) == name
        })
    }
}

/// Builds the in-place shadow for one registration and its fragments.
#[allow(clippy::too_many_lines)] // one linear assembly pass; split only with a contract change
pub fn build_in_place_shadow(request: &ShadowRequest<'_>) -> Result<BuiltShadow, ShadowError> {
    build_in_place_shadow_with_behavior(request, None)
}

/// Builds the in-place shadow with an explicit behavior properties overlay.
///
/// `behavior` is the statically resolved behavior contribution. Passing `None`
/// keeps the historical behavior of [`build_in_place_shadow`]: a non-empty
/// `behaviors` member is reported as `Partial` because the shadow cannot know
/// which properties it contributes.
///
/// # Errors
///
/// Returns the same errors as [`build_in_place_shadow`].
pub fn build_in_place_shadow_with_behavior(
    request: &ShadowRequest<'_>,
    behavior: Option<&BehaviorPropertyOverlay<'_>>,
) -> Result<BuiltShadow, ShadowError> {
    build_in_place_shadow_inner(request, behavior)
}

#[allow(clippy::too_many_lines)] // one linear assembly pass; split only with a contract change
fn build_in_place_shadow_inner(
    request: &ShadowRequest<'_>,
    behavior: Option<&BehaviorPropertyOverlay<'_>>,
) -> Result<BuiltShadow, ShadowError> {
    let script = Script::new(request.script.text())?;
    let wanted = match request.root {
        RootKind::Page => &[RootKind::Page, RootKind::Component][..],
        RootKind::Component => &[RootKind::Component][..],
        RootKind::Behavior => &[RootKind::Behavior][..],
    };
    let mut registrations: Vec<Registration> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    for kind in wanted {
        let (found, rejected) = find_registrations(&script, *kind);
        unsupported.extend(rejected);
        if !found.is_empty() {
            registrations = found;
            break;
        }
    }
    if registrations.is_empty() {
        let detail = if unsupported.is_empty() {
            String::new()
        } else {
            format!("; saw {}", unsupported.join(", "))
        };
        return Err(ShadowError::Unsupported {
            feature: "registration",
            reason: format!(
                "no {} registration with an object literal argument{detail}",
                request.root.label()
            ),
        });
    }
    let registration_count = registrations.len();
    let registration = registrations.remove(0);
    let mut notes: Vec<String> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();
    if registration_count > 1 {
        reasons.push(format!(
            "{registration_count} {} registrations; the probe mounts the first one",
            registration.kind.label()
        ));
        notes.push(
            "multi-registration: candidate selection is not frozen, results are reported as partial"
                .to_owned(),
        );
    }

    let data_root = classify_data_root(&script, &registration);
    match data_root {
        DataRootKind::TypeReference => {
            return Err(ShadowError::Unsupported {
                feature: "data.root",
                reason: "data root is a type-only reference (interface/type alias); no runtime data object exists"
                    .to_owned(),
            });
        }
        DataRootKind::Unsupported => {
            return Err(ShadowError::Unsupported {
                feature: "data.root",
                reason:
                    "`data` member is not a property assignment; the template root cannot be bound"
                        .to_owned(),
            });
        }
        DataRootKind::Absent => {
            reasons
                .push("registration has no `data` member; the template root is empty".to_owned());
        }
        DataRootKind::Expression => {
            reasons.push(
                "`data` member is not an inline object literal; field-level roots are not enumerated"
                    .to_owned(),
            );
            notes.push(
                "the probe still uses `this.data`; only the field list is unknown".to_owned(),
            );
        }
        DataRootKind::Object => {}
    }

    let mut properties = scan_properties(&script, &registration);
    if let Some(overlay) = behavior {
        merge_behavior_properties(&mut properties, request.script.path(), overlay);
    }
    if !properties.complete {
        reasons.push(format!(
            "component properties are incomplete: {}",
            properties.problems.join("; ")
        ));
        notes.push(
            "optionalTypes coverage may be under-reported because `properties` is not a plain literal"
                .to_owned(),
        );
    }
    let behaviors_present = member_last(&registration.members, "behaviors").is_some();
    if let Some(overlay) = behavior {
        if behaviors_present && !overlay.complete {
            reasons.push(format!(
                "component behaviors are partially resolved: {}",
                overlay
                    .reason
                    .unwrap_or("the static behavior graph is incomplete")
            ));
            notes.push(
                "behavior properties that cannot be resolved statically are not checked".to_owned(),
            );
        }
    } else if behaviors_incomplete(&script, &registration) {
        reasons.push(
            "component behaviors are partially resolved: behavior graph resolution is not implemented in the Rust shadow slice"
                .to_owned(),
        );
        notes.push(
            "behavior data/properties that cannot be resolved statically are not checked"
                .to_owned(),
        );
    }

    let property_names: BTreeSet<String> = properties
        .properties
        .iter()
        .map(|property| property.name.clone())
        .collect();
    let syntax = if request.script.language().is_javascript() {
        Syntax::JavaScript
    } else {
        Syntax::TypeScript
    };
    let is_javascript = request.script.language().is_javascript();
    let want_pragma =
        request.js_check == JsCheckPolicy::Pragma && !request.check_js && is_javascript;
    let pragma = scan_pragmas(request.script.text(), want_pragma);
    let js_check_applied = effective_js_check(
        request.js_check,
        JsCheckInput {
            check_js: request.check_js,
            is_javascript,
            pragma_check: pragma.check,
            pragma_no_check: pragma.no_check,
            want_pragma,
        },
        &mut notes,
    );

    let mut body = BodyBuilder::new();
    body.push("\n    ");
    body.mark_probe = body.length_utf16;
    body.push(IN_PLACE_PROBE);
    body.push("() {\n");
    body.push(BODY_INDENT);
    body.push("const ");
    body.mark_root_name = body.length_utf16;
    body.push(ROOT_NAME);
    body.push(" = this.");
    body.mark_root_expression = body.length_utf16;
    body.push("data;\n");

    if !properties.properties.is_empty() {
        if syntax == Syntax::TypeScript {
            let (rendered, members) = render_props_type(&properties.properties, BODY_INDENT);
            let start = body.length_utf16;
            body.push(&rendered);
            body.push("\n");
            for member in members {
                if let Some(source) = member.source {
                    body.generated.push(GeneratedDraft {
                        name: member.name.clone(),
                        start_utf16: start + member.offset,
                        length_utf16: utf16_len(&member.name),
                        source: Some(source),
                    });
                }
            }
        }
        let (rendered, members) = render_props_value(&properties.properties, BODY_INDENT, syntax);
        let start = body.length_utf16;
        body.push(&rendered);
        body.push("\n");
        if let Some(offset) = find_utf16(&rendered, PROPS_VALUE_NAME) {
            body.mark_props = Some(start + offset);
        }
        for member in members {
            if let Some(source) = member.source {
                body.generated.push(GeneratedDraft {
                    name: member.name.clone(),
                    start_utf16: start + member.offset,
                    length_utf16: utf16_len(&member.name),
                    source: Some(source),
                });
            }
        }
    }

    let has_bindings = request
        .fragments
        .iter()
        .any(|fragment| !fragment.bindings.is_empty());
    if has_bindings {
        let helpers = local_helpers(syntax);
        if !helpers.is_empty() {
            body.push(&indent_lines(helpers, BODY_INDENT));
            body.push("\n");
        }
    }

    let mut unresolved_bindings = 0_usize;
    let mut unresolved_guards = 0_usize;
    let mut seen_ids = BTreeSet::new();
    for fragment in request.fragments {
        if fragment.id.is_empty() || !seen_ids.insert(fragment.id.as_str()) {
            return Err(ShadowError::Invalid {
                feature: "fragment.id",
                reason: "fragment ids must be non-empty and unique".to_owned(),
            });
        }
        validate_fragment_range(fragment)?;
        let planned = plan_fragment(fragment, &property_names, syntax)?;
        let depth = if fragment.bindings.is_empty() {
            0
        } else {
            group_local_levels(&fragment.bindings, DEFAULT_LOCAL_BUDGET)?.len()
        };
        let fragment_indent = format!("{BODY_INDENT}  ");
        let mut scope = ScopeBuilder::new(&fragment_indent, depth);
        if fragment.bindings.is_empty() {
            if fragment.locals.len() > DEFAULT_LOCAL_BUDGET.max_bindings {
                return Err(ShadowError::BudgetExceeded { budget: "locals" });
            }
            let mut names: Vec<String> = Vec::new();
            let mut dropped = 0_usize;
            for name in &fragment.locals {
                if !expression::is_binding_identifier(name) {
                    dropped += 1;
                    continue;
                }
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            if names.len() > DEFAULT_LOCAL_BUDGET.max_bindings {
                return Err(ShadowError::BudgetExceeded { budget: "locals" });
            }
            if dropped > 0 {
                reasons.push(format!(
                    "fragment `{}` has {dropped} invalid local name(s) that were dropped",
                    fragment.id
                ));
                notes.push(
                    "invalid legacy `locals` names are not emitted into the probe".to_owned(),
                );
                unresolved_bindings += dropped;
            }
            if !names.is_empty() || dropped > 0 {
                let declarations = local_declarations(&names, &fragment_indent, syntax);
                scope.local_text = declarations;
                scope.unresolved = true;
                unresolved_bindings += names.len();
            }
        } else {
            if fragment.bindings.len() > DEFAULT_LOCAL_BUDGET.max_bindings {
                return Err(ShadowError::BudgetExceeded { budget: "locals" });
            }
            let invalid_bindings = fragment
                .bindings
                .iter()
                .filter(|binding| !expression::is_binding_identifier(&binding.name))
                .count();
            if invalid_bindings > 0 {
                reasons.push(format!(
                    "fragment `{}` has {invalid_bindings} invalid `wx:for` binding name(s) that were dropped",
                    fragment.id
                ));
                notes.push(
                    "invalid `wx:for` binding names are not emitted into the probe".to_owned(),
                );
                unresolved_bindings += invalid_bindings;
                scope.unresolved = true;
            }
            let locals = render_locals(
                &fragment.bindings,
                &fragment_indent,
                syntax,
                DEFAULT_LOCAL_BUDGET,
                &mut |expr, outer| rewrite_source(expr, outer, &property_names),
            )?;
            if locals.unresolved {
                unresolved_bindings += fragment.bindings.len();
                scope.unresolved = true;
            }
            scope.local_text = locals.text;
            scope.local_close = locals.close;
        }
        let local_names = effective_local_names(fragment);
        for guard in &fragment.guards {
            let planned_guards = render_guards_planned(
                guard,
                &scope.guard_indent(scope.opened_guards),
                &mut |expr| rewrite_condition(expr, &property_names, &local_names),
            );
            if planned_guards.rendered.unresolved {
                unresolved_guards += 1;
                scope.unresolved = true;
            }
            if !planned_guards.rendered.open.is_empty() {
                scope.opened_guards += 1;
            }
            scope.guards.push(planned_guards);
        }

        // A guard wrapper is generated text. Fold the rendered guard block into
        // this fragment's mount when every rendered condition has a provable
        // original range; a checker diagnostic on the `if (...)` copy then maps
        // to the same WXML bytes as the standalone condition fragment instead
        // of being dropped as an unmapped generated range.
        let guard_text = scope.guard_text();
        let guards_start = body.length_utf16 + scope.open_prefix_len();
        let mut guard_offsets = Vec::with_capacity(scope.guards.len());
        let mut guard_offset = 0_u32;
        for guard in &scope.guards {
            guard_offsets.push(guard_offset);
            guard_offset += utf16_len(&guard.rendered.open);
        }
        let mut guard_segments: Vec<MountedSegment> = Vec::new();
        let mut fold = !guard_text.is_empty();
        let mut rendered_conditions = 0_usize;
        for (chain_index, guard) in scope.guards.iter().enumerate() {
            for condition in &guard.conditions {
                rendered_conditions += 1;
                let Some(expr) = fragment
                    .guards
                    .get(chain_index)
                    .and_then(|chain| chain.conditions.get(condition.condition_index))
                    .map(|condition| condition.expr.as_str())
                else {
                    fold = false;
                    continue;
                };
                let Some(source) = guard_condition_source(
                    request.fragments,
                    fragment,
                    chain_index,
                    expr,
                    &property_names,
                    &condition.text,
                ) else {
                    fold = false;
                    continue;
                };
                let origin = source.start.get();
                let base = guard_offsets[chain_index] + condition.open_start_utf16;
                for segment in &condition.segments {
                    guard_segments.push(MountedSegment {
                        start_utf16: base + segment.start_utf16,
                        length_utf16: segment.length_utf16,
                        source: segment.source.map(|source| {
                            SourceRange::new(
                                origin.saturating_add(source.start.get()),
                                origin.saturating_add(source.end.get()),
                            )
                        }),
                    });
                }
            }
        }
        if rendered_conditions == 0 || guard_segments.is_empty() {
            fold = false;
        }

        body.push(&scope.open());
        body.push(scope.mount_indent());
        let expression_start = body.length_utf16;
        body.push(&planned.rewritten.text);
        let mount_start = if fold { guards_start } else { expression_start };
        let prefix_len = expression_start.saturating_sub(mount_start);
        let origin = fragment.wxml_range.start.get();
        let mut segments: Vec<MountedSegment> = if fold { guard_segments } else { Vec::new() };
        segments.extend(
            planned
                .rewritten
                .segments
                .iter()
                .map(|segment| MountedSegment {
                    start_utf16: prefix_len + segment.start_utf16,
                    length_utf16: segment.length_utf16,
                    source: segment.source.map(|source| {
                        SourceRange::new(
                            origin.saturating_add(source.start.get()),
                            origin.saturating_add(source.end.get()),
                        )
                    }),
                }),
        );
        let mount_text = if fold {
            format!(
                "{guard_text}{}{}",
                scope.mount_indent(),
                planned.rewritten.text
            )
        } else {
            planned.rewritten.text.clone()
        };
        body.fragments.push(PendingFragment {
            fragment_id: fragment.id.clone(),
            relative_start_utf16: mount_start,
            text: mount_text,
            target_offset_utf16: prefix_len + planned.rewritten.target_offset_utf16,
            segments,
        });
        body.push(";\n");
        body.push(&scope.close());
    }
    body.push(METHOD_INDENT);
    body.push("},\n  ");

    let placement = probe_placement(&script, &registration)?;
    let (probe_text, wrapper_shift) = if placement.wrap_in_methods {
        (
            format!("\n    methods: {{\n  {}    }},\n  ", body.text),
            utf16_len("\n    methods: {\n  "),
        )
    } else {
        (body.text.clone(), 0)
    };

    if unresolved_bindings > 0 || unresolved_guards > 0 {
        reasons.push(
            "some template bindings or guards could not be resolved and were not checked"
                .to_owned(),
        );
        notes.push(
            "unresolved `wx:for` bindings or `wx:if` guards are mounted as `any`/unnarrowed and not checked"
                .to_owned(),
        );
    }

    let mut edits: Vec<ShadowEdit> = Vec::new();
    let must_drop_bom = pragma.bom_length_utf16 > 0
        && (js_check_applied == JsCheckApplied::Pragma || pragma.check || pragma.no_check);
    if must_drop_bom {
        edits.push(ShadowEdit::replace("bom", EditKind::Bom, 0, 1, ""));
        notes.push(
            "BOM dropped in the derived view so pragmas are recognised after the shebang"
                .to_owned(),
        );
    }
    if js_check_applied == JsCheckApplied::Pragma {
        edits.push(ShadowEdit::insert(
            "pragma",
            EditKind::Pragma,
            byte_to_utf16(request.script.text(), pragma.injection_offset)?,
            "// @ts-check\n",
        ));
        notes.push(
            "shadow-only `// @ts-check` enables JS checking without changing project options"
                .to_owned(),
        );
    }
    if js_check_applied == JsCheckApplied::Off && pragma.check {
        edits.push(ShadowEdit::insert(
            "nocheck",
            EditKind::Pragma,
            byte_to_utf16(request.script.text(), pragma.neutralize_offset)?,
            "// @ts-nocheck\n",
        ));
        notes.push(
            "source `@ts-check` neutralised with a shadow-only `@ts-nocheck` (policy off)"
                .to_owned(),
        );
    }
    edits.push(ShadowEdit::insert(
        "probe",
        EditKind::Probe,
        byte_to_utf16(request.script.text(), placement.at_byte)?,
        probe_text,
    ));
    let assembled = apply_edits(request.script.text(), &edits)?;
    let probe_insertion = assembled
        .edits
        .iter()
        .find(|edit| edit.id == "probe")
        .ok_or_else(|| ShadowError::Invalid {
            feature: "layout.probe",
            reason: "probe insertion missing after assembly".to_owned(),
        })?;
    let probe_start = probe_insertion.inserted_start_utf16;
    let base = probe_start + wrapper_shift;

    let mut mounts: Vec<MountRecord> = Vec::new();
    for pending in &body.fragments {
        let mount_start = base + pending.relative_start_utf16;
        let segments = pending
            .segments
            .iter()
            .map(|segment| MountedSegment {
                start_utf16: mount_start + segment.start_utf16,
                length_utf16: segment.length_utf16,
                source: segment.source,
            })
            .collect::<Vec<_>>();
        mounts.push(MountRecord {
            mount_id: format!("{}#0", pending.fragment_id),
            fragment_id: pending.fragment_id.clone(),
            insertion_id: "probe".to_owned(),
            text: pending.text.clone(),
            start_utf16: mount_start,
            target_start_utf16: mount_start + pending.target_offset_utf16,
            target_length_utf16: target_length(&pending.text, pending.target_offset_utf16),
            segments,
        });
    }

    let generated = body
        .generated
        .iter()
        .map(|declaration| GeneratedDeclaration {
            name: declaration.name.clone(),
            start_utf16: base + declaration.start_utf16,
            length_utf16: declaration.length_utf16,
            source: declaration.source,
        })
        .collect();

    let probe_offset = base + body.mark_probe;
    let root_offset = base + body.mark_root_expression;
    let props_offset = body.mark_props.map(|offset| base + offset);
    let mut helpers = vec![
        (IN_PLACE_PROBE, probe_offset),
        (ROOT_NAME, base + body.mark_root_name),
    ];
    if let Some(offset) = props_offset {
        helpers.push((PROPS_VALUE_NAME, offset));
    }

    let insertions = assembled
        .edits
        .iter()
        .map(|edit| InsertionRecord {
            insertion_id: edit.id.clone(),
            kind: edit.kind,
            start_utf16: edit.inserted_start_utf16,
            length_utf16: edit.inserted_length_utf16,
            replace_length_utf16: edit.replace_length_utf16,
        })
        .collect();
    let map = ShadowMap::new(request.script.text(), &assembled);
    let view = DerivedView {
        original: request.script.text().to_owned(),
        text: assembled.text.clone(),
        edits: assembled.edits,
        insertions,
        mounts,
        generated_declarations: generated,
        probe_anchors: ProbeAnchors {
            probe_offset,
            root_offset,
            props_offset,
            helpers,
        },
        map,
    };

    Ok(BuiltShadow {
        script_path: request.script.path().to_owned(),
        status: if reasons.is_empty() {
            BuildStatus::Supported
        } else {
            BuildStatus::Partial
        },
        reason: if reasons.is_empty() {
            None
        } else {
            Some(reasons.join("; "))
        },
        notes,
        js_check_applied,
        registration: RegistrationInfo {
            kind: registration.kind,
            count: registration_count,
        },
        data_root,
        properties,
        view,
    })
}

fn target_length(text: &str, target_offset_utf16: u32) -> u32 {
    let Ok(offset) = utf16_to_byte(text, target_offset_utf16) else {
        return 0;
    };
    text.get(offset..)
        .and_then(|rest| {
            let mut characters = rest.chars();
            let first = characters.next()?;
            if first == '_' || first == '$' || first.is_alphabetic() {
                let mut length = first.len_utf16();
                for character in characters {
                    if character == '_' || character == '$' || character.is_alphanumeric() {
                        length += character.len_utf16();
                    } else {
                        break;
                    }
                }
                Some(u32::try_from(length).unwrap_or(u32::MAX))
            } else {
                None
            }
        })
        .unwrap_or(0)
}

fn script_scan_error(error: ShadowError) -> ShadowError {
    match error {
        ShadowError::BadExpression { detail } => ShadowError::Invalid {
            feature: "script.scan",
            reason: detail,
        },
        other => other,
    }
}

fn validate_fragment_range(fragment: &FragmentSpec) -> Result<(), ShadowError> {
    let length = u32::try_from(fragment.expr.len()).unwrap_or(u32::MAX);
    if fragment.wxml_range.len() != length {
        return Err(ShadowError::Invalid {
            feature: "fragment.range",
            reason: format!(
                "wxmlRange length {} does not match expression byte length {length}",
                fragment.wxml_range.len()
            ),
        });
    }
    Ok(())
}

struct PlannedFragment {
    rewritten: RewrittenExpression,
}

/// Effective local names of one fragment, in the same order as [`RewriteOptions`].
fn effective_local_names(fragment: &FragmentSpec) -> BTreeSet<String> {
    if fragment.bindings.is_empty() {
        fragment
            .locals
            .iter()
            .filter(|name| expression::is_binding_identifier(name))
            .cloned()
            .collect()
    } else {
        fragment
            .bindings
            .iter()
            .filter(|binding| expression::is_binding_identifier(&binding.name))
            .map(|binding| binding.name.clone())
            .collect()
    }
}

fn plan_fragment(
    fragment: &FragmentSpec,
    property_names: &BTreeSet<String>,
    syntax: Syntax,
) -> Result<PlannedFragment, ShadowError> {
    let options = RewriteOptions {
        root_prefix: ROOT_PREFIX.to_owned(),
        props_prefix: PROPS_PREFIX.to_owned(),
        property_overlay: property_names.clone(),
        data_overlay: BTreeSet::new(),
        locals: effective_local_names(fragment),
    };
    let rewritten = rewrite_expression(&fragment.expr, fragment.target.as_deref(), &options)?;
    let _ = syntax;
    Ok(PlannedFragment { rewritten })
}

fn rewrite_source(
    expr: &str,
    outer: &BTreeSet<String>,
    property_names: &BTreeSet<String>,
) -> Option<String> {
    let options = RewriteOptions {
        root_prefix: ROOT_PREFIX.to_owned(),
        props_prefix: PROPS_PREFIX.to_owned(),
        property_overlay: property_names.clone(),
        data_overlay: BTreeSet::new(),
        locals: outer.clone(),
    };
    rewrite_expression(expr, None, &options)
        .ok()
        .map(|rewritten| rewritten.text)
}

fn rewrite_condition(
    expr: &str,
    property_names: &BTreeSet<String>,
    locals: &BTreeSet<String>,
) -> Option<RewrittenExpression> {
    let options = RewriteOptions {
        root_prefix: ROOT_PREFIX.to_owned(),
        props_prefix: PROPS_PREFIX.to_owned(),
        property_overlay: property_names.clone(),
        data_overlay: BTreeSet::new(),
        locals: locals.clone(),
    };
    rewrite_expression(expr, None, &options).ok()
}

/// Returns the original WXML range of one rendered guard condition.
///
/// The template analyzer emits every `wx:if`/`wx:elif` mustache as its own
/// fragment, but `FragmentSpec::guards` only carries the condition text (the
/// `wxml_types::GuardCondition::range` is not handed to the shadow). A wrapper
/// mount may therefore only use a range that is provably the same expression:
///
/// - exactly one other fragment has the same expression text and byte length;
/// - its enclosing guard chains equal this chain's outer chains;
/// - rewriting it with its own scope produces exactly the rendered text.
///
/// Any ambiguity returns `None`; the wrapper copy is then left unmapped instead
/// of being attributed to a guessed range.
fn guard_condition_source(
    fragments: &[FragmentSpec],
    owner: &FragmentSpec,
    chain_index: usize,
    expr: &str,
    property_names: &BTreeSet<String>,
    expected_text: &str,
) -> Option<SourceRange> {
    let outer = owner.guards.get(..chain_index)?;
    let expected_bytes = u32::try_from(expr.len()).ok()?;
    let mut found: Option<SourceRange> = None;
    for candidate in fragments {
        if candidate.id == owner.id || candidate.expr != expr {
            continue;
        }
        if candidate.guards.as_slice() != outer {
            continue;
        }
        if candidate.wxml_range.len() != expected_bytes {
            continue;
        }
        let rewritten = rewrite_condition(
            &candidate.expr,
            property_names,
            &effective_local_names(candidate),
        )?;
        if rewritten.text != expected_text {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(candidate.wxml_range);
    }
    found
}

struct ScopeBuilder {
    open_line: String,
    local_text: String,
    local_close: String,
    guards: Vec<PlannedGuards>,
    guard_base: String,
    opened_guards: usize,
    mount_indent: String,
    unresolved: bool,
}

impl ScopeBuilder {
    fn new(fragment_indent: &str, depth: usize) -> Self {
        let guard_base = format!("{fragment_indent}{}", "  ".repeat(depth));
        Self {
            open_line: format!("{BODY_INDENT}{{"),
            local_text: String::new(),
            local_close: String::new(),
            guards: Vec::new(),
            mount_indent: guard_base.clone(),
            guard_base,
            opened_guards: 0,
            unresolved: false,
        }
    }

    fn guard_indent(&self, opened: usize) -> String {
        format!("{}{}", self.guard_base, "  ".repeat(opened))
    }

    fn mount_indent(&self) -> &str {
        &self.mount_indent
    }

    fn open_prefix(&self) -> String {
        format!("{}\n{}", self.open_line, self.local_text)
    }

    fn open_prefix_len(&self) -> u32 {
        utf16_len(&self.open_line) + 1 + utf16_len(&self.local_text)
    }

    fn guard_text(&self) -> String {
        let mut text = String::new();
        for guard in &self.guards {
            text.push_str(&guard.rendered.open);
        }
        if self.unresolved {
            text.push_str(&format!("{}// @ts-ignore\n", self.mount_indent));
        }
        text
    }

    fn open(&self) -> String {
        format!("{}{}", self.open_prefix(), self.guard_text())
    }

    fn close(&self) -> String {
        let mut text = String::new();
        for guard in self.guards.iter().rev() {
            text.push_str(&guard.rendered.close);
        }
        text.push_str(&self.local_close);
        text.push_str(&format!("{BODY_INDENT}}}\n"));
        text
    }
}

struct PendingFragment {
    fragment_id: String,
    relative_start_utf16: u32,
    text: String,
    target_offset_utf16: u32,
    segments: Vec<MountedSegment>,
}

struct GeneratedDraft {
    name: String,
    start_utf16: u32,
    length_utf16: u32,
    source: Option<SourceRange>,
}

struct BodyBuilder {
    text: String,
    length_utf16: u32,
    fragments: Vec<PendingFragment>,
    generated: Vec<GeneratedDraft>,
    mark_probe: u32,
    mark_root_name: u32,
    mark_root_expression: u32,
    mark_props: Option<u32>,
}

impl BodyBuilder {
    const fn new() -> Self {
        Self {
            text: String::new(),
            length_utf16: 0,
            fragments: Vec::new(),
            generated: Vec::new(),
            mark_probe: 0,
            mark_root_name: 0,
            mark_root_expression: 0,
            mark_props: None,
        }
    }

    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.length_utf16 += utf16_len(text);
    }
}

struct RenderedMemberInfo {
    name: String,
    offset: u32,
    source: Option<SourceRange>,
}

fn render_props_type(
    properties: &[DeclaredProperty],
    indent: &str,
) -> (String, Vec<RenderedMemberInfo>) {
    let mut text = format!("{indent}type {PROPS_TYPE_NAME} = {{\n");
    let mut members = Vec::new();
    for property in properties {
        if !property.doc.is_empty() {
            text.push_str(&format!(
                "{indent}  /** {} */\n",
                sanitize_doc(&property.doc)
            ));
        }
        text.push_str(&format!("{indent}  "));
        let name_text = quote_name(&property.name);
        let quoted = name_text.len() != property.name.len();
        let offset = utf16_len(&text) + u32::from(quoted);
        members.push(RenderedMemberInfo {
            name: property.name.clone(),
            offset,
            source: property.source,
        });
        text.push_str(&format!("{name_text}: {};\n", property.type_text));
    }
    text.push_str(&format!("{indent}}};"));
    (text, members)
}

fn render_props_value(
    properties: &[DeclaredProperty],
    indent: &str,
    syntax: Syntax,
) -> (String, Vec<RenderedMemberInfo>) {
    match syntax {
        Syntax::TypeScript => (
            format!("{indent}const {PROPS_VALUE_NAME} = null as unknown as {PROPS_TYPE_NAME};"),
            Vec::new(),
        ),
        Syntax::JavaScript => {
            let mut text =
                format!("{indent}/**\n{indent} * @typedef {{Object}} {PROPS_TYPE_NAME}\n");
            let mut members = Vec::new();
            for property in properties {
                text.push_str(&format!("{indent} * @property {{{}}} ", property.type_text));
                let name_text = quote_name(&property.name);
                let quoted = name_text.len() != property.name.len();
                let offset = utf16_len(&text) + u32::from(quoted);
                members.push(RenderedMemberInfo {
                    name: property.name.clone(),
                    offset,
                    source: property.source,
                });
                let doc = if property.doc.is_empty() {
                    String::new()
                } else {
                    format!(" {}", sanitize_doc(&property.doc))
                };
                text.push_str(&format!("{name_text}{doc}\n"));
            }
            text.push_str(&format!("{indent} */\n"));
            text.push_str(&format!(
                "{indent}/** @type {{unknown}} */\n{indent}const {PROPS_RAW_NAME} = null;\n"
            ));
            text.push_str(&format!(
                "{indent}const {PROPS_VALUE_NAME} = /** @type {{{PROPS_TYPE_NAME}}} */ ({PROPS_RAW_NAME});"
            ));
            (text, members)
        }
    }
}

fn sanitize_doc(doc: &str) -> String {
    doc.replace("*/", "*\\/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_name(name: &str) -> String {
    if expression::is_identifier_name(name) {
        name.to_owned()
    } else {
        let mut quoted = String::from("\"");
        for character in name.chars() {
            match character {
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                '\n' => quoted.push_str("\\n"),
                '\r' => quoted.push_str("\\r"),
                '\t' => quoted.push_str("\\t"),
                character if character.is_control() => {
                    quoted.push_str(&format!("\\u{:04x}", u32::from(character)));
                }
                character => quoted.push(character),
            }
        }
        quoted.push('"');
        quoted
    }
}

fn find_utf16(text: &str, needle: &str) -> Option<u32> {
    text.find(needle).map(|offset| utf16_len(&text[..offset]))
}

#[derive(Debug, Clone)]
struct ProbePlacement {
    at_byte: usize,
    wrap_in_methods: bool,
}

fn probe_placement(
    script: &Script<'_>,
    registration: &Registration,
) -> Result<ProbePlacement, ShadowError> {
    if registration.kind == RootKind::Page {
        return Ok(ProbePlacement {
            at_byte: script.tokens[registration.options_open].end,
            wrap_in_methods: false,
        });
    }
    if let Some(methods) = member_last(&registration.members, "methods") {
        if methods.kind != MemberKind::Property {
            return Err(ShadowError::Unsupported {
                feature: "component.methods",
                reason: "component `methods` is not a property assignment".to_owned(),
            });
        }
        if let Some((open, _close)) = object_bounds(script, methods) {
            return Ok(ProbePlacement {
                at_byte: script.tokens[open].end,
                wrap_in_methods: false,
            });
        }
        return Err(ShadowError::Unsupported {
            feature: "component.methods",
            reason:
                "component `methods` is not an object literal; the in-place probe cannot be mounted"
                    .to_owned(),
        });
    }
    Ok(ProbePlacement {
        at_byte: script.tokens[registration.options_open].end,
        wrap_in_methods: true,
    })
}

fn find_registrations(script: &Script<'_>, kind: RootKind) -> (Vec<Registration>, Vec<String>) {
    let name = kind.registrar();
    if registrar_shadowed(script, name) {
        return (Vec::new(), vec![format!("{name} (shadowed identifier)")]);
    }
    let mut found = Vec::new();
    let mut rejected = Vec::new();
    for index in 0..script.tokens.len() {
        if !script.is_ident(index, name) {
            continue;
        }
        if index > 0 {
            let previous = script.kind(index - 1);
            if previous == TokenKind::Punctuator('.') {
                continue;
            }
            if script.is_ident(index - 1, "new")
                || script.is_ident(index - 1, "function")
                || script.is_ident(index - 1, "class")
            {
                continue;
            }
        }
        if !script.is_punct(index + 1, '(') {
            continue;
        }
        let argument = index + 2;
        if argument >= script.tokens.len() || !script.is_punct(argument, '{') {
            rejected.push(format!("{name} (argument is not an object literal)"));
            continue;
        }
        let Ok(close) = script.close_of(argument) else {
            rejected.push(format!("{name} (unbalanced object literal)"));
            continue;
        };
        let after = close + 1;
        if !(script.is_punct(after, ',') || script.is_punct(after, ')')) {
            rejected.push(format!("{name} (argument is not an entire object literal)"));
            continue;
        }
        let members = scan_members(script, argument + 1, close);
        found.push(Registration {
            kind,
            options_open: argument,
            members,
        });
    }
    (found, rejected)
}

fn registrar_shadowed(script: &Script<'_>, name: &str) -> bool {
    for index in 0..script.tokens.len() {
        if !script.tokens[index].kind.eq(&TokenKind::Identifier) {
            continue;
        }
        let token = script.token_text(index);
        if matches!(token, "function" | "class" | "const" | "let" | "var")
            && script.is_ident(index + 1, name)
        {
            return true;
        }
        if token == "import" {
            let mut cursor = index + 1;
            if script.is_punct(cursor, '(') {
                continue; // dynamic import is not a declaration
            }
            let mut depth = 0_usize;
            let mut previous_end = script.tokens[index].end;
            let mut scanned = 0_usize;
            while cursor < script.tokens.len() {
                scanned += 1;
                if scanned > MAX_IMPORT_SCAN_TOKENS {
                    return true; // malformed import: fail closed as shadowed
                }
                let token = script.tokens[cursor];
                if cursor > index + 1
                    && depth == 0
                    && has_line_break(script.text, previous_end, token.start)
                    && !import_continuation(script, cursor)
                {
                    break;
                }
                match token.kind {
                    TokenKind::Punctuator(';') | TokenKind::StringLiteral => break,
                    TokenKind::Punctuator('{' | '(' | '[') | TokenKind::TemplateExpressionStart => {
                        depth += 1;
                    }
                    TokenKind::Punctuator('}' | ')' | ']') | TokenKind::TemplateExpressionEnd => {
                        depth = depth.saturating_sub(1);
                    }
                    TokenKind::Identifier if script.token_text(cursor) == name => return true,
                    _ => {}
                }
                previous_end = token.end;
                cursor += 1;
            }
        }
    }
    // A single-parameter arrow `Page => ...` shadows the registrar.
    for index in 0..script.tokens.len() {
        if script.is_ident(index, name)
            && script.is_punct(index + 1, '=')
            && script.is_punct(index + 2, '>')
            && !(index > 0 && script.is_punct(index - 1, '.'))
        {
            return true;
        }
    }
    // Parameter detection is limited to actual function/arrow parameter lists,
    // not control-flow conditions such as `if (Page)`.
    for index in 0..script.tokens.len() {
        if !script.is_punct(index, '(') {
            continue;
        }
        let Some(close) = script.pairs[index] else {
            continue;
        };
        let arrow = script.is_punct(close + 1, '=') && script.is_punct(close + 2, '>');
        let function = function_keyword_before(script, index);
        let method = script.is_punct(close + 1, '{') && method_name_before(script, index);
        if !(arrow || function || method) {
            continue;
        }
        if immediate_level_contains(script, index, close, name) {
            return true;
        }
    }
    false
}

fn has_line_break(text: &str, from: usize, to: usize) -> bool {
    to > from
        && text
            .get(from..to)
            .is_some_and(|between| between.contains('\n'))
}

fn import_continuation(script: &Script<'_>, cursor: usize) -> bool {
    if cursor == 0 {
        return false;
    }
    let previous = script.token_text(cursor - 1);
    matches!(
        previous,
        "import" | "from" | "type" | "as" | "*" | "," | "{" | "(" | "["
    )
}

fn function_keyword_before(script: &Script<'_>, open: usize) -> bool {
    let mut cursor = open;
    while cursor > 0 {
        cursor -= 1;
        if script.is_ident(cursor, "function") {
            return true;
        }
        if script.is_ident(cursor, "if")
            || script.is_ident(cursor, "while")
            || script.is_ident(cursor, "switch")
            || script.is_ident(cursor, "for")
            || script.is_ident(cursor, "with")
            || script.is_ident(cursor, "catch")
        {
            return false;
        }
        if open - cursor > 3 {
            break;
        }
    }
    false
}

fn method_name_before(script: &Script<'_>, open: usize) -> bool {
    let Some(previous) = open.checked_sub(1) else {
        return false;
    };
    if !matches!(
        script.kind(previous),
        TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number
    ) {
        return false;
    }
    !matches!(
        script.token_text(previous),
        "if" | "while" | "switch" | "for" | "with" | "catch"
    )
}

fn immediate_level_contains(script: &Script<'_>, open: usize, close: usize, name: &str) -> bool {
    let mut cursor = open + 1;
    while cursor < close {
        if script.is_ident(cursor, name) {
            return true;
        }
        match script.kind(cursor) {
            TokenKind::Punctuator('(' | '[' | '{') | TokenKind::TemplateExpressionStart => {
                if let Some(nested_close) = script.pairs[cursor] {
                    cursor = nested_close + 1;
                    continue;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    false
}

fn scan_members(script: &Script<'_>, start: usize, end: usize) -> Vec<Member> {
    let mut members = Vec::new();
    let mut index = start;
    while index < end {
        while index < end && script.is_punct(index, ',') {
            index += 1;
        }
        if index >= end {
            break;
        }
        let segment_end = member_end(script, index, end);
        if segment_end <= index {
            break;
        }
        members.push(parse_member(script, index, segment_end));
        index = segment_end;
    }
    members
}

fn member_end(script: &Script<'_>, start: usize, end: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < end {
        match script.kind(index) {
            TokenKind::TemplateExpressionStart | TokenKind::Punctuator('(' | '[' | '{') => {
                depth += 1;
            }
            TokenKind::TemplateExpressionEnd | TokenKind::Punctuator(')' | ']' | '}') => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            TokenKind::Punctuator(',') if depth == 0 => break,
            _ => {}
        }
        index += 1;
    }
    index
}

#[allow(clippy::too_many_lines)] // one member grammar; mirrors the TypeScript scanner
fn parse_member(script: &Script<'_>, start: usize, end: usize) -> Member {
    if start >= end {
        return Member {
            name: None,
            kind: MemberKind::Unknown,
            key_start: 0,
            key_end: 0,
            value_start: start,
            value_end: end,
        };
    }
    let unknown = Member {
        name: None,
        kind: MemberKind::Unknown,
        key_start: script.tokens[start].start,
        key_end: script.tokens[start].end,
        value_start: start,
        value_end: end,
    };
    if script.is_punct(start, '.')
        && script.is_punct(start + 1, '.')
        && script.is_punct(start + 2, '.')
    {
        return Member {
            kind: MemberKind::Spread,
            value_start: start + 3,
            ..unknown
        };
    }
    if script.is_punct(start, '[') {
        return Member {
            kind: MemberKind::Computed,
            ..unknown
        };
    }
    if script.is_punct(start, '*') {
        return Member {
            kind: MemberKind::Method,
            ..unknown
        };
    }
    let token = script.tokens[start];
    if !matches!(
        token.kind,
        TokenKind::Identifier | TokenKind::StringLiteral | TokenKind::Number
    ) {
        return unknown;
    }
    let name = match token.kind {
        TokenKind::StringLiteral => unquote(script.token_text(start)),
        TokenKind::Identifier => Some(script.token_text(start).to_owned()),
        _ => None,
    };
    let key_end = token.end;
    let second = start + 1;
    if second >= end {
        return Member {
            name,
            kind: MemberKind::Shorthand,
            key_start: token.start,
            key_end,
            value_start: start,
            value_end: end,
        };
    }
    match script.kind(second) {
        TokenKind::Punctuator(':') => Member {
            name,
            kind: MemberKind::Property,
            key_start: token.start,
            key_end,
            value_start: second + 1,
            value_end: end,
        },
        TokenKind::Punctuator('(') => Member {
            name,
            kind: MemberKind::Method,
            key_start: token.start,
            key_end,
            value_start: second,
            value_end: end,
        },
        TokenKind::Identifier
            if matches!(script.token_text(start), "get" | "set")
                && script.is_punct(second + 1, '(') =>
        {
            Member {
                name: Some(script.token_text(second).to_owned()),
                kind: MemberKind::Accessor,
                key_start: token.start,
                key_end,
                value_start: second,
                value_end: end,
            }
        }
        _ => Member {
            name,
            kind: MemberKind::Unknown,
            key_start: token.start,
            key_end,
            value_start: second,
            value_end: end,
        },
    }
}

fn object_bounds(script: &Script<'_>, member: &Member) -> Option<(usize, usize)> {
    if member.kind != MemberKind::Property || member.value_start >= member.value_end {
        return None;
    }
    if !script.is_punct(member.value_start, '{') {
        return None;
    }
    let close = script.close_of(member.value_start).ok()?;
    if close + 1 != member.value_end {
        return None;
    }
    Some((member.value_start, close))
}

fn member_last<'a>(members: &'a [Member], name: &str) -> Option<&'a Member> {
    members
        .iter()
        .rev()
        .find(|member| member.name.as_deref() == Some(name))
}

fn classify_data_root(script: &Script<'_>, registration: &Registration) -> DataRootKind {
    let Some(data) = member_last(&registration.members, "data") else {
        return DataRootKind::Absent;
    };
    if data.kind != MemberKind::Property {
        return DataRootKind::Unsupported;
    }
    if object_bounds(script, data).is_some() {
        return DataRootKind::Object;
    }
    if data.value_start + 1 == data.value_end
        && script.tokens[data.value_start].kind == TokenKind::Identifier
        && is_same_file_type(script, script.token_text(data.value_start))
    {
        return DataRootKind::TypeReference;
    }
    DataRootKind::Expression
}

fn is_same_file_type(script: &Script<'_>, name: &str) -> bool {
    for index in 0..script.tokens.len() {
        if (script.is_ident(index, "interface") || script.is_ident(index, "type"))
            && script.is_ident(index + 1, name)
        {
            return true;
        }
    }
    false
}

/// Merges behavior-contributed properties into the own `properties` scan.
///
/// The component's own declaration wins over every behavior. Behavior
/// properties from another file keep `source = None` because a generated
/// declaration has no owning-file definition target.
fn merge_behavior_properties(
    properties: &mut PropertyScan,
    script_path: &str,
    overlay: &BehaviorPropertyOverlay<'_>,
) {
    for property in overlay.properties {
        if properties
            .properties
            .iter()
            .any(|existing| existing.name == property.name)
        {
            continue;
        }
        properties.properties.push(DeclaredProperty {
            name: property.name.clone(),
            type_text: property.type_text.clone(),
            has_optional_types: property.has_optional_types,
            doc: property.documentation.clone(),
            source: (property.path == script_path).then_some(property.range.utf8),
        });
    }
}

fn scan_properties(script: &Script<'_>, registration: &Registration) -> PropertyScan {
    let Some(member) = member_last(&registration.members, "properties") else {
        return PropertyScan::empty();
    };
    if member.kind != MemberKind::Property {
        return PropertyScan {
            properties: Vec::new(),
            complete: false,
            problems: vec!["component `properties` is not a property assignment".to_owned()],
        };
    }
    let Some((open, close)) = object_bounds(script, member) else {
        return PropertyScan {
            properties: Vec::new(),
            complete: false,
            problems: vec!["component `properties` is not an object literal".to_owned()],
        };
    };
    let entries = scan_members(script, open + 1, close);
    let mut properties = Vec::new();
    let mut problems = Vec::new();
    for entry in &entries {
        if entry.kind == MemberKind::Spread {
            problems.push(
                "`properties` uses a spread assignment; declared properties are incomplete"
                    .to_owned(),
            );
            continue;
        }
        if entry.kind != MemberKind::Property {
            problems.push("`properties` uses a shorthand or method member; declared properties are incomplete".to_owned());
            continue;
        }
        let Some(name) = &entry.name else {
            problems.push("`properties` has a computed member name".to_owned());
            continue;
        };
        let Some((option_open, option_close)) = object_bounds(script, entry) else {
            problems.push(format!(
                "property `{name}` is not declared with an object literal"
            ));
            continue;
        };
        let options = scan_members(script, option_open + 1, option_close);
        let type_text = union_text(script, &options);
        let Some(type_text) = type_text else {
            problems.push(format!(
                "property `{name}` has no readable `type` declaration"
            ));
            continue;
        };
        let has_optional_types = member_last(&options, "optionalTypes").is_some();
        properties.push(DeclaredProperty {
            name: name.clone(),
            type_text,
            has_optional_types,
            doc: doc_before(script.text, entry.key_start),
            source: Some(entry.key_range()),
        });
    }
    PropertyScan {
        complete: problems.is_empty(),
        properties,
        problems,
    }
}

fn union_text(script: &Script<'_>, options: &[Member]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(main) = constructor_text(script, options, "type") {
        parts.push(main);
    }
    if let Some(optional) = member_last(options, "optionalTypes") {
        if optional.kind == MemberKind::Property {
            if let Some(elements) = array_elements(script, optional) {
                for element in elements {
                    if let Some(text) = constructor_token(script, element) {
                        if !parts.contains(&text) {
                            parts.push(text);
                        }
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" | "))
    }
}

fn constructor_text(script: &Script<'_>, options: &[Member], name: &str) -> Option<String> {
    let member = member_last(options, name)?;
    if member.kind != MemberKind::Property || member.value_start + 1 != member.value_end {
        return None;
    }
    constructor_token(script, member.value_start)
}

fn constructor_token(script: &Script<'_>, index: usize) -> Option<String> {
    let token = script.tokens.get(index)?;
    match token.kind {
        TokenKind::Identifier => match script.token_text(index) {
            "String" => Some("string".to_owned()),
            "Number" => Some("number".to_owned()),
            "Boolean" => Some("boolean".to_owned()),
            "Array" => Some("any[]".to_owned()),
            "Object" => Some("Record<string, any>".to_owned()),
            "Function" => Some("(...args: any[]) => any".to_owned()),
            "null" => Some("any".to_owned()),
            _ => None,
        },
        _ => None,
    }
}

fn array_elements(script: &Script<'_>, member: &Member) -> Option<Vec<usize>> {
    if member.value_start >= member.value_end || !script.is_punct(member.value_start, '[') {
        return None;
    }
    let close = script.close_of(member.value_start).ok()?;
    if close + 1 != member.value_end {
        return None;
    }
    let mut elements = Vec::new();
    let mut index = member.value_start + 1;
    while index < close {
        let end = member_end(script, index, close);
        if end == index {
            break;
        }
        if end == index + 1 {
            elements.push(index);
        } else {
            elements.push(usize::MAX);
        }
        index = end;
        if index < close && script.is_punct(index, ',') {
            index += 1;
        }
    }
    Some(
        elements
            .into_iter()
            .filter(|index| *index != usize::MAX)
            .collect(),
    )
}

fn doc_before(text: &str, key_start: usize) -> String {
    let prefix = &text[..key_start];
    let trimmed = prefix.trim_end();
    if !trimmed.ends_with("*/") {
        return String::new();
    }
    let Some(start) = trimmed[..trimmed.len() - 2].rfind("/**") else {
        return String::new();
    };
    let comment = &trimmed[start + 3..trimmed.len() - 2];
    comment
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn unquote(text: &str) -> Option<String> {
    let mut characters = text.chars();
    let quote = characters.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for character in characters {
        if escaped {
            match character {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            break;
        } else {
            value.push(character);
        }
    }
    Some(value)
}

fn behaviors_incomplete(script: &Script<'_>, registration: &Registration) -> bool {
    let Some(member) = member_last(&registration.members, "behaviors") else {
        return false;
    };
    if member.kind != MemberKind::Property
        || member.value_start >= member.value_end
        || !script.is_punct(member.value_start, '[')
    {
        return true;
    }
    let Ok(close) = script.close_of(member.value_start) else {
        return true;
    };
    if close + 1 != member.value_end {
        return true;
    }
    close != member.value_start + 1
}

#[derive(Debug, Clone, Copy)]
struct PragmaScan {
    bom_length_utf16: u32,
    injection_offset: usize,
    neutralize_offset: usize,
    check: bool,
    no_check: bool,
}

fn scan_pragmas(text: &str, want_pragma: bool) -> PragmaScan {
    let bom_length_utf16 = u32::from(text.starts_with('\u{FEFF}'));
    let mut shebang_end = if bom_length_utf16 > 0 { 3 } else { 0 };
    if text[shebang_end..].starts_with("#!") {
        shebang_end = text[shebang_end..]
            .find('\n')
            .map_or(text.len(), |offset| shebang_end + offset + 1);
    }
    let mut last_pragma: Option<bool> = None; // true = check, false = nocheck
    let mut cursor = shebang_end;
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
            record_pragmas(&text[cursor..end], &mut last_pragma);
            cursor = end;
            continue;
        }
        if text[cursor..].starts_with("/*") {
            let end = text[cursor + 2..]
                .find("*/")
                .map_or(text.len(), |offset| cursor + 2 + offset + 2);
            record_pragmas(&text[cursor..end], &mut last_pragma);
            cursor = end;
            continue;
        }
        break;
    }
    let _ = want_pragma;
    PragmaScan {
        bom_length_utf16,
        injection_offset: shebang_end,
        neutralize_offset: cursor,
        check: last_pragma == Some(true),
        no_check: last_pragma == Some(false),
    }
}

fn record_pragmas(comment: &str, last: &mut Option<bool>) {
    for (index, _) in comment.match_indices("@ts-") {
        let tail = &comment[index + 4..];
        if tail.starts_with("check") {
            let after = tail.get(5..).unwrap_or("");
            if after.is_empty()
                || !after
                    .starts_with(|character: char| character.is_alphanumeric() || character == '_')
            {
                *last = Some(true);
            }
        } else if tail.starts_with("nocheck") {
            let after = tail.get(7..).unwrap_or("");
            if after.is_empty()
                || !after
                    .starts_with(|character: char| character.is_alphanumeric() || character == '_')
            {
                *last = Some(false);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // mirrors the independent policy flags of the oracle
struct JsCheckInput {
    check_js: bool,
    is_javascript: bool,
    pragma_check: bool,
    pragma_no_check: bool,
    want_pragma: bool,
}

fn effective_js_check(
    policy: JsCheckPolicy,
    input: JsCheckInput,
    notes: &mut Vec<String>,
) -> JsCheckApplied {
    if policy == JsCheckPolicy::Off {
        return JsCheckApplied::Off;
    }
    if input.pragma_no_check {
        notes.push("`@ts-nocheck` in the source disables template diagnostics".to_owned());
        return JsCheckApplied::Off;
    }
    if !input.is_javascript {
        return JsCheckApplied::Inherit;
    }
    if input.check_js || input.pragma_check {
        return JsCheckApplied::Inherit;
    }
    if input.want_pragma {
        return JsCheckApplied::Pragma;
    }
    JsCheckApplied::Off
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::DocumentRevision;

    fn source_file(path: &str, text: &str) -> SourceFile {
        SourceFile::new(path, text, DocumentRevision::new(1)).expect("valid source")
    }

    fn build(text: &str, root: RootKind) -> Result<BuiltShadow, ShadowError> {
        let script = source_file("index.ts", text);
        let request = ShadowRequest::new(&script, root);
        build_in_place_shadow(&request)
    }

    #[test]
    fn page_probe_is_inserted_inside_the_options_object() {
        let built = build("Page({\n  data: { n: 1 },\n});\n", RootKind::Page).expect("layout");
        assert_eq!(built.status, BuildStatus::Supported);
        assert!(
            built.view.text.starts_with(
                "Page({\n    __wxml_check_0() {\n        const __wxml_root = this.data;"
            )
        );
        assert!(
            built
                .view
                .text
                .ends_with("      },\n  \n  data: { n: 1 },\n});\n")
        );
        assert_eq!(built.view.insertions.len(), 1);
        assert_eq!(
            built.view.probe_anchors.root_offset,
            built.view.probe_anchors.root_offset
        );
    }

    #[test]
    fn component_probe_mounts_inside_methods_or_wraps_one() {
        let with_methods = build(
            "Component({\n  methods: {\n    tap() {},\n  },\n});\n",
            RootKind::Component,
        )
        .expect("layout");
        assert!(
            with_methods
                .view
                .text
                .starts_with("Component({\n  methods: {\n    __wxml_check_0() {")
        );
        let without =
            build("Component({ data: { n: 1 } });\n", RootKind::Component).expect("layout");
        assert!(
            without
                .view
                .text
                .starts_with("Component({\n    methods: {\n  ")
        );
    }

    #[test]
    fn behavior_uses_component_probe_placement() {
        let built = build("Behavior({ methods: {} });", RootKind::Behavior).expect("layout");
        assert!(
            built
                .view
                .text
                .starts_with("Behavior({ methods: {\n    __wxml_check_0() {")
        );
    }

    #[test]
    fn no_registration_is_unsupported() {
        let error = build("const x = 1;", RootKind::Page).expect_err("unsupported");
        assert!(matches!(
            error,
            ShadowError::Unsupported {
                feature: "registration",
                ..
            }
        ));
    }

    #[test]
    fn non_literal_methods_and_data_are_unsupported() {
        let methods =
            build("Component({ methods: shared });", RootKind::Component).expect_err("unsupported");
        assert!(matches!(
            methods,
            ShadowError::Unsupported {
                feature: "component.methods",
                ..
            }
        ));
        let data = build("Page({ data: sharedData });", RootKind::Page)
            .expect("expression data is partial");
        assert_eq!(data.status, BuildStatus::Partial);
        assert_eq!(data.data_root, DataRootKind::Expression);
        let type_only = build(
            "interface IData { n: number }\nPage({ data: IData });",
            RootKind::Page,
        )
        .expect_err("type-only data root");
        assert!(matches!(
            type_only,
            ShadowError::Unsupported {
                feature: "data.root",
                ..
            }
        ));
    }

    #[test]
    fn properties_overlay_is_rendered_and_optional_types_union() {
        let built = build(
            "Component({\n  properties: {\n    /** 标题 */\n    \"a-b\": { type: String, optionalTypes: [Number, String], value: '' },\n  },\n  data: {},\n});\n",
            RootKind::Component,
        )
        .expect("layout");
        assert_eq!(built.properties.properties.len(), 1);
        let property = &built.properties.properties[0];
        assert_eq!(property.name, "a-b");
        assert_eq!(property.type_text, "string | number");
        assert!(built
            .view
            .text
            .contains("type __WxmlProps_0 = {\n          /** 标题 */\n          \"a-b\": string | number;\n        };"));
        assert!(
            built
                .view
                .text
                .contains("const __wxml_props_0 = null as unknown as __WxmlProps_0;")
        );
    }

    #[test]
    fn non_literal_properties_are_incomplete_not_silently_narrowed() {
        let built = build(
            "Component({ properties: sharedProperties, data: {} });",
            RootKind::Component,
        )
        .expect("layout");
        assert_eq!(built.status, BuildStatus::Partial);
        assert!(!built.properties.complete);
        assert!(
            built
                .properties
                .problems
                .iter()
                .any(|problem| problem.contains("not an object literal"))
        );
    }

    #[test]
    fn js_pragma_and_bom_edits_are_recorded() {
        let script = source_file(
            "index.js",
            "\u{FEFF}#!/usr/bin/env node\nComponent({ methods: {} });",
        );
        let request = ShadowRequest {
            script: &script,
            root: RootKind::Component,
            fragments: &[],
            js_check: JsCheckPolicy::Pragma,
            check_js: false,
        };
        let built = build_in_place_shadow(&request).expect("layout");
        assert_eq!(built.js_check_applied, JsCheckApplied::Pragma);
        assert!(built.view.text.starts_with("#!"));
        assert!(built.view.text.contains("// @ts-check\n"));
        let bom = built
            .view
            .insertions
            .iter()
            .find(|insertion| insertion.insertion_id == "bom")
            .expect("BOM edit");
        assert_eq!(bom.replace_length_utf16, 1);
        let pragma = built
            .view
            .insertions
            .iter()
            .find(|insertion| insertion.insertion_id == "pragma")
            .expect("pragma edit");
        assert_eq!(
            &built.view.text[usize::try_from(pragma.start_utf16).expect("offset")..]
                .chars()
                .take("// @ts-check\n".chars().count())
                .collect::<String>(),
            "// @ts-check\n"
        );
    }

    #[test]
    fn fragments_mount_and_map_back_to_wxml_bytes() {
        let text = "Page({ data: { user: { name: 'Ada' } } });";
        let script = source_file("index.ts", text);
        let expr = "user.name";
        let fragment = FragmentSpec::new(
            "f0",
            expr,
            SourceRange::new(134, 134 + u32::try_from(expr.len()).expect("length")),
        );
        let request = ShadowRequest {
            script: &script,
            root: RootKind::Page,
            fragments: &[fragment],
            js_check: JsCheckPolicy::Inherit,
            check_js: false,
        };
        let built = build_in_place_shadow(&request).expect("layout");
        assert!(built.view.text.contains(
            "const __wxml_root = this.data;\n        {\n          __wxml_root.user.name;"
        ));
        assert_eq!(built.view.mounts.len(), 1);
        let mount = &built.view.mounts[0];
        assert_eq!(mount.text, "__wxml_root.user.name");
        let mapped = crate::shadow::mapping::map_span_to_source(
            mount,
            crate::native::strings::Utf16Offset::new(mount.target_start_utf16),
            mount.target_length_utf16,
        )
        .expect("mapped");
        assert_eq!(mapped.range, Some(SourceRange::new(134 + 5, 134 + 9)));
    }

    #[test]
    fn invalid_fragment_ranges_are_rejected() {
        let script = source_file("index.ts", "Page({ data: {} });");
        let fragment = FragmentSpec::new("f0", "user.name", SourceRange::new(0, 1));
        let request = ShadowRequest {
            script: &script,
            root: RootKind::Page,
            fragments: &[fragment],
            js_check: JsCheckPolicy::Inherit,
            check_js: false,
        };
        assert!(matches!(
            build_in_place_shadow(&request),
            Err(ShadowError::Invalid {
                feature: "fragment.range",
                ..
            })
        ));
    }
}
