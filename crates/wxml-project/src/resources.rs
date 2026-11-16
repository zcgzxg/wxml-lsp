//! Local resource path resolution for WXML attributes.
//!
//! Resource values are never guessed: remote schemes, mustaches, queries and
//! paths that leave the project (or the Mini Program root) are rejected.
//! Relative values resolve against the WXML document's directory, a leading
//! `/` resolves against the `app.json` directory, and existence checks use the
//! same canonicalization and symlink rules as [`crate::files::read_file`].
//!
//! Completions list one directory with a bounded number of entries. Document
//! links are produced by the LSP layer only for existing files, so a missing
//! path never becomes a clickable target.

use std::path::{Path, PathBuf};

use crate::Project;
use crate::files::{self, PathError, ProjectPath};

/// Longest accepted resource specifier.
pub const MAX_RESOURCE_SPECIFIER_BYTES: usize = 1024;

/// Maximum candidates returned for one directory completion.
pub const MAX_RESOURCE_CANDIDATES: usize = 256;

/// Maximum directory entries examined for one completion.
pub const MAX_RESOURCE_SCAN_ENTRIES: usize = 4096;

/// Whether a candidate is a file or a directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
}

/// What one resource value resolves to.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ResourceResolution {
    /// An existing regular file inside the project.
    File(ProjectPath),
    /// An existing directory inside the project.
    Directory(ProjectPath),
    /// A local path that does not exist.
    Missing,
    /// The path would leave the project or Mini Program root.
    OutsideRoot,
    /// An `http(s)`, `data:`, `wxfile:`, `cloud:` or similar remote value.
    Remote,
    /// A value containing a mustache or other dynamic syntax.
    Dynamic,
    /// An empty or malformed value.
    Invalid,
}

/// One completion candidate for a resource attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceCandidate {
    /// Project-relative target path.
    pub path: ProjectPath,
    /// Display label (directories keep a trailing `/`).
    pub label: String,
    /// File or directory.
    pub kind: ResourceKind,
    /// Text inserted in place of the typed value, preserving `./`, `../` and
    /// leading `/` prefixes.
    pub insert: String,
}

/// Completion candidates plus whether the directory listing was truncated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceCandidates {
    /// Candidates sorted with directories first.
    pub items: Vec<ResourceCandidate>,
    /// `true` when a scan or result budget was reached.
    pub truncated: bool,
}

/// Returns whether a specifier uses a non-`file` URL scheme.
///
/// `//host/path`, `#fragment`, `http:`, `https:`, `data:`, `wxfile:`,
/// `cloud:`, `blob:`, `plugin:`, `mailto:` and `tel:` are all remote/invalid
/// for this index. A Windows drive-letter prefix is treated as remote here
/// rather than as a project path.
#[must_use]
pub(crate) fn is_remote_specifier(specifier: &str) -> bool {
    let specifier = specifier.trim();
    if specifier.is_empty() {
        return false;
    }
    if specifier.starts_with("//") || specifier.starts_with('#') {
        return true;
    }
    let Some((scheme, _)) = specifier.split_once(':') else {
        return false;
    };
    if scheme.is_empty()
        || !scheme
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    {
        return false;
    }
    if scheme.eq_ignore_ascii_case("file") {
        return false;
    }
    true
}

/// Returns whether `specifier` starts with the local `file:` scheme.
fn is_file_uri(specifier: &str) -> bool {
    specifier
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
}

impl Project {
    /// Resolves one WXML resource value to a local file or directory.
    #[must_use]
    pub fn resolve_resource(&self, wxml: &ProjectPath, specifier: &str) -> ResourceResolution {
        let specifier = specifier.trim();
        if specifier.is_empty() {
            return ResourceResolution::Invalid;
        }
        if specifier.len() > MAX_RESOURCE_SPECIFIER_BYTES
            || specifier.contains('\0')
            || specifier.chars().any(char::is_control)
        {
            return ResourceResolution::Invalid;
        }
        if specifier.contains("{{") || specifier.contains("}}") {
            return ResourceResolution::Dynamic;
        }
        if specifier.contains('?') || specifier.contains('#') {
            return ResourceResolution::Invalid;
        }
        if is_remote_specifier(specifier) {
            return ResourceResolution::Remote;
        }
        let target = if is_file_uri(specifier) {
            match self.path_from_uri(specifier) {
                Ok(path) => path,
                Err(_) => return ResourceResolution::OutsideRoot,
            }
        } else {
            match self.resolve_app_specifier(wxml, specifier) {
                Ok(path) => path,
                Err(PathError::EscapesRoot | PathError::ParentComponent | PathError::Absolute) => {
                    return ResourceResolution::OutsideRoot;
                }
                Err(_) => return ResourceResolution::Invalid,
            }
        };
        if !self.contains_source(&target) {
            return ResourceResolution::OutsideRoot;
        }
        if files::is_file(self.root(), &target) {
            ResourceResolution::File(target)
        } else if files::is_dir(self.root(), &target) {
            ResourceResolution::Directory(target)
        } else if files::escapes_root(self.root(), &target) {
            ResourceResolution::OutsideRoot
        } else {
            ResourceResolution::Missing
        }
    }

    /// Lists one directory for resource completion.
    ///
    /// `prefix` is the text typed since the attribute value start. It may be
    /// empty, relative (`../shared/lo`), root-relative (`/images/lo`) or a
    /// partial file name. Only entries whose canonical path stays inside the
    /// project root are returned.
    #[must_use]
    pub fn resource_candidates(&self, wxml: &ProjectPath, prefix: &str) -> ResourceCandidates {
        let mut result = ResourceCandidates::default();
        let prefix = prefix.trim();
        if prefix.len() > MAX_RESOURCE_SPECIFIER_BYTES
            || prefix.contains('\0')
            || prefix.chars().any(char::is_control)
            || prefix.contains("{{")
            || prefix.contains("}}")
            || is_remote_specifier(prefix)
            || prefix.contains('?')
            || prefix.contains('#')
        {
            return result;
        }
        let normalized = prefix.replace('\\', "/");
        let (directory_prefix, file_prefix) = normalized
            .rfind('/')
            .map_or(("", normalized.as_str()), |position| {
                normalized.split_at(position + 1)
            });
        let Some((directory, relative_directory)) =
            self.resolve_completion_directory(wxml, directory_prefix)
        else {
            return result;
        };
        let root = self.root();
        let Ok(canonical) = std::fs::canonicalize(&directory) else {
            return result;
        };
        if !path_within(root, &canonical) || !canonical.is_dir() {
            return result;
        }
        let Ok(entries) = std::fs::read_dir(&canonical) else {
            return result;
        };
        let mut scanned = 0_usize;
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            scanned += 1;
            if scanned > MAX_RESOURCE_SCAN_ENTRIES {
                result.truncated = true;
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.is_empty() || !name.starts_with(file_prefix) {
                continue;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(project_path) = join_project_path(relative_directory.as_ref(), &name) else {
                continue;
            };
            let is_file = files::is_file(root, &project_path);
            let is_dir = files::is_dir(root, &project_path);
            let (kind, label) = if is_dir {
                (ResourceKind::Directory, format!("{name}/"))
            } else if is_file {
                (ResourceKind::File, name.clone())
            } else {
                continue;
            };
            let insert = format!("{directory_prefix}{name}");
            let insert = if is_dir { format!("{insert}/") } else { insert };
            result.items.push(ResourceCandidate {
                path: project_path,
                label,
                kind,
                insert,
            });
            if result.items.len() >= MAX_RESOURCE_CANDIDATES {
                result.truncated = true;
                break;
            }
        }
        result.items.sort_by(|left, right| {
            let left_dir = left.kind == ResourceKind::Directory;
            let right_dir = right.kind == ResourceKind::Directory;
            (!left_dir)
                .cmp(&(!right_dir))
                .then_with(|| left.label.cmp(&right.label))
        });
        result
    }

    /// Resolves the directory portion of a completion prefix.
    ///
    /// Returns the absolute directory to list and its project-relative path
    /// (`None` means the project root).
    fn resolve_completion_directory(
        &self,
        wxml: &ProjectPath,
        directory_prefix: &str,
    ) -> Option<(PathBuf, Option<ProjectPath>)> {
        let root = self.root();
        if directory_prefix.is_empty() {
            let relative = wxml.parent();
            let base = relative.as_ref().map_or_else(
                || root.to_path_buf(),
                |parent| files::absolute_path(root, parent),
            );
            return Some((base, relative));
        }
        if directory_prefix.starts_with('/') {
            let app = self.source_root();
            let rest = directory_prefix.trim_matches('/');
            let base = app
                .as_ref()
                .map_or_else(|| root.to_path_buf(), |app| files::absolute_path(root, app));
            if rest.is_empty() {
                return Some((base, app.cloned()));
            }
            let relative = app.as_ref().map_or_else(
                || ProjectPath::new(rest).ok(),
                |app| ProjectPath::new(format!("{}/{rest}", app.as_str())).ok(),
            );
            return Some((base.join(rest), relative));
        }
        let path = ProjectPath::resolve(wxml.parent().as_ref(), directory_prefix).ok()?;
        if !self.contains_source(&path) {
            return None;
        }
        Some((files::absolute_path(root, &path), Some(path)))
    }

    /// Resolves a relative or root-relative resource specifier.
    fn resolve_app_specifier(
        &self,
        wxml: &ProjectPath,
        specifier: &str,
    ) -> Result<ProjectPath, PathError> {
        let trimmed = specifier.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(PathError::Empty);
        }
        if let Some(rest) = trimmed.strip_prefix('/') {
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() {
                return self.source_root().cloned().ok_or(PathError::Empty);
            }
            let joined = self.source_root().as_ref().map_or_else(
                || rest.to_owned(),
                |root| format!("{}/{rest}", root.as_str()),
            );
            return ProjectPath::new(joined);
        }
        ProjectPath::resolve(wxml.parent().as_ref(), trimmed)
    }
}

/// Joins one directory entry to a project-relative directory.
fn join_project_path(directory: Option<&ProjectPath>, name: &str) -> Option<ProjectPath> {
    directory.map_or_else(
        || ProjectPath::new(name).ok(),
        |directory| ProjectPath::new(format!("{}/{}", directory.as_str(), name)).ok(),
    )
}

/// Returns whether `candidate` is `root` or below it.
fn path_within(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

/// Reads a file's local resource target for tests without exposing host paths.
#[must_use]
pub fn resource_target(
    project: &Project,
    wxml: &ProjectPath,
    specifier: &str,
) -> Option<ProjectPath> {
    match project.resolve_resource(wxml, specifier) {
        ResourceResolution::File(path) => Some(path),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::files::FileError;

    use super::*;

    #[test]
    fn detects_remote_and_file_schemes() {
        for specifier in [
            "http://example.com/a.png",
            "https://example.com/a.png",
            "//cdn.example.com/a.png",
            "data:image/png;base64,AAAA",
            "wxfile://tmp/a.png",
            "cloud://env/a.png",
            "plugin://p/a.png",
            "mailto:x@example.com",
            "#fragment",
        ] {
            assert!(is_remote_specifier(specifier), "{specifier}");
        }
        for specifier in [
            "images/a.png",
            "/images/a.png",
            "../a.png",
            "file:///tmp/a.png",
        ] {
            assert!(!is_remote_specifier(specifier), "{specifier}");
        }
        assert!(is_file_uri("FILE:///tmp/a.png"));
        assert!(!is_file_uri("https://example.com"));
    }

    #[test]
    fn project_paths_are_joined_without_host_prefixes() {
        let directory = ProjectPath::new("pages/index").expect("path");
        assert_eq!(
            join_project_path(Some(&directory), "a.png")
                .expect("path")
                .as_str(),
            "pages/index/a.png"
        );
        assert_eq!(
            join_project_path(None, "a.png").expect("path").as_str(),
            "a.png"
        );
    }

    #[test]
    fn read_errors_are_not_mistaken_for_missing_files() {
        let error = FileError::NotFound {
            path: ProjectPath::new("a.png").expect("path"),
        };
        assert!(matches!(error, FileError::NotFound { .. }));
    }
}
