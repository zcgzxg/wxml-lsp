//! Mini Program discovery: `app.json`, pages, subpackages and components.
//!
//! Discovery is best-effort: malformed or missing inputs are recorded as
//! [`DiscoveryIssue`] entries and the remaining project facts are still
//! returned. JSON syntax errors abort one file only and keep the project
//! usable while the user edits it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use thiserror::Error;

use crate::files::{self, MAX_MESSAGE_BYTES, ProjectPath, truncate_message, truncate_owned};
use crate::overlay::Overlay;
use crate::script::{self, DataRoot, DataRootKind};

/// Maximum component-reference depth followed through `usingComponents`.
pub const MAX_COMPONENT_DEPTH: usize = 16;

/// Maximum number of page and component entries kept from one `app.json`.
///
/// Subpackage roots and pages count toward the same budget.
pub const MAX_APP_ENTRIES: usize = 10_000;

/// Maximum number of pages kept by one discovery run.
pub const MAX_PAGES: usize = 20_000;

/// Maximum number of components kept by one discovery run.
pub const MAX_COMPONENTS: usize = 20_000;

/// Maximum number of issues kept by one discovery run.
pub const MAX_ISSUES: usize = 1_000;

/// Maximum page-local `usingComponents` entries kept across the whole project.
///
/// Without this cross-page budget, a small workspace could make every page
/// reuse a large JSON file (for example via symlinks) and multiply its local
/// table by the page count.
pub const MAX_TOTAL_LOCAL_COMPONENTS: usize = 20_000;

/// Maximum page-local `usingComponents` entries kept for a single page.
///
/// Real pages rarely exceed a hundred custom components; this single-page cap
/// stops one page from consuming the entire project budget.
pub const MAX_PAGE_LOCAL_COMPONENTS: usize = 1_024;

/// Maximum JSON bytes parsed by one discovery run.
///
/// App, page and component JSON files both on disk and in overlays count. When
/// the budget is exhausted, remaining JSON is not parsed (paths are still
/// kept) and one `Truncated` issue is recorded.
pub const MAX_DISCOVERY_JSON_BYTES: usize = 32 * 1024 * 1024;

/// Maximum component-script bytes read by one discovery run.
///
/// Component scripts are read only to extract `properties` roots. The budget
/// covers overlay and disk text, including scripts reached through symlinks;
/// once exhausted, further scripts are skipped and one `Truncated` issue is
/// recorded.
pub const MAX_COMPONENT_SCRIPT_BYTES: usize = 32 * 1024 * 1024;

/// Top-level `app.json` facts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppConfig {
    /// Main-package page routes, in declaration order and normalized form.
    pub pages: Vec<String>,
    /// Subpackages with their declared pages.
    pub sub_packages: Vec<SubPackage>,
    /// Global `usingComponents`.
    pub using_components: BTreeMap<String, UsingComponent>,
}

/// One `subPackages` entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubPackage {
    /// Subpackage root, without a trailing slash.
    pub root: ProjectPath,
    /// Full page routes (`root/page`), in declaration order and normalized.
    pub pages: Vec<String>,
}

/// One `usingComponents` entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsingComponent {
    /// Local tag/name used by the referencing template.
    pub name: String,
    /// Raw specifier from the JSON file.
    pub specifier: String,
    /// Form of the specifier.
    pub kind: ComponentTargetKind,
    /// Resolved component base path for local components.
    pub component: Option<ProjectPath>,
}

/// Form of one `usingComponents` specifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ComponentTargetKind {
    /// Relative or root-absolute path inside this project.
    Local,
    /// `plugin://` component provided by a plugin.
    Plugin,
    /// npm package component.
    Package,
}

/// Effective configuration for one page or component JSON file.
///
/// The project-global table is shared by reference across every page; local
/// entries shadow global names without copying the global map.
///
/// - `global` is shared and must be treated as read-only.
/// - `local` takes precedence over `global`.
/// - Consumers should use [`PageConfig::get`] or [`PageConfig::iter`] instead
///   of merging the two maps.
///
/// Discovery bounds `local` per page ([`MAX_PAGE_LOCAL_COMPONENTS`]) and
/// across the project ([`MAX_TOTAL_LOCAL_COMPONENTS`]); truncation is reported
/// once through a `Truncated` discovery issue.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PageConfig {
    /// Project-global `usingComponents`, shared by all pages.
    pub global: Arc<BTreeMap<String, UsingComponent>>,
    /// Page/component-local `usingComponents`; these shadow global entries.
    pub local: BTreeMap<String, UsingComponent>,
}

impl PageConfig {
    /// Returns the effective component for `name`; local entries win.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&UsingComponent> {
        self.local.get(name).or_else(|| self.global.get(name))
    }

    /// Iterates effective entries, with local entries shadowing global ones.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &UsingComponent)> + '_ {
        self.local.iter().chain(
            self.global
                .iter()
                .filter(|(name, _)| !self.local.contains_key(*name)),
        )
    }
}

/// One discovered page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page {
    /// Normalized page route, without extension (`root/page` for subpackages).
    ///
    /// Leading `./` and backslashes from `app.json` are normalized away.
    pub route: String,
    /// Template path (`<route>.wxml`).
    pub wxml: ProjectPath,
    /// Resolved `.ts`/`.js` script, if either exists.
    pub script: Option<ProjectPath>,
    /// Existing page JSON, if any.
    pub json: Option<ProjectPath>,
    /// Global plus page-local component configuration.
    pub config: PageConfig,
}

/// One discovered custom component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Component {
    /// Component base path, also used as its identity.
    pub id: ProjectPath,
    /// Template path (`<id>.wxml`).
    pub wxml: ProjectPath,
    /// Resolved `.ts`/`.js` script, if either exists.
    pub script: Option<ProjectPath>,
    /// Existing component JSON, if any.
    pub json: Option<ProjectPath>,
    /// `properties` roots extracted from the script.
    pub properties: Vec<DataRoot>,
}

/// Severity-free category of one discovery issue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DiscoveryIssueKind {
    /// A referenced file does not exist.
    MissingFile,
    /// A JSON file is malformed or has the wrong shape.
    InvalidConfig,
    /// A page route or subpackage root is not a valid project path.
    InvalidRoute,
    /// A `usingComponents` specifier could not be resolved.
    InvalidComponentRef,
    /// A local component is referenced but its `.wxml` is missing.
    MissingComponent,
    /// Both `.ts` and `.js` exist; `.ts` was chosen.
    ScriptConflict,
    /// A file exists but could not be read.
    UnreadableFile,
    /// A list or result set was truncated at a safety limit.
    Truncated,
}

/// One recoverable discovery problem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryIssue {
    /// Project-relative file that produced the issue.
    pub file: ProjectPath,
    /// Issue category.
    pub kind: DiscoveryIssueKind,
    /// Human-readable message; never contains absolute paths.
    pub message: String,
}

/// Complete project discovery output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Discovery {
    /// Pages in declaration order.
    pub pages: Vec<Page>,
    /// Components sorted by id.
    pub components: Vec<Component>,
    /// Recoverable issues in discovery order.
    pub issues: Vec<DiscoveryIssue>,
    /// `true` when any page, component, JSON, script or issue budget truncated
    /// discovery.
    pub truncated: bool,
}

/// The result of parsing one JSON configuration file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parsed<T> {
    /// Parsed value with invalid entries already dropped.
    pub value: T,
    /// Per-entry problems that did not invalidate the whole file.
    pub issues: Vec<DiscoveryIssue>,
}

/// Why a configuration file could not be parsed.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// The file is not valid JSON.
    #[error("invalid JSON in {file} at line {line}, column {column}: {message}")]
    Json {
        /// Project-relative path of the file.
        file: ProjectPath,
        /// One-based line number.
        line: usize,
        /// One-based column number.
        column: usize,
        /// Parser message.
        message: String,
    },
}

/// Parses `app.json`.
///
/// # Errors
///
/// Returns [`DiscoveryError::Json`] when the text is not valid JSON.
pub fn parse_app_config(
    file: &ProjectPath,
    text: &str,
) -> Result<Parsed<AppConfig>, DiscoveryError> {
    let raw: RawAppConfig = parse_json(file, text)?;
    let RawAppConfig {
        pages: raw_pages,
        sub_packages: raw_sub_packages,
        using_components: raw_using_components,
    } = raw;
    let mut issues = Vec::new();
    let mut remaining = MAX_APP_ENTRIES;
    let mut truncated = false;

    let mut pages = Vec::new();
    for route in raw_pages {
        if remaining == 0 {
            truncated = true;
            break;
        }
        remaining -= 1;
        match ProjectPath::new(&route) {
            Ok(path) => pages.push(path.as_str().to_owned()),
            Err(error) => issues.push(issue(
                file,
                DiscoveryIssueKind::InvalidRoute,
                format!(
                    "invalid page route `{}`: {error}",
                    truncate_message(&route, MAX_MESSAGE_BYTES)
                ),
            )),
        }
    }

    let mut sub_packages = Vec::new();
    'sub_packages: for sub_package in raw_sub_packages {
        if remaining == 0 {
            truncated = true;
            break;
        }
        remaining -= 1;
        let root = match ProjectPath::new(&sub_package.root) {
            Ok(root) => root,
            Err(error) => {
                issues.push(issue(
                    file,
                    DiscoveryIssueKind::InvalidRoute,
                    format!(
                        "invalid subpackage root `{}`: {error}",
                        truncate_message(&sub_package.root, MAX_MESSAGE_BYTES)
                    ),
                ));
                continue;
            }
        };
        let mut routes = Vec::new();
        for page in sub_package.pages {
            if remaining == 0 {
                truncated = true;
                sub_packages.push(SubPackage {
                    root,
                    pages: routes,
                });
                break 'sub_packages;
            }
            remaining -= 1;
            let route = ProjectPath::new(&page)
                .and_then(|relative| ProjectPath::resolve(Some(&root), relative.as_str()));
            match route {
                Ok(route) => routes.push(route.as_str().to_owned()),
                Err(error) => issues.push(issue(
                    file,
                    DiscoveryIssueKind::InvalidRoute,
                    format!(
                        "invalid subpackage page `{}`: {error}",
                        truncate_message(&page, MAX_MESSAGE_BYTES)
                    ),
                )),
            }
        }
        sub_packages.push(SubPackage {
            root,
            pages: routes,
        });
    }

    let (using_components, component_issues, components_truncated) =
        resolve_components(file, raw_using_components, remaining);
    issues.extend(component_issues);
    truncated |= components_truncated;
    if truncated {
        issues.push(issue(
            file,
            DiscoveryIssueKind::Truncated,
            format!("app.json entries truncated to {MAX_APP_ENTRIES}"),
        ));
    }

    Ok(Parsed {
        value: AppConfig {
            pages,
            sub_packages,
            using_components,
        },
        issues,
    })
}

/// Parses one page or component JSON file.
///
/// # Errors
///
/// Returns [`DiscoveryError::Json`] when the text is not valid JSON.
pub fn parse_page_config(
    file: &ProjectPath,
    text: &str,
) -> Result<Parsed<PageConfig>, DiscoveryError> {
    let raw: RawPageConfig = parse_json(file, text)?;
    let (using_components, mut issues, truncated) =
        resolve_components(file, raw.using_components, MAX_APP_ENTRIES);
    if truncated {
        issues.push(issue(
            file,
            DiscoveryIssueKind::Truncated,
            format!("usingComponents truncated to {MAX_APP_ENTRIES} entries"),
        ));
    }
    Ok(Parsed {
        value: PageConfig {
            global: Arc::new(BTreeMap::new()),
            local: using_components,
        },
        issues,
    })
}

/// Discovers pages and components from `app_json`.
///
/// `root` must be an absolute, canonical project root. Overlay text wins over
/// disk for every file that discovery reads. Missing or malformed inputs are
/// reported in [`Discovery::issues`] instead of failing the whole project.
/// Mutable state shared while building pages.
struct PageContext<'a> {
    root: &'a Path,
    overlay: &'a Overlay,
    source: &'a ProjectPath,
    global_components: &'a Arc<BTreeMap<String, UsingComponent>>,
    budget: &'a mut JsonBudget,
}

/// Issue sink that enforces the project-wide issue cap at the push site.
///
/// Once the cap is reached, the sink appends one `Truncated` marker and drops
/// everything after it. This keeps peak memory bounded even when many files
/// each produce thousands of issues.
struct Issues {
    file: ProjectPath,
    items: Vec<DiscoveryIssue>,
    truncated: bool,
}

impl Issues {
    /// Creates an empty sink that attributes the final marker to `file`.
    const fn new(file: ProjectPath) -> Self {
        Self {
            file,
            items: Vec::new(),
            truncated: false,
        }
    }

    /// Pushes one issue, or the truncation marker once the cap is reached.
    fn push(&mut self, new_issue: DiscoveryIssue) {
        if self.truncated {
            return;
        }
        if self.items.len() >= MAX_ISSUES.saturating_sub(1) {
            self.items.push(issue(
                &self.file,
                DiscoveryIssueKind::Truncated,
                format!("issues truncated to {MAX_ISSUES} entries"),
            ));
            self.truncated = true;
            return;
        }
        self.items.push(new_issue);
    }

    /// Pushes each issue in order, respecting the cap.
    fn extend(&mut self, new_issues: impl IntoIterator<Item = DiscoveryIssue>) {
        for new_issue in new_issues {
            self.push(new_issue);
        }
    }

    /// Consumes the sink.
    fn into_vec(self) -> Vec<DiscoveryIssue> {
        self.items
    }

    /// Returns whether a truncation marker was recorded.
    fn contains_truncated(&self) -> bool {
        self.items
            .iter()
            .any(|issue| issue.kind == DiscoveryIssueKind::Truncated)
    }
}

/// Per-discovery byte budget for parsed JSON files.
///
/// The budget applies to every `app.json`/page/component JSON read, including
/// empty `usingComponents` files. Once exhausted, callers skip parsing (and
/// reads after the first over-budget file) but keep paths and continue
/// discovery. A single marker issue is reported.
struct JsonBudget {
    remaining: usize,
    truncated: bool,
    reported: bool,
}

impl JsonBudget {
    /// Creates a fresh budget for one discovery run.
    const fn new() -> Self {
        Self {
            remaining: MAX_DISCOVERY_JSON_BYTES,
            truncated: false,
            reported: false,
        }
    }

    /// Returns whether the budget has already been exhausted.
    const fn exhausted(&self) -> bool {
        self.truncated
    }

    /// Charges `bytes`; returns `false` when the budget is exhausted.
    const fn consume(&mut self, bytes: usize) -> bool {
        if self.truncated {
            return false;
        }
        if bytes > self.remaining {
            self.truncated = true;
            return false;
        }
        self.remaining -= bytes;
        true
    }

    /// Records the global truncation issue once.
    fn report(&mut self, issues: &mut Issues, file: &ProjectPath) {
        if self.truncated && !self.reported {
            issues.push(issue(
                file,
                DiscoveryIssueKind::Truncated,
                format!("discovery JSON budget of {MAX_DISCOVERY_JSON_BYTES} bytes exceeded"),
            ));
            self.reported = true;
        }
    }
}

/// Charges a JSON read attempt; returns `false` when no read is allowed.
fn charge_json_attempt(budget: &mut JsonBudget, attempt: Option<usize>) -> bool {
    if budget.exhausted() {
        return false;
    }
    attempt.is_none_or(|size| budget.consume(size))
}

/// Per-discovery byte budget for component scripts read for `properties`.
struct ComponentScriptBudget {
    remaining: usize,
    truncated: bool,
}

impl ComponentScriptBudget {
    /// Creates a fresh script budget.
    const fn new() -> Self {
        Self {
            remaining: MAX_COMPONENT_SCRIPT_BYTES,
            truncated: false,
        }
    }

    /// Returns whether another script can be read.
    const fn has_capacity(&self) -> bool {
        !self.truncated && self.remaining > 0
    }

    /// Charges script text bytes; returns `false` when it does not fit.
    const fn charge(&mut self, bytes: usize) -> bool {
        if self.truncated || bytes > self.remaining {
            self.remaining = 0;
            self.truncated = true;
            return false;
        }
        self.remaining -= bytes;
        true
    }
}

/// Discovers pages and components from `app_json`.
///
/// `root` must be an absolute, canonical project root. Overlay text wins over
/// disk for every file that discovery reads. Missing or malformed inputs are
/// reported in [`Discovery::issues`] instead of failing the whole project.
///
/// Results are bounded by [`MAX_PAGES`], [`MAX_COMPONENTS`],
/// [`MAX_ISSUES`], [`MAX_APP_ENTRIES`], [`MAX_TOTAL_LOCAL_COMPONENTS`],
/// [`MAX_DISCOVERY_JSON_BYTES`] and [`MAX_COMPONENT_SCRIPT_BYTES`].
#[must_use]
pub fn discover(root: &Path, overlay: &Overlay, app_json: &ProjectPath) -> Discovery {
    discover_with_limits(root, overlay, app_json, MAX_PAGES, MAX_COMPONENTS)
}

/// Reads and parses `app.json`, converting failures into issues.
fn load_app(
    root: &Path,
    overlay: &Overlay,
    app_json: &ProjectPath,
    issues: &mut Issues,
    budget: &mut JsonBudget,
) -> AppConfig {
    let attempt = json_attempt_size(root, overlay, app_json);
    if !charge_json_attempt(budget, attempt) {
        budget.report(issues, app_json);
        return AppConfig::default();
    }
    match overlay.read_text(root, app_json) {
        Ok(Some(text)) => {
            if text.len() > attempt.unwrap_or(0) {
                let extra = text.len() - attempt.unwrap_or(0);
                if !budget.consume(extra) {
                    budget.report(issues, app_json);
                    return AppConfig::default();
                }
            }
            match parse_app_config(app_json, &text) {
                Ok(parsed) => {
                    issues.extend(parsed.issues);
                    parsed.value
                }
                Err(error) => {
                    issues.push(issue(
                        app_json,
                        DiscoveryIssueKind::InvalidConfig,
                        error.to_string(),
                    ));
                    AppConfig::default()
                }
            }
        }
        Ok(None) => {
            issues.push(issue(
                app_json,
                DiscoveryIssueKind::MissingFile,
                "app.json was not found".to_owned(),
            ));
            AppConfig::default()
        }
        Err(error) => {
            budget.truncated = true;
            budget.report(issues, app_json);
            issues.push(issue(
                app_json,
                DiscoveryIssueKind::UnreadableFile,
                error.to_string(),
            ));
            AppConfig::default()
        }
    }
}

/// Discovery with explicit page/component limits, used by tests and future
/// incremental analysis. The public entry point uses [`MAX_PAGES`] and
/// [`MAX_COMPONENTS`].
fn discover_with_limits(
    root: &Path,
    overlay: &Overlay,
    app_json: &ProjectPath,
    max_pages: usize,
    max_components: usize,
) -> Discovery {
    let mut issues = Issues::new(app_json.clone());
    let mut budget = JsonBudget::new();
    let mut app = load_app(root, overlay, app_json, &mut issues, &mut budget);

    let mut reported_missing = BTreeSet::new();
    let global_components = Arc::new(std::mem::take(&mut app.using_components));
    report_missing_components(
        root,
        overlay,
        app_json,
        &global_components,
        &mut reported_missing,
        &mut issues,
    );

    let mut routes = Vec::new();
    routes.extend(app.pages.iter().cloned());
    for sub_package in &app.sub_packages {
        routes.extend(sub_package.pages.iter().cloned());
    }
    let pages_truncated = routes.len() > max_pages;
    routes.truncate(max_pages);

    let mut context = PageContext {
        root,
        overlay,
        source: app_json,
        global_components: &global_components,
        budget: &mut budget,
    };
    let mut local_budget = MAX_TOTAL_LOCAL_COMPONENTS;
    let mut local_truncated = false;
    let mut script_budget = ComponentScriptBudget::new();
    let mut pages = Vec::new();
    let mut seen_routes = BTreeSet::new();
    for route in routes {
        if !seen_routes.insert(route.clone()) {
            continue;
        }
        if let Some(page) = build_page(
            &mut context,
            &route,
            &mut reported_missing,
            &mut issues,
            &mut local_budget,
            &mut local_truncated,
        ) {
            pages.push(page);
        }
    }
    if pages_truncated {
        issues.push(issue(
            app_json,
            DiscoveryIssueKind::Truncated,
            format!("pages truncated to {max_pages} entries"),
        ));
    }
    if local_truncated {
        issues.push(issue(
            app_json,
            DiscoveryIssueKind::Truncated,
            format!(
                "page-local usingComponents truncated to {MAX_TOTAL_LOCAL_COMPONENTS} project entries"
            ),
        ));
    }

    let (components, components_truncated) = discover_components(
        &mut context,
        &pages,
        max_components,
        &mut reported_missing,
        &mut issues,
        &mut script_budget,
    );
    if components_truncated {
        issues.push(issue(
            app_json,
            DiscoveryIssueKind::Truncated,
            format!("components truncated to {max_components} entries"),
        ));
    }
    if script_budget.truncated {
        issues.push(issue(
            app_json,
            DiscoveryIssueKind::Truncated,
            format!("component scripts truncated to {MAX_COMPONENT_SCRIPT_BYTES} bytes"),
        ));
    }

    let truncated = issues.contains_truncated();
    Discovery {
        pages,
        components,
        issues: issues.into_vec(),
        truncated,
    }
}

/// Raw `app.json` shape.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAppConfig {
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default, alias = "subpackages")]
    sub_packages: Vec<RawSubPackage>,
    #[serde(default)]
    using_components: BTreeMap<String, String>,
}

/// Raw `subPackages` entry.
#[derive(Debug, Deserialize)]
struct RawSubPackage {
    root: String,
    #[serde(default)]
    pages: Vec<String>,
}

/// Raw page/component JSON shape.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPageConfig {
    #[serde(default)]
    using_components: BTreeMap<String, String>,
}

/// Parses JSON, stripping an optional byte-order mark.
fn parse_json<T>(file: &ProjectPath, text: &str) -> Result<T, DiscoveryError>
where
    T: for<'de> Deserialize<'de>,
{
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    serde_json::from_str(text).map_err(|error| DiscoveryError::Json {
        file: file.clone(),
        line: error.line(),
        column: error.column(),
        message: truncate_message(&error.to_string(), MAX_MESSAGE_BYTES).to_owned(),
    })
}

/// Resolves `usingComponents` entries, collecting per-entry issues.
///
/// At most `limit` entries are processed. Returns the map, the issues and
/// whether entries were dropped.
fn resolve_components(
    file: &ProjectPath,
    entries: BTreeMap<String, String>,
    limit: usize,
) -> (BTreeMap<String, UsingComponent>, Vec<DiscoveryIssue>, bool) {
    let truncated = entries.len() > limit;
    let mut issues = Vec::new();
    let mut resolved = BTreeMap::new();
    for (name, specifier) in entries.into_iter().take(limit) {
        if name.is_empty() {
            issues.push(issue(
                file,
                DiscoveryIssueKind::InvalidComponentRef,
                "component name must not be empty".to_owned(),
            ));
            continue;
        }
        if name.contains('\0') {
            issues.push(issue(
                file,
                DiscoveryIssueKind::InvalidComponentRef,
                "component name must not contain NUL".to_owned(),
            ));
            continue;
        }
        match classify_component(file, &specifier) {
            Ok((kind, component)) => {
                resolved.insert(
                    name.clone(),
                    UsingComponent {
                        name,
                        specifier,
                        kind,
                        component,
                    },
                );
            }
            Err(error) => issues.push(issue(
                file,
                DiscoveryIssueKind::InvalidComponentRef,
                format!(
                    "cannot resolve component `{}` from `{}`: {error}",
                    truncate_message(&name, MAX_MESSAGE_BYTES),
                    truncate_message(&specifier, MAX_MESSAGE_BYTES)
                ),
            )),
        }
    }
    (resolved, issues, truncated)
}

/// Classifies one component specifier and resolves local paths.
fn classify_component(
    file: &ProjectPath,
    specifier: &str,
) -> Result<(ComponentTargetKind, Option<ProjectPath>), files::PathError> {
    if specifier.starts_with("plugin://") {
        return Ok((ComponentTargetKind::Plugin, None));
    }
    if specifier.is_empty() {
        return Err(files::PathError::Empty);
    }
    if specifier.contains('\0') {
        return Err(files::PathError::Nul);
    }
    if !is_local_specifier(specifier) {
        return Ok((ComponentTargetKind::Package, None));
    }
    ProjectPath::resolve(file.parent().as_ref(), specifier)
        .map(|base| (ComponentTargetKind::Local, Some(base)))
}

/// Returns whether a specifier names a file inside this project.
fn is_local_specifier(specifier: &str) -> bool {
    let bytes = specifier.as_bytes();
    specifier.starts_with('/')
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with(".\\")
        || specifier.starts_with("..\\")
        || specifier.contains('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// Builds one page entry, recording missing files and script conflicts.
///
/// Page-local components are capped per page and by the shared project budget.
/// Once the project budget is exhausted, later page JSON files are only probed
/// for existence and are not parsed, so a large shared JSON cannot multiply
/// memory or CPU across thousands of pages.
fn build_page(
    context: &mut PageContext<'_>,
    route: &str,
    reported_missing: &mut BTreeSet<ProjectPath>,
    issues: &mut Issues,
    local_budget: &mut usize,
    local_truncated: &mut bool,
) -> Option<Page> {
    let root = context.root;
    let overlay = context.overlay;
    let source = context.source;
    let base = match ProjectPath::new(route) {
        Ok(base) => base,
        Err(error) => {
            issues.push(issue(
                source,
                DiscoveryIssueKind::InvalidRoute,
                format!(
                    "invalid page route `{}`: {error}",
                    truncate_message(route, MAX_MESSAGE_BYTES)
                ),
            ));
            return None;
        }
    };
    let wxml = match base.with_extension("wxml") {
        Ok(wxml) => wxml,
        Err(error) => {
            issues.push(issue(
                source,
                DiscoveryIssueKind::InvalidRoute,
                format!(
                    "invalid page template path for `{}`: {error}",
                    truncate_message(route, MAX_MESSAGE_BYTES)
                ),
            ));
            return None;
        }
    };
    if !exists(root, overlay, &wxml) {
        issues.push(issue(
            &wxml,
            DiscoveryIssueKind::MissingFile,
            format!(
                "page template `{}` was not found for route `{}`",
                truncate_message(wxml.as_str(), MAX_MESSAGE_BYTES),
                truncate_message(route, MAX_MESSAGE_BYTES)
            ),
        ));
    }

    let script = script::resolve_script(&base, |candidate| exists(root, overlay, candidate));
    if script.conflict {
        let conflict_file = script.script.clone().unwrap_or_else(|| base.clone());
        issues.push(issue(
            &conflict_file,
            DiscoveryIssueKind::ScriptConflict,
            format!(
                "both `{}.ts` and `{}.js` exist; using TypeScript",
                truncate_message(base.as_str(), MAX_MESSAGE_BYTES),
                truncate_message(base.as_str(), MAX_MESSAGE_BYTES)
            ),
        ));
    }

    let (json, local) = load_page_local(
        context,
        &base,
        reported_missing,
        issues,
        local_budget,
        local_truncated,
    );
    let config = PageConfig {
        global: Arc::clone(context.global_components),
        local,
    };

    Some(Page {
        route: route.to_owned(),
        wxml,
        script: script.script,
        json,
        config,
    })
}

/// Reads a page JSON file and applies per-page and project-wide local budgets.
///
/// When the project budget is exhausted, the file is only probed for existence
/// and its `usingComponents` are not parsed.
fn load_page_local(
    context: &mut PageContext<'_>,
    base: &ProjectPath,
    reported_missing: &mut BTreeSet<ProjectPath>,
    issues: &mut Issues,
    local_budget: &mut usize,
    local_truncated: &mut bool,
) -> (Option<ProjectPath>, BTreeMap<String, UsingComponent>) {
    let root = context.root;
    let overlay = context.overlay;
    let Ok(candidate) = base.with_extension("json") else {
        return (None, BTreeMap::new());
    };
    if *local_budget == 0 {
        let json = exists(root, overlay, &candidate).then_some(candidate);
        if json.is_some() {
            // The file may contain local components we deliberately do not
            // parse; report a single project-level truncation marker.
            *local_truncated = true;
        }
        return (json, BTreeMap::new());
    }
    let attempt = if context.budget.exhausted() {
        None
    } else {
        json_attempt_size(root, overlay, &candidate)
    };
    if !charge_json_attempt(context.budget, attempt) {
        context.budget.report(issues, &candidate);
        let json = exists(root, overlay, &candidate).then_some(candidate);
        return (json, BTreeMap::new());
    }
    match overlay.read_text(root, &candidate) {
        Ok(Some(text)) => {
            if text.len() > attempt.unwrap_or(0) {
                let extra = text.len() - attempt.unwrap_or(0);
                if !context.budget.consume(extra) {
                    context.budget.report(issues, &candidate);
                    return (Some(candidate), BTreeMap::new());
                }
            }
            match parse_page_config(&candidate, &text) {
                Ok(parsed) => {
                    issues.extend(parsed.issues);
                    let mut local = parsed.value.local;
                    let page_cap_hit = local.len() > MAX_PAGE_LOCAL_COMPONENTS;
                    if page_cap_hit {
                        local = local.into_iter().take(MAX_PAGE_LOCAL_COMPONENTS).collect();
                    }
                    let keep = local.len().min(*local_budget);
                    if keep < local.len() {
                        local = local.into_iter().take(keep).collect();
                        *local_truncated = true;
                    }
                    *local_budget -= keep;
                    if page_cap_hit {
                        *local_truncated = true;
                    }
                    report_missing_components(
                        root,
                        overlay,
                        &candidate,
                        &local,
                        reported_missing,
                        issues,
                    );
                    (Some(candidate), local)
                }
                Err(error) => {
                    issues.push(issue(
                        &candidate,
                        DiscoveryIssueKind::InvalidConfig,
                        error.to_string(),
                    ));
                    (Some(candidate), BTreeMap::new())
                }
            }
        }
        Ok(None) => (None, BTreeMap::new()),
        Err(error) => {
            context.budget.truncated = true;
            context.budget.report(issues, &candidate);
            issues.push(issue(
                &candidate,
                DiscoveryIssueKind::UnreadableFile,
                error.to_string(),
            ));
            (Some(candidate), BTreeMap::new())
        }
    }
}

/// Reports missing local component templates once per component base.
fn report_missing_components(
    root: &Path,
    overlay: &Overlay,
    source: &ProjectPath,
    components: &BTreeMap<String, UsingComponent>,
    reported: &mut BTreeSet<ProjectPath>,
    issues: &mut Issues,
) {
    for component in components.values() {
        let Some(base) = &component.component else {
            continue;
        };
        let Ok(wxml) = base.with_extension("wxml") else {
            continue;
        };
        if exists(root, overlay, &wxml) || !reported.insert(base.clone()) {
            continue;
        }
        issues.push(issue(
            source,
            DiscoveryIssueKind::MissingComponent,
            format!(
                "component `{}` (`{}`) has no template `{wxml}`",
                component.name, component.specifier
            ),
        ));
    }
}

/// Queue of component bases still to visit.
#[derive(Default)]
struct PendingComponents {
    queue: VecDeque<(ProjectPath, usize)>,
    queued: BTreeSet<ProjectPath>,
}

impl PendingComponents {
    /// Enqueues every resolved local component in `components`.
    fn push_from(&mut self, components: &BTreeMap<String, UsingComponent>, depth: usize) {
        for component in components.values() {
            let Some(base) = &component.component else {
                continue;
            };
            if self.queued.insert(base.clone()) {
                self.queue.push_back((base.clone(), depth));
            }
        }
    }

    /// Returns the next component to visit.
    fn pop(&mut self) -> Option<(ProjectPath, usize)> {
        self.queue.pop_front()
    }
}

/// Follows `usingComponents` transitively from the app and every page.
fn discover_components(
    context: &mut PageContext<'_>,
    pages: &[Page],
    max_components: usize,
    reported_missing: &mut BTreeSet<ProjectPath>,
    issues: &mut Issues,
    script_budget: &mut ComponentScriptBudget,
) -> (Vec<Component>, bool) {
    let mut pending = PendingComponents::default();
    pending.push_from(context.global_components, 0);
    for page in pages {
        pending.push_from(&page.config.local, 0);
    }

    let mut components = Vec::new();
    let mut truncated = false;
    while let Some((base, depth)) = pending.pop() {
        if components.len() >= max_components {
            truncated = true;
            break;
        }
        if let Some(component) = visit_component(
            context,
            &base,
            depth,
            &mut pending,
            reported_missing,
            issues,
            script_budget,
        ) {
            components.push(component);
        }
    }
    components.sort_by(|left, right| left.id.cmp(&right.id));
    (components, truncated)
}

/// Builds one component and enqueues its local children.
fn visit_component(
    context: &mut PageContext<'_>,
    base: &ProjectPath,
    depth: usize,
    pending: &mut PendingComponents,
    reported_missing: &mut BTreeSet<ProjectPath>,
    issues: &mut Issues,
    script_budget: &mut ComponentScriptBudget,
) -> Option<Component> {
    let root = context.root;
    let overlay = context.overlay;
    if depth > MAX_COMPONENT_DEPTH {
        issues.push(issue(
            base,
            DiscoveryIssueKind::InvalidComponentRef,
            format!("component reference depth exceeded at `{base}`"),
        ));
        return None;
    }
    let wxml = base.with_extension("wxml").ok()?;
    if !exists(root, overlay, &wxml) {
        return None;
    }

    let script = script::resolve_script(base, |candidate| exists(root, overlay, candidate));
    if script.conflict {
        let conflict_file = script.script.clone().unwrap_or_else(|| base.clone());
        issues.push(issue(
            &conflict_file,
            DiscoveryIssueKind::ScriptConflict,
            format!(
                "both `{}.ts` and `{}.js` exist; using TypeScript",
                truncate_message(base.as_str(), MAX_MESSAGE_BYTES),
                truncate_message(base.as_str(), MAX_MESSAGE_BYTES)
            ),
        ));
    }

    let json = load_component_json(context, base, depth, pending, reported_missing, issues);
    let properties =
        component_properties(root, overlay, script.script.as_ref(), issues, script_budget);

    Some(Component {
        id: base.clone(),
        wxml,
        script: script.script,
        json,
        properties,
    })
}

/// Parses a component JSON file and enqueues its local children.
fn load_component_json(
    context: &mut PageContext<'_>,
    base: &ProjectPath,
    depth: usize,
    pending: &mut PendingComponents,
    reported_missing: &mut BTreeSet<ProjectPath>,
    issues: &mut Issues,
) -> Option<ProjectPath> {
    let root = context.root;
    let overlay = context.overlay;
    let candidate = base.with_extension("json").ok()?;
    let attempt = if context.budget.exhausted() {
        None
    } else {
        json_attempt_size(root, overlay, &candidate)
    };
    if !charge_json_attempt(context.budget, attempt) {
        context.budget.report(issues, &candidate);
        return exists(root, overlay, &candidate).then_some(candidate);
    }
    match overlay.read_text(root, &candidate) {
        Ok(Some(text)) => {
            if text.len() > attempt.unwrap_or(0) {
                let extra = text.len() - attempt.unwrap_or(0);
                if !context.budget.consume(extra) {
                    context.budget.report(issues, &candidate);
                    return Some(candidate);
                }
            }
            match parse_page_config(&candidate, &text) {
                Ok(parsed) => {
                    issues.extend(parsed.issues);
                    report_missing_components(
                        root,
                        overlay,
                        &candidate,
                        &parsed.value.local,
                        reported_missing,
                        issues,
                    );
                    pending.push_from(&parsed.value.local, depth + 1);
                }
                Err(error) => issues.push(issue(
                    &candidate,
                    DiscoveryIssueKind::InvalidConfig,
                    error.to_string(),
                )),
            }
            Some(candidate)
        }
        Ok(None) => None,
        Err(error) => {
            context.budget.truncated = true;
            context.budget.report(issues, &candidate);
            issues.push(issue(
                &candidate,
                DiscoveryIssueKind::UnreadableFile,
                error.to_string(),
            ));
            Some(candidate)
        }
    }
}

/// Extracts `properties` roots from a component script.
///
/// The attempted script size is charged against
/// [`MAX_COMPONENT_SCRIPT_BYTES`] **before** reading, so unreadable or invalid
/// UTF-8 scripts cannot bypass the budget. If the text grows between metadata
/// and read (TOCTOU/overlay update), the difference is charged afterwards.
/// When the budget is exhausted, no further script is read and the global
/// truncation marker is recorded by the caller.
fn component_properties(
    root: &Path,
    overlay: &Overlay,
    script_path: Option<&ProjectPath>,
    issues: &mut Issues,
    script_budget: &mut ComponentScriptBudget,
) -> Vec<DataRoot> {
    let Some(script_path) = script_path else {
        return Vec::new();
    };
    if !script_budget.has_capacity() {
        script_budget.truncated = true;
        return Vec::new();
    }
    let attempt = path_attempt_size(root, overlay, script_path);
    if let Some(size) = attempt {
        if !script_budget.charge(size) {
            return Vec::new();
        }
    }
    match overlay.read_text(root, script_path) {
        Ok(Some(text)) => {
            if text.len() > attempt.unwrap_or(0) {
                let extra = text.len() - attempt.unwrap_or(0);
                if !script_budget.charge(extra) {
                    return Vec::new();
                }
            }
            script::extract_data_roots(script_path, &text)
                .into_iter()
                .filter(|root| root.kind == DataRootKind::ComponentProperties)
                .collect()
        }
        Ok(None) => Vec::new(),
        Err(error) => {
            // A failed read still consumed an attempt; make the truncation
            // state explicit so "budget untouched" cannot hide the failure.
            script_budget.truncated = true;
            issues.push(issue(
                script_path,
                DiscoveryIssueKind::UnreadableFile,
                error.to_string(),
            ));
            Vec::new()
        }
    }
}

/// Returns the likely byte size of a project file without reading contents.
fn path_attempt_size(root: &Path, overlay: &Overlay, path: &ProjectPath) -> Option<usize> {
    if let Some(text) = overlay.text(path) {
        return Some(text.len());
    }
    std::fs::metadata(root.join(path.as_str()))
        .ok()
        .and_then(|metadata| usize::try_from(metadata.len()).ok())
}

/// Returns the likely byte size of a JSON file without reading contents.
fn json_attempt_size(root: &Path, overlay: &Overlay, path: &ProjectPath) -> Option<usize> {
    path_attempt_size(root, overlay, path)
}

/// Creates an issue with a project-relative file.
pub(crate) fn issue(
    file: &ProjectPath,
    kind: DiscoveryIssueKind,
    message: String,
) -> DiscoveryIssue {
    DiscoveryIssue {
        file: file.clone(),
        kind,
        message: truncate_owned(message, MAX_MESSAGE_BYTES),
    }
}

/// Returns `true` when the path exists on disk or in the overlay.
fn exists(root: &Path, overlay: &Overlay, path: &ProjectPath) -> bool {
    overlay.contains(path) || files::is_file(root, path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "wxml-project-discovery-{}-{id}-{name}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn root(&self) -> PathBuf {
            self.path.canonicalize().expect("canonical root")
        }

        fn write(&self, relative: &str, text: &str) {
            let target = self.path.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(target, text).expect("write file");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    fn kinds(issues: &[DiscoveryIssue]) -> Vec<DiscoveryIssueKind> {
        issues.iter().map(|issue| issue.kind).collect()
    }

    #[test]
    fn parses_pages_subpackages_and_global_components() {
        let parsed = parse_app_config(
            &path("app.json"),
            r#"{
                "pages": ["pages/index/index", "pages/about/about"],
                "subPackages": [
                    {"root": "pkg", "pages": ["a/a", "b/b"]},
                    {"root": "other", "pages": ["c/c"]}
                ],
                "usingComponents": {
                    "card": "/components/card/card",
                    "remote": "plugin://my-plugin/remote",
                    "pkg-comp": "weui-miniprogram/button/button"
                }
            }"#,
        )
        .expect("parse");

        assert_eq!(
            parsed.value.pages,
            ["pages/index/index", "pages/about/about"]
        );
        assert_eq!(parsed.value.sub_packages.len(), 2);
        assert_eq!(parsed.value.sub_packages[0].root, path("pkg"));
        assert_eq!(parsed.value.sub_packages[0].pages, ["pkg/a/a", "pkg/b/b"]);
        assert_eq!(parsed.value.sub_packages[1].pages, ["other/c/c"]);

        let card = &parsed.value.using_components["card"];
        assert_eq!(card.kind, ComponentTargetKind::Local);
        assert_eq!(card.component, Some(path("components/card/card")));
        assert_eq!(
            parsed.value.using_components["remote"].kind,
            ComponentTargetKind::Plugin
        );
        assert_eq!(
            parsed.value.using_components["pkg-comp"].kind,
            ComponentTargetKind::Package
        );
        assert_eq!(parsed.issues, Vec::new());
    }

    #[test]
    fn parses_lowercase_subpackages_alias() {
        let parsed = parse_app_config(
            &path("app.json"),
            r#"{"pages": [], "subpackages": [{"root": "pkg", "pages": ["a"]}]}"#,
        )
        .expect("parse");
        assert_eq!(parsed.value.sub_packages[0].pages, ["pkg/a"]);
    }

    #[test]
    fn reports_invalid_routes_and_component_refs_without_failing() {
        let parsed = parse_app_config(
            &path("app.json"),
            r#"{
                "pages": ["../escape", "pages/ok/ok"],
                "subPackages": [{"root": "pkg", "pages": ["../escape", "a/a"]}],
                "usingComponents": {
                    "bad": "../../../outside/card",
                    "good": "./components/good/good"
                }
            }"#,
        )
        .expect("parse");
        assert_eq!(parsed.value.pages, ["pages/ok/ok"]);
        assert_eq!(parsed.value.sub_packages[0].pages, ["pkg/a/a"]);
        assert_eq!(
            parsed.value.using_components["good"].component,
            Some(path("components/good/good"))
        );
        assert!(!parsed.value.using_components.contains_key("bad"));
        assert_eq!(
            kinds(&parsed.issues),
            [
                DiscoveryIssueKind::InvalidRoute,
                DiscoveryIssueKind::InvalidRoute,
                DiscoveryIssueKind::InvalidComponentRef
            ]
        );
    }

    #[test]
    fn parses_page_local_components() {
        let parsed = parse_page_config(
            &path("pages/index/index.json"),
            r#"{"usingComponents": {"card": "../../components/card/card"}}"#,
        )
        .expect("parse");
        assert_eq!(
            parsed.value.local["card"].component,
            Some(path("components/card/card"))
        );
    }

    #[test]
    fn rejects_non_string_component_values() {
        let error = parse_page_config(
            &path("pages/index/index.json"),
            r#"{"usingComponents": {"card": 1}}"#,
        )
        .expect_err("invalid type");
        assert!(matches!(error, DiscoveryError::Json { .. }));
    }

    #[test]
    fn reports_json_errors_with_position() {
        let error =
            parse_app_config(&path("app.json"), "{\n  \"pages\": [\n}").expect_err("invalid json");
        let DiscoveryError::Json {
            file, line, column, ..
        } = error;
        assert_eq!(file, path("app.json"));
        assert_eq!(line, 3);
        assert!(column >= 1);
    }

    #[test]
    fn discovers_pages_scripts_and_page_configs() {
        let temp = TempDir::new("pages");
        temp.write(
            "app.json",
            r#"{
                "pages": ["pages/index/index", "pages/about/about"],
                "subPackages": [{"root": "pkg", "pages": ["a/a"]}],
                "usingComponents": {"global-card": "/components/card/card"}
            }"#,
        );
        temp.write("pages/index/index.wxml", "<view/>");
        temp.write("pages/index/index.ts", "Page({ data: { x: 1 } });");
        temp.write("pages/index/index.js", "Page({ data: { x: 2 } });");
        temp.write(
            "pages/index/index.json",
            r#"{"usingComponents": {"card": "../../components/card/card"}}"#,
        );
        temp.write("pages/about/about.wxml", "<view/>");
        temp.write("pages/about/about.js", "Page({});");
        temp.write("pkg/a/a.wxml", "<view/>");
        temp.write("components/card/card.wxml", "<view/>");
        temp.write(
            "components/card/card.ts",
            "Component({ properties: { title: { type: String } } });",
        );
        temp.write("components/card/card.json", "{}");

        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        let routes: Vec<&str> = discovery
            .pages
            .iter()
            .map(|page| page.route.as_str())
            .collect();
        assert_eq!(
            routes,
            ["pages/index/index", "pages/about/about", "pkg/a/a"]
        );

        let index = &discovery.pages[0];
        assert_eq!(index.wxml, path("pages/index/index.wxml"));
        assert_eq!(index.script, Some(path("pages/index/index.ts")));
        assert_eq!(index.json, Some(path("pages/index/index.json")));
        assert_eq!(
            index
                .config
                .get("card")
                .and_then(|entry| entry.component.as_ref()),
            Some(&path("components/card/card"))
        );
        assert_eq!(
            index
                .config
                .get("global-card")
                .and_then(|entry| entry.component.as_ref()),
            Some(&path("components/card/card"))
        );

        let about = &discovery.pages[1];
        assert_eq!(about.script, Some(path("pages/about/about.js")));
        assert_eq!(about.json, None);

        assert_eq!(discovery.components.len(), 1);
        let card = &discovery.components[0];
        assert_eq!(card.id, path("components/card/card"));
        assert_eq!(card.script, Some(path("components/card/card.ts")));
        assert_eq!(card.properties.len(), 1);
        assert_eq!(card.properties[0].kind, DataRootKind::ComponentProperties);
        assert_eq!(card.properties[0].text, "{ title: { type: String } }");
        assert!(!card.properties[0].range.is_empty());
    }

    #[test]
    fn follows_nested_and_cyclic_component_references() {
        let temp = TempDir::new("component-graph");
        temp.write(
            "app.json",
            r#"{"pages": ["pages/index/index"], "usingComponents": {"a": "/components/a/a"}}"#,
        );
        temp.write("pages/index/index.wxml", "<view/>");
        temp.write("components/a/a.wxml", "<view/>");
        temp.write(
            "components/a/a.json",
            r#"{"usingComponents": {"b": "../b/b"}}"#,
        );
        temp.write("components/b/b.wxml", "<view/>");
        temp.write(
            "components/b/b.json",
            r#"{"usingComponents": {"a": "../a/a"}}"#,
        );

        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        let ids: Vec<&str> = discovery
            .components
            .iter()
            .map(|component| component.id.as_str())
            .collect();
        assert_eq!(ids, ["components/a/a", "components/b/b"]);
    }

    #[test]
    fn reports_missing_and_escaping_components() {
        let temp = TempDir::new("missing-components");
        temp.write(
            "app.json",
            r#"{"pages": ["pages/index/index"], "usingComponents": {
                "missing": "/components/missing/missing",
                "outside": "../../../outside/card"
            }}"#,
        );
        temp.write("pages/index/index.wxml", "<view/>");
        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(
            kinds(&discovery.issues),
            [
                DiscoveryIssueKind::InvalidComponentRef,
                DiscoveryIssueKind::MissingComponent
            ]
        );
    }

    #[test]
    fn reports_missing_app_json_and_missing_page_files() {
        let temp = TempDir::new("missing-app");
        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.pages, Vec::new());
        assert_eq!(kinds(&discovery.issues), [DiscoveryIssueKind::MissingFile]);

        temp.write("app.json", r#"{"pages": ["pages/index/index"]}"#);
        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.pages.len(), 1);
        assert_eq!(kinds(&discovery.issues), [DiscoveryIssueKind::MissingFile]);
        assert!(discovery.issues.iter().all(|issue| {
            !issue
                .message
                .contains(temp.root().to_str().expect("utf8 root"))
        }));
    }

    #[test]
    fn malformed_app_json_is_an_issue_not_a_panic() {
        let temp = TempDir::new("malformed");
        temp.write("app.json", "{\n  \"pages\": [\n}");
        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.pages, Vec::new());
        assert_eq!(
            kinds(&discovery.issues),
            [DiscoveryIssueKind::InvalidConfig]
        );
        assert!(discovery.issues[0].message.contains("line 3"));
        assert!(
            !discovery.issues[0]
                .message
                .contains(temp.root().to_str().expect("utf8 root"))
        );
    }

    #[test]
    fn overlay_app_json_and_scripts_win_over_disk() {
        let temp = TempDir::new("overlay");
        temp.write("app.json", r#"{"pages": ["pages/a/a"]}"#);
        temp.write("pages/a/a.wxml", "<view/>");
        temp.write("pages/a/a.ts", "Page({ data: { old: true } });");
        temp.write("pages/b/b.wxml", "<view/>");

        let mut overlay = Overlay::new();
        overlay
            .set(&path("app.json"), r#"{"pages": ["pages/b/b"]}"#, 1)
            .expect("set app");
        overlay
            .set(&path("pages/b/b.ts"), "Page({ data: { fresh: true } });", 1)
            .expect("set script");

        let discovery = discover(&temp.root(), &overlay, &path("app.json"));
        assert_eq!(discovery.pages.len(), 1);
        assert_eq!(discovery.pages[0].route, "pages/b/b");
        assert_eq!(discovery.pages[0].script, Some(path("pages/b/b.ts")));
    }

    #[test]
    fn component_json_parse_errors_are_issues() {
        let temp = TempDir::new("bad-component-json");
        temp.write(
            "app.json",
            r#"{"pages": [], "usingComponents": {"a": "/components/a/a"}}"#,
        );
        temp.write("components/a/a.wxml", "<view/>");
        temp.write("components/a/a.json", "{");
        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.components.len(), 1);
        assert_eq!(
            kinds(&discovery.issues),
            [DiscoveryIssueKind::InvalidConfig]
        );
    }

    #[test]
    fn script_conflicts_are_reported() {
        let temp = TempDir::new("script-conflict");
        temp.write("app.json", r#"{"pages": ["pages/a/a"]}"#);
        temp.write("pages/a/a.wxml", "<view/>");
        temp.write("pages/a/a.ts", "Page({});");
        temp.write("pages/a/a.js", "Page({});");
        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(
            kinds(&discovery.issues),
            [DiscoveryIssueKind::ScriptConflict]
        );
        assert_eq!(discovery.pages[0].script, Some(path("pages/a/a.ts")));
    }

    #[test]
    fn pages_share_global_components_and_local_overrides_win() {
        let temp = TempDir::new("shared-globals");
        temp.write(
            "app.json",
            r#"{
                "pages": ["pages/a/a", "pages/b/b"],
                "usingComponents": {"shared": "/components/global/global"}
            }"#,
        );
        temp.write("pages/a/a.wxml", "<view/>");
        temp.write("pages/b/b.wxml", "<view/>");
        temp.write(
            "pages/a/a.json",
            r#"{"usingComponents": {"shared": "../../components/local/local"}}"#,
        );
        temp.write("components/global/global.wxml", "<view/>");
        temp.write("components/local/local.wxml", "<view/>");

        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.pages.len(), 2);
        assert!(Arc::ptr_eq(
            &discovery.pages[0].config.global,
            &discovery.pages[1].config.global
        ));
        assert_eq!(
            discovery.pages[0]
                .config
                .get("shared")
                .map(|entry| &entry.component),
            Some(&Some(path("components/local/local")))
        );
        assert_eq!(
            discovery.pages[1]
                .config
                .get("shared")
                .map(|entry| &entry.component),
            Some(&Some(path("components/global/global")))
        );
        assert_eq!(discovery.pages[0].config.local.len(), 1);
        assert_eq!(discovery.pages[0].config.global.len(), 1);
        let effective: Vec<&str> = discovery.pages[0]
            .config
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(effective, ["shared"]);
    }

    #[test]
    fn normalizes_and_deduplicates_page_routes() {
        let temp = TempDir::new("normalized-routes");
        temp.write(
            "app.json",
            r#"{"pages": ["./pages/a/a", "pages\\a\\a", "pages/b/b"]}"#,
        );
        temp.write("pages/a/a.wxml", "<view/>");
        temp.write("pages/b/b.wxml", "<view/>");

        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        let routes: Vec<&str> = discovery
            .pages
            .iter()
            .map(|page| page.route.as_str())
            .collect();
        assert_eq!(routes, ["pages/a/a", "pages/b/b"]);
    }

    #[test]
    fn truncates_app_entries_at_the_budget() {
        let routes: Vec<String> = (0..=MAX_APP_ENTRIES)
            .map(|index| format!("\"pages/p{index}/p{index}\""))
            .collect();
        let json = format!("{{\"pages\": [{}]}}", routes.join(","));

        let parsed = parse_app_config(&path("app.json"), &json).expect("parse");
        assert_eq!(parsed.value.pages.len(), MAX_APP_ENTRIES);
        assert_eq!(
            parsed.issues.last().map(|issue| issue.kind),
            Some(DiscoveryIssueKind::Truncated)
        );
    }

    #[test]
    fn caps_the_issue_list() {
        let temp = TempDir::new("issue-cap");
        let routes: Vec<String> = (0..MAX_ISSUES + 200)
            .map(|index| format!("\"../invalid{index}\""))
            .collect();
        temp.write(
            "app.json",
            &format!("{{\"pages\": [{}]}}", routes.join(",")),
        );

        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.issues.len(), MAX_ISSUES);
        assert_eq!(
            discovery.issues.last().map(|issue| issue.kind),
            Some(DiscoveryIssueKind::Truncated)
        );
        assert_eq!(discovery.issues[0].kind, DiscoveryIssueKind::InvalidRoute);
    }

    #[test]
    fn discovery_limits_pages_and_components() {
        let temp = TempDir::new("result-limits");
        temp.write(
            "app.json",
            r#"{
                "pages": ["pages/a/a", "pages/b/b", "pages/c/c"],
                "usingComponents": {
                    "one": "/components/one/one",
                    "two": "/components/two/two",
                    "three": "/components/three/three"
                }
            }"#,
        );
        for page in ["a", "b", "c"] {
            temp.write(&format!("pages/{page}/{page}.wxml"), "<view/>");
        }
        for component in ["one", "two", "three"] {
            temp.write(
                &format!("components/{component}/{component}.wxml"),
                "<view/>",
            );
        }

        let discovery =
            discover_with_limits(&temp.root(), &Overlay::new(), &path("app.json"), 2, 2);
        assert_eq!(discovery.pages.len(), 2);
        assert_eq!(discovery.components.len(), 2);
        assert!(discovery.truncated);
        assert_eq!(
            discovery
                .issues
                .iter()
                .filter(|issue| issue.kind == DiscoveryIssueKind::Truncated)
                .count(),
            2
        );
    }

    #[test]
    fn oversized_script_is_reported_and_skipped() {
        let temp = TempDir::new("oversized-script");
        temp.write(
            "app.json",
            r#"{"pages": [], "usingComponents": {"big": "/components/big/big"}}"#,
        );
        temp.write("components/big/big.wxml", "<view/>");
        let script = temp.path.join("components/big/big.ts");
        fs::create_dir_all(script.parent().expect("parent")).expect("create dir");
        let file = fs::File::create(&script).expect("create script");
        file.set_len(files::MAX_FILE_BYTES + 1).expect("set length");

        let discovery = discover(&temp.root(), &Overlay::new(), &path("app.json"));
        assert_eq!(discovery.components.len(), 1);
        assert_eq!(discovery.components[0].properties, Vec::new());
        assert_eq!(
            kinds(&discovery.issues),
            [
                DiscoveryIssueKind::UnreadableFile,
                DiscoveryIssueKind::Truncated
            ]
        );
        assert!(discovery.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn caps_total_local_components_across_symlinked_page_json() {
        use std::os::unix::fs::symlink;
        use std::time::{Duration, Instant};

        const PAGES: usize = 1_000;
        const ENTRIES: usize = 10_000;

        let temp = TempDir::new("local-budget");
        let pages: Vec<String> = (0..PAGES)
            .map(|index| format!("\"pages/p{index}/p{index}\""))
            .collect();
        temp.write("app.json", &format!("{{\"pages\": [{}]}}", pages.join(",")));

        let entries: Vec<String> = (0..ENTRIES)
            .map(|index| format!("\"c{index}\": \"/components/shared/shared\""))
            .collect();
        temp.write(
            "shared.json",
            &format!("{{\"usingComponents\": {{{}}}}}", entries.join(",")),
        );
        temp.write("components/shared/shared.wxml", "<view/>");

        let shared = temp.path.join("shared.json");
        for index in 0..PAGES {
            temp.write(&format!("pages/p{index}/p{index}.wxml"), "<view/>");
            symlink(
                &shared,
                temp.path.join(format!("pages/p{index}/p{index}.json")),
            )
            .expect("symlink page json");
        }

        let started = Instant::now();
        let project = crate::Project::open(temp.path.clone()).expect("open");
        let elapsed = started.elapsed();

        assert_eq!(project.pages().len(), PAGES);
        let total: usize = project
            .pages()
            .iter()
            .map(|page| page.config.local.len())
            .sum();
        assert_eq!(total, MAX_TOTAL_LOCAL_COMPONENTS);
        assert_eq!(
            project.pages()[0].config.local.len(),
            MAX_PAGE_LOCAL_COMPONENTS
        );
        assert!(
            project
                .issues()
                .iter()
                .any(|issue| issue.kind == DiscoveryIssueKind::Truncated)
        );
        assert!(elapsed < Duration::from_secs(5), "elapsed={elapsed:?}");
    }
}
