//! Missing event-handler diagnostics from Rust script facts.
//!
//! `bindtap="onMissing"` (and every other static event-binding form) is
//! checked against the method table of the view's owning `Page`/`Component`
//! script, including statically resolved `Behavior` methods. The table comes
//! from [`analyze_script`](wxml_backend_tsgo::script::analyze_script), so this
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

use wxml_backend_tsgo::script::events::EventBinding;
use wxml_types::{CapabilityStatus, Diagnostic, DiagnosticSeverity};

use crate::project_host::PreparedView;
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

/// Checks every static handler binding of `view` against its script table.
///
/// `text` must be the template text the prepared view was built from; the
/// handler token is re-validated against it so a stale or synthetic document
/// can never produce a diagnostic at a range it does not own.
pub(crate) fn diagnostics(view: &PreparedView, text: &str) -> EventDiagnostics {
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
    let bindings: Vec<EventBinding> = view
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let range = event.handler_range;
            let handler = sites.get(&(range.start(), range.end()))?;
            if handler != &event.handler {
                return None;
            }
            Some(EventBinding::new(
                format!("e{index}"),
                event.attribute.clone(),
                event.event.clone(),
                event.handler.clone(),
                wxml_backend_tsgo::shadow::mapping::SourceRange::new(range.start(), range.end()),
            ))
        })
        .collect();
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
    for result in &collection.results {
        note_status(&mut status, &result.status);
        let Some(diagnostic) = result.diagnostic.as_ref() else {
            continue;
        };
        // A dropped behavior/script file may own the missing method; an
        // incomplete source list can never prove a handler missing.
        if analysis.dropped > 0 || !analysis.view.source_complete {
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

/// Returns the coverage status for a truncated event batch.
fn collection_skipped_status(
    collection: &wxml_backend_tsgo::script::events::EventCollection,
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
        events: Vec<wxml_types::EventHandler>,
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
            components: Arc::from(Vec::new()),
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
        )
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
        let result = diagnostics(&prepared, template);
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
        let result = diagnostics(&prepared, template);
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
        let result = diagnostics(&prepared, template);
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
        let result = diagnostics(&prepared, template);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        assert_eq!(result.status, None);
    }
}
