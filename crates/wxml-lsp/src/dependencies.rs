//! Project-owned dependency edges used to bound invalidation.
//!
//! `wxml-project` does not expose a dependency graph, so the LSP host keeps a
//! conservative reverse index per project: source file to the WXML views that
//! can observe it. Edges come from the facts discovery already produces
//! (`app.json`, page/component JSON, owning scripts, components and the
//! `tsconfig.json` chain). A missing edge is never treated as "unaffected":
//! callers refresh every view of the owning project instead.
//!
//! The index deliberately contains only project-relative paths and no
//! filesystem identity, so it cannot point at another workspace folder.

use std::collections::{BTreeMap, BTreeSet};

use wxml_project::{Project, ProjectPath};

/// Reverse dependency edges for one loaded project.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DependencyIndex {
    /// Source path to the views that may change when it changes.
    sources: BTreeMap<ProjectPath, BTreeSet<ProjectPath>>,
    /// Every discovered view of the project.
    views: BTreeSet<ProjectPath>,
}

impl DependencyIndex {
    /// Builds the index from the current discovery and tsconfig facts.
    #[must_use]
    pub(crate) fn build(project: &Project) -> Self {
        let mut index = Self::default();
        for page in project.pages() {
            index.views.insert(page.wxml.clone());
            index.add(&page.wxml, &page.wxml);
            if let Some(script) = &page.script {
                index.add(script, &page.wxml);
            }
            if let Some(json) = &page.json {
                index.add(json, &page.wxml);
            }
        }
        for component in project.components() {
            index.views.insert(component.wxml.clone());
            index.add(&component.wxml, &component.wxml);
            if let Some(script) = &component.script {
                index.add(script, &component.wxml);
            }
            if let Some(json) = &component.json {
                index.add(json, &component.wxml);
            }
        }

        // `app.json` can change pages, subpackages and global components.
        let views: Vec<ProjectPath> = index.views.iter().cloned().collect();
        for view in &views {
            index.add(project.app_json(), view);
        }
        if let Some(tsconfig) = project.tsconfig() {
            for view in &views {
                index.add(&tsconfig.path, view);
                for parent in &tsconfig.extends {
                    index.add(parent, view);
                }
            }
        }

        // A page/component `usingComponents` entry links the calling view to
        // the resolved component properties. Component templates are not
        // added: changing them only affects their own view. Nested component
        // usages propagate to every transitive caller so an inner component
        // script also invalidates the page that renders the outer one.
        //
        // The reverse walk marks a view visited when it is queued. Marking on
        // pop instead would re-enqueue every parent once per already-visited
        // path; on a large app with shared components that grows to hundreds
        // of millions of iterations and seconds of startup latency.
        let mut own_base: BTreeMap<ProjectPath, ProjectPath> = BTreeMap::new();
        let mut users_by_base: BTreeMap<ProjectPath, BTreeSet<ProjectPath>> = BTreeMap::new();
        for page in project.pages() {
            for (_, usage) in page.config.iter() {
                let Some(base) = usage.component.as_ref() else {
                    continue;
                };
                users_by_base
                    .entry(base.clone())
                    .or_default()
                    .insert(page.wxml.clone());
            }
        }
        let mut component_by_id: BTreeMap<ProjectPath, &wxml_project::Component> = BTreeMap::new();
        for component in project.components() {
            component_by_id.insert(component.id.clone(), component);
            own_base.insert(component.wxml.clone(), component.id.clone());
            for (_, usage) in component.config.iter() {
                let Some(base) = usage.component.as_ref() else {
                    continue;
                };
                users_by_base
                    .entry(base.clone())
                    .or_default()
                    .insert(component.wxml.clone());
            }
        }
        for (base, direct) in &users_by_base {
            let mut pending: Vec<ProjectPath> = direct.iter().cloned().collect();
            let mut affected = BTreeSet::new();
            for view in direct {
                affected.insert(view.clone());
            }
            while let Some(view) = pending.pop() {
                if let Some(parent_base) = own_base.get(&view)
                    && let Some(parents) = users_by_base.get(parent_base)
                {
                    for parent in parents {
                        if affected.insert(parent.clone()) {
                            pending.push(parent.clone());
                        }
                    }
                }
            }
            let Some(component) = component_by_id.get(base) else {
                continue;
            };
            for source in [component.script.as_ref(), component.json.as_ref()]
                .into_iter()
                .flatten()
            {
                for view in &affected {
                    index.add(source, view);
                }
            }
        }
        index
    }

    fn add(&mut self, source: &ProjectPath, view: &ProjectPath) {
        self.sources
            .entry(source.clone())
            .or_default()
            .insert(view.clone());
    }

    /// Returns the views that declare an edge from `source`.
    ///
    /// `None` means the source was never seen by discovery; the caller must
    /// treat it as an unknown dependency and refresh the owning project.
    #[must_use]
    pub(crate) fn affected(&self, source: &ProjectPath) -> Option<Vec<ProjectPath>> {
        self.sources
            .get(source)
            .map(|views| views.iter().cloned().collect())
    }

    /// Returns every discovered view in deterministic order.
    #[cfg(test)]
    pub(crate) fn all_views(&self) -> Vec<ProjectPath> {
        self.views.iter().cloned().collect()
    }

    /// Returns the first view that declares this write target, if any.
    ///
    /// Used by tests and diagnostics to assert the graph shape without
    /// depending on internal map order.
    #[cfg(test)]
    pub(crate) fn is_view(&self, path: &ProjectPath) -> bool {
        self.views.contains(path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::assert_is_empty)]

    use std::path::Path;

    use wxml_project::{Project, ProjectPath};

    use super::DependencyIndex;

    fn fixture() -> Project {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        Project::open(root).expect("component fixture")
    }

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("project path")
    }

    fn affected(index: &DependencyIndex, raw: &str) -> Option<Vec<ProjectPath>> {
        index.affected(&path(raw))
    }

    #[test]
    fn app_json_affects_every_project_view() {
        let project = fixture();
        let index = DependencyIndex::build(&project);
        let views = affected(&index, "app.json").expect("app edge");
        assert_eq!(views.len(), 2, "{views:?}");
        assert!(views.contains(&path("pages/index/index.wxml")));
        assert!(views.contains(&path("components/card/card.wxml")));
    }

    #[test]
    fn owning_scripts_and_page_json_are_precise() {
        let project = fixture();
        let index = DependencyIndex::build(&project);
        assert_eq!(
            affected(&index, "pages/index/index.ts").expect("page script"),
            vec![path("pages/index/index.wxml")]
        );
        assert_eq!(
            affected(&index, "pages/index/index.json").expect("page json"),
            vec![path("pages/index/index.wxml")]
        );
        assert_eq!(
            affected(&index, "components/card/card.ts").expect("component script"),
            vec![
                path("components/card/card.wxml"),
                path("pages/index/index.wxml")
            ]
        );
    }

    #[test]
    fn component_template_changes_only_affect_its_own_view() {
        let project = fixture();
        let index = DependencyIndex::build(&project);
        assert_eq!(
            affected(&index, "components/card/card.wxml").expect("component template"),
            vec![path("components/card/card.wxml")]
        );
    }

    #[test]
    fn unknown_sources_are_explicitly_unknown() {
        let project = fixture();
        let index = DependencyIndex::build(&project);
        assert!(affected(&index, "utils/unknown.ts").is_none());
        assert!(index.is_view(&path("pages/index/index.wxml")));
        assert!(!index.is_view(&path("utils/unknown.ts")));
        assert_eq!(index.all_views().len(), 2);
    }

    #[test]
    fn nested_component_scripts_affect_transitive_users() {
        let temp = std::env::var_os("TMPDIR")
            .map_or_else(
                || std::path::PathBuf::from("/tmp/opencode"),
                std::path::PathBuf::from,
            )
            .join(format!("wxml-deps-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write(&temp, "app.json", r#"{"pages":["pages/index/index"]}"#);
        write(
            &temp,
            "pages/index/index.wxml",
            r"<outer-card></outer-card>",
        );
        write(
            &temp,
            "pages/index/index.json",
            r#"{"usingComponents":{"outer-card":"../../components/outer/outer"}}"#,
        );
        write(&temp, "pages/index/index.ts", "Page({})");
        write(
            &temp,
            "components/outer/outer.wxml",
            r"<inner-badge></inner-badge>",
        );
        write(
            &temp,
            "components/outer/outer.json",
            r#"{"component":true,"usingComponents":{"inner-badge":"../inner/inner"}}"#,
        );
        write(&temp, "components/outer/outer.ts", "Component({})");
        write(&temp, "components/inner/inner.wxml", "<view></view>");
        write(
            &temp,
            "components/inner/inner.json",
            r#"{"component":true}"#,
        );
        write(&temp, "components/inner/inner.ts", "Component({})");

        let project = Project::open(&temp).expect("nested component project");
        let index = DependencyIndex::build(&project);
        assert_eq!(
            affected(&index, "components/inner/inner.ts").expect("inner script"),
            vec![
                path("components/inner/inner.wxml"),
                path("components/outer/outer.wxml"),
                path("pages/index/index.wxml")
            ]
        );
        assert_eq!(
            affected(&index, "components/outer/outer.ts").expect("outer script"),
            vec![
                path("components/outer/outer.wxml"),
                path("pages/index/index.wxml")
            ]
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let target = root.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(target, contents).expect("fixture file");
    }

    #[test]
    fn tsconfig_edges_include_the_extended_chain() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/component-basic");
        let temp = std::env::var_os("TMPDIR")
            .map_or_else(
                || std::path::PathBuf::from("/tmp/opencode"),
                std::path::PathBuf::from,
            )
            .join(format!("wxml-deps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let mut files = Vec::new();
        for entry in walk(&root) {
            let relative = entry.strip_prefix(&root).expect("relative");
            let target = temp.join(relative);
            std::fs::create_dir_all(target.parent().expect("parent")).expect("parent dir");
            std::fs::copy(&entry, &target).expect("copy fixture");
            files.push(relative.to_path_buf());
        }
        std::fs::write(
            temp.join("tsconfig.base.json"),
            r#"{"compilerOptions":{"strict":true}}"#,
        )
        .expect("base config");
        std::fs::write(
            temp.join("tsconfig.json"),
            r#"{"extends":"./tsconfig.base.json"}"#,
        )
        .expect("root config");
        let project = Project::open(&temp).expect("project with extends");
        let index = DependencyIndex::build(&project);
        assert!(affected(&index, "tsconfig.json").is_some());
        assert!(affected(&index, "tsconfig.base.json").is_some());
        let _ = std::fs::remove_dir_all(&temp);
        assert!(!files.is_empty());
    }

    fn walk(root: &Path) -> Vec<std::path::PathBuf> {
        let mut pending = vec![root.to_path_buf()];
        let mut files = Vec::new();
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name == "expected") {
                        continue;
                    }
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }
}
