//! Event-handler completion from Rust script facts.
//!
//! The native adapter answers [`CompletionKind::EventHandler`] without the
//! TypeScript checker: candidates come from
//! [`ScriptAnalysis::methods`](wxml_backend_tsgo::script::ScriptAnalysis) for
//! the view's owning `Page`/`Component` registration (including statically
//! resolved `Behavior` methods). This path never builds the in-place shadow,
//! never opens a native session and never sends `getCompletionsAtPosition`;
//! the script method table is a purely syntactic host fact.
//!
//! Honesty rules:
//!
//! - a complete literal table with no opaque member is `Supported`;
//! - spread/computed members, opaque members, several registrations, a nested
//!   registration, an incomplete behavior graph or an incomplete prepared
//!   source list stay `Partial` while still publishing the proven names;
//! - no registration at all is `Unsupported` with an empty list, never a
//!   supported empty answer;
//! - a missing owning script or unrepresentable host input is an
//!   [`BackendError::Unavailable`];
//! - more than [`MAX_COMPLETION_ITEMS`] candidates are truncated to that many
//!   entries with `truncated = true` and a `Partial` status.
//!
//! The replacement range is the handler prefix inside the attribute value
//! (see [`crate::completion::expression::event_value_route`]), not
//! [`crate::completion::context::Context::replace`]: quotes, leading spaces
//! and any typed tail after the cursor survive.

use std::sync::{Mutex, PoisonError};

use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::script::events::{EventMethod, MethodTable};
use wxml_backend_tsgo::script::registration::RegistrationKind;
use wxml_backend_tsgo::script::{ScriptRequest, analyze_script};
use wxml_backend_tsgo::shadow::expression::is_identifier_name;
use wxml_project::typing::canonical_component_name;
use wxml_project::typing_contract::expected_signature;
use wxml_text::SourceRange;
use wxml_types::{
    BackendError, CapabilityStatus, CompletionItem, CompletionItemKind, CompletionRequest,
    CompletionResult, InsertTextFormat, MAX_COMPLETION_ITEMS, RequestContext, RootKind,
};

use crate::backend_tsgo::input::{ViewProvider, input_error, prepare_view};
use crate::completion::context;
use crate::completion::expression::{EventValueRejection, EventValueRoute, event_value_route};
use crate::project_host::{PreparedFile, PreparedView, ViewComponent, ViewTarget};

/// Maximum characters kept in one item `detail`.
const MAX_DETAIL_CHARS: usize = 120;
/// Maximum characters kept in one item `documentation`.
const MAX_DOCUMENTATION_CHARS: usize = 2048;
/// Maximum bytes kept in one degradation reason.
const MAX_REASON_BYTES: usize = 512;
/// FNV-1a offset basis for the prepared-file revision hash.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a prime for the prepared-file revision hash.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Stable reason for a script whose registration facts cannot be scanned.
const SCRIPT_UNSUPPORTED: &str =
    "script registration facts are unavailable for this event-handler table";
/// Stable reason for a template without an owning script.
const NO_TARGET: &str = "the template has no page or component script";
/// Stable reason for a non-event completion site.
const NOT_AN_EVENT_VALUE: &str = "the cursor is not inside an event-binding attribute value";

/// Cache identity of one event-handler candidate list.
///
/// The key deliberately excludes `view_id`, `view_revision`, the template
/// source revision and the cursor offset: script analysis does not read the
/// template, so editing WXML text must not invalidate the candidate list.
/// Any script path/revision change, the owner kind, the prepared-source
/// completeness flag, the project or its root invalidates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EventCacheKey {
    project: String,
    project_root: String,
    script_path: String,
    owner: RootKind,
    source_complete: bool,
    files_revision_hash: u64,
}

impl EventCacheKey {
    /// Builds the key from the request context and the prepared view.
    fn new(
        context: &RequestContext,
        view: &PreparedView,
        target: &ViewTarget,
    ) -> Result<Self, BackendError> {
        let project_root = view.project_root.to_str().ok_or_else(|| {
            BackendError::Unavailable("the project root is not valid UTF-8".to_owned())
        })?;
        Ok(Self {
            project: context.file().project().as_str().to_owned(),
            project_root: project_root.to_owned(),
            script_path: target.script.as_str().to_owned(),
            owner: target.owner,
            source_complete: view.source_complete,
            files_revision_hash: hash_files(&view.files),
        })
    }
}

/// One cached candidate list plus its degradation state.
///
/// The status is computed from the script facts once per cache entry and
/// reused for every request that hits it; it already includes any list
/// truncation, which does not depend on the request offset.
pub(crate) struct EventCandidateCache {
    key: EventCacheKey,
    candidates: Vec<EventMethod>,
    status: CapabilityStatus,
    truncated: bool,
    /// False for a script-analysis failure, which must be retried per request.
    cacheable: bool,
}

/// Answers one event-handler completion from the Rust script method table.
///
/// `provider` is only asked for the prepared view; the adapter's native
/// session, shadow builder and environment are untouched. A cache hit reuses
/// the analyzed candidates and only recomputes the item replacement range.
pub(crate) fn completion(
    provider: &dyn ViewProvider,
    request: &CompletionRequest,
    cache: &Mutex<Option<EventCandidateCache>>,
) -> Result<CompletionResult, BackendError> {
    if request.kind != wxml_types::CompletionKind::EventHandler {
        return Ok(unsupported(
            "the native event-handler path only serves event-handler completion",
        ));
    }
    let text = request.context.source().text();
    let Some(detected) = context::detect(text, request.offset) else {
        return Ok(unsupported(NOT_AN_EVENT_VALUE));
    };
    let replace = match event_value_route(text, request.offset, &detected) {
        EventValueRoute::Handler(prefix) => SourceRange::new(prefix.replace),
        EventValueRoute::Unsupported(reason) => return Ok(unsupported(rejection_reason(reason))),
        EventValueRoute::NotEvent => return Ok(unsupported(NOT_AN_EVENT_VALUE)),
    };

    let view = prepare_view(provider, request.context.source())?;
    let Some(target) = view.target.as_ref() else {
        return Ok(unsupported(NO_TARGET));
    };
    let key = EventCacheKey::new(&request.context, &view, target)?;
    {
        let guard = cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cached) = guard.as_ref().filter(|cached| cached.key == key) {
            return Ok(result_of(cached, replace));
        }
    }

    let fresh = build_candidates(&request.context, &view, target, key)?;
    let result = result_of(&fresh, replace);
    if fresh.cacheable {
        *cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(fresh);
    }
    Ok(result)
}

/// Builds the owning registration's method table without a prepared view.
///
/// This is the script-only seam used by view preparation to decide whether a
/// declared event handler has an unannotated parameter. It runs the same
/// analysis as [`completion`] but keeps no cache, maps no replacement range and
/// never touches the native engine. Files that cannot be represented are
/// skipped: the table is only ever used to prove an already-existing method's
/// parameter shape, never to prove a handler missing.
pub(crate) fn owner_methods(
    files: &[PreparedFile],
    project_root: &str,
    project: &str,
    script: &str,
    owner: RootKind,
) -> Option<MethodTable> {
    let mut sources = Vec::with_capacity(files.len());
    for prepared in files {
        let Ok(file) = SourceFile::new(
            prepared.path.as_str(),
            prepared.text.to_string(),
            DocumentRevision::new(prepared.source_revision),
        ) else {
            continue;
        };
        sources.push(file);
    }
    let root = ProjectRoot::new(project_root.to_owned()).ok()?;
    let input = AnalysisInput::new(
        ProjectKey::new(project),
        root,
        SnapshotRevision::new(1),
        sources,
    )
    .ok()?;
    analyze_script(&ScriptRequest::new(
        &input,
        script,
        registration_kind(owner),
    ))
    .ok()
    .map(|analysis| analysis.methods)
}

/// Maps a view owner to the registration kind whose method table it uses.
const fn registration_kind(owner: RootKind) -> RegistrationKind {
    match owner {
        RootKind::Page => RegistrationKind::Page,
        RootKind::Component => RegistrationKind::Component,
    }
}

/// Rebuilds one candidate list from the original effective script files.
///
/// The prepared files carry overlay-first text; the owning script must be
/// present. Files that cannot be represented are counted (and force
/// `Partial`), never silently dropped, except for the owning script, where the
/// request fails as `Unavailable`.
#[allow(clippy::too_many_lines, reason = "one linear script-fact pipeline")]
fn build_candidates(
    context: &RequestContext,
    view: &PreparedView,
    target: &ViewTarget,
    key: EventCacheKey,
) -> Result<EventCandidateCache, BackendError> {
    let script_path = target.script.as_str();
    if !view
        .files
        .iter()
        .any(|file| file.path.as_str() == script_path)
    {
        return Err(BackendError::Unavailable(
            "the owning script is missing from the prepared view".to_owned(),
        ));
    }
    let mut files = Vec::with_capacity(view.files.len());
    let mut dropped = 0_usize;
    for prepared in view.files.iter() {
        let represented = SourceFile::new(
            prepared.path.as_str(),
            prepared.text.to_string(),
            DocumentRevision::new(prepared.source_revision),
        );
        match represented {
            Ok(file) => files.push(file),
            Err(error) => {
                if prepared.path.as_str() == script_path {
                    return Err(input_error(&error));
                }
                dropped = dropped.saturating_add(1);
            }
        }
    }
    let root = ProjectRoot::new(key.project_root.clone()).map_err(|error| input_error(&error))?;
    let input = AnalysisInput::new(
        ProjectKey::new(context.file().project().as_str()),
        root,
        SnapshotRevision::new(1),
        files,
    )
    .map_err(|error| input_error(&error))?;
    let kind = registration_kind(target.owner);
    let Ok(analysis) = analyze_script(&ScriptRequest::new(&input, script_path, kind)) else {
        return Ok(EventCandidateCache {
            key,
            candidates: Vec::new(),
            status: CapabilityStatus::Unsupported {
                reason: SCRIPT_UNSUPPORTED.to_owned(),
            },
            truncated: false,
            cacheable: false,
        });
    };

    let mut candidates = analysis
        .methods
        .methods
        .iter()
        .filter(|method| is_identifier_name(&method.name))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.dedup_by(|left, right| left.name == right.name);
    let truncated = candidates.len() > MAX_COMPLETION_ITEMS;

    if analysis.registration.selected.is_none() {
        let reason = problems_reason(&analysis.methods.problems, &analysis.methods.status)
            .unwrap_or_else(|| "no statically readable page or component registration".to_owned());
        let no_registration_shape = analysis.registration.candidates.is_empty()
            && analysis.registration.unsupported.is_empty();
        return Ok(EventCandidateCache {
            key,
            candidates: Vec::new(),
            status: if no_registration_shape {
                CapabilityStatus::Unsupported { reason }
            } else {
                CapabilityStatus::Partial { reason }
            },
            truncated: false,
            cacheable: true,
        });
    }

    let mut reasons = analysis.methods.problems.clone();
    if reasons.is_empty() {
        if let Some(reason) = analysis.methods.status.reason() {
            reasons.push(reason.to_owned());
        }
    }
    if analysis
        .methods
        .opaque
        .iter()
        .any(|name| is_identifier_name(name))
    {
        push_reason(
            &mut reasons,
            "one or more handler-shaped members are not literal functions",
        );
    }
    if !analysis.registration.complete {
        push_reason(
            &mut reasons,
            "the selected registration is not a single top-level literal call",
        );
    }
    if !view.source_complete {
        push_reason(&mut reasons, "the prepared source file list is incomplete");
    }
    if dropped > 0 {
        push_reason(
            &mut reasons,
            format!("{dropped} script files could not be represented"),
        );
    }
    if truncated {
        push_reason(
            &mut reasons,
            format!("handler candidates are truncated at {MAX_COMPLETION_ITEMS} entries"),
        );
    }
    let status = if reasons.is_empty() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: bounded_reason(&reasons.join("; ")),
        }
    };
    Ok(EventCandidateCache {
        key,
        candidates,
        status,
        truncated,
        cacheable: true,
    })
}

/// Maps one cached candidate list and the current prefix range to items.
fn result_of(cache: &EventCandidateCache, replace: SourceRange) -> CompletionResult {
    let items = cache
        .candidates
        .iter()
        .take(MAX_COMPLETION_ITEMS)
        .map(|method| CompletionItem {
            label: method.name.clone(),
            kind: CompletionItemKind::Method,
            detail: detail_of(&method.signature),
            documentation: documentation_of(&method.documentation),
            insert_text: method.name.clone(),
            insert_text_format: InsertTextFormat::PlainText,
            replace,
            sort_text: Some(format!("0_{}", method.name)),
            filter_text: Some(method.name.clone()),
        })
        .collect();
    CompletionResult {
        items,
        status: cache.status.clone(),
        truncated: cache.truncated,
    }
}

/// Collapses one signature to a bounded single line.
fn detail_of(signature: &str) -> Option<String> {
    let collapsed = signature.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(cap_chars(&collapsed, MAX_DETAIL_CHARS))
}

/// Colon-form event prefixes whose event-name fragment can be completed from a
/// component declaration.
///
/// Checked before the compact forms so `bind:` is never read as compact `bind`
/// plus a leading `:`.
const DECLARED_EVENT_COLON_PREFIXES: [&str; 5] = [
    "capture-bind:",
    "capture-catch:",
    "mut-bind:",
    "bind:",
    "catch:",
];

/// Compact event prefixes. They keep the same literal-prefix semantics as
/// [`wxml_types`](wxml_types::TemplateAnalyzer): `binder` is `bind` + event
/// `er`, and a compact prefix never swallows a leading `:`.
const DECLARED_EVENT_COMPACT_PREFIXES: [&str; 5] =
    ["capture-bind", "capture-catch", "mut-bind", "bind", "catch"];

/// Returns the binding prefix that `typed_name` starts with, if any.
fn declared_event_prefix(typed_name: &str) -> Option<&'static str> {
    if let Some(prefix) = DECLARED_EVENT_COLON_PREFIXES
        .iter()
        .find(|prefix| typed_name.starts_with(**prefix))
    {
        return Some(*prefix);
    }
    DECLARED_EVENT_COMPACT_PREFIXES
        .iter()
        .find(|prefix| {
            typed_name
                .strip_prefix(**prefix)
                .is_some_and(|rest| !rest.starts_with(':'))
        })
        .copied()
}

/// Returns whether a typed fragment may still name the declared event.
///
/// Both sides are compared through
/// [`canonical_component_name`](wxml_project::typing::canonical_component_name),
/// so `some-` matches a declared `someEvent` and `someE` matches a declared
/// `'some-event'`; the raw prefix is accepted as well.
fn fragment_matches(name: &str, fragment: &str) -> bool {
    name.starts_with(fragment) || {
        let canonical = canonical_component_name(name);
        canonical.starts_with(&canonical_component_name(fragment))
    }
}

/// `someEvent` -> `some-event`.
fn kebab_form(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_uppercase() {
            result.push('-');
            result.extend(character.to_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

/// Event-name completion from a component's declared events.
///
/// `typed_name` is the attribute-name text up to the cursor and
/// `name_start` its byte start. The replacement covers only the event-name
/// fragment after the binding prefix, so `bind:cha` becomes `bind:change` and
/// the prefix is never rewritten. Every legal binding form (colon and compact)
/// is recognised.
///
/// The inserted text follows the typed fragment: a fragment containing `-`
/// inserts the declaration's kebab spelling, anything else the camel spelling,
/// so `bind:some-` becomes `bind:some-event` while the label keeps the declared
/// name.
///
/// Returns `None` for an undeclared, incomplete or event-less component, for a
/// name that is not a binding prefix, and for a fragment that is not an
/// event-name prefix. Callers keep the pre-declaration candidates in every
/// such case.
pub(crate) fn declared_event_items(
    component: &ViewComponent,
    typed_name: &str,
    name_start: u32,
) -> Option<Vec<CompletionItem>> {
    let typing = component.typing.as_deref()?;
    if !typing.complete || typing.events.is_empty() {
        return None;
    }
    let prefix = declared_event_prefix(typed_name)?;
    let fragment = typed_name.get(prefix.len()..)?;
    if !fragment
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let start = name_start.checked_add(u32::try_from(prefix.len()).ok()?)?;
    let end = name_start.checked_add(u32::try_from(typed_name.len()).ok()?)?;
    let replace = SourceRange::from_bounds(start, end).ok()?;
    let kebab_style = fragment.contains('-');
    let mut items = Vec::new();
    for event in &typing.events {
        if !fragment_matches(&event.name, fragment) {
            continue;
        }
        let insert_text = if kebab_style {
            kebab_form(&event.name)
        } else {
            canonical_component_name(&event.name)
        };
        let signature = expected_signature(event.detail_text.as_deref());
        let detail = if event.proven {
            signature.clone()
        } else {
            format!("{signature}（detail 类型未证明）")
        };
        items.push(CompletionItem {
            label: event.name.clone(),
            kind: CompletionItemKind::Event,
            detail: Some(cap_chars(&detail, MAX_DETAIL_CHARS)),
            documentation: Some(format!(
                "组件 `{}` 声明的事件处理函数签名：`{}`",
                component.name, signature
            )),
            insert_text,
            insert_text_format: InsertTextFormat::PlainText,
            replace,
            sort_text: Some(format!("0_{}", event.name)),
            filter_text: Some(event.name.clone()),
        });
    }
    Some(items)
}

/// Returns bounded script `JSDoc`; empty documentation stays absent.
fn documentation_of(documentation: &str) -> Option<String> {
    if documentation.trim().is_empty() {
        return None;
    }
    Some(cap_chars(documentation, MAX_DOCUMENTATION_CHARS))
}

/// Caps text by `char` count so UTF-8 and surrogate pairs stay intact.
fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut capped = text.chars().take(max.saturating_sub(1)).collect::<String>();
    capped.push('…');
    capped
}

/// Stable, source-free reason for one precise value rejection.
const fn rejection_reason(rejection: EventValueRejection) -> &'static str {
    match rejection {
        EventValueRejection::OutsideValue => {
            "the cursor is outside the event-binding attribute value"
        }
        EventValueRejection::QuoteMismatch => "the event-binding value quoting is inconsistent",
        EventValueRejection::DynamicValue => {
            "event-handler completion is unavailable for dynamic attribute values"
        }
        EventValueRejection::BeforeToken => "the cursor is before the event-handler name",
        EventValueRejection::AtTokenStart => {
            "the cursor is at the start of the handler name; type a prefix first"
        }
        EventValueRejection::AfterToken => "the cursor is after the event-handler name",
        EventValueRejection::InvalidHandlerToken => {
            "the event-binding value is not a single handler name"
        }
        EventValueRejection::Unrepresentable => {
            "the event-binding attribute value cannot be represented"
        }
    }
}

/// Builds the checked `Unsupported` result; never a supported empty list.
fn unsupported(reason: &str) -> CompletionResult {
    CompletionResult {
        items: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
        truncated: false,
    }
}

/// Joins the degradation problems when present, else the table reason.
fn problems_reason(problems: &[String], status: &CapabilityStatus) -> Option<String> {
    if !problems.is_empty() {
        return Some(bounded_reason(&problems.join("; ")));
    }
    status.reason().map(bounded_reason)
}

/// Appends one reason unless the exact text is already present.
fn push_reason(reasons: &mut Vec<String>, reason: impl Into<String>) {
    let reason = reason.into();
    if !reasons.iter().any(|existing| existing == &reason) {
        reasons.push(reason);
    }
}

/// Caps a reason at [`MAX_REASON_BYTES`] on a character boundary.
fn bounded_reason(reason: &str) -> String {
    if reason.len() <= MAX_REASON_BYTES {
        return reason.to_owned();
    }
    let mut end = MAX_REASON_BYTES;
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_owned()
}

/// Hashes the sorted `(path, source_revision)` pairs of the prepared files.
fn hash_files(files: &[PreparedFile]) -> u64 {
    let mut entries = files
        .iter()
        .map(|file| (file.path.as_str(), file.source_revision))
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut hash = FNV_OFFSET_BASIS;
    for (path, revision) in entries {
        hash_bytes(&mut hash, path.as_bytes());
        hash_bytes(&mut hash, &revision.to_le_bytes());
        // Path bytes never contain 0xff, so it separates adjacent entries.
        hash_bytes(&mut hash, &[0xff]);
    }
    hash
}

/// Extends one FNV-1a hash with `bytes`.
fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
mod tests;
