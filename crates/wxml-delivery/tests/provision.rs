//! F3-03 L2 provisioning tests (lock, extraction, cache, opt-in real engine).
//!
//! Every fixture is created under the system temporary directory and removed
//! on drop. No test downloads, executes Node/npm or writes into the repository.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names,
    clippy::redundant_pub_crate,
    dead_code,
    unused_imports
)]

#[path = "assets_lock/fixtures.rs"]
mod fixtures;

#[path = "assets_lock/engine_table.rs"]
mod engine_table;

#[path = "assets_lock/digests.rs"]
mod digests;

#[path = "assets_lock/support.rs"]
mod lock_support;

#[path = "provision/support.rs"]
mod support;

#[path = "provision/lock.rs"]
mod lock;

#[path = "provision/extract.rs"]
mod extract;

#[path = "provision/pipeline.rs"]
mod pipeline;

#[path = "provision/cache.rs"]
mod cache;

#[path = "provision/real_engine.rs"]
mod real_engine;
