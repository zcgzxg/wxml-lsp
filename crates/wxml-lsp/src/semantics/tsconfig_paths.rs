//! tsconfig `paths` alias resolution shared by semantic analyses.
//!
//! A view's effective `compilerOptions` are turned into a resolver over the
//! prepared script list, so an aliased specifier only resolves when the mapped
//! script is part of the view. Aliases are probed with the same extension and
//! `index` order as relative script imports.

use std::path::Path;

use wxml_backend_tsgo::script::ModuleResolver;

use crate::project_host::{PreparedFile, PreparedView};

/// Resolves non-relative specifiers through the view's tsconfig `paths`.
///
/// `baseUrl` is already absolute and inside the project, so mapping it back to
/// a project-relative directory keeps alias candidates comparable with the
/// prepared script list. A candidate only resolves when the mapped script is
/// part of the view, using the same extension and `index` probing as relative
/// imports.
pub(crate) struct TsconfigPaths<'a> {
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
    pub(crate) fn from_view(view: &'a PreparedView) -> Option<Self> {
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
