//! Kill-window gates for the F3-06/F3-09 crash tests.
//!
//! A gate names one syscall boundary (`symlink(...)/entry`, `rename(...)/exit`)
//! and one expected on-disk state. The tracer only kills the tracee when both
//! the syscall path and the disk predicates match; a path match with a
//! different disk state is recorded as a rejected candidate and fails the test
//! at the end instead of silently producing a false-green sample.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::tracer::{Decision, Gate, Phase, SyscallEvent, SyscallFamily};

pub const CURRENT_LINK: &str = "current";
pub const STATE_FILE: &str = "state.json";
pub const JOURNAL_DIR: &str = "txn";
pub const VERSIONS_DIR: &str = "versions";

/// `state.json` projection used by the gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateView {
    pub active: String,
    pub previous: Option<String>,
}

/// One parsed transaction journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalView {
    pub file: String,
    pub state: String,
    pub op: String,
    pub target: String,
    pub previous: Option<String>,
}

impl JournalView {
    pub fn label(&self) -> String {
        format!(
            "{}({}<-{})",
            self.state,
            self.target,
            self.previous.as_deref().unwrap_or("null")
        )
    }
}

/// The on-disk state the parent reads before it sends SIGKILL.
#[derive(Debug, Clone, Default)]
pub struct DiskSnapshot {
    pub root: PathBuf,
    pub current_raw: Option<PathBuf>,
    pub current: Option<String>,
    pub state_bytes: Option<Vec<u8>>,
    pub state: Option<StateView>,
    pub journals: Vec<JournalView>,
    pub journal_errors: Vec<String>,
    pub versions: Vec<String>,
    pub root_temps: Vec<String>,
    pub txn_temps: Vec<String>,
}

impl DiskSnapshot {
    /// Reads `current`, `state.json`, `txn/*.json`, `versions/` and dot-temps.
    pub fn read(root: &Path) -> Self {
        let current_raw = fs::read_link(root.join(CURRENT_LINK)).ok();
        let current = current_raw.as_deref().and_then(version_from_link);
        let state_bytes = fs::read(root.join(STATE_FILE)).ok();
        let state = state_bytes.as_deref().and_then(parse_state);
        let (journals, journal_errors) = read_journals(root);
        let versions = list_dir_names(&root.join(VERSIONS_DIR));
        let root_temps = dot_temps(root);
        let txn_temps = dot_temps(&root.join(JOURNAL_DIR));
        Self {
            root: root.to_path_buf(),
            current_raw,
            current,
            state_bytes,
            state,
            journals,
            journal_errors,
            versions,
            root_temps,
            txn_temps,
        }
    }

    pub fn journal(&self) -> Option<&JournalView> {
        match self.journals.as_slice() {
            [journal] => Some(journal),
            _ => None,
        }
    }

    pub fn state_label(&self) -> String {
        match &self.state {
            Some(state) => format!(
                "active:{}/previous:{}",
                state.active,
                state.previous.as_deref().unwrap_or("null")
            ),
            None if self.state_bytes.is_some() => "unparseable".to_owned(),
            None => "absent".to_owned(),
        }
    }

    pub fn journal_label(&self) -> String {
        if self.journals.is_empty() {
            return "none".to_owned();
        }
        self.journals
            .iter()
            .map(JournalView::label)
            .collect::<Vec<_>>()
            .join("+")
    }

    pub fn summary(&self) -> String {
        format!(
            "current={} state={} journal={} versions=[{}] root-temp=[{}] txn-temp=[{}]",
            self.current.as_deref().unwrap_or("none"),
            self.state_label(),
            self.journal_label(),
            self.versions.join(","),
            self.root_temps.join(","),
            self.txn_temps.join(",")
        )
    }
}

fn version_from_link(link: &Path) -> Option<String> {
    let mut components = link.components();
    match components.next()? {
        Component::Normal(name) if name == VERSIONS_DIR => {}
        _ => return None,
    }
    match components.next()? {
        Component::Normal(name) => {
            if components.next().is_some() {
                return None;
            }
            Some(name.to_string_lossy().into_owned())
        }
        _ => None,
    }
}

fn parse_state(bytes: &[u8]) -> Option<StateView> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let active = value.get("active")?.as_str()?.to_owned();
    let previous = value
        .get("previous")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(StateView { active, previous })
}

fn read_journals(root: &Path) -> (Vec<JournalView>, Vec<String>) {
    let directory = root.join(JOURNAL_DIR);
    let Ok(entries) = fs::read_dir(&directory) else {
        return (Vec::new(), Vec::new());
    };
    let mut journals = Vec::new();
    let mut errors = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            errors.push(format!("{name}: unreadable"));
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            errors.push(format!("{name}: malformed"));
            continue;
        };
        let state = value
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let op = value
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let target = value
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let previous = value
            .get("previous")
            .and_then(Value::as_str)
            .map(str::to_owned);
        journals.push(JournalView {
            file: name.to_owned(),
            state,
            op,
            target,
            previous,
        });
    }
    journals.sort_by(|left, right| left.file.cmp(&right.file));
    (journals, errors)
}

fn list_dir_names(directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn dot_temps(directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with('.') && name.contains(".tmp-")).then_some(name)
        })
        .collect();
    names.sort();
    names
}

/// Expected on-disk state for one window.
#[derive(Debug, Clone)]
pub enum StateExpect {
    Absent,
    Active {
        active: &'static str,
        previous: Option<&'static str>,
    },
}

/// Expected single committed/staged/activated journal.
#[derive(Debug, Clone)]
pub struct JournalExpect {
    pub state: &'static str,
    pub target: &'static str,
    pub previous: Option<&'static str>,
}

/// Full precondition of a window.
#[derive(Debug, Clone)]
pub struct Expectation {
    pub current: Option<&'static str>,
    pub state: StateExpect,
    pub journal: Option<JournalExpect>,
    pub present_versions: Vec<&'static str>,
    pub absent_versions: Vec<&'static str>,
    pub no_root_temp: bool,
}

impl Expectation {
    fn check(&self, disk: &DiskSnapshot) -> Result<(), String> {
        if disk.current.as_deref() != self.current {
            return Err(format!(
                "current is {:?}, expected {:?}",
                disk.current, self.current
            ));
        }
        match &self.state {
            StateExpect::Absent => {
                if disk.state.is_some() {
                    return Err(format!(
                        "state.json must be absent, got {}",
                        disk.state_label()
                    ));
                }
                if disk.state_bytes.is_some() {
                    return Err("state.json exists but is unparseable".to_owned());
                }
            }
            StateExpect::Active { active, previous } => {
                let Some(state) = &disk.state else {
                    return Err(format!(
                        "state.json must be active:{active}/previous:{}, got {}",
                        previous.unwrap_or("null"),
                        disk.state_label()
                    ));
                };
                if state.active != *active || state.previous.as_deref() != *previous {
                    return Err(format!(
                        "state is {}, expected active:{active}/previous:{}",
                        disk.state_label(),
                        previous.unwrap_or("null")
                    ));
                }
            }
        }
        if let Some(expected) = &self.journal {
            if !disk.journal_errors.is_empty() {
                return Err(format!("journal errors: {}", disk.journal_errors.join(";")));
            }
            let Some(journal) = disk.journal() else {
                return Err(format!(
                    "expected exactly one journal {}, got {}",
                    expected.state,
                    disk.journal_label()
                ));
            };
            if journal.state != expected.state
                || journal.target != expected.target
                || journal.previous.as_deref() != expected.previous
            {
                return Err(format!(
                    "journal is {}, expected {}({}<-{})",
                    journal.label(),
                    expected.state,
                    expected.target,
                    expected.previous.unwrap_or("null")
                ));
            }
        } else if !disk.journals.is_empty() {
            return Err(format!("expected no journal, got {}", disk.journal_label()));
        }
        for version in &self.present_versions {
            if !disk.versions.iter().any(|entry| entry == version) {
                return Err(format!(
                    "versions/{version} must exist; versions=[{}]",
                    disk.versions.join(",")
                ));
            }
        }
        for version in &self.absent_versions {
            if disk.versions.iter().any(|entry| entry == version) {
                return Err(format!(
                    "versions/{version} must not exist; versions=[{}]",
                    disk.versions.join(",")
                ));
            }
        }
        if self.no_root_temp && !disk.root_temps.is_empty() {
            return Err(format!(
                "no .current.tmp-* is expected; root-temp=[{}]",
                disk.root_temps.join(",")
            ));
        }
        Ok(())
    }
}

/// Which syscall boundary and path shape identifies the window.
#[derive(Debug, Clone)]
pub enum Matcher {
    /// `rename(.current.tmp-*, current)` exit.
    RenameToCurrent,
    /// `symlink(.current.tmp-*, versions/<target>)` entry.
    SymlinkCurrentTemp,
    /// `rename(.*.tmp-*, state.json)` exit.
    RenameToState,
    /// `rename(.<id>.json.tmp-*, txn/<id>.json)` exit for one journal state.
    RenameToJournal { state: &'static str },
    /// `rename(tmp/<staged>, versions/<target>)` exit.
    RenameTree { target: &'static str },
    /// `fsync(<root directory fd>)` exit.
    FsyncRoot,
    /// Never matches; used by the tracer control test.
    Never,
}

impl Matcher {
    const fn family_phase(&self) -> Option<(SyscallFamily, Phase)> {
        match self {
            Self::RenameToCurrent | Self::RenameToState | Self::RenameToJournal { .. } => {
                Some((SyscallFamily::Rename, Phase::Exit))
            }
            Self::RenameTree { .. } => Some((SyscallFamily::Rename, Phase::Exit)),
            Self::SymlinkCurrentTemp => Some((SyscallFamily::Symlink, Phase::Entry)),
            Self::FsyncRoot => Some((SyscallFamily::Fsync, Phase::Exit)),
            Self::Never => None,
        }
    }

    /// `fsync(root)` recurs at several transaction phases; the disk predicate
    /// selects the intended one. Unique paths stay strict.
    const fn is_recurring(&self) -> bool {
        matches!(self, Self::FsyncRoot)
    }

    fn matches(
        &self,
        gate_root: &Path,
        journal_target: Option<&str>,
        event: &SyscallEvent,
    ) -> bool {
        if matches!(self, Self::FsyncRoot) {
            let fd = event.args[0];
            return fs::read_link(format!("/proc/{}/fd/{fd}", event.pid))
                .is_ok_and(|target| target == gate_root);
        }
        let Some(new_path) = event.new_path.as_deref() else {
            return false;
        };
        match self {
            Self::RenameToCurrent => {
                new_path == gate_root.join(CURRENT_LINK)
                    && event
                        .old_path
                        .as_deref()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(".current.tmp-"))
            }
            Self::SymlinkCurrentTemp => {
                let target_matches = journal_target.is_some_and(|target| {
                    event.old_path.as_deref()
                        == Some(Path::new(VERSIONS_DIR).join(target).as_path())
                });
                target_matches
                    && new_path.parent() == Some(gate_root)
                    && new_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(".current.tmp-"))
            }
            Self::RenameToState => new_path == gate_root.join(STATE_FILE),
            Self::RenameToJournal { state } => {
                let Some(parent) = new_path.parent() else {
                    return false;
                };
                if parent != gate_root.join(JOURNAL_DIR) {
                    return false;
                }
                let Some(name) = new_path.file_name().and_then(|name| name.to_str()) else {
                    return false;
                };
                if !name.ends_with(".json") || name.starts_with('.') {
                    return false;
                }
                fs::read(new_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|value| {
                        value
                            .get("state")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .is_some_and(|observed| observed == *state)
            }
            Self::RenameTree { target } => new_path == gate_root.join(VERSIONS_DIR).join(target),
            Self::FsyncRoot | Self::Never => false,
        }
    }
}

/// One observed window that satisfied both the syscall and the disk predicates.
#[derive(Debug, Clone)]
pub struct Hit {
    pub pid: i32,
    pub syscall: &'static str,
    pub phase: Phase,
    pub paths: String,
    pub disk: DiskSnapshot,
}

impl Hit {
    pub fn line(&self) -> String {
        format!(
            "pid={} gate={}({})/{} pre {}",
            self.pid,
            self.syscall,
            self.paths,
            self.phase.as_str(),
            self.disk.summary()
        )
    }
}

/// A syscall/path/disk gate that records exactly what it observed.
#[derive(Debug)]
pub struct KillGate {
    root: PathBuf,
    matcher: Matcher,
    expectation: Expectation,
    hits: Vec<Hit>,
    rejected: Vec<String>,
    skipped: Vec<String>,
    candidates: u64,
}

impl KillGate {
    /// Builds a gate rooted at a canonical install root.
    pub fn new(root: &Path, matcher: Matcher, expectation: Expectation) -> Self {
        let root = fs::canonicalize(root)
            .unwrap_or_else(|error| panic!("canonicalize gate root {}: {error}", root.display()));
        Self {
            root,
            matcher,
            expectation,
            hits: Vec::new(),
            rejected: Vec::new(),
            skipped: Vec::new(),
            candidates: 0,
        }
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    pub fn candidates(&self) -> u64 {
        self.candidates
    }

    /// Panics unless exactly one candidate matched and its disk state matched.
    pub fn assert_exactly_one_hit(&self, test: &str) {
        if !self.matcher.is_recurring() {
            assert_eq!(
                self.candidates, 1,
                "{test}: the gate matched {} syscall candidates, expected exactly one \
                 (0 means the window was missed and would be a false green)",
                self.candidates
            );
        }
        assert!(
            self.rejected.is_empty(),
            "{test}: {} syscall candidate(s) had a different disk state:\n{}",
            self.rejected.len(),
            self.rejected.join("\n")
        );
        assert_eq!(
            self.hits.len(),
            1,
            "{test}: expected exactly one kill window, got {} (candidates={}, \
             skipped={})",
            self.hits.len(),
            self.candidates,
            self.skipped.len()
        );
    }
}

impl Gate for KillGate {
    fn interested(&self, family: SyscallFamily, phase: Phase) -> bool {
        self.matcher.family_phase() == Some((family, phase))
    }

    fn evaluate(&mut self, event: &SyscallEvent) -> Decision {
        let journal_target = self
            .expectation
            .journal
            .as_ref()
            .map(|journal| journal.target);
        if !self.matcher.matches(&self.root, journal_target, event) {
            return Decision::Ignore;
        }
        self.candidates = self.candidates.saturating_add(1);
        let disk = DiskSnapshot::read(&self.root);
        match self.expectation.check(&disk) {
            Ok(()) => {
                let hit = Hit {
                    pid: event.pid,
                    syscall: event.syscall_label(),
                    phase: event.phase,
                    paths: event.paths_summary(),
                    disk,
                };
                println!("KILL-WINDOW: matched {}", hit.line());
                self.hits.push(hit);
                Decision::Kill
            }
            Err(reason) => {
                let detail = format!(
                    "{}({})/{} before-kill predicates failed: {reason}; observed {}",
                    event.syscall_label(),
                    event.paths_summary(),
                    event.phase.as_str(),
                    disk.summary()
                );
                if self.matcher.is_recurring() {
                    // A recurring path (fsync(root)) legitimately appears at
                    // several transaction phases; only the matching disk state
                    // is the window.
                    println!("KILL-WINDOW: skipped recurring stop: {detail}");
                    self.skipped.push(detail);
                    Decision::Ignore
                } else {
                    self.rejected.push(detail.clone());
                    Decision::Reject(detail)
                }
            }
        }
    }
}
