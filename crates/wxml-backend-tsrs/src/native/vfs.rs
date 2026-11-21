//! Read-only virtual filesystem and engine filesystem callbacks.
//!
//! The engine may only see files from the current snapshot and directory
//! listings derived from those files. Every callback answer comes from the
//! in-memory table; the host filesystem is never consulted and `useOS` is
//! never returned. Besides project files, a full snapshot may contain verified
//! TypeScript standard-library and mini-program typings assets mounted from
//! their fixed roots. Assets answer callbacks but never appear in
//! `source_paths()` or content handles. Paths outside the project root and the
//! fixed asset roots fail closed with the callback-specific negative shape:
//!
//! | callback | outside/invalid answer |
//! | --- | --- |
//! | `readFile` | `{"kind":"missing"}` |
//! | `fileExists` | `{"kind":"value","value":false}` |
//! | `directoryExists` | `{"kind":"value","value":false}` |
//! | `getAccessibleEntries` | empty `value` entries |
//! | `realpath` | `{"kind":"identity"}` |
//! | `stat` | `{"kind":"missing"}` |
//!
//! `writeFile` and `removeFile` always answer `{"kind":"noop"}` so method
//! whitelisting remains the primary write defence. Unknown callbacks are
//! reported as a JSON-RPC error; they never fall back to the host.
//!
//! `readFile` supports the protocol's `value` and `missing` answers. `useOS`
//! is deliberately never emitted by this dispatcher: a host fallback would
//! break the read boundary. `stat` is registered in the frozen callback list
//! and answered from the same table, so no callback falls through to the host
//! filesystem. A canary self-check additionally proves that a real host file
//! outside the root is answered as `missing` for both `readFile` and `stat`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::input::{
    AnalysisInput, CompilerOptions, InputError, MINIPROGRAM_TYPINGS, ProjectRoot, TypeAssetError,
    TypeAssetFile, TypeAssets, hex_lower,
};

/// Name of the synthesized project configuration file.
pub const VIRTUAL_TSCONFIG: &str = "tsconfig.json";

/// Fixed modification time reported for virtual files (`RFC3339Nano`).
pub const VIRTUAL_MTIME: &str = "1970-01-01T00:00:00Z";

/// POSIX mode reported for a regular virtual file.
pub const FILE_MODE: u32 = 0o100_644;

/// POSIX mode reported for a virtual directory.
pub const DIRECTORY_MODE: u32 = 0o040_755;

/// One explicit degradation applied while building the type environment.
///
/// The native VFS never reads the host filesystem. A project `typeRoots`,
/// `baseUrl` or `paths` target that falls outside the virtual snapshot is
/// dropped instead of being handed to the engine, and recorded here so the
/// adapter can publish `Partial` rather than a silent TS2688/no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentDegradation {
    field: &'static str,
    reason: &'static str,
}

impl EnvironmentDegradation {
    /// Names the compiler-option field that was filtered.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the stable degradation reason.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

/// One file in the virtual snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualFile {
    text: Arc<str>,
    hash: String,
    size: u64,
}

impl VirtualFile {
    fn new(text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let hash = hex_lower(&Sha256::digest(text.as_bytes()));
        let size = u64::try_from(text.len()).unwrap_or(u64::MAX);
        Self { text, hash, size }
    }

    fn from_asset(asset: &TypeAssetFile) -> Self {
        Self {
            text: Arc::clone(asset.text_arc()),
            hash: asset.sha256().to_owned(),
            size: asset.size(),
        }
    }

    /// Returns the exact text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the lowercase SHA-256 of the text.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Returns the text length in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

/// Immediate children of one virtual directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryEntries {
    /// File names, sorted.
    pub files: Vec<String>,
    /// Directory names, sorted.
    pub directories: Vec<String>,
    /// Symlink names; always empty because snapshots have no symlinks.
    pub symlinks: Vec<String>,
}

impl DirectoryEntries {
    /// Returns the engine callback `value` object.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "files": &self.files,
            "directories": &self.directories,
            "symlinks": &self.symlinks,
        })
    }
}

/// In-memory snapshot filesystem used to answer engine callbacks.
///
/// Project files live under the project root. Verified type assets are kept in
/// a separate table with their own directory index, so `source_paths()` and
/// content handles never expose them while `fileSystem.kind="full"` snapshots
/// still contain them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualFileSystem {
    root: String,
    files: BTreeMap<String, VirtualFile>,
    assets: BTreeMap<String, VirtualFile>,
    directories: BTreeMap<String, DirectoryEntries>,
    asset_directories: BTreeMap<String, DirectoryEntries>,
    environment_degradations: Vec<EnvironmentDegradation>,
    tsconfig_path: String,
}

impl VirtualFileSystem {
    /// Builds the virtual filesystem for one analysis input.
    ///
    /// Project files are mounted under the project root. An input created with
    /// [`AnalysisInput::with_environment`] additionally mounts the verified
    /// standard-library and typings declarations and synthesizes a
    /// `tsconfig.json` from the project compiler options; the legacy
    /// [`AnalysisInput::new`] keeps the historical `noLib:true` probe
    /// configuration.
    ///
    /// # Errors
    ///
    /// Rejects duplicate absolute paths, an unusable project root, an asset
    /// outside its fixed root, a duplicate asset path or an asset that collides
    /// with a project file.
    pub fn from_analysis_input(input: &AnalysisInput) -> Result<Self, VfsError> {
        let root = input.root().as_str().to_owned();
        let mut files = BTreeMap::new();
        let mut relative_paths = Vec::new();
        for file in input.files() {
            let absolute = join_root(&root, file.path())?;
            if files
                .insert(absolute, VirtualFile::new(file.text()))
                .is_some()
            {
                return Err(VfsError::DuplicateFile {
                    path: file.path().to_owned(),
                });
            }
            relative_paths.push(file.path().to_owned());
        }
        relative_paths.sort();
        relative_paths.dedup();
        let tsconfig_path = join_root(&root, VIRTUAL_TSCONFIG)?;
        let (assets, asset_directories, environment_degradations) = if let Some(assets) =
            input.type_assets()
        {
            let directories = virtual_directories(&root, files.keys().map(String::as_str), assets);
            let (tsconfig, degradations) = environment_tsconfig_text(
                &root,
                &relative_paths,
                input.compiler_options(),
                assets,
                &directories,
            );
            files.insert(tsconfig_path.clone(), VirtualFile::new(tsconfig));
            let (assets, directories) = mount_type_assets(&files, assets)?;
            (assets, directories, degradations)
        } else {
            files.insert(
                tsconfig_path.clone(),
                VirtualFile::new(tsconfig_text(&relative_paths)),
            );
            (BTreeMap::new(), BTreeMap::new(), Vec::new())
        };

        Self::build(
            root,
            files,
            assets,
            asset_directories,
            environment_degradations,
            tsconfig_path,
        )
    }

    /// Builds a virtual filesystem from explicit absolute paths and texts.
    ///
    /// This is used by the handshake/schema probe, which needs a minimal
    /// snapshot without an [`AnalysisInput`]. It never mounts type assets and
    /// keeps the legacy `noLib:true` configuration.
    ///
    /// # Errors
    ///
    /// Rejects an invalid root or a file path outside it, and duplicate paths.
    pub fn from_root_and_files(
        root: &str,
        files: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, VfsError> {
        let root = ProjectRoot::new(root.to_owned())?.as_str().to_owned();
        let mut table = BTreeMap::new();
        for (path, text) in files {
            let absolute = absolute_from_root(&root, &path)?;
            if table.insert(absolute, VirtualFile::new(text)).is_some() {
                return Err(VfsError::DuplicateFile { path });
            }
        }
        let tsconfig_path = join_root(&root, VIRTUAL_TSCONFIG)?;
        if !table.contains_key(&tsconfig_path) {
            let mut relative = Vec::new();
            for path in table.keys() {
                if let Some(rest) = path.strip_prefix(&root)
                    && let Some(rest) = rest.strip_prefix('/')
                {
                    relative.push(rest.to_owned());
                }
            }
            relative.sort();
            relative.dedup();
            table.insert(
                tsconfig_path.clone(),
                VirtualFile::new(tsconfig_text(&relative)),
            );
        }
        Self::build(
            root,
            table,
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
            tsconfig_path,
        )
    }

    fn build(
        root: String,
        files: BTreeMap<String, VirtualFile>,
        assets: BTreeMap<String, VirtualFile>,
        asset_directories: BTreeMap<String, DirectoryEntries>,
        environment_degradations: Vec<EnvironmentDegradation>,
        tsconfig_path: String,
    ) -> Result<Self, VfsError> {
        let directories = index_project_directories(&root, &files)?;

        Ok(Self {
            root,
            files,
            assets,
            directories,
            asset_directories,
            environment_degradations,
            tsconfig_path,
        })
    }

    /// Returns the normalized project root.
    #[must_use]
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Returns the explicit type-environment degradations.
    #[must_use]
    pub fn environment_degradations(&self) -> &[EnvironmentDegradation] {
        &self.environment_degradations
    }

    /// Returns the absolute path of the synthesized `tsconfig.json`.
    #[must_use]
    pub fn tsconfig_path(&self) -> &str {
        &self.tsconfig_path
    }

    /// Returns all project absolute paths, including the configuration file.
    ///
    /// Type assets are intentionally excluded; they are not project documents
    /// and must never appear in `openFiles` or content handles.
    pub fn absolute_paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Returns only absolute project source-file paths (excluding
    /// `tsconfig.json` and type assets).
    pub fn source_paths(&self) -> impl Iterator<Item = &str> {
        self.files
            .keys()
            .map(String::as_str)
            .filter(|path| *path != self.tsconfig_path)
    }

    /// Returns the project file at an absolute path.
    #[must_use]
    pub fn file(&self, absolute: &str) -> Option<&VirtualFile> {
        self.files.get(absolute)
    }

    /// Returns the verified type asset at an absolute path.
    #[must_use]
    pub fn asset(&self, absolute: &str) -> Option<&VirtualFile> {
        self.assets.get(absolute)
    }

    /// Returns either a project file or a mounted type asset.
    ///
    /// Invalid paths and paths outside both tables return `None`; this never
    /// consults the host filesystem.
    #[must_use]
    pub fn file_or_asset(&self, absolute: &str) -> Option<&VirtualFile> {
        self.virtual_file(absolute)
    }

    /// Returns whether an absolute path names a virtual directory.
    #[must_use]
    pub fn is_directory(&self, absolute: &str) -> bool {
        validate_absolute(absolute).is_ok()
            && (self.directories.contains_key(absolute)
                || self.asset_directories.contains_key(absolute))
    }

    /// Returns the immediate children of one virtual directory.
    ///
    /// Project directories are checked first, then mounted asset directories.
    #[must_use]
    pub fn directory_entries(&self, absolute: &str) -> Option<&DirectoryEntries> {
        if validate_absolute(absolute).is_err() {
            return None;
        }
        self.directories
            .get(absolute)
            .or_else(|| self.asset_directories.get(absolute))
    }

    /// Returns every mounted type-asset path, sorted.
    pub fn asset_paths(&self) -> impl Iterator<Item = &str> {
        self.assets.keys().map(String::as_str)
    }

    /// Returns the content hash at an absolute project path.
    #[must_use]
    pub fn hash(&self, absolute: &str) -> Option<&str> {
        self.files.get(absolute).map(VirtualFile::hash)
    }

    /// Returns an owned copy of every absolute path and text for snapshot
    /// request construction.
    ///
    /// The full snapshot contains project files, `tsconfig.json` and every
    /// mounted type asset. The engine makes zero filesystem callbacks for a
    /// `kind="full"` snapshot, so assets must be present in this dictionary.
    #[must_use]
    pub fn snapshot_files(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .chain(self.assets.iter())
            .map(|(path, file)| (path.clone(), file.text().to_owned()))
            .collect()
    }

    /// Resolves a project-relative path to its absolute virtual path.
    ///
    /// # Errors
    ///
    /// Rejects empty paths, absolute paths, backslashes, control characters
    /// and `.`/`..`/empty components.
    pub fn absolute_path(&self, relative: &str) -> Result<String, VfsError> {
        validate_relative(relative)?;
        join_root(&self.root, relative)
    }

    /// Returns whether an absolute path is inside the project root.
    #[must_use]
    pub fn contains(&self, absolute: &str) -> bool {
        within_root(&self.root, absolute)
    }

    /// Answers one engine filesystem callback.
    ///
    /// # Errors
    ///
    /// Returns [`CallbackError::UnknownCallback`] for a method outside the
    /// frozen eight, or [`CallbackError::InvalidParams`] for malformed params.
    /// Invalid paths are not errors; they receive the fail-closed shape.
    pub fn answer(&self, method: &str, params: &Value) -> Result<Value, CallbackError> {
        match method {
            "readFile" => {
                let path = path_param(params, "readFile")?;
                Ok(self.virtual_file(path).map_or_else(
                    || json!({"kind": "missing"}),
                    |file| json!({"kind": "value", "value": file.text()}),
                ))
            }
            "fileExists" => {
                let path = path_param(params, "fileExists")?;
                Ok(json!({"kind": "value", "value": self.virtual_file(path).is_some()}))
            }
            "directoryExists" => {
                let path = path_param(params, "directoryExists")?;
                let exists = validate_absolute(path).is_ok()
                    && (self.directories.contains_key(path)
                        || self.asset_directories.contains_key(path));
                Ok(json!({"kind": "value", "value": exists}))
            }
            "getAccessibleEntries" => {
                let path = path_param(params, "getAccessibleEntries")?;
                let entries = if validate_absolute(path).is_ok() {
                    self.directories
                        .get(path)
                        .or_else(|| self.asset_directories.get(path))
                } else {
                    None
                };
                Ok(json!({
                    "kind": "value",
                    "value": entries.map_or_else(DirectoryEntries::default, Clone::clone).to_value(),
                }))
            }
            "realpath" => {
                let _path = path_param(params, "realpath")?;
                Ok(json!({"kind": "identity"}))
            }
            "stat" => {
                let path = path_param(params, "stat")?;
                Ok(self.stat_value(path))
            }
            "writeFile" => {
                let _ = write_params(params, "writeFile")?;
                Ok(json!({"kind": "noop"}))
            }
            "removeFile" => {
                let _path = path_param(params, "removeFile")?;
                Ok(json!({"kind": "noop"}))
            }
            _ => Err(CallbackError::UnknownCallback {
                method: method.to_owned(),
            }),
        }
    }

    /// Returns the per-callback fail-closed answer for a known method.
    ///
    /// This is used while no virtual snapshot is installed (for example during
    /// `initialize` or the `createSnapshot` window) and must never be a
    /// JSON-RPC error: F0 §5.2 records that read-callback errors drive the
    /// engine into a panic path and cannot be used as a deny signal.
    #[must_use]
    pub fn fail_closed_answer(method: &str) -> Option<Value> {
        Some(match method {
            "readFile" | "stat" => json!({"kind": "missing"}),
            "fileExists" | "directoryExists" => json!({"kind": "value", "value": false}),
            "getAccessibleEntries" => json!({
                "kind": "value",
                "value": {"files": [], "directories": [], "symlinks": []},
            }),
            "realpath" => json!({"kind": "identity"}),
            "writeFile" | "removeFile" => json!({"kind": "noop"}),
            _ => return None,
        })
    }

    /// Proves that a real host path outside the project root is never served.
    ///
    /// The path must exist and must be outside `root`; this is a startup
    /// canary, not a host read. It answers both `readFile` and `stat` (the two
    /// `{"kind":"missing"}` callbacks), plus existence and directory probes.
    ///
    /// # Errors
    ///
    /// Returns [`VfsError::Confinement`] when the canary is inside the root or
    /// any callback does not return the fail-closed shape.
    pub fn assert_confined(&self, host_path: &str) -> Result<(), VfsError> {
        if self.contains(host_path) {
            return Err(VfsError::Confinement {
                path: host_path.to_owned(),
            });
        }

        let expected = [
            ("readFile", json!({"kind": "missing"})),
            ("stat", json!({"kind": "missing"})),
            ("fileExists", json!({"kind": "value", "value": false})),
            ("directoryExists", json!({"kind": "value", "value": false})),
            (
                "getAccessibleEntries",
                json!({
                    "kind": "value",
                    "value": {"files": [], "directories": [], "symlinks": []},
                }),
            ),
            ("realpath", json!({"kind": "identity"})),
        ];
        for (method, expected) in expected {
            let answer = self
                .answer(method, &Value::String(host_path.to_owned()))
                .map_err(|_| VfsError::Confinement {
                    path: host_path.to_owned(),
                })?;
            if answer != expected {
                return Err(VfsError::Confinement {
                    path: host_path.to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Returns a conservative upper bound of the serialized snapshot request.
    ///
    /// The bound covers JSON escaping (at most six bytes per UTF-8 byte for
    /// control characters), every file path, and both the close and open URI
    /// lists used by an update. It is intentionally larger than the final body
    /// so a request that passes this preflight always fits the frozen request
    /// budget; callers reject a too-large input before building the JSON body.
    #[must_use]
    pub fn request_size_upper_bound(&self) -> usize {
        let mut total = 512usize;
        for (path, file) in self.files.iter().chain(self.assets.iter()) {
            total = total.saturating_add(json_escaped_upper_bound(path));
            total = total.saturating_add(json_escaped_upper_bound(file.text()));
            total = total.saturating_add(file_uri_upper_bound(path).saturating_mul(2));
            total = total.saturating_add(128);
        }
        total.saturating_add(file_uri_upper_bound(&self.tsconfig_path))
    }

    /// Returns the virtual file for a validated absolute path.
    ///
    /// Project files are checked first, then mounted type assets. Invalid
    /// paths and paths outside both tables return `None`; the host filesystem
    /// is never consulted.
    fn virtual_file(&self, path: &str) -> Option<&VirtualFile> {
        if validate_absolute(path).is_err() {
            return None;
        }
        self.files.get(path).or_else(|| self.assets.get(path))
    }

    fn stat_value(&self, path: &str) -> Value {
        if validate_absolute(path).is_err() {
            return json!({"kind": "missing"});
        }
        let (mode, size) = if let Some(file) =
            self.files.get(path).or_else(|| self.assets.get(path))
        {
            (FILE_MODE, file.size())
        } else if self.directories.contains_key(path) || self.asset_directories.contains_key(path) {
            (DIRECTORY_MODE, 0)
        } else {
            return json!({"kind": "missing"});
        };
        json!({
            "kind": "value",
            "value": {
                "mode": mode,
                "size": size,
                "mtime": VIRTUAL_MTIME,
            },
        })
    }
}

/// Callback parameter failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CallbackError {
    /// The engine invoked a name outside the frozen eight.
    #[error("unknown filesystem callback: {method}")]
    UnknownCallback {
        /// Callback method name (bounded by the router).
        method: String,
    },
    /// The callback parameters are not the documented shape.
    #[error("invalid parameters for filesystem callback {method}")]
    InvalidParams {
        /// Callback method name.
        method: &'static str,
    },
}

impl CallbackError {
    /// Returns the JSON-RPC error code for this callback failure.
    #[must_use]
    pub const fn code(&self) -> i64 {
        match self {
            Self::UnknownCallback { .. } => -32601,
            Self::InvalidParams { .. } => -32602,
        }
    }

    /// Returns a bounded reason string for the error response.
    #[must_use]
    pub const fn reason(&self) -> &str {
        match self {
            Self::UnknownCallback { .. } => "unknown filesystem callback",
            Self::InvalidParams { .. } => "invalid filesystem callback parameters",
        }
    }
}

/// Virtual filesystem construction failure.
#[derive(Debug, Error)]
pub enum VfsError {
    /// A path is not a normalized absolute/relative project path.
    #[error("invalid project path: {reason}")]
    InvalidPath {
        /// Static rejection reason.
        reason: &'static str,
    },
    /// Two virtual files resolve to the same absolute path.
    #[error("duplicate virtual file: {path}")]
    DuplicateFile {
        /// Duplicated path.
        path: String,
    },
    /// The project root itself is invalid.
    #[error(transparent)]
    Root(#[from] InputError),
    /// The startup VFS confinement canary was served or could not be checked.
    #[error("virtual filesystem confinement check failed for {path}")]
    Confinement {
        /// Host path used by the canary.
        path: String,
    },
    /// A type asset is invalid, duplicated or outside its fixed root.
    #[error(transparent)]
    Asset(#[from] TypeAssetError),
    /// Two type assets resolve to the same virtual path.
    #[error("duplicate type asset: {path}")]
    DuplicateAsset {
        /// Duplicated path.
        path: String,
    },
    /// A type asset uses the same virtual path as a project file.
    #[error("type asset collides with a project file: {path}")]
    AssetCollision {
        /// Colliding path.
        path: String,
    },
}

fn path_param<'a>(params: &'a Value, method: &'static str) -> Result<&'a str, CallbackError> {
    params
        .as_str()
        .ok_or(CallbackError::InvalidParams { method })
}

fn write_params<'a>(params: &'a Value, method: &'static str) -> Result<&'a str, CallbackError> {
    let path = params
        .as_object()
        .and_then(|object| object.get("path"))
        .and_then(Value::as_str)
        .ok_or(CallbackError::InvalidParams { method })?;
    Ok(path)
}

fn within_root(root: &str, path: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn parent_directory(path: &str) -> Option<&str> {
    let index = path.rfind('/')?;
    if index == 0 {
        return None;
    }
    path.get(..index)
}

fn file_name(path: &str) -> Option<&str> {
    let index = path.rfind('/')?;
    path.get(index.saturating_add(1)..)
        .filter(|name| !name.is_empty())
}

fn join_root(root: &str, relative: &str) -> Result<String, VfsError> {
    validate_relative(relative)?;
    Ok(format!("{root}/{relative}"))
}

fn absolute_from_root(root: &str, path: &str) -> Result<String, VfsError> {
    if path.starts_with('/') {
        if !within_root(root, path) {
            return Err(VfsError::InvalidPath {
                reason: "file is outside the project root",
            });
        }
        validate_absolute(path)?;
        Ok(path.to_owned())
    } else {
        join_root(root, path)
    }
}

fn validate_relative(path: &str) -> Result<(), VfsError> {
    if path.is_empty() {
        return Err(VfsError::InvalidPath { reason: "empty" });
    }
    if path.starts_with('/') {
        return Err(VfsError::InvalidPath { reason: "absolute" });
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(VfsError::InvalidPath {
            reason: "control or backslash",
        });
    }
    if path.contains("//") {
        return Err(VfsError::InvalidPath {
            reason: "empty component",
        });
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(VfsError::InvalidPath {
            reason: "invalid component",
        });
    }
    Ok(())
}

fn validate_absolute(path: &str) -> Result<(), VfsError> {
    if !path.starts_with('/') {
        return Err(VfsError::InvalidPath {
            reason: "not absolute",
        });
    }
    if path == "/" {
        return Err(VfsError::InvalidPath {
            reason: "filesystem root",
        });
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(VfsError::InvalidPath {
            reason: "control or backslash",
        });
    }
    if path.contains("//") {
        return Err(VfsError::InvalidPath {
            reason: "empty component",
        });
    }
    if path
        .split('/')
        .skip(1)
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(VfsError::InvalidPath {
            reason: "invalid component",
        });
    }
    Ok(())
}

fn index_project_directories(
    root: &str,
    files: &BTreeMap<String, VirtualFile>,
) -> Result<BTreeMap<String, DirectoryEntries>, VfsError> {
    let mut directories = BTreeMap::<String, DirectoryEntries>::new();
    directories.entry(root.to_owned()).or_default();

    for path in files.keys() {
        let Some(parent) = parent_directory(path) else {
            return Err(VfsError::InvalidPath {
                reason: "virtual file has no parent directory",
            });
        };
        if !within_root(root, parent) {
            return Err(VfsError::InvalidPath {
                reason: "virtual file is outside the project root",
            });
        }
        let Some(name) = file_name(path) else {
            return Err(VfsError::InvalidPath {
                reason: "virtual file has no name",
            });
        };
        directories
            .entry(parent.to_owned())
            .or_default()
            .files
            .push(name.to_owned());

        let mut current = parent;
        while within_root(root, current) && current != root {
            let Some(parent_of_current) = parent_directory(current) else {
                break;
            };
            let Some(name) = file_name(current) else {
                break;
            };
            directories
                .entry(parent_of_current.to_owned())
                .or_default()
                .directories
                .push(name.to_owned());
            directories.entry(current.to_owned()).or_default();
            current = parent_of_current;
        }
    }

    sort_directory_entries(&mut directories);
    Ok(directories)
}

/// Indexes the fixed asset roots and their descendants only.
///
/// Ancestors above a fixed root are deliberately absent so a full snapshot
/// never exposes host directory names. Prefix work is bounded by
/// [`crate::input::MAX_ASSET_PATH_COMPONENTS`] and the 256-file asset cap.
fn index_asset_directories(
    standard_lib_root: &str,
    typings_root: &str,
    assets: &BTreeMap<String, VirtualFile>,
) -> BTreeMap<String, DirectoryEntries> {
    let mut directories = BTreeMap::<String, DirectoryEntries>::new();
    for path in assets.keys() {
        let root = if within_root(standard_lib_root, path) {
            standard_lib_root
        } else if within_root(typings_root, path) {
            typings_root
        } else {
            continue;
        };
        let Some(parent) = parent_directory(path) else {
            continue;
        };
        if !within_root(root, parent) {
            continue;
        }
        let Some(name) = file_name(path) else {
            continue;
        };
        directories
            .entry(parent.to_owned())
            .or_default()
            .files
            .push(name.to_owned());

        let mut current = parent;
        while current != root {
            let Some(parent_of_current) = parent_directory(current) else {
                break;
            };
            if !within_root(root, parent_of_current) {
                break;
            }
            let Some(name) = file_name(current) else {
                break;
            };
            directories
                .entry(parent_of_current.to_owned())
                .or_default()
                .directories
                .push(name.to_owned());
            current = parent_of_current;
        }
        directories.entry(root.to_owned()).or_default();
    }

    sort_directory_entries(&mut directories);
    directories
}

fn sort_directory_entries(directories: &mut BTreeMap<String, DirectoryEntries>) {
    for entries in directories.values_mut() {
        entries.files.sort();
        entries.files.dedup();
        entries.directories.sort();
        entries.directories.dedup();
    }
}

/// Mounted asset files plus their separate directory index.
type AssetMount = (
    BTreeMap<String, VirtualFile>,
    BTreeMap<String, DirectoryEntries>,
);

/// Mounts verified declarations and re-checks them at the VFS boundary.
fn mount_type_assets(
    project: &BTreeMap<String, VirtualFile>,
    assets: &TypeAssets,
) -> Result<AssetMount, VfsError> {
    let standard_lib_root = assets.standard_lib_root().as_str();
    let typings_package_root = assets.typings_package_root();
    let mut table = BTreeMap::new();
    if assets.is_typings_only() {
        for name in tsrs_vfs::bundled::LIB_NAMES {
            let path = join_root(standard_lib_root, name)?;
            let text = tsrs_vfs::bundled::embedded_file(&format!("bundled:///libs/{name}"))
                .ok_or_else(|| TypeAssetError::MissingRequired { path: path.clone() })?;
            table.insert(path, VirtualFile::new(text.to_owned()));
        }
    }
    for asset in assets.files() {
        let path = asset.path().as_str();
        if !within_root(standard_lib_root, path) && !within_root(&typings_package_root, path) {
            return Err(VfsError::Asset(TypeAssetError::InvalidPath {
                path: path.to_owned(),
                reason: "outside fixed asset roots",
            }));
        }
        if validate_absolute(path).is_err() {
            return Err(VfsError::Asset(TypeAssetError::InvalidPath {
                path: path.to_owned(),
                reason: "not a normalized absolute path",
            }));
        }
        if project.contains_key(path) {
            return Err(VfsError::AssetCollision {
                path: path.to_owned(),
            });
        }
        if table
            .insert(path.to_owned(), VirtualFile::from_asset(asset))
            .is_some()
        {
            return Err(VfsError::DuplicateAsset {
                path: path.to_owned(),
            });
        }
    }
    let directories =
        index_asset_directories(standard_lib_root, assets.typings_root().as_str(), &table);
    Ok((table, directories))
}

fn tsconfig_text(relative_paths: &[String]) -> String {
    json!({
        "compilerOptions": {
            "noLib": true,
            "strict": true,
            "module": "esnext",
            "target": "esnext",
        },
        "files": relative_paths,
    })
    .to_string()
}

/// Synthesizes the production type-environment configuration.
///
/// Only `noEmit:true`, the mandatory typings entry/root and the project's
/// validated options are emitted. `noLib`, `strict`, `target`, `module` and
/// `skipLibCheck` are never forced; the Node production path does not add
/// them either. Project `types`/`typeRoots`/`baseUrl`/`paths` entries that
/// point outside the virtual snapshot are dropped and reported as explicit
/// degradations instead of being forwarded to the engine.
fn environment_tsconfig_text(
    project_root: &str,
    relative_paths: &[String],
    options: &CompilerOptions,
    assets: &TypeAssets,
    directories: &BTreeSet<String>,
) -> (String, Vec<EnvironmentDegradation>) {
    let config = environment_options(project_root, options, assets, directories);
    let mut compiler = serde_json::Map::new();
    compiler.insert("noEmit".to_owned(), Value::Bool(true));
    compiler.insert(
        "types".to_owned(),
        Value::Array(config.types.into_iter().map(Value::String).collect()),
    );
    compiler.insert(
        "typeRoots".to_owned(),
        Value::Array(config.type_roots.into_iter().map(Value::String).collect()),
    );
    if let Some(value) = config.base_url {
        compiler.insert("baseUrl".to_owned(), value);
    }
    if let Some(value) = config.paths {
        compiler.insert("paths".to_owned(), value);
    }
    for (key, value) in options.values() {
        if matches!(key.as_str(), "types" | "typeRoots" | "baseUrl" | "paths") {
            continue;
        }
        compiler.insert(key.clone(), value.clone());
    }

    let tsconfig = json!({
        "compilerOptions": Value::Object(compiler),
        "files": relative_paths,
    })
    .to_string();
    (tsconfig, config.degradations)
}

/// Computes the type-environment degradations for one project snapshot.
///
/// This is the plan-time counterpart of the VFS configuration: the adapter
/// calls it before any session exists, so the very first typed request can
/// report `Partial` when `types`/`typeRoots`/`baseUrl`/`paths` point outside
/// the virtual snapshot. It is path-free and performs no I/O.
#[must_use]
pub fn environment_degradations_for(
    project_root: &str,
    project_paths: &[String],
    options: &CompilerOptions,
    assets: &TypeAssets,
) -> Vec<EnvironmentDegradation> {
    let directories = virtual_directories(
        project_root,
        project_paths.iter().map(String::as_str),
        assets,
    );
    environment_options(project_root, options, assets, &directories).degradations
}

struct EnvironmentOptions {
    types: Vec<String>,
    type_roots: Vec<String>,
    base_url: Option<Value>,
    paths: Option<Value>,
    degradations: Vec<EnvironmentDegradation>,
}

fn environment_options(
    project_root: &str,
    options: &CompilerOptions,
    assets: &TypeAssets,
    directories: &BTreeSet<String>,
) -> EnvironmentOptions {
    let mut degradations = Vec::new();

    let mut type_roots = Vec::new();
    let mut virtual_type_roots = Vec::new();
    for entry in options_string_array(options.get("typeRoots")) {
        let Some(normalized) = normalize_virtual_entry(project_root, &entry) else {
            push_degradation(
                &mut degradations,
                "typeRoots",
                "entry outside the virtual snapshot",
            );
            continue;
        };
        if !directories.contains(&normalized) {
            push_degradation(
                &mut degradations,
                "typeRoots",
                "entry outside the virtual snapshot",
            );
            continue;
        }
        type_roots.push(entry);
        virtual_type_roots.push(normalized);
    }
    let typings_root = assets.typings_root().as_str().to_owned();
    push_unique(&mut type_roots, typings_root.clone());
    push_unique(&mut virtual_type_roots, typings_root);

    let mut types = Vec::new();
    for entry in options_string_array(options.get("types")) {
        if type_entry_is_virtual(&entry, &virtual_type_roots, directories) {
            types.push(entry);
        } else {
            push_degradation(
                &mut degradations,
                "types",
                "entry outside the virtual snapshot",
            );
        }
    }
    push_unique(&mut types, MINIPROGRAM_TYPINGS.to_owned());

    let mut base_url = None;
    let base_url_removed = options
        .get("baseUrl")
        .and_then(Value::as_str)
        .is_some_and(|base| {
            normalize_virtual_entry(project_root, base)
                .is_none_or(|base| !directories.contains(&base))
        });
    if base_url_removed {
        push_degradation(
            &mut degradations,
            "baseUrl",
            "baseUrl outside the virtual snapshot",
        );
    } else if let Some(value) = options.get("baseUrl") {
        base_url = Some(value.clone());
    }

    let mut paths = None;
    if let Some(mapping) = options.get("paths").and_then(Value::as_object) {
        let mut kept = serde_json::Map::new();
        let mut dropped = base_url_removed;
        for (name, targets) in mapping {
            let all_virtual = targets.as_array().is_some_and(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .all(path_target_is_virtual)
            });
            if !base_url_removed && all_virtual {
                kept.insert(name.clone(), targets.clone());
            } else {
                dropped = true;
            }
        }
        if dropped {
            push_degradation(
                &mut degradations,
                "paths",
                "mapping target outside the virtual snapshot",
            );
        }
        if !kept.is_empty() {
            paths = Some(Value::Object(kept));
        }
    }

    EnvironmentOptions {
        types,
        type_roots,
        base_url,
        paths,
        degradations,
    }
}

/// Normalizes one project-relative or absolute option entry to a virtual path.
///
/// Returns `None` for absolute roots that escape every allowed root, control
/// characters, backslashes and `..` components. Relative entries are resolved
/// against the synthesized `tsconfig.json` at the project root.
fn normalize_virtual_entry(project_root: &str, entry: &str) -> Option<String> {
    if entry.is_empty() || entry.contains('\\') || entry.chars().any(char::is_control) {
        return None;
    }
    let mut entry = entry;
    while let Some(rest) = entry.strip_prefix("./") {
        entry = rest;
    }
    let entry = entry.trim_end_matches('/');
    if entry.is_empty() || entry == "." {
        return Some(project_root.to_owned());
    }
    if entry.starts_with('/') {
        if entry == "/" {
            return None;
        }
        if entry
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || component == "." || component == "..")
        {
            return None;
        }
        return Some(entry.to_owned());
    }
    if entry
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }
    Some(format!("{project_root}/{entry}"))
}

/// Returns whether one `types` entry resolves inside a virtual type root.
fn type_entry_is_virtual(
    entry: &str,
    virtual_type_roots: &[String],
    directories: &BTreeSet<String>,
) -> bool {
    if entry.is_empty()
        || entry.contains('/')
        || entry.contains('\\')
        || entry.chars().any(char::is_control)
        || matches!(entry, "." | "..")
    {
        return false;
    }
    virtual_type_roots
        .iter()
        .any(|root| directories.contains(&format!("{root}/{entry}")))
}

/// Returns the virtual directories that the engine can see.
///
/// Project directories are the ancestor closure of `project_paths` below the
/// project root; asset directories are the ancestor closure of every verified
/// declaration below its fixed root. Ancestors above those roots are never
/// inserted, so a missing directory is provably outside the snapshot.
fn virtual_directories<'a>(
    project_root: &str,
    project_paths: impl Iterator<Item = &'a str>,
    assets: &TypeAssets,
) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    directories.insert(project_root.to_owned());
    for path in project_paths {
        let mut current = parent_directory(path);
        while let Some(directory) = current {
            if !within_root(project_root, directory) {
                break;
            }
            let inserted = directories.insert(directory.to_owned());
            if !inserted || directory == project_root {
                break;
            }
            current = parent_directory(directory);
        }
    }
    let roots = [
        assets.standard_lib_root().as_str().to_owned(),
        assets.typings_root().as_str().to_owned(),
    ];
    for root in &roots {
        directories.insert(root.clone());
    }
    for asset in assets.files() {
        let path = asset.path().as_str();
        let Some(root) = roots.iter().find(|root| within_root(root, path)) else {
            continue;
        };
        let mut current = parent_directory(path);
        while let Some(directory) = current {
            if !within_root(root, directory) {
                break;
            }
            let inserted = directories.insert(directory.to_owned());
            if !inserted || directory == root.as_str() {
                break;
            }
            current = parent_directory(directory);
        }
    }
    directories
}

fn path_target_is_virtual(target: &str) -> bool {
    !target.starts_with('/') && !target.split('/').any(|component| component == "..")
}

fn push_degradation(
    degradations: &mut Vec<EnvironmentDegradation>,
    field: &'static str,
    reason: &'static str,
) {
    if !degradations
        .iter()
        .any(|item| item.field == field && item.reason == reason)
    {
        degradations.push(EnvironmentDegradation { field, reason });
    }
}

fn options_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// Upper bound for one JSON string value after escaping.
///
/// `serde_json` emits at most six bytes for a control character, two for a
/// quote or backslash, and the raw UTF-8 bytes otherwise.
fn json_escaped_upper_bound(text: &str) -> usize {
    text.chars().fold(0usize, |total, character| {
        let width = if character == '"' || character == '\\' {
            2
        } else if character.is_control() {
            6
        } else {
            character.len_utf8()
        };
        total.saturating_add(width)
    })
}

/// Upper bound for a `file://` URI after percent-encoding.
const fn file_uri_upper_bound(path: &str) -> usize {
    path.len().saturating_mul(3).saturating_add(8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::test_support::{fixture_files_at, pin_for};
    use crate::input::{
        AssetPath, CompilerOptions, DocumentRevision, Language, ProjectKey, ProjectRoot,
        SnapshotRevision, SourceFile, TypeAssets,
    };

    fn input(files: &[(&str, &str)]) -> AnalysisInput {
        let files = files
            .iter()
            .map(|(path, text)| {
                SourceFile::new(*path, *text, DocumentRevision::new(1)).expect("source file")
            })
            .collect();
        AnalysisInput::new(
            ProjectKey::new("demo"),
            ProjectRoot::new("/work/demo").expect("root"),
            SnapshotRevision::new(1),
            files,
        )
        .expect("input")
    }

    fn vfs() -> VirtualFileSystem {
        VirtualFileSystem::from_analysis_input(&input(&[
            ("main.ts", "const x = 1;\n"),
            ("model.ts", "export const y = 2;\n"),
            ("src/nested.ts", "export const z = 3;\n"),
        ]))
        .expect("vfs")
    }

    #[allow(clippy::needless_pass_by_value)]
    fn answer(vfs: &VirtualFileSystem, method: &str, params: Value) -> Value {
        vfs.answer(method, &params).expect("callback answer")
    }

    fn type_assets() -> TypeAssets {
        let (standard_lib_root, typings_root, files) =
            fixture_files_at("/pkg/engine/lib", "/pkg/typings", "vfs");
        let pin = pin_for(&standard_lib_root, &typings_root, &files);
        TypeAssets::new(
            AssetPath::new(standard_lib_root).expect("stdlib root"),
            AssetPath::new(typings_root).expect("typings root"),
            files,
            pin,
        )
        .expect("type assets")
    }

    fn environment_input(
        files: &[(&str, &str)],
        options: BTreeMap<String, Value>,
    ) -> AnalysisInput {
        let files = files
            .iter()
            .map(|(path, text)| {
                SourceFile::new(*path, *text, DocumentRevision::new(1)).expect("source file")
            })
            .collect();
        AnalysisInput::with_environment(
            ProjectKey::new("demo"),
            ProjectRoot::new("/work/demo").expect("root"),
            SnapshotRevision::new(1),
            files,
            CompilerOptions::new(options).expect("compiler options"),
            Arc::new(type_assets()),
        )
        .expect("environment input")
    }

    #[test]
    fn reads_only_virtual_files_and_never_host_files() {
        let vfs = vfs();
        assert_eq!(vfs.root(), "/work/demo");
        assert_eq!(vfs.tsconfig_path(), "/work/demo/tsconfig.json");

        let value = answer(
            &vfs,
            "readFile",
            Value::String("/work/demo/main.ts".to_owned()),
        );
        assert_eq!(value["kind"], "value");
        assert_eq!(value["value"], "const x = 1;\n");

        let missing = answer(
            &vfs,
            "readFile",
            Value::String("/work/demo/missing.ts".to_owned()),
        );
        assert_eq!(missing, json!({"kind": "missing"}));

        // A real host file outside the project must never be served.
        let host =
            std::env::temp_dir().join(format!("wxml-tsrs-vfs-host-{}.txt", std::process::id()));
        std::fs::write(&host, b"host-secret").expect("host file");
        let outside = answer(
            &vfs,
            "readFile",
            Value::String(host.to_string_lossy().into_owned()),
        );
        assert_eq!(outside, json!({"kind": "missing"}));
        let _ = std::fs::remove_file(host);
    }

    #[test]
    fn reports_existence_and_directory_entries() {
        let vfs = vfs();
        assert_eq!(
            answer(
                &vfs,
                "fileExists",
                Value::String("/work/demo/main.ts".to_owned())
            ),
            json!({"kind": "value", "value": true})
        );
        assert_eq!(
            answer(
                &vfs,
                "fileExists",
                Value::String("/work/demo/src".to_owned())
            ),
            json!({"kind": "value", "value": false})
        );
        assert_eq!(
            answer(
                &vfs,
                "directoryExists",
                Value::String("/work/demo/src".to_owned())
            ),
            json!({"kind": "value", "value": true})
        );
        assert_eq!(
            answer(&vfs, "directoryExists", Value::String("/tmp".to_owned())),
            json!({"kind": "value", "value": false})
        );

        let root = answer(
            &vfs,
            "getAccessibleEntries",
            Value::String("/work/demo".to_owned()),
        );
        assert_eq!(
            root["value"]["files"],
            json!(["main.ts", "model.ts", "tsconfig.json"])
        );
        assert_eq!(root["value"]["directories"], json!(["src"]));
        assert_eq!(root["value"]["symlinks"], json!([]));

        let nested = answer(
            &vfs,
            "getAccessibleEntries",
            Value::String("/work/demo/src".to_owned()),
        );
        assert_eq!(nested["value"]["files"], json!(["nested.ts"]));
        assert_eq!(nested["value"]["directories"], json!([]));

        let outside = answer(&vfs, "getAccessibleEntries", Value::String("/".to_owned()));
        assert_eq!(outside["value"]["files"], json!([]));
        assert_eq!(outside["value"]["directories"], json!([]));
    }

    #[test]
    fn realpath_and_stat_use_virtual_metadata() {
        let vfs = vfs();
        assert_eq!(
            answer(
                &vfs,
                "realpath",
                Value::String("/work/demo/main.ts".to_owned())
            ),
            json!({"kind": "identity"})
        );
        assert_eq!(
            answer(&vfs, "realpath", Value::String("/tmp/outside".to_owned())),
            json!({"kind": "identity"})
        );

        let file = answer(&vfs, "stat", Value::String("/work/demo/main.ts".to_owned()));
        assert_eq!(file["kind"], "value");
        assert_eq!(file["value"]["mode"], FILE_MODE);
        assert_eq!(file["value"]["size"], 13);
        assert_eq!(file["value"]["mtime"], VIRTUAL_MTIME);

        let directory = answer(&vfs, "stat", Value::String("/work/demo/src".to_owned()));
        assert_eq!(directory["value"]["mode"], DIRECTORY_MODE);
        assert_eq!(directory["value"]["size"], 0);

        assert_eq!(
            answer(
                &vfs,
                "stat",
                Value::String("/work/demo/missing.ts".to_owned())
            ),
            json!({"kind": "missing"})
        );
        assert_eq!(
            answer(&vfs, "stat", Value::String("/tmp".to_owned())),
            json!({"kind": "missing"})
        );
    }

    #[test]
    fn writes_are_noops_and_unknown_callbacks_are_errors() {
        let vfs = vfs();
        let write = answer(
            &vfs,
            "writeFile",
            json!({"path": "/work/demo/main.ts", "data": "changed"}),
        );
        assert_eq!(write, json!({"kind": "noop"}));
        assert_eq!(
            vfs.file("/work/demo/main.ts").expect("main").text(),
            "const x = 1;\n"
        );
        assert_eq!(
            answer(
                &vfs,
                "removeFile",
                Value::String("/work/demo/main.ts".to_owned())
            ),
            json!({"kind": "noop"})
        );
        assert!(vfs.file("/work/demo/main.ts").is_some());

        assert!(matches!(
            vfs.answer(
                "appendFile",
                &Value::String("/work/demo/main.ts".to_owned())
            ),
            Err(CallbackError::UnknownCallback { .. })
        ));
        assert!(matches!(
            vfs.answer("readFile", &json!({"path": "/work/demo/main.ts"})),
            Err(CallbackError::InvalidParams { .. })
        ));
        assert!(matches!(
            vfs.answer("writeFile", &Value::String("/work/demo/main.ts".to_owned())),
            Err(CallbackError::InvalidParams { .. })
        ));
    }

    #[test]
    fn traversal_and_relative_paths_fail_closed() {
        let vfs = vfs();
        for path in [
            "/work/demo/../secret.ts",
            "/work/demo//main.ts",
            "/work/demo/main.ts/",
            "relative.ts",
            "/work/demo/./main.ts",
        ] {
            let value = answer(&vfs, "readFile", Value::String(path.to_owned()));
            assert_eq!(value, json!({"kind": "missing"}), "{path}");
            let exists = answer(&vfs, "fileExists", Value::String(path.to_owned()));
            assert_eq!(exists, json!({"kind": "value", "value": false}), "{path}");
        }
    }

    #[test]
    fn missing_vfs_uses_typed_fail_closed_answers_not_errors() {
        for (method, expected) in [
            ("readFile", json!({"kind": "missing"})),
            ("stat", json!({"kind": "missing"})),
            ("fileExists", json!({"kind": "value", "value": false})),
            ("directoryExists", json!({"kind": "value", "value": false})),
            (
                "getAccessibleEntries",
                json!({
                    "kind": "value",
                    "value": {"files": [], "directories": [], "symlinks": []},
                }),
            ),
            ("realpath", json!({"kind": "identity"})),
            ("writeFile", json!({"kind": "noop"})),
            ("removeFile", json!({"kind": "noop"})),
        ] {
            assert_eq!(
                VirtualFileSystem::fail_closed_answer(method),
                Some(expected),
                "{method}"
            );
        }
        assert!(VirtualFileSystem::fail_closed_answer("appendFile").is_none());
    }

    #[test]
    fn startup_canary_rejects_a_real_host_file_outside_the_root() {
        let vfs = vfs();
        let canary = std::env::temp_dir().join(format!(
            "wxml-tsrs-vfs-canary-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::write(&canary, b"host-canary").expect("canary");
        let canary = canary.to_str().expect("utf8 canary");
        vfs.assert_confined(canary).expect("confined");
        let _ = std::fs::remove_file(canary);

        // A path inside the root is not a valid canary and must be rejected.
        assert!(matches!(
            vfs.assert_confined("/work/demo/main.ts"),
            Err(VfsError::Confinement { .. })
        ));
    }

    #[test]
    fn snapshot_request_bound_accounts_for_escaping() {
        let plain = json_escaped_upper_bound("const x = 1;\n");
        assert_eq!(plain, 18);
        let controls = json_escaped_upper_bound("\u{1}\u{2}\u{3}\u{4}");
        assert_eq!(controls, 24);

        let vfs = vfs();
        let bound = vfs.request_size_upper_bound();
        assert!(bound > 0);
        assert!(bound > vfs.file("/work/demo/main.ts").expect("file").text().len());
    }

    #[test]
    fn synthesized_tsconfig_lists_sorted_project_files() {
        let vfs = vfs();
        let tsconfig = vfs.file(vfs.tsconfig_path()).expect("tsconfig").text();
        let value: Value = serde_json::from_str(tsconfig).expect("tsconfig json");
        assert_eq!(value["compilerOptions"]["noLib"], true);
        assert_eq!(value["compilerOptions"]["strict"], true);
        assert_eq!(
            value["files"],
            json!(["main.ts", "model.ts", "src/nested.ts"])
        );
        assert!(
            vfs.source_paths()
                .all(|path| !path.ends_with("tsconfig.json"))
        );
    }

    #[test]
    fn probes_build_a_minimal_root_without_source_languages() {
        let vfs = VirtualFileSystem::from_root_and_files(
            "/tmp/opencode/probe",
            [(
                "/tmp/opencode/probe/probe.ts".to_owned(),
                "const x = 1;\n".to_owned(),
            )],
        )
        .expect("probe vfs");
        assert!(vfs.file("/tmp/opencode/probe/probe.ts").is_some());
        assert!(vfs.file("/tmp/opencode/probe/tsconfig.json").is_some());
        assert_eq!(
            Language::TypeScript,
            Language::from_path("probe.ts").expect("language")
        );
    }

    #[test]
    fn mounts_standard_lib_and_typings_as_virtual_files() {
        let input = environment_input(
            &[
                ("main.ts", "const x = 1;\n"),
                ("src/nested.ts", "export {};\n"),
            ],
            BTreeMap::new(),
        );
        let vfs = VirtualFileSystem::from_analysis_input(&input).expect("environment vfs");

        let snapshot = vfs.snapshot_files();
        assert!(snapshot.contains_key("/work/demo/main.ts"));
        assert!(snapshot.contains_key("/work/demo/tsconfig.json"));
        assert!(snapshot.contains_key("/pkg/engine/lib/lib.d.ts"));
        assert!(snapshot.contains_key("/pkg/engine/lib/lib.es5.d.ts"));
        assert!(snapshot.contains_key("/pkg/typings/miniprogram-api-typings/index.d.ts"));
        assert!(snapshot.contains_key("/pkg/typings/miniprogram-api-typings/types/index.d.ts"));

        let sources: Vec<&str> = vfs.source_paths().collect();
        assert_eq!(
            sources,
            vec!["/work/demo/main.ts", "/work/demo/src/nested.ts"]
        );
        assert!(sources.iter().all(|path| !path.contains("/pkg/")));
        assert!(vfs.absolute_paths().all(|path| !path.contains("/pkg/")));
        assert!(vfs.file("/pkg/engine/lib/lib.d.ts").is_none());
        assert!(vfs.asset("/pkg/engine/lib/lib.d.ts").is_some());
        assert_eq!(
            vfs.asset_paths().count(),
            usize::try_from(crate::input::PINNED_TYPE_ASSET_FILES).expect("count")
        );
    }

    #[test]
    fn environment_tsconfig_uses_project_options_without_forcing_defaults() {
        let options = BTreeMap::from([
            ("strict".to_owned(), Value::Bool(false)),
            ("target".to_owned(), Value::String("ES2020".to_owned())),
            ("module".to_owned(), Value::String("CommonJS".to_owned())),
            (
                "moduleResolution".to_owned(),
                Value::String("node10".to_owned()),
            ),
            ("skipLibCheck".to_owned(), Value::Bool(true)),
            (
                "lib".to_owned(),
                Value::Array(vec![Value::String("es2020".to_owned())]),
            ),
            ("allowJs".to_owned(), Value::Bool(true)),
            ("checkJs".to_owned(), Value::Bool(false)),
            (
                "types".to_owned(),
                Value::Array(vec![Value::String("custom".to_owned())]),
            ),
        ]);
        let input = environment_input(
            &[
                ("model.ts", "export const y = 2;\n"),
                ("main.ts", "const x = 1;\n"),
            ],
            options,
        );
        let vfs = VirtualFileSystem::from_analysis_input(&input).expect("environment vfs");
        let text = vfs.file(vfs.tsconfig_path()).expect("tsconfig").text();
        let value: Value = serde_json::from_str(text).expect("tsconfig json");
        let compiler = &value["compilerOptions"];

        assert!(compiler.get("noLib").is_none());
        assert_eq!(compiler["noEmit"], true);
        assert_eq!(compiler["strict"], false);
        assert_eq!(compiler["target"], "ES2020");
        assert_eq!(compiler["module"], "CommonJS");
        assert_eq!(compiler["moduleResolution"], "node10");
        assert_eq!(compiler["skipLibCheck"], true);
        assert_eq!(compiler["allowJs"], true);
        assert_eq!(compiler["checkJs"], false);
        assert_eq!(compiler["lib"], json!(["es2020"]));
        assert_eq!(compiler["types"], json!(["miniprogram-api-typings"]));
        assert_eq!(compiler["typeRoots"], json!(["/pkg/typings"]));
        assert_eq!(value["files"], json!(["main.ts", "model.ts"]));
        assert!(compiler.get("moduleDetection").is_none());
        assert!(
            vfs.environment_degradations()
                .iter()
                .any(|item| item.field() == "types")
        );
    }

    #[test]
    fn legacy_inputs_keep_no_lib_and_mount_no_assets() {
        for vfs in [
            VirtualFileSystem::from_analysis_input(&input(&[("main.ts", "")])).expect("vfs"),
            VirtualFileSystem::from_root_and_files(
                "/tmp/opencode/probe",
                [("/tmp/opencode/probe/probe.ts".to_owned(), String::new())],
            )
            .expect("probe vfs"),
        ] {
            let value: Value =
                serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
                    .expect("tsconfig json");
            assert_eq!(value["compilerOptions"]["noLib"], true);
            assert!(vfs.asset_paths().next().is_none());
            assert_eq!(vfs.snapshot_files().len(), vfs.absolute_paths().count());
        }
    }

    #[test]
    fn keeps_asset_type_roots_and_rejects_relative_base_url_escape() {
        let options = BTreeMap::from([
            ("typeRoots".to_owned(), json!(["/pkg/typings"])),
            ("baseUrl".to_owned(), json!("../outside")),
            ("paths".to_owned(), json!({"@/*": ["src/*"]})),
        ]);
        let vfs =
            VirtualFileSystem::from_analysis_input(&environment_input(&[("main.ts", "")], options))
                .expect("environment vfs");
        let value: Value =
            serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
                .expect("tsconfig json");
        assert_eq!(
            value["compilerOptions"]["typeRoots"],
            json!(["/pkg/typings"])
        );
        assert!(value["compilerOptions"].get("baseUrl").is_none());
        assert!(value["compilerOptions"].get("paths").is_none());
        assert!(
            vfs.environment_degradations()
                .iter()
                .any(|item| item.field() == "baseUrl")
        );
    }

    #[test]
    fn keeps_virtual_base_url_and_partially_filters_paths() {
        let options = BTreeMap::from([
            (
                "typeRoots".to_owned(),
                json!(["/work/demo/types", "/pkg/typings"]),
            ),
            ("baseUrl".to_owned(), json!("/work/demo")),
            (
                "paths".to_owned(),
                json!({
                    "@good/*": ["src/*"],
                    "@escape/*": ["../outside/*"],
                    "@absolute/*": ["/etc/*"],
                }),
            ),
        ]);
        let vfs = VirtualFileSystem::from_analysis_input(&environment_input(
            &[("main.ts", ""), ("types/globals.d.ts", "")],
            options,
        ))
        .expect("environment vfs");
        let value: Value =
            serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
                .expect("tsconfig json");
        let compiler = &value["compilerOptions"];
        assert_eq!(compiler["baseUrl"], "/work/demo");
        assert_eq!(
            compiler["typeRoots"],
            json!(["/work/demo/types", "/pkg/typings"])
        );
        assert_eq!(compiler["paths"], json!({"@good/*": ["src/*"]}));
        assert!(
            vfs.environment_degradations()
                .iter()
                .any(|item| item.field() == "paths")
        );
    }

    #[test]
    fn filters_project_resolution_options_outside_the_virtual_snapshot() {
        let options = BTreeMap::from([
            (
                "typeRoots".to_owned(),
                json!(["/etc/types", "../outside-types", "local-types"]),
            ),
            ("baseUrl".to_owned(), json!("/outside/base")),
            (
                "paths".to_owned(),
                json!({"@/*": ["src/*"], "@escape/*": ["../../etc/*"]}),
            ),
        ]);
        let vfs =
            VirtualFileSystem::from_analysis_input(&environment_input(&[("main.ts", "")], options))
                .expect("environment vfs");
        let value: Value =
            serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
                .expect("tsconfig json");
        let compiler = &value["compilerOptions"];
        assert_eq!(compiler["typeRoots"], json!(["/pkg/typings"]));
        assert!(compiler.get("baseUrl").is_none());
        assert!(compiler.get("paths").is_none());

        let degradations = vfs.environment_degradations();
        assert!(degradations.iter().any(|item| item.field() == "typeRoots"));
        assert!(degradations.iter().any(|item| item.field() == "baseUrl"));
        assert!(degradations.iter().any(|item| item.field() == "paths"));
    }

    #[test]
    fn types_and_relative_type_roots_outside_the_snapshot_are_degraded() {
        let options = BTreeMap::from([
            (
                "typeRoots".to_owned(),
                json!(["types", "missing-types", "../escape"]),
            ),
            (
                "types".to_owned(),
                json!([
                    "miniprogram-api-typings",
                    "custom",
                    "../escape",
                    "/absolute"
                ]),
            ),
        ]);
        let input = environment_input(
            &[
                ("main.ts", "const x = 1;\n"),
                ("types/local/index.d.ts", "export {};\n"),
            ],
            options,
        );
        let vfs = VirtualFileSystem::from_analysis_input(&input).expect("environment vfs");
        let value: Value =
            serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
                .expect("tsconfig json");
        let compiler = &value["compilerOptions"];
        assert_eq!(compiler["typeRoots"], json!(["types", "/pkg/typings"]));
        assert_eq!(compiler["types"], json!(["miniprogram-api-typings"]));
        let degradations = vfs.environment_degradations();
        assert!(degradations.iter().any(|item| item.field() == "typeRoots"));
        assert!(degradations.iter().any(|item| item.field() == "types"));
        assert!(degradations.iter().all(|item| !item.reason().contains('/')));

        let assets = input.type_assets().expect("trusted assets");
        let planned = environment_degradations_for(
            "/work/demo",
            &[
                "/work/demo/main.ts".to_owned(),
                "/work/demo/types/local/index.d.ts".to_owned(),
            ],
            input.compiler_options(),
            assets,
        );
        assert_eq!(planned.as_slice(), degradations);
    }

    #[test]
    fn types_resolve_under_a_local_virtual_type_root() {
        let options = BTreeMap::from([
            ("typeRoots".to_owned(), json!(["types"])),
            (
                "types".to_owned(),
                json!(["local", "miniprogram-api-typings"]),
            ),
        ]);
        let vfs = VirtualFileSystem::from_analysis_input(&environment_input(
            &[("main.ts", ""), ("types/local/index.d.ts", "export {};\n")],
            options,
        ))
        .expect("environment vfs");
        let value: Value =
            serde_json::from_str(vfs.file(vfs.tsconfig_path()).expect("tsconfig").text())
                .expect("tsconfig json");
        assert_eq!(
            value["compilerOptions"]["typeRoots"],
            json!(["types", "/pkg/typings"])
        );
        assert_eq!(
            value["compilerOptions"]["types"],
            json!(["local", "miniprogram-api-typings"])
        );
        assert!(
            vfs.environment_degradations()
                .iter()
                .all(|item| item.field() != "types")
        );
    }

    #[test]
    fn rejects_asset_project_collisions() {
        let project_files = vec![
            SourceFile::new(
                "lib.d.ts",
                "declare const projectLib: true;\n",
                DocumentRevision::new(1),
            )
            .expect("project d.ts"),
        ];
        let (standard_lib_root, typings_root, asset_files) =
            fixture_files_at("/work/demo", "/pkg/typings-collision", "collision");
        let pin = pin_for(&standard_lib_root, &typings_root, &asset_files);
        let assets = TypeAssets::new(
            AssetPath::new(standard_lib_root).expect("stdlib root"),
            AssetPath::new(typings_root).expect("typings root"),
            asset_files,
            pin,
        )
        .expect("assets");
        let input = AnalysisInput::with_environment(
            ProjectKey::new("demo"),
            ProjectRoot::new("/work/demo").expect("root"),
            SnapshotRevision::new(1),
            project_files,
            CompilerOptions::default(),
            Arc::new(assets),
        )
        .expect("input");
        assert!(matches!(
            VirtualFileSystem::from_analysis_input(&input),
            Err(VfsError::AssetCollision { .. })
        ));
    }

    #[test]
    fn asset_directories_answer_component_confined_entries() {
        let vfs = VirtualFileSystem::from_analysis_input(&environment_input(
            &[("main.ts", "")],
            BTreeMap::new(),
        ))
        .expect("environment vfs");

        let stdlib = answer(
            &vfs,
            "getAccessibleEntries",
            Value::String("/pkg/engine/lib".to_owned()),
        );
        assert_eq!(
            stdlib["value"]["files"].as_array().map(Vec::len),
            Some(usize::try_from(crate::input::PINNED_STANDARD_LIB_FILE_COUNT).expect("count"))
        );
        assert_eq!(stdlib["value"]["files"][0], "lib.d.ts");
        assert_eq!(stdlib["value"]["directories"], json!([]));

        let typings = answer(
            &vfs,
            "getAccessibleEntries",
            Value::String("/pkg/typings/miniprogram-api-typings".to_owned()),
        );
        assert_eq!(typings["value"]["files"], json!(["index.d.ts"]));
        assert_eq!(typings["value"]["directories"], json!(["types"]));

        let typings_root = answer(
            &vfs,
            "getAccessibleEntries",
            Value::String("/pkg/typings".to_owned()),
        );
        assert_eq!(typings_root["value"]["files"], json!([]));
        assert_eq!(
            typings_root["value"]["directories"],
            json!(["miniprogram-api-typings"])
        );

        for outside in ["/", "/pkg", "/pkg/engine", "/work"] {
            let entries = answer(
                &vfs,
                "getAccessibleEntries",
                Value::String(outside.to_owned()),
            );
            assert_eq!(entries["value"]["files"], json!([]), "{outside}");
            assert_eq!(entries["value"]["directories"], json!([]), "{outside}");
            assert_eq!(
                answer(&vfs, "directoryExists", Value::String(outside.to_owned())),
                json!({"kind": "value", "value": false}),
                "{outside}"
            );
        }
    }

    #[test]
    fn reads_and_stats_assets_without_exposing_them_as_project_files() {
        let vfs = VirtualFileSystem::from_analysis_input(&environment_input(
            &[("main.ts", "")],
            BTreeMap::new(),
        ))
        .expect("environment vfs");

        let path = "/pkg/engine/lib/lib.d.ts";
        assert_eq!(
            answer(&vfs, "readFile", Value::String(path.to_owned())),
            json!({"kind": "value", "value": "declare const stdlib0: number; // vfs\n"})
        );
        assert_eq!(
            answer(&vfs, "fileExists", Value::String(path.to_owned())),
            json!({"kind": "value", "value": true})
        );
        assert_eq!(
            answer(
                &vfs,
                "directoryExists",
                Value::String("/pkg/engine/lib".to_owned())
            ),
            json!({"kind": "value", "value": true})
        );
        let stat = answer(&vfs, "stat", Value::String(path.to_owned()));
        assert_eq!(stat["kind"], "value");
        assert_eq!(stat["value"]["mode"], FILE_MODE);
        assert_eq!(
            stat["value"]["size"],
            u64::try_from("declare const stdlib0: number; // vfs\n".len()).expect("size")
        );

        // Still fail closed for real host files outside both tables.
        assert_eq!(
            answer(&vfs, "readFile", Value::String("/etc/passwd".to_owned())),
            json!({"kind": "missing"})
        );
        assert_eq!(
            answer(&vfs, "stat", Value::String("/etc/passwd".to_owned())),
            json!({"kind": "missing"})
        );
    }

    #[test]
    fn request_bound_includes_assets() {
        let legacy =
            VirtualFileSystem::from_analysis_input(&input(&[("main.ts", "")])).expect("legacy vfs");
        let environment = VirtualFileSystem::from_analysis_input(&environment_input(
            &[("main.ts", "")],
            BTreeMap::new(),
        ))
        .expect("environment vfs");

        let asset_bytes: usize = environment
            .asset_paths()
            .filter_map(|path| environment.asset(path))
            .map(|file| file.text().len())
            .sum();
        assert!(asset_bytes > 0);
        assert!(environment.request_size_upper_bound() > legacy.request_size_upper_bound());
        assert!(
            environment.request_size_upper_bound()
                > legacy.request_size_upper_bound() + asset_bytes
        );
    }
}
