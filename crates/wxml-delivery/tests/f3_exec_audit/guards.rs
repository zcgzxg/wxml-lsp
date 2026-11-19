//! Non-real exec-audit gates: the positive control and the fixture installer.
//!
//! These tests need no pinned asset and run by default. The positive control
//! proves the tracer records a Node-named exec; the fixture run proves the
//! production installer's `install`/`verify`/`current`/`env` flows only exec
//! the installer and the package's own entry points. Both are labelled
//! `FIXTURE`/control evidence, never as a real release PASS.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::json;

use crate::fixture;
use crate::real_asset_support::{TestDir, create_private_install_root};
use crate::support;
use crate::tracer::{self, ExecLog, Spec, TraceError};
use wxml_delivery::manifest::set_mode;

/// Proves the tracer is not vacuous: an executable literally named `node` is
/// exec'd by a *descendant* of the traced root and must be recorded with its
/// absolute `/proc/<pid>/exe` path and fork parent.
#[test]
fn positive_control_records_a_node_named_exec() {
    let base = TestDir::new("f3-exec-audit-control");
    if !support::ensure_audit_available("positive control") {
        return;
    }

    let node = base.path().join("node");
    fs::copy("/bin/true", &node).unwrap_or_else(|error| panic!("copy /bin/true: {error}"));
    set_mode(&node, 0o755).expect("make the node control executable");
    let script = format!("{} ; /bin/true", node.display());

    // The shell forks for the first command because a second command follows,
    // so the `node` exec is a descendant, not the root process.
    let spec = Spec::new("/bin/sh", base.path())
        .arg("-c")
        .arg(script)
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .env("LC_ALL", "C");
    let Some(traced) = support::spawn("positive control", spec, Duration::from_secs(15)) else {
        return;
    };
    let (outcome, stdout, stderr) = traced.wait_with_output();
    support::assert_success("positive control", &outcome, &stdout, &stderr);
    support::print_outcome("positive-control", &outcome);

    assert!(
        outcome.log.covers("/node"),
        "the traced shell did not exec the node control; log:\n{}",
        outcome.log.summary()
    );
    assert!(
        outcome
            .log
            .forks
            .iter()
            .any(|fork| fork.parent == outcome.log.root_pid),
        "no fork event was recorded for the traced shell; log:\n{}",
        outcome.log.summary()
    );
    let node_record = outcome
        .log
        .execs
        .iter()
        .find(|record| record.exe_name() == "node")
        .unwrap_or_else(|| {
            panic!(
                "the node-named exec is missing; log:\n{}",
                outcome.log.summary()
            )
        });
    assert_eq!(
        node_record.ppid,
        outcome.log.root_pid,
        "the node exec must be a child of the traced shell: {}",
        node_record.line()
    );
    assert!(
        node_record.exe.is_absolute(),
        "the recorded node path is not absolute: {}",
        node_record.line()
    );

    // The exact check used by the real audit must flag this log, proving the
    // checker cannot silently pass a Node exec.
    let evidence = support::forbidden_evidence(&outcome.log);
    assert!(
        !evidence.is_empty() && evidence.iter().any(|line| line.contains("node")),
        "the audit check did not flag the node control: {evidence:?}"
    );
    println!(
        "EXEC-AUDIT: PASS (positive control): node exec recorded at {}; forbidden evidence: {}",
        node_record.exe.display(),
        evidence.join(" | ")
    );
}

/// `FIXTURE`: production installer code paths against a shell fixture package.
#[test]
fn fixture_installer_flows_exec_only_the_installer_and_package_entry_points() {
    let base = TestDir::new("f3-exec-audit-fixture");
    if !support::ensure_audit_available("fixture installer") {
        return;
    }
    let release = fixture::build_release(base.path());
    let (work, home, tmp) = support::workspace_paths(base.path());
    let tripwires = support::install_tripwires(base.path());
    let env = support::clean_env(&tripwires, &home, &tmp);
    let root = create_private_install_root(&base, "install-root");
    let root_text = root.to_str().expect("UTF-8 install root").to_owned();
    let channel_text = release
        .channel
        .to_str()
        .expect("UTF-8 fixture channel")
        .to_owned();
    let installer = PathBuf::from(env!("CARGO_BIN_EXE_wxml-lsp-install"));

    let runs: Vec<(&str, Vec<OsString>, String)> = vec![
        (
            "install",
            vec![
                "install".into(),
                "--root".into(),
                root_text.clone().into(),
                "--from-dir".into(),
                channel_text.into(),
                "--version".into(),
                fixture::VERSION.into(),
                "--offline".into(),
            ],
            format!("installed {} (native)", fixture::VERSION),
        ),
        (
            "verify",
            vec!["verify".into(), "--root".into(), root_text.clone().into()],
            format!("verified {} (native)", fixture::VERSION),
        ),
        (
            "current",
            vec!["current".into(), "--root".into(), root_text.clone().into()],
            format!("version: {}", fixture::VERSION),
        ),
        (
            "env",
            vec!["env".into(), "--root".into(), root_text.into()],
            "export WXML_LSP_BIN=".to_owned(),
        ),
    ];

    let mut merged = ExecLog::default();
    for (label, args, expected) in runs {
        let args: Vec<String> = args
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let spec = support::spec_for(&installer, &args, &env, &work);
        let Some(traced) = support::spawn(label, spec, support::INSTALL_TIMEOUT) else {
            return;
        };
        let (outcome, stdout, stderr) = traced.wait_with_output();
        support::assert_success(label, &outcome, &stdout, &stderr);
        support::assert_exec_audit(&outcome.log, label);
        support::print_outcome(label, &outcome);
        let text = String::from_utf8_lossy(&stdout);
        assert!(
            text.contains(&expected),
            "{label}: expected `{expected}` in stdout: {text}\nstderr={}",
            String::from_utf8_lossy(&stderr)
        );
        let root_exec = outcome
            .log
            .execs
            .iter()
            .find(|record| record.root)
            .unwrap_or_else(|| panic!("{label}: no root exec recorded"));
        assert!(
            root_exec
                .exe
                .to_string_lossy()
                .ends_with("wxml-lsp-install"),
            "{label}: the traced root is not the installer: {}",
            root_exec.line()
        );
        if label == "install" {
            assert!(
                outcome.log.covers("engine/lib/tsc"),
                "install: the fixture engine probe was not recorded; log:\n{}",
                outcome.log.summary()
            );
            assert!(
                outcome.log.covers("bin/wxml-lsp"),
                "install: the packaged `bin/wxml-lsp` probe was not recorded; log:\n{}",
                outcome.log.summary()
            );
        }
        if label == "env" {
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 1, "env must print exactly one export: {text}");
            assert!(
                lines[0].contains("current/bin/wxml-lsp"),
                "env must export the stable current path: {text}"
            );
        }
        merged.merge(&outcome.log);
    }
    tripwires.assert_not_executed();

    support::assert_exec_audit(&merged, "fixture installer (all commands)");
    assert!(
        merged.execs.iter().any(|record| record.root),
        "fixture audit recorded no root exec"
    );
    println!(
        "EXEC-AUDIT: FIXTURE PASS: install/verify/current/env exec'd {} processes \
         (installer + package entry points), no Node-family exec",
        merged.exec_count()
    );
}

/// Child mode for [`threaded_child_stress_keeps_the_tracer_live`].
///
/// Runs as a no-op in the normal suite; when the stress parent sets
/// `WXML_EXEC_AUDIT_THREAD_CHILD=1` it creates, joins and exits threads, which
/// exercises the tracer's clone/thread-exit handling.
#[test]
fn thread_churn_helper() {
    if std::env::var_os("WXML_EXEC_AUDIT_THREAD_CHILD").is_none() {
        return;
    }
    let handles: Vec<_> = (0_u64..8)
        .map(|index| {
            std::thread::spawn(move || {
                let mut total = index;
                for value in 0..10_000_u64 {
                    total = total.wrapping_add(value);
                }
                total
            })
        })
        .collect();
    for handle in handles {
        let _ = handle.join();
    }
}

/// Repeatedly traces a multithreaded child.
///
/// This is the regression test for the clone/ECHILD race: a thread dropped from
/// the tracer's live set stays in ptrace-stop and deadlocks the child, which the
/// per-iteration timeout would catch. No Node/asset is involved.
#[test]
fn threaded_child_stress_keeps_the_tracer_live() {
    let base = TestDir::new("f3-exec-audit-threads");
    if !support::ensure_audit_available("threaded child stress") {
        return;
    }
    let binary = std::env::current_exe().expect("current test binary");
    let env = vec![
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
        (OsString::from("LANG"), OsString::from("C")),
        (OsString::from("LC_ALL"), OsString::from("C")),
        (
            OsString::from("WXML_EXEC_AUDIT_THREAD_CHILD"),
            OsString::from("1"),
        ),
    ];
    let mut clones = 0_usize;
    let iterations = 40;
    for iteration in 0..iterations {
        let spec = support::spec_for(
            &binary,
            &[
                "--exact".to_owned(),
                "guards::thread_churn_helper".to_owned(),
                "--test-threads=1".to_owned(),
            ],
            &env,
            base.path(),
        );
        let Some(traced) = support::spawn("threaded child stress", spec, Duration::from_secs(20))
        else {
            return;
        };
        let (outcome, stdout, stderr) = traced.wait_with_output();
        support::assert_success(
            &format!("threaded child stress iteration {iteration}"),
            &outcome,
            &stdout,
            &stderr,
        );
        support::assert_exec_audit(
            &outcome.log,
            &format!("threaded child stress iteration {iteration}"),
        );
        clones += outcome
            .log
            .forks
            .iter()
            .filter(|fork| fork.kind == "clone")
            .count();
    }
    assert!(
        clones >= iterations,
        "the stress child recorded only {clones} clone events for {iterations} runs"
    );
    println!(
        "EXEC-AUDIT: STRESS PASS: traced {iterations} multithreaded child runs with {clones} \
         clone events, no stuck tracee"
    );
}
