//! Path, version and source-name validation shared by all delivery paths.

use std::path::Path;

use crate::error::{DeliveryError, Result};

/// Maximum UTF-8 bytes in a package-relative path.
pub const MAX_PATH_BYTES: usize = 4096;
/// Maximum path components below the package root.
pub const MAX_PATH_COMPONENTS: usize = 32;
/// Maximum UTF-8 bytes in one path component.
pub const MAX_COMPONENT_BYTES: usize = 255;

/// Returns the platform label stored in manifests for the current host.
///
/// # Errors
///
/// Returns an error on operating systems or architectures this project has not
/// assigned a release label to.
pub fn host_platform() -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        other => {
            return Err(DeliveryError::UnsupportedPlatform(format!(
                "operating system `{other}`"
            )));
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        other => {
            return Err(DeliveryError::UnsupportedPlatform(format!(
                "architecture `{other}`"
            )));
        }
    };
    Ok(format!("{os}-{arch}"))
}

/// Validates a package-relative path from a manifest or archive.
///
/// The function accepts `/` separators only. It rejects absolute paths, drive
/// prefixes, `.`/`..`, empty components, backslashes, control characters and
/// paths longer than the fixed budgets. `allow_reserved` permits the two
/// generated root files (`manifest.json`, `SHA256SUMS`) that are not listed as
/// payload files in the manifest.
///
/// # Errors
///
/// Returns [`DeliveryError::UnsafeArchive`] when the path is not a safe
/// package-relative path.
pub fn validate_relative_path(path: &str, allow_reserved: bool) -> Result<()> {
    if path.is_empty() {
        return Err(DeliveryError::UnsafeArchive("empty path".to_owned()));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(DeliveryError::SizeLimit(format!(
            "path longer than {MAX_PATH_BYTES} bytes"
        )));
    }
    if path.contains('\\') {
        return Err(DeliveryError::UnsafeArchive(
            "backslash separators are not allowed".to_owned(),
        ));
    }
    if path.contains('\0') {
        return Err(DeliveryError::UnsafeArchive("NUL byte in path".to_owned()));
    }
    if path.chars().any(char::is_control) {
        return Err(DeliveryError::UnsafeArchive(
            "control character in path".to_owned(),
        ));
    }
    if path.starts_with('/') {
        return Err(DeliveryError::UnsafeArchive(
            "absolute path is not allowed".to_owned(),
        ));
    }
    let bytes = path.as_bytes();
    if bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':') {
        return Err(DeliveryError::UnsafeArchive(
            "drive-prefixed path is not allowed".to_owned(),
        ));
    }
    if !allow_reserved && matches!(path, "manifest.json" | "SHA256SUMS") {
        return Err(DeliveryError::InvalidPackage(format!(
            "reserved file `{path}` must not be listed as payload"
        )));
    }

    let mut components = 0;
    for component in path.split('/') {
        components += 1;
        if components > MAX_PATH_COMPONENTS {
            return Err(DeliveryError::SizeLimit(format!(
                "path has more than {MAX_PATH_COMPONENTS} components"
            )));
        }
        if component.is_empty() {
            return Err(DeliveryError::UnsafeArchive(
                "empty path component".to_owned(),
            ));
        }
        if matches!(component, "." | "..") {
            return Err(DeliveryError::UnsafeArchive(format!(
                "`{component}` path component is not allowed"
            )));
        }
        if component.len() > MAX_COMPONENT_BYTES {
            return Err(DeliveryError::SizeLimit(format!(
                "path component longer than {MAX_COMPONENT_BYTES} bytes"
            )));
        }
    }
    Ok(())
}

/// Returns the file name component of an archive name after checking it.
///
/// # Errors
///
/// Returns an error when the archive name is not a single safe `.tar.gz`
/// component.
pub fn validate_archive_name(name: &str) -> Result<()> {
    if !name.ends_with(".tar.gz") {
        return Err(DeliveryError::InvalidPackage(
            "archive name must end with `.tar.gz`".to_owned(),
        ));
    }
    validate_relative_path(name, false)?;
    if name.contains('/') {
        return Err(DeliveryError::UnsafeArchive(
            "archive name must be a single path component".to_owned(),
        ));
    }
    Ok(())
}

/// Validates a release version used in directory and archive names.
///
/// # Errors
///
/// Returns an error for empty or over-long values and for characters that
/// could change a path or a URL.
pub fn validate_version(version: &str) -> Result<()> {
    if version.is_empty() || version.len() > 64 {
        return Err(DeliveryError::InvalidArgument(
            "version must be 1-64 bytes".to_owned(),
        ));
    }
    if version.starts_with('.') || version.starts_with('-') {
        return Err(DeliveryError::InvalidArgument(
            "version must start with an alphanumeric character".to_owned(),
        ));
    }
    if !version
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+' | '_'))
    {
        return Err(DeliveryError::InvalidArgument(
            "version contains unsupported characters".to_owned(),
        ));
    }
    if version.contains("..") {
        return Err(DeliveryError::InvalidArgument(
            "version must not contain `..`".to_owned(),
        ));
    }
    Ok(())
}

/// Validates a platform label such as `linux-x86_64`.
///
/// # Errors
///
/// Returns an error for empty, over-long or path-like labels.
pub fn validate_platform(platform: &str) -> Result<()> {
    if platform.is_empty() || platform.len() > 64 {
        return Err(DeliveryError::InvalidArgument(
            "platform must be 1-64 bytes".to_owned(),
        ));
    }
    if !platform
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        return Err(DeliveryError::InvalidArgument(
            "platform contains unsupported characters".to_owned(),
        ));
    }
    Ok(())
}

/// Validates a lowercase SHA-256 hex digest.
///
/// # Errors
///
/// Returns an error when the digest is not exactly 64 lowercase hex digits.
pub fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DeliveryError::Integrity(
            "expected a 64-character lowercase SHA-256 digest".to_owned(),
        ));
    }
    Ok(())
}

/// Quotes a path for a POSIX shell assignment without evaluating it.
#[must_use]
pub fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// Returns a redacted printable form of a path for user messages.
#[must_use]
pub fn redact(path: &Path) -> String {
    crate::error::display_name(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_package_paths() {
        for path in [
            "bin/wxml-lsp",
            "bin/wxml-type-backend/server.js",
            "node_modules/@scope/pkg/index.js",
            "zed-extension/extension.wasm",
        ] {
            validate_relative_path(path, false).unwrap();
        }
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        for path in [
            "",
            "/etc/passwd",
            "C:/windows/system32",
            "../escape",
            "a/../../b",
            "a//b",
            "a/./b",
            "a\\b",
            "a/\u{0}b",
        ] {
            assert!(
                validate_relative_path(path, false).is_err(),
                "path should fail: {path:?}"
            );
        }
    }

    #[test]
    fn rejects_reserved_payload_names_but_allows_archive_metadata() {
        assert!(validate_relative_path("manifest.json", false).is_err());
        assert!(validate_relative_path("SHA256SUMS", false).is_err());
        validate_relative_path("manifest.json", true).unwrap();
        validate_relative_path("SHA256SUMS", true).unwrap();
    }

    #[test]
    fn validates_versions_and_platforms() {
        validate_version("0.1.0").unwrap();
        validate_version("0.1.0-rc.1+build_2").unwrap();
        for version in ["", ".1", "-1", "../1", "1/2", "1..2", "1 2"] {
            assert!(validate_version(version).is_err(), "{version}");
        }
        validate_platform("linux-x86_64").unwrap();
        for platform in ["", "Linux", "../linux", "linux/x86"] {
            assert!(validate_platform(platform).is_err(), "{platform}");
        }
    }

    #[test]
    fn validates_sha256_shape() {
        validate_sha256(&"0".repeat(64)).unwrap();
        for value in [
            String::new(),
            "0".to_owned(),
            "a".repeat(63),
            "A".repeat(64),
            "g".repeat(64),
        ] {
            assert!(validate_sha256(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn quotes_shell_assignments_without_evaluation() {
        assert_eq!(shell_quote("/opt/wxml"), "'/opt/wxml'");
        assert_eq!(shell_quote("/opt/wxml tools"), "'/opt/wxml tools'");
        assert_eq!(shell_quote("/opt/it's"), "'/opt/it'\\''s'");
    }
}
