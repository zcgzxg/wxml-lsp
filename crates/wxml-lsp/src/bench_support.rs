//! Benchmark-only entry points (feature `bench-support`).
//!
//! This module exists so the Criterion target under `crates/wxml-lsp/benches/`
//! can drive the real LSP path (transport, scheduler, workspace lookup, shadow
//! mapping and engine/style queries) without widening the production
//! `pub(crate)` surface.
//! Nothing here changes runtime behavior: the module is compiled only with the
//! `bench-support` feature and every function is a thin wrapper.
//!
//! The benchmark harness gets both halves of a shared host from
//! [`BenchBackend`]: pass [`BenchBackend::backend`] and [`BenchBackend::host`]
//! to `wxml_lsp::run_with_host` so the adapter's views and the server's session
//! observe the same [`ProjectState`] instance.

use std::path::Path;
use std::sync::Arc;

use wxml_types::Backend;

use crate::backend_tsgo::{TsgoBackendAdapter, engine};
use crate::project_host::{ProjectHost, ProjectState};

/// A backend plus the host instance it was built from.
///
/// Both fields must be used together: the adapter captured
/// [`BenchBackend::host`] when it was constructed, and a benchmark that opened
/// a *different* host would measure views the adapter never sees.
pub struct BenchBackend {
    /// Backend to pass to `wxml_lsp::run_with_host`.
    pub backend: Arc<dyn Backend>,
    /// Host to pass to `wxml_lsp::run_with_host`; the same instance the
    /// adapter holds.
    pub host: Arc<dyn ProjectHost>,
}

impl std::fmt::Debug for BenchBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BenchBackend")
            .field("epoch", &self.backend.epoch())
            .finish_non_exhaustive()
    }
}

/// Builds a native backend from an installed package root.
///
/// `root` is the install root (`<root>/bin/wxml-lsp`,
/// `<root>/engine/lib/tsc`, `<root>/typings`), e.g.
/// `$HOME/.local/share/wxml-lsp`. Discovery runs on
/// `<root>/bin/wxml-lsp` with the production engine pin, and the adapter then
/// verifies the pinned standard-library and typings assets before any sandbox
/// or session exists.
///
/// # Errors
///
/// Returns the discovery or startup failure as a path-free message. The string
/// is the `Display` form of [`crate::EngineDiscoveryError`] or
/// `NativeStartupError`, so it is safe to print.
pub fn backend_from_install(root: &Path) -> Result<BenchBackend, String> {
    let host: Arc<dyn ProjectHost> = Arc::new(ProjectState::new());
    let executable = root.join("bin/wxml-lsp");
    let install = engine::discover_at(&executable).map_err(|error| error.to_string())?;
    let adapter = TsgoBackendAdapter::with_install(host.clone(), &install)
        .map_err(|error| error.to_string())?;
    Ok(BenchBackend {
        backend: Arc::new(adapter),
        host,
    })
}

/// Builds a backend without an engine.
///
/// Every session-backed query fails closed with the `HostOnly` reason, so this
/// serves the static groups (template syntax, class index) without the pinned
/// engine or typings assets.
#[must_use]
pub fn host_only_backend() -> BenchBackend {
    let host: Arc<dyn ProjectHost> = Arc::new(ProjectState::new());
    let adapter = TsgoBackendAdapter::without_engine(host.clone());
    BenchBackend {
        backend: Arc::new(adapter),
        host,
    }
}

/// Returns every static class token in a template as `(start, end, name)`.
///
/// Offsets are UTF-8 byte offsets into `text`, matching the internal
/// [`crate::workspace::Document::offset_at`] representation. This wraps the
/// same extraction the class diagnostics use; the LSP has no equivalent
/// method.
#[must_use]
pub fn class_tokens(text: &str) -> Vec<(u32, u32, String)> {
    crate::styles::class_tokens(text)
        .into_iter()
        .map(|site| (site.token.start(), site.token.end(), site.name))
        .collect()
}

/// Returns the class token at `offset` as `(name, start, end)`.
///
/// `offset` is a UTF-8 byte offset. Returns `None` when the cursor is not
/// inside a static class token (mixed `class="card {{extra}}"` values still
/// expose `card`).
#[must_use]
pub fn class_site_at(text: &str, offset: u32) -> Option<(String, u32, u32)> {
    crate::styles::class_site_at(text, offset)
        .map(|site| (site.name, site.token.start(), site.token.end()))
}
