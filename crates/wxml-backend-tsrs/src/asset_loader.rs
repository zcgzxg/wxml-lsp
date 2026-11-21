//! Trusted on-disk loader for the pinned native type-asset set.
//!
//! [`TypeAssets::load_pinned`] is the full production entry point: it walks two
//! caller-supplied package roots, reads every TypeScript declaration once and
//! validates the result against the crate's built-in [`TypeAssetsPin::pinned_runtime`].
//! [`TypeAssets::load_pinned_typings`] is the typings-only entry point for the
//! in-process engine: the standard library is bundled with the engine, so only
//! the `miniprogram-api-typings` package is read and verified against
//! [`TypeAssetsPin::pinned_typings_only`]. [`TypeAssets::load_trusted`] and
//! [`TypeAssets::load_trusted_typings`] are the same walks with a caller-supplied
//! pin and exist for tests and for callers that already verified a manifest.
//!
//! In full mode the standard-library root is the engine's `lib` directory
//! (flat); in typings-only mode there is no such root. The typings root is the
//! directory that contains the `miniprogram-api-typings` package. Only files
//! below those roots are read.
//!
//! # Platform support
//!
//! This loader is POSIX-only:
//!
//! - roots are absolute, `/`-separated paths; Windows path semantics are not
//!   supported;
//! - on Unix, declarations are opened with `O_NOFOLLOW | O_NONBLOCK` and then
//!   verified with `fstat` (`regular file`, length, `dev`, `ino`, `nlink`);
//! - on non-Unix targets only the pre-open size, the read length and the
//!   content digest are verified; liveness/containment hardening and FIFO
//!   rejection are not available there.
//!
//! # Security model
//!
//! The loader never canonicalizes a root. On Unix, symbolic links are refused
//! rather than resolved and declarations are opened with `O_NOFOLLOW`; on
//! non-Unix targets symlink refusal is a pre-open check only. Every visited
//! entry is inspected with `symlink_metadata`; symlinks, non-regular files,
//! hard-linked files (`nlink != 1`) and names that are not valid UTF-8 or not
//! portable path components are rejected. After opening, the file's identity
//! (`dev`/`ino`), link count and length are compared with the pre-open
//! metadata, reads are capped at the declared length and any change fails
//! closed. On targets without the inlined flag constants only the pre/post
//! identity check and digest verification are available.
//!
//! Names are compared case-insensitively so a case-insensitive file system
//! cannot merge two pinned declarations. Every directory entry counts against
//! [`MAX_ASSET_ENTRIES`], including hidden names, directories and
//! non-declaration files. Path bytes and declaration text share the
//! [`MAX_TYPE_ASSET_BYTES`] budget and are charged while walking, so the tree
//! is never materialized as a whole.
//!
//! Only unpacked delivery trees are supported. Package-manager layouts that
//! hard-link files (for example a pnpm store) are rejected by the `nlink`
//! check; this loader does not extract archives or repair links.
//!
//! # Asset set
//!
//! Every `.d.ts` file below the roots is part of the mounted set, so an extra
//! declaration changes the count or bundle digest and is rejected. Other
//! regular files that ship in the package (`tsc`, `package.json`, licenses,
//! READMEs) are scanned and counted, but not read, hashed or mounted, so they
//! cannot influence the type environment.
//!
//! Virtual asset paths are validated with [`AssetPath`]. A path that exceeds
//! the path/component limits maps to [`TypeAssetError::InvalidRoot`] (the
//! existing variant used by [`AssetPath::new`]), even though the offending
//! value is a file path rather than a root.
//!
//! # Pin provenance
//!
//! [`TypeAssetsPin::new`] only anchors the typings version and both file counts
//! to crate constants. It accepts arbitrary byte totals and bundle digests, so
//! a pin derived from the same tree that is about to be loaded proves nothing.
//! Production wiring must use [`TypeAssets::load_pinned`] or pass a pin that
//! was independently verified against the delivery manifest.
//!
//! # Boundary
//!
//! This is **not** install verification. The caller must still establish that
//! the supplied roots belong to the verified delivery package and must verify
//! the delivery manifest; reconciling the packaged typings payload with the
//! runtime 20-file closure remains **BLOCKED** under P6-3-4/F3-03. The loader
//! only proves that the files under the supplied roots match the supplied pin.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::input::{
    AssetPath, MAX_ASSET_COMPONENT_BYTES, MAX_ASSET_PATH_COMPONENTS, MAX_TYPE_ASSET_BYTES,
    MAX_TYPE_ASSET_FILES, MINIPROGRAM_TYPINGS, TypeAssetError, TypeAssetFile, TypeAssets,
    TypeAssetsPin, hex_lower,
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Maximum number of directory entries visited in one load.
///
/// The counter is cumulative across both roots and includes hidden names,
/// directories and non-declaration files. It bounds traversal work and index
/// memory even when a package root contains far more entries than the pinned
/// declaration set.
pub const MAX_ASSET_ENTRIES: usize = 4096;

#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NOFOLLOW: i32 = 0o400_000;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: i32 = 0o4000;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const O_NONBLOCK: i32 = 0x0004;

#[cfg(any(target_os = "solaris", target_os = "illumos"))]
const O_NOFOLLOW: i32 = 0x20000;
#[cfg(any(target_os = "solaris", target_os = "illumos"))]
const O_NONBLOCK: i32 = 0x0080;

// Constants are inlined because this crate has no direct `libc` dependency and
// the dependency set is frozen. Other Unix targets fall back to the pre-open
// `symlink_metadata` check plus the post-open identity check below.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "solaris",
    target_os = "illumos"
)))]
const O_NOFOLLOW: i32 = 0;
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "solaris",
    target_os = "illumos"
)))]
const O_NONBLOCK: i32 = 0;

impl TypeAssets {
    /// Loads and verifies a type-asset set against the built-in production pin.
    ///
    /// This is the production entry point. It fixes the pin to
    /// [`TypeAssetsPin::pinned_runtime`] and does not accept a caller-supplied
    /// anchor, so a caller cannot accidentally derive the expected bytes or
    /// digests from the tree being loaded.
    ///
    /// `engine_root` is the engine `lib` directory that contains the pinned
    /// `lib*.d.ts` files. `typings_root` is the directory that contains the
    /// `miniprogram-api-typings` package. Both roots must be absolute,
    /// normalized paths.
    ///
    /// # Errors
    ///
    /// See [`Self::load_trusted`].
    pub fn load_pinned(engine_root: &Path, typings_root: &Path) -> Result<Self, TypeAssetError> {
        Self::load_trusted(engine_root, typings_root, &TypeAssetsPin::pinned_runtime())
    }

    /// Loads and verifies a type-asset set from two trusted on-disk roots.
    ///
    /// `engine_root` is the engine `lib` directory that contains the pinned
    /// `lib*.d.ts` files. `typings_root` is the directory that contains the
    /// `miniprogram-api-typings` package; the package directory is the only
    /// subtree walked for the second root. Both roots must be absolute,
    /// normalized paths. The pin is mandatory and every recomputed count, byte
    /// total and bundle digest must match it.
    ///
    /// `pin` must come from [`TypeAssetsPin::pinned_runtime`] or from a
    /// manifest whose byte totals and bundle digests were verified
    /// independently. [`TypeAssetsPin::new`] anchors only version and counts;
    /// it does not establish provenance for bytes or digests. Production
    /// callers should use [`Self::load_pinned`].
    ///
    /// Limits: at most [`MAX_ASSET_ENTRIES`] directory entries,
    /// [`MAX_TYPE_ASSET_FILES`] declaration files and [`MAX_TYPE_ASSET_BYTES`]
    /// combined declaration text plus virtual-path bytes, in addition to the
    /// exact per-root counts, byte totals and bundle digests in `pin`. With
    /// [`TypeAssetsPin::pinned_runtime`] that means 116 standard-library files
    /// (3,870,196 B) and 20 runtime typings files (2,522,752 B).
    ///
    /// # Errors
    ///
    /// Fails closed with a [`TypeAssetError`] for a missing or unsafe root, a
    /// symlink, hard link, non-regular file, non-UTF-8 or non-portable name,
    /// case collision, over-entry/over-budget tree, non-UTF-8 declaration, a
    /// file that changes while being read, a missing required declaration
    /// (`lib.d.ts`, `index.d.ts`, `types/index.d.ts`) or any pin mismatch.
    /// Over-long or non-portable virtual asset paths are reported as
    /// [`TypeAssetError::InvalidRoot`], inherited from [`AssetPath::new`].
    /// Callers must not start a native session after an error.
    ///
    /// This function does not verify the install or the delivery manifest; see
    /// the module documentation.
    pub fn load_trusted(
        engine_root: &Path,
        typings_root: &Path,
        pin: &TypeAssetsPin,
    ) -> Result<Self, TypeAssetError> {
        load_trusted_type_assets(engine_root, typings_root, pin)
    }

    /// Loads and verifies a typings-only asset set against the built-in pin.
    ///
    /// This is the production entry point for the in-process engine: the
    /// TypeScript standard library is bundled with the engine, so only the
    /// `miniprogram-api-typings` package below `typings_root` is read and
    /// verified. No `engine/lib` directory or `lib*.d.ts` file is required.
    ///
    /// # Errors
    ///
    /// See [`Self::load_trusted_typings`].
    pub fn load_pinned_typings(typings_root: &Path) -> Result<Self, TypeAssetError> {
        Self::load_trusted_typings(typings_root, &TypeAssetsPin::pinned_typings_only())
    }

    /// Loads and verifies a typings-only asset set against a caller-supplied pin.
    ///
    /// `typings_root` is the directory that contains the
    /// `miniprogram-api-typings` package; only that package subtree is walked.
    /// The pin must come from [`TypeAssetsPin::pinned_typings_only`] or from an
    /// independently verified manifest.
    ///
    /// # Errors
    ///
    /// Fails closed with a [`TypeAssetError`] for a missing or unsafe root, a
    /// symlink, hard link, non-regular file, non-UTF-8 or non-portable name,
    /// case collision, over-entry/over-budget tree, non-UTF-8 declaration, a
    /// file that changes while being read, a missing typings entry point
    /// (`index.d.ts`, `types/index.d.ts`) or any pin mismatch.
    pub fn load_trusted_typings(
        typings_root: &Path,
        pin: &TypeAssetsPin,
    ) -> Result<Self, TypeAssetError> {
        load_trusted_typings_type_assets(typings_root, pin)
    }
}

/// Loads a typings-only asset set from a trusted on-disk root.
///
/// This is the free-function form of [`TypeAssets::load_trusted_typings`]; see
/// its documentation for the accepted layout, guarantees and errors.
///
/// # Errors
///
/// See [`TypeAssets::load_trusted_typings`].
pub fn load_trusted_typings_type_assets(
    typings_root: &Path,
    pin: &TypeAssetsPin,
) -> Result<TypeAssets, TypeAssetError> {
    let typings = Root::open(typings_root)?;
    let package = Root::open_child(&typings, MINIPROGRAM_TYPINGS)?;

    let mut walk = Walk::default();
    package.collect_tree("", 0, &mut walk)?;

    TypeAssets::new_typings_only(typings.virtual_root, walk.files, pin.clone())
}

/// Loads a complete type-asset set from trusted on-disk roots.
///
/// This is the free-function form of [`TypeAssets::load_trusted`]; see its
/// documentation for the accepted layout, guarantees and errors.
///
/// # Errors
///
/// See [`TypeAssets::load_trusted`].
pub fn load_trusted_type_assets(
    engine_root: &Path,
    typings_root: &Path,
    pin: &TypeAssetsPin,
) -> Result<TypeAssets, TypeAssetError> {
    let engine = Root::open(engine_root)?;
    let typings = Root::open(typings_root)?;
    let package = Root::open_child(&typings, MINIPROGRAM_TYPINGS)?;

    let mut walk = Walk::default();
    engine.collect_flat(&mut walk)?;
    package.collect_tree("", 0, &mut walk)?;

    TypeAssets::new(
        engine.virtual_root,
        typings.virtual_root,
        walk.files,
        pin.clone(),
    )
}

/// One validated root directory and its virtual spelling.
#[derive(Debug)]
struct Root {
    disk: PathBuf,
    virtual_root: AssetPath,
}

/// Validates that `disk` is a real directory and returns its virtual path.
fn validate_directory(disk: &Path) -> Result<AssetPath, TypeAssetError> {
    let Some(disk_text) = disk.to_str() else {
        return Err(TypeAssetError::NonUtf8Path {
            path: disk.to_string_lossy().into_owned(),
        });
    };
    let root = AssetPath::new(disk_text)?;
    let metadata = fs::symlink_metadata(disk).map_err(|source| match source.kind() {
        ErrorKind::NotFound => TypeAssetError::MissingRoot {
            root: disk_text.to_owned(),
        },
        _ => io_error(disk, source),
    })?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(TypeAssetError::Symlink {
            path: disk_text.to_owned(),
        });
    }
    if !file_type.is_dir() {
        return Err(TypeAssetError::RootNotDirectory {
            root: disk_text.to_owned(),
        });
    }
    Ok(root)
}

impl Root {
    fn open(root: &Path) -> Result<Self, TypeAssetError> {
        let virtual_root = validate_directory(root)?;
        Ok(Self {
            disk: root.to_path_buf(),
            virtual_root,
        })
    }

    fn open_child(parent: &Self, name: &str) -> Result<Self, TypeAssetError> {
        let disk = parent.disk.join(name);
        let virtual_root = validate_directory(&disk)?;
        Ok(Self { disk, virtual_root })
    }

    /// Collects the flat engine `lib` directory.
    fn collect_flat(&self, walk: &mut Walk) -> Result<(), TypeAssetError> {
        for entry in walk.read_dir(&self.disk)? {
            let name = entry_name(&self.disk, &entry)?;
            let virtual_path = self.virtual_path(&name);
            walk.insert_path(&virtual_path)?;
            let path = entry.path();
            let metadata = symlink_metadata(&path, &virtual_path)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(TypeAssetError::Symlink { path: virtual_path });
            }
            if file_type.is_dir() {
                return Err(TypeAssetError::UnexpectedEntry {
                    path: virtual_path,
                    reason: "directory in a flat asset root",
                });
            }
            if !file_type.is_file() {
                return Err(TypeAssetError::NonRegularFile { path: virtual_path });
            }
            if !is_declaration(&name) {
                continue;
            }
            walk.read_asset(&path, &virtual_path, &metadata)?;
        }
        Ok(())
    }

    /// Collects the typings package recursively.
    ///
    /// `virtual_dir` is empty at the package root and `/`-joined below it.
    fn collect_tree(
        &self,
        virtual_dir: &str,
        depth: usize,
        walk: &mut Walk,
    ) -> Result<(), TypeAssetError> {
        if depth > MAX_ASSET_PATH_COMPONENTS {
            return Err(TypeAssetError::NestingTooDeep {
                path: self.virtual_path(virtual_dir),
            });
        }
        for entry in walk.read_dir(&self.disk)? {
            let name = entry_name(&self.disk, &entry)?;
            let relative = relative_path(virtual_dir, &name);
            let virtual_path = self.virtual_path(&relative);
            walk.insert_path(&virtual_path)?;
            let path = entry.path();
            let metadata = symlink_metadata(&path, &virtual_path)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(TypeAssetError::Symlink { path: virtual_path });
            }
            if file_type.is_dir() {
                let child = Self {
                    disk: path,
                    virtual_root: self.virtual_root.clone(),
                };
                child.collect_tree(&relative, depth + 1, walk)?;
            } else if !file_type.is_file() {
                return Err(TypeAssetError::NonRegularFile { path: virtual_path });
            } else if is_declaration(&name) {
                walk.read_asset(&path, &virtual_path, &metadata)?;
            }
        }
        Ok(())
    }

    fn virtual_path(&self, relative: &str) -> String {
        format!("{}/{}", self.virtual_root.as_str(), relative)
    }
}

fn relative_path(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

/// Walk state: collected declarations, budgets, entry count and seen paths.
#[derive(Debug, Default)]
struct Walk {
    files: Vec<TypeAssetFile>,
    budget: AssetBudget,
    seen: SeenPaths,
    entries: usize,
}

impl Walk {
    /// Reads one directory with the entry limit enforced before sorting.
    ///
    /// Only the current directory is materialized; the limit stops runaway
    /// directories before a large vector can be built.
    fn read_dir(&mut self, dir: &Path) -> Result<Vec<fs::DirEntry>, TypeAssetError> {
        let mut collected = Vec::new();
        for entry in fs::read_dir(dir).map_err(|source| io_error(dir, source))? {
            self.entries = self.entries.saturating_add(1);
            if self.entries > MAX_ASSET_ENTRIES {
                return Err(TypeAssetError::TooManyEntries {
                    count: self.entries,
                    max: MAX_ASSET_ENTRIES,
                });
            }
            collected.push(entry.map_err(|source| io_error(dir, source))?);
        }
        collected.sort_by_key(fs::DirEntry::file_name);
        Ok(collected)
    }

    /// Registers one visited path, charging its bytes and rejecting case
    /// collisions.
    fn insert_path(&mut self, path: &str) -> Result<(), TypeAssetError> {
        self.budget
            .reserve(u64::try_from(path.len()).unwrap_or(u64::MAX))?;
        self.seen.insert(path)
    }

    /// Reads one declaration and adds it to the mounted set.
    fn read_asset(
        &mut self,
        path: &Path,
        virtual_path: &str,
        metadata: &Metadata,
    ) -> Result<(), TypeAssetError> {
        if self.files.len() >= MAX_TYPE_ASSET_FILES {
            return Err(TypeAssetError::TooManyFiles {
                count: self.files.len().saturating_add(1),
                max: MAX_TYPE_ASSET_FILES,
            });
        }
        #[cfg(unix)]
        if metadata.nlink() != 1 {
            return Err(TypeAssetError::HardLink {
                path: virtual_path.to_owned(),
            });
        }
        let declared = metadata.len();
        self.budget.reserve(declared)?;

        let file = open_asset(path, virtual_path)?;
        let opened = file.metadata().map_err(|source| io_error(path, source))?;
        if !opened.is_file() {
            return Err(TypeAssetError::NonRegularFile {
                path: virtual_path.to_owned(),
            });
        }
        if opened.len() != declared {
            return Err(TypeAssetError::AssetChanged {
                path: virtual_path.to_owned(),
            });
        }
        #[cfg(unix)]
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() || opened.nlink() != 1 {
            return Err(TypeAssetError::AssetChanged {
                path: virtual_path.to_owned(),
            });
        }

        let mut bytes = Vec::with_capacity(usize::try_from(declared).unwrap_or(0));
        file.take(declared.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|source| io_error(path, source))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != declared {
            return Err(TypeAssetError::AssetChanged {
                path: virtual_path.to_owned(),
            });
        }
        let text = String::from_utf8(bytes).map_err(|_| TypeAssetError::NonUtf8Content {
            path: virtual_path.to_owned(),
        })?;
        let digest = hex_lower(&Sha256::digest(text.as_bytes()));
        self.files.push(TypeAssetFile::new(
            AssetPath::new(virtual_path)?,
            text,
            digest,
            declared,
        )?);
        Ok(())
    }
}

/// Opens one declaration without following a symlink or blocking on a FIFO.
///
/// On Unix, `O_NOFOLLOW` rejects a link swapped in after the pre-open
/// `symlink_metadata` check, and `O_NONBLOCK` makes a FIFO return immediately so
/// the post-open `fstat` can reject it instead of hanging. Other Unix targets
/// without inlined flag constants fall back to the pre/post identity checks.
#[cfg(unix)]
fn open_asset(path: &Path, virtual_path: &str) -> Result<File, TypeAssetError> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.read(true).custom_flags(O_NOFOLLOW | O_NONBLOCK);
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(source) => Err(classify_open_error(path, virtual_path, source)),
    }
}

/// Non-Unix fallback: no `O_NOFOLLOW`/`O_NONBLOCK`, so only the size, read
/// length and digest are verified. Liveness/containment hardening is not
/// available on these targets.
#[cfg(not(unix))]
fn open_asset(path: &Path, virtual_path: &str) -> Result<File, TypeAssetError> {
    File::open(path).map_err(|source| classify_open_error(path, virtual_path, source))
}

fn classify_open_error(path: &Path, virtual_path: &str, source: std::io::Error) -> TypeAssetError {
    if source.kind() == ErrorKind::NotFound {
        return TypeAssetError::AssetChanged {
            path: virtual_path.to_owned(),
        };
    }
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return TypeAssetError::Symlink {
            path: virtual_path.to_owned(),
        };
    }
    io_error(path, source)
}

/// Combined declaration-text and virtual-path budget.
#[derive(Debug, Default)]
struct AssetBudget {
    used: u64,
}

impl AssetBudget {
    fn reserve(&mut self, bytes: u64) -> Result<(), TypeAssetError> {
        let next = self.used.saturating_add(bytes);
        if next > u64::try_from(MAX_TYPE_ASSET_BYTES).unwrap_or(u64::MAX) {
            return Err(TypeAssetError::TooLarge {
                bytes: next,
                max: MAX_TYPE_ASSET_BYTES,
            });
        }
        self.used = next;
        Ok(())
    }
}

/// Case-insensitive path registry for one complete load.
#[derive(Debug, Default)]
struct SeenPaths {
    paths: BTreeMap<String, String>,
}

impl SeenPaths {
    fn insert(&mut self, path: &str) -> Result<(), TypeAssetError> {
        let key = path.to_lowercase();
        if let Some(first) = self.paths.get(&key) {
            return Err(TypeAssetError::CaseCollision {
                first: first.clone(),
                second: path.to_owned(),
            });
        }
        self.paths.insert(key, path.to_owned());
        Ok(())
    }
}

fn entry_name(dir: &Path, entry: &fs::DirEntry) -> Result<String, TypeAssetError> {
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return Err(TypeAssetError::NonUtf8Path {
            path: dir.join(&name).to_string_lossy().into_owned(),
        });
    };
    validate_component(dir, name)?;
    Ok(name.to_owned())
}

fn validate_component(dir: &Path, name: &str) -> Result<(), TypeAssetError> {
    let reject = |reason| {
        Err(TypeAssetError::UnsafeName {
            path: dir.join(name).to_string_lossy().into_owned(),
            reason,
        })
    };
    if name.is_empty() {
        return reject("empty name");
    }
    if name == "." || name == ".." {
        return reject("dot component");
    }
    if name.contains('/') || name.contains('\\') {
        return reject("separator in name");
    }
    if name.chars().any(char::is_control) {
        return reject("control character");
    }
    if name.len() > MAX_ASSET_COMPONENT_BYTES {
        return reject("name too long");
    }
    Ok(())
}

fn symlink_metadata(path: &Path, virtual_path: &str) -> Result<Metadata, TypeAssetError> {
    fs::symlink_metadata(path).map_err(|source| match source.kind() {
        ErrorKind::NotFound => TypeAssetError::AssetChanged {
            path: virtual_path.to_owned(),
        },
        _ => io_error(path, source),
    })
}

fn is_declaration(name: &str) -> bool {
    name.ends_with(".d.ts")
}

fn io_error(path: &Path, source: std::io::Error) -> TypeAssetError {
    TypeAssetError::AssetIo {
        path: path.to_string_lossy().into_owned(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(tag: &str) -> (PathBuf, Cleanup) {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join("opencode/wxml-tsrs-asset-loader-unit");
        fs::create_dir_all(&base).expect("unit scratch base");
        let path = base.join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("unit scratch");
        (path.clone(), Cleanup(path))
    }

    #[test]
    fn declaration_filter_is_case_sensitive() {
        for name in ["lib.d.ts", "lib.es5.d.ts", "index.d.ts"] {
            assert!(is_declaration(name), "{name} is a declaration");
        }
        for name in [
            "lib.d.ts.map",
            "tsc",
            "README.md",
            "LIB.D.TS",
            "lib.d.ts.bak",
        ] {
            assert!(!is_declaration(name), "{name} is not a declaration");
        }
    }

    #[test]
    fn component_validation_rejects_unsafe_names() {
        let dir = Path::new("/pkg");
        for name in ["", ".", "..", "a/b", "a\\b", "a\u{0}b", "a\nb"] {
            assert!(
                matches!(
                    validate_component(dir, name),
                    Err(TypeAssetError::UnsafeName { .. })
                ),
                "{name:?} must be rejected"
            );
        }
        let long = "a".repeat(MAX_ASSET_COMPONENT_BYTES + 1);
        assert!(matches!(
            validate_component(dir, &long),
            Err(TypeAssetError::UnsafeName { .. })
        ));
        for name in ["lib.d.ts", "types", "x-y_1.d.ts"] {
            assert!(validate_component(dir, name).is_ok(), "{name} is safe");
        }
    }

    #[test]
    fn case_insensitive_registry_rejects_collisions() {
        let mut seen = SeenPaths::default();
        seen.insert("/pkg/lib.d.ts").expect("first path");
        assert!(matches!(
            seen.insert("/pkg/LIB.D.TS"),
            Err(TypeAssetError::CaseCollision { .. })
        ));
        assert!(matches!(
            seen.insert("/pkg/lib.d.ts"),
            Err(TypeAssetError::CaseCollision { .. })
        ));
        seen.insert("/pkg/types/index.d.ts").expect("distinct path");
    }

    #[test]
    fn text_and_path_bytes_share_the_budget() {
        let mut budget = AssetBudget::default();
        budget.reserve(1).expect("small");
        budget.reserve(2).expect("still small");
        assert_eq!(budget.used, 3);
        budget
            .reserve(u64::try_from(MAX_TYPE_ASSET_BYTES).unwrap() - 3)
            .expect("exact budget");
        assert!(matches!(
            budget.reserve(1),
            Err(TypeAssetError::TooLarge { .. })
        ));

        let mut walk = Walk::default();
        walk.budget
            .reserve(u64::try_from(MAX_TYPE_ASSET_BYTES).unwrap() - 1)
            .expect("almost full");
        assert!(matches!(
            walk.insert_path("/pkg/lib.d.ts"),
            Err(TypeAssetError::TooLarge { .. })
        ));
    }

    #[test]
    fn entry_budget_is_checked_while_scanning() {
        let (dir, _cleanup) = scratch("entries");
        fs::write(dir.join(".hidden"), b"x").expect("hidden file");
        let mut walk = Walk {
            entries: MAX_ASSET_ENTRIES,
            ..Walk::default()
        };
        let error = walk
            .read_dir(&dir)
            .expect_err("an entry beyond the limit must fail closed");
        assert!(
            matches!(error, TypeAssetError::TooManyEntries { .. }),
            "expected TooManyEntries, got {error}"
        );
    }

    #[test]
    fn open_rejects_non_utf8_roots() {
        use std::os::unix::ffi::OsStringExt;

        let (dir, _cleanup) = scratch("non-utf8-root");
        let path = dir.join(std::ffi::OsString::from_vec(b"root\xff".to_vec()));
        fs::create_dir_all(&path).expect("directory");
        let error = Root::open(&path).expect_err("a non-UTF-8 root must fail closed");
        assert!(
            matches!(error, TypeAssetError::NonUtf8Path { .. }),
            "expected NonUtf8Path, got {error}"
        );
    }

    #[test]
    fn read_asset_reports_a_missing_file_as_a_change() {
        let (dir, _cleanup) = scratch("missing");
        let present = dir.join("present.d.ts");
        fs::write(&present, b"abc").expect("present file");
        let metadata = fs::symlink_metadata(&present).expect("metadata");
        let error = Walk::default()
            .read_asset(&dir.join("absent.d.ts"), "/pkg/absent.d.ts", &metadata)
            .expect_err("a missing file must fail closed");
        assert!(
            matches!(error, TypeAssetError::AssetChanged { .. }),
            "expected AssetChanged, got {error}"
        );
    }

    #[test]
    fn read_asset_detects_a_length_change() {
        let (dir, _cleanup) = scratch("length");
        let path = dir.join("lib.d.ts");
        fs::write(&path, b"abc").expect("declaration");
        let metadata = fs::symlink_metadata(&path).expect("metadata");
        fs::write(&path, b"abcdef").expect("grown declaration");
        let error = Walk::default()
            .read_asset(&path, "/pkg/lib.d.ts", &metadata)
            .expect_err("a grown file must fail closed");
        assert!(
            matches!(error, TypeAssetError::AssetChanged { .. }),
            "expected AssetChanged, got {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_asset_detects_an_identity_change() {
        let (dir, _cleanup) = scratch("identity");
        let first = dir.join("first.d.ts");
        let second = dir.join("second.d.ts");
        fs::write(&first, b"abc").expect("first declaration");
        fs::write(&second, b"xyz").expect("second declaration");
        let metadata = fs::symlink_metadata(&first).expect("metadata");
        let error = Walk::default()
            .read_asset(&second, "/pkg/second.d.ts", &metadata)
            .expect_err("a replaced file must fail closed");
        assert!(
            matches!(error, TypeAssetError::AssetChanged { .. }),
            "expected AssetChanged, got {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_a_symlink_swapped_after_lstat() {
        let (dir, _cleanup) = scratch("symlink-race");
        let target = dir.join("target.d.ts");
        let link = dir.join("lib.d.ts");
        fs::write(&target, b"abc").expect("target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let error = open_asset(&link, "/pkg/lib.d.ts")
            .expect_err("O_NOFOLLOW must reject a swapped-in symlink");
        assert!(
            matches!(error, TypeAssetError::Symlink { .. }),
            "expected Symlink, got {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_asset_rejects_a_fifo_without_blocking() {
        let (dir, _cleanup) = scratch("fifo");
        let fifo = dir.join("lib.fifo.d.ts");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed: {status}");
        let metadata = fs::symlink_metadata(&fifo).expect("metadata");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = Walk::default().read_asset(&fifo, "/pkg/lib.fifo.d.ts", &metadata);
            let _ = sender.send(result);
        });
        match receiver.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => panic!("a FIFO must be rejected"),
            Ok(Err(error)) => assert!(
                matches!(error, TypeAssetError::NonRegularFile { .. }),
                "expected NonRegularFile, got {error}"
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("FIFO open blocked; O_NONBLOCK is missing");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("FIFO reader thread exited without a result");
            }
        }
    }

    /// Writes the pinned 20-file typings closure and returns its typical pin.
    fn write_typings_package(root: &Path) -> TypeAssetsPin {
        let package = root.join(MINIPROGRAM_TYPINGS);
        fs::create_dir_all(package.join("types")).expect("typings package");
        let mut entries = Vec::new();
        for index in 0..20_u32 {
            let relative = match index {
                0 => "index.d.ts".to_owned(),
                1 => "types/index.d.ts".to_owned(),
                _ => format!("types/fake{index}.d.ts"),
            };
            let text = format!("// fake typings {index}\n");
            fs::write(package.join(&relative), text.as_bytes()).expect("typings file");
            entries.push((relative, hex_lower(&Sha256::digest(text.as_bytes())), text));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut hasher = Sha256::new();
        for (relative, digest, _) in &entries {
            hasher.update(digest.as_bytes());
            hasher.update(b"  ");
            hasher.update(relative.as_bytes());
            hasher.update(b"\n");
        }
        let bundle = hex_lower(&hasher.finalize());
        let bytes = entries.iter().map(|(_, _, text)| text.len() as u64).sum();
        TypeAssetsPin::typings_only("5.3.1", 20, bytes, bundle).expect("typings pin")
    }

    #[test]
    fn typings_only_load_needs_no_engine_directory() {
        let (dir, _cleanup) = scratch("typings-only");
        let typings_root = dir.join("typings");
        let pin = write_typings_package(&typings_root);
        assert!(!dir.join("engine").exists(), "no engine directory exists");

        let assets =
            TypeAssets::load_trusted_typings(&typings_root, &pin).expect("typings-only assets");
        assert!(assets.is_typings_only());
        assert_eq!(assets.file_count(), 20);
        assert_eq!(assets.pin().standard_lib_file_count(), 0);
        assert_eq!(assets.pin().standard_lib_bytes(), 0);
        assert_eq!(
            assets.standard_lib_root().as_str(),
            crate::input::BUNDLED_STANDARD_LIBRARY_ROOT
        );
        assert!(
            assets
                .files()
                .iter()
                .all(|file| { file.path().as_str().contains("/miniprogram-api-typings/") })
        );
    }

    #[test]
    fn typings_only_load_reports_a_missing_typings_root() {
        let (dir, _cleanup) = scratch("typings-missing-root");
        let pin = TypeAssetsPin::pinned_typings_only();
        let error = TypeAssets::load_trusted_typings(&dir.join("absent"), &pin)
            .expect_err("a missing typings root must fail closed");
        assert!(
            matches!(error, TypeAssetError::MissingRoot { .. }),
            "expected MissingRoot, got {error}"
        );
    }

    #[test]
    fn typings_only_load_requires_the_typings_package() {
        let (dir, _cleanup) = scratch("typings-missing-package");
        let pin = TypeAssetsPin::pinned_typings_only();
        let error = TypeAssets::load_trusted_typings(&dir, &pin)
            .expect_err("a missing typings package must fail closed");
        assert!(
            matches!(error, TypeAssetError::MissingRoot { .. }),
            "expected MissingRoot, got {error}"
        );
    }
}
