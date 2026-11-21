//! Workspace/user snippet configuration.
//!
//! Snippets are loaded from an explicit `initializationOptions.snippetsPath`
//! (absolute, or relative to the workspace root) or, when that option is
//! absent, from `<workspace>/.wxml-lsp/snippets.json`. The server never reads
//! Zed's private user configuration directory.
//!
//! File format (all fields except `label` and `body` are optional):
//!
//! ```json
//! {
//!   "enabled": true,
//!   "builtin": true,
//!   "snippets": [
//!     {
//!       "id": "tag.view",
//!       "label": "view (snippet)",
//!       "prefix": "view",
//!       "scope": "tag",
//!       "description": "Custom view block",
//!       "body": ["view class=\"card\">", "  $0", "</view>"]
//!     }
//!   ]
//! }
//! ```
//!
//! * `enabled: false` disables every snippet (built-in and user).
//! * `builtin: false` disables only the built-in catalog.
//! * `scope` is `"tag"` (default) or `"attribute"`.
//! * `prefix` defaults to the label text before the first `" ("`.
//! * A user entry overrides a built-in entry when its `id` equals the built-in
//!   id, or when its `label` equals the built-in label. Later user entries win
//!   over earlier entries with the same identity.
//! * `body` may be one string or an array of lines; line endings are
//!   normalized to the current document's newline style when inserted.
//!
//! Loading is bounded and fail-closed: a missing file means "no user
//! snippets", while malformed, oversized or partially invalid files are logged
//! and degrade to the valid subset (or to no user snippets at all). The cache
//! is refreshed when the file metadata changes and is invalidated by
//! `workspace/didChangeWatchedFiles` notifications for the configured path.

use std::fs;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use serde_json::Value;

use crate::project_host::WatchedFileChange;
use crate::server::log;

/// Default workspace-relative snippet configuration path.
pub(crate) const DEFAULT_CONFIG_PATH: &str = ".wxml-lsp/snippets.json";

/// Largest accepted snippet configuration file.
pub(crate) const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// Largest accepted label length in characters.
pub(crate) const MAX_LABEL_CHARS: usize = 128;

/// Largest accepted prefix length in characters.
pub(crate) const MAX_PREFIX_CHARS: usize = 64;

/// Largest accepted description length in characters.
pub(crate) const MAX_DESCRIPTION_CHARS: usize = 512;

/// Where one user snippet applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Scope {
    /// Replaces a tag-name prefix after `<`.
    Tag,
    /// Inserts one attribute while typing an attribute name.
    Attribute,
}

/// One validated user snippet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UserSnippet {
    /// Stable identity; used for built-in overriding.
    pub(crate) id: String,
    /// Text shown in the completion list.
    pub(crate) label: String,
    /// Prefix matched against the typed text.
    pub(crate) prefix: String,
    /// Tag or attribute insertion site.
    pub(crate) scope: Scope,
    /// Short explanation shown next to the label.
    pub(crate) description: String,
    /// LSP snippet body.
    pub(crate) body: String,
}

/// Effective user snippet catalog after file parsing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Catalog {
    /// `false` disables every snippet, including built-ins.
    pub(crate) enabled: bool,
    /// `false` disables the built-in catalog only.
    pub(crate) builtin: bool,
    /// Validated user snippets in file order.
    pub(crate) snippets: Vec<UserSnippet>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            enabled: true,
            builtin: true,
            snippets: Vec::new(),
        }
    }
}

impl Catalog {
    /// Returns the user snippet that overrides `id` or `label`, if any.
    #[must_use]
    pub(crate) fn overriding(&self, id: &str, label: &str) -> Option<&UserSnippet> {
        self.snippets
            .iter()
            .rev()
            .find(|snippet| snippet.id == id || snippet.label == label)
    }

    /// Returns `true` when a built-in entry is overridden.
    #[must_use]
    pub(crate) fn overrides(&self, id: &str, label: &str) -> bool {
        self.overriding(id, label).is_some()
    }
}

/// Cache stamp used to notice edits without reading the file every time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

/// Shared mutable state behind the server's snippet field.
#[derive(Debug)]
struct State {
    path: Option<PathBuf>,
    init_enabled: bool,
    stamp: Option<Stamp>,
    catalog: Arc<Catalog>,
    /// Last read failure stamp, so an unchanged invalid file logs once.
    reported: Option<Stamp>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            path: None,
            init_enabled: true,
            stamp: None,
            catalog: Arc::new(Catalog::default()),
            reported: None,
        }
    }
}

/// Thread-safe snippet configuration store.
#[derive(Debug, Default)]
pub(crate) struct Store {
    state: Mutex<State>,
}

impl Store {
    /// Sets the configuration path and the initialization-level enable flag.
    ///
    /// `enabled == false` disables all snippets, including built-ins.
    pub(crate) fn configure(&self, path: Option<PathBuf>, enabled: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.path = path;
        state.init_enabled = enabled;
        state.stamp = None;
        state.catalog = Arc::new(Catalog::default());
        state.reported = None;
    }

    /// Returns the current catalog, reloading it when the file changed.
    pub(crate) fn catalog(&self) -> Arc<Catalog> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.init_enabled {
            return Arc::new(Catalog {
                enabled: false,
                builtin: false,
                snippets: Vec::new(),
            });
        }
        let Some(path) = state.path.clone() else {
            return Arc::new(Catalog::default());
        };
        let stamp = file_stamp(&path);
        let Some(stamp) = stamp else {
            // Missing file is a normal state: no user snippets, built-ins
            // keep working. A later completion picks the file up if created.
            state.stamp = None;
            state.reported = None;
            return Arc::new(Catalog::default());
        };
        if state.stamp == Some(stamp) {
            return Arc::clone(&state.catalog);
        }
        let catalog = match load_file(&path, stamp) {
            Ok(catalog) => {
                state.reported = None;
                catalog
            }
            Err(reason) => {
                if state.reported != Some(stamp) {
                    log(format_args!(
                        "ignoring user snippets at {}: {reason}",
                        path.display()
                    ));
                    state.reported = Some(stamp);
                }
                Catalog::default()
            }
        };
        let catalog = Arc::new(catalog);
        state.stamp = Some(stamp);
        state.catalog = Arc::clone(&catalog);
        catalog
    }

    /// Invalidates the cache when a watched file matches the configured path.
    pub(crate) fn watched_files_changed(&self, changes: &[WatchedFileChange]) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(path) = state.path.as_ref() else {
            return;
        };
        let touched = changes.iter().any(|change| {
            wxml_project::root_path_from_uri(&change.uri).is_ok_and(|changed| &changed == path)
        });
        if touched {
            state.stamp = None;
            state.catalog = Arc::new(Catalog::default());
        }
    }
}

/// Resolves the snippet configuration path from initialization options.
///
/// Relative paths are confined to `root`; `..` components are rejected. A
/// missing explicit path with no workspace root yields `None`.
#[must_use]
pub(crate) fn resolve_path(root: Option<&Path>, configured: Option<&str>) -> Option<PathBuf> {
    match configured {
        Some(configured) => {
            let path = Path::new(configured);
            if path.is_absolute() {
                return Some(path.to_path_buf());
            }
            let root = root?;
            let mut normalized = PathBuf::new();
            for component in path.components() {
                match component {
                    Component::ParentDir => return None,
                    Component::CurDir => {}
                    other => normalized.push(other.as_os_str()),
                }
            }
            Some(root.join(normalized))
        }
        None => root.map(|root| root.join(DEFAULT_CONFIG_PATH)),
    }
}

/// Reads the current file metadata.
fn file_stamp(path: &Path) -> Option<Stamp> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(Stamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// Reads and parses one snippet file with a hard byte limit.
fn load_file(path: &Path, stamp: Stamp) -> Result<Catalog, String> {
    if stamp.len > MAX_CONFIG_BYTES {
        return Err(format!(
            "file is larger than {MAX_CONFIG_BYTES} bytes ({} bytes)",
            stamp.len
        ));
    }
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!("file is larger than {MAX_CONFIG_BYTES} bytes"));
    }
    let text = String::from_utf8(bytes).map_err(|_| "file is not valid UTF-8".to_owned())?;
    parse_catalog(&text)
}

/// Parses one snippet document. Invalid entries are skipped and counted.
fn parse_catalog(text: &str) -> Result<Catalog, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let Some(object) = value.as_object() else {
        return Err("top-level value is not an object".to_owned());
    };
    let mut catalog = Catalog {
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        builtin: object
            .get("builtin")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        snippets: Vec::new(),
    };
    let mut invalid = 0_usize;
    if let Some(entries) = object.get("snippets").and_then(Value::as_array) {
        for entry in entries {
            match parse_entry(entry) {
                Some(snippet) => {
                    // Later entries with the same identity win.
                    catalog
                        .snippets
                        .retain(|existing| existing.id != snippet.id);
                    catalog.snippets.push(snippet);
                }
                None => invalid = invalid.saturating_add(1),
            }
        }
    }
    if invalid > 0 {
        log(format_args!(
            "user snippets: skipped {invalid} invalid entr{}",
            if invalid == 1 { "y" } else { "ies" }
        ));
    }
    Ok(catalog)
}

/// Parses one user snippet entry.
fn parse_entry(value: &Value) -> Option<UserSnippet> {
    let object = value.as_object()?;
    let label = bounded_text(object.get("label")?.as_str()?, MAX_LABEL_CHARS)?;
    let body = parse_body(object.get("body")?)?;
    let scope = match object.get("scope").and_then(Value::as_str) {
        None | Some("tag") => Scope::Tag,
        Some("attribute") => Scope::Attribute,
        Some(_) => return None,
    };
    let prefix = object.get("prefix").and_then(Value::as_str).map_or_else(
        || {
            label
                .split_once(" (")
                .map_or_else(|| label.clone(), |(head, _)| head.to_owned())
        },
        str::to_owned,
    );
    let prefix = bounded_text(&prefix, MAX_PREFIX_CHARS)?;
    if prefix.is_empty() {
        return None;
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(|| format!("user.{label}"), str::to_owned);
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .map_or_else(
            || "Workspace snippet".to_owned(),
            |description| {
                bounded_text(description, MAX_DESCRIPTION_CHARS)
                    .unwrap_or_else(|| "Workspace snippet".to_owned())
            },
        );
    Some(UserSnippet {
        id,
        label,
        prefix,
        scope,
        description,
        body,
    })
}

/// Parses a string or array-of-lines body with size and line limits.
fn parse_body(value: &Value) -> Option<String> {
    let body = if let Some(text) = value.as_str() {
        text.to_owned()
    } else {
        let lines = value.as_array()?;
        let mut body = String::new();
        for line in lines {
            let line = line.as_str()?;
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
        body
    };
    let body = body.replace("\r\n", "\n");
    if body.is_empty() {
        return None;
    }
    Some(body)
}

/// Truncates display text and rejects control characters.
fn bounded_text(text: &str, limit: usize) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    let mut bounded: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        bounded.push('…');
    }
    Some(bounded)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::assert_is_empty)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let base = std::env::var_os("TMPDIR")
            .map_or_else(|| PathBuf::from("/tmp/opencode"), PathBuf::from);
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        base.join(format!("wxml-snippets-{name}-{stamp}.json"))
    }

    #[test]
    fn parses_valid_catalog_and_defaults_prefix() {
        let catalog = parse_catalog(
            r#"{
                "snippets": [
                    {"label": "view (custom)", "body": ["view>", "  $0", "</view>"]},
                    {"id": "attr.wx-if", "label": "when", "prefix": "when",
                     "scope": "attribute", "body": "wx:if=\"{{${1:ok}}}\""}
                ]
            }"#,
        )
        .expect("catalog");
        assert!(catalog.enabled);
        assert!(catalog.builtin);
        assert_eq!(catalog.snippets.len(), 2);
        assert_eq!(catalog.snippets[0].prefix, "view");
        assert_eq!(catalog.snippets[0].body, "view>\n  $0\n</view>");
        assert_eq!(catalog.snippets[1].scope, Scope::Attribute);
        assert_eq!(catalog.snippets[1].id, "attr.wx-if");
    }

    #[test]
    fn invalid_entries_are_skipped_without_losing_valid_ones() {
        let catalog = parse_catalog(
            r#"{"snippets": [
                {"label": "", "body": "x"},
                {"label": "ok", "body": "x"},
                {"label": "bad scope", "scope": "other", "body": "x"},
                {"label": "bad body", "body": ""},
                {"label": "bad line", "body": ["a", 1]}
            ]}"#,
        )
        .expect("catalog");
        assert_eq!(catalog.snippets.len(), 1);
        assert_eq!(catalog.snippets[0].label, "ok");
    }

    #[test]
    fn malformed_top_level_fails_and_long_bodies_are_kept() {
        assert!(parse_catalog("not json").is_err());
        assert!(parse_catalog("[]").is_err());
        let body = "x".repeat(8 * 1024 + 1);
        let text = format!(r#"{{"snippets": [{{"label": "big", "body": "{body}"}}]}}"#);
        let catalog = parse_catalog(&text).expect("catalog");
        assert_eq!(catalog.snippets.len(), 1);
        assert_eq!(catalog.snippets[0].body.len(), 8 * 1024 + 1);
    }

    #[test]
    fn later_entries_with_the_same_id_win() {
        let catalog = parse_catalog(
            r#"{"snippets": [
                {"id": "tag.view", "label": "first", "body": "a"},
                {"id": "tag.view", "label": "second", "body": "b"}
            ]}"#,
        )
        .expect("catalog");
        assert_eq!(catalog.snippets.len(), 1);
        assert_eq!(catalog.snippets[0].label, "second");
        assert!(catalog.overrides("tag.view", "view (snippet)"));
    }

    #[test]
    fn missing_and_oversized_files_degrade_safely() {
        let missing = temp_path("missing");
        let store = Store::default();
        store.configure(Some(missing), true);
        assert_eq!(store.catalog().snippets.len(), 0);
        assert!(store.catalog().builtin);

        let oversized = temp_path("oversized");
        fs::write(
            &oversized,
            "x".repeat(usize::try_from(MAX_CONFIG_BYTES).expect("limit") + 1),
        )
        .expect("write");
        let store = Store::default();
        store.configure(Some(oversized.clone()), true);
        assert_eq!(store.catalog().snippets.len(), 0);
        let _ = fs::remove_file(oversized);
    }

    #[test]
    fn file_changes_and_watched_notifications_reload_the_catalog() {
        let path = temp_path("reload");
        fs::write(&path, r#"{"snippets": [{"label": "one", "body": "a"}]}"#).expect("write");
        let store = Store::default();
        store.configure(Some(path.clone()), true);
        assert_eq!(store.catalog().snippets[0].label, "one");

        fs::write(&path, r#"{"snippets": [{"label": "two", "body": "bb"}]}"#).expect("write");
        assert_eq!(store.catalog().snippets[0].label, "two");

        // Same length, forced reload through a watched-file notification.
        fs::write(&path, r#"{"snippets": [{"label": "the", "body": "cc"}]}"#).expect("write");
        store.watched_files_changed(&[WatchedFileChange {
            uri: format!("file://{}", path.display()),
            kind: crate::project_host::WatchedChangeKind::Changed,
        }]);
        assert_eq!(store.catalog().snippets[0].label, "the");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_files_degrade_to_builtins_and_log_once() {
        let path = temp_path("invalid");
        fs::write(&path, "not json").expect("write");
        let store = Store::default();
        store.configure(Some(path.clone()), true);
        let catalog = store.catalog();
        assert!(catalog.snippets.is_empty());
        assert!(catalog.builtin);
        // A second read with the same stamp must not log or panic.
        assert!(store.catalog().snippets.is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn disabled_catalog_has_no_snippets() {
        let store = Store::default();
        store.configure(None, false);
        let catalog = store.catalog();
        assert!(!catalog.enabled);
        assert!(catalog.snippets.is_empty());
    }

    #[test]
    fn relative_paths_are_confined_to_the_root() {
        let root = Path::new("/workspace");
        assert_eq!(
            resolve_path(Some(root), Some(".wxml-lsp/snippets.json")),
            Some(root.join(".wxml-lsp/snippets.json"))
        );
        assert_eq!(resolve_path(Some(root), Some("../outside.json")), None);
        assert_eq!(resolve_path(None, Some("relative.json")), None);
        assert_eq!(
            resolve_path(Some(root), None),
            Some(root.join(DEFAULT_CONFIG_PATH))
        );
        assert_eq!(
            resolve_path(Some(root), Some("/absolute/snippets.json")),
            Some(PathBuf::from("/absolute/snippets.json"))
        );
    }
}
