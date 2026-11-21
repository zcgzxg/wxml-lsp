//! Opt-in in-process type-query performance budgets for the native `tsrs`
//! session (P5).
//!
//! Budgets are frozen from the historical migration plan (section 9.5); the
//! plan is available at tag `pre-slim-20261120`:
//!
//! | metric | budget |
//! | --- | --- |
//! | hot type query (`getTypeAtPosition` + `typeToString`) | p95 ≤ 200 ms |
//! | diagnostics after an edit (stop-typing) | p95 ≤ 500 ms |
//! | repeated diagnostics on an unchanged snapshot | p95 ≤ 500 ms (informational) |
//! | hot completion (`getCompletionsAtPosition`) | p95 ≤ 200 ms |
//! | queued cancellation/skip | p95 ≤ 100 ms |
//! | static completion/structural query | p95 ≤ 50 ms (BLOCKED: no static path) |
//!
//! The engine runs in-process: there is no executable, frame pipe or stderr
//! reader to measure. Cold start (`NativeSession::start` plus handshake) is
//! measured and printed but excluded from the samples; every metric
//! additionally runs [`WARMUP`] untimed iterations first. The wall-clock
//! assertions assume a quiet local machine, so run this strictly serial and do
//! not run it on shared CI.
//!
//! The `diagnostics_after_edit` row is the plan's stop-typing metric: every
//! sample first commits a real `updateProject` edit whose text and line number
//! change, then times `getSemanticDiagnostics` + `getSyntacticDiagnostics` on
//! `main.ts`, and rejects any answer whose 2339 diagnostic does not name the
//! revision marker just written. The unchanged-snapshot row stays as
//! informational context only; it is not the sole evidence for the 500 ms
//! budget.
//!
//! ```text
//! TSRS_TYPINGS_ROOT=$HOME/.local/share/wxml-lsp/typings/miniprogram-api-typings \
//! WXML_REQUIRE_REAL_ENGINE=1 \
//!   cargo test -p wxml-backend-tsrs --test perf_budget -- --nocapture --test-threads=1
//! ```
//!
//! The gate is typings-based: the test runs when `TSRS_TYPINGS_ROOT` (or the
//! staged fixture) provides a verified `miniprogram-api-typings` package, and
//! skips explicitly otherwise. `WXML_REQUIRE_REAL_ENGINE=1` turns missing or
//! invalid typings into a hard failure. The TypeScript standard library is
//! bundled with the engine, so no `lib.d.ts` tree is read.
//!
//! The static completion budget has no measurable subject yet: this crate only
//! implements the engine-backed completion query, so that metric is reported as
//! BLOCKED instead of being measured with a substitute. Likewise the session
//! layer exposes no cancel token for a running or queued query, so both
//! cancellation metrics are reported as BLOCKED.

#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use wxml_backend_tsrs::config::MAX_LIVE_SNAPSHOTS;
use wxml_backend_tsrs::input::{
    AnalysisInput, DocumentRevision, MINIPROGRAM_TYPINGS, ProjectKey, ProjectRoot,
    SnapshotRevision, SourceFile, TypeAssets,
};
use wxml_backend_tsrs::native::session::{DiagnosticsEvidence, NativeSession, SessionOptions};
use wxml_backend_tsrs::native::strings::Utf16Position;

#[path = "support/typings_fixture.rs"]
mod typings_fixture;

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
    format!("wxml-tsrs-perf-{tag}-{}-{count}", std::process::id())
}

/// Whether the real-engine gate is hard, accepting both `1` and `true`.
fn required_engine() -> bool {
    std::env::var("WXML_REQUIRE_REAL_ENGINE")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Normalizes `TSRS_TYPINGS_ROOT`, accepting either the parent root or the
/// `miniprogram-api-typings` package directory itself.
fn normalize_typings_root(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some(MINIPROGRAM_TYPINGS) {
        path.parent()
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    } else {
        path.to_path_buf()
    }
}

/// A validated typings root plus the staged copy that keeps it alive.
struct RealTypings {
    _root: PathBuf,
    _staged: Option<typings_fixture::Staged>,
}

/// Resolves and verifies the pinned typings, or skips explicitly.
fn prepare_real_typings() -> Option<RealTypings> {
    let required = required_engine();
    let (root, staged) = if let Some(raw) = std::env::var_os("TSRS_TYPINGS_ROOT") {
        (normalize_typings_root(Path::new(&raw)), None)
    } else if let Some(staged) = typings_fixture::stage() {
        (staged.root().to_path_buf(), Some(staged))
    } else {
        assert!(
            !required,
            "TSRS_TYPINGS_ROOT is required when WXML_REQUIRE_REAL_ENGINE=1; \
             point it at miniprogram-api-typings or its parent directory"
        );
        eprintln!(
            "perf-budget test skipped: set TSRS_TYPINGS_ROOT to \
             miniprogram-api-typings or its parent directory \
             (or WXML_REQUIRE_REAL_ENGINE=1 to make this a hard gate)"
        );
        return None;
    };
    if let Err(error) = TypeAssets::load_pinned_typings(&root) {
        assert!(
            !required,
            "invalid typings under {} when WXML_REQUIRE_REAL_ENGINE=1: {error}",
            root.display()
        );
        eprintln!(
            "perf-budget test skipped: cannot verify typings under {}: {error}",
            root.display()
        );
        return None;
    }
    Some(RealTypings {
        _root: root,
        _staged: staged,
    })
}

/// Builds one virtual project revision.
///
/// With `cycle` set, `main.ts` is replaced by a small valid edit so every
/// update changes real source text instead of only the revision number.
fn project_input(root: &str, revision: u64, cycle: Option<u32>) -> AnalysisInput {
    let main = cycle.map_or_else(|| MAIN.to_owned(), cycle_source);
    project_input_with_main(root, revision, main)
}

/// Builds one virtual project revision from an explicit `main.ts` text.
fn project_input_with_main(root: &str, revision: u64, main: String) -> AnalysisInput {
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

/// One valid edit used by the post-edit diagnostics workload.
///
/// The text stays parseable but keeps deliberate property errors: the generic
/// `u.missing` error and a per-revision `u.cycle{edit}` error whose diagnostic
/// text names the current edit. Both the edited line and its line number move
/// with `edit` (0–2 padding lines), so an answer computed from a stale snapshot
/// fails the evidence check.
fn edited_source(edit: u64) -> String {
    let padding = "\n".repeat((edit % 3) as usize);
    format!(
        "import type {{ User }} from \"./model\";\n\
         \n\
         const u: User = {{ name: \"a\", age: 1 }};\n\
         const cycle{edit}: number = {edit};\n\
         {padding}u.name;\n\
         u.missing;\n\
         u.cycle{edit};\n"
    )
}

/// Commits one real `updateProject` edit and returns the new `main.ts` text.
fn apply_edit(session: &mut NativeSession, root: &str, revision: &mut u64) -> String {
    *revision += 1;
    let source = edited_source(*revision);
    let input = project_input_with_main(root, *revision, source.clone());
    if let Err(error) = session.update_project(&input) {
        panic!("post-edit update at revision {revision} failed: {error}");
    }
    source
}

/// Rejects a stale, empty or unrelated answer for an edited revision.
///
/// The 2339 diagnostic for `u.cycle{revision}` must name the revision marker:
/// a cached answer from the previous snapshot cannot satisfy it.
fn assert_edited_diagnostics(evidence: &DiagnosticsEvidence, revision: u64) {
    let marker = format!("cycle{revision}");
    assert!(
        !evidence.semantic.is_empty(),
        "revision {revision} returned no semantic diagnostics: {evidence:?}"
    );
    assert!(
        evidence
            .semantic
            .iter()
            .any(|diagnostic| diagnostic.code == 2339 && diagnostic.text.contains(&marker)),
        "revision {revision} diagnostics must reflect the edited text ({marker}): {evidence:?}"
    );
}

/// Measures diagnostics on a revision that was just edited.
///
/// The `updateProject` request itself is outside the timed region, but it is a
/// real edit committed for every sample, so the timed query is the stop-typing
/// metric instead of a cache hit on one frozen snapshot. Each answer must still
/// name the revision marker just written, which fails a stale or fast-empty
/// result.
fn measure_diagnostics_after_edit(
    session: &mut NativeSession,
    root: &str,
    base_revision: u64,
) -> Measurement {
    let mut revision = base_revision;
    let mut previous = String::new();
    let mut edited = |session: &mut NativeSession| -> f64 {
        let source = apply_edit(session, root, &mut revision);
        assert_ne!(
            source, previous,
            "every post-edit revision must change the main.ts text"
        );
        previous = source;
        let start = Instant::now();
        let evidence = session
            .diagnostics(MAIN_PATH)
            .expect("diagnostics after edit");
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        assert_edited_diagnostics(&evidence, revision);
        elapsed_ms
    };
    for _ in 0..WARMUP {
        edited(session);
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        samples.push(edited(session));
    }
    Measurement {
        label: "diagnostics_after_edit",
        budget_p95_ms: DIAGNOSTICS_P95_MS,
        stats: LatencyStats::from_samples(samples),
    }
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
/// The former dispatch queue no longer exists: `tsrs_api::Session` executes one
/// caller-supplied request at a time and exposes no cancel token, so both
/// running and queued cancellation are reported as BLOCKED.

#[test]
fn real_engine_meets_p5_performance_budgets() {
    let Some(_typings) = prepare_real_typings() else {
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
    let mut session = NativeSession::start(SessionOptions::new(PathBuf::new(), sandbox))
        .unwrap_or_else(|error| panic!("failed to start the in-process engine: {error}"));
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
    // The stop-typing metric runs last on the original fixture: every sample
    // commits a new revision, so fixed type/completion positions must be
    // sampled before the snapshot text moves.
    measurements.push(measure_diagnostics_after_edit(
        &mut session,
        &project_root,
        1,
    ));

    println!(
        "[perf] cancel_queued_skip     BLOCKED budget_p95={CANCEL_QUEUED_P95_MS:.0}ms \
         reason=\"the in-process session exposes no queued-request queue\""
    );
    println!(
        "[perf] static_completion       BLOCKED budget_p95={STATIC_COMPLETION_P95_MS:.0}ms \
         reason=\"no static completion/structural path exists in wxml-backend-tsrs\""
    );
    println!(
        "[perf] running_cancel          BLOCKED budget_p95={CANCEL_QUEUED_P95_MS:.0}ms \
         reason=\"NativeSession exposes no cancel token and the engine cancel method is not frozen\""
    );
    println!(
        "[perf] diagnostics_unchanged_snapshot scope: repeated query on one frozen \
         snapshot; informational context, not the stop-typing evidence"
    );
    println!(
        "[perf] diagnostics_after_edit scope: each sample commits a real updateProject \
         edit outside the timed region, then times diagnostics on the new revision"
    );
    if required_engine() {
        println!(
            "[perf] COVERAGE: PARTIAL static_completion=BLOCKED running_cancel=BLOCKED; \
             exit 0 does not claim all 9.5 budgets"
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

    session.shutdown().expect("shutdown");
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
