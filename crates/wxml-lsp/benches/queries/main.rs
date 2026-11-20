//! Criterion entry point for the WXML query benchmarks.
//!
//! Group layout:
//!
//! - `query/type/{root,member,local}` — engine-backed variable hover.
//! - `query/tag/{builtin,component,attribute,value}` — static metadata hover.
//! - `query/class/{extract,site,hover,completion,definition}` — class parsing
//!   and the style index.
//! - `first-query/{type,tag,class}` — cold sessions, sample size 10.
//!
//! Every sample is one full request/response round trip through the real
//! server event loop; see `docs/reference/benchmarks.md`. The type group is
//! skipped unless an installed native layout exists; it is never reported as
//! measured without one.

// The bench target is a binary-like crate: modules are private, so `pub(crate)`
// items are idiomatic here and `unreachable_pub` already forced them over
// `pub`; Criterion's closures also take message values by value.
#![allow(clippy::needless_pass_by_value, clippy::redundant_pub_crate)]

mod groups;
mod scenario;
mod session;

use std::path::{Path, PathBuf};

use criterion::Criterion;
use wxml_lsp::bench_support::backend_from_install;

fn main() {
    let fixture = scenario::Fixture::load();
    fixture.print_scale();

    let install = install_root();
    let engine = backend_from_install(&install);
    let available = match &engine {
        Ok(_) => {
            println!("native layout: {}", install.display());
            true
        }
        Err(reason) => {
            let require =
                std::env::var_os("WXML_REQUIRE_REAL_ENGINE").is_some_and(|value| value == "1");
            assert!(
                !require,
                "WXML_REQUIRE_REAL_ENGINE=1 but no native layout at {}: {reason}",
                install.display()
            );
            eprintln!(
                "SKIP query/type/* and first-query/type: no native layout at {}: {reason}",
                install.display()
            );
            eprintln!(
                "Install it with `cargo build --release -p wxml-lsp && \
                 scripts/install-native.sh --root {}`.",
                install.display()
            );
            false
        }
    };

    let mut criterion = Criterion::default().configure_from_args();
    groups::tag(&mut criterion, &fixture);
    groups::class(&mut criterion, &fixture);
    if let Ok(backend) = engine {
        groups::r#type(&mut criterion, &fixture, backend);
    }
    groups::first_query(
        &mut criterion,
        &fixture,
        available.then_some(install.as_path()),
    );
    criterion.final_summary();
}

/// Resolves `WXML_BENCH_INSTALL_ROOT`, defaulting to `$HOME/.local/share/wxml-lsp`.
fn install_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WXML_BENCH_INSTALL_ROOT") {
        return PathBuf::from(root);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    Path::new(&home).join(".local/share/wxml-lsp")
}
