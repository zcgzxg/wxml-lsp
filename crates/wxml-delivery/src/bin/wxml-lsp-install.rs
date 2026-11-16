//! `wxml-lsp-install`: verified local installer, upgrade and rollback.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wxml_delivery::acquire::{AcquireMode, AcquireOptions, acquire};
use wxml_delivery::error::{DeliveryError, Result};
use wxml_delivery::install::{BINARY_ENV, InstallOutcome, InstallRoot, explicit_binary};
use wxml_delivery::manifest::{ExpectedManifest, now_rfc3339, set_mode};
use wxml_delivery::paths::{host_platform, shell_quote};
use wxml_delivery::semver::compare_versions;
use wxml_delivery::source::Source;
use wxml_delivery::verify::{binary_version, check_backend, check_binary, check_node};

const USAGE: &str = "\
Usage: wxml-lsp-install <COMMAND> [OPTIONS]

Commands:
  install        Install an exact version (requires --version)
  upgrade        Install the newest release from the source
  rollback       Switch to a previously installed verified version (--to)
  versions       List versions available from a source
  list           List installed versions
  current        Print the active binary path and reported version
  verify         Fully verify the active installation and Node.js
  env            Print WXML_LSP_BIN/WXML_TYPE_BACKEND exports
  verify-package Verify a package archive without installing it

Options:
  --root DIR          Install root (default: $XDG_DATA_HOME/wxml-lsp or
                      $HOME/.local/share/wxml-lsp)
  --version V         Exact release version (install/verify-package/verify)
  --to V              Target version (rollback)
  --source URL        Release base URL; HTTPS, or loopback HTTP for fixtures
  --from-dir DIR      Local release output directory
  --enable-download   Explicitly allow contact with a remote source
  --offline           Use only verified cache/local releases; never network
  --help              Print this message

Download is disabled unless --enable-download is passed with a remote source.
A local --from-dir is an explicit offline source. WXML_LSP_BIN always takes
precedence: when set it is validated and used, and no install or download is
attempted; when set but invalid the command fails and never falls back.

Read-only commands (list/current/verify/env) never create or chmod the install
root. `verify` hashes the complete tree before executing its packaged binary.
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
        "list" | "current" | "env" => vec!["root", "help"],
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
                _ => unreachable!("allowed options are exhaustive"),
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
    let root = root_mut(options)?;
    let _lock = root.lock()?;
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
    let root = root_mut(options)?;
    let _lock = root.lock()?;
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
            let _ = std::fs::remove_dir_all(&acquired.tree);
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
    let _lock = root.lock()?;
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
    root.verify_installed(&target)?;
    root.switch(&target)?;
    let binary = root.current_binary()?;
    let version = binary_version(&binary)?;
    println!("rolled back to {target}");
    println!("binary: {}", binary.display());
    println!("reported version: {version}");
    Ok(())
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
    let index = source.read_index()?;
    let platform = host_platform()?;
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
        println!("{marker} {version}");
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
    let version = root
        .current_version()?
        .ok_or_else(|| DeliveryError::InstallState("no installed version is active".to_owned()))?;
    let directory = root.versions_dir().join(&version);
    let manifest = root.verify_installed(&version)?;
    let reported = check_binary(&directory, &manifest)?;
    check_backend(&directory, &manifest)?;
    check_node(&manifest.node.minimum_version)?;
    println!("binary: {}", root.current_binary()?.display());
    println!("version: {reported}");
    println!("manifest protocol: {}", manifest.protocol_version);
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
    let version = options
        .version
        .clone()
        .or(root.current_version()?)
        .ok_or_else(|| DeliveryError::InstallState("no installed version is active".to_owned()))?;
    let directory = root.versions_dir().join(&version);
    // Hash-verify the whole tree before executing the packaged binary. A
    // tampered `bin/wxml-lsp` must never run just because the user asked to
    // verify it.
    let manifest = root.verify_installed(&version)?;
    let binary = check_binary(&directory, &manifest)?;
    check_backend(&directory, &manifest)?;
    let node = check_node(&manifest.node.minimum_version)?;
    println!("verified {version}");
    println!("binary version: {binary}");
    println!("node: {node} (minimum {})", manifest.node.minimum_version);
    println!("protocol: {}", manifest.protocol_version);
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
    let version = root
        .current_version()?
        .ok_or_else(|| DeliveryError::InstallState("no installed version is active".to_owned()))?;
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
    let index = source.read_index()?;
    let platform = host_platform()?;
    let release = index.find(&version, &platform)?;
    let scratch = scratch_dir()?;
    let result = (|| {
        let archive = scratch.join("archive.tar.gz");
        source.fetch_archive(release, &archive)?;
        wxml_delivery::verify::verify_archive(
            &archive,
            &scratch.join("verify"),
            ExpectedManifest {
                version: Some(&release.version),
                platform: Some(&platform),
                protocol_version: Some(release.protocol_version),
            },
        )
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    let manifest = result?;
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
                let _ = root.switch(&previous);
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
                eprintln!("warning: remote source is ignored in offline mode");
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
    let directory = base.join(format!(
        "wxml-verify-{}-{}",
        std::process::id(),
        now_rfc3339().replace([':', '-'], "")
    ));
    std::fs::create_dir(&directory).map_err(|error| DeliveryError::io(&directory, error))?;
    set_mode(&directory, 0o700)?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

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
