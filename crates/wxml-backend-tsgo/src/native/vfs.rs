//! Read-only virtual filesystem and engine filesystem callbacks.
//!
//! The engine may only see files from the current snapshot and directory
//! listings derived from those files. Every callback answer comes from the
//! in-memory table; the host filesystem is never consulted and `useOS` is
//! never returned. Paths outside the project root fail closed with the
//! callback-specific negative shape:
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

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::input::{AnalysisInput, InputError, ProjectRoot};

/// Name of the synthesized project configuration file.
pub const VIRTUAL_TSCONFIG: &str = "tsconfig.json";

/// Fixed modification time reported for virtual files (`RFC3339Nano`).
pub const VIRTUAL_MTIME: &str = "1970-01-01T00:00:00Z";

/// POSIX mode reported for a regular virtual file.
pub const FILE_MODE: u32 = 0o100_644;

/// POSIX mode reported for a virtual directory.
pub const DIRECTORY_MODE: u32 = 0o040_755;

/// One file in the virtual snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualFile {
    text: String,
    hash: String,
    size: u64,
}

impl VirtualFile {
    fn new(text: String) -> Self {
        let hash = hex_lower(&Sha256::digest(text.as_bytes()));
        let size = u64::try_from(text.len()).unwrap_or(u64::MAX);
        Self { text, hash, size }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualFileSystem {
    root: String,
    files: BTreeMap<String, VirtualFile>,
    directories: BTreeMap<String, DirectoryEntries>,
    tsconfig_path: String,
}

impl VirtualFileSystem {
    /// Builds the virtual filesystem for one analysis input.
    ///
    /// A `tsconfig.json` with `noLib`, `strict` and the sorted project files is
    /// synthesized at the project root. The input's own files must be
    /// TypeScript/JavaScript; the configuration is separate and never appears
    /// in [`AnalysisInput`].
    ///
    /// # Errors
    ///
    /// Rejects duplicate absolute paths or an unusable project root.
    pub fn from_analysis_input(input: &AnalysisInput) -> Result<Self, VfsError> {
        let root = input.root().as_str().to_owned();
        let mut files = BTreeMap::new();
        let mut relative_paths = Vec::new();
        for file in input.files() {
            let absolute = join_root(&root, file.path())?;
            if files
                .insert(absolute, VirtualFile::new(file.text().to_owned()))
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
        let tsconfig = tsconfig_text(&relative_paths);
        let tsconfig_path = join_root(&root, VIRTUAL_TSCONFIG)?;
        files.insert(tsconfig_path.clone(), VirtualFile::new(tsconfig));

        Self::build(root, files, tsconfig_path)
    }

    /// Builds a virtual filesystem from explicit absolute paths and texts.
    ///
    /// This is used by the handshake/schema probe, which needs a minimal
    /// snapshot without an [`AnalysisInput`].
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
                if let Some(rest) = path.strip_prefix(&root) {
                    if let Some(rest) = rest.strip_prefix('/') {
                        relative.push(rest.to_owned());
                    }
                }
            }
            relative.sort();
            relative.dedup();
            table.insert(
                tsconfig_path.clone(),
                VirtualFile::new(tsconfig_text(&relative)),
            );
        }
        Self::build(root, table, tsconfig_path)
    }

    fn build(
        root: String,
        files: BTreeMap<String, VirtualFile>,
        tsconfig_path: String,
    ) -> Result<Self, VfsError> {
        let mut directories = BTreeMap::<String, DirectoryEntries>::new();
        directories.entry(root.clone()).or_default();

        for path in files.keys() {
            let Some(parent) = parent_directory(path) else {
                return Err(VfsError::InvalidPath {
                    reason: "virtual file has no parent directory",
                });
            };
            if !within_root(&root, parent) {
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
            while within_root(&root, current) && current != root {
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

        for entries in directories.values_mut() {
            entries.files.sort();
            entries.files.dedup();
            entries.directories.sort();
            entries.directories.dedup();
        }

        Ok(Self {
            root,
            files,
            directories,
            tsconfig_path,
        })
    }

    /// Returns the normalized project root.
    #[must_use]
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Returns the absolute path of the synthesized `tsconfig.json`.
    #[must_use]
    pub fn tsconfig_path(&self) -> &str {
        &self.tsconfig_path
    }

    /// Returns all absolute paths, including the configuration file.
    pub fn absolute_paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Returns only absolute source-file paths (excluding `tsconfig.json`).
    pub fn source_paths(&self) -> impl Iterator<Item = &str> {
        self.files
            .keys()
            .map(String::as_str)
            .filter(|path| *path != self.tsconfig_path)
    }

    /// Returns the file at an absolute path.
    #[must_use]
    pub fn file(&self, absolute: &str) -> Option<&VirtualFile> {
        self.files.get(absolute)
    }

    /// Returns the content hash at an absolute path.
    #[must_use]
    pub fn hash(&self, absolute: &str) -> Option<&str> {
        self.files.get(absolute).map(VirtualFile::hash)
    }

    /// Returns an owned copy of every absolute path and text for snapshot
    /// request construction.
    #[must_use]
    pub fn snapshot_files(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
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
                Ok(match self.classify(path) {
                    PathClass::InRoot(path) if self.files.contains_key(path) => {
                        json!({"kind": "value", "value": self.files.get(path).map(VirtualFile::text)})
                    }
                    _ => json!({"kind": "missing"}),
                })
            }
            "fileExists" => {
                let path = path_param(params, "fileExists")?;
                let exists = matches!(self.classify(path), PathClass::InRoot(path) if self.files.contains_key(path));
                Ok(json!({"kind": "value", "value": exists}))
            }
            "directoryExists" => {
                let path = path_param(params, "directoryExists")?;
                let exists = matches!(self.classify(path), PathClass::InRoot(path) if self.directories.contains_key(path));
                Ok(json!({"kind": "value", "value": exists}))
            }
            "getAccessibleEntries" => {
                let path = path_param(params, "getAccessibleEntries")?;
                let entries = match self.classify(path) {
                    PathClass::InRoot(path) => self.directories.get(path),
                    PathClass::Outside | PathClass::Invalid => None,
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
        for (path, file) in &self.files {
            total = total.saturating_add(json_escaped_upper_bound(path));
            total = total.saturating_add(json_escaped_upper_bound(file.text()));
            total = total.saturating_add(file_uri_upper_bound(path).saturating_mul(2));
            total = total.saturating_add(128);
        }
        total.saturating_add(file_uri_upper_bound(&self.tsconfig_path))
    }

    fn stat_value(&self, path: &str) -> Value {
        let (mode, size) = match self.classify(path) {
            PathClass::InRoot(path) => {
                if let Some(file) = self.files.get(path) {
                    (FILE_MODE, file.size())
                } else if self.directories.contains_key(path) {
                    (DIRECTORY_MODE, 0)
                } else {
                    return json!({"kind": "missing"});
                }
            }
            PathClass::Outside | PathClass::Invalid => return json!({"kind": "missing"}),
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

    fn classify<'a>(&self, path: &'a str) -> PathClass<'a> {
        if validate_absolute(path).is_err() {
            return PathClass::Invalid;
        }
        if within_root(&self.root, path) {
            PathClass::InRoot(path)
        } else {
            PathClass::Outside
        }
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
}

enum PathClass<'a> {
    InRoot(&'a str),
    Outside,
    Invalid,
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

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        if let (Some(&high), Some(&low)) = (
            DIGITS.get(usize::from(byte >> 4)),
            DIGITS.get(usize::from(byte & 0x0f)),
        ) {
            encoded.push(char::from(high));
            encoded.push(char::from(low));
        }
    }
    encoded
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
    use crate::input::{
        DocumentRevision, Language, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
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
            std::env::temp_dir().join(format!("wxml-tsgo-vfs-host-{}.txt", std::process::id()));
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
            "wxml-tsgo-vfs-canary-{}-{}.txt",
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
}
