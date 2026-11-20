//! `wxml-lsp-package`: build a deterministic node-free release package,
//! generate its third-party license inventory, or provision locked assets.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wxml_delivery::licenses::{LicenseRequest, MetadataRoot, generate};
use wxml_delivery::manifest::{atomic_write, rfc3339_from_unix};
use wxml_delivery::manifest_v2::{NATIVE_PACKAGE_PLATFORM, SUPPORTED_NATIVE_PROTOCOL_VERSIONS};
use wxml_delivery::package::{BuildOptionsV2, build_native_package};
use wxml_delivery::semver::validate_strict;

const USAGE: &str = "\
Usage: wxml-lsp-package <COMMAND> [OPTIONS]

Commands:
  native       build a schema v2 native package (`native --help`)
  licenses     generate a deterministic third-party license inventory
               (`licenses --help`)
  provision    provision the assets declared by `assets.lock.json`

Options:
  --print-version  print this packager's version and exit
  --help           print this message

Schema v1 (legacy Node) packages are no longer built by this tool. Release
artifacts are schema v2 only and never require Node.

This command never uploads artifacts, creates tags/releases or contacts the
network. The generated index is a local file for a release directory or a
fixture server.";

const PROVISION_USAGE: &str = "\
Usage: wxml-lsp-package provision --lock FILE --cache DIR [OPTIONS]

Provisions the assets declared by a repository `assets.lock.json` into an
immutable, content-addressed cache. This build never downloads:

  --lock FILE               repository assets.lock.json (required)
  --cache DIR               absolute cache directory (required; 0700 on create)
  --from-dir DIR            absolute directory containing the exact locked
                            tarball file names
  --offline                 default behavior: nothing is downloaded; cached
                            verified tarballs may be reused without --from-dir
  --print-plan              print cache keys, payload/archive hits and sources;
                            write nothing
  --allow-download          rejected: the HTTPS download path is not implemented
  --help                    print this message

Provisioning stores every verified tarball in a content-addressed archive cache
(`archives/<asset>/<sha256>-<size>`), so a later run can rebuild the payload
after the source tarballs are deleted. Every payload and archive hit is
re-hashed against the submitted lock (size, SHA-256 and SHA-512 SRI); corrupt or
foreign entries fail closed and are never repaired in place. This command never
downloads and never executes node/npm/npx/pnpm/yarn.";

const LICENSES_USAGE: &str = "\
Usage: wxml-lsp-package licenses --output FILE --source-commit SHA [OPTIONS]

Generates the deterministic `THIRD_PARTY_LICENSES.md` inventory from one or
more saved `cargo metadata --format-version 1 --locked --offline` documents:

  --output FILE             destination Markdown file (required)
  --metadata FILE           cargo metadata JSON document (required; repeat for
                            the root workspace and `editors/zed`)
  --package NAME            root package to traverse from normal dependency
                            edges (repeatable; applies to the last --metadata)
  --label LABEL             workspace label for the last --metadata
  --target TRIPLE           Cargo target triple for the last --metadata
  --source-commit SHA       hexadecimal source commit id (required)
  --source-date-epoch N     reproducible timestamp in Unix seconds (default:
                            $SOURCE_DATE_EPOCH; required when both are absent)
  --allow-unlicensed        accept crates with no license expression and no
                            local license text; local-only, never publishable
  --help                    print this message

License texts are read from each package's `manifest_path` directory. Crates
that ship no license file fall back to canonical SPDX texts vendored with this
tool and selected by the recorded license expression; the inventory records the
text source as `local` or `spdx`, and prints a `text-coverage` marker. Crates
whose text is absent from both are listed explicitly; they are never silently
omitted. A crate with neither a license expression nor a text fails generation
unless --allow-unlicensed is passed. Output ordering and the only timestamp
come from the inputs and SOURCE_DATE_EPOCH, so two runs with the same metadata
and epoch are byte-identical.";

const NATIVE_USAGE: &str = "\
Usage: wxml-lsp-package native --input DIR --output DIR --assets-lock FILE --cache DIR [OPTIONS]

Builds a deterministic schema v2 package for linux-x86_64 from a prepared,
Node-free input tree plus a provisioned asset cache. `engine/` and `typings/`
must come from the cache and must match the submitted lock byte for byte; the
input tree must not contain either root. Accepted input payload:

  bin/wxml-lsp                        0755
  bin/wxml-lsp-install                0755
  zed-extension/extension.wasm        0644
  zed-extension/extension.toml        0644 (top-level version required)
  zed-extension/languages/**          0644 (optional)
  THIRD_PARTY_LICENSES.md             0644

Engine and typings files are taken from the cache entry for
`<payload.treeDigest>.<lockSha256[..16]>`. `engine/lib/tsc` must match the
pinned upstream SHA-256 and size. The packager never downloads: a missing or
stale cache fails closed.

The packager generates `provenance.json` (measured per-file facts, the lock
identity, per-asset payload tree digests, `archivePayloadDigest` and, when
supplied, `sourceCommit`/`sourceDateEpoch`); the prepared tree must not supply
it.

Options:
  --input DIR               prepared input root (required)
  --output DIR              artifact directory (required; keep it outside the repository)
  --assets-lock FILE        repository assets.lock.json (required)
  --cache DIR               provisioned, content-addressed asset cache (required)
  --release-version V       strict semantic version (default: this build's version)
  --platform P              fixed target platform (default and only value: linux-x86_64)
  --protocol-version N      Rust/engine protocol version (default: 2)
  --built-at TIME           RFC 3339 UTC build time (required unless SOURCE_DATE_EPOCH is set)
  --source-commit SHA       hexadecimal source commit recorded in provenance.json
  --source-date-epoch N     SOURCE_DATE_EPOCH recorded in provenance.json
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
    if first == "provision" {
        let Some(options) = parse_provision_options(args)? else {
            return Ok(());
        };
        if options.print_plan {
            let plan =
                wxml_delivery::provision::plan(&options).map_err(|error| error.to_string())?;
            print_provision_plan(&plan);
        } else {
            let report =
                wxml_delivery::provision::provision(&options).map_err(|error| error.to_string())?;
            print_provision_report(&report);
        }
        return Ok(());
    }
    if first == "licenses" {
        let Some((output, request)) = parse_licenses_options(args)? else {
            return Ok(());
        };
        let (bytes, summary) = generate(&request).map_err(|error| error.to_string())?;
        atomic_write(&output, &bytes, 0o644).map_err(|error| error.to_string())?;
        println!(
            "licenses: {} third-party crates ({} with text: {} local, {} vendored SPDX; {} without text, {} unlicensed), {} distinct texts",
            summary.third_party_crates,
            summary.crates_with_text,
            summary.crates_with_local_text,
            summary.crates_with_spdx_text,
            summary.crates_without_text,
            summary.unlicensed_crates,
            summary.distinct_texts
        );
        if summary.first_party_blocked {
            println!("first-party-license: BLOCKED owner decision");
        }
        println!("output: {}", output.display());
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
    Err(format!(
        "unknown command `{first}`: schema v1 packaging was removed; use `native` or `provision`\n\n{USAGE}"
    ))
}

fn parse_provision_options(
    mut args: impl Iterator<Item = String>,
) -> Result<Option<wxml_delivery::provision::ProvisionOptions>, String> {
    use wxml_delivery::provision::ProvisionOptions;

    let mut lock: Option<PathBuf> = None;
    let mut cache: Option<PathBuf> = None;
    let mut from_dir: Option<PathBuf> = None;
    let mut offline = false;
    let mut print_plan = false;
    let mut allow_download = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--lock" => lock = Some(PathBuf::from(take_next(&mut args, "--lock")?)),
            "--cache" => cache = Some(PathBuf::from(take_next(&mut args, "--cache")?)),
            "--from-dir" => {
                from_dir = Some(PathBuf::from(take_next(&mut args, "--from-dir")?));
            }
            "--offline" => {
                offline = true;
            }
            "--print-plan" => print_plan = true,
            "--allow-download" => allow_download = true,
            "--help" | "-h" => {
                println!("{PROVISION_USAGE}");
                return Ok(None);
            }
            other => {
                return Err(format!(
                    "unexpected argument `{other}`\n\n{PROVISION_USAGE}"
                ));
            }
        }
    }
    let lock = lock.ok_or_else(|| "--lock is required".to_owned())?;
    let cache = cache.ok_or_else(|| "--cache is required".to_owned())?;
    Ok(Some(ProvisionOptions {
        lock,
        cache,
        from_dir,
        offline,
        print_plan,
        allow_download,
    }))
}

fn print_provision_plan(plan: &wxml_delivery::provision::ProvisionPlan) {
    println!("provision plan: lock sha256 {}", plan.lock_sha256);
    for asset in &plan.assets {
        let source = asset.source.map_or("missing", |source| source.label());
        println!(
            "asset: {}  cache-key {}  cache {}  source {}  tarball {}",
            asset.name,
            asset.cache_key,
            if asset.cache_hit { "hit" } else { "miss" },
            source,
            asset.tarball_file_name
        );
    }
    println!("dry-run: no cache entries written");
}

fn print_provision_report(report: &wxml_delivery::provision::ProvisionReport) {
    println!("provisioned: lock sha256 {}", report.lock_sha256);
    for entry in &report.entries {
        println!(
            "asset: {}  source {}  cache-key {}",
            entry.name,
            entry.source.label(),
            entry.cache_key
        );
        println!(
            "payload tree: {}  files {}  bytes {}",
            entry.payload_tree_digest, entry.file_count, entry.total_bytes
        );
    }
}

fn parse_licenses_options(
    mut args: impl Iterator<Item = String>,
) -> Result<Option<(PathBuf, LicenseRequest)>, String> {
    let mut output: Option<PathBuf> = None;
    let mut roots: Vec<MetadataRoot> = Vec::new();
    let mut source_commit: Option<String> = None;
    let mut source_date_epoch: Option<u64> = None;
    let mut allow_unlicensed = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--output" => output = Some(PathBuf::from(take_next(&mut args, "--output")?)),
            "--metadata" => roots.push(MetadataRoot {
                metadata: PathBuf::from(take_next(&mut args, "--metadata")?),
                label: String::new(),
                target: String::new(),
                packages: Vec::new(),
            }),
            "--package" => {
                let name = take_next(&mut args, "--package")?;
                let root = roots.last_mut().ok_or_else(|| {
                    "--package must follow the --metadata it belongs to".to_owned()
                })?;
                root.packages.push(name);
            }
            "--label" => {
                let label = take_next(&mut args, "--label")?;
                let root = roots
                    .last_mut()
                    .ok_or_else(|| "--label must follow the --metadata it belongs to".to_owned())?;
                root.label = label;
            }
            "--target" => {
                let target = take_next(&mut args, "--target")?;
                let root = roots.last_mut().ok_or_else(|| {
                    "--target must follow the --metadata it belongs to".to_owned()
                })?;
                root.target = target;
            }
            "--source-commit" => source_commit = Some(take_next(&mut args, "--source-commit")?),
            "--source-date-epoch" => {
                source_date_epoch = Some(
                    take_next(&mut args, "--source-date-epoch")?
                        .parse::<u64>()
                        .map_err(|_| "--source-date-epoch must be a number".to_owned())?,
                );
            }
            "--allow-unlicensed" => allow_unlicensed = true,
            "--help" | "-h" => {
                println!("{LICENSES_USAGE}");
                return Ok(None);
            }
            other => return Err(format!("unexpected argument `{other}`\n\n{LICENSES_USAGE}")),
        }
    }
    let output = output.ok_or_else(|| "--output is required".to_owned())?;
    let source_commit = source_commit.ok_or_else(|| "--source-commit is required".to_owned())?;
    let source_date_epoch = match source_date_epoch {
        Some(epoch) => epoch,
        None => std::env::var("SOURCE_DATE_EPOCH")
            .map_err(|_| {
                "--source-date-epoch is required for deterministic output (or set SOURCE_DATE_EPOCH)"
                    .to_owned()
            })?
            .parse::<u64>()
            .map_err(|_| "SOURCE_DATE_EPOCH must be an integer number of seconds".to_owned())?,
    };
    for (index, root) in roots.iter_mut().enumerate() {
        if root.label.is_empty() {
            root.label = format!("workspace {index}");
        }
        if root.target.is_empty() {
            return Err("--target is required for every --metadata".to_owned());
        }
    }
    Ok(Some((
        output,
        LicenseRequest {
            roots,
            source_commit,
            source_date_epoch,
            allow_unlicensed,
        },
    )))
}

fn parse_native_options(
    mut args: impl Iterator<Item = String>,
) -> Result<Option<BuildOptionsV2>, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut assets_lock: Option<PathBuf> = None;
    let mut assets_cache: Option<PathBuf> = None;
    let mut release_version = env!("CARGO_PKG_VERSION").to_owned();
    let mut platform = NATIVE_PACKAGE_PLATFORM.to_owned();
    let mut protocol_version = *SUPPORTED_NATIVE_PROTOCOL_VERSIONS
        .first()
        .ok_or_else(|| "no supported native protocol version".to_owned())?;
    let mut built_at: Option<String> = None;
    let mut source_commit: Option<String> = None;
    let mut source_date_epoch: Option<u64> = None;
    let mut force = false;
    let mut dry_run = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--input" => input = Some(PathBuf::from(take_next(&mut args, "--input")?)),
            "--output" => output = Some(PathBuf::from(take_next(&mut args, "--output")?)),
            "--assets-lock" => {
                assets_lock = Some(PathBuf::from(take_next(&mut args, "--assets-lock")?));
            }
            "--cache" => {
                assets_cache = Some(PathBuf::from(take_next(&mut args, "--cache")?));
            }
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
            "--source-commit" => source_commit = Some(take_next(&mut args, "--source-commit")?),
            "--source-date-epoch" => {
                source_date_epoch = Some(
                    take_next(&mut args, "--source-date-epoch")?
                        .parse::<u64>()
                        .map_err(|_| "--source-date-epoch must be a number".to_owned())?,
                );
            }
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
    if source_date_epoch.is_none() {
        source_date_epoch = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|value| value.parse::<u64>().ok());
    }
    let assets_lock = assets_lock
        .ok_or_else(|| "native builds require --assets-lock <repo>/assets.lock.json".to_owned())?;
    let assets_cache = assets_cache
        .ok_or_else(|| "native builds require --cache <provisioned-asset-cache>".to_owned())?;
    Ok(Some(BuildOptionsV2 {
        input,
        output,
        assets_lock: Some(assets_lock),
        assets_cache: Some(assets_cache),
        release_version,
        protocol_version,
        built_at,
        source_commit,
        source_date_epoch,
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
