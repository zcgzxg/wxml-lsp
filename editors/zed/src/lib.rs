//! Zed extension for WXML.
//!
//! The extension starts the repository's `wxml-lsp` binary over stdio. Only an
//! explicit absolute `WXML_LSP_BIN` is accepted. The extension does not probe
//! the worktree, does not search `PATH`, and runs only the binary named by that
//! trusted user configuration.

use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

/// Language server id from `extension.toml` (`[language_servers.wxml-lsp]`).
const LANGUAGE_SERVER_ID: &str = "wxml-lsp";

/// Absolute host path to a locally built `wxml-lsp` binary.
const BINARY_ENV: &str = "WXML_LSP_BIN";

/// Non-legacy tuning variables forwarded to the server when the user has
/// exported them in the shell Zed inherited.
///
/// The legacy Node variables (`WXML_TYPE_BACKEND`, `WXML_BACKEND_DIR`,
/// `WXML_NODE`) are deliberately absent: the delivery contract requires the
/// extension to stop forwarding them, and the native package never needs
/// them. Values here are not interpreted by the extension.
const FORWARDED_ENV: [&str; 2] = ["WXML_ANALYZE_TIMEOUT_MS", "WXML_TEMPLATE_CHECK"];

struct WxmlExtension;

impl zed::Extension for WxmlExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        if language_server_id.as_ref() != LANGUAGE_SERVER_ID {
            return Err(format!(
                "wxml extension does not provide language server `{language_server_id}`"
            ));
        }

        let os = zed::current_platform().0;
        let shell_env = worktree.shell_env();
        let binary = resolve_binary(&shell_env, os, |path| worktree.which(path))?;
        log(&format!(
            "starting language server binary `{}`",
            display_name(&binary)
        ));

        Ok(Command {
            command: binary,
            args: vec!["--stdio".to_owned()],
            env: forwarded_env(&shell_env),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        _worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(Some(zed::serde_json::json!({})))
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        _worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(Some(zed::serde_json::json!({})))
    }
}

zed::register_extension!(WxmlExtension);

/// Resolves the language server from an explicit environment variable.
///
/// There is intentionally no fallback: an unset, relative, parent-traversing,
/// missing, or non-executable `WXML_LSP_BIN` is an error. In particular this
/// extension never looks at `<worktree>/target/...`, so opening an untrusted
/// project cannot cause a project-controlled executable to run.
fn resolve_binary(
    shell_env: &[(String, String)],
    os: zed::Os,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Result<String> {
    let Some(raw) = env_lookup(shell_env, BINARY_ENV) else {
        return Err(format!(
            "{BINARY_ENV} is not set. Export an absolute path to a locally built wxml-lsp \
             binary before launching Zed; the extension does not probe the worktree or PATH."
        ));
    };

    let candidate = validate_configured_binary(raw, os)?;
    let Some(found) = lookup(&candidate) else {
        return Err(format!(
            "{BINARY_ENV} points to `{}`, which does not exist or is not executable",
            display_name(&candidate)
        ));
    };

    validate_configured_binary(&found, os).map_err(|_| {
        format!(
            "Zed resolved {BINARY_ENV} to a path that is invalid for this host; \
             refusing to start the language server"
        )
    })?;

    Ok(found)
}

/// Validates a user-provided host path using string rules only.
///
/// The extension is compiled to `wasm32-wasip2`, whose path APIs implement
/// POSIX semantics. Host paths must therefore be checked as strings and with
/// the host OS, or a Windows/Unix path would be misclassified on the other
/// platform.
fn validate_configured_binary(raw: &str, os: zed::Os) -> Result<String> {
    if raw.is_empty() {
        return Err(format!("{BINARY_ENV} is set but empty"));
    }
    if ends_with_separator(raw, os) {
        return Err(format!(
            "{BINARY_ENV} must name a file, not end with a path separator; got `{}`",
            display_name(raw)
        ));
    }
    if !is_absolute_host_path(raw, os) {
        return Err(format!(
            "{BINARY_ENV} must be {}; got `{}`",
            path_requirement(os),
            display_name(raw)
        ));
    }
    if has_parent_dir(raw) {
        return Err(format!(
            "{BINARY_ENV} must not contain `..` path components; got `{}`",
            display_name(raw)
        ));
    }

    Ok(raw.to_owned())
}

/// Describes the accepted absolute-path shape for the host.
fn path_requirement(os: zed::Os) -> &'static str {
    match os {
        zed::Os::Windows => {
            "an absolute Windows drive path (`C:\\...` or `C:/...`) or UNC path \
             (`\\\\server\\share...` or `//server/share...`)"
        }
        zed::Os::Linux | zed::Os::Mac => "an absolute POSIX path (`/...`)",
    }
}

/// Checks the host-specific absolute-path shape.
///
/// - Windows: drive-absolute (`C:\...`, `C:/...`) or UNC (`\\server\share`,
///   `//server/share`). A leading single `/` is rejected, including `/C:/...`.
/// - Linux/macOS: a leading `/`, except UNC-looking `//server/share` paths.
fn is_absolute_host_path(raw: &str, os: zed::Os) -> bool {
    match os {
        zed::Os::Windows => is_windows_drive_absolute(raw) || is_unc_path(raw),
        zed::Os::Linux | zed::Os::Mac => raw.starts_with('/') && !is_unc_path(raw),
    }
}

/// Accepts `[A-Za-z]:[\\/]`, but not drive-relative `C:foo` or bare `C:`.
fn is_windows_drive_absolute(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.first().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.get(1) == Some(&b':')
        && matches!(bytes.get(2), Some(b'/' | b'\\'))
}

/// Accepts `\\server\share...` and `//server/share...`, requiring both a
/// server and a share segment. `which` may return `\\?\C:\...`; that form also
/// satisfies the two-segment check.
fn is_unc_path(raw: &str) -> bool {
    let rest = if let Some(rest) = raw.strip_prefix("\\\\") {
        rest
    } else if let Some(rest) = raw.strip_prefix("//") {
        rest
    } else {
        return false;
    };

    let mut segments = rest
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty());
    segments.next().is_some() && segments.next().is_some()
}

/// A trailing separator is only a separator on the host: `/` on Unix, both `/`
/// and `\` on Windows.
fn ends_with_separator(raw: &str, os: zed::Os) -> bool {
    match os {
        zed::Os::Windows => raw.ends_with('/') || raw.ends_with('\\'),
        zed::Os::Linux | zed::Os::Mac => raw.ends_with('/'),
    }
}

/// Splits on both `/` and `\`, then rejects any `..` segment.
fn has_parent_dir(raw: &str) -> bool {
    raw.split(['/', '\\']).any(|segment| segment == "..")
}

/// Returns only the final path segment, so errors and logs do not expose the
/// user's full directory layout.
fn display_name(raw: &str) -> &str {
    raw.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(BINARY_ENV)
}

/// Returns the last value for `key`, matching shell override semantics.
fn env_lookup<'a>(shell_env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    shell_env
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Copies only the documented non-legacy tuning variables. This is not a
/// redaction boundary: Zed may still merge the full shell environment when
/// spawning the language server, and the server itself ignores the legacy
/// Node variables.
fn forwarded_env(shell_env: &[(String, String)]) -> Vec<(String, String)> {
    FORWARDED_ENV
        .iter()
        .filter_map(|key| {
            env_lookup(shell_env, key).map(|value| ((*key).to_owned(), value.to_owned()))
        })
        .collect()
}

/// Extension logs go to stderr, which Zed forwards to its own process. The
/// 0.7.0 extension API has no logging module, so `eprintln!` is the supported
/// channel (`zed --foreground` shows it).
fn log(message: &str) {
    eprintln!("[wxml] {message}");
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const WINDOWS: zed::Os = zed::Os::Windows;
    const LINUX: zed::Os = zed::Os::Linux;
    const MAC: zed::Os = zed::Os::Mac;

    #[test]
    fn windows_accepts_drive_and_unc_paths() {
        for path in [
            "C:\\tools\\wxml-lsp.exe",
            "C:/tools/wxml-lsp.exe",
            "c:\\tools\\wxml-lsp.exe",
            "C:\\tools/wxml-lsp.exe",
            "C:\\Program Files\\wxml-lsp.exe",
            "\\\\server\\share\\wxml-lsp.exe",
            "//server/share/wxml-lsp.exe",
            "\\\\server/share\\wxml-lsp.exe",
        ] {
            assert!(validate_configured_binary(path, WINDOWS).is_ok(), "{path}");
        }
    }

    #[test]
    fn windows_rejects_posix_and_drive_relative_paths() {
        for path in [
            "/abs",
            "/opt/wxml-lsp",
            "/C:/tools/wxml-lsp.exe",
            "C:",
            "C:tools\\wxml-lsp.exe",
            "\\server\\share\\wxml-lsp.exe",
            "target\\debug\\wxml-lsp",
            "wxml-lsp",
            "\\\\server",
        ] {
            assert!(validate_configured_binary(path, WINDOWS).is_err(), "{path}");
        }
    }

    #[test]
    fn windows_rejects_trailing_separators() {
        for path in ["C:\\", "C:/", "C:\\tools\\", "\\\\server\\share\\"] {
            assert!(validate_configured_binary(path, WINDOWS).is_err(), "{path}");
        }
    }

    #[test]
    fn posix_accepts_absolute_paths_for_linux_and_mac() {
        for os in [LINUX, MAC] {
            for path in [
                "/abs",
                "/usr/local/bin/wxml-lsp",
                "/opt/wxml tools/wxml-lsp",
                "/C:/tools/wxml-lsp",
            ] {
                assert!(
                    validate_configured_binary(path, os).is_ok(),
                    "{os:?} {path}"
                );
            }
        }
    }

    #[test]
    fn posix_rejects_drive_and_unc_paths() {
        for os in [LINUX, MAC] {
            for path in [
                "C:\\tools\\wxml-lsp.exe",
                "C:/tools/wxml-lsp.exe",
                "C:tools\\wxml-lsp.exe",
                "\\\\server\\share\\wxml-lsp.exe",
                "//server/share/wxml-lsp.exe",
                "target/debug/wxml-lsp",
                "wxml-lsp",
            ] {
                assert!(
                    validate_configured_binary(path, os).is_err(),
                    "{os:?} {path}"
                );
            }
        }
    }

    #[test]
    fn posix_rejects_trailing_separator() {
        for os in [LINUX, MAC] {
            for path in ["/", "/opt/wxml-lsp/"] {
                assert!(
                    validate_configured_binary(path, os).is_err(),
                    "{os:?} {path}"
                );
            }
        }
    }

    #[test]
    fn posix_treats_backslash_as_filename_character() {
        // `\` is not a POSIX separator, so this is not a trailing separator.
        assert!(validate_configured_binary("/opt/wxml-lsp\\", LINUX).is_ok());
    }

    #[test]
    fn rejects_parent_directory_components_on_both_platforms() {
        for (os, path) in [
            (WINDOWS, "C:\\tools\\..\\wxml-lsp.exe"),
            (WINDOWS, "\\\\server\\share\\..\\wxml-lsp.exe"),
            (LINUX, "/opt/../bin/wxml-lsp"),
            (MAC, "/opt/../bin/wxml-lsp"),
            (LINUX, "..\\..\\bin"),
        ] {
            assert!(has_parent_dir(path), "{path}");
            assert!(
                validate_configured_binary(path, os).is_err(),
                "{os:?} {path}"
            );
        }
    }

    #[test]
    fn does_not_confuse_parent_like_names() {
        assert!(validate_configured_binary("/opt/..foo/wxml-lsp", LINUX).is_ok());
        assert!(validate_configured_binary("C:\\tools\\..foo\\wxml-lsp.exe", WINDOWS).is_ok());
    }

    #[test]
    fn rejects_empty_and_leading_space_values() {
        for os in [WINDOWS, LINUX, MAC] {
            for path in ["", "   "] {
                assert!(validate_configured_binary(path, os).is_err(), "{os:?}");
            }
        }
        assert!(validate_configured_binary(" /opt/wxml-lsp", LINUX).is_err());
        assert!(validate_configured_binary(" C:\\tools\\wxml-lsp.exe", WINDOWS).is_err());
    }

    #[test]
    fn unc_requires_server_and_share() {
        for path in ["\\\\", "\\\\server", "//", "//server"] {
            assert!(!is_unc_path(path), "{path}");
            assert!(validate_configured_binary(path, WINDOWS).is_err(), "{path}");
        }
    }

    #[test]
    fn windows_drive_requires_a_separator_after_the_colon() {
        assert!(!is_windows_drive_absolute("C:"));
        assert!(!is_windows_drive_absolute("C:tools"));
        assert!(is_windows_drive_absolute("C:\\"));
        assert!(is_windows_drive_absolute("C:/"));
    }

    #[test]
    fn display_name_hides_directories() {
        assert_eq!(display_name("/opt/secret/wxml-lsp"), "wxml-lsp");
        assert_eq!(
            display_name("C:\\Program Files\\wxml-lsp.exe"),
            "wxml-lsp.exe"
        );
        assert_eq!(display_name(""), BINARY_ENV);
    }

    #[test]
    fn resolve_requires_explicit_binary() {
        let lookup_called = Cell::new(false);
        let result = resolve_binary(&[], LINUX, |_| {
            lookup_called.set(true);
            None
        });

        assert!(result.is_err());
        assert!(!lookup_called.get());
    }

    #[test]
    fn resolve_rejects_relative_path_without_lookup() {
        let env = vec![(BINARY_ENV.to_owned(), "target/debug/wxml-lsp".to_owned())];
        let lookup_called = Cell::new(false);
        let result = resolve_binary(&env, LINUX, |_| {
            lookup_called.set(true);
            None
        });

        assert!(result.is_err());
        assert!(!lookup_called.get());
    }

    #[test]
    fn resolve_does_not_fall_back_when_explicit_path_is_missing() {
        let env = vec![(BINARY_ENV.to_owned(), "/opt/wxml tools/wxml-lsp".to_owned())];
        let lookup_calls = Cell::new(0);
        let error = resolve_binary(&env, LINUX, |path| {
            assert_eq!(path, "/opt/wxml tools/wxml-lsp");
            lookup_calls.set(lookup_calls.get() + 1);
            None
        })
        .expect_err("missing binary must fail");

        assert_eq!(lookup_calls.get(), 1);
        assert!(!error.contains("/opt/wxml tools"));
        assert!(error.contains("wxml-lsp"));
    }

    #[test]
    fn resolve_uses_verified_absolute_binary() {
        let env = vec![(BINARY_ENV.to_owned(), "/opt/wxml-lsp".to_owned())];
        let result = resolve_binary(&env, LINUX, |path| Some(path.to_owned()));

        assert_eq!(result, Ok("/opt/wxml-lsp".to_owned()));
    }

    #[test]
    fn resolve_uses_verified_windows_binary() {
        let env = vec![(BINARY_ENV.to_owned(), "C:\\tools\\wxml-lsp.exe".to_owned())];
        let result = resolve_binary(&env, WINDOWS, |_| Some("C:/tools/wxml-lsp.exe".to_owned()));

        assert_eq!(result, Ok("C:/tools/wxml-lsp.exe".to_owned()));
    }

    #[test]
    fn resolve_rejects_wrong_platform_configured_path() {
        let linux_env = vec![(BINARY_ENV.to_owned(), "C:\\tools\\wxml-lsp.exe".to_owned())];
        let linux_lookup_called = Cell::new(false);
        assert!(
            resolve_binary(&linux_env, LINUX, |_| {
                linux_lookup_called.set(true);
                None
            })
            .is_err()
        );
        assert!(!linux_lookup_called.get());

        let windows_env = vec![(BINARY_ENV.to_owned(), "/opt/wxml-lsp".to_owned())];
        let windows_lookup_called = Cell::new(false);
        assert!(
            resolve_binary(&windows_env, WINDOWS, |_| {
                windows_lookup_called.set(true);
                None
            })
            .is_err()
        );
        assert!(!windows_lookup_called.get());
    }

    #[test]
    fn resolve_rejects_unsafe_lookup_result() {
        let linux_env = vec![(BINARY_ENV.to_owned(), "/opt/wxml-lsp".to_owned())];
        for found in [
            "wxml-lsp",
            "/opt/../wxml-lsp",
            "C:\\tools\\wxml-lsp.exe",
            "//server/share/wxml-lsp",
        ] {
            assert!(
                resolve_binary(&linux_env, LINUX, |_| Some(found.to_owned())).is_err(),
                "{found}"
            );
        }

        let windows_env = vec![(BINARY_ENV.to_owned(), "C:\\tools\\wxml-lsp.exe".to_owned())];
        for found in [
            "/opt/wxml-lsp",
            "/C:/tools/wxml-lsp.exe",
            "target\\debug\\wxml-lsp",
            "C:\\tools\\..\\wxml-lsp.exe",
        ] {
            assert!(
                resolve_binary(&windows_env, WINDOWS, |_| Some(found.to_owned())).is_err(),
                "{found}"
            );
        }
    }

    #[test]
    fn last_environment_value_wins() {
        let env = vec![
            ("WXML_LSP_BIN".to_owned(), "/opt/first".to_owned()),
            ("WXML_LSP_BIN".to_owned(), "/opt/second".to_owned()),
        ];
        assert_eq!(env_lookup(&env, "WXML_LSP_BIN"), Some("/opt/second"));
        assert_eq!(env_lookup(&env, "MISSING"), None);
    }

    /// Languages the `wxml-lsp` server is registered for, in manifest order.
    /// JSX is deliberately absent: `wxml-lsp` handles WXML templates plus the
    /// JavaScript/TypeScript/JSON files they import, not JSX.
    const EXPECTED_LANGUAGES: [&str; 5] = ["WXML", "JavaScript", "TypeScript", "TSX", "JSON"];

    /// Language id each registered language maps to in the LSP protocol.
    const EXPECTED_LANGUAGE_IDS: [(&str, &str); 5] = [
        ("WXML", "wxml"),
        ("JavaScript", "javascript"),
        ("TypeScript", "typescript"),
        ("TSX", "typescriptreact"),
        ("JSON", "json"),
    ];

    const MANIFEST: &str = include_str!("../extension.toml");
    const WXML_LANGUAGE_CONFIG: &str = include_str!("../languages/wxml/config.toml");

    /// Returns the non-comment, non-blank lines of `[section]` in `text`.
    ///
    /// This is a deliberately small hand-written reader: the manifest is a
    /// flat file whose only nested data is a table header, so a real TOML
    /// dependency is not worth it just to lock the contract.
    fn section_lines<'a>(text: &'a str, section: &str) -> Vec<&'a str> {
        let header = format!("[{section}]");
        let mut in_section = false;
        let mut lines = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_section = line == header;
                continue;
            }
            if in_section {
                lines.push(line);
            }
        }
        lines
    }

    /// Extracts every double-quoted string from `text`, honouring `\"` and `\\`.
    fn quoted_strings(text: &str) -> Vec<String> {
        let mut strings = Vec::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch != '"' {
                continue;
            }
            let mut value = String::new();
            while let Some(ch) = chars.next() {
                match ch {
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => break,
                    _ => value.push(ch),
                }
            }
            strings.push(value);
        }
        strings
    }

    /// Reads `key = [ ... ]` (single- or multi-line) as ordered strings.
    fn string_array(section: &str, key: &str, text: &str) -> Vec<String> {
        let assignment = format!("{key} =");
        let mut body = String::new();
        let mut collecting = false;
        for line in section_lines(text, section) {
            if collecting {
                body.push_str(line);
                body.push('\n');
            } else if line.starts_with(&assignment) {
                collecting = true;
                body.push_str(line);
                body.push('\n');
            }
            if collecting && line.contains(']') {
                break;
            }
        }
        assert!(collecting, "section [{section}] has no `{key}` array");
        quoted_strings(&body)
    }

    #[test]
    fn manifest_registers_the_five_languages_in_order_without_jsx() {
        let languages = string_array("language_servers.wxml-lsp", "languages", MANIFEST);

        assert_eq!(languages, EXPECTED_LANGUAGES);
        assert!(
            !MANIFEST.to_ascii_lowercase().contains("jsx"),
            "manifest must not register JSX"
        );
    }

    #[test]
    fn manifest_language_ids_map_every_registered_language() {
        let ids = section_lines(MANIFEST, "language_servers.wxml-lsp.language_ids");

        let mut mapped = Vec::new();
        for line in &ids {
            let (key, value) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("malformed language_ids entry `{line}`"));
            let key = quoted_strings(key);
            let value = quoted_strings(value);
            assert_eq!(key.len(), 1, "malformed language_ids key in `{line}`");
            assert_eq!(value.len(), 1, "malformed language_ids value in `{line}`");
            mapped.push((key[0].clone(), value[0].clone()));
        }

        let mut expected: Vec<(String, String)> = EXPECTED_LANGUAGE_IDS
            .iter()
            .map(|(name, id)| ((*name).to_owned(), (*id).to_owned()))
            .collect();
        expected.sort();
        let mut mapped_sorted = mapped.clone();
        mapped_sorted.sort();
        assert_eq!(mapped_sorted, expected);
        assert_eq!(mapped.len(), 5, "language_ids must not carry duplicates");
    }

    #[test]
    fn wxml_language_config_keeps_its_grammar_and_path_suffix() {
        fn has_line(text: &str, expected: &str) -> bool {
            text.lines().map(str::trim).any(|line| line == expected)
        }

        assert!(
            has_line(WXML_LANGUAGE_CONFIG, r#"name = "WXML""#),
            "languages/wxml/config.toml must keep its language name"
        );
        assert!(
            has_line(WXML_LANGUAGE_CONFIG, r#"grammar = "wxml""#),
            "languages/wxml/config.toml must keep the wxml grammar"
        );
        assert!(
            has_line(WXML_LANGUAGE_CONFIG, r#"path_suffixes = ["wxml"]"#),
            "languages/wxml/config.toml must keep the wxml path suffix"
        );
    }

    #[test]
    fn forwards_only_timeout_and_template_check() {
        let env = vec![
            (
                "WXML_TYPE_BACKEND".to_owned(),
                "/opt/wxml/server.js".to_owned(),
            ),
            ("WXML_BACKEND_DIR".to_owned(), "/opt/wxml".to_owned()),
            ("WXML_NODE".to_owned(), "node".to_owned()),
            ("WXML_LSP_BIN".to_owned(), "/opt/wxml-lsp".to_owned()),
            ("WXML_ANALYZE_TIMEOUT_MS".to_owned(), "1500".to_owned()),
            ("WXML_TEMPLATE_CHECK".to_owned(), "off".to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
        ];
        let forwarded = forwarded_env(&env);
        assert_eq!(
            forwarded,
            vec![
                ("WXML_ANALYZE_TIMEOUT_MS".to_owned(), "1500".to_owned()),
                ("WXML_TEMPLATE_CHECK".to_owned(), "off".to_owned()),
            ]
        );
    }

    #[test]
    fn never_forwards_legacy_node_variables() {
        for legacy in ["WXML_TYPE_BACKEND", "WXML_BACKEND_DIR", "WXML_NODE"] {
            assert!(
                !FORWARDED_ENV.contains(&legacy),
                "{legacy} must not be forwarded by the Zed extension"
            );
            let env = vec![(legacy.to_owned(), "poison".to_owned())];
            assert!(
                forwarded_env(&env).is_empty(),
                "{legacy} leaked into the language server command environment"
            );
        }
    }
}
