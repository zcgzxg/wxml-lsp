//! Thin `wxml-lsp` adapter shell for the native tsgo backend.
//!
//! `P3-I` only wires the `wxml-backend-tsgo` dependency. This module is
//! intentionally inert: it is not constructed by `server.rs`,
//! `project_host.rs` or the configuration defaults, and it does not implement
//! or replace [`wxml_types::Backend`]. `P3-N`/`P4` will replace the shell with
//! the real adapter once native sessions, snapshots and mapping exist.
//!
//! Until then every future query entry point must return an explicit
//! [`wxml_backend_tsgo::BackendError::Unimplemented`]; there is no fallback to
//! the Node backend and no synthesized empty success.
//!
//! The module-level `allow(dead_code)` records that the shell is deliberately
//! unwired while keeping `-D warnings` meaningful for the rest of the crate.

#![allow(
    dead_code,
    reason = "P3-I wires the tsgo adapter but no server or config path selects it yet"
)]

use wxml_backend_tsgo::BackendError;
use wxml_backend_tsgo::config::PinnedEngine;

/// Marker used until `P3-N`/`P4` replace this shell with the real adapter.
pub(crate) const UNIMPLEMENTED: &str =
    "tsgo adapter is scaffolding only; P3-N/P4 own native sessions";

/// Inert adapter between the LSP crate and the native backend crate.
#[derive(Debug)]
pub(crate) struct TsgoBackendAdapter {
    engine: PinnedEngine,
}

impl TsgoBackendAdapter {
    /// Creates the adapter around `engine` without starting a process.
    #[must_use]
    pub(crate) const fn new(engine: PinnedEngine) -> Self {
        Self { engine }
    }

    /// Creates the adapter around the frozen engine identity.
    #[must_use]
    pub(crate) const fn pinned() -> Self {
        Self::new(PinnedEngine::pinned())
    }

    /// Returns the pinned identity the future backend will re-verify.
    #[must_use]
    pub(crate) const fn engine(&self) -> &PinnedEngine {
        &self.engine
    }

    /// Explicit unsupported result for any future query entry point.
    ///
    /// This is an associated function because the shell holds no session and
    /// callers should not need an instance to record the failure.
    #[must_use]
    pub(crate) const fn unsupported(feature: &'static str) -> BackendError {
        BackendError::Unimplemented { feature }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_is_explicitly_unimplemented() {
        let adapter = TsgoBackendAdapter::pinned();
        assert_eq!(
            adapter.engine().version(),
            wxml_backend_tsgo::ENGINE_VERSION
        );

        let error = TsgoBackendAdapter::unsupported(UNIMPLEMENTED);
        assert!(matches!(error, BackendError::Unimplemented { .. }));
        assert!(UNIMPLEMENTED.contains("P3-N/P4"));
    }
}
