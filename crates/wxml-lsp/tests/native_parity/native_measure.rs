//! Bounded `/proc` sampling, percentile summaries and report writing for the
//! native performance and stability gates (Linux only).
//!
//! Sampling is deliberately bounded: at most [`MAX_TREE_PIDS`] processes per
//! tree sample, at most [`MAX_POINTS`] retained timeline points, and at most one
//! `/proc/<pid>/status` + `/proc/<pid>/stat` read per process per sample. No
//! external process (`ps`, `top`, `node`) is started.
//!
//! Every read failure is treated as "process gone" and produces a zero sample;
//! the gates fail on a zero baseline instead of silently skipping the metric.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Maximum processes inspected in one server tree sample.
pub(crate) const MAX_TREE_PIDS: usize = 64;
/// Maximum timeline points retained per resource series.
pub(crate) const MAX_POINTS: usize = 32;
/// Bounded wait for a killed/leaked PID to disappear.
pub(crate) const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// Nearest-rank percentile summary for one latency metric.
#[derive(Debug, Clone)]
pub(crate) struct Stats {
    /// Number of measured samples.
    pub(crate) count: usize,
    /// Nearest-rank p50.
    pub(crate) p50_ms: f64,
    /// Nearest-rank p95.
    pub(crate) p95_ms: f64,
    /// Nearest-rank p99.
    pub(crate) p99_ms: f64,
    /// Maximum sample.
    pub(crate) max_ms: f64,
    /// Arithmetic mean.
    pub(crate) mean_ms: f64,
}

impl Stats {
    /// Sorts a copy of the samples and computes nearest-rank percentiles.
    pub(crate) fn from_samples(samples: &[f64]) -> Self {
        assert!(
            !samples.is_empty(),
            "a metric must have at least one sample"
        );
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let mean = sorted.iter().sum::<f64>() / as_f64(sorted.len());
        Self {
            count: sorted.len(),
            p50_ms: percentile(&sorted, 50, 100),
            p95_ms: percentile(&sorted, 95, 100),
            p99_ms: percentile(&sorted, 99, 100),
            max_ms: *sorted.last().expect("non-empty samples"),
            mean_ms: mean,
        }
    }

    /// Returns the report projection with millisecond values rounded to 3 dp.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "count": self.count,
            "p50_ms": round(self.p50_ms),
            "p95_ms": round(self.p95_ms),
            "p99_ms": round(self.p99_ms),
            "max_ms": round(self.max_ms),
            "mean_ms": round(self.mean_ms),
        })
    }
}

/// One snapshot of the server process tree.
#[derive(Debug, Clone, Default)]
pub(crate) struct TreeSample {
    /// Root plus bounded descendants.
    pub(crate) pids: Vec<u32>,
    /// Summed `VmRSS` in KiB.
    pub(crate) rss_kib: u64,
    /// Summed thread count.
    pub(crate) threads: usize,
    /// Summed user+system CPU ticks.
    pub(crate) cpu_ticks: u64,
}

impl TreeSample {
    /// Returns the report projection.
    pub(crate) fn to_json(&self, label: &str) -> Value {
        json!({
            "label": label,
            "pids": self.pids,
            "processes": self.pids.len(),
            "rss_kib": self.rss_kib,
            "threads": self.threads,
            "cpu_ticks": self.cpu_ticks,
        })
    }
}

fn as_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("sample count fits u32"))
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn percentile(sorted: &[f64], numerator: usize, denominator: usize) -> f64 {
    let rank = (numerator * sorted.len()).div_ceil(denominator);
    let index = rank.clamp(1, sorted.len()) - 1;
    sorted[index]
}

/// Returns `VmRSS` in KiB for one PID.
pub(crate) fn rss_kib(pid: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

/// Returns the thread count for one PID.
pub(crate) fn threads(pid: u32) -> Option<usize> {
    let entries = fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    Some(entries.filter_map(Result::ok).count())
}

/// Returns user+system CPU ticks for one PID.
///
/// `/proc/<pid>/stat` fields after the final `)` start at field 3 (state), so
/// fields 14 (`utime`) and 15 (`stime`) are indexes 11 and 12.
pub(crate) fn cpu_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let fields = stat
        .get(close + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let user = fields.get(11)?.parse::<u64>().ok()?;
    let system = fields.get(12)?.parse::<u64>().ok()?;
    Some(user + system)
}

/// Returns true while `/proc/<pid>` names a runnable process.
///
/// A zombie still has a `/proc` entry until its parent reaps it; for leak
/// accounting it is already dead and must not make the gate wait or fail.
pub(crate) fn process_alive(pid: u32) -> bool {
    let Some(state) = process_state(pid) else {
        return false;
    };
    !matches!(state, 'Z' | 'X' | 'x')
}

/// Returns the single-letter process state from `/proc/<pid>/stat`.
fn process_state(pid: u32) -> Option<char> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    stat.get(close + 1..)?
        .split_whitespace()
        .next()?
        .chars()
        .next()
}

/// Reads the executable behind one PID.
pub(crate) fn process_exe(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// Samples one process tree with a hard process cap.
///
/// Returns zeroes when the root is already gone; callers treat a zero baseline
/// as a failure rather than a skip.
pub(crate) fn sample_tree(root: u32) -> TreeSample {
    let mut pids = vec![root];
    pids.extend(descendants(root));
    pids.sort_unstable();
    pids.dedup();
    pids.truncate(MAX_TREE_PIDS);

    let mut sample = TreeSample {
        pids: pids.clone(),
        ..TreeSample::default()
    };
    for pid in pids {
        sample.rss_kib += rss_kib(pid).unwrap_or(0);
        sample.threads += threads(pid).unwrap_or(0);
        sample.cpu_ticks += cpu_ticks(pid).unwrap_or(0);
    }
    sample
}

/// Returns engine descendants of `root` whose executable lives under
/// `package_root`.
pub(crate) fn engine_children(root: u32, package_root: &Path) -> Vec<u32> {
    descendants(root)
        .into_iter()
        .filter(|pid| {
            process_exe(*pid).is_some_and(|executable| executable.starts_with(package_root))
        })
        .collect()
}

/// Sends `SIGKILL` to one PID without a shell and without `PATH` dependence.
pub(crate) fn kill_process(pid: u32) {
    let status = Command::new("/bin/kill")
        .arg("-9")
        .arg(pid.to_string())
        .status()
        .unwrap_or_else(|error| panic!("kill -9 {pid}: {error}"));
    assert!(status.success(), "kill -9 {pid} exited with {status}");
}

/// Waits until every PID is gone, or fails with the survivors.
pub(crate) fn wait_gone(pids: &[u32], context: &str) {
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        let alive = pids
            .iter()
            .copied()
            .filter(|pid| process_alive(*pid))
            .collect::<Vec<_>>();
        if alive.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{context}: PIDs still alive after {REAP_TIMEOUT:?}: {alive:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// Returns every descendant PID of `root` using `/proc` (Linux only).
pub(crate) fn descendants(root: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) || seen.len() > MAX_TREE_PIDS {
            continue;
        }
        let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let children = Path::new("/proc")
                .join(pid.to_string())
                .join("task")
                .join(task.file_name())
                .join("children");
            let Ok(text) = fs::read_to_string(&children) else {
                continue;
            };
            for child in text
                .split_whitespace()
                .filter_map(|text| text.parse::<u32>().ok())
            {
                found.push(child);
                queue.push(child);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

/// Returns the growth of `after` over `baseline` in percent.
pub(crate) fn growth_percent(baseline: u64, after: u64) -> f64 {
    if baseline == 0 {
        return if after == 0 { 0.0 } else { f64::INFINITY };
    }
    (u64_to_f64(after) - u64_to_f64(baseline)) / u64_to_f64(baseline) * 100.0
}

/// Returns the median of a non-empty `u64` series.
pub(crate) fn median(values: &[u64]) -> u64 {
    assert!(!values.is_empty(), "median needs at least one value");
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// Converts a `/proc` counter (RSS KiB / CPU ticks) without precision loss.
pub(crate) fn u64_to_f64(value: u64) -> f64 {
    f64::from(u32::try_from(value).expect("/proc counter fits u32"))
}

/// Formats the elapsed time in milliseconds.
pub(crate) fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// Records one latency assertion failure while printing the measured value.
pub(crate) fn check_p95(failures: &mut Vec<String>, metric: &str, stats: &Stats, limit_ms: f64) {
    let pass = stats.p95_ms <= limit_ms;
    println!(
        "threshold {metric}_p95: {:.3} ms <= {limit_ms:.3} ms -> {pass}",
        stats.p95_ms
    );
    if !pass {
        failures.push(format!(
            "{metric}_p95: {:.3} ms > {limit_ms:.3} ms",
            stats.p95_ms
        ));
    }
}

/// Returns the current `HEAD` without failing when git is unavailable.
pub(crate) fn git_head(root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |value| value.trim().to_owned())
}

/// Writes one evidence report outside the repository and returns its path.
///
/// The default is `/tmp/opencode/wxml-native-<name>.json`; the environment
/// variable is only an output-path override and never selects a skip. A full or
/// unwritable temporary filesystem prints a warning instead of turning a passing
/// latency/resource run into a false gate failure.
pub(crate) fn write_report(variable: &str, name: &str, report: &Value) -> PathBuf {
    let path = std::env::var_os(variable).map_or_else(
        || PathBuf::from(format!("/tmp/opencode/wxml-native-{name}.json")),
        PathBuf::from,
    );
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            eprintln!(
                "WARNING: native gate report directory {} was not created: {error}",
                parent.display()
            );
            return path;
        }
    }
    let bytes = serde_json::to_vec_pretty(report).expect("report JSON");
    if let Err(error) = fs::write(&path, bytes) {
        eprintln!(
            "WARNING: native gate report was not written to {}: {error}",
            path.display()
        );
    }
    path
}
