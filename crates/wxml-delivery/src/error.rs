//! Error type shared by the packager and the installer.
//!
//! Messages intentionally avoid embedding full user paths. Callers pass a path
//! through [`display_name`] or a fixed label so logs and errors cannot leak a
//! home directory layout.

use std::path::Path;

/// Result alias used throughout this crate.
pub type Result<T> = std::result::Result<T, DeliveryError>;

/// A packaging, download, verification or installation failure.
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    /// A command-line value or API argument is malformed.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The host platform cannot be packaged or installed by this build.
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
    /// A network request failed before a trustworthy HTTP response arrived.
    #[error("network request failed: {0}")]
    Network(String),
    /// A server answered with an unexpected status.
    #[error("unexpected HTTP status {status} for `{resource}`")]
    HttpStatus {
        /// Numeric HTTP status.
        status: u16,
        /// Redacted resource name.
        resource: String,
    },
    /// The configured source is not allowed by the trusted-source policy.
    #[error("untrusted download source: {0}")]
    UntrustedSource(String),
    /// A manifest, index or archive is structurally invalid.
    #[error("invalid package: {0}")]
    InvalidPackage(String),
    /// A checksum, size or declared version did not match.
    #[error("package integrity check failed: {0}")]
    Integrity(String),
    /// An archive entry attempts to escape the extraction root or has an
    /// unsupported file type.
    #[error("unsafe archive entry: {0}")]
    UnsafeArchive(String),
    /// A declared or observed size exceeds a fixed limit.
    #[error("size limit exceeded: {0}")]
    SizeLimit(String),
    /// The on-disk installation state is inconsistent.
    #[error("invalid install state: {0}")]
    InstallState(String),
    /// A runtime (Node.js, the installed server or its dependencies) failed a
    /// post-install check.
    #[error("runtime check failed: {0}")]
    Runtime(String),
    /// An I/O operation failed.
    #[error("I/O error on `{path}`: {source}")]
    Io {
        /// Basename or fixed label, never a full user path.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

impl DeliveryError {
    /// Wraps an I/O error without exposing the full path.
    #[must_use]
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: display_name(path),
            source,
        }
    }

    /// Wraps an I/O error with a fixed label.
    #[must_use]
    pub fn io_label(label: &str, source: std::io::Error) -> Self {
        Self::Io {
            path: label.to_owned(),
            source,
        }
    }
}

/// Returns the final path component for diagnostics.
///
/// The installer writes to user-selected directories; echoing them in errors
/// would leak the home layout into logs and bug reports.
#[must_use]
pub fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "<path>".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}
