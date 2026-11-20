//! Exact event-handler definitions and hover facts from Rust script facts.
//!
//! `bindtap="onTap"` and the other WXML event-binding forms resolve to the
//! literal method declaration in the view's owning `Page`/`Component` script,
//! including statically resolved `Behavior` methods. The method table comes
//! from [`ScriptAnalysis::methods`](wxml_backend_tsgo::script::ScriptAnalysis),
//! so this path never builds the in-place shadow, never opens a native session
//! and never sends a definition or hover query to the type backend.
//!
//! Honesty rules:
//!
//! - only a single identifier handler token resolves; mustaches, empty values,
//!   member expressions and cursors outside the token are checked empty;
//! - a missing method is a checked empty only when the selected registration
//!   is complete and the method table is `Supported`;
//! - multiple same-kind registrations, a non-top-level selection, a rejected
//!   registration call or a script that cannot be scanned stay `Partial` and
//!   return no location, because another registration may own the handler;
//! - a method that is present in a literal table keeps its exact declaration
//!   even when unrelated members made the table `Partial` (for example a
//!   spread member or an incomplete behavior graph).
//!
//! LSP `textDocument/definition` has no status field, so an unproven answer is
//! `null`, never a location from an ambiguous registration.

use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::script::registration::RegistrationKind;
use wxml_backend_tsgo::script::{ScriptAnalysis, ScriptRequest, analyze_script};
use wxml_backend_tsgo::shadow::expression::is_identifier_name;
use wxml_text::SourceRange;
use wxml_types::{CapabilityStatus, RootKind, SourceLocation};

use crate::completion::catalog::{self, EventAttributeClass};
use crate::project_host::{PreparedFile, PreparedView};

/// Maximum byte length of a static handler name, matching the template layer.
const MAX_HANDLER_NAME_BYTES: usize = 256;
/// Stable reason for a template without an owning script.
const NO_TARGET_REASON: &str = "the template has no page or component script";
/// Stable reason for an analysis that found no literal registration.
const NO_REGISTRATION_REASON: &str = "no statically readable page or component registration";
/// Stable reason for a failed registration selection.
const INCOMPLETE_REGISTRATION_REASON: &str = "the page or component registration is ambiguous";
/// Stable reason for a missing declaration file.
const MISSING_FILE_REASON: &str = "the declaring script is not part of the prepared view";
/// Stable reason for an unusable declaration range.
const INVALID_RANGE_REASON: &str = "the script declaration range is not an exact name token";
/// Stable reason for an incomplete prepared source list.
const INCOMPLETE_SOURCE_REASON: &str = "the prepared source file list is incomplete";

/// The event attribute value under the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EventValueSite {
    /// A single static handler name and its exact token.
    Handler {
        /// Handler name as written in the template.
        handler: String,
        /// Exact handler token in the WXML source.
        token: SourceRange,
    },
    /// A binding-shaped value that cannot name a static handler.
    NotStatic,
}

/// One checked event-handler definition answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventDefinitionAnswer {
    /// Exact declaration location, only when the method is proven.
    pub(crate) location: Option<SourceLocation>,
    /// Coverage of the script facts behind the answer.
    pub(crate) status: CapabilityStatus,
}

/// Script facts for one proven handler declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventHandlerFacts {
    /// Exact declaration location in the declaring script.
    pub(crate) location: SourceLocation,
    /// One-line signature already capped by the script layer.
    pub(crate) signature: String,
    /// `JSDoc` text kept by the script layer; empty when there is none.
    pub(crate) documentation: String,
}

/// One checked handler lookup shared by definition and hover.
///
/// Definition uses the location, hover uses the signature and documentation;
/// both consume the same status so the two features cannot disagree about
/// whether a handler was proven.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EventHandlerResolution {
    /// Declaration facts, only when the method is proven.
    pub(crate) facts: Option<EventHandlerFacts>,
    /// Coverage of the script facts behind the answer.
    pub(crate) status: CapabilityStatus,
}

impl EventHandlerResolution {
    /// Creates a checked empty answer.
    const fn empty(status: CapabilityStatus) -> Self {
        Self {
            facts: None,
            status,
        }
    }
}

/// Classifies the event attribute value at one byte offset.
///
/// Returns `None` when the cursor is not inside an event-binding value, so the
/// caller can keep the static and backend definition paths. A binding-shaped
/// value that cannot name a handler is an explicit [`EventValueSite::NotStatic`]
/// instead of a silent fallback.
pub(crate) fn event_site(text: &str, offset: u32) -> Option<EventValueSite> {
    let tree = wxml_syntax::parse(text).tree;
    for element in tree.document().elements() {
        for attribute in element.attributes() {
            let Some(name) = attribute.name() else {
                continue;
            };
            let class = catalog::classify_event_attribute(name);
            if class == EventAttributeClass::Other {
                continue;
            }
            let Some(value) = attribute.value() else {
                continue;
            };
            let range = value.text_range();
            if offset < range.start() || offset > range.end() {
                continue;
            }
            return Some(match class {
                EventAttributeClass::Handler => static_handler(text, value, Some(offset)),
                EventAttributeClass::Rejected(_) | EventAttributeClass::Other => {
                    EventValueSite::NotStatic
                }
            });
        }
    }
    None
}

/// Returns every static handler value of one document in source order.
///
/// This is the document-wide form of [`event_site`]: both share
/// [`static_handler`], so a missing-handler diagnostic, a hover and a
/// definition cannot disagree about which values are static handler tokens.
pub(crate) fn event_handler_sites(text: &str) -> Vec<EventValueSite> {
    let tree = wxml_syntax::parse(text).tree;
    let mut sites = Vec::new();
    for element in tree.document().elements() {
        for attribute in element.attributes() {
            let Some(name) = attribute.name() else {
                continue;
            };
            if catalog::classify_event_attribute(name) != EventAttributeClass::Handler {
                continue;
            }
            let Some(value) = attribute.value() else {
                continue;
            };
            let site = static_handler(text, value, None);
            if matches!(site, EventValueSite::Handler { .. }) {
                sites.push(site);
            }
        }
    }
    sites
}

/// Returns the exact handler token for one event value, or `NotStatic`.
///
/// `offset` restricts the answer to a cursor inside the handler token; the
/// document-wide scan passes `None` because it is not answering a point query.
fn static_handler(
    text: &str,
    value: wxml_syntax::AttributeValue<'_>,
    offset: Option<u32>,
) -> EventValueSite {
    if value.mustaches().next().is_some() {
        return EventValueSite::NotStatic;
    }
    let Some(literal) = value.literal_text() else {
        return EventValueSite::NotStatic;
    };
    let handler = literal.trim();
    if handler.is_empty() || handler.len() > MAX_HANDLER_NAME_BYTES || !is_identifier_name(handler)
    {
        return EventValueSite::NotStatic;
    }
    let Some(range) = handler_range(value, literal, handler, offset) else {
        return EventValueSite::NotStatic;
    };
    if text.get(range.start() as usize..range.end() as usize) != Some(handler) {
        return EventValueSite::NotStatic;
    }
    EventValueSite::Handler {
        handler: handler.to_owned(),
        token: range,
    }
}

/// Returns the trimmed handler token range inside `value`.
fn handler_range(
    value: wxml_syntax::AttributeValue<'_>,
    literal: &str,
    handler: &str,
    offset: Option<u32>,
) -> Option<SourceRange> {
    let range = value.text_range();
    let quote = u32::from(value.is_quoted());
    let leading = literal.len().saturating_sub(literal.trim_start().len());
    let start = range
        .start()
        .checked_add(quote)?
        .checked_add(u32::try_from(leading).ok()?)?;
    let end = start.checked_add(u32::try_from(handler.len()).ok()?)?;
    if end > range.end() {
        return None;
    }
    if offset.is_some_and(|offset| offset < start || offset > end) {
        return None;
    }
    SourceRange::from_bounds(start, end).ok()
}

/// Resolves one static handler to its exact declaration in `view`.
pub(crate) fn resolve(view: &PreparedView, handler: &str) -> EventDefinitionAnswer {
    let resolution = resolve_handler(view, handler);
    EventDefinitionAnswer {
        location: resolution.facts.map(|facts| facts.location),
        status: resolution.status,
    }
}

/// Resolves one static handler to the declaration facts used by hover.
///
/// Every honesty rule lives here so `textDocument/definition` and
/// `textDocument/hover` cannot disagree: an ambiguous registration, a missing
/// method, an opaque value or a declaration without an exact name token yields
/// no facts at all.
pub(crate) fn resolve_handler(view: &PreparedView, handler: &str) -> EventHandlerResolution {
    let Some(target) = view.target.as_ref() else {
        return EventHandlerResolution::empty(unsupported(NO_TARGET_REASON));
    };
    let analysis = match analyze(view, target.script.as_str(), target.owner) {
        Ok(analysis) => analysis,
        Err(status) => return EventHandlerResolution::empty(status),
    };
    if analysis.analysis.registration.selected.is_none() {
        return EventHandlerResolution::empty(unsupported(NO_REGISTRATION_REASON));
    }
    if !analysis.analysis.registration.complete {
        return EventHandlerResolution::empty(partial(INCOMPLETE_REGISTRATION_REASON));
    }
    let Some(method) = analysis
        .analysis
        .methods
        .methods
        .iter()
        .find(|method| method.name == handler)
    else {
        return EventHandlerResolution::empty(missing_status(&analysis, handler));
    };
    let Some(file) = analysis
        .view
        .files
        .iter()
        .find(|file| file.path.as_str() == method.path)
    else {
        return EventHandlerResolution::empty(partial(MISSING_FILE_REASON));
    };
    let Some((start, end)) = name_token(
        file,
        method.range.utf8.start.get(),
        method.range.utf8.end.get(),
        handler,
    ) else {
        return EventHandlerResolution::empty(partial(INVALID_RANGE_REASON));
    };
    let Some(range) = SourceRange::from_bounds(start, end).ok() else {
        return EventHandlerResolution::empty(partial(INVALID_RANGE_REASON));
    };
    let status = if analysis.dropped > 0 || !view.source_complete {
        partial(INCOMPLETE_SOURCE_REASON)
    } else {
        analysis.analysis.methods.status.clone()
    };
    EventHandlerResolution {
        facts: Some(EventHandlerFacts {
            location: SourceLocation {
                file: file.file.clone(),
                range,
            },
            signature: method.signature.clone(),
            documentation: method.documentation.clone(),
        }),
        status,
    }
}

/// One analyzed view plus the files that could not be represented.
pub(crate) struct ViewAnalysis<'a> {
    /// Prepared view the analysis was built from.
    pub(crate) view: &'a PreparedView,
    /// Script facts for the owning registration.
    pub(crate) analysis: ScriptAnalysis,
    /// Prepared files that could not be represented.
    pub(crate) dropped: usize,
}

/// Builds the event method table from the prepared script files.
pub(crate) fn analyze<'a>(
    view: &'a PreparedView,
    script_path: &str,
    owner: RootKind,
) -> Result<ViewAnalysis<'a>, CapabilityStatus> {
    if !view
        .files
        .iter()
        .any(|file| file.path.as_str() == script_path)
    {
        return Err(partial(MISSING_FILE_REASON));
    }
    let Some(root) = view
        .project_root
        .to_str()
        .and_then(|root| ProjectRoot::new(root).ok())
    else {
        return Err(partial("the project root cannot be represented"));
    };
    let mut files = Vec::with_capacity(view.files.len());
    let mut dropped = 0_usize;
    for prepared in view.files.iter() {
        match source_file(prepared) {
            Ok(file) => files.push(file),
            Err(()) if prepared.path.as_str() == script_path => {
                return Err(partial("the owning script cannot be represented"));
            }
            Err(()) => dropped = dropped.saturating_add(1),
        }
    }
    let input = AnalysisInput::new(
        ProjectKey::new("wxml-lsp-event-definition"),
        root,
        SnapshotRevision::new(1),
        files,
    )
    .map_err(|_| partial("the script snapshot cannot be represented"))?;
    let kind = match owner {
        RootKind::Page => RegistrationKind::Page,
        RootKind::Component => RegistrationKind::Component,
    };
    let Ok(analysis) = analyze_script(&ScriptRequest::new(&input, script_path, kind)) else {
        return Err(unsupported("the owning script could not be analyzed"));
    };
    Ok(ViewAnalysis {
        view,
        analysis,
        dropped,
    })
}

/// Copies one prepared file into an analysis source file.
fn source_file(prepared: &PreparedFile) -> Result<SourceFile, ()> {
    SourceFile::new(
        prepared.path.as_str(),
        prepared.text.to_string(),
        DocumentRevision::new(prepared.source_revision),
    )
    .map_err(|_| ())
}

/// Returns the exact name token, shrinking one pair of string quotes.
fn name_token(file: &PreparedFile, start: u32, end: u32, handler: &str) -> Option<(u32, u32)> {
    let slice = file.text.get(start as usize..end as usize)?;
    if slice == handler {
        return Some((start, end));
    }
    let quote = slice.chars().next()?;
    if !matches!(quote, '"' | '\'') || !slice.ends_with(quote) {
        return None;
    }
    let width = u32::try_from(quote.len_utf8()).ok()?;
    let inner = slice.get(quote.len_utf8()..slice.len().checked_sub(quote.len_utf8())?)?;
    if inner != handler {
        return None;
    }
    Some((start.checked_add(width)?, end.checked_sub(width)?))
}

/// Returns the status for a method that is not in the table.
fn missing_status(analysis: &ViewAnalysis<'_>, handler: &str) -> CapabilityStatus {
    if analysis
        .analysis
        .methods
        .opaque
        .iter()
        .any(|name| name == handler)
    {
        return partial("the handler is not statically known to be a function");
    }
    if analysis.dropped > 0 || !analysis.view.source_complete {
        return partial(INCOMPLETE_SOURCE_REASON);
    }
    if analysis.analysis.methods.status.is_supported() {
        return CapabilityStatus::Supported;
    }
    partial(table_reason(&analysis.analysis))
}

/// Returns the most specific bounded reason for an incomplete table.
fn table_reason(analysis: &ScriptAnalysis) -> String {
    analysis
        .methods
        .problems
        .first()
        .cloned()
        .or_else(|| analysis.methods.status.reason().map(str::to_owned))
        .or_else(|| analysis.reasons.first().cloned())
        .unwrap_or_else(|| INCOMPLETE_REGISTRATION_REASON.to_owned())
}

fn partial(reason: impl Into<String>) -> CapabilityStatus {
    CapabilityStatus::Partial {
        reason: reason.into(),
    }
}

fn unsupported(reason: impl Into<String>) -> CapabilityStatus {
    CapabilityStatus::Unsupported {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::panic,
        clippy::unwrap_used,
        clippy::indexing_slicing
    )]

    use std::path::PathBuf;
    use std::sync::Arc;

    use wxml_project::ProjectPath;
    use wxml_types::{FileId, ProjectId, ViewRevision};

    use super::*;
    use crate::project_host::ViewTarget;

    const SCRIPT: &str = "pages/index/index.ts";
    const WXML: &str = "pages/index/index.wxml";

    fn project_id() -> ProjectId {
        ProjectId::new("demo")
    }

    fn file_id(path: &str) -> FileId {
        FileId::new(project_id(), path).expect("file id")
    }

    fn project_path(path: &str) -> ProjectPath {
        ProjectPath::new(path).expect("project path")
    }

    fn prepared(path: &str, text: &str) -> PreparedFile {
        PreparedFile {
            file: file_id(path),
            path: project_path(path),
            kind: "ts",
            text: Arc::from(text),
            source_revision: 1,
        }
    }

    fn view(owner: RootKind, files: Vec<PreparedFile>) -> PreparedView {
        view_with_target(
            Some(ViewTarget {
                wxml: project_path(WXML),
                owner,
                script: project_path(SCRIPT),
                script_file: file_id(SCRIPT),
                view_revision: ViewRevision::new(1),
            }),
            files,
        )
    }

    fn view_with_target(target: Option<ViewTarget>, files: Vec<PreparedFile>) -> PreparedView {
        PreparedView {
            view_id: format!("{WXML}:1"),
            target,
            files: Arc::from(files),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(Vec::new()),
            components: Arc::from(Vec::new()),
            typing: None,
            events: Arc::from(Vec::new()),
            project_root: PathBuf::from("/work/demo"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        }
    }

    fn cursor(template: &str) -> (String, u32) {
        let offset = template.find('|').expect("cursor marker");
        (
            template.replacen('|', "", 1),
            u32::try_from(offset).expect("small offset"),
        )
    }

    fn site(template: &str) -> EventValueSite {
        let (text, offset) = cursor(template);
        event_site(&text, offset).expect("event value site")
    }

    fn answer(owner: RootKind, files: &[(&str, &str)], template: &str) -> EventDefinitionAnswer {
        let site = site(template);
        let EventValueSite::Handler { handler, .. } = site else {
            panic!("expected a static handler site");
        };
        let files = files
            .iter()
            .map(|(path, text)| prepared(path, text))
            .collect();
        resolve(&view(owner, files), &handler)
    }

    fn assert_location(answer: &EventDefinitionAnswer, path: &str, script: &str, name: &str) {
        let location = answer.location.as_ref().expect("definition location");
        assert_source_location(location, path, script, name);
    }

    fn assert_source_location(location: &SourceLocation, path: &str, script: &str, name: &str) {
        assert_eq!(location.file, file_id(path));
        let start = u32::try_from(script.find(name).expect("name in script")).expect("start");
        assert_eq!(
            (location.range.start(), location.range.end()),
            (
                start,
                start + u32::try_from(name.len()).expect("name length")
            )
        );
        assert_eq!(&script[start as usize..location.range.end() as usize], name);
    }

    /// Resolves the handler under the marker and keeps the template text and
    /// its exact WXML token so hover tests can assert both sides.
    fn hover_facts(
        owner: RootKind,
        files: &[(&str, &str)],
        template: &str,
    ) -> (EventHandlerResolution, String, SourceRange) {
        let (text, offset) = cursor(template);
        let site = event_site(&text, offset).expect("event value site");
        let EventValueSite::Handler { handler, token } = site else {
            panic!("expected a static handler site: {template}");
        };
        let files = files
            .iter()
            .map(|(path, text)| prepared(path, text))
            .collect();
        (resolve_handler(&view(owner, files), &handler), text, token)
    }

    fn facts(resolution: &EventHandlerResolution) -> &EventHandlerFacts {
        resolution
            .facts
            .as_ref()
            .unwrap_or_else(|| panic!("expected handler facts: {resolution:?}"))
    }

    #[test]
    fn every_event_binding_form_resolves_to_the_exact_page_method() {
        let script = "Page({\n  onTap() {},\n});";
        for attribute in [
            "bindtap",
            "catchtap",
            "bind:tap",
            "catch:tap",
            "capture-bind:tap",
            "capture-catch:tap",
            "mut-bind:tap",
            "bind:my-event",
        ] {
            let template = format!("<view {attribute}=\"onTa|p\" />");
            let answer = answer(RootKind::Page, &[(SCRIPT, script)], &template);
            assert!(answer.status.is_supported(), "{attribute}: {answer:?}");
            assert_location(&answer, SCRIPT, script, "onTap");
        }
    }

    #[test]
    fn an_unquoted_event_value_resolves() {
        let script = "Page({ onTap() {} });";
        let answer = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=on|Tap />",
        );
        assert_location(&answer, SCRIPT, script, "onTap");
    }

    #[test]
    fn component_methods_and_behavior_methods_resolve() {
        let script = "Component({\n  behaviors: [Behavior({ methods: { onBack() {} } })],\n  methods: {\n    onTap() {},\n  },\n});";
        for (handler, offset_marker) in [("onTa|p", "onTap"), ("onBa|ck", "onBack")] {
            let template = format!("<view bindtap=\"{handler}\" />");
            let answer = answer(RootKind::Component, &[(SCRIPT, script)], &template);
            assert_location(&answer, SCRIPT, script, offset_marker);
        }
    }

    #[test]
    fn an_unreferenced_behavior_method_is_not_merged() {
        let script =
            "Behavior({ methods: { orphan() {} } });\nComponent({ methods: { onTap() {} } });";
        let answer = answer(
            RootKind::Component,
            &[(SCRIPT, script)],
            "<view bindtap=\"orp|han\" />",
        );
        assert!(answer.location.is_none(), "{answer:?}");
        assert!(answer.status.is_supported(), "{answer:?}");
    }

    #[test]
    fn imported_behavior_declaration_points_at_the_behavior_file() {
        let owner = "import base from \"./base\";\nComponent({ behaviors: [base], methods: {} });";
        let behavior = "export default Behavior({\n  methods: {\n    onBack() {},\n  },\n});";
        let answer = answer(
            RootKind::Component,
            &[(SCRIPT, owner), ("pages/index/base.ts", behavior)],
            "<view bindtap=\"onBa|ck\" />",
        );
        assert_location(&answer, "pages/index/base.ts", behavior, "onBack");
    }

    #[test]
    fn a_page_template_falls_back_to_a_component_registration() {
        let script = "Component({ methods: { onTap() {} } });";
        let answer = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"onTa|p\" />",
        );
        assert_location(&answer, SCRIPT, script, "onTap");
    }

    #[test]
    fn a_static_registrar_alias_is_followed() {
        let script = "const page = Page;\npage({\n  onAlias() {},\n});";
        let answer = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"onAli|as\" />",
        );
        assert_location(&answer, SCRIPT, script, "onAlias");
    }

    #[test]
    fn literal_function_and_arrow_values_are_static_methods() {
        let script = "Page({\n  onShorthand() {},\n  onFunction: function () {},\n  onArrow: () => {},\n  onAsync: async () => {},\n});";
        for handler in ["onShorthand", "onFunction", "onArrow", "onAsync"] {
            let marked = handler.replacen("on", "on|", 1);
            let template = format!("<view bindtap=\"{marked}\" />");
            let answer = answer(RootKind::Page, &[(SCRIPT, script)], &template);
            assert_location(&answer, SCRIPT, script, handler);
        }
    }

    #[test]
    fn a_unique_top_level_registration_wins_over_a_nested_one() {
        let script = "function factory() { Page({ nested() {} }); }\nPage({ top() {} });";
        let top = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"to|p\" />",
        );
        assert_location(&top, SCRIPT, script, "top");
        let nested = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"nest|ed\" />",
        );
        assert!(nested.location.is_none(), "{nested:?}");
        assert!(nested.status.is_supported(), "{nested:?}");
    }

    #[test]
    fn a_nested_registration_alone_stays_partial_and_empty() {
        let script = "function factory() { Page({ nested() {} }); }";
        let answer = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"nest|ed\" />",
        );
        assert!(answer.location.is_none(), "{answer:?}");
        assert!(
            matches!(answer.status, CapabilityStatus::Partial { .. }),
            "{answer:?}"
        );
    }

    #[test]
    fn multiple_top_level_registrations_stay_partial_and_empty() {
        let script = "Page({ onFirst() {} });\nPage({ onSecond() {} });";
        let answer = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"onFir|st\" />",
        );
        assert!(answer.location.is_none(), "{answer:?}");
        assert!(
            matches!(answer.status, CapabilityStatus::Partial { .. }),
            "{answer:?}"
        );
    }

    #[test]
    fn a_missing_method_is_a_checked_empty_only_for_a_complete_table() {
        let script = "Page({ onTap() {} });";
        let missing = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"onMis|sing\" />",
        );
        assert!(missing.location.is_none(), "{missing:?}");
        assert!(missing.status.is_supported(), "{missing:?}");

        let opaque = "Page({ onTap: handlerRef });";
        let opaque_answer = answer(
            RootKind::Page,
            &[(SCRIPT, opaque)],
            "<view bindtap=\"onTa|p\" />",
        );
        assert!(opaque_answer.location.is_none(), "{opaque_answer:?}");
        assert!(
            matches!(opaque_answer.status, CapabilityStatus::Partial { .. }),
            "{opaque_answer:?}"
        );
    }

    #[test]
    fn dynamic_empty_and_member_values_are_not_static() {
        for template in [
            "<view bindtap=\"{{ dynamic|Handler }}\" />",
            "<view bindtap=\"|\" />",
            "<view bindtap=\"this.on|Tap\" />",
            "<view bindtap=\"onTap\"| />",
            "<view bindtap=\"onTap |\" />",
            "<view bindex=\"on|Tap\" />",
            "<view bind:=\"on|Tap\" />",
        ] {
            assert_eq!(site(template), EventValueSite::NotStatic, "{template}");
        }
        // A compact bind with a camel event name is a real handler, not a member value.
        assert!(matches!(
            site("<view bindTap=\"on|Tap\" />"),
            EventValueSite::Handler { .. }
        ));
        assert_eq!(event_site("<view class=\"on|Tap\" />", 12), None);
    }

    #[test]
    fn the_document_wide_scan_matches_the_point_query_tokens() {
        let text = concat!(
            "🙂<view bindtap=\"onTap\" />\n",
            "<view catch:tap=\"保存\" />\n",
            "<view bindtap=\"{{dynamic}}\" />\n",
            "<view bindtap=\"\" />\n",
            "<view bindtap=\"this.onTap\" />\n",
            "<view bindtap=\"onTap()\" />\n",
            "<view class=\"onTap\" />",
        );
        let sites = event_handler_sites(text);
        assert_eq!(sites.len(), 2, "{sites:?}");
        for (site, (needle, token)) in sites.iter().zip([("onTap", "onTap"), ("保存", "保存")])
        {
            let EventValueSite::Handler {
                handler,
                token: range,
            } = site
            else {
                panic!("expected a static handler site: {site:?}");
            };
            let start = u32::try_from(text.find(needle).expect("token")).expect("start");
            assert_eq!(handler, token);
            assert_eq!(
                (range.start(), range.end()),
                (start, start + u32::try_from(needle.len()).expect("len"))
            );
        }
    }

    #[test]
    fn a_handler_token_resolves_with_cjk_and_emoji_prefixes() {
        let script = "// 🙂 页面\nPage({\n  保存() {},\n});";
        let template = "🙂<view>中文</view><view bind:tap=\"保|存\" />";
        let (text, offset) = cursor(template);
        let site = event_site(&text, offset).expect("event site");
        let EventValueSite::Handler { handler, token } = site else {
            panic!("expected a static handler site");
        };
        assert_eq!(handler, "保存");
        let expected = u32::try_from(text.find("保存").expect("handler")).expect("offset");
        assert_eq!((token.start(), token.end()), (expected, expected + 6));
        let answer = resolve(
            &view(RootKind::Page, vec![prepared(SCRIPT, script)]),
            &handler,
        );
        assert_location(&answer, SCRIPT, script, "保存");
    }

    #[test]
    fn a_template_without_an_owning_script_is_empty() {
        let files = vec![prepared(SCRIPT, "Page({ onTap() {} });")];
        let view = view_with_target(None, files);
        let answer = resolve(&view, "onTap");
        assert!(answer.location.is_none(), "{answer:?}");
        assert!(
            matches!(answer.status, CapabilityStatus::Unsupported { .. }),
            "{answer:?}"
        );
    }

    #[test]
    fn an_owning_script_missing_from_the_view_is_empty() {
        let view = view(RootKind::Page, Vec::new());
        let answer = resolve(&view, "onTap");
        assert!(answer.location.is_none(), "{answer:?}");
        assert!(
            matches!(answer.status, CapabilityStatus::Partial { .. }),
            "{answer:?}"
        );
    }

    #[test]
    fn a_quoted_method_key_keeps_the_exact_name_token() {
        let script = "Page({ \"onTap\"() {} });";
        let answer = answer(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"onTa|p\" />",
        );
        assert_location(&answer, SCRIPT, script, "onTap");
        let location = answer.location.as_ref().expect("location");
        assert_eq!(
            &script[location.range.start() as usize..location.range.end() as usize],
            "onTap"
        );
    }

    #[test]
    fn every_event_binding_form_hovers_the_exact_handler_token() {
        let script = "Page({\n  /** Taps the card. */\n  onTap(event: { detail: { x: number } }) { void event; },\n});";
        for attribute in [
            "bindtap",
            "catchtap",
            "bind:tap",
            "catch:tap",
            "capture-bind:tap",
            "capture-catch:tap",
            "mut-bind:tap",
            "bind:my-event",
        ] {
            let template = format!("<view {attribute}=\"onTa|p\" />");
            let (resolution, text, token) =
                hover_facts(RootKind::Page, &[(SCRIPT, script)], &template);
            assert!(
                resolution.status.is_supported(),
                "{attribute}: {resolution:?}"
            );
            let facts = facts(&resolution);
            assert_eq!(&text[token.start() as usize..token.end() as usize], "onTap");
            assert!(facts.signature.contains("onTap"), "{attribute}: {facts:?}");
            assert!(
                facts.documentation.contains("Taps the card."),
                "{attribute}: {facts:?}"
            );
            assert_source_location(&facts.location, SCRIPT, script, "onTap");
        }

        let template = "<view bindtap=on|Tap />";
        let (resolution, text, token) = hover_facts(RootKind::Page, &[(SCRIPT, script)], template);
        assert!(resolution.status.is_supported(), "{resolution:?}");
        assert_eq!(&text[token.start() as usize..token.end() as usize], "onTap");
        assert!(facts(&resolution).signature.contains("onTap"));
    }

    #[test]
    fn shorthand_function_and_arrow_handlers_expose_hover_facts() {
        let script = "Page({\n  /** Shorthand. */\n  onShorthandTap() {},\n  onFunctionTap: function (event: unknown) { void event; },\n  onArrowTap: (event: unknown) => { void event; },\n  onAsyncTap: async () => {},\n});";
        for (marked, name) in [
            ("onShort|handTap", "onShorthandTap"),
            ("onFunc|tionTap", "onFunctionTap"),
            ("onAr|rowTap", "onArrowTap"),
            ("onAs|yncTap", "onAsyncTap"),
        ] {
            let template = format!("<view bindtap=\"{marked}\" />");
            let (resolution, _, _) = hover_facts(RootKind::Page, &[(SCRIPT, script)], &template);
            let facts = facts(&resolution);
            assert!(facts.signature.contains(name), "{name}: {facts:?}");
        }
    }

    #[test]
    fn component_behavior_alias_and_top_level_handlers_expose_hover_facts() {
        let component = "Component({\n  behaviors: [Behavior({ methods: { onBack() {} } })],\n  methods: { onTap() {} },\n});";
        for (marked, name) in [("onTa|p", "onTap"), ("onBa|ck", "onBack")] {
            let template = format!("<view bindtap=\"{marked}\" />");
            let (resolution, _, _) =
                hover_facts(RootKind::Component, &[(SCRIPT, component)], &template);
            assert_source_location(&facts(&resolution).location, SCRIPT, component, name);
        }

        let owner = "import base from \"./base\";\nComponent({ behaviors: [base], methods: {} });";
        let behavior = "export default Behavior({ methods: { onBack() {} } });";
        let (resolution, _, _) = hover_facts(
            RootKind::Component,
            &[(SCRIPT, owner), ("pages/index/base.ts", behavior)],
            "<view bindtap=\"onBa|ck\" />",
        );
        assert_source_location(
            &facts(&resolution).location,
            "pages/index/base.ts",
            behavior,
            "onBack",
        );

        let alias = "const page = Page;\npage({\n  onAlias() {},\n});";
        let (resolution, _, _) = hover_facts(
            RootKind::Page,
            &[(SCRIPT, alias)],
            "<view bindtap=\"onAli|as\" />",
        );
        assert_source_location(&facts(&resolution).location, SCRIPT, alias, "onAlias");

        // S1: a unique top-level registration wins over a nested one.
        let top_level = "function factory() { Page({ nested() {} }); }\nPage({ top() {} });";
        let (top, _, _) = hover_facts(
            RootKind::Page,
            &[(SCRIPT, top_level)],
            "<view bindtap=\"to|p\" />",
        );
        assert!(facts(&top).signature.contains("top"), "{top:?}");
        let (nested, _, _) = hover_facts(
            RootKind::Page,
            &[(SCRIPT, top_level)],
            "<view bindtap=\"nest|ed\" />",
        );
        assert!(nested.facts.is_none(), "{nested:?}");
        assert!(nested.status.is_supported(), "{nested:?}");
    }

    #[test]
    fn unproven_handlers_never_produce_hover_facts() {
        let missing_script = "Page({ onTap() {} });";
        let (missing, _, _) = hover_facts(
            RootKind::Page,
            &[(SCRIPT, missing_script)],
            "<view bindtap=\"onMis|sing\" />",
        );
        assert!(missing.facts.is_none(), "{missing:?}");
        assert!(missing.status.is_supported(), "{missing:?}");

        for (label, owner, script, template) in [
            (
                "ambiguous",
                RootKind::Page,
                "Page({ onFirst() {} });\nPage({ onSecond() {} });",
                "<view bindtap=\"onFir|st\" />",
            ),
            (
                "opaque",
                RootKind::Page,
                "Page({ onTap: handlerRef });",
                "<view bindtap=\"onTa|p\" />",
            ),
            (
                "spread",
                RootKind::Page,
                "const base = {};\nPage({ ...base });",
                "<view bindtap=\"onMis|sing\" />",
            ),
            (
                "nested",
                RootKind::Page,
                "function factory() { Page({ nested() {} }); }",
                "<view bindtap=\"nest|ed\" />",
            ),
            (
                "non-literal methods",
                RootKind::Component,
                "Component({ methods: getMethods() });",
                "<view bindtap=\"onTa|p\" />",
            ),
        ] {
            let (resolution, _, _) = hover_facts(owner, &[(SCRIPT, script)], template);
            assert!(resolution.facts.is_none(), "{label}: {resolution:?}");
            assert!(
                matches!(resolution.status, CapabilityStatus::Partial { .. }),
                "{label}: {resolution:?}"
            );
        }
    }

    #[test]
    fn a_literal_method_survives_an_incomplete_table() {
        let script = "const base = {};\nPage({ ...base, onTap() {} });";
        let (resolution, _, _) = hover_facts(
            RootKind::Page,
            &[(SCRIPT, script)],
            "<view bindtap=\"onTa|p\" />",
        );
        assert!(
            matches!(resolution.status, CapabilityStatus::Partial { .. }),
            "{resolution:?}"
        );
        assert!(facts(&resolution).signature.contains("onTap"));
    }

    #[test]
    fn hover_facts_keep_cjk_docs_and_exact_tokens() {
        let script = "// 🙂 页面\nPage({\n  /** 保存当前页面 😀 */\n  保存(event: { detail: { 值: string } }) { void event; },\n});";
        let (resolution, text, token) = hover_facts(
            RootKind::Page,
            &[(SCRIPT, script)],
            "🙂<view bind:tap=\"保|存\" />",
        );
        let facts = facts(&resolution);
        assert_eq!(&text[token.start() as usize..token.end() as usize], "保存");
        assert_eq!(token.end() - token.start(), 6);
        assert!(facts.signature.contains("保存"), "{facts:?}");
        assert!(facts.documentation.contains("保存当前页面 😀"), "{facts:?}");
        assert_eq!(facts.location.file, file_id(SCRIPT));
        // The doc comment also mentions `保存`; the definition must be the
        // declaration token, not the first occurrence in the file.
        let start = u32::try_from(script.rfind("保存").expect("declaration")).expect("start");
        assert_eq!(
            (facts.location.range.start(), facts.location.range.end()),
            (start, start + 6)
        );
    }

    #[test]
    fn mustache_and_member_values_have_no_handler_hover_site() {
        for template in [
            "<view bindtap=\"{{ dynamic|Handler }}\" />",
            "<view bindtap=\"this.on|Tap\" />",
            "<view bindtap=\"item.on|Tap\" />",
        ] {
            let (text, offset) = cursor(template);
            assert_eq!(
                event_site(&text, offset),
                Some(EventValueSite::NotStatic),
                "{template}"
            );
        }
    }
}
