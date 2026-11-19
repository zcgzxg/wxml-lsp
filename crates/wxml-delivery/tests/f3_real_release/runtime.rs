//! F3-04/05/06/07/10 runtime acceptance on a real release package.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use wxml_delivery::install::InstallRoot;
use wxml_delivery::manifest::set_mode;
use wxml_delivery::manifest_v2::{ENGINE_NAME, ENGINE_VERSION};
use wxml_delivery::state::{InstallState, Journal, TransactionOp, journal_dir, state_path};

use crate::lsp::{
    PROJECT_WXML, Session, assert_no_node_process, observe_process_tree, write_project,
};
use crate::real_asset_support::{
    TestDir, Tripwires, assert_journal_absent_or_empty, assert_success, mode_of, run_with_timeout,
    stderr, stdout,
};
use crate::support::{
    PROTOCOL, VERSION_N1, VERSION_N2, assert_clean_environment_probe, assert_installed_version,
    build_release, clean_command, install_root, installed_fingerprint, provision_real_assets,
    resolve_real_inputs, retarget_binary_version, write_input_tree,
};

fn workspace(base: &TestDir) -> (PathBuf, PathBuf, PathBuf) {
    let work = base.path().join("work");
    let home = base.path().join("home");
    let tmp = base.path().join("tmp");
    for directory in [&work, &home, &tmp] {
        fs::create_dir(directory).expect("create isolated directory");
        set_mode(directory, 0o700).expect("private isolated directory");
    }
    (work, home, tmp)
}

fn run_installer_bin(
    installer: &Path,
    args: &[&str],
    tripwires: &Tripwires,
    work: &Path,
    label: &str,
) -> Output {
    let mut command = clean_command(installer, tripwires, work);
    command.args(args);
    let output = run_with_timeout(&mut command, Duration::from_secs(180), label);
    tripwires.assert_not_executed();
    output
}

fn assert_action(output: &Output, action: &str, version: &str, native_suffix: bool) {
    assert_success(output, action);
    let text = stdout(output);
    let action_line = if native_suffix {
        format!("{action} {version} (native)")
    } else {
        format!("{action} {version}")
    };
    assert!(text.lines().any(|line| line == action_line), "{text}");
    assert!(
        text.lines()
            .any(|line| line == format!("engine: {ENGINE_NAME} {ENGINE_VERSION}")),
        "{text}"
    );
}

fn assert_state(root: &InstallRoot, active: &str, previous: Option<&str>, op: TransactionOp) {
    let state = InstallState::read(root)
        .expect("read state.json")
        .expect("state.json exists");
    assert_eq!(state.active, active);
    assert_eq!(state.previous.as_deref(), previous);
    assert_eq!(state.engine.name, ENGINE_NAME);
    assert_eq!(state.engine.version, ENGINE_VERSION);
    assert_eq!(state.engine.protocol_version, PROTOCOL);
    let last = state.last_transaction.as_ref().expect("last transaction");
    assert_eq!(last.op, op);
    assert_eq!(last.target, active);
    assert_eq!(last.previous.as_deref(), previous);
    state.validate_against(root).expect("state matches disk");
}

fn assert_current(root: &InstallRoot, version: &str) {
    assert_eq!(
        root.current_version().expect("current version").as_deref(),
        Some(version)
    );
    let current = fs::canonicalize(root.path().join("current")).expect("canonical current");
    let target =
        fs::canonicalize(root.versions_dir().join(version)).expect("canonical version dir");
    assert_eq!(current, target, "current does not point at {version}");
}

fn assert_clean_state(root: &InstallRoot) {
    assert!(
        Journal::list(root).expect("list journals").is_empty(),
        "transaction journals were left behind"
    );
    assert_journal_absent_or_empty(root);
    if root.tmp_dir().exists() {
        assert!(
            fs::read_dir(root.tmp_dir())
                .expect("read tmp")
                .next()
                .is_none(),
            "staging files were left in tmp/"
        );
    }
}

fn assert_versions(root: &InstallRoot, expected: &[&str]) {
    let mut versions: Vec<String> = fs::read_dir(root.versions_dir())
        .expect("read versions")
        .map(|entry| {
            entry
                .expect("version entry")
                .file_name()
                .to_str()
                .expect("UTF-8 version")
                .to_owned()
        })
        .collect();
    versions.sort_by(|left, right| wxml_delivery::semver::compare_versions(left, right));
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(versions, expected);
}

fn assert_cli_views(root_path: &Path, installer: &Path, tripwires: &Tripwires, work: &Path) {
    let root_text = root_path.to_str().expect("UTF-8 root");
    let verify = run_installer_bin(
        installer,
        &["verify", "--root", root_text, "--version", VERSION_N1],
        tripwires,
        work,
        "verify",
    );
    assert_success(&verify, "verify");
    let text = stdout(&verify);
    assert!(
        text.lines()
            .any(|line| line == format!("verified {VERSION_N1} (native)")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line == format!("engine version: {ENGINE_VERSION}")),
        "{text}"
    );

    let current = run_installer_bin(
        installer,
        &["current", "--root", root_text],
        tripwires,
        work,
        "current",
    );
    assert_success(&current, "current");
    let text = stdout(&current);
    for expected in [
        format!("version: {VERSION_N1}"),
        "platform: linux-x86_64".to_owned(),
        format!("engine: {ENGINE_NAME} {ENGINE_VERSION}"),
    ] {
        assert!(text.lines().any(|line| line == expected), "{text}");
    }

    let env = run_installer_bin(
        installer,
        &["env", "--root", root_text],
        tripwires,
        work,
        "env",
    );
    assert_success(&env, "env");
    let text = stdout(&env);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "env must print exactly one export: {text}");
    assert!(lines[0].starts_with("export WXML_LSP_BIN="), "{text}");
    assert!(lines[0].contains("current/bin/wxml-lsp"), "{text}");
}

fn assert_installed_binary(binary: &Path, tripwires: &Tripwires, work: &Path) {
    let output = run_installer_bin(
        binary,
        &["--version"],
        tripwires,
        work,
        "installed --version",
    );
    assert_success(&output, "installed --version");
    assert_eq!(
        stdout(&output).trim_end(),
        format!("wxml-lsp {VERSION_N1}"),
        "installed binary reported an unexpected version"
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn real_release_install_verify_current_env_without_node() {
    let Some(inputs) = resolve_real_inputs("real_release_install") else {
        return;
    };
    let base = TestDir::new("f3-install");
    let provisioned = provision_real_assets(&base, &inputs);
    let tree = write_input_tree(&base, &inputs, &provisioned);
    let release = build_release(
        &base.path().join("release"),
        &tree,
        &provisioned,
        VERSION_N1,
        true,
    );
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    assert_clean_environment_probe(&tripwires, &work);

    let root_path = install_root(&base, "install-root");
    let root_text = root_path.to_str().expect("UTF-8 root");
    let release_text = release.output.to_str().expect("UTF-8 release");
    let output = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            root_text,
            "--from-dir",
            release_text,
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "install real release",
    );
    assert_action(&output, "installed", VERSION_N1, true);

    let root = InstallRoot::open(&root_path)
        .expect("open install root")
        .expect("install root exists");
    assert_state(&root, VERSION_N1, None, TransactionOp::Install);
    assert_current(&root, VERSION_N1);
    assert_versions(&root, &[VERSION_N1]);
    assert_clean_state(&root);
    assert_installed_version(&root_path, VERSION_N1, &tripwires);
    assert_cli_views(&root_path, &inputs.install_bin, &tripwires, &work);
    assert_installed_binary(&root_path.join("current/bin/wxml-lsp"), &tripwires, &work);

    let project = base.path().join("project");
    write_project(&project);
    let mut session = Session::start(
        &root_path.join("current/bin/wxml-lsp"),
        &project,
        &tripwires,
        &work,
    );
    let uri = format!("file://{}/pages/index/index.wxml", project.display());
    session.did_open(&uri, "wxml", PROJECT_WXML);
    let diagnostics = session.wait_diagnostics(&uri);
    println!(
        "LSP-DIAGNOSTICS: {} item(s) for pages/index/index.wxml",
        diagnostics.as_array().map_or(0, Vec::len)
    );

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
    println!("LSP-DOCUMENT-SYMBOL: {names:?}");

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
        "LSP-HOVER: {}",
        contents.lines().next().unwrap_or("<empty line>")
    );

    let processes = observe_process_tree(session.pid(), Duration::from_millis(750));
    assert_no_node_process(&processes);
    println!(
        "LSP-NATIVE: initialize/documentSymbol/hover completed; observed processes: {}",
        processes.join(" | ")
    );
    session.shutdown();

    assert_clean_state(&root);
    assert_eq!(mode_of(&root_path), 0o700);
    tripwires.assert_not_executed();
    println!(
        "F3-04: real package installed at {} (engine {}, archiveBytes {}), verify/current/env clean",
        root_path.display(),
        ENGINE_VERSION,
        release.archive_size
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn real_release_upgrade_rollback_and_recover_without_node() {
    let Some(inputs) = resolve_real_inputs("real_release_transitions") else {
        return;
    };
    let base = TestDir::new("f3-transitions");
    let provisioned = provision_real_assets(&base, &inputs);
    let tree = write_input_tree(&base, &inputs, &provisioned);
    let channel = base.path().join("channel");
    let n1_release = build_release(&channel, &tree, &provisioned, VERSION_N1, true);
    // No second production release exists at this commit. The N2 fixture keeps
    // the real engine/typings and every other binary byte, replacing only the
    // same-length embedded version constant that the install probe compares.
    let tree_n2 = base.path().join("input-tree-n2");
    copy_tree(&tree, &tree_n2);
    for name in ["wxml-lsp", "wxml-lsp-install"] {
        retarget_binary_version(
            &tree.join("bin").join(name),
            &tree_n2.join("bin").join(name),
            VERSION_N1,
            VERSION_N2,
        );
    }
    let n2_release = build_release(&channel, &tree_n2, &provisioned, VERSION_N2, true);
    assert_eq!(n1_release.output, n2_release.output);
    let index = wxml_delivery::index_v2::read_index_v2(&channel.join("index.json"))
        .expect("read two-version channel index");
    assert!(index.find(VERSION_N1, "linux-x86_64").is_ok());
    assert!(index.find(VERSION_N2, "linux-x86_64").is_ok());

    let (work, _home, tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let root_path = install_root(&base, "transitions-root");
    let root_text = root_path.to_str().expect("UTF-8 root");
    let channel_text = channel.to_str().expect("UTF-8 channel");

    let install = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            root_text,
            "--from-dir",
            channel_text,
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "install N1",
    );
    assert_action(&install, "installed", VERSION_N1, true);
    let root = InstallRoot::open(&root_path)
        .expect("open root")
        .expect("root exists");
    assert_state(&root, VERSION_N1, None, TransactionOp::Install);
    assert_installed_version(&root_path, VERSION_N1, &tripwires);
    let n1_fingerprint = installed_fingerprint(&root_path, VERSION_N1);

    let upgrade = run_installer_bin(
        &inputs.install_bin,
        &[
            "upgrade",
            "--root",
            root_text,
            "--from-dir",
            channel_text,
            "--offline",
        ],
        &tripwires,
        &work,
        "upgrade N2",
    );
    assert_action(&upgrade, "upgraded to", VERSION_N2, true);
    assert_state(&root, VERSION_N2, Some(VERSION_N1), TransactionOp::Upgrade);
    assert_current(&root, VERSION_N2);
    assert_versions(&root, &[VERSION_N1, VERSION_N2]);
    assert_clean_state(&root);
    assert_installed_version(&root_path, VERSION_N2, &tripwires);
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N1),
        n1_fingerprint,
        "N1 tree changed during upgrade"
    );
    let n2_fingerprint = installed_fingerprint(&root_path, VERSION_N2);

    let rollback = run_installer_bin(
        &inputs.install_bin,
        &[
            "rollback",
            "--root",
            root_text,
            "--to",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "rollback N1",
    );
    assert_action(&rollback, "rolled back to", VERSION_N1, false);
    assert_state(&root, VERSION_N1, Some(VERSION_N2), TransactionOp::Rollback);
    assert_current(&root, VERSION_N1);
    assert_installed_version(&root_path, VERSION_N1, &tripwires);
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N1),
        n1_fingerprint
    );
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N2),
        n2_fingerprint
    );

    let state_before = fs::read(state_path(&root)).expect("state before recover");
    let current_before =
        fs::canonicalize(root.path().join("current")).expect("current before recover");
    fs::write(tmp.join("abandoned-staging"), b"stale").expect("write stale staging file");
    let journal = Journal::new(
        TransactionOp::Install,
        "0.3.0",
        None,
        "2026-11-19T00:00:00Z",
    )
    .expect("staged journal");
    journal.write(&root).expect("write staged journal");
    assert!(
        journal_dir(&root)
            .join(format!("{}.json", journal.id))
            .is_file()
    );

    let recover = run_installer_bin(
        &inputs.install_bin,
        &["recover", "--root", root_text],
        &tripwires,
        &work,
        "recover",
    );
    assert_success(&recover, "recover");
    let text = stdout(&recover);
    assert!(
        text.lines()
            .any(|line| line == "staged 0.3.0 -> removed temporary state; current unchanged"),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line == format!("current: {VERSION_N1}")),
        "{text}"
    );
    assert!(Journal::list(&root).expect("list journals").is_empty());
    assert_clean_state(&root);
    assert_eq!(
        fs::read(state_path(&root)).expect("state after recover"),
        state_before,
        "recover changed state.json"
    );
    assert_eq!(
        fs::canonicalize(root.path().join("current")).expect("current after recover"),
        current_before,
        "recover changed current"
    );
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N1),
        n1_fingerprint
    );
    assert_eq!(
        installed_fingerprint(&root_path, VERSION_N2),
        n2_fingerprint
    );

    let recover_again = run_installer_bin(
        &inputs.install_bin,
        &["recover", "--root", root_text],
        &tripwires,
        &work,
        "recover idempotent",
    );
    assert_success(&recover_again, "recover idempotent");
    assert_eq!(
        stdout(&recover_again).lines().next(),
        Some("no recovery actions were needed")
    );

    tripwires.assert_not_executed();
    println!(
        "F3-06/F3-07: real {VERSION_N1} -> {VERSION_N2} -> {VERSION_N1}, trees intact, \
         offline rollback re-verified, staged journal recovered without activation"
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn tampered_real_release_archive_fails_before_activation() {
    let Some(inputs) = resolve_real_inputs("real_release_tamper") else {
        return;
    };
    let base = TestDir::new("f3-tamper");
    let provisioned = provision_real_assets(&base, &inputs);
    let tree = write_input_tree(&base, &inputs, &provisioned);
    let release = build_release(
        &base.path().join("release"),
        &tree,
        &provisioned,
        VERSION_N1,
        true,
    );
    let tampered = base.path().join("tampered");
    copy_tree(&release.output, &tampered);
    let archive = tampered.join(release.archive.file_name().expect("archive file name"));
    let mut bytes = fs::read(&archive).expect("read archive for tampering");
    assert!(!bytes.is_empty());
    *bytes.last_mut().expect("non-empty archive") ^= 0xff;
    fs::write(&archive, &bytes).expect("write tampered archive");

    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let root_path = install_root(&base, "tamper-root");
    let output = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            root_path.to_str().expect("UTF-8 root"),
            "--from-dir",
            tampered.to_str().expect("UTF-8 tampered release"),
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "tampered install",
    );
    assert!(
        !output.status.success(),
        "tampered archive unexpectedly installed: stdout={} stderr={}",
        stdout(&output),
        stderr(&output)
    );
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.to_ascii_lowercase().contains("integrity")
            || combined.contains("SHA-256")
            || combined.contains("SRI"),
        "tampered archive was not rejected by an integrity check: {combined}"
    );
    for name in ["current", "versions", "state.json"] {
        assert!(
            !root_path.join(name).exists(),
            "tampered install created {name}"
        );
    }
    if root_path.join("tmp").exists() {
        assert!(
            fs::read_dir(root_path.join("tmp"))
                .expect("read tmp")
                .next()
                .is_none(),
            "tampered install left staging files"
        );
    }
    assert_eq!(mode_of(&root_path), 0o700);
    tripwires.assert_not_executed();
    println!(
        "F3-10: tampered real archive rejected before activation: {}",
        combined.trim()
    );
}

#[test]
#[ignore = "requires pinned real assets; run with --ignored"]
fn real_release_offline_cache_reinstall_and_fail_closed() {
    let Some(inputs) = resolve_real_inputs("real_release_cache") else {
        return;
    };
    let base = TestDir::new("f3-cache");
    let provisioned = provision_real_assets(&base, &inputs);
    let tree = write_input_tree(&base, &inputs, &provisioned);
    let release = build_release(
        &base.path().join("release"),
        &tree,
        &provisioned,
        VERSION_N1,
        true,
    );
    let (work, _home, _tmp) = workspace(&base);
    let tripwires = Tripwires::install(base.path());
    let root_path = install_root(&base, "cache-root");
    let root_text = root_path.to_str().expect("UTF-8 root");
    let release_text = release.output.to_str().expect("UTF-8 release");

    let install = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            root_text,
            "--from-dir",
            release_text,
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "install N1 for cache",
    );
    assert_action(&install, "installed", VERSION_N1, true);

    let root = InstallRoot::open(&root_path)
        .expect("open cache root")
        .expect("cache root exists");
    assert_state(&root, VERSION_N1, None, TransactionOp::Install);
    assert_installed_version(&root_path, VERSION_N1, &tripwires);
    let cache = wxml_delivery::acquire_v2::native_cache_dir(&root);
    assert!(
        cache
            .join(wxml_delivery::acquire_v2::NATIVE_CACHE_INDEX)
            .is_file(),
        "a --from-dir install did not cache the verified index"
    );
    let archives = cache_archives(&cache);
    assert_eq!(
        archives.len(),
        1,
        "expected one cached archive below {}: {archives:?}",
        cache.display()
    );
    println!(
        "CACHE: index and archive stored below {} (archive {})",
        cache.display(),
        archives[0].display()
    );

    // Drop the installed tree/state/current but keep the verified cache: a
    // source-less `--offline` install must reinstall only from cached bytes.
    reset_installation_keep_cache(&root_path);
    let reinstall = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            root_text,
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "offline cache reinstall",
    );
    assert_action(&reinstall, "installed", VERSION_N1, true);
    let root = InstallRoot::open(&root_path)
        .expect("open reinstalled root")
        .expect("reinstalled root exists");
    assert_state(&root, VERSION_N1, None, TransactionOp::Install);
    assert_current(&root, VERSION_N1);
    assert_installed_version(&root_path, VERSION_N1, &tripwires);
    assert_clean_state(&root);
    assert_cli_views(&root_path, &inputs.install_bin, &tripwires, &work);
    println!("F3-12: source-less offline reinstall used the verified archive cache");

    // A corrupted cached archive must fail closed before activation.
    let cache = wxml_delivery::acquire_v2::native_cache_dir(&root);
    let archives = cache_archives(&cache);
    assert_eq!(archives.len(), 1, "{archives:?}");
    let mut bytes = fs::read(&archives[0]).expect("read cached archive");
    *bytes.last_mut().expect("non-empty cached archive") ^= 0xff;
    fs::write(&archives[0], &bytes).expect("write corrupted cached archive");
    reset_installation_keep_cache(&root_path);
    let corrupt = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            root_text,
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "corrupt cache reinstall",
    );
    assert!(
        !corrupt.status.success(),
        "a corrupted cache archive installed: stdout={} stderr={}",
        stdout(&corrupt),
        stderr(&corrupt)
    );
    let combined = format!("{}{}", stdout(&corrupt), stderr(&corrupt));
    assert!(
        combined.to_ascii_lowercase().contains("integrity")
            || combined.contains("SHA-256")
            || combined.contains("SRI"),
        "corrupted cache was not rejected by an integrity check: {combined}"
    );
    assert!(!root_path.join("current").exists());
    assert!(!root_path.join("state.json").exists());
    if root_path.join("versions").exists() {
        assert!(
            fs::read_dir(root_path.join("versions"))
                .expect("read versions")
                .next()
                .is_none(),
            "corrupted cache install activated a version"
        );
    }
    println!(
        "F3-12: corrupted cache rejected before activation: {}",
        combined.trim()
    );

    // No cache and no source: the installer must fail without creating state
    // or contacting the dead proxies inherited from `clean_command`.
    let empty_path = install_root(&base, "empty-cache-root");
    let empty = run_installer_bin(
        &inputs.install_bin,
        &[
            "install",
            "--root",
            empty_path.to_str().expect("UTF-8 empty root"),
            "--version",
            VERSION_N1,
            "--offline",
        ],
        &tripwires,
        &work,
        "empty cache install",
    );
    assert!(
        !empty.status.success(),
        "an empty cache installed without a source: {}",
        stdout(&empty)
    );
    let text = format!("{}{}", stdout(&empty), stderr(&empty));
    assert!(
        text.contains("no cached native index")
            || text.contains("no verified cache entry is available for this version while offline"),
        "empty-cache failure did not name the missing cache: {text}"
    );
    for name in ["current", "state.json", "versions"] {
        assert!(
            !empty_path.join(name).exists(),
            "empty-cache install created {name}"
        );
    }
    tripwires.assert_not_executed();
    println!("F3-12: empty cache fails closed without a source and without network");
}

fn cache_archives(cache: &Path) -> Vec<PathBuf> {
    let mut archives = Vec::new();
    for entry in fs::read_dir(cache).expect("read native cache") {
        let directory = entry.expect("cache entry").path();
        if !directory.is_dir() {
            continue;
        }
        let archive = directory.join(wxml_delivery::acquire_v2::NATIVE_CACHE_ARCHIVE);
        if archive.is_file() {
            archives.push(archive);
        }
    }
    archives.sort();
    archives
}

/// Removes activation state and installed trees while keeping `cache/`.
fn reset_installation_keep_cache(root: &Path) {
    for name in ["current", "versions", "state.json", "txn", "tmp"] {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                fs::remove_dir_all(&path)
                    .unwrap_or_else(|error| panic!("remove {}: {error}", path.display()));
            }
            Ok(_) => {
                fs::remove_file(&path)
                    .unwrap_or_else(|error| panic!("remove {}: {error}", path.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("metadata for {}: {error}", path.display()),
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("create copied root");
    set_mode(destination, 0o755).expect("copied root mode");
    let mut entries: Vec<_> = fs::read_dir(source)
        .expect("read source tree")
        .map(|entry| entry.expect("source entry").path())
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("file name"));
        let metadata = fs::symlink_metadata(&entry).expect("source metadata");
        if metadata.file_type().is_dir() {
            copy_tree(&entry, &target);
        } else {
            fs::copy(&entry, &target).expect("copy file");
            set_mode(&target, metadata.permissions().mode() & 0o777).expect("copied file mode");
        }
    }
}
