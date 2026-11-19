//! Trusted on-disk loader for the pinned native type-asset set.
//!
//! [`TypeAssets::load_trusted`] walks two caller-supplied package roots, reads
//! every TypeScript declaration once and returns a [`TypeAssets`] value that is
//! validated against the mandatory [`TypeAssetsPin`]. The standard-library root
//! is the engine's `lib` directory (flat); the typings root is the directory
//! that contains the `miniprogram-api-typings` package. Only files below those
//! roots are read.
//!
//! # Security model
//!
//! This loader never canonicalizes or resolves links and never trusts a name
//! from disk:
//!
//! - both roots must be absolute, normalized paths and must exist as real
//!   directories (a root that is a symlink is rejected);
//! - every visited entry is inspected with `symlink_metadata`; symlinks,
//!   non-regular files, hard-linked files and names that are not valid UTF-8 or
//!   not portable path components are rejected;
//! - after opening, the file's identity (`dev`/`ino`) and length are compared
//!   with the pre-open metadata, reads are capped at the declared length and
//!   any change fails closed;
//! - names are compared case-insensitively so a case-insensitive file system
//!   cannot merge two pinned declarations;
//! - the file-count and text-byte budgets are enforced while walking, before
//!   the whole tree is read.
//!
//! Non-declaration files that ship in the package (`tsc`, `package.json`,
//! licenses, READMEs) are ignored: they are never mounted into the virtual
//! filesystem, so they cannot influence the type environment. Every `.d.ts`
//! file is part of the mounted set, so an extra declaration changes the count
//! or bundle digest and is rejected by [`TypeAssets::new`].
//!
//! # Boundary
//!
//! This is **not** install verification. The caller must still establish that
//! the supplied roots belong to the verified delivery package and must verify
//! the delivery manifest; reconciling the packaged typings payload with the
//! runtime 20-file closure remains **BLOCKED** under P6-3-4/F3-03. The loader
//! only proves that the files under the supplied roots match the supplied pin.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
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

impl TypeAssets {
    /// Loads and verifies a type-asset set from two trusted on-disk roots.
    ///
    /// `engine_root` is the engine `lib` directory that contains the pinned
    /// `lib*.d.ts` files. `typings_root` is the directory that contains the
    /// `miniprogram-api-typings` package; the package directory is the only
    /// subtree walked for the second root. Both roots must be absolute,
    /// normalized paths. The pin is mandatory and every recomputed count, byte
    /// total and bundle digest must match it.
    ///
    /// Limits: at most [`MAX_TYPE_ASSET_FILES`] declaration files and
    /// [`MAX_TYPE_ASSET_BYTES`] declaration bytes, in addition to the exact
    /// per-root file counts and byte totals in `pin`. With
    /// [`TypeAssetsPin::pinned_runtime`] that means 116 standard-library files
    /// (3,870,196 B) and 20 runtime typings files (2,522,752 B) plus their
    /// bundle digests.
    ///
    /// # Errors
    ///
    /// Fails closed with a [`TypeAssetError`] for a missing or unsafe root, a
    /// symlink, hard link, non-regular file, non-UTF-8 or non-portable name,
    /// case collision, over-budget tree, non-UTF-8 declaration, a file that
    /// changes while being read, a missing required declaration (`lib.d.ts`,
    /// `index.d.ts`, `types/index.d.ts`) or any pin mismatch. Callers must not
    /// start a native session after an error.
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

    let mut files = Vec::new();
    let mut budget = TextBudget::default();
    let mut seen = SeenPaths::default();
    engine.collect_flat(&mut files, &mut budget, &mut seen)?;
    package.collect_tree("", 0, &mut files, &mut budget, &mut seen)?;

    TypeAssets::new(
        engine.virtual_root,
        typings.virtual_root,
        files,
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
    fn collect_flat(
        &self,
        files: &mut Vec<TypeAssetFile>,
        budget: &mut TextBudget,
        seen: &mut SeenPaths,
    ) -> Result<(), TypeAssetError> {
        for entry in read_dir_sorted(&self.disk)? {
            let name = entry_name(&self.disk, &entry)?;
            let virtual_path = self.virtual_path(&name);
            seen.insert(&virtual_path)?;
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
            let file = read_asset(&path, &virtual_path, &metadata, files.len(), budget)?;
            files.push(file);
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
        files: &mut Vec<TypeAssetFile>,
        budget: &mut TextBudget,
        seen: &mut SeenPaths,
    ) -> Result<(), TypeAssetError> {
        if depth > MAX_ASSET_PATH_COMPONENTS {
            return Err(TypeAssetError::NestingTooDeep {
                path: self.virtual_root.as_str().to_owned(),
            });
        }
        for entry in read_dir_sorted(&self.disk)? {
            let name = entry_name(&self.disk, &entry)?;
            let relative = relative_path(virtual_dir, &name);
            let virtual_path = self.virtual_path(&relative);
            seen.insert(&virtual_path)?;
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
                child.collect_tree(&relative, depth + 1, files, budget, seen)?;
            } else if !file_type.is_file() {
                return Err(TypeAssetError::NonRegularFile { path: virtual_path });
            } else if is_declaration(&name) {
                let file = read_asset(&path, &virtual_path, &metadata, files.len(), budget)?;
                files.push(file);
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

/// Remaining declaration-text budget.
#[derive(Debug, Default)]
struct TextBudget {
    used: u64,
}

impl TextBudget {
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

fn read_dir_sorted(dir: &Path) -> Result<Vec<fs::DirEntry>, TypeAssetError> {
    let entries = fs::read_dir(dir).map_err(|source| io_error(dir, source))?;
    let mut collected = Vec::new();
    for entry in entries {
        collected.push(entry.map_err(|source| io_error(dir, source))?);
    }
    collected.sort_by_key(fs::DirEntry::file_name);
    Ok(collected)
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

fn read_asset(
    path: &Path,
    virtual_path: &str,
    metadata: &Metadata,
    file_count: usize,
    budget: &mut TextBudget,
) -> Result<TypeAssetFile, TypeAssetError> {
    if file_count >= MAX_TYPE_ASSET_FILES {
        return Err(TypeAssetError::TooManyFiles {
            count: file_count.saturating_add(1),
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
    budget.reserve(declared)?;

    let file = File::open(path).map_err(|source| io_error(path, source))?;
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
    TypeAssetFile::new(AssetPath::new(virtual_path)?, text, digest, declared)
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
    fn text_budget_is_cumulative_and_bounded() {
        let mut budget = TextBudget::default();
        budget.reserve(1).expect("small");
        budget.reserve(2).expect("still small");
        assert_eq!(budget.used, 3);
        assert!(matches!(
            budget.reserve(u64::try_from(MAX_TYPE_ASSET_BYTES).unwrap()),
            Err(TypeAssetError::TooLarge { .. })
        ));
    }

    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(tag: &str) -> (PathBuf, Cleanup) {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join("opencode/wxml-tsgo-asset-loader-unit");
        fs::create_dir_all(&base).expect("unit scratch base");
        let path = base.join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("unit scratch");
        (path.clone(), Cleanup(path))
    }

    #[cfg(unix)]
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
    fn read_asset_reports_a_missing_file_as_an_io_error() {
        let (dir, _cleanup) = scratch("missing");
        let present = dir.join("present.d.ts");
        fs::write(&present, b"abc").expect("present file");
        let metadata = fs::symlink_metadata(&present).expect("metadata");
        let error = read_asset(
            &dir.join("absent.d.ts"),
            "/pkg/absent.d.ts",
            &metadata,
            0,
            &mut TextBudget::default(),
        )
        .expect_err("a missing file must fail closed");
        assert!(
            matches!(error, TypeAssetError::AssetIo { .. }),
            "expected AssetIo, got {error}"
        );
    }

    #[test]
    fn read_asset_detects_a_length_change() {
        let (dir, _cleanup) = scratch("length");
        let path = dir.join("lib.d.ts");
        fs::write(&path, b"abc").expect("declaration");
        let metadata = fs::symlink_metadata(&path).expect("metadata");
        fs::write(&path, b"abcdef").expect("grown declaration");
        let error = read_asset(
            &path,
            "/pkg/lib.d.ts",
            &metadata,
            0,
            &mut TextBudget::default(),
        )
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
        let error = read_asset(
            &second,
            "/pkg/second.d.ts",
            &metadata,
            0,
            &mut TextBudget::default(),
        )
        .expect_err("a replaced file must fail closed");
        assert!(
            matches!(error, TypeAssetError::AssetChanged { .. }),
            "expected AssetChanged, got {error}"
        );
    }
}
