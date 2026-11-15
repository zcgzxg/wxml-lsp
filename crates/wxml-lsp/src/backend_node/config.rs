//! Configuration for the Node type backend.
//!
//! Backend paths are security-sensitive: a workspace can contain a file named
//! `backends/typescript/dist/server.js`, so a relative path must never be
//! resolved against the process working directory. The default is either an
//! executable-relative installation layout or nothing; `WXML_TYPE_BACKEND` and
//! `WXML_BACKEND_DIR` are trusted configuration and must be absolute.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default NDJSON line ceiling accepted from the Node process.
///
/// This bounds JSON parsing and the typed response in one place. A valid C1
/// response for the configured request limits stays far below 8 MiB.
pub const DEFAULT_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Default request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// Default cooldown after three consecutive backend failures.
pub const DEFAULT_RESTART_COOLDOWN: Duration = Duration::from_secs(2);

/// Installation layout relative to the executable directory.
const SERVER_RELATIVE: &str = "backends/typescript/dist/server.js";

/// Alternate documented installation layout relative to the executable.
const SERVER_INSTALL_RELATIVE: &str = "wxml-type-backend/server.js";

/// C1 request bounds. The Node server clamps these to its own ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestLimits {
    /// Maximum number of `files[]` entries.
    pub max_files: usize,
    /// Maximum total `files[].text` bytes.
    pub max_total_bytes: usize,
    /// Maximum number of fragments.
    pub max_fragments: usize,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_files: 512,
            max_total_bytes: 8 * 1024 * 1024,
            max_fragments: 2048,
        }
    }
}

/// Typings package requested from the Node backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Typings {
    /// Package name.
    pub package: String,
    /// Exact version.
    pub version: String,
}

impl Default for Typings {
    fn default() -> Self {
        Self {
            package: "miniprogram-api-typings".to_owned(),
            version: "5.3.1".to_owned(),
        }
    }
}

/// JS check policy forwarded in the C1 request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JsCheckMode {
    /// Use the project's `checkJs` setting.
    Inherit,
    /// Inject `// @ts-check` into the derived view.
    #[default]
    Pragma,
    /// Disable template diagnostics.
    Off,
}

impl JsCheckMode {
    /// Returns the C1 wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Pragma => "pragma",
            Self::Off => "off",
        }
    }

    /// Parses `WXML_TEMPLATE_CHECK`; unknown values fall back to `pragma`.
    #[must_use]
    pub fn from_value(value: &str) -> Self {
        match value {
            "off" => Self::Off,
            "inherit" => Self::Inherit,
            _ => Self::Pragma,
        }
    }
}

/// How to start and query the Node type backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeBackendConfig {
    /// Node executable (a PATH name or an absolute path).
    pub node: PathBuf,
    /// Absolute `server.js` entry point, or `None` when not installed.
    pub server: Option<PathBuf>,
    /// Absolute directory containing the backend's `node_modules`.
    pub backend_dir: Option<PathBuf>,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Maximum NDJSON response line.
    pub max_line_bytes: usize,
    /// C1 limits.
    pub limits: RequestLimits,
    /// Typings package/version.
    pub typings: Typings,
    /// JS check policy.
    pub js_check: JsCheckMode,
    /// Cooldown after three consecutive failures.
    pub restart_cooldown: Duration,
    /// Configuration error, if a trusted environment value was rejected.
    pub config_error: Option<String>,
}

impl Default for NodeBackendConfig {
    fn default() -> Self {
        let server = default_server_path();
        let backend_dir = server.as_ref().map(|server| backend_dir_for(server));
        Self {
            node: PathBuf::from("node"),
            server,
            backend_dir,
            timeout: DEFAULT_TIMEOUT,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            limits: RequestLimits::default(),
            typings: Typings::default(),
            js_check: JsCheckMode::Pragma,
            restart_cooldown: DEFAULT_RESTART_COOLDOWN,
            config_error: None,
        }
    }
}

impl NodeBackendConfig {
    /// Reads configuration from the documented `WXML_*` environment variables.
    ///
    /// The environment is trusted configuration. `WXML_TYPE_BACKEND` and
    /// `WXML_BACKEND_DIR` must be absolute; relative values disable the backend
    /// instead of resolving against the editor's working directory.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// Variant of [`Self::from_env`] with an injectable lookup for tests.
    #[must_use]
    pub fn from_lookup<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let mut config = Self::default();
        if let Some(node) = lookup("WXML_NODE") {
            let node = PathBuf::from(node);
            let mut components = node.components();
            let single_normal = matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none();
            if !node.is_absolute() && !single_normal {
                return clear_backend(
                    config,
                    "WXML_NODE must be a command name or an absolute path",
                );
            }
            config.node = node;
        }
        if let Some(server) = lookup("WXML_TYPE_BACKEND").map(PathBuf::from) {
            if !server.is_absolute() {
                return clear_backend(config, "WXML_TYPE_BACKEND must be an absolute path");
            }
            config.server = Some(server);
        }
        if let Some(dir) = lookup("WXML_BACKEND_DIR").map(PathBuf::from) {
            if !dir.is_absolute() {
                return clear_backend(config, "WXML_BACKEND_DIR must be an absolute path");
            }
            config.backend_dir = Some(dir);
        } else if let Some(server) = config.server.as_ref() {
            config.backend_dir = Some(backend_dir_for(server));
        }
        if config.server.is_none() {
            config.backend_dir = None;
        }
        if let Some(value) = lookup("WXML_ANALYZE_TIMEOUT_MS") {
            if let Ok(value) = value.to_string_lossy().parse::<u64>() {
                if value > 0 {
                    config.timeout = Duration::from_millis(value);
                }
            }
        }
        if let Some(value) = lookup("WXML_TEMPLATE_CHECK") {
            config.js_check = JsCheckMode::from_value(&value.to_string_lossy());
        }
        config
    }

    /// Returns why the backend cannot start, if it is disabled.
    #[must_use]
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.config_error.as_deref().or_else(|| {
            self.server.is_none().then_some(
                "type backend script is not installed; set WXML_TYPE_BACKEND to an absolute path",
            )
        })
    }
}

/// Disables the backend and records a path-free reason.
fn clear_backend(mut config: NodeBackendConfig, reason: &str) -> NodeBackendConfig {
    config.server = None;
    config.backend_dir = None;
    config.config_error = Some(reason.to_owned());
    config
}

/// Returns the executable-relative server path if one is actually installed.
fn default_server_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let directory = exe.parent()?;
    default_server_for(directory)
}

/// Probes only executable-relative layouts; the process cwd is never used.
fn default_server_for(executable_dir: &Path) -> Option<PathBuf> {
    [SERVER_RELATIVE, SERVER_INSTALL_RELATIVE]
        .into_iter()
        .map(|relative| executable_dir.join(relative))
        .find(|candidate| candidate.is_file())
}

/// Derives the directory containing `node_modules` from the server path.
fn backend_dir_for(server: &Path) -> PathBuf {
    let parent = server.parent();
    let is_dist_server = server.file_name().is_some_and(|name| name == "server.js")
        && parent
            .and_then(Path::file_name)
            .is_some_and(|name| name == "dist");
    if is_dist_server {
        parent
            .and_then(Path::parent)
            .map_or_else(|| server.to_path_buf(), Path::to_path_buf)
    } else {
        parent.map_or_else(|| server.to_path_buf(), Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn js_check_wire_values_match_c1() {
        assert_eq!(JsCheckMode::Inherit.as_str(), "inherit");
        assert_eq!(JsCheckMode::Pragma.as_str(), "pragma");
        assert_eq!(JsCheckMode::Off.as_str(), "off");
        assert_eq!(JsCheckMode::from_value("off"), JsCheckMode::Off);
        assert_eq!(JsCheckMode::from_value("inherit"), JsCheckMode::Inherit);
        assert_eq!(JsCheckMode::from_value("bogus"), JsCheckMode::Pragma);
    }

    #[test]
    fn derives_the_backend_directory_from_the_server_path() {
        assert_eq!(
            backend_dir_for(Path::new("/repo/backends/typescript/dist/server.js")),
            PathBuf::from("/repo/backends/typescript")
        );
        assert_eq!(
            backend_dir_for(Path::new("/repo/custom/server.js")),
            PathBuf::from("/repo/custom")
        );
    }

    #[test]
    fn executable_relative_layout_is_probed() {
        let base = std::env::temp_dir().join(format!(
            "wxml-p7-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let server = base.join(SERVER_RELATIVE);
        std::fs::create_dir_all(server.parent().expect("parent")).expect("dirs");
        std::fs::write(&server, "// backend").expect("write");
        assert_eq!(default_server_for(&base), Some(server));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cwd_backend_is_never_used_as_a_default() {
        let _guard = CWD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let base = std::env::temp_dir().join(format!(
            "wxml-p7-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let malicious = base.join(SERVER_RELATIVE);
        std::fs::create_dir_all(malicious.parent().expect("parent")).expect("dirs");
        std::fs::write(&malicious, "// malicious").expect("write");
        let previous = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&base).expect("set cwd");
        let config = NodeBackendConfig::from_lookup(|_| None);
        std::env::set_current_dir(previous).expect("restore cwd");
        assert!(
            config
                .server
                .as_ref()
                .is_none_or(|server| !server.starts_with(&base)),
            "cwd-controlled backend must never be selected: {:?}",
            config.server
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn absolute_environment_paths_are_accepted() {
        let config = NodeBackendConfig::from_lookup(|key| match key {
            "WXML_TYPE_BACKEND" => Some(OsString::from("/opt/wxml/dist/server.js")),
            "WXML_BACKEND_DIR" => Some(OsString::from("/opt/wxml")),
            _ => None,
        });
        assert_eq!(
            config.server,
            Some(PathBuf::from("/opt/wxml/dist/server.js"))
        );
        assert_eq!(config.backend_dir, Some(PathBuf::from("/opt/wxml")));
        assert!(config.config_error.is_none());
    }

    #[test]
    fn relative_environment_paths_fail_closed() {
        let config = NodeBackendConfig::from_lookup(|key| match key {
            "WXML_TYPE_BACKEND" => Some(OsString::from("backends/typescript/dist/server.js")),
            _ => None,
        });
        assert!(config.server.is_none());
        assert!(config.backend_dir.is_none());
        assert!(config.config_error.is_some());
        assert!(config.unavailable_reason().is_some());

        let config = NodeBackendConfig::from_lookup(|key| match key {
            "WXML_BACKEND_DIR" => Some(OsString::from("backends/typescript")),
            _ => None,
        });
        assert!(config.server.is_none());
        assert!(config.backend_dir.is_none());
        assert!(config.config_error.is_some());
    }

    #[test]
    fn node_must_be_a_name_or_absolute_path() {
        let name = NodeBackendConfig::from_lookup(|key| match key {
            "WXML_NODE" => Some(OsString::from("node")),
            _ => None,
        });
        assert_eq!(name.node, PathBuf::from("node"));

        let absolute = NodeBackendConfig::from_lookup(|key| match key {
            "WXML_NODE" => Some(OsString::from("/usr/bin/node")),
            _ => None,
        });
        assert_eq!(absolute.node, PathBuf::from("/usr/bin/node"));

        for relative in ["../node", "./node", "bin/node", "..", ".", ""] {
            let config = NodeBackendConfig::from_lookup(|key| match key {
                "WXML_NODE" => Some(OsString::from(relative)),
                _ => None,
            });
            assert!(config.config_error.is_some(), "{relative}");
            assert!(config.server.is_none(), "{relative}");
        }
    }

    #[test]
    fn unavailable_reason_is_path_free() {
        let config = NodeBackendConfig::from_lookup(|_| None);
        let reason = config.unavailable_reason().unwrap_or("configured");
        assert!(!reason.contains('/'));
        assert!(!reason.contains('\\'));
    }
}
