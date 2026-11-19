//! F3-06/F3-09 exact process-level SIGKILL windows for the real installer.
//!
//! This suite adds a test-only `ptrace` syscall tracer
//! (`PTRACE_SYSCALL` + `PTRACE_GETREGS` + `PTRACE_PEEKDATA`) that stops the
//! production `wxml-lsp-install` process at the entry or exit of
//! `symlink`/`rename`/`renameat`/`renameat2`/`fsync`. The parent reads
//! `current`, `state.json` and `txn/*.json`, asserts the expected window, and
//! only then sends an *external* `SIGKILL`. The killed process never runs
//! cleanup code; the evidence therefore covers process crashes, not power
//! loss.
//!
//! Coverage:
//!
//! - `PASS` (opt-in, real package): F3-06 upgrade windows U1/U3/U5/U6 and
//!   F3-09 legacy pre/post-switch windows L3/L4, each followed by the real
//!   `recover`/`verify` (and an explicit retry where the target was committed
//!   but not activated);
//! - `PASS` control: a never-matching gate runs a full upgrade under the
//!   tracer without a kill, proving the gate is not vacuous;
//! - `FIXTURE`: a synthetic `mv` rename is killed at the same boundary and a
//!   never-matching control runs beside it (no real assets needed);
//! - `BLOCKED`: hosts without ptrace/`/proc` print a loud block. A hard gate
//!   (`WXML_REQUIRE_KILL_WINDOW=1`, `WXML_REQUIRE_REAL_ENGINE=1` or
//!   `WXML_REQUIRE_EXEC_AUDIT=1`) fails closed instead.
//!
//! Run:
//!
//! ```text
//! cargo test -p wxml-delivery --test f3_kill_window -- --nocapture
//!
//! scripts/f3-kill-window.sh
//! ```
//!
//! Only test/script/doc files are added by this slice; no production code,
//! Cargo manifest or workspace file is touched. Real-mode support is compiled
//! from the already committed harness files so provisioning, packaging and
//! installer rules stay identical to F3-02/F3-06/F3-09.

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
    clippy::verbose_bit_mask,
    clippy::unused_self,
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    clippy::single_match_else,
    clippy::manual_string_new,
    clippy::format_push_string,
    clippy::needless_raw_string_hashes,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::redundant_clone,
    clippy::assigning_clones
)]

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_kill_window/tracer.rs"]
mod tracer;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_kill_window/gate.rs"]
mod gate;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_kill_window/support.rs"]
mod support;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "f3_kill_window/real.rs"]
mod real;

// Real-mode support is compiled from the already committed harness files so
// the kill windows reuse the same provisioning, packaging and installer rules.
// Those modules reference each other through these exact crate-level names.
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
fn kill_window_is_blocked_outside_linux_x86_64() {
    let required = ["WXML_REQUIRE_KILL_WINDOW", "WXML_REQUIRE_REAL_ENGINE"]
        .iter()
        .any(|name| {
            std::env::var(name)
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        });
    assert!(
        !required,
        "F3-06/F3-09: KILL-WINDOW BLOCKED (Linux x86_64 + ptrace required) and a hard gate is set"
    );
    println!(
        "F3-EVIDENCE: F3-06/F3-09 BLOCKED :: this suite requires Linux x86_64 with ptrace and \
         /proc; WXML_REQUIRE_KILL_WINDOW=1 makes this a failure"
    );
}
