//! Immutable analysis input, revisions and the host provider seam.
//!
//! `P3-I` owns these DTOs; `wxml-lsp` will implement [`InputProvider`] over its
//! project host in a later package. Nothing here reads the file system, writes a
//! business file or mutates host state: [`AnalysisInput`] is a bounded,
//! immutable snapshot of text that has already been read by the host.
//!
//! # Path rules
//!
//! A [`ProjectRoot`] is a normalized absolute Unix path. A [`SourceFile`] path
//! is project-relative, `/`-separated and contains no empty, `.` or `..`
//! components. Both reject NUL/control characters and backslashes. Duplicate
//! file paths are rejected instead of being silently merged. Paths are stored
//! verbatim after validation; this module never canonicalizes or touches the
//! file system.
//!
//! # Language and text
//!
//! [`Language`] is derived from the extension only. The source text is kept
//! byte-for-byte, including a BOM, CRLF and a hashbang line; the shadow builder
//! is responsible for handling those bytes, not this snapshot.
//!
//! # Type environment
//!
//! [`AnalysisInput::with_environment`] carries the validated project
//! [`CompilerOptions`] and an [`Arc`] of [`TypeAssets`]. [`TypeAssets`] is a
//! pure in-memory value validated against a mandatory [`TypeAssetsPin`]: it checks
//! roots, required declarations, per-file SHA-256/size, version and the
//! 256-file/8-MiB budget. The legacy [`AnalysisInput::new`] constructor is
//! reserved for probes and old tests and carries no assets.
//!
//! [`TypeAssets::load_pinned`] is
//! the production disk entry point: it walks the caller-supplied engine and
//! typings roots, rejects unsafe entries, hashes every declaration and runs the
//! same pin validation as [`TypeAssets::new`] against the built-in
//! [`TypeAssetsPin::pinned_runtime`]. The caller-pinned
//! [`TypeAssets::load_trusted`]
//! is for tests and independently verified manifests. Neither verifies the
//! install or the delivery manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Maximum number of files accepted in one [`AnalysisInput`].
pub const MAX_INPUT_FILES: usize = 20_000;

/// Maximum combined UTF-8 source bytes accepted in one [`AnalysisInput`].
///
/// The frozen native request budget is 32 MiB. JSON escaping can expand a
/// control character to six bytes, so an accepted 48 MiB input cannot always
/// be sent in one snapshot request; `NativeSession` preflights a conservative
/// serialized-size bound and returns a typed `InputTooLarge` error before
/// allocating the body.
pub const MAX_INPUT_BYTES: usize = 48 * 1024 * 1024;

/// Compiler option keys forwarded by the Node production path.
///
/// This mirrors `SUPPORTED_COMPILER_OPTIONS` in `wxml-lsp::project_host` and
/// `REQUEST_COMPILER_OPTION_KEYS` in `backends/typescript/src/protocol.ts`.
/// Unknown keys are rejected instead of being silently dropped; the adapter
/// records its own `Partial` degradation before it reaches this crate.
pub const COMPILER_OPTION_KEYS: [&str; 33] = [
    "strict",
    "strictNullChecks",
    "strictFunctionTypes",
    "strictBindCallApply",
    "strictPropertyInitialization",
    "noImplicitAny",
    "noImplicitThis",
    "noUncheckedIndexedAccess",
    "exactOptionalPropertyTypes",
    "strictBuiltinIteratorReturn",
    "useUnknownInCatchVariables",
    "allowJs",
    "checkJs",
    "esModuleInterop",
    "allowSyntheticDefaultImports",
    "resolveJsonModule",
    "isolatedModules",
    "verbatimModuleSyntax",
    "forceConsistentCasingInFileNames",
    "skipLibCheck",
    "target",
    "module",
    "moduleResolution",
    "jsx",
    "moduleDetection",
    "lib",
    "baseUrl",
    "paths",
    "types",
    "typeRoots",
    "rootDir",
    "experimentalDecorators",
    "outDir",
];

/// Typings package that must always be present in a native type environment.
pub const MINIPROGRAM_TYPINGS: &str = "miniprogram-api-typings";

/// Maximum number of type-asset files accepted in one [`TypeAssets`].
///
/// The pinned package currently has 136 `.d.ts` files (116 standard-library
/// files plus 20 typings files); this cap leaves a bounded safety margin.
pub const MAX_TYPE_ASSET_FILES: usize = 256;

/// Maximum combined UTF-8 bytes accepted in one [`TypeAssets`].
///
/// The budget covers declaration text **and** virtual-path bytes so a path
/// table cannot grow without bound before the request preflight runs.
pub const MAX_TYPE_ASSET_BYTES: usize = 8 * 1024 * 1024;

/// Maximum bytes in one [`AssetPath`] (delivery path limit).
pub const MAX_ASSET_PATH_BYTES: usize = 4096;

/// Maximum `/`-separated components in one [`AssetPath`].
pub const MAX_ASSET_PATH_COMPONENTS: usize = 32;

/// Maximum bytes in one [`AssetPath`] component (delivery path limit).
pub const MAX_ASSET_COMPONENT_BYTES: usize = 255;

/// Exact typings package version pinned by the native type environment.
pub const PINNED_TYPINGS_VERSION: &str = "5.3.1";

/// Number of runtime typings `.d.ts` files in the pinned 5.3.1 package.
pub const PINNED_TYPINGS_FILE_COUNT: u32 = 20;

/// Combined runtime typings `.d.ts` bytes in the pinned 5.3.1 package.
pub const PINNED_TYPINGS_BYTES: u64 = 2_522_752;

/// Number of pinned 7.1.0-beta `lib*.d.ts` files.
pub const PINNED_STANDARD_LIB_FILE_COUNT: u32 = 116;

/// Combined pinned 7.1.0-beta `lib*.d.ts` bytes.
pub const PINNED_STANDARD_LIB_BYTES: u64 = 3_870_196;

/// Total mounted `.d.ts` files for the pinned fixture.
pub const PINNED_TYPE_ASSET_FILES: u32 = PINNED_STANDARD_LIB_FILE_COUNT + PINNED_TYPINGS_FILE_COUNT;

/// Total mounted `.d.ts` bytes for the pinned fixture.
pub const PINNED_TYPE_ASSET_BYTES: u64 = PINNED_STANDARD_LIB_BYTES + PINNED_TYPINGS_BYTES;

/// Bundle digest of the pinned `lib*.d.ts` set.
///
/// Algorithm: files are sorted by path relative to the standard-library root
/// and hashed as `"{sha256}  {relative_path}\n"` concatenated in that order.
/// This reproduces the manifest digest recorded for the pinned 7.1.0-beta
/// fixture; the fixture itself is not redistributed with this crate.
pub const PINNED_STANDARD_LIB_BUNDLE_DIGEST: &str =
    "5e126e7206065b6fff9d651eff3ad66b9413804d3e23d7ecfb4257299af3e85e";

/// Bundle digest of the 20 runtime typings `.d.ts` files.
///
/// Derived from the local pinned fixture
/// `backends/typescript/node_modules/miniprogram-api-typings` (5.3.1) using the
/// same manifest algorithm, relative to the typings package root. This pin
/// covers only the runtime declarations mounted into the VFS. The delivery
/// manifest's 26-file package digest and 23-file allowlist conflict is **not**
/// resolved here (F3-03/P6-3-4 blocked), so this value is not a packaged-chain
/// claim.
pub const PINNED_TYPINGS_BUNDLE_DIGEST: &str =
    "821c93bfdf41324faa7ee44011a963993cfecc3b4ea9e41cd834e894755e9ba9";

/// Maximum bytes in one compiler-option string value or `paths` key.
const MAX_OPTION_STRING_BYTES: usize = 4096;

/// Maximum entries in one compiler-option string array or `paths` object.
const MAX_OPTION_ARRAY_ENTRIES: usize = 256;

/// Maximum aggregate compiler-option payload (keys, strings and container
/// entries).
const MAX_OPTION_TOTAL_BYTES: usize = 256 * 1024;

/// Compiler options accepted by the native type environment.
///
/// Only [`COMPILER_OPTION_KEYS`] are accepted and every value shape is
/// validated. Values are stored in a [`BTreeMap`] so serialization and the
/// digest are stable regardless of the host's insertion order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerOptions {
    values: BTreeMap<String, Value>,
    digest: String,
}

impl CompilerOptions {
    /// Validates and stores project compiler options.
    ///
    /// # Errors
    ///
    /// Returns [`InputError::UnsupportedCompilerOption`] for a key outside
    /// [`COMPILER_OPTION_KEYS`] and [`InputError::InvalidCompilerOption`] when
    /// the value does not have the documented shape.
    pub fn new(values: BTreeMap<String, Value>) -> Result<Self, InputError> {
        if options_payload_size(&values) > MAX_OPTION_TOTAL_BYTES {
            return Err(InputError::InvalidCompilerOption {
                key: "<compilerOptions>".to_owned(),
                reason: "option payload too large",
            });
        }
        for (key, value) in &values {
            validate_compiler_option(key, value)?;
        }
        let digest = options_digest(&values);
        Ok(Self { values, digest })
    }

    /// Returns the validated options in stable key order.
    #[must_use]
    pub const fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }

    /// Returns the lowercase SHA-256 of the canonical option JSON.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Looks one option up by its exact key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }
}

impl Default for CompilerOptions {
    fn default() -> Self {
        let values = BTreeMap::new();
        Self {
            digest: options_digest(&values),
            values,
        }
    }
}

/// Validated absolute path of one type-asset file or fixed asset root.
///
/// The path is `/`-separated, absolute, not `/`, and free of empty, `.`/`..`
/// components, backslashes, NUL and other control characters. No
/// canonicalization or file-system access happens here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetPath(String);

impl AssetPath {
    /// Validates a normalized absolute Unix path.
    ///
    /// # Errors
    ///
    /// Returns [`TypeAssetError::InvalidRoot`] when the path violates the
    /// normalized absolute-path rules.
    pub fn new(path: impl Into<String>) -> Result<Self, TypeAssetError> {
        let path = path.into();
        validate_asset_path(&path).map_err(|reason| TypeAssetError::InvalidRoot {
            path: path.clone(),
            reason,
        })?;
        Ok(Self(path))
    }

    /// Returns the path exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns whether `path` is this root or a descendant on a component
    /// boundary.
    #[must_use]
    pub fn contains(&self, path: &str) -> bool {
        path == self.0
            || path
                .strip_prefix(&self.0)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

impl fmt::Display for AssetPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One verified TypeScript declaration asset.
///
/// The text is shared through an [`Arc`] so a snapshot can mount the same
/// buffer without copying it again. `sha256` and `size` are the manifest
/// values and must match the supplied text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAssetFile {
    path: AssetPath,
    text: Arc<str>,
    sha256: String,
    size: u64,
}

impl TypeAssetFile {
    /// Validates a declaration asset against its manifest identity.
    ///
    /// # Errors
    ///
    /// Rejects non-`.d.ts` paths, malformed SHA-256 digests, a declared size
    /// that differs from the text and a digest that differs from the text.
    pub fn new(
        path: AssetPath,
        text: impl Into<Arc<str>>,
        sha256: impl Into<String>,
        size: u64,
    ) -> Result<Self, TypeAssetError> {
        let text = text.into();
        if !path.as_str().ends_with(".d.ts") {
            return Err(TypeAssetError::UnsupportedFile {
                path: path.as_str().to_owned(),
            });
        }
        let sha256 = sha256.into();
        if !is_sha256(&sha256) {
            return Err(TypeAssetError::InvalidDigest {
                path: path.as_str().to_owned(),
            });
        }
        let actual = u64::try_from(text.len()).unwrap_or(u64::MAX);
        if size != actual {
            return Err(TypeAssetError::SizeMismatch {
                path: path.as_str().to_owned(),
                declared: size,
                actual,
            });
        }
        if hex_lower(&Sha256::digest(text.as_bytes())) != sha256 {
            return Err(TypeAssetError::HashMismatch {
                path: path.as_str().to_owned(),
            });
        }
        Ok(Self {
            path,
            text,
            sha256,
            size,
        })
    }

    /// Returns the validated absolute virtual path.
    #[must_use]
    pub const fn path(&self) -> &AssetPath {
        &self.path
    }

    /// Returns the exact declaration text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the shared declaration text.
    #[must_use]
    pub const fn text_arc(&self) -> &Arc<str> {
        &self.text
    }

    /// Returns the verified lowercase SHA-256.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Returns the text length in bytes.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }
}

/// Caller-supplied trusted anchor for one complete type-asset set.
///
/// The pin fixes the typings version, the exact standard-library/typings file
/// counts and the bundle digests for each fixed root. [`TypeAssets::new`]
/// recomputes every value and rejects a mismatch; there is no empty or default
/// pin and no boolean bypass.
///
/// Only [`Self::pinned_runtime`] is a built-in trusted anchor. [`Self::new`]
/// anchors the version and both file counts to crate constants, but accepts
/// caller-provided byte totals and bundle digests without establishing their
/// provenance; a pin derived from the same tree that is about to be loaded
/// proves nothing. Production wiring should use
/// [`TypeAssets::load_pinned`]
/// or pass a pin verified independently against the delivery manifest.
///
/// The trusted *source* of this pin (manifest/package-root discovery and the
/// 26-vs-23 delivery reconciliation) remains **BLOCKED** under P6-3-4/F3-03.
/// This type only proves that a caller-supplied pin matches the supplied
/// assets. [`Self::pinned_runtime`] records the local pinned fixture:
/// TypeScript 7.1.0-beta `lib*.d.ts` plus
/// `backends/typescript` fixture `miniprogram-api-typings` 5.3.1
/// runtime `.d.ts` closure (20 files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAssetsPin {
    typings_version: String,
    standard_lib_file_count: u32,
    standard_lib_bytes: u64,
    standard_lib_bundle_digest: String,
    typings_file_count: u32,
    typings_bytes: u64,
    typings_bundle_digest: String,
}

impl TypeAssetsPin {
    /// Validates a caller-supplied pin.
    ///
    /// Version and both file counts must equal the pinned constants; digest
    /// values must be lowercase SHA-256. There is deliberately no constructor
    /// that skips these checks.
    ///
    /// This constructor does **not** verify that the byte totals or digests
    /// correspond to a trusted package. Only version and counts are anchored to
    /// crate constants; bytes and digests are caller-asserted. Production
    /// callers should use [`Self::pinned_runtime`] through
    /// [`TypeAssets::load_pinned`],
    /// or verify the values against the delivery manifest first.
    ///
    /// # Errors
    ///
    /// Rejects a version/count mismatch or a malformed digest.
    pub fn new(
        typings_version: impl Into<String>,
        standard_lib_file_count: u32,
        standard_lib_bytes: u64,
        standard_lib_bundle_digest: impl Into<String>,
        typings_file_count: u32,
        typings_bytes: u64,
        typings_bundle_digest: impl Into<String>,
    ) -> Result<Self, TypeAssetError> {
        let typings_version = typings_version.into();
        if typings_version != PINNED_TYPINGS_VERSION {
            return Err(TypeAssetError::InvalidVersion {
                version: typings_version,
            });
        }
        if standard_lib_file_count != PINNED_STANDARD_LIB_FILE_COUNT {
            return Err(TypeAssetError::PinMismatch {
                field: "standardLibFileCount",
                expected: PINNED_STANDARD_LIB_FILE_COUNT.to_string(),
                actual: standard_lib_file_count.to_string(),
            });
        }
        if typings_file_count != PINNED_TYPINGS_FILE_COUNT {
            return Err(TypeAssetError::PinMismatch {
                field: "typingsFileCount",
                expected: PINNED_TYPINGS_FILE_COUNT.to_string(),
                actual: typings_file_count.to_string(),
            });
        }
        let standard_lib_bundle_digest = standard_lib_bundle_digest.into();
        if !is_sha256(&standard_lib_bundle_digest) {
            return Err(TypeAssetError::InvalidPinDigest {
                field: "standardLibBundleDigest",
            });
        }
        let typings_bundle_digest = typings_bundle_digest.into();
        if !is_sha256(&typings_bundle_digest) {
            return Err(TypeAssetError::InvalidPinDigest {
                field: "typingsBundleDigest",
            });
        }
        Ok(Self {
            typings_version,
            standard_lib_file_count,
            standard_lib_bytes,
            standard_lib_bundle_digest,
            typings_file_count,
            typings_bytes,
            typings_bundle_digest,
        })
    }

    /// Returns the built-in production pin for the local pinned fixture.
    ///
    /// This is the only pin whose byte totals and bundle digests are fixed by
    /// this crate. Use it through
    /// [`TypeAssets::load_pinned`]
    /// in production.
    ///
    /// `PINNED_TYPINGS_BUNDLE_DIGEST` is derived from the repository fixture;
    /// it does not claim a packaged-chain digest.
    #[must_use]
    pub fn pinned_runtime() -> Self {
        Self {
            typings_version: PINNED_TYPINGS_VERSION.to_owned(),
            standard_lib_file_count: PINNED_STANDARD_LIB_FILE_COUNT,
            standard_lib_bytes: PINNED_STANDARD_LIB_BYTES,
            standard_lib_bundle_digest: PINNED_STANDARD_LIB_BUNDLE_DIGEST.to_owned(),
            typings_file_count: PINNED_TYPINGS_FILE_COUNT,
            typings_bytes: PINNED_TYPINGS_BYTES,
            typings_bundle_digest: PINNED_TYPINGS_BUNDLE_DIGEST.to_owned(),
        }
    }

    /// Returns the exact typings package version.
    #[must_use]
    pub fn typings_version(&self) -> &str {
        &self.typings_version
    }

    /// Returns the pinned standard-library file count.
    #[must_use]
    pub const fn standard_lib_file_count(&self) -> u32 {
        self.standard_lib_file_count
    }

    /// Returns the pinned standard-library text bytes.
    #[must_use]
    pub const fn standard_lib_bytes(&self) -> u64 {
        self.standard_lib_bytes
    }

    /// Returns the pinned standard-library bundle digest.
    #[must_use]
    pub fn standard_lib_bundle_digest(&self) -> &str {
        &self.standard_lib_bundle_digest
    }

    /// Returns the pinned typings file count.
    #[must_use]
    pub const fn typings_file_count(&self) -> u32 {
        self.typings_file_count
    }

    /// Returns the pinned typings text bytes.
    #[must_use]
    pub const fn typings_bytes(&self) -> u64 {
        self.typings_bytes
    }

    /// Returns the pinned typings bundle digest.
    #[must_use]
    pub fn typings_bundle_digest(&self) -> &str {
        &self.typings_bundle_digest
    }

    /// Returns the pinned total file count.
    #[must_use]
    pub const fn total_file_count(&self) -> u32 {
        self.standard_lib_file_count + self.typings_file_count
    }

    /// Returns the pinned total text bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.standard_lib_bytes + self.typings_bytes
    }
}

/// Verified TypeScript standard library and mini-program typings.
///
/// [`Self::new`] is a pure in-memory value: it performs no disk I/O and only
/// re-validates text that a caller already read. [`Self::load_pinned`] is the
/// production disk loader: it reads the two pinned package roots, rejects unsafe
/// entries, hashes every declaration and validates the built-in production pin.
/// [`Self::load_trusted`] performs the same walk with a caller-supplied pin and
/// is reserved for tests or independently verified manifests. A production host
/// reads and hashes the pinned package once, then shares the value through an
/// [`Arc`] for every snapshot. Construction requires a [`TypeAssetsPin`] and
/// recomputes counts, byte totals and bundle digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAssets {
    standard_lib_root: AssetPath,
    typings_root: AssetPath,
    pin: TypeAssetsPin,
    files: Vec<TypeAssetFile>,
    file_count: u32,
    total_bytes: u64,
    path_bytes: u64,
    accounted_bytes: u64,
}

impl TypeAssets {
    /// Validates one complete type-asset set against a mandatory pin.
    ///
    /// `standard_lib_root` is the engine's `lib` directory and `typings_root`
    /// is the directory that contains the `miniprogram-api-typings` package.
    /// Every file must live under one of those fixed roots; `lib.d.ts`,
    /// `miniprogram-api-typings/index.d.ts` and its referenced
    /// `miniprogram-api-typings/types/index.d.ts` are mandatory.
    ///
    /// # Errors
    ///
    /// Rejects overlapping roots, duplicate or unsafe paths, an oversized set
    /// (text plus path bytes), missing required declarations and any count,
    /// byte or bundle-digest mismatch against `pin`. A failure here is
    /// fail-closed: callers must not spawn or reuse a native session with an
    /// incomplete type environment.
    pub fn new(
        standard_lib_root: AssetPath,
        typings_root: AssetPath,
        mut files: Vec<TypeAssetFile>,
        pin: TypeAssetsPin,
    ) -> Result<Self, TypeAssetError> {
        if standard_lib_root.contains(typings_root.as_str())
            || typings_root.contains(standard_lib_root.as_str())
        {
            return Err(TypeAssetError::RootsOverlap {
                first: standard_lib_root.as_str().to_owned(),
                second: typings_root.as_str().to_owned(),
            });
        }
        if files.len() > MAX_TYPE_ASSET_FILES {
            return Err(TypeAssetError::TooManyFiles {
                count: files.len(),
                max: MAX_TYPE_ASSET_FILES,
            });
        }

        let typings_package_root = typings_package_root(&typings_root);
        let mut total_bytes = 0u64;
        let mut path_bytes = 0u64;
        let mut seen = BTreeSet::new();
        let mut standard_lib = Vec::new();
        let mut typings = Vec::new();
        for file in &files {
            let path = file.path().as_str();
            let in_standard_lib = standard_lib_root.contains(path);
            let in_typings = within_asset_root(&typings_package_root, path);
            if !in_standard_lib && !in_typings {
                return Err(TypeAssetError::InvalidPath {
                    path: path.to_owned(),
                    reason: "outside fixed asset roots",
                });
            }
            if !seen.insert(path) {
                return Err(TypeAssetError::DuplicatePath {
                    path: path.to_owned(),
                });
            }
            total_bytes = total_bytes.saturating_add(file.size());
            path_bytes = path_bytes.saturating_add(u64::try_from(path.len()).unwrap_or(u64::MAX));
            if in_standard_lib {
                standard_lib.push(file);
            } else {
                typings.push(file);
            }
        }
        let accounted_bytes = total_bytes.saturating_add(path_bytes);
        if accounted_bytes > u64::try_from(MAX_TYPE_ASSET_BYTES).unwrap_or(u64::MAX) {
            return Err(TypeAssetError::TooLarge {
                bytes: accounted_bytes,
                max: MAX_TYPE_ASSET_BYTES,
            });
        }

        for required in [
            format!("{}/lib.d.ts", standard_lib_root.as_str()),
            format!("{typings_package_root}/index.d.ts"),
            format!("{typings_package_root}/types/index.d.ts"),
        ] {
            if !seen.contains(required.as_str()) {
                return Err(TypeAssetError::MissingRequired { path: required });
            }
        }

        verify_pin_count(
            "standardLibFileCount",
            u64::from(pin.standard_lib_file_count),
            u64::try_from(standard_lib.len()).unwrap_or(u64::MAX),
        )?;
        verify_pin_count(
            "typingsFileCount",
            u64::from(pin.typings_file_count),
            u64::try_from(typings.len()).unwrap_or(u64::MAX),
        )?;
        let standard_lib_bytes = standard_lib.iter().map(|file| file.size()).sum();
        verify_pin_count(
            "standardLibBytes",
            pin.standard_lib_bytes,
            standard_lib_bytes,
        )?;
        let typings_bytes = typings.iter().map(|file| file.size()).sum();
        verify_pin_count("typingsBytes", pin.typings_bytes, typings_bytes)?;
        verify_pin_digest(
            "standardLibBundleDigest",
            &pin.standard_lib_bundle_digest,
            &bundle_digest(standard_lib_root.as_str(), &mut standard_lib),
        )?;
        verify_pin_digest(
            "typingsBundleDigest",
            &pin.typings_bundle_digest,
            &bundle_digest(&typings_package_root, &mut typings),
        )?;

        files.sort_by(|left, right| left.path().cmp(right.path()));
        Ok(Self {
            standard_lib_root,
            typings_root,
            pin,
            file_count: u32::try_from(files.len()).unwrap_or(u32::MAX),
            total_bytes,
            path_bytes,
            accounted_bytes,
            files,
        })
    }

    /// Returns the fixed engine `lib` root.
    #[must_use]
    pub const fn standard_lib_root(&self) -> &AssetPath {
        &self.standard_lib_root
    }

    /// Returns the fixed directory that contains the typings package.
    #[must_use]
    pub const fn typings_root(&self) -> &AssetPath {
        &self.typings_root
    }

    /// Returns the validated trusted anchor.
    #[must_use]
    pub const fn pin(&self) -> &TypeAssetsPin {
        &self.pin
    }

    /// Returns `<typings_root>/miniprogram-api-typings`.
    #[must_use]
    pub fn typings_package_root(&self) -> String {
        typings_package_root(&self.typings_root)
    }

    /// Returns the typings package version from the pin.
    #[must_use]
    pub fn package_version(&self) -> &str {
        self.pin.typings_version()
    }

    /// Returns the typings runtime bundle digest from the pin.
    ///
    /// This is the 20-file runtime closure, not the delivery manifest's
    /// 26-file package digest.
    #[must_use]
    pub fn package_digest(&self) -> &str {
        self.pin.typings_bundle_digest()
    }

    /// Returns every verified declaration file in path order.
    #[must_use]
    pub fn files(&self) -> &[TypeAssetFile] {
        &self.files
    }

    /// Looks a declaration up by its exact absolute virtual path.
    #[must_use]
    pub fn file(&self, path: &str) -> Option<&TypeAssetFile> {
        self.files.iter().find(|file| file.path().as_str() == path)
    }

    /// Returns the verified file count.
    #[must_use]
    pub const fn file_count(&self) -> u32 {
        self.file_count
    }

    /// Returns the verified combined declaration text size.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Returns the combined virtual-path bytes counted against the budget.
    #[must_use]
    pub const fn path_bytes(&self) -> u64 {
        self.path_bytes
    }

    /// Returns the budgeted token: declaration text plus virtual-path bytes.
    #[must_use]
    pub const fn accounted_bytes(&self) -> u64 {
        self.accounted_bytes
    }
}

/// Stable identity of one configured type environment.
///
/// A native session records this value when it opens a project. Updates must
/// keep exactly the same identity; changing the environment (including the
/// compiler-option digest or any asset root/pin field) requires reopening the
/// session instead of silently reusing handles bound to the old program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeEnvironmentIdentity {
    options_digest: String,
    typings_version: String,
    standard_lib_root: String,
    typings_root: String,
    standard_lib_file_count: u32,
    standard_lib_bytes: u64,
    standard_lib_bundle_digest: String,
    typings_file_count: u32,
    typings_bytes: u64,
    typings_bundle_digest: String,
}

impl TypeEnvironmentIdentity {
    /// Captures the identity of one validated environment.
    #[must_use]
    pub fn new(options: &CompilerOptions, assets: &TypeAssets) -> Self {
        let pin = assets.pin();
        Self {
            options_digest: options.digest().to_owned(),
            typings_version: pin.typings_version().to_owned(),
            standard_lib_root: assets.standard_lib_root().as_str().to_owned(),
            typings_root: assets.typings_root().as_str().to_owned(),
            standard_lib_file_count: pin.standard_lib_file_count(),
            standard_lib_bytes: pin.standard_lib_bytes(),
            standard_lib_bundle_digest: pin.standard_lib_bundle_digest().to_owned(),
            typings_file_count: pin.typings_file_count(),
            typings_bytes: pin.typings_bytes(),
            typings_bundle_digest: pin.typings_bundle_digest().to_owned(),
        }
    }

    /// Returns the compiler-options digest.
    #[must_use]
    pub fn options_digest(&self) -> &str {
        &self.options_digest
    }

    /// Returns the exact typings version.
    #[must_use]
    pub fn typings_version(&self) -> &str {
        &self.typings_version
    }

    /// Returns the fixed standard-library root.
    #[must_use]
    pub fn standard_lib_root(&self) -> &str {
        &self.standard_lib_root
    }

    /// Returns the fixed typings root.
    #[must_use]
    pub fn typings_root(&self) -> &str {
        &self.typings_root
    }

    /// Returns the standard-library file count.
    #[must_use]
    pub const fn standard_lib_file_count(&self) -> u32 {
        self.standard_lib_file_count
    }

    /// Returns the standard-library text bytes.
    #[must_use]
    pub const fn standard_lib_bytes(&self) -> u64 {
        self.standard_lib_bytes
    }

    /// Returns the standard-library bundle digest.
    #[must_use]
    pub fn standard_lib_bundle_digest(&self) -> &str {
        &self.standard_lib_bundle_digest
    }

    /// Returns the typings file count.
    #[must_use]
    pub const fn typings_file_count(&self) -> u32 {
        self.typings_file_count
    }

    /// Returns the typings text bytes.
    #[must_use]
    pub const fn typings_bytes(&self) -> u64 {
        self.typings_bytes
    }

    /// Returns the typings bundle digest.
    #[must_use]
    pub fn typings_bundle_digest(&self) -> &str {
        &self.typings_bundle_digest
    }
}

/// Failure raised while validating a type-asset set.
#[derive(Debug, Error)]
pub enum TypeAssetError {
    /// A fixed asset root is not a normalized absolute path.
    #[error("invalid type-asset root `{path}`: {reason}")]
    InvalidRoot {
        /// Rejected root.
        path: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// A declaration path is not under its fixed root.
    #[error("invalid type-asset path `{path}`: {reason}")]
    InvalidPath {
        /// Rejected path.
        path: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// The file is not a TypeScript declaration.
    #[error("type asset is not a .d.ts file: {path}")]
    UnsupportedFile {
        /// Rejected path.
        path: String,
    },
    /// Two declarations use the same virtual path.
    #[error("duplicate type-asset path: {path}")]
    DuplicatePath {
        /// Duplicated path.
        path: String,
    },
    /// The fixed standard-library and typings roots overlap.
    #[error("type-asset roots overlap: {first} and {second}")]
    RootsOverlap {
        /// First root.
        first: String,
        /// Second root.
        second: String,
    },
    /// The asset count exceeds the budget.
    #[error("type assets have {count} files, limit is {max}")]
    TooManyFiles {
        /// Observed file count.
        count: usize,
        /// Configured limit.
        max: usize,
    },
    /// The combined asset text exceeds the budget.
    #[error("type assets have {bytes} bytes, limit is {max}")]
    TooLarge {
        /// Observed byte count.
        bytes: u64,
        /// Configured limit.
        max: usize,
    },
    /// A declared SHA-256 is not lowercase hexadecimal.
    #[error("type asset has a malformed SHA-256 digest: {path}")]
    InvalidDigest {
        /// Rejected path.
        path: String,
    },
    /// A pin digest is not a lowercase SHA-256.
    #[error("type-asset pin digest is malformed: {field}")]
    InvalidPinDigest {
        /// Rejected pin field.
        field: &'static str,
    },
    /// A recomputed count, byte total or bundle digest differs from the pin.
    #[error("type-asset pin mismatch for {field}: expected {expected}, got {actual}")]
    PinMismatch {
        /// Pin field that failed.
        field: &'static str,
        /// Value from the pin.
        expected: String,
        /// Recomputed value.
        actual: String,
    },
    /// The declared size differs from the supplied text.
    #[error("type asset `{path}` declares {declared} bytes but has {actual}")]
    SizeMismatch {
        /// Rejected path.
        path: String,
        /// Manifest size.
        declared: u64,
        /// Actual text size.
        actual: u64,
    },
    /// The declared SHA-256 differs from the supplied text.
    #[error("type asset hash mismatch: {path}")]
    HashMismatch {
        /// Rejected path.
        path: String,
    },
    /// The typings package version is empty or not version-shaped.
    #[error("invalid typings package version: {version}")]
    InvalidVersion {
        /// Rejected version.
        version: String,
    },
    /// A required declaration is absent from the set.
    #[error("required type asset is missing: {path}")]
    MissingRequired {
        /// Missing path.
        path: String,
    },
    /// A trusted asset root does not exist on disk.
    #[error("type-asset root is missing: {root}")]
    MissingRoot {
        /// Missing root.
        root: String,
    },
    /// A trusted asset root exists but is not a directory.
    #[error("type-asset root is not a directory: {root}")]
    RootNotDirectory {
        /// Rejected root.
        root: String,
    },
    /// A trusted asset tree contains a symbolic link.
    #[error("type-asset entry is a symbolic link: {path}")]
    Symlink {
        /// Rejected entry.
        path: String,
    },
    /// A trusted asset tree contains a file that is not a regular file.
    #[error("type-asset entry is not a regular file: {path}")]
    NonRegularFile {
        /// Rejected entry.
        path: String,
    },
    /// A trusted asset tree contains an unexpected entry.
    #[error("unexpected type-asset entry `{path}`: {reason}")]
    UnexpectedEntry {
        /// Rejected entry.
        path: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// A trusted declaration has more than one hard link.
    #[error("type-asset file has multiple hard links: {path}")]
    HardLink {
        /// Rejected entry.
        path: String,
    },
    /// A file name in a trusted asset tree is not portable or safe.
    #[error("unsafe type-asset entry name `{path}`: {reason}")]
    UnsafeName {
        /// Rejected entry.
        path: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// A path in a trusted asset tree is not valid UTF-8.
    #[error("type-asset path is not valid UTF-8: {path}")]
    NonUtf8Path {
        /// Lossy rendering of the rejected path.
        path: String,
    },
    /// A declaration file is not valid UTF-8 text.
    #[error("type-asset file is not valid UTF-8: {path}")]
    NonUtf8Content {
        /// Rejected entry.
        path: String,
    },
    /// Two trusted entries collide on a case-insensitive file system.
    #[error("type-asset entries collide case-insensitively: {first} and {second}")]
    CaseCollision {
        /// First path in traversal order.
        first: String,
        /// Colliding path.
        second: String,
    },
    /// Reading a trusted asset tree failed.
    #[error("type-asset I/O failed for {path}: {source}")]
    AssetIo {
        /// Rejected entry.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A declaration changed identity or length while it was being read.
    #[error("type-asset file changed while being read: {path}")]
    AssetChanged {
        /// Rejected entry.
        path: String,
    },
    /// A trusted asset tree is nested more deeply than asset paths may be.
    #[error("type-asset directory nesting is too deep: {path}")]
    NestingTooDeep {
        /// Rejected directory.
        path: String,
    },
    /// A trusted asset tree has more directory entries than the walk budget.
    #[error("type-asset tree has {count} directory entries, limit is {max}")]
    TooManyEntries {
        /// Observed entry count.
        count: usize,
        /// Configured limit.
        max: usize,
    },
}

/// Stable key of one discovered mini-program project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectKey(String);

impl ProjectKey {
    /// Creates a project key from a host-provided identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the host identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Normalized absolute root directory of one project.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectRoot(String);

impl ProjectRoot {
    /// Validates a host-supplied absolute Unix path.
    ///
    /// The path must start with `/`, must not be the file-system root itself,
    /// must not end with `/` and must not contain empty, `.` or `..`
    /// components, a backslash, a NUL or any other control character. No
    /// canonicalization or symlink resolution is performed.
    pub fn new(path: impl Into<String>) -> Result<Self, InputError> {
        let path = path.into();
        validate_root(&path)?;
        Ok(Self(path))
    }

    /// Returns the path exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Script language inferred from the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// TypeScript (`.ts`, `.mts`, `.cts`, including `.d.ts`).
    ///
    /// `.tsx` is deliberately unsupported until JSX lexing exists.
    TypeScript,
    /// JavaScript (`.js`, `.mjs`, `.cjs`).
    ///
    /// `.jsx` is deliberately unsupported until JSX lexing exists.
    JavaScript,
}

impl Language {
    /// Infers the language from a project-relative path.
    ///
    /// Extensions are matched case-sensitively. Unknown extensions return
    /// `None`; the caller reports [`InputError::UnsupportedLanguage`]. `.tsx`
    /// and `.jsx` are intentionally rejected: the shadow expression lexer does
    /// not implement JSX and must not silently mislex apostrophes/comments.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = path.rsplit_once('.')?.1;
        match extension {
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "js" | "mjs" | "cjs" => Some(Self::JavaScript),
            _ => None,
        }
    }

    /// Returns whether the language is JavaScript.
    #[must_use]
    pub const fn is_javascript(self) -> bool {
        matches!(self, Self::JavaScript)
    }
}

/// Monotonic version of one open document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRevision(u64);

impl DocumentRevision {
    /// Creates a revision from the host's version counter.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Revision of one complete input snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnapshotRevision(u64);

impl SnapshotRevision {
    /// Creates a snapshot revision from the host sequence.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One immutable file snapshot supplied by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    path: String,
    language: Language,
    text: String,
    revision: DocumentRevision,
}

impl SourceFile {
    /// Creates a validated source snapshot.
    ///
    /// `path` must be non-empty, project-relative, `/`-separated and free of
    /// empty, `.`/`..` components, backslashes and control characters. Its
    /// extension must be a TypeScript or JavaScript extension; see
    /// [`Language::from_path`].
    pub fn new(
        path: impl Into<String>,
        text: impl Into<String>,
        revision: DocumentRevision,
    ) -> Result<Self, InputError> {
        let path = path.into();
        validate_file_path(&path)?;
        let language = Language::from_path(&path)
            .ok_or_else(|| InputError::UnsupportedLanguage { path: path.clone() })?;
        Ok(Self {
            path,
            language,
            text: text.into(),
            revision,
        })
    }

    /// Returns the validated path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the language inferred from [`Self::path`].
    #[must_use]
    pub const fn language(&self) -> Language {
        self.language
    }

    /// Returns the exact source text, including any BOM, CRLF or hashbang.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the document revision the text belongs to.
    #[must_use]
    pub const fn revision(&self) -> DocumentRevision {
        self.revision
    }
}

/// Immutable input for one project revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisInput {
    project: ProjectKey,
    root: ProjectRoot,
    revision: SnapshotRevision,
    files: Vec<SourceFile>,
    compiler_options: CompilerOptions,
    type_assets: Option<Arc<TypeAssets>>,
}

impl AnalysisInput {
    /// Builds a bounded, validated input snapshot without a type environment.
    ///
    /// This is the legacy probe/test constructor. Sessions created from it
    /// keep the historical `noLib:true` configuration and must never be used
    /// for a production type environment; use [`Self::with_environment`] when
    /// standard-library and mini-program typings are required.
    ///
    /// File count, combined size and duplicate paths are rejected here. Path
    /// and language validation happen in [`SourceFile::new`], so passing
    /// already-constructed files cannot bypass them.
    pub fn new(
        project: ProjectKey,
        root: ProjectRoot,
        revision: SnapshotRevision,
        files: Vec<SourceFile>,
    ) -> Result<Self, InputError> {
        Self::build(
            project,
            root,
            revision,
            files,
            CompilerOptions::default(),
            None,
        )
    }

    /// Builds an input with project compiler options and verified type assets.
    ///
    /// The [`TypeAssets`] value has already been validated and is fail-closed:
    /// an incomplete or tampered asset set cannot reach this constructor.
    /// Production adapters must use this constructor; the resulting snapshot
    /// omits `noLib` and mounts every asset into the full virtual filesystem.
    pub fn with_environment(
        project: ProjectKey,
        root: ProjectRoot,
        revision: SnapshotRevision,
        files: Vec<SourceFile>,
        compiler_options: CompilerOptions,
        type_assets: Arc<TypeAssets>,
    ) -> Result<Self, InputError> {
        Self::build(
            project,
            root,
            revision,
            files,
            compiler_options,
            Some(type_assets),
        )
    }

    fn build(
        project: ProjectKey,
        root: ProjectRoot,
        revision: SnapshotRevision,
        files: Vec<SourceFile>,
        compiler_options: CompilerOptions,
        type_assets: Option<Arc<TypeAssets>>,
    ) -> Result<Self, InputError> {
        if files.len() > MAX_INPUT_FILES {
            return Err(InputError::TooManyFiles {
                count: files.len(),
                max: MAX_INPUT_FILES,
            });
        }

        let bytes: usize = files.iter().map(|file| file.text.len()).sum();
        if bytes > MAX_INPUT_BYTES {
            return Err(InputError::TooLarge {
                bytes,
                max: MAX_INPUT_BYTES,
            });
        }

        let mut seen = BTreeSet::new();
        for file in &files {
            if !seen.insert(file.path()) {
                return Err(InputError::DuplicatePath {
                    path: file.path().to_owned(),
                });
            }
        }

        Ok(Self {
            project,
            root,
            revision,
            files,
            compiler_options,
            type_assets,
        })
    }

    /// Returns the project this input belongs to.
    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }

    /// Returns the validated project root.
    #[must_use]
    pub const fn root(&self) -> &ProjectRoot {
        &self.root
    }

    /// Returns the snapshot revision.
    #[must_use]
    pub const fn revision(&self) -> SnapshotRevision {
        self.revision
    }

    /// Returns the file snapshots in host order.
    #[must_use]
    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    /// Looks a file up by its exact project-relative path.
    #[must_use]
    pub fn file(&self, path: &str) -> Option<&SourceFile> {
        self.files.iter().find(|file| file.path() == path)
    }

    /// Returns the validated project compiler options.
    ///
    /// The legacy constructor returns an empty set, which is distinct from
    /// "the project explicitly set no options".
    #[must_use]
    pub const fn compiler_options(&self) -> &CompilerOptions {
        &self.compiler_options
    }

    /// Returns the verified type assets for an environment input.
    ///
    /// `None` means the legacy constructor was used and the snapshot keeps the
    /// historical `noLib:true` probing behavior.
    #[must_use]
    pub fn type_assets(&self) -> Option<&TypeAssets> {
        self.type_assets.as_deref()
    }

    /// Returns whether this input requires a real type environment.
    #[must_use]
    pub const fn has_type_environment(&self) -> bool {
        self.type_assets.is_some()
    }

    /// Returns the stable identity an open session must preserve across
    /// updates.
    ///
    /// `None` is the legacy no-type-environment mode; `Some` includes the
    /// compiler-options digest and every asset root/pin field.
    #[must_use]
    pub fn type_environment_identity(&self) -> Option<TypeEnvironmentIdentity> {
        self.type_assets
            .as_deref()
            .map(|assets| TypeEnvironmentIdentity::new(&self.compiler_options, assets))
    }
}

/// Supplies immutable inputs for a project without owning native state.
///
/// Implementations must be side-effect free with respect to the host while
/// the call is in flight and must not read outside the project boundary.
pub trait InputProvider: Send + Sync {
    /// Loads the current bounded input for `project`.
    fn load(&self, project: &ProjectKey) -> Result<AnalysisInput, InputError>;
}

/// Failures raised while collecting or validating an [`AnalysisInput`].
#[derive(Debug, Error)]
pub enum InputError {
    /// The host has no project with this key.
    #[error("unknown project: {project}")]
    UnknownProject {
        /// Host identifier that could not be resolved.
        project: String,
    },
    /// The file count exceeds the input budget.
    #[error("input has {count} files, limit is {max}")]
    TooManyFiles {
        /// Observed file count.
        count: usize,
        /// Configured limit.
        max: usize,
    },
    /// The combined source size exceeds the input budget.
    #[error("input has {bytes} bytes, limit is {max}")]
    TooLarge {
        /// Observed byte count.
        bytes: usize,
        /// Configured limit.
        max: usize,
    },
    /// The host cannot currently serve the project.
    #[error("input host unavailable: {reason}")]
    HostUnavailable {
        /// Sanitized reason supplied by the adapter.
        reason: String,
    },
    /// The project root is not a normalized absolute path.
    #[error("invalid project root: {reason}")]
    InvalidProjectRoot {
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// A file path is not a valid project-relative path.
    #[error("invalid input path `{path}`: {reason}")]
    InvalidPath {
        /// Rejected path.
        path: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
    /// Two files use the same project-relative path.
    #[error("duplicate input path: {path}")]
    DuplicatePath {
        /// Duplicated path.
        path: String,
    },
    /// The extension is not a TypeScript or JavaScript script extension.
    #[error("unsupported input language: {path}")]
    UnsupportedLanguage {
        /// Rejected path.
        path: String,
    },
    /// A compiler option key is outside the frozen allow-list.
    #[error("unsupported compiler option: {key}")]
    UnsupportedCompilerOption {
        /// Rejected key.
        key: String,
    },
    /// A compiler option value does not have the documented shape.
    #[error("invalid compiler option `{key}`: {reason}")]
    InvalidCompilerOption {
        /// Rejected key.
        key: String,
        /// Stable rejection reason.
        reason: &'static str,
    },
}

fn validate_root(path: &str) -> Result<(), InputError> {
    let reject = |reason| Err(InputError::InvalidProjectRoot { reason });

    if path.is_empty() {
        return reject("empty");
    }
    if !path.starts_with('/') {
        return reject("not absolute");
    }
    if path == "/" {
        return reject("file-system root is not a project root");
    }
    if path.ends_with('/') {
        return reject("trailing separator");
    }
    if path.contains('\\') {
        return reject("backslash separator");
    }
    if path.contains("//") {
        return reject("empty component");
    }
    if path.chars().any(char::is_control) {
        return reject("control character");
    }
    if path
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return reject("dot component");
    }
    Ok(())
}

fn validate_file_path(path: &str) -> Result<(), InputError> {
    let reject = |reason| {
        Err(InputError::InvalidPath {
            path: path.to_owned(),
            reason,
        })
    };

    if path.is_empty() {
        return reject("empty");
    }
    if path.starts_with('/') {
        return reject("absolute");
    }
    if path.ends_with('/') {
        return reject("trailing separator");
    }
    if path.contains('\\') {
        return reject("backslash separator");
    }
    if path.contains("//") {
        return reject("empty component");
    }
    if path.chars().any(char::is_control) {
        return reject("control character");
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return reject("invalid component");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionKind {
    Boolean,
    String,
    StringArray,
    Paths,
}

/// Exhaustive allow-list table. Every key in [`COMPILER_OPTION_KEYS`] has
/// exactly one kind here; unknown keys are rejected before allocation.
fn option_kind(key: &str) -> Option<OptionKind> {
    match key {
        "strict"
        | "strictNullChecks"
        | "strictFunctionTypes"
        | "strictBindCallApply"
        | "strictPropertyInitialization"
        | "noImplicitAny"
        | "noImplicitThis"
        | "noUncheckedIndexedAccess"
        | "exactOptionalPropertyTypes"
        | "strictBuiltinIteratorReturn"
        | "useUnknownInCatchVariables"
        | "allowJs"
        | "checkJs"
        | "esModuleInterop"
        | "allowSyntheticDefaultImports"
        | "resolveJsonModule"
        | "isolatedModules"
        | "verbatimModuleSyntax"
        | "forceConsistentCasingInFileNames"
        | "skipLibCheck"
        | "experimentalDecorators" => Some(OptionKind::Boolean),
        "target" | "module" | "moduleResolution" | "jsx" | "moduleDetection" | "baseUrl"
        | "rootDir" | "outDir" => Some(OptionKind::String),
        "lib" | "types" | "typeRoots" => Some(OptionKind::StringArray),
        "paths" => Some(OptionKind::Paths),
        _ => None,
    }
}

fn validate_compiler_option(key: &str, value: &Value) -> Result<(), InputError> {
    let Some(kind) = option_kind(key) else {
        return Err(InputError::UnsupportedCompilerOption {
            key: key.to_owned(),
        });
    };
    match kind {
        OptionKind::Boolean => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(InputError::InvalidCompilerOption {
                    key: key.to_owned(),
                    reason: "expected boolean",
                })
            }
        }
        OptionKind::String => validate_option_string(key, value),
        OptionKind::StringArray => validate_option_string_array(key, value),
        OptionKind::Paths => validate_option_paths(value),
    }
}

fn validate_option_paths(value: &Value) -> Result<(), InputError> {
    let invalid = |reason| {
        Err(InputError::InvalidCompilerOption {
            key: "paths".to_owned(),
            reason,
        })
    };
    let Some(paths) = value.as_object() else {
        return invalid("expected object");
    };
    if paths.len() > MAX_OPTION_ARRAY_ENTRIES {
        return invalid("too many entries");
    }
    for (name, targets) in paths {
        if name.is_empty()
            || name.len() > MAX_OPTION_STRING_BYTES
            || name.chars().any(char::is_control)
        {
            return invalid("invalid path mapping name");
        }
        validate_option_string_array("paths", targets)?;
    }
    Ok(())
}

fn validate_option_string(key: &str, value: &Value) -> Result<(), InputError> {
    let Some(text) = value.as_str() else {
        return Err(InputError::InvalidCompilerOption {
            key: key.to_owned(),
            reason: "expected string",
        });
    };
    if text.is_empty() {
        return Err(InputError::InvalidCompilerOption {
            key: key.to_owned(),
            reason: "empty string",
        });
    }
    if text.len() > MAX_OPTION_STRING_BYTES {
        return Err(InputError::InvalidCompilerOption {
            key: key.to_owned(),
            reason: "string too long",
        });
    }
    if text.chars().any(char::is_control) {
        return Err(InputError::InvalidCompilerOption {
            key: key.to_owned(),
            reason: "control character",
        });
    }
    Ok(())
}

fn validate_option_string_array(key: &str, value: &Value) -> Result<(), InputError> {
    let Some(items) = value.as_array() else {
        return Err(InputError::InvalidCompilerOption {
            key: key.to_owned(),
            reason: "expected string array",
        });
    };
    if items.len() > MAX_OPTION_ARRAY_ENTRIES {
        return Err(InputError::InvalidCompilerOption {
            key: key.to_owned(),
            reason: "too many entries",
        });
    }
    for item in items {
        validate_option_string(key, item)?;
    }
    Ok(())
}

fn options_payload_size(values: &BTreeMap<String, Value>) -> usize {
    values.iter().fold(0usize, |total, (key, value)| {
        total
            .saturating_add(key.len())
            .saturating_add(value_payload_size(value))
    })
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(items) => items.iter().fold(8usize, |total, item| {
            total.saturating_add(value_payload_size(item))
        }),
        Value::Object(map) => map.iter().fold(8usize, |total, (key, value)| {
            total
                .saturating_add(key.len())
                .saturating_add(value_payload_size(value))
        }),
        Value::Null => 0,
        Value::Bool(_) | Value::Number(_) => 8,
    }
}

fn options_digest(values: &BTreeMap<String, Value>) -> String {
    let encoded = serde_json::to_vec(values).unwrap_or_default();
    hex_lower(&Sha256::digest(&encoded))
}

fn validate_asset_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("empty");
    }
    if !path.starts_with('/') {
        return Err("not absolute");
    }
    if path == "/" {
        return Err("file-system root is not an asset root");
    }
    if path.ends_with('/') {
        return Err("trailing separator");
    }
    if path.len() > MAX_ASSET_PATH_BYTES {
        return Err("path too long");
    }
    if path.contains('\\') {
        return Err("backslash separator");
    }
    if path.contains("//") {
        return Err("empty component");
    }
    if path.chars().any(char::is_control) {
        return Err("control character");
    }
    let components: Vec<&str> = path.split('/').skip(1).collect();
    if components.len() > MAX_ASSET_PATH_COMPONENTS {
        return Err("too many components");
    }
    if components
        .iter()
        .any(|component| component.len() > MAX_ASSET_COMPONENT_BYTES)
    {
        return Err("component too long");
    }
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        return Err("dot component");
    }
    Ok(())
}

fn verify_pin_count(field: &'static str, expected: u64, actual: u64) -> Result<(), TypeAssetError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TypeAssetError::PinMismatch {
            field,
            expected: expected.to_string(),
            actual: actual.to_string(),
        })
    }
}

fn verify_pin_digest(
    field: &'static str,
    expected: &str,
    actual: &str,
) -> Result<(), TypeAssetError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TypeAssetError::PinMismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

/// Computes the manifest bundle digest for one fixed root.
///
/// Files are sorted by relative path and hashed as
/// `"{sha256}  {relative_path}\n"` concatenated in that order. This matches
/// the delivery manifest algorithm recorded for the pinned fixture.
fn bundle_digest(root: &str, files: &mut Vec<&TypeAssetFile>) -> String {
    files.sort_by(|left, right| left.path().cmp(right.path()));
    let mut hasher = Sha256::new();
    for file in files {
        let path = file.path().as_str();
        let relative = path
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(path);
        hasher.update(file.sha256().as_bytes());
        hasher.update(b"  ");
        hasher.update(relative.as_bytes());
        hasher.update(b"\n");
    }
    hex_lower(&hasher.finalize())
}

fn typings_package_root(root: &AssetPath) -> String {
    format!("{}/{MINIPROGRAM_TYPINGS}", root.as_str())
}

fn within_asset_root(root: &str, path: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
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

#[cfg(test)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn file(path: &str, text: &str) -> TypeAssetFile {
        TypeAssetFile::new(
            AssetPath::new(path).expect("asset path"),
            text,
            hex_lower(&Sha256::digest(text.as_bytes())),
            u64::try_from(text.len()).expect("size"),
        )
        .expect("asset")
    }

    pub(crate) fn pin_for(
        standard_lib_root: &str,
        typings_root: &str,
        files: &[TypeAssetFile],
    ) -> TypeAssetsPin {
        let typings_package = typings_package_root(&AssetPath::new(typings_root).expect("root"));
        let mut standard_lib: Vec<&TypeAssetFile> = files
            .iter()
            .filter(|file| within_asset_root(standard_lib_root, file.path().as_str()))
            .collect();
        let mut typings: Vec<&TypeAssetFile> = files
            .iter()
            .filter(|file| within_asset_root(&typings_package, file.path().as_str()))
            .collect();
        TypeAssetsPin::new(
            PINNED_TYPINGS_VERSION,
            PINNED_STANDARD_LIB_FILE_COUNT,
            standard_lib.iter().map(|file| file.size()).sum(),
            bundle_digest(standard_lib_root, &mut standard_lib),
            PINNED_TYPINGS_FILE_COUNT,
            typings.iter().map(|file| file.size()).sum(),
            bundle_digest(&typings_package, &mut typings),
        )
        .expect("matching pin")
    }

    pub(crate) fn fixture_files(tag: &str) -> (String, String, Vec<TypeAssetFile>) {
        fixture_files_at(
            &format!("/pkg/{tag}/engine/lib"),
            &format!("/pkg/{tag}/typings"),
            tag,
        )
    }

    pub(crate) fn fixture_files_at(
        standard_lib_root: &str,
        typings_root: &str,
        tag: &str,
    ) -> (String, String, Vec<TypeAssetFile>) {
        let standard_lib_root = standard_lib_root.to_owned();
        let typings_root = typings_root.to_owned();
        let typings_package = format!("{typings_root}/{MINIPROGRAM_TYPINGS}");
        let mut files = Vec::new();
        for index in 0..PINNED_STANDARD_LIB_FILE_COUNT {
            let name = match index {
                0 => "lib.d.ts".to_owned(),
                1 => "lib.es5.d.ts".to_owned(),
                _ => format!("lib.extra{index}.d.ts"),
            };
            files.push(file(
                &format!("{standard_lib_root}/{name}"),
                &format!("declare const stdlib{index}: number; // {tag}\n"),
            ));
        }
        for index in 0..PINNED_TYPINGS_FILE_COUNT {
            let name = match index {
                0 => "index.d.ts".to_owned(),
                1 => "types/index.d.ts".to_owned(),
                2 => "types/wx/index.d.ts".to_owned(),
                _ => format!("types/extra{index}.d.ts"),
            };
            files.push(file(
                &format!("{typings_package}/{name}"),
                &format!("declare const typings{index}: number; // {tag}\n"),
            ));
        }
        (standard_lib_root, typings_root, files)
    }

    pub(crate) fn fixture_assets(tag: &str) -> TypeAssets {
        let (standard_lib_root, typings_root, files) = fixture_files(tag);
        let pin = pin_for(&standard_lib_root, &typings_root, &files);
        TypeAssets::new(
            AssetPath::new(standard_lib_root).expect("stdlib root"),
            AssetPath::new(typings_root).expect("typings root"),
            files,
            pin,
        )
        .expect("fixture assets")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> SourceFile {
        SourceFile::new(path, text, DocumentRevision::new(1)).expect("valid fixture")
    }

    fn input(files: Vec<SourceFile>) -> Result<AnalysisInput, InputError> {
        AnalysisInput::new(
            ProjectKey::new("demo"),
            ProjectRoot::new("/work/demo").expect("valid root"),
            SnapshotRevision::new(7),
            files,
        )
    }

    #[test]
    fn accepts_normalized_project_paths_and_keeps_text_bytes() {
        let result = input(vec![file("src/index.ts", "\u{FEFF}const x = 1;\r\n")]).expect("valid");
        let script = result.file("src/index.ts").expect("file");
        assert_eq!(script.language(), Language::TypeScript);
        assert!(script.text().starts_with('\u{FEFF}'));
        assert!(script.text().ends_with("\r\n"));
    }

    #[test]
    fn rejects_invalid_and_duplicate_paths() {
        for path in [
            "",
            "/abs.ts",
            "../escape.ts",
            "a//b.ts",
            "a/./b.ts",
            "a/../b.ts",
            "a\\b.ts",
            "a\u{0}b.ts",
            "a/",
        ] {
            assert!(
                SourceFile::new(path, "", DocumentRevision::new(1)).is_err(),
                "{path:?} must be rejected"
            );
        }

        let duplicate = input(vec![file("a.ts", ""), file("a.ts", "x")]);
        assert!(matches!(duplicate, Err(InputError::DuplicatePath { .. })));
    }

    #[test]
    fn rejects_non_script_extensions_and_invalid_roots() {
        for path in ["index.wxml", "index.tsx", "index.jsx"] {
            assert!(
                matches!(
                    SourceFile::new(path, "", DocumentRevision::new(1)),
                    Err(InputError::UnsupportedLanguage { .. })
                ),
                "{path} must be rejected until JSX lexing exists"
            );
        }
        for root in [
            "",
            "work",
            "/",
            "/work/",
            "/work//x",
            "/work/../x",
            "/work\\x",
        ] {
            assert!(ProjectRoot::new(root).is_err(), "{root:?} must be rejected");
        }
    }

    #[test]
    fn javascript_is_detected_from_the_extension() {
        let script = file("index.js", "Page({});");
        assert_eq!(script.language(), Language::JavaScript);
        assert!(script.language().is_javascript());
    }

    #[test]
    fn missing_file_lookup_is_none() {
        let result = input(vec![file("a.ts", "")]).expect("valid");
        assert!(result.file("b.ts").is_none());
    }

    use super::test_support::{file as asset_file, fixture_assets, fixture_files, pin_for};

    fn sha(text: &str) -> String {
        hex_lower(&Sha256::digest(text.as_bytes()))
    }

    fn full_options() -> BTreeMap<String, Value> {
        COMPILER_OPTION_KEYS
            .iter()
            .map(|key| {
                let value = match option_kind(key).expect("allowlisted kind") {
                    OptionKind::Boolean => Value::Bool(true),
                    OptionKind::String => Value::String("es2020".to_owned()),
                    OptionKind::StringArray => Value::Array(vec![Value::String("x".to_owned())]),
                    OptionKind::Paths => serde_json::json!({"@/*": ["src/*"]}),
                };
                ((*key).to_owned(), value)
            })
            .collect()
    }

    #[test]
    fn compiler_options_accept_exactly_the_frozen_keys() {
        let options = full_options();
        assert_eq!(options.len(), COMPILER_OPTION_KEYS.len());
        let parsed = CompilerOptions::new(options.clone()).expect("valid options");
        assert_eq!(parsed.values().len(), 33);
        assert_eq!(parsed.digest().len(), 64);

        let reversed: BTreeMap<String, Value> = options.into_iter().rev().collect();
        let parsed_reversed = CompilerOptions::new(reversed).expect("valid options");
        assert_eq!(parsed.digest(), parsed_reversed.digest());

        let mut kinds = BTreeMap::new();
        for key in COMPILER_OPTION_KEYS {
            assert!(
                kinds.insert(key, option_kind(key).expect("kind")).is_none(),
                "{key} must appear once"
            );
        }
        assert_eq!(kinds.len(), 33);

        assert!(matches!(
            CompilerOptions::new(BTreeMap::from([("unknown".to_owned(), Value::Bool(true))])),
            Err(InputError::UnsupportedCompilerOption { .. })
        ));
    }

    #[test]
    fn compiler_options_reject_wrong_value_shapes() {
        for (key, value) in [
            ("strict", Value::String("true".to_owned())),
            ("target", Value::Bool(true)),
            ("lib", Value::String("es2020".to_owned())),
            ("paths", Value::Array(vec![])),
            ("types", serde_json::json!(["ok", 1])),
        ] {
            assert!(
                CompilerOptions::new(BTreeMap::from([(key.to_owned(), value)])).is_err(),
                "{key} must reject this value shape"
            );
        }
    }

    #[test]
    fn compiler_options_enforce_string_array_and_payload_caps() {
        let long = "x".repeat(MAX_OPTION_STRING_BYTES + 1);
        assert!(matches!(
            CompilerOptions::new(BTreeMap::from([
                ("target".to_owned(), Value::String(long),)
            ])),
            Err(InputError::InvalidCompilerOption { .. })
        ));

        let too_many = Value::Array(
            (0..=MAX_OPTION_ARRAY_ENTRIES)
                .map(|index| Value::String(format!("x{index}")))
                .collect(),
        );
        assert!(matches!(
            CompilerOptions::new(BTreeMap::from([("types".to_owned(), too_many)])),
            Err(InputError::InvalidCompilerOption { .. })
        ));

        let many_paths = (0..=MAX_OPTION_ARRAY_ENTRIES)
            .map(|index| (format!("@/{index}"), serde_json::json!(["src/*"])))
            .collect::<serde_json::Map<String, Value>>();
        assert!(matches!(
            CompilerOptions::new(BTreeMap::from([(
                "paths".to_owned(),
                Value::Object(many_paths),
            )])),
            Err(InputError::InvalidCompilerOption { .. })
        ));

        let wildcard = Value::Array(
            (0..MAX_OPTION_ARRAY_ENTRIES)
                .map(|index| Value::String(format!("src/{index}/*")))
                .collect(),
        );
        assert!(matches!(
            CompilerOptions::new(BTreeMap::from([
                ("types".to_owned(), wildcard),
                (
                    "lib".to_owned(),
                    Value::Array(vec![
                        Value::String("a".repeat(MAX_OPTION_STRING_BYTES));
                        MAX_OPTION_ARRAY_ENTRIES
                    ])
                ),
            ])),
            Err(InputError::InvalidCompilerOption { .. })
        ));
    }

    #[test]
    fn type_assets_validate_pin_identity_and_paths() {
        let valid = fixture_assets("valid");
        assert_eq!(valid.file_count(), PINNED_TYPE_ASSET_FILES);
        assert_eq!(valid.total_bytes(), valid.pin().total_bytes());
        assert!(valid.accounted_bytes() >= valid.total_bytes());
        assert_eq!(valid.package_version(), PINNED_TYPINGS_VERSION);
        assert_eq!(
            valid.typings_package_root(),
            format!("{}/{}", valid.typings_root().as_str(), MINIPROGRAM_TYPINGS)
        );

        let (standard_lib_root, typings_root, files) = fixture_files("paths");
        let pin = pin_for(&standard_lib_root, &typings_root, &files);

        let outside = {
            let mut changed = files.clone();
            changed.push(asset_file(
                "/pkg/paths/engine/lib-evil/extra.d.ts",
                "export {};\n",
            ));
            TypeAssets::new(
                AssetPath::new(&standard_lib_root).expect("stdlib root"),
                AssetPath::new(&typings_root).expect("typings root"),
                changed,
                pin.clone(),
            )
        };
        assert!(matches!(outside, Err(TypeAssetError::InvalidPath { .. })));

        let duplicate = {
            let mut changed = files.clone();
            changed.push(files[0].clone());
            TypeAssets::new(
                AssetPath::new(&standard_lib_root).expect("stdlib root"),
                AssetPath::new(&typings_root).expect("typings root"),
                changed,
                pin.clone(),
            )
        };
        assert!(matches!(
            duplicate,
            Err(TypeAssetError::DuplicatePath { .. })
        ));

        let overlap = TypeAssets::new(
            AssetPath::new("/pkg/paths").expect("root"),
            AssetPath::new("/pkg/paths/typings").expect("root"),
            files,
            pin,
        );
        assert!(matches!(overlap, Err(TypeAssetError::RootsOverlap { .. })));
    }

    #[test]
    fn type_assets_pin_rejects_transitive_stdlib_and_typings_loss() {
        let (standard_lib_root, typings_root, files) = fixture_files("closure");
        let pin = pin_for(&standard_lib_root, &typings_root, &files);

        let without_lib_es5 = files
            .iter()
            .filter(|file| !file.path().as_str().ends_with("/lib.es5.d.ts"))
            .cloned()
            .collect::<Vec<_>>();
        let result = TypeAssets::new(
            AssetPath::new(&standard_lib_root).expect("stdlib root"),
            AssetPath::new(&typings_root).expect("typings root"),
            without_lib_es5,
            pin.clone(),
        );
        assert!(matches!(
            result,
            Err(TypeAssetError::PinMismatch {
                field: "standardLibFileCount",
                ..
            })
        ));

        let without_wx = files
            .iter()
            .filter(|file| !file.path().as_str().ends_with("/types/wx/index.d.ts"))
            .cloned()
            .collect::<Vec<_>>();
        let result = TypeAssets::new(
            AssetPath::new(&standard_lib_root).expect("stdlib root"),
            AssetPath::new(&typings_root).expect("typings root"),
            without_wx,
            pin.clone(),
        );
        assert!(matches!(
            result,
            Err(TypeAssetError::PinMismatch {
                field: "typingsFileCount",
                ..
            })
        ));

        let mutated = files
            .iter()
            .map(|file| {
                if file.path().as_str().ends_with("/lib.d.ts") {
                    asset_file(
                        file.path().as_str(),
                        "declare const stdlibZ: number; // closure\n",
                    )
                } else {
                    file.clone()
                }
            })
            .collect::<Vec<_>>();
        let result = TypeAssets::new(
            AssetPath::new(&standard_lib_root).expect("stdlib root"),
            AssetPath::new(&typings_root).expect("typings root"),
            mutated,
            pin,
        );
        assert!(matches!(
            result,
            Err(TypeAssetError::PinMismatch {
                field: "standardLibBundleDigest",
                ..
            })
        ));
    }

    #[test]
    fn type_assets_reject_bad_digest_size_version_and_pin() {
        let path = AssetPath::new("/pkg/engine/lib/lib.d.ts").expect("path");
        assert!(matches!(
            TypeAssetFile::new(path.clone(), "x", "A".repeat(64), 1),
            Err(TypeAssetError::InvalidDigest { .. })
        ));
        assert!(matches!(
            TypeAssetFile::new(path.clone(), "x", sha("x"), 2),
            Err(TypeAssetError::SizeMismatch { .. })
        ));
        assert!(matches!(
            TypeAssetFile::new(path, "x", sha("y"), 1),
            Err(TypeAssetError::HashMismatch { .. })
        ));

        assert!(matches!(
            TypeAssetsPin::new("5.3.1/../escape", 116, 1, sha("pin-a"), 20, 1, sha("pin-b")),
            Err(TypeAssetError::InvalidVersion { .. })
        ));
        assert!(matches!(
            TypeAssetsPin::new("5.3.1", 1, 1, sha("pin-a"), 20, 1, sha("pin-b")),
            Err(TypeAssetError::PinMismatch {
                field: "standardLibFileCount",
                ..
            })
        ));
        assert!(matches!(
            TypeAssetsPin::new("5.3.1", 116, 1, "not-a-sha", 20, 1, sha("pin-b")),
            Err(TypeAssetError::InvalidPinDigest {
                field: "standardLibBundleDigest"
            })
        ));

        for path in [
            "",
            "relative/lib.d.ts",
            "/",
            "/pkg/../escape.d.ts",
            "/pkg//lib.d.ts",
            "/pkg\\lib.d.ts",
            "/pkg/\u{0}lib.d.ts",
        ] {
            assert!(
                AssetPath::new(path).is_err(),
                "{path:?} must not be an asset path"
            );
        }

        let long = format!("/pkg/{}", "a".repeat(MAX_ASSET_PATH_BYTES));
        assert!(AssetPath::new(long).is_err());
        let sixty_four_kib = format!("/pkg/{}", "a".repeat(64 * 1024));
        assert!(AssetPath::new(sixty_four_kib).is_err());
        let components = format!("/{}", vec!["a"; MAX_ASSET_PATH_COMPONENTS + 1].join("/"));
        assert!(AssetPath::new(components).is_err());
        let component = format!("/pkg/{}", "a".repeat(MAX_ASSET_COMPONENT_BYTES + 1));
        assert!(AssetPath::new(component).is_err());

        assert!(matches!(
            TypeAssetFile::new(
                AssetPath::new("/pkg/engine/lib/lib.txt").expect("path"),
                "x",
                sha("x"),
                1
            ),
            Err(TypeAssetError::UnsupportedFile { .. })
        ));
    }

    #[test]
    fn type_assets_enforce_file_and_byte_budgets() {
        let files = (0..=MAX_TYPE_ASSET_FILES)
            .map(|index| {
                asset_file(
                    &format!("/pkg/budget/lib{index}.d.ts"),
                    "declare const x: 1;\n",
                )
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            TypeAssets::new(
                AssetPath::new("/pkg/budget").expect("root"),
                AssetPath::new("/pkg/typings").expect("root"),
                files,
                TypeAssetsPin::pinned_runtime(),
            ),
            Err(TypeAssetError::TooManyFiles { .. })
        ));

        let long_path = format!(
            "/pkg/budget/{}.d.ts",
            "a".repeat(MAX_ASSET_COMPONENT_BYTES - 5)
        );
        let huge = "x".repeat(MAX_TYPE_ASSET_BYTES - 100);
        assert!(matches!(
            TypeAssets::new(
                AssetPath::new("/pkg/budget").expect("root"),
                AssetPath::new("/pkg/typings").expect("root"),
                vec![asset_file(&long_path, &huge)],
                TypeAssetsPin::pinned_runtime(),
            ),
            Err(TypeAssetError::TooLarge { .. })
        ));
    }

    #[test]
    fn analysis_input_keeps_the_legacy_constructor_asset_free() {
        let legacy = input(vec![file("a.ts", "")]).expect("legacy input");
        assert!(!legacy.has_type_environment());
        assert!(legacy.type_assets().is_none());
        assert!(legacy.type_environment_identity().is_none());
        assert!(legacy.compiler_options().values().is_empty());

        let assets = fixture_assets("identity");
        let environment = AnalysisInput::with_environment(
            ProjectKey::new("demo"),
            ProjectRoot::new("/work/demo").expect("root"),
            SnapshotRevision::new(1),
            vec![file("a.ts", "")],
            CompilerOptions::new(full_options()).expect("options"),
            Arc::new(assets.clone()),
        )
        .expect("environment input");
        assert!(environment.has_type_environment());
        assert_eq!(
            environment.type_assets().expect("assets").file_count(),
            PINNED_TYPE_ASSET_FILES
        );
        assert_eq!(environment.compiler_options().values().len(), 33);
        let identity = environment
            .type_environment_identity()
            .expect("environment identity");
        assert_eq!(
            identity.standard_lib_root(),
            assets.standard_lib_root().as_str()
        );
        assert_eq!(identity.options_digest().len(), 64);
    }
}
