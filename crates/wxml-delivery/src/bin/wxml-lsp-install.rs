//! `wxml-lsp-install`: verified local installer, upgrade and rollback.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wxml_delivery::acquire::{AcquireMode, AcquireOptions, acquire};
use wxml_delivery::acquire_v2::{AcquireModeV2, AcquireOptionsV2, acquire_v2};
use wxml_delivery::archive::extract_tar_gz;
use wxml_delivery::error::{DeliveryError, Result};
use wxml_delivery::install::{
    BINARY_ENV, InstallLock, InstallOutcome, InstallRoot, explicit_binary,
};
use wxml_delivery::manifest::ExpectedManifest;
use wxml_delivery::manifest_v2::{
    ExpectedManifestV2, ManifestSchema, NATIVE_PACKAGE_PLATFORM, read_manifest_schema,
    read_manifest_v2,
};
use wxml_delivery::native_install::{
    CommandEngineProbe, CurrentInstall, NativeInstallOutcome, NoHooks, inspect_current,
    install_native_locked, recover_native, rollback_native_locked, verify_native,
};
use wxml_delivery::paths::{host_platform, shell_quote};
use wxml_delivery::semver::compare_versions;
use wxml_delivery::source::Source;
use wxml_delivery::state::InstallState;
use wxml_delivery::verify::{
    binary_version, check_backend, check_binary, check_node, verify_install,
};
use wxml_delivery::verify_v2::verify_tree_v2;

const USAGE: &str = "\
Usage: wxml-lsp-install <COMMAND> [OPTIONS]

Commands:
  install        Install an exact version (requires --version)
  upgrade        Install the newest stable release from the source
  rollback       Switch to a previously installed verified version (--to)
  recover        Apply journal recovery rules without activating a new version
  versions       List versions available from a source
  list           List installed versions (legacy schema v1 is read-only)
  current        Print the active binary path and reported version
  verify         Fully verify the active installation and native engine
  env            Print WXML_LSP_BIN exports
  verify-package Verify a package archive without installing it

Options:
  --root DIR          Install root (default: $XDG_DATA_HOME/wxml-lsp or
                      $HOME/.local/share/wxml-lsp)
  --version V         Exact release version (install/verify-package/verify)
  --to V              Target version (rollback)
  --source URL        Release base URL; HTTPS, or loopback HTTP for fixtures
  --from-dir DIR      Local release output directory
  --enable-download   Explicitly allow contact with a remote source
  --offline           Use only verified local releases; never network
  --help              Print this message

Manifest schema 2 packages are native and never require Node. Schema 1
packages remain readable for migration, but the installer never executes their
Node backend and never reactivates one as a rollback target.

Download is disabled unless --enable-download is passed with a remote source.
A local --from-dir is an explicit offline source. WXML_LSP_BIN always takes
precedence: when set it is validated and used, and no install or download is
attempted; when set but invalid the command fails and never falls back.

Read-only commands (list/current/verify/env) never create or chmod the install
root. `verify` hashes the complete tree before executing its packaged binaries.
The installer never reads project or editor configuration for a download
source, never uploads anything and never creates remote tags or releases.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let Some(command_os) = args.next() else {
        eprintln!("{USAGE}");
        return Err(DeliveryError::InvalidArgument(
            "a command is required".to_owned(),
        ));
    };
    let command = command_os.to_str().ok_or_else(|| {
        DeliveryError::InvalidArgument("command names must be valid UTF-8".to_owned())
    })?;
    if matches!(command, "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if matches!(command, "--version" | "-V") {
        println!("wxml-lsp-install {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = Options::parse(args, &allowed_for(command)?)?;
    if options.help {
        println!("{USAGE}");
        return Ok(());
    }
    match command {
        "install" => install(&options),
        "upgrade" => upgrade(&options),
        "rollback" => rollback(&options),
        "recover" => recover(&options),
        "versions" => versions(&options),
        "list" => list(&options),
        "current" => current(&options),
        "verify" => verify(&options),
        "env" => env(&options),
        "verify-package" => verify_package(&options),
        other => Err(DeliveryError::InvalidArgument(format!(
            "unknown command `{other}`\n\n{USAGE}"
        ))),
    }
}

fn allowed_for(command: &str) -> Result<Vec<&'static str>> {
    let allowed = match command {
        "install" | "verify-package" => vec![
            "root",
            "version",
            "source",
            "from-dir",
            "enable-download",
            "offline",
            "help",
        ],
        "upgrade" | "versions" => vec![
            "root",
            "source",
            "from-dir",
            "enable-download",
            "offline",
            "help",
        ],
        "rollback" => vec![
            "root",
            "to",
            "source",
            "from-dir",
            "enable-download",
            "offline",
            "help",
        ],
        "list" | "current" | "env" | "recover" => vec!["root", "help"],
        "verify" => vec!["root", "version", "help"],
        other => {
            return Err(DeliveryError::InvalidArgument(format!(
                "unknown command `{other}`\n\n{USAGE}"
            )));
        }
    };
    Ok(allowed)
}

#[derive(Default)]
struct Options {
    root: Option<PathBuf>,
    version: Option<String>,
    to: Option<String>,
    source: Option<String>,
    from_dir: Option<PathBuf>,
    enable_download: bool,
    offline: bool,
    help: bool,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = OsString>, allowed: &[&str]) -> Result<Self> {
        let mut options = Self::default();
        while let Some(argument) = args.next() {
            let text = argument.to_str().ok_or_else(|| {
                DeliveryError::InvalidArgument(
                    "option names must be valid UTF-8; non-UTF-8 values are allowed only after a path option"
                        .to_owned(),
                )
            })?;
            let name = text.strip_prefix("--").ok_or_else(|| {
                DeliveryError::InvalidArgument(format!("unexpected argument `{text}`"))
            })?;
            let (name, inline) = name
                .split_once('=')
                .map_or((name, None), |(name, value)| (name, Some(value.to_owned())));
            if !allowed.contains(&name) {
                return Err(DeliveryError::InvalidArgument(format!(
                    "unexpected option `--{name}`"
                )));
            }
            match name {
                "root" => options.root = Some(take_path(&mut args, inline.as_deref(), "root")?),
                "from-dir" => {
                    options.from_dir = Some(take_path(&mut args, inline.as_deref(), "from-dir")?);
                }
                "version" => {
                    options.version = Some(take_string(&mut args, inline.as_deref(), "version")?);
                }
                "to" => options.to = Some(take_string(&mut args, inline.as_deref(), "to")?),
                "source" => {
                    options.source = Some(take_string(&mut args, inline.as_deref(), "source")?);
                }
                "enable-download" => {
                    options.enable_download = parse_flag(inline.as_deref(), "enable-download")?;
                }
                "offline" => options.offline = parse_flag(inline.as_deref(), "offline")?,
                "help" => {
                    if inline.is_some() {
                        return Err(DeliveryError::InvalidArgument(
                            "--help does not take a value".to_owned(),
                        ));
                    }
                    options.help = true;
                }
                _ => {
                    return Err(DeliveryError::InvalidArgument(format!(
                        "unhandled option `--{name}`"
                    )));
                }
            }
        }
        Ok(options)
    }
}

fn take_path(
    args: &mut impl Iterator<Item = OsString>,
    inline: Option<&str>,
    option: &str,
) -> Result<PathBuf> {
    if let Some(value) = inline {
        return Ok(PathBuf::from(value));
    }
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| DeliveryError::InvalidArgument(format!("--{option} requires a path value")))
}

fn take_string(
    args: &mut impl Iterator<Item = OsString>,
    inline: Option<&str>,
    option: &str,
) -> Result<String> {
    if let Some(value) = inline {
        return Ok(value.to_owned());
    }
    args.next()
        .ok_or_else(|| DeliveryError::InvalidArgument(format!("--{option} requires a value")))?
        .into_string()
        .map_err(|_| DeliveryError::InvalidArgument(format!("--{option} must be valid UTF-8")))
}

fn parse_flag(inline: Option<&str>, option: &str) -> Result<bool> {
    match inline {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(value) => Err(DeliveryError::InvalidArgument(format!(
            "--{option}={value} is not a boolean; use true or false"
        ))),
    }
}

fn install(options: &Options) -> Result<()> {
    if let Some(path) = skip_because_explicit()? {
        println!("{BINARY_ENV} is set; it takes precedence. No install or download was attempted.");
        println!("binary: {}", path.display());
        return Ok(());
    }
    let version = options
        .version
        .clone()
        .ok_or_else(|| DeliveryError::InvalidArgument("install requires --version".to_owned()))?;
    let source = source(options)?;
    if let Some(source) = source.as_ref() {
        ensure_download_enabled(source, options)?;
    }
    let root = root_mut(options)?;
    let lock = root.lock()?;
    if let Some(source) = source.as_ref() {
        let (schema, bytes) = read_source_schema(source)?;
        if schema == 2 {
            return install_native_version(
                options,
                &root,
                &lock,
                source,
                &bytes,
                AcquireModeV2::Exact(version),
            );
        }
        if schema != 1 {
            return Err(unsupported_index_schema(schema));
        }
        reject_v1_over_native(&root)?;
    } else if active_is_native(&root)? {
        return Err(native_source_required());
    }
    // Legacy schema v1 package: keep the existing verified Node-package flow.
    let acquired = acquire(&AcquireOptions {
        root: &root,
        source: source.as_ref(),
        mode: AcquireMode::Exact(version.clone()),
        allow_download: options.enable_download,
        offline: options.offline,
    })?;
    install_and_report(&root, &acquired.tree, &version, acquired.cache_hit)
}

fn upgrade(options: &Options) -> Result<()> {
    if let Some(path) = skip_because_explicit()? {
        println!("{BINARY_ENV} is set; it takes precedence. No upgrade or download was attempted.");
        println!("binary: {}", path.display());
        return Ok(());
    }
    let source = source(options)?;
    if let Some(source) = source.as_ref() {
        ensure_download_enabled(source, options)?;
    }
    let root = root_mut(options)?;
    let lock = root.lock()?;
    if let Some(source) = source.as_ref() {
        let (schema, bytes) = read_source_schema(source)?;
        if schema == 2 {
            return install_native_version(
                options,
                &root,
                &lock,
                source,
                &bytes,
                AcquireModeV2::Latest,
            );
        }
        if schema != 1 {
            return Err(unsupported_index_schema(schema));
        }
        reject_v1_over_native(&root)?;
    } else if active_is_native(&root)? {
        return Err(native_source_required());
    }
    // Legacy schema v1 package: keep the existing verified Node-package flow.
    let current = root.current_version()?;
    let acquired = acquire(&AcquireOptions {
        root: &root,
        source: source.as_ref(),
        mode: AcquireMode::Latest,
        allow_download: options.enable_download,
        offline: options.offline,
    })?;
    let version = acquired.manifest.package_version.clone();
    if let Some(current) = current {
        if compare_versions(&version, &current).is_lt() {
            cleanup_staging(&root, &acquired.tree);
            return Err(DeliveryError::InvalidArgument(format!(
                "refusing to downgrade from {current} to {version}; use `rollback --to {version}` explicitly"
            )));
        }
    }
    install_and_report(&root, &acquired.tree, &version, acquired.cache_hit)
}

fn rollback(options: &Options) -> Result<()> {
    if let Some(path) = skip_because_explicit()? {
        println!("{BINARY_ENV} is set; it takes precedence. No rollback was attempted.");
        println!("binary: {}", path.display());
        return Ok(());
    }
    let target = options.to.clone().ok_or_else(|| {
        DeliveryError::InvalidArgument("rollback requires --to VERSION".to_owned())
    })?;
    let root = root_mut(options)?;
    let lock = root.lock()?;
    if matches!(inspect_current(&root)?, CurrentInstall::Native { .. }) {
        if options.source.is_some() || options.from_dir.is_some() {
            return Err(DeliveryError::InvalidArgument(
                "rollback never downloads; remove --source/--from-dir and use an installed version"
                    .to_owned(),
            ));
        }
        let outcome = rollback_native_locked(
            &root,
            &target,
            &CommandEngineProbe::default(),
            &NoHooks,
            &lock,
        )?;
        let directory = root.versions_dir().join(&target);
        let manifest = read_manifest_v2(
            &directory,
            ExpectedManifestV2 {
                version: Some(&target),
                platform: Some(NATIVE_PACKAGE_PLATFORM),
                ..ExpectedManifestV2::default()
            },
        )?;
        let binary = root.current_binary()?;
        if outcome == NativeInstallOutcome::AlreadyInstalled {
            println!("already active {target}");
        } else {
            println!("rolled back to {target}");
        }
        println!(
            "engine: {} {}",
            manifest.engine.name, manifest.engine.version
        );
        println!("binary: {}", binary.display());
        println!(
            "export {BINARY_ENV}={}",
            shell_quote(binary.to_string_lossy().as_ref())
        );
        return Ok(());
    }
    // Legacy schema v1 installation: keep the existing verified Node-package flow.
    if !root.versions_dir().join(&target).is_dir() {
        let source = source(options)?;
        let acquired = acquire(&AcquireOptions {
            root: &root,
            source: source.as_ref(),
            mode: AcquireMode::Exact(target.clone()),
            allow_download: options.enable_download,
            offline: options.offline,
        })?;
        let result = root.install_tree(&acquired.tree, &target);
        cleanup_staging(&root, &acquired.tree);
        result?;
    }
    let manifest = root.verify_installed(&target)?;
    let target_dir = root.versions_dir().join(&target);
    check_binary(&target_dir, &manifest)?;
    check_backend(&target_dir, &manifest)?;
    let previous = root.current_version()?;
    root.switch_durable(&target)?;
    let binary = root.current_binary()?;
    let version = match binary_version(&binary) {
        Ok(version) => version,
        Err(error) => {
            if let Some(previous) = previous {
                let _ = root.switch_durable(&previous);
                return Err(DeliveryError::Runtime(format!(
                    "{error}; rolled back to {previous}"
                )));
            }
            return Err(error);
        }
    };
    println!("rolled back to {target}");
    println!("binary: {}", binary.display());
    println!("reported version: {version}");
    Ok(())
}

fn recover(options: &Options) -> Result<()> {
    let path = root_path(options)?;
    let Some(root) = InstallRoot::open(&path)? else {
        println!("nothing to recover: the install root does not exist");
        return Ok(());
    };
    let report = recover_native(&root, &CommandEngineProbe::default())?;
    if report.actions.is_empty() {
        println!("no recovery actions were needed");
    } else {
        for action in &report.actions {
            println!("{action}");
        }
    }
    match report.current {
        Some(version) => println!("current: {version}"),
        None => println!("current: none"),
    }
    Ok(())
}

fn install_native_version(
    options: &Options,
    root: &InstallRoot,
    lock: &InstallLock,
    source: &Source,
    index_bytes: &[u8],
    mode: AcquireModeV2,
) -> Result<()> {
    let index = wxml_delivery::index_v2::parse_index_v2(index_bytes)?;
    let acquired = acquire_v2(&AcquireOptionsV2 {
        root,
        source: Some(source),
        index: &index,
        mode,
        allow_download: options.enable_download,
        offline: options.offline,
    })?;
    let expected = ExpectedManifestV2 {
        version: Some(&acquired.release.version),
        platform: Some(NATIVE_PACKAGE_PLATFORM),
        protocol_version: Some(acquired.release.protocol_version),
    };
    let result = install_native_locked(
        root,
        &acquired.tree,
        expected,
        &CommandEngineProbe::default(),
        &NoHooks,
        lock,
    );
    cleanup_staging(root, &acquired.tree);
    let outcome = result?;
    let version = root
        .current_version()?
        .ok_or_else(|| DeliveryError::InstallState("no version is active".to_owned()))?;
    let binary = root.current_binary()?;
    let action = match outcome {
        NativeInstallOutcome::Installed => "installed",
        NativeInstallOutcome::Migrated => "migrated to",
        NativeInstallOutcome::Upgraded => "upgraded to",
        NativeInstallOutcome::AlreadyInstalled => "already installed",
        NativeInstallOutcome::RolledBack => "rolled back to",
    };
    println!("{action} {version} (native)");
    println!(
        "engine: {} {}",
        acquired.manifest.engine.name, acquired.manifest.engine.version
    );
    println!("binary: {}", binary.display());
    println!(
        "export {BINARY_ENV}={}",
        shell_quote(binary.to_string_lossy().as_ref())
    );
    Ok(())
}

fn read_source_schema(source: &Source) -> Result<(u32, Vec<u8>)> {
    let bytes = source.read_index_bytes()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        DeliveryError::InvalidPackage(format!("index.json is malformed: {error}"))
    })?;
    let schema = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            DeliveryError::InvalidPackage("index.json has no integer schemaVersion".to_owned())
        })?;
    Ok((u32::try_from(schema).unwrap_or(u32::MAX), bytes))
}

fn unsupported_index_schema(schema: u32) -> DeliveryError {
    DeliveryError::InvalidPackage(format!(
        "unsupported release index schema {schema}; this installer supports schema 1 (legacy read-only) and 2 (native)"
    ))
}

fn active_is_native(root: &InstallRoot) -> Result<bool> {
    // `state.json` is the authoritative native marker. A legacy root may have a
    // dangling `current` after a manual version removal; that must keep working
    // with the schema v1 cache flow rather than becoming a native error.
    if InstallState::read(root)?.is_some() {
        return Ok(true);
    }
    Ok(matches!(
        inspect_current(root),
        Ok(CurrentInstall::Native { .. })
    ))
}

fn ensure_download_enabled(source: &Source, options: &Options) -> Result<()> {
    if matches!(source, Source::Remote(_)) && !options.enable_download {
        return Err(DeliveryError::InvalidArgument(
            "download is disabled; pass --enable-download to contact the source".to_owned(),
        ));
    }
    Ok(())
}

fn reject_v1_over_native(root: &InstallRoot) -> Result<()> {
    if active_is_native(root)? {
        return Err(DeliveryError::InstallState(
            "refusing to install a legacy schema v1 package over an active native installation; \
             roll back explicitly or remove the native root"
                .to_owned(),
        ));
    }
    Ok(())
}

fn native_source_required() -> DeliveryError {
    DeliveryError::InstallState(
        "a native source is required; offline installs need --from-dir because no native archive cache is implemented"
            .to_owned(),
    )
}

fn versions(options: &Options) -> Result<()> {
    if options.offline && options.source.is_some() && options.from_dir.is_none() {
        return Err(DeliveryError::InvalidArgument(
            "offline version listing cannot query a remote source".to_owned(),
        ));
    }
    let source = source(options)?.ok_or_else(|| {
        DeliveryError::InvalidArgument("versions requires --source or --from-dir".to_owned())
    })?;
    if matches!(source, Source::Remote(_)) && !options.enable_download {
        return Err(DeliveryError::InvalidArgument(
            "download is disabled; pass --enable-download to contact the source".to_owned(),
        ));
    }
    let (schema, bytes) = read_source_schema(&source)?;
    let platform = host_platform()?;
    if schema == 2 {
        let index = wxml_delivery::index_v2::parse_index_v2(&bytes)?;
        let mut available: Vec<_> = index
            .releases
            .iter()
            .filter(|release| release.platform == platform)
            .collect();
        available.sort_by(|left, right| compare_versions(&left.version, &right.version));
        if available.is_empty() {
            println!("no native releases for {platform}");
            return Ok(());
        }
        for release in available {
            println!(
                "{}  protocol {}  engine {}",
                release.version, release.protocol_version, release.engine_version
            );
        }
        return Ok(());
    }
    if schema != 1 {
        return Err(unsupported_index_schema(schema));
    }
    let index = wxml_delivery::manifest::parse_index(&bytes)?;
    let mut available: Vec<_> = index
        .releases
        .iter()
        .filter(|release| release.platform == platform)
        .collect();
    available.sort_by(|left, right| compare_versions(&left.version, &right.version));
    if available.is_empty() {
        println!("no releases for {platform}");
        return Ok(());
    }
    for release in available {
        println!(
            "{}  protocol {}  node >= {}",
            release.version, release.protocol_version, release.node_minimum_version
        );
    }
    Ok(())
}

fn list(options: &Options) -> Result<()> {
    let Some(root) = InstallRoot::open(&root_path(options)?)? else {
        println!("no installed versions");
        return Ok(());
    };
    let current = root.current_version()?;
    let installed = root.versions()?;
    if installed.is_empty() {
        println!("no installed versions");
        return Ok(());
    }
    for version in installed {
        let marker = if current.as_deref() == Some(version.as_str()) {
            "*"
        } else {
            " "
        };
        let suffix = match read_manifest_schema(&root.versions_dir().join(&version)) {
            Ok(ManifestSchema::V1) => " (legacy, read-only)",
            Ok(ManifestSchema::V2) => "",
            Err(_) => " (unreadable)",
        };
        println!("{marker} {version}{suffix}");
    }
    Ok(())
}

fn current(options: &Options) -> Result<()> {
    if let Some(path) = explicit_binary()? {
        let version = binary_version(&path)?;
        println!("binary: {}", path.display());
        println!("version: {version}");
        println!("source: {BINARY_ENV}");
        return Ok(());
    }
    let root = open_root(options)?;
    match inspect_current(&root)? {
        CurrentInstall::Native { version, manifest } => {
            let state = InstallState::read(&root)?.ok_or_else(|| {
                DeliveryError::InstallState("native current exists without state.json".to_owned())
            })?;
            state.validate_against(&root)?;
            println!("binary: {}", root.current_binary()?.display());
            println!("version: {version}");
            println!("platform: {}", manifest.platform);
            println!(
                "engine: {} {}",
                manifest.engine.name, manifest.engine.version
            );
            println!("manifest protocol: {}", manifest.protocol_version);
        }
        CurrentInstall::Legacy { version } => {
            // Legacy schema v1: read-only, same behavior as the old installer.
            let directory = root.versions_dir().join(&version);
            let manifest = root.verify_installed(&version)?;
            let reported = check_binary(&directory, &manifest)?;
            check_backend(&directory, &manifest)?;
            check_node(&manifest.node.minimum_version)?;
            println!("binary: {}", root.current_binary()?.display());
            println!("version: {reported}");
            println!("manifest protocol: {}", manifest.protocol_version);
        }
        CurrentInstall::None => {
            return Err(DeliveryError::InstallState(
                "no installed version is active".to_owned(),
            ));
        }
    }
    Ok(())
}

fn verify(options: &Options) -> Result<()> {
    if let Some(path) = explicit_binary()? {
        let version = binary_version(&path)?;
        println!("binary: {}", path.display());
        println!("version: {version}");
        println!("source: {BINARY_ENV}");
        println!("note: installed state is not checked while {BINARY_ENV} is set");
        return Ok(());
    }
    let root = open_root(options)?;
    match inspect_current(&root)? {
        CurrentInstall::Native { version, .. } => {
            if let Some(expected) = &options.version {
                if expected != &version {
                    return Err(DeliveryError::InstallState(format!(
                        "active version is {version}, not {expected}"
                    )));
                }
            }
            let report = verify_native(&root, &CommandEngineProbe::default())?;
            println!("verified {version} (native)");
            println!("engine version: {}", report.engine_version);
            println!(
                "files: {}  bytes: {}",
                report.file_count, report.total_bytes
            );
            println!("binary: {}", root.current_binary()?.display());
        }
        CurrentInstall::Legacy { version } => {
            if let Some(expected) = &options.version {
                if expected != &version {
                    return Err(DeliveryError::InstallState(format!(
                        "active version is {version}, not {expected}"
                    )));
                }
            }
            let directory = root.versions_dir().join(&version);
            // Hash-verify the whole tree before executing the packaged binary.
            // A tampered `bin/wxml-lsp` must never run just because the user
            // asked to verify it.
            let manifest = root.verify_installed(&version)?;
            let report = verify_install(&directory, &manifest)?;
            println!("verified {version}");
            println!("binary version: {}", report.binary_version);
            println!(
                "node: {} (minimum {})",
                report.node_version, manifest.node.minimum_version
            );
            println!("protocol: {}", manifest.protocol_version);
        }
        CurrentInstall::None => {
            return Err(DeliveryError::InstallState(
                "no installed version is active".to_owned(),
            ));
        }
    }
    Ok(())
}

fn env(options: &Options) -> Result<()> {
    if let Some(path) = explicit_binary()? {
        println!(
            "export {BINARY_ENV}={}",
            shell_quote(path.to_string_lossy().as_ref())
        );
        return Ok(());
    }
    let root = open_root(options)?;
    match inspect_current(&root)? {
        CurrentInstall::Native { .. } => {
            let state = InstallState::read(&root)?.ok_or_else(|| {
                DeliveryError::InstallState("native current exists without state.json".to_owned())
            })?;
            state.validate_against(&root)?;
            let binary = root.current_binary()?;
            println!(
                "export {BINARY_ENV}={}",
                shell_quote(binary.to_string_lossy().as_ref())
            );
        }
        CurrentInstall::Legacy { version } => {
            // Legacy schema v1: keep the historical exports for read-only
            // compatibility. No native command forwards them.
            root.verify_installed(&version)?;
            let binary = root.current_binary()?;
            let backend = binary
                .parent()
                .ok_or_else(|| DeliveryError::InstallState("binary has no parent".to_owned()))?
                .join("wxml-type-backend/server.js");
            println!(
                "export {BINARY_ENV}={}",
                shell_quote(binary.to_string_lossy().as_ref())
            );
            println!(
                "export WXML_TYPE_BACKEND={}",
                shell_quote(backend.to_string_lossy().as_ref())
            );
        }
        CurrentInstall::None => {
            return Err(DeliveryError::InstallState(
                "no installed version is active".to_owned(),
            ));
        }
    }
    Ok(())
}

fn verify_package(options: &Options) -> Result<()> {
    let version = options.version.clone().ok_or_else(|| {
        DeliveryError::InvalidArgument("verify-package requires --version".to_owned())
    })?;
    let source = source(options)?.ok_or_else(|| {
        DeliveryError::InvalidArgument("verify-package requires --source or --from-dir".to_owned())
    })?;
    if matches!(source, Source::Remote(_)) {
        if options.offline {
            return Err(DeliveryError::InvalidArgument(
                "verify-package with --offline requires --from-dir".to_owned(),
            ));
        }
        if !options.enable_download {
            return Err(DeliveryError::InvalidArgument(
                "download is disabled; pass --enable-download to contact the source".to_owned(),
            ));
        }
    }
    let (schema, bytes) = read_source_schema(&source)?;
    let platform = host_platform()?;
    let scratch = scratch_dir()?;
    let result = (|| {
        let archive = scratch.join("archive.tar.gz");
        if schema == 2 {
            let index = wxml_delivery::index_v2::parse_index_v2(&bytes)?;
            let release = index.find(&version, &platform)?;
            source.fetch_archive_v2(release, &archive)?;
            let tree = scratch.join("verify");
            extract_tar_gz(&archive, &tree)?;
            let report = verify_tree_v2(
                &tree,
                ExpectedManifestV2 {
                    version: Some(&release.version),
                    platform: Some(NATIVE_PACKAGE_PLATFORM),
                    protocol_version: Some(release.protocol_version),
                },
            )?;
            if !release.matches_manifest(&report.manifest) {
                return Err(DeliveryError::Integrity(
                    "native manifest metadata does not match the release index".to_owned(),
                ));
            }
            println!(
                "verified native package {} {} ({})",
                report.manifest.package, report.manifest.package_version, report.manifest.platform
            );
            println!(
                "protocol: {}  engine: {}  files: {}",
                report.manifest.protocol_version,
                report.manifest.engine.version,
                report.manifest.files.len()
            );
            println!("archive sha256: {}", release.sha256);
            return Ok(());
        }
        if schema != 1 {
            return Err(unsupported_index_schema(schema));
        }
        let index = wxml_delivery::manifest::parse_index(&bytes)?;
        let release = index.find(&version, &platform)?;
        source.fetch_archive(release, &archive)?;
        let manifest = wxml_delivery::verify::verify_archive(
            &archive,
            &scratch.join("verify"),
            ExpectedManifest {
                version: Some(&release.version),
                platform: Some(&platform),
                protocol_version: Some(release.protocol_version),
            },
        )?;
        println!(
            "verified package {} {} ({})",
            manifest.package, manifest.package_version, manifest.platform
        );
        println!(
            "protocol: {}  node: >= {}  files: {}",
            manifest.protocol_version,
            manifest.node.minimum_version,
            manifest.files.len()
        );
        println!("archive sha256: {}", release.sha256);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn install_and_report(
    root: &InstallRoot,
    tree: &Path,
    version: &str,
    cache_hit: bool,
) -> Result<()> {
    let previous = root.current_version()?;
    let result = root.install_tree(tree, version);
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(tree);
        cleanup_staging(root, tree);
        return Err(error);
    }
    let outcome = result?;
    let report = print_install_result(root, version, outcome, cache_hit);
    cleanup_staging(root, tree);
    match report {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Some(previous) = previous {
                let _ = root.switch_durable(&previous);
                return Err(DeliveryError::Runtime(format!(
                    "{error}; rolled back to {previous}"
                )));
            }
            Err(error)
        }
    }
}

fn cleanup_staging(root: &InstallRoot, tree: &Path) {
    let Some(parent) = tree.parent() else {
        return;
    };
    let Ok(parent) = std::fs::canonicalize(parent) else {
        return;
    };
    let Ok(tmp) = std::fs::canonicalize(root.tmp_dir()) else {
        return;
    };
    if parent.starts_with(&tmp) && parent != tmp {
        let _ = std::fs::remove_dir_all(parent);
    }
}

fn print_install_result(
    root: &InstallRoot,
    version: &str,
    outcome: InstallOutcome,
    cache_hit: bool,
) -> Result<()> {
    let directory = root.versions_dir().join(version);
    let manifest = root.verify_installed(version)?;
    check_binary(&directory, &manifest)?;
    check_backend(&directory, &manifest)?;
    match check_node(&manifest.node.minimum_version) {
        Ok(node) => println!("node: {node}"),
        Err(error) => eprintln!(
            "warning: {error}\nwarning: syntax-only features remain available; install a supported Node.js for type features"
        ),
    }
    let action = match outcome {
        InstallOutcome::Installed => "installed",
        InstallOutcome::AlreadyInstalled => "already installed",
    };
    let origin = if cache_hit { "cache" } else { "source" };
    println!("{action} {version} ({origin})");
    println!("binary: {}", root.current_binary()?.display());
    println!(
        "export {BINARY_ENV}={}",
        shell_quote(root.current_binary()?.to_string_lossy().as_ref())
    );
    Ok(())
}

fn root_path(options: &Options) -> Result<PathBuf> {
    options.root.clone().map_or_else(default_root, Ok)
}

fn root_mut(options: &Options) -> Result<InstallRoot> {
    InstallRoot::create(&root_path(options)?)
}

fn open_root(options: &Options) -> Result<InstallRoot> {
    InstallRoot::open(&root_path(options)?)?.ok_or_else(|| {
        DeliveryError::InstallState(
            "no installed version is available; run install first".to_owned(),
        )
    })
}

fn default_root() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(xdg);
        if !path.is_absolute() {
            return Err(DeliveryError::InvalidArgument(
                "XDG_DATA_HOME must be an absolute path; pass --root to choose an install directory"
                    .to_owned(),
            ));
        }
        return Ok(path.join("wxml-lsp"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        DeliveryError::InvalidArgument(
            "HOME is not set; pass --root to choose an install directory".to_owned(),
        )
    })?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(DeliveryError::InvalidArgument(
            "HOME must be an absolute path; pass --root to choose an install directory".to_owned(),
        ));
    }
    Ok(home.join(".local/share/wxml-lsp"))
}

fn source(options: &Options) -> Result<Option<Source>> {
    match (&options.source, &options.from_dir) {
        (Some(_), Some(_)) => Err(DeliveryError::InvalidArgument(
            "use either --source or --from-dir, not both".to_owned(),
        )),
        (Some(source), None) => {
            if options.offline {
                return Err(DeliveryError::InvalidArgument(
                    "--offline cannot be combined with a remote --source; pass --from-dir or omit --source"
                        .to_owned(),
                ));
            }
            Source::remote(source).map(Some)
        }
        (None, Some(directory)) => Source::local(directory).map(Some),
        (None, None) => {
            if options.offline {
                Ok(None)
            } else {
                Err(DeliveryError::InvalidArgument(
                    "a source is required unless --offline is used".to_owned(),
                ))
            }
        }
    }
}

fn skip_because_explicit() -> Result<Option<PathBuf>> {
    explicit_binary()
}

fn scratch_dir() -> Result<PathBuf> {
    let base = if Path::new("/tmp/opencode").is_dir() {
        PathBuf::from("/tmp/opencode")
    } else {
        std::env::temp_dir()
    };
    for _ in 0..32 {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let directory = base.join(format!("wxml-verify-{}-{nonce}", std::process::id()));
        match create_private_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(DeliveryError::io(&directory, error)),
        }
    }
    Err(DeliveryError::InstallState(
        "could not allocate a private verify directory".to_owned(),
    ))
}

/// Creates a directory with mode `0700` in one atomic operation on Unix.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn scratch_directories_are_private_and_unique() {
        use std::os::unix::fs::PermissionsExt;

        let first = scratch_dir().expect("first scratch dir");
        let second = scratch_dir().expect("second scratch dir");
        assert_ne!(first, second);
        for directory in [&first, &second] {
            let mode = std::fs::metadata(directory).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", directory.display());
        }
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn parses_non_utf8_paths_and_boolean_flags() {
        let raw_path = OsString::from_vec(b"/tmp/non-utf8-\xff".to_vec());
        let options = Options::parse(
            vec![OsString::from("--root"), raw_path.clone()].into_iter(),
            &["root"],
        )
        .expect("non-UTF-8 path must be accepted");
        assert_eq!(options.root, Some(PathBuf::from(raw_path)));

        let options = Options::parse(
            vec![OsString::from("--enable-download=false")].into_iter(),
            &["enable-download"],
        )
        .expect("false is a valid boolean");
        assert!(!options.enable_download);

        assert!(
            Options::parse(
                vec![OsString::from("--offline=maybe")].into_iter(),
                &["offline"]
            )
            .is_err()
        );
        assert!(
            Options::parse(
                vec![
                    OsString::from("--enable-download=true"),
                    OsString::from("--help=x")
                ]
                .into_iter(),
                &["enable-download", "help"]
            )
            .is_err()
        );
    }
}
