//! Mini Program project model.
//!
//! Discovers `app.json`, pages and components, resolves `usingComponents`
//! references, tracks unsaved editor overlays, and reads the applicable
//! TypeScript project configuration. Analysis and LSP code query this model
//! instead of walking the filesystem themselves.
//!
//! Paths are project-relative [`ProjectPath`] values; absolute host paths never
//! leave this crate through error messages. `wxml-project` depends only on
//! `wxml-text` so the layering in ADR-0001 (`project -> text`) stays intact.
//! The LSP layer maps [`FileKey`] to its own `FileId`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

pub mod config;
pub mod discovery;
pub mod files;
pub mod overlay;
pub mod script;
pub mod sources;

pub use config::{ConfigError, TsConfig, load_ts_config};
pub use discovery::{
    AppConfig, Component, ComponentTargetKind, Discovery, DiscoveryIssue, DiscoveryIssueKind, Page,
    PageConfig, Parsed, UsingComponent, discover,
};
pub use files::{
    FileError, FileKey, PathError, ProjectPath, UriError, read_file, root_path_from_uri,
};
pub use overlay::{Overlay, OverlayError};
pub use script::{DataRoot, DataRootKind, ScriptResolution, extract_data_roots, resolve_script};
pub use sources::{SourceFile, SourceIssue, SourceIssueKind, SourceKind, SourceLimits, SourceSet};

/// Why a project could not be opened.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProjectError {
    /// The project root does not exist or cannot be inspected.
    #[error("project root is not accessible: {kind:?}")]
    RootUnavailable {
        /// Operating-system error category. The raw message may contain an
        /// absolute path, so it is not retained.
        kind: std::io::ErrorKind,
    },
    /// The project root exists but is not a directory.
    #[error("project root is not a directory")]
    RootNotDirectory,
    /// A fixed project path could not be constructed.
    #[error(transparent)]
    Path(#[from] PathError),
    /// The project root URI is invalid or not a local file URI.
    #[error(transparent)]
    Uri(#[from] UriError),
}

/// An opened Mini Program project rooted at one canonical directory.
///
/// `open` performs discovery once. Overlay edits do not automatically refresh
/// the page/component lists; call [`Project::rediscover`] after changing
/// project JSON or scripts so unsaved configuration is reflected.
pub struct Project {
    root: PathBuf,
    app_json: ProjectPath,
    /// Directory the Mini Program routes and scripts are relative to.
    ///
    /// This is the `app.json` directory. It is `None` when `app.json` lives at
    /// the project root, which keeps the common case free of a path prefix.
    source_root: Option<ProjectPath>,
    tsconfig_path: ProjectPath,
    overlay: Overlay,
    discovery: Discovery,
    tsconfig: Option<TsConfig>,
}

#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for Project {
    /// Formats project facts without exposing the absolute root path.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Project")
            .field("root", &"<project root>")
            .field("pages", &self.discovery.pages.len())
            .field("components", &self.discovery.components.len())
            .field("issues", &self.discovery.issues.len())
            .field("overlays", &self.overlay.len())
            .field(
                "tsconfig",
                &self.tsconfig.as_ref().map(|config| &config.path),
            )
            .finish()
    }
}

impl Project {
    /// Opens a project rooted at `root`.
    ///
    /// The root is canonicalized so symlink checks compare against the real
    /// directory. Discovery and tsconfig problems are reported through
    /// [`Project::issues`] instead of failing the open.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when the root is inaccessible or not a
    /// directory.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ProjectError> {
        Self::open_with_app_json(root, ProjectPath::new("app.json")?)
    }

    /// Opens a project whose `app.json` is at `app_json`.
    ///
    /// The project root is the directory holding the TypeScript configuration,
    /// which may be an ancestor of the directory holding `app.json`. For
    /// example, a monorepo may keep `tsconfig.json` in `apps/miniprogram` and
    /// `app.json` in `apps/miniprogram/src`; the project root is then
    /// `apps/miniprogram`, and routes and scripts use a `src/` prefix.
    ///
    /// `app_json` must be a canonical path inside `root`. Discovery still uses
    /// the `app.json` directory as the base for page routes, so the returned
    /// [`ProjectPath`] values remain valid project paths.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] when the root is inaccessible or not a
    /// directory, or when `app_json` is not a valid path inside it.
    pub fn open_with_app_json(
        root: impl Into<PathBuf>,
        app_json: ProjectPath,
    ) -> Result<Self, ProjectError> {
        let root = root.into();
        let canonical = std::fs::canonicalize(&root)
            .map_err(|error| ProjectError::RootUnavailable { kind: error.kind() })?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| ProjectError::RootUnavailable { kind: error.kind() })?;
        if !metadata.is_dir() {
            return Err(ProjectError::RootNotDirectory);
        }
        let source_root = app_json.parent();
        let tsconfig_path = ProjectPath::new("tsconfig.json")?;
        let overlay = Overlay::new();
        let discovery = discover(&canonical, &overlay, &app_json);
        let mut project = Self {
            root: canonical,
            app_json,
            source_root,
            tsconfig_path,
            overlay,
            discovery,
            tsconfig: None,
        };
        project.reload_tsconfig();
        Ok(project)
    }

    /// Opens a project from a local `file://` URI.
    ///
    /// The URI is decoded and validated by [`files::root_path_from_uri`]; the
    /// resulting path then goes through [`Project::open`] and its usual
    /// canonicalization and symlink rules.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError::Uri`] for invalid or non-local URIs, and the
    /// same root errors as [`Project::open`].
    pub fn open_uri(uri: &str) -> Result<Self, ProjectError> {
        let root = root_path_from_uri(uri)?;
        Self::open(root)
    }

    /// Returns the canonical absolute project root.
    ///
    /// This is the value the LSP layer passes to the Node backend as
    /// `projectRoot`. It is an internal host path: always convert it to a
    /// `file://` URI (for example with [`Project::uri_for`]) before sending
    /// anything to a client.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns discovered pages in declaration order.
    #[must_use]
    pub fn pages(&self) -> &[Page] {
        &self.discovery.pages
    }

    /// Returns discovered components sorted by id.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.discovery.components
    }

    /// Returns the `app.json` path relative to the project root.
    #[must_use]
    pub const fn app_json(&self) -> &ProjectPath {
        &self.app_json
    }

    /// Returns the directory containing `app.json`, relative to the root.
    ///
    /// `None` means `app.json` is at the project root.
    #[must_use]
    pub const fn source_root(&self) -> Option<&ProjectPath> {
        self.source_root.as_ref()
    }

    /// Returns whether `path` belongs to this Mini Program's source tree.
    pub(crate) fn contains_source(&self, path: &ProjectPath) -> bool {
        self.source_root.as_ref().is_none_or(|root| {
            path == root
                || path
                    .as_str()
                    .strip_prefix(root.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// Finds a page by route, normalizing leading `./` and `\` separators.
    #[must_use]
    pub fn page(&self, route: &str) -> Option<&Page> {
        let normalized = ProjectPath::new(route);
        let needle = normalized.as_ref().map_or(route, |path| path.as_str());
        self.discovery
            .pages
            .iter()
            .find(|page| page.route == needle)
    }

    /// Finds a component by its project-relative base path.
    #[must_use]
    pub fn component(&self, id: &ProjectPath) -> Option<&Component> {
        self.discovery
            .components
            .iter()
            .find(|component| &component.id == id)
    }

    /// Returns discovery issues accumulated at open or rediscovery.
    #[must_use]
    pub fn issues(&self) -> &[DiscoveryIssue] {
        &self.discovery.issues
    }

    /// Returns whether the latest discovery was truncated by a budget.
    ///
    /// This mirrors [`Discovery::truncated`] and is the structured signal for
    /// page, component, JSON, component-script and issue limits.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.discovery.truncated
    }

    /// Returns the loaded root `tsconfig.json`, if any.
    #[must_use]
    pub const fn tsconfig(&self) -> Option<&TsConfig> {
        self.tsconfig.as_ref()
    }

    /// Returns merged raw `compilerOptions` from the loaded tsconfig.
    ///
    /// `None` means no root `tsconfig.json` was found or it failed to load.
    /// The map includes known and unknown options; the LSP layer filters what
    /// it forwards to the type backend and marks unsupported values as
    /// partial instead of dropping them here.
    #[must_use]
    pub fn compiler_options(&self) -> Option<&BTreeMap<String, serde_json::Value>> {
        self.tsconfig
            .as_ref()
            .map(|config| &config.compiler_options)
    }

    /// Returns the overlay.
    #[must_use]
    pub const fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// Sets overlay text with a monotonic revision.
    ///
    /// # Errors
    ///
    /// Returns [`OverlayError::TooLarge`] when the text exceeds
    /// [`files::MAX_FILE_BYTES`] and [`OverlayError::StaleRevision`] when
    /// `revision` is not newer than the stored revision.
    pub fn set_overlay(
        &mut self,
        path: &ProjectPath,
        text: impl Into<String>,
        revision: u64,
    ) -> Result<(), OverlayError> {
        self.overlay.set(path, text, revision)
    }

    /// Removes an overlay entry, falling back to disk contents.
    pub fn remove_overlay(&mut self, path: &ProjectPath) -> bool {
        self.overlay.remove(path)
    }

    /// Returns the effective size of a file without reading its contents when
    /// it is not overlaid. Used to bound large optional sources (for example a
    /// WXS `src`) before allocating their text.
    #[must_use]
    pub fn file_size(&self, path: &ProjectPath) -> Option<u64> {
        if let Some(text) = self.overlay.text(path) {
            return Some(u64::try_from(text.len()).unwrap_or(u64::MAX));
        }
        let target = self.root.join(path.as_str());
        let canonical = std::fs::canonicalize(&target).ok()?;
        if !canonical.starts_with(&self.root) {
            return None;
        }
        std::fs::metadata(canonical)
            .ok()
            .map(|metadata| metadata.len())
    }

    /// Reads effective text: overlay first, then disk.
    ///
    /// # Errors
    ///
    /// Returns [`FileError`] when the file is missing, escapes the root, is not
    /// UTF-8, or cannot be read.
    pub fn read(&self, path: &ProjectPath) -> Result<String, FileError> {
        self.read_optional(path)?
            .map_or_else(|| Err(FileError::NotFound { path: path.clone() }), Ok)
    }

    /// Reads effective text, returning `None` when the file is absent.
    ///
    /// # Errors
    ///
    /// Returns [`FileError`] for symlink escapes, invalid UTF-8, or read
    /// failures other than a missing file.
    pub fn read_optional(&self, path: &ProjectPath) -> Result<Option<String>, FileError> {
        self.overlay.read_text(&self.root, path)
    }

    /// Extracts registration data roots from the effective script text.
    ///
    /// # Errors
    ///
    /// Returns [`FileError`] when the script cannot be read.
    pub fn data_roots(&self, script: &ProjectPath) -> Result<Vec<DataRoot>, FileError> {
        let text = self.read(script)?;
        Ok(extract_data_roots(script, &text))
    }

    /// Converts a `file://` URI into a project-relative path.
    ///
    /// # Errors
    ///
    /// Returns [`UriError`] for non-file schemes, malformed escapes and paths
    /// outside the project root.
    pub fn path_from_uri(&self, uri: &str) -> Result<ProjectPath, UriError> {
        files::project_path_from_uri(&self.root, uri)
    }

    /// Builds a `file://` URI for a project-relative path.
    #[must_use]
    pub fn uri_for(&self, path: &ProjectPath) -> Option<String> {
        files::uri_for_project_path(&self.root, path)
    }

    /// Re-runs discovery and tsconfig loading against the current overlay.
    ///
    /// Each call re-reads at most `MAX_DISCOVERY_JSON_BYTES` (32 MiB) of
    /// discovery JSON and `MAX_COMPONENT_SCRIPT_BYTES` (32 MiB) of component
    /// scripts, plus the bounded tsconfig chain. Truncation is reported
    /// through [`Project::issues`] and [`Discovery::truncated`].
    pub fn rediscover(&mut self) {
        self.discovery = discover(&self.root, &self.overlay, &self.app_json);
        self.reload_tsconfig();
    }

    /// Loads the root tsconfig, recording failures as discovery issues.
    fn reload_tsconfig(&mut self) {
        self.tsconfig = match load_ts_config(&self.root, &self.tsconfig_path, &self.overlay) {
            Ok(config) => Some(config),
            Err(ConfigError::NotFound { .. }) => None,
            Err(error) => {
                let new_issue = discovery::issue(
                    &self.tsconfig_path,
                    DiscoveryIssueKind::InvalidConfig,
                    error.to_string(),
                );
                if self.discovery.issues.len() < discovery::MAX_ISSUES {
                    self.discovery.issues.push(new_issue);
                }
                None
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn exposes_fixed_project_paths() {
        assert_eq!(
            ProjectPath::new("app.json").expect("valid").as_str(),
            "app.json"
        );
    }

    #[test]
    fn a_parent_tsconfig_roots_the_project_above_the_app_directory() {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = base.join(format!("wxml-parent-tsconfig-{stamp}"));
        let app = root.join("src");
        std::fs::create_dir_all(app.join("pages/index")).expect("create app");
        std::fs::write(
            app.join("app.json"),
            r#"{"pages":["pages/index/index"],"usingComponents":{"card":"/components/card/card"}}"#,
        )
        .expect("write app.json");
        std::fs::write(
            app.join("pages/index/index.wxml"),
            "<view>{{ title }}</view><card />",
        )
        .expect("write wxml");
        std::fs::write(
            app.join("pages/index/index.ts"),
            "Page({ data: { title: 'ok' } });",
        )
        .expect("write script");
        std::fs::create_dir_all(app.join("components/card")).expect("create component");
        std::fs::write(app.join("components/card/card.wxml"), "<view />")
            .expect("write component wxml");
        std::fs::write(app.join("components/card/card.ts"), "Component({})")
            .expect("write component script");
        std::fs::write(app.join("components/card/card.json"), "{}").expect("write component json");
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"./","paths":{"@/*":["src/*"]}}}"#,
        )
        .expect("write tsconfig");

        let project =
            Project::open_with_app_json(&root, ProjectPath::new("src/app.json").expect("app path"))
                .expect("open nested app");
        assert_eq!(project.root(), root.canonicalize().expect("canonical"));
        assert_eq!(
            project.tsconfig().map(|config| config.path.as_str()),
            Some("tsconfig.json")
        );
        assert_eq!(
            project.pages()[0].wxml.as_str(),
            "src/pages/index/index.wxml"
        );
        assert_eq!(
            project.pages()[0].script.as_ref().map(ProjectPath::as_str),
            Some("src/pages/index/index.ts")
        );
        let card = project
            .components()
            .iter()
            .find(|component| component.id.as_str() == "src/components/card/card")
            .expect("absolute usingComponents path resolves from the app root");
        assert_eq!(card.wxml.as_str(), "src/components/card/card.wxml");
        assert_eq!(
            card.script.as_ref().map(ProjectPath::as_str),
            Some("src/components/card/card.ts")
        );
        assert_eq!(
            project
                .tsconfig()
                .and_then(|config| config.paths.get("@/*"))
                .map(|paths| paths.iter().map(ProjectPath::as_str).collect::<Vec<_>>()),
            Some(vec!["src/*"])
        );
        let sources = project.collect_sources(SourceLimits::default());
        assert!(
            sources
                .files
                .iter()
                .all(|file| file.path.as_str().starts_with("src/"))
        );
        assert!(
            sources
                .files
                .iter()
                .any(|file| file.path.as_str() == "src/pages/index/index.ts")
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
