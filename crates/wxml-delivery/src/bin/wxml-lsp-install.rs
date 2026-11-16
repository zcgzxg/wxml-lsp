//! `wxml-lsp-install`: verified local installer, upgrade and rollback.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wxml_delivery::acquire::{AcquireMode, AcquireOptions, acquire};
use wxml_delivery::error::{DeliveryError, Result};
use wxml_delivery::install::{BINARY_ENV, InstallOutcome, InstallRoot, explicit_binary};
use wxml_delivery::manifest::{ExpectedManifest, now_rfc3339};
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
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return Err(DeliveryError::InvalidArgument(
            "a command is required".to_owned(),
        ));
    };
    if matches!(command.as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if matches!(command.as_str(), "--version" | "-V") {
        println!("wxml-lsp-install {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = Options::parse(args, &allowed_for(&command)?)?;
    if options.help {
        println!("{USAGE}");
        return Ok(());
    }
    match command.as_str() {
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
    fn parse(mut args: impl Iterator<Item = String>, allowed: &[&str]) -> Result<Self> {
        let mut options = Self::default();
        while let Some(argument) = args.next() {
            let name = argument.strip_prefix("--").ok_or_else(|| {
                DeliveryError::InvalidArgument(format!("unexpected argument `{argument}`"))
            })?;
            let (name, inline) = name
                .split_once('=')
                .map_or((name, None), |(name, value)| (name, Some(value.to_owned())));
            if !allowed.contains(&name) {
                return Err(DeliveryError::InvalidArgument(format!(
                    "unexpected option `--{name}`"
                )));
            }
            let mut value = |option: &str| -> Result<String> {
                if let Some(value) = inline.clone() {
                    return Ok(value);
                }
                args.next().ok_or_else(|| {
                    DeliveryError::InvalidArgument(format!("--{option} requires a value"))
                })
            };
            match name {
                "root" => options.root = Some(PathBuf::from(value("root")?)),
                "version" => options.version = Some(value("version")?),
                "to" => options.to = Some(value("to")?),
                "source" => options.source = Some(value("source")?),
                "from-dir" => options.from_dir = Some(PathBuf::from(value("from-dir")?)),
                "enable-download" => options.enable_download = true,
                "offline" => options.offline = true,
                "help" => options.help = true,
                _ => unreachable!("allowed options are exhaustive"),
            }
        }
        Ok(options)
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
    let root = root(options)?;
    let _lock = root.lock()?;
    let acquired = acquire(&AcquireOptions {
        root: &root,
        source: source.as_ref(),
        mode: AcquireMode::Exact(version.clone()),
        allow_download: options.enable_download,
        offline: options.offline,
    })?;
    let result = root.install_tree(&acquired.tree, &version);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&acquired.tree);
    }
    let outcome = result?;
    print_install_result(&root, &version, outcome, acquired.cache_hit)
}

fn upgrade(options: &Options) -> Result<()> {
    if let Some(path) = skip_because_explicit()? {
        println!("{BINARY_ENV} is set; it takes precedence. No upgrade or download was attempted.");
        println!("binary: {}", path.display());
        return Ok(());
    }
    let source = source(options)?;
    let root = root(options)?;
    let _lock = root.lock()?;
    let acquired = acquire(&AcquireOptions {
        root: &root,
        source: source.as_ref(),
        mode: AcquireMode::Latest,
        allow_download: options.enable_download,
        offline: options.offline,
    })?;
    let version = acquired.manifest.package_version.clone();
    let result = root.install_tree(&acquired.tree, &version);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&acquired.tree);
    }
    let outcome = result?;
    print_install_result(&root, &version, outcome, acquired.cache_hit)
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
    let root = root(options)?;
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
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&acquired.tree);
        }
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
    let root = root(options)?;
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
    let root = root(options)?;
    let _lock = root.lock()?;
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
    let root = root(options)?;
    let _lock = root.lock()?;
    let version = options
        .version
        .clone()
        .or(root.current_version()?)
        .ok_or_else(|| DeliveryError::InstallState("no installed version is active".to_owned()))?;
    let directory = root.versions_dir().join(&version);
    let platform = host_platform()?;
    let manifest = wxml_delivery::manifest::read_manifest(
        &directory,
        ExpectedManifest {
            version: Some(&version),
            platform: Some(&platform),
            protocol_version: None,
        },
    )?;
    let binary = check_binary(&directory, &manifest)?;
    check_backend(&directory, &manifest)?;
    let node = check_node(&manifest.node.minimum_version)?;
    root.verify_installed(&version)?;
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
    let root = root(options)?;
    let _lock = root.lock()?;
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

fn root(options: &Options) -> Result<InstallRoot> {
    InstallRoot::create(&options.root.clone().map_or_else(default_root, Ok)?)
}

fn default_root() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg).join("wxml-lsp"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        DeliveryError::InvalidArgument(
            "HOME is not set; pass --root to choose an install directory".to_owned(),
        )
    })?;
    Ok(PathBuf::from(home).join(".local/share/wxml-lsp"))
}

fn source(options: &Options) -> Result<Option<Source>> {
    match (&options.source, &options.from_dir) {
        (Some(_), Some(_)) => Err(DeliveryError::InvalidArgument(
            "use either --source or --from-dir, not both".to_owned(),
        )),
        (Some(source), None) => Source::remote(source).map(Some),
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
    Ok(directory)
}
