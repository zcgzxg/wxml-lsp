//! F3-06/F3-09 process-level SIGKILL windows against the real installer.
//!
//! Each `#[ignore]` test installs a real N1 (or a legacy schema-v1 fixture),
//! starts the production `wxml-lsp-install` under a syscall-stop tracer, kills
//! it with an external `SIGKILL` at one exact `symlink`/`rename` boundary,
//! asserts the on-disk pre-state the parent observed, then runs the real
//! `recover`/`verify`/retry commands. Nothing here is a production failpoint:
//! the process is killed from outside while it is stopped in the kernel.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use wxml_delivery::manifest::set_mode;

use crate::gate::{DiskSnapshot, Expectation, JournalExpect, KillGate, Matcher, StateExpect};
use crate::shared::{
    LEGACY_VERSION, TestDir, Tripwires, VERSION_N1, VERSION_N2, assert_contains,
    assert_current_verified, assert_journal_empty, assert_legacy_current, assert_marker_absent,
    assert_state, assert_success, assert_tmp_empty, build_legacy_release, clear_marker,
    install_legacy, install_n1, installed_fingerprint, installer_bin, legacy_fingerprint,
    open_root, private_root, run_installer, shared_channel, state_bytes, upgrade_n2, workspace,
};
use crate::support;

const REAL_SKIP: &str = "pinned real assets are absent; set \
    TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL and WXML_F3_LSP_BIN, or run \
    scripts/f3-kill-window.sh";

struct Fixture {
    base: TestDir,
    work: PathBuf,
    tripwires: Tripwires,
    install_bin: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let base = TestDir::new(label);
        let (work, _home, _tmp) = workspace(&base);
        let tripwires = Tripwires::install(base.path());
        let install_bin = fs::canonicalize(installer_bin()).expect("canonical installer binary");
        Self {
            base,
            work,
            tripwires,
            install_bin,
        }
    }

    fn root(&self, name: &str) -> PathBuf {
        private_root(&self.base, name)
    }

    /// Runs the installer under the gate, then asserts a single exact SIGKILL.
    fn kill(
        &self,
        label: &str,
        root: &Path,
        args: &[String],
        matcher: Matcher,
        expectation: Expectation,
    ) -> KillGate {
        let mut gate = KillGate::new(root, matcher, expectation);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let outcome = support::traced_installer(
            label,
            &self.install_bin,
            &refs,
            &self.tripwires,
            &self.work,
            &mut gate,
        );
        support::print_outcome(label, &outcome);
        gate.assert_exactly_one_hit(label);
        support::assert_killed(label, &outcome);
        self.tripwires.assert_not_executed();
        gate
    }
}

fn args_upgrade(root: &Path, channel: &Path) -> Vec<String> {
    vec![
        "upgrade".to_owned(),
        "--root".to_owned(),
        root.display().to_string(),
        "--from-dir".to_owned(),
        channel.display().to_string(),
        "--offline".to_owned(),
    ]
}

fn args_install(root: &Path, channel: &Path, version: &str) -> Vec<String> {
    vec![
        "install".to_owned(),
        "--root".to_owned(),
        root.display().to_string(),
        "--from-dir".to_owned(),
        channel.display().to_string(),
        "--version".to_owned(),
        version.to_owned(),
        "--offline".to_owned(),
    ]
}

fn args_verify(root: &Path) -> Vec<String> {
    vec![
        "verify".to_owned(),
        "--root".to_owned(),
        root.display().to_string(),
    ]
}

fn run_args(
    install_bin: &Path,
    args: &[String],
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> Output {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_installer(install_bin, &refs, tripwires, work, label)
}

fn expect_current_verified(root: &Path, version: &str) {
    let files = assert_current_verified(root, version);
    assert!(files > 0, "current {version} verified with zero files");
}

fn assert_state_bytes(root: &Path, expected: &[u8], label: &str) {
    let actual = state_bytes(root);
    assert_eq!(
        actual.as_deref(),
        Some(expected),
        "{label}: state.json bytes changed"
    );
}

fn assert_single_journal(root: &Path, state: &str, target: &str, previous: Option<&str>) {
    let disk = DiskSnapshot::read(root);
    assert_eq!(
        disk.journals.len(),
        1,
        "expected one journal, got {}",
        disk.journal_label()
    );
    let journal = &disk.journals[0];
    assert_eq!(journal.state, state);
    assert_eq!(journal.target, target);
    assert_eq!(journal.previous.as_deref(), previous);
}

fn assert_no_state(root: &Path, label: &str) {
    assert_eq!(
        state_bytes(root),
        None,
        "{label}: state.json must not exist"
    );
}

fn commit_evidence(f3: &str, gate: &KillGate, detail: impl std::fmt::Display) {
    support::evidence(
        f3,
        "PASS",
        format!(
            "real CLI sig=9 kill=({}); {}",
            support::hit_detail(gate),
            detail
        ),
    );
}

fn sha256_for_log(path: &Path) -> String {
    wxml_delivery::manifest::sha256_file(path).expect("binary sha256")
}

/// F3-06's provenance condition: N2 must be an independently built release,
/// not the N1 binary with a retargeted version constant.
fn require_real_n2(channel: &crate::shared::SharedChannel, id: &str) -> bool {
    if channel.n2_is_real {
        return true;
    }
    support::skip(
        "F3-06",
        format!(
            "{id}: N2 is the retargeted N1 fixture; set {} to a channel built by two real \
             scripts/build-release.sh runs",
            crate::shared::REAL_CHANNEL_ENV
        ),
    );
    false
}

// ---------------------------------------------------------------------------
// Capability and fixture-only control
// ---------------------------------------------------------------------------

#[test]
fn f3_kill_window_tracer_preflight() {
    if support::ensure_tracer("F3-06/F3-09") {
        support::evidence(
            "F3-06/F3-09",
            "TRACER-PASS",
            "ptrace syscall-stop preflight on /bin/true observed at least one syscall stop; \
             the gate is not vacuous",
        );
    }
}

#[test]
fn f3_kill_window_real_mode_is_opt_in() {
    support::skip(
        "F3-06/F3-09",
        "the real CLI SIGKILL windows are `#[ignore]` opt-in; run \
         scripts/f3-kill-window.sh or `cargo test -p wxml-delivery --test f3_kill_window -- \
         --ignored --nocapture --test-threads=1`",
    );
}

#[test]
fn f3_kill_window_fixture_rename_kill_and_control() {
    if !support::ensure_tracer("FIXTURE") {
        return;
    }
    for program in ["/bin/sh", "/bin/ln", "/bin/mv"] {
        if !Path::new(program).is_file() {
            support::skip("FIXTURE", format!("{program} is missing"));
            return;
        }
    }
    let base = TestDir::new("f3-kill-fixture");
    let script = base.path().join("fixture.sh");
    fs::write(
        &script,
        b"#!/bin/sh\nset -e\nroot=\"$1\"\nmkdir -p \"$root/versions/9.9.9\"\n\
          rm -f \"$root/current\" \"$root/.current.tmp-fixture\"\n\
          ln -s versions/9.9.9 \"$root/.current.tmp-fixture\"\n\
          exec /bin/mv \"$root/.current.tmp-fixture\" \"$root/current\"\n",
    )
    .expect("write fixture script");
    set_mode(&script, 0o755).expect("fixture script mode");

    let run_fixture = |name: &str, matcher: Matcher, expectation: Expectation| {
        let root = private_root(&base, name);
        let mut gate = KillGate::new(&root, matcher, expectation);
        let (stdout_path, stderr_path) = (
            base.path().join(format!("{name}.stdout")),
            base.path().join(format!("{name}.stderr")),
        );
        let spec = crate::tracer::Spec::new("/bin/sh", base.path())
            .args([script.display().to_string(), root.display().to_string()])
            .envs([("PATH", "/usr/bin:/bin")])
            .output_paths(stdout_path, stderr_path);
        let outcome = crate::tracer::run(&spec, &mut gate, Duration::from_secs(30))
            .expect("trace the fixture script");
        support::print_outcome(name, &outcome);
        (root, gate, outcome)
    };

    let (root, gate, outcome) = run_fixture(
        "kill",
        Matcher::RenameToCurrent,
        Expectation {
            current: Some("9.9.9"),
            state: StateExpect::Absent,
            journal: None,
            present_versions: vec!["9.9.9"],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );
    gate.assert_exactly_one_hit("FIXTURE kill");
    support::assert_killed("FIXTURE kill", &outcome);
    let disk = DiskSnapshot::read(&root);
    assert_eq!(
        disk.current.as_deref(),
        Some("9.9.9"),
        "the fixture rename completed before the exit-stop kill"
    );
    support::evidence(
        "F3-06/F3-09",
        "FIXTURE",
        format!(
            "synthetic `mv` rename kill at exit: gate={}; post current={}",
            support::hit_detail(&gate),
            disk.current.as_deref().unwrap_or("none")
        ),
    );

    let (root, gate, outcome) = run_fixture(
        "control",
        Matcher::Never,
        Expectation {
            current: None,
            state: StateExpect::Absent,
            journal: None,
            present_versions: vec![],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );
    assert!(
        outcome.exit.success(),
        "control run must finish without a kill: exit={:?} stderr={}",
        outcome.exit,
        outcome.stderr_text()
    );
    assert!(
        !outcome.killed,
        "the never-matching gate killed the control"
    );
    assert_eq!(gate.candidates(), 0, "the never-matching gate matched");
    assert!(
        outcome.stops > 0,
        "the control run recorded no syscall stops"
    );
    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some("9.9.9"));
    support::evidence(
        "F3-06/F3-09",
        "FIXTURE",
        format!(
            "control: never-matching gate completed the same fixture without a kill; \
             stops={} syscalls={} current={}",
            outcome.stops,
            outcome.syscall_summary(),
            disk.current.as_deref().unwrap_or("none")
        ),
    );
}

// ---------------------------------------------------------------------------
// F3-06: real N1 -> N2 upgrade
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_06_real_cli_sigkill_upgrade_before_switch() {
    if !support::ensure_tracer("F3-06") {
        return;
    }
    let Some(channel) = support::real_channel("f3_06_kill_before_switch") else {
        support::skip("F3-06", format!("U3: {REAL_SKIP}"));
        return;
    };
    if !require_real_n2(channel, "U3") {
        return;
    }
    support::evidence(
        "F3-06",
        "PROVENANCE",
        format!(
            "independent real builds: N1 archive={} lsp={}; N2 archive={} lsp={}",
            channel.n1.archive_sha256,
            sha256_for_log(&channel.n1.release_dir.join("bin/wxml-lsp")),
            channel.n2.archive_sha256,
            sha256_for_log(&channel.n2.release_dir.join("bin/wxml-lsp")),
        ),
    );
    let fixture = Fixture::new("f3-kill-u3");
    let root = fixture.root("root");
    assert_success(
        &install_n1(
            &fixture.install_bin,
            channel,
            &root,
            &fixture.tripwires,
            &fixture.work,
        ),
        "U3 baseline N1",
    );
    let n1_fingerprint = installed_fingerprint(&root, VERSION_N1);
    let state_before = state_bytes(&root).expect("N1 state.json");

    let gate = fixture.kill(
        "F3-06/U3",
        &root,
        &args_upgrade(&root, &channel.channel),
        Matcher::SymlinkCurrentTemp,
        Expectation {
            current: Some(VERSION_N1),
            state: StateExpect::Active {
                active: VERSION_N1,
                previous: None,
            },
            journal: Some(JournalExpect {
                state: "committed",
                target: VERSION_N2,
                previous: Some(VERSION_N1),
            }),
            present_versions: vec![VERSION_N1, VERSION_N2],
            absent_versions: vec![],
            no_root_temp: true,
        },
    );

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(VERSION_N1));
    assert_state_bytes(&root, &state_before, "U3 post-kill");
    assert_single_journal(&root, "committed", VERSION_N2, Some(VERSION_N1));
    assert_eq!(
        installed_fingerprint(&root, VERSION_N1),
        n1_fingerprint,
        "N1 tree changed before the switch"
    );
    expect_current_verified(&root, VERSION_N1);

    let recover = support::recover(
        &fixture.install_bin,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&recover, "U3 recover");
    assert_contains(&recover, &["kept target; not auto-activated"]);
    let current_line = format!("current: {VERSION_N1}");
    assert_contains(&recover, &[current_line.as_str()]);
    assert_state_bytes(&root, &state_before, "U3 after recover");
    assert_single_journal(&root, "committed", VERSION_N2, Some(VERSION_N1));
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);

    let retry = upgrade_n2(
        &fixture.install_bin,
        channel,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&retry, "U3 explicit retry");
    let upgraded = format!("upgraded to {VERSION_N2} (native)");
    assert_contains(&retry, &[upgraded.as_str()]);
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&open_root(&root));
    assert_tmp_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N2);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);
    let verify = run_args(
        &fixture.install_bin,
        &args_verify(&root),
        &fixture.tripwires,
        &fixture.work,
        "U3 verify",
    );
    assert_success(&verify, "U3 verify");
    let verified = format!("verified {VERSION_N2} (native)");
    assert_contains(&verify, &[verified.as_str()]);
    commit_evidence(
        "F3-06",
        &gate,
        format!(
            "U3 pre current={VERSION_N1} state-bytes=unchanged journal=committed({VERSION_N2}<-\
             {VERSION_N1}); post current={VERSION_N1} state-bytes=unchanged; recover=kept \
             target/not auto-activated; retry=upgraded to {VERSION_N2}; N1 fingerprint \
             unchanged; verify={VERSION_N2}"
        ),
    );
}

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_06_real_cli_sigkill_upgrade_after_tree_rename() {
    if !support::ensure_tracer("F3-06") {
        return;
    }
    let Some(channel) = support::real_channel("f3_06_kill_after_tree_rename") else {
        support::skip("F3-06", format!("U1: {REAL_SKIP}"));
        return;
    };
    if !require_real_n2(channel, "U1") {
        return;
    }
    let fixture = Fixture::new("f3-kill-u1");
    let root = fixture.root("root");
    assert_success(
        &install_n1(
            &fixture.install_bin,
            channel,
            &root,
            &fixture.tripwires,
            &fixture.work,
        ),
        "U1 baseline N1",
    );
    let n1_fingerprint = installed_fingerprint(&root, VERSION_N1);
    let state_before = state_bytes(&root).expect("N1 state.json");

    let gate = fixture.kill(
        "F3-06/U1",
        &root,
        &args_upgrade(&root, &channel.channel),
        Matcher::RenameTree { target: VERSION_N2 },
        Expectation {
            current: Some(VERSION_N1),
            state: StateExpect::Active {
                active: VERSION_N1,
                previous: None,
            },
            journal: Some(JournalExpect {
                state: "staged",
                target: VERSION_N2,
                previous: Some(VERSION_N1),
            }),
            present_versions: vec![VERSION_N1, VERSION_N2],
            absent_versions: vec![],
            no_root_temp: true,
        },
    );

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(VERSION_N1));
    assert_state_bytes(&root, &state_before, "U1 post-kill");
    assert_single_journal(&root, "staged", VERSION_N2, Some(VERSION_N1));
    expect_current_verified(&root, VERSION_N1);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);

    let recover = support::recover(
        &fixture.install_bin,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&recover, "U1 recover");
    assert_contains(
        &recover,
        &["target exists; marked committed; not auto-activated"],
    );
    assert_state_bytes(&root, &state_before, "U1 after recover");
    assert_single_journal(&root, "committed", VERSION_N2, Some(VERSION_N1));
    expect_current_verified(&root, VERSION_N1);

    let retry = upgrade_n2(
        &fixture.install_bin,
        channel,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&retry, "U1 explicit retry");
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N2);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);
    commit_evidence(
        "F3-06",
        &gate,
        format!(
            "U1 rename(staged,versions/{VERSION_N2}) exit; post current={VERSION_N1} \
             state-bytes=unchanged journal=staged; recover=target exists/marked committed/not \
             auto-activated; retry=upgraded to {VERSION_N2}; N1 fingerprint unchanged"
        ),
    );
}

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_06_real_cli_sigkill_upgrade_after_switch() {
    if !support::ensure_tracer("F3-06") {
        return;
    }
    let Some(channel) = support::real_channel("f3_06_kill_after_switch") else {
        support::skip("F3-06", format!("U5: {REAL_SKIP}"));
        return;
    };
    if !require_real_n2(channel, "U5") {
        return;
    }
    let fixture = Fixture::new("f3-kill-u5");
    let root = fixture.root("root");
    assert_success(
        &install_n1(
            &fixture.install_bin,
            channel,
            &root,
            &fixture.tripwires,
            &fixture.work,
        ),
        "U5 baseline N1",
    );
    let n1_fingerprint = installed_fingerprint(&root, VERSION_N1);
    let state_before = state_bytes(&root).expect("N1 state.json");

    let gate = fixture.kill(
        "F3-06/U5",
        &root,
        &args_upgrade(&root, &channel.channel),
        Matcher::RenameToCurrent,
        Expectation {
            current: Some(VERSION_N2),
            state: StateExpect::Active {
                active: VERSION_N1,
                previous: None,
            },
            journal: Some(JournalExpect {
                state: "committed",
                target: VERSION_N2,
                previous: Some(VERSION_N1),
            }),
            present_versions: vec![VERSION_N1, VERSION_N2],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(VERSION_N2));
    assert_state_bytes(&root, &state_before, "U5 post-kill (state is still N1)");
    assert_single_journal(&root, "committed", VERSION_N2, Some(VERSION_N1));
    expect_current_verified(&root, VERSION_N2);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);

    let recover = support::recover(
        &fixture.install_bin,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&recover, "U5 recover");
    let action = format!("{VERSION_N2} -> verified and state written");
    assert_contains(&recover, &[action.as_str()]);
    let current_line = format!("current: {VERSION_N2}");
    assert_contains(&recover, &[current_line.as_str()]);
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N2);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);
    let verify = run_args(
        &fixture.install_bin,
        &args_verify(&root),
        &fixture.tripwires,
        &fixture.work,
        "U5 verify",
    );
    assert_success(&verify, "U5 verify");
    let verified = format!("verified {VERSION_N2} (native)");
    assert_contains(&verify, &[verified.as_str()]);
    commit_evidence(
        "F3-06",
        &gate,
        format!(
            "U5 rename(current) exit; post current={VERSION_N2} state=active:{VERSION_N1}/\
             previous:null (old bytes); recover=verified and state written; state=active:\
             {VERSION_N2}/previous:{VERSION_N1}; journal removed; verify={VERSION_N2}"
        ),
    );
}

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_06_real_cli_sigkill_upgrade_after_durable_switch() {
    if !support::ensure_tracer("F3-06") {
        return;
    }
    let Some(channel) = support::real_channel("f3_06_kill_after_durable_switch") else {
        support::skip("F3-06", format!("U5-durable: {REAL_SKIP}"));
        return;
    };
    if !require_real_n2(channel, "U5-durable") {
        return;
    }
    let fixture = Fixture::new("f3-kill-u5d");
    let root = fixture.root("root");
    assert_success(
        &install_n1(
            &fixture.install_bin,
            channel,
            &root,
            &fixture.tripwires,
            &fixture.work,
        ),
        "U5-durable baseline N1",
    );
    let n1_fingerprint = installed_fingerprint(&root, VERSION_N1);
    let state_before = state_bytes(&root).expect("N1 state.json");

    // The fsync(root) inside `root.switch` is the durability barrier after the
    // rename; the state write has not happened yet.
    let gate = fixture.kill(
        "F3-06/U5-durable",
        &root,
        &args_upgrade(&root, &channel.channel),
        Matcher::FsyncRoot,
        Expectation {
            current: Some(VERSION_N2),
            state: StateExpect::Active {
                active: VERSION_N1,
                previous: None,
            },
            journal: Some(JournalExpect {
                state: "committed",
                target: VERSION_N2,
                previous: Some(VERSION_N1),
            }),
            present_versions: vec![VERSION_N1, VERSION_N2],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(VERSION_N2));
    assert_state_bytes(&root, &state_before, "U5-durable post-kill");
    assert_single_journal(&root, "committed", VERSION_N2, Some(VERSION_N1));
    expect_current_verified(&root, VERSION_N2);

    let recover = support::recover(
        &fixture.install_bin,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&recover, "U5-durable recover");
    let action = format!("{VERSION_N2} -> verified and state written");
    assert_contains(&recover, &[action.as_str()]);
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N2);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);
    let verify = run_args(
        &fixture.install_bin,
        &args_verify(&root),
        &fixture.tripwires,
        &fixture.work,
        "U5-durable verify",
    );
    assert_success(&verify, "U5-durable verify");
    commit_evidence(
        "F3-06",
        &gate,
        format!(
            "U5-durable fsync(root) exit after the switch rename; post current={VERSION_N2} \
             state=active:{VERSION_N1}/previous:null; recover=verified and state written; \
             state=active:{VERSION_N2}/previous:{VERSION_N1}; journal removed; verify=\
             {VERSION_N2}"
        ),
    );
}

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_06_real_cli_sigkill_upgrade_after_state_write() {
    if !support::ensure_tracer("F3-06") {
        return;
    }
    let Some(channel) = support::real_channel("f3_06_kill_after_state_write") else {
        support::skip("F3-06", format!("U6: {REAL_SKIP}"));
        return;
    };
    if !require_real_n2(channel, "U6") {
        return;
    }
    let fixture = Fixture::new("f3-kill-u6");
    let root = fixture.root("root");
    assert_success(
        &install_n1(
            &fixture.install_bin,
            channel,
            &root,
            &fixture.tripwires,
            &fixture.work,
        ),
        "U6 baseline N1",
    );
    let n1_fingerprint = installed_fingerprint(&root, VERSION_N1);

    let gate = fixture.kill(
        "F3-06/U6",
        &root,
        &args_upgrade(&root, &channel.channel),
        Matcher::RenameToState,
        Expectation {
            current: Some(VERSION_N2),
            state: StateExpect::Active {
                active: VERSION_N2,
                previous: Some(VERSION_N1),
            },
            journal: Some(JournalExpect {
                state: "committed",
                target: VERSION_N2,
                previous: Some(VERSION_N1),
            }),
            present_versions: vec![VERSION_N1, VERSION_N2],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );
    let state_at_kill = gate
        .hits()
        .first()
        .and_then(|hit| hit.disk.state_bytes.clone())
        .expect("U6 hit recorded the written state.json");

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(VERSION_N2));
    assert_state_bytes(&root, &state_at_kill, "U6 post-kill");
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    assert_single_journal(&root, "committed", VERSION_N2, Some(VERSION_N1));
    expect_current_verified(&root, VERSION_N2);

    let recover = support::recover(
        &fixture.install_bin,
        &root,
        &fixture.tripwires,
        &fixture.work,
    );
    assert_success(&recover, "U6 recover");
    assert_contains(&recover, &["valid state already active; journal removed"]);
    assert_state_bytes(&root, &state_at_kill, "U6 after recover");
    assert_journal_empty(&open_root(&root));
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    expect_current_verified(&root, VERSION_N2);
    assert_eq!(installed_fingerprint(&root, VERSION_N1), n1_fingerprint);
    let verify = run_args(
        &fixture.install_bin,
        &args_verify(&root),
        &fixture.tripwires,
        &fixture.work,
        "U6 verify",
    );
    assert_success(&verify, "U6 verify");
    commit_evidence(
        "F3-06",
        &gate,
        format!(
            "U6 rename(state.json) exit; post current={VERSION_N2} state=active:{VERSION_N2}/\
             previous:{VERSION_N1}; recover=valid state already active/journal removed; state \
             bytes unchanged; verify={VERSION_N2}"
        ),
    );
}

// ---------------------------------------------------------------------------
// F3-09: legacy schema-v1 -> native N1 migration
// ---------------------------------------------------------------------------

struct LegacyFixture {
    fixture: Fixture,
    root: PathBuf,
    marker: PathBuf,
    fingerprint: String,
}

impl LegacyFixture {
    fn install(label: &str) -> Self {
        let fixture = Fixture::new(label);
        let marker = fixture.base.path().join("LEGACY_ENTRY_EXECUTED");
        let release = build_legacy_release(&fixture.base, &marker);
        let root = fixture.root("root");
        install_legacy(
            &fixture.install_bin,
            &release,
            &root,
            &fixture.tripwires,
            &fixture.work,
        );
        clear_marker(&marker);
        assert_legacy_current(&root);
        let fingerprint = legacy_fingerprint(&root, LEGACY_VERSION);
        assert_no_state(&root, "legacy baseline");
        assert_journal_empty(&open_root(&root));
        assert_tmp_empty(&open_root(&root));
        Self {
            fixture,
            root,
            marker,
            fingerprint,
        }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn kill(
        &self,
        label: &str,
        args: &[String],
        matcher: Matcher,
        expectation: Expectation,
    ) -> KillGate {
        self.fixture
            .kill(label, self.root(), args, matcher, expectation)
    }
}

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_09_real_cli_sigkill_legacy_before_switch() {
    if !support::ensure_tracer("F3-09") {
        return;
    }
    let Some(channel) = support::real_channel("f3_09_kill_before_switch") else {
        support::skip("F3-09", format!("L3: {REAL_SKIP}"));
        return;
    };
    let legacy = LegacyFixture::install("f3-kill-l3");
    let root = legacy.root().to_path_buf();

    let gate = legacy.kill(
        "F3-09/L3",
        &args_install(&root, &channel.channel, VERSION_N1),
        Matcher::SymlinkCurrentTemp,
        Expectation {
            current: Some(LEGACY_VERSION),
            state: StateExpect::Absent,
            journal: Some(JournalExpect {
                state: "committed",
                target: VERSION_N1,
                previous: None,
            }),
            present_versions: vec![LEGACY_VERSION, VERSION_N1],
            absent_versions: vec![],
            no_root_temp: true,
        },
    );

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(LEGACY_VERSION));
    assert_no_state(&root, "L3 post-kill");
    assert_single_journal(&root, "committed", VERSION_N1, None);
    assert_eq!(
        legacy_fingerprint(&root, LEGACY_VERSION),
        legacy.fingerprint,
        "legacy tree changed before the migration switch"
    );
    assert_marker_absent(&legacy.marker);
    assert_legacy_current(&root);

    let recover = support::recover(
        &legacy.fixture.install_bin,
        &root,
        &legacy.fixture.tripwires,
        &legacy.fixture.work,
    );
    assert_success(&recover, "L3 recover");
    assert_contains(&recover, &["kept target; not auto-activated"]);
    let current_line = format!("current: {LEGACY_VERSION}");
    assert_contains(&recover, &[current_line.as_str()]);
    assert_no_state(&root, "L3 after recover");
    assert_single_journal(&root, "committed", VERSION_N1, None);
    assert_eq!(
        legacy_fingerprint(&root, LEGACY_VERSION),
        legacy.fingerprint
    );
    assert_marker_absent(&legacy.marker);
    let disk = DiskSnapshot::read(&root);
    assert!(
        disk.journals
            .iter()
            .all(|journal| journal.previous.as_deref() != Some(LEGACY_VERSION)),
        "a legacy package must never be recorded as previous: {}",
        disk.journal_label()
    );

    let retry = run_args(
        &legacy.fixture.install_bin,
        &args_install(&root, &channel.channel, VERSION_N1),
        &legacy.fixture.tripwires,
        &legacy.fixture.work,
        "L3 explicit retry",
    );
    assert_success(&retry, "L3 explicit retry");
    let migrated = format!("migrated to {VERSION_N1} (native)");
    assert_contains(&retry, &[migrated.as_str()]);
    assert_state(&root, VERSION_N1, None);
    assert_journal_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N1);
    assert_eq!(
        legacy_fingerprint(&root, LEGACY_VERSION),
        legacy.fingerprint,
        "legacy tree changed during the retried migration"
    );
    assert_marker_absent(&legacy.marker);
    commit_evidence(
        "F3-09",
        &gate,
        format!(
            "L3 legacy pre-switch; post current={LEGACY_VERSION} state=absent \
             journal=committed({VERSION_N1}<-null); recover=kept target/not auto-activated; \
             legacy fingerprint unchanged; legacy marker absent; retry=migrated to \
             {VERSION_N1} (native); previous=null"
        ),
    );
}

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_09_real_cli_sigkill_legacy_after_switch() {
    if !support::ensure_tracer("F3-09") {
        return;
    }
    let Some(channel) = support::real_channel("f3_09_kill_after_switch") else {
        support::skip("F3-09", format!("L4: {REAL_SKIP}"));
        return;
    };
    let legacy = LegacyFixture::install("f3-kill-l4");
    let root = legacy.root().to_path_buf();

    let gate = legacy.kill(
        "F3-09/L4",
        &args_install(&root, &channel.channel, VERSION_N1),
        Matcher::RenameToCurrent,
        Expectation {
            current: Some(VERSION_N1),
            state: StateExpect::Absent,
            journal: Some(JournalExpect {
                state: "committed",
                target: VERSION_N1,
                previous: None,
            }),
            present_versions: vec![LEGACY_VERSION, VERSION_N1],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );

    let disk = DiskSnapshot::read(&root);
    assert_eq!(disk.current.as_deref(), Some(VERSION_N1));
    assert_no_state(&root, "L4 post-kill");
    assert_single_journal(&root, "committed", VERSION_N1, None);
    expect_current_verified(&root, VERSION_N1);
    assert_eq!(
        legacy_fingerprint(&root, LEGACY_VERSION),
        legacy.fingerprint
    );
    assert_marker_absent(&legacy.marker);

    let recover = support::recover(
        &legacy.fixture.install_bin,
        &root,
        &legacy.fixture.tripwires,
        &legacy.fixture.work,
    );
    assert_success(&recover, "L4 recover");
    let action = format!("{VERSION_N1} -> verified and state written");
    assert_contains(&recover, &[action.as_str()]);
    assert_state(&root, VERSION_N1, None);
    assert_journal_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N1);
    assert_eq!(
        legacy_fingerprint(&root, LEGACY_VERSION),
        legacy.fingerprint,
        "legacy tree changed after the post-switch recovery"
    );
    assert_marker_absent(&legacy.marker);
    let verify = run_args(
        &legacy.fixture.install_bin,
        &args_verify(&root),
        &legacy.fixture.tripwires,
        &legacy.fixture.work,
        "L4 verify",
    );
    assert_success(&verify, "L4 verify");
    let verified = format!("verified {VERSION_N1} (native)");
    assert_contains(&verify, &[verified.as_str()]);
    commit_evidence(
        "F3-09",
        &gate,
        format!(
            "L4 legacy post-switch; post current={VERSION_N1} state=absent journal=committed\
             ({VERSION_N1}<-null); recover=verified and state written; state=active:\
             {VERSION_N1}/previous:null; legacy fingerprint unchanged; legacy marker absent; \
             verify={VERSION_N1}"
        ),
    );
}

// ---------------------------------------------------------------------------
// Tracer control on the real CLI
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires pinned real assets + ptrace; run with --ignored"]
fn f3_06_real_cli_kill_control_never_matches() {
    if !support::ensure_tracer("F3-06/CONTROL") {
        return;
    }
    let Some(channel) = support::real_channel("f3_06_kill_control") else {
        support::skip("F3-06/CONTROL", REAL_SKIP);
        return;
    };
    if !require_real_n2(channel, "CONTROL") {
        return;
    }
    let fixture = Fixture::new("f3-kill-control");
    let root = fixture.root("root");
    assert_success(
        &install_n1(
            &fixture.install_bin,
            channel,
            &root,
            &fixture.tripwires,
            &fixture.work,
        ),
        "control baseline N1",
    );
    let mut gate = KillGate::new(
        &root,
        Matcher::Never,
        Expectation {
            current: None,
            state: StateExpect::Absent,
            journal: None,
            present_versions: vec![],
            absent_versions: vec![],
            no_root_temp: false,
        },
    );
    let args = args_upgrade(&root, &channel.channel);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let outcome = support::traced_installer(
        "F3-06/CONTROL",
        &fixture.install_bin,
        &refs,
        &fixture.tripwires,
        &fixture.work,
        &mut gate,
    );
    support::print_outcome("F3-06/CONTROL", &outcome);
    assert!(
        outcome.exit.success(),
        "the control upgrade failed under the tracer: exit={:?} stderr={}",
        outcome.exit,
        outcome.stderr_text()
    );
    assert!(!outcome.killed, "the never-matching gate killed the CLI");
    assert_eq!(gate.candidates(), 0);
    assert!(gate.hits().is_empty());
    assert!(
        outcome.stops > 0,
        "the control run recorded no syscall stops"
    );
    assert_state(&root, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&open_root(&root));
    expect_current_verified(&root, VERSION_N2);
    fixture.tripwires.assert_not_executed();
    support::evidence(
        "F3-06",
        "PASS",
        format!(
            "control: never-matching gate ran a full N1->{VERSION_N2} upgrade under \
             PTRACE_SYSCALL without a kill; stops={} syscalls={}; state=active:{VERSION_N2}/\
             previous:{VERSION_N1}; verify={VERSION_N2}",
            outcome.stops,
            outcome.syscall_summary()
        ),
    );
}
