//! F3-02 syscall-level exec audit for the installer and the native LSP.
//!
//! This suite replaces the missing `strace -f -e trace=execve,execveat` gate
//! with an in-process `ptrace` tracer. The tracer starts the production binary
//! as its own child (`PTRACE_TRACEME`), enables
//! `PTRACE_O_TRACEEXEC|TRACEFORK|TRACEVFORK|TRACECLONE|EXITKILL` and records
//! every `execve`/`execveat` in the whole fork/clone tree, including the
//! absolute executable path from `/proc/<pid>/exe`. A PATH shim cannot observe
//! an absolute-path invocation; this tracer can.
//!
//! Coverage:
//!
//! - positive control: a binary literally named `node` is exec'd and the
//!   recorded event proves the tracer is not vacuous;
//! - `FIXTURE`: a schema v2 test package (shell fixture engine, clearly not a
//!   real release) exercises `install`, `verify`, `current` and `env`;
//! - `PASS` (`#[ignore]`, opt-in): the pinned real engine/typings tarballs are
//!   provisioned and packaged by the production packager, installed by the
//!   production installer, and queried through a real
//!   `wxml-lsp --stdio --backend native` session
//!   (`initialize`/`documentSymbol`/`hover`/`shutdown`).
//!
//! Gate behaviour: a host without ptrace/`/proc` prints `BLOCKED` and skips,
//! but `WXML_REQUIRE_EXEC_AUDIT=1|true` or `WXML_REQUIRE_REAL_ENGINE=1|true`
//! fails closed instead. Real inputs are resolved with the existing
//! `f3_acceptance`/`f3_real_release` rules: missing pinned assets skip loudly
//! unless `WXML_REQUIRE_REAL_ENGINE=1`.
//!
//! Run:
//!
//! ```text
//! cargo test -p wxml-delivery --test f3_exec_audit -- --nocapture
//!
//! TSGO_NATIVE_TARBALL=... TSGO_TYPINGS_TARBALL=... WXML_F3_LSP_BIN=... \
//! WXML_F3_INSTALL_BIN=... WXML_F3_ZED_WASM=... WXML_REQUIRE_REAL_ENGINE=1 \
//!   cargo test -p wxml-delivery --test f3_exec_audit -- \
//!     --ignored --nocapture --test-threads=1
//! ```
//!
//! Only test/script files are added by this slice; no production code, Cargo
//! manifest or workspace file is touched. The tracer needs no Cargo
//! dependency: it declares the small libc surface it uses itself.
//!
//! Debugging: `WXML_EXEC_AUDIT_TRACE=1` prints every wait/continue event and a
//! stall watchdog; `WXML_EXEC_AUDIT_FORCE_BLOCKED=1` makes the preflight report
//! a blocked host so the fail-closed gate can be exercised.

#![allow(
    unsafe_code,
    dead_code,
    unused_imports,
    unreachable_pub,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::assert_is_empty,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::missing_const_for_fn,
    clippy::similar_names,
    clippy::uninlined_format_args,
    clippy::struct_field_names,
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps,
    clippy::redundant_closure_for_method_calls,
    clippy::map_unwrap_or,
    clippy::if_not_else,
    clippy::manual_let_else,
    clippy::option_if_let_else,
    clippy::items_after_statements,
    clippy::doc_markdown,
    clippy::verbose_bit_mask
)]

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_exec_audit/tracer.rs"]
mod tracer;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_exec_audit/support.rs"]
mod support;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_exec_audit/fixture.rs"]
mod fixture;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_exec_audit/lsp.rs"]
mod lsp;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_exec_audit/guards.rs"]
mod guards;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_exec_audit/real.rs"]
mod real;

// Real-mode support is compiled from the already committed harness files so
// the audit reuses the same provisioning, packaging and installer rules. Those
// modules reference each other through these exact crate-level names.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "real_asset/support.rs"]
mod real_asset_support;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_real_release/support.rs"]
mod release;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_acceptance/support.rs"]
mod shared;

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
#[test]
fn exec_audit_is_blocked_outside_linux_x86_64() {
    let required = ["WXML_REQUIRE_EXEC_AUDIT", "WXML_REQUIRE_REAL_ENGINE"]
        .iter()
        .any(|name| {
            std::env::var(name)
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        });
    assert!(
        !required,
        "EXEC-AUDIT: BLOCKED (Linux x86_64 + ptrace required) and a hard gate is set"
    );
    println!(
        "EXEC-AUDIT: BLOCKED (this suite requires Linux x86_64 with ptrace and /proc; \
         WXML_REQUIRE_EXEC_AUDIT=1 makes this a failure)"
    );
}
