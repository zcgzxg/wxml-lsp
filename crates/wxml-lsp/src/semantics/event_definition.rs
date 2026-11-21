//! Static event-handler facts from Rust script facts.
//!
//! `bindtap="onTap"` and the other WXML event-binding forms are matched
//! against the literal method table of the view's owning `Page`/`Component`
//! script, including statically resolved `Behavior` methods. The table comes
//! from [`ScriptAnalysis::methods`](wxml_backend_tsrs::script::ScriptAnalysis),
//! so this module never builds the in-place shadow, never opens a native
//! session and never queries the type backend.
//!
//! This module only produces static analysis facts for two consumers:
//!
//! - [`event_handler_sites`] gives the missing-handler diagnostics the exact
//!   handler tokens of a document, so a dynamic value can never be reported;
//! - [`analyze`] builds the method table used by those diagnostics and by
//!   handler-name completion.
//!
//! Event-handler hover and definition are no longer answered here: the native
//! backend answers them from the in-place shadow, so there is a single source
//! of types, `JSDoc` and declaration positions.

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsrs::script::registration::RegistrationKind;
use wxml_backend_tsrs::script::{ScriptAnalysis, ScriptRequest, analyze_script};
use wxml_backend_tsrs::shadow::expression::is_identifier_name;
use wxml_text::SourceRange;
use wxml_types::{CapabilityStatus, RootKind};

use crate::completion::catalog::{self, EventAttributeClass};
use crate::project_host::{PreparedFile, PreparedView};
use crate::semantics::tsconfig_paths::TsconfigPaths;

/// Stable reason for a missing declaration file.
const MISSING_FILE_REASON: &str = "the declaring script is not part of the prepared view";

/// One event attribute value that either names a static handler or does not.
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

/// Returns every static handler value of one document in source order.
///
/// All consumers share [`static_handler`], so a missing-handler diagnostic and
/// handler completion cannot disagree about which values are static handler
/// tokens. Dynamic values (mustaches, empty values, member expressions) never
/// appear in the result.
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
            let site = static_handler(text, value);
            if matches!(site, EventValueSite::Handler { .. }) {
                sites.push(site);
            }
        }
    }
    sites
}

/// Returns the exact handler token for one event value, or `NotStatic`.
fn static_handler(text: &str, value: wxml_syntax::AttributeValue<'_>) -> EventValueSite {
    if value.mustaches().next().is_some() {
        return EventValueSite::NotStatic;
    }
    let Some(literal) = value.literal_text() else {
        return EventValueSite::NotStatic;
    };
    let handler = literal.trim();
    if handler.is_empty() || !is_identifier_name(handler) {
        return EventValueSite::NotStatic;
    }
    let Some(range) = handler_range(value, literal, handler) else {
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
    SourceRange::from_bounds(start, end).ok()
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
    let resolver = TsconfigPaths::from_view(view);
    let request = ScriptRequest::new(&input, script_path, kind);
    let request = resolver
        .as_ref()
        .map_or(request, |resolver| request.with_resolver(resolver));
    let Ok(analysis) = analyze_script(&request) else {
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

    use wxml_backend_tsrs::script::ModuleResolver;
    use wxml_backend_tsrs::script::events::EventMethod;
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
            events: Arc::from(Vec::new()),
            project_root: PathBuf::from("/work/demo"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        }
    }

    fn with_compiler_options(mut view: PreparedView, options: serde_json::Value) -> PreparedView {
        let serde_json::Value::Object(options) = options else {
            panic!("compiler options must be an object");
        };
        view.compiler_options = options;
        view
    }

    /// Returns `(handler, token)` for every static handler of one document.
    fn handler_sites(template: &str) -> Vec<(String, SourceRange)> {
        event_handler_sites(template)
            .into_iter()
            .map(|site| match site {
                EventValueSite::Handler { handler, token } => (handler, token),
                EventValueSite::NotStatic => {
                    panic!("the document scan must never return a dynamic value")
                }
            })
            .collect()
    }

    /// Returns the table entry for `name`, failing when it was not merged.
    fn method<'a>(analysis: &'a ViewAnalysis<'_>, name: &str) -> &'a EventMethod {
        analysis
            .analysis
            .methods
            .methods
            .iter()
            .find(|method| method.name == name)
            .unwrap_or_else(|| panic!("expected a {name} method in the table"))
    }

    /// Asserts the table entry carries the exact declaration name token.
    fn assert_method(method: &EventMethod, path: &str, script: &str, name: &str) {
        assert_eq!(method.path, path);
        let start = u32::try_from(script.find(name).expect("name in script")).expect("start");
        let end = start + u32::try_from(name.len()).expect("name length");
        assert_eq!(
            (method.range.utf8.start.get(), method.range.utf8.end.get()),
            (start, end)
        );
        assert_eq!(&script[start as usize..end as usize], name);
    }

    #[test]
    fn every_event_binding_form_is_a_document_wide_handler_site() {
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
            let template = format!("<view {attribute}=\"onTap\" />");
            let sites = handler_sites(&template);
            assert_eq!(sites.len(), 1, "{attribute}: {sites:?}");
            assert_eq!(sites[0].0, "onTap", "{attribute}");
        }
        // An unquoted value is still an exact handler token.
        assert_eq!(handler_sites("<view bindtap=onTap />").len(), 1);
        // A compact camel event name is a real handler, not a member value.
        assert_eq!(handler_sites("<view bindTap=\"onTap\" />").len(), 1);
    }

    #[test]
    fn dynamic_empty_and_member_values_are_never_handler_sites() {
        for template in [
            "<view bindtap=\"{{ dynamicHandler }}\" />",
            "<view bindtap=\"\" />",
            "<view bindtap=\"this.onTap\" />",
            "<view bindtap=\"item.onTap\" />",
            "<view bindtap=\"onTap()\" />",
            "<view bindtap=\"1onTap\" />",
            "<view bindex=\"onTap\" />",
            "<view bind:=\"onTap\" />",
            "<view class=\"onTap\" />",
        ] {
            assert!(handler_sites(template).is_empty(), "{template}");
        }
    }

    #[test]
    fn the_document_wide_scan_finds_static_tokens_in_source_order() {
        let text = concat!(
            "🙂<view bindtap=\"onTap\" />\n",
            "<view catch:tap=\"保存\" />\n",
            "<view bindtap=\"{{dynamic}}\" />\n",
            "<view bindtap=\"\" />\n",
            "<view bindtap=\"this.onTap\" />\n",
            "<view bindtap=\"onTap()\" />\n",
            "<view class=\"onTap\" />",
        );
        let sites = handler_sites(text);
        assert_eq!(sites.len(), 2, "{sites:?}");
        for ((handler, token), (needle, expected)) in
            sites.iter().zip([("onTap", "onTap"), ("保存", "保存")])
        {
            let start = u32::try_from(text.find(needle).expect("token")).expect("start");
            assert_eq!(handler, expected);
            assert_eq!(
                (token.start(), token.end()),
                (start, start + u32::try_from(needle.len()).expect("len"))
            );
        }
    }

    #[test]
    fn a_handler_token_keeps_utf8_bounds_with_cjk_and_emoji_prefixes() {
        let text = "🙂<view>中文</view><view bind:tap=\"保存\" />";
        let sites = handler_sites(text);
        assert_eq!(sites.len(), 1, "{sites:?}");
        assert_eq!(sites[0].0, "保存");
        let expected = u32::try_from(text.find("保存").expect("handler")).expect("offset");
        assert_eq!(
            (sites[0].1.start(), sites[0].1.end()),
            (expected, expected + 6)
        );
    }

    #[test]
    fn component_and_behavior_methods_are_in_the_table() {
        let script = "Component({\n  behaviors: [Behavior({ methods: { onBack() {} } })],\n  methods: {\n    onTap() {},\n  },\n});";
        let prepared_view = view(RootKind::Component, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Component).expect("analysis");
        assert_method(method(&analysis, "onTap"), SCRIPT, script, "onTap");
        assert_method(method(&analysis, "onBack"), SCRIPT, script, "onBack");
        assert!(analysis.analysis.methods.status.is_supported());
    }

    #[test]
    fn an_unreferenced_behavior_method_is_not_merged() {
        let script =
            "Behavior({ methods: { orphan() {} } });\nComponent({ methods: { onTap() {} } });";
        let prepared_view = view(RootKind::Component, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Component).expect("analysis");
        assert_method(method(&analysis, "onTap"), SCRIPT, script, "onTap");
        assert!(
            !analysis
                .analysis
                .methods
                .methods
                .iter()
                .any(|method| method.name == "orphan"),
            "an unrelated behavior must not contribute methods"
        );
    }

    #[test]
    fn imported_behavior_declaration_lands_in_the_behavior_file() {
        let owner = "import base from \"./base\";\nComponent({ behaviors: [base], methods: {} });";
        let behavior = "export default Behavior({\n  methods: {\n    onBack() {},\n  },\n});";
        let prepared_view = view(
            RootKind::Component,
            vec![
                prepared(SCRIPT, owner),
                prepared("pages/index/base.ts", behavior),
            ],
        );
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Component).expect("analysis");
        assert_method(
            method(&analysis, "onBack"),
            "pages/index/base.ts",
            behavior,
            "onBack",
        );
    }

    #[test]
    fn a_tsconfig_alias_behavior_lands_in_the_behavior_file() {
        let owner = "import base from \"@/behaviors/base\";\nComponent({ behaviors: [base], methods: {} });";
        let behavior = "export default Behavior({\n  methods: {\n    onBack() {},\n  },\n});";
        let prepared_view = with_compiler_options(
            view(
                RootKind::Component,
                vec![
                    prepared(SCRIPT, owner),
                    prepared("src/behaviors/base.ts", behavior),
                ],
            ),
            serde_json::json!({
                "baseUrl": "/work/demo",
                "paths": { "@/*": ["./src/*"] },
            }),
        );
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Component).expect("analysis");
        assert_method(
            method(&analysis, "onBack"),
            "src/behaviors/base.ts",
            behavior,
            "onBack",
        );
    }

    #[test]
    fn alias_mappings_prefer_the_longest_prefix_and_probe_extensions() {
        let owner = "Component({ methods: {} });";
        let prepared_view = with_compiler_options(
            view(
                RootKind::Component,
                vec![
                    prepared(SCRIPT, owner),
                    prepared("src/behaviors/base.ts", "export default Behavior({});"),
                    prepared("behaviors/base.ts", "export default Behavior({});"),
                    prepared("behaviors/dir/base.ts", "export default Behavior({});"),
                ],
            ),
            serde_json::json!({
                "baseUrl": "/work/demo",
                "paths": {
                    "@/*": ["./src/*"],
                    "@/behaviors/*": ["./behaviors/*"],
                    "@/dir/*": ["./behaviors/dir/*"],
                },
            }),
        );
        let resolver = TsconfigPaths::from_view(&prepared_view).expect("alias resolver");
        assert_eq!(
            resolver.resolve("@/behaviors/base", SCRIPT).as_deref(),
            Some("behaviors/base.ts"),
            "the longest prefix must win over the shorter one"
        );
        assert_eq!(
            resolver.resolve("@/dir/base", SCRIPT).as_deref(),
            Some("behaviors/dir/base.ts")
        );
    }

    #[test]
    fn alias_mappings_probe_index_files() {
        let prepared_view = with_compiler_options(
            view(
                RootKind::Component,
                vec![
                    prepared(SCRIPT, "Component({ methods: {} });"),
                    prepared("src/base/index.ts", "export default Behavior({});"),
                ],
            ),
            serde_json::json!({
                "baseUrl": "/work/demo",
                "paths": { "@/*": ["./src/*"] },
            }),
        );
        let resolver = TsconfigPaths::from_view(&prepared_view).expect("alias resolver");
        assert_eq!(
            resolver.resolve("@/base", SCRIPT).as_deref(),
            Some("src/base/index.ts")
        );
    }

    #[test]
    fn alias_targets_cannot_escape_the_project() {
        let prepared_view = with_compiler_options(
            view(
                RootKind::Component,
                vec![prepared(SCRIPT, "Component({ methods: {} });")],
            ),
            serde_json::json!({
                "baseUrl": "/work/demo",
                "paths": { "@/*": ["../../outside/*"] },
            }),
        );
        let resolver = TsconfigPaths::from_view(&prepared_view).expect("alias resolver");
        assert!(resolver.resolve("@/base", SCRIPT).is_none());
    }

    #[test]
    fn a_page_template_falls_back_to_a_component_registration() {
        let script = "Component({ methods: { onTap() {} } });";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        assert!(analysis.analysis.registration.selected.is_some());
        assert_method(method(&analysis, "onTap"), SCRIPT, script, "onTap");
    }

    #[test]
    fn a_static_registrar_alias_is_followed() {
        let script = "const page = Page;\npage({\n  onAlias() {},\n});";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        assert_method(method(&analysis, "onAlias"), SCRIPT, script, "onAlias");
    }

    #[test]
    fn literal_function_and_arrow_values_are_methods() {
        let script = "Page({\n  onShorthand() {},\n  onFunction: function () {},\n  onArrow: () => {},\n  onAsync: async () => {},\n});";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        for handler in ["onShorthand", "onFunction", "onArrow", "onAsync"] {
            assert_method(method(&analysis, handler), SCRIPT, script, handler);
        }
    }

    #[test]
    fn a_unique_top_level_registration_wins_over_a_nested_one() {
        let script = "function factory() { Page({ nested() {} }); }\nPage({ top() {} });";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        assert!(analysis.analysis.methods.status.is_supported());
        assert!(analysis.analysis.registration.selected.is_some());
        assert_method(method(&analysis, "top"), SCRIPT, script, "top");
        assert!(
            !analysis
                .analysis
                .methods
                .methods
                .iter()
                .any(|method| method.name == "nested"),
            "a nested registration must not contribute methods"
        );
    }

    #[test]
    fn a_nested_registration_alone_is_not_complete() {
        let script = "function factory() { Page({ nested() {} }); }";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        assert!(!analysis.analysis.registration.complete);
        assert!(!analysis.analysis.methods.status.is_supported());
    }

    #[test]
    fn multiple_top_level_registrations_are_ambiguous() {
        let script = "Page({ onFirst() {} });\nPage({ onSecond() {} });";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        assert!(!analysis.analysis.registration.complete);
        assert!(!analysis.analysis.methods.status.is_supported());
    }

    #[test]
    fn an_opaque_member_stays_named_but_is_not_a_literal_method() {
        let script = "Page({ onTap: handlerRef });";
        let prepared_view = view(RootKind::Page, vec![prepared(SCRIPT, script)]);
        let analysis = analyze(&prepared_view, SCRIPT, RootKind::Page).expect("analysis");
        assert!(
            analysis
                .analysis
                .methods
                .opaque
                .iter()
                .any(|name| name == "onTap")
        );
        assert!(
            !analysis
                .analysis
                .methods
                .methods
                .iter()
                .any(|method| method.name == "onTap")
        );
    }

    #[test]
    fn a_missing_owning_script_is_an_analysis_error() {
        let prepared_view = view(RootKind::Page, Vec::new());
        let Err(status) = analyze(&prepared_view, SCRIPT, RootKind::Page) else {
            panic!("a missing owning script must not analyze");
        };
        assert!(
            matches!(status, CapabilityStatus::Partial { .. }),
            "{status:?}"
        );
    }
}
