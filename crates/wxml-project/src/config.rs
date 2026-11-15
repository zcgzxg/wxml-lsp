//! Minimal `tsconfig.json` model: `extends`, `baseUrl`, `paths` and file sets.
//!
//! The model is intentionally smaller than TypeScript project resolution. It
//! resolves `paths` aliases to project-relative paths so template code can be
//! mapped to scripts; it does not interpret `moduleResolution`, project
//! references, or package-style `extends` values.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::files::{FileError, MAX_MESSAGE_BYTES, PathError, ProjectPath, truncate_message};
use crate::overlay::Overlay;

/// Maximum number of chained `extends` files loaded.
pub const MAX_EXTENDS_DEPTH: usize = 8;

/// Maximum number of configuration files loaded in one [`load_ts_config`]
/// call. Shared parents are memoized, so a diamond graph counts each file once.
pub const MAX_CONFIG_LOADS: usize = 64;

/// Maximum total source bytes read from configuration files in one call.
///
/// Individual files are already bounded by [`crate::files::MAX_FILE_BYTES`];
/// this budget prevents a 64-file chain from reading a gigabyte. Cache hits do
/// not count again.
pub const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// Maximum number of `extends` edges processed in one call.
///
/// The file-load budget does not bound cache-hit edges: a small config can
/// list a huge `extends` array that repeatedly mounts a cached parent. This
/// limit keeps merge work and transient layer clones bounded.
pub const MAX_CONFIG_EXTENDS_EDGES: usize = 4_096;

/// Maximum serialized bytes of the merged `compilerOptions` in one layer.
///
/// Every cached layer keeps a merged copy; without this budget a small set of
/// large `extends` files can multiply into hundreds of MiB of cached options.
pub const MAX_MERGED_OPTION_BYTES: usize = 2 * 1024 * 1024;

/// Maximum number of unknown `compilerOptions` keys retained per config file.
pub const MAX_EXTRA_OPTION_KEYS: usize = 512;

/// Maximum serialized bytes for one unknown `compilerOptions` value.
pub const MAX_OPTION_VALUE_BYTES: usize = 64 * 1024;

/// The resolved, minimal view of one `tsconfig.json`.
// serde_json::Value is not Eq because it can contain floats; PartialEq is the
// strongest useful bound.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Clone, PartialEq)]
pub struct TsConfig {
    /// Project-relative location of the config that was loaded.
    pub path: ProjectPath,
    /// Effective base for alias targets: the declared `baseUrl` or the
    /// config's directory, normalized to a project path. `None` means the
    /// project root.
    pub base_url: Option<ProjectPath>,
    /// Directly extended configs, in declaration order.
    pub extends: Vec<ProjectPath>,
    /// `compilerOptions.paths` patterns mapped to project-relative targets.
    /// Targets keep their `*` placeholder.
    pub paths: BTreeMap<String, Vec<ProjectPath>>,
    /// Raw `include` patterns from the nearest config that declared them.
    pub include: Vec<String>,
    /// Raw `exclude` patterns from the nearest config that declared them.
    pub exclude: Vec<String>,
    /// Merged raw `compilerOptions` from the whole `extends` chain.
    ///
    /// Parent layers are applied first; scalar and array values from a child
    /// replace the parent value. The `paths` object is merged key by key, with
    /// the child winning on duplicate patterns; other objects are replaced as
    /// a whole. Unknown options are preserved so the LSP can forward filtered
    /// values to the type backend.
    ///
    /// `baseUrl` and `paths` are rewritten to final project-root-relative
    /// values (with `*` preserved). When paths or a declared `baseUrl`
    /// remain, raw `baseUrl` is always `"."` and path targets are already
    /// project-relative; consumers must not append the config directory
    /// again. If neither remains, both keys are removed.
    pub compiler_options: BTreeMap<String, Value>,
}

impl fmt::Debug for TsConfig {
    /// Formats config metadata without dumping raw `compilerOptions` values.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TsConfig")
            .field("path", &self.path)
            .field("base_url", &self.base_url)
            .field("extends", &self.extends)
            .field("paths", &self.paths)
            .field("include", &self.include)
            .field("exclude", &self.exclude)
            .field("compiler_options", &self.compiler_options.len())
            .finish()
    }
}

impl TsConfig {
    /// Resolves an import specifier through `compilerOptions.paths`.
    ///
    /// Exact patterns win. Otherwise a single-`*` pattern whose prefix and
    /// suffix match is selected, preferring the longest prefix, and the first
    /// target containing `*` is substituted. Relative and root-absolute
    /// specifiers are not aliases. Returns `None` when nothing matches or the
    /// expanded path would be invalid.
    #[must_use]
    pub fn resolve_alias(&self, specifier: &str) -> Option<ProjectPath> {
        if specifier.is_empty() || specifier.starts_with('.') || specifier.starts_with('/') {
            return None;
        }
        if let Some(targets) = self.paths.get(specifier) {
            return targets.first().cloned();
        }
        let mut best: Option<(usize, &str, &ProjectPath)> = None;
        for (pattern, targets) in &self.paths {
            let mut parts = pattern.split('*');
            let Some(prefix) = parts.next() else {
                continue;
            };
            let Some(suffix) = parts.next() else {
                continue;
            };
            if parts.next().is_some() {
                continue;
            }
            let Some(rest) = specifier.strip_prefix(prefix) else {
                continue;
            };
            let Some(capture) = rest.strip_suffix(suffix) else {
                continue;
            };
            let Some(target) = targets.iter().find(|target| target.as_str().contains('*')) else {
                continue;
            };
            if best.is_none_or(|(length, _, _)| prefix.len() > length) {
                best = Some((prefix.len(), capture, target));
            }
        }
        let (_, capture, target) = best?;
        let expanded = target.as_str().replacen('*', capture, 1);
        ProjectPath::new(expanded).ok()
    }
}

/// Why a `tsconfig.json` could not be loaded.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// The config or an extended config does not exist.
    #[error("configuration file not found: {path}")]
    NotFound {
        /// Project-relative path of the missing file.
        path: ProjectPath,
    },
    /// A symlink or path component leads outside the project root.
    #[error("configuration path escapes the project root: {path}")]
    EscapesRoot {
        /// Project-relative path of the config.
        path: ProjectPath,
    },
    /// The config is not valid UTF-8.
    #[error("configuration is not valid UTF-8: {path}")]
    InvalidUtf8 {
        /// Project-relative path of the config.
        path: ProjectPath,
    },
    /// Reading the config failed.
    #[error("failed to read configuration {path}: {kind:?}")]
    Io {
        /// Project-relative path of the config.
        path: ProjectPath,
        /// Operating-system error category.
        kind: std::io::ErrorKind,
    },
    /// The config is not valid JSON (comments and trailing commas allowed).
    #[error("invalid JSON in {path} at line {line}, column {column}: {message}")]
    Json {
        /// Project-relative path of the config.
        path: ProjectPath,
        /// One-based line number.
        line: usize,
        /// One-based column number.
        column: usize,
        /// Parser message.
        message: String,
    },
    /// An `extends` entry does not name an existing file.
    #[error("{path} extends a missing config: {target}")]
    ExtendsNotFound {
        /// Project-relative path of the config that declared `extends`.
        path: ProjectPath,
        /// Project-relative target that is missing.
        target: ProjectPath,
    },
    /// An `extends` chain repeats a config.
    #[error("configuration extends cycle at {path}")]
    ExtendsCycle {
        /// Project-relative path of the repeated config.
        path: ProjectPath,
    },
    /// An `extends` chain is deeper than [`MAX_EXTENDS_DEPTH`].
    #[error("configuration extends depth exceeded at {path}")]
    ExtendsDepthExceeded {
        /// Project-relative path where the limit was hit.
        path: ProjectPath,
    },
    /// One call tried to load more than [`MAX_CONFIG_LOADS`] configuration
    /// files. The cache does not help when every file is distinct.
    #[error("configuration load budget exceeded at {path}")]
    TooManyConfigs {
        /// Project-relative path that would have been loaded next.
        path: ProjectPath,
    },
    /// One call read more than [`MAX_CONFIG_BYTES`] of configuration source.
    #[error("configuration byte budget exceeded at {path} ({limit} bytes)")]
    TooManyConfigBytes {
        /// Project-relative path that pushed the budget over the limit.
        path: ProjectPath,
        /// Per-call byte budget.
        limit: u64,
    },
    /// One call processed more than [`MAX_CONFIG_EXTENDS_EDGES`] `extends`
    /// edges.
    #[error("configuration extends graph is too large at {path}")]
    TooManyExtends {
        /// Project-relative path that declared the excess edge.
        path: ProjectPath,
        /// Per-call edge budget.
        limit: usize,
    },
    /// Merged or single-value `compilerOptions` exceeded its serialized byte
    /// budget. The message intentionally omits paths and values.
    #[error("compilerOptions exceeded its {limit}-byte budget ({bytes} bytes)")]
    TooManyOptions {
        /// Observed serialized size in bytes.
        bytes: usize,
        /// Applicable budget in bytes.
        limit: usize,
    },
    /// One config declared too many unknown `compilerOptions` keys.
    #[error("compilerOptions has too many unknown keys ({keys}, limit {limit})")]
    TooManyOptionKeys {
        /// Observed unknown key count.
        keys: usize,
        /// Applicable key budget.
        limit: usize,
    },
    /// The configuration file is larger than the project read limit.
    #[error("configuration is too large: {path} ({size} bytes)")]
    TooLarge {
        /// Project-relative path of the configuration file.
        path: ProjectPath,
        /// Observed size in bytes.
        size: u64,
    },
    /// An `extends` value is not a supported relative path.
    #[error("unsupported extends value in {path}: {specifier}")]
    UnsupportedExtends {
        /// Project-relative path of the config.
        path: ProjectPath,
        /// Raw `extends` value.
        specifier: String,
    },
    /// A configured path is not a valid project path.
    #[error("invalid path in {path}: {error}")]
    InvalidPath {
        /// Project-relative path of the config.
        path: ProjectPath,
        /// Path validation error.
        error: PathError,
    },
}

/// Loads and merges a `tsconfig.json`, following relative `extends` entries.
///
/// `root` must be an absolute, canonical project root. Overlay text wins over
/// disk for every config in the chain. `extends` values are resolved relative
/// to the declaring config; a missing `.json` extension is added, and a
/// trailing `/` means `<dir>/tsconfig.json`. Package-style `extends` values
/// are rejected with [`ConfigError::UnsupportedExtends`].
///
/// One call loads at most [`MAX_CONFIG_LOADS`] distinct files, reads at most
/// [`MAX_CONFIG_BYTES`] of source, processes at most
/// [`MAX_CONFIG_EXTENDS_EDGES`] `extends` edges, is at most
/// [`MAX_EXTENDS_DEPTH`] levels deep, and detects cycles. Loaded layers are
/// memoized for the duration of the call, so shared parents in a diamond graph
/// are read once. The cache is **not** shared across calls and therefore never
/// leaks overlay revisions between [`load_ts_config`] invocations.
///
/// # Errors
///
/// Returns [`ConfigError`] when a config is missing, unreadable, too large,
/// not valid JSON, escapes the root, forms a cycle, exceeds the depth limit,
/// or exhausts the per-call load budget.
pub fn load_ts_config(
    root: &Path,
    path: &ProjectPath,
    overlay: &Overlay,
) -> Result<TsConfig, ConfigError> {
    let mut loader = Loader::new(root, overlay);
    let layer = loader.load(path, 0)?;
    let base_url = if layer.base_url_declared {
        layer.base_url
    } else {
        path.parent()
    };
    let mut compiler_options = layer.compiler_options;
    rewrite_compiler_options(&mut compiler_options, &layer.paths);
    Ok(TsConfig {
        path: path.clone(),
        base_url,
        extends: layer.direct_extends,
        paths: layer.paths,
        include: layer.include.unwrap_or_default(),
        exclude: layer.exclude.unwrap_or_default(),
        compiler_options,
    })
}

/// One config after merging its `extends` chain and resolving its own paths.
#[derive(Clone, Debug, Default)]
struct Layer {
    base_url: Option<ProjectPath>,
    base_url_declared: bool,
    paths: BTreeMap<String, Vec<ProjectPath>>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    direct_extends: Vec<ProjectPath>,
    compiler_options: BTreeMap<String, Value>,
}

impl Layer {
    /// Applies a parent layer; later parents and the child override earlier
    /// values.
    fn apply_parent(&mut self, parent: Self) {
        if parent.base_url_declared {
            self.base_url = parent.base_url;
            self.base_url_declared = true;
        }
        self.paths.extend(parent.paths);
        if parent.include.is_some() {
            self.include = parent.include;
        }
        if parent.exclude.is_some() {
            self.exclude = parent.exclude;
        }
        merge_compiler_options(&mut self.compiler_options, parent.compiler_options);
    }
}

/// Raw JSON shape accepted for one config file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawConfig {
    #[serde(default)]
    extends: Option<RawExtends>,
    #[serde(default)]
    compiler_options: Option<RawCompilerOptions>,
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
}

/// `extends` accepts a string or, since TypeScript 5, an array of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawExtends {
    One(String),
    Many(Vec<String>),
}

impl RawExtends {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

/// The subset of `compilerOptions` that this model understands.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCompilerOptions {
    base_url: Option<String>,
    paths: Option<BTreeMap<String, Vec<String>>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// Per-call loader state: cycle stack, memoized layers and load budget.
///
/// The cache is intentionally scoped to one [`load_ts_config`] call. Reusing
/// it across calls would serve stale text after an overlay update.
struct Loader<'a> {
    root: &'a Path,
    overlay: &'a Overlay,
    visiting: BTreeSet<ProjectPath>,
    cache: BTreeMap<ProjectPath, Layer>,
    loads: usize,
    bytes: u64,
    edges: usize,
}

impl<'a> Loader<'a> {
    /// Creates an empty loader for one call.
    const fn new(root: &'a Path, overlay: &'a Overlay) -> Self {
        Self {
            root,
            overlay,
            visiting: BTreeSet::new(),
            cache: BTreeMap::new(),
            loads: 0,
            bytes: 0,
            edges: 0,
        }
    }

    /// Loads one config, memoizing the merged result and enforcing limits.
    fn load(&mut self, path: &ProjectPath, depth: usize) -> Result<Layer, ConfigError> {
        if depth > MAX_EXTENDS_DEPTH {
            return Err(ConfigError::ExtendsDepthExceeded { path: path.clone() });
        }
        if let Some(cached) = self.cache.get(path) {
            return Ok(cached.clone());
        }
        if !self.visiting.insert(path.clone()) {
            return Err(ConfigError::ExtendsCycle { path: path.clone() });
        }
        let result = if self.loads >= MAX_CONFIG_LOADS {
            Err(ConfigError::TooManyConfigs { path: path.clone() })
        } else {
            self.loads += 1;
            self.load_body(path, depth)
        };
        self.visiting.remove(path);
        match result {
            Ok(layer) => {
                validate_merged_options(&layer.compiler_options)?;
                self.cache.insert(path.clone(), layer.clone());
                Ok(layer)
            }
            Err(error) => Err(error),
        }
    }

    /// Parses one config file and merges its parents.
    fn load_body(&mut self, path: &ProjectPath, depth: usize) -> Result<Layer, ConfigError> {
        let Some(text) = self
            .overlay
            .read_text(self.root, path)
            .map_err(|error| ConfigError::from_file(path, &error))?
        else {
            return Err(ConfigError::NotFound { path: path.clone() });
        };
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
        if self.bytes > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooManyConfigBytes {
                path: path.clone(),
                limit: MAX_CONFIG_BYTES,
            });
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let sanitized = sanitize_jsonc(text);
        let raw: RawConfig =
            serde_json::from_str(&sanitized).map_err(|error| ConfigError::Json {
                path: path.clone(),
                line: error.line(),
                column: error.column(),
                message: truncate_message(&error.to_string(), MAX_MESSAGE_BYTES).to_owned(),
            })?;
        let RawConfig {
            extends,
            compiler_options,
            include,
            exclude,
        } = raw;

        let directory = path.parent();
        let compiler_options = compiler_options.unwrap_or_default();
        validate_extra_options(&compiler_options.extra)?;
        let child_compiler_options = raw_compiler_options(&compiler_options);
        let declared_base_url = match compiler_options.base_url.as_deref() {
            Some(value) => Some(resolve_directory_value(directory.as_ref(), value, path)?),
            None => None,
        };
        let paths_base = declared_base_url
            .clone()
            .flatten()
            .or_else(|| directory.clone());
        let child_paths = resolve_paths(compiler_options.paths, paths_base.as_ref(), path)?;

        let mut merged = Layer::default();
        let mut direct_extends = Vec::new();
        let mut applied = BTreeSet::new();
        for specifier in extends.map(RawExtends::into_vec).unwrap_or_default() {
            if self.edges >= MAX_CONFIG_EXTENDS_EDGES {
                return Err(ConfigError::TooManyExtends {
                    path: path.clone(),
                    limit: MAX_CONFIG_EXTENDS_EDGES,
                });
            }
            self.edges += 1;
            let target = resolve_extends(directory.as_ref(), &specifier, path)?;
            // Repeated `extends` targets are idempotent: load and merge each
            // distinct parent once, in first-appearance order.
            if !applied.insert(target.clone()) {
                continue;
            }
            let parent = self.load(&target, depth + 1).map_err(|error| match error {
                ConfigError::NotFound { .. } => ConfigError::ExtendsNotFound {
                    path: path.clone(),
                    target: target.clone(),
                },
                other => other,
            })?;
            direct_extends.push(target);
            merged.apply_parent(parent);
        }

        if let Some(base_url) = declared_base_url {
            merged.base_url = base_url;
            merged.base_url_declared = true;
        }
        merged.paths.extend(child_paths);
        merge_compiler_options(&mut merged.compiler_options, child_compiler_options);
        if include.is_some() {
            merged.include = include;
        }
        if exclude.is_some() {
            merged.exclude = exclude;
        }
        merged.direct_extends = direct_extends;
        Ok(merged)
    }
}

/// Rebuilds the raw `compilerOptions` object, including unknown keys.
fn raw_compiler_options(options: &RawCompilerOptions) -> BTreeMap<String, Value> {
    let mut map = options.extra.clone();
    if let Some(base_url) = &options.base_url {
        map.insert("baseUrl".to_owned(), Value::String(base_url.clone()));
    }
    if let Some(paths) = &options.paths {
        let mut object = Map::new();
        for (pattern, targets) in paths {
            object.insert(
                pattern.clone(),
                Value::Array(targets.iter().cloned().map(Value::String).collect()),
            );
        }
        map.insert("paths".to_owned(), Value::Object(object));
    }
    map
}

/// Merges `source` options over `target`.
///
/// Scalar/array values replace the previous value; the `paths` object is
/// merged key by key so a child can add patterns while overriding individual
/// ones. Other objects are replaced as a whole.
fn merge_compiler_options(target: &mut BTreeMap<String, Value>, source: BTreeMap<String, Value>) {
    for (key, value) in source {
        if key == "paths" {
            if let Value::Object(incoming) = value {
                if let Some(Value::Object(mut existing)) = target.remove("paths") {
                    existing.extend(incoming);
                    target.insert("paths".to_owned(), Value::Object(existing));
                } else {
                    target.insert("paths".to_owned(), Value::Object(incoming));
                }
            } else {
                target.insert(key, value);
            }
            continue;
        }
        target.insert(key, value);
    }
}

/// Rewrites raw `baseUrl`/`paths` to project-root-relative final values.
///
/// If any `paths` or a declared `baseUrl` remains, `baseUrl` is fixed to `.`
/// and path targets are already project-relative. The backend must resolve
/// them from the project root and must not add the config directory again.
fn rewrite_compiler_options(
    options: &mut BTreeMap<String, Value>,
    paths: &BTreeMap<String, Vec<ProjectPath>>,
) {
    let had_base_url = options.contains_key("baseUrl");
    if paths.is_empty() && !had_base_url {
        options.remove("baseUrl");
        options.remove("paths");
        return;
    }
    options.insert("baseUrl".to_owned(), Value::String(".".to_owned()));
    if paths.is_empty() {
        options.remove("paths");
        return;
    }
    let mut object = Map::new();
    for (pattern, targets) in paths {
        object.insert(
            pattern.clone(),
            Value::Array(
                targets
                    .iter()
                    .map(|target| Value::String(target.as_str().to_owned()))
                    .collect(),
            ),
        );
    }
    options.insert("paths".to_owned(), Value::Object(object));
}

/// Validates unknown option keys and values from one config file.
fn validate_extra_options(extra: &BTreeMap<String, Value>) -> Result<(), ConfigError> {
    if extra.len() > MAX_EXTRA_OPTION_KEYS {
        return Err(ConfigError::TooManyOptionKeys {
            keys: extra.len(),
            limit: MAX_EXTRA_OPTION_KEYS,
        });
    }
    for value in extra.values() {
        let bytes = serde_json::to_vec(value).map_or(usize::MAX, |serialized| serialized.len());
        if bytes > MAX_OPTION_VALUE_BYTES {
            return Err(ConfigError::TooManyOptions {
                bytes,
                limit: MAX_OPTION_VALUE_BYTES,
            });
        }
    }
    Ok(())
}

/// Validates a fully merged option map before it is cached.
fn validate_merged_options(options: &BTreeMap<String, Value>) -> Result<(), ConfigError> {
    let bytes = serde_json::to_vec(options).map_or(usize::MAX, |serialized| serialized.len());
    if bytes > MAX_MERGED_OPTION_BYTES {
        return Err(ConfigError::TooManyOptions {
            bytes,
            limit: MAX_MERGED_OPTION_BYTES,
        });
    }
    Ok(())
}

/// Resolves a `baseUrl` value, where `.` means the config's directory.
fn resolve_directory_value(
    directory: Option<&ProjectPath>,
    value: &str,
    config: &ProjectPath,
) -> Result<Option<ProjectPath>, ConfigError> {
    if value == "." || value == "./" {
        return Ok(directory.cloned());
    }
    ProjectPath::resolve(directory, value)
        .map(Some)
        .map_err(|error| ConfigError::InvalidPath {
            path: config.clone(),
            error,
        })
}

/// Resolves every `paths` target against the effective base directory.
fn resolve_paths(
    paths: Option<BTreeMap<String, Vec<String>>>,
    base: Option<&ProjectPath>,
    config: &ProjectPath,
) -> Result<BTreeMap<String, Vec<ProjectPath>>, ConfigError> {
    let mut resolved = BTreeMap::new();
    for (pattern, targets) in paths.unwrap_or_default() {
        if pattern.is_empty() {
            return Err(ConfigError::InvalidPath {
                path: config.clone(),
                error: PathError::Empty,
            });
        }
        if pattern.contains('\0') {
            return Err(ConfigError::InvalidPath {
                path: config.clone(),
                error: PathError::Nul,
            });
        }
        let mut values = Vec::with_capacity(targets.len());
        for target in targets {
            let value =
                ProjectPath::resolve(base, &target).map_err(|error| ConfigError::InvalidPath {
                    path: config.clone(),
                    error,
                })?;
            values.push(value);
        }
        resolved.insert(pattern, values);
    }
    Ok(resolved)
}

/// Resolves one `extends` value to a project-relative config path.
fn resolve_extends(
    directory: Option<&ProjectPath>,
    specifier: &str,
    config: &ProjectPath,
) -> Result<ProjectPath, ConfigError> {
    if specifier.is_empty()
        || specifier.contains('\0')
        || !(specifier.starts_with("./") || specifier.starts_with("../"))
    {
        return Err(ConfigError::UnsupportedExtends {
            path: config.clone(),
            specifier: truncate_message(specifier, MAX_MESSAGE_BYTES).to_owned(),
        });
    }
    let (base, directory_form) = match specifier.strip_suffix('/') {
        Some(prefix) => (
            ProjectPath::resolve(directory, prefix).map_err(|error| ConfigError::InvalidPath {
                path: config.clone(),
                error,
            })?,
            true,
        ),
        None => (
            ProjectPath::resolve(directory, specifier).map_err(|error| {
                ConfigError::InvalidPath {
                    path: config.clone(),
                    error,
                }
            })?,
            false,
        ),
    };
    if directory_form {
        return ProjectPath::resolve(Some(&base), "tsconfig.json").map_err(|error| {
            ConfigError::InvalidPath {
                path: config.clone(),
                error,
            }
        });
    }
    if base.extension() == Some("json") {
        return Ok(base);
    }
    ProjectPath::new(format!("{}.json", base.as_str())).map_err(|error| ConfigError::InvalidPath {
        path: config.clone(),
        error,
    })
}

impl ConfigError {
    /// Converts a file-read error into a config error.
    fn from_file(path: &ProjectPath, error: &FileError) -> Self {
        match error {
            FileError::NotFound { .. } => Self::NotFound { path: path.clone() },
            FileError::EscapesRoot { .. } => Self::EscapesRoot { path: path.clone() },
            FileError::InvalidUtf8 { .. } => Self::InvalidUtf8 { path: path.clone() },
            FileError::TooLarge { size, .. } => Self::TooLarge {
                path: path.clone(),
                size: *size,
            },
            FileError::Io { kind, .. } => Self::Io {
                path: path.clone(),
                kind: *kind,
            },
        }
    }
}

/// Removes comments and trailing commas while preserving line numbers.
fn sanitize_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut pending_comma = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                flush_comma(&mut output, &mut pending_comma);
                let start = index;
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                output.extend_from_slice(&bytes[start..index.min(bytes.len())]);
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    output.push(b' ');
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                output.extend_from_slice(b"  ");
                index += 2;
                let mut closed = false;
                while index < bytes.len() {
                    if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                        output.extend_from_slice(b"  ");
                        index += 2;
                        closed = true;
                        break;
                    }
                    output.push(if bytes[index] == b'\n' { b'\n' } else { b' ' });
                    index += 1;
                }
                if !closed {
                    output.push(b' ');
                }
            }
            b',' => {
                pending_comma = true;
                index += 1;
            }
            b'}' | b']' => {
                if pending_comma {
                    pending_comma = false;
                    output.push(b' ');
                }
                output.push(bytes[index]);
                index += 1;
            }
            byte if byte.is_ascii_whitespace() => {
                output.push(byte);
                index += 1;
            }
            byte => {
                flush_comma(&mut output, &mut pending_comma);
                output.push(byte);
                index += 1;
            }
        }
    }
    match String::from_utf8(output) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

/// Emits a pending comma before a significant token.
fn flush_comma(output: &mut Vec<u8>, pending: &mut bool) {
    if *pending {
        output.push(b',');
        *pending = false;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::files::{MAX_FILE_BYTES, MAX_MESSAGE_BYTES};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "wxml-project-config-{}-{id}-{name}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn root(&self) -> PathBuf {
            self.path.canonicalize().expect("canonical root")
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
    fn loads_paths_and_resolves_aliases() {
        let temp = TempDir::new("aliases");
        temp.write(
            "tsconfig.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {
                        "@/*": ["src/*"],
                        "utils": ["src/utils/index"],
                        "lib/*": ["lib/*", "vendor/*"]
                    }
                },
                "include": ["src/**/*.ts"],
                "exclude": ["node_modules"]
            }"#,
        );

        let config =
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()).expect("load");

        assert_eq!(config.path, path("tsconfig.json"));
        assert_eq!(config.base_url, None);
        assert_eq!(config.paths["@/*"], vec![path("src/*")]);
        assert_eq!(config.include, ["src/**/*.ts"]);
        assert_eq!(config.exclude, ["node_modules"]);

        assert_eq!(config.resolve_alias("@/pages/x"), Some(path("src/pages/x")));
        assert_eq!(config.resolve_alias("utils"), Some(path("src/utils/index")));
        assert_eq!(config.resolve_alias("lib/a"), Some(path("lib/a")));
        assert_eq!(config.resolve_alias("./relative"), None);
        assert_eq!(config.resolve_alias("/absolute"), None);
        assert_eq!(config.resolve_alias("missing"), None);
        assert_eq!(config.resolve_alias("@/../evil"), None);
    }

    #[test]
    fn accepts_comments_trailing_commas_and_bom() {
        let temp = TempDir::new("jsonc");
        temp.write(
            "tsconfig.json",
            "\u{feff}{\n  // line comment\n  /* block\n     comment */\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"@/*\": [\"src/*\"], },\n  },\n}",
        );
        let config =
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()).expect("load");
        assert_eq!(config.paths["@/*"], vec![path("src/*")]);
    }

    #[test]
    fn extends_merges_and_child_overrides() {
        let temp = TempDir::new("extends");
        temp.write(
            "tsconfig.base.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": { "@/*": ["src/*"], "shared": ["shared/index"] }
                },
                "include": ["base/**/*.ts"]
            }"#,
        );
        temp.write(
            "tsconfig.json",
            r#"{
                "extends": "./tsconfig.base",
                "compilerOptions": {
                    "paths": { "@/*": ["app/*"], "extra": ["extra/index"] }
                }
            }"#,
        );
        let config =
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()).expect("load");

        assert_eq!(config.extends, vec![path("tsconfig.base.json")]);
        assert_eq!(config.paths["@/*"], vec![path("app/*")]);
        assert_eq!(config.paths["shared"], vec![path("shared/index")]);
        assert_eq!(config.paths["extra"], vec![path("extra/index")]);
        assert_eq!(config.include, ["base/**/*.ts"]);
        assert_eq!(config.resolve_alias("@/x"), Some(path("app/x")));
    }

    #[test]
    fn extends_array_applies_in_order() {
        let temp = TempDir::new("extends-array");
        temp.write(
            "a.json",
            r#"{"compilerOptions": {"paths": {"x": ["a/x"]}}}"#,
        );
        temp.write(
            "b.json",
            r#"{"compilerOptions": {"paths": {"x": ["b/x"], "y": ["b/y"]}}}"#,
        );
        temp.write("tsconfig.json", r#"{"extends": ["./a.json", "./b.json"]}"#);
        let config =
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()).expect("load");
        assert_eq!(config.extends, vec![path("a.json"), path("b.json")]);
        assert_eq!(config.paths["x"], vec![path("b/x")]);
        assert_eq!(config.paths["y"], vec![path("b/y")]);
        assert_eq!(config.resolve_alias("x"), Some(path("b/x")));
    }

    #[test]
    fn resolves_base_url_and_paths_relative_to_nested_config() {
        let temp = TempDir::new("nested");
        temp.write(
            "packages/app/tsconfig.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": "../shared",
                    "paths": { "@/*": ["*"] }
                }
            }"#,
        );
        let config = load_ts_config(
            &temp.root(),
            &path("packages/app/tsconfig.json"),
            &Overlay::new(),
        )
        .expect("load");
        assert_eq!(config.base_url, Some(path("packages/shared")));
        assert_eq!(config.paths["@/*"], vec![path("packages/shared/*")]);
        assert_eq!(config.resolve_alias("@/x"), Some(path("packages/shared/x")));
    }

    #[test]
    fn uses_config_directory_when_base_url_is_absent() {
        let temp = TempDir::new("no-base-url");
        temp.write(
            "sub/tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@/*": ["./src/*"]}}}"#,
        );
        let config = load_ts_config(&temp.root(), &path("sub/tsconfig.json"), &Overlay::new())
            .expect("load");
        assert_eq!(config.base_url, Some(path("sub")));
        assert_eq!(config.paths["@/*"], vec![path("sub/src/*")]);
        assert_eq!(config.resolve_alias("@/app"), Some(path("sub/src/app")));
    }

    #[test]
    fn reports_json_errors_with_position() {
        let temp = TempDir::new("bad-json");
        temp.write("tsconfig.json", "{\n  \"compilerOptions\": \n}");
        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("error");
        let ConfigError::Json { line, column, .. } = &error else {
            panic!("expected JSON error, got {error:?}");
        };
        assert_eq!(*line, 3);
        assert!(*column >= 1);
    }

    #[test]
    fn rejects_missing_and_unsupported_extends() {
        let temp = TempDir::new("extends-errors");
        temp.write("tsconfig.json", r#"{"extends": "./missing"}"#);
        assert_eq!(
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()),
            Err(ConfigError::ExtendsNotFound {
                path: path("tsconfig.json"),
                target: path("missing.json")
            })
        );

        temp.write(
            "tsconfig.json",
            r#"{"extends": "@tsconfig/node16/tsconfig.json"}"#,
        );
        assert_eq!(
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()),
            Err(ConfigError::UnsupportedExtends {
                path: path("tsconfig.json"),
                specifier: "@tsconfig/node16/tsconfig.json".to_owned()
            })
        );
    }

    #[test]
    fn detects_extends_cycles() {
        let temp = TempDir::new("cycle");
        temp.write("a.json", r#"{"extends": "./b.json"}"#);
        temp.write("b.json", r#"{"extends": "./a.json"}"#);
        assert_eq!(
            load_ts_config(&temp.root(), &path("a.json"), &Overlay::new()),
            Err(ConfigError::ExtendsCycle {
                path: path("a.json")
            })
        );
    }

    #[test]
    fn rejects_paths_that_escape_the_root() {
        let temp = TempDir::new("escape");
        temp.write(
            "tsconfig.json",
            r#"{"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["../../outside/*"]}}}"#,
        );
        assert_eq!(
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()),
            Err(ConfigError::InvalidPath {
                path: path("tsconfig.json"),
                error: PathError::EscapesRoot
            })
        );
    }

    #[test]
    fn overlay_text_wins_over_disk() {
        let temp = TempDir::new("overlay");
        temp.write(
            "tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@/*": ["disk/*"]}}}"#,
        );
        let mut overlay = Overlay::new();
        overlay
            .set(
                &path("tsconfig.json"),
                r#"{"compilerOptions": {"paths": {"@/*": ["overlay/*"]}}}"#,
                1,
            )
            .expect("set");
        let config = load_ts_config(&temp.root(), &path("tsconfig.json"), &overlay).expect("load");
        assert_eq!(config.paths["@/*"], vec![path("overlay/*")]);
    }

    #[test]
    fn enforces_config_load_budget() {
        const FANOUT: usize = 3;
        const LEVELS: usize = 5;
        let temp = TempDir::new("budget");

        // 1 + 3 + 9 + 27 + 81 = 121 distinct configs, more than the budget.
        let mut count = 1;
        for level in 0..LEVELS {
            for index in 0..count {
                let children: Vec<String> = if level + 1 < LEVELS {
                    (0..FANOUT)
                        .map(|child| {
                            format!("\"./l{}_{}.json\"", level + 1, index * FANOUT + child)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let body = if children.is_empty() {
                    "{}".to_owned()
                } else {
                    format!("{{\"extends\": [{}]}}", children.join(","))
                };
                temp.write(&format!("l{level}_{index}.json"), &body);
            }
            count *= FANOUT;
        }
        temp.write("tsconfig.json", r#"{"extends": "./l0_0.json"}"#);

        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("budget must reject the fan-out");
        assert!(
            matches!(error, ConfigError::TooManyConfigs { .. }),
            "expected TooManyConfigs, got {error:?}"
        );
    }

    #[test]
    fn memoizes_shared_extends_parents() {
        let temp = TempDir::new("memo");
        for index in 0..8 {
            temp.write(
                &format!("s{index}.json"),
                &format!(r#"{{"compilerOptions": {{"paths": {{"s{index}/*": ["s{index}/*"]}}}}}}"#),
            );
        }
        let shared: Vec<String> = (0..8).map(|index| format!("\"./s{index}.json\"")).collect();
        for index in 0..10 {
            temp.write(
                &format!("p{index}.json"),
                &format!("{{\"extends\": [{}]}}", shared.join(",")),
            );
        }
        let parents: Vec<String> = (0..10)
            .map(|index| format!("\"./p{index}.json\""))
            .collect();
        temp.write(
            "tsconfig.json",
            &format!("{{\"extends\": [{}]}}", parents.join(",")),
        );

        // Without memoization this graph would load 91 files and exceed the
        // 64-file budget; with it, only 19 distinct files are read.
        let config = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect("shared parents are loaded once");
        assert_eq!(config.paths.len(), 8);
        assert_eq!(config.resolve_alias("s7/x"), Some(path("s7/x")));
    }

    #[test]
    fn reports_the_deepest_missing_extends() {
        let temp = TempDir::new("deep-missing");
        temp.write("a.json", r#"{"extends": "./b.json"}"#);
        temp.write("b.json", r#"{"extends": "./c.json"}"#);

        assert_eq!(
            load_ts_config(&temp.root(), &path("a.json"), &Overlay::new()),
            Err(ConfigError::ExtendsNotFound {
                path: path("b.json"),
                target: path("c.json")
            })
        );
    }

    #[test]
    fn rejects_oversized_config_files() {
        let temp = TempDir::new("too-large");
        let target = temp.path.join("tsconfig.json");
        let file = fs::File::create(&target).expect("create config");
        file.set_len(MAX_FILE_BYTES + 1).expect("set length");

        assert_eq!(
            load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new()),
            Err(ConfigError::TooLarge {
                path: path("tsconfig.json"),
                size: MAX_FILE_BYTES + 1
            })
        );
    }

    #[test]
    fn truncates_long_unsupported_extends_values() {
        let temp = TempDir::new("truncate-extends");
        let long = format!("pkg/{}", "a".repeat(300));
        temp.write("tsconfig.json", &format!("{{\"extends\": \"{long}\"}}"));

        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("package extends is unsupported");
        let ConfigError::UnsupportedExtends { specifier, .. } = error else {
            panic!("expected UnsupportedExtends");
        };
        assert!(
            specifier.len() <= MAX_MESSAGE_BYTES,
            "len={}",
            specifier.len()
        );
    }

    #[test]
    fn enforces_config_byte_budget() {
        let temp = TempDir::new("byte-budget");
        let padding = "a".repeat(usize::try_from(MAX_CONFIG_BYTES / 2).expect("fits usize") + 1);
        let body = format!("// {padding}\n{{}}");
        temp.write("a.json", &body);
        temp.write("b.json", &body);
        temp.write("tsconfig.json", r#"{"extends": ["./a.json", "./b.json"]}"#);

        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("byte budget must reject the chain");
        assert!(
            matches!(error, ConfigError::TooManyConfigBytes { .. }),
            "expected TooManyConfigBytes, got {error:?}"
        );
    }

    #[test]
    fn enforces_extends_edge_budget() {
        let temp = TempDir::new("edge-budget");
        temp.write("leaf.json", "{}");
        let entries: Vec<String> = (0..=MAX_CONFIG_EXTENDS_EDGES)
            .map(|_| "\"./leaf.json\"".to_owned())
            .collect();
        temp.write(
            "tsconfig.json",
            &format!("{{\"extends\": [{}]}}", entries.join(",")),
        );

        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("edge budget must reject the fan-in");
        assert!(
            matches!(error, ConfigError::TooManyExtends { .. }),
            "expected TooManyExtends, got {error:?}"
        );
    }

    #[test]
    fn merged_compiler_options_are_bounded() {
        use std::time::{Duration, Instant};

        let temp = TempDir::new("option-budget");
        temp.write("app.json", r#"{"pages": []}"#);

        let value = "x".repeat(60 * 1024);
        let mut big_names = Vec::new();
        for index in 0..8 {
            let mut options = Map::new();
            for key in 0..12 {
                options.insert(format!("big{index}_{key}"), Value::String(value.clone()));
            }
            temp.write(
                &format!("big{index}.json"),
                &format!("{{\"compilerOptions\": {}}}", Value::Object(options)),
            );
            big_names.push(format!("\"./big{index}.json\""));
        }

        for leaf in 0..56 {
            temp.write(
                &format!("leaf{leaf}.json"),
                &format!("{{\"extends\": [{}]}}", big_names.join(",")),
            );
        }
        let leaves: Vec<String> = (0..56)
            .map(|leaf| format!("\"./leaf{leaf}.json\""))
            .collect();
        temp.write(
            "tsconfig.json",
            &format!("{{\"extends\": [{}]}}", leaves.join(",")),
        );

        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("merged options must be bounded");
        match error {
            ConfigError::TooManyOptions { bytes, limit } => {
                assert_eq!(limit, MAX_MERGED_OPTION_BYTES);
                assert!(bytes > limit, "bytes={bytes}, limit={limit}");
            }
            other => panic!("expected TooManyOptions, got {other:?}"),
        }

        let started = Instant::now();
        let project = crate::Project::open(temp.path.clone()).expect("open must not fail");
        let elapsed = started.elapsed();
        assert!(project.tsconfig().is_none());
        assert!(project.compiler_options().is_none());
        assert!(project.issues().iter().any(|issue| issue.kind
            == crate::DiscoveryIssueKind::InvalidConfig
            && issue.file == path("tsconfig.json")));
        assert!(elapsed < Duration::from_secs(10), "elapsed={elapsed:?}");
    }

    #[test]
    fn extra_option_key_and_value_limits() {
        let temp = TempDir::new("extra-option-limits");

        let mut options = Map::new();
        for index in 0..=MAX_EXTRA_OPTION_KEYS {
            options.insert(format!("k{index}"), Value::Bool(true));
        }
        temp.write(
            "tsconfig.json",
            &format!("{{\"compilerOptions\": {}}}", Value::Object(options)),
        );
        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("key budget must reject the config");
        assert_eq!(
            error,
            ConfigError::TooManyOptionKeys {
                keys: MAX_EXTRA_OPTION_KEYS + 1,
                limit: MAX_EXTRA_OPTION_KEYS
            }
        );

        let huge = "x".repeat(MAX_OPTION_VALUE_BYTES + 1);
        temp.write(
            "tsconfig.json",
            &format!("{{\"compilerOptions\": {{\"huge\": \"{huge}\"}}}}"),
        );
        let error = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect_err("value budget must reject the config");
        let ConfigError::TooManyOptions { bytes, limit } = error else {
            panic!("expected TooManyOptions");
        };
        assert_eq!(limit, MAX_OPTION_VALUE_BYTES);
        assert!(bytes > limit);
    }

    #[test]
    fn repeated_extends_edges_are_deduplicated() {
        use std::time::{Duration, Instant};

        let temp = TempDir::new("dedup-extends");
        temp.write("app.json", r#"{"pages": []}"#);

        let mut paths = Map::new();
        for index in 0..20_000u32 {
            paths.insert(
                format!("p{index}/*"),
                Value::Array(vec![Value::String(format!("src{index}/*"))]),
            );
        }
        temp.write(
            "base.json",
            &format!(
                "{{\"compilerOptions\": {{\"paths\": {}}}}}",
                Value::Object(paths)
            ),
        );
        let duplicates: Vec<&str> = (0..MAX_CONFIG_EXTENDS_EDGES)
            .map(|_| "\"./base.json\"")
            .collect();
        temp.write(
            "tsconfig.json",
            &format!("{{\"extends\": [{}]}}", duplicates.join(",")),
        );

        let started = Instant::now();
        let config = load_ts_config(&temp.root(), &path("tsconfig.json"), &Overlay::new())
            .expect("duplicate extends must still load");
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "elapsed={elapsed:?}");
        assert_eq!(config.extends.len(), 1);
        assert_eq!(config.paths.len(), 20_000);
        assert_eq!(config.resolve_alias("p19999/x"), Some(path("src19999/x")));

        let started = Instant::now();
        let project = crate::Project::open(temp.path.clone()).expect("open");
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "elapsed={elapsed:?}");
        assert_eq!(
            project.tsconfig().map(|config| config.paths.len()),
            Some(20_000)
        );
    }

    #[test]
    fn compiler_options_are_rebased_to_project_root() {
        let temp = TempDir::new("rebased-options");
        temp.write(
            "configs/tsconfig.base.json",
            r#"{
                "compilerOptions": {
                    "baseUrl": ".",
                    "paths": {"@/*": ["./src/*"]}
                }
            }"#,
        );
        temp.write(
            "configs/tsconfig.json",
            r#"{"extends": "./tsconfig.base.json"}"#,
        );

        let config = load_ts_config(
            &temp.root(),
            &path("configs/tsconfig.json"),
            &Overlay::new(),
        )
        .expect("load");
        assert_eq!(config.base_url, Some(path("configs")));
        assert_eq!(config.paths["@/*"], vec![path("configs/src/*")]);
        assert_eq!(
            config
                .compiler_options
                .get("baseUrl")
                .and_then(|value| value.as_str()),
            Some(".")
        );
        assert_eq!(
            config
                .compiler_options
                .get("paths")
                .and_then(|value| value.as_object())
                .and_then(|paths| paths.get("@/*"))
                .and_then(|value| value.as_array())
                .and_then(|targets| targets.first())
                .and_then(|value| value.as_str()),
            Some("configs/src/*")
        );
    }
}
