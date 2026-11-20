//! Opt-in real-asset exec audit (`PASS` evidence).
//!
//! Inputs are resolved through the already committed `f3_real_release` /
//! `f3_acceptance` support: the frozen `assets.lock.json`, the pinned
//! `typescript-linux-x64-7.1.0-beta` and `miniprogram-api-typings-5.3.1`
//! tarballs, the production packager and the production installer. Missing
//! inputs skip loudly unless `WXML_REQUIRE_REAL_ENGINE=1`.
//!
//! Both tests are `#[ignore]`; run with `--ignored`.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crate::lsp::{self, Session};
use crate::real_asset_support::{TestDir, create_private_install_root};
use crate::support;
use crate::tracer::{ExecLog, Spec};

const INSTALL_PROBE_TIMEOUT: Duration = Duration::from_secs(300);

/// Real production installer `install`/`verify`/`current`/`env` under trace.
#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn real_installer_flows_record_only_the_installer_and_pinned_engine() {
    if !support::ensure_audit_available("real installer") {
        return;
    }
    support::validate_pinned_roots("real installer");
    let Some(channel) = support::real_channel("real installer") else {
        return;
    };
    let base = TestDir::new("f3-exec-audit-real-install");
    let (work, home, tmp) = support::workspace_paths(base.path());
    let tripwires = support::install_tripwires(base.path());
    let env = support::clean_env(&tripwires, &home, &tmp);
    let root = create_private_install_root(&base, "install-root");
    let installer = crate::shared::installer_bin();

    let root_text = root.to_str().expect("UTF-8 install root").to_owned();
    let channel_text = channel
        .n1
        .output
        .to_str()
        .expect("UTF-8 release channel")
        .to_owned();
    let version = crate::shared::VERSION_N1.to_owned();
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
                version.clone().into(),
                "--offline".into(),
            ],
            format!("installed {version} (native)"),
        ),
        (
            "verify",
            vec!["verify".into(), "--root".into(), root_text.clone().into()],
            format!("verified {version} (native)"),
        ),
        (
            "current",
            vec!["current".into(), "--root".into(), root_text.clone().into()],
            format!("version: {version}"),
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
    support::assert_exec_audit(&merged, "real installer (all commands)");

    // `install` and `verify` both probe the engine and the packaged Rust
    // binaries. Their `/proc/<pid>/exe` paths must be the real absolute paths.
    let engine = merged
        .execs
        .iter()
        .find(|record| record.exe.to_string_lossy().ends_with("engine/lib/tsc"))
        .unwrap_or_else(|| {
            panic!(
                "the pinned engine was never exec'd; log:\n{}",
                merged.summary()
            )
        });
    assert_eq!(
        engine.exe_name(),
        "tsc",
        "unexpected engine exec: {}",
        engine.line()
    );
    if let Some(value) = std::env::var_os("TSGO_BIN") {
        let given = PathBuf::from(&value);
        let installed = root.join("versions").join(&version).join("engine/lib/tsc");
        let expected = wxml_delivery::manifest::sha256_file(&given)
            .unwrap_or_else(|error| panic!("hash TSGO_BIN {}: {error}", given.display()));
        let actual = wxml_delivery::manifest::sha256_file(&installed).unwrap_or_else(|error| {
            panic!("hash installed engine {}: {error}", installed.display())
        });
        assert_eq!(
            actual,
            expected,
            "the installer-audited engine is not the TSGO_BIN engine ({})",
            given.display()
        );
        println!(
            "EXEC-AUDIT: real installer engine bytes match TSGO_BIN ({})",
            given.display()
        );
    }
    assert!(
        merged.covers("bin/wxml-lsp"),
        "the packaged Rust LSP was never exec'd; log:\n{}",
        merged.summary()
    );

    let distinct: BTreeSet<String> = merged
        .execs
        .iter()
        .map(|record| record.exe.to_string_lossy().into_owned())
        .collect();
    println!(
        "EXEC-AUDIT: PASS (real installer): install/verify/current/env exec'd {} processes, \
         {} distinct absolute executables; engine={}",
        merged.exec_count(),
        distinct.len(),
        engine.exe.display()
    );
    for executable in &distinct {
        println!("EXEC-AUDIT: real installer executable: {executable}");
    }
}

/// Real `wxml-lsp --stdio --backend native` session under trace.
#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn real_native_stdio_session_execs_only_wxml_lsp_and_pinned_engine() {
    if !support::ensure_audit_available("real native LSP") {
        return;
    }
    support::validate_pinned_roots("real native LSP");
    let base = TestDir::new("f3-exec-audit-real-lsp");
    // Prebuilt-package mode (`WXML_F3_LSP_BIN=<pkg>/bin/wxml-lsp`) audits the
    // exact binary and engine roots supplied by the operator. Otherwise the
    // tarball -> packager -> installer path prepares an equivalent package.
    let prebuilt = support::prepare_prebuilt_package(base.path(), "real native LSP");
    let (work, home, tmp) = support::workspace_paths(base.path());
    let tripwires = support::install_tripwires(base.path());
    let env = support::clean_env(&tripwires, &home, &tmp);
    let version = crate::shared::VERSION_N1;

    let binary = if let Some(package) = &prebuilt {
        tripwires.assert_not_executed();
        package.join("bin/wxml-lsp")
    } else {
        let Some(channel) = support::real_channel("real native LSP") else {
            return;
        };
        let root = create_private_install_root(&base, "install-root");
        let installer = crate::shared::installer_bin();
        // Installation itself is the other test's audit; here it only prepares
        // the verified package tree the native server discovers from its path.
        let mut install = crate::shared::clean_command(&installer, &tripwires, &work);
        install.args([
            "install",
            "--root",
            root.to_str().expect("UTF-8 install root"),
            "--from-dir",
            channel.n1.output.to_str().expect("UTF-8 release channel"),
            "--version",
            version,
            "--offline",
        ]);
        let installed =
            crate::shared::run_with_deadline(&mut install, INSTALL_PROBE_TIMEOUT, "real install");
        crate::shared::assert_success(&installed, "real native install");
        tripwires.assert_not_executed();
        root.join("current/bin/wxml-lsp")
    };

    let project = base.path().join("project");
    lsp::write_project(&project);
    assert!(
        binary.is_file(),
        "native LSP binary is missing: {}",
        binary.display()
    );

    let spec = Spec::new(&binary, &project)
        .args(["--stdio", "--backend", "native"])
        .envs(env)
        .piped_stdin();
    let mut session = match Session::start(spec, &project) {
        Ok(session) => session,
        Err(crate::tracer::TraceError::Blocked(reason)) => {
            support::blocked("real native LSP", &reason);
            return;
        }
        Err(crate::tracer::TraceError::Failed(reason)) => {
            panic!("real native LSP: cannot start traced server: {reason}")
        }
    };

    let uri = format!("file://{}/pages/index/index.wxml", project.display());
    session.did_open(&uri, "wxml", lsp::PROJECT_WXML);
    let symbols = session.document_symbol(&uri);
    assert!(
        symbols.get("error").is_none(),
        "documentSymbol failed: {}",
        session.describe(&symbols)
    );
    let names: Vec<&str> = symbols["result"]
        .as_array()
        .expect("documentSymbol result array")
        .iter()
        .filter_map(|symbol| symbol["name"].as_str())
        .collect();
    assert!(
        names.contains(&"view") && names.contains(&"button"),
        "WXML outline is missing fixture symbols: {symbols}"
    );
    println!("EXEC-AUDIT: native LSP documentSymbol: {names:?}");

    let hover = session.hover(&uri, 0, 23);
    assert!(
        hover.get("error").is_none(),
        "hover failed: {}",
        session.describe(&hover)
    );
    let contents = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover contents value");
    assert!(
        !contents.trim().is_empty(),
        "hover returned empty contents: {}",
        session.describe(&hover)
    );
    println!(
        "EXEC-AUDIT: native LSP hover: {}",
        contents.lines().next().unwrap_or("<empty line>")
    );

    // The engine is spawned lazily on the first native query; wait until the
    // tracer has observed it before shutting the session down.
    let live = support::wait_for_exec(
        "real native LSP",
        "engine/lib/tsc",
        Duration::from_secs(30),
        || session.exec_log(),
    );
    assert!(
        live.covers("bin/wxml-lsp"),
        "the traced LSP exec is missing; log:\n{}",
        live.summary()
    );

    let outcome = session.shutdown();
    support::assert_exit_success("real native LSP shutdown", &outcome);
    support::print_outcome("real-native-lsp", &outcome);
    support::assert_exec_audit(&outcome.log, "real native LSP");

    let lsp_exec = outcome
        .log
        .execs
        .iter()
        .find(|record| record.root)
        .unwrap_or_else(|| panic!("no root LSP exec recorded"));
    if prebuilt.is_some() {
        assert!(
            lsp_exec
                .exe
                .to_string_lossy()
                .ends_with("native-package/bin/wxml-lsp"),
            "the traced LSP is not the prebuilt package binary: {}",
            lsp_exec.line()
        );
    } else {
        assert!(
            lsp_exec
                .exe
                .to_string_lossy()
                .ends_with(&format!("versions/{version}/bin/wxml-lsp")),
            "the traced LSP is not the installed versioned binary: {}",
            lsp_exec.line()
        );
    }
    let engine = outcome
        .log
        .execs
        .iter()
        .find(|record| record.exe.to_string_lossy().ends_with("engine/lib/tsc"))
        .unwrap_or_else(|| {
            panic!(
                "the pinned engine was never exec'd; log:\n{}",
                outcome.log.summary()
            )
        });
    assert_eq!(
        engine.exe_name(),
        "tsc",
        "unexpected engine exec: {}",
        engine.line()
    );

    let distinct: BTreeSet<String> = outcome
        .log
        .execs
        .iter()
        .map(|record| record.exe.to_string_lossy().into_owned())
        .collect();
    println!(
        "EXEC-AUDIT: PASS (real native LSP): initialize/documentSymbol/hover/shutdown completed; \
         {} exec(s), {} distinct absolute executables; no Node-family exec",
        outcome.log.exec_count(),
        distinct.len()
    );
    for executable in &distinct {
        println!("EXEC-AUDIT: real native LSP executable: {executable}");
    }
}
