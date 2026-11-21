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
//! [`ShadowError::Invalid`]. A component whose `methods` member cannot host the
//! probe is the deliberate exception: it returns a `Partial` build with the
//! original script and no mounts, because the TypeScript oracle treats that as
//! a coverage gap rather than an unsupported script.
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
//!
//! # Fragment modes
//!
//! An ordinary fragment is a WXML expression: free roots are rewritten to
//! `__wxml_root`, declared properties or probe locals, and every rewritten
//! segment keeps the byte range it was copied from.
//!
//! A *member* fragment ([`FragmentSpec::member`]) is a bare event-handler or
//! method name. It is mounted as `this.<name>` inside the probe, so `this`
//! resolves to the page/component instance type. Only the name is real source
//! text and maps back to the WXML token; the leading `this.` is synthetic and
//! therefore unattributable when a span lies entirely inside it. Member
//! fragments are otherwise ordinary fragments: placement, guards and the mount
//! record are built by the same code path.

#![allow(clippy::format_push_string)] // generated probe text is tiny; format! stays readable

use std::collections::BTreeSet;

use crate::input::SourceFile;
use crate::script::behavior::BehaviorProperty;
use crate::script::members::{
    Member, MemberKind, Members, doc_before, function_keyword_before, member_end, member_last,
    method_name_before, object_bounds, scan_members,
};
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
    AppliedEdit, EditKind, HoverTarget, MountedSegment, RewriteOptions, RewrittenExpression,
    ShadowEdit, apply_edits, rewrite_expression,
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
/// Reported when a component `methods` member cannot host the injected probe.
const METHODS_NOT_LITERAL_REASON: &str =
    "component `methods` is not an object literal; the in-place probe cannot be mounted";
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
    /// Mount `expr` as a member of the registration instance instead of
    /// rewriting it as a WXML expression.
    ///
    /// In member mode `expr` is a bare event-handler or method name whose UTF-8
    /// byte length equals `wxml_range`'s length, exactly like an ordinary
    /// fragment. The mounted text is `this.<expr>`: the `this.` prefix is
    /// synthetic and the name segment maps back to `wxml_range`. Free-root
    /// rewriting is skipped because the name is read as a member of `this`, not
    /// as template data.
    pub member: bool,
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
            member: false,
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
    /// Static WXS module names declared by the owning template.
    ///
    /// A `<wxs module="m1">` name is a file-level value, not a data field, so
    /// it is kept unprefixed and declared once as `any` in the probe. Entries
    /// must be valid binding identifiers; the renderer re-validates defensively
    /// before writing any probe text.
    pub wxs_modules: &'a [String],
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
            wxs_modules: &[],
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

impl Members for Script<'_> {
    fn kind_at(&self, index: usize) -> Option<TokenKind> {
        self.tokens.get(index).map(|token| token.kind)
    }

    fn span_at(&self, index: usize) -> Option<(usize, usize)> {
        self.tokens.get(index).map(|token| (token.start, token.end))
    }

    fn text_at(&self, index: usize) -> Option<&str> {
        self.tokens.get(index).map(|token| token.text(self.text))
    }

    fn close_at(&self, index: usize) -> Option<usize> {
        self.pairs.get(index).copied().flatten()
    }

    fn punct_at(&self, index: usize, character: char) -> bool {
        self.is_punct(index, character)
    }

    fn ident_at(&self, index: usize, name: &str) -> bool {
        self.is_ident(index, name)
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

    // Static WXS module names are values, not data fields. Declare each one
    // once so a template call such as `m1.getLimitSettingText(...)` is checked
    // against `any` instead of `__wxml_root.m1`. This is generated text without
    // a source mapping and it deliberately does not mark the fragment as
    // unresolved: a valid static module is complete coverage, not a gap.
    for name in probe_wxs_modules(request.wxs_modules) {
        match syntax {
            Syntax::TypeScript => {
                body.push(&format!("{BODY_INDENT}const {name}: any = null as any;\n"));
            }
            Syntax::JavaScript => {
                body.push(&format!(
                    "{BODY_INDENT}const {name} = /** @type {{any}} */ (null);\n"
                ));
            }
        }
    }

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
        let planned = plan_fragment(fragment, request.wxs_modules, &property_names, syntax)?;
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
        let local_names = effective_local_names(fragment, request.wxs_modules);
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
                    request.wxs_modules,
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
        // A bare expression starting with `{` is parsed by TypeScript as a
        // block, so wrap object literals in parentheses. The parentheses are
        // generated text: `expression_start` still points at the expression's
        // first character and every segment keeps its expression offset. The
        // closing `)` is always outside the mount (see `mount_text` below for
        // the folded case).
        let needs_parens = planned.rewritten.text.trim_start().starts_with('{');
        if needs_parens {
            body.push("(");
        }
        let expression_start = body.length_utf16;
        body.push(&planned.rewritten.text);
        if needs_parens {
            body.push(")");
        }
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
            // A folded mount starts at the guard block, so the wrapping `(`
            // falls inside the mount's derived span. It must be part of
            // `mount_text` to keep the mount text aligned with the derived
            // body; the closing `)` stays outside (the mount ends at the
            // expression). Without a fold the parentheses are both outside
            // the mount and `text` remains the bare expression.
            let open_paren = if needs_parens { "(" } else { "" };
            format!(
                "{guard_text}{}{open_paren}{}",
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

    let placement = match probe_placement(&script, &registration) {
        Ok(placement) => placement,
        Err(ShadowError::Unsupported {
            feature: "component.methods",
            reason,
        }) => {
            // A component `methods` member that is not an object literal cannot
            // host the probe, but that is a coverage gap, not an unsupported
            // script: the TypeScript oracle reports `Partial`, analyses the
            // script as-is and mounts no fragment. Returning `Unsupported`
            // here would turn the same gap into a hard backend failure.
            //
            // The oracle publishes the probe-placement problem as the single
            // reason; other coverage gaps stay visible as notes instead of
            // changing the public Partial reason.
            if !reasons.is_empty() {
                notes.push(format!("additional coverage gaps: {}", reasons.join("; ")));
            }
            notes.push(
                "the in-place probe is not mounted; the script is analysed without template fragments"
                    .to_owned(),
            );
            let js_check_applied = match js_check_applied {
                JsCheckApplied::Off => JsCheckApplied::Off,
                JsCheckApplied::Pragma | JsCheckApplied::Inherit => JsCheckApplied::Inherit,
            };
            let assembled = apply_edits(request.script.text(), &[])?;
            let map = ShadowMap::new(request.script.text(), &assembled);
            let view = DerivedView {
                original: request.script.text().to_owned(),
                text: assembled.text,
                edits: assembled.edits,
                insertions: Vec::new(),
                mounts: Vec::new(),
                generated_declarations: Vec::new(),
                probe_anchors: ProbeAnchors::default(),
                map,
            };
            return Ok(BuiltShadow {
                script_path: request.script.path().to_owned(),
                status: BuildStatus::Partial,
                reason: Some(reason),
                notes,
                js_check_applied,
                registration: RegistrationInfo {
                    kind: registration.kind,
                    count: registration_count,
                },
                data_root,
                properties,
                view,
            });
        }
        Err(error) => return Err(error),
    };
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
///
/// WXS module names are template-level values carried by the request, so they
/// stay unprefixed exactly like `wx:for` locals. Invalid names and the reserved
/// `__wxml_` namespaces are dropped, which keeps a caller-built
/// `ShadowRequest`/`FragmentSpec` from emitting invalid probe text.
fn effective_local_names(fragment: &FragmentSpec, wxs_modules: &[String]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = wxs_modules
        .iter()
        .filter(|name| expression::is_binding_identifier(name))
        .cloned()
        .collect();
    if fragment.bindings.is_empty() {
        names.extend(
            fragment
                .locals
                .iter()
                .filter(|name| expression::is_binding_identifier(name))
                .cloned(),
        );
    } else {
        names.extend(
            fragment
                .bindings
                .iter()
                .filter(|binding| expression::is_binding_identifier(&binding.name))
                .map(|binding| binding.name.clone()),
        );
    }
    names
}

/// Deduplicated WXS module names declared once at the top of the probe.
///
/// The request-level list is deduplicated in first-occurrence order. Invalid
/// names (including the reserved `__wxml_` namespaces) are skipped so
/// hand-built requests cannot inject arbitrary probe text. Nothing here touches
/// the unresolved-binding counter.
fn probe_wxs_modules(wxs_modules: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in wxs_modules {
        if !expression::is_binding_identifier(name) {
            continue;
        }
        if names.iter().any(|existing| existing == name) {
            continue;
        }
        names.push(name.clone());
    }
    names
}

fn plan_fragment(
    fragment: &FragmentSpec,
    wxs_modules: &[String],
    property_names: &BTreeSet<String>,
    syntax: Syntax,
) -> Result<PlannedFragment, ShadowError> {
    if fragment.member {
        return Ok(PlannedFragment {
            rewritten: member_expression(fragment),
        });
    }
    let options = RewriteOptions {
        root_prefix: ROOT_PREFIX.to_owned(),
        props_prefix: PROPS_PREFIX.to_owned(),
        property_overlay: property_names.clone(),
        data_overlay: BTreeSet::new(),
        locals: effective_local_names(fragment, wxs_modules),
    };
    let rewritten = rewrite_expression(&fragment.expr, fragment.target.as_deref(), &options)?;
    let _ = syntax;
    Ok(PlannedFragment { rewritten })
}

/// Prefix that turns a handler name into a member of the registration instance.
const MEMBER_PREFIX: &str = "this.";

/// Plans a member fragment as `this.<expr>` with a synthetic prefix.
///
/// This bypasses [`rewrite_expression`] on purpose: `expr` is a method name, so
/// prefixing it as a free data root would mount `this.__wxml_root.<name>`. The
/// name segment keeps `wxml_range`'s exact bytes; the `this.` prefix has no
/// source. `validate_fragment_range` guarantees the byte lengths agree.
fn member_expression(fragment: &FragmentSpec) -> RewrittenExpression {
    let prefix_len = utf16_len(MEMBER_PREFIX);
    let name_len = utf16_len(&fragment.expr);
    let name_source = SourceRange::new(0, fragment.wxml_range.len());
    RewrittenExpression {
        text: format!("{MEMBER_PREFIX}{}", fragment.expr),
        segments: vec![
            MountedSegment {
                start_utf16: 0,
                length_utf16: prefix_len,
                source: None,
            },
            MountedSegment {
                start_utf16: prefix_len,
                length_utf16: name_len,
                source: Some(name_source),
            },
        ],
        target_offset_utf16: prefix_len,
        hover_targets: vec![HoverTarget {
            offset_utf16: prefix_len,
            source: name_source,
        }],
        roots: Vec::new(),
        local_root: false,
    }
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
    wxs_modules: &[String],
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
            &effective_local_names(candidate, wxs_modules),
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
                reason: METHODS_NOT_LITERAL_REASON.to_owned(),
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
            reason: METHODS_NOT_LITERAL_REASON.to_owned(),
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

    fn build_js_with_policy(text: &str, policy: JsCheckPolicy) -> BuiltShadow {
        let script = source_file("index.js", text);
        let mut request = ShadowRequest::new(&script, RootKind::Component);
        request.js_check = policy;
        build_in_place_shadow(&request).expect("layout")
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
    fn non_literal_methods_are_partial_and_expression_data_is_partial() {
        let methods = build("Component({ methods: shared });", RootKind::Component)
            .expect("non-literal methods build partially");
        assert_eq!(methods.status, BuildStatus::Partial);
        assert_eq!(
            methods.reason.as_deref(),
            Some(
                "component `methods` is not an object literal; the in-place probe cannot be mounted"
            )
        );
        assert_eq!(methods.view.mounts.len(), 0);
        assert_eq!(methods.view.insertions.len(), 0);
        assert_eq!(methods.view.text, "Component({ methods: shared });");
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
        let mut request = ShadowRequest::new(&script, RootKind::Component);
        request.js_check = JsCheckPolicy::Pragma;
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
    #[allow(clippy::too_many_lines)] // one fixture per oracle branch keeps the offsets auditable
    fn case_probes_0027_pragma_scanning_handles_plain_files_bom_and_hashbang() {
        let plain_text = "// @ts-check\nconst a = 1;\nComponent({ data: {} });\n";
        let plain = scan_pragmas(plain_text, true);
        assert!(plain.check, "the leading @ts-check must be recognised");
        assert!(!plain.no_check);
        assert_eq!(plain.injection_offset, 0);
        assert_eq!(
            utf16_len(&plain_text[..plain.neutralize_offset]),
            u32::try_from("// @ts-check\n".len()).expect("length"),
            "the leading pragma block ends after the check line"
        );
        assert_eq!(plain.bom_length_utf16, 0);
        // `dropBom` is the layout decision built from the scan: a plain file
        // never drops a BOM.
        let plain_built = build_js_with_policy(plain_text, JsCheckPolicy::Inherit);
        assert_eq!(
            plain_built.js_check_applied,
            JsCheckApplied::Inherit,
            "the source pragma enables checking without the shadow-only injection"
        );
        assert!(
            plain_built
                .view
                .insertions
                .iter()
                .all(|insertion| insertion.insertion_id != "bom"),
            "a plain file must not record a BOM edit"
        );

        let bom_text = "\u{FEFF}const a = 1;\nComponent({ data: {} });\n";
        let bom = scan_pragmas(bom_text, true);
        assert_eq!(bom.bom_length_utf16, 1);
        assert_eq!(
            utf16_len(&bom_text[..bom.injection_offset]),
            1,
            "the pragma goes after the dropped BOM"
        );
        let bom_built = build_js_with_policy(bom_text, JsCheckPolicy::Pragma);
        let bom_edit = bom_built
            .view
            .insertions
            .iter()
            .find(|insertion| insertion.insertion_id == "bom")
            .expect("BOM edit");
        assert_eq!(bom_edit.replace_length_utf16, 1);
        let bom_pragma = bom_built
            .view
            .edits
            .iter()
            .find(|edit| edit.id == "pragma")
            .expect("pragma edit");
        assert_eq!(bom_pragma.at_utf16, 1);

        let shebang_text = "#!/usr/bin/env node\nconst a = 1;\nComponent({ data: {} });\n";
        let shebang = scan_pragmas(shebang_text, true);
        assert_eq!(
            utf16_len(&shebang_text[..shebang.injection_offset]),
            20,
            "the pragma goes after the hashbang line"
        );
        assert_eq!(shebang.bom_length_utf16, 0);
        let shebang_built = build_js_with_policy(shebang_text, JsCheckPolicy::Pragma);
        assert!(
            shebang_built
                .view
                .insertions
                .iter()
                .all(|insertion| insertion.insertion_id != "bom"),
            "a hashbang file without a BOM must not record a BOM edit"
        );
        let shebang_pragma = shebang_built
            .view
            .edits
            .iter()
            .find(|edit| edit.id == "pragma")
            .expect("pragma edit");
        assert_eq!(shebang_pragma.at_utf16, 20);

        let both_text = "\u{FEFF}#!/usr/bin/env node\nconst a = 1;\nComponent({ data: {} });\n";
        let both = scan_pragmas(both_text, true);
        assert_eq!(both.bom_length_utf16, 1);
        assert_eq!(utf16_len(&both_text[..both.injection_offset]), 21);
        let both_built = build_js_with_policy(both_text, JsCheckPolicy::Pragma);
        assert!(
            both_built
                .view
                .insertions
                .iter()
                .any(|insertion| insertion.insertion_id == "bom"),
            "a BOM before the hashbang is dropped"
        );
        let both_pragma = both_built
            .view
            .edits
            .iter()
            .find(|edit| edit.id == "pragma")
            .expect("pragma edit");
        assert_eq!(both_pragma.at_utf16, 21);
        assert!(both_built.view.text.starts_with("#!"));

        // `neutralizeOffset` is where the policy-off `@ts-nocheck` goes.
        let neutralized = build_js_with_policy(plain_text, JsCheckPolicy::Off);
        let nocheck = neutralized
            .view
            .edits
            .iter()
            .find(|edit| edit.id == "nocheck")
            .expect("nocheck edit");
        assert_eq!(nocheck.at_utf16, 13);
    }

    #[test]
    fn case_probes_0048_the_last_pragma_in_the_leading_block_wins() {
        // Oracle: `probes.test.ts:48`. TypeScript lets the last leading pragma
        // decide, so `@ts-check` followed by `@ts-nocheck` disables checking and
        // the neutralising insertion goes after the whole comment block.
        let check_then_no_check_text = "// @ts-check\n// @ts-nocheck\nconst a = 1;\n";
        let check_then_no_check = scan_pragmas(check_then_no_check_text, false);
        assert!(!check_then_no_check.check);
        assert!(check_then_no_check.no_check);
        assert_eq!(
            check_then_no_check.neutralize_offset,
            "// @ts-check\n// @ts-nocheck\n".len()
        );
        assert_eq!(
            utf16_len(&check_then_no_check_text[..check_then_no_check.neutralize_offset]),
            28,
            "two ASCII pragma lines are 28 UTF-16 code units"
        );

        let no_check_then_check_text = "// @ts-nocheck\n// @ts-check\nconst a = 1;\n";
        let no_check_then_check = scan_pragmas(no_check_then_check_text, false);
        assert!(no_check_then_check.check);
        assert!(!no_check_then_check.no_check);
        assert_eq!(
            no_check_then_check.neutralize_offset,
            "// @ts-nocheck\n// @ts-check\n".len()
        );
        assert_eq!(
            utf16_len(&no_check_then_check_text[..no_check_then_check.neutralize_offset]),
            28
        );
    }

    #[test]
    fn case_probes_0059_ts_nocheck_outside_the_leading_comments_is_ignored() {
        // Oracle: `probes.test.ts:59`. Only the leading comment block is scanned.
        let ordinary_text = "// ordinary comment\nPage({ data: { x: 1 } });\n";
        let ordinary = scan_pragmas(ordinary_text, false);
        assert!(!ordinary.check);
        assert!(!ordinary.no_check);
        assert_eq!(
            ordinary.neutralize_offset,
            "// ordinary comment\n".len(),
            "an ordinary leading comment is consumed but does not disable checking"
        );

        let after_code_text = "const a = 1;\n// @ts-nocheck\n";
        let after_code = scan_pragmas(after_code_text, false);
        assert!(!after_code.check);
        assert!(!after_code.no_check);
        assert_eq!(
            after_code.neutralize_offset, 0,
            "the scan stops at the first non-comment token, so the later pragma is not seen"
        );
    }

    #[test]
    fn case_analysis_0349_duplicate_fragment_ids_are_a_structured_error() {
        // Oracle: `analysis.test.ts:349` rejects duplicate fragment ids with the
        // structured `bad-fragment` code. Native reports the typed
        // `Invalid { feature: "fragment.id" }` before producing a view.
        let text = "Page({ data: { title: 'x' } });";
        let script = source_file("index.ts", text);
        let duplicate = [
            FragmentSpec::new("f0", "title", SourceRange::new(0, 5)),
            FragmentSpec::new("f0", "title", SourceRange::new(0, 5)),
        ];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &duplicate;
        let error = build_in_place_shadow(&request).expect_err("duplicate fragment id");
        match error {
            ShadowError::Invalid { feature, reason } => {
                assert_eq!(feature, "fragment.id");
                assert_eq!(reason, "fragment ids must be non-empty and unique");
            }
            other => panic!("expected the typed fragment.id error, got {other:?}"),
        }
    }

    #[test]
    fn case_protocol_0600_legacy_fragments_without_target_or_locals_still_build() {
        // Oracle: `protocol.test.ts:600`. A fragment with no `target` and no
        // `locals` is the legacy shape and must still validate and mount.
        let text = "Page({ data: { title: 'x', user: { name: 'Ada' } } });";
        let script = source_file("index.ts", text);
        let title = FragmentSpec::new("title", "title", SourceRange::new(0, 5));
        let name = FragmentSpec::new("user.name", "user.name", SourceRange::new(6, 15));
        assert!(title.target.is_none());
        assert_eq!(title.locals.len(), 0);
        assert_eq!(title.bindings.len(), 0);
        assert_eq!(title.guards.len(), 0);
        assert!(name.target.is_none());
        assert_eq!(name.locals.len(), 0);

        let fragments = [title, name];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &fragments;
        let built = build_in_place_shadow(&request).expect("legacy fragments build");
        assert_eq!(built.view.mounts.len(), 2);
        assert_eq!(built.view.mounts[0].text, "__wxml_root.title");
        assert_eq!(built.view.mounts[1].text, "__wxml_root.user.name");
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
        let fragments = [fragment];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &fragments;
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
    fn member_fragments_mount_the_handler_as_this_dot_name() {
        // A member fragment is a bare handler name mounted as `this.<name>`:
        // the `this.` prefix is synthetic, the name maps to the WXML token and
        // the target offset points at the name, not at the prefix.
        let text = "Page({ data: { title: 'x' }, onBackTap() {} });";
        let script = source_file("index.ts", text);
        let wxml = "<view bindtap=\"onBackTap\">back</view>";
        let handler = "onBackTap";
        let start = wxml.find(handler).expect("handler in wxml");
        let handler_range = SourceRange::new(
            u32::try_from(start).expect("offset"),
            u32::try_from(start + handler.len()).expect("offset"),
        );
        let mut fragment = FragmentSpec::new("event#0", handler, handler_range);
        fragment.member = true;
        let fragments = [fragment];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &fragments;
        let built = build_in_place_shadow(&request).expect("layout");
        assert_eq!(built.status, BuildStatus::Supported);
        assert!(
            built.view.text.contains("this.onBackTap;"),
            "the member is mounted against the instance: {}",
            built.view.text
        );
        assert_eq!(built.view.mounts.len(), 1);
        let mount = &built.view.mounts[0];
        assert_eq!(mount.text, "this.onBackTap");
        assert_eq!(
            mount.target_start_utf16 - mount.start_utf16,
            5,
            "the target offset points just past `this.`"
        );
        assert_eq!(mount.target_length_utf16, 9);
        assert_eq!(mount.segments.len(), 2);
        assert_eq!(mount.segments[0].source, None, "`this.` is synthetic");
        assert_eq!(mount.segments[0].length_utf16, 5);
        assert_eq!(
            mount.segments[1].source,
            Some(handler_range),
            "the name segment carries the handler bytes"
        );

        let name = crate::shadow::mapping::map_span_to_source(
            mount,
            crate::native::strings::Utf16Offset::new(mount.start_utf16 + 5),
            9,
        )
        .expect("mapped name");
        assert_eq!(name.range, Some(handler_range));
        assert!(
            !name.trimmed_synthetic_prefix,
            "the name starts after `this.`"
        );

        let prefix = crate::shadow::mapping::map_span_to_source(
            mount,
            crate::native::strings::Utf16Offset::new(mount.start_utf16),
            5,
        )
        .expect("mapped prefix");
        assert_eq!(prefix.range, None, "`this.` has no WXML bytes");
        assert!(prefix.unattributable);
    }

    #[test]
    fn object_literal_fragments_are_parenthesized_and_map_back() {
        // A multiline object literal is a statement-level `{` in the shadow
        // unless it is parenthesized. `mount.text` must stay the expression
        // alone so targets and segments keep their original offsets.
        let text = "Page({ data: { user: { name: 'Ada', id: 7 } } });";
        let script = source_file("index.ts", text);
        let wxml =
            "<view>\n  <text>{{ {\n    name: user.name,\n    id: user.id\n  } }}</text>\n</view>";
        let expr = "{\n    name: user.name,\n    id: user.id\n  }";
        let start = wxml.find(expr).expect("expression in wxml");
        let fragment = FragmentSpec::new(
            "f0",
            expr,
            SourceRange::new(
                u32::try_from(start).expect("offset"),
                u32::try_from(start + expr.len()).expect("offset"),
            ),
        );
        let fragments = [fragment];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &fragments;
        let built = build_in_place_shadow(&request).expect("layout");
        assert_eq!(built.status, BuildStatus::Supported);
        assert!(
            built.view.text.contains("({\n"),
            "the object literal is wrapped: {}",
            built.view.text
        );
        let close = built
            .view
            .text
            .find("});")
            .expect("closing paren precedes the statement semicolon");
        assert_eq!(
            &built.view.text[close..close + 3],
            "});",
            "the parenthesis is written before the generated `;\\n`"
        );

        assert_eq!(built.view.mounts.len(), 1);
        let mount = &built.view.mounts[0];
        assert!(mount.text.starts_with('{'), "{}", mount.text);
        assert!(mount.text.ends_with('}'), "{}", mount.text);
        assert!(!mount.text.starts_with('('), "{}", mount.text);
        assert!(!mount.text.ends_with(')'), "{}", mount.text);
        assert!(mount.text.contains("__wxml_root.user.name"));

        // The rightmost value identifier (`id`) still maps to its own WXML
        // bytes; the generated parentheses never enter the mount.
        let target = expr.rfind("id").expect("target identifier");
        let mapped = crate::shadow::mapping::map_span_to_source(
            mount,
            crate::native::strings::Utf16Offset::new(mount.target_start_utf16),
            mount.target_length_utf16,
        )
        .expect("mapped");
        assert_eq!(
            mapped.range,
            Some(SourceRange::new(
                u32::try_from(start + target).expect("offset"),
                u32::try_from(start + target + "id".len()).expect("offset"),
            ))
        );
    }

    #[test]
    fn guarded_object_literal_fragments_keep_guard_and_value_offsets() {
        use crate::shadow::guards::{GuardCondition, GuardKind};

        // Folded guards put `(` between the guard block and the expression.
        // Guard segments are still relative to the guard start and expression
        // segments still resolve through `prefix_len`.
        let text = "Page({ data: { title: 'x', user: { name: 'Ada', id: 7 } } });";
        let script = source_file("index.ts", text);
        let wxml = "<view>\n  <view wx:if=\"{{ title.length }}\">\n    <text>{{ {\n      name: user.name,\n      id: user.id\n    } }}</text>\n  </view>\n</view>";
        let condition = "title.length";
        let expr = "{\n      name: user.name,\n      id: user.id\n    }";
        let expr_start = wxml.find(expr).expect("expression in wxml");
        let condition_start = wxml.find(condition).expect("condition in wxml");
        let guard_range = SourceRange::new(
            u32::try_from(condition_start).expect("offset"),
            u32::try_from(condition_start + condition.len()).expect("offset"),
        );
        let mut value = FragmentSpec::new(
            "value",
            expr,
            SourceRange::new(
                u32::try_from(expr_start).expect("offset"),
                u32::try_from(expr_start + expr.len()).expect("offset"),
            ),
        );
        value.guards = vec![GuardChain {
            conditions: vec![GuardCondition {
                branch: GuardKind::If,
                expr: condition.to_owned(),
            }],
            selected: 0,
        }];
        let guard = FragmentSpec::new("guard", condition, guard_range);
        let fragments = [value, guard];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &fragments;
        let built = build_in_place_shadow(&request).expect("layout");
        assert_eq!(built.status, BuildStatus::Supported);

        let mount = built
            .view
            .mounts
            .iter()
            .find(|mount| mount.fragment_id == "value")
            .expect("value mount");
        assert!(
            mount
                .text
                .trim_start()
                .starts_with("if (__wxml_root.title.length) {"),
            "{}",
            mount.text
        );
        assert!(
            built.view.text.contains("({\n"),
            "the object literal is wrapped in the shadow body: {}",
            built.view.text
        );
        assert!(
            mount.text.contains("({\n"),
            "a folded mount starts at the guard block, so its text carries the opening paren: {}",
            mount.text
        );
        assert!(built.view.text.contains("});"));

        // The wrapper copy maps to the original `wx:if` bytes.
        let wrapper_byte = mount.text.find("if (").expect("guard wrapper") + "if (".len();
        let wrapper_start = mount.start_utf16 + utf16_len(&mount.text[..wrapper_byte]);
        let wrapper = crate::shadow::mapping::map_span_to_source(
            mount,
            crate::native::strings::Utf16Offset::new(wrapper_start),
            utf16_len("__wxml_root.title.length"),
        )
        .expect("mapped wrapper");
        assert_eq!(wrapper.range, Some(guard_range));
        assert!(!wrapper.trimmed_synthetic_prefix);

        // The object value maps through the fold without an offset.
        let target = expr.rfind("id").expect("target identifier");
        let mapped = crate::shadow::mapping::map_span_to_source(
            mount,
            crate::native::strings::Utf16Offset::new(mount.target_start_utf16),
            mount.target_length_utf16,
        )
        .expect("mapped value");
        assert_eq!(
            mapped.range,
            Some(SourceRange::new(
                u32::try_from(expr_start + target).expect("offset"),
                u32::try_from(expr_start + target + "id".len()).expect("offset"),
            ))
        );
    }

    #[test]
    fn invalid_fragment_ranges_are_rejected() {
        let script = source_file("index.ts", "Page({ data: {} });");
        let fragment = FragmentSpec::new("f0", "user.name", SourceRange::new(0, 1));
        let fragments = [fragment];
        let mut request = ShadowRequest::new(&script, RootKind::Page);
        request.fragments = &fragments;
        assert!(matches!(
            build_in_place_shadow(&request),
            Err(ShadowError::Invalid {
                feature: "fragment.range",
                ..
            })
        ));
    }

    /// The shared grammar reads both script token views; they must agree.
    #[test]
    fn layout_and_scanner_member_views_agree() {
        let text = "const o = {\n\
             a: 1,\n\
             b() {},\n\
             \"c\": 2,\n\
             4: 'five',\n\
             d,\n\
             ...e,\n\
             [f]: 3,\n\
             g: { h: 1, i: [2, 3] },\n\
             j: () => ({ k: 1 }),\n\
             l: `t${m}`,\n\
             n: function () { return 1; },\n\
             o(): number { return 1; },\n\
             get p() { return 1; },\n\
             set q(v: number) {},\n\
             *r() {},\n\
             async s() {},\n\
             't': 9,\n\
         };\n";
        let layout = Script::new(text).expect("layout lex");
        let scanner = crate::script::scanner::Script::new(text).expect("scanner lex");
        let (layout_open, layout_close) = object_bounds_of(&layout);
        let (scanner_open, scanner_close) = object_bounds_of(&scanner);
        assert_eq!((layout_open, layout_close), (scanner_open, scanner_close));
        let layout_members = scan_members(&layout, layout_open + 1, layout_close);
        let scanner_members = scan_members(&scanner, scanner_open + 1, scanner_close);
        assert_eq!(layout_members.len(), scanner_members.len());
        for (left, right) in layout_members.iter().zip(&scanner_members) {
            assert_eq!(left.name, right.name);
            assert_eq!(left.kind, right.kind);
            assert_eq!(
                (
                    left.key_start,
                    left.key_end,
                    left.value_start,
                    left.value_end
                ),
                (
                    right.key_start,
                    right.key_end,
                    right.value_start,
                    right.value_end
                )
            );
        }
    }

    fn object_bounds_of(script: &impl Members) -> (usize, usize) {
        let open = (0..4096)
            .find(|&index| script.punct_at(index, '{'))
            .expect("object open");
        (open, script.close_at(open).expect("object close"))
    }
}
