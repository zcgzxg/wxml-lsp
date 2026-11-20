//! Source-file collection for the type backend.
//!
//! The LSP layer gathers the project scripts that the Node/TypeScript backend
//! should see. Collection is bounded, overlay-aware, and never follows
//! symlinks. Unreadable or oversized entries become [`SourceIssue`] values
//! instead of failing the whole collection.

use std::collections::BTreeSet;
use std::path::Path;

use crate::files::{MAX_FILE_BYTES, ProjectPath};
use crate::{FileError, Project};

/// Directory names skipped by [`Project::collect_sources`].
///
/// Matching is ASCII case-insensitive and applies to any path component.
pub const EXCLUDED_DIRECTORIES: [&str; 9] = [
    "node_modules",
    ".git",
    "dist",
    "build",
    "miniprogram_npm",
    "coverage",
    ".cache",
    ".next",
    "out",
];

/// Maximum directory entries examined by [`Project::collect_sources`].
pub const MAX_SCANNED_ENTRIES: usize = 100_000;

/// Maximum number of [`SourceIssue`] entries returned by one collection.
///
/// When the limit is reached, one final [`SourceIssueKind::Truncated`] entry is
/// returned and scanning stops.
pub const MAX_SOURCE_ISSUES: usize = 1_000;

/// Script dialect inferred from the file extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SourceKind {
    /// TypeScript (`.ts`, `.mts`, `.cts`).
    Ts,
    /// TypeScript with JSX (`.tsx`).
    Tsx,
    /// JavaScript (`.js`, `.mjs`, `.cjs`).
    Js,
    /// JavaScript with JSX (`.jsx`).
    Jsx,
    /// TypeScript declaration (`.d.ts`, `.d.mts`, `.d.cts`).
    Dts,
}

impl SourceKind {
    /// Returns the stable lower-case name of the kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ts => "ts",
            Self::Tsx => "tsx",
            Self::Js => "js",
            Self::Jsx => "jsx",
            Self::Dts => "dts",
        }
    }
}

/// One collected source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFile {
    /// Project-relative path of the script.
    pub path: ProjectPath,
    /// Dialect inferred from the extension.
    pub kind: SourceKind,
    /// Effective text: overlay content when present, otherwise disk content.
    pub text: String,
    /// Overlay revision when the file is overlaid, otherwise `1`.
    ///
    /// Overlay revision `0` is valid and preserved verbatim; revisions are
    /// opaque counters supplied by the overlay writer.
    pub source_revision: u64,
}

/// Bounds applied to one [`Project::collect_sources`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLimits {
    /// Maximum number of collected files.
    pub max_files: usize,
    /// Maximum total bytes across all collected file texts.
    pub max_total_bytes: usize,
    /// Maximum size of one file. Values above the crate-wide 16 MiB read limit
    /// are additionally clamped to that limit.
    pub max_file_bytes: usize,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            max_files: 2048,
            max_total_bytes: 16 * 1024 * 1024,
            max_file_bytes: 16 * 1024 * 1024,
        }
    }
}

impl SourceLimits {
    /// Limits for a project whose boundary is a whole monorepo workspace.
    ///
    /// A workspace snapshot spans every configured app package and its
    /// workspace dependencies, so the single-app defaults are too small. The
    /// file and byte budgets still bound memory and startup work.
    #[must_use]
    pub const fn workspace() -> Self {
        Self {
            max_files: 8192,
            max_total_bytes: 64 * 1024 * 1024,
            max_file_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Why a source entry was skipped or collection stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SourceIssueKind {
    /// The file exceeds [`SourceLimits::max_file_bytes`] (or the crate-wide
    /// 16 MiB read limit) and was not collected.
    TooLarge,
    /// The file is a symlink, failed to read, or is not valid UTF-8.
    Unreadable,
    /// A file, byte, scanned-entry or issue limit was reached; remaining work
    /// stopped.
    Truncated,
}

/// One skipped source file or collection boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIssue {
    /// Project-relative path that produced the issue.
    pub path: ProjectPath,
    /// Issue category.
    pub kind: SourceIssueKind,
}

/// Result of one source collection pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceSet {
    /// Collected files, sorted by [`ProjectPath`].
    pub files: Vec<SourceFile>,
    /// `true` when a file, byte, scanned-entry or issue limit stopped
    /// collection.
    pub truncated: bool,
    /// Recoverable problems, including one `Truncated` marker when applicable.
    pub issues: Vec<SourceIssue>,
}

/// Issue collector with a project-wide cap and a single truncation marker.
struct SourceIssues {
    items: Vec<SourceIssue>,
    truncated: bool,
}

impl SourceIssues {
    /// Creates an empty issue collector.
    const fn new() -> Self {
        Self {
            items: Vec::new(),
            truncated: false,
        }
    }

    /// Pushes one issue, replacing the last slot with a marker at the cap.
    fn push(&mut self, new_issue: SourceIssue) {
        if self.truncated {
            return;
        }
        if self.items.len() >= MAX_SOURCE_ISSUES.saturating_sub(1) {
            self.items.push(SourceIssue {
                path: new_issue.path,
                kind: SourceIssueKind::Truncated,
            });
            self.truncated = true;
            return;
        }
        self.items.push(new_issue);
    }

    /// Adds the single truncation marker unless one already exists.
    fn mark_truncated(&mut self, path: ProjectPath) {
        if self.truncated {
            return;
        }
        self.push(SourceIssue {
            path,
            kind: SourceIssueKind::Truncated,
        });
        self.truncated = true;
    }

    /// Consumes the collector.
    fn into_vec(self) -> Vec<SourceIssue> {
        self.items
    }
}

impl Project {
    /// Returns whether an unsaved script buffer may become a project overlay.
    ///
    /// A path is accepted when it is a script the source collector would pick
    /// up (including `.d.ts`, `.d.mts` and `.d.cts`) and it has no
    /// [`EXCLUDED_DIRECTORIES`] component, so `node_modules` or `dist` buffers
    /// never change analysis.
    #[must_use]
    pub fn accepts_script_overlay(&self, path: &ProjectPath) -> bool {
        self.contains_source_root(path)
            && source_kind(path).is_some()
            && !has_excluded_component(path)
    }

    /// Returns whether an unsaved JSON buffer may become a project overlay.
    ///
    /// Only configuration discovery consumes is accepted: the root `app.json`,
    /// the root `tsconfig.json`, and the JSON of a discovered page or
    /// component. Arbitrary project JSON such as `package.json` is ignored.
    #[must_use]
    pub fn accepts_json_overlay(&self, path: &ProjectPath) -> bool {
        if path == &self.app_json || path == &self.tsconfig_path {
            return true;
        }
        if self.tsconfig.as_ref().is_some_and(|config| {
            &config.path == path || config.extends.iter().any(|extended| extended == path)
        }) {
            return true;
        }
        self.discovery
            .pages
            .iter()
            .any(|page| page.json.as_ref() == Some(path))
            || self
                .discovery
                .components
                .iter()
                .any(|component| component.json.as_ref() == Some(path))
    }

    /// Returns whether an unsaved buffer may become a project overlay.
    ///
    /// WXML templates are never overlaid here: their text lives in the LSP
    /// workspace snapshot instead.
    #[must_use]
    pub fn accepts_overlay(&self, path: &ProjectPath) -> bool {
        self.accepts_script_overlay(path)
            || self.accepts_wxs_overlay(path)
            || self.accepts_json_overlay(path)
    }

    /// Returns whether an unsaved WXS buffer may become a project overlay.
    ///
    /// WXS is not part of the script snapshot sent to Node; the static WXS
    /// analyzer reads it through [`Project::read`], so an overlay matters for
    /// hover/completion/definition and dependency invalidation.
    #[must_use]
    pub fn accepts_wxs_overlay(&self, path: &ProjectPath) -> bool {
        self.contains_source(path)
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wxs"))
            && !has_excluded_component(path)
    }

    /// Returns whether `path` is a page or component config that discovery has
    /// not seen yet (for example a JSON file created after startup).
    ///
    /// A config is recognizable by its sibling template. Such a file is never
    /// overlaid unless [`Self::accepts_json_overlay`] also returns `true`; it
    /// only triggers rediscovery.
    #[must_use]
    pub fn is_project_json(&self, path: &ProjectPath) -> bool {
        if !self.contains_source(path)
            || !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            || has_excluded_component(path)
        {
            return false;
        }
        path.with_extension("wxml").is_ok_and(|template| {
            self.overlay.contains(&template) || self.root.join(template.as_str()).is_file()
        })
    }

    /// Collects project scripts for the type backend.
    ///
    /// Only `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts`, `.mjs` and `.cjs`
    /// files are considered; `.d.ts` (and `.d.mts`/`.d.cts`) use
    /// [`SourceKind::Dts`]. Excluded directories are listed in
    /// [`EXCLUDED_DIRECTORIES`]. Hidden files and directories are otherwise
    /// collected: for example `.hidden/h.ts` is included.
    ///
    /// Symlinks are never followed and are reported as
    /// [`SourceIssueKind::Unreadable`] unless an overlay supplies text for
    /// them.
    ///
    /// Overlay text always wins over disk, and overlay files that do not exist
    /// on disk are included. Files are processed in sorted path order. When a
    /// file, total-byte, scanned-entry or issue limit is reached, collection
    /// stops with [`SourceSet::truncated`] and a single
    /// [`SourceIssueKind::Truncated`] entry. Scanning is additionally bounded
    /// by [`MAX_SCANNED_ENTRIES`] and issues by [`MAX_SOURCE_ISSUES`].
    #[must_use]
    pub fn collect_sources(&self, limits: SourceLimits) -> SourceSet {
        let mut issues = SourceIssues::new();
        let (candidates, scan_truncated) = collect_candidates(self, &mut issues);
        let max_file_bytes = limits
            .max_file_bytes
            .min(usize::try_from(MAX_FILE_BYTES).unwrap_or(usize::MAX));

        let mut files = Vec::new();
        let mut total = 0usize;
        let mut truncated = scan_truncated || issues.truncated;
        for path in candidates {
            if files.len() >= limits.max_files {
                issues.mark_truncated(path);
                truncated = true;
                break;
            }
            let Some(kind) = source_kind(&path) else {
                continue;
            };
            let revision = self.overlay().revision(&path);
            let text = if revision.is_some() {
                let Some(text) = self.overlay().text(&path) else {
                    continue;
                };
                text.to_owned()
            } else {
                if let Some(size) = disk_size(self.root(), &path) {
                    if size > u64::try_from(max_file_bytes).unwrap_or(u64::MAX) {
                        issues.push(SourceIssue {
                            path,
                            kind: SourceIssueKind::TooLarge,
                        });
                        if issues.truncated {
                            truncated = true;
                            break;
                        }
                        continue;
                    }
                }
                match self.read(&path) {
                    Ok(text) => text,
                    Err(error) => {
                        issues.push(SourceIssue {
                            path,
                            kind: source_issue_kind(&error),
                        });
                        if issues.truncated {
                            truncated = true;
                            break;
                        }
                        continue;
                    }
                }
            };
            if text.len() > max_file_bytes {
                issues.push(SourceIssue {
                    path,
                    kind: SourceIssueKind::TooLarge,
                });
                if issues.truncated {
                    truncated = true;
                    break;
                }
                continue;
            }
            let Some(new_total) = total.checked_add(text.len()) else {
                issues.mark_truncated(path);
                truncated = true;
                break;
            };
            if new_total > limits.max_total_bytes {
                issues.mark_truncated(path);
                truncated = true;
                break;
            }
            total = new_total;
            files.push(SourceFile {
                path,
                kind,
                text,
                source_revision: revision.unwrap_or(1),
            });
        }

        truncated |= issues.truncated;
        SourceSet {
            files,
            truncated,
            issues: issues.into_vec(),
        }
    }
}

/// Walks the project and returns sorted candidates plus a scan-truncation flag.
fn collect_candidates(
    project: &Project,
    issues: &mut SourceIssues,
) -> (BTreeSet<ProjectPath>, bool) {
    collect_candidates_with_limit(project, issues, MAX_SCANNED_ENTRIES)
}

/// Walks with an explicit entry limit, used by tests and future tuning.
fn collect_candidates_with_limit(
    project: &Project,
    issues: &mut SourceIssues,
    max_entries: usize,
) -> (BTreeSet<ProjectPath>, bool) {
    let mut candidates = BTreeSet::new();
    let starts: Vec<_> = if project.source_roots().is_empty() {
        vec![(project.root().to_path_buf(), None)]
    } else {
        project
            .source_roots()
            .iter()
            .map(|root| (project.root().join(root.as_str()), Some(root.clone())))
            .collect()
    };
    let mut stack = starts;
    let mut scanned = 0usize;
    while let Some((directory, relative)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            if let Some(path) = relative {
                issues.push(SourceIssue {
                    path,
                    kind: SourceIssueKind::Unreadable,
                });
                if issues.truncated {
                    return (candidates, true);
                }
            }
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.file_name().to_str().and_then(|name| {
                relative.as_ref().map_or_else(
                    || ProjectPath::new(name).ok(),
                    |parent| ProjectPath::resolve(Some(parent), name).ok(),
                )
            });
            scanned = scanned.saturating_add(1);
            if scanned > max_entries {
                if let Some(path) = child.or_else(|| relative.clone()) {
                    issues.mark_truncated(path);
                }
                return (candidates, true);
            }
            let Some(child) = child else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if is_excluded_dir(&name) {
                    continue;
                }
                stack.push((entry.path(), Some(child)));
            } else if file_type.is_symlink() {
                if source_kind(&child).is_some() && !project.overlay().contains(&child) {
                    issues.push(SourceIssue {
                        path: child,
                        kind: SourceIssueKind::Unreadable,
                    });
                    if issues.truncated {
                        return (candidates, true);
                    }
                }
            } else if file_type.is_file() && source_kind(&child).is_some() {
                candidates.insert(child);
            }
        }
    }
    for path in project.overlay().paths() {
        if project.contains_source_root(path)
            && source_kind(path).is_some()
            && !has_excluded_component(path)
        {
            candidates.insert(path.clone());
        }
    }
    (candidates, false)
}

/// Returns whether a directory name is excluded (ASCII case-insensitive).
fn is_excluded_dir(name: &str) -> bool {
    EXCLUDED_DIRECTORIES
        .iter()
        .any(|excluded| name.eq_ignore_ascii_case(excluded))
}

/// Returns whether any path component is an excluded directory.
pub(crate) fn has_excluded_component(path: &ProjectPath) -> bool {
    path.as_str().split('/').any(is_excluded_dir)
}

/// Infers the script kind from the extension.
fn source_kind(path: &ProjectPath) -> Option<SourceKind> {
    let name = path.file_name().to_ascii_lowercase();
    if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
        return Some(SourceKind::Dts);
    }
    let (_, extension) = name.rsplit_once('.')?;
    match extension {
        "ts" | "mts" | "cts" => Some(SourceKind::Ts),
        "tsx" => Some(SourceKind::Tsx),
        "js" | "mjs" | "cjs" => Some(SourceKind::Js),
        "jsx" => Some(SourceKind::Jsx),
        _ => None,
    }
}

/// Returns the on-disk size without reading file contents.
fn disk_size(root: &Path, path: &ProjectPath) -> Option<u64> {
    std::fs::metadata(root.join(path.as_str()))
        .ok()
        .map(|metadata| metadata.len())
}

/// Maps a read error to a source issue kind.
const fn source_issue_kind(error: &FileError) -> SourceIssueKind {
    match error {
        FileError::TooLarge { .. } => SourceIssueKind::TooLarge,
        _ => SourceIssueKind::Unreadable,
    }
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
                "wxml-project-sources-{}-{id}-{name}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
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

    #[test]
    fn classifies_supported_extensions() {
        assert_eq!(source_kind(&path("a.ts")), Some(SourceKind::Ts));
        assert_eq!(source_kind(&path("a.mts")), Some(SourceKind::Ts));
        assert_eq!(source_kind(&path("a.cts")), Some(SourceKind::Ts));
        assert_eq!(source_kind(&path("a.tsx")), Some(SourceKind::Tsx));
        assert_eq!(source_kind(&path("a.js")), Some(SourceKind::Js));
        assert_eq!(source_kind(&path("a.mjs")), Some(SourceKind::Js));
        assert_eq!(source_kind(&path("a.cjs")), Some(SourceKind::Js));
        assert_eq!(source_kind(&path("a.jsx")), Some(SourceKind::Jsx));
        assert_eq!(source_kind(&path("types.d.ts")), Some(SourceKind::Dts));
        assert_eq!(source_kind(&path("types.d.mts")), Some(SourceKind::Dts));
        assert_eq!(source_kind(&path("types.d.cts")), Some(SourceKind::Dts));
        assert_eq!(source_kind(&path("a.wxml")), None);
        assert_eq!(source_kind(&path("a.json")), None);
        assert_eq!(source_kind(&path("no-extension")), None);
    }

    #[test]
    fn accepts_only_discoverable_config_and_script_overlays() {
        let temp = TempDir::new("accepts");
        temp.write(
            "app.json",
            r#"{"pages":["pages/index/index"],"usingComponents":{"card":"./components/card/card"}}"#,
        );
        temp.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
        temp.write("pages/index/index.json", r#"{"usingComponents":{}}"#);
        temp.write("components/card/card.wxml", "<view>{{ title }}</view>");
        temp.write("components/card/card.json", r#"{"component":true}"#);
        temp.write("package.json", r#"{"name":"fixture"}"#);
        temp.write("src/util.ts", "export const x = 1;");
        temp.write("src/types.d.ts", "export interface X { a: string }");
        temp.write("node_modules/pkg/index.js", "module.exports = 1;");
        temp.write("dist/bundle.js", "var x = 1;");
        temp.write("pages/index/index.wxml", "{{ x }}");
        let project = Project::open(temp.path.clone()).expect("open");

        for accepted in ["src/util.ts", "src/types.d.ts"] {
            let path = path(accepted);
            assert!(project.accepts_script_overlay(&path), "{accepted}");
            assert!(project.accepts_overlay(&path), "{accepted}");
            assert!(!project.accepts_json_overlay(&path), "{accepted}");
        }
        let wxs = path("src/util.wxs");
        assert!(project.accepts_wxs_overlay(&wxs));
        assert!(project.accepts_overlay(&wxs));
        assert!(!project.accepts_script_overlay(&wxs));
        for rejected in ["node_modules/pkg/index.js", "dist/bundle.js"] {
            let path = path(rejected);
            assert!(!project.accepts_script_overlay(&path), "{rejected}");
            assert!(!project.accepts_overlay(&path), "{rejected}");
        }
        for accepted in [
            "app.json",
            "tsconfig.json",
            "pages/index/index.json",
            "components/card/card.json",
        ] {
            let path = path(accepted);
            assert!(project.accepts_json_overlay(&path), "{accepted}");
            assert!(project.accepts_overlay(&path), "{accepted}");
            assert!(!project.accepts_script_overlay(&path), "{accepted}");
        }
        for rejected in ["package.json", "app.wxml", "pages/index/index.wxml"] {
            let path = path(rejected);
            assert!(!project.accepts_json_overlay(&path), "{rejected}");
            assert!(!project.accepts_overlay(&path), "{rejected}");
        }
    }

    #[test]
    fn excludes_directories_case_insensitively() {
        assert!(is_excluded_dir("node_modules"));
        assert!(is_excluded_dir("NODE_MODULES"));
        assert!(is_excluded_dir(".GIT"));
        assert!(is_excluded_dir("Dist"));
        assert!(!is_excluded_dir("src"));
        assert!(!is_excluded_dir("node_modules_two"));
        assert!(has_excluded_component(&path("a/node_modules/b.ts")));
        assert!(!has_excluded_component(&path("src/node_modules.ts")));
    }

    #[test]
    fn default_limits_match_the_documented_values() {
        let limits = SourceLimits::default();
        assert_eq!(limits.max_files, 2048);
        assert_eq!(limits.max_total_bytes, 16 * 1024 * 1024);
        assert_eq!(limits.max_file_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn bounds_scanned_entries() {
        let temp = TempDir::new("scan-limit");
        temp.write("app.json", r#"{"pages": []}"#);
        for name in ["a.ts", "b.ts", "c.ts", "d.ts"] {
            temp.write(name, "");
        }
        let project = Project::open(temp.path.clone()).expect("open");

        let mut issues = SourceIssues::new();
        let (_candidates, truncated) = collect_candidates_with_limit(&project, &mut issues, 2);
        assert!(truncated);
        assert!(issues.truncated);
        assert_eq!(issues.items.len(), 1);
        assert_eq!(issues.items[0].kind, SourceIssueKind::Truncated);
    }
}
