//! Native completion mapping with explicit details/auto-import degradation.
//!
//! The pinned 7.1.0-beta `getCompletionsAtPosition` response emits `name`,
//! `kind` (an LSP `CompletionItemKind` number), `sortText` and, when
//! `includeSymbol:true` is requested on a snapshot prepared with
//! `prepareAutoImports`, `symbol`. Optional `insertText` / `filterText` /
//! `detail` / `labelDetails` members are used when present.
//!
//! `includeSymbol:true` must never be sent on an unprepared snapshot: the
//! engine rejects the whole request. The adapter therefore prepares the current
//! snapshot for the owning script first and only then requests symbols. A
//! preparation failure keeps completion functional without symbols and reports
//! the bounded reason in the status; it never becomes a supported empty list.
//!
//! The pinned API has no `getCompletionEntryDetails`, no per-entry
//! documentation, no display parts and no auto-import origin or import edit
//! (`data`/`textEdit`/`additionalTextEdits`). A prepared non-empty result is
//! therefore still `Partial` with the typed native gaps, and this module never
//! invents an `import` statement, a text edit or an auto-import marker.
//!
//! # Data roots and incomplete expressions
//!
//! The template analyzer drops empty mustaches (`{{ }}`) and expressions it
//! cannot parse (`{{ user. }}`), so those sites have no mounted fragment.
//! Root candidates come from the Rust
//! [`ScriptAnalysis`](wxml_backend_tsgo::script::ScriptAnalysis) data overlay
//! and need neither a native session nor a native snapshot. A bare identifier
//! prefix replaces only that prefix. A trailing-dot member access is asked
//! through a completion-only shadow plan whose valid prefix is mounted and
//! followed by a synthetic `.` and placeholder; the published replacement
//! range is still computed from the original text and never covers the
//! synthetic query bytes. The prefix may be any complete expression whose first
//! identifier is a resolvable root: a bare data root, a template `wx:for` local
//! (`item.`) or a member chain (`user.name.`, `item.Setting.`). An unclosed
//! mustache with a typed member prefix (`{{ user.na`) mounts the complete
//! prefix and replaces only `na`. Template locals are resolved before script
//! data roots, so a `wx:for` local shadows a same-named `data` root. An
//! expression that cannot be completed safely is an explicit
//! `Unsupported`/`Partial` reason, never a silent empty list.

use crate::backend_tsgo::query::completion::{
    AutoImportContext, CompletionEntry as NativeCompletionEntry, CompletionQueryOptions,
    map_completions_with,
};
use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::native::NativeError;
use wxml_backend_tsgo::native::session::AutoImportPreparation;
use wxml_backend_tsgo::native::strings::{Utf16Offset, Utf16Position, offset_to_position};
use wxml_backend_tsgo::script::properties::DataRootKind;
use wxml_backend_tsgo::script::registration::RegistrationKind;
use wxml_backend_tsgo::script::{ScriptRequest, analyze_script};
use wxml_syntax::{Expr, NodeKind, parse, parse_expression};
use wxml_text::SourceRange;
use wxml_types::{
    BackendError, CapabilityStatus, CompletionItem, CompletionItemKind, CompletionKind,
    CompletionRequest, CompletionResult, InsertTextFormat, MAX_COMPLETION_ITEMS, RequestContext,
    RootKind,
};

use crate::backend_tsgo::input::{ViewProvider, input_error, prepare_view};
use crate::backend_tsgo::map_native_error;
use crate::backend_tsgo::mapping::{MappingError, ShadowLayout};
use crate::backend_tsgo::session::QuerySession;

/// Returns completion entries mapped to original template bytes.
///
/// `query` overrides the native position and `replace` overrides the published
/// replacement range. The synthetic trailing-dot path uses both: its query is
/// derived from an inserted `.` while the replacement range still comes from
/// the original template bytes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn completion(
    session: &mut dyn QuerySession,
    layout: &ShadowLayout,
    script_path: &str,
    degradation: Option<&str>,
    request: &CompletionRequest,
    query: Option<Utf16Position>,
    replace: Option<SourceRange>,
) -> Result<CompletionResult, BackendError> {
    if request.kind != CompletionKind::ExpressionMember {
        return Ok(unsupported_completion(
            "only expression-member completion is implemented for the native backend",
        ));
    }
    let position = match query {
        Some(position) => position,
        None => match query_position(layout, request.offset)? {
            Some(position) => position,
            None => {
                return Ok(unsupported_completion(
                    "the cursor is in an incomplete or unmounted template expression; member completion requires a host-parsed fragment",
                ));
            }
        },
    };
    let (options, auto_imports, prepare_reason) = auto_import_request(session, script_path)?;
    let degradation = merge_degradation(degradation, prepare_reason);
    let raw = match session.completions_with_options(script_path, position, options) {
        Ok(raw) => raw,
        Err(NativeError::Unsupported { .. }) => {
            return Ok(unsupported_completion(
                "native completion queries are unavailable",
            ));
        }
        Err(error) => return Err(map_native_error(error)),
    };
    let mapped = map_completions_with(raw.as_ref(), auto_imports);
    let replace = if let Some(replace) = replace {
        replace
    } else if let Some(replace) = completion_replace(layout, request.offset)? {
        replace
    } else {
        let status = if mapped.entries.is_empty() {
            mapped.status
        } else {
            CapabilityStatus::Partial {
                reason: "the completion replacement range cannot be determined".to_owned(),
            }
        };
        return Ok(CompletionResult {
            items: Vec::new(),
            status: merge_status(status, degradation.as_deref()),
            truncated: mapped.truncated,
        });
    };
    Ok(CompletionResult {
        items: mapped
            .entries
            .into_iter()
            .map(|entry| completion_item(entry, replace))
            .collect(),
        status: merge_status(mapped.status, degradation.as_deref()),
        truncated: mapped.truncated,
    })
}

/// Prepares the snapshot and returns the request options plus its context.
///
/// A preparation failure is downgraded to a status reason: completion still
/// runs without symbols instead of failing the whole request.
fn auto_import_request(
    session: &mut dyn QuerySession,
    script_path: &str,
) -> Result<
    (
        CompletionQueryOptions,
        AutoImportContext,
        Option<&'static str>,
    ),
    BackendError,
> {
    Ok(match session.prepare_auto_imports(script_path) {
        Ok(AutoImportPreparation::Prepared) => (
            CompletionQueryOptions::SYMBOLS,
            AutoImportContext::Prepared,
            None,
        ),
        Ok(AutoImportPreparation::Unavailable { reason }) => (
            CompletionQueryOptions::default(),
            AutoImportContext::Unavailable,
            Some(reason),
        ),
        Ok(AutoImportPreparation::NotSupported) => (
            CompletionQueryOptions::default(),
            AutoImportContext::Unavailable,
            None,
        ),
        Err(NativeError::Unsupported { .. }) => (
            CompletionQueryOptions::default(),
            AutoImportContext::Unavailable,
            Some("auto-import preparation is unavailable in this session"),
        ),
        Err(error) => return Err(map_native_error(error)),
    })
}

/// Maps the original-template offset to a native shadow position.
pub(crate) fn query_position(
    layout: &ShadowLayout,
    offset: u32,
) -> Result<Option<Utf16Position>, BackendError> {
    let derived = match layout.derived_offset(offset, true) {
        Ok(offset) => offset,
        Err(MappingError::NoFragment | MappingError::MappingGap) => return Ok(None),
        Err(MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. }) => {
            return Err(BackendError::Protocol(
                "completion offset is not a valid template character boundary".to_owned(),
            ));
        }
        Err(error) => {
            return Err(BackendError::Protocol(format!(
                "completion mapping failed: {error}"
            )));
        }
    };
    offset_to_position(layout.derived(), Utf16Offset::new(derived))
        .map(Some)
        .map_err(|_| {
            BackendError::Protocol("completion position is invalid in the shadow".to_owned())
        })
}

/// Returns the replacement range, or `None` when the prefix cannot be mapped.
fn completion_replace(
    layout: &ShadowLayout,
    offset: u32,
) -> Result<Option<SourceRange>, BackendError> {
    match layout.completion_replace_range(offset) {
        Ok(range) => Ok(Some(range)),
        Err(MappingError::NoFragment | MappingError::MappingGap) => Ok(None),
        Err(MappingError::NotCharBoundary { .. } | MappingError::OffsetOutOfRange { .. }) => {
            Err(BackendError::Protocol(
                "completion replacement is not a valid template character boundary".to_owned(),
            ))
        }
        Err(error) => Err(BackendError::Protocol(format!(
            "completion replacement mapping failed: {error}"
        ))),
    }
}

/// Publishes one native entry as a C0 completion item.
///
/// `insertText`/`filterText` are native evidence; snippet-shaped insert text
/// was already dropped by the mapper. No `documentation`, import statement or
/// additional text edit can be represented or is invented.
fn completion_item(entry: NativeCompletionEntry, replace: SourceRange) -> CompletionItem {
    let sort_text = (!entry.sort_text.is_empty()).then_some(entry.sort_text);
    let insert_text = entry.insert_text.unwrap_or_else(|| entry.label.clone());
    let filter_text = entry.filter_text.unwrap_or_else(|| entry.label.clone());
    CompletionItem {
        label: entry.label,
        kind: entry.kind,
        detail: entry.detail,
        documentation: None,
        insert_text,
        insert_text_format: InsertTextFormat::PlainText,
        replace,
        sort_text,
        filter_text: Some(filter_text),
    }
}

/// Appends the auto-import preparation reason to a host degradation.
fn merge_degradation(
    degradation: Option<&str>,
    prepare_reason: Option<&'static str>,
) -> Option<String> {
    match (degradation, prepare_reason) {
        (Some(existing), Some(reason)) => Some(format!("{existing}; {reason}")),
        (Some(existing), None) => Some(existing.to_owned()),
        (None, Some(reason)) => Some(reason.to_owned()),
        (None, None) => None,
    }
}

fn merge_status(status: CapabilityStatus, degradation: Option<&str>) -> CapabilityStatus {
    match (status, degradation) {
        (CapabilityStatus::Supported, Some(reason)) => CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
        (CapabilityStatus::Partial { reason }, Some(degradation)) => CapabilityStatus::Partial {
            reason: format!("{reason}; {degradation}"),
        },
        (status, _) => status,
    }
}

pub(crate) fn unsupported_completion(reason: &str) -> CompletionResult {
    CompletionResult {
        items: Vec::new(),
        status: CapabilityStatus::Unsupported {
            reason: reason.to_owned(),
        },
        truncated: false,
    }
}

/// Returns an empty `Partial` answer with an explicit reason.
pub(crate) const fn partial_completion(reason: String) -> CompletionResult {
    CompletionResult {
        items: Vec::new(),
        status: CapabilityStatus::Partial { reason },
        truncated: false,
    }
}

/* ------------------------------------------------------------------ expression sites */

/// One mustache completion site that may not be covered by a mounted fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExpressionSite {
    /// A bare data root (empty or an identifier prefix before the cursor).
    Root {
        /// Range replaced by an inserted root; only the typed prefix.
        replace: SourceRange,
    },
    /// A member access with a resolvable expression prefix.
    ///
    /// `{{ user.` has an empty typed prefix; `{{ user.na` (unclosed) has the
    /// typed prefix `na`. The prefix may be a bare root, a member chain
    /// (`user.name`) or a template local (`item.Setting`). Only the typed
    /// prefix is replaced.
    Member {
        /// Valid expression before the trailing dot.
        prefix: String,
        /// Exact original byte range of `prefix`.
        prefix_range: SourceRange,
        /// Range replaced by an inserted member; only the typed prefix.
        replace: SourceRange,
    },
    /// The expression before the cursor cannot be completed safely.
    Unsupported(&'static str),
}

/// Classifies the mustache expression at `offset` without a prepared view.
///
/// Returns `None` when the cursor is not inside a mustache body. A cursor in
/// the delimiters, outside any mustache or in a literal-only expression is not
/// a data-root or member site.
///
/// A closed mustache with a complete expression keeps the ordinary mounted
/// fragment path. An unclosed mustache is dropped by the template analyzer, so
/// a typed member prefix (`{{ user.na`) is mounted as its complete prefix and
/// queried through the same synthetic dot as `{{ user.`.
pub(crate) fn expression_site(text: &str, offset: u32) -> Option<ExpressionSite> {
    let (expr_start, expr_end, closed) = mustache_expression_range(text, offset)?;
    let cursor = usize::try_from(offset).ok()?;
    let start = usize::try_from(expr_start).ok()?;
    if cursor < start {
        let replace = SourceRange::from_bounds(offset, offset).ok()?;
        return Some(ExpressionSite::Root { replace });
    }
    let before = text.get(start..cursor)?;
    let body = before.trim_start();
    let leading = before.len().checked_sub(body.len())?;
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        let replace_start = identifier_start(text, offset, expr_start)?;
        let replace = SourceRange::from_bounds(replace_start, offset).ok()?;
        return Some(ExpressionSite::Root { replace });
    }
    // A root prefix must be the identifier directly before the cursor. A
    // trailing space means the cursor is between tokens, not inside a root.
    if trimmed.len() == body.len() && is_identifier_text(trimmed) {
        let replace_start = identifier_start(text, offset, expr_start)?;
        let replace = SourceRange::from_bounds(replace_start, offset).ok()?;
        return Some(ExpressionSite::Root { replace });
    }
    // A complete full expression already has a mounted fragment (or is
    // deliberately unmountable as a literal); let the ordinary native path
    // answer it instead of synthesizing a query. An unclosed mustache has no
    // mounted fragment, so only a typed member prefix at the cursor can be
    // synthesized safely.
    let full_end = usize::try_from(expr_end).ok()?;
    if let Some(full) = text.get(start..full_end) {
        if !full.is_empty() {
            let parsed = parse_expression(full, 0);
            if parsed.errors.is_empty() && !parsed.expr.walk().any(Expr::is_missing) {
                if !crate::backend_tsgo::input::expression_has_free_root(&parsed.expr, full) {
                    return Some(ExpressionSite::Unsupported(
                        "the expression has no data root to complete",
                    ));
                }
                if closed {
                    return None;
                }
            }
        }
    }
    // Only a chain ending directly at the cursor may be synthesized. A trailing
    // space or newline makes the intended member ambiguous.
    if trimmed.len() != body.len() {
        return Some(ExpressionSite::Unsupported(
            "the cursor is not immediately after the expression prefix",
        ));
    }
    // An unclosed mustache may still carry a complete expression plus trailing
    // template text (`{{ user.na</view>`). The typed identifier itself is the
    // safe prefix; it is mounted as its bare data root.
    if !closed && body.ends_with(is_identifier_character) {
        return Some(typed_member_site(text, body, offset, expr_start, leading));
    }
    let Some(prefix) = split_member_prefix(body) else {
        return Some(ExpressionSite::Unsupported(
            "the expression before the cursor is not a data root or a member access",
        ));
    };
    if prefix.is_empty() {
        return Some(ExpressionSite::Unsupported(
            "the expression before the cursor has no value to complete members on",
        ));
    }
    // The synthetic query mounts the whole prefix. It is safe as long as the
    // prefix parses completely and its first identifier is a root the scope can
    // resolve; `completion_member` decides which root actually applies.
    let parsed = parse_expression(prefix, 0);
    if !parsed.errors.is_empty() || parsed.expr.walk().any(Expr::is_missing) {
        return Some(ExpressionSite::Unsupported(
            "the expression before the trailing dot is not a complete expression",
        ));
    }
    if !crate::backend_tsgo::input::expression_has_free_root(&parsed.expr, prefix) {
        return Some(ExpressionSite::Unsupported(
            "the expression before the trailing dot has no data root to complete",
        ));
    }
    let prefix_len = u32::try_from(prefix.len()).ok()?;
    let prefix_start = expr_start.checked_add(u32::try_from(leading).ok()?)?;
    let prefix_range =
        SourceRange::from_bounds(prefix_start, prefix_start.checked_add(prefix_len)?).ok()?;
    let replace_start = identifier_start(text, offset, expr_start)?;
    let replace = SourceRange::from_bounds(replace_start, offset).ok()?;
    Some(ExpressionSite::Member {
        prefix: prefix.to_owned(),
        prefix_range,
        replace,
    })
}

/// Returns the `wx:for` bindings in scope at `offset` in `text`.
///
/// The template analyzer drops an incomplete trailing-dot mustache, so member
/// completion re-parses the current request text to recover the lexical loop
/// scope. The result is outermost to innermost, exactly as the ordinary shadow
/// plan attaches it to a fragment.
pub(crate) fn loop_bindings(text: &str, offset: u32) -> Vec<wxml_types::LocalBinding> {
    let parsed = parse(text);
    wxml_types::template::loop_bindings_at(&parsed.tree, offset)
}

/// Splits the valid expression before a trailing dot or optional-chain dot.
fn split_member_prefix(chain: &str) -> Option<&str> {
    chain.strip_suffix("?.").or_else(|| chain.strip_suffix('.'))
}

/// Classifies `user.na` in an unclosed mustache as a member prefix.
///
/// The mount is the bare root `user` plus the synthetic dot used by
/// `{{ user.`; the published replacement range is still exactly the typed
/// identifier `na`, never the dot or the unclosed suffix.
fn typed_member_site(
    text: &str,
    body: &str,
    offset: u32,
    expr_start: u32,
    leading: usize,
) -> ExpressionSite {
    let Some(typed_start) = identifier_start(text, offset, expr_start) else {
        return ExpressionSite::Unsupported("the typed member prefix cannot be located");
    };
    let Ok(cursor) = usize::try_from(offset) else {
        return ExpressionSite::Unsupported("the typed member prefix is out of range");
    };
    let Ok(typed_start_usize) = usize::try_from(typed_start) else {
        return ExpressionSite::Unsupported("the typed member prefix is out of range");
    };
    let Some(typed) = text.get(typed_start_usize..cursor) else {
        return ExpressionSite::Unsupported("the typed member prefix is not valid text");
    };
    if !is_identifier_text(typed) {
        return ExpressionSite::Unsupported(
            "the typed member prefix is not an identifier before the cursor",
        );
    }
    let Some(body_start) = usize::try_from(expr_start)
        .ok()
        .and_then(|start| start.checked_add(leading))
    else {
        return ExpressionSite::Unsupported("the expression start is out of range");
    };
    let Some(relative) = typed_start_usize.checked_sub(body_start) else {
        return ExpressionSite::Unsupported("the typed member prefix is outside the expression");
    };
    let Some(before_typed) = body.get(..relative) else {
        return ExpressionSite::Unsupported("the typed member prefix is outside the expression");
    };
    let Some(prefix) = split_member_prefix(before_typed) else {
        return ExpressionSite::Unsupported(
            "the typed prefix is not a member access of a data root",
        );
    };
    if prefix.is_empty() {
        return ExpressionSite::Unsupported(
            "the expression before the cursor has no value to complete members on",
        );
    }
    // The typed member is completed against the whole prefix before it, which
    // may itself be a member chain such as `item.Setting.`.
    let parsed = parse_expression(prefix, 0);
    if !parsed.errors.is_empty() || parsed.expr.walk().any(Expr::is_missing) {
        return ExpressionSite::Unsupported(
            "the expression before the typed prefix is not a complete expression",
        );
    }
    if !crate::backend_tsgo::input::expression_has_free_root(&parsed.expr, prefix) {
        return ExpressionSite::Unsupported(
            "the expression before the typed prefix has no data root to complete",
        );
    }
    let Ok(prefix_len) = u32::try_from(prefix.len()) else {
        return ExpressionSite::Unsupported("the data root is too long to complete");
    };
    let Ok(prefix_start) = u32::try_from(body_start) else {
        return ExpressionSite::Unsupported("the expression start is out of range");
    };
    let Some(prefix_end) = prefix_start.checked_add(prefix_len) else {
        return ExpressionSite::Unsupported("the data root range overflows");
    };
    let Ok(prefix_range) = SourceRange::from_bounds(prefix_start, prefix_end) else {
        return ExpressionSite::Unsupported("the data root range is invalid");
    };
    let Ok(replace) = SourceRange::from_bounds(typed_start, offset) else {
        return ExpressionSite::Unsupported("the typed member range is invalid");
    };
    ExpressionSite::Member {
        prefix: prefix.to_owned(),
        prefix_range,
        replace,
    }
}

/// Returns the trimmed expression range of the mustache containing `offset`.
fn mustache_expression_range(text: &str, offset: u32) -> Option<(u32, u32, bool)> {
    let parsed = parse(text);
    for (_, node) in parsed.tree.nodes() {
        if node.kind() != NodeKind::Mustache {
            continue;
        }
        let range = node.text_range();
        let body_start = range.start().checked_add(2)?;
        if offset < body_start {
            continue;
        }
        let inside = if node.is_closed() {
            offset <= range.end().saturating_sub(2)
        } else {
            offset <= range.end()
        };
        if !inside {
            continue;
        }
        let child = node
            .children()
            .iter()
            .copied()
            .find(|child| parsed.tree.kind(*child) == Some(NodeKind::MustacheExpr))?;
        let expr = parsed.tree.node(child)?.text_range();
        return Some((expr.start(), expr.end(), node.is_closed()));
    }
    None
}

/// Returns the start of the identifier characters directly before `offset`.
fn identifier_start(text: &str, offset: u32, floor: u32) -> Option<u32> {
    let mut start = usize::try_from(offset).ok()?;
    let floor = usize::try_from(floor).ok()?;
    if start > text.len() {
        return None;
    }
    while start > floor {
        let (previous, character) = text.get(..start)?.char_indices().next_back()?;
        if previous < floor || !is_identifier_character(character) {
            break;
        }
        start = previous;
    }
    u32::try_from(start).ok()
}

/// Returns whether `text` is one ECMAScript-style identifier (Unicode aware).
fn is_identifier_text(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_alphabetic())
        && characters
            .all(|character| character == '_' || character == '$' || character.is_alphanumeric())
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character == '$' || character.is_alphanumeric()
}

/// Returns the first identifier spelling in an expression prefix.
fn first_identifier(prefix: &str) -> Option<&str> {
    for (index, character) in prefix.char_indices() {
        if !(character == '_' || character == '$' || character.is_alphabetic()) {
            continue;
        }
        let end = prefix[index..]
            .char_indices()
            .find(|(_, candidate)| !is_identifier_character(*candidate))
            .map_or(prefix.len(), |(relative, _)| index + relative);
        return prefix.get(index..end);
    }
    None
}

/* ---------------------------------------------------------------------- roots */

/// One data-root candidate from the script overlay.
struct RootCandidate {
    name: String,
    detail: String,
    documentation: String,
}

/// Root facts for one prepared view.
struct RootFacts {
    candidates: Vec<RootCandidate>,
    reasons: Vec<String>,
    /// True when every data/property/registration fact was read exactly.
    overlay_complete: bool,
    truncated: bool,
}

/// Outcome of a root lookup.
enum RootOutcome {
    /// The script facts cannot answer this template at all.
    Unsupported(String),
    /// Root candidates plus any completeness reasons.
    Facts(RootFacts),
}

/// Result of checking whether a member-access root is a known data field.
pub(crate) enum MemberRoot {
    /// The root is a declared data field, component property or behavior data.
    Known,
    /// The root is not declared and every overlay was read completely.
    UnknownComplete,
    /// The root is not declared but the overlays are incomplete.
    UnknownIncomplete(String),
    /// The script facts cannot answer this template.
    Unavailable(String),
}

/// Answers a bare `{{ }}` / `{{ prefix }}` completion from the script overlay.
pub(crate) fn root_completion(
    provider: &dyn ViewProvider,
    request: &CompletionRequest,
    replace: SourceRange,
) -> Result<CompletionResult, BackendError> {
    let outcome = script_roots(provider, &request.context)?;
    let facts = match outcome {
        RootOutcome::Unsupported(reason) => return Ok(unsupported_completion(&reason)),
        RootOutcome::Facts(facts) => facts,
    };
    let items = facts
        .candidates
        .iter()
        .take(MAX_COMPLETION_ITEMS)
        .map(|candidate| CompletionItem {
            label: candidate.name.clone(),
            kind: CompletionItemKind::Property,
            detail: (!candidate.detail.is_empty()).then(|| candidate.detail.clone()),
            // Script `JSDoc` is real evidence, unlike the pinned completion
            // API which exposes no documentation.
            documentation: (!candidate.documentation.is_empty())
                .then(|| candidate.documentation.clone()),
            insert_text: candidate.name.clone(),
            insert_text_format: InsertTextFormat::PlainText,
            replace,
            sort_text: Some(format!("0_{}", candidate.name)),
            filter_text: Some(candidate.name.clone()),
        })
        .collect();
    let status = if facts.reasons.is_empty() {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: bounded_reason(&facts.reasons.join("; ")),
        }
    };
    Ok(CompletionResult {
        items,
        status,
        truncated: facts.truncated,
    })
}

/// Checks one member-access root against the script overlay.
pub(crate) fn member_root(
    provider: &dyn ViewProvider,
    request: &CompletionRequest,
    root: &str,
) -> Result<MemberRoot, BackendError> {
    match script_roots(provider, &request.context)? {
        RootOutcome::Unsupported(reason) => Ok(MemberRoot::Unavailable(reason)),
        RootOutcome::Facts(facts) => {
            if facts
                .candidates
                .iter()
                .any(|candidate| candidate.name == root)
            {
                return Ok(MemberRoot::Known);
            }
            if facts.overlay_complete {
                return Ok(MemberRoot::UnknownComplete);
            }
            let mut reasons = facts.reasons;
            reasons.push(format!(
                "members of `{root}` cannot be resolved from an incomplete script data overlay"
            ));
            Ok(MemberRoot::UnknownIncomplete(bounded_reason(
                &reasons.join("; "),
            )))
        }
    }
}

/// Reads the Rust script overlay for one prepared view.
#[allow(clippy::too_many_lines, reason = "one linear script-fact pipeline")]
fn script_roots(
    provider: &dyn ViewProvider,
    context: &RequestContext,
) -> Result<RootOutcome, BackendError> {
    let view = prepare_view(provider, context.source())?;
    let Some(target) = view.target.as_ref() else {
        return Ok(RootOutcome::Unsupported(
            "the template has no page or component script".to_owned(),
        ));
    };
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
        match SourceFile::new(
            prepared.path.as_str(),
            prepared.text.to_string(),
            DocumentRevision::new(prepared.source_revision),
        ) {
            Ok(file) => files.push(file),
            Err(error) => {
                if prepared.path.as_str() == script_path {
                    return Err(input_error(&error));
                }
                dropped = dropped.saturating_add(1);
            }
        }
    }
    let root = ProjectRoot::new(view.project_root.to_str().ok_or_else(|| {
        BackendError::Unavailable("the project root is not valid UTF-8".to_owned())
    })?)
    .map_err(|error| input_error(&error))?;
    let input = AnalysisInput::new(
        ProjectKey::new(context.file().project().as_str()),
        root,
        SnapshotRevision::new(1),
        files,
    )
    .map_err(|error| input_error(&error))?;
    let kind = match target.owner {
        RootKind::Page => RegistrationKind::Page,
        RootKind::Component => RegistrationKind::Component,
    };
    let Ok(analysis) = analyze_script(&ScriptRequest::new(&input, script_path, kind)) else {
        return Ok(RootOutcome::Unsupported(
            "script registration facts are unavailable for this template".to_owned(),
        ));
    };

    let mut reasons = Vec::new();
    if let Some(reason) = view.status_hint.as_ref().and_then(CapabilityStatus::reason) {
        reasons.push(reason.to_owned());
    }
    if !view.source_complete {
        reasons.push("the prepared source file list is incomplete".to_owned());
    }
    if dropped > 0 {
        reasons.push(format!("{dropped} script files could not be represented"));
    }
    if analysis.registration.selected.is_none() {
        let reason = analysis
            .data
            .problems
            .first()
            .or_else(|| analysis.properties.problems.first())
            .cloned()
            .unwrap_or_else(|| "no statically readable page or component registration".to_owned());
        return Ok(RootOutcome::Unsupported(reason));
    }
    if let Some(reason) = analysis.data.status.reason() {
        reasons.push(reason.to_owned());
    }
    if let Some(reason) = analysis.properties.status.reason() {
        reasons.push(reason.to_owned());
    }
    if !analysis.registration.complete {
        reasons.push("the selected registration is not a single top-level literal call".to_owned());
    }

    let mut candidates: Vec<RootCandidate> = Vec::new();
    for field in &analysis.data.fields {
        push_root_candidate(
            &mut candidates,
            &field.name,
            &field.type_text,
            &field.documentation,
        );
    }
    for property in &analysis.properties.properties {
        push_root_candidate(
            &mut candidates,
            &property.name,
            &property.type_text,
            &property.documentation,
        );
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    let truncated = candidates.len() > MAX_COMPLETION_ITEMS;
    if truncated {
        candidates.truncate(MAX_COMPLETION_ITEMS);
        reasons.push(format!(
            "root candidates are truncated at {MAX_COMPLETION_ITEMS} entries"
        ));
    }
    let hard_unsupported = matches!(
        analysis.data.root,
        DataRootKind::TypeReference | DataRootKind::Unsupported
    );
    if hard_unsupported && candidates.is_empty() {
        let reason = if reasons.is_empty() {
            "the script data root cannot be enumerated".to_owned()
        } else {
            bounded_reason(&reasons.join("; "))
        };
        return Ok(RootOutcome::Unsupported(reason));
    }
    let overlay_complete =
        analysis.data.complete && analysis.properties.complete && analysis.registration.complete;
    Ok(RootOutcome::Facts(RootFacts {
        candidates,
        reasons,
        overlay_complete,
        truncated,
    }))
}

fn push_root_candidate(
    candidates: &mut Vec<RootCandidate>,
    name: &str,
    detail: &str,
    documentation: &str,
) {
    if !is_identifier_text(name) || candidates.iter().any(|existing| existing.name == name) {
        return;
    }
    candidates.push(RootCandidate {
        name: name.to_owned(),
        detail: detail.to_owned(),
        documentation: documentation.to_owned(),
    });
}

/// Bounds a stable degradation reason for a status.
fn bounded_reason(reason: &str) -> String {
    const LIMIT: usize = 512;
    if reason.len() <= LIMIT {
        return reason.to_owned();
    }
    let mut end = LIMIT;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &reason[..end])
}

/// Maps one supported member-access prefix to its first data root.
pub(crate) fn member_prefix_root(prefix: &str) -> Option<&str> {
    first_identifier(prefix)
}
