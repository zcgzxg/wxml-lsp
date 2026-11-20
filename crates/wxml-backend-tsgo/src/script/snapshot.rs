//! Snapshot-only module lookup for behavior/import resolution.
//!
//! Nothing here touches the file system. A relative specifier is normalized
//! against the requiring file and probed with the same extension/index order as
//! the TypeScript backend; a non-relative specifier is delegated to the
//! optional [`super::ModuleResolver`] supplied by the caller (the future LSP
//! adapter maps `baseUrl`/`paths` there). Escaping the project root or naming a
//! file outside the request snapshot returns `None`.

use crate::input::{AnalysisInput, SourceFile};

/// Read-only project snapshot used by script analysis.
#[derive(Clone, Copy)]
pub(crate) struct Snapshot<'a> {
    input: &'a AnalysisInput,
    resolver: Option<&'a dyn super::ModuleResolver>,
}

impl<'a> Snapshot<'a> {
    /// Creates a snapshot view.
    pub(crate) const fn new(
        input: &'a AnalysisInput,
        resolver: Option<&'a dyn super::ModuleResolver>,
    ) -> Self {
        Self { input, resolver }
    }

    /// Returns one file by exact project-relative path.
    pub(crate) fn file(&self, path: &str) -> Option<&'a SourceFile> {
        self.input.file(path)
    }

    /// Resolves a module specifier to a project-relative path in the snapshot.
    pub(crate) fn resolve(&self, specifier: &str, from_path: &str) -> Option<String> {
        if specifier.starts_with("./") || specifier.starts_with("../") {
            return resolve_relative(from_path, specifier)
                .into_iter()
                .find(|candidate| self.file(candidate).is_some());
        }
        if specifier.is_empty() || specifier.contains('\\') || specifier.contains('\0') {
            return None;
        }
        let resolved = self.resolver?.resolve(specifier, from_path)?;
        self.file(&resolved).map(|_| resolved)
    }
}

/// Returns candidate project paths for a relative specifier.
fn resolve_relative(from_path: &str, specifier: &str) -> Vec<String> {
    if specifier.contains('\\') || specifier.contains('\0') {
        return Vec::new();
    }
    let mut components: Vec<&str> = from_path.split('/').collect();
    components.pop();
    for component in specifier.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Vec::new();
                }
            }
            other => components.push(other),
        }
    }
    if components.is_empty() {
        return Vec::new();
    }
    let base = components.join("/");
    if has_extension(&base) {
        return vec![base];
    }
    let mut candidates = vec![
        base.clone(),
        format!("{base}.ts"),
        format!("{base}.js"),
        format!("{base}.mts"),
        format!("{base}.cts"),
        format!("{base}/index.ts"),
        format!("{base}/index.js"),
        format!("{base}/index.mts"),
        format!("{base}/index.cts"),
    ];
    candidates.dedup();
    candidates
}

fn has_extension(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
}
