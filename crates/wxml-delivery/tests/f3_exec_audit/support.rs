//! Shared gates and assertions for the F3-02 exec audit.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::real_asset_support::Tripwires;
use crate::tracer::{self, ExecLog, Spec, TraceError, TraceOutcome};

/// Node-family executable basenames that must never appear in the audit.
pub const FORBIDDEN_BASENAMES: &[&str] = &["node", "nodejs", "npm", "npx", "yarn", "pnpm", "bun"];
/// Environment keys that only the Node backend consumes.
pub const NODE_BACKEND_ENV_KEYS: &[&str] = &["WXML_TYPE_BACKEND", "WXML_BACKEND_DIR"];

/// Timeout for one installer invocation under trace.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Timeout for the whole traced native stdio session.
pub const LSP_TIMEOUT: Duration = Duration::from_secs(180);

fn hard_gate_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value.is_empty() => false,
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => true,
        Ok(value) => panic!("{name} must be unset, empty, `1` or `true`; got `{value}`"),
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} must be valid UTF-8"),
    }
}

/// `WXML_REQUIRE_EXEC_AUDIT=1` or `WXML_REQUIRE_REAL_ENGINE=1` fail closed.
pub fn required_mode() -> bool {
    hard_gate_enabled("WXML_REQUIRE_EXEC_AUDIT") || hard_gate_enabled("WXML_REQUIRE_REAL_ENGINE")
}

/// Reports a blocked audit. A hard gate turns the block into a failure.
pub fn blocked(test: &str, reason: &str) {
    assert!(
        !required_mode(),
        "{test}: EXEC-AUDIT BLOCKED and WXML_REQUIRE_EXEC_AUDIT/WXML_REQUIRE_REAL_ENGINE \
         is set: {reason}"
    );
    println!("EXEC-AUDIT: BLOCKED ({test}): {reason}");
}

/// Runs `/bin/true` through the tracer to prove ptrace works on this host.
pub fn preflight() -> Result<(), TraceError> {
    if std::env::var_os("WXML_EXEC_AUDIT_FORCE_BLOCKED").is_some() {
        return Err(TraceError::Blocked(
            "forced by WXML_EXEC_AUDIT_FORCE_BLOCKED".to_owned(),
        ));
    }
    let cwd = std::env::temp_dir();
    let spec = Spec::new("/bin/true", &cwd)
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C");
    let traced = tracer::spawn(spec, Duration::from_secs(10))?;
    let (outcome, _stdout, stderr) = traced.wait_with_output();
    if !outcome.exit.success() {
        return Err(TraceError::Failed(format!(
            "preflight /bin/true exited {:?} (stderr {})",
            outcome.exit,
            String::from_utf8_lossy(&stderr)
        )));
    }
    if outcome.log.exec_count() == 0 {
        return Err(TraceError::Failed(
            "preflight recorded no execve event".to_owned(),
        ));
    }
    if let Some(record) = outcome.log.execs.iter().find(|record| record.root) {
        if !record.exe.is_absolute() {
            return Err(TraceError::Blocked(
                "/proc/<pid>/exe did not resolve; /proc is unavailable or restricted".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Returns false (after printing `BLOCKED`) when ptrace is unavailable.
pub fn ensure_audit_available(test: &str) -> bool {
    // Evaluate the gate eagerly: an invalid value must fail closed even when
    // this host could trace successfully.
    let _ = required_mode();
    match preflight() {
        Ok(()) => true,
        Err(TraceError::Blocked(reason)) => {
            blocked(test, &reason);
            false
        }
        Err(TraceError::Failed(reason)) => {
            panic!("{test}: exec audit preflight failed: {reason}")
        }
    }
}

/// Validates the optional pinned-root environment used by the real backend
/// tests (`TSGO_BIN`, `TSGO_ASSET_ROOT`, `TSGO_TYPINGS_ROOT`). These roots are
/// verification inputs for the exec audit: `TSGO_BIN` must be the same engine
/// bytes the installer audits.
pub fn validate_pinned_roots(test: &str) {
    if let Some(value) = std::env::var_os("TSGO_BIN") {
        let path = PathBuf::from(&value);
        assert!(
            path.is_file(),
            "{test}: TSGO_BIN is not a file: {}",
            path.display()
        );
    }
    if let Some(value) = std::env::var_os("TSGO_ASSET_ROOT") {
        let path = PathBuf::from(&value);
        assert!(
            path.join("lib/tsc").is_file() || path.join("tsc").is_file(),
            "{test}: TSGO_ASSET_ROOT must be an engine root (lib/tsc) or engine/lib: {}",
            path.display()
        );
    }
    if let Some(value) = std::env::var_os("TSGO_TYPINGS_ROOT") {
        let path = PathBuf::from(&value);
        assert!(
            path.join("index.d.ts").is_file(),
            "{test}: TSGO_TYPINGS_ROOT must contain index.d.ts: {}",
            path.display()
        );
    }
}

/// Installs Node tripwire shims and a marker; the returned `Tripwires` also
/// proves the production binaries never resolved a Node command through PATH.
pub fn install_tripwires(base: &Path) -> Tripwires {
    let tripwires = Tripwires::install(base);
    println!("EXEC-AUDIT: tripwire PATH={}", tripwires.path().display());
    tripwires
}

/// A clean environment: no real Node on PATH and explicit Node poisons.
///
/// `WXML_TYPE_BACKEND` and `WXML_BACKEND_DIR` are deliberately *absent*: the
/// audit asserts that no exec'd process carries those Node-backend keys.
pub fn clean_env(tripwires: &Tripwires, home: &Path, tmp: &Path) -> Vec<(OsString, OsString)> {
    let node = tripwires.path().join("node");
    vec![
        ("PATH".into(), tripwires.path().as_os_str().to_owned()),
        ("HOME".into(), home.as_os_str().to_owned()),
        ("XDG_DATA_HOME".into(), home.as_os_str().to_owned()),
        ("TMPDIR".into(), tmp.as_os_str().to_owned()),
        ("LANG".into(), OsString::from("C")),
        ("LC_ALL".into(), OsString::from("C")),
        ("NO_COLOR".into(), OsString::from("1")),
        ("WXML_NODE".into(), node.as_os_str().to_owned()),
        ("NODE".into(), node.as_os_str().to_owned()),
        ("HTTP_PROXY".into(), OsString::from("http://127.0.0.1:9")),
        ("HTTPS_PROXY".into(), OsString::from("http://127.0.0.1:9")),
        ("ALL_PROXY".into(), OsString::from("http://127.0.0.1:9")),
    ]
}

/// Builds a traced invocation with an absolute program, argv and env.
pub fn spec_for(program: &Path, args: &[String], env: &[(OsString, OsString)], cwd: &Path) -> Spec {
    Spec::new(program, cwd)
        .args(args.iter().map(OsString::from))
        .envs(env.to_vec())
}

/// `EXEC-AUDIT` lines for one invocation.
pub fn print_outcome(label: &str, outcome: &TraceOutcome) {
    println!(
        "EXEC-AUDIT: {label}: execs={} forks={} exit={:?} timed_out={} survivors={:?} elapsed={:?}",
        outcome.log.exec_count(),
        outcome.log.forks.len(),
        outcome.exit,
        outcome.timed_out,
        outcome.survivors,
        outcome.elapsed
    );
    for record in &outcome.log.execs {
        println!("EXEC-AUDIT: {label}: {}", record.line());
    }
}

/// Runs one preflight and spawns a traced child, mapping ptrace restrictions
/// to a loud `BLOCKED` (or a hard failure under a gate).
pub fn spawn(test: &str, spec: Spec, timeout: Duration) -> Option<tracer::TracedChild> {
    match tracer::spawn(spec, timeout) {
        Ok(child) => Some(child),
        Err(TraceError::Blocked(reason)) => {
            blocked(test, &reason);
            None
        }
        Err(TraceError::Failed(reason)) => {
            panic!("{test}: cannot start the traced child: {reason}")
        }
    }
}

/// Asserts a traced invocation exited successfully (without its streams).
pub fn assert_exit_success(label: &str, outcome: &TraceOutcome) {
    assert!(
        !outcome.timed_out,
        "{label}: timed out after {:?}; exec log:\n{}",
        outcome.elapsed,
        outcome.log.summary()
    );
    assert!(
        outcome.exit.success(),
        "{label}: exit {:?}; survivors={:?}; exec log:\n{}",
        outcome.exit,
        outcome.survivors,
        outcome.log.summary()
    );
}

/// Asserts that a traced command completed successfully and produced output.
pub fn assert_success(label: &str, outcome: &TraceOutcome, stdout: &[u8], stderr: &[u8]) {
    assert!(
        !outcome.timed_out,
        "{label}: timed out after {:?}; exec log:\n{}\nstdout={}\nstderr={}",
        outcome.elapsed,
        outcome.log.summary(),
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    assert!(
        outcome.exit.success(),
        "{label}: exit {:?}; survivors={:?}; exec log:\n{}\nstdout={}\nstderr={}",
        outcome.exit,
        outcome.survivors,
        outcome.log.summary(),
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
}

/// Finds every Node-family / Node-backend trace in one exec log.
pub fn forbidden_evidence(log: &ExecLog) -> Vec<String> {
    let mut evidence = Vec::new();
    for record in &log.execs {
        let exe = record.exe.to_string_lossy().to_ascii_lowercase();
        let exe_name = record.exe_name();
        if FORBIDDEN_BASENAMES.contains(&exe_name.as_str()) {
            evidence.push(format!(
                "Node-family executable `{exe_name}`: {}",
                record.line()
            ));
        }
        for argument in &record.argv {
            let argument_lower = argument.to_ascii_lowercase();
            let name = Path::new(&argument_lower)
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            if FORBIDDEN_BASENAMES.contains(&name.as_str()) {
                evidence.push(format!(
                    "Node-family argv basename `{name}`: {}",
                    record.line()
                ));
            }
            if argument_lower.contains("server.js") {
                evidence.push(format!(
                    "Node backend server.js argument `{argument}`: {}",
                    record.line()
                ));
            }
            if argument_lower.contains("wxml_type_backend") {
                evidence.push(format!(
                    "node backend argument `{argument}`: {}",
                    record.line()
                ));
            }
        }
        if exe.contains("server.js") {
            evidence.push(format!(
                "Node backend server.js executable: {}",
                record.line()
            ));
        }
        for key in &record.env_keys {
            if NODE_BACKEND_ENV_KEYS.contains(&key.as_str()) {
                evidence.push(format!(
                    "Node backend environment key `{key}` in exec'd process: {}",
                    record.line()
                ));
            }
        }
    }
    evidence
}

/// Asserts a log is non-vacuous, absolute-path only and Node-free.
pub fn assert_exec_audit(log: &ExecLog, label: &str) {
    assert!(
        !log.execs.is_empty(),
        "{label}: no execve/execveat event was recorded; the audit would be vacuous"
    );
    for record in &log.execs {
        assert!(
            record.exe.is_absolute(),
            "{label}: /proc/<pid>/exe did not resolve to an absolute path: {}\n{}",
            record.line(),
            log.summary()
        );
        let argv0 = record.argv.first().map_or("", String::as_str);
        assert!(
            Path::new(argv0).is_absolute(),
            "{label}: argv[0] is not absolute (`{argv0}`): {}\n{}",
            record.line(),
            log.summary()
        );
    }
    let evidence = forbidden_evidence(log);
    assert!(
        evidence.is_empty(),
        "{label}: the exec audit found Node-family evidence:\n{}\n{}",
        evidence.join("\n"),
        log.summary()
    );
}

/// Blocks until the live log covers `suffix` or fails after `timeout`.
pub fn wait_for_exec(
    label: &str,
    suffix: &str,
    timeout: Duration,
    snapshot: impl Fn() -> ExecLog,
) -> ExecLog {
    let deadline = Instant::now() + timeout;
    loop {
        let log = snapshot();
        if log.covers(suffix) {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: no exec covering `{suffix}` within {timeout:?}; log:\n{}",
            log.summary()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Resolves the shared pinned real channel with the existing F3 rules.
pub fn real_channel(test: &str) -> Option<&'static crate::shared::SharedChannel> {
    crate::shared::shared_channel(test)
}

/// Creates the isolated work/home/tmp directories used by traced runs.
pub fn workspace_paths(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let work = base.join("work");
    let home = base.join("home");
    let tmp = base.join("tmp");
    for directory in [&work, &home, &tmp] {
        fs::create_dir_all(directory).expect("create isolated directory");
        wxml_delivery::manifest::set_mode(directory, 0o700).expect("private isolated directory");
    }
    (work, home, tmp)
}

/// Prepares a runnable native package from an already built package tree.
///
/// This is the prebuilt-package mode behind
/// `WXML_F3_LSP_BIN=<pkg>/bin/wxml-lsp`: the binary and engine are copied
/// byte-for-byte into a test-owned package, and typings come from the package
/// itself, from `TSGO_TYPINGS_ROOT`, or from the pinned 23-file fixture.
/// `TSGO_BIN` and `TSGO_ASSET_ROOT`, when set, must agree with the package.
///
/// Returns `None` when `WXML_F3_LSP_BIN` is not a `<pkg>/bin/<binary>` layout,
/// in which case the caller uses the tarball/install path.
pub fn prepare_prebuilt_package(base: &Path, test: &str) -> Option<PathBuf> {
    let lsp = std::env::var_os("WXML_F3_LSP_BIN").map(PathBuf::from)?;
    if !lsp.is_file() {
        return None;
    }
    let bin_dir = lsp.parent()?;
    if bin_dir.file_name()? != "bin" {
        return None;
    }
    let source = bin_dir.parent()?;
    let source_manifest = source.join("manifest.json");
    let source_engine = source.join("engine/lib/tsc");
    if !source_manifest.is_file() || !source_engine.is_file() {
        return None;
    }

    let canonical_engine = fs::canonicalize(&source_engine)
        .unwrap_or_else(|error| panic!("{test}: resolve {}: {error}", source_engine.display()));
    if let Some(value) = std::env::var_os("TSGO_BIN") {
        let given = fs::canonicalize(PathBuf::from(&value)).unwrap_or_else(|error| {
            panic!(
                "{test}: TSGO_BIN {} cannot be resolved: {error}",
                PathBuf::from(&value).display()
            )
        });
        assert_eq!(
            given,
            canonical_engine,
            "{test}: TSGO_BIN must be the package engine {}",
            source_engine.display()
        );
    }
    if let Some(value) = std::env::var_os("TSGO_ASSET_ROOT") {
        let given = fs::canonicalize(PathBuf::from(&value)).unwrap_or_else(|error| {
            panic!(
                "{test}: TSGO_ASSET_ROOT {} cannot be resolved: {error}",
                PathBuf::from(&value).display()
            )
        });
        let engine_root = fs::canonicalize(source.join("engine")).expect("canonical engine root");
        let lib_root = fs::canonicalize(source.join("engine/lib")).expect("canonical engine lib");
        assert!(
            given == engine_root || given == lib_root,
            "{test}: TSGO_ASSET_ROOT must be {} or {}",
            engine_root.display(),
            lib_root.display()
        );
    }

    let root = base.join("native-package");
    copy_file(&lsp, &root.join("bin/wxml-lsp"), 0o755);
    copy_file(&source_manifest, &root.join("manifest.json"), 0o644);
    copy_tree(&source.join("engine"), &root.join("engine"));

    let typings_destination = root.join("typings/miniprogram-api-typings");
    let package_typings = source.join("typings/miniprogram-api-typings");
    let typings_source = if package_typings.join("index.d.ts").is_file() {
        package_typings
    } else if let Some(value) = std::env::var_os("TSGO_TYPINGS_ROOT") {
        let path = PathBuf::from(value);
        assert!(
            path.join("index.d.ts").is_file(),
            "{test}: TSGO_TYPINGS_ROOT must contain index.d.ts: {}",
            path.display()
        );
        path
    } else {
        crate::fixture::write_typings_payload(&typings_destination);
        println!("{test}: prebuilt package typings from the pinned 23-file fixture");
        finish_prebuilt_package(test, source, &lsp, &root, source_engine.as_path());
        return Some(root);
    };
    copy_tree(&typings_source, &typings_destination);
    println!(
        "{test}: prebuilt package bin={} engine={} typings={}",
        lsp.display(),
        source_engine.display(),
        typings_source.display()
    );
    finish_prebuilt_package(test, source, &lsp, &root, source_engine.as_path());
    Some(root)
}

fn finish_prebuilt_package(test: &str, source: &Path, lsp: &Path, root: &Path, engine: &Path) {
    assert!(
        root.join("typings/miniprogram-api-typings/index.d.ts")
            .is_file(),
        "{test}: prebuilt package is missing typings/miniprogram-api-typings/index.d.ts"
    );
    println!(
        "{test}: PREBUILT package ready at {} (source={} lsp={} engine={})",
        root.display(),
        source.display(),
        lsp.display(),
        engine.display()
    );
}

fn copy_file(source: &Path, destination: &Path, mode: u32) {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
    }
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    wxml_delivery::manifest::set_mode(destination, mode)
        .unwrap_or_else(|error| panic!("chmod {}: {error}", destination.display()));
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|error| panic!("create {}: {error}", destination.display()));
    wxml_delivery::manifest::set_mode(destination, 0o755)
        .unwrap_or_else(|error| panic!("chmod {}: {error}", destination.display()));
    let mut entries: Vec<_> = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()))
        .map(|entry| entry.expect("package entry").path())
        .collect();
    entries.sort();
    for entry in entries {
        let target = destination.join(entry.file_name().expect("file name"));
        let metadata = fs::symlink_metadata(&entry)
            .unwrap_or_else(|error| panic!("metadata {}: {error}", entry.display()));
        if metadata.file_type().is_dir() {
            copy_tree(&entry, &target);
        } else if metadata.file_type().is_file() {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = metadata.permissions().mode() & 0o777;
            copy_file(&entry, &target, mode);
        } else {
            panic!(
                "prebuilt package source contains a non-file: {}",
                entry.display()
            );
        }
    }
}
