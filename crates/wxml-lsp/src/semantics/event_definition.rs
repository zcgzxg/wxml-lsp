//! Static event-handler facts from Rust script facts.
//!
//! `bindtap="onTap"` and the other WXML event-binding forms are matched
//! against the literal method table of the view's owning `Page`/`Component`
//! script, including statically resolved `Behavior` methods. The table comes
//! from [`ScriptAnalysis::methods`](wxml_backend_tsgo::script::ScriptAnalysis),
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

use std::path::Path;

use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::script::registration::RegistrationKind;
use wxml_backend_tsgo::script::{ModuleResolver, ScriptAnalysis, ScriptRequest, analyze_script};
use wxml_backend_tsgo::shadow::expression::is_identifier_name;
use wxml_text::SourceRange;
use wxml_types::{CapabilityStatus, RootKind};

use crate::completion::catalog::{self, EventAttributeClass};
use crate::project_host::{PreparedFile, PreparedView};

/// Maximum byte length of a static handler name, matching the template layer.
const MAX_HANDLER_NAME_BYTES: usize = 256;
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
    if handler.is_empty() || handler.len() > MAX_HANDLER_NAME_BYTES || !is_identifier_name(handler)
    {
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

/// Resolves non-relative specifiers through the view's tsconfig `paths`.
///
/// `baseUrl` is already absolute and inside the project, so mapping it back to
/// a project-relative directory keeps alias candidates comparable with the
/// prepared script list. A candidate only resolves when the mapped script is
/// part of the view, using the same extension and `index` probing as relative
/// imports.
struct TsconfigPaths<'a> {
    /// Project-relative `baseUrl` directory, empty for the project root.
    base: String,
    /// Sorted alias patterns; earlier entries win.
    mappings: Vec<PathMapping>,
    /// Prepared scripts that can satisfy a mapping.
    files: &'a [PreparedFile],
}

/// One tsconfig `paths` pattern and its replacement targets.
struct PathMapping {
    /// Pattern text before the optional `*`.
    prefix: String,
    /// Pattern text after the optional `*`.
    suffix: String,
    /// Whether the pattern contains a `*`.
    wildcard: bool,
    /// Replacement targets as written in tsconfig.
    targets: Vec<String>,
}

impl<'a> TsconfigPaths<'a> {
    /// Builds a resolver from the prepared view, `None` without usable paths.
    fn from_view(view: &'a PreparedView) -> Option<Self> {
        let mappings = view.compiler_options.get("paths")?.as_object()?;
        let base = match view
            .compiler_options
            .get("baseUrl")
            .and_then(serde_json::Value::as_str)
        {
            Some(base_url) => project_relative(&view.project_root, base_url)?,
            None => String::new(),
        };
        let mut parsed = Vec::with_capacity(mappings.len());
        for (pattern, targets) in mappings {
            let Some((prefix, suffix, wildcard)) = split_pattern(pattern) else {
                continue;
            };
            let targets: Vec<String> = targets
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .filter(|target| {
                    !target.starts_with('/') && !target.contains('\\') && !target.contains('\0')
                })
                .map(str::to_owned)
                .collect();
            if targets.is_empty() {
                continue;
            }
            parsed.push(PathMapping {
                prefix: prefix.to_owned(),
                suffix: suffix.to_owned(),
                wildcard,
                targets,
            });
        }
        if parsed.is_empty() {
            return None;
        }
        // Exact patterns win over wildcards; longer prefixes win among
        // wildcards, matching TypeScript's most-specific-pattern rule.
        parsed.sort_by(|left, right| {
            left.wildcard
                .cmp(&right.wildcard)
                .then_with(|| right.prefix.len().cmp(&left.prefix.len()))
                .then_with(|| right.suffix.len().cmp(&left.suffix.len()))
        });
        Some(Self {
            base,
            mappings: parsed,
            files: view.files.as_ref(),
        })
    }

    /// Returns the first mapped script present in the view.
    fn probe(&self, base: &str) -> Option<String> {
        candidate_paths(base).into_iter().find(|candidate| {
            self.files
                .iter()
                .any(|file| file.path.as_str() == candidate.as_str())
        })
    }
}

impl PathMapping {
    /// Returns the text captured by the pattern's `*`, if it matches.
    fn capture<'s>(&self, specifier: &'s str) -> Option<&'s str> {
        if self.wildcard {
            let rest = specifier.strip_prefix(self.prefix.as_str())?;
            rest.strip_suffix(self.suffix.as_str())
        } else if specifier == self.prefix {
            Some("")
        } else {
            None
        }
    }
}

impl ModuleResolver for TsconfigPaths<'_> {
    fn resolve(&self, specifier: &str, _from_path: &str) -> Option<String> {
        if specifier.is_empty() || specifier.contains('\\') || specifier.contains('\0') {
            return None;
        }
        let (mapping, capture) = self
            .mappings
            .iter()
            .find_map(|mapping| mapping.capture(specifier).map(|capture| (mapping, capture)))?;
        for target in &mapping.targets {
            let Some(base) = join_project_path(&self.base, &target.replace('*', capture)) else {
                continue;
            };
            if let Some(path) = self.probe(&base) {
                return Some(path);
            }
        }
        None
    }
}

/// Splits a `paths` pattern into prefix, suffix and wildcard state.
fn split_pattern(pattern: &str) -> Option<(&str, &str, bool)> {
    match pattern.split_once('*') {
        None => Some((pattern, "", false)),
        Some((prefix, suffix)) if !suffix.contains('*') => Some((prefix, suffix, true)),
        Some(_) => None,
    }
}

/// Maps an absolute `baseUrl` back to a project-relative directory.
fn project_relative(root: &Path, base_url: &str) -> Option<String> {
    let relative = Path::new(base_url).strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// Joins a project-relative base with one alias target, rejecting escapes.
fn join_project_path(base: &str, target: &str) -> Option<String> {
    if target.starts_with('/') || target.contains('\\') || target.contains('\0') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in base.split('/').chain(target.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Returns the same candidate order as a relative script import.
fn candidate_paths(base: &str) -> Vec<String> {
    if has_script_extension(base) {
        return vec![base.to_owned()];
    }
    vec![
        base.to_owned(),
        format!("{base}.ts"),
        format!("{base}.js"),
        format!("{base}.mts"),
        format!("{base}.cts"),
        format!("{base}/index.ts"),
        format!("{base}/index.js"),
        format!("{base}/index.mts"),
        format!("{base}/index.cts"),
    ]
}

/// Returns whether the final path segment already carries an extension.
fn has_script_extension(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
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

    use wxml_backend_tsgo::script::events::EventMethod;
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
