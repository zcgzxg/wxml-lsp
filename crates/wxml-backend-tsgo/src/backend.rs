//! Backend state and result-cache seams for the native backend.
//!
//! The shell does not implement `wxml_types::Backend` and cannot serve LSP
//! requests. `P3-N`/`P4` replace [`TsgoBackend`] with the real orchestrator
//! once native sessions, snapshots and mapping exist.
//!
//! [`BackendState`] is an aggregate projection of the supervisor
//! [`crate::native::supervisor::Lifecycle`]. The supervisor owns the lifecycle;
//! this module never keeps a second independent state.

use thiserror::Error;

use crate::config::PinnedEngine;
use crate::input::{AnalysisInput, InputError, InputProvider, ProjectKey};
use crate::native::supervisor::{EngineSupervisor, Lifecycle};

/// Aggregate view of the supervisor lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendState {
    /// No native process has been started.
    Unstarted,
    /// A session is starting.
    Starting,
    /// A validated session can serve queries.
    Ready,
    /// The last session failed.
    Failed,
    /// Shutdown completed.
    Stopped,
}

impl BackendState {
    /// Projects the authoritative supervisor lifecycle into the backend view.
    #[must_use]
    pub const fn from_lifecycle(lifecycle: Lifecycle) -> Self {
        match lifecycle {
            Lifecycle::NotStarted => Self::Unstarted,
            Lifecycle::Starting => Self::Starting,
            Lifecycle::Ready => Self::Ready,
            Lifecycle::Failed => Self::Failed,
            Lifecycle::Stopped => Self::Stopped,
        }
    }
}

impl From<Lifecycle> for BackendState {
    fn from(lifecycle: Lifecycle) -> Self {
        Self::from_lifecycle(lifecycle)
    }
}

/// Bounded per-epoch result cache placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultCache {
    max_entries: usize,
}

impl ResultCache {
    /// Creates a cache description with a hard entry limit.
    ///
    /// `max_entries == 0` means caching is disabled; the future implementation
    /// must not store results in that case.
    #[must_use]
    pub const fn new(max_entries: usize) -> Self {
        Self { max_entries }
    }

    /// Returns the configured entry limit; zero disables caching.
    #[must_use]
    pub const fn max_entries(self) -> usize {
        self.max_entries
    }
}

/// Inert backend shell: identity, input seam and cache metadata only.
pub struct TsgoBackend {
    supervisor: EngineSupervisor,
    inputs: Box<dyn InputProvider>,
    cache: ResultCache,
}

impl std::fmt::Debug for TsgoBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TsgoBackend")
            .field("supervisor", &self.supervisor)
            .field("inputs", &"dyn InputProvider")
            .field("cache", &self.cache)
            .finish()
    }
}

impl TsgoBackend {
    /// Builds the inert backend shell. No process is started.
    #[must_use]
    pub fn new(engine: PinnedEngine, inputs: Box<dyn InputProvider>, cache: ResultCache) -> Self {
        Self {
            supervisor: EngineSupervisor::new(engine),
            inputs,
            cache,
        }
    }

    /// Returns the pinned engine identity.
    #[must_use]
    pub const fn engine(&self) -> &PinnedEngine {
        self.supervisor.engine()
    }

    /// Returns the backend view derived from the supervisor lifecycle.
    #[must_use]
    pub const fn state(&self) -> BackendState {
        BackendState::from_lifecycle(self.supervisor.lifecycle())
    }

    /// Returns the cache limits.
    #[must_use]
    pub const fn cache(&self) -> &ResultCache {
        &self.cache
    }

    /// Loads the host input only; no native session exists yet.
    pub fn load_input(&self, project: &ProjectKey) -> Result<AnalysisInput, BackendError> {
        self.inputs.load(project).map_err(BackendError::from)
    }
}

/// Errors exposed by the backend shell.
#[derive(Debug, Error)]
pub enum BackendError {
    /// The requested capability is scaffolding only.
    #[error("tsgo backend capability is not implemented: {feature}")]
    Unimplemented {
        /// Capability marker supplied by the caller.
        feature: &'static str,
    },
    /// Input collection failed before any native work.
    #[error(transparent)]
    Input(#[from] InputError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_state_is_a_projection_of_every_lifecycle_variant() {
        let pairs = [
            (Lifecycle::NotStarted, BackendState::Unstarted),
            (Lifecycle::Starting, BackendState::Starting),
            (Lifecycle::Ready, BackendState::Ready),
            (Lifecycle::Failed, BackendState::Failed),
            (Lifecycle::Stopped, BackendState::Stopped),
        ];

        for (lifecycle, expected) in pairs {
            assert_eq!(BackendState::from_lifecycle(lifecycle), expected);
            assert_eq!(BackendState::from(lifecycle), expected);
        }
    }

    #[test]
    fn zero_capacity_means_cache_disabled() {
        assert_eq!(ResultCache::new(0).max_entries(), 0);
        assert_eq!(ResultCache::new(128).max_entries(), 128);
    }
}
