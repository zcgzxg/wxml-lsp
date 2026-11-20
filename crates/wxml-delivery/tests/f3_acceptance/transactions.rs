//! F3-06: real N1 -> N2 failure/interruption windows through the public
//! `InstallHooks` seam, plus the explicit BLOCKED process-level kill window.
//! F3-16: staged/committed/activated recovery, concurrent install and rollback
//! from a corrupted active tree.

use std::fs;

use wxml_delivery::error::DeliveryError;
use wxml_delivery::native_install::{CommandEngineProbe, FailPoint, verify_native};
use wxml_delivery::state::{Journal, TransactionState};

use crate::support::{
    INSTALL_TIMEOUT, TestDir, Tripwires, VERSION_N1, VERSION_N2, active_version, assert_contains,
    assert_current_verified, assert_journal_empty, assert_state, assert_success, assert_tmp_empty,
    combined, evidence, fixture, install_n1, install_with_failpoint, installed_fingerprint,
    installed_versions, installer_bin, open_root, private_root, rollback_with_failpoint,
    run_installer, run_two_installers, shared_channel, skip, stage_release_tree, state_bytes,
    stdout, upgrade_n2, workspace, write_journal,
};

/// Explicit "current is always valid" sample at one named critical point.
///
/// The contract asks that `current` stays resolvable and points at a verified
/// tree; this suite samples that at every transition instead of claiming
/// continuous monitoring.
fn sample_current(root_path: &std::path::Path, version: &str, point: &str) {
    let files = assert_current_verified(root_path, version);
    println!(
        "F3-16 CURRENT-SAMPLE: {point}: current -> versions/{version} \
         ({files} files verified)"
    );
}

/// Returns the stable identity of `state.json`, used to prove a recovery or
/// rollback did not rewrite it.
fn state_identity(path: &std::path::Path) -> (u64, i64, i64) {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path).expect("state.json metadata");
    (metadata.ino(), metadata.ctime(), metadata.ctime_nsec())
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
#[allow(clippy::too_many_lines)]
fn f3_06_real_upgrade_and_rollback_failpoint_windows() {
    let Some(channel) = shared_channel("f3_06_failpoint_windows") else {
        skip(
            "F3-06",
            "pinned real assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-06");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();
    fixture(
        "F3-06",
        "the failure checkpoints are test-injected through `InstallHooks`; package trees, engine \
         probe and Rust version probes are real",
    );
    println!(
        "F3-06 SCOPE: both releases are production-packager output; the fault is injected through \
         the public `InstallHooks` library seam, not by killing the CLI at a wall-clock instant"
    );

    for point in [FailPoint::BeforeSwitch, FailPoint::AfterSwitch] {
        let root_path = private_root(&base, &format!("upgrade-{point:?}"));
        let install = install_n1(&install_bin, channel, &root_path, &tripwires, &work);
        assert_success(&install, "F3-06 baseline N1");
        let root = open_root(&root_path);
        let n1_fingerprint = installed_fingerprint(&root_path, VERSION_N1);
        let state_before = state_bytes(&root_path);
        let staged = stage_release_tree(&root, &channel.n2);

        let error = install_with_failpoint(&root, &staged, VERSION_N2, point).unwrap_err();
        match point {
            FailPoint::BeforeSwitch => {
                assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");
                assert_eq!(active_version(&root).as_deref(), Some(VERSION_N1));
                assert_eq!(state_bytes(&root_path), state_before);
                assert_eq!(
                    installed_fingerprint(&root_path, VERSION_N1),
                    n1_fingerprint
                );
                assert_eq!(installed_versions(&root), vec![VERSION_N1, VERSION_N2]);
                let journals = Journal::list(&root).expect("list journals");
                assert_eq!(journals.len(), 1, "{journals:?}");
                assert_eq!(journals[0].target, VERSION_N2);
                assert_eq!(journals[0].state, TransactionState::Committed);
                assert_tmp_empty(&root);
                evidence(
                    "F3-06",
                    "PASS",
                    "upgrade failpoint BeforeSwitch left N1 current, state bytes unchanged, \
                     N1 byte-identical, N2 committed but inactive",
                );
            }
            FailPoint::AfterSwitch => {
                assert!(matches!(error, DeliveryError::InstallState(_)), "{error:?}");
                assert!(
                    error.to_string().contains("restored previous native"),
                    "{error}"
                );
                assert_eq!(active_version(&root).as_deref(), Some(VERSION_N1));
                assert_eq!(state_bytes(&root_path), state_before);
                assert_eq!(
                    installed_fingerprint(&root_path, VERSION_N1),
                    n1_fingerprint
                );
                assert!(root.versions_dir().join(VERSION_N2).is_dir());
                verify_native(&root, &CommandEngineProbe::default())
                    .expect("N1 must still verify after restore");
                assert_tmp_empty(&root);
                evidence(
                    "F3-06",
                    "PASS",
                    "upgrade failpoint AfterSwitch restored N1 current, kept state bytes and both \
                     real trees byte-identical",
                );
            }
            _ => unreachable!(),
        }
    }

    for point in [FailPoint::BeforeSwitch, FailPoint::AfterSwitch] {
        let root_path = private_root(&base, &format!("rollback-{point:?}"));
        let install = install_n1(&install_bin, channel, &root_path, &tripwires, &work);
        assert_success(&install, "F3-06 rollback baseline N1");
        let upgrade = upgrade_n2(&install_bin, channel, &root_path, &tripwires, &work);
        assert_success(&upgrade, "F3-06 rollback baseline N2");
        let root = open_root(&root_path);
        assert_state(&root_path, VERSION_N2, Some(VERSION_N1));
        let n1_fingerprint = installed_fingerprint(&root_path, VERSION_N1);
        let n2_fingerprint = installed_fingerprint(&root_path, VERSION_N2);
        let state_before = state_bytes(&root_path);

        let error = rollback_with_failpoint(&root, VERSION_N1, point).unwrap_err();
        match point {
            FailPoint::BeforeSwitch => {
                assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");
                assert_eq!(active_version(&root).as_deref(), Some(VERSION_N2));
                assert_eq!(state_bytes(&root_path), state_before);
                assert_eq!(
                    installed_fingerprint(&root_path, VERSION_N1),
                    n1_fingerprint
                );
                assert_eq!(
                    installed_fingerprint(&root_path, VERSION_N2),
                    n2_fingerprint
                );
                let journals = Journal::list(&root).expect("list journals");
                assert_eq!(journals.len(), 1, "{journals:?}");
                assert_eq!(journals[0].target, VERSION_N1);
                assert_eq!(journals[0].state, TransactionState::Committed);
                evidence(
                    "F3-06",
                    "PASS",
                    "rollback failpoint BeforeSwitch kept N2 current, state bytes unchanged and \
                     both real trees byte-identical; rollback journal stays committed",
                );
            }
            FailPoint::AfterSwitch => {
                assert!(matches!(error, DeliveryError::InstallState(_)), "{error:?}");
                assert_eq!(active_version(&root).as_deref(), Some(VERSION_N2));
                assert_eq!(state_bytes(&root_path), state_before);
                assert_eq!(
                    installed_fingerprint(&root_path, VERSION_N1),
                    n1_fingerprint
                );
                assert_eq!(
                    installed_fingerprint(&root_path, VERSION_N2),
                    n2_fingerprint
                );
                verify_native(&root, &CommandEngineProbe::default())
                    .expect("N2 must still verify after rollback restore");
                evidence(
                    "F3-06",
                    "PASS",
                    "rollback failpoint AfterSwitch restored N2 current, kept state bytes and both \
                     real trees byte-identical",
                );
            }
            _ => unreachable!(),
        }
    }

    evidence(
        "F3-06",
        "BLOCKED",
        "a real CLI process kill exactly before/after the symlink switch needs a production \
         failpoint env/config seam; `wxml-lsp-install` currently exposes only `NoHooks`",
    );
    tripwires.assert_not_executed();
    println!(
        "F3-06: switch-before/after and rollback-before/after windows passed on real package trees; \
         process-level CLI injection remains BLOCKED"
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
#[allow(clippy::too_many_lines)]
fn f3_16_journal_recovery_concurrent_install_and_corrupt_active_rollback() {
    let Some(channel) = shared_channel("f3_16_recovery") else {
        skip(
            "F3-16",
            "pinned real assets are absent; set TSGO_NATIVE_TARBALL/TSGO_TYPINGS_TARBALL or WXML_REQUIRE_REAL_ENGINE=1",
        );
        return;
    };
    let base = TestDir::new("f3-16");
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let install_bin = installer_bin();
    fixture(
        "F3-16",
        "staged/committed/activated journal files are written by the test; installed package \
         trees and all recovery probes are real",
    );

    // staged journal: removed without activation.
    let staged_root_path = private_root(&base, "staged-root");
    let install = install_n1(&install_bin, channel, &staged_root_path, &tripwires, &work);
    assert_success(&install, "F3-16 staged baseline N1");
    sample_current(&staged_root_path, VERSION_N1, "staged baseline");
    let staged_root = open_root(&staged_root_path);
    let staged_state = state_bytes(&staged_root_path);
    fs::create_dir_all(staged_root.tmp_dir()).expect("create tmp");
    fs::write(staged_root.tmp_dir().join("abandoned"), b"stale").expect("write stale tmp");
    write_journal(
        &staged_root,
        wxml_delivery::state::TransactionOp::Install,
        "0.3.0",
        None,
        TransactionState::Staged,
    );
    let recover = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            staged_root_path.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover staged journal",
    );
    assert_success(&recover, "recover staged");
    assert_contains(
        &recover,
        &["staged 0.3.0 -> removed temporary state; current unchanged"],
    );
    assert_eq!(state_bytes(&staged_root_path), staged_state);
    assert_eq!(active_version(&staged_root).as_deref(), Some(VERSION_N1));
    assert_journal_empty(&staged_root);
    assert_tmp_empty(&staged_root);
    sample_current(&staged_root_path, VERSION_N1, "after staged recovery");
    evidence(
        "F3-16",
        "PASS",
        "staged journal for 0.3.0 removed; current/state unchanged; tmp cleaned",
    );

    // committed journal: target must not auto-activate.
    let committed_root_path = private_root(&base, "committed-root");
    let install = install_n1(
        &install_bin,
        channel,
        &committed_root_path,
        &tripwires,
        &work,
    );
    assert_success(&install, "F3-16 committed baseline N1");
    let committed_root = open_root(&committed_root_path);
    sample_current(&committed_root_path, VERSION_N1, "committed baseline");
    let n1_fingerprint = installed_fingerprint(&committed_root_path, VERSION_N1);
    let committed_state = state_bytes(&committed_root_path);
    let staged = stage_release_tree(&committed_root, &channel.n2);
    let error =
        install_with_failpoint(&committed_root, &staged, VERSION_N2, FailPoint::AfterRename)
            .unwrap_err();
    assert!(matches!(error, DeliveryError::Runtime(_)), "{error:?}");
    assert_eq!(active_version(&committed_root).as_deref(), Some(VERSION_N1));
    sample_current(
        &committed_root_path,
        VERSION_N1,
        "after committed-target failpoint",
    );
    let recover = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            committed_root_path.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover committed journal",
    );
    assert_success(&recover, "recover committed");
    assert_contains(&recover, &["not auto-activated"]);
    assert_eq!(active_version(&committed_root).as_deref(), Some(VERSION_N1));
    assert_eq!(state_bytes(&committed_root_path), committed_state);
    assert_eq!(
        installed_fingerprint(&committed_root_path, VERSION_N1),
        n1_fingerprint
    );
    assert_eq!(
        Journal::list(&committed_root)
            .expect("list journals")
            .iter()
            .filter(|journal| journal.target == VERSION_N2)
            .count(),
        1,
        "the committed journal must be retained as evidence"
    );
    sample_current(&committed_root_path, VERSION_N1, "after committed recovery");
    evidence(
        "F3-16",
        "PASS",
        "committed N2 target recovered without auto-activation; N1 current/state/tree unchanged",
    );

    // Explicit upgrade activates the committed target and clears its journal.
    let upgrade = upgrade_n2(
        &install_bin,
        channel,
        &committed_root_path,
        &tripwires,
        &work,
    );
    assert_success(&upgrade, "F3-16 explicit upgrade after committed recovery");
    assert_state(&committed_root_path, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&committed_root);
    sample_current(&committed_root_path, VERSION_N2, "explicit upgrade");
    evidence(
        "F3-16",
        "PASS",
        "explicit upgrade after committed recovery activated N2 and removed the retained journal",
    );

    // activated journal with a missing state file: re-verify and write state.
    let state_path = committed_root_path.join("state.json");
    fs::remove_file(&state_path).expect("remove state for activated recovery");
    write_journal(
        &committed_root,
        wxml_delivery::state::TransactionOp::Upgrade,
        VERSION_N2,
        Some(VERSION_N1),
        TransactionState::Activated,
    );
    let recover = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            committed_root_path.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover activated journal",
    );
    assert_success(&recover, "recover activated");
    assert_state(&committed_root_path, VERSION_N2, Some(VERSION_N1));
    assert_journal_empty(&committed_root);
    sample_current(&committed_root_path, VERSION_N2, "after activated recovery");
    evidence(
        "F3-16",
        "PASS",
        "activated journal re-verified the real engine/binary probes and rewrote a missing \
         state.json",
    );

    // A second `recover` with no journal must be a pure no-op: the journal is
    // already gone, `current` stays N2, and state.json is not rewritten.
    let activated_state = state_bytes(&committed_root_path).expect("state.json exists");
    let activated_identity = state_identity(&state_path);
    let second = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            committed_root_path.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover activated journal a second time",
    );
    assert_success(&second, "second recover activated");
    assert_contains(&second, &["no recovery actions were needed"]);
    assert_journal_empty(&committed_root);
    assert_eq!(
        state_bytes(&committed_root_path).as_deref(),
        Some(activated_state.as_slice()),
        "a no-op recovery changed state.json bytes"
    );
    assert_eq!(
        state_identity(&state_path),
        activated_identity,
        "a no-op recovery rewrote state.json"
    );
    assert_eq!(active_version(&committed_root).as_deref(), Some(VERSION_N2));
    sample_current(
        &committed_root_path,
        VERSION_N2,
        "second recover (no journal)",
    );
    evidence(
        "F3-16",
        "PASS",
        "a second `recover` found no journal, reported no actions and left state.json \
         byte- and inode-identical with current still 0.2.0",
    );

    // Re-create the activated journal exactly as a crash after `state.write`
    // but before journal removal would. `current` and state are already valid,
    // so recovery must only remove the journal and never rewrite state.
    write_journal(
        &committed_root,
        wxml_delivery::state::TransactionOp::Upgrade,
        VERSION_N2,
        Some(VERSION_N1),
        TransactionState::Activated,
    );
    let idempotent = run_installer(
        &install_bin,
        &[
            "recover",
            "--root",
            committed_root_path.to_str().expect("UTF-8 root"),
        ],
        &tripwires,
        &work,
        "recover re-created activated journal with valid state",
    );
    assert_success(&idempotent, "idempotent activated recovery");
    assert_contains(
        &idempotent,
        &["valid state already active", "journal removed"],
    );
    assert_journal_empty(&committed_root);
    assert_eq!(
        state_bytes(&committed_root_path).as_deref(),
        Some(activated_state.as_slice()),
        "idempotent activated recovery changed state.json bytes"
    );
    assert_eq!(
        state_identity(&state_path),
        activated_identity,
        "idempotent activated recovery rewrote state.json"
    );
    assert_eq!(active_version(&committed_root).as_deref(), Some(VERSION_N2));
    sample_current(
        &committed_root_path,
        VERSION_N2,
        "idempotent activated journal removal",
    );
    evidence(
        "F3-16",
        "PASS",
        "a re-created activated journal for the already-valid state was removed without \
         rewriting state.json; current stayed 0.2.0",
    );

    // corrupted active tree: rollback to the verified target.
    let n1_fingerprint = installed_fingerprint(&committed_root_path, VERSION_N1);
    let corrupted = committed_root_path
        .join("versions")
        .join(VERSION_N2)
        .join("engine/lib/tsc.sig");
    let mut bytes = fs::read(&corrupted).expect("read active signature");
    bytes[0] ^= 0x01;
    fs::write(&corrupted, &bytes).expect("corrupt active N2 file");
    let rollback = run_installer(
        &install_bin,
        &[
            "rollback",
            "--root",
            committed_root_path.to_str().expect("UTF-8 root"),
            "--to",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "rollback from corrupted active",
    );
    assert_success(&rollback, "rollback from corrupted active");
    assert_contains(&rollback, &["rolled back to 0.1.0"]);
    assert_state(&committed_root_path, VERSION_N1, Some(VERSION_N2));
    assert_eq!(
        installed_fingerprint(&committed_root_path, VERSION_N1),
        n1_fingerprint,
        "N1 rollback target changed"
    );
    sample_current(
        &committed_root_path,
        VERSION_N1,
        "rollback from corrupted active",
    );
    evidence(
        "F3-16",
        "PASS",
        "rollback from a corrupted active N2 tree verified the N1 target and activated it",
    );

    // concurrent installs: both processes must succeed and serialize.
    let concurrent_root = private_root(&base, "concurrent-root");
    let root_text = concurrent_root.to_str().expect("UTF-8 root");
    let channel_text = channel.channel.to_str().expect("UTF-8 channel");
    let args = [
        "install",
        "--root",
        root_text,
        "--from-dir",
        channel_text,
        "--version",
        VERSION_N1,
        "--offline",
    ];
    let mut first = crate::support::clean_command(&install_bin, &tripwires, &work);
    first.args(args);
    let mut second = crate::support::clean_command(&install_bin, &tripwires, &work);
    second.args(args);
    let (first, second) = run_two_installers(&mut first, &mut second, INSTALL_TIMEOUT, &tripwires);
    assert_success(&first, "concurrent installer one");
    assert_success(&second, "concurrent installer two");
    assert!(
        stdout(&first).contains("0.1.0") && stdout(&second).contains("0.1.0"),
        "first={} second={}",
        combined(&first),
        combined(&second)
    );
    let concurrent = open_root(&concurrent_root);
    assert_eq!(active_version(&concurrent).as_deref(), Some(VERSION_N1));
    assert_eq!(installed_versions(&concurrent), vec![VERSION_N1]);
    assert_state(&concurrent_root, VERSION_N1, None);
    assert_journal_empty(&concurrent);
    assert_tmp_empty(&concurrent);
    sample_current(&concurrent_root, VERSION_N1, "concurrent installs");
    evidence(
        "F3-16",
        "PASS",
        format!(
            "two concurrent real installs both succeeded under the exclusive root lock; final \
             current={VERSION_N1}"
        ),
    );

    evidence(
        "F3-16",
        "PASS",
        "current was sampled at 11 explicit critical points and each time resolved to \
         versions/<v> inside the root and passed a full tree verification; this is sampling, \
         not continuous monitoring",
    );
    evidence(
        "F3-16",
        "BLOCKED",
        "continuous `current` monitoring and power-loss/directory-fsync durability still need \
         an external supervisor; this suite samples only at transaction boundaries",
    );

    tripwires.assert_not_executed();
    println!(
        "F3-16: staged/committed/activated recovery, concurrent install and corrupted-active \
         rollback all completed without Node"
    );
}
