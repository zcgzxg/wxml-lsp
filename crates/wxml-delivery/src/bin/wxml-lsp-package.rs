//! `wxml-lsp-package`: build a deterministic local release package.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wxml_delivery::manifest::{SUPPORTED_PROTOCOL_VERSIONS, now_rfc3339, rfc3339_from_unix};
use wxml_delivery::manifest_v2::{NATIVE_PACKAGE_PLATFORM, SUPPORTED_NATIVE_PROTOCOL_VERSIONS};
use wxml_delivery::package::{BuildOptions, BuildOptionsV2, build_native_package, build_package};
use wxml_delivery::paths::{host_platform, validate_platform, validate_version};
use wxml_delivery::semver::validate_strict;

const USAGE: &str = "\
Usage: wxml-lsp-package [native] --input DIR --output DIR [OPTIONS]

Without `native`, builds the legacy schema v1 Node package described below.
With `native`, builds the schema v2 Node-free package described by
`native --help`. The v2 platform is fixed to linux-x86_64.

Legacy schema v1 input tree:

  bin/wxml-lsp
  bin/wxml-type-backend/server.js
  bin/wxml-type-backend/node_modules/typescript/package.json
  bin/wxml-type-backend/node_modules/miniprogram-api-typings/package.json
  zed-extension/extension.wasm
  zed-extension/extension.toml

Options:
  --input DIR               staged package root (required)
  --output DIR              artifact directory (required; keep it outside the repository)
  --release-version V       package version (default: this build's version)
  --platform P              target platform (default: host)
  --protocol-version N      Rust/Node protocol version (default: 2)
  --node-minimum V          minimum Node.js version (required)
  --backend-version V       Node backend package version (default: release version)
  --zed-version V           Zed extension version (default: release version)
  --built-at TIME           RFC 3339 UTC build time (default: now)
  --mtime SECONDS           tar entry timestamp (default: 0)
  --force                   replace existing versioned output
  --dry-run                 validate and describe without writing
  --print-version           print this packager's version and exit
  --print-platform          print the host platform label and exit
  --help                    print this message

This command never uploads artifacts, creates tags/releases or contacts the
network. The generated index is a local file for a release directory or a
fixture server.";

const NATIVE_USAGE: &str = "\
Usage: wxml-lsp-package native --input DIR --output DIR [OPTIONS]

Builds a deterministic schema v2 package for linux-x86_64 from a prepared,
Node-free tree. Accepted payload (required unless marked optional):

  bin/wxml-lsp                        0755
  bin/wxml-lsp-install                0755
  engine/lib/tsc                      0755
  engine/lib/tsc.sig                  0644
  engine/lib/lib*.d.ts                0644 (116 files, including lib.d.ts)
  engine/LICENSE                      0644
  engine/NOTICE.txt                   0644
  engine/package.json                 0644 (pinned engine name/version)
  typings/miniprogram-api-typings/**  0644 (26 files, including LICENSE)
  zed-extension/extension.wasm        0644
  zed-extension/extension.toml        0644 (top-level version required)
  zed-extension/languages/**          0644 (optional)
  THIRD_PARTY_LICENSES.md             0644

Optional: engine/README.md, zed-extension/grammars/wxml.wasm,
zed-extension/languages/**.

`engine/lib/tsc` must match the pinned upstream SHA-256 and size. The packager
generates `provenance.json` (measured per-file facts plus explicitly labeled
pinned upstream expectations); the prepared tree must not supply it.

Options:
  --input DIR               prepared package root (required)
  --output DIR              artifact directory (required; keep it outside the repository)
  --release-version V       strict semantic version (default: this build's version)
  --platform P              fixed target platform (default and only value: linux-x86_64)
  --protocol-version N      Rust/engine protocol version (default: 2)
  --built-at TIME           RFC 3339 UTC build time (required unless SOURCE_DATE_EPOCH is set)
  --force                   replace an existing versioned directory/archive
  --dry-run                 validate and describe without writing
  --help                    print this message

The tar.gz uses mtime=0, uid/gid=0, empty uname/gname and 0644/0755 modes.
Two builds with identical inputs produce byte-identical archives. This command
never executes node/npm/npx/pnpm/yarn and never contacts the network.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        eprintln!("{USAGE}");
        return Err("missing arguments".to_owned());
    };
    if matches!(first.as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if first == "--print-version" {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if first == "--print-platform" {
        println!("{}", host_platform().map_err(|error| error.to_string())?);
        return Ok(());
    }
    if matches!(first.as_str(), "native" | "v2") {
        let Some(options) = parse_native_options(args)? else {
            return Ok(());
        };
        let dry_run = options.dry_run;
        let report = build_native_package(&options).map_err(|error| error.to_string())?;
        print_native_report(&report, &options.output, dry_run);
        return Ok(());
    }
    let Some(options) = parse_build_options(first, args)? else {
        return Ok(());
    };
    let dry_run = options.dry_run;
    let report = build_package(&options).map_err(|error| error.to_string())?;
    print_report(&report, dry_run);
    Ok(())
}

fn parse_build_options(
    first: String,
    mut args: impl Iterator<Item = String>,
) -> Result<Option<BuildOptions>, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut release_version = env!("CARGO_PKG_VERSION").to_owned();
    let mut platform = host_platform().map_err(|error| error.to_string())?;
    let mut protocol_version = *SUPPORTED_PROTOCOL_VERSIONS
        .first()
        .ok_or_else(|| "no supported protocol version".to_owned())?;
    let mut node_minimum: Option<String> = None;
    let mut backend_version: Option<String> = None;
    let mut zed_version: Option<String> = None;
    let mut built_at = now_rfc3339();
    let mut mtime = 0_u64;
    let mut force = false;
    let mut dry_run = false;

    let mut current = Some(first);
    while let Some(argument) = current {
        let mut next = || args.next();
        match argument.as_str() {
            "--input" => input = Some(take_path(&mut next, "--input")?),
            "--output" => output = Some(take_path(&mut next, "--output")?),
            "--release-version" => release_version = take_value(&mut next, "--release-version")?,
            "--platform" => platform = take_value(&mut next, "--platform")?,
            "--protocol-version" => {
                protocol_version = take_value(&mut next, "--protocol-version")?
                    .parse::<u64>()
                    .map_err(|_| "--protocol-version must be a number".to_owned())?;
            }
            "--node-minimum" => node_minimum = Some(take_value(&mut next, "--node-minimum")?),
            "--backend-version" => {
                backend_version = Some(take_value(&mut next, "--backend-version")?);
            }
            "--zed-version" => zed_version = Some(take_value(&mut next, "--zed-version")?),
            "--built-at" => built_at = take_value(&mut next, "--built-at")?,
            "--mtime" => {
                mtime = take_value(&mut next, "--mtime")?
                    .parse::<u64>()
                    .map_err(|_| "--mtime must be an integer".to_owned())?;
            }
            "--force" => force = true,
            "--dry-run" => dry_run = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
        current = args.next();
    }

    let input = input.ok_or_else(|| "--input is required".to_owned())?;
    let output = output.ok_or_else(|| "--output is required".to_owned())?;
    let node_minimum = node_minimum.ok_or_else(|| "--node-minimum is required".to_owned())?;
    let backend_version = backend_version.unwrap_or_else(|| release_version.clone());
    validate_version(&release_version).map_err(|error| error.to_string())?;
    validate_platform(&platform).map_err(|error| error.to_string())?;
    Ok(Some(BuildOptions {
        input,
        output,
        release_version,
        platform,
        protocol_version,
        node_minimum,
        backend_version,
        zed_version,
        built_at,
        mtime,
        force,
        dry_run,
    }))
}

fn print_report(report: &wxml_delivery::package::BuildReport, dry_run: bool) {
    println!(
        "package: {} {} ({})",
        report.manifest.package, report.manifest.package_version, report.manifest.platform
    );
    println!(
        "protocol: {}  node: >= {}  files: {}",
        report.manifest.protocol_version,
        report.manifest.node.minimum_version,
        report.manifest.files.len()
    );
    if dry_run {
        println!("dry-run: validated staged tree; no files written");
        return;
    }
    if let Some(directory) = report.release_dir.as_ref() {
        println!("release directory: {}", directory.display());
    }
    if let Some(archive) = report.archive.as_ref() {
        println!("archive: {}", archive.display());
        println!("sha256: {}", report.sha256);
        println!("size: {} bytes", report.size);
    }
}

fn parse_native_options(
    mut args: impl Iterator<Item = String>,
) -> Result<Option<BuildOptionsV2>, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut release_version = env!("CARGO_PKG_VERSION").to_owned();
    let mut platform = NATIVE_PACKAGE_PLATFORM.to_owned();
    let mut protocol_version = *SUPPORTED_NATIVE_PROTOCOL_VERSIONS
        .first()
        .ok_or_else(|| "no supported native protocol version".to_owned())?;
    let mut built_at: Option<String> = None;
    let mut force = false;
    let mut dry_run = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--input" => input = Some(PathBuf::from(take_next(&mut args, "--input")?)),
            "--output" => output = Some(PathBuf::from(take_next(&mut args, "--output")?)),
            "--release-version" => {
                release_version = take_next(&mut args, "--release-version")?;
            }
            "--platform" => platform = take_next(&mut args, "--platform")?,
            "--protocol-version" => {
                protocol_version = take_next(&mut args, "--protocol-version")?
                    .parse::<u64>()
                    .map_err(|_| "--protocol-version must be a number".to_owned())?;
            }
            "--built-at" => built_at = Some(take_next(&mut args, "--built-at")?),
            "--force" => force = true,
            "--dry-run" => dry_run = true,
            "--help" | "-h" => {
                println!("{NATIVE_USAGE}");
                return Ok(None);
            }
            other => return Err(format!("unexpected argument `{other}`\n\n{NATIVE_USAGE}")),
        }
    }

    let input = input.ok_or_else(|| "--input is required".to_owned())?;
    let output = output.ok_or_else(|| "--output is required".to_owned())?;
    if platform != NATIVE_PACKAGE_PLATFORM {
        return Err(format!(
            "native packages support only platform `{NATIVE_PACKAGE_PLATFORM}`"
        ));
    }
    validate_strict(&release_version).map_err(|error| error.to_string())?;
    let built_at = native_built_at(built_at)?;
    Ok(Some(BuildOptionsV2 {
        input,
        output,
        release_version,
        protocol_version,
        built_at,
        force,
        dry_run,
    }))
}

fn native_built_at(explicit: Option<String>) -> Result<String, String> {
    if let Some(value) = explicit {
        return Ok(value);
    }
    match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => {
            let seconds = value
                .parse::<u64>()
                .map_err(|_| "SOURCE_DATE_EPOCH must be an integer number of seconds".to_owned())?;
            Ok(rfc3339_from_unix(seconds))
        }
        Err(_) => Err(
            "--built-at is required for deterministic native builds (or set SOURCE_DATE_EPOCH)"
                .to_owned(),
        ),
    }
}

fn print_native_report(
    report: &wxml_delivery::package::BuildReportV2,
    output: &Path,
    dry_run: bool,
) {
    println!(
        "native package: {} {} ({})",
        report.manifest.package, report.manifest.package_version, report.manifest.platform
    );
    println!(
        "engine: {} {} gitHead {}",
        report.manifest.engine.name,
        report.manifest.engine.version,
        report.manifest.engine.source_commit
    );
    if let Some(binary) = report.manifest.file(&report.manifest.engine.binary) {
        println!(
            "engine binary (measured): sha256 {} size {} mode {}",
            binary.sha256, binary.size, binary.mode
        );
    }
    println!(
        "engine distribution (pinned expected): sha256 {} size {} files {} unpacked {}",
        report.manifest.engine.distribution_sha256,
        report.manifest.engine.distribution_size,
        report.manifest.engine.distribution_file_count,
        report.manifest.engine.distribution_unpacked_size
    );
    println!(
        "protocol: {}  files: {}",
        report.manifest.protocol_version,
        report.manifest.files.len()
    );
    if dry_run {
        println!("dry-run: validated prepared tree; no files written");
        return;
    }
    if let Some(directory) = report.release_dir.as_ref() {
        println!("release directory: {}", directory.display());
    }
    if let Some(archive) = report.archive.as_ref() {
        println!("archive: {}", archive.display());
        println!("sha256: {}", report.sha256);
        println!("archive integrity: {}", report.archive_integrity);
        println!("size: {} bytes", report.size);
    }
    println!("index: {}", output.join("index.json").display());
}

fn take_next(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn take_value(next: &mut impl FnMut() -> Option<String>, option: &str) -> Result<String, String> {
    next().ok_or_else(|| format!("{option} requires a value"))
}

fn take_path(next: &mut impl FnMut() -> Option<String>, option: &str) -> Result<PathBuf, String> {
    take_value(next, option).map(PathBuf::from)
}
