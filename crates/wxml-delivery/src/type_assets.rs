//! Type-asset glob matching and Node-runtime payload policy.

use crate::error::{DeliveryError, Result};
use crate::manifest::{FileEntry, sha256_bytes};
use crate::paths::{
    MAX_COMPONENT_BYTES, MAX_PATH_BYTES, MAX_PATH_COMPONENTS, validate_relative_path,
};

/// Maximum number of `*` characters accepted in one type-asset pattern.
pub const MAX_GLOB_WILDCARDS: usize = 64;

/// Result of matching a type-asset bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeAssetBundle {
    /// Number of matched files.
    pub file_count: u64,
    /// Bundle digest.
    pub sha256: String,
}

/// Returns a reason when a payload path would bring a Node runtime into the
/// package. This is a lexical scan; it is also applied to extracted trees.
#[must_use]
pub fn forbidden_runtime_path(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let components: Vec<&str> = lower.split('/').collect();
    if components.contains(&"node_modules") {
        return Some("`node_modules/` is forbidden");
    }
    if lower == "bin/wxml-type-backend" || lower.starts_with("bin/wxml-type-backend/") {
        return Some("`bin/wxml-type-backend/` is forbidden");
    }
    let name = components.last().copied().unwrap_or_default();
    if let Some((_, extension)) = name.rsplit_once('.') {
        if matches!(extension, "js" | "cjs" | "mjs") {
            return Some("JavaScript runtime files are forbidden");
        }
    }
    if matches!(name, "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml") {
        return Some("Node dependency lockfiles are forbidden");
    }
    if name == "package.json" && lower != "engine/package.json" {
        return Some("only `engine/package.json` metadata is allowed");
    }
    None
}

/// Validates a type-asset include pattern.
///
/// Only `*` (inside one component) and a whole-component `**` are supported.
///
/// # Errors
///
/// Returns an error for absolute paths, `..`, backslashes, control characters
/// or unsupported wildcard syntax.
pub fn validate_glob_pattern(pattern: &str) -> Result<()> {
    if pattern.is_empty() {
        return Err(DeliveryError::InvalidPackage(
            "type asset include pattern is empty".to_owned(),
        ));
    }
    if pattern.len() > MAX_PATH_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "type asset include pattern exceeds {MAX_PATH_BYTES} bytes"
        )));
    }
    if pattern.starts_with('/') || pattern.contains('\\') || pattern.contains("..") {
        return Err(DeliveryError::InvalidPackage(format!(
            "type asset include `{pattern}` is not a safe relative glob"
        )));
    }
    if pattern.chars().any(char::is_control) {
        return Err(DeliveryError::InvalidPackage(format!(
            "type asset include `{pattern}` contains control characters"
        )));
    }
    if pattern
        .chars()
        .any(|ch| matches!(ch, '?' | '[' | ']' | '{' | '}' | '!'))
    {
        return Err(DeliveryError::InvalidPackage(format!(
            "type asset include `{pattern}` uses unsupported wildcard syntax"
        )));
    }
    let wildcards = pattern.bytes().filter(|byte| *byte == b'*').count();
    if wildcards > MAX_GLOB_WILDCARDS {
        return Err(DeliveryError::SizeLimit(format!(
            "type asset include `{pattern}` has more than {MAX_GLOB_WILDCARDS} wildcards"
        )));
    }
    let components: Vec<&str> = pattern.split('/').collect();
    if components.len() > MAX_PATH_COMPONENTS {
        return Err(DeliveryError::SizeLimit(format!(
            "type asset include `{pattern}` has too many path components"
        )));
    }
    for component in components {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(DeliveryError::InvalidPackage(format!(
                "type asset include `{pattern}` has an invalid component"
            )));
        }
        if component.len() > MAX_COMPONENT_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "type asset include `{pattern}` has an oversized component"
            )));
        }
        if component.len() > 2 && component.contains("**") {
            return Err(DeliveryError::InvalidPackage(format!(
                "type asset include `{pattern}` only supports `**` as a whole component"
            )));
        }
        if !component
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '*' | '.' | '_' | '-' | '+'))
        {
            return Err(DeliveryError::InvalidPackage(format!(
                "type asset include `{pattern}` contains unsupported characters"
            )));
        }
    }
    Ok(())
}

/// Matches one type-asset glob against a package-relative path.
///
/// `*` matches within one path component and `**` matches zero or more
/// components. The implementation is iterative and bounded by the path/pattern
/// caps, so adversarial patterns cannot cause exponential backtracking.
#[must_use]
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    if pattern.is_empty() || path.is_empty() {
        return false;
    }
    if pattern.len() > MAX_PATH_BYTES || path.len() > MAX_PATH_BYTES {
        return false;
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    if pattern.len() > MAX_PATH_COMPONENTS || path.len() > MAX_PATH_COMPONENTS {
        return false;
    }
    let mut dp = vec![vec![false; path.len() + 1]; pattern.len() + 1];
    dp[pattern.len()][path.len()] = true;
    for pattern_index in (0..pattern.len()).rev() {
        for path_index in (0..=path.len()).rev() {
            let component = pattern[pattern_index];
            dp[pattern_index][path_index] = if component == "**" {
                dp[pattern_index + 1][path_index]
                    || (path_index < path.len() && dp[pattern_index][path_index + 1])
            } else {
                path_index < path.len()
                    && component_matches(component, path[path_index])
                    && dp[pattern_index + 1][path_index + 1]
            };
        }
    }
    dp[0][0]
}

/// Greedy linear wildcard match for one path component (`*` only).
const fn component_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut pattern_index = 0;
    let mut text_index = 0;
    let mut star_index = None;
    let mut star_text_index = 0;
    while text_index < text.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == text[text_index] {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_text_index = text_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_text_index += 1;
            text_index = star_text_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// Computes the type-asset bundle digest defined by the delivery contract.
///
/// # Errors
///
/// Returns an error for an invalid root or include pattern.
pub fn compute_type_asset_bundle(
    files: &[FileEntry],
    root: &str,
    include: &[String],
) -> Result<TypeAssetBundle> {
    validate_relative_path(root, false)?;
    if root.ends_with('/') {
        return Err(DeliveryError::InvalidPackage(
            "type asset root must not end with `/`".to_owned(),
        ));
    }
    for pattern in include {
        validate_glob_pattern(pattern)?;
    }
    let prefix = format!("{root}/");
    let mut matched = Vec::new();
    for file in files {
        let Some(relative) = file.path.strip_prefix(&prefix) else {
            continue;
        };
        if include
            .iter()
            .any(|pattern| glob_matches(pattern, relative))
        {
            matched.push((file.path.as_str(), relative, file.sha256.as_str()));
        }
    }
    matched.sort_by(|left, right| left.0.cmp(right.0));
    let mut lines = String::new();
    for (_, relative, sha256) in &matched {
        lines.push_str(sha256);
        lines.push_str("  ");
        lines.push_str(relative);
        lines.push('\n');
    }
    let file_count = u64::try_from(matched.len())
        .map_err(|_| DeliveryError::SizeLimit("type asset count overflows".to_owned()))?;
    Ok(TypeAssetBundle {
        file_count,
        sha256: sha256_bytes(lines.as_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensionless_js_like_names_are_not_runtime_files() {
        assert!(forbidden_runtime_path("js").is_none());
        assert!(forbidden_runtime_path("types/js").is_none());
        assert!(forbidden_runtime_path("runtime.js").is_some());
        assert!(forbidden_runtime_path("runtime.JS").is_some());
        assert!(forbidden_runtime_path("runtime.cjs").is_some());
        assert!(forbidden_runtime_path("runtime.mjs").is_some());
    }

    #[test]
    fn caps_pattern_length_components_and_wildcards() {
        assert!(validate_glob_pattern(&"a".repeat(MAX_PATH_BYTES + 1)).is_err());
        let too_many_components = vec!["a"; MAX_PATH_COMPONENTS + 1].join("/");
        assert!(validate_glob_pattern(&too_many_components).is_err());
        let too_many_stars = "a*".repeat(MAX_GLOB_WILDCARDS + 1);
        assert!(validate_glob_pattern(&too_many_stars).is_err());
        assert!(validate_glob_pattern("a**b").is_err());
    }

    #[test]
    fn adversarial_glob_is_bounded() {
        let pattern = vec!["**"; MAX_PATH_COMPONENTS].join("/");
        let path = vec!["a"; MAX_PATH_COMPONENTS].join("/");
        validate_glob_pattern(&pattern).unwrap();
        let started = std::time::Instant::now();
        for _ in 0..2_000 {
            assert!(glob_matches(&pattern, &path));
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "glob matching must stay bounded"
        );

        let star_pattern = format!("{}b", "a*".repeat(MAX_COMPONENTS));
        let star_path = "a".repeat(MAX_COMPONENT_BYTES);
        assert!(!glob_matches(&star_pattern, &star_path));
    }

    const MAX_COMPONENTS: usize = MAX_PATH_COMPONENTS;
}
