//! `miniprogram-api-typings@5.3.1` staging for opt-in native tests.
//!
//! The trusted loader and every native session require the canonical layout
//! `<typings_root>/miniprogram-api-typings/**`. Tests source that package from
//! `TSGO_TYPINGS_ROOT` or `TSGO_ASSET_ROOT/typings`, then copy its runtime
//! `.d.ts` closure below a private temporary root.
//!
//! A missing source returns `None`; callers must turn that into an explicit
//! skip or a hard failure, never a silent pass. `WXML_REQUIRE_REAL_ENGINE=1`
//! and `WXML_REQUIRE_REAL_ENGINE=true` remain the caller's decision.

#![allow(
    dead_code,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_pub_crate,
    clippy::unwrap_used
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Package directory name required by the trusted loader.
pub(crate) const PACKAGE: &str = "miniprogram-api-typings";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns the `miniprogram-api-typings` package from the configured roots.
///
/// `TSGO_TYPINGS_ROOT` is the directory containing the package;
/// `TSGO_ASSET_ROOT/typings` is accepted as a fallback. When neither is set the
/// returned path does not exist, so [`stage`] returns `None` and opt-in tests
/// skip.
pub(crate) fn fixture_package() -> PathBuf {
    if let Some(root) = std::env::var_os("TSGO_TYPINGS_ROOT") {
        return PathBuf::from(root).join(PACKAGE);
    }
    if let Some(root) = std::env::var_os("TSGO_ASSET_ROOT") {
        return PathBuf::from(root).join("typings").join(PACKAGE);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/miniprogram-api-typings-5.3.1")
}

/// One staged temporary typings root, removed on drop.
#[derive(Debug)]
pub(crate) struct Staged {
    root: PathBuf,
}

impl Staged {
    /// Returns the parent root that contains [`PACKAGE`].
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Returns `<root>/miniprogram-api-typings`.
    pub(crate) fn package(&self) -> PathBuf {
        self.root.join(PACKAGE)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Stages the configured typings package below a private temporary root.
///
/// Returns `None` when no configured package contains the two declaration
/// roots the loader requires. Copy failures are fatal because a present
/// package must not degrade into a skip.
pub(crate) fn stage() -> Option<Staged> {
    let fixture = fixture_package();
    if !fixture.join("index.d.ts").is_file() || !fixture.join("types/index.d.ts").is_file() {
        return None;
    }
    let root = std::env::temp_dir().join("opencode").join(format!(
        "tsgo-typings-fixture-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let package = root.join(PACKAGE);
    copy_declarations(&fixture, &package);
    assert!(
        package.join("index.d.ts").is_file() && package.join("types/index.d.ts").is_file(),
        "staged typings fixture is incomplete at {}",
        package.display()
    );
    Some(Staged { root })
}

/// Copies the `.d.ts` runtime closure, preserving nested directories.
fn copy_declarations(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("staged typings directory");
    for entry in fs::read_dir(source).expect("typings fixture directory") {
        let entry = entry.expect("typings fixture entry");
        let name = entry.file_name();
        let path = entry.path();
        let target = destination.join(&name);
        if path.is_dir() {
            copy_declarations(&path, &target);
        } else if name.to_str().is_some_and(|name| name.ends_with(".d.ts")) {
            fs::copy(&path, &target).expect("copy typings declaration");
        }
    }
}
