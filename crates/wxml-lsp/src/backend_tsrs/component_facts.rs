//! Shared component property facts for one prepared view.
//!
//! Both the request plan (`input`) and the component-binding pass read the same
//! declaration-first property facts: a complete `components.d.ts` declaration
//! describes the whole attribute surface, otherwise the component script's
//! `properties` overlay is used. The overlay may be incomplete because a
//! behavior could not be read; the members it does contain stay checkable and
//! callers must not treat an undeclared attribute as an error.
//!
//! Script facts are resolved once per component path so one request never
//! analyzes the same component twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsrs::script::properties::{PropertyFact, PropertyOverlay};
use wxml_backend_tsrs::script::registration::RegistrationKind;
use wxml_backend_tsrs::script::{ScriptRequest, analyze_script};
use wxml_project::typing::TypingComponent;
use wxml_types::ComponentUsage;

use crate::backend_tsrs::hover::DeclaredPropertyType;
use crate::project_host::PreparedView;
use crate::semantics::tsconfig_paths::TsconfigPaths;

/// Builds one pure-Rust analysis input over the prepared script files.
///
/// Files the native input cannot represent are skipped, exactly like the
/// per-caller loops this replaced. Callers keep their own project key and
/// revision so the analysis snapshot identity is unchanged.
pub(crate) fn analysis_input(
    view: &PreparedView,
    project: &ProjectKey,
    root: &ProjectRoot,
    revision: SnapshotRevision,
) -> Option<AnalysisInput> {
    let files = view
        .files
        .iter()
        .filter_map(|prepared| {
            SourceFile::new(
                prepared.path.as_str(),
                prepared.text.to_string(),
                DocumentRevision::new(prepared.source_revision),
            )
            .ok()
        })
        .collect();
    AnalysisInput::new(project.clone(), root.clone(), revision, files).ok()
}

/// Script-derived component property facts and their coverage.
#[derive(Clone, Debug)]
pub(crate) struct ScriptPropertyFacts {
    /// Property name to declaration.
    ///
    /// The overlay merges own and behavior properties by name, so the map is
    /// unique and keeps the existing lookup semantics.
    members: BTreeMap<String, PropertyFact>,
    /// Every property, including behavior-contributed ones, was read.
    complete: bool,
}

impl ScriptPropertyFacts {
    /// Wraps one script property overlay.
    pub(crate) fn from_overlay(overlay: &PropertyOverlay) -> Self {
        Self {
            members: overlay
                .properties
                .iter()
                .map(|fact| (fact.name.clone(), fact.clone()))
                .collect(),
            complete: overlay.complete,
        }
    }

    /// Returns whether the script declared no properties at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Returns whether every property, including behaviors, was read.
    ///
    /// `false` means a behavior may contribute members not listed here: the
    /// listed members stay checkable, but an unlisted attribute is not an
    /// error.
    pub(crate) const fn complete(&self) -> bool {
        self.complete
    }

    /// Returns one member using the `WeChat` kebab/camel name rule.
    pub(crate) fn member(&self, attribute: &str) -> Option<&PropertyFact> {
        if let Some(found) = self.members.get(attribute) {
            return Some(found);
        }
        self.members
            .iter()
            .find(|(name, _)| wxml_project::component_name_eq(name, attribute))
            .map(|(_, fact)| fact)
    }

    /// Maps the facts to the hover-facing declared type overlay.
    pub(crate) fn declared_types(&self) -> BTreeMap<String, DeclaredPropertyType> {
        self.members
            .iter()
            .map(|(name, fact)| {
                (
                    name.clone(),
                    DeclaredPropertyType {
                        type_text: fact.type_text.clone(),
                        complete: fact.type_complete && self.complete,
                    },
                )
            })
            .collect()
    }
}

/// Declaration-first property facts for one component usage.
#[derive(Clone, Debug)]
pub(crate) enum ComponentUsageFacts {
    /// The project declares the tag completely; this is the whole surface.
    Declared(Arc<TypingComponent>),
    /// Script `properties` are the fallback source.
    Script(Arc<ScriptPropertyFacts>),
    /// The component script is missing or not analyzable.
    Unavailable,
}

/// Resolves component property facts for one prepared view.
///
/// Declarations are looked up by tag and script facts are cached by component
/// path, so repeated usages of the same component never re-analyze it.
pub(crate) struct ComponentFactsCache<'a> {
    view: &'a PreparedView,
    input: Option<AnalysisInput>,
    scripts: BTreeMap<String, Option<Arc<ScriptPropertyFacts>>>,
}

impl<'a> ComponentFactsCache<'a> {
    /// Creates a cache over one prepared view.
    #[must_use]
    pub(crate) fn new(view: &'a PreparedView) -> Self {
        let input = view
            .project_root
            .to_str()
            .and_then(|root| ProjectRoot::new(root).ok())
            .and_then(|root| {
                analysis_input(
                    view,
                    &ProjectKey::new("wxml-lsp-component-bindings"),
                    &root,
                    SnapshotRevision::new(1),
                )
            });
        Self {
            view,
            input,
            scripts: BTreeMap::new(),
        }
    }

    /// Returns whether the view has a representable script analysis input.
    pub(crate) const fn has_input(&self) -> bool {
        self.input.is_some()
    }

    /// Resolves the declaration-first facts of one usage.
    pub(crate) fn resolve(&mut self, usage: &ComponentUsage) -> ComponentUsageFacts {
        if let Some(component) = self
            .view
            .typing
            .as_ref()
            .and_then(|facts| facts.component(&usage.tag))
            && component.complete
        {
            return ComponentUsageFacts::Declared(Arc::new(component.clone()));
        }
        let Some(path) = usage.component_file.as_ref().map(wxml_types::FileId::path) else {
            return ComponentUsageFacts::Unavailable;
        };
        self.script(path).map_or(
            ComponentUsageFacts::Unavailable,
            ComponentUsageFacts::Script,
        )
    }

    /// Returns one component's script facts, cached by component path.
    fn script(&mut self, path: &str) -> Option<Arc<ScriptPropertyFacts>> {
        if let Some(cached) = self.scripts.get(path) {
            return cached.clone();
        }
        let parsed = self
            .input
            .as_ref()
            .and_then(|input| analyze_component_properties(input, self.view, path))
            .map(Arc::new);
        self.scripts.insert(path.to_owned(), parsed.clone());
        parsed
    }
}

/// Analyzes one component script and extracts its property overlay.
///
/// A registration that is not structurally complete is unavailable rather than
/// partially trusted, and a script whose own `properties` could not be
/// enumerated is unavailable because unlisted members would look unknown.
fn analyze_component_properties(
    input: &AnalysisInput,
    view: &PreparedView,
    path: &str,
) -> Option<ScriptPropertyFacts> {
    let resolver = TsconfigPaths::from_view(view);
    let request = ScriptRequest::new(input, path, RegistrationKind::Component);
    let request = resolver
        .as_ref()
        .map_or(request, |resolver| request.with_resolver(resolver));
    let analysis = analyze_script(&request).ok()?;
    if !analysis.registration.complete {
        return None;
    }
    // behaviors 不可读但自身 properties 已完整枚举时，仍可用自身声明做校验；
    // 未声明的属性可能由 behavior 贡献，调用方不得报错。
    if !analysis.properties.complete && !analysis.properties.own_complete {
        return None;
    }
    Some(ScriptPropertyFacts::from_overlay(&analysis.properties))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use wxml_project::ProjectPath;
    use wxml_types::{FileId, ProjectId};

    use super::*;
    use crate::project_host::PreparedFile;

    const CARD_PATH: &str = "components/card/card.ts";
    const DECLARATION_PATH: &str = "components.d.ts";

    fn file_id(path: &str) -> FileId {
        FileId::new(ProjectId::new("demo"), path).expect("file id")
    }

    fn prepared_file(path: &str, text: &str) -> PreparedFile {
        PreparedFile {
            file: file_id(path),
            path: ProjectPath::new(path).expect("project path"),
            kind: "ts",
            text: Arc::from(text),
            source_revision: 1,
        }
    }

    fn usage(tag: &str, component: Option<&str>) -> ComponentUsage {
        ComponentUsage {
            tag: tag.to_owned(),
            component_file: component.map(file_id),
            attributes: Vec::new(),
        }
    }

    fn view(files: Vec<PreparedFile>, declaration: Option<&str>) -> PreparedView {
        PreparedView {
            view_id: "pages/index/index.wxml:1".to_owned(),
            target: None,
            files: Arc::from(files),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(Vec::new()),
            wxs_modules: Arc::from(Vec::new()),
            components: Arc::from(Vec::new()),
            typing: declaration.map(|text| {
                let source = wxml_project::SourceFile {
                    path: ProjectPath::new(DECLARATION_PATH).expect("declaration path"),
                    kind: wxml_project::SourceKind::Dts,
                    text: text.to_owned(),
                    source_revision: 1,
                };
                Arc::new(wxml_project::parse_component_typing_facts(
                    &wxml_project::SourceSet {
                        files: vec![source],
                        truncated: false,
                        issues: Vec::new(),
                    },
                ))
            }),
            events: Arc::from(Vec::new()),
            project_root: PathBuf::from("/work/demo"),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
            source_complete: true,
        }
    }

    const CARD_SCRIPT: &str =
        "Component({\n  properties: {\n    title: { type: String },\n  },\n});\n";

    #[test]
    fn complete_declaration_wins_without_a_component_script() {
        let declaration = "interface WxmlComponentMap {\n  'my-card': CardDeclaration;\n}\ninterface CardDeclaration {\n  props: { title: string };\n}\n";
        let view = view(vec![], Some(declaration));
        let mut cache = ComponentFactsCache::new(&view);
        let facts = cache.resolve(&usage("my-card", Some(CARD_PATH)));
        let ComponentUsageFacts::Declared(component) = facts else {
            panic!("expected declaration facts, got {facts:?}");
        };
        assert_eq!(component.props.len(), 1);
        assert_eq!(component.props[0].name, "title");
    }

    #[test]
    fn incomplete_declaration_falls_back_to_script_properties() {
        let declaration = "interface WxmlComponentMap {\n  'my-card': GhostBox;\n}\n";
        let view = view(
            vec![prepared_file(CARD_PATH, CARD_SCRIPT)],
            Some(declaration),
        );
        let mut cache = ComponentFactsCache::new(&view);
        let facts = cache.resolve(&usage("my-card", Some(CARD_PATH)));
        let ComponentUsageFacts::Script(properties) = facts else {
            panic!("expected script facts, got {facts:?}");
        };
        assert!(properties.complete());
        assert_eq!(
            properties
                .member("title")
                .map(|fact| fact.type_text.as_str()),
            Some("string")
        );
    }

    #[test]
    fn script_facts_are_cached_by_component_path() {
        let view = view(vec![prepared_file(CARD_PATH, CARD_SCRIPT)], None);
        let mut cache = ComponentFactsCache::new(&view);
        let first = cache.resolve(&usage("first-card", Some(CARD_PATH)));
        let second = cache.resolve(&usage("second-card", Some(CARD_PATH)));
        let (ComponentUsageFacts::Script(first), ComponentUsageFacts::Script(second)) =
            (first, second)
        else {
            panic!("expected script facts");
        };
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn incomplete_behaviors_keep_the_declared_members() {
        let script = "Component({\n  behaviors: [Behavior({ properties: { score: { type: Number } } })],\n  properties: {\n    title: { type: String },\n  },\n});\n";
        let view = view(vec![prepared_file(CARD_PATH, script)], None);
        let mut cache = ComponentFactsCache::new(&view);
        let facts = cache.resolve(&usage("my-card", Some(CARD_PATH)));
        let ComponentUsageFacts::Script(properties) = facts else {
            panic!("expected script facts, got {facts:?}");
        };
        assert_eq!(
            properties
                .member("title")
                .map(|fact| fact.type_text.as_str()),
            Some("string")
        );
        assert!(
            properties.complete(),
            "the behavior is statically readable in this fixture"
        );
    }

    #[test]
    fn unreadable_behaviors_are_incomplete_but_keep_own_members() {
        let script = "import missing from './missing';\nComponent({\n  behaviors: [missing],\n  properties: {\n    title: { type: String },\n  },\n});\n";
        let view = view(vec![prepared_file(CARD_PATH, script)], None);
        let mut cache = ComponentFactsCache::new(&view);
        let facts = cache.resolve(&usage("my-card", Some(CARD_PATH)));
        let ComponentUsageFacts::Script(properties) = facts else {
            panic!("expected script facts, got {facts:?}");
        };
        assert!(!properties.complete());
        assert_eq!(
            properties
                .member("title")
                .map(|fact| fact.type_text.as_str()),
            Some("string")
        );
    }

    #[test]
    fn missing_component_script_is_unavailable() {
        let view = view(vec![], None);
        let mut cache = ComponentFactsCache::new(&view);
        assert!(matches!(
            cache.resolve(&usage("my-card", Some(CARD_PATH))),
            ComponentUsageFacts::Unavailable
        ));
    }

    #[test]
    fn declared_types_keep_the_overlay_completeness() {
        let script = "import missing from './missing';\nComponent({\n  behaviors: [missing],\n  properties: {\n    title: { type: String },\n  },\n});\n";
        let view = view(vec![prepared_file(CARD_PATH, script)], None);
        let mut cache = ComponentFactsCache::new(&view);
        let ComponentUsageFacts::Script(properties) =
            cache.resolve(&usage("my-card", Some(CARD_PATH)))
        else {
            panic!("expected script facts");
        };
        let declared = properties.declared_types();
        let title = declared.get("title").expect("title fact");
        assert!(!title.complete);
    }
}
