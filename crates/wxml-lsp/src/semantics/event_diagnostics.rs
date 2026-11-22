//! Missing event-handler diagnostics from Rust script facts.
//!
//! `bindtap="onMissing"` (and every other static event-binding form) is
//! checked against the method table of the view's owning `Page`/`Component`
//! script, including statically resolved `Behavior` methods. The table comes
//! from [`analyze_script`](wxml_backend_tsrs::script::analyze_script), so this
//! path never builds an in-place shadow, never opens a native session and never
//! asks the type backend whether a handler exists.
//!
//! Honesty rules (identical to the TypeScript oracle):
//!
//! - a handler missing from a complete, unambiguous table is an error at the
//!   exact WXML handler token;
//! - mustaches, empty values, member expressions (`this.onTap`) and other
//!   non-identifier values are skipped, not reported;
//! - a name that exists with a non-literal value (`onTap: handlerRef`) is
//!   `Partial` but never an error;
//! - spread/computed members, several same-kind registrations, a non-top-level
//!   selection, a rejected registration call or an incomplete behavior graph
//!   make the table incomplete: no missing-handler error is produced, only a
//!   `Partial` coverage status;
//! - a handler that exists is never reported, even when unrelated members made
//!   the table `Partial`.
//!
//! The LSP publishes these diagnostics next to the backend result. Node owns
//! the same diagnostics when it is the active backend, so the server only runs
//! this check when the backend does not claim a complete diagnostics
//! capability.

use wxml_backend_tsrs::script::events::EventBinding;
use wxml_project::typing::TypingComponent;
use wxml_types::{CapabilityStatus, Diagnostic, DiagnosticSeverity, EventHandler};

use crate::project_host::{PreparedView, ViewComponent};
use crate::semantics::event_definition::{EventValueSite, analyze, event_handler_sites};

/// Stable reason for a prepared source list that hid a behavior/script file.
const INCOMPLETE_SOURCE_REASON: &str = "the prepared script source list is incomplete";
/// Stable reason for a truncated event batch.
const TRUNCATED_EVENTS_REASON: &str = "the event binding list is truncated";

/// One local event-diagnostics result.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EventDiagnostics {
    /// Missing-handler errors in source order.
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Coverage to merge into the published diagnostics status.
    ///
    /// `None` means the checked bindings were all proven, so this check does
    /// not degrade the backend status.
    pub(crate) status: Option<CapabilityStatus>,
}

impl EventDiagnostics {
    /// Creates an empty result that does not change the published status.
    pub(crate) const fn empty() -> Self {
        Self {
            diagnostics: Vec::new(),
            status: None,
        }
    }
}

/// Checks every static handler binding of `view` against its script table and
/// its declared component events.
///
/// `text` must be the template text the prepared view was built from; the
/// handler token is re-validated against it so a stale or synthetic document
/// can never produce a diagnostic at a range it does not own.
///
/// Declared event facts come from `components`; a tag without a declaration
/// (`typing == None`) keeps exactly the pre-declaration diagnostics, which is
/// what makes a project without `components.d.ts` behave unchanged. Handler
/// parameter compatibility is not decided here: the engine contract probe
/// publishes it from the declaration's real types.
#[allow(
    clippy::too_many_lines,
    reason = "one linear token scan plus one per-binding declared-event pass"
)]
pub(crate) fn diagnostics(
    view: &PreparedView,
    text: &str,
    components: &[ViewComponent],
) -> EventDiagnostics {
    let Some(target) = view.target.as_ref() else {
        return EventDiagnostics::empty();
    };
    if view.events.is_empty() {
        return EventDiagnostics::empty();
    }
    let analysis = match analyze(view, target.script.as_str(), target.owner) {
        Ok(analysis) => analysis,
        Err(status) => {
            return EventDiagnostics {
                diagnostics: Vec::new(),
                status: (!status.is_supported()).then_some(status),
            };
        }
    };

    // Only the shared token scan can name a static handler. Events whose
    // analyzer range has no matching static site (a mixed/dynamic value that
    // changed shape) are ignored instead of guessed at.
    let mut sites = std::collections::BTreeMap::new();
    for site in event_handler_sites(text) {
        if let EventValueSite::Handler { handler, token } = site {
            sites.insert((token.start(), token.end()), handler);
        }
    }
    let mut bindings = Vec::new();
    let mut origins = Vec::new();
    for (index, event) in view.events.iter().enumerate() {
        let range = event.handler_range;
        let Some(handler) = sites.get(&(range.start(), range.end())) else {
            continue;
        };
        if handler != &event.handler {
            continue;
        }
        bindings.push(EventBinding::new(
            format!("e{index}"),
            event.attribute.clone(),
            event.event.clone(),
            event.handler.clone(),
            wxml_backend_tsrs::shadow::mapping::SourceRange::new(range.start(), range.end()),
        ));
        origins.push(index);
    }
    if bindings.is_empty() {
        return EventDiagnostics::empty();
    }

    let collection = analysis.analysis.resolve_events(&bindings);
    let mut status = None;
    note_status(&mut status, &collection_skipped_status(&collection));
    if analysis.dropped > 0 || !analysis.view.source_complete {
        note_status(
            &mut status,
            &CapabilityStatus::Partial {
                reason: INCOMPLETE_SOURCE_REASON.to_owned(),
            },
        );
    }

    let mut diagnostics = Vec::new();
    for ((binding, index), result) in bindings
        .iter()
        .zip(origins.iter())
        .zip(collection.results.iter())
    {
        note_status(&mut status, &result.status);
        let source = view.events.get(*index);
        // A dropped behavior/script file may own the missing method; an
        // incomplete source list can never prove a handler missing.
        let proven_missing = analysis.dropped == 0 && analysis.view.source_complete;
        if let Some(diagnostic) = result.diagnostic.as_ref() {
            if !proven_missing {
                continue;
            }
            let Ok(range) = wxml_text::SourceRange::from_bounds(
                diagnostic.range.start.get(),
                diagnostic.range.end.get(),
            ) else {
                continue;
            };
            diagnostics.push(Diagnostic {
                range,
                severity: DiagnosticSeverity::Error,
                message: diagnostic.message.clone(),
                code: None,
            });
            // A missing handler is the only error this check layers on a
            // binding; parameter compatibility is published from the engine
            // contract probe, not compared here.
            continue;
        }
        let (Some((tag, typing)), Some(source)) = (
            source.and_then(|event| typing_of_event(text, event, components)),
            source,
        ) else {
            continue;
        };
        if let Some(diagnostic) = unknown_event_diagnostic(text, binding, source, tag, typing) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics.sort_by(|left, right| {
        (left.range.start(), left.range.end(), &left.message).cmp(&(
            right.range.start(),
            right.range.end(),
            &right.message,
        ))
    });
    diagnostics.dedup_by(|left, right| left.range == right.range && left.message == right.message);
    EventDiagnostics {
        diagnostics,
        status,
    }
}

/// Returns the diagnostic for one binding whose event is not declared.
///
/// Only an undeclared event name produces a diagnostic here (`Warning` on the
/// event token). Handler parameter compatibility is decided by the engine
/// contract probe and a missing handler by the method-table result above, so a
/// single binding never carries two messages from this check.
///
/// The lookup uses [`TypingComponent::event_named`], so a kebab binding and a
/// camel declaration (in either direction) are the same event. The raw binding
/// text is kept for the diagnostic message and the token range.
fn unknown_event_diagnostic(
    text: &str,
    binding: &EventBinding,
    source: &EventHandler,
    tag: &str,
    typing: &TypingComponent,
) -> Option<Diagnostic> {
    if !typing.complete || typing.events.is_empty() {
        return None;
    }
    if typing.event_named(&binding.event).is_some() {
        return None;
    }
    Some(Diagnostic {
        range: event_name_range(text, source, &binding.event)?,
        severity: DiagnosticSeverity::Warning,
        message: unknown_event_message(&binding.event, tag),
        code: None,
    })
}

/// Returns the declared component facts that own one event binding.
///
/// The owning tag is read from the template text at the binding's attribute
/// range; only a tag with `ViewComponent.typing` exposes declared events, so a
/// built-in element or an undeclared custom component returns `None`.
fn typing_of_event<'a>(
    text: &'a str,
    event: &EventHandler,
    components: &'a [ViewComponent],
) -> Option<(&'a str, &'a TypingComponent)> {
    let tag = owning_tag(text, event.attribute_range.start())?;
    let component = components.iter().find(|component| component.name == tag)?;
    Some((tag, component.typing.as_deref()?))
}

/// Returns the start-tag name that contains `offset`.
///
/// Scanning backwards from the attribute token avoids any shadow mapping: the
/// answer is the literal `<tag` that opens the element, and a `>` before the
/// nearest `<` means the offset is not inside a start tag.
fn owning_tag(text: &str, offset: u32) -> Option<&str> {
    let offset = usize::try_from(offset).ok()?;
    if offset > text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut index = offset;
    while index > 0 {
        index -= 1;
        match bytes.get(index) {
            Some(b'<') => {
                let mut end = index + 1;
                while end < bytes.len()
                    && bytes.get(end).is_some_and(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
                    })
                {
                    end += 1;
                }
                if end == index + 1 {
                    return None;
                }
                return text.get(index + 1..end);
            }
            Some(b'>') => return None,
            _ => {}
        }
    }
    None
}

/// Returns the exact event-name token range inside one attribute name.
fn event_name_range(
    text: &str,
    event: &EventHandler,
    name: &str,
) -> Option<wxml_text::SourceRange> {
    let attribute = event.attribute.as_str();
    let prefix_len = attribute.len().checked_sub(name.len())?;
    if !attribute.ends_with(name) {
        return None;
    }
    let start = event
        .attribute_range
        .start()
        .checked_add(u32::try_from(prefix_len).ok()?)?;
    let end = start.checked_add(u32::try_from(name.len()).ok()?)?;
    if end > event.attribute_range.end() {
        return None;
    }
    let slice = text.get(usize::try_from(start).ok()?..usize::try_from(end).ok()?)?;
    if slice != name {
        return None;
    }
    wxml_text::SourceRange::from_bounds(start, end).ok()
}

/// Stable message for one undeclared event name.
fn unknown_event_message(event: &str, tag: &str) -> String {
    format!("Event \"{event}\" is not declared by component \"{tag}\"")
}

/// Returns the coverage status for a truncated event batch.
fn collection_skipped_status(
    collection: &wxml_backend_tsrs::script::events::EventCollection,
) -> CapabilityStatus {
    if collection.skipped == 0 {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Partial {
            reason: TRUNCATED_EVENTS_REASON.to_owned(),
        }
    }
}

/// Keeps the first degraded status; a later `Partial` never hides an earlier
/// more specific reason.
fn note_status(status: &mut Option<CapabilityStatus>, candidate: &CapabilityStatus) {
    if candidate.is_supported() {
        return;
    }
    if status.as_ref().is_none_or(CapabilityStatus::is_supported) {
        *status = Some(candidate.clone());
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
    use wxml_types::{
        DiagnosticSeverity, FileId, ProjectId, RootKind, SourceRevision, ViewRevision,
    };

    use super::*;
    use crate::project_host::{PreparedFile, ViewTarget};

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

    fn view(owner: RootKind, files: Vec<PreparedFile>, template: &str) -> PreparedView {
        let parsed = wxml_syntax::parse(template);
        let analysis =
            wxml_types::TemplateAnalyzer::new(owner, SourceRevision::new(1)).analyze(&parsed.tree);
        prepared_view(owner, files, analysis.events)
    }

    fn prepared_view(
        owner: RootKind,
        files: Vec<PreparedFile>,
        events: Vec<EventHandler>,
    ) -> PreparedView {
        PreparedView {
            view_id: format!("{WXML}:1"),
            target: Some(ViewTarget {
                wxml: project_path(WXML),
                owner,
                script: project_path(SCRIPT),
                script_file: file_id(SCRIPT),
                view_revision: ViewRevision::new(1),
            }),
            files: Arc::from(files),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(Vec::new()),
            wxs_modules: Arc::from(Vec::new()),
            components: Arc::from(Vec::new()),
            typing: None,
            events: Arc::from(events),
            project_root: PathBuf::from("/work/demo"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        }
    }

    fn check(owner: RootKind, script: &str, template: &str) -> EventDiagnostics {
        diagnostics(
            &view(owner, vec![prepared(SCRIPT, script)], template),
            template,
            &[],
        )
    }

    /// Builds one declared component used by the event-contract tests.
    fn component(
        name: &str,
        events: Vec<wxml_project::typing::TypingEvent>,
        complete: bool,
    ) -> ViewComponent {
        ViewComponent {
            name: name.to_owned(),
            specifier: format!("/components/{name}"),
            kind: wxml_project::ComponentTargetKind::Local,
            component: None,
            wxml: None,
            json: None,
            property_facts: wxml_project::ComponentPropertyFacts {
                properties: Vec::new(),
                complete: true,
                reason: None,
                source: wxml_project::PropertySource::Script,
            },
            typing: Some(Arc::new(TypingComponent {
                props: Vec::new(),
                events,
                complete,
                source: project_path("components.d.ts"),
            })),
            origin: crate::project_host::ComponentOrigin::Local { file: None },
        }
    }

    /// Builds one declared event.
    fn event(name: &str, detail: Option<&str>, proven: bool) -> wxml_project::typing::TypingEvent {
        wxml_project::typing::TypingEvent {
            name: name.to_owned(),
            detail_text: detail.map(str::to_owned),
            detail: detail.map(wxml_project::typing::TypeText::parse),
            proven,
            documentation: None,
        }
    }

    fn check_with_components(
        owner: RootKind,
        script: &str,
        template: &str,
        components: &[ViewComponent],
    ) -> EventDiagnostics {
        diagnostics(
            &view(owner, vec![prepared(SCRIPT, script)], template),
            template,
            components,
        )
    }

    /// Asserts one diagnostic at the exact `needle` token.
    fn assert_at(
        result: &EventDiagnostics,
        template: &str,
        needle: &str,
        severity: DiagnosticSeverity,
        message: &str,
    ) {
        let (start, end) = token(template, needle);
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range.start() == start && diagnostic.range.end() == end)
            .unwrap_or_else(|| panic!("no diagnostic at {start}..{end}: {result:?}"));
        assert_eq!(diagnostic.severity, severity);
        assert_eq!(diagnostic.code, None);
        assert_eq!(diagnostic.message, message);
        assert_eq!(
            &template[start as usize..end as usize],
            needle,
            "range must cover the token"
        );
    }

    fn token(template: &str, needle: &str) -> (u32, u32) {
        let start = template.find(needle).expect("handler token");
        (
            u32::try_from(start).expect("offset"),
            u32::try_from(start + needle.len()).expect("end"),
        )
    }

    fn assert_missing(result: &EventDiagnostics, template: &str, needle: &str, message: &str) {
        let (start, end) = token(template, needle);
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.range.start() == start && diagnostic.range.end() == end)
            .unwrap_or_else(|| panic!("no diagnostic at {start}..{end}: {result:?}"));
        assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostic.code, None);
        assert_eq!(diagnostic.message, message);
    }

    #[test]
    fn a_missing_page_handler_is_an_error_at_the_exact_token() {
        let script = "Page({\n  onTap() {},\n});";
        let template = "<view bindtap=\" onMissing \" />";
        let result = check(RootKind::Page, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "onMissing",
            "Event handler \"onMissing\" for \"tap\" is not defined on this page",
        );
        assert_eq!(result.status, None);
    }

    #[test]
    fn an_unquoted_missing_handler_is_reported() {
        let script = "Page({ onTap() {} });";
        let template = "<view bindtap=onMissing />";
        let result = check(RootKind::Page, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "onMissing",
            "Event handler \"onMissing\" for \"tap\" is not defined on this page",
        );
        assert_eq!(result.status, None);
    }

    #[test]
    fn every_binding_form_reports_the_same_missing_handler() {
        let script = "Page({ onTap() {} });";
        let forms = [
            "bindtap",
            "catchtap",
            "bind:tap",
            "catch:tap",
            "capture-bind:tap",
            "capture-catch:tap",
            "mut-bind:tap",
            "bind:my-event",
        ];
        let template = forms.iter().fold(String::new(), |mut template, form| {
            template.push_str("<view ");
            template.push_str(form);
            template.push_str("=\"onMissing\" />");
            template
        });
        let result = check(RootKind::Page, script, &template);
        assert_eq!(
            result.diagnostics.len(),
            forms.len(),
            "one error per binding: {result:?}"
        );
        for diagnostic in &result.diagnostics {
            assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
            assert!(
                diagnostic.message.contains("\"onMissing\""),
                "{diagnostic:?}"
            );
        }
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("\"my-event\"")),
            "{result:?}"
        );
        assert_eq!(result.status, None);
    }

    #[test]
    fn present_handlers_in_every_literal_form_are_quiet() {
        let script = "Page({\n  onShorthand() {},\n  onFunction: function () {},\n  onArrow: () => {},\n  onAsync: async () => {},\n});";
        let template = concat!(
            "<view bindtap=\"onShorthand\" />",
            "<view catchtap=\"onFunction\" />",
            "<view capture-bind:tap=\"onArrow\" />",
            "<view mut-bind:tap=\"onAsync\" />",
        );
        let result = check(RootKind::Page, script, template);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert_eq!(result.status, None);
    }

    #[test]
    fn component_behavior_and_alias_handlers_are_known() {
        let script = concat!(
            "const registrar = Component;\n",
            "registrar({\n",
            "  behaviors: [Behavior({ methods: { onBack() {} } })],\n",
            "  methods: { onTap() {} },\n",
            "});",
        );
        let template = "<view bindtap=\"onTap\" /><view bind:tap=\"onBack\" />";
        let result = check(RootKind::Component, script, template);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert_eq!(result.status, None);
    }

    #[test]
    fn imported_behavior_methods_are_known_and_other_names_are_missing() {
        let owner = "import base from \"./base\";\nComponent({ behaviors: [base], methods: {} });";
        let behavior = "export default Behavior({ methods: { onBack() {} } });";
        let template = "<view bind:bounce=\"onBack\" />\n<view bindtap=\"onGone\" />";
        let result = diagnostics(
            &view(
                RootKind::Component,
                vec![
                    prepared(SCRIPT, owner),
                    prepared("pages/index/base.ts", behavior),
                ],
                template,
            ),
            template,
            &[],
        );
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "onGone",
            "Event handler \"onGone\" for \"tap\" is not defined on this component",
        );
    }

    #[test]
    fn a_unique_top_level_registration_wins_over_a_nested_one() {
        let script = "function factory() { Page({ nested() {} }); }\nPage({ top() {} });";
        let template = "<view bindtap=\"top\" />\n<view bindtap=\"nested\" />";
        let result = check(RootKind::Page, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "nested",
            "Event handler \"nested\" for \"tap\" is not defined on this page",
        );
        assert_eq!(result.status, None);
    }

    #[test]
    fn a_page_template_falls_back_to_a_component_registration() {
        let script = "Component({ methods: { onTap() {} } });";
        let template = "<view bindtap=\"onTap\" />\n<view bindtap=\"onMissing\" />";
        let result = check(RootKind::Page, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "onMissing",
            "Event handler \"onMissing\" for \"tap\" is not defined on this component",
        );
    }

    #[test]
    fn a_component_without_methods_reports_a_real_missing_handler() {
        let script = "Component({ properties: {} });";
        let template = "<view bindtap=\"onMissing\" />";
        let result = check(RootKind::Component, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "onMissing",
            "Event handler \"onMissing\" for \"tap\" is not defined on this component",
        );
    }

    #[test]
    fn an_unresolved_behavior_never_proves_a_handler_missing() {
        let script =
            "import missing from \"./missing\";\nComponent({ behaviors: [missing], methods: {} });";
        let template = "<view bindtap=\"onLost\" />";
        let result = check(RootKind::Component, script, template);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(
            matches!(result.status, Some(CapabilityStatus::Partial { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn spread_and_computed_members_keep_missing_handlers_unproven() {
        for script in [
            "const base = {};\nPage({ ...base });",
            "const key = \"onTap\";\nPage({ [key]: function () {} });",
        ] {
            let template = "<view bindtap=\"onMissing\" />";
            let result = check(RootKind::Page, script, template);
            assert!(result.diagnostics.is_empty(), "{script}: {result:?}");
            assert!(
                matches!(result.status, Some(CapabilityStatus::Partial { .. })),
                "{script}: {result:?}"
            );
        }
    }

    #[test]
    fn an_unrelated_opaque_member_does_not_hide_a_missing_handler() {
        // `onTap` is known to exist with a non-literal value, but it cannot be
        // `onMissing`; the node oracle still reports the other name missing.
        let script = "Page({ onTap: handlerRef });";
        let template = "<view bindtap=\"onMissing\" />";
        let result = check(RootKind::Page, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "onMissing",
            "Event handler \"onMissing\" for \"tap\" is not defined on this page",
        );
        assert_eq!(result.status, None);
    }

    #[test]
    fn an_opaque_named_handler_is_not_reported() {
        let script = "Page({ onTap: handlerRef });";
        let template = "<view bindtap=\"onTap\" />";
        let result = check(RootKind::Page, script, template);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(
            matches!(result.status, Some(CapabilityStatus::Partial { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn ambiguous_or_nested_registrations_never_report_missing() {
        for script in [
            "Page({ onFirst() {} });\nPage({ onSecond() {} });",
            "function factory() { Page({ nested() {} }); }",
        ] {
            let template = "<view bindtap=\"onMissing\" />";
            let result = check(RootKind::Page, script, template);
            assert!(result.diagnostics.is_empty(), "{script}: {result:?}");
            assert!(
                matches!(result.status, Some(CapabilityStatus::Partial { .. })),
                "{script}: {result:?}"
            );
        }
    }

    #[test]
    fn a_non_literal_registration_never_reports_missing() {
        let script = "Page(getOptions());";
        let template = "<view bindtap=\"onMissing\" />";
        let result = check(RootKind::Page, script, template);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(
            !result
                .status
                .as_ref()
                .is_some_and(CapabilityStatus::is_supported)
        );
    }

    #[test]
    fn dynamic_empty_member_and_mustache_values_are_ignored() {
        let script = "Page({ onTap() {} });";
        for template in [
            "<view bindtap=\"{{ dynamic }}\" />",
            "<view bindtap=\"\" />",
            "<view bindtap=\"   \" />",
            "<view bindtap=\"this.onTap\" />",
            "<view bindtap=\"item.onTap\" />",
            "<view bindtap=\"onTap()\" />",
            "<view bindtap=\"1onTap\" />",
        ] {
            let result = check(RootKind::Page, script, template);
            assert!(result.diagnostics.is_empty(), "{template}: {result:?}");
            assert_eq!(result.status, None, "{template}: {result:?}");
        }
    }

    #[test]
    fn missing_handlers_get_utf8_exact_ranges_with_cjk_and_emoji() {
        let script = "// 🙂 页面\nPage({\n  保存() {},\n});";
        let template = "🙂<view bind:tap=\"保存\" />\n<view catch:tap=\"丢失\" />";
        let result = check(RootKind::Page, script, template);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_missing(
            &result,
            template,
            "丢失",
            "Event handler \"丢失\" for \"tap\" is not defined on this page",
        );
        let diagnostic = &result.diagnostics[0];
        assert_eq!(
            &template[diagnostic.range.start() as usize..diagnostic.range.end() as usize],
            "丢失"
        );
    }

    #[test]
    fn one_diagnostic_per_binding_is_sorted_and_deduplicated() {
        let script = "Page({ onTap() {} });";
        let template = "<view bindtap=\"onMissing\" catchtap=\"onMissing\" />";
        let once = check(RootKind::Page, script, template);
        assert_eq!(once.diagnostics.len(), 2, "{once:?}");
        let mut sorted = once.diagnostics.clone();
        sorted.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
        assert_eq!(once.diagnostics, sorted);
    }

    #[test]
    fn an_incomplete_source_list_keeps_missing_handlers_unproven() {
        let script = "Page({ onTap() {} });";
        let template = "<view bindtap=\"onMissing\" />";
        let mut prepared = view(RootKind::Page, vec![prepared(SCRIPT, script)], template);
        prepared.source_complete = false;
        let result = diagnostics(&prepared, template, &[]);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(
            matches!(result.status, Some(CapabilityStatus::Partial { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn a_missing_owning_script_is_quiet() {
        let template = "<view bindtap=\"onMissing\" />";
        let prepared = view(RootKind::Page, Vec::new(), template);
        let result = diagnostics(&prepared, template, &[]);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert!(result.status.is_some(), "{result:?}");
    }

    #[test]
    fn a_view_without_a_target_is_quiet() {
        let template = "<view bindtap=\"onMissing\" />";
        let mut prepared = view(
            RootKind::Page,
            vec![prepared(SCRIPT, "Page({ onTap() {} });")],
            template,
        );
        prepared.target = None;
        let result = diagnostics(&prepared, template, &[]);
        assert_eq!(result, EventDiagnostics::empty());
    }

    #[test]
    fn an_unrelated_stale_event_range_is_not_reported() {
        // The analyzer event range does not match any static handler site in
        // this text; a stale range must never become a diagnostic.
        let script = "Page({ onTap() {} });";
        let with_events = "<view bindtap=\"onMissing\" />";
        let analysis = wxml_types::TemplateAnalyzer::new(RootKind::Page, SourceRevision::new(1))
            .analyze(&wxml_syntax::parse(with_events).tree);
        let template = "<view />";
        let prepared = prepared_view(
            RootKind::Page,
            vec![prepared(SCRIPT, script)],
            analysis.events,
        );
        let result = diagnostics(&prepared, template, &[]);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert_eq!(result.status, None);
    }

    // ---- declared component events (task 5.4) ----

    #[test]
    fn an_undeclared_event_name_is_a_warning_at_the_event_token() {
        let script = "Page({ onTap() {} });";
        let template = "<van-field bind:chaneg=\"onTap\" />";
        let components = vec![component(
            "van-field",
            vec![event("change", Some("{ index: number }"), true)],
            true,
        )];
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_at(
            &result,
            template,
            "chaneg",
            DiagnosticSeverity::Warning,
            "Event \"chaneg\" is not declared by component \"van-field\"",
        );
    }

    #[test]
    fn a_declared_event_name_is_quiet() {
        let script = "Page({ onTap() {} });";
        let template = "<van-field bind:change=\"onTap\" />";
        let components = vec![component(
            "van-field",
            vec![event("change", None, true)],
            true,
        )];
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert!(result.diagnostics.is_empty(), "{result:?}");
    }

    #[test]
    fn an_incomplete_declaration_never_warns_about_event_names() {
        let script = "Page({ onTap() {} });";
        let template = "<van-field bind:chaneg=\"onTap\" />";
        let components = vec![component(
            "van-field",
            vec![event("change", None, true)],
            false,
        )];
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert!(result.diagnostics.is_empty(), "{result:?}");
    }

    #[test]
    fn a_component_without_events_keeps_the_previous_behaviour() {
        let script = "Page({ onTap() {} });";
        let template = "<van-field bind:chaneg=\"onTap\" />";
        let components = vec![component("van-field", Vec::new(), true)];
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert!(result.diagnostics.is_empty(), "{result:?}");
    }

    #[test]
    fn a_dynamic_event_binding_is_never_warned() {
        let script = "Page({ onTap() {} });";
        for template in [
            "<van-field bind:change=\"{{ name }}\" />",
            "<van-field bind:chaneg=\"{{ name }}\" />",
        ] {
            let components = vec![component(
                "van-field",
                vec![event("change", None, true)],
                true,
            )];
            let result = check_with_components(RootKind::Page, script, template, &components);
            assert!(result.diagnostics.is_empty(), "{template}: {result:?}");
        }
    }

    #[test]
    fn a_zero_argument_handler_accepts_any_detail() {
        let components = vec![component(
            "van-field",
            vec![event("change", Some("{ index: number }"), true)],
            true,
        )];
        let template = "<van-field bind:change=\"onChange\" />";
        for script in [
            "Page({ onChange() {} });",
            "Page({ onChange: function () {} });",
            "Page({ onChange: () => {} });",
        ] {
            let result = check_with_components(RootKind::Page, script, template, &components);
            assert!(result.diagnostics.is_empty(), "{script}: {result:?}");
        }
    }

    #[test]
    fn a_handler_parameter_mismatch_is_left_to_the_engine_probe() {
        // The engine contract probe publishes parameter compatibility; this
        // Rust-side check must not repeat it with a hand-written comparison.
        let components = vec![component(
            "van-field",
            vec![event("change", Some("{ index: number }"), true)],
            true,
        )];
        let template = "<van-field bind:change=\"onChange\" />";
        let script =
            "Page({ onChange(event: WechatMiniprogram.CustomEvent<{ name: string }>) {} });";
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert!(result.diagnostics.is_empty(), "{result:?}");
    }

    #[test]
    fn an_unannotated_handler_parameter_is_quiet() {
        let components = vec![component(
            "van-field",
            vec![event("change", Some("{ index: number }"), true)],
            true,
        )];
        let template = "<van-field bind:change=\"onChange\" />";
        for script in [
            "Page({ onChange(event) {} });",
            "Page({ onChange: function (event) {} });",
            "Page({ onChange: (event) => {} });",
        ] {
            let result = check_with_components(RootKind::Page, script, template, &components);
            assert!(result.diagnostics.is_empty(), "{script}: {result:?}");
        }
    }

    #[test]
    fn an_unproven_detail_never_reports_a_mismatch() {
        let components = vec![component(
            "van-field",
            vec![event("change", Some("SomeLib.Options"), false)],
            true,
        )];
        let template = "<van-field bind:change=\"onChange\" />";
        let script =
            "Page({ onChange(event: WechatMiniprogram.CustomEvent<{ index: number }>) {} });";
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert!(result.diagnostics.is_empty(), "{result:?}");
    }

    #[test]
    fn a_missing_handler_reports_only_the_missing_error() {
        let script = "Page({ onTap() {} });";
        let template = "<van-field bind:change=\"onMissing\" />";
        let components = vec![component(
            "van-field",
            vec![event("change", Some("{ index: number }"), true)],
            true,
        )];
        let result = check_with_components(RootKind::Page, script, template, &components);
        assert_eq!(result.diagnostics.len(), 1, "{result:?}");
        assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Error);
        assert!(
            result.diagnostics[0].message.contains("is not defined"),
            "{result:?}"
        );
    }

    #[test]
    fn no_declaration_keeps_the_previous_diagnostics() {
        let script = "Page({ onTap() {} });";
        let template = "<van-field bind:chaneg=\"onTap\" />";
        let with_empty = check_with_components(RootKind::Page, script, template, &[]);
        let with_undeclared = check_with_components(
            RootKind::Page,
            script,
            template,
            &[ViewComponent {
                typing: None,
                ..component("van-field", Vec::new(), true)
            }],
        );
        assert!(with_empty.diagnostics.is_empty(), "{with_empty:?}");
        assert_eq!(with_empty.diagnostics, with_undeclared.diagnostics);
        assert_eq!(with_empty.status, with_undeclared.status);
    }

    // ---- kebab/camel event-name equivalence (change 6.1) ----

    /// Builds the sole static event fact of one template.
    fn template_event(template: &str) -> EventHandler {
        let analysis = wxml_types::TemplateAnalyzer::new(RootKind::Page, SourceRevision::new(1))
            .analyze(&wxml_syntax::parse(template).tree);
        let mut events = analysis.events;
        assert_eq!(events.len(), 1, "{template}");
        events.pop().expect("one event")
    }

    /// Asserts one spelling of a declared event produces no contract diagnostic.
    fn assert_declared_spelling(declared: &[&str], attribute: &str) {
        let template = format!("<card {attribute}=\"onSome\" />");
        let source = template_event(&template);
        let view_component = component(
            "card",
            declared
                .iter()
                .map(|name| event(name, None, true))
                .collect(),
            true,
        );
        let typing = view_component.typing.as_deref().expect("typing");
        let binding = EventBinding::new(
            "e0",
            source.attribute.clone(),
            source.event.clone(),
            source.handler.clone(),
            wxml_backend_tsrs::shadow::mapping::SourceRange::new(
                source.handler_range.start(),
                source.handler_range.end(),
            ),
        );
        let diagnostic = unknown_event_diagnostic(&template, &binding, &source, "card", typing);
        assert!(
            diagnostic.is_none(),
            "{declared:?} {attribute}: {diagnostic:?}"
        );
    }

    #[test]
    fn every_binding_spelling_resolves_a_camel_and_kebab_declaration() {
        for declared in [&["someEvent"][..], &["some-event"][..]] {
            for family in ["bind", "catch", "capture-bind", "capture-catch", "mut-bind"] {
                for (separator, name) in [
                    ("", "someEvent"),
                    (":", "someEvent"),
                    ("", "some-event"),
                    (":", "some-event"),
                ] {
                    assert_declared_spelling(declared, &format!("{family}{separator}{name}"));
                }
            }
        }
    }

    #[test]
    fn a_kebab_binding_is_quiet_for_a_camel_declaration() {
        // Full path: every compact/colon and kebab/camel spelling of the same
        // declared camel event must be checked and stay quiet.
        let script = "Page({ onSome() {} });";
        let components = vec![component(
            "card",
            vec![event("someEvent", None, true)],
            true,
        )];
        for template in [
            "<card bindsome-event=\"onSome\" />",
            "<card bind:some-event=\"onSome\" />",
            "<card bindsomeEvent=\"onSome\" />",
            "<card bind:someEvent=\"onSome\" />",
        ] {
            let result = check_with_components(RootKind::Page, script, template, &components);
            assert!(result.diagnostics.is_empty(), "{template}: {result:?}");
        }
    }

    #[test]
    fn a_camel_binding_is_quiet_for_a_kebab_declaration() {
        let script = "Page({ onSome() {} });";
        let components = vec![component(
            "card",
            vec![event("some-event", None, true)],
            true,
        )];
        for template in [
            "<card bind:someEvent=\"onSome\" />",
            "<card catch:someEvent=\"onSome\" />",
        ] {
            let result = check_with_components(RootKind::Page, script, template, &components);
            assert!(result.diagnostics.is_empty(), "{template}: {result:?}");
        }
    }

    #[test]
    fn an_undeclared_kebab_spelling_still_warns_on_the_event_token() {
        let script = "Page({ onSome() {} });";
        let components = vec![component(
            "card",
            vec![event("someEvent", None, true)],
            true,
        )];
        for (template, needle) in [
            ("<card bind:someevnt=\"onSome\" />", "someevnt"),
            ("<card bindsomeevnt=\"onSome\" />", "someevnt"),
        ] {
            let result = check_with_components(RootKind::Page, script, template, &components);
            assert_eq!(result.diagnostics.len(), 1, "{template}: {result:?}");
            assert_at(
                &result,
                template,
                needle,
                DiagnosticSeverity::Warning,
                "Event \"someevnt\" is not declared by component \"card\"",
            );
        }
    }
}
