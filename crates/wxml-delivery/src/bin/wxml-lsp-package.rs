//! `wxml-lsp-package`: build a deterministic local release package.

use std::path::PathBuf;
use std::process::ExitCode;

use wxml_delivery::manifest::{SUPPORTED_PROTOCOL_VERSIONS, now_rfc3339};
use wxml_delivery::package::{BuildOptions, build_package};
use wxml_delivery::paths::{host_platform, validate_platform, validate_version};

const USAGE: &str = "\
Usage: wxml-lsp-package --input DIR --output DIR [OPTIONS]

Builds a deterministic versioned directory, tar.gz archive, SHA256SUMS,
per-file checksums and index.json from a staged package tree. The input tree
must contain:

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

fn take_value(next: &mut impl FnMut() -> Option<String>, option: &str) -> Result<String, String> {
    next().ok_or_else(|| format!("{option} requires a value"))
}

fn take_path(next: &mut impl FnMut() -> Option<String>, option: &str) -> Result<PathBuf, String> {
    take_value(next, option).map(PathBuf::from)
}
