//! Opt-in real-engine performance budgets for the native `tsgo` path (P5).
//!
//! Budgets follow section 9.5 of `docs/plans/2026-11-17-rust-tsgo-migration.md`:
//!
//! | metric | budget |
//! | --- | --- |
//! | hot type query (`getTypeAtPosition` + `typeToString`) | p95 ≤ 200 ms |
//! | semantic + syntactic diagnostics | p95 ≤ 500 ms |
//! | hot completion (`getCompletionsAtPosition`) | p95 ≤ 200 ms |
//! | queued cancellation/skip | p95 ≤ 100 ms |
//! | static completion/structural query | p95 ≤ 50 ms (BLOCKED: no static path) |
//!
//! The test is opt-in and strictly serial. Cold start (spawn, `--version`,
//! handshake and probe) is measured and printed but excluded from the samples;
//! every metric additionally runs [`WARMUP`] untimed iterations first. The
//! wall-clock assertions assume a quiet local machine with one engine, so do
//! not run this on shared CI. Exact invocation with the pinned engine:
//!
//! ```text
//! TSGO_BIN=/tmp/opencode/tsgo-spike/extract/7.1.0-beta/package/lib/tsc \
//!   WXML_REQUIRE_REAL_ENGINE=1 \
//!   cargo test -p wxml-backend-tsgo --test perf_budget -- --nocapture --test-threads=1
//! ```
//!
//! Without `TSGO_BIN` the test prints an explicit skip and passes. With
//! `WXML_REQUIRE_REAL_ENGINE=1` a missing `TSGO_BIN` is a hard failure, and a
//! `TSGO_BIN` that is not the pinned 7.1.0-beta executable fails the session
//! identity check inside [`NativeSession::start`]; nothing is silently skipped.
//!
//! The static completion budget has no measurable subject yet: this crate only
//! implements the engine-backed completion query, so that metric is reported as
//! BLOCKED instead of being measured with a substitute. Likewise the session
//! layer exposes no cancel token for a running query, so end-to-end running
//! cancellation is BLOCKED; queued cancellation is real production code in
//! [`RpcDispatch`] (the queued request is removed before it is ever sent) and is
//! measured there.

#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use wxml_backend_tsgo::config::{MAX_IN_FLIGHT_REQUESTS, MAX_LIVE_SNAPSHOTS};
use wxml_backend_tsgo::input::{
    AnalysisInput, DocumentRevision, ProjectKey, ProjectRoot, SnapshotRevision, SourceFile,
};
use wxml_backend_tsgo::native::framing::DEFAULT_FRAME_LIMITS;
use wxml_backend_tsgo::native::rpc::RpcDispatch;
use wxml_backend_tsgo::native::session::{NativeSession, SessionOptions};
use wxml_backend_tsgo::native::strings::Utf16Position;

/// Measured samples per metric; plan section 9.5 requires at least 200.
const SAMPLES: usize = 200;
/// Untimed warm-up iterations per metric, excluding cold caches.
const WARMUP: usize = 5;

/// Hot type query budget (hover-equivalent), p95 in milliseconds.
const TYPE_QUERY_P95_MS: f64 = 200.0;
/// Combined semantic and syntactic diagnostics budget, p95 in milliseconds.
const DIAGNOSTICS_P95_MS: f64 = 500.0;
/// Hot completion budget, p95 in milliseconds.
const COMPLETION_P95_MS: f64 = 200.0;
/// Queued cancellation/skip budget, p95 in milliseconds.
const CANCEL_QUEUED_P95_MS: f64 = 100.0;
/// Static completion/structural query budget, p95 in milliseconds.
const STATIC_COMPLETION_P95_MS: f64 = 50.0;

/// Number of open/edit/close cycles for the live-snapshot growth check.
const SNAPSHOT_CYCLES: u32 = 10;

const MAIN_PATH: &str = "main.ts";
const MODEL_PATH: &str = "model.ts";
const PROJECT_KEY: &str = "perf-budget";

/// Workload with one deliberate type error and a dangling member access.
///
/// Line numbers are part of the fixture: line 16 is `u.name;` (type query) and
/// line 18 is `u.` (member completion).
const MAIN: &str = r#"import type { User } from "./model";

interface Audit {
    actor: string;
    at: number;
}

const u: User = { name: "a", age: 1 };
const active: boolean = true;
const audit: Audit = { actor: "me", at: 0 };

function describe(user: User): string {
    const suffix = active ? "!" : ".";
    return `${user.name} (${user.age})${suffix}`;
}

u.name;
u.missing;
u.
"#;

const MODEL: &str = r"export interface User {
    /** Display name. */
    name: string;
    age: number;
}
";

/// UTF-16 position of `u` in `u.name;` (line 16, column 0).
const TYPE_POSITION: Utf16Position = Utf16Position {
    line: 16,
    character: 0,
};
/// UTF-16 position directly after `u.` (line 18, column 2).
const COMPLETION_POSITION: Utf16Position = Utf16Position {
    line: 18,
    character: 2,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Removes the temporary sandbox directory when the test ends.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique(tag: &str) -> String {
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("wxml-tsgo-perf-{tag}-{}-{count}", std::process::id())
}

/// Whether the real-engine gate is hard, accepting both `1` and `true`.
fn required_engine() -> bool {
    std::env::var("WXML_REQUIRE_REAL_ENGINE")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Returns the pinned executable, or an explicit skip when the gate is off.
fn engine_executable() -> Option<PathBuf> {
    let required = required_engine();
    let Ok(bin) = std::env::var("TSGO_BIN") else {
        assert!(
            !required,
            "TSGO_BIN is required when WXML_REQUIRE_REAL_ENGINE=1; \
             set it to the pinned 7.1.0-beta lib/tsc"
        );
        eprintln!(
            "perf-budget gate skipped: set TSGO_BIN to the pinned 7.1.0-beta lib/tsc \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return None;
    };
    let executable = PathBuf::from(&bin);
    assert!(
        executable.is_file(),
        "TSGO_BIN does not point at a file: {bin}"
    );
    Some(executable)
}

/// Builds one virtual project revision.
///
/// With `cycle` set, `main.ts` is replaced by a small valid edit so every
/// update changes real source text instead of only the revision number.
fn project_input(root: &str, revision: u64, cycle: Option<u32>) -> AnalysisInput {
    let main = cycle.map_or_else(|| MAIN.to_owned(), cycle_source);
    let files = vec![
        SourceFile::new(MAIN_PATH, main, DocumentRevision::new(revision)).expect("main.ts"),
        SourceFile::new(MODEL_PATH, MODEL, DocumentRevision::new(revision)).expect("model.ts"),
    ];
    AnalysisInput::new(
        ProjectKey::new(PROJECT_KEY),
        ProjectRoot::new(root).expect("project root"),
        SnapshotRevision::new(revision),
        files,
    )
    .expect("analysis input")
}

/// One valid edit used by the open/update/release cycle loop.
fn cycle_source(cycle: u32) -> String {
    format!(
        "import type {{ User }} from \"./model\";\n\
         \n\
         const u: User = {{ name: \"a\", age: 1 }};\n\
         const cycle{cycle} = {cycle};\n\
         u.name;\n"
    )
}

/// Nearest-rank percentile over ascending samples.
fn nearest_rank(samples: &[f64], percent: usize) -> f64 {
    let rank = (samples.len() * percent).div_ceil(100).max(1);
    samples[rank - 1]
}

struct LatencyStats {
    sample_count: usize,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

impl LatencyStats {
    fn from_samples(mut samples: Vec<f64>) -> Self {
        assert_eq!(
            samples.len(),
            SAMPLES,
            "each metric must collect {SAMPLES} samples"
        );
        samples.sort_by(f64::total_cmp);
        Self {
            sample_count: samples.len(),
            median_ms: nearest_rank(&samples, 50),
            p95_ms: nearest_rank(&samples, 95),
            max_ms: samples[samples.len() - 1],
        }
    }
}

struct Measurement {
    label: &'static str,
    budget_p95_ms: f64,
    stats: LatencyStats,
}

impl Measurement {
    fn passed(&self) -> bool {
        self.stats.p95_ms <= self.budget_p95_ms
    }
}

/// Warms up, then samples one serial operation `SAMPLES` times.
fn measure(label: &'static str, budget_p95_ms: f64, mut operation: impl FnMut()) -> Measurement {
    for _ in 0..WARMUP {
        operation();
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        operation();
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    Measurement {
        label,
        budget_p95_ms,
        stats: LatencyStats::from_samples(samples),
    }
}

fn report(measurement: &Measurement, failures: &mut Vec<String>) {
    let stats = &measurement.stats;
    let verdict = if measurement.passed() { "PASS" } else { "FAIL" };
    println!(
        "[perf] {label:<21} samples={samples} p50={p50:.3}ms p95={p95:.3}ms \
         max={max:.3}ms budget_p95={budget:.0}ms {verdict}",
        label = measurement.label,
        samples = stats.sample_count,
        p50 = stats.median_ms,
        p95 = stats.p95_ms,
        max = stats.max_ms,
        budget = measurement.budget_p95_ms,
    );
    if !measurement.passed() {
        failures.push(format!(
            "{} p95 {:.3} ms exceeds the {:.0} ms budget",
            measurement.label, stats.p95_ms, measurement.budget_p95_ms
        ));
    }
}

/// Measures cancelling the oldest queued request before it is ever sent.
///
/// This is the real F1-24 queued path: the id is admitted as [`Queued`] after
/// the eight in-flight slots are occupied, then abandoned without an engine
/// round trip. A running-query cancel has no session API and stays BLOCKED.
fn measure_queued_cancel() -> f64 {
    let mut dispatch = RpcDispatch::new(DEFAULT_FRAME_LIMITS);
    for _ in 0..MAX_IN_FLIGHT_REQUESTS {
        let admission = dispatch.allocate_request().expect("in-flight request");
        assert!(!admission.is_queued());
    }
    let queued = dispatch.allocate_request().expect("queued request");
    assert!(queued.is_queued(), "the request must be queued");
    let start = Instant::now();
    dispatch
        .abandon_request(queued.id())
        .expect("abandon queued request");
    start.elapsed().as_secs_f64() * 1_000.0
}

#[test]
fn real_engine_meets_p5_performance_budgets() {
    let Some(executable) = engine_executable() else {
        return;
    };

    let sandbox = std::env::temp_dir().join(unique("sandbox"));
    std::fs::create_dir_all(&sandbox).expect("sandbox");
    let _sandbox_guard = Cleanup(sandbox.clone());
    assert!(
        std::fs::read_dir(&sandbox)
            .expect("sandbox listing")
            .next()
            .is_none(),
        "sandbox must start empty"
    );

    let project_root = format!("/tmp/opencode/{}", unique("project"));
    assert!(
        !Path::new(&project_root).exists(),
        "the virtual project root must not exist on disk: {project_root}"
    );

    let cold_start = Instant::now();
    let mut session = NativeSession::start(SessionOptions::new(executable.clone(), sandbox))
        .unwrap_or_else(|error| {
            panic!(
                "failed to start the pinned 7.1.0-beta engine at {}: {error}",
                executable.display()
            )
        });
    println!(
        "[perf] cold_start_ms={:.3} (excluded from samples)",
        cold_start.elapsed().as_secs_f64() * 1_000.0
    );

    let first = project_input(&project_root, 1, None);
    let open_start = Instant::now();
    session.open_project(&first).expect("open project");
    println!(
        "[perf] open_project_ms={:.3} live_snapshots={}",
        open_start.elapsed().as_secs_f64() * 1_000.0,
        session.live_snapshot_count()
    );
    assert_eq!(session.live_snapshot_count(), 1);

    // Sanity-probe every workload once so a fast but empty query cannot pass.
    let probe = session
        .type_query(MAIN_PATH, TYPE_POSITION)
        .expect("type query probe");
    assert!(
        probe
            .text
            .as_deref()
            .is_some_and(|text| text.contains("User")),
        "the type query workload must resolve the fixture type: {probe:?}"
    );
    let evidence = session.diagnostics(MAIN_PATH).expect("diagnostics probe");
    assert!(
        evidence
            .semantic
            .iter()
            .any(|diagnostic| diagnostic.code == 2339),
        "the diagnostics workload must contain the deliberate 2339 error"
    );
    let completions = session
        .completions(MAIN_PATH, COMPLETION_POSITION)
        .expect("completion probe")
        .expect("completion list");
    let names: Vec<&str> = completions
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert!(
        names.contains(&"name") && names.contains(&"age"),
        "the completion workload must return member entries: {names:?}"
    );
    println!(
        "[perf] workload: type_error=2339 semantic={} completions={}",
        evidence.semantic.len(),
        completions.entries.len()
    );

    let mut measurements = Vec::new();
    measurements.push(measure("type_query", TYPE_QUERY_P95_MS, || {
        session
            .type_query(MAIN_PATH, TYPE_POSITION)
            .expect("type query");
    }));
    measurements.push(measure(
        "diagnostics_unchanged_snapshot",
        DIAGNOSTICS_P95_MS,
        || {
            session.diagnostics(MAIN_PATH).expect("diagnostics");
        },
    ));
    measurements.push(measure("completion", COMPLETION_P95_MS, || {
        session
            .completions(MAIN_PATH, COMPLETION_POSITION)
            .expect("completions");
    }));
    measurements.push(measure("cancel_queued_skip", CANCEL_QUEUED_P95_MS, || {
        std::hint::black_box(measure_queued_cancel());
    }));

    println!(
        "[perf] cancel_queued_skip scope: dispatch layer only; the queued request \
         is removed before any engine send and no engine round trip is timed"
    );
    println!(
        "[perf] static_completion       BLOCKED budget_p95={STATIC_COMPLETION_P95_MS:.0}ms \
         reason=\"no static completion/structural path exists in wxml-backend-tsgo\""
    );
    println!(
        "[perf] running_cancel          BLOCKED budget_p95={CANCEL_QUEUED_P95_MS:.0}ms \
         reason=\"NativeSession exposes no cancel token and the engine cancel method is not frozen\""
    );
    println!(
        "[perf] diagnostics_unchanged_snapshot scope: repeated query on one frozen \
         snapshot; post-edit diagnostics are NOT measured"
    );
    if required_engine() {
        println!(
            "[perf] COVERAGE: PARTIAL static_completion=BLOCKED running_cancel=BLOCKED \
             diagnostics_post_edit=BLOCKED; exit 0 does not claim all 9.5 budgets"
        );
    }

    let mut failures = Vec::new();
    for measurement in &measurements {
        report(measurement, &mut failures);
    }

    // Ten open/edit/close cycles: each update must stay bounded, and each
    // release must return the ledger to zero instead of growing by one.
    let mut update_counts = Vec::new();
    let mut release_counts = Vec::new();
    let mut revision = 1_000_u64;
    for cycle in 0..SNAPSHOT_CYCLES {
        revision += 1;
        let updated = project_input(&project_root, revision, Some(cycle));
        session.update_project(&updated).expect("cycle update");
        let update_count = session.live_snapshot_count();
        assert!(
            update_count <= MAX_LIVE_SNAPSHOTS,
            "cycle {cycle}: {update_count} live snapshots exceed the cap of {MAX_LIVE_SNAPSHOTS}"
        );
        if let Some(first) = update_counts.first() {
            assert!(
                update_count <= *first,
                "cycle {cycle}: live snapshots grew from {first} to {update_count}"
            );
        }
        update_counts.push(update_count);

        session.release().expect("cycle release");
        let release_count = session.live_snapshot_count();
        assert_eq!(
            release_count, 0,
            "cycle {cycle}: release must return the live snapshot count to 0"
        );
        release_counts.push(release_count);

        revision += 1;
        let reopened = project_input(&project_root, revision, Some(cycle));
        session.open_project(&reopened).expect("cycle reopen");
    }
    session.release().expect("final release");
    assert_eq!(session.live_snapshot_count(), 0);
    println!(
        "[perf] {label:<21} cycles={cycles} update_counts={update_counts:?} \
         release_counts={release_counts:?} cap={cap} PASS",
        label = "snapshot_cycles",
        cycles = SNAPSHOT_CYCLES,
        cap = MAX_LIVE_SNAPSHOTS,
    );

    let shutdown = session.shutdown().expect("shutdown report");
    println!(
        "[perf] shutdown forced_kill={} detached_threads={} stderr_discarded_bytes={}",
        shutdown.forced_kill(),
        shutdown.threads_detached(),
        shutdown.stderr_discarded_bytes()
    );
    assert!(
        !shutdown.forced_kill(),
        "perf shutdown needed SIGKILL; graceful reclamation failed"
    );
    assert!(
        !shutdown.threads_detached(),
        "perf shutdown detached reader threads"
    );
    assert!(
        !Path::new(&project_root).exists(),
        "the engine must not write the virtual project root to the host"
    );

    assert!(
        failures.is_empty(),
        "P5 performance budgets failed:\n{}",
        failures.join("\n")
    );
}
