//! Bounded zero-config discovery of workspace packages.
//!
//! The helper intentionally has no glob dependency: it understands the
//! `workspaces` field of the root `package.json` and the `packages` list of
//! `pnpm-workspace.yaml`, expanding only simple patterns whose wildcard is a
//! whole path component (`apps/*`, `packages/*`, `packages/*/src`).
//!
//! Discovery is deterministic (candidate directories are processed in sorted
//! order and the first path wins duplicate names) and fail-soft (unreadable
//! manifests, invalid names and unsafe export targets are skipped). Every read
//! and scan is bounded so a hostile or accidental workspace cannot stall
//! startup.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;
use wxml_project::ProjectPath;

/// Maximum number of workspace patterns considered from all manifests.
const MAX_PATTERNS: usize = 256;

/// Maximum number of directory entries visited while expanding patterns.
const MAX_SCANNED_ENTRIES: usize = 4096;

/// Maximum number of workspace packages kept after duplicate-name resolution.
const MAX_PACKAGES: usize = 512;

/// Maximum size of one `package.json` read during discovery.
const MAX_PACKAGE_JSON_BYTES: u64 = 256 * 1024;

/// Maximum size of `pnpm-workspace.yaml`.
const MAX_WORKSPACE_YAML_BYTES: u64 = 256 * 1024;

/// Maximum number of `exports` entries consumed from one package.
const MAX_PACKAGE_EXPORTS: usize = 512;

/// Maximum number of dependency names considered for one manifest.
const MAX_DEPENDENCIES: usize = 4096;

/// Maximum number of YAML lines inspected in `pnpm-workspace.yaml`.
const MAX_YAML_LINES: usize = 4096;

/// Directory names that never contain workspace packages.
const EXCLUDED_DIRECTORIES: [&str; 9] = [
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

/// One workspace package discovered from a `package.json` manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspacePackage {
    /// Package name from the manifest.
    pub(crate) name: String,
    /// Canonical absolute package directory.
    pub(crate) directory: PathBuf,
    /// Workspace-relative package directory.
    pub(crate) relative: ProjectPath,
    /// TypeScript path aliases derived from `exports`.
    pub(crate) aliases: BTreeMap<String, Vec<ProjectPath>>,
}

/// Workspace packages indexed by name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorkspacePackages {
    packages: BTreeMap<String, WorkspacePackage>,
}

impl WorkspacePackages {
    /// Discovers workspace packages below `workspace_root`.
    ///
    /// A missing root, unreadable manifests or an exhausted scan budget all
    /// yield an empty or partial index instead of an error. Packages are keyed
    /// by name and iterated in name order.
    #[must_use]
    pub(crate) fn discover(workspace_root: &Path) -> Self {
        let Ok(root) = std::fs::canonicalize(workspace_root) else {
            return Self::default();
        };
        let patterns = workspace_patterns(&root);
        let candidates = candidate_directories(&root, &patterns);
        let mut packages = BTreeMap::new();
        for relative in candidates {
            if packages.len() >= MAX_PACKAGES {
                break;
            }
            if let Some(package) = load_package(&root, &relative) {
                // Candidates are sorted, so the first path wins duplicates.
                packages.entry(package.name.clone()).or_insert(package);
            }
        }
        Self { packages }
    }

    /// Returns the package named `name`, if it was discovered.
    #[must_use]
    #[allow(dead_code, reason = "used by unit tests only")]
    pub(crate) fn get(&self, name: &str) -> Option<&WorkspacePackage> {
        self.packages.get(name)
    }

    /// Iterates over discovered packages in name order.
    #[allow(dead_code, reason = "used by unit tests only")]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &WorkspacePackage> + '_ {
        self.packages.values()
    }

    /// Returns the workspace packages named as dependencies of a manifest.
    ///
    /// `dependencies`, `devDependencies` and `peerDependencies` are read from
    /// `package_json` with the same size bound as discovery. Missing or invalid
    /// manifests yield an empty list; matches are sorted by package name.
    #[must_use]
    pub(crate) fn dependencies_of(&self, package_json: &Path) -> Vec<&WorkspacePackage> {
        let Some(value) = read_json(package_json) else {
            return Vec::new();
        };
        let mut names = BTreeSet::new();
        for field in ["dependencies", "devDependencies", "peerDependencies"] {
            let Some(object) = value.get(field).and_then(Value::as_object) else {
                continue;
            };
            let remaining = MAX_DEPENDENCIES.saturating_sub(names.len());
            for dependency in object.keys().take(remaining) {
                names.insert(dependency.as_str());
            }
        }
        names
            .into_iter()
            .filter_map(|name| self.packages.get(name))
            .collect()
    }
}

/// A parsed workspace pattern.
enum Pattern {
    /// A literal workspace-relative directory.
    Exact(ProjectPath),
    /// One wildcard component, with literal components after it.
    Children {
        /// Directory whose entries provide the wildcard component.
        parent: Option<ProjectPath>,
        /// Literal suffix appended to each matched entry, including its `/`.
        suffix: String,
    },
}

/// Reads workspace patterns from the root `package.json` and pnpm YAML.
fn workspace_patterns(root: &Path) -> Vec<String> {
    let mut patterns = Vec::new();
    if let Some(value) = read_json(&root.join("package.json")) {
        manifest_patterns(&value, &mut patterns);
    }
    pnpm_patterns(root, &mut patterns);
    patterns.truncate(MAX_PATTERNS);
    patterns
}

/// Collects `workspaces` entries from an array or an object with `packages`.
fn manifest_patterns(value: &Value, patterns: &mut Vec<String>) {
    let list = match value.get("workspaces") {
        Some(Value::Array(items)) => Some(items),
        Some(Value::Object(object)) => object.get("packages").and_then(Value::as_array),
        _ => None,
    };
    let Some(items) = list else {
        return;
    };
    for item in items {
        if patterns.len() >= MAX_PATTERNS {
            break;
        }
        if let Some(pattern) = item.as_str().map(str::trim).filter(|item| !item.is_empty()) {
            patterns.push(pattern.to_owned());
        }
    }
}

/// Collects the top-level `packages` list from `pnpm-workspace.yaml`.
fn pnpm_patterns(root: &Path, patterns: &mut Vec<String>) {
    let Some(text) = read_text(&root.join("pnpm-workspace.yaml"), MAX_WORKSPACE_YAML_BYTES) else {
        return;
    };
    let mut in_packages = false;
    for line in text.lines().take(MAX_YAML_LINES) {
        if patterns.len() >= MAX_PATTERNS {
            break;
        }
        let trimmed = yaml_without_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        if in_packages {
            if let Some(item) = trimmed.strip_prefix('-') {
                push_pattern(item, patterns);
                continue;
            }
            break;
        }
        if line.len() != line.trim_start().len() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("packages:") else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            in_packages = true;
        } else if let Some(inner) = rest
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            for item in inner.split(',') {
                if patterns.len() >= MAX_PATTERNS {
                    break;
                }
                push_pattern(item, patterns);
            }
        }
    }
}

/// Adds one trimmed, unquoted YAML scalar to `patterns`.
fn push_pattern(raw: &str, patterns: &mut Vec<String>) {
    let pattern = yaml_scalar(raw);
    if !pattern.is_empty() {
        patterns.push(pattern.to_owned());
    }
}

/// Removes an inline YAML comment that is outside quoted scalars.
fn yaml_without_comment(line: &str) -> &str {
    let mut quote = None;
    for (index, character) in line.char_indices() {
        match character {
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            '#' if quote.is_none() => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Strips matching quotes from one simple YAML scalar.
fn yaml_scalar(raw: &str) -> &str {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('\'') {
        return rest.strip_suffix('\'').unwrap_or(rest);
    }
    if let Some(rest) = raw.strip_prefix('"') {
        return rest.strip_suffix('"').unwrap_or(rest);
    }
    raw
}

/// Expands patterns into sorted, unique workspace-relative directories.
fn candidate_directories(root: &Path, patterns: &[String]) -> BTreeSet<ProjectPath> {
    let mut candidates = BTreeSet::new();
    let mut remaining_entries = MAX_SCANNED_ENTRIES;
    for raw in patterns {
        let Some(pattern) = parse_pattern(raw) else {
            continue;
        };
        match pattern {
            Pattern::Exact(path) => {
                if !is_excluded(&path) {
                    candidates.insert(path);
                }
            }
            Pattern::Children { parent, suffix } => {
                if parent.as_ref().is_some_and(is_excluded) {
                    continue;
                }
                scan_children(
                    root,
                    parent.as_ref(),
                    &suffix,
                    &mut candidates,
                    &mut remaining_entries,
                );
            }
        }
    }
    candidates
}

/// Parses one pattern, accepting a single whole-component `*` wildcard.
fn parse_pattern(raw: &str) -> Option<Pattern> {
    let mut pattern = raw.trim().replace('\\', "/");
    while let Some(rest) = pattern.strip_prefix("./") {
        pattern = rest.to_owned();
    }
    let pattern = pattern.trim_end_matches('/');
    if pattern.is_empty() {
        return None;
    }
    match pattern.matches('*').count() {
        0 => ProjectPath::new(pattern).ok().map(Pattern::Exact),
        1 => {
            let (prefix, suffix) = pattern.split_once('*')?;
            if !prefix.is_empty() && !prefix.ends_with('/') {
                return None;
            }
            if !suffix.is_empty() && !suffix.starts_with('/') {
                return None;
            }
            if !suffix.is_empty() && ProjectPath::new(suffix.trim_start_matches('/')).is_err() {
                return None;
            }
            let parent = prefix.trim_end_matches('/');
            let parent = if parent.is_empty() {
                None
            } else {
                ProjectPath::new(parent).ok()
            };
            Some(Pattern::Children {
                parent,
                suffix: suffix.to_owned(),
            })
        }
        _ => None,
    }
}

/// Visits one directory and records each child as a candidate package.
fn scan_children(
    root: &Path,
    parent: Option<&ProjectPath>,
    suffix: &str,
    candidates: &mut BTreeSet<ProjectPath>,
    remaining_entries: &mut usize,
) {
    let directory = parent.map_or_else(|| root.to_path_buf(), |parent| root.join(parent.as_str()));
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    // Sorting names makes duplicate-name resolution independent of read order.
    let mut names = Vec::new();
    for entry in entries {
        if *remaining_entries == 0 {
            break;
        }
        *remaining_entries -= 1;
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_excluded_name(name) {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    for name in names {
        let mut candidate =
            parent.map_or_else(PathBuf::new, |parent| PathBuf::from(parent.as_str()));
        candidate.push(name.as_str());
        if !suffix.is_empty() {
            candidate.push(suffix.trim_start_matches('/'));
        }
        let Some(relative) = candidate.to_str() else {
            continue;
        };
        let Ok(path) = ProjectPath::new(relative) else {
            continue;
        };
        if !is_excluded(&path) {
            candidates.insert(path);
        }
    }
}

/// Returns whether any component of `path` is an excluded directory.
fn is_excluded(path: &ProjectPath) -> bool {
    path.as_str().split('/').any(is_excluded_name)
}

/// Returns whether `name` is an excluded directory component.
fn is_excluded_name(name: &str) -> bool {
    EXCLUDED_DIRECTORIES.contains(&name)
}

/// Loads one candidate directory as a workspace package.
fn load_package(root: &Path, relative: &ProjectPath) -> Option<WorkspacePackage> {
    let directory = root.join(relative.as_str());
    let metadata = std::fs::symlink_metadata(&directory).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let value = read_json(&directory.join("package.json"))?;
    let name = package_name(&value)?;
    let directory = std::fs::canonicalize(&directory).ok()?;
    let canonical_relative = directory.strip_prefix(root).ok()?;
    let relative = ProjectPath::new(canonical_relative.to_str()?).ok()?;
    let aliases = package_aliases(&value, name, &relative);
    Some(WorkspacePackage {
        name: name.to_owned(),
        directory,
        relative,
        aliases,
    })
}

/// Reads a nonempty, control-free package name from a manifest.
fn package_name(value: &Value) -> Option<&str> {
    value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control))
}

/// Builds `exports` aliases that resolve inside the workspace.
fn package_aliases(
    value: &Value,
    name: &str,
    relative: &ProjectPath,
) -> BTreeMap<String, Vec<ProjectPath>> {
    let Some(exports) = value.get("exports").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let mut aliases: BTreeMap<String, Vec<ProjectPath>> = BTreeMap::new();
    for (key, exposure) in exports.iter().take(MAX_PACKAGE_EXPORTS) {
        let Some(target) = export_target(exposure, 0) else {
            continue;
        };
        let Some(target) = target.strip_prefix("./") else {
            continue;
        };
        // Reject parent traversal and root-relative targets so the alias always
        // resolves against the package directory.
        let target = target.replace('\\', "/");
        if target.is_empty()
            || target.starts_with('/')
            || target.split('/').any(|part| part == "..")
        {
            continue;
        }
        let alias = if key == "." {
            name.to_owned()
        } else if let Some(rest) = key.strip_prefix("./") {
            if rest.is_empty() || rest.chars().any(char::is_control) {
                continue;
            }
            format!("{name}/{rest}")
        } else {
            continue;
        };
        let Ok(path) = ProjectPath::resolve(Some(relative), &target) else {
            continue;
        };
        aliases.entry(alias).or_default().push(path);
    }
    aliases
}

/// Picks the first usable target from an `exports` condition object.
fn export_target(value: &Value, depth: usize) -> Option<&str> {
    if depth > 4 {
        return None;
    }
    if let Some(target) = value.as_str() {
        return Some(target);
    }
    let object = value.as_object()?;
    for key in ["types", "import", "default", "require"] {
        if let Some(target) = object
            .get(key)
            .and_then(|value| export_target(value, depth + 1))
        {
            return Some(target);
        }
    }
    None
}

/// Reads and parses a JSON file with the discovery size bound.
fn read_json(path: &Path) -> Option<Value> {
    let text = read_text(path, MAX_PACKAGE_JSON_BYTES)?;
    serde_json::from_str(&text).ok()
}

/// Reads a regular file, returning `None` when it exceeds `limit` bytes.
fn read_text(path: &Path, limit: u64) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    let mut reader = file.take(limit.saturating_add(1));
    reader.read_to_string(&mut text).ok()?;
    if u64::try_from(text.len()).ok()? > limit {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            loop {
                let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "wxml-workspace-packages-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        let canonical =
                            fs::canonicalize(&path).expect("canonical temporary directory");
                        return Self(canonical);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("create temporary directory: {error}"),
                }
            }
        }

        fn directory(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).expect("create fixture directory");
            path
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixture parent");
            fs::write(path, text).expect("write fixture");
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn path(raw: &str) -> ProjectPath {
        ProjectPath::new(raw).expect("valid project path")
    }

    fn names(packages: &WorkspacePackages) -> Vec<&str> {
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect()
    }

    #[test]
    fn discovers_array_workspaces_in_name_order() {
        let temp = TestDir::new();
        temp.write(
            "package.json",
            r#"{"workspaces":["apps/*","packages/*","packages/exact"]}"#,
        );
        temp.write("apps/web/package.json", r#"{"name":"web"}"#);
        temp.write("packages/api/package.json", r#"{"name":"api"}"#);
        temp.write("packages/exact/package.json", r#"{"name":"exact"}"#);
        temp.write("packages/nameless/package.json", r#"{"version":"1.0.0"}"#);
        temp.directory("packages/empty");

        let packages = WorkspacePackages::discover(&temp.0);
        assert_eq!(names(&packages), ["api", "exact", "web"]);
        let api = packages.get("api").expect("api package");
        assert_eq!(api.relative, path("packages/api"));
        assert_eq!(api.directory, temp.0.join("packages/api"));
        assert!(packages.get("nameless").is_none());
    }

    #[test]
    fn discovers_object_workspaces_and_pnpm_packages() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":{"packages":["apps/*"]}}"#);
        temp.write(
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/*' # shared libraries\n  - \"tools/*\"\n",
        );
        temp.write("apps/web/package.json", r#"{"name":"web"}"#);
        temp.write("packages/api/package.json", r#"{"name":"api"}"#);
        temp.write("tools/cli/package.json", r#"{"name":"cli"}"#);
        temp.write("ignored/other/package.json", r#"{"name":"other"}"#);

        let packages = WorkspacePackages::discover(&temp.0);
        assert_eq!(names(&packages), ["api", "cli", "web"]);
    }

    #[test]
    fn discovers_nested_wildcard_suffixes() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":["packages/*/src"]}"#);
        temp.write("packages/api/src/package.json", r#"{"name":"api-src"}"#);
        temp.write("packages/web/package.json", r#"{"name":"web"}"#);

        let packages = WorkspacePackages::discover(&temp.0);
        assert_eq!(names(&packages), ["api-src"]);
        assert_eq!(
            packages.get("api-src").expect("api-src package").relative,
            path("packages/api/src")
        );
    }

    #[test]
    fn maps_exports_to_workspace_relative_aliases() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":["packages/*"]}"#);
        temp.write(
            "packages/api/package.json",
            r#"{
                "name": "@acme/api",
                "exports": {
                    ".": {"types": "./src/index.ts", "default": "./dist/index.js"},
                    "./utils": "./src/utils.ts",
                    "./*": {"import": "./src/*.ts"},
                    "./browser": {"browser": "./src/browser.ts"}
                }
            }"#,
        );

        let packages = WorkspacePackages::discover(&temp.0);
        let api = packages.get("@acme/api").expect("api package");

        assert_eq!(
            api.aliases.get("@acme/api").map(Vec::as_slice),
            Some(&[path("packages/api/src/index.ts")][..])
        );
        assert_eq!(
            api.aliases.get("@acme/api/utils").map(Vec::as_slice),
            Some(&[path("packages/api/src/utils.ts")][..])
        );
        assert_eq!(
            api.aliases.get("@acme/api/*").map(Vec::as_slice),
            Some(&[path("packages/api/src/*.ts")][..])
        );
        assert!(!api.aliases.contains_key("@acme/api/browser"));
    }

    #[test]
    fn ignores_unsafe_export_targets() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":["packages/*"]}"#);
        temp.write(
            "packages/api/package.json",
            r#"{
                "name": "api",
                "exports": {
                    "./safe": "./src/safe.ts",
                    "./parent": "../../outside.ts",
                    "./traversal": "./src/../../outside.ts",
                    "./absolute": "/etc/passwd",
                    "./doubled": ".//etc/passwd"
                }
            }"#,
        );

        let packages = WorkspacePackages::discover(&temp.0);
        let api = packages.get("api").expect("api package");

        assert_eq!(
            api.aliases.get("api/safe").map(Vec::as_slice),
            Some(&[path("packages/api/src/safe.ts")][..])
        );
        assert_eq!(api.aliases.len(), 1);
    }

    #[test]
    fn dependencies_of_filters_and_sorts_workspace_packages() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":["packages/*"]}"#);
        temp.write(
            "packages/app/package.json",
            r#"{
                "name": "app",
                "dependencies": {"api": "1", "external": "1"},
                "devDependencies": {"web": "*"},
                "peerDependencies": {"shared": "*"}
            }"#,
        );
        temp.write("packages/api/package.json", r#"{"name":"api"}"#);
        temp.write("packages/shared/package.json", r#"{"name":"shared"}"#);
        temp.write("packages/web/package.json", r#"{"name":"web"}"#);
        temp.write("packages/unused/package.json", r#"{"name":"unused"}"#);

        let packages = WorkspacePackages::discover(&temp.0);
        let dependencies = packages.dependencies_of(&temp.0.join("packages/app/package.json"));
        let names: Vec<&str> = dependencies
            .iter()
            .map(|package| package.name.as_str())
            .collect();

        assert_eq!(names, ["api", "shared", "web"]);
        assert_eq!(
            packages
                .dependencies_of(&temp.0.join("packages/app/missing.json"))
                .len(),
            0
        );
    }

    #[test]
    fn duplicate_names_keep_the_first_sorted_path() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":["packages/*"]}"#);
        temp.write("packages/zeta/package.json", r#"{"name":"dup"}"#);
        temp.write("packages/alpha/package.json", r#"{"name":"dup"}"#);

        let packages = WorkspacePackages::discover(&temp.0);

        assert_eq!(packages.iter().count(), 1);
        assert_eq!(
            packages.get("dup").expect("dup package").relative,
            path("packages/alpha")
        );
    }

    #[test]
    fn ignores_excluded_and_symlinked_directories() {
        let temp = TestDir::new();
        temp.write(
            "package.json",
            r#"{"workspaces":["packages/*","node_modules/*"]}"#,
        );
        temp.write("packages/api/package.json", r#"{"name":"api"}"#);
        temp.write(
            "packages/node_modules/hidden/package.json",
            r#"{"name":"hidden"}"#,
        );
        temp.write("packages/build/package.json", r#"{"name":"built"}"#);
        temp.write("node_modules/linked/package.json", r#"{"name":"linked"}"#);
        temp.write("outside/package.json", r#"{"name":"outside"}"#);

        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.0.join("outside"), temp.0.join("packages/symlink"))
            .expect("create symlinked package");

        let packages = WorkspacePackages::discover(&temp.0);

        assert_eq!(names(&packages), ["api"]);
    }

    #[test]
    fn ignores_oversized_manifests() {
        let temp = TestDir::new();
        temp.write("package.json", r#"{"workspaces":["packages/*"]}"#);
        let padding = " ".repeat(256 * 1024);
        temp.write(
            "packages/large/package.json",
            &format!(r#"{{"name":"large"}}{padding}"#),
        );

        let packages = WorkspacePackages::discover(&temp.0);

        assert!(packages.get("large").is_none());
    }

    #[test]
    fn rejects_patterns_that_are_not_single_component_globs() {
        let temp = TestDir::new();
        temp.write(
            "package.json",
            r#"{"workspaces":["packages/api","packages/**","packages/*/src/*","packages/prefix-*"]}"#,
        );
        temp.write("packages/api/package.json", r#"{"name":"api"}"#);
        temp.write("packages/api/src/package.json", r#"{"name":"api-src"}"#);

        let packages = WorkspacePackages::discover(&temp.0);

        assert_eq!(names(&packages), ["api"]);
    }
}
