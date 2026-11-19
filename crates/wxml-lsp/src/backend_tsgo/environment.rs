//! Trusted native type environment for the opt-in backend.
//!
//! [`Environment::Trusted`] owns one verified [`TypeAssets`] set that was
//! loaded once from a [`DiscoveredInstall`]. [`Environment::Legacy`] is the
//! historical no-asset mode used by the explicit test/probe constructors
//! [`TsgoBackendAdapter::new`](super::TsgoBackendAdapter::new) and
//! [`TsgoBackendAdapter::with_host`](super::TsgoBackendAdapter::with_host); the
//! production `run_stdio_native` path can only obtain `Trusted`.
//!
//! Asset loading happens before any sandbox or session exists. A missing,
//! unsafe, incomplete or pin-mismatched asset tree therefore fails the process
//! startup with a path-free [`EnvironmentLoadError`] instead of falling back to
//! Node or to a legacy session.
//!
//! The only supported production platform is Linux x86-64 with POSIX absolute
//! asset roots. Windows drive-letter/UNC roots and non-UTF-8 roots are rejected
//! before a session exists; no cross-platform asset claim is made.
//!
//! # Residual risks
//!
//! Assets are verified once per process, not re-hashed per request. The
//! backend still sends the full snapshot file set, including the ~6.4 MiB
//! asset closure, on every `updateSnapshot`; that is a performance limitation
//! outside this slice. Compiler `types` entries outside the virtual snapshot
//! are forwarded without a recorded degradation today.
//!
//! # Pin policy
//!
//! [`TypeAssets::load_pinned`] fixes the built-in production pin. The adapter
//! never derives the pin from the tree it is about to load and never accepts a
//! caller-supplied pin. Tests build in-memory assets with an explicit pin
//! through [`TypeAssets::new`]; that seam is not reachable from production.

use std::fmt;
use std::sync::Arc;

use thiserror::Error;
use wxml_backend_tsgo::input::{
    CompilerOptions, TypeAssetError, TypeAssets, TypeEnvironmentIdentity,
};
use wxml_backend_tsgo::native::vfs::EnvironmentDegradation;

use crate::backend_tsgo::engine::DiscoveredInstall;

/// One native type environment.
///
/// The variant is not an `Option` seam: a trusted adapter never silently falls
/// back to [`Self::Legacy`], and a legacy adapter never mounts type assets.
pub(crate) enum Environment {
    /// Verified standard-library and mini-program typings loaded once.
    Trusted(Arc<TypeAssets>),
    /// Historical no-asset mode; only `new`/`with_host` and tests produce it.
    Legacy,
}

impl Environment {
    /// Returns the legacy no-asset mode.
    pub(crate) const fn legacy() -> Self {
        Self::Legacy
    }

    /// Wraps one already verified asset set.
    pub(crate) const fn trusted(assets: Arc<TypeAssets>) -> Self {
        Self::Trusted(assets)
    }

    /// Returns whether this environment carries verified type assets.
    pub(crate) const fn is_trusted(&self) -> bool {
        matches!(self, Self::Trusted(_))
    }

    /// Returns the verified asset set, if any.
    pub(crate) const fn assets(&self) -> Option<&Arc<TypeAssets>> {
        match self {
            Self::Trusted(assets) => Some(assets),
            Self::Legacy => None,
        }
    }

    /// Returns the stable environment identity for `options`.
    ///
    /// `None` is the legacy mode. `Some` includes the compiler-options digest
    /// and every asset root/pin field, exactly the value a native session
    /// requires to stay unchanged across updates.
    pub(crate) fn identity(&self, options: &CompilerOptions) -> Option<TypeEnvironmentIdentity> {
        self.assets()
            .map(|assets| TypeEnvironmentIdentity::new(options, assets))
    }
}

impl Clone for Environment {
    fn clone(&self) -> Self {
        match self {
            Self::Trusted(assets) => Self::Trusted(Arc::clone(assets)),
            Self::Legacy => Self::Legacy,
        }
    }
}

impl fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trusted(assets) => formatter
                .debug_tuple("Trusted")
                .field(&format_args!("{} files", assets.file_count()))
                .finish(),
            Self::Legacy => formatter.write_str("Legacy"),
        }
    }
}

/// Why the production native type environment could not be established.
///
/// Messages are path-free and stable: asset errors that carry host paths or
/// underlying `io::Error` values are reduced to an auditable category before
/// they reach the LSP startup error, so command-line output never exposes the
/// installation layout.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EnvironmentLoadError {
    /// The verified engine is not inside the standard-library root.
    #[error("the pinned engine is not inside the standard-library root")]
    EngineRootMismatch,
    /// An asset root is missing, a symlink or not a directory.
    #[error("a type-asset root is missing or not a directory")]
    RootUnavailable,
    /// The asset tree violates a structural safety rule.
    #[error("the type-asset tree is unsafe: {reason}")]
    UnsafeTree {
        /// Stable rule that failed; never a path.
        reason: &'static str,
    },
    /// A mandatory declaration is absent from the tree.
    #[error("a required type declaration is missing")]
    MissingRequired,
    /// A count, byte total, version or bundle digest does not match the pin.
    #[error("type assets do not match the pinned identity: {field}")]
    PinMismatch {
        /// Pin field or identity category that failed.
        field: &'static str,
    },
    /// A per-file size or digest differs from its recorded identity.
    #[error("a type asset does not match its recorded identity")]
    AssetMismatch,
    /// The asset set exceeds a fixed file, byte or entry budget.
    #[error("the type-asset set exceeds the native budget")]
    BudgetExceeded,
    /// An asset could not be read.
    #[error("a type asset could not be read")]
    Unreadable,
    /// An asset changed identity or length while it was being read.
    #[error("a type asset changed while it was read")]
    ChangedWhileReading,
}

/// Loads the production type environment from a discovered install.
///
/// This is the only production asset entry point. It checks the engine/root
/// layout, then reads and verifies both roots against the crate pin. The
/// caller must not create a sandbox or start a session after an error.
pub(crate) fn load_environment(
    install: &DiscoveredInstall,
) -> Result<Environment, EnvironmentLoadError> {
    if install.engine.parent() != Some(install.standard_lib_root()) {
        return Err(EnvironmentLoadError::EngineRootMismatch);
    }
    let assets = TypeAssets::load_pinned(install.standard_lib_root(), install.typings_root())
        .map_err(|error| map_asset_error(&error))?;
    Ok(Environment::trusted(Arc::new(assets)))
}

/// Reduces one asset error to a stable category without host paths.
const fn map_asset_error(error: &TypeAssetError) -> EnvironmentLoadError {
    use TypeAssetError as Asset;
    match error {
        Asset::InvalidRoot { reason, .. }
        | Asset::InvalidPath { reason, .. }
        | Asset::UnsafeName { reason, .. }
        | Asset::UnexpectedEntry { reason, .. } => unsafe_tree(reason),
        Asset::DuplicatePath { .. } => unsafe_tree("duplicate asset path"),
        Asset::RootsOverlap { .. } => unsafe_tree("overlapping asset roots"),
        Asset::UnsupportedFile { .. } => unsafe_tree("non-declaration asset path"),
        Asset::Symlink { .. } => unsafe_tree("symbolic link in the asset tree"),
        Asset::NonRegularFile { .. } => unsafe_tree("non-regular asset entry"),
        Asset::HardLink { .. } => unsafe_tree("hard-linked asset file"),
        Asset::NonUtf8Path { .. } => unsafe_tree("non-UTF-8 asset path"),
        Asset::CaseCollision { .. } => unsafe_tree("case-insensitive asset path collision"),
        Asset::NestingTooDeep { .. } => unsafe_tree("asset directory nesting is too deep"),
        Asset::MissingRoot { .. } | Asset::RootNotDirectory { .. } => {
            EnvironmentLoadError::RootUnavailable
        }
        Asset::MissingRequired { .. } => EnvironmentLoadError::MissingRequired,
        Asset::PinMismatch { field, .. } => EnvironmentLoadError::PinMismatch { field },
        Asset::InvalidVersion { .. } => EnvironmentLoadError::PinMismatch {
            field: "typingsVersion",
        },
        Asset::InvalidDigest { .. }
        | Asset::InvalidPinDigest { .. }
        | Asset::SizeMismatch { .. }
        | Asset::HashMismatch { .. }
        | Asset::NonUtf8Content { .. } => EnvironmentLoadError::AssetMismatch,
        Asset::TooManyFiles { .. } | Asset::TooLarge { .. } | Asset::TooManyEntries { .. } => {
            EnvironmentLoadError::BudgetExceeded
        }
        Asset::AssetIo { .. } => EnvironmentLoadError::Unreadable,
        Asset::AssetChanged { .. } => EnvironmentLoadError::ChangedWhileReading,
    }
}

/// Builds an [`EnvironmentLoadError::UnsafeTree`] with a stable rule name.
const fn unsafe_tree(reason: &'static str) -> EnvironmentLoadError {
    EnvironmentLoadError::UnsafeTree { reason }
}

/// Formats open-session VFS degradations as one path-free reason.
///
/// Only the `'static` field name and reason of each [`EnvironmentDegradation`]
/// are rendered; user option values (which may embed host paths) are never
/// included.
pub(crate) fn degradation_reason(degradations: &[EnvironmentDegradation]) -> Option<String> {
    if degradations.is_empty() {
        return None;
    }
    let fields = degradations
        .iter()
        .map(|degradation| format!("{} ({})", degradation.field(), degradation.reason()))
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!("the native type environment is partial: {fields}"))
}

#[cfg(test)]
mod fakes;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod gate;
#[cfg(test)]
mod tests;
