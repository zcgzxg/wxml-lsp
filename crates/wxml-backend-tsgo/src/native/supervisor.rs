//! Native process supervision seam.
//!
//! No process is spawned in `P3-I`. The supervisor is the single authority for
//! lifecycle state and carries the pinned identity plus the frozen launch
//! description that `P3-N` will execute.

use crate::config::{LaunchError, LaunchPlan, PinnedEngine};

/// Lifecycle marker owned by the future supervisor.
///
/// This is the only lifecycle authority. `wxml-backend-tsgo`'s
/// `BackendState` is a projection of this enum, not a second source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// No child process exists.
    NotStarted,
    /// Probe and handshake are in progress.
    Starting,
    /// Probe passed and the session can serve requests.
    Ready,
    /// The child exited or the probe failed.
    Failed,
    /// Shutdown completed.
    Stopped,
}

/// Data-only supervisor placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineSupervisor {
    engine: PinnedEngine,
    lifecycle: Lifecycle,
}

impl EngineSupervisor {
    /// Creates the supervisor description without starting anything.
    #[must_use]
    pub const fn new(engine: PinnedEngine) -> Self {
        Self {
            engine,
            lifecycle: Lifecycle::NotStarted,
        }
    }

    /// Returns the pinned identity the future session will verify.
    #[must_use]
    pub const fn engine(&self) -> &PinnedEngine {
        &self.engine
    }

    /// Returns the current lifecycle marker.
    #[must_use]
    pub const fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// Returns the frozen launch plan for review; this does not spawn.
    pub fn launch_plan(&self, sandbox_root: &str) -> Result<LaunchPlan, LaunchError> {
        self.engine.launch_plan(sandbox_root)
    }
}
