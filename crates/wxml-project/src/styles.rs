//! Project-level WXSS/SCSS class index.
//!
//! The index answers three questions without rescanning the whole project for
//! every request:
//!
//! - which classes apply to one WXML view (page/component style, `app.wxss`
//!   and explicitly configured global styles, including static `@import` /
//!   `@use` / `@forward` chains);
//! - where one class is defined (file, UTF-8 byte range and effective
//!   selector);
//! - which classes one style file declares.
//!
//! Parsing is deliberately conservative. Comments and strings are ignored,
//! literal class selectors in comma lists, compound selectors, nested SCSS and
//! provable `&` concatenations are recorded, while dynamic interpolation,
//! mixins and `@extend` only set an incomplete marker instead of claiming
//! classes the index cannot prove.
//!
//! Files are read through [`Project::read_optional`], so unsaved style buffers
//! win over disk and symlink escapes are rejected by the file layer. Every
//! read is bounded by [`StyleLimits`]; imports are bounded by depth, file count
//! and total bytes, and cycles are reported instead of followed.

mod parse;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use wxml_text::SourceRange;

use crate::Project;
use crate::files::{self, FileError, ProjectPath};
use crate::resources::is_remote_specifier;

pub use parse::parse_style_sheet;

/// Maximum bytes read from one style file by the index.
pub const MAX_STYLE_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Maximum bytes read for one view's style chain.
pub const MAX_STYLE_TOTAL_BYTES: usize = 16 * 1024 * 1024;

/// Maximum number of style files followed for one view.
pub const MAX_STYLE_FILES: usize = 256;

/// Maximum `@import`/`@use`/`@forward` depth.
pub const MAX_IMPORT_DEPTH: usize = 16;

/// Maximum classes kept for one style sheet.
pub const MAX_STYLE_CLASSES: usize = 20_000;

/// Maximum classes merged for one view across all style files.
pub const MAX_VIEW_CLASSES: usize = 50_000;

/// Maximum issues kept for one style sheet or view.
pub const MAX_STYLE_ISSUES: usize = 64;

/// Longest class name recorded from a selector.
pub const MAX_CLASS_NAME_BYTES: usize = 128;

/// Longest documentation comment kept for a class.
pub const MAX_DOC_COMMENT_BYTES: usize = 512;

/// Number of parsed sheets kept before the cache is cleared.
pub const MAX_CACHED_SHEETS: usize = 256;

/// Maximum parsed bytes kept in the sheet cache.
pub const MAX_CACHED_STYLE_BYTES: usize = 32 * 1024 * 1024;

/// Budgets for one style index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StyleLimits {
    /// Maximum bytes of one style file.
    pub max_file_bytes: usize,
    /// Maximum bytes read for one view's style chain.
    pub max_total_bytes: usize,
    /// Maximum style files followed for one view.
    pub max_files: usize,
    /// Maximum import depth.
    pub max_import_depth: usize,
    /// Maximum classes kept per sheet.
    pub max_classes: usize,
}

impl Default for StyleLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: MAX_STYLE_FILE_BYTES,
            max_total_bytes: MAX_STYLE_TOTAL_BYTES,
            max_files: MAX_STYLE_FILES,
            max_import_depth: MAX_IMPORT_DEPTH,
            max_classes: MAX_STYLE_CLASSES,
        }
    }
}

/// Where a style file's classes apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[non_exhaustive]
pub enum StyleScope {
    /// The page's own style file.
    Page,
    /// A component's own style file.
    Component,
    /// `app.wxss`/`app.scss` next to `app.json`.
    AppGlobal,
    /// A style file listed in [`StyleConfig::global_styles`].
    ConfiguredGlobal,
    /// The view is not a discovered page or component.
    Unknown,
}

impl StyleScope {
    /// Returns a stable short label for hover text.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Page => "page style",
            Self::Component => "component style",
            Self::AppGlobal => "app global style",
            Self::ConfiguredGlobal => "configured global style",
            Self::Unknown => "local style",
        }
    }
}

/// File format of one style source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[non_exhaustive]
pub enum StyleFileKind {
    /// Plain Mini Program WXSS.
    Wxss,
    /// SCSS/Sass source.
    Scss,
    /// Plain CSS.
    Css,
}

/// One class occurrence in a style file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassToken {
    /// Class name without the leading dot.
    pub name: String,
    /// UTF-8 byte range of the class name (or `&` concatenation) in the file.
    pub range: SourceRange,
    /// Effective selector after conservative SCSS nesting expansion.
    pub selector: String,
    /// Block comment immediately before the enclosing rule, when present.
    pub comment: Option<String>,
}

/// How one style import was classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StyleImportKind {
    /// A local path (resolved later against the importing file).
    Local,
    /// A remote URL that is never fetched.
    Remote,
}

/// One static `@import`/`@use`/`@forward` statement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleImport {
    /// Specifier text without quotes.
    pub specifier: String,
    /// UTF-8 byte range of the specifier in the importing file.
    pub range: SourceRange,
    /// Local or remote.
    pub kind: StyleImportKind,
}

/// Category of a recoverable style-index problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StyleIssueKind {
    /// An imported file does not exist.
    MissingImport,
    /// The import chain contains a cycle.
    CyclicImport,
    /// A path would leave the project or Mini Program root.
    EscapesRoot,
    /// A remote import was skipped.
    RemoteImport,
    /// A file or chain exceeded a size budget.
    TooLarge,
    /// A list was truncated at a safety limit.
    Truncated,
    /// Dynamic SCSS constructs could add classes the index cannot prove.
    DynamicConstruct,
    /// A file could not be read.
    ReadError,
}

/// One recoverable style-index problem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleIssue {
    /// Project-relative file that produced the issue.
    pub file: ProjectPath,
    /// Issue category.
    pub kind: StyleIssueKind,
    /// Human-readable message; never contains absolute paths.
    pub message: String,
}

impl StyleIssue {
    /// Creates one issue.
    fn new(file: &ProjectPath, kind: StyleIssueKind, message: impl Into<String>) -> Self {
        Self {
            file: file.clone(),
            kind,
            message: message.into(),
        }
    }
}

/// Parsed facts for one style file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleSheet {
    /// Project-relative path.
    pub path: ProjectPath,
    /// Classes in source order.
    pub classes: Vec<ClassToken>,
    /// Static imports in source order.
    pub imports: Vec<StyleImport>,
    /// Recoverable parse/read problems.
    pub issues: Vec<StyleIssue>,
    /// `true` when no dynamic construct could add classes.
    pub complete: bool,
    /// First reason the sheet is incomplete.
    pub reason: Option<String>,
    /// Number of bytes parsed.
    pub bytes: usize,
}

impl StyleSheet {
    /// Creates an empty, complete sheet.
    fn new(path: &ProjectPath) -> Self {
        Self {
            path: path.clone(),
            classes: Vec::new(),
            imports: Vec::new(),
            issues: Vec::new(),
            complete: true,
            reason: None,
            bytes: 0,
        }
    }

    /// Returns whether the sheet contains no dynamic constructs.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

/// One style file that contributed classes to a view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleFileInfo {
    /// Project-relative path.
    pub path: ProjectPath,
    /// File format.
    pub kind: StyleFileKind,
    /// Application scope inherited from the chain root.
    pub scope: StyleScope,
    /// `true` when reached through an import instead of a chain root.
    pub imported: bool,
    /// Importing file, when `imported` is set.
    pub imported_from: Option<ProjectPath>,
}

/// One class definition with view-level precedence information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassDefinition {
    /// Class name without the leading dot.
    pub name: String,
    /// Defining file.
    pub file: ProjectPath,
    /// UTF-8 byte range of the class token in the defining file.
    pub range: SourceRange,
    /// Effective selector.
    pub selector: String,
    /// Documentation comment, when present.
    pub comment: Option<String>,
    /// Application scope.
    pub scope: StyleScope,
    /// `true` when the definition came through an import.
    pub imported: bool,
    /// Global order in the view's style chain (lower is more local).
    pub order: u32,
}

/// Merged class facts for one WXML view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleViewFacts {
    /// Owning template path.
    pub view: ProjectPath,
    /// Style files in chain order.
    pub files: Vec<StyleFileInfo>,
    /// Class definitions in precedence order.
    pub classes: Vec<ClassDefinition>,
    /// Recoverable issues from every file in the chain.
    pub issues: Vec<StyleIssue>,
    /// `true` when the chain is provably complete.
    pub complete: bool,
    /// First reason the chain is incomplete.
    pub reason: Option<String>,
}

impl StyleViewFacts {
    /// Returns definitions of `name`, most specific first.
    ///
    /// Local page/component rules win over app/global rules of the same
    /// specificity. Within one scope the later declaration wins, matching the
    /// order in which imported and local rules appear in the cascade.
    #[must_use]
    pub fn definitions(&self, name: &str) -> Vec<&ClassDefinition> {
        let mut definitions: Vec<&ClassDefinition> = self
            .classes
            .iter()
            .filter(|class| class.name == name)
            .collect();
        definitions.sort_by(|left, right| {
            scope_rank(left.scope)
                .cmp(&scope_rank(right.scope))
                .then_with(|| right.order.cmp(&left.order))
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| left.range.cmp(&right.range))
        });
        definitions
    }

    /// Returns the most local definition of `name`.
    #[must_use]
    pub fn primary(&self, name: &str) -> Option<&ClassDefinition> {
        self.definitions(name).into_iter().next()
    }

    /// Iterates classes declared in `file` in source order.
    pub fn classes_in_file<'a>(
        &'a self,
        file: &'a ProjectPath,
    ) -> impl Iterator<Item = &'a ClassDefinition> {
        self.classes.iter().filter(move |class| &class.file == file)
    }

    /// Returns unique class names in precedence order.
    #[must_use]
    pub fn class_names(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        let mut names = Vec::new();
        for class in &self.classes {
            if seen.insert(class.name.as_str()) {
                names.push(class.name.as_str());
            }
        }
        names
    }

    /// Returns whether no dynamic construct could add unknown classes.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

/// Extra style roots explicitly configured by the caller.
///
/// `app.wxss`/`app.scss` are always included. This list is for projects that
/// keep additional global styles (for example a shared theme) outside the
/// page's own style file. Paths are project-relative and are never guessed
/// from a fixed file name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StyleConfig {
    /// Additional global style files.
    pub global_styles: Vec<ProjectPath>,
}

impl StyleConfig {
    /// Creates a configuration with the given explicit global styles.
    #[must_use]
    pub fn with_global_styles(global_styles: impl IntoIterator<Item = ProjectPath>) -> Self {
        Self {
            global_styles: global_styles.into_iter().collect(),
        }
    }
}

impl Project {
    /// Returns whether a path is a style file the project may overlay.
    ///
    /// Only `.wxss`, `.scss` and `.css` files inside the source tree and
    /// outside excluded build directories are accepted.
    #[must_use]
    pub fn accepts_style_overlay(&self, path: &ProjectPath) -> bool {
        is_style_path(path)
            && self.contains_source(path)
            && !crate::sources::has_excluded_component(path)
    }

    /// Returns whether `path` is a style file outside excluded build
    /// directories.
    #[must_use]
    pub fn is_style_path(path: &ProjectPath) -> bool {
        is_style_path(path) && !crate::sources::has_excluded_component(path)
    }
}

/// Returns whether a path has a style extension.
fn is_style_path(path: &ProjectPath) -> bool {
    path.extension().is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "wxss" | "scss" | "css"
        )
    })
}

/// Cached style sheets and the view query built on top of them.
///
/// One index belongs to one open project. Call [`StyleIndex::invalidate`] after
/// a style file changes, or [`StyleIndex::invalidate_all`] when project
/// configuration or the active app root changes.
#[derive(Debug, Default)]
pub struct StyleIndex {
    limits: StyleLimits,
    sheets: BTreeMap<ProjectPath, Arc<StyleSheet>>,
    cached_bytes: usize,
}

impl StyleIndex {
    /// Creates an index with the default limits.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(StyleLimits::default())
    }

    /// Creates an index with explicit limits.
    #[must_use]
    pub const fn with_limits(limits: StyleLimits) -> Self {
        Self {
            limits,
            sheets: BTreeMap::new(),
            cached_bytes: 0,
        }
    }

    /// Returns the limits used by this index.
    #[must_use]
    pub const fn limits(&self) -> StyleLimits {
        self.limits
    }

    /// Drops the cached parse of one file.
    ///
    /// Returns whether the file was cached.
    pub fn invalidate(&mut self, path: &ProjectPath) -> bool {
        let Some(sheet) = self.sheets.remove(path) else {
            return false;
        };
        self.cached_bytes = self.cached_bytes.saturating_sub(sheet.bytes);
        true
    }

    /// Drops every cached parse.
    pub fn invalidate_all(&mut self) {
        self.sheets.clear();
        self.cached_bytes = 0;
    }

    /// Returns the number of cached sheets.
    #[must_use]
    pub fn cached_sheet_count(&self) -> usize {
        self.sheets.len()
    }

    /// Parses and caches one style file.
    ///
    /// Missing files produce an incomplete sheet with a read issue; callers
    /// normally check existence first.
    pub fn sheet(&mut self, project: &Project, path: &ProjectPath) -> Arc<StyleSheet> {
        if let Some(sheet) = self.sheets.get(path) {
            return Arc::clone(sheet);
        }
        let sheet = Arc::new(self.parse(project, path));
        if self.sheets.len() >= MAX_CACHED_SHEETS
            || self.cached_bytes.saturating_add(sheet.bytes) > MAX_CACHED_STYLE_BYTES
        {
            self.invalidate_all();
        }
        self.cached_bytes = self.cached_bytes.saturating_add(sheet.bytes);
        self.sheets.insert(path.clone(), Arc::clone(&sheet));
        sheet
    }

    /// Parses one file without touching the cache.
    fn parse(&self, project: &Project, path: &ProjectPath) -> StyleSheet {
        let mut sheet = StyleSheet::new(path);
        match project.read_optional(path) {
            Ok(Some(text)) => {
                sheet.bytes = text.len();
                if text.len() > self.limits.max_file_bytes {
                    sheet.issues.push(StyleIssue::new(
                        path,
                        StyleIssueKind::TooLarge,
                        format!(
                            "style file exceeds the {} byte index limit",
                            self.limits.max_file_bytes
                        ),
                    ));
                    sheet.complete = false;
                    sheet.reason = Some("style file exceeds the index size limit".to_owned());
                } else {
                    let parsed = parse_style_sheet(path, &text, self.limits.max_classes);
                    sheet.classes = parsed.classes;
                    sheet.imports = parsed.imports;
                    for issue in parsed.issues {
                        push_issue(&mut sheet.issues, issue);
                    }
                    sheet.complete = parsed.complete;
                    sheet.reason = parsed.reason;
                }
            }
            Ok(None) => {
                sheet.issues.push(StyleIssue::new(
                    path,
                    StyleIssueKind::MissingImport,
                    "style file does not exist",
                ));
                sheet.complete = false;
                sheet.reason = Some("style file does not exist".to_owned());
            }
            Err(FileError::EscapesRoot { .. }) => {
                sheet.issues.push(StyleIssue::new(
                    path,
                    StyleIssueKind::EscapesRoot,
                    "style file resolves outside the project root",
                ));
                sheet.complete = false;
                sheet.reason = Some("style file resolves outside the project root".to_owned());
            }
            Err(FileError::TooLarge { .. }) => {
                sheet.issues.push(StyleIssue::new(
                    path,
                    StyleIssueKind::TooLarge,
                    "style file exceeds the file size limit",
                ));
                sheet.complete = false;
                sheet.reason = Some("style file exceeds the file size limit".to_owned());
            }
            Err(_) => {
                sheet.issues.push(StyleIssue::new(
                    path,
                    StyleIssueKind::ReadError,
                    "style file could not be read",
                ));
                sheet.complete = false;
                sheet.reason = Some("style file could not be read".to_owned());
            }
        }
        sheet
    }

    /// Builds the class facts that apply to one WXML view.
    ///
    /// The chain is: the view's own `wxss`/`scss`, `app.wxss`/`app.scss` next
    /// to `app.json`, then every explicitly configured global style. Static
    /// imports are expanded depth-first in source order.
    pub fn view_facts(
        &mut self,
        project: &Project,
        view: &ProjectPath,
        config: &StyleConfig,
    ) -> StyleViewFacts {
        let scope = view_scope(project, view);
        let roots = style_roots(project, view, scope, config);
        let limits = self.limits;
        let mut collector = Collector {
            index: self,
            project,
            limits,
            files: Vec::new(),
            classes: Vec::new(),
            issues: Vec::new(),
            stack: Vec::new(),
            visited: BTreeSet::new(),
            total_bytes: 0,
            complete: true,
            reason: None,
        };
        for root in roots {
            collector.collect(&root.path, root.scope, true, 0, None);
        }
        StyleViewFacts {
            view: view.clone(),
            files: collector.files,
            classes: collector.classes,
            issues: collector.issues,
            complete: collector.complete,
            reason: collector.reason,
        }
    }
}

/// One chain root before parsing.
struct RootStyle {
    path: ProjectPath,
    scope: StyleScope,
}

/// Builds chain roots for one view.
fn style_roots(
    project: &Project,
    view: &ProjectPath,
    scope: StyleScope,
    config: &StyleConfig,
) -> Vec<RootStyle> {
    let mut roots = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |path: ProjectPath, scope: StyleScope| {
        if !is_style_path(&path) || !exists(project, &path) || !seen.insert(path.clone()) {
            return;
        }
        roots.push(RootStyle { path, scope });
    };
    for extension in ["wxss", "scss"] {
        if let Ok(path) = view.with_extension(extension) {
            push(path, scope);
        }
    }
    for name in ["app.wxss", "app.scss"] {
        if let Some(path) = app_style_path(project, name) {
            push(path, StyleScope::AppGlobal);
        }
    }
    for path in &config.global_styles {
        push(path.clone(), StyleScope::ConfiguredGlobal);
    }
    roots
}

/// Returns the app-root-relative style path for a fixed file name.
fn app_style_path(project: &Project, name: &str) -> Option<ProjectPath> {
    project.source_root().map_or_else(
        || ProjectPath::new(name).ok(),
        |root| ProjectPath::new(format!("{}/{name}", root.as_str())).ok(),
    )
}

/// Determines page/component scope from discovery.
fn view_scope(project: &Project, view: &ProjectPath) -> StyleScope {
    if project.pages().iter().any(|page| &page.wxml == view) {
        StyleScope::Page
    } else if project
        .components()
        .iter()
        .any(|component| &component.wxml == view)
    {
        StyleScope::Component
    } else {
        StyleScope::Unknown
    }
}

/// Cascade priority of a scope; lower ranks override higher ranks.
const fn scope_rank(scope: StyleScope) -> u8 {
    match scope {
        StyleScope::Page | StyleScope::Component | StyleScope::Unknown => 0,
        StyleScope::AppGlobal => 1,
        StyleScope::ConfiguredGlobal => 2,
    }
}

/// Returns whether a style file exists on disk or in the overlay.
fn exists(project: &Project, path: &ProjectPath) -> bool {
    project.overlay().contains(path) || files::is_file(project.root(), path)
}

/// Returns the format of a style path.
fn style_file_kind(path: &ProjectPath) -> Option<StyleFileKind> {
    match path.extension().map(str::to_ascii_lowercase).as_deref() {
        Some("wxss") => Some(StyleFileKind::Wxss),
        Some("scss") => Some(StyleFileKind::Scss),
        Some("css") => Some(StyleFileKind::Css),
        _ => None,
    }
}

/// Accumulates one view's import chain.
struct Collector<'a> {
    index: &'a mut StyleIndex,
    project: &'a Project,
    limits: StyleLimits,
    files: Vec<StyleFileInfo>,
    classes: Vec<ClassDefinition>,
    issues: Vec<StyleIssue>,
    stack: Vec<ProjectPath>,
    visited: BTreeSet<ProjectPath>,
    total_bytes: usize,
    complete: bool,
    reason: Option<String>,
}

impl Collector<'_> {
    /// Marks the chain incomplete only once.
    fn mark_incomplete(&mut self, reason: impl Into<String>) {
        if self.complete {
            self.complete = false;
            self.reason = Some(reason.into());
        }
    }

    /// Records one issue within the view budget.
    fn issue(&mut self, file: &ProjectPath, kind: StyleIssueKind, message: impl Into<String>) {
        push_issue(&mut self.issues, StyleIssue::new(file, kind, message));
    }

    /// Follows one file and its imports.
    fn collect(
        &mut self,
        path: &ProjectPath,
        scope: StyleScope,
        is_root: bool,
        depth: usize,
        imported_from: Option<&ProjectPath>,
    ) {
        if self.files.len() >= self.limits.max_files {
            self.mark_incomplete("style file budget reached");
            self.issue(path, StyleIssueKind::Truncated, "style file budget reached");
            return;
        }
        if depth > self.limits.max_import_depth {
            self.mark_incomplete("style import depth budget reached");
            self.issue(
                path,
                StyleIssueKind::Truncated,
                "style import depth budget reached",
            );
            return;
        }
        if self.stack.iter().any(|open| open == path) {
            self.mark_incomplete("circular style import");
            self.issue(path, StyleIssueKind::CyclicImport, "circular style import");
            return;
        }
        if self.visited.contains(path) {
            return;
        }
        let sheet = self.index.sheet(self.project, path);
        if self
            .total_bytes
            .saturating_add(sheet.bytes)
            .gt(&self.limits.max_total_bytes)
        {
            self.mark_incomplete("style byte budget reached");
            self.issue(path, StyleIssueKind::TooLarge, "style byte budget reached");
            return;
        }
        self.total_bytes = self.total_bytes.saturating_add(sheet.bytes);
        self.visited.insert(path.clone());
        for issue in &sheet.issues {
            push_issue(&mut self.issues, issue.clone());
        }
        if !sheet.complete {
            self.mark_incomplete(
                sheet
                    .reason
                    .clone()
                    .unwrap_or_else(|| "style file is not fully indexed".to_owned()),
            );
        }
        if let Some(kind) = style_file_kind(path) {
            self.files.push(StyleFileInfo {
                path: path.clone(),
                kind,
                scope,
                imported: !is_root,
                imported_from: imported_from.cloned(),
            });
        }

        self.stack.push(path.clone());
        let mut class_index = 0_usize;
        let mut import_index = 0_usize;
        while class_index < sheet.classes.len() || import_index < sheet.imports.len() {
            if self.classes.len() >= MAX_VIEW_CLASSES {
                self.mark_incomplete("view class budget reached");
                self.issue(path, StyleIssueKind::Truncated, "view class budget reached");
                break;
            }
            let import_first = sheet.imports.get(import_index).is_some_and(|import| {
                sheet
                    .classes
                    .get(class_index)
                    .is_none_or(|class| import.range.start() < class.range.start())
            });
            if import_first {
                let import = &sheet.imports[import_index];
                import_index += 1;
                self.follow_import(path, import, scope, depth);
                continue;
            }
            let class = &sheet.classes[class_index];
            class_index += 1;
            let order = u32::try_from(self.classes.len()).unwrap_or(u32::MAX);
            self.classes.push(ClassDefinition {
                name: class.name.clone(),
                file: path.clone(),
                range: class.range,
                selector: class.selector.clone(),
                comment: class.comment.clone(),
                scope,
                imported: !is_root,
                order,
            });
        }
        self.stack.pop();
    }

    /// Resolves and follows one import statement.
    fn follow_import(
        &mut self,
        from: &ProjectPath,
        import: &StyleImport,
        scope: StyleScope,
        depth: usize,
    ) {
        match import.kind {
            StyleImportKind::Remote => {
                self.mark_incomplete("remote style import is not fetched");
                self.issue(
                    from,
                    StyleIssueKind::RemoteImport,
                    format!("remote import `{}` is not fetched", import.specifier),
                );
            }
            StyleImportKind::Local => match resolve_import(self.project, from, &import.specifier) {
                ImportTarget::Found(target) => {
                    self.collect(&target, scope, false, depth.saturating_add(1), Some(from));
                }
                ImportTarget::Remote => {
                    self.mark_incomplete("remote style import is not fetched");
                    self.issue(
                        from,
                        StyleIssueKind::RemoteImport,
                        format!("remote import `{}` is not fetched", import.specifier),
                    );
                }
                ImportTarget::Missing => {
                    self.mark_incomplete("style import target is missing");
                    self.issue(
                        from,
                        StyleIssueKind::MissingImport,
                        format!("import `{}` was not found", import.specifier),
                    );
                }
                ImportTarget::Escapes => {
                    self.mark_incomplete("style import leaves the project root");
                    self.issue(
                        from,
                        StyleIssueKind::EscapesRoot,
                        format!(
                            "import `{}` resolves outside the project root",
                            import.specifier
                        ),
                    );
                }
            },
        }
    }
}

/// Result of resolving one import specifier.
enum ImportTarget {
    Found(ProjectPath),
    Missing,
    Remote,
    Escapes,
}

/// Resolves an import to an existing local style file.
fn resolve_import(project: &Project, from: &ProjectPath, specifier: &str) -> ImportTarget {
    if is_remote_specifier(specifier) {
        return ImportTarget::Remote;
    }
    if specifier.contains('#') || specifier.contains('?') {
        return ImportTarget::Escapes;
    }
    let trimmed = specifier.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return ImportTarget::Missing;
    }
    let base = from.parent();
    let resolved = if trimmed.starts_with('/') {
        let rest = trimmed.trim_start_matches('/');
        let joined = project.source_root().as_ref().map_or_else(
            || rest.to_owned(),
            |root| format!("{}/{rest}", root.as_str()),
        );
        ProjectPath::new(joined)
    } else {
        ProjectPath::resolve(base.as_ref(), trimmed)
    };
    let Ok(path) = resolved else {
        return ImportTarget::Escapes;
    };
    if !project_contains(project, &path) {
        return ImportTarget::Escapes;
    }
    for candidate in import_candidates(&path, from) {
        if exists(project, &candidate) {
            return ImportTarget::Found(candidate);
        }
    }
    ImportTarget::Missing
}

/// Returns whether the project owns a style path.
fn project_contains(project: &Project, path: &ProjectPath) -> bool {
    project.contains_source(path)
}

/// Builds deterministic import candidates for one specifier path.
///
/// When the extension is omitted the importing format wins: an SCSS file looks
/// for `.scss` (including `_partial.scss`) before `.wxss`, and a WXSS file
/// looks for `.wxss` first. Directory imports try index files last.
fn import_candidates(path: &ProjectPath, from: &ProjectPath) -> Vec<ProjectPath> {
    let mut candidates = Vec::new();
    if path.extension().is_some() {
        candidates.push(path.clone());
        return candidates;
    }
    let ordered: [&str; 3] = match from.extension().map(str::to_ascii_lowercase).as_deref() {
        Some("scss") => ["scss", "wxss", "css"],
        Some("wxss") => ["wxss", "scss", "css"],
        _ => ["css", "wxss", "scss"],
    };
    let name = path.file_name();
    let parent = path.parent();
    for extension in ordered {
        if let Ok(candidate) = path.with_extension(extension) {
            candidates.push(candidate);
        }
        if let Some(parent) = parent.as_ref() {
            if let Ok(candidate) =
                ProjectPath::new(format!("{}/_{name}.{extension}", parent.as_str()))
            {
                candidates.push(candidate);
            }
        }
    }
    for extension in ordered {
        if let Ok(candidate) = ProjectPath::new(format!("{}/index.{extension}", path.as_str())) {
            candidates.push(candidate);
        }
        if let Ok(candidate) = ProjectPath::new(format!("{}/_index.{extension}", path.as_str())) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// Adds one issue within the per-view budget.
fn push_issue(issues: &mut Vec<StyleIssue>, issue: StyleIssue) {
    if issues.len() < MAX_STYLE_ISSUES {
        issues.push(issue);
    }
}

/// Optional style/resource existence diagnostics.
///
/// The default is disabled. [`StyleDiagnosticsPolicy::enabled`] is intended for
/// a caller that has a complete index; undefined classes are never reported
/// from a partial index and remote/dynamic resource values are never reported
/// as missing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StyleDiagnosticsPolicy {
    /// Whether diagnostics may be produced.
    pub enabled: bool,
}

impl StyleDiagnosticsPolicy {
    /// Creates an enabled policy.
    #[must_use]
    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    /// Returns class names used by the template but not defined by the index.
    ///
    /// The result is empty when the policy is disabled or the index is
    /// incomplete. Values containing mustaches or invalid class characters are
    /// skipped rather than guessed.
    #[must_use]
    pub fn undefined_classes(&self, facts: &StyleViewFacts, used: &[String]) -> Vec<String> {
        if !self.enabled || !facts.complete {
            return Vec::new();
        }
        let mut undefined = BTreeSet::new();
        for name in used {
            if !is_static_class_name(name) {
                continue;
            }
            if facts.primary(name).is_none() {
                undefined.insert(name.clone());
            }
        }
        undefined.into_iter().collect()
    }

    /// Returns local resource values that do not name an existing file.
    ///
    /// Remote schemes, mustaches, empty values, directories and paths outside
    /// the project are skipped; only a provably missing local file is returned.
    #[must_use]
    pub fn missing_resources(
        &self,
        project: &Project,
        wxml: &ProjectPath,
        used: &[String],
    ) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut missing = BTreeSet::new();
        for value in used {
            let trimmed = value.trim();
            if trimmed.is_empty() || trimmed.contains("{{") || trimmed.contains("}}") {
                continue;
            }
            if matches!(
                project.resolve_resource(wxml, trimmed),
                crate::resources::ResourceResolution::Missing
            ) {
                missing.insert(trimmed.to_owned());
            }
        }
        missing.into_iter().collect()
    }
}

/// Returns whether `name` is a static class name without dynamic syntax.
fn is_static_class_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CLASS_NAME_BYTES
        && !name.contains("{{")
        && !name.contains("}}")
        && !name.contains('#')
        && name.bytes().all(parse::is_class_name_byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid path")
    }

    #[test]
    fn import_candidates_prefer_the_importing_format() {
        let from_scss = path("pages/index/index.scss");
        let candidates = import_candidates(&path("shared/theme"), &from_scss);
        assert_eq!(
            candidates.first().map(ProjectPath::as_str),
            Some("shared/theme.scss")
        );
        let scss_partial = candidates
            .iter()
            .position(|path| path.as_str() == "shared/_theme.scss")
            .expect("partial scss candidate");
        let wxss = candidates
            .iter()
            .position(|path| path.as_str() == "shared/theme.wxss")
            .expect("wxss candidate");
        assert!(scss_partial < wxss, "{candidates:?}");
        assert!(
            candidates
                .iter()
                .any(|path| path.as_str() == "shared/theme/index.scss")
        );

        let from_wxss = path("pages/index/index.wxss");
        let candidates = import_candidates(&path("shared/theme"), &from_wxss);
        assert_eq!(
            candidates.first().map(ProjectPath::as_str),
            Some("shared/theme.wxss")
        );

        let exact = import_candidates(&path("shared/theme.scss"), &from_wxss);
        assert_eq!(exact, vec![path("shared/theme.scss")]);
    }
}
